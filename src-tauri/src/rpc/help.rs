//! What a person helping sees and does through the help portal, beyond what
//! the permission table says (ADR-044 §5).
//!
//! The table decides which methods the help portal may call. This is the rest of
//! what the person being helped is promised, applied to every call and event
//! that portal carries:
//!
//! - A device is named by a handle that lasts as long as the help, never by its
//!   id, which can carry a serial number. The helper's choice of a device names
//!   a handle; it is turned back into the id here, and a handle this help never
//!   showed is refused.
//! - A failure carries no error text, which can name a device id or a file
//!   path. An audio setting that could not be applied says why in a closed set
//!   of reasons ([`Refusal`]).
//! - A change to an audio setting is announced to the room once it is applied
//!   (the mute and the volumes, which the person can see move, are not).
//! - The meter is read at most [`METER_INTERVAL`] apart, however often it is
//!   asked, and never twice at once: the readings asked for while one is being
//!   taken share it. A helper's polling then cannot load the person's app with
//!   more than ten readings a second, however slowly the app answers.
//! - Another participant's address, which the audio status reports, is not
//!   passed on.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};
use tokio::time::Instant;

use super::spec::Portal;
use super::{authorize, dispatch, Call, Code, RpcError};
use crate::settings::{self, AudioSettings, SettingChange, SettingsError};
use crate::settings_help::setting_name;

/// The least time between two readings of a meter answered afresh. A reading
/// asked for sooner is answered with the last one.
pub const METER_INTERVAL: Duration = Duration::from_millis(100);

/// What a failed call says. The command's own error is never passed on.
const FAILED: &str = "the request failed";

/// Why an audio setting could not be applied or read. A closed set rather than
/// the error's text, which can name a device id or a file path; the helper's app
/// says it in its own language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The device is no longer offered (it was unplugged).
    DeviceGone,
    /// The value does not fit (a channel the device lacks, a size not offered).
    InvalidValue,
    /// The settings could not be read or saved.
    Unavailable,
}

impl Refusal {
    /// What goes on the wire as the error's message; the helper's screen turns
    /// it into a sentence.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::DeviceGone => "device_gone",
            Refusal::InvalidValue => "invalid_value",
            Refusal::Unavailable => "unavailable",
        }
    }
}

impl From<&SettingsError> for Refusal {
    fn from(error: &SettingsError) -> Self {
        match error {
            SettingsError::DeviceNotOffered(_) => Refusal::DeviceGone,
            SettingsError::ChannelOutOfRange { .. }
            | SettingsError::NoLeftChannel
            | SettingsError::InvalidTransmitChannels(_)
            | SettingsError::InvalidBufferSize(_)
            | SettingsError::InvalidSampleRate(_) => Refusal::InvalidValue,
            SettingsError::Unavailable(_) => Refusal::Unavailable,
        }
    }
}

fn refused(refusal: Refusal) -> RpcError {
    RpcError::failed(refusal.code())
}

/// `error` as the helper may hear it: a command's own text is dropped.
fn scrub(error: RpcError) -> RpcError {
    match error.code {
        Code::Failed => RpcError::failed(FAILED),
        _ => error,
    }
}

/// Takes another participant's address out of an audio status.
fn without_addresses(reading: &mut Value) {
    if let Some(status) = reading.as_object_mut() {
        if status.contains_key("remote_addr") {
            status.insert("remote_addr".to_string(), Value::Null);
        }
    }
}

/// Stands in for device ids while someone helps: an id can carry a serial
/// number (it is masked in `jamjam.log` for that reason), so the helper sees a
/// handle instead and the helped side turns it back. Inputs and outputs are
/// numbered apart, so a handle from one list cannot pick a device in the other.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeviceHandles {
    input: Vec<String>,
    output: Vec<String>,
}

impl DeviceHandles {
    const INPUT: &'static str = "input-";
    const OUTPUT: &'static str = "output-";

    fn handle(shown: &mut Vec<String>, prefix: &str, id: &str) -> String {
        let index = match shown.iter().position(|known| known == id) {
            Some(index) => index,
            None => {
                shown.push(id.to_string());
                shown.len() - 1
            }
        };
        format!("{}{}", prefix, index + 1)
    }

