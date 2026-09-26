//! Who is in the room and which of them the audio goes to.
//!
//! This decides; it does no I/O. [`Roster`] is told what the room did and
//! answers with what to do to the audio, and `session` carries that out. Kept
//! apart so the rules about the audio link - who to start with, what happens
//! when they leave - are tested without a server or a sound card.

use std::net::SocketAddr;

use jamjam::network::PeerInfo;
use uuid::Uuid;

/// One thing to do to the audio, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audio {
    /// End the audio session.
    Stop,
    /// Bind the audio socket and tell the room where to send audio.
    ///
    /// Nothing can send this app audio until it does: the port is chosen when
    /// the socket is bound, and peers learn it only from this (ADR-026).
    Advertise,
    /// Send audio to `peer`, racing `candidates` (best first).
    Start {
        peer: Uuid,
        candidates: Vec<SocketAddr>,
    },
}

/// The other participants in the room, and whom the audio session is with.
#[derive(Debug, Default)]
pub struct Roster {
    peers: Vec<PeerInfo>,
    /// Set while an audio session to this peer runs (or is being started), so
    /// a second update from the same peer does not start a second one, and a
    /// peer leaving can be told from the peer the audio is with.
    streaming_with: Option<Uuid>,
}

impl Roster {
    pub fn peers(&self) -> &[PeerInfo] {
        &self.peers
    }

    pub fn streaming_with(&self) -> Option<Uuid> {
        self.streaming_with
    }

    /// This app is in a room with `peers` (it just created, joined or rejoined
    /// it). Whichever side comes second finds the other's address already
    /// there; whichever comes first learns it from [`Roster::updated`].
    pub fn entered(&mut self, peers: Vec<PeerInfo>) -> Vec<Audio> {
        self.streaming_with = None;
        self.peers = peers;
        let mut audio = vec![Audio::Advertise];
        audio.extend(self.start_with(&self.peers.clone()));
        audio
    }

    /// A peer joined. It has published no address yet.
    pub fn joined(&mut self, peer: PeerInfo) {
        if !self.peers.iter().any(|p| p.id == peer.id) {
            self.peers.push(peer);
        }
    }

    /// A peer published (or changed) its audio address. This is what lets
    /// whichever side entered first start streaming: at that time the other
    /// had no address yet (ADR-026).
    pub fn updated(&mut self, peer: PeerInfo) -> Vec<Audio> {
        if let Some(known) = self.peers.iter_mut().find(|p| p.id == peer.id) {
            *known = peer.clone();
        }
        self.start_with(std::slice::from_ref(&peer))
            .into_iter()
            .collect()
    }

    /// A peer left. If the audio was with them, it ends rather than reporting
    /// a lost connection, and the slot is free for whoever is still here or
    /// joins next. Starting audio used up the advertised socket, so a fresh
    /// one is advertised first.
    pub fn left(&mut self, peer_id: Uuid) -> Vec<Audio> {
        self.peers.retain(|p| p.id != peer_id);
        if self.streaming_with != Some(peer_id) {
            return Vec::new();
        }
        self.streaming_with = None;
        let mut audio = vec![Audio::Stop, Audio::Advertise];
        audio.extend(self.start_with(&self.peers.clone()));
        audio
    }

    /// The audio session to `peer` could not be started. A later update from
    /// someone may retry, rather than leaving the room silent for good.
    pub fn start_failed(&mut self, peer: Uuid) {
        if self.streaming_with == Some(peer) {
            self.streaming_with = None;
        }
    }

    /// The audio session ended without the room changing (the connection to
    /// the server was lost).
    pub fn audio_ended(&mut self) {
        self.streaming_with = None;
    }

