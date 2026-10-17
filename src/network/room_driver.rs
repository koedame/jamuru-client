//! The steps that carry a participant through the room procedure: connecting to the signaling
//! server, listing its rooms, creating, joining and leaving one, opening an audio link to
//! everyone who publishes an address, and connecting again when the connection is lost
//! (ADR-044 §6). The app and the CLI run the same [`Driver`]; they differ in the [`Backend`]
//! they give it.
//!
//! Deciding about the session is [`State`]'s and about the peers [`Roster`](super::Roster)'s, both
//! without I/O; this is the part that talks to the server and the audio engine through the
//! backend, one step at a time, and keeps the state. A step can be ended by another: each runs for
//! an epoch of the state, and stops, leaving the session alone, when it finds the epoch changed.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use uuid::Uuid;

use super::room_entry::Entered;
use super::room_session::{Reconnect, Snapshot, State};
use super::roster::{Audio, LinkHealth};
use super::signaling::{PeerInfo, RoomInfo};
use super::NetworkError;

/// How many times to reach the signaling server again after it is lost before giving up and
/// leaving the retry to the person.
const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// How long the driver waits between its steps
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// The wait before reconnect attempt N + 1 is N times this, so attempts back off.
    pub reconnect_base_delay: Duration,
    /// How often the server's time is looked at again while the computer's clock is what keeps
    /// the participant from connecting.
    pub clock_recheck_interval: Duration,
    /// How often the room's events are read.
    pub poll_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            reconnect_base_delay: Duration::from_secs(2),
            clock_recheck_interval: Duration::from_secs(10),
            poll_interval: Duration::from_millis(500),
        }
    }
}

/// What the room says, as far as the session follows it
#[derive(Debug, Clone)]
pub enum RoomEvent {
    PeerJoined(PeerInfo),
    PeerUpdated(PeerInfo),
    PeerLeft(Uuid),
    /// The connection dropped (a network blip, a proxy reset, a server restart): it is dead
    /// either way
    ConnectionLost(String),
}

/// One thing the connection to the server brought: what the session follows, or something of
/// the backend's own (a chat line, say), which the backend is handed in order.
pub enum Polled<O> {
    Room(RoomEvent),
    Other(O),
}

/// What the driver steers: the connection to the server, the audio engine, and what is shown of
/// the session. Each connection to the server has a number, which the driver keeps with the
/// session to tell an event of a connection it let go of.
pub trait Backend: Send + Sync + 'static {
    /// What the connection brings besides the room's events
    type Other: Send + 'static;

    /// Connects to the server and makes the connection known by a number
    fn connect(&self) -> impl Future<Output = Result<u32, NetworkError>> + Send;
    fn disconnect(&self, conn: u32) -> impl Future<Output = Result<(), String>> + Send;
    fn list_rooms(&self, conn: u32) -> impl Future<Output = Result<Vec<RoomInfo>, String>> + Send;
    /// Makes a room and enters it as `peer_name`
    fn create_room(
        &self,
        conn: u32,
        peer_name: &str,
    ) -> impl Future<Output = Result<Entered, String>> + Send;
    /// Enters the room `code` names as `peer_name`
    fn join_room(
        &self,
        conn: u32,
        code: &str,
        peer_name: &str,
    ) -> impl Future<Output = Result<Entered, String>> + Send;
    fn leave_room(&self, conn: u32) -> impl Future<Output = Result<(), String>> + Send;
    /// What has come from the server since this was last asked, oldest first
    fn poll_events(
        &self,
        conn: u32,
    ) -> impl Future<Output = Result<Vec<Polled<Self::Other>>, String>> + Send;
    fn other_event(&self, event: Self::Other) -> impl Future<Output = ()> + Send;
    /// A room event, as it comes and before the session follows it
    fn room_event(&self, _event: &RoomEvent) {}
    /// The name the participant enters rooms with
    fn peer_name(&self) -> String;
    /// Whether the computer's clock agrees with the server's closely enough to connect
    fn clock_is_right(&self) -> impl Future<Output = bool> + Send;

    /// Ends the audio session: every link, the devices and the socket
    fn stop_audio(&self) -> impl Future<Output = ()> + Send;
    /// Whether the audio session runs, and how each of its links is doing
    fn audio_state(&self) -> (bool, Vec<(Uuid, LinkHealth)>);
    /// Tells the room where to send audio
    fn advertise(&self, conn: u32) -> impl Future<Output = Result<(), String>> + Send;
    /// Starts the audio session on the devices if none runs, with no link, so the microphone is
    /// heard while the participant is alone in the room
    fn open_audio(&self) -> impl Future<Output = Result<(), String>> + Send;
    /// Opens an audio link to `peer`, racing `candidates` (best first), starting the audio
    /// session if none runs. `link_key` is the key the server gave for `peer`.
    fn link(
        &self,
        conn: u32,
        peer: Uuid,
        candidates: Vec<std::net::SocketAddr>,
        link_key: Option<String>,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn unlink(&self, peer: Uuid) -> impl Future<Output = Result<(), String>> + Send;

    /// The session changed to `snapshot`, which differs from what was last announced
    fn announce(&self, snapshot: &Snapshot);
}

