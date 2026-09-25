//! GUI E2E control channel (ADR-025, ADR-043)
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tracing::{info, warn};

/// Environment variable carrying the port the harness wants us to listen on.
/// Absent means "do not start" (see module docs).
const PORT_ENV: &str = "JAMJAM_E2E_CONTROL_PORT";

/// Window inspected when a request names none. Matches the single entry in
/// `tauri.conf.json`'s `app.windows`, which gets Tauri's default label.
/// Other labels live in [`crate::windows::labels`] (`settings`, `chat`, ...)
/// and are reachable by passing `window` on the request.
const MAIN_WINDOW: &str = "main";

/// How long to wait for the webview to answer an eval. Bounded so a wedged
/// webview surfaces as a 504 instead of hanging the harness forever.
const EVAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Cap on returned element text, so one enormous node can't blow up a
/// response. Well above anything this UI renders in a single element.
const MAX_TEXT_LEN: usize = 2000;

/// How long a command called through `/e2e/invoke` may take. Longer than
/// [`EVAL_TIMEOUT`] because it is the command's own run time - a complete
/// diagnostics run measures the network - not the webview's responsiveness.
const INVOKE_TIMEOUT: Duration = Duration::from_secs(60);

/// How often to look for a called command's result.
const INVOKE_POLL: Duration = Duration::from_millis(20);

/// Numbers the `/e2e/invoke` calls, so concurrent ones keep their results apart.
static NEXT_INVOKE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct ControlState {
    app: AppHandle,
}

/// Structured result of a `querySelectorAll`, reported for the first match.
#[derive(Debug, Serialize, Deserialize)]
struct QueryResult {
    count: usize,
    exists: bool,
    text: Option<String>,
    value: Option<String>,
    visible: bool,
    /// Whether the element is disabled. A user cannot operate a disabled
    /// control, so assertions about "can the user do X" need this.
    #[serde(default)]
    disabled: bool,
    /// For a `<select>`, the choices offered to the user. Empty for anything
    /// else. Needed because a dropdown's contents are the state under test
    /// (e.g. "the device list is populated"), not just its current value.
    #[serde(default)]
    options: Vec<SelectOption>,
    /// Value of the attribute named in the request, when one was named.
    /// Lets a component publish a machine-readable value alongside the
    /// human-readable text it renders (e.g. a meter's raw level next to its
    /// rounded, localised dB string).
    #[serde(default)]
    attribute: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SelectOption {
    value: String,
    label: String,
    /// A disabled option (such as a "Select device" placeholder) is shown
    /// but cannot be chosen.
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Deserialize)]
struct QueryRequest {
    selector: String,
    /// Window label; defaults to [`MAIN_WINDOW`]. Settings and chat are
    /// separate Tauri windows, so a user-facing flow spans several.
    #[serde(default)]
    window: Option<String>,
    /// Attribute to read off the first match, if any.
    #[serde(default)]
    attribute: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClickRequest {
    selector: String,
    #[serde(default)]
    window: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InputRequest {
    selector: String,
    value: String,
    #[serde(default)]
    window: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DomRequest {
    #[serde(default)]
    window: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    /// A command the app registers (`settings_change`, `streaming_status`, ...)
    command: String,
    /// Its arguments as the webview passes them: camelCase names
    /// (`{"connId": 1}` for `conn_id`). Absent means none.
    #[serde(default)]
    args: serde_json::Value,
    /// Window whose IPC to call through; defaults to [`MAIN_WINDOW`].
    #[serde(default)]
    window: Option<String>,
}

/// What the command returned, or the error it failed with. Either way the
/// call happened: a command's error is an answer for the scenario to assert
/// on, not a failure of this channel.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum InvokeResult {
    Ok { value: serde_json::Value },
    Err { error: String },
}

/// Outcome of an operation that acts on a single element.
#[derive(Debug, Serialize, Deserialize)]
struct ActionResult {
    /// False when the selector matched nothing - the caller decides whether
    /// that is a failure, so a missing element never looks like success.
    performed: bool,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

#[derive(Debug)]
enum ControlError {
    NoWindow(String),
    Eval(String),
    Timeout,
}

impl IntoResponse for ControlError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            ControlError::NoWindow(label) => (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("webview window {:?} not found", label),
            ),
            ControlError::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                format!("webview did not answer within {:?}", EVAL_TIMEOUT),
            ),
            ControlError::Eval(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(ErrorBody { error })).into_response()
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

/// Labels of the windows currently open. Lets a scenario assert that
/// e.g. opening settings actually created the settings window.
async fn windows(State(state): State<ControlState>) -> Json<Vec<String>> {
    let mut labels: Vec<String> = state.app.webview_windows().keys().cloned().collect();
    labels.sort();
    Json(labels)
}

/// Full rendered DOM. Used for dumps and for assertions that are easier to
/// express over the whole document than a selector (e.g. "this string
/// appears nowhere").
async fn dom(
    State(state): State<ControlState>,
    Json(request): Json<DomRequest>,
) -> Result<String, ControlError> {
    let value = eval_in(
        &state.app,
        request.window.as_deref(),
        "document.documentElement.outerHTML",
    )
    .await?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ControlError::Eval(format!("expected a string, got {}", value)))
}

/// Structured lookup for a CSS selector. This is what the page object model
/// is built on; returning parsed fields keeps an HTML parser out of the
/// harness.
async fn query(
    State(state): State<ControlState>,
    Json(request): Json<QueryRequest>,
) -> Result<Json<QueryResult>, ControlError> {
    let selector = js_string(&request.selector)?;
    let attribute = js_string(request.attribute.as_deref().unwrap_or(""))?;

    let js = format!(
        r#"(function () {{
            const els = document.querySelectorAll({selector});
            const first = els[0];
            return {{
                count: els.length,
                exists: els.length > 0,
                text: first ? (first.innerText || "").slice(0, {MAX_TEXT_LEN}) : null,
                value: first && "value" in first ? String(first.value) : null,
                visible: first
                    ? !!(first.offsetWidth || first.offsetHeight || first.getClientRects().length)
                    : false,
                disabled: first ? !!first.disabled : false,
                attribute: first && {attribute} ? first.getAttribute({attribute}) : null,
                options: first && first.tagName === "SELECT"
                    ? Array.prototype.map.call(first.options, function (o) {{
                        return {{
                            value: String(o.value),
                            label: String(o.textContent || ""),
                            disabled: !!o.disabled,
                        }};
                    }})
                    : [],
            }};
        }})()"#
    );

    let value = eval_in(&state.app, request.window.as_deref(), &js).await?;
    serde_json::from_value(value.clone())
        .map(Json)
        .map_err(|e| ControlError::Eval(format!("unexpected query result {}: {}", value, e)))
}

