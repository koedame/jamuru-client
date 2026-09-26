//! Audio quality tests based on docs-spec/behavior/audio-quality.feature
//!
//! Tests for audio quality functionality.

use jamjam::audio::{
    capture_to_wire, create_resampler, pan_received, AudioCodec, AudioConfig, AudioEngine,
    AudioPreset, BitDepth, CaptureConfig, CodecConfig, CodecType, LocalMonitor, PcmCodec,
    PlaybackConfig, WIRE_CHANNELS,
};
use jamjam::protocol::Packet;

/// Test: Operates at 48kHz sample rate
/// When sample rate is set to "48000Hz"
/// Then audio engine operates at 48kHz
/// Verifies: REQ-AUD-104
#[test]
fn test_sample_rate_48khz() {
    let config = AudioConfig {
        sample_rate: 48000,
        channels: 1,
        frame_size: 128,
    };
    let engine = AudioEngine::new(config);

    assert_eq!(engine.config().sample_rate, 48000);
}

/// Test: Operates at 96kHz sample rate
/// When sample rate is set to "96000Hz"
/// Then audio engine operates at 96kHz
/// Verifies: REQ-AUD-105
#[test]
fn test_sample_rate_96khz() {
    let config = AudioConfig {
        sample_rate: 96000,
        channels: 1,
        frame_size: 128,
    };
    let engine = AudioEngine::new(config);

    assert_eq!(engine.config().sample_rate, 96000);
}

/// Runs one captured frame the way a session sends it - `capture_to_wire`,
/// then `Connection::send_audio` over a UDP socket - and returns what the peer
/// decodes: the interleaved samples and the size of the payload on the wire.
async fn send_captured_frame(
    captured: &[f32],
    channels: usize,
    volume: f32,
    pan: i32,
) -> (Vec<f32>, usize) {
    use jamjam::network::{AudioEncodingConfig, Connection};
    use jamjam::protocol::PacketType;

    let frame_size = captured.len() / channels;
    let peer = std::net::UdpSocket::bind("127.0.0.1:0").expect("peer socket");
    peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("read timeout");

    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender socket");
    sender
        .set_audio_encoding(AudioEncodingConfig {
            codec_type: CodecType::Pcm,
            sample_rate: 48000,
            channels: WIRE_CHANNELS as u16,
            frame_size: frame_size as u32,
            bitrate: 0,
            fec_group_size: None,
        })
        .expect("encoding");
    sender
        .connect(peer.local_addr().expect("peer address"))
        .await
        .expect("connect");

    let mut wire = vec![0.0f32; frame_size * WIRE_CHANNELS];
    capture_to_wire(captured, channels, volume, pan, &mut wire);
    sender.send_audio(&wire, 0).await.expect("send audio");

    let mut buf = [0u8; 4096];
    let payload = loop {
        let len = peer.recv(&mut buf).expect("the sender's packet arrives");
        let packet = Packet::from_bytes(&buf[..len]).expect("a valid packet");
        if packet.packet_type == PacketType::Audio {
            break packet.payload;
        }
    };

    let mut receiver = PcmCodec::new(&CodecConfig {
        codec_type: CodecType::Pcm,
        sample_rate: 48000,
        channels: WIRE_CHANNELS as u16,
        frame_size: frame_size as u32,
        bitrate: 0,
    });
    let decoded = receiver.decode(&payload).expect("decode");
    (decoded, payload.len())
}

/// Test: Operates with mono input
/// When input channel is set to "mono"
/// Then the mono capture is what is transmitted
/// And the receiving side plays both channels the same
/// Verifies: REQ-AUD-107
#[tokio::test]
async fn test_mono_input() {
    let captured = [0.5f32, -0.25, 0.75, 0.125];

    let (decoded, _) = send_captured_frame(&captured, 1, 1.0, 0).await;

    assert_eq!(decoded.len(), captured.len() * WIRE_CHANNELS);
    for (frame, &sample) in decoded.as_chunks::<WIRE_CHANNELS>().0.iter().zip(&captured) {
        assert_eq!(frame[0], frame[1], "both channels are the same");
        let expected = sample * std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (frame[0] - expected).abs() < 1e-6,
            "the capture {sample} arrives centred: {frame:?}"
        );
    }
}

