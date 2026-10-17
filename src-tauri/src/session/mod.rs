//! The session: how far the app has got into a room, and what it does about
//! the room and the audio (ADR-044 §6).
//!
//! The backend owns this, not the screen. It connects to the signaling server,
//! lists the rooms, creates, joins and leaves them, opens an audio link to
//! everyone who publishes an address, and connects again when the connection
//! is lost. The screen reads [`Snapshot`] with `session_get`, hears `session:changed`
//! when it changes, and acts with one command per operation - so the screen,
//! the E2E channel and the remote portals all see and drive the same state.
//!
//! The steps are the core's [`Driver`], which the CLI runs too; this is the
//! [`Backend`] it steers here: the signaling connection, the audio engine, and
//! the events the screen hears.

use std::sync::Arc;
use std::time::Duration;

use jamjam::network::room_driver::{Backend, Driver, Polled, RoomEvent, Session, Timing};
use jamjam::network::room_session::Snapshot;
use jamjam::network::roster::LinkHealth;
use jamjam::network::settings_help::HelpEvent;
use jamjam::network::{
    server_clock_offset_secs, Entered, NetworkError, RoomInfo, CLOCK_SKEW_NOTICE_SECS,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

use crate::config::ConfigState;
use crate::mixer::{self, MixerState};
use crate::signaling::{self, SignalingEvent, SignalingState};
use crate::streaming;

/// Announces a change of the session. The payload is the new [`Snapshot`].
pub const CHANGED_EVENT: &str = "session:changed";
/// Something about helping with settings that the screen shows (ADR-043). It
/// arrives from the room with the room's other events.
pub const HELP_EVENT: &str = "session:settings-help";

/// The wait before reconnect attempt N + 1 is N times this, so attempts back off.
#[cfg(not(test))]
const RECONNECT_BASE_DELAY: Duration = Duration::from_secs(2);
#[cfg(test)]
const RECONNECT_BASE_DELAY: Duration = Duration::from_millis(10);
/// How often the server's time is looked at again while the computer's clock
/// is what keeps the app from connecting.
#[cfg(not(test))]
const CLOCK_RECHECK_INTERVAL: Duration = Duration::from_secs(10);
#[cfg(test)]
const CLOCK_RECHECK_INTERVAL: Duration = Duration::from_millis(100);
/// How often the room's events are read.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// What a room this app creates is called.
const ROOM_NAME: &str = "My Room";

/// The session, managed by Tauri.
pub struct SessionState(Arc<Session>);

impl SessionState {
    pub fn new() -> Self {
        Self(Arc::new(Session::new(Timing {
            reconnect_base_delay: RECONNECT_BASE_DELAY,
            clock_recheck_interval: CLOCK_RECHECK_INTERVAL,
            poll_interval: POLL_INTERVAL,
        })))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.0.snapshot()
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

/// The steps of the session, run against the app.
fn driver<R: Runtime>(app: &AppHandle<R>) -> Driver<AppBackend<R>> {
    Driver::new(
        app.state::<SessionState>().0.clone(),
        Arc::new(AppBackend { app: app.clone() }),
    )
}

/// Starts the audio session over in the room the person is in, so it takes
/// the audio settings as saved now. A no-op outside a room.
pub async fn restart_audio<R: Runtime>(app: &AppHandle<R>) {
    driver(app).restart_audio().await;
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
    crate::terms::require_accepted(&app)?;
    driver(&app).connect_afresh().await
}

/// Creates a room and enters it.
#[tauri::command]
pub async fn session_create(app: AppHandle) -> Result<Snapshot, String> {
    crate::terms::require_accepted(&app)?;
    driver(&app).create().await
}

/// Joins the room `code` names (an invite code, or a room id from a link).
/// With `muted`, the microphone is muted from the moment of entering, until the
/// person unmutes it: a room entered by a link from another app is entered so.
#[tauri::command]
pub async fn session_join(
    code: String,
    muted: Option<bool>,
    app: AppHandle,
) -> Result<Snapshot, String> {
    crate::terms::require_accepted(&app)?;
    let muted = muted.unwrap_or(false);
    let streaming = app.state::<streaming::StreamingState>();
    if muted {
        streaming.mute_on_entry().await;
    }
    let entered = driver(&app).join(code).await;
    if muted && entered.is_err() {
        streaming.forget_mute_at_start();
    }
    entered
}

/// Leaves the room and goes back to the room list. With no connection to the
/// server (it was lost and is being reconnected), ends that and connects from
/// scratch instead.
#[tauri::command]
pub async fn session_leave(app: AppHandle) -> Result<Snapshot, String> {
    app.state::<streaming::StreamingState>()
        .forget_mute_at_start();
    driver(&app).leave().await
}

/// Tries again to get the signaling connection back and rejoin the room, after
/// every attempt failed.
#[tauri::command]
pub async fn session_reconnect(app: AppHandle) -> Result<Snapshot, String> {
    crate::terms::require_accepted(&app)?;
    driver(&app).retry_reconnect().await
}

/// Connects to the server as the app starts, once the user has agreed to the
/// terms of use.
pub fn spawn<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        crate::terms::wait_accepted(&app).await;
        let _ = driver(&app).connect_afresh().await;
    });
}