/// Clicks the first match, as a user would. A disabled element reports
/// `performed: false` rather than silently doing nothing, so a test cannot
/// pass by clicking something the user could not have clicked.
async fn click(
    State(state): State<ControlState>,
    Json(request): Json<ClickRequest>,
) -> Result<Json<ActionResult>, ControlError> {
    let selector = js_string(&request.selector)?;

    let js = format!(
        r#"(function () {{
            const el = document.querySelector({selector});
            if (!el || el.disabled) return {{ performed: false }};
            el.click();
            return {{ performed: true }};
        }})()"#
    );

    let value = eval_in(&state.app, request.window.as_deref(), &js).await?;
    serde_json::from_value(value.clone())
        .map(Json)
        .map_err(|e| ControlError::Eval(format!("unexpected click result {}: {}", value, e)))
}

/// Types into the first match, or picks an option when it is a `<select>`.
///
/// Assigning `.value` directly is not enough: React tracks the previous
/// value on the DOM node and would treat the assignment as a no-op, so
/// `onChange` would never fire and the component state would not update.
/// Going through the prototype's native setter and then dispatching a
/// bubbling `input` event is what makes React observe the change.
async fn input(
    State(state): State<ControlState>,
    Json(request): Json<InputRequest>,
) -> Result<Json<ActionResult>, ControlError> {
    let selector = js_string(&request.selector)?;
    let value = js_string(&request.value)?;

    let js = format!(
        r#"(function () {{
            const el = document.querySelector({selector});
            if (!el || el.disabled) return {{ performed: false }};
            const proto = el instanceof window.HTMLSelectElement
                ? window.HTMLSelectElement.prototype
                : el instanceof window.HTMLTextAreaElement
                ? window.HTMLTextAreaElement.prototype
                : window.HTMLInputElement.prototype;
            // A <select> can only hold a value it actually offers; assigning
            // an absent one silently leaves the old selection, which would
            // look like a successful change. A disabled option is offered to
            // the eye only - a user cannot pick it, so neither can a test.
            if (el instanceof window.HTMLSelectElement) {{
                const offered = Array.prototype.some.call(el.options, function (o) {{
                    return String(o.value) === {value} && !o.disabled;
                }});
                if (!offered) return {{ performed: false }};
            }}
            const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
            setter.call(el, {value});
            el.dispatchEvent(new Event("input", {{ bubbles: true }}));
            el.dispatchEvent(new Event("change", {{ bubbles: true }}));
            return {{ performed: true }};
        }})()"#
    );

    let result = eval_in(&state.app, request.window.as_deref(), &js).await?;
    serde_json::from_value(result.clone())
        .map(Json)
        .map_err(|e| ControlError::Eval(format!("unexpected input result {}: {}", result, e)))
}

