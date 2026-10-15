//! What a settings help did, kept for the person helped (ADR-044 §5).
//!
//! While someone helps, the helped app writes down what they do - a setting
//! changed, the microphone muted or unmuted - and how long the help may sit
//! idle before it ends by itself. The record is what the person's log window
//! shows, and what they are told when the help is over: which settings were
//! changed, with a way to put them back as they were when the help began.
//!
//! One record is kept at a time: the help in progress, or the last one that
//! ended until the person dismisses it, undoes it, or another help begins.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jamjam::config::AudioPreset;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tokio::time::Instant;

use crate::help_link::HelpLinks;
use crate::rpc::help::Refusal;
use crate::settings::{self, AudioSettings, ChannelSide, SettingChange};

/// Emitted with an [`Entry`] each time something is written to the record.
pub const LOG_EVENT: &str = "help:log";

/// Emitted with `{"remaining_ms": n}`: how long until the help ends for lack
/// of anything being done. Sent when something is done and every
/// [`HEARTBEAT`], so a screen that joined late catches up.
pub const REMAINING_EVENT: &str = "help:remaining";

/// How long a help may go without anything being done before it ends.
pub const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);

/// How often the time left is sent to the screens.
pub const HEARTBEAT: Duration = Duration::from_secs(10);

/// The label of the window that shows the record.
const LOG_WINDOW: &str = "help-log";

/// What the help did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryKind {
    Started,
    /// An audio setting, by the name a settings change carries
    SettingChanged {
        setting: String,
    },
    Muted,
    Unmuted,
    /// The person put the settings back as they were before the help
    Restored,
}

/// One line of the record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    /// Milliseconds since the Unix epoch
    pub at_ms: u64,
    #[serde(flatten)]
    pub kind: EntryKind,
}

/// The audio settings as they were when the help began.
#[derive(Debug, Clone)]
pub struct Baseline {
    pub settings: AudioSettings,
    pub preset: AudioPreset,
}

/// Whether a call to the help portal is something being done, which keeps the
/// help from ending for idleness. Reads are not: the screen reads the meters
/// ten times a second all the while.
pub fn is_operation(method: &str) -> bool {
    matches!(
        method,
        "settings_change"
            | "streaming_set_mute"
            | "streaming_set_monitoring"
            | "streaming_set_master_volume"
            | "mixer_set_local_volume"
            | "mixer_set_local_pan"
            | "mixer_set_peer_volume"
            | "mixer_set_peer_pan"
            | "mixer_set_peer_muted"
            | "config_set_peer_name"
            | "config_set_language"
            | "diagnostics_run_complete"
            | "diagnostics_run_network"
            | "diagnostics_run_audio"
            | "diagnostics_run_cpu"
    )
}

struct Inner {
    helper_name: String,
    entries: Vec<Entry>,
    /// The settings changed, each once, in the order they were first changed
    changed: Vec<String>,
    baseline: Option<Baseline>,
    last_operation: Instant,
    active: bool,
}

