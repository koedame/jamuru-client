//! The relay connections of a settings help, and the window the helper works
//! in (ADR-044 §5).
//!
//! The helped side opens its end of the relay first, under a number it chose,
//! and serves whatever arrives on it as the help portal: the requests go through
//! the permission table and the promises of [`HelpGuard`](crate::rpc::help). The
//! helper opens the same number and gets a window that draws the same screen the
//! helped app has, from the helped app's state. The window's calls to the
//! backend do not go to this app's backend but through [`help_call`] to the
//! other end, and the helped app's events come back as [`REMOTE_EVENT`].
//!
//! One help is given and one received at a time, so each side keeps one slot.
//! A slot's task is the connection: aborting it ends the help's relay
//! connection, which the relay tells the other end.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jamjam::network::{
    connect_remote, discover_signaling_url, help_relay_url, HelpEnd, RemoteReader, RemoteWriter,
};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::async_runtime::JoinHandle;
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::config::ConfigState;
use crate::device_identity::DeviceIdentityState;
use crate::rpc::help::HelpGuard;
use crate::rpc::link::{self, Ended, Session};
use crate::rpc::{Code, Portal, RpcError};
use crate::settings_help::{Link, Role};

/// What the helper's window hears the helped app's events as. The payload is
/// `{"name": <the event>, "payload": <its payload>}`.
pub const REMOTE_EVENT: &str = "help-remote:event";

/// Numbers the helper's windows, so no two share a label. The label says
/// nothing of the help's number: that one pairs the two ends on the relay.
static NEXT_WINDOW: AtomicU64 = AtomicU64::new(1);

/// How long a helper's window waits for the helped app to answer a call.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// What the two ends of a help hold on to, one slot for each side.
#[derive(Default)]
pub struct HelpLinks {
    host: Mutex<Option<HostSlot>>,
    helper: Mutex<Option<HelperSlot>>,
}

struct HostSlot {
    session: String,
    task: JoinHandle<()>,
}

struct HelperSlot {
    session: String,
    /// The label of the window the help is done in
    label: String,
    peer_name: String,
    /// Set once the relay is connected
    bridge: Option<Arc<Bridge>>,
    task: Option<JoinHandle<()>>,
}

impl HelpLinks {
    fn host(&self) -> std::sync::MutexGuard<'_, Option<HostSlot>> {
        self.host.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn helper(&self) -> std::sync::MutexGuard<'_, Option<HelperSlot>> {
        self.helper.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// An event of the helped app, as the helper's window is told it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteEvent {
    pub name: String,
    pub payload: Value,
}

/// The helper's side of the line: numbers its calls, sends them, and matches
/// what comes back to the caller that is waiting.
pub struct Bridge {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>,
    outgoing: mpsc::Sender<String>,
}

impl Bridge {
    pub fn new(outgoing: mpsc::Sender<String>) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            outgoing,
        }
    }

    fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Calls `method` on the other end and waits for its answer. Nothing here
    /// judges whether it may: the other end does, and says `denied`.
    pub async fn call(&self, method: String, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending().insert(id, tx);
        let frame = json!({ "id": id, "method": method, "params": params }).to_string();
        if self.outgoing.send(frame).await.is_err() {
            self.pending().remove(&id);
            return Err(RpcError::failed("the help connection has ended"));
        }
        match tokio::time::timeout(REPLY_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::failed("the help connection has ended")),
            Err(_) => {
                self.pending().remove(&id);
                Err(RpcError::new(
                    Code::Timeout,
                    format!("{} was not answered within {:?}", method, REPLY_TIMEOUT),
                ))
            }
        }
    }

    /// A frame from the other end: an answer settles the call it answers, an
    /// event is returned for the window, anything else (the greeting, the
    /// relay's notice that a pair formed) is nothing to act on.
    pub fn receive(&self, text: &str) -> Option<RemoteEvent> {
        let frame: Value = serde_json::from_str(text).ok()?;
        if let Some(id) = frame.get("id").and_then(Value::as_u64) {
            let result = match (frame.get("ok"), frame.get("error")) {
                (Some(value), _) => Ok(value.clone()),
                (None, Some(error)) => Err(serde_json::from_value::<RpcError>(error.clone())
                    .unwrap_or_else(|_| RpcError::failed("the answer could not be read"))),
                (None, None) => return None,
            };
            if let Some(waiting) = self.pending().remove(&id) {
                let _ = waiting.send(result);
            }
            return None;
        }
        Some(RemoteEvent {
            name: frame.get("event")?.as_str()?.to_string(),
            payload: frame.get("payload").cloned().unwrap_or(Value::Null),
        })
    }

    /// The connection ended: every call still waiting is answered as failed.
    pub fn close(&self) {
        self.pending().clear();
    }
}

