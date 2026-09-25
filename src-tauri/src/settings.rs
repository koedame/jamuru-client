//! Audio settings (ADR-043)
//!
//! Every change to the audio settings goes through [`apply`]: from the
//! settings window ([`settings_change`]), from the E2E control channel, and
//! from a peer who is helping with the settings. One implementation means a
//! change takes effect the same way whoever makes it - the saved config and a
//! running session both follow - and every window hears about it on
//! [`CHANGED_EVENT`], carrying the settings now in effect.
//!
//! The decision of what a change does is [`plan`], a pure function of the
//! current config and the devices on offer. [`apply`] only carries it out.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;

use jamjam::config::{AppConfig, AudioPreset, VALID_BUFFER_SIZES, VALID_SAMPLE_RATES};

use crate::audio::{self, AudioDeviceInfo};
use crate::config::{self, ConfigState, SampleRateInfo};
use crate::logging::{redact_device_id, redaction_enabled};
use crate::streaming::{SessionSetting, StreamingState};
use crate::usage::UsageState;

/// Emitted to every window after a change, with the [`AudioSettings`] now in
/// effect as the payload.
pub const CHANGED_EVENT: &str = "audio:config-changed";

/// Channels a device is assumed to have when it reports none.
const FALLBACK_CHANNELS: u32 = 2;

/// One change to the audio settings.
///
/// Serialized with the setting's name in `setting` (for example
/// `{"setting": "buffer_size", "samples": 128}`), which is also what a chat
/// line names when a helper made the change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "setting", rename_all = "snake_case")]
pub enum SettingChange {
    InputDevice {
        device_id: String,
    },
    OutputDevice {
        device_id: String,
    },
    /// Input device channels to capture from (1-based; `right` is `None` for mono)
    InputChannels {
        left: u32,
        right: Option<u32>,
    },
    /// Output device channels to play on (1-based; `right` is `None` for mono)
    OutputChannels {
        left: u32,
        right: Option<u32>,
    },
    /// 1 (mono) or 2 (stereo)
    TransmitChannels {
        count: u32,
    },
    /// Frame size in samples. Takes effect on the next connect.
    BufferSize {
        samples: u32,
    },
    /// Takes effect on the next connect.
    SampleRate {
        hz: u32,
    },
    /// A preset's frame size and jitter buffer depth (ADR-019)
    Preset {
        preset: AudioPreset,
    },
}

impl SettingChange {
    /// The change as a log line. Device ids are masked like everywhere else
    /// in `jamjam.log` (REQ-GUI-022).
    fn describe(&self, redact: bool) -> String {
        match self {
            SettingChange::InputDevice { device_id } => {
                format!("input device {}", redact_device_id(device_id, redact))
            }
            SettingChange::OutputDevice { device_id } => {
                format!("output device {}", redact_device_id(device_id, redact))
            }
            other => format!("{:?}", other),
        }
    }
}

/// A left/right pair of 1-based device channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPair {
    pub left: u32,
    /// `None` for mono
    pub right: Option<u32>,
}

/// The audio settings in effect, with the choices on offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_devices: Vec<AudioDeviceInfo>,
    /// The chosen input device; `None` means the system default
    pub input_device_id: Option<String>,
    /// The chosen output device; `None` means the system default
    pub output_device_id: Option<String>,
    pub input_channels: ChannelPair,
    pub output_channels: ChannelPair,
    pub transmit_channels: u32,
    pub buffer_size: u32,
    pub buffer_sizes: Vec<u32>,
    pub sample_rate: u32,
    pub sample_rates: Vec<SampleRateInfo>,
}

