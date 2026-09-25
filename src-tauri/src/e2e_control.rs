//! GUI E2E control channel (ADR-025, ADR-043, ADR-044)
//!
//! Exposes the running app's rendered DOM over loopback HTTP so
//! `tests/e2e/` can assert on it, and lets a scenario call any of the app's
//! commands (`/e2e/invoke`) the way the webview does. This fills the gap on the right-hand side
//! of the V-model: nothing else exercises `src-tauri/` and `ui/` together
//! (Storybook only covers Pure components, which are not wired to `invoke`).
//!
//! # Disabled twice over
//!
//! 1. The whole module sits behind the `e2e-control` cargo feature, which is
//!    off by default - in a release build this code is not compiled in at all.
//! 2. Even when compiled in, the listener only starts if the harness sets
//!    `JAMJAM_E2E_CONTROL_PORT`. The port is chosen by the harness rather
//!    than discovered, so there is no port file to find and no listener on a
//!    developer's machine unless they asked for one.
//!
//! Never bind anything but loopback here: this channel can read whatever the
//! app is displaying (room codes, chat, participant names).

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tracing::{info, warn};

use crate::rpc::{self, Call, Code, Portal, RpcError};

/// Environment variable carrying the port the harness wants us to listen on.
/// Absent means "do not start" (see module docs).
const PORT_ENV: &str = "JAMJAM_E2E_CONTROL_PORT";

/// How long a command called through `/e2e/invoke` may take. It is the
/// command's own run time - a complete diagnostics run measures the network -
/// not the webview's responsiveness.
const INVOKE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct ControlState {
    app: AppHandle,
}

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    /// A command the app registers (`settings_change`, `streaming_status`, ...)
    command: String,
    /// Its arguments as the webview passes them: camelCase names
    /// (`{"connId": 1}` for `conn_id`). Absent means none.
    #[serde(default)]
    args: Value,
    /// Window whose IPC to call through; the main window when absent.
    #[serde(default)]
    window: Option<String>,
}

/// What the command returned, or the error it failed with. Either way the
/// call happened: a command's error is an answer for the scenario to assert
/// on, not a failure of this channel.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum InvokeResult {
    Ok { value: Value },
    Err { error: String },
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

/// A call the channel could not carry out (as opposed to a command that ran
/// and failed).
struct ControlError(RpcError);

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            Code::NoWindow => StatusCode::SERVICE_UNAVAILABLE,
            Code::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Code::Denied => StatusCode::FORBIDDEN,
            Code::UnknownMethod => StatusCode::NOT_FOUND,
            Code::InvalidParams => StatusCode::BAD_REQUEST,
            Code::Failed => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                error: self.0.message,
            }),
        )
            .into_response()
    }
}

fn build_router(app: AppHandle) -> Router {
    Router::new()
        .route("/e2e/health", get(health))
        .route("/e2e/windows", get(windows))
        .route("/e2e/dom", post(dom))
        .route("/e2e/query", post(query))
        .route("/e2e/click", post(click))
        .route("/e2e/input", post(input))
        .route("/e2e/invoke", post(invoke))
        .with_state(ControlState { app })
}

/// Readiness probe. The harness polls this after spawning the app so it can
/// tell "still starting up" apart from "failed to start".
async fn health() -> &'static str {
    "ok"
}

/// Calls `method` as the loopback portal, the same way every other portal
/// calls it (ADR-044). The endpoints below only translate HTTP.
async fn call(
    state: &ControlState,
    method: &str,
    params: Value,
    window: Option<String>,
    timeout: Duration,
) -> Result<Value, RpcError> {
    rpc::dispatch(
        &state.app,
        Portal::Loopback,
        Call {
            method: method.to_string(),
            params,
            window,
            timeout,
        },
    )
    .await
}

/// A `ui.*` call: the request body is the method's parameters.
async fn ui(
    state: &ControlState,
    method: &str,
    params: Value,
) -> Result<Json<Value>, ControlError> {
    call(state, method, params, None, Duration::from_secs(10))
        .await
        .map(Json)
        .map_err(ControlError)
}

/// Labels of the windows currently open.
async fn windows(State(state): State<ControlState>) -> Result<Json<Value>, ControlError> {
    ui(&state, "ui.windows", Value::Null).await
}

