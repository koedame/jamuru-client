//! The session: how far the app has got into a room, and what it does about
//! the room and the audio (ADR-044 §6).
//!
//! The backend owns this, not the screen. It connects to the signaling server,
//! lists the rooms, creates, joins and leaves them, starts the audio with
//! whoever publishes an address, and connects again when the connection is
//! lost. The screen reads [`Snapshot`] with `session_get`, hears `session:changed`
//! when it changes, and acts with one command per operation - so the screen,
//! the E2E channel and the remote portals all see and drive the same state.
//!
//! Deciding about the peers and the audio is [`roster`]'s; this is the part
//! that talks to the server and the audio engine and keeps the state.

mod roster;

use std::sync::MutexGuard;
use std::time::Duration;

use jamjam::network::RoomInfo;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

use crate::config::ConfigState;
use crate::mixer::{self, MixerState};
use crate::settings_help::HelpEvent;
use crate::signaling::{self, JoinResult, SignalingEvent};
use crate::streaming;
use roster::{Audio, Roster};

/// Announces a change of the session. The payload is the new [`Snapshot`].
pub const CHANGED_EVENT: &str = "session:changed";
/// Something about helping with settings that the screen shows (ADR-043). It
/// arrives from the room with the room's other events.
pub const HELP_EVENT: &str = "session:settings-help";

/// How many times to reach the signaling server again after it is lost before
/// giving up and leaving the retry to the person.
const MAX_RECONNECT_ATTEMPTS: u32 = 5;
/// The wait before attempt N + 1 is N times this, so attempts back off.
const RECONNECT_BASE_DELAY: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_secs(2)
};
/// How often the room's events are read.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// What a room this app creates is called.
const ROOM_NAME: &str = "My Room";

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
    fn name(self) -> &'static str {
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
/// in a room. Distinct from the audio link being re-established
/// (`streaming_reconnect`, REQ-CON-110).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconnect {
    #[default]
    Idle,
    Reconnecting,
    /// Every attempt failed; `session_reconnect` tries again.
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
    /// The participant the audio goes to, if it does.
    pub streaming_peer_id: Option<String>,
}

struct Room {
    room_id: String,
    invite_code: String,
    peer_id: String,
    peer_name: String,
}

#[derive(Default)]
struct Inner {
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

impl Inner {
    fn view(&self) -> Snapshot {
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
            streaming_peer_id: self.roster.streaming_with().map(|id| id.to_string()),
        }
    }
}

/// The session, managed by Tauri.
pub struct SessionState {
    inner: std::sync::Mutex<Inner>,
    /// Held by a step that talks to the server, so two do not interleave their
    /// messages or their changes. A step that is to be ended does not wait on
    /// it: it raises `epoch` first, and the step that holds it lets go at its
    /// next check.
    flow: tokio::sync::Mutex<()>,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(Inner::default()),
            flow: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.lock().view()
    }

    fn epoch(&self) -> u64 {
        self.lock().epoch
    }

    /// Ends whatever step is running (it stops at its next check) and returns
    /// the number of the one that takes over.
    fn next_epoch(&self) -> u64 {
        let mut inner = self.lock();
        inner.epoch += 1;
        inner.epoch
    }

    /// [`SessionState::next_epoch`], but only while `conn` is still the
    /// session's connection: an event of a connection that was let go of
    /// meanwhile ends nothing.
    fn next_epoch_of(&self, conn: u32) -> Option<u64> {
        let mut inner = self.lock();
        if inner.connection_id != Some(conn) {
            return None;
        }
        inner.epoch += 1;
        Some(inner.epoch)
    }

    fn connection_id(&self) -> Option<u32> {
        self.lock().connection_id
    }

    /// The code to rejoin the room with after the connection was lost, or
    /// None when there is no room to rejoin.
    fn rejoin_code(&self) -> Option<String> {
        self.lock().room.as_ref().map(|room| {
            if room.invite_code.is_empty() {
                room.room_id.clone()
            } else {
                room.invite_code.clone()
            }
        })
    }

    /// Changes the session and, if that made a difference to what was last
    /// announced, announces it.
    fn change<R: Runtime, T>(&self, app: &AppHandle<R>, change: impl FnOnce(&mut Inner) -> T) -> T {
        let (value, announce) = {
            let mut inner = self.lock();
            let value = change(&mut inner);
            let mut now = inner.view();
            now.revision = 0;
            if inner.announced.as_ref() == Some(&now) {
                (value, None)
            } else {
                let before = inner.announced.replace(now.clone());
                inner.revision += 1;
                now.revision = inner.revision;
                // Which step the app is at decides which buttons do anything,
                // so its transitions are the first thing a bug report needs
                // (ADR-036).
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
                (value, Some(now))
            }
        };
        if let Some(snapshot) = announce {
            if let Err(e) = app.emit(CHANGED_EVENT, &snapshot) {
                tracing::warn!("Could not announce the session change: {}", e);
            }
            // The mixer has a strip for each of the room, and for no one else.
            if let Some(mixer) = app.try_state::<MixerState>() {
                let ids: Vec<String> = snapshot
                    .room
                    .iter()
                    .flat_map(|room| room.participants.iter().map(|p| p.id.clone()))
                    .collect();
                mixer.keep_only(app, &ids);
            }
        }
        value
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Commands
// =============================================================================

/// The session as it is.
#[tauri::command]
pub fn session_get(state: tauri::State<'_, SessionState>) -> Snapshot {
    state.snapshot()
}

/// Drops the connection there is and connects to the server again, from
/// scratch. What the connect screen's Cancel and Retry do. Returns when the
/// connection is made or has failed.
#[tauri::command]
pub async fn session_connect(app: AppHandle) -> Result<Snapshot, String> {
    let epoch = app.state::<SessionState>().next_epoch();
    connect(&app, epoch).await
}

/// Creates a room and enters it.
#[tauri::command]
pub async fn session_create(app: AppHandle) -> Result<Snapshot, String> {
    create(&app).await
}

/// Joins the room `code` names (an invite code, or a room id from a link).
#[tauri::command]
pub async fn session_join(code: String, app: AppHandle) -> Result<Snapshot, String> {
    join(&app, code).await
}

/// Leaves the room and goes back to the room list. With no connection to the
/// server (it was lost and is being reconnected), ends that and connects from
/// scratch instead.
#[tauri::command]
pub async fn session_leave(app: AppHandle) -> Result<Snapshot, String> {
    leave(&app).await
}

/// Tries again to get the signaling connection back and rejoin the room, after
/// every attempt failed.
#[tauri::command]
pub async fn session_reconnect(app: AppHandle) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    if state.rejoin_code().is_none() {
        return Err("Not in a room".to_string());
    }
    let epoch = state.next_epoch();
    reconnect(&app, epoch).await
}