/// Calls an app command through the webview's IPC, exactly as the UI calls
/// it, and returns its result.
///
/// This is how a scenario reaches every feature, not only the ones a page
/// object wraps: any command the app registers is callable, and a new one is
/// callable without touching this channel. Going through the webview rather
/// than calling Rust directly also keeps the IPC's own checks (argument
/// names, the capability ACL of a release build) in the path under test.
///
/// `eval_with_callback` cannot wait for a promise, so the call is started in
/// one eval and its settled result read back by later ones.
async fn invoke(
    State(state): State<ControlState>,
    Json(request): Json<InvokeRequest>,
) -> Result<Json<InvokeResult>, ControlError> {
    let id = NEXT_INVOKE_ID.fetch_add(1, Ordering::Relaxed);
    let window = request.window.as_deref();
    eval_in(&state.app, window, &start_invoke_js(id, &request)?).await?;

    let deadline = Instant::now() + INVOKE_TIMEOUT;
    loop {
        let settled = eval_in(&state.app, window, &settled_invoke_js(id)).await?;
        if settled.get("done").and_then(|d| d.as_bool()) == Some(true) {
            return serde_json::from_value(settled.clone())
                .map(Json)
                .map_err(|e| {
                    ControlError::Eval(format!("unexpected invoke result {}: {}", settled, e))
                });
        }
        if Instant::now() >= deadline {
            // Forget the call, so an answer arriving later is not kept forever.
            let _ = eval_in(&state.app, window, &forget_invoke_js(id)).await;
            return Err(ControlError::Eval(format!(
                "{} did not settle within {:?}",
                request.command, INVOKE_TIMEOUT
            )));
        }
        tokio::time::sleep(INVOKE_POLL).await;
    }
}

/// Starts `request` and parks its outcome under `id` once it settles. Every
/// value from the request goes in as a JSON literal, so nothing in it can run
/// as code.
fn start_invoke_js(id: u64, request: &InvokeRequest) -> Result<String, ControlError> {
    let command = js_string(&request.command)?;
    let args = if request.args.is_null() {
        "{}".to_string()
    } else {
        serde_json::to_string(&request.args)
            .map_err(|e| ControlError::Eval(format!("args are not encodable: {}", e)))?
    };
    Ok(format!(
        r#"(function () {{
            const results = (window.__jamjamE2eInvoke = window.__jamjamE2eInvoke || {{}});
            results[{id}] = {{ done: false }};
            window.__TAURI_INTERNALS__.invoke({command}, {args}).then(
                function (value) {{
                    results[{id}] = {{ done: true, outcome: "ok", value: value === undefined ? null : value }};
                }},
                function (error) {{
                    let text;
                    if (typeof error === "string") {{
                        text = error;
                    }} else {{
                        // Not every rejection serializes (undefined, a cycle).
                        try {{ text = JSON.stringify(error); }} catch (e) {{ text = undefined; }}
                        if (typeof text !== "string") text = String(error);
                    }}
                    results[{id}] = {{ done: true, outcome: "err", error: text }};
                }}
            );
            return {{ started: true }};
        }})()"#
    ))
}

/// Drops whatever is parked under `id`.
fn forget_invoke_js(id: u64) -> String {
    format!(
        r#"(function () {{
            if (window.__jamjamE2eInvoke) delete window.__jamjamE2eInvoke[{id}];
            return {{ forgotten: true }};
        }})()"#
    )
}

/// Reads the outcome parked under `id`, removing it once settled.
fn settled_invoke_js(id: u64) -> String {
    format!(
        r#"(function () {{
            const results = window.__jamjamE2eInvoke || {{}};
            const result = results[{id}];
            if (!result || !result.done) return {{ done: false }};
            delete results[{id}];
            return result;
        }})()"#
    )
}

/// Encodes a Rust string as a JS string literal. Going through serde_json
/// escapes quotes and backslashes, so a selector can never break out of the
/// literal and run as code.
fn js_string(value: &str) -> Result<String, ControlError> {
    serde_json::to_string(value)
        .map_err(|e| ControlError::Eval(format!("value is not encodable: {}", e)))
}