/// Full rendered DOM.
async fn dom(
    State(state): State<ControlState>,
    Json(request): Json<Value>,
) -> Result<String, ControlError> {
    let value = ui(&state, "ui.dom", request).await?.0;
    match value {
        Value::String(html) => Ok(html),
        other => Err(ControlError(RpcError::failed(format!(
            "expected a string, got {}",
            other
        )))),
    }
}

/// Structured lookup for a CSS selector.
async fn query(
    State(state): State<ControlState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ControlError> {
    ui(&state, "ui.query", request).await
}

/// Clicks the first match, as a user would.
async fn click(
    State(state): State<ControlState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ControlError> {
    ui(&state, "ui.click", request).await
}

/// Types into the first match, or picks an option when it is a `<select>`.
async fn input(
    State(state): State<ControlState>,
    Json(request): Json<Value>,
) -> Result<Json<Value>, ControlError> {
    ui(&state, "ui.input", request).await
}

/// Calls an app command through the webview's IPC, exactly as the UI calls
/// it, and returns its result.
///
/// This is how a scenario reaches every feature, not only the ones a page
/// object wraps: any command in the permission table is callable, and a new
/// one is callable without touching this channel.
async fn invoke(
    State(state): State<ControlState>,
    Json(request): Json<InvokeRequest>,
) -> Result<Json<InvokeResult>, ControlError> {
    match call(
        &state,
        &request.command,
        request.args,
        request.window,
        INVOKE_TIMEOUT,
    )
    .await
    {
        Ok(value) => Ok(Json(InvokeResult::Ok { value })),
        // A command that failed, and one the table does not list for this
        // portal, are both answers the scenario asserts on. What the channel
        // could not carry out (no window, no answer) is not.
        Err(RpcError {
            code: Code::Failed | Code::UnknownMethod | Code::Denied,
            message,
        }) => Ok(Json(InvokeResult::Err { error: message })),
        Err(other) => Err(ControlError(other)),
    }
}

/// Starts the control channel if [`PORT_ENV`] is set. Call from Tauri's
/// `setup` hook; returns immediately either way.
pub fn spawn(app: AppHandle) {
    let Some(port) = read_port() else {
        return;
    };

    tauri::async_runtime::spawn(async move {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                info!("E2E control channel listening on {}", addr);
                if let Err(e) = axum::serve(listener, build_router(app)).await {
                    warn!("E2E control channel stopped: {}", e);
                }
            }
            Err(e) => warn!("E2E control channel could not bind {}: {}", addr, e),
        }
    });
}

/// `None` when unset, empty, or unparseable - a malformed value must not
/// silently fall back to some default port.
fn read_port() -> Option<u16> {
    let raw = std::env::var(PORT_ENV).ok()?;
    match raw.trim().parse::<u16>() {
        Ok(0) | Err(_) => {
            warn!("{} is not a valid port: {:?}", PORT_ENV, raw);
            None
        }
        Ok(port) => Some(port),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Serialized because they mutate the process environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_port_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var(PORT_ENV).ok();
        match value {
            Some(v) => std::env::set_var(PORT_ENV, v),
            None => std::env::remove_var(PORT_ENV),
        }
        let result = f();
        match previous {
            Some(v) => std::env::set_var(PORT_ENV, v),
            None => std::env::remove_var(PORT_ENV),
        }
        result
    }

    /// Verifies: REQ-GUI-004
    #[test]
    fn test_no_port_env_means_no_listener() {
        assert_eq!(with_port_env(None, read_port), None);
    }

    /// Verifies: REQ-GUI-004
    #[test]
    fn test_port_env_is_used_verbatim() {
        assert_eq!(with_port_env(Some("39999"), read_port), Some(39999));
        assert_eq!(with_port_env(Some("  39999  "), read_port), Some(39999));
    }

    /// A malformed value must not fall back to a default port - that would
    /// open a listener the harness never asked for.
    ///
    /// Verifies: REQ-GUI-004
    #[test]
    fn test_malformed_port_env_means_no_listener() {
        for value in ["", "0", "not-a-port", "-1", "70000", "8080abc"] {
            assert_eq!(
                with_port_env(Some(value), read_port),
                None,
                "{:?} should not yield a port",
                value
            );
        }
    }

    #[test]
    fn test_a_command_that_failed_reads_back_as_its_error() {
        let err: InvokeResult = serde_json::from_value(serde_json::json!({
            "outcome": "err", "error": "Device not found: x"
        }))
        .unwrap();
        assert_eq!(
            err,
            InvokeResult::Err {
                error: "Device not found: x".to_string()
            }
        );
    }
}
