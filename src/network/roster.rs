//! Who is in the room and which of them an audio link goes to.
//!
//! This decides; it does no I/O. [`Roster`] is told what the room and the
//! links did and answers with what to do to the audio, and whoever drives the
//! audio - the app's `session`, the CLI - carries that out. Kept apart, and in
//! the core, so that both decide the same way and the rules about the audio -
//! who gets a link, what happens when someone leaves or goes silent - are
//! tested without a server or a sound card.

use std::net::SocketAddr;

use uuid::Uuid;

use super::{ConnectionState, PeerInfo, MAX_PEERS_PER_ROOM};

/// How the audio link to one participant is doing, as far as deciding what to
/// do about it goes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkHealth {
    /// Nothing has been heard from them yet: the link is being set up, or it
    /// is up on an address that has not answered
    Starting,
    /// Their packets arrive
    Heard,
    /// Their packets stopped arriving; the link is still trying
    Silent,
    /// The link gave up on them. It is taken up again if they are heard.
    Failed,
}

impl LinkHealth {
    /// The health of a link whose connection is in `state` and which has
    /// received `packets_received` packets so far. "Heard" is a packet
    /// arriving, not the connection coming up: it comes up on the first
    /// address even if nobody answers there.
    pub fn of(state: ConnectionState, packets_received: u64) -> Self {
        match state {
            ConnectionState::Failed => Self::Failed,
            ConnectionState::Reconnecting => Self::Silent,
            ConnectionState::Connected if packets_received > 0 => Self::Heard,
            _ => Self::Starting,
        }
    }
}

/// One thing to do to the audio, in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audio {
    /// Bind the audio socket and tell the room where to send audio.
    ///
    /// Nothing can send this app audio until it does: the port is chosen when
    /// the socket is bound, and peers learn it only from this (ADR-026).
    Advertise,
    /// Start the audio session on the devices, with no link yet, if none
    /// runs. Done on entering the room, so the microphone is heard (the meter
    /// moves) while the participant is alone in it; the links are opened on
    /// the session that is there.
    Open,
    /// Open an audio link to `peer`, racing `candidates` (best first).
    Start {
        peer: Uuid,
        candidates: Vec<SocketAddr>,
        /// The key the server gave for `peer`, which its key exchange is checked against
        link_key: Option<String>,
    },
    /// Close the audio link to `peer`. The audio with the others goes on, on
    /// the address the room already has.
    End { peer: Uuid },
    /// Leave the room and enter it again, as a new participant with a new
    /// audio address.
    ///
    /// For when a link gave up and nobody is heard: whatever broke the audio
    /// (an address that changed on either side, a path that is gone) is not
    /// something the two ends can tell each other, but a fresh entry tells
    /// the room the address to use.
    Rejoin,
}

/// How many links there are at most: one to each of the others in a full room.
const MAX_LINKS: usize = MAX_PEERS_PER_ROOM - 1;

/// How many times in a row the room is entered again because a link gave up,
/// before the roster leaves it be. A participant who is really gone would
/// otherwise have the app leaving and entering the room for as long as the
/// server lists them.
const MAX_REJOINS: u32 = 3;

/// The audio link to one participant.
#[derive(Debug)]
struct Link {
    peer: Uuid,
    /// The addresses the link was started with.
    started_with: Vec<SocketAddr>,
    health: LinkHealth,
}

/// The other participants in the room, and whom there is an audio link to.
#[derive(Debug, Default)]
pub struct Roster {
    peers: Vec<PeerInfo>,
    /// The links that run (or are being started), in the order they were
    /// opened: one to each participant who has published an address, as far
    /// as there is room.
    links: Vec<Link>,
    /// The ids this app has had in the room. A connection the server has not
    /// noticed is gone keeps its entry in the room after the app comes back as
    /// someone new, so the room lists the app as one of its own peers.
    own_ids: Vec<Uuid>,
    /// How many times the room was entered again because a link gave up, since
    /// anyone was last heard.
    rejoins: u32,
    /// Set from asking to enter the room again until it is entered, so that
    /// the links that give up meanwhile do not ask again.
    rejoining: bool,
}