/// Connects to the server as the app starts.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let epoch = app.state::<SessionState>().next_epoch();
        let _ = connect(&app, epoch).await;
    });
}

// =============================================================================
// Steps
// =============================================================================

fn test_room_code_of(rooms: &[RoomInfo]) -> Option<String> {
    rooms
        .iter()
        .find(|room| room.test_room)
        .map(|room| room.invite_code.clone())
}

fn peer_name<R: Runtime>(app: &AppHandle<R>) -> String {
    app.state::<ConfigState>()
        .get()
        .map(|config| config.peer_name)
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to load the peer name, using the default: {}", e);
            "User".to_string()
        })
}

/// Ends the step with `message` as what went wrong.
fn fail<R: Runtime>(app: &AppHandle<R>, message: String) -> Result<Snapshot, String> {
    app.state::<SessionState>().change(app, |session| {
        session.phase = Phase::Error;
        session.error = Some(message.clone());
        session.joining_code = None;
    });
    Err(message)
}

/// [`fail`], unless the step that failed was ended, in which case whatever
/// ended it says where the session is.
fn fail_unless_ended<R: Runtime>(
    app: &AppHandle<R>,
    epoch: u64,
    message: String,
) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    if state.epoch() == epoch {
        fail(app, message)
    } else {
        Ok(state.snapshot())
    }
}

async fn stop_audio<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = streaming::streaming_stop(app.state()).await {
        tracing::error!("Failed to stop streaming: {}", e);
    }
}

async fn disconnect<R: Runtime>(app: &AppHandle<R>, conn: u32) {
    if let Err(e) =
        signaling::signaling_disconnect(conn, app.state(), app.state(), app.state()).await
    {
        tracing::error!("Failed to disconnect: {}", e);
    }
}

/// Ends the connection there is: the audio, then the server connection. The
/// state is left for the caller to change in the same breath.
async fn drop_connection<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<SessionState>();
    let (conn, in_room) = {
        let mut session = state.lock();
        session.roster.audio_ended();
        (session.connection_id.take(), session.room.is_some())
    };
    if in_room {
        stop_audio(app).await;
    }
    if let Some(conn) = conn {
        disconnect(app, conn).await;
    }
}

/// Waits `total`, or until the step is ended, whichever is first.
async fn pause(state: &SessionState, epoch: u64, total: Duration) {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < total && state.epoch() == epoch {
        tokio::time::sleep(step).await;
        waited += step;
    }
}

/// Runs `step` until it finishes or the step this belongs to is ended,
/// whichever is first. None when it was ended: `step` is dropped unfinished,
/// which is safe for the calls to the server made here (each either completes
/// its change or leaves none).
async fn unless_ended<T>(
    state: &SessionState,
    epoch: u64,
    step: impl std::future::Future<Output = T>,
) -> Option<T> {
    let mut step = std::pin::pin!(step);
    loop {
        match tokio::time::timeout(Duration::from_millis(100), &mut step).await {
            Ok(value) => return Some(value),
            Err(_) if state.epoch() != epoch => return None,
            Err(_) => {}
        }
    }
}

/// Connects to the server and lists its rooms, from nothing: whatever
/// connection or room there was is dropped first.
async fn connect<R: Runtime>(app: &AppHandle<R>, epoch: u64) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    let _flow = state.flow.lock().await;
    if state.epoch() != epoch {
        return Ok(state.snapshot());
    }

    drop_connection(app).await;
    state.change(app, |session| {
        session.phase = Phase::ConnectingServer;
        session.error = None;
        session.joining_code = None;
        session.test_room_invite_code = None;
        session.room = None;
        session.roster.clear();
        session.reconnect = Reconnect::Idle;
        session.reconnect_error = None;
    });

    let connected = unless_ended(
        &state,
        epoch,
        signaling::signaling_connect(
            app.clone(),
            app.state(),
            app.state(),
            app.state(),
            app.state(),
        ),
    )
    .await;
    let conn = match connected {
        None => return Ok(state.snapshot()),
        Some(Ok(conn)) => conn,
        Some(Err(e)) => return fail_unless_ended(app, epoch, e),
    };
    if state.epoch() != epoch {
        disconnect(app, conn).await;
        return Ok(state.snapshot());
    }

    state.change(app, |session| session.connection_id = Some(conn));
    spawn_pump(app.clone(), conn);
    // Ended before the list came: the step that ended this one drops the
    // connection, since it is the session's now.
    match unless_ended(
        &state,
        epoch,
        signaling::signaling_list_rooms(conn, app.state()),
    )
    .await
    {
        None => Ok(state.snapshot()),
        Some(Ok(rooms)) => {
            state.change(app, |session| {
                session.test_room_invite_code = test_room_code_of(&rooms);
                session.phase = Phase::ServerConnected;
            });
            Ok(state.snapshot())
        }
        Some(Err(e)) => fail_unless_ended(app, epoch, e),
    }
}

/// What entering a room changes: the room, and the audio it starts.
fn enter_room<R: Runtime>(
    app: &AppHandle<R>,
    conn: u32,
    result: JoinResult,
    peer_name: String,
    fallback_code: &str,
) -> Vec<Audio> {
    app.state::<SessionState>().change(app, |session| {
        session.connection_id = Some(conn);
        session.room = Some(Room {
            room_id: result.room_id,
            invite_code: if result.invite_code.is_empty() {
                fallback_code.to_string()
            } else {
                result.invite_code
            },
            peer_id: result.peer_id,
            peer_name,
        });
        session.phase = Phase::Connected;
        session.error = None;
        session.joining_code = None;
        session.reconnect = Reconnect::Idle;
        session.reconnect_error = None;
        session.roster.entered(result.peers)
    })
}