    fn find<'s>(shown: &'s [String], prefix: &str, handle: &str) -> Option<&'s String> {
        let number: usize = handle.strip_prefix(prefix)?.parse().ok()?;
        shown.get(number.checked_sub(1)?)
    }

    /// `settings` as the helper may see them.
    pub fn hide(&mut self, mut settings: AudioSettings) -> AudioSettings {
        for device in settings.input_devices.iter_mut() {
            device.id = Self::handle(&mut self.input, Self::INPUT, &device.id);
        }
        for device in settings.output_devices.iter_mut() {
            device.id = Self::handle(&mut self.output, Self::OUTPUT, &device.id);
        }
        settings.input_device_id = settings
            .input_device_id
            .map(|id| Self::handle(&mut self.input, Self::INPUT, &id));
        settings.output_device_id = settings
            .output_device_id
            .map(|id| Self::handle(&mut self.output, Self::OUTPUT, &id));
        settings
    }

    /// `change` with the device it names turned back into its id; `None` for a
    /// handle this help never showed in that list.
    pub fn reveal(&self, change: SettingChange) -> Option<SettingChange> {
        Some(match change {
            SettingChange::InputDevice { device_id } => SettingChange::InputDevice {
                device_id: Self::find(&self.input, Self::INPUT, &device_id)?.clone(),
            },
            SettingChange::OutputDevice { device_id } => SettingChange::OutputDevice {
                device_id: Self::find(&self.output, Self::OUTPUT, &device_id)?.clone(),
            },
            other => other,
        })
    }
}

/// The last reading of one meter and when it was taken. The lock is held while
/// a reading is being taken, so callers asking meanwhile wait for it and take
/// its result instead of asking again.
#[derive(Default)]
struct Meter {
    last: tokio::sync::Mutex<Option<(Instant, Value)>>,
}

impl Meter {
    /// The reading: the last one if it was taken less than [`METER_INTERVAL`]
    /// ago, otherwise a new one from `take`.
    async fn read<F, Fut>(&self, take: F) -> Result<Value, RpcError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value, RpcError>>,
    {
        let mut last = self.last.lock().await;
        if let Some((at, reading)) = last.as_ref() {
            if at.elapsed() < METER_INTERVAL {
                return Ok(reading.clone());
            }
        }
        let reading = take().await?;
        *last = Some((Instant::now(), reading.clone()));
        Ok(reading)
    }
}

/// The person being helped, as far as one help goes: what this help has shown
/// of their devices, how lately their meters were read, and how the room is told
/// of a change.
pub struct HelpGuard {
    handles: Mutex<DeviceHandles>,
    /// The audio status (`streaming_status`) and the input level
    status: Meter,
    input_level: Meter,
    /// Called with the setting's name once a change to it is applied.
    announce: Box<dyn Fn(String) + Send + Sync>,
}

impl HelpGuard {
    pub fn new(announce: impl Fn(String) + Send + Sync + 'static) -> Self {
        Self {
            handles: Mutex::new(DeviceHandles::default()),
            status: Meter::default(),
            input_level: Meter::default(),
            announce: Box::new(announce),
        }
    }

