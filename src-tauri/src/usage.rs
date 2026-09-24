//! Usage reporting in the app (`jamjam::telemetry`)
//!
//! Holds the reporter for the app's lifetime and connects it to what the app
//! does: launch, a change of settings or audio device, joining and leaving a
//! room, errors, a panic. While the user has "usage reporting" off, every call
//! here does nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jamjam::config::AppConfig;
use jamjam::telemetry::{
    snapshot, AppStart, AudioEnv, Component, EndReason, ErrorCode, EventBody, HttpTransport,
    NoTransport, SessionMode, Transport, UsageReporter,
};
use tauri::{AppHandle, Manager, Runtime};

use crate::streaming::StreamingState;

/// How often the open session's link is read for the totals.
#[cfg(not(test))]
const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(test)]
const SAMPLE_INTERVAL: Duration = Duration::from_millis(20);

/// How long a change of settings or devices must stay quiet before it is
/// reported, so a run of changes goes out as its last state.
#[cfg(not(test))]
const CHANGE_SETTLE: Duration = Duration::from_secs(2);
#[cfg(test)]
const CHANGE_SETTLE: Duration = Duration::from_millis(50);

/// The longest the app waits to send the last events when it is closed.
const EXIT_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// Tauri-managed state: the reporter.
#[derive(Clone)]
pub struct UsageState {
    reporter: UsageReporter,
    changes: Arc<Changes>,
}

/// What was last reported about the machine's settings and audio devices,
/// and the count of changes seen so far. A change is reported only when its
/// state differs from the last one reported, and only if no newer change
/// arrived while it settled.
#[derive(Default)]
struct Changes {
    settings_seen: AtomicU64,
    devices_seen: AtomicU64,
    reported_app_start: Mutex<Option<AppStart>>,
    reported_audio_env: Mutex<Option<AudioEnv>>,
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
        Self {
            reporter,
            changes: Arc::default(),
        }
    }

    pub fn reporter(&self) -> &UsageReporter {
        &self.reporter
    }

    /// Reports this launch: the crash of the last one if there was one, then
    /// what this machine is like. Sent in the background.
    pub fn report_launch(&self, config: AppConfig) {
        let reporter = self.reporter.clone();
        let changes = self.changes.clone();
        tauri::async_runtime::spawn(async move {
            reporter.report_previous_crash();
            let app_start = app_start_of(&config);
            let audio_env = audio_env_of(&config);
            *changes.reported_app_start.lock().unwrap() = Some(app_start.clone());
            *changes.reported_audio_env.lock().unwrap() = Some(audio_env.clone());
            reporter.record(EventBody::AppStart(app_start));
            reporter.record(EventBody::AudioEnv(audio_env));
            reporter.flush().await;
        });
    }

    /// The settings file was saved. Follows the `usage_reporting` setting,
    /// then reports the settings again if they now differ from the last
    /// `app_start` sent. Turning reporting on reports the whole launch, which
    /// already carries the settings.
    pub fn settings_saved(&self, config: &AppConfig) {
        let was_enabled = self.reporter.is_enabled();
        self.apply_setting(config);
        if !was_enabled || !self.reporter.is_enabled() {
            return;
        }
        let seen = self.changes.settings_seen.fetch_add(1, Ordering::SeqCst) + 1;
        let (reporter, changes, config) =
            (self.reporter.clone(), self.changes.clone(), config.clone());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(CHANGE_SETTLE).await;
            if changes.settings_seen.load(Ordering::SeqCst) != seen {
                return;
            }
            let app_start = app_start_of(&config);
            {
                let mut reported = changes.reported_app_start.lock().unwrap();
                if reported.as_ref() == Some(&app_start) {
                    return;
                }
                *reported = Some(app_start.clone());
            }
            reporter.record(EventBody::AppStart(app_start));
            reporter.flush().await;
        });
    }

    /// The user chose another input or output device (`None`: the OS
    /// default). Reports the devices in use if they now differ from the last
    /// `audio_env` sent.
    pub fn devices_selected(&self, input_id: Option<String>, output_id: Option<String>) {
        if !self.reporter.is_enabled() {
            return;
        }
        let seen = self.changes.devices_seen.fetch_add(1, Ordering::SeqCst) + 1;
        let (reporter, changes) = (self.reporter.clone(), self.changes.clone());
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(CHANGE_SETTLE).await;
            if changes.devices_seen.load(Ordering::SeqCst) != seen {
                return;
            }
            let audio_env = snapshot::audio_env(input_id.as_deref(), output_id.as_deref());
            {
                let mut reported = changes.reported_audio_env.lock().unwrap();
                if reported.as_ref() == Some(&audio_env) {
                    return;
                }
                *reported = Some(audio_env.clone());
            }
            reporter.record(EventBody::AudioEnv(audio_env));
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
    pub fn session_started<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        mode: SessionMode,
        participants: u32,
    ) {
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

/// This launch's `app_start`: the machine and the settings in `config`.
fn app_start_of(config: &AppConfig) -> AppStart {
    let mut app_start = snapshot::app_start(config);
    app_start.webview_version = tauri::webview_version()
        .ok()
        .and_then(|version| major_of(&version));
    app_start
}

/// The `audio_env` for the devices `config` selects.
fn audio_env_of(config: &AppConfig) -> AudioEnv {
    snapshot::audio_env(
        config.input_device_id.as_deref(),
        config.output_device_id.as_deref(),
    )
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
        if let Some((rtt_ms, loss_rate, fec_recovered)) = reading {
            tally.sample(rtt_ms, loss_rate);
            if let Some(recovered) = fec_recovered {
                tally.sample_fec_total(recovered);
            }
        }
        tally.add_xruns_total(underruns);
        tally.add_reconnects_total(reconnects);
    });
}

