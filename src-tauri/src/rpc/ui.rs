//! `ui.*`: looking at and operating the rendered screen (ADR-025, ADR-044).
//!
//! What the E2E control channel exposes over loopback HTTP and what the debug
//! portal exposes over the relay are the same methods, implemented once here.
//! Only builds with `debug-tools` (E2E and beta) contain this module.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};

use super::spec::{Access, Kind, Method};
use super::webview::{eval_in, js_string};
use super::{Call, Code, RpcError};

/// Cap on returned element text, so one enormous node can't blow up a
/// response. Well above anything this UI renders in a single element.
const MAX_TEXT_LEN: usize = 2000;

pub const METHODS: &[Method] = &[
    Method {
        name: "ui.windows",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "開いているウィンドウの名前の一覧",
    },
    Method {
        name: "ui.dom",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "描画された画面の HTML 全体",
    },
    Method {
        name: "ui.query",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "セレクタに合う要素の数・文字・値・見えているか・使えるか",
    },
    Method {
        name: "ui.click",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "セレクタに合う最初の要素を押す",
    },
    Method {
        name: "ui.input",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "セレクタに合う最初の要素に入力する・選ぶ",
    },
];

/// Structured result of a `querySelectorAll`, reported for the first match.
#[derive(Debug, Serialize, Deserialize)]
pub struct QueryResult {
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
struct DomParams {
    /// Window label; the main window when absent. Settings and chat are
    /// separate Tauri windows, so a user-facing flow spans several.
    #[serde(default)]
    window: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueryParams {
    selector: String,
    #[serde(default)]
    window: Option<String>,
    /// Attribute to read off the first match, if any.
    #[serde(default)]
    attribute: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClickParams {
    selector: String,
    #[serde(default)]
    window: Option<String>,
}

#[derive(Debug, Deserialize)]
struct InputParams {
    selector: String,
    value: String,
    #[serde(default)]
    window: Option<String>,
}

/// Outcome of an operation that acts on a single element.
#[derive(Debug, Serialize, Deserialize)]
struct ActionResult {
    /// False when the selector matched nothing - the caller decides whether
    /// that is a failure, so a missing element never looks like success.
    performed: bool,
}

fn params<T: for<'de> Deserialize<'de>>(call: &Call) -> Result<T, RpcError> {
    let params = if call.params.is_null() {
        Value::Object(Default::default())
    } else {
        call.params.clone()
    };
    serde_json::from_value(params).map_err(|e| {
        RpcError::invalid_params(format!("parameters of {} do not fit: {}", call.method, e))
    })
}

fn result<T: for<'de> Deserialize<'de> + Serialize>(
    what: &str,
    value: Value,
) -> Result<Value, RpcError> {
    let parsed: T = serde_json::from_value(value.clone())
        .map_err(|e| RpcError::failed(format!("unexpected {} result {}: {}", what, value, e)))?;
    serde_json::to_value(parsed).map_err(|e| RpcError::failed(e.to_string()))
}

/// Runs the `ui.*` method `name`.
pub async fn call<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    call: Call,
) -> Result<Value, RpcError> {
    match name {
        "ui.windows" => Ok(windows(app)),
        "ui.dom" => dom(app, params(&call)?).await,
        "ui.query" => query(app, params(&call)?).await,
        "ui.click" => click(app, params(&call)?).await,
        "ui.input" => input(app, params(&call)?).await,
        other => Err(RpcError::new(
            Code::UnknownMethod,
            format!("no method named {:?}", other),
        )),
    }
}

/// Labels of the windows currently open. Lets a scenario assert that
/// e.g. opening settings actually created the settings window.
fn windows<R: Runtime>(app: &AppHandle<R>) -> Value {
    let mut labels: Vec<String> = app.webview_windows().keys().cloned().collect();
    labels.sort();
    serde_json::json!(labels)
}

/// Full rendered DOM. Used for dumps and for assertions that are easier to
/// express over the whole document than a selector (e.g. "this string
/// appears nowhere").
async fn dom<R: Runtime>(app: &AppHandle<R>, params: DomParams) -> Result<Value, RpcError> {
    let value = eval_in(
        app,
        params.window.as_deref(),
        "document.documentElement.outerHTML",
    )
    .await?;
    match value {
        Value::String(_) => Ok(value),
        other => Err(RpcError::failed(format!(
            "expected a string, got {}",
            other
        ))),
    }
}

/// Structured lookup for a CSS selector. This is what the page object model
/// is built on; returning parsed fields keeps an HTML parser out of the
/// harness.
async fn query<R: Runtime>(app: &AppHandle<R>, params: QueryParams) -> Result<Value, RpcError> {
    let selector = js_string(&params.selector)?;
    let attribute = js_string(params.attribute.as_deref().unwrap_or(""))?;

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

    let value = eval_in(app, params.window.as_deref(), &js).await?;
    result::<QueryResult>("query", value)
}

/// Clicks the first match, as a user would. A disabled element reports
/// `performed: false` rather than silently doing nothing, so a test cannot
/// pass by clicking something the user could not have clicked.
async fn click<R: Runtime>(app: &AppHandle<R>, params: ClickParams) -> Result<Value, RpcError> {
    let selector = js_string(&params.selector)?;

    let js = format!(
        r#"(function () {{
            const el = document.querySelector({selector});
            if (!el || el.disabled) return {{ performed: false }};
            el.click();
            return {{ performed: true }};
        }})()"#
    );

    let value = eval_in(app, params.window.as_deref(), &js).await?;
    result::<ActionResult>("click", value)
}

/// Types into the first match, or picks an option when it is a `<select>`.
///
/// Assigning `.value` directly is not enough: React tracks the previous
/// value on the DOM node and would treat the assignment as a no-op, so
/// `onChange` would never fire and the component state would not update.
/// Going through the prototype's native setter and then dispatching a
/// bubbling `input` event is what makes React observe the change.
async fn input<R: Runtime>(app: &AppHandle<R>, params: InputParams) -> Result<Value, RpcError> {
    let selector = js_string(&params.selector)?;
    let value = js_string(&params.value)?;

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

    let value = eval_in(app, params.window.as_deref(), &js).await?;
    result::<ActionResult>("input", value)
}

#[cfg(test)]
mod tests {
    use super::*;

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
