//! Calling a command through the webview's IPC (ADR-043, ADR-044).
//!
//! A portal other than the screen reaches an app command the way the screen
//! does: by asking the webview to call it. Going through the webview rather
//! than calling Rust directly keeps the IPC's own checks (argument names, the
//! capability ACL of a release build) in the path.
//!
//! `eval_with_callback` cannot wait for a promise, so the call is started in
//! one eval and the webview reports how it settled by calling
//! [`super::rpc_settle`], which wakes the waiting [`invoke`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::oneshot;

use super::{Call, Code, RpcError};

/// Window called through when a call names none. Matches the single entry in
/// `tauri.conf.json`'s `app.windows`, which gets Tauri's default label.
pub const MAIN_WINDOW: &str = "main";

/// How long to wait for the webview to answer an eval. Bounded so a wedged
/// webview surfaces as an error instead of hanging the caller forever.
const EVAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Numbers the calls, so concurrent ones keep their results apart.
static NEXT_INVOKE_ID: AtomicU64 = AtomicU64::new(1);

type Settled = Result<Value, String>;

/// Calls that have been started and not yet settled, by number.
static PENDING: LazyLock<Mutex<HashMap<u64, oneshot::Sender<Settled>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn pending() -> std::sync::MutexGuard<'static, HashMap<u64, oneshot::Sender<Settled>>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wakes the call numbered `id` with how it settled. A number nobody waits for
/// (the call timed out) is dropped.
pub fn settle(id: u64, outcome: &str, value: Value) {
    let Some(tx) = pending().remove(&id) else {
        return;
    };
    let settled = if outcome == "ok" {
        Ok(value)
    } else {
        Err(match value {
            Value::String(text) => text,
            other => other.to_string(),
        })
    };
    let _ = tx.send(settled);
}

/// Calls `call.method` as a command through the webview and returns its
/// result. A command's own error comes back as [`Code::Failed`]: it is an
/// answer, not a failure of the channel.
pub async fn invoke<R: Runtime>(app: &AppHandle<R>, call: &Call) -> Result<Value, RpcError> {
    let id = NEXT_INVOKE_ID.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    pending().insert(id, tx);

    let started = match start_invoke_js(id, &call.method, &call.params) {
        Ok(js) => eval_in(app, call.window.as_deref(), &js).await.map(|_| ()),
        Err(e) => Err(e),
    };
    if let Err(e) = started {
        pending().remove(&id);
        return Err(e);
    }

    match tokio::time::timeout(call.timeout, rx).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(error))) => Err(RpcError::failed(error)),
        Ok(Err(_)) => Err(RpcError::failed("the webview dropped the call")),
        Err(_) => {
            pending().remove(&id);
            Err(RpcError::new(
                Code::Timeout,
                format!("{} did not settle within {:?}", call.method, call.timeout),
            ))
        }
    }
}

/// Starts `command` and has the webview report how it settled under `id`.
/// Every value from the call goes in as a JSON literal, so nothing in it can
/// run as code.
fn start_invoke_js(id: u64, command: &str, params: &Value) -> Result<String, RpcError> {
    let command = js_string(command)?;
    let args = if params.is_null() {
        "{}".to_string()
    } else {
        serde_json::to_string(params)
            .map_err(|e| RpcError::invalid_params(format!("args are not encodable: {}", e)))?
    };
    Ok(format!(
        r#"(function () {{
            const done = function (outcome, value) {{
                window.__TAURI_INTERNALS__.invoke("rpc_settle", {{ id: {id}, outcome: outcome, value: value }});
            }};
            window.__TAURI_INTERNALS__.invoke({command}, {args}).then(
                function (value) {{
                    done("ok", value === undefined ? null : value);
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
                    done("err", text);
                }}
            );
            return {{ started: true }};
        }})()"#
    ))
}

