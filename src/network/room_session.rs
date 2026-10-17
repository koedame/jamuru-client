//! How far a participant has got into a room, and what each step changes (ADR-044 §6).
//!
//! [`State`] is the session as a state machine with no I/O: the phase (reaching the server,
//! in a room, failed), the connection and room there are, the participants, and getting the
//! connection back. A step of the app - connecting, entering a room, leaving, getting the
//! connection back, an event from the room - is one method that says what it changes and, for
//! those that decide about the audio, which audio steps follow ([`Audio`]). The app talks to
//! the server and the audio engine and calls these; [`Roster`] decides about the peers.
//!
//! [`Snapshot`] is what a screen is told. It is sent as it is, so its shape is the screen's
//! contract.

use serde::Serialize;
use uuid::Uuid;

use super::room_entry::Entered;
use super::roster::{Audio, LinkHealth, Roster};
use super::signaling::{PeerInfo, RoomInfo};

/// How far the app has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Reaching the signaling server.
    #[default]
    ConnectingServer,
    /// Connected to the server, in no room.
    ServerConnected,
    Creating,
    Joining,
    /// In a room.
    Connected,
    /// The last step failed. `error` says how; the connection, if there is
    /// one, is still usable for another try.
    Error,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::ConnectingServer => "connecting_server",
            Phase::ServerConnected => "server_connected",
            Phase::Creating => "creating",
            Phase::Joining => "joining",
            Phase::Connected => "connected",
            Phase::Error => "error",
        }
    }
}

/// Getting the signaling connection back after it dropped unexpectedly, while
/// in a room. Distinct from the audio link being re-established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconnect {
    #[default]
    Idle,
    Reconnecting,
    /// Every attempt failed; trying again is the person's to ask for.
    Failed,
}

/// Someone else in the room. Their addresses stay in the backend.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    /// What their app can do beyond the base protocol (ADR-043).
    pub features: Vec<String>,
}

/// The room this app is in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoomView {
    pub room_id: String,
    /// The code others can join with, as the server reported it. Empty from a
    /// server that reports none.
    pub invite_code: String,
    /// This app's id in the room.
    pub peer_id: String,
    /// The name this app joined with.
    pub peer_name: String,
    pub participants: Vec<Participant>,
}

/// The whole session at one moment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    /// Rises with every change, so a reader that hears an old snapshot after a
    /// newer one can tell.
    pub revision: u64,
    pub phase: Phase,
    /// The connection the room's commands (chat, help) are addressed to. None
    /// while there is no connection to the server.
    pub connection_id: Option<u32>,
    /// The invite code of the room the server offers for trying a connection,
    /// or None when it offers none. The app holds no such code itself.
    pub test_room_invite_code: Option<String>,
    /// The code being joined, while `phase` is `joining`.
    pub joining_code: Option<String>,
    /// Why `phase` is `error`.
    pub error: Option<String>,
    pub room: Option<RoomView>,
    pub signaling_reconnect: Reconnect,
    pub signaling_reconnect_error: Option<String>,
    /// The participants there is an audio link to, in the order the links
    /// were opened.
    pub streaming_peer_ids: Vec<String>,
    /// The first of `streaming_peer_ids`: in a room of two, the participant
    /// the audio goes to, which is what a reader written for such a room
    /// expects.
    pub streaming_peer_id: Option<String>,
}

struct Room {
    room_id: String,
    invite_code: String,
    peer_id: String,
    peer_name: String,
}

/// The invite code of the room the server offers for trying a connection.
pub fn test_room_code_of(rooms: &[RoomInfo]) -> Option<String> {
    rooms
        .iter()
        .find(|room| room.test_room)
        .map(|room| room.invite_code.to_string())
}

/// The session: where it is, and what its steps change.
#[derive(Default)]
pub struct State {
    revision: u64,
    /// Raised to end a step that is still running and start another: a step
    /// that finds it changed stops without touching the session.
    epoch: u64,
    phase: Phase,
    connection_id: Option<u32>,
    test_room_invite_code: Option<String>,
    joining_code: Option<String>,
    error: Option<String>,
    room: Option<Room>,
    reconnect: Reconnect,
    reconnect_error: Option<String>,
    roster: Roster,
    /// What was last announced (its revision left at 0), so a change that
    /// only undoes a quiet one is still told, and one that changes nothing
    /// is not.
    announced: Option<Snapshot>,
}