impl Roster {
    pub fn peers(&self) -> &[PeerInfo] {
        &self.peers
    }

    /// The participants there is an audio link to, in the order the links
    /// were opened.
    pub fn linked(&self) -> Vec<Uuid> {
        self.links.iter().map(|link| link.peer).collect()
    }

    /// This app, as `own`, is in a room with `peers` (it just created, joined
    /// or rejoined it). Whichever side comes second finds the other's address
    /// already there; whichever comes first learns it from [`Roster::updated`].
    ///
    /// Entries of this app's earlier ids are not peers: sending audio to one
    /// is sending it to an address nothing listens on.
    pub fn entered(&mut self, own: Uuid, peers: Vec<PeerInfo>) -> Vec<Audio> {
        self.links.clear();
        self.rejoining = false;
        if !self.own_ids.contains(&own) {
            self.own_ids.push(own);
        }
        self.peers = peers
            .into_iter()
            .filter(|p| !self.own_ids.contains(&p.id))
            .collect();
        let mut audio = vec![Audio::Advertise, Audio::Open];
        audio.extend(self.open_links());
        audio
    }

    /// A peer joined. It has published no address yet.
    pub fn joined(&mut self, peer: PeerInfo) {
        if !self.peers.iter().any(|p| p.id == peer.id) {
            self.peers.push(peer);
        }
    }

    /// A peer published (or changed) its audio address. This is what lets
    /// whichever side entered first open its link: at that time the other
    /// had no address yet (ADR-026).
    ///
    /// A link to them that is not hearing them is opened again at what they
    /// published, if that is something else than it was started with: they
    /// are somewhere this app was not looking.
    pub fn updated(&mut self, peer: PeerInfo) -> Vec<Audio> {
        let id = peer.id;
        match self.peers.iter_mut().find(|p| p.id == id) {
            Some(known) => *known = peer,
            None => return Vec::new(),
        }
        if self.links.iter().any(|link| link.peer == id) {
            return self.follow(id);
        }
        self.open_links()
    }

    /// How the link to `peer` is doing now. Something is done about it only
    /// when that changed.
    ///
    /// A link that went silent or gave up is opened again where its peer has
    /// since published other addresses. Failing that, when a link gave up and
    /// nobody at all is heard, the room is entered again - as long as someone
    /// is heard the trouble is with the one link, and entering again would
    /// cut the audio that works.
    pub fn link_is(&mut self, peer: Uuid, health: LinkHealth) -> Vec<Audio> {
        let Some(link) = self.links.iter_mut().find(|link| link.peer == peer) else {
            return Vec::new();
        };
        if link.health == health {
            return Vec::new();
        }
        link.health = health;
        match health {
            LinkHealth::Starting => Vec::new(),
            LinkHealth::Heard => {
                self.rejoins = 0;
                Vec::new()
            }
            LinkHealth::Silent => self.follow(peer),
            LinkHealth::Failed => {
                let followed = self.follow(peer);
                if followed.is_empty() {
                    self.recover()
                } else {
                    followed
                }
            }
        }
    }

    /// A peer left: the link to them ends, and whoever had no link for want of
    /// room gets one. When that was the last link the audio session goes on
    /// with none, on the address the room has, so the microphone is still heard
    /// and whoever joins next finds the audio where it was.
    pub fn left(&mut self, peer_id: Uuid) -> Vec<Audio> {
        self.peers.retain(|p| p.id != peer_id);
        let Some(at) = self.links.iter().position(|link| link.peer == peer_id) else {
            return Vec::new();
        };
        self.links.remove(at);
        let mut audio = vec![Audio::End { peer: peer_id }];
        let opened = self.open_links();
        if opened.is_empty() {
            // Whoever is left may be someone the link gave up on
            audio.extend(self.recover());
        }
        audio.extend(opened);
        audio
    }

