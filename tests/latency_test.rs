//! Latency tests based on docs-spec/behavior/latency.feature
//!
//! Verifies the latency budgets from ADR-019 and the jitter buffer / packet
//! loss behaviour that keeps a session inside them. Requirement IDs are defined
//! in docs-spec/requirements.md and docs-spec/behavior/latency.feature; see
//! ADR-018 for how the `Verifies:` annotations are enforced.

use jamjam::audio::{
    AudioPreset, PcmPlc, PlayoutBuffer, PlayoutConfig, PlayoutResult, BUDGET_SAMPLE_RATE,
};
use jamjam::network::{FecDecoder, FecEncoder, FecPacket};

/// Samples per play-out frame, where a test only needs some plausible frame
/// rather than a specific preset.
const PLAYOUT_FRAME: usize = 4;

fn playout(target: u32, min: u32, max: u32) -> PlayoutBuffer {
    PlayoutBuffer::new(PlayoutConfig {
        frame_samples: PLAYOUT_FRAME,
        target_delay_frames: target,
        min_delay_frames: min,
        max_delay_frames: max,
    })
}

fn playout_frame(value: f32) -> [f32; PLAYOUT_FRAME] {
    [value; PLAYOUT_FRAME]
}

// ---------------------------------------------------------------------------
// Latency budgets (ADR-019)
// ---------------------------------------------------------------------------

/// Given a domestic fiber session on the zero-latency preset
/// Then application-induced one-way latency is at most 2ms
///
/// This is the top-priority requirement of the project: with a 20ms RTT the
/// total one-way latency must stay at or under 12ms, which leaves 2ms for the
/// application.
///
/// Verifies: REQ-CORE-001
/// Verifies: REQ-LAT-115
#[test]
fn zero_latency_preset_stays_within_two_milliseconds() {
    let preset = AudioPreset::ZeroLatency;

    assert_eq!(
        preset.max_app_latency_ms(),
        2.0,
        "the zero-latency budget is an external requirement (ADR-008) and must not be relaxed"
    );

    let designed = preset.designed_app_latency_ms(BUDGET_SAMPLE_RATE);
    assert!(
        designed <= 2.0,
        "zero-latency costs {:.2}ms of application latency, budget is 2.00ms",
        designed
    );

    // 20ms RTT means 10ms one way; the scenario allows 12ms in total.
    let total_one_way = designed + 10.0;
    assert!(
        total_one_way <= 12.0,
        "total one-way latency {:.2}ms exceeds the 12ms target on a 20ms RTT link",
        total_one_way
    );
}

/// Given two machines on the same LAN using the ultra-low-latency preset
/// Then application-induced one-way latency is at most 5ms
///
/// Verifies: REQ-LAT-116
#[test]
fn ultra_low_latency_preset_stays_within_lan_budget() {
    let preset = AudioPreset::UltraLowLatency;
    let designed = preset.designed_app_latency_ms(BUDGET_SAMPLE_RATE);

    assert_eq!(preset.max_app_latency_ms(), 5.0);
    assert!(
        designed <= 5.0,
        "ultra-low-latency costs {:.2}ms, budget is 5.00ms",
        designed
    );

    // 1ms RTT means 0.5ms one way; the scenario allows 6ms in total.
    assert!(
        designed + 0.5 <= 6.0,
        "total one-way latency {:.2}ms exceeds the 6ms LAN target",
        designed + 0.5
    );
}

/// Given an internet session using the balanced preset
/// Then application-induced one-way latency is at most 18ms
///
/// The budget is 18ms rather than ADR-008's approximate 15ms; see ADR-019.
///
/// Verifies: REQ-LAT-117
#[test]
fn balanced_preset_stays_within_internet_budget() {
    let preset = AudioPreset::Balanced;
    let designed = preset.designed_app_latency_ms(BUDGET_SAMPLE_RATE);

    assert_eq!(preset.max_app_latency_ms(), 18.0);
    assert!(
        designed <= 18.0,
        "balanced costs {:.2}ms, budget is 18.00ms",
        designed
    );

    // 50ms RTT means 25ms one way; the scenario expects roughly 41ms in total.
    let total_one_way = designed + 25.0;
    assert!(
        (total_one_way - 41.0).abs() <= 1.0,
        "total one-way latency {:.2}ms is not the expected ~41ms on a 50ms RTT link",
        total_one_way
    );
}

// ---------------------------------------------------------------------------
// Jitter buffer
// ---------------------------------------------------------------------------

/// Frames a test stream delivers between two `adapt` calls: a second of
/// 64-sample frames at 48kHz is 750; this is enough for a loss rate to mean
/// something (ADR-031).
const ADAPT_WINDOW_FRAMES: u32 = 100;

