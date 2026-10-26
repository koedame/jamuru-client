//! Connection tests based on docs-spec/behavior/connection.feature
//!
//! Tests for session connection functionality.

mod common;

use jamjam::network::Connection;

/// Test: Connection stats are initialized
#[tokio::test]
async fn test_connection_stats_initial() {
    let conn = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");
    let stats = conn.stats();

    assert_eq!(stats.packets_sent, 0, "Initial packets_sent should be 0");
    assert_eq!(
        stats.packets_received, 0,
        "Initial packets_received should be 0"
    );
    assert_eq!(stats.bytes_sent, 0, "Initial bytes_sent should be 0");
    assert_eq!(
        stats.bytes_received, 0,
        "Initial bytes_received should be 0"
    );
}

/// Test: Connection is in disconnected state when created
#[tokio::test]
async fn test_connection_initial_state() {
    let conn = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");

    assert!(
        !conn.is_connected(),
        "New connection should not be connected"
    );
    assert!(
        conn.local_addr().port() > 0,
        "Local address should have valid port"
    );
}

// ---------------------------------------------------------------------------
// Invite links
// ---------------------------------------------------------------------------

/// Given the creator has created a room
/// When the user opens the invite URL "jamuru://join/ABC234XYZ"
/// Then the room code is recovered so the join can start
///
/// The OS-level registration that hands the URL to the app is platform plumbing;
/// what this covers is that a link the app produced round-trips, and that a link
/// it did not produce is rejected rather than guessed at.
///
/// Verifies: REQ-CON-103
#[test]
fn test_invite_url_round_trips() {
    use jamjam::network::{invite_url, parse_invite_url, InviteCode};

    // A link the app built must parse back to the same code.
    for _ in 0..20 {
        let code = InviteCode::generate();
        let url = invite_url(code.as_str());
        assert!(
            url.starts_with("jamuru://join/"),
            "unexpected invite URL: {}",
            url
        );
        assert_eq!(parse_invite_url(&url), Some(code.to_string()));
    }

    // A lower-cased link still works: mail clients and chat apps do this.
    assert_eq!(
        parse_invite_url("jamuru://join/abc234xyz"),
        Some("ABC234XYZ".to_string())
    );

    // Query strings and a trailing slash are tolerated.
    assert_eq!(
        parse_invite_url("jamuru://join/ABC234XYZ?from=chat"),
        Some("ABC234XYZ".to_string())
    );
    assert_eq!(
        parse_invite_url("jamuru://join/ABC234XYZ/"),
        Some("ABC234XYZ".to_string())
    );

    // Anything else must be refused rather than half-interpreted.
    for rejected in [
        "https://example.com/join/ABC234XYZ", // wrong scheme
        "jamuru://leave/ABC234XYZ",           // wrong action
        "jamuru://join/ABC",                  // too short
        "jamuru://join/ABC2345",              // too long
        "jamuru://join/ABC01I",               // excluded confusing characters
        "jamuru://join/",                     // no code
        "jamuru://join/ABC234XYZ/extra",      // deeper path
        "",
    ] {
        assert_eq!(
            parse_invite_url(rejected),
            None,
            "{} should not parse as an invite URL",
            rejected
        );
    }
}

// ---------------------------------------------------------------------------
// Reconnection (ADR-022)
// ---------------------------------------------------------------------------

