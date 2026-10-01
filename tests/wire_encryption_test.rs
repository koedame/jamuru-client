//! What actually leaves the socket: the packets two connections exchange are looked at on the
//! wire, not the pieces that encrypt them.
//!
//! An earlier check ran the encryption's own unit tests and called the audio encrypted, while
//! nothing sent through it. These tests put a relay between two real connections, send a known
//! signal, and read the datagrams the relay carried.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jamjam::network::{AudioEncodingConfig, Connection, LinkSecurity, Session, SessionConfig};
use jamjam::protocol::{Packet, PacketType, HEADER_SIZE};

use common::{wait_until_encrypted, Rewrite, Tap};

const FRAME: usize = 64;

/// A stereo frame no other audio would repeat: every sample different, none a round number
fn known_signal() -> Vec<f32> {
    (0..FRAME * 2)
        .map(|i| (i as f32 + 0.5) / 257.0 - 0.25)
        .collect()
}

fn bytes_of(signal: &[f32]) -> Vec<u8> {
    signal.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn contains(datagram: &[u8], needle: &[u8]) -> bool {
    datagram
        .windows(needle.len())
        .any(|window| window == needle)
}

/// What the receiving connection was handed: sequence and payload of each audio frame
type Received = Arc<Mutex<Vec<(u32, Vec<u8>)>>>;

struct Pair {
    sender: Connection,
    receiver: Connection,
    tap: Tap,
    received: Received,
}

fn encoding(fec_group_size: Option<usize>) -> AudioEncodingConfig {
    AudioEncodingConfig {
        channels: 2,
        frame_size: FRAME as u32,
        fec_group_size,
        ..Default::default()
    }
}

/// Two connections whose every packet crosses a relay, once their keys are agreed
async fn connected_pair(rewrite: Option<Rewrite>, fec_group_size: Option<usize>) -> Pair {
    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender");
    let mut receiver = Connection::new("127.0.0.1:0").await.expect("receiver");
    sender.set_audio_encoding(encoding(fec_group_size)).unwrap();
    receiver
        .set_audio_encoding(encoding(fec_group_size))
        .unwrap();
    let received: Received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    receiver.set_audio_callback(move |sequence, payload, _| {
        sink.lock().unwrap().push((sequence, payload));
    });

    let (a, b) = (sender.local_addr(), receiver.local_addr());
    let tap = match rewrite {
        Some(rewrite) => Tap::start_rewriting(a, b, rewrite).await,
        None => Tap::start(a, b).await,
    };
    sender.connect(tap.addr()).await.expect("sender connects");
    receiver
        .connect(tap.addr())
        .await
        .expect("receiver connects");
    wait_until_encrypted(&[&sender, &receiver]).await;

    Pair {
        sender,
        receiver,
        tap,
        received,
    }
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// The audio a connection sends is not on the wire as it was given, and every packet that
/// carries anything but a keep-alive is marked encrypted
///
/// Verifies: REQ-SEC-001
#[tokio::test]
async fn when_audio_is_sent_the_datagrams_on_the_wire_carry_no_readable_audio() {
    let pair = connected_pair(None, Some(4)).await;
    settle().await;
    pair.tap.forget();

    let signal = known_signal();
    let plain = bytes_of(&signal);
    for _ in 0..8 {
        pair.sender.send_audio(&signal, 0).await.unwrap();
    }
    // Long enough for the keep-alive and latency ping that go out every second as well
    tokio::time::sleep(Duration::from_millis(1300)).await;

    let datagrams = pair.tap.datagrams();
    let packets: Vec<Packet> = datagrams
        .iter()
        .map(|datagram| Packet::from_bytes(datagram).expect("a packet"))
        .collect();
    assert!(
        packets
            .iter()
            .filter(|p| p.packet_type == PacketType::Audio)
            .count()
            >= 8,
        "the audio went out"
    );
    assert!(
        packets.iter().any(|p| p.packet_type == PacketType::Fec),
        "FEC went out"
    );
    assert!(
        packets
            .iter()
            .any(|p| p.packet_type == PacketType::LatencyPing),
        "a latency ping went out"
    );
    for (datagram, packet) in datagrams.iter().zip(&packets) {
        assert!(
            !contains(datagram, &plain[..16]),
            "{:?} datagram holds the audio as it was given",
            packet.packet_type
        );
        // The key exchange holds a public key and nothing else
        assert!(
            packet.packet_type == PacketType::Control || packet.flags.encrypted,
            "{:?} packet is not marked encrypted",
            packet.packet_type
        );
        assert_eq!(
            datagram.len(),
            HEADER_SIZE + packet.payload.len(),
            "no trailing bytes"
        );
    }
    pair.tap.forget();
}

/// What the sender sent comes out the other end unchanged
///
/// Verifies: REQ-SEC-001
#[tokio::test]
async fn when_audio_is_sent_the_peer_receives_it_exactly() {
    let pair = connected_pair(None, None).await;
    let signal = known_signal();

    for _ in 0..5 {
        pair.sender.send_audio(&signal, 0).await.unwrap();
    }
    settle().await;

    let received = pair.received.lock().unwrap().clone();
    assert_eq!(received.len(), 5);
    for (index, (sequence, payload)) in received.iter().enumerate() {
        assert_eq!(*sequence as usize, index);
        assert_eq!(payload, &bytes_of(&signal));
    }
    assert_eq!(pair.receiver.security(), LinkSecurity::Encrypted);
    assert_eq!(pair.sender.stats().security, LinkSecurity::Encrypted);
}

/// The same holds in the other direction, where the peer's audio reaches us
///
/// Verifies: REQ-SEC-001
#[tokio::test]
async fn when_the_peer_sends_audio_back_it_is_encrypted_too() {
    let pair = connected_pair(None, None).await;
    let heard = Arc::new(Mutex::new(Vec::new()));
    // The receiving connection sends the signal back through the same relay
    let signal = known_signal();
    pair.tap.forget();
    for _ in 0..3 {
        pair.receiver.send_audio(&signal, 0).await.unwrap();
    }
    settle().await;
    heard.lock().unwrap().extend(pair.tap.datagrams());

    let plain = bytes_of(&signal);
    let datagrams = heard.lock().unwrap().clone();
    assert!(datagrams.len() >= 3);
    assert!(datagrams.iter().all(|d| !contains(d, &plain[..16])));
}

/// A packet changed on its way is not played, and the ones around it are
///
/// Verifies: REQ-SEC-002
#[tokio::test]
async fn when_a_packet_is_altered_on_the_wire_it_is_not_played() {
    let altered = Arc::new(Mutex::new(false));
    let flag = altered.clone();
    let sender_addr: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
    let pair = {
        let sender_addr = sender_addr.clone();
        connected_pair(
            Some(Box::new(move |from, mut datagram| {
                // Flip one bit in the audio of the packet with sequence 2, on its way to the
                // receiver
                let packet = Packet::from_bytes(&datagram);
                if let Some(packet) = packet {
                    if packet.packet_type == PacketType::Audio
                        && packet.sequence == 2
                        && Some(from) == *sender_addr.lock().unwrap()
                    {
                        let last = datagram.len() - 1;
                        datagram[last] ^= 0x01;
                        *flag.lock().unwrap() = true;
                    }
                }
                vec![datagram]
            })),
            None,
        )
        .await
    };
    *sender_addr.lock().unwrap() = Some(pair.sender.local_addr());

    for _ in 0..5 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    settle().await;

    assert!(*altered.lock().unwrap(), "the relay altered a packet");
    let sequences: Vec<u32> = pair.received.lock().unwrap().iter().map(|r| r.0).collect();
    assert_eq!(sequences, vec![0, 1, 3, 4]);
    assert!(pair.receiver.stats().packets_refused >= 1);
}

/// A header changed on its way (the timestamp, which the payload does not carry) is refused too
///
/// Verifies: REQ-SEC-002
#[tokio::test]
async fn when_the_header_of_a_packet_is_altered_on_the_wire_it_is_not_played() {
    let sender_addr: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
    let pair = {
        let sender_addr = sender_addr.clone();
        connected_pair(
            Some(Box::new(move |from, mut datagram| {
                if let Some(packet) = Packet::from_bytes(&datagram) {
                    if packet.packet_type == PacketType::Audio
                        && Some(from) == *sender_addr.lock().unwrap()
                    {
                        datagram[9] ^= 0x01; // a bit of the timestamp
                    }
                }
                vec![datagram]
            })),
            None,
        )
        .await
    };
    *sender_addr.lock().unwrap() = Some(pair.sender.local_addr());

    for _ in 0..3 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    settle().await;

    assert!(pair.received.lock().unwrap().is_empty());
}

/// A packet sent twice is played once
///
/// Verifies: REQ-SEC-002
#[tokio::test]
async fn when_a_packet_is_repeated_on_the_wire_it_is_played_once() {
    let pair = connected_pair(
        Some(Box::new(|_, datagram| vec![datagram.clone(), datagram])),
        None,
    )
    .await;

    for _ in 0..4 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    settle().await;

    let sequences: Vec<u32> = pair.received.lock().unwrap().iter().map(|r| r.0).collect();
    assert_eq!(sequences, vec![0, 1, 2, 3]);
    assert!(pair.receiver.stats().packets_refused >= 4);
}

/// Audio someone puts on the wire without the keys is not played, and does not turn the link
/// off encryption
///
/// Verifies: REQ-SEC-002
#[tokio::test]
async fn when_plain_audio_is_put_on_the_wire_after_the_keys_are_agreed_it_is_not_played() {
    let pair = connected_pair(None, None).await;

    let forged = Packet::audio(77, 0, bytes_of(&known_signal())).to_bytes();
    pair.tap.inject_to_b(&forged).await;
    let mut as_if_sealed = Packet::audio(78, 0, vec![7u8; 64]);
    as_if_sealed.flags.encrypted = true;
    pair.tap.inject_to_b(&as_if_sealed.to_bytes()).await;
    settle().await;

    assert!(pair.received.lock().unwrap().is_empty());
    assert_eq!(pair.receiver.security(), LinkSecurity::Encrypted);
    assert!(pair.receiver.stats().packets_refused >= 2);

    // The real audio still goes through
    pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    settle().await;
    assert_eq!(pair.received.lock().unwrap().len(), 1);
}

/// Nothing goes until the two have agreed keys: what is held back is not sent plain
///
/// Verifies: REQ-SEC-005
#[tokio::test]
async fn when_the_peer_has_not_answered_the_keys_no_audio_is_sent() {
    let mut sender = Connection::new("127.0.0.1:0").await.unwrap();
    sender.set_audio_encoding(encoding(None)).unwrap();
    // A peer that never says anything: a socket nobody reads
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender.connect(silent.local_addr().unwrap()).await.unwrap();

    for _ in 0..5 {
        sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(600)).await;

    assert_eq!(sender.security(), LinkSecurity::Negotiating);
    let plain = bytes_of(&known_signal());
    let mut buf = [0u8; 4096];
    let mut types = Vec::new();
    while let Ok(Ok((len, _))) =
        tokio::time::timeout(Duration::from_millis(50), silent.recv_from(&mut buf)).await
    {
        assert!(!contains(&buf[..len], &plain[..16]));
        types.push(Packet::from_bytes(&buf[..len]).unwrap().packet_type);
    }
    assert!(
        types
            .iter()
            .all(|t| matches!(t, PacketType::KeepAlive | PacketType::Control)),
        "only keep-alives and the key exchange go out before the keys are agreed: {types:?}"
    );
    assert!(types.contains(&PacketType::Control), "our key was sent");
    assert_eq!(sender.stats().packets_sent, 0);
}

/// A peer on an older version, which sends audio and pings plain and takes no key, is heard
/// and spoken to, and the connection says it is not encrypted
///
/// Verifies: REQ-SEC-004
#[tokio::test]
async fn when_the_peer_cannot_encrypt_the_link_carries_plain_packets_and_says_so() {
    let mut conn = Connection::new("127.0.0.1:0").await.unwrap();
    conn.set_audio_encoding(encoding(None)).unwrap();
    let heard: Received = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    conn.set_audio_callback(move |sequence, payload, _| {
        sink.lock().unwrap().push((sequence, payload));
    });
    let old_app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    conn.connect(old_app.local_addr().unwrap()).await.unwrap();
    assert_eq!(conn.security(), LinkSecurity::Negotiating);

    // What an older app sends first: a keep-alive, then a ping and its audio, all plain
    let to = conn.local_addr();
    old_app
        .send_to(&Packet::keep_alive(0).to_bytes(), to)
        .await
        .unwrap();
    let ping = jamjam::protocol::LatencyPing {
        sent_time_us: 1,
        ping_sequence: 1,
    };
    old_app
        .send_to(&Packet::latency_ping(1, &ping).to_bytes(), to)
        .await
        .unwrap();
    old_app
        .send_to(
            &Packet::audio(0, 0, bytes_of(&known_signal())).to_bytes(),
            to,
        )
        .await
        .unwrap();
    settle().await;

    assert_eq!(conn.security(), LinkSecurity::Unencrypted);
    assert_eq!(conn.stats().security, LinkSecurity::Unencrypted);
    assert_eq!(heard.lock().unwrap().len(), 1, "its audio is played");

    // And it hears us, plain, as it expects
    conn.send_audio(&known_signal(), 0).await.unwrap();
    let mut buf = [0u8; 4096];
    let mut got_audio = false;
    while let Ok(Ok((len, _))) =
        tokio::time::timeout(Duration::from_millis(300), old_app.recv_from(&mut buf)).await
    {
        let packet = Packet::from_bytes(&buf[..len]).unwrap();
        if packet.packet_type == PacketType::Audio {
            assert!(!packet.flags.encrypted);
            assert_eq!(packet.payload, bytes_of(&known_signal()));
            got_audio = true;
        }
    }
    assert!(got_audio);
}

/// Someone who has learned our port cannot make the link give up encrypting by sending plain
/// audio to it
///
/// Verifies: REQ-SEC-004
#[tokio::test]
async fn when_a_stranger_sends_plain_audio_the_link_keeps_negotiating() {
    let mut conn = Connection::new("127.0.0.1:0").await.unwrap();
    conn.set_audio_encoding(encoding(None)).unwrap();
    let heard: Received = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    conn.set_audio_callback(move |sequence, payload, _| {
        sink.lock().unwrap().push((sequence, payload));
    });
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    conn.connect(peer.local_addr().unwrap()).await.unwrap();

    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger
        .send_to(
            &Packet::audio(0, 0, bytes_of(&known_signal())).to_bytes(),
            conn.local_addr(),
        )
        .await
        .unwrap();
    settle().await;

    assert_eq!(conn.security(), LinkSecurity::Negotiating);
    assert!(heard.lock().unwrap().is_empty());
}

/// A key from someone other than the peer is not taken
///
/// Verifies: REQ-SEC-004
#[tokio::test]
async fn when_a_stranger_sends_a_key_it_is_not_taken() {
    let mut conn = Connection::new("127.0.0.1:0").await.unwrap();
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    conn.connect(peer.local_addr().unwrap()).await.unwrap();

    let stranger = common::Peer::bind();
    stranger.send_key_exchange(conn.local_addr());
    settle().await;

    assert_eq!(conn.security(), LinkSecurity::Negotiating);
}

/// A session of several peers encrypts what it sends to each, with a key for each
///
/// Verifies: REQ-SEC-001
#[tokio::test]
async fn when_a_session_broadcasts_each_peer_gets_audio_it_alone_can_open() {
    let ids = [
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
    ];
    let mut sessions = Vec::new();
    for _ in 0..3 {
        sessions.push(Session::new(SessionConfig::default()).await.unwrap());
    }
    let heard: Vec<Arc<Mutex<Vec<f32>>>> = (0..3).map(|_| Default::default()).collect();
    for (session, heard) in sessions.iter_mut().zip(&heard) {
        let sink = heard.clone();
        session.set_peer_audio_callback(move |_, samples, _| {
            sink.lock().unwrap().extend_from_slice(samples)
        });
    }
    let addr_of = |session: &Session| -> SocketAddr {
        format!("127.0.0.1:{}", session.local_addr().port())
            .parse()
            .unwrap()
    };
    let addrs: Vec<SocketAddr> = sessions.iter().map(addr_of).collect();
    let peer_info = |index: usize| jamjam::network::PeerInfo {
        id: ids[index],
        name: format!("peer-{index}"),
        candidates: vec![],
        public_addr: None,
        local_addr: None,
        joined_at: 0,
        features: vec![],
    };
    for (i, session) in sessions.iter().enumerate() {
        for j in (0..3).filter(|&j| j != i) {
            session.add_peer(peer_info(j), addrs[j]).await.unwrap();
        }
    }
    for session in &mut sessions {
        session.start();
    }

    // Keys are agreed pairwise; wait for every pair
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut all = true;
        for (i, session) in sessions.iter().enumerate() {
            for j in (0..3).filter(|&j| j != i) {
                all &= session.peer_security(ids[j]).await == Some(LinkSecurity::Encrypted);
            }
        }
        if all {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the keys were not agreed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let signal = known_signal();
    sessions[0].broadcast_audio(&signal, 0).await.unwrap();
    settle().await;

    assert!(heard[0].lock().unwrap().is_empty(), "not sent to itself");
    assert_eq!(*heard[1].lock().unwrap(), signal);
    assert_eq!(*heard[2].lock().unwrap(), signal);
}