/// The session: its state, and the lock the steps that talk to the server take turns with.
/// The app keeps one for its whole life and gives it to a [`Driver`] for each backend it runs.
pub struct Session {
    timing: Timing,
    state: Mutex<State>,
    /// Held by a step that talks to the server, so two do not interleave their messages or
    /// their changes. A step that is to be ended does not wait on it: it raises the epoch
    /// first, and the step that holds it lets go at its next check.
    flow: tokio::sync::Mutex<()>,
}

impl Session {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            state: Mutex::new(State::default()),
            flow: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The session as it is
    pub fn snapshot(&self) -> Snapshot {
        self.lock().view()
    }
}

/// The steps that change a session, run against a backend
pub struct Driver<B: Backend> {
    session: Arc<Session>,
    backend: Arc<B>,
}

impl<B: Backend> Clone for Driver<B> {
    fn clone(&self) -> Self {
        Self {
            session: self.session.clone(),
            backend: self.backend.clone(),
        }
    }
}

impl<B: Backend> Driver<B> {
    pub fn new(session: Arc<Session>, backend: Arc<B>) -> Self {
        Self { session, backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.session.lock()
    }

    /// The session as it is
    pub fn snapshot(&self) -> Snapshot {
        self.session.snapshot()
    }

    fn epoch(&self) -> u64 {
        self.lock().epoch()
    }

    /// Ends whatever step is running (it stops at its next check) and returns the number of the
    /// one that takes over.
    fn next_epoch(&self) -> u64 {
        self.lock().next_epoch()
    }

    /// Changes the session and, if that made a difference to what was last announced, announces
    /// it.
    fn change<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let (value, announce) = {
            let mut state = self.lock();
            let value = change(&mut state);
            (value, state.announcement())
        };
        if let Some(snapshot) = announce {
            self.backend().announce(&snapshot);
        }
        value
    }

    // =========================================================================
    // The steps a person asks for
    // =========================================================================

    /// Drops the connection there is and connects to the server again, from scratch. Returns
    /// when the connection is made or has failed.
    pub async fn connect_afresh(&self) -> Result<Snapshot, String> {
        let epoch = self.next_epoch();
        self.connect(epoch).await
    }

    /// Creates a room and enters it.
    pub async fn create(&self) -> Result<Snapshot, String> {
        let _flow = self.session.flow.lock().await;
        let conn = self.connection_to_enter()?;
        let name = self.backend().peer_name();
        self.change(|session| session.creating());

        let entered = match self.backend().create_room(conn, &name).await {
            Ok(entered) => entered,
            Err(e) => return self.fail(e),
        };
        let audio = self.change(|session| session.entered(conn, entered, name, ""));
        self.run_audio(conn, audio).await;
        Ok(self.snapshot())
    }

    /// Joins the room `code` names (an invite code, or a room id from a link).
    pub async fn join(&self, code: String) -> Result<Snapshot, String> {
        let _flow = self.session.flow.lock().await;
        let conn = self.connection_to_enter()?;
        let name = self.backend().peer_name();
        self.change(|session| session.joining(code.clone()));

        let entered = match self.backend().join_room(conn, &code, &name).await {
            Ok(entered) => entered,
            Err(e) => return self.fail(e),
        };
        let audio = self.change(|session| session.entered(conn, entered, name, ""));
        self.run_audio(conn, audio).await;
        Ok(self.snapshot())
    }

