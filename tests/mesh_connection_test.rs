//! A room of more than two: every participant has one audio socket, told to the room once,
//! and a link to each of the others through it.
//!
//! These tests run real connections over real sockets on loopback, one socket per
//! participant, and look at what each link was handed.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::network::{
    AudioEncodingConfig, Connection, ConnectionState, LinkIdentity, UdpMux, MAX_PEERS_PER_ROOM,
};

use common::wait_until_encrypted;

const FRAME: usize = 64;

/// A stereo frame that says who sent it: every sample is the sender's number
fn signal_of(sender: usize) -> Vec<f32> {
    vec![sender as f32 + 0.5; FRAME * 2]
}

fn bytes_of(signal: &[f32]) -> Vec<u8> {
    signal.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// The payloads one link was handed, in the order they came
type Heard = Arc<Mutex<Vec<Vec<u8>>>>;

/// One participant: its socket, its key in the room, and its links by the number of the
/// participant each goes to
struct Participant {
    mux: UdpMux,
    identity: LinkIdentity,
    links: BTreeMap<usize, Connection>,
    heard: BTreeMap<usize, Heard>,
}

impl Participant {
    async fn enter() -> Self {
        Self {
            mux: UdpMux::bind("127.0.0.1:0").await.expect("audio socket"),
            identity: LinkIdentity::generate(),
            links: BTreeMap::new(),
            heard: BTreeMap::new(),
        }
    }

    fn addr(&self) -> SocketAddr {
        self.mux.local_addr()
    }

    /// A link to participant `to`, whose key in the room is `their_key`, not yet connected
    fn link_to(&mut self, to: usize, their_key: &str) -> &mut Connection {
        let link = self.new_link(to, their_key);
        self.links.entry(to).or_insert(link)
    }

    /// [`Participant::link_to`], handed to the caller to connect and keep
    fn new_link(&mut self, to: usize, their_key: &str) -> Connection {
        let mut link = Connection::on(&self.mux).expect("link");
        link.verify_peer(&self.identity, Some(their_key))
            .expect("peer key");
        link.set_audio_encoding(AudioEncodingConfig {
            channels: 2,
            frame_size: FRAME as u32,
            fec_group_size: None,
            ..Default::default()
        })
        .unwrap();
        let heard: Heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        link.set_audio_callback(move |_, payload, _| sink.lock().unwrap().push(payload));
        self.heard.insert(to, heard);
        link
    }

    fn heard_from(&self, from: usize) -> Vec<Vec<u8>> {
        self.heard[&from].lock().unwrap().clone()
    }
}

/// `count` participants, each linked to every other one, with the keys of every link agreed
async fn room_of(count: usize) -> Vec<Participant> {
    let mut room = Vec::new();
    for _ in 0..count {
        room.push(Participant::enter().await);
    }
    let known: Vec<(SocketAddr, String)> = room
        .iter()
        .map(|p| (p.addr(), p.identity.public_key()))
        .collect();
    for (me, participant) in room.iter_mut().enumerate() {
        for (other, (addr, key)) in known.iter().enumerate() {
            if other != me {
                participant
                    .link_to(other, key)
                    .connect(*addr)
                    .await
                    .expect("link connects");
            }
        }
    }
    let links: Vec<&Connection> = room.iter().flat_map(|p| p.links.values()).collect();
    wait_until_encrypted(&links).await;
    room
}

async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Everyone sends one frame of their own signal to everyone else
async fn everyone_sends(room: &[Participant]) {
    for (me, participant) in room.iter().enumerate() {
        for link in participant.links.values() {
            link.send_audio(&signal_of(me), 0).await.expect("send");
        }
    }
}

/// The bug this guards: with three in a room, the third was heard by no one and heard no
/// one, because an app kept a link to one participant only.
///
/// Verifies: REQ-CON-133
/// Verifies: REQ-CON-033
#[tokio::test]
async fn three_in_a_room_each_hears_both_others_on_the_link_to_each() {
    let room = room_of(3).await;

    everyone_sends(&room).await;

    for (me, participant) in room.iter().enumerate() {
        for other in (0..3).filter(|&other| other != me) {
            until("the frame arrived", || {
                !participant.heard_from(other).is_empty()
            })
            .await;
            assert_eq!(
                participant.heard_from(other),
                vec![bytes_of(&signal_of(other))],
                "participant {me} should hear exactly what {other} sent on the link to {other}"
            );
        }
    }
}

/// Verifies: REQ-CON-020
/// Verifies: REQ-CON-133
#[tokio::test]
async fn a_full_room_of_ten_has_nine_links_per_participant_and_all_of_them_carry_audio() {
    let room = room_of(MAX_PEERS_PER_ROOM).await;
    assert!(room.iter().all(|p| p.links.len() == 9));

    everyone_sends(&room).await;

    for (me, participant) in room.iter().enumerate() {
        for other in (0..MAX_PEERS_PER_ROOM).filter(|&other| other != me) {
            until("the frame arrived", || {
                !participant.heard_from(other).is_empty()
            })
            .await;
            assert_eq!(
                participant.heard_from(other),
                vec![bytes_of(&signal_of(other))]
            );
        }
    }
}

/// Each link has keys of its own, so what one participant is sent cannot be opened by the
/// link to another, even though both arrive on the same socket.
///
/// Verifies: REQ-CON-133
#[tokio::test]
async fn audio_sealed_for_one_participant_is_not_played_by_another_it_is_sent_to() {
    let mut room = room_of(3).await;

    // Participant 2 sends what it sealed for participant 1 to participant 0 instead.
    let to_0 = room[0].addr();
    let link_2_to_1 = room[2].links.get_mut(&1).unwrap();
    link_2_to_1.disconnect();
    link_2_to_1.connect(to_0).await.unwrap();
    for _ in 0..5 {
        let _ = link_2_to_1.send_audio(&signal_of(2), 0).await;
    }
    room[2].links[&0]
        .send_audio(&signal_of(2), 0)
        .await
        .unwrap();

    until("the frame on the right link arrived", || {
        !room[0].heard_from(2).is_empty()
    })
    .await;
    assert_eq!(room[0].heard_from(2), vec![bytes_of(&signal_of(2))]);
    assert!(room[0].heard_from(1).is_empty());
}

/// Verifies: REQ-CON-133
#[tokio::test]
async fn when_one_participant_leaves_the_links_between_the_others_keep_carrying_audio() {
    let mut room = room_of(3).await;

    let mut gone = room.pop().unwrap();
    for link in gone.links.values_mut() {
        link.disconnect();
    }
    drop(gone);
    for participant in &mut room {
        participant.links.remove(&2).unwrap().disconnect();
    }

    everyone_sends(&room).await;

    until("the frames arrived", || {
        !room[0].heard_from(1).is_empty() && !room[1].heard_from(0).is_empty()
    })
    .await;
    assert_eq!(room[0].heard_from(1), vec![bytes_of(&signal_of(1))]);
    assert_eq!(room[1].heard_from(0), vec![bytes_of(&signal_of(0))]);
}

/// The connectivity check of one link reads the socket while the others carry audio: the
/// probes and their answers have to reach the link they belong to, and the audio must not be
/// taken by the check.
///
/// Verifies: REQ-CON-133
#[tokio::test]
async fn a_third_participant_checks_its_candidates_while_the_first_two_go_on_talking() {
    const FRAMES_MEANWHILE: usize = 200;
    let mut room = room_of(2).await;
    let mut third = Participant::enter().await;
    let (addr_0, key_0) = (room[0].addr(), room[0].identity.public_key());
    let (addr_1, key_1) = (room[1].addr(), room[1].identity.public_key());
    let (addr_2, key_2) = (third.addr(), third.identity.public_key());
    // Nothing answers here. Offered first, as the LAN address of another network would be.
    let nowhere: SocketAddr = "127.0.0.1:9".parse().unwrap();

    let mut from_2_to_0 = third.new_link(0, &key_0);
    let mut from_2_to_1 = third.new_link(1, &key_1);
    let mut from_0_to_2 = room[0].new_link(2, &key_2);
    let mut from_1_to_2 = room[1].new_link(2, &key_2);
    let the_first_two_talk = async {
        for _ in 0..FRAMES_MEANWHILE {
            room[0].links[&1]
                .send_audio(&signal_of(0), 0)
                .await
                .unwrap();
            room[1].links[&0]
                .send_audio(&signal_of(1), 0)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    let (to_0, to_1, to_2) = ([nowhere, addr_0], [nowhere, addr_1], [nowhere, addr_2]);
    let (a, b, c, d, ()) = tokio::join!(
        from_2_to_0.connect_with_candidates(&to_0),
        from_2_to_1.connect_with_candidates(&to_1),
        from_0_to_2.connect_with_candidates(&to_2),
        from_1_to_2.connect_with_candidates(&to_2),
        the_first_two_talk,
    );
    for result in [a, b, c, d] {
        result.expect("the check ends with a route");
    }

    for (link, peer_addr) in [
        (&from_2_to_0, addr_0),
        (&from_2_to_1, addr_1),
        (&from_0_to_2, addr_2),
        (&from_1_to_2, addr_2),
    ] {
        assert_eq!(link.state(), ConnectionState::Connected);
        assert_eq!(link.remote_addr(), peer_addr);
        assert_eq!(link.link_facts().snapshot().route_confirmed, Some(true));
    }
    until("what the first two said meanwhile arrived", || {
        room[0].heard_from(1).len() == FRAMES_MEANWHILE
            && room[1].heard_from(0).len() == FRAMES_MEANWHILE
    })
    .await;

    wait_until_encrypted(&[&from_2_to_0, &from_2_to_1, &from_0_to_2, &from_1_to_2]).await;
    from_2_to_0.send_audio(&signal_of(2), 0).await.unwrap();
    from_0_to_2.send_audio(&signal_of(0), 0).await.unwrap();
    until("the third and the first hear each other", || {
        !room[0].heard_from(2).is_empty() && !third.heard_from(0).is_empty()
    })
    .await;
    assert_eq!(room[0].heard_from(2), vec![bytes_of(&signal_of(2))]);
    assert_eq!(third.heard_from(0), vec![bytes_of(&signal_of(0))]);
}
