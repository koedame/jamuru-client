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
    /// Bind the audio socket on the port the audio session that just ended
    /// used, and tell the room only if that port could not be had again.
    ///
    /// For going from one peer to another while the room already knows this
    /// app's address: the peer that is on its way to that address finds it
    /// still good, so neither side has to chase the other's new one.
    KeepAddress,
    /// Send audio to `peer`, racing `candidates` (best first).
    Start {
        peer: Uuid,
        candidates: Vec<SocketAddr>,
    },
}

/// How the audio session to the peer it is with is doing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Link {
    #[default]
    Healthy,
    /// Packets stopped arriving; the session is still trying.
    Silent,
    /// The session gave up on the peer.
    Failed,
}

/// The other participants in the room, and whom the audio session is with.
#[derive(Debug, Default)]
pub struct Roster {
    peers: Vec<PeerInfo>,
    /// Set while an audio session to this peer runs (or is being started), so
    /// a second update from the same peer does not start a second one, and a
    /// peer leaving can be told from the peer the audio is with.
    streaming_with: Option<Uuid>,
    /// The addresses the audio session to `streaming_with` was started with.
    started_with: Vec<SocketAddr>,
    link: Link,
    /// The ids this app has had in the room. A connection the server has not
    /// noticed is gone keeps its entry in the room after the app comes back as
    /// someone new, so the room lists the app as one of its own peers.
    own_ids: Vec<Uuid>,
    /// Peers the audio was moved away from because it went silent. Not gone
    /// back to unless they publish something new: they are most likely old
    /// entries of a participant who came back, and the next stop would
    /// otherwise bounce between two of them.
    left_behind: Vec<Uuid>,
}

impl Roster {
    pub fn peers(&self) -> &[PeerInfo] {
        &self.peers
    }

    pub fn streaming_with(&self) -> Option<Uuid> {
        self.streaming_with
    }