    /// Leaves the room and goes back to the room list. With no connection to the server (it was
    /// lost and is being got back), ends that and connects from scratch instead.
    pub async fn leave(&self) -> Result<Snapshot, String> {
        if self.lock().connection_id().is_none() || self.lock().reconnect() != Reconnect::Idle {
            // The connection is gone and is being got back. The person no longer wants the
            // room: end that, and start from the server.
            return self.connect_afresh().await;
        }

        let flow = self.session.flow.lock().await;
        let Some(conn) = self.lock().connection_id() else {
            // Lost while waiting for a turn.
            drop(flow);
            return self.connect_afresh().await;
        };
        if !self.lock().in_room() {
            return Err("Not in a room".to_string());
        }

        self.backend().stop_audio().await;
        // Whatever the server answers, the person asked to leave: the participant is in no room
        // after this, and a connection lost from here on is not a reason to rejoin the one they
        // left.
        let left = self.backend().leave_room(conn).await;
        let listed = match left {
            Ok(()) => self.backend().list_rooms(conn).await,
            Err(e) => Err(e),
        };
        self.change(|session| session.left(&listed));
        listed.map(|_| self.snapshot())
    }

    /// Tries again to get the connection back and rejoin the room, after every attempt failed.
    pub async fn retry_reconnect(&self) -> Result<Snapshot, String> {
        if self.lock().rejoin_code().is_none() {
            return Err("Not in a room".to_string());
        }
        let epoch = self.next_epoch();
        self.reconnect(epoch).await
    }

    /// Leaves the room, if in one, lets go of the connection and ends whatever step is running:
    /// the session is done with.
    pub async fn end(&self) {
        self.next_epoch();
        let _flow = self.session.flow.lock().await;
        let (conn, in_room) = {
            let session = self.lock();
            (session.connection_id(), session.in_room())
        };
        if let (Some(conn), true) = (conn, in_room) {
            if let Err(e) = self.backend().leave_room(conn).await {
                tracing::error!("Failed to leave the room: {}", e);
            }
        }
        self.drop_connection().await;
    }

    // =========================================================================
    // Steps
    // =========================================================================

    /// Ends the step with `message` as what went wrong.
    fn fail(&self, message: String) -> Result<Snapshot, String> {
        self.change(|session| session.failed(message.clone()));
        Err(message)
    }

    /// [`Driver::fail`], unless the step that failed was ended, in which case whatever ended it
    /// says where the session is.
    fn fail_unless_ended(&self, epoch: u64, message: String) -> Result<Snapshot, String> {
        if self.epoch() == epoch {
            self.fail(message)
        } else {
            Ok(self.snapshot())
        }
    }

    async fn disconnect(&self, conn: u32) {
        if let Err(e) = self.backend().disconnect(conn).await {
            tracing::error!("Failed to disconnect: {}", e);
        }
    }

    /// Ends the connection there is: the audio, then the server connection. The state is left
    /// for the caller to change in the same breath.
    async fn drop_connection(&self) {
        let (conn, in_room) = self.lock().dropping_the_connection();
        if in_room {
            self.backend().stop_audio().await;
        }
        if let Some(conn) = conn {
            self.disconnect(conn).await;
        }
    }

    /// Waits `total`, or until the step is ended, whichever is first.
    async fn pause(&self, epoch: u64, total: Duration) {
        let step = Duration::from_millis(100);
        let mut waited = Duration::ZERO;
        while waited < total && self.epoch() == epoch {
            tokio::time::sleep(step).await;
            waited += step;
        }
    }

    /// Runs `step` until it finishes or the step this belongs to is ended, whichever is first.
    /// None when it was ended: `step` is dropped unfinished, which is safe for the calls to the
    /// server made here (each either completes its change or leaves none).
    async fn unless_ended<T>(&self, epoch: u64, step: impl Future<Output = T>) -> Option<T> {
        let mut step = std::pin::pin!(step);
        loop {
            match tokio::time::timeout(Duration::from_millis(100), &mut step).await {
                Ok(value) => return Some(value),
                Err(_) if self.epoch() != epoch => return None,
                Err(_) => {}
            }
        }
    }