/// Why a change was refused. Every variant leaves the settings as they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    DeviceNotOffered(String),
    ChannelOutOfRange {
        channel: u32,
        available: u32,
    },
    InvalidTransmitChannels(u32),
    InvalidBufferSize(u32),
    InvalidSampleRate(u32),
    /// The devices could not be listed or the config could not be saved.
    Unavailable(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::DeviceNotOffered(id) => write!(f, "Device not found: {}", id),
            SettingsError::ChannelOutOfRange { channel, available } => write!(
                f,
                "Channel {} is not on the device, which has {}",
                channel, available
            ),
            SettingsError::InvalidTransmitChannels(count) => write!(
                f,
                "Transmit channels must be 1 (mono) or 2 (stereo), not {}",
                count
            ),
            SettingsError::InvalidBufferSize(samples) => write!(
                f,
                "Invalid buffer size: {}. Valid values are {:?}",
                samples, VALID_BUFFER_SIZES
            ),
            SettingsError::InvalidSampleRate(hz) => write!(
                f,
                "Invalid sample rate: {}. Valid values are {:?}",
                hz, VALID_SAMPLE_RATES
            ),
            SettingsError::Unavailable(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<SettingsError> for String {
    fn from(e: SettingsError) -> Self {
        e.to_string()
    }
}

/// The devices on offer when a change is planned.
#[derive(Debug, Clone, Default)]
pub struct Devices {
    pub input: Vec<AudioDeviceInfo>,
    pub output: Vec<AudioDeviceInfo>,
}

impl Devices {
    fn list() -> Result<Self, SettingsError> {
        Ok(Self {
            input: audio::input_devices().map_err(SettingsError::Unavailable)?,
            output: audio::output_devices().map_err(SettingsError::Unavailable)?,
        })
    }
}

/// What a change does: the config to save, and what a running session is told.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub config: AppConfig,
    pub session: Vec<SessionSetting>,
}

/// Decides what `change` does to `config`, given the devices on offer.
///
/// A new device keeps the chosen channels when it has them, and falls back to
/// the first ones when it does not - otherwise a channel of the previous
/// interface would be asked of one that is too small to have it.
pub fn plan(
    config: &AppConfig,
    devices: &Devices,
    change: &SettingChange,
) -> Result<Plan, SettingsError> {
    let mut next = config.clone();
    let mut session = Vec::new();
    match change {
        SettingChange::InputDevice { device_id } => {
            let device = offered(&devices.input, device_id)?;
            let (left, right) = fit_channels(
                config.input_channel_l,
                config.input_channel_r,
                channel_count(device),
            );
            next.input_device_id = Some(device_id.clone());
            next.input_channel_l = left;
            next.input_channel_r = right;
            if (left, right) != (config.input_channel_l, config.input_channel_r) {
                session.push(SessionSetting::InputChannels(left, right));
            }
            session.push(SessionSetting::InputDevice(Some(device_id.clone())));
        }
        SettingChange::OutputDevice { device_id } => {
            let device = offered(&devices.output, device_id)?;
            let (left, right) = fit_channels(
                config.output_channel_l,
                config.output_channel_r,
                channel_count(device),
            );
            next.output_device_id = Some(device_id.clone());
            next.output_channel_l = left;
            next.output_channel_r = right;
            if (left, right) != (config.output_channel_l, config.output_channel_r) {
                session.push(SessionSetting::OutputChannels(left, right));
            }
            session.push(SessionSetting::OutputDevice(Some(device_id.clone())));
        }
        SettingChange::InputChannels { left, right } => {
            let available = selected(&devices.input, &config.input_device_id).map(channel_count);
            check_channels(*left, *right, available)?;
            next.input_channel_l = *left;
            next.input_channel_r = *right;
            session.push(SessionSetting::InputChannels(*left, *right));
        }
        SettingChange::OutputChannels { left, right } => {
            let available = selected(&devices.output, &config.output_device_id).map(channel_count);
            check_channels(*left, *right, available)?;
            next.output_channel_l = *left;
            next.output_channel_r = *right;
            session.push(SessionSetting::OutputChannels(*left, *right));
        }
        SettingChange::TransmitChannels { count } => {
            if *count != 1 && *count != 2 {
                return Err(SettingsError::InvalidTransmitChannels(*count));
            }
            next.transmit_channels = *count;
            session.push(SessionSetting::TransmitChannels(*count));
        }
        SettingChange::BufferSize { samples } => {
            if !VALID_BUFFER_SIZES.contains(samples) {
                return Err(SettingsError::InvalidBufferSize(*samples));
            }
            next.buffer_size = *samples;
        }
        SettingChange::SampleRate { hz } => {
            if !VALID_SAMPLE_RATES.contains(hz) {
                return Err(SettingsError::InvalidSampleRate(*hz));
            }
            next.sample_rate = *hz;
        }
        SettingChange::Preset { preset } => {
            next.preset = preset.clone();
            next.buffer_size = preset.frame_size();
            session.push(SessionSetting::JitterBufferFrames(
                preset.jitter_buffer_frames(),
            ));
        }
    }
    Ok(Plan {
        config: next,
        session,
    })
}

fn offered<'a>(
    devices: &'a [AudioDeviceInfo],
    device_id: &str,
) -> Result<&'a AudioDeviceInfo, SettingsError> {
    devices
        .iter()
        .find(|d| d.id == device_id)
        .ok_or_else(|| SettingsError::DeviceNotOffered(device_id.to_string()))
}