    /// This app, as `own`, is in a room with `peers` (it just created, joined
    /// or rejoined it). Whichever side comes second finds the other's address
    /// already there; whichever comes first learns it from [`Roster::updated`].
    ///
    /// Entries of this app's earlier ids are not peers: sending audio to one
    /// is sending it to an address nothing listens on.
    pub fn entered(&mut self, own: Uuid, peers: Vec<PeerInfo>) -> Vec<Audio> {
        self.forget_the_audio();
        if !self.own_ids.contains(&own) {
            self.own_ids.push(own);
        }
        self.left_behind.clear();
        self.peers = peers
            .into_iter()
            .filter(|p| !self.own_ids.contains(&p.id))
            .collect();
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
    ///
    /// While the audio to someone else has stopped, it is also what lets this
    /// side follow a peer that came back under another id, or at another
    /// address, and whose old entry the server has not yet removed.
    pub fn updated(&mut self, peer: PeerInfo) -> Vec<Audio> {
        if let Some(known) = self.peers.iter_mut().find(|p| p.id == peer.id) {
            *known = peer.clone();
        }
        self.left_behind.retain(|id| *id != peer.id);
        if self.streaming_with.is_some() {
            return self.go_onward();
        }
        self.start_with(std::slice::from_ref(&peer))
            .into_iter()
            .collect()
    }

    /// The audio session stopped receiving from the peer it is with. Only a
    /// peer that looks like the same participant coming back (same name) is
    /// followed now: a gap of a couple of seconds on a working link is not a
    /// reason to leave someone else.
    pub fn audio_silent(&mut self) -> Vec<Audio> {
        if self.streaming_with.is_none() || self.link != Link::Healthy {
            return Vec::new();
        }
        self.link = Link::Silent;
        self.go_onward()
    }

    /// The audio session gave up on the peer it is with, so anyone else who
    /// has an address is better than staying.
    pub fn audio_failed(&mut self) -> Vec<Audio> {
        if self.streaming_with.is_none() || self.link == Link::Failed {
            return Vec::new();
        }
        self.link = Link::Failed;
        self.go_onward()
    }

    /// Packets are arriving again.
    pub fn audio_recovered(&mut self) {
        if self.streaming_with.is_some() {
            self.link = Link::Healthy;
        }
    }

    /// A peer left. If the audio was with them, it ends rather than reporting
    /// a lost connection, and the slot is free for whoever is still here or
    /// joins next. Starting audio used up the advertised socket, so a fresh
    /// one is advertised first.
    pub fn left(&mut self, peer_id: Uuid) -> Vec<Audio> {
        self.peers.retain(|p| p.id != peer_id);
        self.left_behind.retain(|id| *id != peer_id);
        if self.streaming_with != Some(peer_id) {
            return Vec::new();
        }
        self.forget_the_audio();
        let mut audio = vec![Audio::Stop, Audio::Advertise];
        audio.extend(self.start_with(&self.peers.clone()));
        audio
    }

    /// The audio session to `peer` could not be started. A later update from
    /// someone may retry, rather than leaving the room silent for good.
    pub fn start_failed(&mut self, peer: Uuid) {
        if self.streaming_with == Some(peer) {
            self.forget_the_audio();
        }
    }

    /// The audio session ended without the room changing (the connection to
    /// the server was lost).
    pub fn audio_ended(&mut self) {
        self.forget_the_audio();
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
        self.started_with = candidates.clone();
        self.link = Link::Healthy;
        Some(Audio::Start {
            peer: peer.id,
            candidates,
        })
    }

    fn forget_the_audio(&mut self) {
        self.streaming_with = None;
        self.started_with.clear();
        self.link = Link::Healthy;
    }

    /// Moves the audio on while its link is down, to where it can be heard:
    /// the same peer at addresses other than the ones the session was started
    /// with, or another peer who has published one (any, once the session gave
    /// up; only the same participant under a new id, before that).
    ///
    /// The address this app advertised is kept, so the peer being moved to
    /// finds it where it already looks.
    fn go_onward(&mut self) -> Vec<Audio> {
        let Some(with) = self.streaming_with else {
            return Vec::new();
        };
        if self.link == Link::Healthy {
            return Vec::new();
        }
        let name = self
            .peers
            .iter()
            .find(|p| p.id == with)
            .map(|p| p.name.clone());
        let failed = self.link == Link::Failed;
        let Some(next) = self
            .peers
            .iter()
            .find(|p| {
                let has_address = !p.get_sorted_candidates().is_empty();
                if self.left_behind.contains(&p.id) {
                    false
                } else if p.id == with {
                    has_address && p.get_sorted_candidates() != self.started_with
                } else {
                    has_address && (failed || Some(&p.name) == name.as_ref())
                }
            })
            .cloned()
        else {
            return Vec::new();
        };
        self.streaming_with = None;
        match self.start_with(std::slice::from_ref(&next)) {
            Some(start) => {
                if next.id != with {
                    self.left_behind.push(with);
                }
                vec![Audio::Stop, Audio::KeepAddress, start]
            }
            None => {
                self.streaming_with = Some(with);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jamjam::network::AddressCandidate;

    fn me() -> Uuid {
        Uuid::from_u128(1)
    }

    fn peer(id: u128, addr: Option<&str>) -> PeerInfo {
        named(id, &format!("peer-{id}"), addr)
    }

    fn named(id: u128, name: &str, addr: Option<&str>) -> PeerInfo {
        PeerInfo {
            id: Uuid::from_u128(id),
            name: name.to_string(),
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

        let audio = roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        assert_eq!(audio, vec![Audio::Advertise, start(2, "192.0.2.2:5000")]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    #[test]
    fn nobody_has_published_an_address_when_entering_only_advertises_ours() {
        let mut roster = Roster::default();

        let audio = roster.entered(me(), vec![peer(2, None)]);

        assert_eq!(audio, vec![Audio::Advertise]);
        assert_eq!(roster.streaming_with(), None);
    }

    #[test]
    fn a_peer_publishes_after_we_entered_starts_the_audio_with_them() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);
        roster.joined(peer(2, None));

        let audio = roster.updated(peer(2, Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![start(2, "192.0.2.2:5000")]);
        assert_eq!(roster.peers()[0].candidates.len(), 1);
    }

    #[test]
    fn the_audio_is_already_with_someone_when_another_peer_publishes_does_not_start_a_second_session(
    ) {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);
        roster.joined(peer(3, None));

        let audio = roster.updated(peer(3, Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    #[test]
    fn a_peer_we_never_heard_join_publishes_does_not_add_them_to_the_room() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);

        roster.updated(peer(9, Some("192.0.2.9:5000")));

        assert!(roster.peers().is_empty());
    }

    #[test]
    fn a_peer_joins_twice_lists_them_once() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);

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
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        let audio = roster.left(Uuid::from_u128(2));

        assert_eq!(audio, vec![Audio::Stop, Audio::Advertise]);
        assert_eq!(roster.streaming_with(), None);
    }

    #[test]
    fn a_peer_publishes_after_the_one_the_audio_was_with_left_starts_the_audio_with_the_new_peer() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);
        roster.left(Uuid::from_u128(2));
        roster.joined(peer(3, None));

        let audio = roster.updated(peer(3, Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![start(3, "192.0.2.3:6000")]);
    }

    #[test]
    fn the_next_peer_already_has_an_address_when_the_audio_peer_leaves_reconnects_to_them_after_advertising(
    ) {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);
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
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);
        roster.joined(peer(3, Some("192.0.2.3:6000")));

        let audio = roster.left(Uuid::from_u128(3));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    #[test]
    fn starting_the_audio_failed_lets_a_later_update_try_again() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        roster.start_failed(Uuid::from_u128(2));
        let audio = roster.updated(peer(2, Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![start(2, "192.0.2.2:5000")]);
    }

    #[test]
    fn entering_a_room_again_forgets_who_the_audio_was_with() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        let audio = roster.entered(me(), vec![peer(3, Some("192.0.2.3:6000"))]);

        assert_eq!(audio, vec![Audio::Advertise, start(3, "192.0.2.3:6000")]);
    }

    /// The bug this guards: the app came back to its room as a new peer while
    /// the server still listed its old entry, and started its audio with that
    /// entry - an address nothing listens on - instead of with the other peer.
    /// Verifies: REQ-CON-130
    #[test]
    fn the_room_still_lists_our_own_old_entry_when_entering_again_starts_the_audio_with_the_other_peer(
    ) {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);

        let audio = roster.entered(
            Uuid::from_u128(11),
            vec![
                named(1, "Aki", Some("192.0.2.1:5000")),
                named(2, "Ben", Some("192.0.2.2:5000")),
            ],
        );

        assert_eq!(audio, vec![Audio::Advertise, start(2, "192.0.2.2:5000")]);
        assert_eq!(roster.peers().len(), 1);
        assert_eq!(roster.peers()[0].id, Uuid::from_u128(2));
    }

