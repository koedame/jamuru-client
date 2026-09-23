//! Usage reporting in the app (`jamjam::telemetry`)
//!
//! Holds the reporter for the app's lifetime and connects it to what the app
//! does: launch, joining and leaving a room, errors, a panic. While the user
//! has "usage reporting" off, every call here does nothing.

use std::sync::Arc;
use std::time::Duration;

use jamjam::config::AppConfig;
use jamjam::telemetry::{
    snapshot, Component, EndReason, ErrorCode, EventBody, HttpTransport, NoTransport, SessionMode,
    Transport, UsageReporter,
};
use tauri::{AppHandle, Manager};

use crate::streaming::StreamingState;

/// How often the open session's link is read for the totals.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// The longest the app waits to send the last events when it is closed.
const EXIT_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// Tauri-managed state: the reporter.
pub struct UsageState {
    reporter: UsageReporter,
}

impl UsageState {
    /// The reporter for this launch, on or off as `config` says.
    pub fn new(config: &AppConfig, app_version: &str) -> Self {
        let transport: Arc<dyn Transport> = match HttpTransport::for_build() {
            Some(http) => Arc::new(http),
            None => Arc::new(NoTransport),
        };
        Self::with_reporter(UsageReporter::new(
            jamjam::telemetry::state_dir(),
            app_version,
            transport,
            config.usage_reporting,
        ))
    }

    pub fn with_reporter(reporter: UsageReporter) -> Self {
        Self { reporter }
    }

    pub fn reporter(&self) -> &UsageReporter {
        &self.reporter
    }

    /// Reports this launch: the crash of the last one if there was one, then
    /// what this machine is like. Sent in the background.
    pub fn report_launch(&self, config: AppConfig) {
        let reporter = self.reporter.clone();
        tauri::async_runtime::spawn(async move {
            reporter.report_previous_crash();
            let mut app_start = snapshot::app_start(&config);
            app_start.webview_version = tauri::webview_version()
                .ok()
                .and_then(|version| major_of(&version));
            reporter.record(EventBody::AppStart(app_start));
            reporter.record(EventBody::AudioEnv(snapshot::audio_env()));
            reporter.flush().await;
        });
    }

    /// Follows a change of the `usage_reporting` setting. Turning it on
    /// reports this launch at once; turning it off throws away what was kept.
    pub fn apply_setting(&self, config: &AppConfig) {
        match (config.usage_reporting, self.reporter.is_enabled()) {
            (true, false) => {
                self.reporter.set_enabled(true);
                if self.reporter.is_enabled() {
                    self.report_launch(config.clone());
                }
            }
            (false, true) => self.reporter.set_enabled(false),
            _ => {}
        }
    }

    /// Sends what is waiting, in the background.
    pub fn flush_in_background(&self) {
        let reporter = self.reporter.clone();
        tauri::async_runtime::spawn(async move { reporter.flush().await });
    }

    /// The user made or joined a room. `participants` counts them too.
    pub fn session_started(&self, app: &AppHandle, mode: SessionMode, participants: u32) {
        let Some(session_id) = self.reporter.begin_session(mode) else {
            return;
        };
        self.reporter
            .with_session(|tally| tally.set_participants(participants));
        spawn_sampler(app.clone(), session_id);
    }

    /// The session ended for `reason`. Its totals go out at once.
    pub fn session_ended(&self, streaming: &StreamingState, reason: EndReason) {
        if self.reporter.session_id().is_none() {
            return;
        }
        read_link(streaming, &self.reporter);
        self.reporter.end_session(reason);
        self.flush_in_background();
    }

    pub fn participant_joined(&self) {
        self.reporter
            .with_session(|tally| tally.participant_joined());
    }

    pub fn participant_left(&self) {
        self.reporter.with_session(|tally| tally.participant_left());
    }

    /// The app is closing: closes an open session and gives the last events
    /// a moment to go out.
    pub fn app_exiting(&self, streaming: &StreamingState) {
        if !self.reporter.is_enabled() {
            return;
        }
        if self.reporter.session_id().is_some() {
            read_link(streaming, &self.reporter);
            self.reporter.end_session(EndReason::AppQuit);
        }
        let reporter = self.reporter.clone();
        tauri::async_runtime::block_on(async move {
            let _ = tokio::time::timeout(EXIT_FLUSH_TIMEOUT, reporter.flush()).await;
        });
    }
}

/// The NDJSON the next send will contain, for the "what is sent" view.
#[tauri::command]
pub fn usage_preview(state: tauri::State<'_, UsageState>) -> String {
    state.reporter.preview_ndjson()
}