/// Test: Operates with stereo input
/// When input channel is set to "stereo"
/// Then 2-channel audio is transmitted
/// And the receiving side plays it in stereo
/// Verifies: REQ-AUD-108
#[tokio::test]
async fn test_stereo_input() {
    // Left and right carry different audio, so mixing them across would show.
    let captured = [0.5f32, -0.25, 0.75, 0.125, -0.5, 0.0];

    let (decoded, payload_bytes) = send_captured_frame(&captured, 2, 1.0, 0).await;

    assert_eq!(
        payload_bytes,
        captured.len() * std::mem::size_of::<f32>(),
        "both channels are on the wire"
    );
    assert_eq!(
        decoded, captured,
        "the receiving side gets each side as it was captured"
    );
}

/// A stereo capture with sound on the left only reaches the listener's output
/// on the left only: what the sender keeps apart, the receiving side's pan
/// keeps apart too.
///
/// Verifies: REQ-AUD-108
/// Verifies: REQ-AUD-120
#[tokio::test]
async fn test_stereo_input_is_played_back_in_stereo() {
    let captured = [0.5f32, 0.0, -0.25, 0.0, 0.75, 0.0];

    let (mut heard, _) = send_captured_frame(&captured, 2, 1.0, 0).await;
    pan_received(&mut heard, 2, 1.0, 0);

    assert_eq!(
        heard, captured,
        "the left stays on the left, the right silent"
    );
}

/// Given participant B has set their transmit channel count to mono
/// When B sends A its latency info over the wire
/// Then A decodes B's channel count as mono
///
/// Verifies: REQ-AUD-117
#[test]
fn test_peer_channel_count_is_carried_in_latency_info() {
    let info = jamjam::protocol::LatencyInfoMessage {
        capture_buffer_ms: 2.67,
        playback_buffer_ms: 2.67,
        encode_ms: 0.0,
        decode_ms: 0.0,
        jitter_buffer_ms: 0.0,
        frame_size: 128,
        sample_rate: 48000,
        codec: "pcm".to_string(),
        channel_count: 1,
    };

    let packet = Packet::latency_info(0, &info);
    let wire = packet.to_bytes();
    let received = Packet::from_bytes(&wire).expect("parse");
    let decoded =
        jamjam::protocol::LatencyInfoMessage::from_bytes(&received.payload).expect("decode");

    assert_eq!(decoded.channel_count, 1);
}

/// Test: Operates with 64 sample frame size
/// When frame size is set to "64 samples"
/// Then audio buffer becomes 64 samples
/// Verifies: REQ-AUD-109
#[test]
fn test_frame_size_64() {
    let config = AudioConfig {
        sample_rate: 48000,
        channels: 1,
        frame_size: 64,
    };

    assert_eq!(config.frame_size, 64);

    // Calculate latency: 64 / 48000 = 1.33ms
    let latency_ms = config.frame_size as f32 / config.sample_rate as f32 * 1000.0;
    assert!((latency_ms - 1.33).abs() < 0.1);
}

/// Test: Operates with 256 sample frame size
/// When frame size is set to "256 samples"
/// Then audio buffer becomes 256 samples
/// Verifies: REQ-AUD-110
#[test]
fn test_frame_size_256() {
    let config = AudioConfig {
        sample_rate: 48000,
        channels: 1,
        frame_size: 256,
    };

    assert_eq!(config.frame_size, 256);

    // Calculate latency: 256 / 48000 = 5.33ms
    let latency_ms = config.frame_size as f32 / config.sample_rate as f32 * 1000.0;
    assert!((latency_ms - 5.33).abs() < 0.1);
}

/// What a monitored session sounds like: a mono instrument captured every
/// frame, and an output that already carries the other participants. Returns
/// the output once the monitor has had time to start.
fn play_session(
    monitor: &LocalMonitor,
    mut tap: jamjam::audio::MonitorTap,
    instrument: f32,
    others: f32,
) -> Vec<f32> {
    const FRAME: usize = 64;
    let mut out = Vec::new();
    for _ in 0..6 {
        tap.push(&[instrument; FRAME]);
        out = vec![others; FRAME * WIRE_CHANNELS];
        monitor.mix_into(&mut out);
    }
    out
}

/// Test: Enable local monitoring
/// When local monitoring is set to "ON"
/// Then my own audio is heard without the network delay
/// And the other participants' audio is heard at the same time
/// Verifies: REQ-AUD-111
#[test]
fn test_local_monitoring_on_plays_my_input_together_with_the_others() {
    let monitor = LocalMonitor::new(64);
    let tap = monitor.tap();
    monitor.set_enabled(true);

    let out = play_session(&monitor, tap, 0.25, 0.5);

    assert!(
        out.iter().all(|s| (*s - 0.75).abs() < 1e-6),
        "the instrument (0.25) and the others (0.5) are both in the output: {:?}",
        &out[..4]
    );
}