/// Poll until the connection reports `wanted`, or `within` has passed
async fn wait_for_state(
    conn: &Connection,
    wanted: jamjam::network::ConnectionState,
    within: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + within;
    while conn.state() != wanted {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    true
}

/// Given the user is in a session
/// When the network drops for 3 seconds
/// Then reconnection is attempted, and the session continues once it succeeds
///
/// Thresholds are scaled down so the test does not sleep for seconds; the
/// behaviour under test is the transition, not the wall-clock values.
///
/// Verifies: REQ-CON-109
#[tokio::test]
async fn test_short_outage_recovers_without_asking_the_user() {
    use jamjam::network::{ConnectionState, ReconnectConfig};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // The peer stays unconnected, so its receive loop is not running and it
    // answers nothing - that is the outage.
    let mut peer = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create peer");
    let peer_addr = peer.local_addr();

    let mut conn = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");
    common::trust_each_other(&mut conn, &mut peer);
    let conn_addr = conn.local_addr();

    // Scaled down together: loss is declared after three keep-alives, so an idle
    // link cannot flap, and the whole test runs in well under a second.
    conn.set_reconnect_config(ReconnectConfig {
        keep_alive_interval: Duration::from_millis(50),
        detect_after: Duration::from_millis(150),
        give_up_after: Duration::from_millis(900),
        check_interval: Duration::from_millis(20),
    });

    let states: Arc<Mutex<Vec<ConnectionState>>> = Arc::new(Mutex::new(Vec::new()));
    let states_for_callback = states.clone();
    conn.set_state_change_callback(move |state| {
        states_for_callback.lock().unwrap().push(state);
    });

    conn.connect(peer_addr).await.expect("connect");
    assert!(conn.is_connected());

    // The peer never answers, so the link goes quiet and reconnection starts.
    // Waited for rather than slept for: on a loaded machine the check that
    // notices the silence runs late, and what matters is that it does run.
    assert!(
        wait_for_state(&conn, ConnectionState::Reconnecting, Duration::from_secs(2)).await,
        "silence must be detected as connection loss, state is {:?}",
        conn.state()
    );

    // The network comes back: the peer now answers, which starts its keep-alive
    // traffic towards us. Packets arriving is the only evidence needed.
    peer.connect(conn_addr).await.expect("peer connects back");

    // Recovery is judged by whether it happens, not by a fixed delay: the
    // liveness check runs on a timer, and a stall in this process (CPU
    // throttling, a busy build next door) makes it wake up before the receive
    // loop has counted the packets that are already waiting.
    let recovered = wait_for_state(&conn, ConnectionState::Connected, Duration::from_secs(2)).await;

    let observed = states.lock().unwrap().clone();
    assert!(
        observed.contains(&ConnectionState::Reconnecting),
        "the caller must be told the link was lost, got {:?}",
        observed
    );
    assert!(
        recovered,
        "traffic resuming must restore the session, states seen: {:?}",
        observed
    );
    assert!(
        !observed.contains(&ConnectionState::Failed),
        "a short outage must not give up and ask the user, got {:?}",
        observed
    );
}

/// Given the user is in a session
/// When the network drops for 10 seconds
/// Then automatic recovery gives up so the user can be asked, and choosing to
/// retry starts a full attempt again
///
/// Verifies: REQ-CON-110
#[tokio::test]
async fn test_long_outage_hands_the_decision_to_the_user() {
    use jamjam::network::{ConnectionState, ReconnectConfig};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let peer = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create peer");
    let peer_addr = peer.local_addr();

    let mut conn = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create connection");
    common::trust_unseen_peer(&mut conn);
    conn.set_reconnect_config(ReconnectConfig {
        keep_alive_interval: Duration::from_millis(50),
        detect_after: Duration::from_millis(100),
        give_up_after: Duration::from_millis(300),
        check_interval: Duration::from_millis(20),
    });

    let states: Arc<Mutex<Vec<ConnectionState>>> = Arc::new(Mutex::new(Vec::new()));
    let states_for_callback = states.clone();
    conn.set_state_change_callback(move |state| {
        states_for_callback.lock().unwrap().push(state);
    });

    conn.connect(peer_addr).await.expect("connect");

    // Nothing ever answers.
    tokio::time::sleep(Duration::from_millis(600)).await;

    assert_eq!(
        conn.state(),
        ConnectionState::Failed,
        "a long outage must be reported as failed so the user can decide"
    );

    let observed = states.lock().unwrap().clone();
    assert!(
        observed.contains(&ConnectionState::Reconnecting),
        "loss must be reported before giving up, got {:?}",
        observed
    );
    assert!(
        observed.contains(&ConnectionState::Failed),
        "giving up must be reported so the UI can prompt, got {:?}",
        observed
    );
    assert!(
        conn.last_error().is_some(),
        "the prompt needs a reason to show"
    );

    // The user chooses to retry: probing resumes rather than staying failed.
    conn.reconnect().expect("user-initiated retry");
    assert_eq!(conn.state(), ConnectionState::Reconnecting);

    // A connection that was closed deliberately cannot be retried.
    conn.disconnect();
    assert!(conn.reconnect().is_err());
}

/// A relay between two connections that can be cut and restored, like a link that goes down
/// for a while without either end being told. Connect both ends to the address it returns.
async fn cuttable_relay() -> (
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::net::SocketAddr,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let cut = Arc::new(AtomicBool::new(false));
    let relay = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("relay socket");
    let relay_addr = relay.local_addr().expect("relay address");
    let cut_for_relay = cut.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut ends: Vec<std::net::SocketAddr> = Vec::new();
        while let Ok((len, from)) = relay.recv_from(&mut buf).await {
            if !ends.contains(&from) {
                ends.push(from);
            }
            if cut_for_relay.load(Ordering::SeqCst) {
                continue;
            }
            if let Some(&to) = ends.iter().find(|&&end| end != from) {
                let _ = relay.send_to(&buf[..len], to).await;
            }
        }
    });
    (cut, relay_addr)
}

