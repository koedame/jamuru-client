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

/// The shortest and longest `debug.perf` window, and the one used when the
/// caller names none. Under a second the CPU reading is not yet meaningful.
const MIN_PERF_SECONDS: f32 = 1.0;
const MAX_PERF_SECONDS: f32 = 60.0;
const DEFAULT_PERF_SECONDS: f32 = 3.0;

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
        name: "debug.perf",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "指定の秒数のあいだ、アプリの CPU・メモリ、音声コールバックと受信ループの所要時間（平均・最大）、xrun の数を測って返す",
    },
    Method {
        name: "debug.audio_timing",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "受信した音の欠けを種類ごとに数え、直近の欠けの時刻（マイクロ秒）と、読み出し・書き込みの数を返す",
    },
    Method {
        name: "debug.audio_tone",
        access: Access::TOOLS,
        kind: Kind::Native,
        summary: "送信直前または出力に、指定の周波数・振幅・時間の正弦波を入れる。channel（1 = 左、2 = 右）を付けるとその側だけに入れ、もう一方は無音にする",
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
        "debug.perf" => perf(params(&call)?).await,
        "debug.audio_tone" => audio_tone(params(&call)?),
        "debug.audio_timing" => audio_timing(app),
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
    let (cpu_model, cpu_threads) = jamjam::perf::cpu_description();
    Ok(json!({
        "app_version": app.package_info().version.to_string(),
        "build": if cfg!(feature = "e2e-control") { "e2e" } else { "beta" },
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "os_version": jamjam::environment::os_version(),
        "ram_gb": jamjam::environment::ram_gb(),
        "cpu_model": cpu_model,
        "cpu_threads": cpu_threads,
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
/// WebView2 (Windows) needs its own call, which is not written yet.
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

/// The window as the user sees it, as a PNG. WKWebView takes its own snapshot
/// (the image is the view's size in points).
#[cfg(target_os = "macos")]
async fn screenshot<R: Runtime>(
    app: &AppHandle<R>,
    params: ScreenshotParams,
) -> Result<Value, RpcError> {
    use block2::RcBlock;
    use objc2_app_kit::NSImage;
    use objc2_foundation::{MainThreadMarker, NSError};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

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
            // `with_webview` runs on the main thread, where WKWebView may be used.
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = tx.send(Err("not on the main thread".to_string()));
                return;
            };
            let view = unsafe { &*webview.inner().cast::<WKWebView>() };
            let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
            let tx = std::cell::Cell::new(Some(tx));
            let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                let Some(tx) = tx.take() else { return };
                let result = match unsafe { image.as_ref() } {
                    Some(image) => png_of(image),
                    None => Err(match unsafe { error.as_ref() } {
                        Some(error) => error.localizedDescription().to_string(),
                        None => "WKWebView returned no image".to_string(),
                    }),
                };
                let _ = tx.send(result);
            });
            unsafe {
                view.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &done)
            };
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

#[cfg(target_os = "macos")]
fn png_of(image: &objc2_app_kit::NSImage) -> Result<(i32, i32, Vec<u8>), String> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
    use objc2_foundation::{NSDictionary, NSString};

    let tiff = image
        .TIFFRepresentation()
        .ok_or("the snapshot has no bitmap")?;
    let bitmap = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("the snapshot is not an image")?;
    let properties: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }
    .ok_or("the snapshot could not be written as a PNG")?;
    Ok((
        bitmap.pixelsWide() as i32,
        bitmap.pixelsHigh() as i32,
        png.to_vec(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn screenshot<R: Runtime>(
    _app: &AppHandle<R>,
    _params: ScreenshotParams,
) -> Result<Value, RpcError> {
    Err(RpcError::failed(
        "screenshots are only taken on Linux and macOS so far",
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

#[derive(Debug, Default, Deserialize)]
struct PerfParams {
    /// How long to measure, up to [`MAX_PERF_SECONDS`]. [`DEFAULT_PERF_SECONDS`] when absent.
    #[serde(default)]
    seconds: Option<f32>,
}

/// What the app cost the machine over `seconds`. Measure while a call is
/// running, or the audio numbers are zero.
async fn perf(params: PerfParams) -> Result<Value, RpcError> {
    let seconds = params.seconds.unwrap_or(DEFAULT_PERF_SECONDS);
    if !(MIN_PERF_SECONDS..=MAX_PERF_SECONDS).contains(&seconds) {
        return Err(RpcError::invalid_params(format!(
            "seconds is {} to {}",
            MIN_PERF_SECONDS, MAX_PERF_SECONDS
        )));
    }
    let sampler = jamjam::perf::ProcessSampler::start().map_err(RpcError::failed)?;
    let window = jamjam::perf::Window::start();
    tokio::time::sleep(Duration::from_secs_f32(seconds)).await;
    let report = window.finish();
    let process = sampler.finish().map_err(RpcError::failed)?;
    Ok(json!({ "seconds": seconds, "process": process, "audio": report }))
}

#[derive(Debug, Deserialize)]
struct ToneParams {
    /// `sent` or `output`.
    target: String,
    frequency_hz: f32,
    /// 0 to 1.
    amplitude: f32,
    /// The channel to put the tone on, 1 for the left and 2 for the right,
    /// the other staying silent. Without it the tone is on both.
    channel: Option<u32>,
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
    if params
        .channel
        .is_some_and(|channel| !(1..=2).contains(&channel))
    {
        return Err(RpcError::invalid_params(
            "channel is 1 (left) or 2 (right), or left out for both",
        ));
    }
    if !(params.seconds > 0.0 && params.seconds <= MAX_TONE_SECONDS) {
        return Err(RpcError::invalid_params(format!(
            "seconds is more than 0 and at most {}",
            MAX_TONE_SECONDS
        )));
    }
    audio_tap::arm_tone(
        point,
        params.frequency_hz,
        params.amplitude,
        params.channel,
        params.seconds,
    );
    Ok(json!({ "armed": true, "ends_in_seconds": params.seconds }))
}

/// What went wrong with the received audio in this session, or the last one:
/// the count of each kind, and when the latest of them happened, in
/// microseconds since the session's audio started. Compare the times of
/// `starved` with those of `late_arrival` and `thread_stall` to tell a link
/// that delivered late from a thread that was away; `reads` over `writes`
/// says whether the device asks for frames faster than the peer sends them.
fn audio_timing<R: Runtime>(app: &AppHandle<R>) -> Result<Value, RpcError> {
    let report = app
        .state::<crate::streaming::StreamingState>()
        .flight_report()
        .ok_or_else(|| RpcError::failed("no session has started audio yet"))?;
    Ok(timing_json(&report))
}

fn timing_json(report: &jamjam::audio::FlightReport) -> Value {
    let counts: serde_json::Map<String, Value> = report
        .counts
        .iter()
        .map(|(kind, count)| (kind.name().to_string(), json!(count)))
        .collect();
    let events: Vec<Value> = report
        .events
        .iter()
        .map(|event| json!([event.kind.name(), event.at_us, event.value_us]))
        .collect();
    json!({
        "now_us": report.now_us,
        "reads": report.reads,
        "writes": report.writes,
        "counts": counts,
        "event_fields": ["kind", "at_us", "value_us"],
        "events": events,
    })
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

    /// Verifies: REQ-RMT-027
    #[test]
    fn the_audio_timing_names_each_kind_and_lists_events_with_their_time() {
        let recorder = jamjam::audio::FlightRecorder::new();
        recorder.count_read();
        recorder.count_write();
        recorder.note(jamjam::audio::FlightKind::Starved, 0);
        recorder.note(jamjam::audio::FlightKind::LateArrival, 3_500);

        let value = timing_json(&recorder.report());

        assert_eq!(value["reads"], 1);
        assert_eq!(value["writes"], 1);
        assert_eq!(value["counts"]["starved"], 1);
        assert_eq!(value["counts"]["late_arrival"], 1);
        assert_eq!(value["counts"]["thread_stall"], 0);
        let events = value["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1][0], "late_arrival");
        assert_eq!(events[1][2], 3_500);
        assert_eq!(value["event_fields"], json!(["kind", "at_us", "value_us"]));
    }
}