    /// Connects to the server and lists its rooms, from nothing: whatever connection or room
    /// there was is dropped first.
    async fn connect(&self, epoch: u64) -> Result<Snapshot, String> {
        let _flow = self.session.flow.lock().await;
        if self.epoch() != epoch {
            return Ok(self.snapshot());
        }

        self.drop_connection().await;
        self.change(|session| session.connecting_server());

        let connected = self.unless_ended(epoch, self.backend().connect()).await;
        let conn = match connected {
            None => return Ok(self.snapshot()),
            Some(Ok(conn)) => conn,
            Some(Err(e)) => {
                let clock_is_off = matches!(e, NetworkError::ClockSkew { .. });
                let failed = self.fail_unless_ended(epoch, e.to_string());
                if clock_is_off && self.epoch() == epoch {
                    self.connect_again_when_the_clock_is_right(epoch);
                }
                return failed;
            }
        };
        if self.epoch() != epoch {
            self.disconnect(conn).await;
            return Ok(self.snapshot());
        }

        self.change(|session| session.connected(conn));
        self.spawn_pump(conn);
        // Ended before the list came: the step that ended this one drops the connection, since
        // it is the session's now.
        match self
            .unless_ended(epoch, self.backend().list_rooms(conn))
            .await
        {
            None => Ok(self.snapshot()),
            Some(Ok(rooms)) => {
                self.change(|session| session.listed(&rooms));
                Ok(self.snapshot())
            }
            Some(Err(e)) => self.fail_unless_ended(epoch, e),
        }
    }

