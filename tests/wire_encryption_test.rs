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

use jamjam::network::{
    AudioEncodingConfig, Connection, LinkIdentity, LinkSecurity, Session, SessionConfig,
};
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
    let heard = broadcast_among_three(false).await;

    assert!(heard[0].is_empty(), "not sent to itself");
    assert_eq!(heard[1], known_signal());
    assert_eq!(heard[2], known_signal());
}

/// The same among participants who know each other by the keys the server gave
///
/// Verifies: REQ-SEC-001, REQ-SEC-007
#[tokio::test]
async fn when_a_session_checks_its_peers_each_gets_audio_it_alone_can_open() {
    let heard = broadcast_among_three(true).await;

    assert!(heard[0].is_empty(), "not sent to itself");
    assert_eq!(heard[1], known_signal());
    assert_eq!(heard[2], known_signal());
}

/// What each of three sessions heard when the first broadcast the known signal, once the keys
/// among all three were agreed
async fn broadcast_among_three(check_peers: bool) -> Vec<Vec<f32>> {
    let identities: Vec<LinkIdentity> = (0..3).map(|_| LinkIdentity::generate()).collect();
    let ids = [
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
    ];
    let mut sessions = Vec::new();
    for identity in &identities {
        let mut session = Session::new(SessionConfig::default()).await.unwrap();
        if check_peers {
            session.set_link_identity(identity.clone());
        }
        sessions.push(session);
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
        link_key: check_peers.then(|| identities[index].public_key()),
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

    heard
        .iter()
        .map(|heard| heard.lock().unwrap().clone())
        .collect()
}

/// Two connections whose every packet crosses a relay, connected through it when this returns.
/// With `keys`, they know each other by the keys a server would have given them (the sender's
/// and the receiver's); without, as before the exchange was signed, they do not. `rewrite` is
/// given the sender's address and makes what the relay does to each datagram.
struct RelayedPair {
    sender: Connection,
    receiver: Connection,
    tap: Tap,
    received: Received,
}

async fn relayed_pair(
    keys: Option<(&LinkIdentity, &LinkIdentity)>,
    rewrite: impl FnOnce(SocketAddr) -> Rewrite,
) -> RelayedPair {
    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender");
    let mut receiver = Connection::new("127.0.0.1:0").await.expect("receiver");
    sender.set_audio_encoding(encoding(None)).unwrap();
    receiver.set_audio_encoding(encoding(None)).unwrap();
    if let Some((sender_key, receiver_key)) = keys {
        sender
            .verify_peer(sender_key, Some(&receiver_key.public_key()))
            .unwrap();
        receiver
            .verify_peer(receiver_key, Some(&sender_key.public_key()))
            .unwrap();
    }
    let received: Received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    receiver.set_audio_callback(move |sequence, payload, _| {
        sink.lock().unwrap().push((sequence, payload));
    });

    let (a, b) = (sender.local_addr(), receiver.local_addr());
    let tap = Tap::start_rewriting(a, b, rewrite(a)).await;
    sender.connect(tap.addr()).await.expect("sender connects");
    receiver
        .connect(tap.addr())
        .await
        .expect("receiver connects");
    RelayedPair {
        sender,
        receiver,
        tap,
        received,
    }
}

fn pass_on(_: SocketAddr) -> Rewrite {
    Box::new(|_, datagram| vec![datagram])
}

/// A relay that sits between the two ends the way someone who can alter packets on the path
/// would: it makes a key of its own towards each end, agrees keys with each, and passes on
/// what it opens - listening to the audio on its way. What it overheard is in `heard`.
struct Interloper {
    sender_addr: SocketAddr,
    /// The relay's end of the link to the sender, and to the receiver
    towards_sender: jamjam::network::SecureLink,
    towards_receiver: jamjam::network::SecureLink,
    heard: Mutex<Vec<Vec<u8>>>,
}

impl Interloper {
    /// The ephemeral key in a key exchange of either kind, put in the form an unsigned link takes
    fn as_unsigned(packet: &Packet) -> Packet {
        let mut payload = vec![0x01, 0x01];
        payload.extend_from_slice(&packet.payload[2..34]);
        Packet::control(0, payload)
    }

    /// What goes on to the other end for `datagram`, which came from `from`
    fn handle(&self, from: SocketAddr, datagram: Vec<u8>) -> Vec<Vec<u8>> {
        let Some(packet) = Packet::from_bytes(&datagram) else {
            return vec![];
        };
        let (listening, speaking) = if from == self.sender_addr {
            (&self.towards_sender, &self.towards_receiver)
        } else {
            (&self.towards_receiver, &self.towards_sender)
        };
        if packet.packet_type == PacketType::Control {
            if packet.payload.len() < 34 {
                return vec![];
            }
            // Take the end's key, and give the other end the relay's own in its place
            let _ = listening.open(Self::as_unsigned(&packet));
            return vec![speaking.key_exchange_packet().to_bytes()];
        }
        match listening.open(packet) {
            jamjam::network::Opened::Packet(plain) => {
                if plain.packet_type == PacketType::Audio {
                    self.heard.lock().unwrap().push(plain.payload.clone());
                }
                speaking
                    .seal(plain)
                    .map(|sealed| vec![sealed.to_bytes()])
                    .unwrap_or_default()
            }
            _ => vec![],
        }
    }
}

fn interloper(sender_addr: SocketAddr) -> Arc<Interloper> {
    Arc::new(Interloper {
        sender_addr,
        towards_sender: jamjam::network::SecureLink::new(),
        towards_receiver: jamjam::network::SecureLink::new(),
        heard: Mutex::new(Vec::new()),
    })
}

/// Two ends that know each other by their keys agree keys over a relay that changes nothing,
/// and the audio arrives
///
/// Verifies: REQ-SEC-007
#[tokio::test]
async fn when_both_ends_check_the_peer_and_nothing_interferes_audio_arrives_encrypted() {
    let (sender_key, receiver_key) = (LinkIdentity::generate(), LinkIdentity::generate());
    let pair = relayed_pair(Some((&sender_key, &receiver_key)), pass_on).await;
    wait_until_encrypted(&[&pair.sender, &pair.receiver]).await;
    assert!(pair.sender.checks_peer() && pair.receiver.checks_peer());

    for _ in 0..3 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    settle().await;

    assert_eq!(pair.received.lock().unwrap().len(), 3);
    let plain = bytes_of(&known_signal());
    assert!(pair
        .tap
        .datagrams()
        .iter()
        .all(|d| !contains(d, &plain[..16])));
}

/// What the relay above is able to do, shown against ends that do not know each other: it
/// agrees keys with each, the receiver hears the sender's audio as sent, and the relay has
/// listened to all of it. This is the case the other ends are not in.
///
/// Verifies: REQ-SEC-008
#[tokio::test]
async fn when_the_ends_do_not_check_the_peer_a_relay_in_the_exchange_hears_the_audio() {
    let spy = Arc::new(Mutex::new(None));
    let pair = relayed_pair(None, |sender_addr| {
        let relay = interloper(sender_addr);
        *spy.lock().unwrap() = Some(relay.clone());
        Box::new(move |from, datagram| relay.handle(from, datagram))
    })
    .await;
    wait_until_encrypted(&[&pair.sender, &pair.receiver]).await;
    assert!(!pair.sender.checks_peer() && !pair.receiver.checks_peer());

    for _ in 0..3 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    settle().await;

    let relay = spy.lock().unwrap().clone().unwrap();
    let heard = relay.heard.lock().unwrap().clone();
    assert_eq!(heard.len(), 3, "the relay listened to the audio");
    assert!(heard
        .iter()
        .all(|payload| *payload == bytes_of(&known_signal())));
    assert_eq!(
        pair.received.lock().unwrap().len(),
        3,
        "and the receiver heard it, none the wiser"
    );
}

/// The same relay against ends that know each other by their keys does not get between them:
/// neither takes the relay's key, no audio is sent, and the relay has heard nothing
///
/// Verifies: REQ-SEC-007
#[tokio::test]
async fn when_a_relay_puts_its_own_key_in_the_exchange_the_ends_do_not_agree_keys() {
    let (sender_key, receiver_key) = (LinkIdentity::generate(), LinkIdentity::generate());
    let spy = Arc::new(Mutex::new(None));
    let pair = relayed_pair(Some((&sender_key, &receiver_key)), |sender_addr| {
        let relay = interloper(sender_addr);
        *spy.lock().unwrap() = Some(relay.clone());
        Box::new(move |from, datagram| relay.handle(from, datagram))
    })
    .await;

    for _ in 0..5 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(pair.sender.security(), LinkSecurity::Negotiating);
    assert_eq!(pair.receiver.security(), LinkSecurity::Negotiating);
    assert!(pair.sender.stats().packets_refused >= 1);
    assert!(pair.receiver.stats().packets_refused >= 1);
    assert!(pair.received.lock().unwrap().is_empty());
    assert_eq!(pair.sender.stats().packets_sent, 0, "no audio was sent");
    let relay = spy.lock().unwrap().clone().unwrap();
    assert!(relay.heard.lock().unwrap().is_empty());
}

/// A relay that signs with a key of its own, not the one the server gave for the other end,
/// fares no better
///
/// Verifies: REQ-SEC-007
#[tokio::test]
async fn when_a_relay_signs_the_exchange_with_a_key_the_server_did_not_give_it_is_not_taken() {
    let (sender_key, receiver_key) = (LinkIdentity::generate(), LinkIdentity::generate());
    let relay_key = LinkIdentity::generate();
    let (as_sender, as_receiver) = (
        jamjam::network::SecureLink::for_peer(Some(&relay_key), Some(&receiver_key.public_key())),
        jamjam::network::SecureLink::for_peer(Some(&relay_key), Some(&sender_key.public_key())),
    );
    let (as_sender, as_receiver) = (Arc::new(as_sender), Arc::new(as_receiver));
    let pair = relayed_pair(Some((&sender_key, &receiver_key)), |sender_addr| {
        Box::new(move |from, datagram| match Packet::from_bytes(&datagram) {
            Some(packet) if packet.packet_type == PacketType::Control => {
                let forged = if from == sender_addr {
                    &as_sender
                } else {
                    &as_receiver
                };
                vec![forged.key_exchange_packet().to_bytes()]
            }
            _ => vec![datagram],
        })
    })
    .await;

    for _ in 0..5 {
        pair.sender.send_audio(&known_signal(), 0).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(pair.sender.security(), LinkSecurity::Negotiating);
    assert_eq!(pair.receiver.security(), LinkSecurity::Negotiating);
    assert!(pair.received.lock().unwrap().is_empty());
    assert_eq!(pair.sender.stats().packets_sent, 0);
}
