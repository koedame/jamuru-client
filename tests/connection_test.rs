//! Connection tests based on docs-spec/behavior/connection.feature
//!
//! Tests for session connection functionality.

use jamjam::network::{Connection, Session, SessionConfig};

/// Test: Create a session
/// Given jamjam application is running
/// When user selects "Create Session"
/// Then session is created
#[tokio::test]
async fn test_create_session() {
    let config = SessionConfig::default();
    let session = Session::new(config)
        .await
        .expect("Failed to create session");

    // Local peer ID should be generated
    let peer_id = session.local_peer_id();
    assert!(!peer_id.is_nil(), "Peer ID should not be nil");

    // Local address should be valid
    let addr = session.local_addr();
    assert!(addr.port() > 0, "Local port should be assigned");
}

/// Test: Session configuration
/// Given default session configuration
/// Then max_peers is 10
///
/// Verifies: REQ-CON-020
#[tokio::test]
async fn test_session_config() {
    let config = SessionConfig::default();

    // Check max participants limit
    assert_eq!(config.max_peers, 10, "Default max peers should be 10");
    assert!(config.enable_mixing, "Mixing should be enabled by default");
}

/// Test: Custom session configuration
/// When max_peers is set to 5
/// Then session allows up to 5 participants
#[tokio::test]
async fn test_custom_session_config() {
    let config = SessionConfig {
        local_port: 0,
        max_peers: 5,
        enable_mixing: true,
    };

    let session = Session::new(config)
        .await
        .expect("Failed to create session");
    let peers = session.peers().await;
    assert!(peers.is_empty(), "Initial peers should be empty");
}

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

/// Test: Session can be recreated on same port after stop
/// This verifies SO_REUSEADDR is working correctly
#[tokio::test]
async fn test_session_recreate_same_port() {
    // Create first session with auto-assigned port
    let config1 = SessionConfig::default();
    let mut session1 = Session::new(config1)
        .await
        .expect("Failed to create first session");
    session1.start();

    let port = session1.local_addr().port();

    // Stop and drop the first session
    session1.stop();
    drop(session1);

    // Small delay to ensure cleanup
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create second session on the same port
    let config2 = SessionConfig {
        local_port: port,
        max_peers: 10,
        enable_mixing: true,
    };

    let session2 = Session::new(config2).await;
    assert!(
        session2.is_ok(),
        "Should be able to create session on same port after stop: {:?}",
        session2.err()
    );
    assert_eq!(session2.unwrap().local_addr().port(), port);
}

/// Test: Multiple session create/stop cycles on same port
#[tokio::test]
async fn test_session_repeated_recreate() {
    // Get an available port
    let config = SessionConfig::default();
    let session = Session::new(config)
        .await
        .expect("Failed to create initial session");
    let port = session.local_addr().port();
    drop(session);

    // Perform multiple create/stop cycles
    for i in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let config = SessionConfig {
            local_port: port,
            max_peers: 10,
            enable_mixing: true,
        };

        let session = Session::new(config).await;
        assert!(
            session.is_ok(),
            "Session creation cycle {} should succeed: {:?}",
            i + 1,
            session.err()
        );

        let mut s = session.unwrap();
        s.start();
        s.stop();
        drop(s);
    }
}

// ---------------------------------------------------------------------------
// Invite links and direct connection
// ---------------------------------------------------------------------------

