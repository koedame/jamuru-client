//! Usage reporting: what the app tells the jamjam server about how it runs,
//! when the user has turned "usage reporting" on.
//!
//! - **Off by default.** While it is off nothing is collected or sent, and
//!   there is no install ID.
//! - **What is sent** is defined by `schema.json`: six events (`app_start`,
//!   `audio_env`, `session_start`, `session_end`, `error`, `crash`), each a
//!   line of JSON with the same envelope. Totals only; no audio, no chat, no
//!   room names or codes, no addresses, no paths, no error messages.
//! - **Settings** are sent whole except the items in [`settings::LEFT_OUT`].
//! - **Device names** are sent as the OS reports them.
//! - **How it is sent**: in batches of at most 64 KB and 200 lines, at
//!   launch and when a session ends; errors ride along with the next send. A
//!   batch that does not arrive is dropped.
//! - **What would be sent** can be read at any time with
//!   [`UsageReporter::preview_ndjson`].
//!
//! This is a separate path from the diagnostic log file `jamjam.log`
//! (ADR-036), which stays on the machine. Nothing here reuses its redaction:
//! what may leave the machine is decided by the schema and by
//! [`settings::LEFT_OUT`].

mod collector;
mod crash;
mod event;
mod install;
mod session;
pub mod settings;
pub mod snapshot;
mod transport;

pub use collector::{UsageReporter, MAX_BATCH_BYTES, MAX_BATCH_LINES};
pub use event::{
    format_ts, AppStart, AudioEnv, AudioHost, Component, Crash, Device, DeviceKind, EndReason,
    ErrorCode, ErrorEvent, EventBody, Record, SessionEnd, SessionMode, SessionStart,
    SCHEMA_VERSION,
};
pub use install::state_dir;
pub use session::SessionTally;
pub use transport::{
    Delivery, HttpTransport, NoTransport, Transport, NDJSON_CONTENT_TYPE, USAGE_LOGS_PATH,
};

/// The JSON Schema every line satisfies. The server checks lines against the
/// same file.
pub const SCHEMA_JSON: &str = include_str!("schema.json");

#[cfg(test)]
mod tests;