/// The device in use: the chosen one, or the system default when none is
/// chosen. `None` when neither is on offer (unplugged, or no devices).
fn selected<'a>(
    devices: &'a [AudioDeviceInfo],
    chosen: &Option<String>,
) -> Option<&'a AudioDeviceInfo> {
    match chosen {
        Some(id) => devices.iter().find(|d| &d.id == id),
        None => devices.iter().find(|d| d.is_default),
    }
}

fn channel_count(device: &AudioDeviceInfo) -> u32 {
    device
        .supported_channels
        .iter()
        .max()
        .map(|&c| u32::from(c))
        .unwrap_or(FALLBACK_CHANNELS)
}

/// Keeps the chosen channels that the device has; a left channel it lacks
/// becomes 1, a right one becomes 2 (or 1 on a mono device).
fn fit_channels(left: u32, right: Option<u32>, available: u32) -> (u32, Option<u32>) {
    let left = if left > available { 1 } else { left };
    let right = right.map(|r| if r > available { available.min(2) } else { r });
    (left, right)
}

/// Channels are 1-based and must exist on the device in use. With no device
/// to check against, only the lower bound can be.
fn check_channels(
    left: u32,
    right: Option<u32>,
    available: Option<u32>,
) -> Result<(), SettingsError> {
    for channel in std::iter::once(left).chain(right) {
        let upper = available.unwrap_or(u32::MAX);
        if channel == 0 || channel > upper {
            return Err(SettingsError::ChannelOutOfRange {
                channel,
                available: available.unwrap_or(0),
            });
        }
    }
    Ok(())
}

/// The settings `config` describes, with `devices` on offer.
pub fn snapshot(config: &AppConfig, devices: Devices) -> AudioSettings {
    AudioSettings {
        input_devices: devices.input,
        output_devices: devices.output,
        input_device_id: config.input_device_id.clone(),
        output_device_id: config.output_device_id.clone(),
        input_channels: ChannelPair {
            left: config.input_channel_l,
            right: config.input_channel_r,
        },
        output_channels: ChannelPair {
            left: config.output_channel_l,
            right: config.output_channel_r,
        },
        transmit_channels: config.transmit_channels,
        buffer_size: config.buffer_size,
        buffer_sizes: VALID_BUFFER_SIZES.to_vec(),
        sample_rate: config.sample_rate,
        sample_rates: config::sample_rates(),
    }
}

