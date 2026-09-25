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
use crate::audio_tap::{self, Point, MAX_RECORD_SECONDS};
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

/// The longest tone. It ends by itself.
const MAX_TONE_SECONDS: f32 = 60.0;

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
    Method {
        name: "debug.screenshot",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "アプリの画面の PNG（`window` でウィンドウを選べる。いまは Linux だけ）",
    },
    Method {
        name: "debug.audio_record",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "入力・送信直前・出力のいずれかを指定時間だけ録音し、最大値・RMS・支配的な周波数・途切れを返す",
    },
    Method {
        name: "debug.audio_tone",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "送信直前または出力に、指定の周波数・振幅・時間の正弦波を入れる",
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
        "debug.screenshot" => screenshot(app, params(&call)?).await,
        "debug.audio_record" => audio_record(params(&call)?).await,
        "debug.audio_tone" => audio_tone(params(&call)?),
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

#[derive(Debug, Default, Deserialize)]
struct ScreenshotParams {
    /// The window's label; the main window when absent.
    #[serde(default)]
    window: Option<String>,
}

/// The window as the user sees it, as a PNG. WebKitGTK can snapshot itself;
/// the other webviews need their own calls, which are not written yet.
#[cfg(target_os = "linux")]
async fn screenshot<R: Runtime>(
    app: &AppHandle<R>,
    params: ScreenshotParams,
) -> Result<Value, RpcError> {
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    let label = params
        .window
        .as_deref()
        .unwrap_or(super::webview::MAIN_WINDOW);
    let window = app.get_webview_window(label).ok_or_else(|| {
        RpcError::new(
            Code::NoWindow,
            format!("the window {:?} is not open", label),
        )
    })?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            webview.inner().snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                None::<&webkit2gtk::gio::Cancellable>,
                move |snapshot| {
                    let _ = tx.send(snapshot.map_err(|e| e.to_string()).and_then(png_of));
                },
            );
        })
        .map_err(|e| RpcError::failed(format!("the webview could not be reached: {}", e)))?;
    let (width, height, png) = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .map_err(|_| RpcError::new(Code::Timeout, "the webview did not answer the snapshot"))?
        .map_err(|_| RpcError::failed("the snapshot was dropped"))?
        .map_err(|e| RpcError::failed(format!("the snapshot failed: {}", e)))?;
    Ok(json!({
        "width": width,
        "height": height,
        "png_base64": data_encoding::BASE64.encode(&png),
    }))
}

#[cfg(target_os = "linux")]
fn png_of(surface: cairo::Surface) -> Result<(i32, i32, Vec<u8>), String> {
    let image = cairo::ImageSurface::try_from(surface)
        .map_err(|_| "the snapshot is not an image".to_string())?;
    let mut png = Vec::new();
    image.write_to_png(&mut png).map_err(|e| e.to_string())?;
    Ok((image.width(), image.height(), png))
}

#[cfg(not(target_os = "linux"))]
async fn screenshot<R: Runtime>(
    _app: &AppHandle<R>,
    _params: ScreenshotParams,
) -> Result<Value, RpcError> {
    Err(RpcError::failed(
        "screenshots are only taken on Linux so far",
    ))
}

#[derive(Debug, Deserialize)]
struct RecordParams {
    /// `input`, `sent` or `output`.
    point: String,
    /// How long to record, up to [`MAX_RECORD_SECONDS`].
    seconds: f32,
    /// Also return the recording as a base64 WAV, if it fits a frame.
    #[serde(default)]
    wav: bool,
}

async fn audio_record(params: RecordParams) -> Result<Value, RpcError> {
    let point = Point::parse(&params.point).ok_or_else(|| {
        RpcError::invalid_params("point is one of \"input\", \"sent\" and \"output\"")
    })?;
    if !(params.seconds > 0.0 && params.seconds <= MAX_RECORD_SECONDS) {
        return Err(RpcError::invalid_params(format!(
            "seconds is more than 0 and at most {}",
            MAX_RECORD_SECONDS
        )));
    }
    audio_tap::record(point, params.seconds, params.wav)
        .await
        .map_err(RpcError::failed)
}

#[derive(Debug, Deserialize)]
struct ToneParams {
    /// `sent` or `output`.
    target: String,
    frequency_hz: f32,
    /// 0 to 1.
    amplitude: f32,
    seconds: f32,
}

fn audio_tone(params: ToneParams) -> Result<Value, RpcError> {
    let point = match Point::parse(&params.target) {
        Some(point @ (Point::Sent | Point::Output)) => point,
        _ => {
            return Err(RpcError::invalid_params(
                "target is one of \"sent\" and \"output\"",
            ))
        }
    };
    if !(20.0..=20_000.0).contains(&params.frequency_hz) {
        return Err(RpcError::invalid_params("frequency_hz is 20 to 20000"));
    }
    if !(params.amplitude > 0.0 && params.amplitude <= 1.0) {
        return Err(RpcError::invalid_params(
            "amplitude is more than 0 and at most 1",
        ));
    }
    if !(params.seconds > 0.0 && params.seconds <= MAX_TONE_SECONDS) {
        return Err(RpcError::invalid_params(format!(
            "seconds is more than 0 and at most {}",
            MAX_TONE_SECONDS
        )));
    }
    audio_tap::arm_tone(point, params.frequency_hz, params.amplitude, params.seconds);
    Ok(json!({ "armed": true, "ends_in_seconds": params.seconds }))
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