    /// Verifies: REQ-CON-130
    #[test]
    fn the_room_still_lists_every_old_entry_of_ours_when_entering_a_third_time_lists_none_of_them()
    {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);
        roster.entered(Uuid::from_u128(11), vec![peer(1, Some("192.0.2.1:5000"))]);

        roster.entered(
            Uuid::from_u128(12),
            vec![
                peer(1, Some("192.0.2.1:5000")),
                peer(11, Some("192.0.2.11:5000")),
                peer(2, None),
            ],
        );

        let ids: Vec<_> = roster.peers().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![Uuid::from_u128(2)]);
    }

    #[test]
    fn the_room_is_left_and_another_entered_lists_a_peer_that_has_an_id_we_once_had() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);
        roster.clear();

        roster.entered(Uuid::from_u128(7), vec![peer(1, None)]);

        assert_eq!(roster.peers().len(), 1);
    }

    fn the_audio_is_with(roster: &mut Roster, id: u128, name: &str, addr: &str) {
        roster.entered(me(), vec![named(id, name, Some(addr))]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(id)));
    }

    /// The bug this guards: the other side of a rejoin kept sending audio to
    /// the old entry of the peer that came back, for ever, because the audio
    /// was "already with someone" and the server had not removed the entry.
    /// Verifies: REQ-CON-131
    #[test]
    fn the_peer_comes_back_under_a_new_id_while_the_audio_is_silent_follows_them_on_the_address_we_advertised(
    ) {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.joined(named(12, "Ben", None));
        assert_eq!(
            roster.updated(named(12, "Ben", Some("192.0.2.2:6000"))),
            vec![]
        );

        let audio = roster.audio_silent();

        assert_eq!(
            audio,
            vec![Audio::Stop, Audio::KeepAddress, start(12, "192.0.2.2:6000")]
        );
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(12)));
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn the_peer_comes_back_under_a_new_id_after_the_audio_went_silent_follows_them_when_they_publish(
    ) {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.audio_silent();
        roster.joined(named(12, "Ben", None));

        let audio = roster.updated(named(12, "Ben", Some("192.0.2.2:6000")));

        assert_eq!(
            audio,
            vec![Audio::Stop, Audio::KeepAddress, start(12, "192.0.2.2:6000")]
        );
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn someone_else_publishes_while_the_audio_is_silent_leaves_the_peer_the_audio_is_with_alone() {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.joined(named(3, "Cho", None));
        roster.audio_silent();

        let audio = roster.updated(named(3, "Cho", Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn the_audio_gave_up_on_the_peer_goes_to_someone_else_who_has_an_address() {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.joined(named(3, "Cho", Some("192.0.2.3:6000")));
        roster.audio_silent();

        let audio = roster.audio_failed();

        assert_eq!(
            audio,
            vec![Audio::Stop, Audio::KeepAddress, start(3, "192.0.2.3:6000")]
        );
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn the_audio_gave_up_on_the_peer_and_nobody_else_is_there_waits_for_the_next_one_to_publish() {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");

        assert_eq!(roster.audio_failed(), vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
        roster.joined(named(3, "Cho", None));
        let audio = roster.updated(named(3, "Cho", Some("192.0.2.3:6000")));

        assert_eq!(
            audio,
            vec![Audio::Stop, Audio::KeepAddress, start(3, "192.0.2.3:6000")]
        );
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn the_audio_was_silent_and_came_back_does_not_follow_a_peer_that_publishes_later() {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.audio_silent();
        roster.audio_recovered();
        roster.joined(named(12, "Ben", None));

        let audio = roster.updated(named(12, "Ben", Some("192.0.2.2:6000")));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(2)));
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn the_peer_the_audio_is_with_publishes_other_addresses_while_the_audio_is_silent_follows_them_there(
    ) {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.audio_silent();

        let audio = roster.updated(named(2, "Ben", Some("192.0.2.2:7000")));

        assert_eq!(
            audio,
            vec![Audio::Stop, Audio::KeepAddress, start(2, "192.0.2.2:7000")]
        );
    }

    #[test]
    fn the_peer_the_audio_is_with_publishes_the_same_addresses_while_the_audio_is_silent_changes_nothing(
    ) {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.audio_silent();

        let audio = roster.updated(named(2, "Ben", Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![]);
    }

    /// Verifies: REQ-CON-131
    #[test]
    fn the_audio_goes_silent_after_it_was_moved_goes_on_to_a_further_entry_and_never_back_to_one_it_left(
    ) {
        let mut roster = Roster::default();
        the_audio_is_with(&mut roster, 2, "Ben", "192.0.2.2:5000");
        roster.joined(named(12, "Ben", Some("192.0.2.2:6000")));
        roster.joined(named(22, "Ben", Some("192.0.2.2:8000")));

        let first = roster.audio_silent();
        let second = roster.audio_silent();

        assert_eq!(first.last(), Some(&start(12, "192.0.2.2:6000")));
        assert_eq!(second.last(), Some(&start(22, "192.0.2.2:8000")));
        assert_eq!(roster.audio_silent(), vec![]);
        assert_eq!(roster.streaming_with(), Some(Uuid::from_u128(22)));
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn started_with(roster: &mut Roster, peer: PeerInfo) -> Vec<SocketAddr> {
        match roster.entered(me(), vec![peer]).as_slice() {
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
