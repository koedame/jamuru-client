//! The lines of the usage log: their envelope and the six events.
//!
//! `schema.json` is the definition of what may be sent. These types are what
//! the app builds and serializes; a test checks that every line they produce
//! satisfies the schema, so the two cannot drift apart.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

/// The schema version written to every line (`v`).
pub const SCHEMA_VERSION: u32 = 1;

/// One line of the log: the fields every line has, then the event's own.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Record {
    pub v: u32,
    /// UTC, ISO 8601, whole seconds.
    pub ts: String,
    /// Counter within one launch.
    pub seq: u64,
    pub install_id: String,
    pub launch_id: String,
    /// `None` outside a session, written as `null`.
    pub session_id: Option<String>,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    #[serde(flatten)]
    pub body: EventBody,
}

/// What happened. The variant name is the line's `event`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventBody {
    AppStart(AppStart),
    AudioEnv(AudioEnv),
    SessionStart(SessionStart),
    SessionEnd(SessionEnd),
    Error(ErrorEvent),
    Crash(Crash),
}

/// Formats a time the way `ts` is written.
pub fn format_ts(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// What the machine is, once per launch.
///
/// A value the app could not find out is left out of the line rather than
/// written as a placeholder: absent means "not known".
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct AppStart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_gb: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webview_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_host: Option<AudioHost>,
    /// The settings file's items, minus the ones [`super::settings`] leaves out.
    pub settings: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioHost {
    Coreaudio,
    Wasapi,
    Asio,
    Alsa,
    Jack,
    Other,
}

/// The input and output device in use, `None` when there is none.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioEnv {
    pub input: Option<Device>,
    pub output: Option<Device>,
}

/// One audio device.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Device {
    /// Exactly what the OS reports.
    pub name: String,
    pub kind: DeviceKind,
    pub channels: u32,
    pub sample_rates: Vec<u32>,
    /// Smallest buffer the device accepts; `null` when it does not say.
    pub min_buffer_frames: Option<u32>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Usb,
    Builtin,
    Bluetooth,
    Virtual,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Create,
    Join,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct SessionStart {
    pub mode: SessionMode,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The user left.
    Left,
    /// The connection was lost and did not come back.
    Disconnected,
    Error,
    /// The app was closed while the session was open.
    AppQuit,
}

/// How a session went, in totals. Nothing per packet.
///
/// A figure that was never measured (no round-trip sample arrived, the link
/// has no FEC) is left out of the line, not written as `0`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionEnd {
    pub duration_s: u64,
    pub end_reason: EndReason,
    pub reconnect_count: u32,
    pub peers_max: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms_p50: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms_p95: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss_pct_mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss_pct_max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fec_active_pct: Option<f64>,
    pub xrun_count: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Signaling,
    Ice,
    AudioInput,
    AudioOutput,
    Codec,
    Config,
}

/// What went wrong, as a fixed word. The error's message is never sent: it
/// can carry an address, a path or a room code.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ConnectFailed,
    Timeout,
    Disconnected,
    HandshakeFailed,
    DeviceNotFound,
    DeviceOpenFailed,
    UnsupportedConfig,
    StreamFailed,
    EncodeFailed,
    DecodeFailed,
    InvalidValue,
    Other,
}

/// `code` happened `count` times in `component`.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct ErrorEvent {
    pub component: Component,
    pub code: ErrorCode,
    pub count: u32,
}

/// Where a panic happened. Never its message.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Crash {
    pub file: String,
    pub line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}
