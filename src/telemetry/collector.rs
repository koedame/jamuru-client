//! The reporter: collects events while the user has usage reporting on, and
//! sends them in small batches.
//!
//! While it is off it does nothing at all: no event is kept, nothing is
//! sent, and there is no install ID. Turning it off throws away what was
//! kept and the ID.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::Utc;

use super::crash;
use super::event::{
    format_ts, Component, EndReason, ErrorCode, ErrorEvent, EventBody, Hang, HangStage, Record,
    SessionMode, SessionStart, SCHEMA_VERSION,
};
use super::hang;
use super::install::{create_install_id, discard, load_install_id, random_id};
use super::session::SessionTally;
use super::transport::Transport;

/// The most lines in one send.
pub const MAX_BATCH_LINES: usize = 200;

/// The most bytes in one send.
pub const MAX_BATCH_BYTES: usize = 64 * 1024;

/// Collects and sends the usage log. Cheap to clone: clones share one
/// reporter.
#[derive(Clone)]
pub struct UsageReporter {
    inner: Arc<Inner>,
}

struct Inner {
    dir: Option<PathBuf>,
    transport: Arc<dyn Transport>,
    app_version: String,
    launch_id: String,
    /// Read by the panic hook, which cannot take the lock.
    enabled: AtomicBool,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    install_id: Option<String>,
    session: Option<OpenSession>,
    seq: u64,
    /// Events waiting for the next send.
    pending: Vec<Record>,
    /// The last batch handed to the transport, kept so the preview has
    /// something to show once the events have gone out.
    last_batch: String,
}

struct OpenSession {
    id: String,
    tally: SessionTally,
}