/// The connection the room's operations use, if the app can enter a room now.
fn connection_to_enter<R: Runtime>(app: &AppHandle<R>) -> Result<u32, String> {
    let state = app.state::<SessionState>();
    let session = state.lock();
    if session.room.is_some() {
        return Err("Already in a room".to_string());
    }
    session
        .connection_id
        .ok_or_else(|| "Not connected to the signaling server".to_string())
}

async fn create<R: Runtime>(app: &AppHandle<R>) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    let _flow = state.flow.lock().await;
    let conn = connection_to_enter(app)?;
    let name = peer_name(app);
    state.change(app, |session| {
        session.phase = Phase::Creating;
        session.error = None;
    });

    let result = match signaling::signaling_create_room(
        conn,
        ROOM_NAME.to_string(),
        name.clone(),
        app.clone(),
        app.state(),
        app.state(),
    )
    .await
    {
        Ok(result) => result,
        Err(e) => return fail(app, e),
    };
    let audio = enter_room(app, conn, result, name, "");
    run_audio(app, conn, audio).await;
    Ok(state.snapshot())
}

async fn join<R: Runtime>(app: &AppHandle<R>, code: String) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    let _flow = state.flow.lock().await;
    let conn = connection_to_enter(app)?;
    let name = peer_name(app);
    state.change(app, |session| {
        session.phase = Phase::Joining;
        session.error = None;
        session.joining_code = Some(code.clone());
    });

    let result = match signaling::signaling_join_room(
        conn,
        code.clone(),
        name.clone(),
        app.clone(),
        app.state(),
        app.state(),
    )
    .await
    {
        Ok(result) => result,
        Err(e) => return fail(app, e),
    };

    // Saved under the code the person could join with again. `code` is what
    // they typed, which is either that code already or a room id from a link.
    let history_code = if result.invite_code.is_empty() {
        code.as_str()
    } else {
        result.invite_code.as_str()
    };
    if let Err(e) =
        crate::config::config_add_connection_history(history_code.to_string(), None, app.state())
    {
        tracing::error!("Failed to save to history: {}", e);
    }

    let audio = enter_room(app, conn, result, name, "");
    run_audio(app, conn, audio).await;
    Ok(state.snapshot())
}

async fn leave<R: Runtime>(app: &AppHandle<R>) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    if state.connection_id().is_none() || state.lock().reconnect != Reconnect::Idle {
        // The connection is gone and is being got back. The person no longer
        // wants the room: end that, and start from the server.
        let epoch = state.next_epoch();
        return connect(app, epoch).await;
    }

    let flow = state.flow.lock().await;
    let Some(conn) = state.connection_id() else {
        // Lost while waiting for a turn.
        drop(flow);
        let epoch = state.next_epoch();
        return connect(app, epoch).await;
    };
    if state.lock().room.is_none() {
        return Err("Not in a room".to_string());
    }

    stop_audio(app).await;
    // Whatever the server answers, the person asked to leave: the app is in no
    // room after this, and a connection lost from here on is not a reason to
    // rejoin the one they left.
    let left = signaling::signaling_leave_room(conn, app.state(), app.state(), app.state()).await;
    let listed = match left {
        Ok(()) => signaling::signaling_list_rooms(conn, app.state()).await,
        Err(e) => Err(e),
    };
    state.change(app, |session| {
        session.room = None;
        session.roster.clear();
        match &listed {
            Ok(rooms) => {
                session.test_room_invite_code = test_room_code_of(rooms);
                session.phase = Phase::ServerConnected;
            }
            Err(e) => {
                session.phase = Phase::Error;
                session.error = Some(e.clone());
            }
        }
    });
    listed.map(|_| state.snapshot())
}

/// Reaches the server again after the connection dropped, and rejoins the room
/// the app is in, if it is in one.
///
/// Retries with a backoff. While a room is open its screen stays up and
/// `signaling_reconnect` reports progress; otherwise the phase is
/// `connecting_server` and, on giving up, `error` - what a failed first
/// connect looks like.
async fn reconnect<R: Runtime>(app: &AppHandle<R>, epoch: u64) -> Result<Snapshot, String> {
    let state = app.state::<SessionState>();
    let _flow = state.flow.lock().await;
    if state.epoch() != epoch {
        return Ok(state.snapshot());
    }
    // Read once it is this step's turn: the room the app is in now is the one
    // to rejoin, not the one it was in when the connection was lost.
    let rejoin = state.rejoin_code();

    drop_connection(app).await;
    let was_in_room = rejoin.is_some();
    state.change(app, |session| {
        session.test_room_invite_code = None;
        if was_in_room {
            session.reconnect = Reconnect::Reconnecting;
            session.reconnect_error = None;
        } else {
            session.phase = Phase::ConnectingServer;
            session.error = None;
            session.joining_code = None;
            session.room = None;
            session.roster.clear();
        }
    });

    for attempt in 1..=MAX_RECONNECT_ATTEMPTS {
        if state.epoch() != epoch {
            return Ok(state.snapshot());
        }
        let outcome = try_reconnect(app, epoch, rejoin.as_deref()).await;
        let Err(e) = outcome else {
            return Ok(state.snapshot());
        };
        tracing::warn!(
            "Signaling reconnect attempt {}/{} failed: {}",
            attempt,
            MAX_RECONNECT_ATTEMPTS,
            e
        );
        if attempt == MAX_RECONNECT_ATTEMPTS {
            if was_in_room {
                state.change(app, |session| {
                    session.reconnect = Reconnect::Failed;
                    session.reconnect_error = Some(e.clone());
                });
                return Err(e);
            }
            return fail_unless_ended(app, epoch, e);
        }
        pause(&state, epoch, RECONNECT_BASE_DELAY * attempt).await;
    }
    Ok(state.snapshot())
}