    /// After the server refused the connection because this computer's clock is off, connects
    /// again once the clock agrees with the server's, so putting the clock right is all the
    /// person has to do. Ends when the step is ended (the person connected again by hand, or
    /// left), as `epoch` says.
    fn connect_again_when_the_clock_is_right(&self, epoch: u64) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.pause(epoch, this.session.timing.clock_recheck_interval)
                    .await;
                if this.epoch() != epoch {
                    return;
                }
                if this.backend().clock_is_right().await {
                    break;
                }
            }
            let _ = this.connect(epoch).await;
        });
    }

    /// The connection the room's operations use, if a room can be entered now.
    fn connection_to_enter(&self) -> Result<u32, String> {
        let session = self.lock();
        if session.in_room() {
            return Err("Already in a room".to_string());
        }
        session
            .connection_id()
            .ok_or_else(|| "Not connected to the signaling server".to_string())
    }

    /// Reaches the server again after the connection dropped, and rejoins the room the
    /// participant is in, if it is in one.
    ///
    /// Retries with a backoff. While a room is open its screen stays up and the reconnect
    /// reports progress; otherwise the phase is `connecting_server` and, on giving up, `error` -
    /// what a failed first connect looks like.
    async fn reconnect(&self, epoch: u64) -> Result<Snapshot, String> {
        let _flow = self.session.flow.lock().await;
        if self.epoch() != epoch {
            return Ok(self.snapshot());
        }
        // Read once it is this step's turn: the room the participant is in now is the one to
        // rejoin, not the one it was in when the connection was lost.
        let rejoin = self.lock().rejoin_code();

        self.drop_connection().await;
        let was_in_room = rejoin.is_some();
        self.change(|session| session.reconnecting(was_in_room));

        for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
            if self.epoch() != epoch {
                return Ok(self.snapshot());
            }
            let outcome = self.try_reconnect(epoch, rejoin.as_deref()).await;
            let Err(e) = outcome else {
                return Ok(self.snapshot());
            };
            tracing::warn!(
                "Signaling reconnect attempt {}/{} failed: {}",
                attempt,
                MAX_RECONNECT_ATTEMPTS,
                e
            );
            if attempt == MAX_RECONNECT_ATTEMPTS {
                if was_in_room {
                    self.change(|session| session.reconnect_failed(e.clone()));
                    return Err(e);
                }
                return self.fail_unless_ended(epoch, e);
            }
            self.pause(epoch, self.session.timing.reconnect_base_delay * attempt)
                .await;
        }
        Ok(self.snapshot())
    }

    /// One attempt of [`Driver::reconnect`]. Ok also when the step was ended meanwhile.
    async fn try_reconnect(&self, epoch: u64, rejoin: Option<&str>) -> Result<(), String> {
        let connected = self.unless_ended(epoch, self.backend().connect()).await;
        let conn = match connected {
            None => return Ok(()),
            Some(conn) => conn.map_err(|e| e.to_string())?,
        };
        if self.epoch() != epoch {
            self.disconnect(conn).await;
            return Ok(());
        }

        let entered = self
            .unless_ended(epoch, async {
                match rejoin {
                    Some(code) => {
                        let name = self.backend().peer_name();
                        let entered = self.backend().join_room(conn, code, &name).await?;
                        let audio =
                            self.change(|session| session.entered(conn, entered, name, code));
                        self.spawn_pump(conn);
                        self.run_audio(conn, audio).await;
                    }
                    None => {
                        let rooms = self.backend().list_rooms(conn).await?;
                        self.change(|session| session.reconnected(conn, &rooms));
                        self.spawn_pump(conn);
                    }
                }
                Ok::<(), String>(())
            })
            .await;
        match entered {
            Some(Ok(())) => Ok(()),
            // Ended, or failed: either way the connection made for this attempt is no use to
            // anyone.
            Some(Err(e)) => {
                self.disconnect(conn).await;
                Err(e)
            }
            None => {
                self.disconnect(conn).await;
                Ok(())
            }
        }
    }

    // =========================================================================
    // The room's events
    // =========================================================================

    /// Reads the room's events from `conn` for as long as it is the session's connection. A drop
    /// while merely connected to the server (browsing the room list) needs the same detection as
    /// one in a room.
    fn spawn_pump(&self, conn: u32) {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(this.session.timing.poll_interval).await;
                if this.lock().connection_id() != Some(conn) {
                    return;
                }
                let events = match this.backend().poll_events(conn).await {
                    Ok(events) => events,
                    Err(e) => {
                        tracing::error!("Failed to poll signaling events: {}", e);
                        continue;
                    }
                };
                if !events.is_empty() && !this.handle_events(conn, events).await {
                    return;
                }
                this.follow_the_audio_links(conn).await;
            }
        });
    }

    /// Tells the roster how each audio link is doing, and carries out what it then does about
    /// the audio. The server tells of a peer that left, not of one whose connection it has not
    /// yet found dead, so a link gone silent is the only sign that its peer is no longer there to
    /// hear it.
    async fn follow_the_audio_links(&self, conn: u32) {
        let _flow = self.session.flow.lock().await;
        // Read once it is this step's turn: a step that was opening links while this one waited
        // has opened them by now.
        let (running, healths) = self.backend().audio_state();
        let audio = self.change(|session| session.following_the_links(conn, running, healths));
        self.run_audio(conn, audio).await;
    }

    /// Ends the connection `conn` and gets it back, rejoining the room the participant is in,
    /// unless `conn` is no longer the session's.
    fn get_connection_back(&self, conn: u32) {
        let Some(epoch) = self.lock().next_epoch_of(conn) else {
            return;
        };
        let this = self.clone();
        tokio::spawn(async move {
            let _ = this.reconnect(epoch).await;
        });
    }

    /// Handles one batch of events, in order. False when `conn` is finished with and the events
    /// after the one that ended it are not to be read.
    async fn handle_events(&self, conn: u32, events: Vec<Polled<B::Other>>) -> bool {
        for event in events {
            if let Polled::Room(event) = &event {
                self.backend().room_event(event);
            }
            match event {
                Polled::Other(event) => self.backend().other_event(event).await,
                Polled::Room(RoomEvent::PeerJoined(peer)) => {
                    let _flow = self.session.flow.lock().await;
                    self.change(|session| session.peer_joined(conn, peer));
                }
                Polled::Room(RoomEvent::PeerUpdated(peer)) => {
                    let _flow = self.session.flow.lock().await;
                    let audio = self.change(|session| session.peer_updated(conn, peer));
                    self.run_audio(conn, audio).await;
                }
                Polled::Room(RoomEvent::PeerLeft(peer_id)) => {
                    let _flow = self.session.flow.lock().await;
                    let audio = self.change(|session| session.peer_left(conn, peer_id));
                    self.run_audio(conn, audio).await;
                }
                Polled::Room(RoomEvent::ConnectionLost(reason)) => {
                    // A network blip, a proxy reset or a server restart: the connection is dead
                    // either way.
                    tracing::warn!("Signaling connection lost: {}", reason);
                    self.get_connection_back(conn);
                    return false;
                }
            }
        }
        true
    }

    // =========================================================================
    // The audio
    // =========================================================================

    /// Carries out what the roster decided about the audio.
    async fn run_audio(&self, conn: u32, audio: Vec<Audio>) {
        for step in audio {
            match step {
                Audio::Advertise => self.advertise(conn).await,
                Audio::Open => {
                    if let Err(e) = self.backend().open_audio().await {
                        tracing::error!("Failed to start the audio session: {}", e);
                    }
                }
                Audio::Rejoin => {
                    tracing::warn!(
                        "An audio link gave up and nobody is heard: entering the room again"
                    );
                    self.get_connection_back(conn);
                    // The rest is for a room this participant is on its way out of
                    return;
                }
                Audio::End { peer } => {
                    if let Err(e) = self.backend().unlink(peer).await {
                        tracing::error!("Failed to close an audio link: {}", e);
                    }
                }
                Audio::Start {
                    peer,
                    candidates,
                    link_key,
                } => {
                    let candidate_count = candidates.len();
                    match self.backend().link(conn, peer, candidates, link_key).await {
                        Ok(()) => tracing::info!(
                            "Audio link started ({} address candidate(s))",
                            candidate_count
                        ),
                        Err(e) => {
                            // Allow a later update to retry rather than leaving the participant
                            // permanently unheard.
                            self.change(|session| session.start_failed(peer));
                            tracing::error!("Failed to start an audio link: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Advertises this participant's audio address to the room.
    ///
    /// Not fatal for chat or the participant list, which go through the signaling server; only
    /// audio depends on this.
    async fn advertise(&self, conn: u32) {
        if let Err(e) = self.backend().advertise(conn).await {
            tracing::error!("Failed to publish our audio address: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::network::room_session::Phase;
    use crate::network::signaling::InviteCode;

    /// A server that lets everyone in, and what it was asked, in order
    #[derive(Default)]
    struct FakeBackend {
        asked: Mutex<Vec<String>>,
        /// What each poll brings, oldest first; a poll with nothing left brings nothing
        polls: Mutex<VecDeque<Vec<Polled<()>>>>,
        /// How many connects fail before one is made
        refusing: Mutex<u32>,
        connections: Mutex<u32>,
    }

    impl FakeBackend {
        fn asked(&self, what: &str) -> usize {
            self.asked
                .lock()
                .unwrap()
                .iter()
                .filter(|asked| asked.as_str() == what)
                .count()
        }

        fn ask(&self, what: impl Into<String>) {
            self.asked.lock().unwrap().push(what.into());
        }

        fn entered(&self) -> Entered {
            Entered {
                room_id: "room".to_string(),
                peer_id: Uuid::from_u128(1),
                invite_code: Some("ABC234XYZ".parse::<InviteCode>().unwrap()),
                peers: Vec::new(),
                created: false,
            }
        }
    }

    impl Backend for FakeBackend {
        type Other = ();

        async fn connect(&self) -> Result<u32, NetworkError> {
            self.ask("connect");
            let mut refusing = self.refusing.lock().unwrap();
            if *refusing > 0 {
                *refusing -= 1;
                return Err(NetworkError::ConnectionClosed);
            }
            let mut connections = self.connections.lock().unwrap();
            *connections += 1;
            Ok(*connections)
        }

        async fn disconnect(&self, _conn: u32) -> Result<(), String> {
            self.ask("disconnect");
            Ok(())
        }

        async fn list_rooms(&self, _conn: u32) -> Result<Vec<RoomInfo>, String> {
            self.ask("list_rooms");
            Ok(Vec::new())
        }

        async fn create_room(&self, _conn: u32, _peer_name: &str) -> Result<Entered, String> {
            self.ask("create_room");
            Ok(self.entered())
        }

        async fn join_room(
            &self,
            _conn: u32,
            code: &str,
            _peer_name: &str,
        ) -> Result<Entered, String> {
            self.ask(format!("join_room {code}"));
            Ok(self.entered())
        }

        async fn leave_room(&self, _conn: u32) -> Result<(), String> {
            self.ask("leave_room");
            Ok(())
        }

        async fn poll_events(&self, _conn: u32) -> Result<Vec<Polled<()>>, String> {
            Ok(self.polls.lock().unwrap().pop_front().unwrap_or_default())
        }

        async fn other_event(&self, _event: ()) {}

        fn peer_name(&self) -> String {
            "Aki".to_string()
        }

        async fn clock_is_right(&self) -> bool {
            true
        }

        async fn stop_audio(&self) {
            self.ask("stop_audio");
        }

        fn audio_state(&self) -> (bool, Vec<(Uuid, LinkHealth)>) {
            (false, Vec::new())
        }

        async fn advertise(&self, _conn: u32) -> Result<(), String> {
            self.ask("advertise");
            Ok(())
        }

        async fn open_audio(&self) -> Result<(), String> {
            self.ask("open_audio");
            Ok(())
        }

        async fn link(
            &self,
            _conn: u32,
            _peer: Uuid,
            _candidates: Vec<std::net::SocketAddr>,
            _link_key: Option<String>,
        ) -> Result<(), String> {
            self.ask("link");
            Ok(())
        }

        async fn unlink(&self, _peer: Uuid) -> Result<(), String> {
            self.ask("unlink");
            Ok(())
        }

        fn announce(&self, _snapshot: &Snapshot) {}
    }

    fn driver_over(backend: FakeBackend) -> Driver<FakeBackend> {
        let timing = Timing {
            reconnect_base_delay: Duration::from_millis(10),
            clock_recheck_interval: Duration::from_millis(100),
            poll_interval: Duration::from_millis(10),
        };
        Driver::new(Arc::new(Session::new(timing)), Arc::new(backend))
    }

    async fn until(driver: &Driver<FakeBackend>, wanted: impl Fn(&Snapshot) -> bool) -> Snapshot {
        for _ in 0..500 {
            let snapshot = driver.snapshot();
            if wanted(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "the session never got there; it is at {:?}",
            driver.snapshot()
        );
    }

    /// Verifies: REQ-CON-140
    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn a_room_is_created_after_connecting_lists_the_rooms_first_and_puts_the_participant_in_it(
    ) {
        let driver = driver_over(FakeBackend::default());

        driver.connect_afresh().await.unwrap();
        let snapshot = driver.create().await.unwrap();

        assert_eq!(snapshot.phase, Phase::Connected);
        assert_eq!(
            snapshot.room.map(|room| room.invite_code),
            Some("ABC234XYZ".to_string())
        );
        let asked = driver.backend().asked.lock().unwrap().clone();
        assert_eq!(
            asked,
            [
                "connect",
                "list_rooms",
                "create_room",
                "advertise",
                "open_audio"
            ]
        );
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_connection_is_lost_in_a_room_enters_the_same_room_again_over_a_new_connection() {
        let driver = driver_over(FakeBackend::default());
        driver.connect_afresh().await.unwrap();
        driver.create().await.unwrap();

        driver
            .backend()
            .polls
            .lock()
            .unwrap()
            .push_back(vec![Polled::Room(RoomEvent::ConnectionLost(
                "reset".to_string(),
            ))]);
        until(&driver, |_| {
            driver.backend().asked("join_room ABC234XYZ") == 1
        })
        .await;
        let snapshot = until(&driver, |s| {
            s.phase == Phase::Connected && s.connection_id == Some(2)
        })
        .await;

        assert!(snapshot.room.is_some());
        assert_eq!(driver.backend().asked("connect"), 2);
        assert_eq!(driver.backend().asked("disconnect"), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_server_does_not_answer_the_first_attempts_to_get_the_connection_back_tries_again()
    {
        let driver = driver_over(FakeBackend::default());
        driver.connect_afresh().await.unwrap();
        driver.create().await.unwrap();
        *driver.backend().refusing.lock().unwrap() = 2;

        driver
            .backend()
            .polls
            .lock()
            .unwrap()
            .push_back(vec![Polled::Room(RoomEvent::ConnectionLost(
                "reset".to_string(),
            ))]);
        until(&driver, |_| {
            driver.backend().asked("join_room ABC234XYZ") == 1
        })
        .await;

        assert_eq!(driver.backend().asked("connect"), 4);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_participant_is_done_leaves_the_room_and_lets_go_of_the_connection_for_good() {
        let driver = driver_over(FakeBackend::default());
        driver.connect_afresh().await.unwrap();
        driver.create().await.unwrap();

        driver.end().await;
        // A connection that dropped after this is not a reason to connect again
        driver
            .backend()
            .polls
            .lock()
            .unwrap()
            .push_back(vec![Polled::Room(RoomEvent::ConnectionLost(
                "reset".to_string(),
            ))]);
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(driver.backend().asked("leave_room"), 1);
        assert_eq!(driver.backend().asked("disconnect"), 1);
        assert_eq!(driver.backend().asked("connect"), 1);
    }
}