fn spawn_sampler<R: Runtime>(app: AppHandle<R>, session_id: String) {
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

    /// The whole path a room takes through the app: the session opens, the
    /// sampler reads the link on its own timer, a peer joins, and leaving
    /// closes the session and stops the sampler. Runs on a mock app, so the
    /// state lookups and the spawned tasks are the ones the real app uses.
    ///
    /// Verifies: REQ-TEL-010
    #[tokio::test]
    async fn when_a_room_is_joined_and_left_the_session_is_recorded_and_the_sampler_stops() {
        let dir = tempfile::tempdir().unwrap();
        let reporter = UsageReporter::new(
            Some(dir.path().to_path_buf()),
            "test",
            Arc::new(NoTransport),
            true,
        );
        let app = tauri::test::mock_app();
        app.manage(UsageState::with_reporter(reporter.clone()));
        app.manage(StreamingState::new());
        let usage = app.state::<UsageState>();

        usage.session_started(app.handle(), SessionMode::Join, 3);
        let session_id = reporter.session_id().expect("the session is open");
        // Long enough for the sampler to have read the link several times.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(reporter.session_id().as_deref(), Some(session_id.as_str()));
        usage.participant_joined();
        usage.session_ended(&app.state::<StreamingState>(), EndReason::Left);

        assert_eq!(reporter.session_id(), None);
        let lines: Vec<serde_json::Value> = reporter
            .preview_ndjson()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["event"], "session_start");
        assert_eq!(lines[0]["mode"], "join");
        assert_eq!(lines[1]["event"], "session_end");
        assert_eq!(lines[1]["session_id"], lines[0]["session_id"]);
        assert_eq!(lines[1]["end_reason"], "left");
        assert_eq!(lines[1]["peers_max"], 4);
        assert_eq!(lines[1]["xrun_count"], 0);
    }

    /// Verifies: REQ-TEL-001
    #[tokio::test]
    async fn when_reporting_is_off_a_room_records_nothing_and_starts_no_sampler() {
        let dir = tempfile::tempdir().unwrap();
        let reporter = UsageReporter::new(
            Some(dir.path().to_path_buf()),
            "test",
            Arc::new(NoTransport),
            false,
        );
        let app = tauri::test::mock_app();
        app.manage(UsageState::with_reporter(reporter.clone()));
        app.manage(StreamingState::new());
        let usage = app.state::<UsageState>();

        usage.session_started(app.handle(), SessionMode::Create, 1);
        usage.session_ended(&app.state::<StreamingState>(), EndReason::Left);

        assert_eq!(reporter.session_id(), None);
        assert_eq!(reporter.pending_count(), 0);
        assert_eq!(reporter.preview_ndjson(), "");
    }

    /// A mock app with usage reporting `enabled`, and the launch's settings
    /// and devices already reported, as they are once the app has started.
    fn launched(enabled: bool) -> (tauri::App<tauri::test::MockRuntime>, UsageReporter) {
        let dir = tempfile::tempdir().unwrap();
        let reporter = UsageReporter::new(Some(dir.keep()), "test", Arc::new(NoTransport), enabled);
        let app = tauri::test::mock_app();
        let usage = UsageState::with_reporter(reporter.clone());
        *usage.changes.reported_app_start.lock().unwrap() = Some(app_start_of(&reporting_config()));
        *usage.changes.reported_audio_env.lock().unwrap() = Some(AudioEnv {
            input: Some(jamjam::telemetry::Device {
                name: "Launch microphone".to_string(),
                kind: jamjam::telemetry::DeviceKind::Usb,
                channels: 2,
                sample_rates: vec![48000],
                min_buffer_frames: None,
                is_default: true,
            }),
            output: None,
        });
        app.manage(usage);
        (app, reporter)
    }

    fn reported_lines(reporter: &UsageReporter) -> Vec<serde_json::Value> {
        reporter
            .preview_ndjson()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The settings of a user who has reporting on.
    fn reporting_config() -> AppConfig {
        AppConfig {
            usage_reporting: true,
            ..AppConfig::default()
        }
    }

    fn config_with_buffer_size(buffer_size: u32) -> AppConfig {
        AppConfig {
            buffer_size,
            ..reporting_config()
        }
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_the_settings_change_after_launch_the_new_settings_are_reported() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();

        usage.settings_saved(&config_with_buffer_size(128));
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        let lines = reported_lines(&reporter);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["event"], "app_start");
        assert_eq!(lines[0]["settings"]["buffer_size"], 128);
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_the_settings_change_several_times_in_a_row_only_the_last_state_is_reported() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();

        for buffer_size in [32, 64, 128] {
            usage.settings_saved(&config_with_buffer_size(buffer_size));
        }
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        let lines = reported_lines(&reporter);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["settings"]["buffer_size"], 128);
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_the_settings_are_saved_unchanged_nothing_is_reported() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();

        usage.settings_saved(&reporting_config());
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        assert_eq!(reporter.preview_ndjson(), "");
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_the_settings_change_and_change_back_within_the_wait_nothing_is_reported() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();

        usage.settings_saved(&config_with_buffer_size(128));
        usage.settings_saved(&reporting_config());
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        assert_eq!(reporter.preview_ndjson(), "");
    }

    /// Verifies: REQ-TEL-001
    #[tokio::test]
    async fn when_reporting_is_off_a_change_of_settings_or_devices_reports_nothing() {
        let (app, reporter) = launched(false);
        let usage = app.state::<UsageState>();

        usage.settings_saved(&AppConfig {
            buffer_size: 128,
            ..AppConfig::default()
        });
        usage.devices_selected(Some("no-such-device".to_string()), None);
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        assert_eq!(reporter.pending_count(), 0);
        assert_eq!(reporter.preview_ndjson(), "");
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_another_device_is_chosen_the_devices_in_use_are_reported() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();

        // A device the machine does not have reads as none: it differs from
        // the microphone reported at launch on any machine.
        usage.devices_selected(Some("no-such-device".to_string()), None);
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        let lines = reported_lines(&reporter);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["event"], "audio_env");
        assert_eq!(lines[0]["input"], serde_json::Value::Null);
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_devices_are_chosen_several_times_in_a_row_they_are_reported_once() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();

        for id in ["no-such-device", "another-missing-device", "a-third-one"] {
            usage.devices_selected(Some(id.to_string()), None);
        }
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        assert_eq!(reported_lines(&reporter).len(), 1);
    }

    /// Verifies: REQ-TEL-014
    #[tokio::test]
    async fn when_the_devices_in_use_are_the_ones_already_reported_nothing_is_reported() {
        let (app, reporter) = launched(true);
        let usage = app.state::<UsageState>();
        let now = snapshot::audio_env(Some("no-such-device"), None);
        *usage.changes.reported_audio_env.lock().unwrap() = Some(now);

        usage.devices_selected(Some("no-such-device".to_string()), None);
        tokio::time::sleep(CHANGE_SETTLE * 4).await;

        assert_eq!(reporter.preview_ndjson(), "");
    }

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