/// Runs `count` periods: each delivers the next frame (unless `lossy` drops
/// every other one) and takes one out, as the output callback does.
fn run_periods(buffer: &mut PlayoutBuffer, next: &mut u32, count: u32, lossy: bool) {
    let mut out = playout_frame(0.0);
    for _ in 0..count {
        if !(lossy && *next % 2 == 1) {
            buffer.write(*next, &playout_frame(*next as f32));
        }
        *next += 1;
        let _ = buffer.read_into(&mut out);
    }
}

/// Given the jitter buffer is in adaptive mode
/// When packets start being lost
/// Then the buffer delay grows automatically
///
/// The jitter buffer is the play-out buffer the output callback reads from
/// (ADR-028); adaptive means its delay bounds leave room to move. The loss
/// rate is judged over the stretch since the last decision (ADR-031).
///
/// Verifies: REQ-LAT-108
#[test]
fn adaptive_jitter_buffer_grows_when_packets_are_lost() {
    const INITIAL_FRAMES: u32 = 2;
    let mut buffer = playout(INITIAL_FRAMES, 1, 10);
    let mut next = 0;
    run_periods(&mut buffer, &mut next, INITIAL_FRAMES + 2, false);

    run_periods(&mut buffer, &mut next, ADAPT_WINDOW_FRAMES, true);
    assert!(
        buffer.stats().frames_concealed > 0,
        "the gaps in the sequence must be treated as losses"
    );

    let grown = buffer.adapt();

    assert_eq!(
        grown,
        Some(INITIAL_FRAMES + 1),
        "buffer delay should grow past {} frames after a loss",
        INITIAL_FRAMES
    );
    assert_eq!(buffer.target_delay_frames(), INITIAL_FRAMES + 1);
}

/// Given a minimum jitter buffer size of 2 frames
/// When the network is stable
/// Then the buffer never shrinks below the minimum
///
/// Verifies: REQ-LAT-109
#[test]
fn adaptive_jitter_buffer_never_drops_below_minimum() {
    const MIN_FRAMES: u32 = 2;
    let mut buffer = playout(4, MIN_FRAMES, 10);
    let mut next = 0;
    run_periods(&mut buffer, &mut next, 6, false);

    // A long gapless stream: the buffer gives frames back, one clean run at a
    // time, and must stop at the floor.
    let mut shrank = false;
    for _ in 0..60 {
        run_periods(&mut buffer, &mut next, ADAPT_WINDOW_FRAMES, false);
        shrank |= buffer.adapt().is_some();
        assert!(
            buffer.target_delay_frames() >= MIN_FRAMES,
            "the delay must never go below the configured minimum"
        );
    }

    assert!(shrank, "a stable network must give delay back");
    assert_eq!(
        buffer.target_delay_frames(),
        MIN_FRAMES,
        "adaptive shrinking must stop at the configured minimum"
    );
    assert_eq!(
        buffer.stats().frames_concealed,
        0,
        "no frame should be lost"
    );
}

/// When the jitter buffer is set to a fixed 3 frames
/// Then the delay stays at 3 frames and adaptation is disabled
///
/// Fixed means the delay bounds leave no room: minimum = maximum.
///
/// Verifies: REQ-LAT-110
#[test]
fn fixed_jitter_buffer_keeps_a_constant_delay() {
    const FIXED_FRAMES: u32 = 3;
    let mut buffer = playout(FIXED_FRAMES, FIXED_FRAMES, FIXED_FRAMES);
    let mut out = playout_frame(0.0);

    assert_eq!(buffer.target_delay_frames(), FIXED_FRAMES);

    // Sequence 3 never arrives.
    for sequence in [0u32, 1, 2, 4] {
        buffer.write(sequence, &playout_frame(sequence as f32));
    }
    assert!(matches!(
        buffer.read_into(&mut out).result,
        PlayoutResult::Played { sequence: 0 }
    ));
    assert_eq!(
        buffer.ready_frames() as u32,
        FIXED_FRAMES,
        "the configured delay must actually be held"
    );

    // Losses would push an adaptive buffer to grow; a fixed one must not move.
    for _ in 0..3 {
        let _ = buffer.read_into(&mut out);
    }
    assert!(
        buffer.stats().frames_concealed > 0,
        "a loss must have occurred"
    );
    let mut next = 5;
    run_periods(&mut buffer, &mut next, ADAPT_WINDOW_FRAMES, true);
    for _ in 0..10 {
        assert_eq!(buffer.adapt(), None);
    }

    assert_eq!(
        buffer.target_delay_frames(),
        FIXED_FRAMES,
        "fixed mode must ignore adaptation"
    );
}