/// Serializes changes, so two made at once (the user and a helper) cannot
/// both read the old config and one overwrite the other.
#[derive(Default)]
pub struct SettingsState {
    changing: Mutex<()>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// The audio settings in effect.
pub fn current(app: &AppHandle) -> Result<AudioSettings, SettingsError> {
    let config = app
        .state::<ConfigState>()
        .get()
        .map_err(SettingsError::Unavailable)?;
    Ok(snapshot(&config, Devices::list()?))
}

/// Applies `change`: saves it, tells a running session, and announces the new
/// settings to every window. Returns the settings now in effect.
pub async fn apply(
    app: &AppHandle,
    change: &SettingChange,
) -> Result<AudioSettings, SettingsError> {
    let settings_state = app.state::<SettingsState>();
    let _changing = settings_state.changing.lock().await;

    let config_state = app.state::<ConfigState>();
    let devices = Devices::list()?;
    let current = config_state.get().map_err(SettingsError::Unavailable)?;
    let plan = plan(&current, &devices, change)?;
    config_state
        .update(plan.config.clone())
        .map_err(SettingsError::Unavailable)?;

    let streaming = app.state::<StreamingState>();
    for setting in plan.session {
        streaming.apply_setting(setting).await;
    }
    if matches!(
        change,
        SettingChange::InputDevice { .. } | SettingChange::OutputDevice { .. }
    ) {
        app.state::<UsageState>().devices_selected(
            plan.config.input_device_id.clone(),
            plan.config.output_device_id.clone(),
        );
    }
    tracing::info!(
        "Audio setting changed: {}",
        change.describe(redaction_enabled())
    );

    let settings = snapshot(&plan.config, devices);
    if let Err(e) = app.emit(CHANGED_EVENT, &settings) {
        // The change itself is in effect; only the other windows' view of it
        // is stale until they next read the settings.
        tracing::warn!("Could not announce the audio setting change: {}", e);
    }
    Ok(settings)
}

/// The audio settings in effect, with the choices on offer.
#[tauri::command]
pub async fn settings_get(app: AppHandle) -> Result<AudioSettings, String> {
    Ok(current(&app)?)
}

/// Changes one audio setting. Returns the settings now in effect.
#[tauri::command]
pub async fn settings_change(
    change: SettingChange,
    app: AppHandle,
) -> Result<AudioSettings, String> {
    Ok(apply(&app, &change).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, channels: &[u16], is_default: bool) -> AudioDeviceInfo {
        AudioDeviceInfo {
            id: id.to_string(),
            name: id.to_string(),
            supported_sample_rates: vec![48000],
            supported_channels: channels.to_vec(),
            is_default,
            is_asio: false,
        }
    }

    /// An 8-channel interface chosen for both directions, next to a stereo
    /// built-in device that is the system default.
    fn setup() -> (AppConfig, Devices) {
        let config = AppConfig {
            input_device_id: Some("alsa:interface".to_string()),
            output_device_id: Some("alsa:interface".to_string()),
            input_channel_l: 5,
            input_channel_r: Some(6),
            output_channel_l: 7,
            output_channel_r: Some(8),
            ..AppConfig::default()
        };
        let devices = Devices {
            input: vec![
                device("alsa:interface", &[1, 2, 8], false),
                device("alsa:builtin", &[1, 2], true),
            ],
            output: vec![
                device("alsa:interface", &[2, 8], false),
                device("alsa:builtin", &[2], true),
            ],
        };
        (config, devices)
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn switching_to_an_input_device_with_the_chosen_channels_keeps_them() {
        let (mut config, devices) = setup();
        config.input_device_id = Some("alsa:builtin".to_string());
        config.input_channel_l = 1;
        config.input_channel_r = Some(2);

        let plan = plan(
            &config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:interface".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            plan.config.input_device_id.as_deref(),
            Some("alsa:interface")
        );
        assert_eq!(
            (plan.config.input_channel_l, plan.config.input_channel_r),
            (1, Some(2))
        );
        assert_eq!(
            plan.session,
            vec![SessionSetting::InputDevice(Some("alsa:interface".into()))]
        );
    }

    /// Channels 5 and 6 of the interface do not exist on a stereo device;
    /// asking for them would capture nothing.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn switching_to_an_input_device_without_the_chosen_channels_falls_back_to_the_first_ones() {
        let (config, devices) = setup();

        let plan = plan(
            &config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:builtin".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            (plan.config.input_channel_l, plan.config.input_channel_r),
            (1, Some(2))
        );
        assert_eq!(
            plan.session,
            vec![
                SessionSetting::InputChannels(1, Some(2)),
                SessionSetting::InputDevice(Some("alsa:builtin".into())),
            ],
            "a running session must hear about the channels before it reopens the device"
        );
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn switching_to_a_mono_output_device_plays_on_its_only_channel() {
        let (config, mut devices) = setup();
        devices.output.push(device("alsa:mono", &[1], false));

        let plan = plan(
            &config,
            &devices,
            &SettingChange::OutputDevice {
                device_id: "alsa:mono".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            (plan.config.output_channel_l, plan.config.output_channel_r),
            (1, Some(1))
        );
    }

    /// Verifies: REQ-GUI-011
    /// Verifies: REQ-GUI-024
    #[test]
    fn a_device_that_is_not_offered_is_refused_and_nothing_changes() {
        let (config, devices) = setup();

        let result = plan(
            &config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:unplugged".to_string(),
            },
        );

        assert_eq!(
            result,
            Err(SettingsError::DeviceNotOffered("alsa:unplugged".into()))
        );
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn a_channel_the_device_in_use_does_not_have_is_refused() {
        let (config, devices) = setup();

        let result = plan(
            &config,
            &devices,
            &SettingChange::InputChannels {
                left: 9,
                right: None,
            },
        );

        assert_eq!(
            result,
            Err(SettingsError::ChannelOutOfRange {
                channel: 9,
                available: 8
            })
        );
    }

    /// With no device chosen the system default is the one in use, so its
    /// channels are the ones that exist.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn with_no_device_chosen_channels_are_checked_against_the_system_default() {
        let (mut config, devices) = setup();
        config.input_device_id = None;

        let result = plan(
            &config,
            &devices,
            &SettingChange::InputChannels {
                left: 1,
                right: Some(3),
            },
        );

        assert_eq!(
            result,
            Err(SettingsError::ChannelOutOfRange {
                channel: 3,
                available: 2
            })
        );
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn channel_zero_is_refused_even_when_no_device_can_be_checked() {
        let (mut config, devices) = setup();
        config.output_device_id = Some("alsa:unplugged".to_string());

        let result = plan(
            &config,
            &devices,
            &SettingChange::OutputChannels {
                left: 0,
                right: None,
            },
        );

        assert!(matches!(
            result,
            Err(SettingsError::ChannelOutOfRange { channel: 0, .. })
        ));
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn choosing_channels_the_device_has_is_saved_and_reaches_a_running_session() {
        let (config, devices) = setup();

        let plan = plan(
            &config,
            &devices,
            &SettingChange::OutputChannels {
                left: 3,
                right: Some(4),
            },
        )
        .unwrap();

        assert_eq!(
            (plan.config.output_channel_l, plan.config.output_channel_r),
            (3, Some(4))
        );
        assert_eq!(
            plan.session,
            vec![SessionSetting::OutputChannels(3, Some(4))]
        );
    }

    /// Verifies: REQ-AUD-107
    #[test]
    fn choosing_mono_is_saved_and_reaches_a_running_session() {
        let (config, devices) = setup();

        let plan = plan(
            &config,
            &devices,
            &SettingChange::TransmitChannels { count: 1 },
        )
        .unwrap();

        assert_eq!(plan.config.transmit_channels, 1);
        assert_eq!(plan.session, vec![SessionSetting::TransmitChannels(1)]);
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn a_transmit_channel_count_other_than_mono_or_stereo_is_refused() {
        let (config, devices) = setup();

        let result = plan(
            &config,
            &devices,
            &SettingChange::TransmitChannels { count: 3 },
        );

        assert_eq!(result, Err(SettingsError::InvalidTransmitChannels(3)));
    }

    /// The frame size is fixed for the life of a session's audio engines, so a
    /// new buffer size is saved for the next connect and the session is left
    /// alone - restarting it would drop the peer.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn a_new_buffer_size_is_saved_for_the_next_connect_and_the_session_is_left_alone() {
        let (config, devices) = setup();

        let plan = plan(
            &config,
            &devices,
            &SettingChange::BufferSize { samples: 32 },
        )
        .unwrap();

        assert_eq!(plan.config.buffer_size, 32);
        assert!(plan.session.is_empty());
    }

    /// The app once offered 8 and 16 samples, which the config then refused to
    /// save: the panel showed the choice while the next start used the old one.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn a_buffer_size_the_app_cannot_save_is_refused() {
        let (config, devices) = setup();

        for samples in [8, 16, 100, 512] {
            assert_eq!(
                plan(&config, &devices, &SettingChange::BufferSize { samples }),
                Err(SettingsError::InvalidBufferSize(samples))
            );
        }
    }

    /// Every buffer size on offer is one the config accepts.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn every_buffer_size_on_offer_can_be_saved() {
        let (config, devices) = setup();
        for samples in snapshot(&config, devices.clone()).buffer_sizes {
            let plan = plan(&config, &devices, &SettingChange::BufferSize { samples }).unwrap();
            plan.config
                .validate()
                .unwrap_or_else(|e| panic!("{} samples cannot be saved: {}", samples, e));
        }
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn a_sample_rate_outside_the_offered_ones_is_refused() {
        let (config, devices) = setup();

        assert_eq!(
            plan(&config, &devices, &SettingChange::SampleRate { hz: 22050 }),
            Err(SettingsError::InvalidSampleRate(22050))
        );
        let plan = plan(&config, &devices, &SettingChange::SampleRate { hz: 96000 }).unwrap();
        assert_eq!(plan.config.sample_rate, 96000);
        assert!(plan.session.is_empty());
    }

    /// Applying the preset diagnostics recommends uses the preset's own frame
    /// size - the settings window once mapped presets to buffer sizes of its
    /// own, which disagreed with the presets and could not all be saved.
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn applying_a_preset_saves_its_frame_size_and_retunes_a_running_session() {
        let (config, devices) = setup();

        let plan = plan(
            &config,
            &devices,
            &SettingChange::Preset {
                preset: AudioPreset::HighQuality,
            },
        )
        .unwrap();

        assert_eq!(plan.config.preset, AudioPreset::HighQuality);
        assert_eq!(
            plan.config.buffer_size,
            AudioPreset::HighQuality.frame_size()
        );
        assert_eq!(
            plan.session,
            vec![SessionSetting::JitterBufferFrames(
                AudioPreset::HighQuality.jitter_buffer_frames()
            )]
        );
    }

    /// The name a change travels under is part of the protocol a helping
    /// peer speaks, so it is pinned.
    #[test]
    fn a_change_is_named_by_its_setting_on_the_wire() {
        let change = SettingChange::BufferSize { samples: 128 };
        let json = serde_json::to_value(&change).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"setting": "buffer_size", "samples": 128})
        );
        assert_eq!(
            serde_json::from_value::<SettingChange>(json).unwrap(),
            change
        );
    }

    #[test]
    fn a_device_change_is_logged_with_the_device_id_masked() {
        let change = SettingChange::InputDevice {
            device_id: "coreaudio:AppleUSBAudioEngine:Yamaha:AG06:20221310:1,2".to_string(),
        };
        let line = change.describe(true);
        assert!(!line.contains("20221310"), "{}", line);
        assert!(line.contains("AG06"), "{}", line);
    }
}
