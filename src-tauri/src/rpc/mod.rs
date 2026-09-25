//! Portals and permissions (ADR-044).
//!
//! Every way into the app - the screen, the E2E control channel, remote
//! debugging, someone helping with settings - reaches the same methods
//! through [`dispatch`], which asks the one table in [`spec`] whether that
//! portal may call that method. The judgment is made here, by the app being
//! operated, never by the party operating it.
//!
#![allow(dead_code)]

pub mod events;
pub mod help;
pub mod link;
pub mod spec;
mod webview;

#[cfg(feature = "debug-tools")]
pub mod debug;
#[cfg(feature = "debug-tools")]
pub mod ui;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Runtime};

pub use spec::{Kind, Method, Portal};

/// Why a call did not return a value. `code` is what goes on the wire
/// (ADR-044 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: Code,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The portal is not allowed to call this method.
    Denied,
    /// No method has this name.
    UnknownMethod,
    /// The parameters do not fit the method.
    InvalidParams,
    /// The method ran and failed (a command's own error included).
    Failed,
    /// The method did not finish in time.
    Timeout,
    /// The window it was to run in is not open.
    NoWindow,
}

impl RpcError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(Code::Failed, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(Code::InvalidParams, message)
    }
}

/// One call to make.
pub struct Call {
    /// The method's name: a command (`settings_change`) or a tool method
    /// (`ui.query`).
    pub method: String,
    /// A command's arguments as the webview passes them (camelCase), or a tool
    /// method's parameters. Absent means none.
    pub params: Value,
    /// The window a command is called through; the main window when absent.
    pub window: Option<String>,
    /// How long the method may run.
    pub timeout: Duration,
}

/// The row for `name` if `portal` may call it.
///
/// A method the table does not know is [`Code::UnknownMethod`]; one it knows
/// but not for this portal is [`Code::Denied`].
pub fn authorize(portal: Portal, name: &str) -> Result<&'static Method, RpcError> {
    let method = spec::find(name)
        .ok_or_else(|| RpcError::new(Code::UnknownMethod, format!("no method named {:?}", name)))?;
    if !method.access.allows(portal) {
        return Err(RpcError::new(
            Code::Denied,
            format!("{:?} may not call {}", portal, method.name),
        ));
    }
    Ok(method)
}

/// Calls `call.method` for `portal`, if the table lets it. Neither an unknown
/// method nor one the portal may not call is run.
pub async fn dispatch<R: Runtime>(
    app: &AppHandle<R>,
    portal: Portal,
    call: Call,
) -> Result<Value, RpcError> {
    let method = authorize(portal, &call.method)?;
    match method.kind {
        Kind::App => webview::invoke(app, &call).await,
        Kind::Native => call_native(app, method.name, call).await,
    }
}

#[cfg(feature = "debug-tools")]
async fn call_native<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    call: Call,
) -> Result<Value, RpcError> {
    if name.starts_with("debug.") {
        debug::call(app, name, call).await
    } else {
        ui::call(app, name, call).await
    }
}

#[cfg(not(feature = "debug-tools"))]
async fn call_native<R: Runtime>(
    _app: &AppHandle<R>,
    name: &str,
    _call: Call,
) -> Result<Value, RpcError> {
    Err(RpcError::new(
        Code::UnknownMethod,
        format!("no method named {:?}", name),
    ))
}

/// The methods implemented in Rust rather than as commands, by module.
pub fn native_groups() -> &'static [&'static [Method]] {
    #[cfg(feature = "debug-tools")]
    {
        &[ui::METHODS, debug::METHODS]
    }
    #[cfg(not(feature = "debug-tools"))]
    {
        &[]
    }
}

/// The screen's answer to a call a portal started: the webview reports what
/// the command it was asked to run returned (see [`webview`]). Only the screen
/// calls this; no portal is allowed to.
#[tauri::command]
pub fn rpc_settle(id: u64, outcome: String, value: Value) {
    webview::settle(id, &outcome, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str) -> Call {
        Call {
            method: method.to_string(),
            params: Value::Null,
            window: None,
            timeout: Duration::from_secs(1),
        }
    }

    fn app() -> AppHandle<tauri::test::MockRuntime> {
        tauri::test::mock_app().handle().clone()
    }

    /// A method the table does not list cannot be called, whoever asks.
    ///
    /// Verifies: REQ-RMT-022
    #[tokio::test]
    async fn a_method_that_is_not_in_the_table_is_unknown_to_every_portal() {
        let app = app();
        for portal in [
            Portal::Screen,
            Portal::Loopback,
            Portal::Debug,
            Portal::Help,
        ] {
            let error = dispatch(&app, portal, call("no_such_command"))
                .await
                .unwrap_err();
            assert_eq!(error.code, Code::UnknownMethod, "{:?}", portal);
        }
    }

    /// A method the table lists but not for this portal is refused before it
    /// runs: there is no window here, so a call that ran would say so instead.
    ///
    /// Verifies: REQ-RMT-022, REQ-RMT-025
    #[tokio::test]
    async fn a_method_the_portal_may_not_call_is_denied_and_not_run() {
        let app = app();
        for method in ["signaling_send_chat", "session_leave", "rpc_settle"] {
            let error = dispatch(&app, Portal::Help, call(method))
                .await
                .unwrap_err();
            assert_eq!(error.code, Code::Denied, "{}", method);
        }
    }

    /// A method the portal may call is let through to the webview.
    ///
    /// Verifies: REQ-RMT-022
    #[tokio::test]
    async fn a_method_the_portal_may_call_reaches_the_webview() {
        let app = app();
        let error = dispatch(&app, Portal::Help, call("streaming_set_mute"))
            .await
            .unwrap_err();
        assert_eq!(error.code, Code::NoWindow);
    }
}
