//! What a remote portal says over the relay's WebSocket (ADR-044 §4).
//!
//! The app being operated speaks first with a `hello` and again whenever the
//! relay says a pair has formed; after that the other end sends requests, each
//! answered by the request's `id`, while the app pushes the events its portal
//! may hear. Every request goes through [`dispatch`](super::dispatch), so the
//! table decides what a request may do; nothing here is trusted to.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{broadcast, mpsc, Semaphore};

use super::events::{EventHub, EVENTS};
use super::spec::{all_methods, Portal};
use super::{dispatch, Call, RpcError};

/// The protocol's name and version, sent in the hello.
pub const PROTOCOL: &str = "jamjam-rpc/1";

/// The most a frame may carry, as the relay enforces. A result too large for
/// it is answered with an error instead: the method pages its own output.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long one method may run before it is answered with a timeout.
const CALL_TIMEOUT: Duration = Duration::from_secs(25);

/// Requests answered at once. More than this are refused rather than queued,
/// so a caller cannot make the app hold work without end.
const MAX_IN_FLIGHT: usize = 32;

/// Who this end of the relay is.
#[derive(Clone)]
pub struct Session {
    pub portal: Portal,
    /// `beta` or `e2e`: what the build says about itself.
    pub build: &'static str,
    pub app_version: String,
}

/// The first frame: the protocol, the build, and the methods this portal may
/// call, so the other end need not guess.
pub fn hello(session: &Session) -> String {
    let methods: Vec<&str> = all_methods()
        .filter(|m| m.access.allows(session.portal))
        .map(|m| m.name)
        .collect();
    json!({
        "hello": {
            "protocol": PROTOCOL,
            "app_version": session.app_version,
            "build": session.build,
            "portal": portal_name(session.portal),
            "methods": methods,
        }
    })
    .to_string()
}

fn portal_name(portal: Portal) -> &'static str {
    match portal {
        Portal::Screen => "screen",
        Portal::Loopback => "loopback",
        Portal::Debug => "debug",
        Portal::Help => "help",
    }
}

#[derive(Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    window: Option<String>,
}

fn ok_frame(id: u64, value: Value) -> String {
    let frame = json!({ "id": id, "ok": value }).to_string();
    if frame.len() > MAX_FRAME_BYTES {
        return error_frame(
            id,
            &RpcError::failed("the result is larger than one frame; ask for less"),
        );
    }
    frame
}

fn error_frame(id: u64, error: &RpcError) -> String {
    json!({ "id": id, "error": { "code": error.code, "message": error.message } }).to_string()
}