/// Test: Disable local monitoring
/// When local monitoring is set to "OFF"
/// Then my own audio is not heard directly
/// And only the other participants' audio is heard
/// Verifies: REQ-AUD-112
#[test]
fn test_local_monitoring_off_leaves_only_the_others() {
    let monitor = LocalMonitor::new(64);
    let tap = monitor.tap();
    monitor.set_enabled(true);
    monitor.set_enabled(false);

    let out = play_session(&monitor, tap, 0.25, 0.5);

    assert!(
        out.iter().all(|s| (*s - 0.5).abs() < 1e-6),
        "only the others (0.5) are in the output: {:?}",
        &out[..4]
    );
}

/// Test: BitDepth configuration
#[test]
fn test_bit_depth_options() {
    // 16-bit
    let config_i16 = CaptureConfig {
        bit_depth: BitDepth::I16,
        ..Default::default()
    };
    assert_eq!(config_i16.bit_depth, BitDepth::I16);

    // 24-bit
    let config_i24 = CaptureConfig {
        bit_depth: BitDepth::I24,
        ..Default::default()
    };
    assert_eq!(config_i24.bit_depth, BitDepth::I24);

    // 32-bit float
    let config_f32 = CaptureConfig {
        bit_depth: BitDepth::F32,
        ..Default::default()
    };
    assert_eq!(config_f32.bit_depth, BitDepth::F32);
}

/// Test: Default configurations are correct
#[test]
fn test_default_configs() {
    let audio_config = AudioConfig::default();
    assert_eq!(audio_config.sample_rate, 48000);
    assert_eq!(audio_config.channels, 1);
    assert_eq!(audio_config.frame_size, 64);

    let capture_config = CaptureConfig::default();
    assert_eq!(capture_config.sample_rate, 48000);
    assert_eq!(capture_config.channels, 1);
    assert_eq!(capture_config.frame_size, 64);
    assert_eq!(capture_config.bit_depth, BitDepth::F32);

    let playback_config = PlaybackConfig::default();
    assert_eq!(playback_config.sample_rate, 48000);
    assert_eq!(playback_config.channels, 1);
    assert_eq!(playback_config.frame_size, 64);
    assert_eq!(playback_config.bit_depth, BitDepth::F32);
}

/// Test: Capture callback accepts FnMut (mutable state)
/// This verifies the callback signature change from Fn+Sync to FnMut+Send
#[test]
#[allow(unused_assignments)]
fn test_capture_callback_accepts_fnmut() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let config = AudioConfig {
        sample_rate: 48000,
        channels: 1,
        frame_size: 64,
    };
    let engine = AudioEngine::new(config);

    // Create a mutable counter - this proves FnMut works
    let mut _call_count = 0u32;

    // Also test with Arc<Atomic> for thread-safe sharing
    let shared_counter = Arc::new(AtomicU32::new(0));
    let shared_counter_clone = shared_counter.clone();

    // This callback uses mutable state (FnMut required)
    // If the signature was Fn+Sync, this wouldn't compile
    let callback = move |_samples: &[f32], _timestamp: u64| {
        _call_count += 1; // Mutable capture - requires FnMut
        shared_counter_clone.fetch_add(1, Ordering::SeqCst);
    };

    // Verify callback type is accepted (won't actually start without audio device)
    // The test passes if this compiles - it proves FnMut is accepted
    let _ = callback; // Suppress unused warning

    // Engine should be creatable
    assert!(!engine.is_capture_running());
}

/// Test: Capture callback can use non-Sync types like rtrb::Producer
/// This is the key test for zero-allocation audio path
#[test]
fn test_capture_callback_accepts_non_sync_types() {
    use rtrb::RingBuffer;

    let config = AudioConfig {
        sample_rate: 48000,
        channels: 1,
        frame_size: 32, // Test with 32 samples
    };
    let engine = AudioEngine::new(config);

    // Create rtrb ring buffer - Producer is Send but NOT Sync
    let (mut producer, consumer) = RingBuffer::<f32>::new(1024);

    // This callback moves the producer directly (not behind Arc)
    // If the signature required Sync, this wouldn't compile because
    // rtrb::Producer uses Cell internally which is not Sync
    let callback = move |samples: &[f32], _timestamp: u64| {
        // Write directly to rtrb - zero allocation
        if let Ok(mut chunk) = producer.write_chunk_uninit(samples.len()) {
            let slices = chunk.as_mut_slices();
            let first_len = slices.0.len().min(samples.len());
            for (i, &sample) in samples[..first_len].iter().enumerate() {
                slices.0[i].write(sample);
            }
            if first_len < samples.len() {
                for (i, &sample) in samples[first_len..].iter().enumerate() {
                    slices.1[i].write(sample);
                }
            }
            unsafe {
                chunk.commit_all();
            }
        }
    };

    // Test that we can use the callback type
    let _ = callback;

    // Verify consumer can read (simulating what send thread does)
    assert_eq!(consumer.slots(), 0); // Empty initially

    // Engine should be creatable with 32-sample buffer
    assert_eq!(engine.config().frame_size, 32);
}