/// This app's end of a relay connection, before anything is served on it.
pub struct RelayEnd {
    reader: RemoteReader,
    writer: RemoteWriter,
}

async fn connect<R: Runtime>(
    app: &AppHandle<R>,
    session: &str,
    end: HelpEnd,
) -> Result<RelayEnd, String> {
    let server_url = app.state::<ConfigState>().server_url();
    let signaling = discover_signaling_url(&server_url)
        .await
        .map_err(|e| e.to_string())?;
    let url = help_relay_url(&signaling, session, end).map_err(|e| e.to_string())?;
    let identity = app.state::<DeviceIdentityState>().identity();
    let (writer, reader) = connect_remote(&url, &identity)
        .await
        .map_err(|e| e.to_string())?;
    Ok(RelayEnd { reader, writer })
}

/// What the build says about itself to the one operating it.
fn build_name() -> &'static str {
    if cfg!(feature = "debug-remote") {
        "beta"
    } else if cfg!(feature = "e2e-control") {
        "e2e"
    } else {
        "release"
    }
}

/// Opens the helped side's end of the relay under `session`. The helper is told
/// the number only once this has succeeded, because the relay refuses a guest
/// for a number that has no host.
pub async fn open_host<R: Runtime>(app: &AppHandle<R>, session: &str) -> Result<RelayEnd, String> {
    connect(app, session, HelpEnd::Host).await
}

/// Serves the help of `helper` on `end`, the relay connection opened by
/// [`open_host`] under `session`, until either end closes it.
pub fn serve_host<R: Runtime>(app: AppHandle<R>, session: String, helper: Uuid, end: RelayEnd) {
    let announcing = app.clone();
    let guard = Arc::new(HelpGuard::new(move |setting| {
        let app = announcing.clone();
        tauri::async_runtime::spawn(async move {
            crate::signaling::announce_change(&app, helper, setting).await;
        });
    }));
    let served = Session {
        portal: Portal::Help,
        build: build_name(),
        app_version: app.package_info().version.to_string(),
        guard: Some(guard),
    };
    tracing::info!("Settings help: serving the helper on the relay");
    let links = app.state::<HelpLinks>();
    let mut slot = links.host();
    if let Some(earlier) = slot.take() {
        earlier.task.abort();
    }
    let task_app = app.clone();
    let task_session = session.clone();
    let task = tauri::async_runtime::spawn(async move {
        match link::serve_relay(task_app.clone(), served, end.reader, end.writer).await {
            Ended::ByRelay => tracing::info!("Settings help: the relay connection ended"),
            Ended::ReadFailed(e) => {
                tracing::warn!("Settings help: the relay connection failed: {}", e)
            }
            Ended::SendFailed(e) => {
                tracing::warn!("Settings help: sending to the relay failed: {}", e)
            }
        }
        forget_host(&task_app, &task_session);
        crate::signaling::help_link_closed(&task_app, Role::Helped, &task_session).await;
    });
    *slot = Some(HostSlot { session, task });
}

fn forget_host<R: Runtime>(app: &AppHandle<R>, session: &str) {
    let links = app.state::<HelpLinks>();
    let mut slot = links.host();
    if slot.as_ref().is_some_and(|held| held.session == session) {
        *slot = None;
    }
}

/// Opens the helper's end of the relay for `link` and the window the help is
/// done in. Returns at once; the window appears when the relay is connected.
pub fn open_helper<R: Runtime>(app: AppHandle<R>, link: Link) {
    let label = format!("help-{}", NEXT_WINDOW.fetch_add(1, Ordering::Relaxed));
    let links = app.state::<HelpLinks>();
    let mut slot = links.helper();
    if let Some(earlier) = slot.take() {
        close_helper_slot(&app, earlier);
    }
    let task_app = app.clone();
    let task_label = label.clone();
    let task_link = link.clone();
    let task = tauri::async_runtime::spawn(async move {
        run_helper(task_app, task_label, task_link).await;
    });
    *slot = Some(HelperSlot {
        session: link.session,
        label,
        peer_name: link.peer_name,
        bridge: None,
        task: Some(task),
    });
}