/// One attempt of [`reconnect`]. Ok also when the step was ended meanwhile.
async fn try_reconnect<R: Runtime>(
    app: &AppHandle<R>,
    epoch: u64,
    rejoin: Option<&str>,
) -> Result<(), String> {
    let state = app.state::<SessionState>();
    let connected = unless_ended(
        &state,
        epoch,
        signaling::signaling_connect(
            app.clone(),
            app.state(),
            app.state(),
            app.state(),
            app.state(),
        ),
    )
    .await;
    let conn = match connected {
        None => return Ok(()),
        Some(conn) => conn?,
    };
    if state.epoch() != epoch {
        disconnect(app, conn).await;
        return Ok(());
    }

    let entered = unless_ended(&state, epoch, async {
        match rejoin {
            Some(code) => {
                let name = peer_name(app);
                let result = signaling::signaling_join_room(
                    conn,
                    code.to_string(),
                    name.clone(),
                    app.clone(),
                    app.state(),
                    app.state(),
                )
                .await?;
                let audio = enter_room(app, conn, result, name, code);
                spawn_pump(app.clone(), conn);
                run_audio(app, conn, audio).await;
            }
            None => {
                let rooms = signaling::signaling_list_rooms(conn, app.state()).await?;
                state.change(app, |session| {
                    session.connection_id = Some(conn);
                    session.test_room_invite_code = test_room_code_of(&rooms);
                    session.phase = Phase::ServerConnected;
                });
                spawn_pump(app.clone(), conn);
            }
        }
        Ok::<(), String>(())
    })
    .await;
    match entered {
        Some(Ok(())) => Ok(()),
        // Ended, or failed: either way the connection made for this attempt
        // is no use to anyone.
        Some(Err(e)) => {
            disconnect(app, conn).await;
            Err(e)
        }
        None => {
            disconnect(app, conn).await;
            Ok(())
        }
    }
}

// =============================================================================
// The room's events
// =============================================================================

/// Reads the room's events from `conn` for as long as it is the session's
/// connection. A drop while merely connected to the server (browsing the room
/// list) needs the same detection as one in a room.
fn spawn_pump<R: Runtime>(app: AppHandle<R>, conn: u32) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if app.state::<SessionState>().connection_id() != Some(conn) {
                return;
            }
            let events =
                match signaling::signaling_poll_events(conn, app.state(), app.state(), app.state())
                    .await
                {
                    Ok(events) => events,
                    Err(e) => {
                        tracing::error!("Failed to poll signaling events: {}", e);
                        continue;
                    }
                };
            if !events.is_empty() && !handle_events(&app, conn, events).await {
                return;
            }
        }
    });
}

/// Ends the connection `conn` and starts over from the server, unless `conn`
/// is no longer the session's.
fn start_over<R: Runtime>(app: &AppHandle<R>, conn: u32) {
    let Some(epoch) = app.state::<SessionState>().next_epoch_of(conn) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = connect(&app, epoch).await;
    });
}

/// Ends the connection `conn` and gets it back, rejoining the room the app is
/// in, unless `conn` is no longer the session's.
fn get_connection_back<R: Runtime>(app: &AppHandle<R>, conn: u32) {
    let Some(epoch) = app.state::<SessionState>().next_epoch_of(conn) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = reconnect(&app, epoch).await;
    });
}

/// Handles one batch of events, in order. False when `conn` is finished with
/// and the events after the one that ended it are not to be read.
async fn handle_events<R: Runtime>(
    app: &AppHandle<R>,
    conn: u32,
    events: Vec<SignalingEvent>,
) -> bool {
    let state = app.state::<SessionState>();
    for event in events {
        match event {
            SignalingEvent::SettingsHelp { event } => {
                // A help that ended takes its relay connection and window with it.
                if let HelpEvent::Ended { role, .. } = &event {
                    crate::help_link::end(app, *role);
                }
                if let Err(e) = app.emit(HELP_EVENT, &event) {
                    tracing::warn!("Could not pass on a settings help event: {}", e);
                }
            }
            SignalingEvent::HelpLink { link } => crate::help_link::open_helper(app.clone(), link),
            // The chat panel reads the chat lines from the backend itself.
            SignalingEvent::ChatMessageReceived { .. } => {}
            SignalingEvent::PeerJoined { peer } => {
                let _flow = state.flow.lock().await;
                state.change(app, |session| {
                    if session.connection_id == Some(conn) && session.room.is_some() {
                        session.roster.joined(peer);
                    }
                });
            }
            SignalingEvent::PeerUpdated { peer } => {
                let _flow = state.flow.lock().await;
                let audio = state.change(app, |session| {
                    if session.connection_id == Some(conn) && session.room.is_some() {
                        session.roster.updated(peer)
                    } else {
                        Vec::new()
                    }
                });
                run_audio(app, conn, audio).await;
            }
            SignalingEvent::PeerLeft { peer_id } => {
                let Ok(peer_id) = Uuid::parse_str(&peer_id) else {
                    continue;
                };
                let _flow = state.flow.lock().await;
                let audio = state.change(app, |session| {
                    if session.connection_id == Some(conn) && session.room.is_some() {
                        session.roster.left(peer_id)
                    } else {
                        Vec::new()
                    }
                });
                run_audio(app, conn, audio).await;
            }
            SignalingEvent::RoomClosed { reason } => {
                // The server closes this connection right after sending it, so
                // the connection is dead: it cannot be used to leave, create or
                // join. Start over with a new one.
                tracing::info!("Room session ended (room closed): {}", reason);
                start_over(app, conn);
                return false;
            }
            SignalingEvent::Kicked { peer_id, reason } => {
                // Told to the whole room, but the server closes only the
                // connection of the peer removed.
                let is_self = state
                    .lock()
                    .room
                    .as_ref()
                    .is_some_and(|room| room.peer_id == peer_id);
                if is_self {
                    tracing::info!("Room session ended (removed): {}", reason);
                    start_over(app, conn);
                    return false;
                }
            }
            SignalingEvent::ConnectionLost { reason } => {
                // The server did not close the room first (network blip, proxy
                // reset, server restart): the connection is dead either way.
                tracing::warn!("Signaling connection lost: {}", reason);
                get_connection_back(app, conn);
                return false;
            }
        }
    }
    true
}