impl UsageReporter {
    /// A reporter that keeps its install ID and crash record in `dir`.
    ///
    /// `enabled` is the user's `usage_reporting` setting. A reporter with no
    /// `dir`, or one whose install ID cannot be stored, stays off.
    pub fn new(
        dir: Option<PathBuf>,
        app_version: &str,
        transport: Arc<dyn Transport>,
        enabled: bool,
    ) -> Self {
        let reporter = Self {
            inner: Arc::new(Inner {
                dir,
                transport,
                app_version: app_version.to_string(),
                launch_id: random_id(),
                enabled: AtomicBool::new(false),
                state: Mutex::new(State::default()),
            }),
        };
        reporter.set_enabled(enabled);
        reporter
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::SeqCst)
    }

    /// Where the install ID and the pending crash/hang records are kept -
    /// `None` when there is nowhere to keep them, in which case nothing is
    /// collected. Exposed so the app layer can keep its own small local
    /// records next to these (the watchdog's `running.json`), without a
    /// second, possibly different, notion of where "the usage state dir" is.
    pub fn state_dir(&self) -> Option<&Path> {
        self.inner.dir.as_deref()
    }

    /// This launch's random ID, made once at construction. Exposed so the
    /// watchdog's own local record (`running.json`) can be tied to the same
    /// launch as `write_hang`/`previous_hang` without keeping a second copy.
    pub fn launch_id(&self) -> &str {
        &self.inner.launch_id
    }

    /// This launch's app version. Same reason as `launch_id`.
    pub fn app_version(&self) -> &str {
        &self.inner.app_version
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Follows the user's setting.
    ///
    /// Turning on makes an install ID (a stored one is kept when the app was
    /// already on at the last launch). Turning off throws away what has not
    /// been sent, the open session and the install ID, and removes a crash
    /// record that has not been sent.
    pub fn set_enabled(&self, on: bool) {
        let mut state = self.state();
        if on {
            if self.is_enabled() {
                return;
            }
            let Some(dir) = self.inner.dir.as_deref() else {
                return;
            };
            let id = match load_install_id(dir) {
                Some(id) => Ok(id),
                None => create_install_id(dir),
            };
            match id {
                Ok(id) => {
                    state.install_id = Some(id);
                    self.inner.enabled.store(true, Ordering::SeqCst);
                }
                Err(e) => tracing::warn!("Usage reporting stays off: {}", e),
            }
        } else {
            self.inner.enabled.store(false, Ordering::SeqCst);
            *state = State::default();
            if let Some(dir) = self.inner.dir.as_deref() {
                discard(dir);
            }
        }
    }

    /// Records an event, unless reporting is off.
    pub fn record(&self, body: EventBody) {
        self.push(body, None);
    }

    /// `launch_id` and `app_version` of another launch: for the crash of the
    /// launch before this one.
    fn push(&self, body: EventBody, from_launch: Option<(&str, &str)>) {
        if !self.is_enabled() {
            return;
        }
        let mut state = self.state();
        let Some(install_id) = state.install_id.clone() else {
            return;
        };
        let (launch_id, app_version) =
            from_launch.unwrap_or((self.launch_id(), self.app_version()));
        let record = Record {
            v: SCHEMA_VERSION,
            ts: format_ts(Utc::now()),
            seq: state.seq,
            install_id,
            launch_id: launch_id.to_string(),
            session_id: state.session.as_ref().map(|s| s.id.clone()),
            app_version: app_version.to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            body,
        };
        state.seq += 1;
        state.pending.push(record);
        // Whatever cannot go out in one send is dropped, so there is no use
        // in holding more than one send's worth: the oldest goes first.
        let excess = state.pending.len().saturating_sub(MAX_BATCH_LINES);
        state.pending.drain(..excess);
    }

    /// Records an error. The same error in the same session before the next
    /// send is counted, not repeated.
    pub fn record_error(&self, component: Component, code: ErrorCode) {
        if !self.is_enabled() {
            return;
        }
        {
            let mut state = self.state();
            let session_id = state.session.as_ref().map(|s| s.id.clone());
            let same = state.pending.iter_mut().find(|record| {
                record.session_id == session_id
                    && matches!(&record.body,
                        EventBody::Error(e) if e.component == component && e.code == code)
            });
            if let Some(record) = same {
                if let EventBody::Error(e) = &mut record.body {
                    e.count = e.count.saturating_add(1);
                }
                return;
            }
        }
        self.push(
            EventBody::Error(ErrorEvent {
                component,
                code,
                count: 1,
            }),
            None,
        );
    }

    /// Opens a session and records `session_start`, returning the session's
    /// ID (`None` while reporting is off). A session still open is closed
    /// first, as `disconnected`.
    pub fn begin_session(&self, mode: SessionMode) -> Option<String> {
        if !self.is_enabled() {
            return None;
        }
        if self.state().session.is_some() {
            self.end_session(EndReason::Disconnected);
        }
        let id = random_id();
        self.state().session = Some(OpenSession {
            id: id.clone(),
            tally: SessionTally::new(),
        });
        self.record(EventBody::SessionStart(SessionStart { mode }));
        Some(id)
    }

    /// The open session's ID, so whatever samples it can tell that the
    /// session it was started for is over.
    pub fn session_id(&self) -> Option<String> {
        self.state().session.as_ref().map(|s| s.id.clone())
    }

    /// Adds a reading to the open session's totals.
    pub fn with_session(&self, update: impl FnOnce(&mut SessionTally)) {
        if let Some(session) = self.state().session.as_mut() {
            update(&mut session.tally);
        }
    }

    /// Closes the open session and records `session_end`. Does nothing when
    /// none is open.
    pub fn end_session(&self, end_reason: EndReason) {
        let Some(end) = self
            .state()
            .session
            .as_ref()
            .map(|s| s.tally.finish(end_reason))
        else {
            return;
        };
        self.record(EventBody::SessionEnd(end));
        self.state().session = None;
    }

    /// Records the crash of the launch before this one, if it left a record,
    /// and removes the record.
    pub fn report_previous_crash(&self) {
        let Some(dir) = self.inner.dir.as_deref() else {
            return;
        };
        if !self.is_enabled() {
            return;
        }
        let Some(saved) = crash::take(dir) else {
            return;
        };
        if let (Some(event), true) = (saved.event(), saved.has_valid_ids()) {
            self.push(
                EventBody::Crash(event),
                Some((&saved.launch_id, &saved.app_version)),
            );
        }
    }

    /// Makes a panic leave a record for the next launch (see `crash`).
    pub fn install_panic_hook(&self) {
        crash::install_hook(self);
    }

    /// Writes `stage`/`stalled_ms` as this launch's hang record, for the
    /// next launch to find. Unlike most of this type, this is not gated on
    /// `is_enabled()`: a watchdog notices a stall before the user has said
    /// anything, and the record is decided on at read time instead (see the
    /// module docs and `hang.rs`).
    pub fn write_hang(&self, stage: HangStage, stalled_ms: Option<u32>) {
        let Some(dir) = self.inner.dir.as_deref() else {
            return;
        };
        hang::write(
            dir,
            &hang::HangFile {
                ts: format_ts(Utc::now()),
                launch_id: self.launch_id().to_string(),
                app_version: self.app_version().to_string(),
                stage,
                stalled_ms,
            },
        );
    }

    /// Takes the hang record left by the launch before this one, if there is
    /// a usable one, along with the `launch_id`/`app_version` it happened
    /// in. Removes the record either way it is used next: folded into what
    /// is sent now (`record_previous_hang`) or offered to the user as a
    /// one-off (`send_one_off_hang`). Not gated on `is_enabled()`: reading
    /// what is on disk is not the same as sending it.
    pub fn previous_hang(&self) -> Option<(Hang, String, String)> {
        let dir = self.inner.dir.as_deref()?;
        let saved = hang::take(dir)?;
        saved.has_valid_ids().then(|| {
            (
                saved.event(),
                saved.launch_id.clone(),
                saved.app_version.clone(),
            )
        })
    }

    /// Queues `hang`, from the launch named by `launch_id`/`app_version`, the
    /// way `report_previous_crash` queues a crash. Does nothing while
    /// reporting is off (`push` already checks).
    pub fn record_previous_hang(&self, hang: Hang, launch_id: &str, app_version: &str) {
        self.push(EventBody::Hang(hang), Some((launch_id, app_version)));
    }

    /// Sends `hang` as one `hang` event on its own, using an install ID made
    /// and discarded just for this send. For the case reporting itself is
    /// off: the user agreed to send this one report, which must not turn
    /// `usage_reporting` on or leave an ID behind afterwards.
    pub async fn send_one_off_hang(&self, hang: Hang, launch_id: &str, app_version: &str) {
        if self.inner.dir.is_none() {
            return;
        }
        let was_enabled = self.is_enabled();
        if !was_enabled {
            self.set_enabled(true);
            if !self.is_enabled() {
                return;
            }
        }
        self.push(EventBody::Hang(hang), Some((launch_id, app_version)));
        self.flush().await;
        if !was_enabled {
            self.set_enabled(false);
        }
    }

    /// Sends what is waiting: at most [`MAX_BATCH_LINES`] lines and
    /// [`MAX_BATCH_BYTES`] bytes in one request. What does not fit, and a
    /// batch the server did not take, are dropped rather than kept for later.
    pub async fn flush(&self) {
        let body = {
            let mut state = self.state();
            if !self.is_enabled() || state.pending.is_empty() {
                return;
            }
            let records = std::mem::take(&mut state.pending);
            let body = batch(&records);
            state.last_batch = body.clone();
            body
        };
        if !body.is_empty() {
            let _ = self.inner.transport.send(body.into_bytes()).await;
        }
    }

    /// The NDJSON the next send will contain, exactly as it will go out. When
    /// nothing is waiting, the last batch that was sent, so there is
    /// something to look at. Empty while reporting is off.
    pub fn preview_ndjson(&self) -> String {
        if !self.is_enabled() {
            return String::new();
        }
        let state = self.state();
        if state.pending.is_empty() {
            state.last_batch.clone()
        } else {
            batch(&state.pending)
        }
    }

    /// How many lines are waiting.
    pub fn pending_count(&self) -> usize {
        self.state().pending.len()
    }
}

/// The lines of `records` as one request body, within the size limits.
fn batch(records: &[Record]) -> String {
    let mut body = String::new();
    let mut lines = 0;
    for record in records {
        let Ok(line) = serde_json::to_string(record) else {
            continue;
        };
        if lines == MAX_BATCH_LINES || body.len() + line.len() + 1 > MAX_BATCH_BYTES {
            break;
        }
        body.push_str(&line);
        body.push('\n');
        lines += 1;
    }
    body
}