async fn run_helper<R: Runtime>(app: AppHandle<R>, label: String, link: Link) {
    let session = link.session;
    let mut end = match connect(&app, &session, HelpEnd::Guest).await {
        Ok(end) => end,
        Err(e) => {
            tracing::warn!("Settings help: the relay could not be reached: {}", e);
            forget_helper(&app, &session);
            crate::signaling::help_link_closed(&app, Role::Helper, &session).await;
            return;
        }
    };
    let (outgoing, mut from_window) = mpsc::channel::<String>(32);
    let bridge = Arc::new(Bridge::new(outgoing));
    let registered = {
        let links = app.state::<HelpLinks>();
        let mut slot = links.helper();
        match slot.as_mut() {
            Some(held) if held.session == session => {
                held.bridge = Some(bridge.clone());
                true
            }
            _ => false,
        }
    };
    if !registered {
        // Ended while connecting: there is no help to serve.
        end.writer.close().await;
        return;
    }
    if let Err(e) = open_window(&app, &label, &link.peer_name) {
        tracing::warn!("Settings help: the window could not be opened: {}", e);
        end.writer.close().await;
        forget_helper(&app, &session);
        crate::signaling::help_link_closed(&app, Role::Helper, &session).await;
        return;
    }

    tracing::info!("Settings help: the window {} is open on the relay", label);
    let ended = loop {
        tokio::select! {
            frame = end.reader.recv_text() => match frame {
                Ok(Some(text)) => {
                    if let Some(event) = bridge.receive(&text) {
                        let payload = json!({ "name": event.name, "payload": event.payload });
                        if let Err(e) = app.emit_to(label.as_str(), REMOTE_EVENT, payload) {
                            tracing::debug!("Settings help: an event could not be passed on: {}", e);
                        }
                    }
                }
                Ok(None) => break Ended::ByRelay,
                Err(e) => break Ended::ReadFailed(e.to_string()),
            },
            call = from_window.recv() => match call {
                Some(text) => {
                    if let Err(e) = end.writer.send_text(text).await {
                        break Ended::SendFailed(e.to_string());
                    }
                }
                None => break Ended::ByRelay,
            },
        }
    };
    match ended {
        Ended::ByRelay => tracing::info!("Settings help: the relay connection ended"),
        Ended::ReadFailed(e) => tracing::warn!("Settings help: the relay connection failed: {}", e),
        Ended::SendFailed(e) => tracing::warn!("Settings help: sending to the relay failed: {}", e),
    }
    bridge.close();
    end.writer.close().await;
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.close();
    }
    forget_helper(&app, &session);
    crate::signaling::help_link_closed(&app, Role::Helper, &session).await;
}

fn forget_helper<R: Runtime>(app: &AppHandle<R>, session: &str) {
    let links = app.state::<HelpLinks>();
    let mut slot = links.helper();
    if slot.as_ref().is_some_and(|held| held.session == session) {
        *slot = None;
    }
}

/// The window a helper works in. Closing it stops the help.
fn open_window<R: Runtime>(app: &AppHandle<R>, label: &str, peer_name: &str) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html#/help".into()))
        .title(format!("jamjam - {}", peer_name))
        .inner_size(1134.0, 700.0)
        .min_inner_size(800.0, 500.0)
        .resizable(true)
        .build()?;
    let app = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::signaling::stop_helping_from_window(&app).await;
            });
        }
    });
    Ok(())
}

fn close_helper_slot<R: Runtime>(app: &AppHandle<R>, slot: HelperSlot) {
    if let Some(task) = slot.task {
        task.abort();
    }
    if let Some(bridge) = slot.bridge {
        bridge.close();
    }
    if let Some(window) = app.get_webview_window(&slot.label) {
        let _ = window.close();
    }
}

/// Ends this app's end of the help in `role`: the relay connection is dropped,
/// which the relay tells the other end, and the helper's window closes. Does
/// nothing when there is none, so every way a help can end may call it.
pub fn end<R: Runtime>(app: &AppHandle<R>, role: Role) {
    let links = app.state::<HelpLinks>();
    match role {
        Role::Helped => {
            if let Some(slot) = links.host().take() {
                slot.task.abort();
            }
        }
        Role::Helper => {
            let slot = links.helper().take();
            if let Some(slot) = slot {
                close_helper_slot(app, slot);
            }
        }
    }
}

/// Calls `method` on the app being helped, from the helper's window. What the
/// window may call is decided over there, by the table for the help portal.
#[tauri::command]
pub async fn help_call(
    window: WebviewWindow,
    method: String,
    params: Option<Value>,
    links: tauri::State<'_, HelpLinks>,
) -> Result<Value, RpcError> {
    let bridge = links
        .helper()
        .as_ref()
        .filter(|slot| slot.label == window.label())
        .and_then(|slot| slot.bridge.clone())
        .ok_or_else(|| RpcError::new(Code::NoWindow, "this window is not helping anyone"))?;
    bridge.call(method, params.unwrap_or(Value::Null)).await
}

/// Who the helper's window is helping.
#[derive(Debug, Serialize)]
pub struct HelpWindowInfo {
    pub peer_name: String,
}

