//! How many times a participant who is not heard can have the links of the others started
//! over by publishing other addresses.
//!
//! The roster decides, the audio carries it out. Here the audio is real: every `Audio::Start`
//! opens a connection on loopback, every `Audio::End` drops it, and the addresses the
//! participant publishes are sockets this test counts what arrives at.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use jamjam::network::roster::{Audio, Roster};
use jamjam::network::{AddressCandidate, Connection, LinkIdentity, Opened, PeerInfo, SecureLink};
use jamjam::protocol::Packet;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// How many ports on loopback the participant's published addresses and the ports guessed
/// around them take
const PORTS: u16 = 41;

/// The distance between the two sets of addresses the participant publishes in turn
const SET_APART: u16 = 20;

/// How long the test waits between two publishes: a link opens its own NAT for 0.4 s and probes
/// for a second before it falls back, and then takes about as long again to guess the ports
const BETWEEN_PUBLISHES: Duration = Duration::from_millis(3000);

/// Ports on loopback with something counting what arrives
struct Listeners {
    base: u16,
    arrived: Vec<Arc<AtomicUsize>>,
}

impl Listeners {
    /// Ports on loopback that count what arrives; the first port of each set also answers the
    /// key exchange of whoever talks to it, with `link`, as the participant would
    async fn bind(link: Arc<SecureLink>) -> Self {
        'search: loop {
            let base = 20000 + rand::random::<u16>() % 30000;
            let mut sockets = Vec::new();
            for port in base..base + PORTS {
                match UdpSocket::bind(("127.0.0.1", port)).await {
                    Ok(socket) => sockets.push(socket),
                    Err(_) => continue 'search,
                }
            }
            let arrived: Vec<Arc<AtomicUsize>> = sockets
                .into_iter()
                .enumerate()
                .map(|(offset, socket)| {
                    let count = Arc::new(AtomicUsize::new(0));
                    let counting = count.clone();
                    let answers = [0, usize::from(SET_APART)].contains(&offset);
                    let link = link.clone();
                    tokio::spawn(async move {
                        let mut buf = [0u8; 2048];
                        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                            counting.fetch_add(1, Ordering::SeqCst);
                            let Some(packet) = Packet::from_bytes(&buf[..len]) else {
                                continue;
                            };
                            if let (true, Opened::KeyExchange { answer: true }) =
                                (answers, link.open(packet))
                            {
                                if let Some(half) = link.key_exchange_packet() {
                                    let _ = socket.send_to(&half.to_bytes(), from).await;
                                }
                            }
                        }
                    });
                    count
                })
                .collect();
            return Self { base, arrived };
        }
    }

    /// The two addresses of set `which` (0 or 1): the next free port after them is guessed
    fn set(&self, which: u16) -> [SocketAddr; 2] {
        let first = self.base + which * SET_APART;
        [first, first + 1].map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
    }

    /// How many packets arrived at ports nobody published: the ports that are guessed
    fn guessed_at(&self) -> usize {
        (0..PORTS)
            .filter(|offset| ![0, 1, SET_APART, SET_APART + 1].contains(offset))
            .map(|offset| self.arrived[usize::from(offset)].load(Ordering::SeqCst))
            .sum()
    }
}

fn participant(addresses: [SocketAddr; 2], link_key: &str) -> PeerInfo {
    PeerInfo {
        id: Uuid::from_u128(2),
        name: "other".into(),
        candidates: addresses
            .into_iter()
            .map(AddressCandidate::server_reflexive)
            .collect(),
        public_addr: None,
        local_addr: None,
        joined_at: 0,
        features: vec![],
        link_key: Some(link_key.to_string()),
    }
}

/// Carries out what the roster says, on real connections that nobody answers
#[derive(Default)]
struct Audios {
    links: HashMap<Uuid, JoinHandle<()>>,
    started: usize,
}

impl Audios {
    fn carry_out(&mut self, audio: Vec<Audio>) {
        for step in audio {
            match step {
                Audio::Start {
                    peer,
                    candidates,
                    link_key,
                } => {
                    self.started += 1;
                    let key = link_key.expect("the room lists a key");
                    self.links.insert(
                        peer,
                        tokio::spawn(async move {
                            let mut connection =
                                Connection::new("127.0.0.1:0").await.expect("socket");
                            connection
                                .verify_peer(&LinkIdentity::generate(), &key)
                                .expect("a key");
                            connection.predict_ports_on_loopback();
                            let _ = connection.connect_with_candidates(&candidates).await;
                            // The connection is dropped, and ends, with the task
                            std::future::pending::<()>().await;
                        }),
                    );
                }
                Audio::End { peer } => {
                    if let Some(link) = self.links.remove(&peer) {
                        link.abort();
                    }
                }
                _ => {}
            }
        }
    }
}

impl Drop for Audios {
    fn drop(&mut self) {
        for link in self.links.values() {
            link.abort();
        }
    }
}

/// Verifies: REQ-CON-179
#[tokio::test(flavor = "multi_thread")]
async fn the_link_to_a_participant_not_heard_is_started_over_a_few_times_at_most_when_they_publish_other_addresses(
) {
    let identity = LinkIdentity::generate();
    let link = Arc::new(SecureLink::answering(&identity));
    let key = identity.public_key();
    let listeners = Listeners::bind(link).await;
    let sets = [listeners.set(0), listeners.set(1)];
    let mut roster = Roster::default();
    let mut audios = Audios::default();
    audios.carry_out(roster.entered(Uuid::from_u128(1), vec![participant(sets[0], &key)]));

    let publishes = 9;
    let mut guessed_after = Vec::new();
    let mut started_after = Vec::new();
    for i in 0..publishes {
        tokio::time::sleep(BETWEEN_PUBLISHES).await;
        audios.carry_out(roster.updated(participant(sets[(i + 1) % 2], &key)));
        guessed_after.push(listeners.guessed_at());
        started_after.push(audios.started);
    }
    tokio::time::sleep(BETWEEN_PUBLISHES).await;
    let guessed_at_end = listeners.guessed_at();
    eprintln!(
        "links started after each publish: {started_after:?}; packets to guessed ports after each publish: {guessed_after:?}, at the end: {guessed_at_end}"
    );

    // The link started by the last publish the roster follows has had time to finish guessing
    // by the publish three after it
    assert_eq!(
        started_after[publishes - 1],
        started_after[publishes - 4],
        "links went on being started: {started_after:?}"
    );
    assert_eq!(
        guessed_at_end,
        guessed_after[publishes - 4],
        "packets went on going to guessed ports: {guessed_after:?}"
    );
    assert!(
        started_after[publishes - 1] <= 6,
        "more than the first link and 5 started over: {started_after:?}"
    );
}