/// Encodes a Rust string as a JS string literal. Going through serde_json
/// escapes quotes and backslashes, so a value can never break out of the
/// literal and run as code.
pub fn js_string(value: &str) -> Result<String, RpcError> {
    serde_json::to_string(value)
        .map_err(|e| RpcError::invalid_params(format!("value is not encodable: {}", e)))
}

/// Runs `js` in a webview and returns its value.
///
/// `eval_with_callback` serializes the expression's result to JSON for us.
/// The expression is wrapped so an exception comes back as a value rather
/// than vanishing - on Windows, Tauri documents that exceptions from
/// `eval_with_callback` are swallowed, which would otherwise turn every
/// scripting mistake into an indistinguishable timeout.
pub async fn eval_in<R: Runtime>(
    app: &AppHandle<R>,
    label: Option<&str>,
    js: &str,
) -> Result<Value, RpcError> {
    let label = label.unwrap_or(MAIN_WINDOW);
    let window = app.get_webview_window(label).ok_or_else(|| {
        RpcError::new(
            Code::NoWindow,
            format!("webview window {:?} not found", label),
        )
    })?;

    let wrapped = format!(
        r#"(function () {{ try {{ return ({js}); }} catch (e) {{ return {{ __rpc_error: String(e) }}; }} }})()"#
    );

    let (tx, rx) = oneshot::channel();
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
        .map_err(|e| RpcError::failed(format!("eval failed: {}", e)))?;

    let raw = tokio::time::timeout(EVAL_TIMEOUT, rx)
        .await
        .map_err(|_| {
            RpcError::new(
                Code::Timeout,
                format!("webview did not answer within {:?}", EVAL_TIMEOUT),
            )
        })?
        .map_err(|_| RpcError::failed("webview dropped the callback"))?;

    let value: Value = serde_json::from_str(&raw)
        .map_err(|e| RpcError::failed(format!("eval returned invalid JSON {:?}: {}", raw, e)))?;

    if let Some(message) = value.get("__rpc_error").and_then(|v| v.as_str()) {
        return Err(RpcError::failed(format!("script threw: {}", message)));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let encoded = js_string(hostile).unwrap();
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
        let command = r#"x"); window.__pwned = 1; ("#;
        let args = serde_json::json!({ "name": "\"); window.__pwned = 2; (\"" });
        let js = start_invoke_js(7, command, &args).unwrap();

        let call = js
            .split("window.__TAURI_INTERNALS__.invoke(")
            .nth(2)
            .and_then(|rest| rest.split(").then(").next())
            .unwrap();
        let (encoded_command, encoded_args) = call.split_once(", ").unwrap();
        assert_eq!(
            serde_json::from_str::<String>(encoded_command).unwrap(),
            command
        );
        assert_eq!(serde_json::from_str::<Value>(encoded_args).unwrap(), args);
    }

    #[test]
    fn test_invoke_without_args_passes_an_empty_object() {
        assert!(start_invoke_js(1, "settings_get", &Value::Null)
            .unwrap()
            .contains(r#"invoke("settings_get", {})"#));
    }

    /// Verifies: REQ-RMT-022
    #[test]
    fn test_a_settled_call_wakes_the_call_waiting_for_it() {
        let (tx, mut rx) = oneshot::channel();
        pending().insert(u64::MAX - 1, tx);
        settle(
            u64::MAX - 1,
            "ok",
            serde_json::json!({ "buffer_size": 128 }),
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            Ok(serde_json::json!({ "buffer_size": 128 }))
        );

        let (tx, mut rx) = oneshot::channel();
        pending().insert(u64::MAX - 2, tx);
        settle(
            u64::MAX - 2,
            "err",
            Value::String("Device not found: x".to_string()),
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            Err("Device not found: x".to_string())
        );
    }

    #[test]
    fn test_an_answer_nobody_waits_for_is_dropped() {
        settle(u64::MAX - 3, "ok", Value::Null);
        assert!(!pending().contains_key(&(u64::MAX - 3)));
    }
}