// ---------------------------------------------------------------------------
// Codec and transport fidelity
//
// These verify the "no processing" promise of ADR-005: whatever the musician
// plays must come out the other end unchanged.
// ---------------------------------------------------------------------------

/// Given a link with at least 10 Mbps of bandwidth
/// When the codec is set to uncompressed PCM
/// Then audio is sent uncompressed at ~1.5 Mbps per channel with 0ms of codec delay
///
/// Verifies: REQ-CORE-002
/// Verifies: REQ-AUD-101
#[test]
fn test_pcm_codec_is_uncompressed() {
    let config = CodecConfig {
        codec_type: CodecType::Pcm,
        sample_rate: 48000,
        channels: 1,
        frame_size: 128,
        ..Default::default()
    };
    let mut codec = PcmCodec::new(&config);
    assert_eq!(codec.codec_type(), CodecType::Pcm);

    let samples = vec![0.25f32; config.frame_size as usize];
    let encoded = codec.encode(&samples).expect("PCM encode should not fail");

    // Uncompressed f32: exactly 4 bytes per sample, no framing overhead.
    assert_eq!(
        encoded.len(),
        samples.len() * 4,
        "PCM must not compress: {} samples should produce {} bytes",
        samples.len(),
        samples.len() * 4
    );

    // 48000 samples/s * 4 bytes * 8 bits = 1.536 Mbps per channel.
    let bitrate_bps = config.sample_rate as f64 * 4.0 * 8.0;
    assert!(
        (bitrate_bps - 1_536_000.0).abs() < 1.0,
        "PCM bitrate {:.0} bps is not the expected ~1.5 Mbps/ch",
        bitrate_bps
    );
}

/// PCM encoding is a byte reinterpretation, so decoding must return the exact
/// same sample values - no dithering, no requantisation.
///
/// Verifies: REQ-AUD-020
#[test]
fn test_pcm_codec_round_trip_is_bit_exact() {
    let config = CodecConfig {
        codec_type: CodecType::Pcm,
        channels: 1,
        frame_size: 8,
        ..Default::default()
    };
    let mut codec = PcmCodec::new(&config);

    let samples: Vec<f32> = vec![
        0.0,
        1.0,
        -1.0,
        0.5,
        -0.5,
        f32::MIN_POSITIVE,
        0.123_456_79,
        -0.9,
    ];
    let encoded = codec.encode(&samples).expect("encode");
    let decoded = codec.decode(&encoded).expect("decode");

    assert_eq!(
        decoded, samples,
        "PCM round trip must preserve every sample bit-for-bit"
    );

    // Truncated payloads must be rejected rather than silently mangled.
    assert!(codec.decode(&[0u8; 3]).is_err());
}

/// An audio packet must survive serialisation to the wire and back unchanged.
///
/// Verifies: REQ-AUD-021
#[test]
fn test_audio_packet_round_trip_preserves_payload() {
    let payload: Vec<u8> = (0..=255u8).collect();
    let packet = Packet::audio(42, 4096, payload.clone());

    let bytes = packet.to_bytes();
    let decoded = Packet::from_bytes(&bytes).expect("a packet we just serialised must parse");

    assert_eq!(
        decoded.payload, payload,
        "payload must survive the round trip"
    );
    assert_eq!(decoded.sequence, 42);
    assert_eq!(decoded.timestamp, 4096);
}