/// Two connected, encrypted connections with a relay between them that can be cut, scaled
/// down so an outage that outlasts giving up takes a second. Returns the sender, the listener,
/// the switch that cuts the relay, and the sequences of the audio frames the listener heard.
async fn two_connections_over_a_cuttable_relay() -> (
    Connection,
    Connection,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
) {
    use jamjam::audio::CodecType;
    use jamjam::network::{AudioEncodingConfig, ReconnectConfig};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let encoding = || AudioEncodingConfig {
        codec_type: CodecType::Pcm,
        sample_rate: 48000,
        channels: 1,
        frame_size: 8,
        bitrate: 0,
        fec_group_size: None,
    };
    let config = ReconnectConfig {
        keep_alive_interval: Duration::from_millis(50),
        detect_after: Duration::from_millis(150),
        give_up_after: Duration::from_millis(600),
        check_interval: Duration::from_millis(20),
    };

    let (cut, relay_addr) = cuttable_relay().await;

    let heard: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let mut listener = Connection::new("127.0.0.1:0").await.expect("listener");
    listener.set_audio_encoding(encoding()).expect("encoding");
    listener.set_reconnect_config(config);
    let heard_for_callback = heard.clone();
    listener.set_audio_callback(move |sequence, _payload, _timestamp| {
        heard_for_callback.lock().unwrap().push(sequence);
    });
    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender");
    common::trust_each_other(&mut sender, &mut listener);
    sender.set_audio_encoding(encoding()).expect("encoding");
    sender.set_reconnect_config(config);

    listener.connect(relay_addr).await.expect("listener");
    sender.connect(relay_addr).await.expect("sender");
    common::wait_until_encrypted(&[&sender, &listener]).await;
    (sender, listener, cut, heard)
}