    /// The link to `peer` could not be started. The next thing that happens
    /// in the room tries again, rather than leaving them unheard for good.
    pub fn start_failed(&mut self, peer: Uuid) {
        self.links.retain(|link| link.peer != peer);
    }

    /// The audio session ended without the room changing (the connection to
    /// the server was lost, or the session could not go on).
    pub fn audio_ended(&mut self) {
        self.links.clear();
    }

    /// The room is gone.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Opens a link to everyone who has published an address and has none,
    /// as far as there is room.
    fn open_links(&mut self) -> Vec<Audio> {
        let mut audio = Vec::new();
        for peer in &self.peers {
            if self.links.len() >= MAX_LINKS {
                break;
            }
            if self.links.iter().any(|link| link.peer == peer.id) {
                continue;
            }
            let candidates = peer.get_sorted_candidates();
            if candidates.is_empty() {
                continue;
            }
            self.links.push(Link {
                peer: peer.id,
                started_with: candidates.clone(),
                health: LinkHealth::Starting,
            });
            audio.push(Audio::Start {
                peer: peer.id,
                candidates,
                link_key: peer.link_key.clone(),
            });
        }
        audio
    }

    /// Opens the link to `peer` again at the addresses they have published,
    /// when it is not hearing them and those are not the ones it was started
    /// with. The address this app advertised is kept, so they find it where
    /// they already look.
    fn follow(&mut self, peer: Uuid) -> Vec<Audio> {
        let Some(info) = self.peers.iter().find(|p| p.id == peer) else {
            return Vec::new();
        };
        let candidates = info.get_sorted_candidates();
        let Some(link) = self.links.iter_mut().find(|link| link.peer == peer) else {
            return Vec::new();
        };
        if link.health == LinkHealth::Heard
            || candidates.is_empty()
            || candidates == link.started_with
        {
            return Vec::new();
        }
        link.started_with = candidates.clone();
        link.health = LinkHealth::Starting;
        vec![
            Audio::End { peer },
            Audio::Start {
                peer,
                candidates,
                link_key: info.link_key.clone(),
            },
        ]
    }

    /// Enters the room again when a link gave up and nobody is heard, a few
    /// times at most until someone is.
    fn recover(&mut self) -> Vec<Audio> {
        let gave_up = |link: &Link| link.health == LinkHealth::Failed;
        let heard = |link: &Link| link.health == LinkHealth::Heard;
        if self.rejoining
            || self.rejoins >= MAX_REJOINS
            || !self.links.iter().any(gave_up)
            || self.links.iter().any(heard)
        {
            return Vec::new();
        }
        self.rejoins += 1;
        self.rejoining = true;
        vec![Audio::Rejoin]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::AddressCandidate;

    fn me() -> Uuid {
        Uuid::from_u128(1)
    }

    fn id(id: u128) -> Uuid {
        Uuid::from_u128(id)
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
            link_key: None,
        }
    }

    fn start(id: u128, addr: &str) -> Audio {
        Audio::Start {
            peer: Uuid::from_u128(id),
            candidates: vec![addr.parse().unwrap()],
            link_key: None,
        }
    }

    fn end(id: u128) -> Audio {
        Audio::End {
            peer: Uuid::from_u128(id),
        }
    }

    /// A room this app entered where `ids` have each published an address and
    /// are heard
    fn hearing(ids: &[u128]) -> Roster {
        let mut roster = Roster::default();
        roster.entered(
            me(),
            ids.iter()
                .map(|id| peer(*id, Some(&format!("192.0.2.{id}:5000"))))
                .collect(),
        );
        for peer in ids {
            assert_eq!(roster.link_is(id(*peer), LinkHealth::Heard), vec![]);
        }
        roster
    }