/// One help's record.
pub struct HelpRecord {
    inner: Mutex<Inner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl HelpRecord {
    /// The record of a help by `helper_name` that begins now.
    pub fn begin(helper_name: String) -> Self {
        Self {
            inner: Mutex::new(Inner {
                helper_name,
                entries: vec![Entry {
                    at_ms: now_ms(),
                    kind: EntryKind::Started,
                }],
                changed: Vec::new(),
                baseline: None,
                last_operation: Instant::now(),
                active: true,
            }),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_baseline(&self, baseline: Baseline) {
        self.inner().baseline = Some(baseline);
    }

    /// Writes `kind` down. It counts as something being done.
    pub fn note(&self, kind: EntryKind) -> Entry {
        let mut inner = self.inner();
        if let EntryKind::SettingChanged { setting } = &kind {
            if !inner.changed.contains(setting) {
                inner.changed.push(setting.clone());
            }
        }
        inner.last_operation = Instant::now();
        let entry = Entry {
            at_ms: now_ms(),
            kind,
        };
        inner.entries.push(entry.clone());
        entry
    }

    /// Something was done that is not written down (a volume moved).
    pub fn touch(&self) {
        self.inner().last_operation = Instant::now();
    }

    /// How long until the help ends for lack of anything being done.
    pub fn remaining(&self) -> Duration {
        IDLE_LIMIT.saturating_sub(self.inner().last_operation.elapsed())
    }

    /// The help is over, however it ended.
    pub fn finish(&self) {
        self.inner().active = false;
    }

    pub fn view(&self) -> RecordView {
        let inner = self.inner();
        let remaining = IDLE_LIMIT.saturating_sub(inner.last_operation.elapsed());
        RecordView {
            helper_name: inner.helper_name.clone(),
            entries: inner.entries.clone(),
            changed: inner.changed.clone(),
            active: inner.active,
            remaining_ms: inner.active.then_some(remaining.as_millis() as u64),
            can_undo: !inner.active && !inner.changed.is_empty() && inner.baseline.is_some(),
        }
    }
}

/// A record as a screen reads it.
#[derive(Debug, Clone, Serialize)]
pub struct RecordView {
    pub helper_name: String,
    pub entries: Vec<Entry>,
    /// The settings the help changed, by name
    pub changed: Vec<String>,
    pub active: bool,
    /// Until the help ends for idleness; none once it is over
    pub remaining_ms: Option<u64>,
    /// Whether the settings can be put back as they were: the help is over,
    /// it changed some, and how they were is known
    pub can_undo: bool,
}

/// Tells every window how long is left.
pub fn announce_remaining<R: Runtime>(app: &AppHandle<R>, record: &HelpRecord) {
    let remaining_ms = record.remaining().as_millis() as u64;
    if let Err(e) = app.emit(REMAINING_EVENT, json!({ "remaining_ms": remaining_ms })) {
        tracing::debug!("Settings help: the time left was not announced: {}", e);
    }
}

/// Writes `kind` to `record` and tells every window.
pub fn write<R: Runtime>(app: &AppHandle<R>, record: &HelpRecord, kind: EntryKind) {
    let entry = record.note(kind);
    if let Err(e) = app.emit(LOG_EVENT, &entry) {
        tracing::debug!("Settings help: a record line was not announced: {}", e);
    }
    announce_remaining(app, record);
}

/// Keeps the screens told of the time left until the help has sat idle for
/// [`IDLE_LIMIT`]; then returns, and the caller ends the help.
pub async fn watch_idle<R: Runtime>(app: &AppHandle<R>, record: &HelpRecord) {
    loop {
        announce_remaining(app, record);
        let remaining = record.remaining();
        if remaining.is_zero() {
            return;
        }
        tokio::time::sleep(remaining.min(HEARTBEAT)).await;
    }
}

/// The audio settings as they are now, to be put back after the help.
pub async fn capture_baseline<R: Runtime>(app: &AppHandle<R>) -> Option<Baseline> {
    let settings = settings::current(app).await.ok()?;
    let preset = app.state::<crate::config::ConfigState>().get().ok()?.preset;
    Some(Baseline { settings, preset })
}

/// The order settings are put back in: a device before the channels of it.
const RESTORE_ORDER: [&str; 8] = [
    "input_device",
    "output_device",
    "input_channel",
    "output_channel",
    "transmit_channels",
    "preset",
    "buffer_size",
    "sample_rate",
];

/// What putting one setting back came to: the setting's name, and the changes made or why they were refused.
type Restoration = (String, Result<Vec<SettingChange>, Refusal>);

/// The changes that put `changed` back as `baseline` had them, by setting.
/// A setting that cannot be put back says why instead.
///
/// A device that was the system's default (not chosen) is put back by choosing
/// the device that was the default: settings offer no way to go back to
/// "whatever the default is".
fn restoring(baseline: &Baseline, changed: &[String]) -> Vec<Restoration> {
    let before = &baseline.settings;
    let default_of = |devices: &[jamjam::audio::AudioDevice]| {
        devices
            .iter()
            .find(|d| d.is_default)
            .map(|d| d.id.0.clone())
    };
    RESTORE_ORDER
        .iter()
        .filter(|setting| changed.iter().any(|c| c == *setting))
        .map(|setting| {
            let changes = match *setting {
                "input_device" => before
                    .input_device_id
                    .clone()
                    .or_else(|| default_of(&before.input_devices))
                    .map(|device_id| vec![SettingChange::InputDevice { device_id }])
                    .ok_or(Refusal::DeviceGone),
                "output_device" => before
                    .output_device_id
                    .clone()
                    .or_else(|| default_of(&before.output_devices))
                    .map(|device_id| vec![SettingChange::OutputDevice { device_id }])
                    .ok_or(Refusal::DeviceGone),
                "input_channel" => Ok(vec![
                    SettingChange::InputChannel {
                        side: ChannelSide::Left,
                        channel: Some(before.input_channels.left),
                    },
                    SettingChange::InputChannel {
                        side: ChannelSide::Right,
                        channel: before.input_channels.right,
                    },
                ]),
                "output_channel" => Ok(vec![
                    SettingChange::OutputChannel {
                        side: ChannelSide::Left,
                        channel: Some(before.output_channels.left),
                    },
                    SettingChange::OutputChannel {
                        side: ChannelSide::Right,
                        channel: before.output_channels.right,
                    },
                ]),
                "transmit_channels" => Ok(vec![SettingChange::TransmitChannels {
                    count: before.transmit_channels,
                }]),
                "preset" => Ok(vec![SettingChange::Preset {
                    preset: baseline.preset.clone(),
                }]),
                "buffer_size" => Ok(vec![SettingChange::BufferSize {
                    samples: before.buffer_size,
                }]),
                _ => Ok(vec![SettingChange::SampleRate {
                    hz: before.sample_rate,
                }]),
            };
            (setting.to_string(), changes)
        })
        .collect()
}

/// A setting that could not be put back, and why (a closed set of reasons, as
/// [`Refusal`] gives them).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotRestored {
    pub setting: String,
    pub reason: &'static str,
}

/// What putting the settings back did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UndoView {
    pub restored: Vec<String>,
    pub failed: Vec<NotRestored>,
}

impl HelpRecord {
    /// What putting the settings back would change, or `None` while the help
    /// is going on or when how they were is not known.
    fn restoring(&self) -> Option<Vec<Restoration>> {
        let inner = self.inner();
        if inner.active {
            return None;
        }
        Some(restoring(inner.baseline.as_ref()?, &inner.changed))
    }