    /// The room is gone.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Starts audio with the first of `candidates_from` that has published an
    /// address, unless the audio is already with someone.
    fn start_with(&mut self, candidates_from: &[PeerInfo]) -> Option<Audio> {
        if self.streaming_with.is_some() {
            return None;
        }
        let peer = candidates_from.iter().find(|p| {
            p.public_addr.is_some() || p.local_addr.is_some() || !p.candidates.is_empty()
        })?;
        let candidates = peer.get_sorted_candidates();
        if candidates.is_empty() {
            return None;
        }
        self.streaming_with = Some(peer.id);
        Some(Audio::Start {
            peer: peer.id,
            candidates,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jamjam::network::AddressCandidate;

    fn peer(id: u128, addr: Option<&str>) -> PeerInfo {
        PeerInfo {
            id: Uuid::from_u128(id),
            name: format!("peer-{id}"),
            candidates: addr
                .map(|a| vec![AddressCandidate::host(a.parse().unwrap())])
                .unwrap_or_default(),
            public_addr: None,
            local_addr: None,
            joined_at: 0,
            features: vec![],
        }
    }

    fn start(id: u128, addr: &str) -> Audio {
        Audio::Start {
            peer: Uuid::from_u128(id),
            candidates: vec![addr.parse().unwrap()],
        }
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn a_peer_in_the_room_has_already_published_an_address_when_entering_advertises_ours_and_starts_with_them(
    ) {
        let mut roster = Roster::default();

        let audio = roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);

        assert_eq!(audio, vec![Audio::Advertise, start(2, "192.0.2.2:5000")]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    #[test]
    fn nobody_has_published_an_address_when_entering_only_advertises_ours() {
        let mut roster = Roster::default();

        let audio = roster.entered(vec![peer(2, None)]);

        assert_eq!(audio, vec![Audio::Advertise]);
        assert_eq!(roster.streaming_with(), None);
    }

    #[test]
    fn a_peer_publishes_after_we_entered_starts_the_audio_with_them() {
        let mut roster = Roster::default();
        roster.entered(vec![]);
        roster.joined(peer(2, None));

        let audio = roster.updated(peer(2, Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![start(2, "192.0.2.2:5000")]);
        assert_eq!(roster.peers()[0].candidates.len(), 1);
    }

    #[test]
    fn the_audio_is_already_with_someone_when_another_peer_publishes_does_not_start_a_second_session(
    ) {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);
        roster.joined(peer(3, None));

        let audio = roster.updated(peer(3, Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    #[test]
    fn a_peer_we_never_heard_join_publishes_does_not_add_them_to_the_room() {
        let mut roster = Roster::default();
        roster.entered(vec![]);

        roster.updated(peer(9, Some("192.0.2.9:5000")));

        assert!(roster.peers().is_empty());
    }

    #[test]
    fn a_peer_joins_twice_lists_them_once() {
        let mut roster = Roster::default();
        roster.entered(vec![]);

        roster.joined(peer(2, None));
        roster.joined(peer(2, None));

        assert_eq!(roster.peers().len(), 1);
    }

    /// The bug this guards: after B left, the app kept its audio session to B
    /// (which then reported the connection as lost) and, because it still
    /// counted as streaming, never started audio to the next person.
    #[test]
    fn the_peer_the_audio_is_with_leaves_stops_the_audio_and_advertises_a_fresh_address() {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);

        let audio = roster.left(Uuid::from_u128(2));

        assert_eq!(audio, vec![Audio::Stop, Audio::Advertise]);
        assert_eq!(roster.streaming_with(), None);
    }

    #[test]
    fn a_peer_publishes_after_the_one_the_audio_was_with_left_starts_the_audio_with_the_new_peer() {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);
        roster.left(Uuid::from_u128(2));
        roster.joined(peer(3, None));

        let audio = roster.updated(peer(3, Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![start(3, "192.0.2.3:6000")]);
    }

    #[test]
    fn the_next_peer_already_has_an_address_when_the_audio_peer_leaves_reconnects_to_them_after_advertising(
    ) {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);
        // 3 joined while 2 was talking; the audio did not go to 3 then.
        roster.joined(peer(3, Some("192.0.2.3:6000")));

        let audio = roster.left(Uuid::from_u128(2));

        // Starting audio used up the advertised socket, so it is advertised
        // again (a fresh port) before anyone can send to us.
        assert_eq!(
            audio,
            vec![Audio::Stop, Audio::Advertise, start(3, "192.0.2.3:6000")]
        );
    }

    #[test]
    fn a_peer_the_audio_is_not_with_leaves_does_not_touch_the_audio() {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);
        roster.joined(peer(3, Some("192.0.2.3:6000")));

        let audio = roster.left(Uuid::from_u128(3));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    #[test]
    fn starting_the_audio_failed_lets_a_later_update_try_again() {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);

        roster.start_failed(Uuid::from_u128(2));
        let audio = roster.updated(peer(2, Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![start(2, "192.0.2.2:5000")]);
    }

    #[test]
    fn entering_a_room_again_forgets_who_the_audio_was_with() {
        let mut roster = Roster::default();
        roster.entered(vec![peer(2, Some("192.0.2.2:5000"))]);

        let audio = roster.entered(vec![peer(3, Some("192.0.2.3:6000"))]);

        assert_eq!(audio, vec![Audio::Advertise, start(3, "192.0.2.3:6000")]);
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn started_with(roster: &mut Roster, peer: PeerInfo) -> Vec<SocketAddr> {
        match roster.entered(vec![peer]).as_slice() {
            [Audio::Advertise, Audio::Start { candidates, .. }] => candidates.clone(),
            other => panic!("expected the audio to start, got {other:?}"),
        }
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn a_peer_on_the_same_network_is_tried_on_its_lan_address_before_its_public_one() {
        let mut roster = Roster::default();
        let mut peer = peer(2, None);
        peer.candidates = vec![
            AddressCandidate::server_reflexive(addr("203.0.113.7:5000")),
            AddressCandidate::host(addr("192.168.1.20:5000")),
        ];

        let candidates = started_with(&mut roster, peer);

        assert_eq!(
            candidates,
            vec![addr("192.168.1.20:5000"), addr("203.0.113.7:5000")]
        );
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn a_peer_that_published_legacy_addresses_too_has_them_appended_when_not_already_listed() {
        let mut roster = Roster::default();
        let mut peer = peer(2, None);
        peer.candidates = vec![AddressCandidate::host(addr("192.168.1.20:5000"))];
        peer.public_addr = Some(addr("203.0.113.7:5000"));
        peer.local_addr = Some(addr("192.168.1.20:5000"));

        let candidates = started_with(&mut roster, peer);

        assert_eq!(
            candidates,
            vec![addr("192.168.1.20:5000"), addr("203.0.113.7:5000")]
        );
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn a_peer_that_published_its_bind_address_as_the_legacy_local_one_is_not_sent_to_it() {
        let mut roster = Roster::default();
        let mut peer = peer(2, None);
        peer.candidates = vec![AddressCandidate::host(addr("192.168.1.20:5000"))];
        peer.local_addr = Some(addr("0.0.0.0:5000"));

        let candidates = started_with(&mut roster, peer);

        assert_eq!(candidates, vec![addr("192.168.1.20:5000")]);
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn a_peer_with_only_a_legacy_public_address_is_reached_on_it() {
        let mut roster = Roster::default();
        let mut peer = peer(2, None);
        peer.public_addr = Some(addr("203.0.113.7:5000"));

        let candidates = started_with(&mut roster, peer);

        assert_eq!(candidates, vec![addr("203.0.113.7:5000")]);
    }
}