/// Sends frames from `sender` until `heard` has one, or `within` has passed
async fn audio_flows(
    sender: &Connection,
    heard: &std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    within: std::time::Duration,
) -> bool {
    let before = heard.lock().unwrap().len();
    let deadline = std::time::Instant::now() + within;
    while std::time::Instant::now() < deadline {
        let _ = sender.send_audio(&[0.25; 8], 0).await;
        if heard.lock().unwrap().len() > before {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

/// Given the user is in a session
/// When the link goes down for longer than automatic recovery waits, and comes back
/// Then the audio flows again without the user doing anything
///
/// Verifies: REQ-CON-110
#[tokio::test]
async fn when_the_link_returns_after_recovery_gave_up_audio_flows_again_by_itself() {
    use jamjam::network::ConnectionState;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let (mut sender, mut listener, cut, heard) = two_connections_over_a_cuttable_relay().await;
    assert!(
        audio_flows(&sender, &heard, Duration::from_secs(2)).await,
        "the audio flows before the outage"
    );

    cut.store(true, Ordering::SeqCst);
    assert!(wait_for_state(&sender, ConnectionState::Failed, Duration::from_secs(3)).await);
    assert!(wait_for_state(&listener, ConnectionState::Failed, Duration::from_secs(3)).await);

    cut.store(false, Ordering::SeqCst);
    assert!(
        wait_for_state(&sender, ConnectionState::Connected, Duration::from_secs(3)).await,
        "the sender is still {:?} after the link came back",
        sender.state()
    );
    assert!(
        wait_for_state(
            &listener,
            ConnectionState::Connected,
            Duration::from_secs(3)
        )
        .await,
        "the listener is still {:?} after the link came back",
        listener.state()
    );
    assert!(
        audio_flows(&sender, &heard, Duration::from_secs(2)).await,
        "the audio does not flow after the link came back"
    );
    assert!(
        sender.last_error().is_none(),
        "a recovered link has no error to show"
    );

    sender.disconnect();
    listener.disconnect();
}

/// Given the link was down for longer than automatic recovery waits, and the user is asked
/// When the user chooses to reconnect
/// Then the connection is tried again: it gives up only after a full attempt that fails, and
/// the audio flows once the link is back
///
/// Verifies: REQ-CON-110
#[tokio::test]
async fn when_the_user_reconnects_after_recovery_gave_up_audio_flows_once_the_link_is_back() {
    use jamjam::network::ConnectionState;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let (mut sender, mut listener, cut, heard) = two_connections_over_a_cuttable_relay().await;

    cut.store(true, Ordering::SeqCst);
    assert!(wait_for_state(&sender, ConnectionState::Failed, Duration::from_secs(3)).await);
    assert!(wait_for_state(&listener, ConnectionState::Failed, Duration::from_secs(3)).await);

    sender.reconnect().expect("user-initiated retry");
    listener.reconnect().expect("user-initiated retry");
    assert_eq!(sender.state(), ConnectionState::Reconnecting);

    // The link is still down: nothing has been heard, so the retry does not report it up
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(sender.state(), ConnectionState::Reconnecting);

    // and it is a whole attempt, not an instant verdict
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sender.state(), ConnectionState::Reconnecting);
    assert!(wait_for_state(&sender, ConnectionState::Failed, Duration::from_secs(3)).await);

    // And a retry made while it is down does not stop the connection recovering later
    sender.reconnect().expect("user-initiated retry");
    cut.store(false, Ordering::SeqCst);
    assert!(wait_for_state(&sender, ConnectionState::Connected, Duration::from_secs(3)).await);
    assert!(
        audio_flows(&sender, &heard, Duration::from_secs(2)).await,
        "the audio does not flow after the retry"
    );

    sender.disconnect();
    listener.disconnect();
}

/// `ConnectionStats::packet_loss_rate` must come from the gaps in the received
/// audio sequence, not be a constant.
///
/// It was hardcoded to 0.0, which meant the quality classification that reads it
/// (REQ-LAT-121) could never report loss - only high RTT.
///
/// Verifies: REQ-CON-023
#[tokio::test]
async fn test_packet_loss_is_measured_not_assumed() {
    use jamjam::network::ConnectionQuality;
    use std::time::Duration;

    // Receiver collects whatever arrives; the sender drops some packets.
    let mut receiver = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create receiver");
    let receiver_addr = receiver.local_addr();
    let mut sender = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create sender");
    common::trust_each_other(&mut sender, &mut receiver);
    receiver
        .connect(sender.local_addr())
        .await
        .expect("receiver starts its loop");
    sender
        .connect(receiver_addr)
        .await
        .expect("sender connects");
    common::wait_until_encrypted(&[&sender, &receiver]).await;

    // A clean stream must report no loss.
    assert_eq!(
        receiver.stats().packet_loss_rate,
        0.0,
        "no packets yet means no loss"
    );

    // Send a contiguous run straight at the receiver's socket, then a run with a
    // gap. `send_audio` numbers packets itself, so skipping is done by sending
    // fewer than the sequence advances - use the transport directly instead.
    for sequence in [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
        // 5 and 7 never leave the sender.
        if sequence == 5 || sequence == 7 {
            continue;
        }
        let packet = jamjam::protocol::Packet::audio(sequence, sequence * 64, vec![0u8; 8]);
        sender
            .send_raw_to(&packet, receiver_addr)
            .await
            .expect("send");
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    let stats = receiver.stats();
    assert!(
        stats.packet_loss_rate > 0.0,
        "two missing packets out of ten must register as loss, got {}",
        stats.packet_loss_rate
    );
    assert!(
        stats.packet_loss_rate < 0.5,
        "loss rate {} is implausible for 2 missing out of 10",
        stats.packet_loss_rate
    );

    // The quality classification reads this figure, so it must be able to see it.
    let quality = ConnectionQuality::classify(1.0, stats.packet_loss_rate);
    assert_ne!(
        quality,
        ConnectionQuality::Good,
        "20% loss on a 1ms link must not classify as good"
    );
}

/// Audio is numbered without gaps even when control packets go out between
/// audio frames, and each FEC packet covers exactly the frames the receiver
/// files under its group - so a frame dropped anywhere in the stream is
/// recovered as itself.
///
/// Audio and control packets used to share one counter. The keep-alive sent
/// on connect and the latency info sent after it left holes in the audio
/// numbering, the sender's FEC groups drifted from the receiver's, and a
/// "recovered" frame was the XOR of unrelated frames: a burst of noise.
///
/// Verifies: REQ-AUD-026
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fec_groups_line_up_with_audio_sequences_across_control_packets() {
    use jamjam::audio::CodecType;
    use jamjam::network::{AudioEncodingConfig, FecDecoder, FecPacket};
    use jamjam::protocol::{LatencyInfoMessage, Packet, PacketType};

    const GROUP: usize = 4;
    const FRAME: usize = 8;

    let peer = common::Peer::bind();

    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender socket");
    common::trust_peer(&mut sender, &peer);
    sender
        .set_audio_encoding(AudioEncodingConfig {
            codec_type: CodecType::Pcm,
            sample_rate: 48000,
            channels: 1,
            frame_size: FRAME as u32,
            bitrate: 0,
            fec_group_size: Some(GROUP),
        })
        .expect("encoding");
    sender.connect(peer.local_addr()).await.expect("connect");
    peer.agree_keys_with(sender.local_addr());
    common::wait_until_encrypted(&[&sender]).await;

    // Two frames, a control packet in between, then the rest of two groups.
    let frame = |i: usize| vec![(i as f32 + 1.0) / 16.0; FRAME];
    for i in 0..2 {
        sender.send_audio(&frame(i), 0).await.expect("send audio");
    }
    sender
        .send_latency_info(&LatencyInfoMessage {
            capture_buffer_ms: 0.0,
            playback_buffer_ms: 0.0,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 0.0,
            frame_size: FRAME as u32,
            sample_rate: 48000,
            codec: "pcm".to_string(),
            channel_count: 2,
        })
        .await
        .expect("send latency info");
    for i in 2..GROUP * 2 {
        sender.send_audio(&frame(i), 0).await.expect("send audio");
    }

    let mut audio: Vec<Packet> = Vec::new();
    let mut fec: Vec<FecPacket> = Vec::new();
    while audio.len() < GROUP * 2 || fec.len() < 2 {
        let (packet, _) = peer.recv_packet();
        match packet.packet_type {
            PacketType::Audio => audio.push(packet),
            PacketType::Fec => fec.push(FecPacket::from_bytes(&packet.payload).expect("FEC")),
            _ => {}
        }
    }

    let sequences: Vec<u32> = audio.iter().map(|p| p.sequence).collect();
    assert_eq!(
        sequences,
        (0..(GROUP * 2) as u32).collect::<Vec<_>>(),
        "audio must be numbered from 0 without gaps"
    );

    // Drop one frame from the second group and recover it the way the
    // receiver does: data filed under sequence / group, FEC under its own
    // group number.
    let dropped = GROUP + 2;
    let mut decoder = FecDecoder::with_group_size(GROUP);
    for packet in audio.iter().filter(|p| p.sequence as usize != dropped) {
        let group = packet.sequence / GROUP as u32;
        let index = (packet.sequence % GROUP as u32) as usize;
        decoder.add_packet(group, index, &packet.payload);
    }
    let recovered: Vec<_> = fec
        .into_iter()
        .filter_map(|packet| decoder.add_fec(packet))
        .collect();

    assert_eq!(recovered.len(), 1, "exactly the dropped frame is recovered");
    let recovered = &recovered[0];
    assert_eq!(
        recovered.group_sequence as usize * GROUP + recovered.packet_index,
        dropped,
        "the recovered frame is filed under the sequence that was dropped"
    );
    assert_eq!(
        recovered.data, audio[dropped].payload,
        "the recovered frame is the one that was dropped, not a mix of others"
    );
}

/// Sends `frames` audio frames through a relay that swallows the audio frame
/// numbered `dropped`, and returns what the receiver saw: the sequences its
/// callback was handed, and its connection statistics.
async fn send_frames_through_a_relay_that_drops_one(
    fec_group_size: Option<usize>,
    frames: u32,
    dropped: Option<u32>,
) -> (Vec<u32>, jamjam::network::ConnectionStats) {
    use jamjam::audio::CodecType;
    use jamjam::network::AudioEncodingConfig;
    use jamjam::protocol::{Packet, PacketType};
    use std::sync::{Arc, Mutex};

    const FRAME: usize = 8;
    let encoding = || AudioEncodingConfig {
        codec_type: CodecType::Pcm,
        sample_rate: 48000,
        channels: 1,
        frame_size: FRAME as u32,
        bitrate: 0,
        fec_group_size,
    };

    let mut receiver = Connection::new("127.0.0.1:0").await.expect("receiver");
    receiver
        .set_audio_encoding(encoding())
        .expect("receiver encoding");
    let delivered: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let delivered_for_callback = delivered.clone();
    receiver.set_audio_callback(move |sequence, _payload, _timestamp| {
        delivered_for_callback.lock().unwrap().push(sequence);
    });
    let receiver_addr = receiver.local_addr();

    // A relay between the two that carries every packet in both directions, the keys
    // included, and swallows the audio frame numbered `dropped`. The header stays readable
    // when a packet is encrypted, which is what lets it pick that frame out.
    let relay = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("relay socket");
    let relay_addr = relay.local_addr().expect("relay address");
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut sender_addr = None;
        while let Ok((len, from)) = relay.recv_from(&mut buf).await {
            let swallowed = Packet::from_bytes(&buf[..len]).is_some_and(|packet| {
                matches!(packet.packet_type, PacketType::Audio) && Some(packet.sequence) == dropped
            });
            if swallowed {
                continue;
            }
            if from == receiver_addr {
                if let Some(to) = sender_addr {
                    let _ = relay.send_to(&buf[..len], to).await;
                }
            } else {
                sender_addr = Some(from);
                let _ = relay.send_to(&buf[..len], receiver_addr).await;
            }
        }
    });

    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender");
    common::trust_each_other(&mut sender, &mut receiver);
    receiver
        .connect(relay_addr)
        .await
        .expect("receiver starts its loop");
    sender
        .set_audio_encoding(encoding())
        .expect("sender encoding");
    sender.connect(relay_addr).await.expect("sender connects");
    common::wait_until_encrypted(&[&sender, &receiver]).await;
    for i in 0..frames {
        let frame = vec![(i as f32 + 1.0) / 16.0; FRAME];
        sender.send_audio(&frame, 0).await.expect("send audio");
    }

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let sequences = delivered.lock().unwrap().clone();
    (sequences, receiver.stats())
}

/// When a frame is lost on the way and FEC rebuilds it, the receiver counts
/// it, and the frame reaches the audio callback once, under its own number.
///
/// Verifies: REQ-TEL-010
#[tokio::test]
async fn when_fec_rebuilds_a_lost_frame_the_connection_counts_it() {
    let (sequences, stats) = send_frames_through_a_relay_that_drops_one(Some(4), 8, Some(2)).await;

    assert_eq!(stats.fec_recovered, Some(1));
    assert_eq!(sequences.iter().filter(|&&s| s == 2).count(), 1);
}

/// Verifies: REQ-TEL-010
#[tokio::test]
async fn when_nothing_is_lost_the_connection_counts_no_fec_recovery() {
    let (_, stats) = send_frames_through_a_relay_that_drops_one(Some(4), 8, None).await;

    assert_eq!(stats.fec_recovered, Some(0));
}

/// Verifies: REQ-TEL-010
#[tokio::test]
async fn when_the_link_sends_no_fec_the_connection_has_no_recovery_count() {
    let (sequences, stats) = send_frames_through_a_relay_that_drops_one(None, 8, Some(2)).await;

    assert_eq!(stats.fec_recovered, None);
    assert!(!sequences.contains(&2), "nothing rebuilds the lost frame");
}