    /// `restored` are back as they were: they are no longer changes to undo.
    fn restored(&self, restored: &[String]) {
        self.inner().changed.retain(|s| !restored.contains(s));
    }
}

/// Puts the settings the help changed back as they were when it began.
pub async fn undo<R: Runtime>(app: &AppHandle<R>, record: &HelpRecord) -> Result<UndoView, String> {
    let plan = record
        .restoring()
        .ok_or_else(|| "there is nothing to put back".to_string())?;
    let mut restored = Vec::new();
    let mut failed = Vec::new();
    for (setting, changes) in plan {
        let result = match changes {
            Ok(changes) => {
                let mut result = Ok(());
                for change in changes {
                    // The rest of the setting is still tried: one side of a
                    // channel pair may not apply while the other does.
                    if let Err(e) = settings::apply(app, &change).await {
                        tracing::warn!("Putting a setting back after a help failed: {}", e);
                        result = Err(Refusal::from(&e));
                    }
                }
                result
            }
            Err(refusal) => Err(refusal),
        };
        match result {
            Ok(()) => restored.push(setting),
            Err(refusal) => failed.push(NotRestored {
                setting,
                reason: refusal.code(),
            }),
        }
    }
    record.restored(&restored);
    if !restored.is_empty() {
        let entry = record.note(EntryKind::Restored);
        if let Err(e) = app.emit(LOG_EVENT, &entry) {
            tracing::debug!("Settings help: a record line was not announced: {}", e);
        }
    }
    Ok(UndoView { restored, failed })
}

/// The record of the help in progress, or of the last one that ended.
#[tauri::command]
pub fn help_record_get(links: tauri::State<'_, HelpLinks>) -> Option<RecordView> {
    links.record().map(|record| record.view())
}

/// Puts the audio settings back as they were when the last help began.
#[tauri::command]
pub async fn help_undo(
    app: AppHandle,
    links: tauri::State<'_, HelpLinks>,
) -> Result<UndoView, String> {
    let record = links
        .record()
        .ok_or_else(|| "there is no help to undo".to_string())?;
    undo(&app, &record).await
}

/// The person has read what the last help did: it is forgotten, unless it is
/// still going on.
#[tauri::command]
pub fn help_record_dismiss(links: tauri::State<'_, HelpLinks>) {
    links.forget_record_if_over();
}

/// Opens the window that shows the record, or brings it forward.
#[tauri::command]
pub fn help_log_window_open(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LOG_WINDOW) {
        let _ = window.unminimize();
        return window.set_focus().map_err(|e| e.to_string());
    }
    WebviewWindowBuilder::new(
        &app,
        LOG_WINDOW,
        WebviewUrl::App("index.html#/help-log".into()),
    )
    .title("jamuru")
    .inner_size(420.0, 560.0)
    .min_inner_size(320.0, 360.0)
    .resizable(true)
    .build()
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::help::settings_with_devices;
    use crate::rpc::spec::{Portal, APP_METHODS};