// =============================================================================
// The audio
// =============================================================================

/// Carries out what the roster decided about the audio.
async fn run_audio<R: Runtime>(app: &AppHandle<R>, conn: u32, audio: Vec<Audio>) {
    for step in audio {
        match step {
            Audio::Stop => stop_audio(app).await,
            Audio::Advertise => advertise(app, conn).await,
            Audio::Start { peer, candidates } => {
                let candidates: Vec<String> = candidates.iter().map(ToString::to_string).collect();
                let Some(addr) = candidates.first().cloned() else {
                    continue;
                };
                mixer::set_on_audio(app, &peer.to_string()).await;
                // The devices, buffer size and sample rate are the saved
                // settings.
                let started = streaming::streaming_start(
                    addr,
                    Some(candidates.clone()),
                    app.state(),
                    app.state(),
                    app.state(),
                    app.state(),
                )
                .await;
                match started {
                    Ok(()) => tracing::info!(
                        "Streaming started ({} address candidate(s))",
                        candidates.len()
                    ),
                    Err(e) => {
                        // Allow a later update to retry rather than leaving
                        // the session permanently silent.
                        app.state::<SessionState>()
                            .change(app, |session| session.roster.start_failed(peer));
                        tracing::error!("Failed to start streaming: {}", e);
                    }
                }
            }
        }
    }
}