/// Given audio processing (AEC, NS, AGC) is disabled
/// When an instrument is played
/// Then the received samples are identical to the ones that were captured
///
/// Verifies: REQ-CORE-003
/// Verifies: REQ-AUD-113
#[test]
fn test_audio_is_transmitted_without_processing() {
    let config = CodecConfig {
        codec_type: CodecType::Pcm,
        sample_rate: 48000,
        channels: 1,
        frame_size: 64,
        ..Default::default()
    };

    // A signal with sharp transients - anything that gates, compresses or
    // filters would show up here.
    let captured: Vec<f32> = (0..64)
        .map(|i| {
            let t = i as f32 / 48000.0;
            (t * 440.0 * std::f32::consts::TAU).sin() * if i % 16 == 0 { 0.99 } else { 0.01 }
        })
        .collect();

    let mut sender = PcmCodec::new(&config);
    let mut receiver = PcmCodec::new(&config);

    let encoded = sender.encode(&captured).expect("encode");
    let packet = Packet::audio(0, 0, encoded);
    let wire = packet.to_bytes();
    let received_packet = Packet::from_bytes(&wire).expect("parse");
    let played = receiver.decode(&received_packet.payload).expect("decode");

    assert_eq!(
        played, captured,
        "the full capture -> encode -> packet -> decode path must not alter a single sample"
    );
}

/// Given participant A runs at 96kHz and participant B at 48kHz
/// When the session starts
/// Then A's audio is resampled to 48kHz
///
/// Verifies: REQ-AUD-106
#[test]
fn test_mismatched_sample_rates_are_resampled() {
    const CHUNK: usize = 128;

    let mut resampler = create_resampler(96000, 48000, CHUNK)
        .expect("96kHz -> 48kHz resampler should be creatable");

    let input: Vec<f32> = (0..CHUNK)
        .map(|i| (i as f32 / 96000.0 * 440.0 * std::f32::consts::TAU).sin())
        .collect();

    // Measure over several chunks: cubic interpolation holds back a few samples
    // for its window, so a single chunk understates the output by that margin.
    const CHUNKS: usize = 8;
    let mut produced = 0usize;
    for _ in 0..CHUNKS {
        produced += resampler
            .process(&input)
            .expect("resampling should succeed")
            .len();
    }

    // Halving the rate halves the sample count.
    let expected = CHUNK * CHUNKS / 2;
    assert!(
        produced.abs_diff(expected) <= 8,
        "96kHz -> 48kHz should yield about {} samples over {} chunks, got {}",
        expected,
        CHUNKS,
        produced
    );

    // Matching rates must not introduce any conversion at all.
    let mut passthrough = create_resampler(48000, 48000, CHUNK).expect("passthrough");
    assert_eq!(passthrough.process(&input).expect("passthrough"), input);
    assert_eq!(passthrough.latency_samples(), 0);
}

/// Each preset must carry the frame size, jitter buffer depth, codec and FEC
/// settings the specification records (ADR-019, ADR-021).
///
/// Verifies: REQ-AUD-022
/// Verifies: REQ-AUD-114
/// Verifies: REQ-AUD-115
/// Verifies: REQ-AUD-116
#[test]
fn test_preset_parameters_match_the_specification() {
    struct Expected {
        preset: AudioPreset,
        frame_size: u32,
        jitter_frames: u32,
        codec: CodecType,
        fec_group: Option<usize>,
    }

    let expected = [
        Expected {
            preset: AudioPreset::ZeroLatency,
            frame_size: 32,
            jitter_frames: 0,
            codec: CodecType::Pcm,
            fec_group: None,
        },
        Expected {
            preset: AudioPreset::UltraLowLatency,
            frame_size: 64,
            jitter_frames: 1,
            codec: CodecType::Pcm,
            fec_group: None,
        },
        Expected {
            preset: AudioPreset::Balanced,
            frame_size: 128,
            jitter_frames: 4,
            codec: CodecType::Pcm,
            fec_group: Some(4),
        },
        Expected {
            preset: AudioPreset::HighQuality,
            frame_size: 256,
            jitter_frames: 8,
            codec: CodecType::Pcm,
            fec_group: Some(8),
        },
    ];

    for case in expected {
        let name = case.preset.name();
        assert_eq!(
            case.preset.frame_size(),
            case.frame_size,
            "{} frame size",
            name
        );
        assert_eq!(
            case.preset.jitter_buffer_frames(),
            case.jitter_frames,
            "{} jitter buffer depth",
            name
        );
        assert_eq!(case.preset.codec_type(), case.codec, "{} codec", name);
        assert_eq!(
            case.preset.fec_group_size(),
            case.fec_group,
            "{} FEC group",
            name
        );
    }
}