    fn handles(&self) -> std::sync::MutexGuard<'_, DeviceHandles> {
        self.handles.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Makes `call` as the help portal. The table is asked first; what it lets
    /// through is answered as the promises above say.
    pub async fn call<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        call: Call,
    ) -> Result<Value, RpcError> {
        let method = authorize(Portal::Help, &call.method)?;
        match method.name {
            "settings_get" => self.settings(app).await,
            "settings_change" => self.change(app, &call.params).await,
            "streaming_status" => self.status.read(|| Self::audio_status(app)).await,
            "streaming_get_input_level" => self.input_level.read(|| Self::input_level(app)).await,
            _ => dispatch(app, Portal::Help, call).await.map_err(scrub),
        }
    }

    async fn settings<R: Runtime>(&self, app: &AppHandle<R>) -> Result<Value, RpcError> {
        let current = settings::current(app)
            .await
            .map_err(|e| refused(Refusal::from(&e)))?;
        self.shown(current)
    }

    async fn change<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        params: &Value,
    ) -> Result<Value, RpcError> {
        let asked: SettingChange =
            serde_json::from_value(params.get("change").cloned().unwrap_or(Value::Null))
                .map_err(|_| RpcError::invalid_params("not a change to an audio setting"))?;
        let change = self
            .handles()
            .reveal(asked)
            .ok_or_else(|| RpcError::invalid_params("no such device"))?;
        let applied = settings::apply(app, &change).await.map_err(|e| {
            tracing::warn!("A settings change a helper made was refused: {}", e);
            refused(Refusal::from(&e))
        })?;
        (self.announce)(setting_name(&change));
        self.shown(applied)
    }

    /// `settings` as the helper may see them, as the answer to a call.
    fn shown(&self, settings: AudioSettings) -> Result<Value, RpcError> {
        serde_json::to_value(self.handles().hide(settings)).map_err(|_| RpcError::failed(FAILED))
    }

    /// The audio status, read from the app's own state rather than through its
    /// window: it is read ten times a second while a helper watches, and takes
    /// nothing from the window's own work.
    async fn audio_status<R: Runtime>(app: &AppHandle<R>) -> Result<Value, RpcError> {
        let status = crate::streaming::streaming_status(app.state())
            .await
            .map_err(|_| RpcError::failed(FAILED))?;
        let mut reading = serde_json::to_value(status).map_err(|_| RpcError::failed(FAILED))?;
        without_addresses(&mut reading);
        Ok(reading)
    }

    async fn input_level<R: Runtime>(app: &AppHandle<R>) -> Result<Value, RpcError> {
        let level = crate::streaming::streaming_get_input_level(app.state())
            .await
            .map_err(|_| RpcError::failed(FAILED))?;
        Ok(Value::from(level))
    }

    /// `payload` of the event `name` as the helper may hear it, or `None` when
    /// it must not go on. The settings an event announces name devices by their
    /// handles.
    pub fn event(&self, name: &str, payload: Value) -> Option<Value> {
        if name != settings::CHANGED_EVENT {
            return Some(payload);
        }
        let settings: AudioSettings = serde_json::from_value(payload).ok()?;
        serde_json::to_value(self.handles().hide(settings)).ok()
    }
}

