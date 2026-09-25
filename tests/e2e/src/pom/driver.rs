//! HTTP client for the app's E2E control channel (ADR-025).
//!
//! Deliberately `pub(crate)`: scenarios talk to page objects, never to this.
//! If raw selectors leak into tests the page object model stops paying for
//! itself - every UI change would again mean editing every test.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Matches `QueryResult` in `src-tauri/src/e2e_control.rs`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct QueryResult {
    pub count: usize,
    pub exists: bool,
    pub text: Option<String>,
    pub value: Option<String>,
    pub visible: bool,
    #[serde(default)]
    pub disabled: bool,
    /// Choices offered by a `<select>`; empty for other elements.
    #[serde(default)]
    pub options: Vec<SelectOption>,
    /// Value of the requested attribute, when one was requested.
    #[serde(default)]
    pub attribute: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SelectOption {
    pub value: String,
    pub label: String,
    /// Shown but not choosable (a placeholder such as "Select device").
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Deserialize)]
struct ActionResult {
    performed: bool,
}

#[derive(Serialize)]
struct QueryBody<'a> {
    selector: &'a str,
    window: Option<&'a str>,
}

#[derive(Serialize)]
struct AttributeQueryBody<'a> {
    selector: &'a str,
    window: Option<&'a str>,
    attribute: &'a str,
}

#[derive(Serialize)]
struct InputBody<'a> {
    selector: &'a str,
    value: &'a str,
    window: Option<&'a str>,
}

#[derive(Serialize)]
struct DomBody<'a> {
    window: Option<&'a str>,
}

#[derive(Serialize)]
struct InvokeBody<'a> {
    command: &'a str,
    args: &'a serde_json::Value,
    window: Option<&'a str>,
}

/// Matches `InvokeResult` in `src-tauri/src/e2e_control.rs`.
#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum InvokeResult {
    Ok { value: serde_json::Value },
    Err { error: String },
}

/// Errors are strings rather than a rich enum: every one of them ends up in
/// a test failure message, and a scenario cannot recover from any of them.
pub type DriverResult<T> = Result<T, String>;

pub(crate) struct Driver {
    base: String,
}

impl Driver {
    pub(crate) fn new(port: u16) -> Self {
        Self {
            base: format!("http://127.0.0.1:{}", port),
        }
    }

    /// True once the app has started its control channel.
    pub(crate) fn is_healthy(&self) -> bool {
        ureq::get(&format!("{}/e2e/health", self.base))
            .timeout(Duration::from_secs(1))
            .call()
            .is_ok()
    }

    pub(crate) fn open_windows(&self) -> DriverResult<Vec<String>> {
        self.get_json("/e2e/windows")
    }

    pub(crate) fn dom(&self, window: Option<&str>) -> DriverResult<String> {
        ureq::post(&format!("{}/e2e/dom", self.base))
            .timeout(Duration::from_secs(10))
            .send_json(ureq::json!(DomBody { window }))
            .map_err(|e| format!("dom failed: {}", e))?
            .into_string()
            .map_err(|e| format!("dom body unreadable: {}", e))
    }

    pub(crate) fn query(&self, selector: &str, window: Option<&str>) -> DriverResult<QueryResult> {
        self.post_json("/e2e/query", ureq::json!(QueryBody { selector, window }))
    }

    /// Reads one attribute off the first match. `None` when the element or
    /// the attribute is absent.
    pub(crate) fn attribute(
        &self,
        selector: &str,
        attribute: &str,
        window: Option<&str>,
    ) -> DriverResult<Option<String>> {
        let result: QueryResult = self.post_json(
            "/e2e/query",
            ureq::json!(AttributeQueryBody {
                selector,
                window,
                attribute
            }),
        )?;
        Ok(result.attribute)
    }

    /// Clicks the element. Fails when nothing matched or the control is
    /// disabled - a user could not have clicked it either, so silently
    /// succeeding would let a test pass against a broken UI.
    pub(crate) fn click(&self, selector: &str, window: Option<&str>) -> DriverResult<()> {
        let result: ActionResult =
            self.post_json("/e2e/click", ureq::json!(QueryBody { selector, window }))?;
        if result.performed {
            Ok(())
        } else {
            Err(format!(
                "nothing clickable matched {:?} (missing or disabled)",
                selector
            ))
        }
    }

    pub(crate) fn input(
        &self,
        selector: &str,
        value: &str,
        window: Option<&str>,
    ) -> DriverResult<()> {
        let result: ActionResult = self.post_json(
            "/e2e/input",
            ureq::json!(InputBody {
                selector,
                value,
                window
            }),
        )?;
        if result.performed {
            Ok(())
        } else {
            Err(format!(
                "nothing writable matched {:?} (missing or disabled)",
                selector
            ))
        }
    }

    /// Calls an app command through the webview's IPC. The outer result is
    /// the channel's; the inner one is the command's own answer.
    pub(crate) fn invoke(
        &self,
        command: &str,
        args: &serde_json::Value,
    ) -> DriverResult<Result<serde_json::Value, String>> {
        self.invoke_in(command, args, None)
    }

    /// [`Self::invoke`] through the IPC of the window `window` (the main window
    /// when `None`): a command that answers for the window that called it, such
    /// as a helper's window reaching the app it helps.
    pub(crate) fn invoke_in(
        &self,
        command: &str,
        args: &serde_json::Value,
        window: Option<&str>,
    ) -> DriverResult<Result<serde_json::Value, String>> {
        // The command's own run time, which the channel allows up to a minute.
        let result: InvokeResult = ureq::post(&format!("{}/e2e/invoke", self.base))
            .timeout(Duration::from_secs(70))
            .send_json(ureq::json!(InvokeBody {
                command,
                args,
                window
            }))
            .map_err(|e| format!("invoking {} failed: {}", command, e))?
            .into_json()
            .map_err(|e| format!("invoking {} returned unexpected JSON: {}", command, e))?;
        Ok(match result {
            InvokeResult::Ok { value } => Ok(value),
            InvokeResult::Err { error } => Err(error),
        })
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> DriverResult<T> {
        ureq::get(&format!("{}{}", self.base, path))
            .timeout(Duration::from_secs(10))
            .call()
            .map_err(|e| format!("GET {} failed: {}", path, e))?
            .into_json()
            .map_err(|e| format!("GET {} returned unexpected JSON: {}", path, e))
    }

    fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> DriverResult<T> {
        ureq::post(&format!("{}{}", self.base, path))
            .timeout(Duration::from_secs(10))
            .send_json(body)
            .map_err(|e| format!("POST {} failed: {}", path, e))?
            .into_json()
            .map_err(|e| format!("POST {} returned unexpected JSON: {}", path, e))
    }
}