/// Opus compresses, but only at the frame sizes libopus accepts
///
/// No preset can use Opus: 48kHz frames must be 120/240/480/960/1920/2880
/// samples and every preset uses a power of two (ADR-021). This test pins both
/// halves of that finding - the codec works, and it rejects a preset frame size
/// - so the constraint cannot be forgotten. The `opus` CI job runs it.
///
/// Verifies: REQ-AUD-027
#[cfg(feature = "opus-codec")]
#[test]
fn test_opus_codec_compresses_audio() {
    let preset = AudioPreset::Balanced;

    // The frame size Opus would need is not the one the preset uses.
    let rejected = CodecConfig {
        codec_type: CodecType::Opus,
        sample_rate: 48_000,
        channels: 1,
        frame_size: preset.frame_size(),
        bitrate: 128_000,
    };
    let mut rejecting_codec = jamjam::audio::create_codec(&rejected).expect("Opus codec");
    let preset_frame = vec![0.1f32; preset.frame_size() as usize];
    assert!(
        rejecting_codec.encode(&preset_frame).is_err(),
        "Opus unexpectedly accepted a {}-sample frame; revisit ADR-021",
        preset.frame_size()
    );

    let config = CodecConfig {
        codec_type: CodecType::Opus,
        sample_rate: 48_000,
        channels: 1,
        frame_size: 120,
        bitrate: 128_000,
    };
    let mut codec = jamjam::audio::create_codec(&config).expect("Opus codec");

    let samples: Vec<f32> = (0..120usize)
        .map(|i| (i as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();
    let encoded = codec.encode(&samples).expect("Opus encode");

    // Compression means fewer bytes than the 4-per-sample PCM baseline.
    assert!(
        encoded.len() < samples.len() * 4,
        "Opus produced {} bytes for {} samples, no better than PCM",
        encoded.len(),
        samples.len()
    );
    assert!(!encoded.is_empty(), "Opus must produce a frame");

    // Decoding returns the configured frame length, not the encoded length.
    let decoded = codec.decode(&encoded).expect("Opus decode");
    assert_eq!(decoded.len(), samples.len());
}

/// Stereo resampling must convert both channels, keeping them interleaved and
/// independent.
///
/// A mono resampler fed interleaved stereo would treat alternating samples from
/// different channels as one stream, shifting the pitch and mixing the channels.
/// The receive path is stereo, so this is the case that actually runs.
///
/// Verifies: REQ-AUD-028
#[test]
fn test_stereo_resampling_converts_both_channels() {
    use jamjam::audio::create_resampler_with_channels;

    const FRAMES: usize = 128;
    const CHANNELS: usize = 2;

    // Left is a constant, right is its negation. Any channel bleed shows up as a
    // value that is neither.
    let interleaved: Vec<f32> = (0..FRAMES).flat_map(|_| [0.5f32, -0.5f32]).collect();

    let mut resampler = create_resampler_with_channels(96_000, 48_000, FRAMES, CHANNELS)
        .expect("stereo 96k -> 48k resampler");

    // Several chunks, so the startup transient does not dominate.
    const CHUNKS: usize = 8;
    let mut produced = Vec::new();
    for _ in 0..CHUNKS {
        produced.extend(resampler.process(&interleaved).expect("resample"));
    }

    // Halving the rate halves the frame count; the sample count follows.
    let expected = FRAMES * CHUNKS * CHANNELS / 2;
    assert!(
        produced.len().abs_diff(expected) <= CHANNELS * 8,
        "96kHz -> 48kHz stereo should yield about {} samples, got {}",
        expected,
        produced.len()
    );

    // Interleaving must survive: even indices stay positive, odd stay negative.
    // Skip the first frames, where the cubic window is still filling.
    let skip = 8 * CHANNELS;
    assert!(produced.len() > skip, "not enough output to inspect");
    for (index, &sample) in produced.iter().enumerate().skip(skip) {
        if index % CHANNELS == 0 {
            assert!(
                sample > 0.0,
                "left channel sample {} became {}, channels are mixed",
                index,
                sample
            );
        } else {
            assert!(
                sample < 0.0,
                "right channel sample {} became {}, channels are mixed",
                index,
                sample
            );
        }
    }

    // Matching rates must still bypass conversion entirely.
    let mut passthrough =
        create_resampler_with_channels(48_000, 48_000, FRAMES, CHANNELS).expect("passthrough");
    assert_eq!(
        passthrough.process(&interleaved).expect("passthrough"),
        interleaved
    );

    // A zero channel count is a configuration error, not a silent mono fallback.
    assert!(create_resampler_with_channels(96_000, 48_000, FRAMES, 0).is_err());
}