/// Serves one relay connection until it ends: `incoming` closing (the relay
/// hung up) or `outgoing` closing (this end gave up) both end it.
pub async fn run<R: Runtime>(
    app: AppHandle<R>,
    session: Session,
    mut incoming: mpsc::Receiver<String>,
    outgoing: mpsc::Sender<String>,
) {
    let in_flight = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
    let mut events = app.try_state::<EventHub>().map(|hub| hub.subscribe());

    if outgoing.send(hello(&session)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            frame = incoming.recv() => {
                let Some(text) = frame else { return };
                if !handle_frame(&app, &session, &in_flight, &outgoing, &text).await {
                    return;
                }
            }
            event = next_event(&mut events) => {
                let Some(event) = event else { continue };
                let allowed = EVENTS
                    .iter()
                    .any(|spec| spec.name == event.name && spec.access.allows(session.portal));
                if allowed {
                    let frame = json!({ "event": event.name, "payload": event.payload }).to_string();
                    if frame.len() <= MAX_FRAME_BYTES && outgoing.send(frame).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// The next event, or never when nothing is subscribed. A subscriber that
/// fell behind skips what it lost.
async fn next_event(
    events: &mut Option<broadcast::Receiver<super::events::Event>>,
) -> Option<super::events::Event> {
    let Some(receiver) = events else {
        return std::future::pending().await;
    };
    match receiver.recv().await {
        Ok(event) => Some(event),
        Err(broadcast::error::RecvError::Lagged(_)) => None,
        Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
    }
}

/// Acts on one frame from the other end. `false` when this end can no longer
/// send and should stop.
async fn handle_frame<R: Runtime>(
    app: &AppHandle<R>,
    session: &Session,
    in_flight: &Arc<Semaphore>,
    outgoing: &mpsc::Sender<String>,
    text: &str,
) -> bool {
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        return true;
    };
    // A pair has formed: the other end may have come after the first hello.
    if frame.get("relay").and_then(Value::as_str) == Some("paired") {
        return outgoing.send(hello(session)).await.is_ok();
    }
    let Ok(request) = serde_json::from_value::<Request>(frame) else {
        return true;
    };

    let Ok(permit) = in_flight.clone().try_acquire_owned() else {
        let error = RpcError::failed("too many calls are in flight");
        return outgoing.send(error_frame(request.id, &error)).await.is_ok();
    };
    let app = app.clone();
    let portal = session.portal;
    let outgoing = outgoing.clone();
    tauri::async_runtime::spawn(async move {
        let id = request.id;
        let result = dispatch(
            &app,
            portal,
            Call {
                method: request.method,
                params: request.params,
                window: request.window,
                timeout: CALL_TIMEOUT,
            },
        )
        .await;
        let frame = match result {
            Ok(value) => ok_frame(id, value),
            Err(error) => error_frame(id, &error),
        };
        let _ = outgoing.send(frame).await;
        drop(permit);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::Code;

    fn session(portal: Portal) -> Session {
        Session {
            portal,
            build: "e2e",
            app_version: "0.0.0".to_string(),
        }
    }

    async fn serve(
        portal: Portal,
    ) -> (
        mpsc::Sender<String>,
        mpsc::Receiver<String>,
        AppHandle<tauri::test::MockRuntime>,
    ) {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        // The mock app must outlive the test; leaking it is the simplest way.
        std::mem::forget(app);
        let (to_app, incoming) = mpsc::channel(8);
        let (outgoing, from_app) = mpsc::channel(8);
        tauri::async_runtime::spawn(run(handle.clone(), session(portal), incoming, outgoing));
        (to_app, from_app, handle)
    }

    async fn next(from_app: &mut mpsc::Receiver<String>) -> Value {
        let text = tokio::time::timeout(Duration::from_secs(2), from_app.recv())
            .await
            .expect("the app should answer")
            .expect("the app hung up");
        serde_json::from_str(&text).unwrap()
    }

    /// Verifies: REQ-RMT-024
    #[tokio::test]
    async fn the_app_speaks_first_and_names_what_its_portal_may_call() {
        let (_to_app, mut from_app, _app) = serve(Portal::Help).await;

        let hello = next(&mut from_app).await;

        assert_eq!(hello["hello"]["protocol"], PROTOCOL);
        assert_eq!(hello["hello"]["portal"], "help");
        let methods = hello["hello"]["methods"].as_array().unwrap();
        assert!(methods.iter().any(|m| m == "streaming_set_mute"));
        assert!(!methods.iter().any(|m| m == "signaling_send_chat"));
    }

    /// Verifies: REQ-RMT-024
    #[tokio::test]
    async fn when_a_pair_forms_the_app_says_hello_again() {
        let (to_app, mut from_app, _app) = serve(Portal::Help).await;
        next(&mut from_app).await;

        to_app
            .send(r#"{"relay":"paired"}"#.to_string())
            .await
            .unwrap();

        assert!(next(&mut from_app).await.get("hello").is_some());
    }

    /// The answer carries the request's id, so answers may come in any order.
    ///
    /// Verifies: REQ-RMT-024
    #[tokio::test]
    async fn a_request_is_answered_with_its_id() {
        let (to_app, mut from_app, _app) = serve(Portal::Help).await;
        next(&mut from_app).await;

        to_app
            .send(r#"{"id":7,"method":"no_such_method","params":{}}"#.to_string())
            .await
            .unwrap();

        let answer = next(&mut from_app).await;
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["error"]["code"], "unknown_method");
    }

    /// The table is asked for every request, so what a portal may not call is
    /// refused however the other end asks.
    ///
    /// Verifies: REQ-RMT-025
    #[tokio::test]
    async fn a_request_the_portal_may_not_make_is_denied() {
        let (to_app, mut from_app, _app) = serve(Portal::Help).await;
        next(&mut from_app).await;

        to_app
            .send(r#"{"id":1,"method":"signaling_send_chat","params":{}}"#.to_string())
            .await
            .unwrap();

        let answer = next(&mut from_app).await;
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["error"]["code"], "denied");
    }

    #[tokio::test]
    async fn a_frame_that_is_not_a_request_is_ignored() {
        let (to_app, mut from_app, _app) = serve(Portal::Help).await;
        next(&mut from_app).await;

        to_app.send("not json".to_string()).await.unwrap();
        to_app
            .send(r#"{"nothing":"useful"}"#.to_string())
            .await
            .unwrap();
        to_app
            .send(r#"{"id":2,"method":"no_such_method"}"#.to_string())
            .await
            .unwrap();

        // The next answer is the one for the request: the two before it made no reply.
        assert_eq!(next(&mut from_app).await["id"], 2);
    }

    #[test]
    fn a_result_larger_than_a_frame_is_answered_with_an_error() {
        let frame = ok_frame(3, Value::String("x".repeat(MAX_FRAME_BYTES)));
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["id"], 3);
        assert_eq!(frame["error"]["code"], "failed");
    }

    #[test]
    fn an_error_names_its_code_in_snake_case() {
        let frame = error_frame(1, &RpcError::new(Code::UnknownMethod, "x"));
        assert!(frame.contains(r#""code":"unknown_method""#), "{}", frame);
    }
}