/// Given the creator has created a room
/// When the user opens the invite URL "jamjam://join/ABC234"
/// Then the room code is recovered so the join can start
///
/// The OS-level registration that hands the URL to the app is platform plumbing;
/// what this covers is that a link the app produced round-trips, and that a link
/// it did not produce is rejected rather than guessed at.
///
/// Verifies: REQ-CON-103
#[test]
fn test_invite_url_round_trips() {
    use jamjam::network::{generate_invite_code, invite_url, parse_invite_url};

    // A link the app built must parse back to the same code.
    for _ in 0..20 {
        let code = generate_invite_code();
        let url = invite_url(&code);
        assert!(
            url.starts_with("jamjam://join/"),
            "unexpected invite URL: {}",
            url
        );
        assert_eq!(parse_invite_url(&url), Some(code));
    }

    // A lower-cased link still works: mail clients and chat apps do this.
    assert_eq!(
        parse_invite_url("jamjam://join/abc234"),
        Some("ABC234".to_string())
    );

    // Query strings and a trailing slash are tolerated.
    assert_eq!(
        parse_invite_url("jamjam://join/ABC234?from=chat"),
        Some("ABC234".to_string())
    );
    assert_eq!(
        parse_invite_url("jamjam://join/ABC234/"),
        Some("ABC234".to_string())
    );

    // Anything else must be refused rather than half-interpreted.
    for rejected in [
        "https://example.com/join/ABC234", // wrong scheme
        "jamjam://leave/ABC234",           // wrong action
        "jamjam://join/ABC",               // too short
        "jamjam://join/ABC2345",           // too long
        "jamjam://join/ABC01I",            // excluded confusing characters
        "jamjam://join/",                  // no code
        "jamjam://join/ABC234/extra",      // deeper path
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

/// Given the peer is listening on a port
/// When the user connects by IP address and port
/// Then the connection is established without a signaling server
///
/// Verifies: REQ-CON-107
#[tokio::test]
async fn test_direct_connection_without_signaling() {
    // Two sockets on loopback, connected by address alone.
    let listener = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create listener");
    let listener_addr = listener.local_addr();
    assert!(listener_addr.port() > 0, "listener needs a real port");

    let mut caller = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create caller");
    assert!(!caller.is_connected(), "must start disconnected");

    caller
        .connect(listener_addr)
        .await
        .expect("direct connection by address should succeed");

    assert!(
        caller.is_connected(),
        "connection state must reflect the direct connect"
    );

    // Audio can be sent straight away: no signaling exchange was involved.
    caller
        .send_audio(&[0.25f32; 64], 0)
        .await
        .expect("send over a directly established connection");
    assert!(
        caller.stats().packets_sent > 0,
        "the packet must be counted"
    );
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
/// retry resumes probing
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
        "a long outage must stop retrying so the user can decide"
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
    receiver
        .connect(receiver_addr)
        .await
        .expect("receiver starts its loop");

    let sender = Connection::new("127.0.0.1:0")
        .await
        .expect("Failed to create sender");

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
#[tokio::test]
async fn fec_groups_line_up_with_audio_sequences_across_control_packets() {
    use jamjam::audio::CodecType;
    use jamjam::network::{AudioEncodingConfig, FecDecoder, FecPacket};
    use jamjam::protocol::{LatencyInfoMessage, Packet, PacketType};

    const GROUP: usize = 4;
    const FRAME: usize = 8;

    let peer = std::net::UdpSocket::bind("127.0.0.1:0").expect("peer socket");
    peer.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .expect("read timeout");

    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender socket");
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
    sender
        .connect(peer.local_addr().expect("peer address"))
        .await
        .expect("connect");

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
    let mut buf = [0u8; 2048];
    while audio.len() < GROUP * 2 || fec.len() < 2 {
        let len = peer.recv(&mut buf).expect("the sender's packets arrive");
        let packet = Packet::from_bytes(&buf[..len]).expect("a valid packet");
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
    receiver
        .connect(receiver_addr)
        .await
        .expect("receiver starts its loop");

    let relay = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("relay socket");
    let relay_addr = relay.local_addr().expect("relay address");
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((len, _)) = relay.recv_from(&mut buf).await {
            let swallowed = Packet::from_bytes(&buf[..len]).is_some_and(|packet| {
                matches!(packet.packet_type, PacketType::Audio) && Some(packet.sequence) == dropped
            });
            if !swallowed {
                let _ = relay.send_to(&buf[..len], receiver_addr).await;
            }
        }
    });

    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender");
    sender
        .set_audio_encoding(encoding())
        .expect("sender encoding");
    sender.connect(relay_addr).await.expect("sender connects");
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