    fn record() -> HelpRecord {
        HelpRecord::begin("Aki".into())
    }

    fn changed(setting: &str) -> EntryKind {
        EntryKind::SettingChanged {
            setting: setting.into(),
        }
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_new_record_starts_with_the_help_beginning_and_nothing_changed() {
        let view = record().view();

        assert_eq!(view.helper_name, "Aki");
        assert_eq!(
            view.entries.iter().map(|e| &e.kind).collect::<Vec<_>>(),
            vec![&EntryKind::Started]
        );
        assert!(view.changed.is_empty());
        assert!(view.active);
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn what_the_helper_does_is_written_in_the_order_it_was_done() {
        let record = record();

        record.note(changed("input_device"));
        record.note(EntryKind::Unmuted);
        record.note(EntryKind::Muted);

        let kinds: Vec<_> = record.view().entries.into_iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                EntryKind::Started,
                changed("input_device"),
                EntryKind::Unmuted,
                EntryKind::Muted
            ]
        );
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn a_setting_changed_twice_is_listed_once_in_the_order_it_was_first_changed() {
        let record = record();

        record.note(changed("buffer_size"));
        record.note(changed("input_device"));
        record.note(changed("buffer_size"));

        assert_eq!(record.view().changed, vec!["buffer_size", "input_device"]);
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_muting_is_written_down_but_is_not_a_setting_to_put_back() {
        let record = record();

        record.note(EntryKind::Unmuted);

        assert!(record.view().changed.is_empty());
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_line_of_the_record_says_what_it_is_and_when() {
        let line = serde_json::to_value(Entry {
            at_ms: 1234,
            kind: changed("buffer_size"),
        })
        .unwrap();

        assert_eq!(
            line,
            json!({"at_ms": 1234, "kind": "setting_changed", "setting": "buffer_size"})
        );
        let unmuted = serde_json::to_value(Entry {
            at_ms: 1,
            kind: EntryKind::Unmuted,
        })
        .unwrap();
        assert_eq!(unmuted, json!({"at_ms": 1, "kind": "unmuted"}));
    }

    /// Verifies: REQ-RMT-036
    #[tokio::test(start_paused = true)]
    async fn the_time_left_runs_down_and_an_operation_sets_it_back_to_the_whole() {
        let record = record();
        assert_eq!(record.remaining(), IDLE_LIMIT);

        tokio::time::advance(Duration::from_secs(10 * 60)).await;
        assert_eq!(record.remaining(), Duration::from_secs(20 * 60));

        record.touch();
        assert_eq!(record.remaining(), IDLE_LIMIT);

        tokio::time::advance(Duration::from_secs(10 * 60)).await;
        record.note(EntryKind::Muted);
        assert_eq!(record.remaining(), IDLE_LIMIT, "writing it down counts too");
    }

    /// Verifies: REQ-RMT-036
    #[tokio::test(start_paused = true)]
    async fn nothing_is_left_after_the_limit_and_the_time_never_goes_below_nothing() {
        let record = record();

        tokio::time::advance(IDLE_LIMIT + Duration::from_secs(5)).await;

        assert_eq!(record.remaining(), Duration::ZERO);
    }

    /// Verifies: REQ-RMT-036
    #[tokio::test(start_paused = true)]
    async fn the_view_gives_the_time_left_only_while_the_help_goes_on() {
        let record = record();
        tokio::time::advance(Duration::from_secs(60)).await;

        assert_eq!(record.view().remaining_ms, Some(29 * 60 * 1000));

        record.finish();
        assert_eq!(record.view().remaining_ms, None);
    }

    /// A method that changes something on the helped side is an operation:
    /// leaving one out would let a helper who is working still be cut off.
    ///
    /// Verifies: REQ-RMT-036
    #[test]
    fn every_method_a_helper_may_call_that_changes_something_counts_as_an_operation() {
        for method in APP_METHODS.iter().filter(|m| m.access.allows(Portal::Help)) {
            let changes = method.name.contains("_set_")
                || method.name.contains("_change")
                || method.name.starts_with("diagnostics_run");
            assert_eq!(is_operation(method.name), changes, "{}", method.name);
        }
    }

    /// Verifies: REQ-RMT-036
    #[test]
    fn reading_the_meters_is_not_an_operation() {
        for method in [
            "streaming_status",
            "streaming_get_input_level",
            "mixer_get",
            "settings_get",
        ] {
            assert!(!is_operation(method), "{}", method);
        }
    }

    fn baseline() -> Baseline {
        let mut settings = settings_with_devices(&["in-a", "in-b"]);
        settings.buffer_size = 128;
        settings.sample_rate = 44100;
        settings.transmit_channels = 1;
        Baseline {
            settings,
            preset: AudioPreset::default(),
        }
    }

    fn names(plan: &[(String, Result<Vec<SettingChange>, Refusal>)]) -> Vec<&str> {
        plan.iter().map(|(setting, _)| setting.as_str()).collect()
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn only_the_settings_the_help_changed_are_put_back_and_devices_come_before_what_depends_on_them(
    ) {
        let changed: Vec<String> = ["sample_rate", "input_channel", "input_device", "preset"]
            .into_iter()
            .map(String::from)
            .collect();

        let plan = restoring(&baseline(), &changed);

        assert_eq!(
            names(&plan),
            vec!["input_device", "input_channel", "preset", "sample_rate"]
        );
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn each_setting_is_put_back_to_its_value_before_the_help() {
        let all: Vec<String> = RESTORE_ORDER.iter().map(|s| s.to_string()).collect();

        let plan = restoring(&baseline(), &all);

        let changes: Vec<SettingChange> = plan
            .into_iter()
            .flat_map(|(_, changes)| changes.unwrap())
            .collect();
        assert_eq!(
            changes,
            vec![
                SettingChange::InputDevice {
                    device_id: "in-a".into()
                },
                SettingChange::OutputDevice {
                    device_id: "in-b".into()
                },
                SettingChange::InputChannel {
                    side: ChannelSide::Left,
                    channel: Some(1)
                },
                SettingChange::InputChannel {
                    side: ChannelSide::Right,
                    channel: Some(2)
                },
                SettingChange::OutputChannel {
                    side: ChannelSide::Left,
                    channel: Some(1)
                },
                SettingChange::OutputChannel {
                    side: ChannelSide::Right,
                    channel: Some(2)
                },
                SettingChange::TransmitChannels { count: 1 },
                SettingChange::Preset {
                    preset: AudioPreset::default()
                },
                SettingChange::BufferSize { samples: 128 },
                SettingChange::SampleRate { hz: 44100 },
            ]
        );
    }

    /// A preset also sets the buffer: put back first, so a buffer the helper
    /// changed after it is put back to the person's own and not overwritten.
    ///
    /// Verifies: REQ-RMT-035
    #[test]
    fn a_preset_is_put_back_before_the_buffer() {
        let changed = vec!["buffer_size".to_string(), "preset".to_string()];

        let plan = restoring(&baseline(), &changed);

        assert_eq!(names(&plan), vec!["preset", "buffer_size"]);
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn a_mono_input_is_put_back_as_mono() {
        let mut before = baseline();
        before.settings.input_channels.right = None;

        let plan = restoring(&before, &["input_channel".to_string()]);

        assert_eq!(
            plan[0].1.clone().unwrap()[1],
            SettingChange::InputChannel {
                side: ChannelSide::Right,
                channel: None
            }
        );
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn a_device_that_was_the_systems_default_is_put_back_by_choosing_the_default_and_with_none_to_choose_it_says_so(
    ) {
        let mut before = baseline();
        before.settings.input_device_id = None;
        before.settings.input_devices[1].is_default = true;

        let plan = restoring(&before, &["input_device".to_string()]);
        assert_eq!(
            plan[0].1,
            Ok(vec![SettingChange::InputDevice {
                device_id: "in-b".into()
            }])
        );

        before.settings.input_devices[1].is_default = false;
        let plan = restoring(&before, &["input_device".to_string()]);
        assert_eq!(plan[0].1, Err(Refusal::DeviceGone));
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn the_settings_can_be_put_back_only_once_the_help_is_over_and_how_they_were_is_known() {
        let record = record();
        record.note(changed("buffer_size"));
        assert!(!record.view().can_undo, "the help is still going on");
        assert!(record.restoring().is_none());

        record.finish();
        assert!(!record.view().can_undo, "how they were is not known");
        assert!(record.restoring().is_none());

        record.set_baseline(baseline());
        assert!(record.view().can_undo);
        assert_eq!(record.restoring().unwrap().len(), 1);
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn a_help_that_changed_nothing_has_nothing_to_put_back() {
        let record = record();
        record.set_baseline(baseline());
        record.note(EntryKind::Muted);
        record.finish();

        assert!(!record.view().can_undo);
    }

    /// Verifies: REQ-RMT-035
    #[test]
    fn a_setting_put_back_is_no_longer_listed_and_one_that_failed_stays_to_try_again() {
        let record = record();
        record.set_baseline(baseline());
        record.note(changed("buffer_size"));
        record.note(changed("input_device"));
        record.finish();

        record.restored(&["buffer_size".to_string()]);

        let view = record.view();
        assert_eq!(view.changed, vec!["input_device"]);
        assert!(view.can_undo);
    }

    /// Verifies: REQ-RMT-035
    #[tokio::test]
    async fn putting_back_while_the_help_goes_on_is_refused() {
        let record = record();
        record.set_baseline(baseline());
        record.note(changed("buffer_size"));
        let app = tauri::test::mock_app();

        let result = undo(app.handle(), &record).await;

        assert!(result.is_err());
    }
}