/// Who the calling window is helping; an error for a window that helps no one.
#[tauri::command]
pub fn help_window_info(
    window: WebviewWindow,
    links: tauri::State<'_, HelpLinks>,
) -> Result<HelpWindowInfo, String> {
    links
        .helper()
        .as_ref()
        .filter(|slot| slot.label == window.label())
        .map(|slot| HelpWindowInfo {
            peer_name: slot.peer_name.clone(),
        })
        .ok_or_else(|| "this window is not helping anyone".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> (Arc<Bridge>, mpsc::Receiver<String>) {
        let (outgoing, sent) = mpsc::channel(8);
        (Arc::new(Bridge::new(outgoing)), sent)
    }

    async fn sent(sent: &mut mpsc::Receiver<String>) -> Value {
        serde_json::from_str(&sent.recv().await.expect("a request should go out")).unwrap()
    }

    /// Verifies: REQ-RMT-002
    #[tokio::test]
    async fn a_call_goes_out_numbered_and_returns_what_answers_that_number() {
        let (bridge, mut out) = bridge();
        let calling = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                bridge
                    .call("streaming_set_mute".into(), json!({"muted": true}))
                    .await
            })
        };

        let request = sent(&mut out).await;
        assert_eq!(request["method"], "streaming_set_mute");
        assert_eq!(request["params"], json!({"muted": true}));
        let id = request["id"].as_u64().unwrap();
        // An answer to another number settles nothing.
        assert_eq!(
            bridge.receive(&json!({"id": id + 100, "ok": 1}).to_string()),
            None
        );
        assert_eq!(
            bridge.receive(&json!({"id": id, "ok": true}).to_string()),
            None
        );

        assert_eq!(calling.await.unwrap(), Ok(json!(true)));
    }

    /// Verifies: REQ-RMT-002, REQ-RMT-025
    #[tokio::test]
    async fn a_refusal_comes_back_as_the_error_the_other_end_gave() {
        let (bridge, mut out) = bridge();
        let calling = {
            let bridge = bridge.clone();
            tokio::spawn(
                async move { bridge.call("signaling_send_chat".into(), Value::Null).await },
            )
        };
        let id = sent(&mut out).await["id"].as_u64().unwrap();

        bridge.receive(
            &json!({"id": id, "error": {"code": "denied", "message": "Help may not call x"}})
                .to_string(),
        );

        let error = calling.await.unwrap().unwrap_err();
        assert_eq!(error.code, Code::Denied);
    }

    /// Answers come in any order.
    ///
    /// Verifies: REQ-RMT-002
    #[tokio::test]
    async fn calls_in_flight_at_once_are_answered_each_by_its_own_number() {
        let (bridge, mut out) = bridge();
        let first = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("a".into(), Value::Null).await })
        };
        let first_id = sent(&mut out).await["id"].as_u64().unwrap();
        let second = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("b".into(), Value::Null).await })
        };
        let second_id = sent(&mut out).await["id"].as_u64().unwrap();
        assert_ne!(first_id, second_id);

        bridge.receive(&json!({"id": second_id, "ok": "second"}).to_string());
        bridge.receive(&json!({"id": first_id, "ok": "first"}).to_string());

        assert_eq!(first.await.unwrap(), Ok(json!("first")));
        assert_eq!(second.await.unwrap(), Ok(json!("second")));
    }

    /// Verifies: REQ-RMT-002
    #[test]
    fn an_event_of_the_helped_app_is_handed_to_the_window_and_the_greeting_is_not() {
        let (bridge, _out) = bridge();

        assert_eq!(
            bridge.receive(
                &json!({"event": "session:changed", "payload": {"revision": 4}}).to_string()
            ),
            Some(RemoteEvent {
                name: "session:changed".into(),
                payload: json!({"revision": 4})
            })
        );
        assert_eq!(
            bridge.receive(&json!({"hello": {"portal": "help"}}).to_string()),
            None
        );
        assert_eq!(
            bridge.receive(&json!({"relay": "paired"}).to_string()),
            None
        );
        assert_eq!(bridge.receive("not json"), None);
    }

    /// Verifies: REQ-RMT-003
    #[tokio::test]
    async fn when_the_connection_ends_the_calls_waiting_fail_and_new_ones_do_too() {
        let (bridge, out) = bridge();
        let calling = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.call("a".into(), Value::Null).await })
        };
        tokio::task::yield_now().await;

        bridge.close();
        drop(out);

        assert_eq!(calling.await.unwrap().unwrap_err().code, Code::Failed);
        assert_eq!(
            bridge.call("b".into(), Value::Null).await.unwrap_err().code,
            Code::Failed
        );
    }

    /// Verifies: REQ-RMT-002
    #[tokio::test(start_paused = true)]
    async fn a_call_the_other_end_never_answers_times_out() {
        let (bridge, _out) = bridge();

        let error = bridge.call("a".into(), Value::Null).await.unwrap_err();

        assert_eq!(error.code, Code::Timeout);
    }
}
