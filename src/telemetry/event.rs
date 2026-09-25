//! The lines of the usage log: their envelope and the six events.
//!
//! `schema.json` is the definition of what may be sent. These types are what
//! the app builds and serializes; a test checks that every line they produce
//! satisfies the schema, so the two cannot drift apart.

use std::net::IpAddr;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

pub use crate::network::LinkRoute;

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
    /// Whether the app talks to the jamjam server it was built for, rather
    /// than one the user set in `server_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_is_default: Option<bool>,
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
///
/// `input_id` and `output_id` are the IDs the user chose, as the settings file
/// holds them: absent for a side left on the OS default. They are here rather
/// than in the settings so that they are sent with the device they name, also
/// when that device could not be found.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AudioEnv {
    pub input: Option<Device>,
    pub output: Option<Device>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_id: Option<String>,
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
/// has no FEC, no audio ever arrived) is left out of the line, not written as
/// `0`.
///
/// `route` and the times describe the last link the session brought up. The
/// peer's address is never sent: a peer who has not turned reporting on has not
/// agreed to that. The user's own addresses are.
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
    /// The kind of address the audio went to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<LinkRoute>,
    /// Whether that address was picked because it answered a probe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_confirmed: Option<bool>,
    /// Milliseconds from starting to connect to the link going up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<u64>,
    /// Milliseconds from the link going up to the first audio packet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_audio_ms: Option<u64>,
    /// The user's own addresses on their networks that were offered to the peer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub local_ips: Vec<IpAddr>,
    /// The user's public address as a STUN server saw it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<IpAddr>,
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
    /// The peer stopped sending and the link was given up.
    NoPackets,
    /// The signaling server answered with a 4xx status.
    #[serde(rename = "http_4xx")]
    Http4xx,
    /// The signaling server answered with a 5xx status.
    #[serde(rename = "http_5xx")]
    Http5xx,
    Tls,
    Dns,
    /// The signaling connection was closed by the other end.
    WsClosed,
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