    /// "Heard" is a packet arriving, not the connection coming up: it comes up on the first
    /// address even when nobody answers there, and counting that as heard would stop the
    /// roster from ever trying another address.
    ///
    /// Verifies: REQ-CON-134
    #[test]
    fn a_connection_is_up_but_nothing_arrived_is_starting_and_is_heard_once_something_does() {
        assert_eq!(
            LinkHealth::of(ConnectionState::Connected, 0),
            LinkHealth::Starting
        );
        assert_eq!(
            LinkHealth::of(ConnectionState::Connected, 1),
            LinkHealth::Heard
        );
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn a_connection_is_setting_up_reconnecting_or_failed_has_the_health_the_roster_acts_on() {
        for setting_up in [
            ConnectionState::Disconnected,
            ConnectionState::Connecting,
            ConnectionState::GatheringCandidates,
            ConnectionState::CheckingConnectivity,
        ] {
            assert_eq!(LinkHealth::of(setting_up, 0), LinkHealth::Starting);
        }
        assert_eq!(
            LinkHealth::of(ConnectionState::Reconnecting, 100),
            LinkHealth::Silent
        );
        assert_eq!(
            LinkHealth::of(ConnectionState::Failed, 100),
            LinkHealth::Failed
        );
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn a_peer_in_the_room_has_already_published_an_address_when_entering_advertises_ours_and_starts_with_them(
    ) {
        let mut roster = Roster::default();

        let audio = roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        assert_eq!(
            audio,
            vec![Audio::Advertise, Audio::Open, start(2, "192.0.2.2:5000")]
        );
        assert_eq!(roster.linked(), vec![id(2)]);
    }

    /// The bug this guards: the third to enter a room of two was linked to one
    /// of them only, who was already linked to the other, so the third heard
    /// no one and no one heard the third.
    ///
    /// Verifies: REQ-CON-133
    #[test]
    fn two_peers_have_published_when_entering_opens_a_link_to_each() {
        let mut roster = Roster::default();

        let audio = roster.entered(
            me(),
            vec![
                peer(2, Some("192.0.2.2:5000")),
                peer(3, Some("192.0.2.3:6000")),
            ],
        );

        assert_eq!(
            audio,
            vec![
                Audio::Advertise,
                Audio::Open,
                start(2, "192.0.2.2:5000"),
                start(3, "192.0.2.3:6000")
            ]
        );
        assert_eq!(roster.linked(), vec![id(2), id(3)]);
    }

    /// Verifies: REQ-CON-140
    #[test]
    fn nobody_has_published_an_address_when_entering_advertises_ours_and_opens_the_audio_alone() {
        let mut roster = Roster::default();

        let audio = roster.entered(me(), vec![peer(2, None)]);

        assert_eq!(audio, vec![Audio::Advertise, Audio::Open]);
        assert_eq!(roster.linked(), Vec::<Uuid>::new());
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

    /// The bug this guards: with the audio "already with someone", a second
    /// participant who published an address was given no link, and was heard
    /// by no one.
    ///
    /// Verifies: REQ-CON-133
    #[test]
    fn there_is_a_link_to_someone_when_another_peer_publishes_opens_a_second_link_and_keeps_the_first(
    ) {
        let mut roster = hearing(&[2]);
        roster.joined(peer(3, None));

        let audio = roster.updated(peer(3, Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![start(3, "192.0.2.3:6000")]);
        assert_eq!(roster.linked(), vec![id(2), id(3)]);
    }

    #[test]
    fn a_peer_we_never_heard_join_publishes_does_not_add_them_to_the_room() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![]);

        let audio = roster.updated(peer(9, Some("192.0.2.9:5000")));

        assert_eq!(audio, vec![]);
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

    /// Verifies: REQ-CON-020
    #[test]
    fn nine_others_are_in_a_full_room_opens_a_link_to_each_of_them() {
        let mut roster = Roster::default();
        let others: Vec<PeerInfo> = (2..=10)
            .map(|n| peer(n, Some(&format!("192.0.2.{n}:5000"))))
            .collect();

        let audio = roster.entered(me(), others);

        assert_eq!(audio.len(), 2 + 9);
        assert_eq!(roster.linked().len(), 9);
    }

    /// The room can list more than nine others for a while: entries of
    /// connections the server has not yet found dead are not counted against
    /// its ten. The session has nine places.
    ///
    /// Verifies: REQ-CON-020
    #[test]
    fn more_than_nine_others_are_listed_links_the_first_nine_and_the_next_when_one_of_them_leaves()
    {
        let mut roster = Roster::default();
        let others: Vec<PeerInfo> = (2..=11)
            .map(|n| peer(n, Some(&format!("192.0.2.{n}:5000"))))
            .collect();
        roster.entered(me(), others);
        assert!(!roster.linked().contains(&id(11)));

        let audio = roster.left(id(5));

        assert_eq!(audio, vec![end(5), start(11, "192.0.2.11:5000")]);
        assert_eq!(roster.linked().len(), 9);
    }

    /// The microphone is heard in a room with no one else in it, so the audio
    /// session does not end with the last link: the meter keeps moving, and
    /// whoever joins next gets a link on the session that is there.
    ///
    /// Verifies: REQ-CON-140
    #[test]
    fn the_only_peer_there_is_a_link_to_leaves_ends_the_link_and_keeps_the_audio_session() {
        let mut roster = hearing(&[2]);

        let audio = roster.left(id(2));

        assert_eq!(audio, vec![end(2)]);
        assert_eq!(roster.linked(), Vec::<Uuid>::new());
    }

    #[test]
    fn a_peer_publishes_after_the_only_one_linked_left_starts_the_audio_with_the_new_peer() {
        let mut roster = hearing(&[2]);
        roster.left(id(2));
        roster.joined(peer(3, None));

        let audio = roster.updated(peer(3, Some("192.0.2.3:6000")));

        assert_eq!(audio, vec![start(3, "192.0.2.3:6000")]);
    }

    /// The others go on hearing each other, on the address the room has: the
    /// socket they send to is the one the link that ended used too.
    ///
    /// Verifies: REQ-CON-133
    #[test]
    fn one_of_two_linked_peers_leaves_ends_the_link_to_them_and_touches_nothing_else() {
        let mut roster = hearing(&[2, 3]);

        let audio = roster.left(id(2));

        assert_eq!(audio, vec![end(2)]);
        assert_eq!(roster.linked(), vec![id(3)]);
    }

    #[test]
    fn a_peer_there_is_no_link_to_leaves_does_not_touch_the_audio() {
        let mut roster = hearing(&[2]);
        roster.joined(peer(3, None));

        let audio = roster.left(id(3));

        assert_eq!(audio, vec![]);
        assert_eq!(roster.linked(), vec![id(2)]);
        assert_eq!(roster.peers().len(), 1);
    }

    #[test]
    fn starting_a_link_failed_lets_the_next_update_try_again() {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        roster.start_failed(id(2));
        let audio = roster.updated(peer(2, Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![start(2, "192.0.2.2:5000")]);
    }

    #[test]
    fn the_audio_session_ended_on_its_own_lets_the_next_update_open_every_link_again() {
        let mut roster = hearing(&[2, 3]);

        roster.audio_ended();
        let audio = roster.updated(peer(3, Some("192.0.2.3:5000")));

        assert_eq!(
            audio,
            vec![start(2, "192.0.2.2:5000"), start(3, "192.0.2.3:5000")]
        );
    }

    #[test]
    fn entering_a_room_again_forgets_whom_there_were_links_to() {
        let mut roster = hearing(&[2]);

        let audio = roster.entered(me(), vec![peer(3, Some("192.0.2.3:6000"))]);

        assert_eq!(
            audio,
            vec![Audio::Advertise, Audio::Open, start(3, "192.0.2.3:6000")]
        );
        assert_eq!(roster.linked(), vec![id(3)]);
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

        assert_eq!(
            audio,
            vec![Audio::Advertise, Audio::Open, start(2, "192.0.2.2:5000")]
        );
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

    /// The bug this guards: the other side of a rejoin kept sending audio to
    /// the old entry of the peer that came back, for ever, because the audio
    /// was "already with someone" and the server had not removed the entry.
    /// Verifies: REQ-CON-134
    #[test]
    fn a_peer_comes_back_under_a_new_id_gets_a_link_at_once_and_ours_to_the_old_entry_is_left_to_the_room(
    ) {
        let mut roster = Roster::default();
        roster.entered(me(), vec![named(2, "Ben", Some("192.0.2.2:5000"))]);
        roster.link_is(id(2), LinkHealth::Heard);
        roster.joined(named(12, "Ben", None));

        let audio = roster.updated(named(12, "Ben", Some("192.0.2.2:6000")));

        assert_eq!(audio, vec![start(12, "192.0.2.2:6000")]);
        assert_eq!(roster.link_is(id(2), LinkHealth::Silent), vec![]);
        assert_eq!(roster.left(id(2)), vec![end(2)]);
        assert_eq!(roster.linked(), vec![id(12)]);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn a_peer_publishes_other_addresses_while_the_link_to_them_is_silent_opens_it_again_there() {
        let mut roster = hearing(&[2, 3]);
        roster.link_is(id(2), LinkHealth::Silent);

        let audio = roster.updated(peer(2, Some("192.0.2.2:7000")));

        assert_eq!(audio, vec![end(2), start(2, "192.0.2.2:7000")]);
        assert_eq!(roster.linked(), vec![id(2), id(3)]);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn a_peer_published_other_addresses_before_the_link_to_them_went_silent_opens_it_again_there_when_it_does(
    ) {
        let mut roster = hearing(&[2]);
        assert_eq!(roster.updated(peer(2, Some("192.0.2.2:7000"))), vec![]);

        let audio = roster.link_is(id(2), LinkHealth::Silent);

        assert_eq!(audio, vec![end(2), start(2, "192.0.2.2:7000")]);
    }

    /// A link that was started at an address nothing answers on has not heard
    /// its peer, so it need not wait to go silent to follow them.
    ///
    /// Verifies: REQ-CON-134
    #[test]
    fn a_peer_publishes_other_addresses_before_the_link_to_them_has_heard_them_opens_it_again_there(
    ) {
        let mut roster = Roster::default();
        roster.entered(me(), vec![peer(2, Some("192.0.2.2:5000"))]);

        let audio = roster.updated(peer(2, Some("192.0.2.2:7000")));

        assert_eq!(audio, vec![end(2), start(2, "192.0.2.2:7000")]);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn a_peer_publishes_other_addresses_while_they_are_heard_leaves_the_link_as_it_is() {
        let mut roster = hearing(&[2]);

        let audio = roster.updated(peer(2, Some("192.0.2.2:7000")));

        assert_eq!(audio, vec![]);
    }

    #[test]
    fn a_peer_publishes_the_same_addresses_while_the_link_to_them_is_silent_changes_nothing() {
        let mut roster = hearing(&[2]);
        roster.link_is(id(2), LinkHealth::Silent);

        let audio = roster.updated(peer(2, Some("192.0.2.2:5000")));

        assert_eq!(audio, vec![]);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn a_link_goes_silent_and_comes_back_does_nothing_to_it_or_to_the_others() {
        let mut roster = hearing(&[2, 3]);

        let silent = roster.link_is(id(2), LinkHealth::Silent);
        let back = roster.link_is(id(2), LinkHealth::Heard);

        assert_eq!((silent, back), (vec![], vec![]));
        assert_eq!(roster.linked(), vec![id(2), id(3)]);
    }

    /// Entering again takes this app out of the room for a few seconds, which
    /// cuts the audio with everyone. That is no way to treat a trouble with
    /// one participant while the others are heard.
    ///
    /// Verifies: REQ-CON-134
    #[test]
    fn a_link_gives_up_while_someone_else_is_heard_stays_in_the_room() {
        let mut roster = hearing(&[2, 3]);
        roster.link_is(id(2), LinkHealth::Silent);

        let audio = roster.link_is(id(2), LinkHealth::Failed);

        assert_eq!(audio, vec![]);
        assert_eq!(roster.linked(), vec![id(2), id(3)]);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn the_only_link_gives_up_enters_the_room_again() {
        let mut roster = hearing(&[2]);
        roster.link_is(id(2), LinkHealth::Silent);

        let audio = roster.link_is(id(2), LinkHealth::Failed);

        assert_eq!(audio, vec![Audio::Rejoin]);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn every_link_gives_up_one_after_the_other_enters_the_room_again_once() {
        let mut roster = hearing(&[2, 3]);
        roster.link_is(id(2), LinkHealth::Silent);
        roster.link_is(id(3), LinkHealth::Silent);

        let first = roster.link_is(id(2), LinkHealth::Failed);
        let second = roster.link_is(id(3), LinkHealth::Failed);

        assert_eq!((first, second), (vec![Audio::Rejoin], vec![]));
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn the_heard_peer_leaves_and_the_one_left_is_one_the_link_gave_up_on_enters_the_room_again() {
        let mut roster = hearing(&[2, 3]);
        roster.link_is(id(2), LinkHealth::Failed);

        let audio = roster.left(id(3));

        assert_eq!(audio, vec![end(3), Audio::Rejoin]);
    }

    /// A link that comes up on an address that never answers reports itself
    /// as up and then gives up again. Only hearing the peer counts as the
    /// audio being back; anything less had the app entering the room again
    /// for as long as the server listed someone who was gone.
    ///
    /// Verifies: REQ-CON-134
    #[test]
    fn nobody_is_heard_after_entering_the_room_again_enters_it_again_only_a_few_times() {
        let mut roster = Roster::default();
        let peers = vec![named(2, "Ben", Some("192.0.2.2:5000"))];
        roster.entered(me(), peers.clone());

        let mut entered_again = 0;
        for round in 0..10 {
            roster.link_is(id(2), LinkHealth::Silent);
            if roster.link_is(id(2), LinkHealth::Failed) == vec![Audio::Rejoin] {
                entered_again += 1;
            }
            roster.entered(Uuid::from_u128(100 + round), peers.clone());
        }

        assert_eq!(entered_again, 3);
    }

    /// Verifies: REQ-CON-134
    #[test]
    fn someone_is_heard_after_entering_the_room_again_enters_it_again_when_a_link_next_gives_up() {
        let mut roster = Roster::default();
        let peers = vec![named(2, "Ben", Some("192.0.2.2:5000"))];
        roster.entered(me(), peers.clone());
        for round in 0..3 {
            assert_eq!(
                roster.link_is(id(2), LinkHealth::Failed),
                vec![Audio::Rejoin]
            );
            roster.entered(Uuid::from_u128(100 + round), peers.clone());
        }
        assert_eq!(roster.link_is(id(2), LinkHealth::Failed), vec![]);
        roster.link_is(id(2), LinkHealth::Heard);

        let audio = roster.link_is(id(2), LinkHealth::Failed);

        assert_eq!(audio, vec![Audio::Rejoin]);
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn started_with(roster: &mut Roster, peer: PeerInfo) -> Vec<SocketAddr> {
        match roster.entered(me(), vec![peer]).as_slice() {
            [Audio::Advertise, Audio::Open, Audio::Start { candidates, .. }] => candidates.clone(),
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