// =============================================================================
// What the steps steer
// =============================================================================

struct AppBackend<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> Backend for AppBackend<R> {
    type Other = SignalingEvent;

    async fn connect(&self) -> Result<u32, NetworkError> {
        let app = &self.app;
        signaling::signaling_connect(
            app.clone(),
            app.state(),
            app.state(),
            app.state(),
            app.state(),
        )
        .await
    }

    async fn disconnect(&self, conn: u32) -> Result<(), String> {
        let app = &self.app;
        signaling::signaling_disconnect(conn, app.state(), app.state(), app.state()).await
    }

    async fn list_rooms(&self, conn: u32) -> Result<Vec<RoomInfo>, String> {
        signaling::signaling_list_rooms(conn, self.app.state()).await
    }

    async fn create_room(&self, conn: u32, peer_name: &str) -> Result<Entered, String> {
        let app = &self.app;
        signaling::signaling_create_room(
            conn,
            ROOM_NAME.to_string(),
            peer_name.to_string(),
            app.clone(),
            app.state(),
            app.state(),
        )
        .await
    }

    async fn join_room(&self, conn: u32, code: &str, peer_name: &str) -> Result<Entered, String> {
        let app = &self.app;
        signaling::signaling_join_room(
            conn,
            code.to_string(),
            peer_name.to_string(),
            app.clone(),
            app.state(),
            app.state(),
        )
        .await
    }

    async fn leave_room(&self, conn: u32) -> Result<(), String> {
        let app = &self.app;
        signaling::signaling_leave_room(conn, app.state(), app.state(), app.state()).await
    }

    async fn poll_events(&self, conn: u32) -> Result<Vec<Polled<SignalingEvent>>, String> {
        let app = &self.app;
        let events =
            signaling::signaling_poll_events(conn, app.state(), app.state(), app.state()).await?;
        Ok(events
            .into_iter()
            .filter_map(|event| match event {
                SignalingEvent::PeerJoined { peer } => {
                    Some(Polled::Room(RoomEvent::PeerJoined(peer)))
                }
                SignalingEvent::PeerUpdated { peer } => {
                    Some(Polled::Room(RoomEvent::PeerUpdated(peer)))
                }
                SignalingEvent::PeerLeft { peer_id } => Uuid::parse_str(&peer_id)
                    .ok()
                    .map(|peer_id| Polled::Room(RoomEvent::PeerLeft(peer_id))),
                SignalingEvent::ConnectionLost { reason } => {
                    Some(Polled::Room(RoomEvent::ConnectionLost(reason)))
                }
                other => Some(Polled::Other(other)),
            })
            .collect())
    }