/// Runs `js` in the main webview and returns its value.
///
/// `eval_with_callback` serializes the expression's result to JSON for us.
/// The expression is wrapped so an exception comes back as a value rather
/// than vanishing - on Windows, Tauri documents that exceptions from
/// `eval_with_callback` are swallowed, which would otherwise turn every
/// scripting mistake into an indistinguishable timeout.
async fn eval_in(
    app: &AppHandle,
    label: Option<&str>,
    js: &str,
) -> Result<serde_json::Value, ControlError> {
    let label = label.unwrap_or(MAIN_WINDOW);
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| ControlError::NoWindow(label.to_string()))?;

    let wrapped = format!(
        r#"(function () {{ try {{ return ({js}); }} catch (e) {{ return {{ __e2e_error: String(e) }}; }} }})()"#
    );

    let (tx, rx) = tokio::sync::oneshot::channel();
    // The callback is Fn (not FnOnce) so the sender lives in a slot the
    // first invocation takes.
    let slot = Mutex::new(Some(tx));
    window
        .eval_with_callback(wrapped, move |result| {
            if let Ok(mut slot) = slot.lock() {
                if let Some(tx) = slot.take() {
                    let _ = tx.send(result);
                }
            }
        })
        .map_err(|e| ControlError::Eval(format!("eval failed: {}", e)))?;

    let raw = tokio::time::timeout(EVAL_TIMEOUT, rx)
        .await
        .map_err(|_| ControlError::Timeout)?
        .map_err(|_| ControlError::Eval("webview dropped the callback".to_string()))?;

    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| ControlError::Eval(format!("eval returned invalid JSON {:?}: {}", raw, e)))?;

    if let Some(message) = value.get("__e2e_error").and_then(|v| v.as_str()) {
        return Err(ControlError::Eval(format!("script threw: {}", message)));
    }
    Ok(value)
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

    /// A selector containing quotes must not be able to terminate the string
    /// literal and inject script into the evaluated expression.
    #[test]
    fn test_selector_is_encoded_as_a_js_string_literal() {
        for hostile in [
            r#"a"] ; window.__pwned = 1; //"#,
            r#"a\"#,
            "a\nb",
            r#"</script>"#,
        ] {
            let encoded = serde_json::to_string(hostile).unwrap();
            assert!(encoded.starts_with('"') && encoded.ends_with('"'));

            // Every quote inside the literal must be backslash-escaped -
            // an unescaped one would end the literal early and let the rest
            // of the selector run as code.
            let body: Vec<char> = encoded[1..encoded.len() - 1].chars().collect();
            for (i, c) in body.iter().enumerate() {
                if *c == '"' {
                    let escaped = i > 0 && body[i - 1] == '\\';
                    assert!(escaped, "unescaped quote in {:?}", encoded);
                }
            }

            assert_eq!(serde_json::from_str::<String>(&encoded).unwrap(), hostile);
        }
    }

    /// A command name or argument holding quotes stays a JSON literal and
    /// cannot close the call it is passed to.
    #[test]
    fn test_invoke_request_values_go_in_as_literals() {
        let request = InvokeRequest {
            command: r#"x"); window.__pwned = 1; ("#.to_string(),
            args: serde_json::json!({ "name": "\"); window.__pwned = 2; (\"" }),
            window: None,
        };
        let js = start_invoke_js(7, &request).unwrap();

        let call = js
            .split("window.__TAURI_INTERNALS__.invoke(")
            .nth(1)
            .and_then(|rest| rest.split(").then(").next())
            .unwrap();
        let (command, args) = call.split_once(", ").unwrap();
        assert_eq!(
            serde_json::from_str::<String>(command).unwrap(),
            request.command
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(args).unwrap(),
            request.args
        );
    }

    #[test]
    fn test_invoke_without_args_passes_an_empty_object() {
        let request: InvokeRequest =
            serde_json::from_value(serde_json::json!({ "command": "settings_get" })).unwrap();
        assert!(start_invoke_js(1, &request)
            .unwrap()
            .contains(r#"invoke("settings_get", {})"#));
    }

    #[test]
    fn test_a_settled_call_reads_back_as_its_outcome() {
        let ok: InvokeResult = serde_json::from_value(serde_json::json!({
            "done": true, "outcome": "ok", "value": { "buffer_size": 128 }
        }))
        .unwrap();
        assert_eq!(
            ok,
            InvokeResult::Ok {
                value: serde_json::json!({ "buffer_size": 128 })
            }
        );

        let err: InvokeResult = serde_json::from_value(serde_json::json!({
            "done": true, "outcome": "err", "error": "Device not found: x"
        }))
        .unwrap();
        assert_eq!(
            err,
            InvokeResult::Err {
                error: "Device not found: x".to_string()
            }
        );
    }

    #[test]
    fn test_query_result_round_trips_through_json() {
        let value = serde_json::json!({
            "count": 2,
            "exists": true,
            "text": "jamjam へようこそ",
            "value": null,
            "visible": true,
        });
        let parsed: QueryResult = serde_json::from_value(value).unwrap();

        assert_eq!(parsed.count, 2);
        assert!(parsed.exists);
        assert_eq!(parsed.text.as_deref(), Some("jamjam へようこそ"));
        assert_eq!(parsed.value, None);
        assert!(parsed.visible);
    }
}
