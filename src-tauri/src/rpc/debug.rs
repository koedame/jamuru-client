//! `debug.*`: what someone verifying a running app needs (ADR-044).
//!
//! Only builds with `debug-tools` (E2E and beta) contain this module, and only
//! the two portals that exist to test and debug the app may call it.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::spec::{Access, Kind, Method};
use super::{Call, Code, RpcError};
use crate::config::ConfigState;
use crate::device_identity::DeviceIdentityState;
use crate::logging::strip_userinfo;

/// The log file's name, as `logging` writes it.
const LOG_FILE: &str = "jamjam.log";

/// The crash record's name in the usage state directory.
const CRASH_FILE: &str = "crash.json";

/// Bytes of log returned when the caller names no amount.
const DEFAULT_LOG_BYTES: u64 = 64 * 1024;

/// The most log one answer carries. Well under a frame even after the JSON
/// escaping of a log full of quotes and newlines.
const MAX_LOG_BYTES: u64 = 256 * 1024;

/// How many panic lines from the log `debug.crashes` returns.
const MAX_PANIC_LINES: usize = 20;

/// Time given to the answer to get out before the app restarts.
const RESTART_DELAY: Duration = Duration::from_millis(500);

pub const METHODS: &[Method] = &[
    Method {
        name: "debug.info",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "版・ビルド・OS・音声デバイスと設定・接続先・ログの場所",
    },
    Method {
        name: "debug.logs",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "診断ログの末尾、または指定した位置からの続き",
    },
    Method {
        name: "debug.crashes",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "クラッシュの記録と、ログに残った panic の行",
    },
    Method {
        name: "debug.restart",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "アプリを再起動する",
    },
    Method {
        name: "debug.update_apply",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "新しい版があれば、セッション中でも待たずに入れて再起動する",
    },
];

/// Runs the `debug.*` method `name`.
pub async fn call<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    call: Call,
) -> Result<Value, RpcError> {
    match name {
        "debug.info" => info(app).await,
        "debug.logs" => logs(app, params(&call)?),
        "debug.crashes" => crashes(app),
        "debug.restart" => Ok(restart(app)),
        "debug.update_apply" => update_apply(app).await,
        other => Err(RpcError::new(
            Code::UnknownMethod,
            format!("no method named {:?}", other),
        )),
    }
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

fn log_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, RpcError> {
    app.path()
        .app_log_dir()
        .map(|dir| dir.join(LOG_FILE))
        .map_err(|e| RpcError::failed(format!("the log directory is unknown: {}", e)))
}

async fn info<R: Runtime>(app: &AppHandle<R>) -> Result<Value, RpcError> {
    let audio = crate::settings::current(app)
        .await
        .map_err(|e| RpcError::failed(format!("the audio settings could not be read: {:?}", e)))?;
    let server_url = app.state::<ConfigState>().server_url();
    Ok(json!({
        "app_version": app.package_info().version.to_string(),
        "build": if cfg!(feature = "e2e-control") { "e2e" } else { "beta" },
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "os_version": jamjam::environment::os_version(),
        "ram_gb": jamjam::environment::ram_gb(),
        "audio_host": jamjam::environment::audio_host(),
        "device_id": app.state::<DeviceIdentityState>().identity().device_id(),
        "server_url": strip_userinfo(&server_url),
        "log_file": log_path(app).ok().map(|p| p.display().to_string()),
        "audio": audio,
    }))
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    /// Where to start reading, in bytes from the start of the file. Absent
    /// means the tail, [`DEFAULT_LOG_BYTES`] back from the end.
    #[serde(default)]
    offset: Option<u64>,
    /// How much to read, at most [`MAX_LOG_BYTES`].
    #[serde(default)]
    bytes: Option<u64>,
}

/// The log from `offset`, or its tail. `next_offset` is where to continue from,
/// so a caller can follow the file across calls; a file that shrank (rotated)
/// answers from its start.
fn logs<R: Runtime>(app: &AppHandle<R>, params: LogsParams) -> Result<Value, RpcError> {
    let path = log_path(app)?;
    let mut file = std::fs::File::open(&path)
        .map_err(|e| RpcError::failed(format!("the log file could not be opened: {}", e)))?;
    let size = file
        .metadata()
        .map_err(|e| RpcError::failed(e.to_string()))?
        .len();
    let want = params.bytes.unwrap_or(DEFAULT_LOG_BYTES).min(MAX_LOG_BYTES);
    let offset = match params.offset {
        Some(offset) if offset <= size => offset,
        Some(_) => 0,
        None => size.saturating_sub(want),
    };
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| RpcError::failed(e.to_string()))?;
    let mut buffer = Vec::new();
    file.take(want)
        .read_to_end(&mut buffer)
        .map_err(|e| RpcError::failed(e.to_string()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "size": size,
        "offset": offset,
        "next_offset": offset + buffer.len() as u64,
        "text": String::from_utf8_lossy(&buffer),
    }))
}

/// The crash record the app keeps for its next launch (only while usage
/// reporting is on, and gone once sent), and the panics the log holds - the
/// log records every panic, whatever the setting.
fn crashes<R: Runtime>(app: &AppHandle<R>) -> Result<Value, RpcError> {
    let record = jamjam::telemetry::state_dir()
        .map(|dir| dir.join(CRASH_FILE))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let log = std::fs::read_to_string(log_path(app)?).unwrap_or_default();
    let panics = panic_lines(&log);
    Ok(json!({ "record": record, "panics_in_log": panics }))
}

/// The last [`MAX_PANIC_LINES`] lines that report a panic.
fn panic_lines(log: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("panic in thread"))
        .collect();
    let excess = lines.len().saturating_sub(MAX_PANIC_LINES);
    lines.drain(..excess);
    lines
}

fn restart<R: Runtime>(app: &AppHandle<R>) -> Value {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(RESTART_DELAY).await;
        app.restart();
    });
    json!({ "restarting": true })
}

async fn update_apply<R: Runtime>(app: &AppHandle<R>) -> Result<Value, RpcError> {
    let installed = crate::updater::install_now(app)
        .await
        .map_err(|e| RpcError::failed(format!("the update did not finish: {}", e)))?;
    match installed {
        None => Ok(json!({ "up_to_date": true })),
        Some(version) => {
            restart(app);
            Ok(json!({ "installed": version, "restarting": true }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panic_lines_are_the_last_ones_that_report_a_panic() {
        let log = (0..30)
            .map(|i| {
                format!(
                    "ERROR panic in thread 'main' number {}\nINFO ordinary line",
                    i
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let lines = panic_lines(&log);

        assert_eq!(lines.len(), MAX_PANIC_LINES);
        assert!(lines[0].ends_with("number 10"));
        assert!(lines[MAX_PANIC_LINES - 1].ends_with("number 29"));
    }

    #[test]
    fn a_log_without_a_panic_has_no_panic_lines() {
        assert!(panic_lines("INFO all well\nWARN a warning").is_empty());
    }
}
