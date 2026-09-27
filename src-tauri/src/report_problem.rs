//! "Report a problem": the user's own action, from the Diagnostics tab, to
//! send the current `jamjam.log` (masked, tail-capped) and a comment to a
//! receiver kept apart from usage reporting (ADR-057, REQ-TEL-021..024).
//! Pressing send is the only consent asked; there is no setting that gates
//! this, and it does not touch `usage_reporting`.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use jamjam::config::{is_http_url, DEFAULT_SERVER_URL};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::logging::redact_secrets;

/// `jamjam.log` itself is capped at 5 MiB (ADR-036 §6); this feature sends
/// far less of it, tail-first, so the upload stays quick on a bad connection
/// and because the most recent activity is what an ongoing problem needs.
pub const MAX_LOG_BYTES: usize = 512 * 1024;

/// The comment's cap, in Unicode scalar values (matches the size discipline
/// of the `settings` string caps in `telemetry/settings.rs`).
pub const MAX_COMMENT_CHARS: usize = 2000;

const REPORT_PATH: &str = "/api/v1/problem-reports";
const SEND_TIMEOUT: Duration = Duration::from_secs(20);

type Delivery<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Where a report goes. A trait so a test can watch what would have been
/// sent, the same shape as `jamjam::telemetry::Transport`.
trait ReportTransport: Send + Sync {
    fn send(&self, body: Vec<u8>) -> Delivery<'_>;
}

/// Sends to the jamjam server the build was made for. This is a separate
/// channel from usage reporting's (ADR-037 decision 7 keeps `jamjam.log`
/// itself apart from that channel; this feature is a third one, gated by
/// the user's own action rather than a setting).
struct HttpReportTransport {
    endpoint: String,
}

impl HttpReportTransport {
    fn for_build() -> Option<Self> {
        if !is_http_url(DEFAULT_SERVER_URL) {
            return None;
        }
        Some(Self {
            endpoint: format!(
                "{}{}",
                DEFAULT_SERVER_URL.trim_end_matches('/'),
                REPORT_PATH
            ),
        })
    }
}

impl ReportTransport for HttpReportTransport {
    fn send(&self, body: Vec<u8>) -> Delivery<'_> {
        Box::pin(async move {
            let Ok(client) = reqwest::Client::builder()
                .timeout(SEND_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
            else {
                return false;
            };
            let sent = client
                .post(&self.endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await;
            matches!(sent, Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT)
        })
    }
}

#[derive(Serialize)]
struct ReportBody<'a> {
    ts: String,
    app_version: &'a str,
    os: &'a str,
    arch: &'a str,
    comment: &'a str,
    log: &'a str,
}

/// The last `max` bytes of `text`, cut at a char boundary, with a marker at
/// the front when something was cut off.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("...(先頭を省略 / start omitted)\n{}", &text[start..])
}

/// `comment` cut to at most [`MAX_COMMENT_CHARS`] Unicode scalar values.
fn capped_comment(comment: &str) -> String {
    comment.chars().take(MAX_COMMENT_CHARS).collect()
}

/// Reads `jamjam.log`, masked the same way `log_frontend` masks a webview
/// line (ADR-036 §7) and capped to [`MAX_LOG_BYTES`]. The preview command
/// and the send command both call this, so what is shown is exactly what is
/// sent (REQ-TEL-022).
fn read_log(app: &AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("ログのフォルダが分かりません: {}", e))?;
    let raw = std::fs::read_to_string(dir.join("jamjam.log"))
        .map_err(|e| format!("jamjam.log を読めません: {}", e))?;
    Ok(tail(&redact_secrets(&raw), MAX_LOG_BYTES))
}

/// The text `report_problem_send` will submit for `jamjam.log`, so the
/// screen can show it before the user decides to send (REQ-TEL-022).
#[tauri::command]
pub fn report_problem_preview(app: AppHandle) -> Result<String, String> {
    read_log(&app)
}