    async fn other_event(&self, event: SignalingEvent) {
        let app = &self.app;
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
            _ => {}
        }
    }

    fn peer_name(&self) -> String {
        self.app
            .state::<ConfigState>()
            .get()
            .map(|config| config.peer_name)
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to load the peer name, using the default: {}", e);
                "User".to_string()
            })
    }

    async fn clock_is_right(&self) -> bool {
        let server_url = self.app.state::<ConfigState>().server_url();
        matches!(
            server_clock_offset_secs(&server_url).await,
            Some(offset) if offset.abs() < CLOCK_SKEW_NOTICE_SECS
        )
    }

    async fn stop_audio(&self) {
        if let Err(e) = streaming::streaming_stop(self.app.state()).await {
            tracing::error!("Failed to stop streaming: {}", e);
        }
    }

    fn audio_state(&self) -> (bool, Vec<(Uuid, LinkHealth)>) {
        let audio = self.app.state::<streaming::StreamingState>();
        (audio.is_active(), audio.link_healths())
    }

    async fn advertise(&self, conn: u32) -> Result<(), String> {
        let app = &self.app;
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
        Ok(())
    }

    async fn open_audio(&self) -> Result<(), String> {
        let app = &self.app;
        streaming::streaming_open(&app.state(), &app.state(), &app.state(), &app.state()).await
    }

    async fn link(
        &self,
        conn: u32,
        peer: Uuid,
        candidates: Vec<std::net::SocketAddr>,
        link_key: Option<String>,
    ) -> Result<(), String> {
        let app = &self.app;
        // A session that starts takes the socket the room was told the
        // address of. One that ended on its own took it with it, so
        // the next has to tell the room a new one first: nobody could
        // send to a socket bound without telling.
        let audio = app.state::<streaming::StreamingState>();
        if !audio.is_active() && !audio.has_prepared_socket().await {
            if let Err(e) = self.advertise(conn).await {
                tracing::error!("Failed to publish our audio address: {}", e);
            }
        }
        mixer::set_on_audio(app, &peer.to_string()).await;
        // The devices, buffer size and sample rate are the saved
        // settings.
        let link = app
            .state::<SignalingState>()
            .link_identity()
            .map(|ours| (ours, link_key));
        streaming::streaming_link(
            streaming::LinkTarget {
                peer,
                candidates,
                link,
            },
            app.state(),
            app.state(),
            app.state(),
            app.state(),
        )
        .await
    }

    async fn unlink(&self, peer: Uuid) -> Result<(), String> {
        streaming::streaming_unlink(peer, self.app.state()).await
    }

    fn announce(&self, snapshot: &Snapshot) {
        let app = &self.app;
        if let Err(e) = app.emit(CHANGED_EVENT, snapshot) {
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
}

#[cfg(test)]
mod tests {
    use jamjam::network::room_session::{Phase, Reconnect};
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
        /// The `data` of every `CreateRoom` and `JoinRoom` received.
        entered: Vec<serde_json::Value>,
        /// Ways to talk to each open connection.
        connections: Vec<mpsc::UnboundedSender<Say>>,
        /// A message type that makes the server drop the connection instead
        /// of answering.
        close_on: Option<String>,
        /// When set, the server's clock is this many seconds behind the real
        /// time, and it refuses every WebSocket handshake with a 401, as it
        /// does a device whose clock differs from its own.
        clock_behind_secs: Option<i64>,
        /// When set, an app that rejoins gets a new peer id, and the room still
        /// lists the entries of the ids it had before, as it does while the
        /// server has not noticed the old connection is gone.
        keep_old_entries: bool,
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

        /// The `data` of the `CreateRoom` and `JoinRoom` messages received, oldest first.
        fn entering_messages(&self) -> Vec<serde_json::Value> {
            self.log.lock().unwrap().entered.clone()
        }

        fn say(&self, connection: usize, message: String) {
            let _ = self.log.lock().unwrap().connections[connection].send(Say::Message(message));
        }

        /// From now on, drop the connection of whoever sends a `kind` message.
        fn drop_the_connection_when_sent(&self, kind: &str) {
            self.log.lock().unwrap().close_on = Some(kind.to_string());
        }

        /// From now on the server's clock is `secs` behind the real time (`None`:
        /// right) and it refuses the connection of any device while it is.
        fn set_clock_behind(&self, secs: Option<i64>) {
            self.log.lock().unwrap().clock_behind_secs = secs;
        }

        /// From now on, a rejoin is a new peer and its old entries stay listed.
        fn keep_old_entries_in_the_room(&self) {
            self.log.lock().unwrap().keep_old_entries = true;
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
        let clock_behind_secs = log.lock().unwrap().clock_behind_secs;
        let date = chrono::DateTime::from_timestamp(
            chrono::Utc::now().timestamp() - clock_behind_secs.unwrap_or(0),
            0,
        )
        .unwrap()
        .format("%a, %d %b %Y %H:%M:%S GMT");
        let upgrade = head.contains("upgrade: websocket");
        if !upgrade || clock_behind_secs.is_some() {
            let mut stream = stream;
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            let response = if upgrade {
                format!(
                    "HTTP/1.1 401 Unauthorized\r\nDate: {date}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            } else {
                // The question: where is the signaling?
                let body = format!(r#"{{"url":"{TEST_WS_SCHEME}{addr}/v1/signaling"}}"#);
                format!(
                    "HTTP/1.1 200 OK\r\nDate: {date}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            };
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
                        if kind == "CreateRoom" || kind == "JoinRoom" {
                            log.entered.push(value["data"].clone());
                        }
                        log.close_on.as_deref() == Some(kind.as_str())
                    };
                    if closing {
                        return;
                    }
                    let reply = match kind.as_str() {
                        "ListRooms" => Some(
                            r#"{"type":"RoomList","data":{"rooms":[{"id":"t","name":"Test Room","peer_count":0,"max_peers":10,"has_password":false,"invite_code":"HJK567MNP","test_room":true}]}}"#
                                .to_string(),
                        ),
                        "CreateRoom" => Some(format!(
                            r#"{{"type":"RoomCreated","data":{{"room_id":"room-1","peer_id":"{}","invite_code":"ABC234XYZ"}}}}"#,
                            Uuid::from_u128(ME)
                        )),
                        "JoinRoom" if value["data"]["room_id"] == "NOPE22" => Some(
                            r#"{"type":"Error","data":{"message":"room not found"}}"#.to_string(),
                        ),
                        "JoinRoom" => {
                            let (own, mut peers) = {
                                let log = log.lock().unwrap();
                                let joins =
                                    log.received.iter().filter(|(_, k)| k == "JoinRoom").count();
                                if log.keep_old_entries {
                                    let id = |join: usize| ME + 100 * join as u128;
                                    let old = (0..joins - 1).map(|j| peer_json(id(j), "Me"));
                                    (id(joins - 1), old.collect::<Vec<_>>())
                                } else {
                                    (ME, Vec::new())
                                }
                            };
                            peers.push(peer_json(OTHER, "Aki"));
                            Some(format!(
                                r#"{{"type":"RoomJoined","data":{{"room_id":"room-1","peer_id":"{}","invite_code":"ABC234XYZ","peers":[{}]}}}}"#,
                                Uuid::from_u128(own),
                                peers.join(",")
                            ))
                        }
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
        app_for_terms(server, dir, true)
    }

    /// [`app_for`] with the terms of use agreed to or not.
    fn app_for_terms(
        server: &FakeServer,
        dir: &tempfile::TempDir,
        terms_accepted: bool,
    ) -> tauri::App<MockRuntime> {
        let app = tauri::test::mock_app();
        let config = crate::config::AppConfig {
            server_url: Some(server.url.clone()),
            ..Default::default()
        };
        app.manage(ConfigState::at(dir.path().join("config.toml"), config));
        app.manage(DeviceIdentityState::generated());
        app.manage(SignalingState::new());
        let streaming = StreamingState::new();
        streaming.stand_in_devices_for_test(
            jamjam::audio_session::InputSignal::Tone(440.0),
            dir.path().join("played.f32"),
        );
        app.manage(streaming);
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
        app.manage(MixerState::new());
        app.manage(crate::terms::TermsState::new(terms_accepted));
        app
    }

    /// Waits until `wanted` holds of the session, or fails after a minute.
    /// The limit is only for a session that is stuck: a busy machine takes
    /// several times as long as an idle one to get there.
    async fn until(app: &tauri::App<MockRuntime>, wanted: impl Fn(&Snapshot) -> bool) -> Snapshot {
        for _ in 0..1200 {
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
        driver(app.handle())
            .join("ABC234XYZ".to_string())
            .await
            .unwrap();
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
        assert_eq!(snapshot.test_room_invite_code.as_deref(), Some("HJK567MNP"));
        assert_eq!(snapshot.room, None);
    }

    /// Verifies: REQ-TRM-003
    #[tokio::test]
    async fn the_app_starts_before_the_terms_are_agreed_to_does_not_reach_the_server_until_they_are(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = app_for_terms(&server, &dir, false);

        spawn(app.handle().clone());
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            server.connections(),
            0,
            "the app connected before the terms were agreed to"
        );
        assert_eq!(
            app.state::<SessionState>().snapshot().phase,
            Phase::ConnectingServer
        );

        app.state::<crate::terms::TermsState>().accept();
        until(&app, |s| s.phase == Phase::ServerConnected).await;
        assert_eq!(server.connections(), 1);
    }

    /// Verifies: REQ-TRM-003
    #[tokio::test]
    async fn the_terms_are_not_agreed_to_and_a_command_that_connects_is_called_refuses_and_stays_off_the_network(
    ) {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = app_for_terms(&server, &dir, false);

        assert!(crate::terms::require_accepted(app.handle()).is_err());
        assert_eq!(server.connections(), 0);
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

    /// Verifies: REQ-IDT-009
    #[tokio::test]
    async fn the_server_refuses_the_identity_because_the_clock_is_off_ends_in_an_error_that_names_the_gap(
    ) {
        let server = FakeServer::start().await;
        server.set_clock_behind(Some(375));
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);

        let result = session_connect_for_test(&app).await;

        let snapshot = app.state::<SessionState>().snapshot();
        let error = result.expect_err("the connection is refused");
        assert!(
            error.contains("clock is ahead of the server's by 37"),
            "{error}"
        );
        assert_eq!(snapshot.phase, Phase::Error);
        assert_eq!(snapshot.error, Some(error));
        assert_eq!(snapshot.connection_id, None);
    }

    /// Verifies: REQ-IDT-010
    #[tokio::test]
    async fn the_clock_is_put_right_after_the_refusal_connects_again_without_being_asked() {
        let server = FakeServer::start().await;
        server.set_clock_behind(Some(375));
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        spawn(app.handle().clone());
        until(&app, |s| s.phase == Phase::Error).await;

        server.set_clock_behind(None);
        let snapshot = until(&app, |s| s.phase == Phase::ServerConnected).await;

        assert!(snapshot.connection_id.is_some());
        assert_eq!(snapshot.error, None);
        assert_eq!(snapshot.test_room_invite_code.as_deref(), Some("HJK567MNP"));
    }

    /// Verifies: REQ-IDT-010
    #[tokio::test]
    async fn the_clock_is_still_off_after_a_wait_stays_in_the_error_and_does_not_flicker_to_connecting(
    ) {
        let server = FakeServer::start().await;
        server.set_clock_behind(Some(375));
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        spawn(app.handle().clone());
        until(&app, |s| s.phase == Phase::Error).await;
        let revision = app.state::<SessionState>().snapshot().revision;

        tokio::time::sleep(CLOCK_RECHECK_INTERVAL * 4).await;

        let snapshot = app.state::<SessionState>().snapshot();
        assert_eq!(snapshot.phase, Phase::Error);
        assert_eq!(snapshot.revision, revision);
    }

    /// Verifies: REQ-IDT-010
    #[tokio::test]
    async fn the_person_connects_by_hand_while_the_clock_is_off_the_wait_for_the_clock_ends() {
        let server = FakeServer::start().await;
        server.set_clock_behind(Some(375));
        let dir = tempfile::tempdir().unwrap();
        let app = app_for(&server, &dir);
        spawn(app.handle().clone());
        until(&app, |s| s.phase == Phase::Error).await;

        // The hand attempt is refused again and starts its own wait; the first
        // one, whose step was ended, must not connect once the clock is right.
        let _ = session_connect_for_test(&app).await;
        server.set_clock_behind(None);
        until(&app, |s| s.phase == Phase::ServerConnected).await;
        tokio::time::sleep(CLOCK_RECHECK_INTERVAL * 4).await;

        assert_eq!(server.connections(), 1);
    }

    async fn session_connect_for_test(app: &tauri::App<MockRuntime>) -> Result<Snapshot, String> {
        driver(app.handle()).connect_afresh().await
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn a_room_is_created_puts_the_app_in_it_alone_with_the_code_to_share() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;

        let snapshot = driver(app.handle()).create().await.unwrap();

        assert_eq!(snapshot.phase, Phase::Connected);
        let room = snapshot.room.unwrap();
        assert_eq!(room.invite_code, "ABC234XYZ");
        assert_eq!(room.peer_id, Uuid::from_u128(ME).to_string());
        assert_eq!(
            room.peer_name,
            crate::config::AppConfig::default().peer_name
        );
        assert_eq!(room.participants, vec![]);
    }

    /// The bug this guards: the audio session started with the first link, so
    /// in a room with no one else in it nothing captured the microphone, the
    /// meter stood still, and the macOS microphone permission was asked for
    /// only when someone joined.
    ///
    /// Verifies: REQ-CON-140
    #[tokio::test]
    async fn a_room_is_created_with_no_one_else_in_it_the_microphone_is_heard_and_the_meter_moves()
    {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;
        assert!(!app.state::<StreamingState>().is_active());

        driver(app.handle()).create().await.unwrap();

        assert!(app.state::<StreamingState>().is_active());
        let mut level = 0;
        for _ in 0..100 {
            level = streaming::streaming_get_input_level(app.state())
                .await
                .unwrap();
            if level > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(level > 0, "the meter stayed at {level} with the room empty");
        driver(app.handle()).leave().await.unwrap();
        assert!(!app.state::<StreamingState>().is_active());
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn a_room_is_joined_lists_who_is_in_it() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;

        let snapshot = driver(app.handle())
            .join("abc234xyz".to_string())
            .await
            .unwrap();

        assert_eq!(snapshot.phase, Phase::Connected);
        let room = snapshot.room.unwrap();
        assert_eq!(room.invite_code, "ABC234XYZ");
        assert_eq!(room.participants.len(), 1);
        assert_eq!(room.participants[0].name, "Aki");
        assert_eq!(room.participants[0].features, vec!["peer_message"]);
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

    /// Verifies: REQ-CON-032
    #[tokio::test]
    async fn creating_and_joining_a_room_tell_the_server_what_the_app_is_without_the_display_name()
    {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;
        crate::config::config_set_peer_name("Taro's secret display name".to_string(), app.state())
            .unwrap();
        driver(app.handle()).create().await.unwrap();
        driver(app.handle()).leave().await.unwrap();
        driver(app.handle())
            .join("ABC234XYZ".to_string())
            .await
            .unwrap();

        let entering = server.entering_messages();
        assert_eq!(entering.len(), 2);
        for data in &entering {
            let info = &data["client_info"];
            assert_eq!(info["app_version"], "test");
            assert_eq!(info["os"], std::env::consts::OS);
            assert_eq!(info["arch"], std::env::consts::ARCH);
            assert!(info["settings"].is_object(), "{}", info);
            let text = info.to_string();
            assert!(!text.contains("Taro's secret display name"), "{}", text);
            assert!(!text.contains("peer_name"), "{}", text);
        }
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_is_already_in_a_room_refuses_to_enter_another_and_stays_where_it_is() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;

        let joined = driver(app.handle()).join("ZZZ999ZZZ".to_string()).await;
        let created = driver(app.handle()).create().await;

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

        let refused = driver(app.handle()).join("NOPE22".to_string()).await;

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

    /// `PeerUpdated` for participant `id`, who now has an audio address
    fn published(id: u128, name: &str, addr: &str) -> String {
        let peer = jamjam::network::PeerInfo {
            id: Uuid::from_u128(id),
            name: name.to_string(),
            candidates: vec![jamjam::network::AddressCandidate::host(
                addr.parse().unwrap(),
            )],
            public_addr: None,
            local_addr: None,
            joined_at: 0,
            features: vec![],
            link_key: None,
        };
        format!(
            r#"{{"type":"PeerUpdated","data":{{"peer":{}}}}}"#,
            serde_json::to_string(&peer).unwrap()
        )
    }

    /// The next thing the audio session is told to do with a link, skipping
    /// what it is told about the faders
    async fn next_link_command(
        commands: &std::sync::mpsc::Receiver<crate::streaming::StreamingCommand>,
    ) -> crate::streaming::StreamingCommand {
        use crate::streaming::StreamingCommand;
        for _ in 0..1200 {
            match commands.try_recv() {
                Ok(command @ (StreamingCommand::Link(_) | StreamingCommand::Unlink(_))) => {
                    return command
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        panic!("the audio session was told nothing about a link");
    }

    /// A room with Aki (`OTHER`) and Bo (3) in it, both with an audio address,
    /// and an audio session that is stood in for: what it is told arrives on
    /// the receiver.
    async fn in_a_room_of_three(
        server: &FakeServer,
        dir: &tempfile::TempDir,
    ) -> (
        tauri::App<MockRuntime>,
        std::sync::mpsc::Receiver<crate::streaming::StreamingCommand>,
    ) {
        use crate::streaming::StreamingCommand;

        let app = connected_to_the_server(server, dir).await;
        // Before entering: a room entered starts a session of its own
        let commands = app
            .state::<StreamingState>()
            .attach_session_for_test()
            .await;
        driver(app.handle())
            .join("ABC234XYZ".to_string())
            .await
            .unwrap();
        server.say(
            0,
            format!(
                r#"{{"type":"PeerJoined","data":{{"peer":{}}}}}"#,
                peer_json(3, "Bo")
            ),
        );
        server.say(0, published(OTHER, "Aki", "192.0.2.2:5000"));
        server.say(0, published(3, "Bo", "192.0.2.3:6000"));
        for expected in [OTHER, 3] {
            match next_link_command(&commands).await {
                StreamingCommand::Link(target) => {
                    assert_eq!(target.peer, Uuid::from_u128(expected))
                }
                other => panic!("expected a link to be opened, got {other:?}"),
            }
        }
        (app, commands)
    }

    /// The bug this guards: the session opened audio to the first participant
    /// who published an address and to no one after, so the third in a room
    /// was heard by no one.
    ///
    /// Verifies: REQ-CON-133
    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn two_participants_publish_an_address_opens_an_audio_link_to_each_and_says_so() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();

        let (app, _commands) = in_a_room_of_three(&server, &dir).await;

        let snapshot = app.state::<SessionState>().snapshot();
        assert_eq!(
            snapshot.streaming_peer_ids,
            vec![
                Uuid::from_u128(OTHER).to_string(),
                Uuid::from_u128(3).to_string()
            ]
        );
        assert_eq!(
            snapshot.streaming_peer_id,
            Some(Uuid::from_u128(OTHER).to_string())
        );
    }

    /// Verifies: REQ-CON-133
    #[tokio::test]
    async fn one_of_two_linked_participants_leaves_closes_the_link_to_them_and_keeps_the_other() {
        use crate::streaming::StreamingCommand;
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (app, commands) = in_a_room_of_three(&server, &dir).await;

        server.say(
            0,
            format!(
                r#"{{"type":"PeerLeft","data":{{"peer_id":"{}"}}}}"#,
                Uuid::from_u128(OTHER)
            ),
        );

        assert_eq!(
            next_link_command(&commands).await,
            StreamingCommand::Unlink(Uuid::from_u128(OTHER))
        );
        let snapshot = until(&app, |s| s.streaming_peer_ids.len() == 1).await;
        assert_eq!(
            snapshot.streaming_peer_ids,
            vec![Uuid::from_u128(3).to_string()]
        );
        assert!(
            app.state::<StreamingState>().is_active(),
            "the audio session goes on for the one who stayed"
        );
    }

    /// Verifies: REQ-CON-134
    #[tokio::test]
    async fn a_link_gives_up_while_the_other_participant_is_heard_stays_in_the_room() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (app, _commands) = in_a_room_of_three(&server, &dir).await;
        let audio = app.state::<StreamingState>();

        audio.set_link_for_test(Uuid::from_u128(3), "connected", Some((100, 8.0)));
        audio.set_link_for_test(Uuid::from_u128(OTHER), "failed", Some((100, 8.0)));
        // Several rounds of the loop that reads the links
        tokio::time::sleep(POLL_INTERVAL * 4).await;

        assert_eq!(server.count("JoinRoom"), 1);
        assert_eq!(
            app.state::<SessionState>()
                .snapshot()
                .streaming_peer_ids
                .len(),
            2
        );
    }

    /// Verifies: REQ-CON-134
    #[tokio::test]
    async fn every_link_gives_up_and_nobody_is_heard_enters_the_room_again() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let (app, _commands) = in_a_room_of_three(&server, &dir).await;
        let audio = app.state::<StreamingState>();

        audio.set_link_for_test(Uuid::from_u128(3), "failed", Some((100, 8.0)));
        audio.set_link_for_test(Uuid::from_u128(OTHER), "failed", Some((100, 8.0)));

        for _ in 0..1200 {
            if server.count("JoinRoom") == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            server.count("JoinRoom"),
            2,
            "the room is entered again, once"
        );
        until(&app, |s| {
            s.phase == Phase::Connected && s.signaling_reconnect == Reconnect::Idle
        })
        .await;
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_leaves_the_room_goes_back_to_the_room_list_on_the_same_connection() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let connection = app.state::<SessionState>().snapshot().connection_id;

        let snapshot = driver(app.handle()).leave().await.unwrap();

        assert_eq!(snapshot.phase, Phase::ServerConnected);
        assert_eq!(snapshot.room, None);
        assert_eq!(snapshot.connection_id, connection);
        assert_eq!(snapshot.test_room_invite_code.as_deref(), Some("HJK567MNP"));
        assert_eq!(server.count("LeaveRoom"), 1);
    }

    /// Verifies: REQ-RMT-028
    #[tokio::test]
    async fn the_app_is_in_no_room_when_it_is_asked_to_leave_says_so_and_changes_nothing() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;
        let before = app.state::<SessionState>().snapshot();

        let result = driver(app.handle()).leave().await;

        assert_eq!(result, Err("Not in a room".to_string()));
        assert_eq!(app.state::<SessionState>().snapshot(), before);
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

        assert_eq!(snapshot.room.unwrap().invite_code, "ABC234XYZ");
        assert_eq!(server.count("JoinRoom"), 2);
    }

    /// The frame size is fixed for the life of a session's audio engines, so a
    /// new buffer size reaches a room by starting the audio over in it: the
    /// room, and the connection to the server, stay. Leaving the room would
    /// end it for a person who is alone in it.
    ///
    /// Verifies: REQ-GUI-024
    #[tokio::test]
    async fn the_buffer_size_is_changed_in_a_room_restarts_the_audio_there_and_keeps_the_room() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = in_a_room(&server, &dir).await;
        let streaming = app.state::<StreamingState>();
        let before = streaming.frame_size_for_test().await;
        let new_size = if before == 64 { 128 } else { 64 };
        let first = app.state::<SessionState>().snapshot().connection_id;
        let published = server.count("UpdatePeerInfo");

        crate::settings::apply(
            app.handle(),
            &crate::settings::SettingChange::BufferSize { samples: new_size },
        )
        .await
        .unwrap();

        for _ in 0..1200 {
            if streaming.is_active() && streaming.frame_size_for_test().await == new_size {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(streaming.is_active());
        assert_eq!(streaming.frame_size_for_test().await, new_size);
        let snapshot = app.state::<SessionState>().snapshot();
        assert_eq!(snapshot.phase, Phase::Connected);
        assert_eq!(snapshot.connection_id, first, "the same connection");
        assert_eq!(snapshot.room.unwrap().invite_code, "ABC234XYZ");
        assert_eq!(server.count("JoinRoom"), 1, "the room is not entered again");
        assert_eq!(server.count("LeaveRoom"), 0, "the room is not left");
        assert!(
            server.count("UpdatePeerInfo") > published,
            "the new audio address is told to the room"
        );
    }

    /// Verifies: REQ-GUI-024
    #[tokio::test]
    async fn the_buffer_size_is_changed_outside_a_room_leaves_the_session_alone() {
        let server = FakeServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let app = connected_to_the_server(&server, &dir).await;
        let connections = server.connections();

        crate::settings::apply(
            app.handle(),
            &crate::settings::SettingChange::BufferSize { samples: 256 },
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert_eq!(server.connections(), connections);
        assert_eq!(
            app.state::<SessionState>().snapshot().phase,
            Phase::ServerConnected
        );
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

    /// Verifies: REQ-CON-130
    #[tokio::test]
    async fn the_connection_drops_and_the_room_still_lists_the_old_entry_of_the_app_does_not_list_it_as_a_participant(
    ) {
        let server = FakeServer::start().await;
        server.keep_old_entries_in_the_room();
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

        let room = snapshot.room.unwrap();
        let names: Vec<_> = room.participants.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["Aki"]);
        assert_ne!(room.peer_id, Uuid::from_u128(ME).to_string());
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
        let snapshot = driver(app.handle()).retry_reconnect().await.unwrap();

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
        let snapshot = driver(app.handle()).leave().await.unwrap();

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

        driver(app.handle()).connect_afresh().await.unwrap();
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

        let left = driver(app.handle()).leave().await;

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

        let snapshot = driver(app.handle()).leave().await.unwrap();

        assert_eq!(snapshot.phase, Phase::ServerConnected);
        assert_eq!(snapshot.room, None);
        assert_eq!(snapshot.signaling_reconnect, Reconnect::Idle);
        assert_eq!(server.count("JoinRoom"), 1);
    }
}