impl State {
    /// The session as the screen is told it.
    pub fn view(&self) -> Snapshot {
        let linked = self.roster.linked();
        Snapshot {
            revision: self.revision,
            phase: self.phase,
            connection_id: self.connection_id,
            test_room_invite_code: self.test_room_invite_code.clone(),
            joining_code: self.joining_code.clone(),
            error: self.error.clone(),
            room: self.room.as_ref().map(|room| RoomView {
                room_id: room.room_id.clone(),
                invite_code: room.invite_code.clone(),
                peer_id: room.peer_id.clone(),
                peer_name: room.peer_name.clone(),
                participants: self
                    .roster
                    .peers()
                    .iter()
                    .map(|peer| Participant {
                        id: peer.id.to_string(),
                        name: peer.name.clone(),
                        features: peer.features.clone(),
                    })
                    .collect(),
            }),
            signaling_reconnect: self.reconnect,
            signaling_reconnect_error: self.reconnect_error.clone(),
            streaming_peer_ids: linked.iter().map(ToString::to_string).collect(),
            streaming_peer_id: linked.first().map(ToString::to_string),
        }
    }

    /// What there is to announce after the changes made since the last call: the session as it
    /// now is, or None when that makes no difference to what was last announced.
    pub fn announcement(&mut self) -> Option<Snapshot> {
        let mut now = self.view();
        now.revision = 0;
        if self.announced.as_ref() == Some(&now) {
            return None;
        }
        let before = self.announced.replace(now.clone());
        self.revision += 1;
        now.revision = self.revision;
        // Which step the app is at decides which buttons do anything, so its
        // transitions are the first thing a bug report needs (ADR-036).
        let from = before.as_ref().map(|b| b.phase.name()).unwrap_or("(start)");
        let changed = before
            .as_ref()
            .is_none_or(|b| b.phase != now.phase || b.error != now.error);
        if changed {
            match &now.error {
                Some(error) if now.phase == Phase::Error => {
                    tracing::info!("[session] {} -> {}: {}", from, now.phase.name(), error)
                }
                _ => tracing::info!("[session] {} -> {}", from, now.phase.name()),
            }
        }
        Some(now)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Ends whatever step is running (it stops at its next check) and returns
    /// the number of the one that takes over.
    pub fn next_epoch(&mut self) -> u64 {
        self.epoch += 1;
        self.epoch
    }

    /// [`State::next_epoch`], but only while `conn` is still the session's
    /// connection: an event of a connection that was let go of meanwhile ends
    /// nothing.
    pub fn next_epoch_of(&mut self, conn: u32) -> Option<u64> {
        if self.connection_id != Some(conn) {
            return None;
        }
        Some(self.next_epoch())
    }

    pub fn connection_id(&self) -> Option<u32> {
        self.connection_id
    }

    pub fn in_room(&self) -> bool {
        self.room.is_some()
    }

    pub fn reconnect(&self) -> Reconnect {
        self.reconnect
    }

    /// The code to rejoin the room with after the connection was lost, or
    /// None when there is no room to rejoin.
    pub fn rejoin_code(&self) -> Option<String> {
        self.room.as_ref().map(|room| {
            if room.invite_code.is_empty() {
                room.room_id.clone()
            } else {
                room.invite_code.clone()
            }
        })
    }

    /// Whether an event of `conn` is one of the room the session is in.
    fn is_the_room_of(&self, conn: u32) -> bool {
        self.connection_id == Some(conn) && self.room.is_some()
    }

    /// The step that was running failed with `message`.
    pub fn failed(&mut self, message: String) {
        self.phase = Phase::Error;
        self.error = Some(message);
        self.joining_code = None;
    }

    /// The connection there is, and the room, are let go of. Returns the connection to close
    /// and whether the audio of a room is to be stopped.
    pub fn dropping_the_connection(&mut self) -> (Option<u32>, bool) {
        self.roster.audio_ended();
        (self.connection_id.take(), self.room.is_some())
    }

    /// Starting from nothing: reaching the server, in no room.
    pub fn connecting_server(&mut self) {
        self.phase = Phase::ConnectingServer;
        self.error = None;
        self.joining_code = None;
        self.test_room_invite_code = None;
        self.room = None;
        self.roster.clear();
        self.reconnect = Reconnect::Idle;
        self.reconnect_error = None;
    }

    /// The connection to the server was made.
    pub fn connected(&mut self, conn: u32) {
        self.connection_id = Some(conn);
    }

    /// The server's rooms were listed: the app is connected to it and in no room.
    pub fn listed(&mut self, rooms: &[RoomInfo]) {
        self.test_room_invite_code = test_room_code_of(rooms);
        self.phase = Phase::ServerConnected;
    }

    /// A room is being made.
    pub fn creating(&mut self) {
        self.phase = Phase::Creating;
        self.error = None;
    }

    /// The room `code` names is being joined.
    pub fn joining(&mut self, code: String) {
        self.phase = Phase::Joining;
        self.error = None;
        self.joining_code = Some(code);
    }

    /// The app entered `entered` over `conn` as `peer_name`. `fallback_code` is the code to use
    /// when the server told none. Returns the audio steps that follow.
    pub fn entered(
        &mut self,
        conn: u32,
        entered: Entered,
        peer_name: String,
        fallback_code: &str,
    ) -> Vec<Audio> {
        self.connection_id = Some(conn);
        self.room = Some(Room {
            room_id: entered.room_id,
            invite_code: entered
                .invite_code
                .map(String::from)
                .filter(|code| !code.is_empty())
                .unwrap_or_else(|| fallback_code.to_string()),
            peer_id: entered.peer_id.to_string(),
            peer_name,
        });
        self.phase = Phase::Connected;
        self.error = None;
        self.joining_code = None;
        self.reconnect = Reconnect::Idle;
        self.reconnect_error = None;
        self.roster.entered(entered.peer_id, entered.peers)
    }

    /// The app left its room, and the rooms the server then listed are `listed` (or why they
    /// could not be). Whatever the server answered, the person asked to leave: the app is in no
    /// room after this.
    pub fn left(&mut self, listed: &Result<Vec<RoomInfo>, String>) {
        self.room = None;
        self.roster.clear();
        match listed {
            Ok(rooms) => self.listed(rooms),
            Err(e) => {
                self.phase = Phase::Error;
                self.error = Some(e.clone());
            }
        }
    }

    /// The connection is being got back. In a room, its screen stays up and
    /// the reconnect says how it goes; otherwise this is a first connect again.
    pub fn reconnecting(&mut self, was_in_room: bool) {
        self.test_room_invite_code = None;
        if was_in_room {
            self.reconnect = Reconnect::Reconnecting;
            self.reconnect_error = None;
        } else {
            self.phase = Phase::ConnectingServer;
            self.error = None;
            self.joining_code = None;
            self.room = None;
            self.roster.clear();
        }
    }

    /// Every attempt to get the connection back failed, while in a room.
    pub fn reconnect_failed(&mut self, error: String) {
        self.reconnect = Reconnect::Failed;
        self.reconnect_error = Some(error);
    }

    /// The connection was got back to a session that was in no room.
    pub fn reconnected(&mut self, conn: u32, rooms: &[RoomInfo]) {
        self.connection_id = Some(conn);
        self.listed(rooms);
    }

    /// How the audio links are doing: `running` is whether the audio session runs, `healths`
    /// how each link is. Returns the audio steps that follow.
    pub fn following_the_links(
        &mut self,
        conn: u32,
        running: bool,
        healths: Vec<(Uuid, LinkHealth)>,
    ) -> Vec<Audio> {
        if !self.is_the_room_of(conn) {
            return Vec::new();
        }
        if !running {
            // The session ended on its own (a device that would not open):
            // there are no links to follow, and the next thing that happens
            // in the room opens them again.
            self.roster.audio_ended();
            return Vec::new();
        }
        healths
            .into_iter()
            .flat_map(|(peer, health)| self.roster.link_is(peer, health))
            .collect()
    }

    /// Whether the session is in a room over the connection `conn`.
    pub fn is_in_room_of(&self, conn: u32) -> bool {
        self.is_the_room_of(conn)
    }

    /// The audio session was ended to start over on new settings, with the room as it is:
    /// returns the audio steps that bring it back (the new address, the session, the links).
    pub fn audio_restarting(&mut self, conn: u32) -> Vec<Audio> {
        if !self.is_the_room_of(conn) {
            return Vec::new();
        }
        self.roster.audio_restarting()
    }

    /// A participant joined the room of `conn`.
    pub fn peer_joined(&mut self, conn: u32, peer: PeerInfo) {
        if self.is_the_room_of(conn) {
            self.roster.joined(peer);
        }
    }

    /// A participant of the room of `conn` published or changed its address.
    pub fn peer_updated(&mut self, conn: u32, peer: PeerInfo) -> Vec<Audio> {
        if self.is_the_room_of(conn) {
            self.roster.updated(peer)
        } else {
            Vec::new()
        }
    }

    /// A participant left the room of `conn`.
    pub fn peer_left(&mut self, conn: u32, peer_id: Uuid) -> Vec<Audio> {
        if self.is_the_room_of(conn) {
            self.roster.left(peer_id)
        } else {
            Vec::new()
        }
    }

    /// The audio link to `peer` could not be started.
    pub fn start_failed(&mut self, peer: Uuid) {
        self.roster.start_failed(peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::signaling::{AddressCandidate, InviteCode};

    fn peer(id: u128, address: Option<&str>) -> PeerInfo {
        PeerInfo {
            id: Uuid::from_u128(id),
            name: format!("peer-{id}"),
            candidates: address
                .map(|a| vec![AddressCandidate::host(a.parse().unwrap())])
                .unwrap_or_default(),
            public_addr: None,
            local_addr: None,
            joined_at: 0,
            features: vec![],
            link_key: None,
        }
    }

    fn room(test_room: bool) -> RoomInfo {
        RoomInfo {
            id: "room".to_string(),
            name: "room".to_string(),
            peer_count: 0,
            max_peers: 10,
            has_password: false,
            invite_code: InviteCode::generate(),
            test_room,
        }
    }

    fn entered(code: Option<&str>, others: Vec<PeerInfo>) -> Entered {
        Entered {
            room_id: "room-id".to_string(),
            peer_id: Uuid::from_u128(1),
            invite_code: code.map(|c| c.parse::<InviteCode>().unwrap()),
            peers: others,
            created: false,
        }
    }

    /// A session in a room entered over connection 7 with `others` already in it
    fn in_a_room(others: Vec<PeerInfo>) -> State {
        let mut state = State::default();
        state.connected(7);
        state.entered(7, entered(Some("ABC234XYZ"), others), "Aki".into(), "");
        state
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_rooms_are_listed_the_session_is_connected_to_the_server_with_the_test_room_if_any()
    {
        let mut state = State::default();
        assert_eq!(state.view().phase, Phase::ConnectingServer);

        let rooms = [room(false), room(true)];
        state.listed(&rooms);

        let view = state.view();
        assert_eq!(view.phase, Phase::ServerConnected);
        assert_eq!(
            view.test_room_invite_code,
            Some(rooms[1].invite_code.to_string())
        );
        assert_eq!(view.room, None);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_a_room_is_entered_it_is_shown_with_the_code_the_participants_and_the_audio_to_start() {
        let mut state = State::default();
        state.connected(7);

        let audio = state.entered(
            7,
            entered(Some("ABC234XYZ"), vec![peer(2, Some("10.0.0.2:5000"))]),
            "Aki".into(),
            "",
        );

        let view = state.view();
        assert_eq!(view.phase, Phase::Connected);
        let room = view.room.expect("in a room");
        assert_eq!(room.room_id, "room-id");
        assert_eq!(room.invite_code, "ABC234XYZ");
        assert_eq!(room.peer_name, "Aki");
        assert_eq!(room.participants.len(), 1);
        assert!(matches!(audio.first(), Some(Audio::Advertise)));
        assert!(audio.iter().any(|step| matches!(step, Audio::Start { .. })));
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_server_tells_no_code_the_one_the_app_asked_with_is_the_rooms() {
        let mut state = State::default();

        state.entered(7, entered(None, vec![]), "Aki".into(), "XYZ234ABC");

        assert_eq!(state.view().room.unwrap().invite_code, "XYZ234ABC");
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_connection_is_lost_the_room_is_rejoined_by_its_code_or_else_its_id() {
        let mut state = in_a_room(vec![]);
        assert_eq!(state.rejoin_code(), Some("ABC234XYZ".to_string()));

        let mut without_code = State::default();
        without_code.entered(7, entered(None, vec![]), "Aki".into(), "");
        assert_eq!(without_code.rejoin_code(), Some("room-id".to_string()));

        state.left(&Ok(vec![]));
        assert_eq!(state.rejoin_code(), None);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_room_is_left_the_session_is_in_no_room_and_back_on_the_list_or_failed() {
        let mut state = in_a_room(vec![peer(2, None)]);

        state.left(&Ok(vec![room(true)]));
        let view = state.view();
        assert_eq!((view.phase, view.room), (Phase::ServerConnected, None));
        assert!(view.test_room_invite_code.is_some());

        let mut state = in_a_room(vec![peer(2, None)]);
        state.left(&Err("the server hung up".to_string()));
        let view = state.view();
        assert_eq!((view.phase, view.room), (Phase::Error, None));
        assert_eq!(view.error.as_deref(), Some("the server hung up"));
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_connection_is_got_back_in_a_room_the_room_stays_and_the_reconnect_says_how_it_goes()
    {
        let mut state = in_a_room(vec![peer(2, None)]);

        state.reconnecting(true);
        let view = state.view();
        assert_eq!(view.phase, Phase::Connected);
        assert!(view.room.is_some());
        assert_eq!(view.signaling_reconnect, Reconnect::Reconnecting);

        state.reconnect_failed("no route".to_string());
        let view = state.view();
        assert_eq!(view.signaling_reconnect, Reconnect::Failed);
        assert_eq!(view.signaling_reconnect_error.as_deref(), Some("no route"));

        state.entered(8, entered(Some("ABC234XYZ"), vec![]), "Aki".into(), "");
        let view = state.view();
        assert_eq!(view.signaling_reconnect, Reconnect::Idle);
        assert_eq!(view.signaling_reconnect_error, None);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_connection_is_got_back_outside_a_room_it_is_a_first_connect_again() {
        let mut state = State::default();
        state.listed(&[]);

        state.reconnecting(false);

        let view = state.view();
        assert_eq!(view.phase, Phase::ConnectingServer);
        assert_eq!(view.test_room_invite_code, None);
        assert_eq!(view.signaling_reconnect, Reconnect::Idle);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_a_step_fails_the_phase_is_error_with_the_reason_and_the_code_being_joined_is_dropped() {
        let mut state = State::default();
        state.joining("ABC234XYZ".to_string());
        assert_eq!(state.view().phase, Phase::Joining);
        assert_eq!(state.view().joining_code.as_deref(), Some("ABC234XYZ"));

        state.failed("no such room".to_string());

        let view = state.view();
        assert_eq!(view.phase, Phase::Error);
        assert_eq!(view.error.as_deref(), Some("no such room"));
        assert_eq!(view.joining_code, None);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_connection_is_dropped_the_audio_ends_and_the_connection_is_handed_back_to_close() {
        let mut state = in_a_room(vec![peer(2, Some("10.0.0.2:5000"))]);
        assert_eq!(state.view().streaming_peer_ids.len(), 1);

        let (conn, in_room) = state.dropping_the_connection();

        assert_eq!((conn, in_room), (Some(7), true));
        assert_eq!(state.connection_id(), None);
        assert!(state.view().streaming_peer_ids.is_empty());
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_an_event_is_of_another_connection_or_of_no_room_the_session_is_left_as_it_is() {
        let mut state = in_a_room(vec![]);

        state.peer_joined(99, peer(2, None));
        assert!(state
            .peer_updated(99, peer(2, Some("10.0.0.2:5000")))
            .is_empty());
        assert!(state.peer_left(99, Uuid::from_u128(2)).is_empty());
        assert!(state.view().room.unwrap().participants.is_empty());

        state.peer_joined(7, peer(2, None));
        assert_eq!(state.view().room.unwrap().participants.len(), 1);
        state.peer_left(7, Uuid::from_u128(2));
        assert!(state.view().room.unwrap().participants.is_empty());

        let mut no_room = State::default();
        no_room.connected(7);
        no_room.peer_joined(7, peer(2, None));
        assert!(no_room.view().room.is_none());
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn when_the_audio_session_ended_on_its_own_the_links_are_forgotten_and_nothing_is_followed() {
        let mut state = in_a_room(vec![peer(2, Some("10.0.0.2:5000"))]);
        assert_eq!(state.view().streaming_peer_ids.len(), 1);

        let steps = state.following_the_links(7, false, Vec::new());

        assert!(steps.is_empty());
        assert!(state.view().streaming_peer_ids.is_empty());
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn a_step_is_ended_by_a_new_epoch_and_an_event_of_a_let_go_connection_ends_nothing() {
        let mut state = State::default();
        state.connected(7);
        let first = state.epoch();

        assert_eq!(state.next_epoch_of(8), None);
        assert_eq!(state.epoch(), first);
        let second = state.next_epoch_of(7).expect("the session's connection");

        assert!(second > first);
        assert_eq!(state.epoch(), second);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn a_change_is_announced_with_a_higher_revision_and_one_that_changes_nothing_is_not() {
        let mut state = State::default();
        let first = state.announcement().expect("the first state is told");

        assert_eq!(state.announcement(), None);

        state.connected(7);
        let second = state.announcement().expect("a connection is a change");
        assert!(second.revision > first.revision);
        assert_eq!(second.connection_id, Some(7));
        assert_eq!(state.announcement(), None);
    }
}