/// Audio settings that name `ids` as both the inputs and the outputs on offer,
/// for the tests that look at what a helper is shown.
#[cfg(test)]
pub(crate) fn settings_with_devices(ids: &[&str]) -> AudioSettings {
    use crate::audio::AudioDeviceInfo;
    use crate::settings::ChannelPair;
    let device = |(index, id): (usize, &&str)| AudioDeviceInfo {
        id: id.to_string(),
        name: format!("Interface {}", index + 1),
        supported_sample_rates: vec![48000],
        supported_channels: vec![2],
        is_default: false,
        is_asio: false,
    };
    AudioSettings {
        revision: 0,
        input_devices: ids.iter().enumerate().map(device).collect(),
        output_devices: ids.iter().enumerate().map(device).collect(),
        input_device_id: ids.first().map(|id| id.to_string()),
        output_device_id: ids.last().map(|id| id.to_string()),
        input_channel_count: None,
        output_channel_count: None,
        input_channels: ChannelPair {
            left: 1,
            right: Some(2),
        },
        output_channels: ChannelPair {
            left: 1,
            right: Some(2),
        },
        transmit_channels: 2,
        buffer_size: 64,
        buffer_sizes: vec![32, 64, 128],
        sample_rate: 48000,
        sample_rates: vec![],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    fn guard() -> (HelpGuard, Arc<Mutex<Vec<String>>>) {
        let announced = Arc::new(Mutex::new(Vec::new()));
        let sink = announced.clone();
        (
            HelpGuard::new(move |setting| sink.lock().unwrap().push(setting)),
            announced,
        )
    }

    fn call(method: &str, params: Value) -> Call {
        Call {
            method: method.to_string(),
            params,
            window: None,
            timeout: Duration::from_secs(1),
        }
    }

    fn app() -> AppHandle<tauri::test::MockRuntime> {
        tauri::test::mock_app().handle().clone()
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn the_helper_sees_device_names_but_never_device_ids() {
        let (guard, _) = guard();
        let shown = guard
            .shown(settings_with_devices(&[
                "alsa:serial-ABC123",
                "alsa:serial-XYZ789",
            ]))
            .unwrap();
        let text = shown.to_string();

        assert!(
            !text.contains("ABC123") && !text.contains("XYZ789"),
            "{}",
            text
        );
        assert_eq!(shown["input_devices"][0]["id"], "input-1");
        assert_eq!(shown["input_devices"][0]["name"], "Interface 1");
        assert_eq!(shown["output_devices"][1]["id"], "output-2");
        assert_eq!(shown["input_device_id"], "input-1");
        assert_eq!(shown["output_device_id"], "output-2");
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn a_device_the_helper_picks_by_its_handle_is_the_helped_sides_own_and_a_handle_never_shown_is_refused(
    ) {
        let mut handles = DeviceHandles::default();
        handles.hide(settings_with_devices(&["in-a", "in-b"]));

        assert_eq!(
            handles.reveal(SettingChange::InputDevice {
                device_id: "input-2".into()
            }),
            Some(SettingChange::InputDevice {
                device_id: "in-b".into()
            })
        );
        assert_eq!(
            handles.reveal(SettingChange::OutputDevice {
                device_id: "output-1".into()
            }),
            Some(SettingChange::OutputDevice {
                device_id: "in-a".into()
            })
        );
        for hidden in ["input-3", "input-0", "in-a", "output-1", "", "input-x"] {
            assert_eq!(
                handles.reveal(SettingChange::InputDevice {
                    device_id: hidden.into()
                }),
                None,
                "{:?}",
                hidden
            );
        }
        assert!(handles
            .reveal(SettingChange::BufferSize { samples: 64 })
            .is_some());
    }

    /// Verifies: REQ-RMT-006
    #[tokio::test]
    async fn a_change_to_a_device_never_shown_is_refused_before_anything_is_applied() {
        let (guard, announced) = guard();
        let error = guard
            .call(
                &app(),
                call(
                    "settings_change",
                    json!({"change": {"setting": "input_device", "device_id": "alsa:serial-ABC123"}}),
                ),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, Code::InvalidParams);
        assert!(!error.message.contains("ABC123"), "{}", error.message);
        assert!(announced.lock().unwrap().is_empty());
    }

    /// Verifies: REQ-RMT-006
    #[tokio::test]
    async fn a_change_that_is_not_a_change_is_refused() {
        let (guard, _) = guard();
        for params in [
            json!({}),
            json!({"change": {"setting": "nothing"}}),
            json!(null),
        ] {
            let error = guard
                .call(&app(), call("settings_change", params))
                .await
                .unwrap_err();
            assert_eq!(error.code, Code::InvalidParams);
        }
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn a_change_that_could_not_be_applied_says_why_in_a_fixed_reason_not_the_errors_text() {
        let gone = SettingsError::DeviceNotOffered("alsa:serial-ABC123".into());
        assert_eq!(Refusal::from(&gone), Refusal::DeviceGone);
        assert_eq!(
            Refusal::from(&SettingsError::InvalidBufferSize(7)),
            Refusal::InvalidValue
        );
        assert_eq!(
            Refusal::from(&SettingsError::Unavailable("/home/x/config.toml".into())),
            Refusal::Unavailable
        );

        for refusal in [
            Refusal::DeviceGone,
            Refusal::InvalidValue,
            Refusal::Unavailable,
        ] {
            let error = refused(refusal);
            assert_eq!(error.code, Code::Failed);
            assert_eq!(error.message, refusal.code());
        }
    }

    /// A command that fails in the person's app carries no text of its own to
    /// the helper: it can name a device id or a file path.
    ///
    /// Verifies: REQ-RMT-006
    #[test]
    fn a_command_that_fails_answers_without_its_error_text() {
        let error = scrub(RpcError::failed("Device not found: alsa:serial-ABC123"));
        assert_eq!(error.code, Code::Failed);
        assert_eq!(error.message, FAILED);

        let denied = RpcError::new(Code::Denied, "Help may not call x");
        assert_eq!(scrub(denied.clone()), denied, "our own words are kept");
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn another_participants_address_is_not_passed_on_in_the_audio_status() {
        let mut status = json!({"remote_addr": "203.0.113.7:50000", "input_level": 40});
        without_addresses(&mut status);
        assert_eq!(status, json!({"remote_addr": null, "input_level": 40}));

        let mut level = json!(40);
        without_addresses(&mut level);
        assert_eq!(level, json!(40));
    }

    /// Verifies: REQ-RMT-025
    #[tokio::test]
    async fn what_the_table_does_not_let_the_help_portal_call_is_denied_before_the_guard_acts() {
        let (guard, announced) = guard();
        for method in [
            "signaling_send_chat",
            "session_leave",
            "help_call",
            "window_open_settings",
            "config_set_server_url",
        ] {
            let error = guard
                .call(&app(), call(method, json!({})))
                .await
                .unwrap_err();
            assert_eq!(error.code, Code::Denied, "{}", method);
        }
        let error = guard
            .call(&app(), call("debug.info", json!({})))
            .await
            .unwrap_err();
        assert!(
            matches!(error.code, Code::Denied | Code::UnknownMethod),
            "{:?}",
            error
        );
        assert!(announced.lock().unwrap().is_empty());
    }

    /// The room hears of a change to an audio setting, and of nothing else a
    /// helper does: muting and the volumes are not announced.
    ///
    /// Verifies: REQ-RMT-004
    #[tokio::test]
    async fn only_a_change_to_an_audio_setting_is_announced_to_the_room() {
        let (guard, announced) = guard();
        for (method, params) in [
            ("streaming_set_mute", json!({"muted": true})),
            ("streaming_set_peer_volume", json!({"volume": 120})),
            ("streaming_set_local_pan", json!({"pan": -20})),
        ] {
            // A mock app has no window to run these in; either way nothing is announced.
            let _ = guard.call(&app(), call(method, params)).await;
        }
        assert!(announced.lock().unwrap().is_empty());
    }

    /// Verifies: REQ-RMT-030
    #[tokio::test(start_paused = true)]
    async fn a_meter_asked_for_again_within_a_tenth_of_a_second_is_answered_with_the_last_reading()
    {
        let meter = Meter::default();
        let taken = std::sync::atomic::AtomicU32::new(0);
        let take = || async {
            Ok(json!(
                taken.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ))
        };

        assert_eq!(meter.read(take).await, Ok(json!(0)));
        tokio::time::advance(METER_INTERVAL / 2).await;
        assert_eq!(
            meter.read(take).await,
            Ok(json!(0)),
            "the last reading again"
        );
        tokio::time::advance(METER_INTERVAL / 2).await;
        assert_eq!(
            meter.read(take).await,
            Ok(json!(1)),
            "a tenth of a second on, a new one"
        );
    }

    /// Verifies: REQ-RMT-030
    #[tokio::test(start_paused = true)]
    async fn ten_readings_a_second_is_all_that_a_meter_is_taken_afresh() {
        let meter = Meter::default();
        let taken = std::sync::atomic::AtomicU32::new(0);
        // A caller asking every millisecond for a second.
        for _ in 0..1000 {
            meter
                .read(|| async {
                    Ok(json!(
                        taken.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    ))
                })
                .await
                .unwrap();
            tokio::time::advance(Duration::from_millis(1)).await;
        }
        assert_eq!(taken.load(std::sync::atomic::Ordering::SeqCst), 10);
    }

    /// However slowly the app answers, a reading is never taken twice at once:
    /// the callers who ask meanwhile share it.
    ///
    /// Verifies: REQ-RMT-030
    #[tokio::test(start_paused = true)]
    async fn readings_asked_for_while_one_is_being_taken_share_it() {
        let meter = std::sync::Arc::new(Meter::default());
        let taken = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let callers: Vec<_> = (0..20)
            .map(|_| {
                let (meter, taken) = (meter.clone(), taken.clone());
                tokio::spawn(async move {
                    meter
                        .read(|| async {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            Ok(json!(
                                taken.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                            ))
                        })
                        .await
                })
            })
            .collect();

        for caller in callers {
            assert_eq!(caller.await.unwrap(), Ok(json!(0)));
        }
        assert_eq!(taken.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A reading that fails is not remembered: the next caller tries again.
    ///
    /// Verifies: REQ-RMT-030
    #[tokio::test(start_paused = true)]
    async fn a_reading_that_fails_is_not_remembered() {
        let meter = Meter::default();

        assert!(meter
            .read(|| async { Err(RpcError::failed("x")) })
            .await
            .is_err());
        assert_eq!(meter.read(|| async { Ok(json!(7)) }).await, Ok(json!(7)));
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn a_settings_event_reaches_the_helper_with_device_handles_and_other_events_as_they_are() {
        let (guard, _) = guard();
        let settings = settings_with_devices(&["alsa:serial-ABC123"]);

        let payload = guard
            .event(
                settings::CHANGED_EVENT,
                serde_json::to_value(&settings).unwrap(),
            )
            .unwrap();
        assert!(!payload.to_string().contains("ABC123"));
        assert_eq!(payload["input_device_id"], "input-1");

        assert_eq!(
            guard.event("session:changed", json!({"revision": 3})),
            Some(json!({"revision": 3}))
        );
        assert_eq!(
            guard.event(settings::CHANGED_EVENT, json!({"not": "settings"})),
            None,
            "a settings event that cannot be read is not passed on as it is"
        );
    }
}