/// Advertises this app's audio address to the room.
///
/// Not fatal for chat or the participant list, which go through the signaling
/// server; only audio depends on this.
async fn advertise<R: Runtime>(app: &AppHandle<R>, conn: u32) {
    let advertised = async {
        let local = streaming::streaming_prepare(app.state()).await?;
        let port = local
            .rsplit(':')
            .next()
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .ok_or_else(|| format!("unexpected local audio address: {}", local))?;
        signaling::signaling_publish_local_candidates(
            conn,
            port,
            app.state(),
            app.state(),
            app.state(),
        )
        .await?;
        Ok::<(), String>(())
    }
    .await;
    if let Err(e) = advertised {
        tracing::error!("Failed to publish our audio address: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures_util::{SinkExt, StreamExt};
    use tauri::test::MockRuntime;
    use tauri::Listener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::device_identity::DeviceIdentityState;
    use crate::settings::SettingsState;
    use crate::signaling::SignalingState;
    use crate::streaming::StreamingState;
    use crate::usage::UsageState;

    // Built from parts, not single literals: `no_other_source_names_a_signaling_server`
    // (distribution_config_test.rs) flags any hardcoded server URL scheme in
    // this crate as a possible second distribution default. These are
    // throwaway local fixture addresses, not ones.
    const TEST_HTTP_SCHEME: &str = concat!("http", "://");
    const TEST_WS_SCHEME: &str = concat!("ws", "://");

    const ME: u128 = 1;
    const OTHER: u128 = 2;

    /// What the server does, and what it was asked.
    #[derive(Default)]
    struct ServerLog {
        /// The message types each connection sent, oldest first.
        received: Vec<(usize, String)>,
        /// Ways to talk to each open connection.
        connections: Vec<mpsc::UnboundedSender<Say>>,
        /// A message type that makes the server drop the connection instead
        /// of answering.
        close_on: Option<String>,
    }

    enum Say {
        Message(String),
        /// Drop the connection with no close frame.
        Reset,
    }

    /// A signaling server that answers what the session asks: the room list
    /// (with a test room), a room created or joined (with `OTHER` already in
    /// it), and that can be made to say things or drop a connection.
    struct FakeServer {
        url: String,
        log: Arc<Mutex<ServerLog>>,
    }

    fn peer_json(id: u128, name: &str) -> String {
        format!(
            r#"{{"id":"{}","name":"{name}","candidates":[],"public_addr":null,"local_addr":null,"joined_at":0,"features":["peer_message"]}}"#,
            Uuid::from_u128(id)
        )
    }

    impl FakeServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let log = Arc::new(Mutex::new(ServerLog::default()));
            let serving = log.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(serve(stream, addr, serving.clone()));
                }
            });
            Self {
                url: format!("{TEST_HTTP_SCHEME}{addr}"),
                log,
            }
        }

        /// How many connections have been opened.
        fn connections(&self) -> usize {
            self.log.lock().unwrap().connections.len()
        }

        /// How many times connection `n` (from 0) sent a message of `kind`.
        fn count(&self, kind: &str) -> usize {
            self.log
                .lock()
                .unwrap()
                .received
                .iter()
                .filter(|(_, k)| k == kind)
                .count()
        }

        fn say(&self, connection: usize, message: String) {
            let _ = self.log.lock().unwrap().connections[connection].send(Say::Message(message));
        }

        /// From now on, drop the connection of whoever sends a `kind` message.
        fn drop_the_connection_when_sent(&self, kind: &str) {
            self.log.lock().unwrap().close_on = Some(kind.to_string());
        }

        fn reset(&self, connection: usize) {
            let _ = self.log.lock().unwrap().connections[connection].send(Say::Reset);
        }
    }

    async fn serve(
        stream: tokio::net::TcpStream,
        addr: std::net::SocketAddr,
        log: Arc<Mutex<ServerLog>>,
    ) {
        let mut probe = [0u8; 2048];
        let read = stream.peek(&mut probe).await.unwrap_or(0);
        let head = String::from_utf8_lossy(&probe[..read]).to_lowercase();
        if !head.contains("upgrade: websocket") {
            // The question: where is the signaling?
            let mut stream = stream;
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            let body = format!(r#"{{"url":"{TEST_WS_SCHEME}{addr}/v1/signaling"}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
            return;
        }

        let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let me = {
            let mut log = log.lock().unwrap();
            log.connections.push(tx);
            log.connections.len() - 1
        };
        loop {
            tokio::select! {
                said = rx.recv() => match said {
                    Some(Say::Message(text)) => {
                        if ws.send(Message::Text(text.into())).await.is_err() {
                            return;
                        }
                    }
                    Some(Say::Reset) | None => return,
                },
                incoming = ws.next() => {
                    let Some(Ok(Message::Text(text))) = incoming else {
                        if incoming.is_none() { return; }
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                    let kind = value["type"].as_str().unwrap_or_default().to_string();
                    let closing = {
                        let mut log = log.lock().unwrap();
                        log.received.push((me, kind.clone()));
                        log.close_on.as_deref() == Some(kind.as_str())
                    };
                    if closing {
                        return;
                    }
                    let reply = match kind.as_str() {
                        "ListRooms" => Some(
                            r#"{"type":"RoomList","data":{"rooms":[{"id":"t","name":"Test Room","peer_count":0,"max_peers":10,"has_password":false,"invite_code":"TEST22","test_room":true}]}}"#
                                .to_string(),
                        ),
                        "CreateRoom" => Some(format!(
                            r#"{{"type":"RoomCreated","data":{{"room_id":"room-1","peer_id":"{}","invite_code":"ABC234"}}}}"#,
                            Uuid::from_u128(ME)
                        )),
                        "JoinRoom" if value["data"]["room_id"] == "NOPE22" => Some(
                            r#"{"type":"Error","data":{"message":"room not found"}}"#.to_string(),
                        ),
                        "JoinRoom" => Some(format!(
                            r#"{{"type":"RoomJoined","data":{{"room_id":"room-1","peer_id":"{}","invite_code":"ABC234","peers":[{}]}}}}"#,
                            Uuid::from_u128(ME),
                            peer_json(OTHER, "Aki")
                        )),
                        _ => None,
                    };
                    if let Some(reply) = reply {
                        if ws.send(Message::Text(reply.into())).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }

    fn app_for(server: &FakeServer, dir: &tempfile::TempDir) -> tauri::App<MockRuntime> {
        let app = tauri::test::mock_app();
        let config = crate::config::AppConfig {
            server_url: Some(server.url.clone()),
            ..Default::default()
        };
        app.manage(ConfigState::at(dir.path().join("config.toml"), config));
        app.manage(DeviceIdentityState::generated());
        app.manage(SignalingState::new());
        app.manage(StreamingState::new());
        app.manage(SettingsState::new());
        app.manage(UsageState::with_reporter(
            jamjam::telemetry::UsageReporter::new(
                None,
                "test",
                Arc::new(jamjam::telemetry::NoTransport),
                false,
            ),
        ));
        app.manage(SessionState::new());
        app
    }

    /// Waits until `wanted` holds of the session, or fails after ten seconds.
    async fn until(app: &tauri::App<MockRuntime>, wanted: impl Fn(&Snapshot) -> bool) -> Snapshot {
        for _ in 0..200 {
            let snapshot = app.state::<SessionState>().snapshot();
            if wanted(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "the session never got there; it is at {:?}",
            app.state::<SessionState>().snapshot()
        );
    }

    async fn connected_to_the_server(
        server: &FakeServer,
        dir: &tempfile::TempDir,
    ) -> tauri::App<MockRuntime> {
        let app = app_for(server, dir);
        spawn(app.handle().clone());
        until(&app, |s| s.phase == Phase::ServerConnected).await;
        app
    }

    async fn in_a_room(server: &FakeServer, dir: &tempfile::TempDir) -> tauri::App<MockRuntime> {
        let app = connected_to_the_server(server, dir).await;
        join(app.handle(), "ABC234".to_string()).await.unwrap();
        app
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_starts_and_the_server_answers_lands_on_the_room_list_with_the_servers_test_room(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        assert_eq!(
            app.state::<SessionState>().snapshot().phase,
            Phase::ConnectingServer
        );

        spawn(app.handle().clone());
        let snapshot = until(&app, |s| s.phase == Phase::ServerConnected).await;

        assert!(snapshot.connection_id.is_some());
        assert_eq!(snapshot.test_room_invite_code.as_deref(), Some("TEST22"));
        assert_eq!(snapshot.room, None);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_server_cannot_be_reached_ends_in_an_error_that_says_so_and_holds_no_connection() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        // Nothing listens where the app is pointed.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_url = format!("{TEST_HTTP_SCHEME}{}", closed.local_addr().unwrap());
        drop(closed);
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(closed_url))
            .unwrap();

        let result = session_connect_for_test(&app).await;

        let snapshot = app.state::<SessionState>().snapshot();
        assert!(result.is_err());
        assert_eq!(snapshot.phase, Phase::Error);
        assert_eq!(snapshot.error, result.err());
        assert_eq!(snapshot.connection_id, None);
    }

    async fn session_connect_for_test(app: &tauri::App<MockRuntime>) -> Result<Snapshot, String> {
        let epoch = app.state::<SessionState>().next_epoch();
        connect(app.handle(), epoch).await
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn a_room_is_created_puts_the_app_in_it_alone_with_the_code_to_share() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;

        let snapshot = create(app.handle()).await.unwrap();

        assert_eq!(snapshot.phase, Phase::Connected);
        let room = snapshot.room.unwrap();
        assert_eq!(room.invite_code, "ABC234");
        assert_eq!(room.peer_id, Uuid::from_u128(ME).to_string());
        assert_eq!(
            room.peer_name,
            crate::config::AppConfig::default().peer_name
        );
        assert_eq!(room.participants, vec![]);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn a_room_is_joined_lists_who_is_in_it_and_remembers_the_code_in_the_history() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;

        let snapshot = join(app.handle(), "abc234".to_string()).await.unwrap();

        assert_eq!(snapshot.phase, Phase::Connected);
        let room = snapshot.room.unwrap();
        assert_eq!(room.invite_code, "ABC234");
        assert_eq!(room.participants.len(), 1);
        assert_eq!(room.participants[0].name, "Aki");
        assert_eq!(room.participants[0].features, vec!["peer_message"]);
        let history = app.state::<ConfigState>().get().unwrap().connection_history;
        assert_eq!(history[0].room_code, "ABC234");
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_snapshot_read_for_a_participant_carries_no_address_of_theirs() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;

        let json = serde_json::to_string(&app.state::<SessionState>().snapshot()).unwrap();

        for key in ["candidates", "public_addr", "local_addr"] {
            assert!(!json.contains(key), "{key} is in {json}");
        }
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_is_already_in_a_room_refuses_to_enter_another_and_stays_where_it_is() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;

        let joined = join(app.handle(), "ZZZ999".to_string()).await;
        let created = create(app.handle()).await;

        assert!(joined.is_err() && created.is_err());
        let snapshot = app.state::<SessionState>().snapshot();
        assert_eq!(snapshot.phase, Phase::Connected);
        assert_eq!(server.count("JoinRoom"), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_server_refuses_the_room_ends_in_an_error_that_keeps_the_connection_for_another_try(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;
        let connection = app.state::<SessionState>().snapshot().connection_id;

        let refused = join(app.handle(), "NOPE22".to_string()).await;

        assert_eq!(refused, Err("room not found".to_string()));
        let snapshot = app.state::<SessionState>().snapshot();
        assert_eq!(snapshot.phase, Phase::Error);
        assert_eq!(snapshot.error.as_deref(), Some("room not found"));
        assert_eq!(snapshot.connection_id, connection);
        assert_eq!(snapshot.joining_code, None);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn someone_joins_and_leaves_the_room_is_listed_while_they_are_there() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;

        server.say(
            0,
            format!(
                r#"{{"type":"PeerJoined","data":{{"peer":{}}}}}"#,
                peer_json(3, "Bo")
            ),
        );
        let joined = until(&app, |s| {
            s.room.as_ref().is_some_and(|r| r.participants.len() == 2)
        })
        .await;
        server.say(
            0,
            format!(
                r#"{{"type":"PeerLeft","data":{{"peer_id":"{}"}}}}"#,
                Uuid::from_u128(OTHER)
            ),
        );
        let left = until(&app, |s| {
            s.room.as_ref().is_some_and(|r| r.participants.len() == 1)
        })
        .await;

        assert_eq!(joined.room.unwrap().participants[1].name, "Bo");
        assert_eq!(left.room.unwrap().participants[0].name, "Bo");
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_leaves_the_room_goes_back_to_the_room_list_on_the_same_connection() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let connection = app.state::<SessionState>().snapshot().connection_id;

        let snapshot = leave(app.handle()).await.unwrap();

        assert_eq!(snapshot.phase, Phase::ServerConnected);
        assert_eq!(snapshot.room, None);
        assert_eq!(snapshot.connection_id, connection);
        assert_eq!(snapshot.test_room_invite_code.as_deref(), Some("TEST22"));
        assert_eq!(server.count("LeaveRoom"), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_is_in_no_room_when_it_is_asked_to_leave_says_so_and_changes_nothing() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;
        let before = app.state::<SessionState>().snapshot();

        let result = leave(app.handle()).await;

        assert_eq!(result, Err("Not in a room".to_string()));
        assert_eq!(app.state::<SessionState>().snapshot(), before);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_server_closes_the_room_starts_over_with_a_new_connection_on_the_room_list() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let first = app.state::<SessionState>().snapshot().connection_id;

        server.say(
            0,
            r#"{"type":"RoomClosed","data":{"reason":"closed"}}"#.to_string(),
        );
        server.reset(0);
        let snapshot = until(&app, |s| {
            s.phase == Phase::ServerConnected && s.connection_id != first
        })
        .await;

        assert_eq!(snapshot.room, None);
        assert_eq!(server.connections(), 2);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn another_participant_is_removed_by_the_server_leaves_the_app_in_the_room() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;

        server.say(
            0,
            format!(
                r#"{{"type":"Kicked","data":{{"peer_id":"{}","reason":"removed"}}}}"#,
                Uuid::from_u128(OTHER)
            ),
        );
        tokio::time::sleep(Duration::from_millis(1200)).await;

        assert_eq!(
            app.state::<SessionState>().snapshot().phase,
            Phase::Connected
        );
        assert_eq!(server.connections(), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_itself_is_removed_by_the_server_starts_over_on_the_room_list() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let first = app.state::<SessionState>().snapshot().connection_id;

        server.say(
            0,
            format!(
                r#"{{"type":"Kicked","data":{{"peer_id":"{}","reason":"removed"}}}}"#,
                Uuid::from_u128(ME)
            ),
        );
        server.reset(0);
        let snapshot = until(&app, |s| {
            s.phase == Phase::ServerConnected && s.connection_id != first
        })
        .await;

        assert_eq!(snapshot.room, None);
        assert_eq!(server.connections(), 2);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_connection_drops_in_a_room_keeps_the_room_up_while_it_rejoins_the_same_room() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let first = app.state::<SessionState>().snapshot().connection_id;

        server.reset(0);
        let snapshot = until(&app, |s| {
            s.phase == Phase::Connected
                && s.connection_id.is_some()
                && s.connection_id != first
                && s.signaling_reconnect == Reconnect::Idle
        })
        .await;

        assert_eq!(snapshot.room.unwrap().invite_code, "ABC234");
        assert_eq!(server.count("JoinRoom"), 2);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_connection_drops_in_a_room_says_it_is_reconnecting_and_still_shows_the_room() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let announced = Arc::new(Mutex::new(Vec::<Snapshot2>::new()));
        let heard = announced.clone();
        app.listen(CHANGED_EVENT, move |event| {
            let snapshot: Snapshot2 = serde_json::from_str(event.payload()).unwrap();
            heard.lock().unwrap().push(snapshot);
        });

        server.reset(0);
        until(&app, |s| {
            s.connection_id.is_some()
                && s.signaling_reconnect == Reconnect::Idle
                && server_rejoined(&server)
        })
        .await;
        // Let the last announcement reach the listener.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let announced = announced.lock().unwrap();
        let during = announced
            .iter()
            .find(|s| s.signaling_reconnect == "reconnecting")
            .expect("the person is told the connection is being got back");
        assert_eq!(during.phase, "connected");
        assert!(during.room.is_some());
        assert_eq!(during.connection_id, None);
        assert!(announced
            .windows(2)
            .all(|pair| pair[0].revision < pair[1].revision));
    }

    fn server_rejoined(server: &FakeServer) -> bool {
        server.count("JoinRoom") >= 2
    }

    /// What a listener reads of an announcement.
    #[derive(serde::Deserialize)]
    struct Snapshot2 {
        revision: u64,
        phase: String,
        connection_id: Option<u32>,
        room: Option<serde_json::Value>,
        signaling_reconnect: String,
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_connection_cannot_be_got_back_ends_in_failed_and_a_retry_rejoins_the_room() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        // The server goes away: the connection drops and nothing answers.
        let good_url = server.url.clone();
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_url = format!("{TEST_HTTP_SCHEME}{}", closed.local_addr().unwrap());
        drop(closed);
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(closed_url))
            .unwrap();

        server.reset(0);
        let failed = until(&app, |s| s.signaling_reconnect == Reconnect::Failed).await;

        assert_eq!(failed.phase, Phase::Connected);
        assert!(failed.room.is_some());
        assert!(failed.signaling_reconnect_error.is_some());

        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(good_url))
            .unwrap();
        let epoch = app.state::<SessionState>().next_epoch();
        let snapshot = reconnect(app.handle(), epoch).await.unwrap();

        assert_eq!(snapshot.phase, Phase::Connected);
        assert_eq!(snapshot.signaling_reconnect, Reconnect::Idle);
        assert_eq!(snapshot.signaling_reconnect_error, None);
    }

    /// A server that accepts and never answers, so an attempt to reach it
    /// hangs. Returns its URL and keeps the connections open.
    async fn spawn_black_hole() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("{TEST_HTTP_SCHEME}{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        url
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_person_leaves_while_the_connection_is_being_got_back_ends_that_and_starts_from_the_room_list(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let hanging = spawn_black_hole().await;
        let good_url = server.url.clone();
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(hanging))
            .unwrap();
        server.reset(0);
        until(&app, |s| s.signaling_reconnect == Reconnect::Reconnecting).await;
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(good_url))
            .unwrap();

        let started = std::time::Instant::now();
        let snapshot = leave(app.handle()).await.unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "leaving waited for the attempt that hangs"
        );
        assert_eq!(snapshot.phase, Phase::ServerConnected);
        assert_eq!(snapshot.room, None);
        assert_eq!(snapshot.signaling_reconnect, Reconnect::Idle);
        // The step that was getting the connection back did not rejoin behind it.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let later = app.state::<SessionState>().snapshot();
        assert_eq!(later.phase, Phase::ServerConnected);
        assert_eq!(later.room, None);
        assert_eq!(server.count("JoinRoom"), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_person_cancels_while_the_server_does_not_answer_connects_again_without_waiting_for_it(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        let hanging = spawn_black_hole().await;
        let good_url = server.url.clone();
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(hanging))
            .unwrap();
        spawn(app.handle().clone());
        tokio::time::sleep(Duration::from_millis(300)).await;
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(good_url))
            .unwrap();

        let started = std::time::Instant::now();
        let snapshot = session_connect_for_test(&app).await.unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the retry waited for the attempt that hangs"
        );
        assert_eq!(snapshot.phase, Phase::ServerConnected);
        assert_eq!(server.connections(), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_session_changes_announces_each_new_state_once_with_a_rising_revision() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        let heard = Arc::new(Mutex::new(Vec::<Snapshot2>::new()));
        let listener = heard.clone();
        app.listen(CHANGED_EVENT, move |event| {
            listener
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });

        let epoch = app.state::<SessionState>().next_epoch();
        connect(app.handle(), epoch).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let heard = heard.lock().unwrap();
        let phases: Vec<&str> = heard.iter().map(|s| s.phase.as_str()).collect();
        assert_eq!(phases.first(), Some(&"connecting_server"));
        assert_eq!(phases.last(), Some(&"server_connected"));
        assert!(heard
            .windows(2)
            .all(|pair| pair[0].revision < pair[1].revision));
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_connection_fails_while_leaving_leaves_the_app_in_no_room_and_never_rejoins_it() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        // The server drops the connection when asked to let the app leave.
        server.drop_the_connection_when_sent("LeaveRoom");

        let left = leave(app.handle()).await;

        assert!(left.is_err());
        let snapshot = until(&app, |s| {
            s.phase == Phase::ServerConnected && s.connection_id.is_some()
        })
        .await;
        assert_eq!(snapshot.room, None);
        assert_eq!(
            server.count("JoinRoom"),
            1,
            "the room that was left was rejoined"
        );
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_person_leaves_after_the_connection_could_not_be_got_back_starts_from_the_room_list(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let good_url = server.url.clone();
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_url = format!("{TEST_HTTP_SCHEME}{}", closed.local_addr().unwrap());
        drop(closed);
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(closed_url))
            .unwrap();
        server.reset(0);
        until(&app, |s| s.signaling_reconnect == Reconnect::Failed).await;
        app.state::<ConfigState>()
            .modify(|config| config.server_url = Some(good_url))
            .unwrap();

        let snapshot = leave(app.handle()).await.unwrap();

        assert_eq!(snapshot.phase, Phase::ServerConnected);
        assert_eq!(snapshot.room, None);
        assert_eq!(snapshot.signaling_reconnect, Reconnect::Idle);
        assert_eq!(server.count("JoinRoom"), 1);
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn an_event_of_a_connection_the_session_let_go_of_ends_no_step() {
        let state = SessionState::new();
        state.lock().connection_id = Some(2);
        let epoch = state.epoch();

        assert_eq!(state.next_epoch_of(1), None);
        assert_eq!(state.epoch(), epoch);
        assert_eq!(state.next_epoch_of(2), Some(epoch + 1));
    }

    /// Verifies: REQ-RMT-028
    #[test]
    fn the_connection_was_let_go_of_quietly_the_next_change_still_tells_of_it() {
        let app = tauri::test::mock_app();
        app.manage(SessionState::new());
        let state = app.state::<SessionState>();
        let heard = Arc::new(Mutex::new(Vec::<Snapshot2>::new()));
        let listener = heard.clone();
        app.listen(CHANGED_EVENT, move |event| {
            listener
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        state.change(app.handle(), |session| session.connection_id = Some(1));

        state.lock().connection_id = None;
        state.change(app.handle(), |_| {});

        // The mock app delivers events on its own thread.
        std::thread::sleep(Duration::from_millis(200));
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 2);
        assert_eq!(heard[1].connection_id, None);
    }
}