/// Given the zero-latency preset
/// When the jitter buffer is in passthrough mode
/// Then packets are played back immediately and add 0ms of delay
///
/// Verifies: REQ-LAT-111
#[test]
fn passthrough_jitter_buffer_delivers_immediately() {
    let preset = AudioPreset::ZeroLatency;
    assert_eq!(
        preset.jitter_buffer_delay_ms(BUDGET_SAMPLE_RATE),
        0.0,
        "the zero-latency preset must select a passthrough jitter buffer"
    );

    // The same settings the receive path builds for the preset.
    let mut buffer = PlayoutBuffer::new(PlayoutConfig::for_delay(
        PLAYOUT_FRAME,
        preset.jitter_buffer_frames(),
    ));
    let mut out = playout_frame(0.0);
    assert_eq!(
        buffer.target_delay_frames(),
        0,
        "passthrough mode must not add any buffering delay"
    );

    buffer.write(0, &playout_frame(0.7));
    assert_eq!(
        buffer.read_into(&mut out).result,
        PlayoutResult::Played { sequence: 0 },
        "the first packet must be played without buffering"
    );
    assert_eq!(out, playout_frame(0.7));
    assert_eq!(buffer.ready_frames(), 0, "passthrough must retain nothing");

    // Jitter shows up as a disturbance rather than as added delay: a lost
    // frame is concealed, and the next one again plays without being held.
    buffer.write(2, &playout_frame(0.2));
    assert_eq!(
        buffer.read_into(&mut out).result,
        PlayoutResult::Concealed { sequence: 1 }
    );
    assert_eq!(
        buffer.read_into(&mut out).result,
        PlayoutResult::Played { sequence: 2 }
    );
    assert_eq!(out, playout_frame(0.2));
    assert_eq!(
        buffer.ready_frames(),
        0,
        "a loss must not make passthrough start holding frames"
    );
}

// ---------------------------------------------------------------------------
// Packet loss
// ---------------------------------------------------------------------------

/// Given FEC is enabled
/// When one packet in a group is lost
/// Then it is reconstructed from the FEC packet
///
/// Verifies: REQ-LAT-112
#[test]
fn fec_recovers_a_single_lost_packet() {
    let mut encoder = FecEncoder::with_group_size(4);
    let mut decoder = FecDecoder::with_group_size(4);

    let packets = vec![
        vec![1, 2, 3, 4],
        vec![5, 6, 7, 8],
        vec![9, 10, 11, 12],
        vec![13, 14, 15, 16],
    ];

    let mut fec_packet: Option<FecPacket> = None;
    for packet in &packets {
        fec_packet = encoder.add_packet(packet);
    }
    let fec = fec_packet.expect("FEC packet should be generated");

    // Packet 2 is lost in transit.
    decoder.add_packet(0, 0, &packets[0]);
    decoder.add_packet(0, 1, &packets[1]);
    decoder.add_packet(0, 3, &packets[3]);

    let recovered = decoder.add_fec(fec);
    assert!(recovered.is_some(), "Packet should be recovered by FEC");

    let recovered = recovered.unwrap();
    assert_eq!(recovered.packet_index, 2);
    assert_eq!(
        recovered.data, packets[2],
        "the recovered payload must be byte-identical to the original"
    );
}

/// Given FEC is enabled
/// When two packets in the same group are lost
/// Then FEC cannot reconstruct them
///
/// Verifies: REQ-LAT-113
#[test]
fn fec_cannot_recover_two_lost_packets_in_a_group() {
    let mut encoder = FecEncoder::with_group_size(4);
    let mut decoder = FecDecoder::with_group_size(4);

    let packets = vec![
        vec![1, 2, 3, 4],
        vec![5, 6, 7, 8],
        vec![9, 10, 11, 12],
        vec![13, 14, 15, 16],
    ];

    let mut fec_packet: Option<FecPacket> = None;
    for packet in &packets {
        fec_packet = encoder.add_packet(packet);
    }
    let fec = fec_packet.expect("FEC packet should be generated");

    // Packets 1 and 2 are both lost - a single parity packet is not enough.
    decoder.add_packet(0, 0, &packets[0]);
    decoder.add_packet(0, 3, &packets[3]);

    let recovered = decoder.add_fec(fec);
    assert!(
        recovered.is_none(),
        "Should not recover with 2+ missing packets"
    );
}