/// Reads the streaming totals into the open session: underruns and
/// reconnects since the last reading, and one reading of the link.
///
/// `StreamingState` counts from zero each time streaming starts, so a total
/// that went down means a new start and its whole value is new. (A new start
/// that passes the old total before the next reading is counted short.)
fn read_link(streaming: &StreamingState, reporter: &UsageReporter) {
    let reading = streaming.link_reading();
    let (underruns, reconnects) = (streaming.underruns(), streaming.reconnects());
    reporter.with_session(|tally| {
        if let Some((rtt_ms, loss_rate)) = reading {
            tally.sample(rtt_ms, loss_rate);
        }
        tally.add_xruns_total(underruns);
        tally.add_reconnects_total(reconnects);
    });
}

fn spawn_sampler(app: AppHandle, session_id: String) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(SAMPLE_INTERVAL).await;
            let usage = app.state::<UsageState>();
            if usage.reporter.session_id().as_deref() != Some(session_id.as_str()) {
                return;
            }
            read_link(&app.state::<StreamingState>(), &usage.reporter);
        }
    });
}

/// The major number of a version (`128.0.6613.84` -> `128`), or `None` when it
/// does not start with a short number.
fn major_of(version: &str) -> Option<String> {
    let major = version.split('.').next()?;
    (!major.is_empty() && major.len() <= 4 && major.bytes().all(|b| b.is_ascii_digit()))
        .then(|| major.to_string())
}

/// Records why streaming failed, from the message the failing call returned.
/// Only the kind of failure is kept, and only for messages the app knows.
pub fn record_streaming_failure(reporter: &UsageReporter, message: &str) {
    if let Some((component, code)) = classify_streaming_error(message) {
        reporter.record_error(component, code);
    }
}

/// What a failed streaming start says about where it failed. The wording is
/// the app's own (`streaming.rs`); the message itself is never sent.
fn classify_streaming_error(message: &str) -> Option<(Component, ErrorCode)> {
    const KNOWN: [(&str, Component, ErrorCode); 7] = [
        (
            "Failed to start capture",
            Component::AudioInput,
            ErrorCode::DeviceOpenFailed,
        ),
        (
            "Failed to start playback",
            Component::AudioOutput,
            ErrorCode::DeviceOpenFailed,
        ),
        (
            "Failed to connect",
            Component::Ice,
            ErrorCode::ConnectFailed,
        ),
        (
            "Failed to configure audio encoding",
            Component::Codec,
            ErrorCode::UnsupportedConfig,
        ),
        (
            "Failed to create audio decoder",
            Component::Codec,
            ErrorCode::UnsupportedConfig,
        ),
        (
            "Failed to bind the audio socket",
            Component::Ice,
            ErrorCode::StreamFailed,
        ),
        (
            "Failed to create connection",
            Component::Ice,
            ErrorCode::StreamFailed,
        ),
    ];
    KNOWN
        .iter()
        .find(|(prefix, _, _)| message.starts_with(prefix))
        .map(|(_, component, code)| (*component, *code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_the_webview_version_has_a_major_number_only_the_major_is_kept() {
        assert_eq!(major_of("128.0.6613.84").as_deref(), Some("128"));
        assert_eq!(major_of("2.44.0").as_deref(), Some("2"));
        assert_eq!(major_of("17").as_deref(), Some("17"));
    }

    #[test]
    fn when_the_webview_version_is_not_a_number_nothing_is_kept() {
        assert_eq!(major_of("Edge/128"), None);
        assert_eq!(major_of(""), None);
        assert_eq!(major_of("123456.1"), None);
    }

    #[test]
    fn when_capture_fails_to_start_the_error_is_an_input_device_error() {
        assert_eq!(
            classify_streaming_error("Failed to start capture: device busy at hw:0,0"),
            Some((Component::AudioInput, ErrorCode::DeviceOpenFailed))
        );
    }

    #[test]
    fn when_playback_fails_to_start_the_error_is_an_output_device_error() {
        assert_eq!(
            classify_streaming_error("Failed to start playback: no device"),
            Some((Component::AudioOutput, ErrorCode::DeviceOpenFailed))
        );
    }

    #[test]
    fn when_the_connection_fails_the_error_is_a_connect_failure() {
        assert_eq!(
            classify_streaming_error("Failed to connect: timed out reaching 10.0.0.5:5000"),
            Some((Component::Ice, ErrorCode::ConnectFailed))
        );
    }

    #[test]
    fn when_the_message_is_not_one_the_app_knows_nothing_is_reported() {
        assert_eq!(classify_streaming_error("Invalid address: nonsense"), None);
        assert_eq!(classify_streaming_error(""), None);
    }
}