/// Sends the current `jamjam.log` (masked, capped) and `comment` (capped) to
/// the problem report intake. This is the only consent asked for this
/// report: there is no setting to turn on first, and it does not read or
/// change `usage_reporting` (REQ-TEL-021, REQ-TEL-023).
#[tauri::command]
pub async fn report_problem_send(app: AppHandle, comment: String) -> Result<(), String> {
    let log = read_log(&app)?;
    let Some(transport) = HttpReportTransport::for_build() else {
        return Err("送信先が分かりません".to_string());
    };
    let app_version = app.package_info().version.to_string();
    let comment = capped_comment(&comment);
    let body = ReportBody {
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        app_version: &app_version,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        comment: &comment,
        log: &log,
    };
    let json = serde_json::to_vec(&body).map_err(|e| format!("送る内容を作れません: {}", e))?;
    if transport.send(json).await {
        Ok(())
    } else {
        Err("送信できませんでした。しばらくしてからもう一度試してください".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_returned_as_is() {
        assert_eq!(tail("short", 100), "short");
    }

    /// Verifies: REQ-RPT-003
    #[test]
    fn long_text_keeps_the_end_and_marks_the_cut() {
        let text = "a".repeat(10);

        let result = tail(&text, 4);

        assert!(result.ends_with("aaaa"));
        assert!(result.starts_with("...(先頭を省略"));
        assert!(!result.contains("aaaaaaaaaa"));
    }

    #[test]
    fn the_cut_lands_on_a_char_boundary() {
        // Each character is 3 bytes (UTF-8); a byte cut of 4 would land
        // mid-character without the boundary search.
        let text = "あいうえお";

        let result = tail(text, 4);

        assert!(result.is_char_boundary(result.find('\n').unwrap() + 1));
    }

    #[test]
    fn comment_within_the_cap_is_unchanged() {
        assert_eq!(capped_comment("hello"), "hello");
    }

    /// Verifies: REQ-RPT-004
    #[test]
    fn comment_over_the_cap_is_cut_by_unicode_scalar_count_not_bytes() {
        let comment = "あ".repeat(MAX_COMMENT_CHARS + 10);

        let result = capped_comment(&comment);

        assert_eq!(result.chars().count(), MAX_COMMENT_CHARS);
    }

    /// Verifies: REQ-RPT-003
    #[test]
    fn masking_runs_before_the_tail_cap_so_a_secret_within_the_kept_window_is_still_masked() {
        // The secret sits inside the last 15 characters kept by `tail`, so
        // this only stays hidden if `redact_secrets` ran first.
        let raw = format!("{}token: shh-secret", "x".repeat(20));

        let sent = tail(&redact_secrets(&raw), 15);

        assert!(sent.contains("***"));
        assert!(!sent.contains("shh-secret"));
    }

    struct RecordingTransport {
        sent: std::sync::Mutex<Vec<Vec<u8>>>,
        accept: bool,
    }

    impl ReportTransport for RecordingTransport {
        fn send(&self, body: Vec<u8>) -> Delivery<'_> {
            self.sent.lock().unwrap().push(body);
            let accept = self.accept;
            Box::pin(async move { accept })
        }
    }

    #[tokio::test]
    async fn the_body_sent_carries_the_capped_comment_and_the_given_log() {
        let transport = RecordingTransport {
            sent: std::sync::Mutex::new(Vec::new()),
            accept: true,
        };
        let long_comment = "x".repeat(MAX_COMMENT_CHARS + 50);
        let body = ReportBody {
            ts: "2026-09-27T00:00:00Z".to_string(),
            app_version: "0.1.0",
            os: "linux",
            arch: "x86_64",
            comment: &capped_comment(&long_comment),
            log: "line one\nline two",
        };
        let json = serde_json::to_vec(&body).unwrap();

        assert!(transport.send(json.clone()).await);

        let sent = transport.sent.lock().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&sent[0]).unwrap();
        assert_eq!(
            value["comment"].as_str().unwrap().chars().count(),
            MAX_COMMENT_CHARS
        );
        assert_eq!(value["log"], "line one\nline two");
        assert_eq!(value["app_version"], "0.1.0");
    }

    #[tokio::test]
    async fn a_transport_that_refuses_is_reported_as_such() {
        let transport = RecordingTransport {
            sent: std::sync::Mutex::new(Vec::new()),
            accept: false,
        };

        assert!(!transport.send(b"{}".to_vec()).await);
    }
}