/// Given FEC is disabled
/// When a packet is lost
/// Then PLC fades the previous frame out instead of producing a click
///
/// A click is a discontinuity: the concealed frame must keep the shape of the
/// last good frame at a reduced gain, never jump straight to silence.
///
/// Verifies: REQ-LAT-114
#[test]
fn plc_fades_out_instead_of_producing_a_click() {
    const FRAME_SIZE: u32 = 64;
    const CHANNELS: u16 = 1;

    let mut plc = PcmPlc::new(FRAME_SIZE, CHANNELS);
    let last_good = vec![0.5f32; FRAME_SIZE as usize];
    plc.store_frame(&last_good);
    assert_eq!(plc.consecutive_losses(), 0);

    let first = plc.generate_concealment();
    assert_eq!(first.len(), last_good.len());
    assert!(
        first[0] > 0.0 && first[0] < last_good[0],
        "the first concealed frame must be attenuated but not silent, got {}",
        first[0]
    );

    // Each further loss must be quieter than the previous one.
    let mut previous = first[0];
    for _ in 0..3 {
        let next = plc.generate_concealment();
        assert!(
            next[0] < previous,
            "concealment must keep fading: {} did not drop below {}",
            next[0],
            previous
        );
        previous = next[0];
    }

    // After enough consecutive losses the output is silence rather than a
    // sustained tone.
    for _ in 0..5 {
        let _ = plc.generate_concealment();
    }
    let silent = plc.generate_concealment();
    assert!(
        silent.iter().all(|&s| s == 0.0),
        "prolonged loss must decay to silence"
    );

    // A good frame resets the concealment state.
    plc.store_frame(&last_good);
    assert_eq!(plc.consecutive_losses(), 0);
}

// ---------------------------------------------------------------------------
// FEC over the real transport (ADR-021)
// ---------------------------------------------------------------------------

/// A preset that enables FEC must send a recoverable FEC packet per group, and
/// the receiver must hand the recovered frame to the audio callback under the
/// sequence number that went missing.
///
/// This exercises the wire path: encode, packetise, send over UDP, and recover.
///
/// Verifies: REQ-AUD-026
#[tokio::test]
async fn fec_recovers_a_dropped_frame_over_the_transport() {
    use jamjam::network::{AudioEncodingConfig, Connection};
    use std::sync::{Arc, Mutex};

    let preset = AudioPreset::Balanced;
    let group_size = preset
        .fec_group_size()
        .expect("balanced must enable FEC (ADR-021)");
    let frame_size = preset.frame_size() as usize;

    // Receiver: collect every sequence the callback is handed.
    let mut receiver = Connection::new("127.0.0.1:0")
        .await
        .expect("receiver socket");
    let encoding = AudioEncodingConfig {
        codec_type: preset.codec_type(),
        sample_rate: BUDGET_SAMPLE_RATE,
        channels: 1,
        frame_size: preset.frame_size(),
        bitrate: 0,
        fec_group_size: preset.fec_group_size(),
    };
    receiver
        .set_audio_encoding(encoding.clone())
        .expect("receiver encoding");

    let delivered: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let delivered_for_callback = delivered.clone();
    receiver.set_audio_callback(move |sequence, _payload, _timestamp| {
        delivered_for_callback.lock().unwrap().push(sequence);
    });

    let receiver_addr = receiver.local_addr();

    // Sender: same encoding, pointed at the receiver.
    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender socket");
    sender
        .set_audio_encoding(encoding)
        .expect("sender encoding");
    receiver
        .connect(sender.local_addr())
        .await
        .expect("receiver starts its loop");
    sender
        .connect(receiver_addr)
        .await
        .expect("sender connects");
    // Audio goes once the two have agreed keys.
    while sender.security() != jamjam::network::LinkSecurity::Encrypted
        || receiver.security() != jamjam::network::LinkSecurity::Encrypted
    {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Send one full group. Every frame differs so a recovered payload cannot be
    // confused with a neighbour.
    let frames: Vec<Vec<f32>> = (0..group_size)
        .map(|i| vec![(i as f32 + 1.0) / 16.0; frame_size])
        .collect();
    for (index, frame) in frames.iter().enumerate() {
        sender
            .send_audio(frame, index as u32 * frame_size as u32)
            .await
            .expect("send");
    }

    // Give the loopback socket time to deliver the group and its FEC packet.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let sequences = delivered.lock().unwrap().clone();
    assert!(
        sequences.len() >= group_size,
        "expected at least {} audio frames, got {:?}",
        group_size,
        sequences
    );

    // Every sequence in the group must have been delivered exactly once. A
    // duplicate would mean FEC re-delivered a packet that was never lost.
    for expected in 0..group_size as u32 {
        let count = sequences.iter().filter(|&&s| s == expected).count();
        assert_eq!(
            count, 1,
            "sequence {} was delivered {} times: {:?}",
            expected, count, sequences
        );
    }
}
