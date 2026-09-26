//! Audio settings (ADR-043)
//!
//! Every change to the audio settings goes through [`apply`]: from the
//! settings window ([`settings_change`]) and from the E2E control channel. One
//! implementation means a change takes effect the same way whoever makes it -
//! the saved config and a running session both follow - and every window
//! hears about it on [`CHANGED_EVENT`], carrying the settings now in effect.
//!
//! The decision of what a change does is [`plan`], a pure function of the
//! current config and the devices on offer. [`apply`] only carries it out.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Mutex;

use jamjam::config::{
    AppConfig, AudioPreset, MAX_DEVICE_CHANNELS, VALID_BUFFER_SIZES, VALID_SAMPLE_RATES,
};

use crate::audio::{self, AudioDeviceInfo};
use crate::config::{self, ConfigState, SampleRateInfo};
use crate::logging::{redact_device_id, redaction_enabled};
use crate::streaming::{SessionSetting, StreamingState};
use crate::usage::UsageState;

/// Emitted to every window after a change, with the [`AudioSettings`] now in
/// effect as the payload.
pub const CHANGED_EVENT: &str = "audio:config-changed";

/// Which channel of a left/right pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSide {
    /// Left, or the only channel when mono
    Left,
    Right,
}

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
    /// One side of the input channel pair (1-based). The other side stays as
    /// it is, so two quick changes cannot undo each other. A right channel of
    /// `None` is mono; the left one is always a channel. `channel` must be
    /// present (`null` for none): a misspelt field must not read as mono.
    InputChannel {
        side: ChannelSide,
        #[serde(deserialize_with = "present")]
        channel: Option<u32>,
    },
    /// One side of the output channel pair, as [`Self::InputChannel`].
    OutputChannel {
        side: ChannelSide,
        #[serde(deserialize_with = "present")]
        channel: Option<u32>,
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

/// An `Option` field that has to be in the message, as a value or `null`.
/// (`deserialize_with` turns off serde's reading of a missing `Option` as
/// `None`.)
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    Option::<u32>::deserialize(deserializer)
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
    /// Counts up with every change. A window holding settings of a later
    /// revision ignores older ones, whichever order they arrive in.
    pub revision: u64,
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_devices: Vec<AudioDeviceInfo>,
    /// The chosen input device; `None` means the system default
    pub input_device_id: Option<String>,
    /// The chosen output device; `None` means the system default
    pub output_device_id: Option<String>,
    /// Channels the input device in use offers; `None` when it does not say
    pub input_channel_count: Option<u32>,
    /// Channels the output device in use offers; `None` when it does not say
    pub output_channel_count: Option<u32>,
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
    /// The left channel was given as none; only the right one can be (mono).
    NoLeftChannel,
    InvalidTransmitChannels(u32),
    InvalidBufferSize(u32),
    InvalidSampleRate(u32),
    /// The config could not be read or saved.
    Unavailable(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Masked: the message reaches jamjam.log through the webview's
            // record of failed commands (REQ-GUI-022).
            SettingsError::DeviceNotOffered(id) => write!(
                f,
                "Device not found: {}",
                redact_device_id(id, redaction_enabled())
            ),
            SettingsError::ChannelOutOfRange { channel, available } => write!(
                f,
                "Channel {} is not on the device, which has {}",
                channel, available
            ),
            SettingsError::NoLeftChannel => f.write_str("The left channel cannot be none"),
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

impl From<String> for SettingsError {
    fn from(message: String) -> Self {
        SettingsError::Unavailable(message)
    }
}

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
    /// The devices the system lists. A direction that cannot be listed counts
    /// as offering none: that refuses choosing one of its devices, but leaves
    /// every other setting (buffer size, sample rate, ...) changeable.
    ///
    /// Both directions are listed at once, so a driver that does not answer
    /// costs one wait, not two.
    pub(crate) fn list() -> Self {
        let or_none = |listed: Result<Vec<AudioDeviceInfo>, String>, kind: &str| {
            listed.unwrap_or_else(|e| {
                tracing::warn!("Could not list the {} devices: {}", kind, e);
                Vec::new()
            })
        };
        std::thread::scope(|scope| {
            let output = scope.spawn(audio::output_devices);
            let input = audio::input_devices();
            Self {
                input: or_none(input, "input"),
                output: or_none(
                    output.join().unwrap_or_else(|_| Err("panicked".into())),
                    "output",
                ),
            }
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
/// A new device leaves the saved channel pair as it is: the pair is what the
/// user chose, and a device that lacks it is opened with its first channels
/// instead (`pair_in_use`), so going back to the interface brings the chosen
/// channels back. A change to one channel of the pair starts from the pair in
/// use, the one the settings show.
pub fn plan(
    config: &AppConfig,
    devices: &Devices,
    change: &SettingChange,
) -> Result<Plan, SettingsError> {
    let mut next = config.clone();
    let mut session = Vec::new();
    match change {
        SettingChange::InputDevice { device_id } => {
            offered(&devices.input, device_id)?;
            next.input_device_id = Some(device_id.clone());
            session.push(SessionSetting::InputDevice(Some(device_id.clone())));
        }
        SettingChange::OutputDevice { device_id } => {
            offered(&devices.output, device_id)?;
            next.output_device_id = Some(device_id.clone());
            session.push(SessionSetting::OutputDevice(Some(device_id.clone())));
        }
        SettingChange::InputChannel { side, channel } => {
            let available =
                selected(&devices.input, &config.input_device_id).and_then(channel_count);
            let (left, right) = with_side(
                pair_in_use(
                    &devices.input,
                    &config.input_device_id,
                    (config.input_channel_l, config.input_channel_r),
                ),
                *side,
                *channel,
                available,
            )?;
            next.input_channel_l = left;
            next.input_channel_r = right;
            session.push(SessionSetting::InputChannels(left, right));
        }
        SettingChange::OutputChannel { side, channel } => {
            let available =
                selected(&devices.output, &config.output_device_id).and_then(channel_count);
            let (left, right) = with_side(
                pair_in_use(
                    &devices.output,
                    &config.output_device_id,
                    (config.output_channel_l, config.output_channel_r),
                ),
                *side,
                *channel,
                available,
            )?;
            next.output_channel_l = left;
            next.output_channel_r = right;
            session.push(SessionSetting::OutputChannels(left, right));
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

/// The most channels `device` offers; `None` when it does not say.
fn channel_count(device: &AudioDeviceInfo) -> Option<u32> {
    device
        .supported_channels
        .iter()
        .max()
        .map(|&c| u32::from(c))
}

/// The channel pair the device in use is opened with: the saved `pair` when the
/// device has both channels, otherwise its first ones. Decided whenever a
/// device is opened and whenever the settings are shown, never saved.
pub fn pair_in_use(
    devices: &[AudioDeviceInfo],
    chosen: &Option<String>,
    (left, right): (u32, Option<u32>),
) -> (u32, Option<u32>) {
    let available = selected(devices, chosen).and_then(channel_count);
    fit_channels(left, right, available)
}

/// Keeps the pair when the device has both channels, and falls back to the
/// first ones as a pair when it does not: channels 1 and 2 (1 and 1 on a mono
/// device; mono stays mono). A device that does not say keeps the pair.
fn fit_channels(left: u32, right: Option<u32>, available: Option<u32>) -> (u32, Option<u32>) {
    let Some(available) = available else {
        return (left, right);
    };
    let fits = |channel: u32| channel <= available;
    if fits(left) && right.is_none_or(fits) {
        return (left, right);
    }
    (1, right.map(|_| available.min(2)))
}

/// The pair with `side` set to `channel`, checked against the device in use.
/// With no device to check against, only the bounds any device has apply.
fn with_side(
    (left, right): (u32, Option<u32>),
    side: ChannelSide,
    channel: Option<u32>,
    available: Option<u32>,
) -> Result<(u32, Option<u32>), SettingsError> {
    if let Some(channel) = channel {
        let upper = available.unwrap_or(MAX_DEVICE_CHANNELS);
        if channel == 0 || channel > upper {
            return Err(SettingsError::ChannelOutOfRange {
                channel,
                available: upper,
            });
        }
    }
    match side {
        ChannelSide::Left => Ok((channel.ok_or(SettingsError::NoLeftChannel)?, right)),
        ChannelSide::Right => Ok((left, channel)),
    }
}

/// The settings `config` describes, with `devices` on offer.
pub fn snapshot(config: &AppConfig, devices: Devices, revision: u64) -> AudioSettings {
    let input_channel_count =
        selected(&devices.input, &config.input_device_id).and_then(channel_count);
    let output_channel_count =
        selected(&devices.output, &config.output_device_id).and_then(channel_count);
    let input_pair = pair_in_use(
        &devices.input,
        &config.input_device_id,
        (config.input_channel_l, config.input_channel_r),
    );
    let output_pair = pair_in_use(
        &devices.output,
        &config.output_device_id,
        (config.output_channel_l, config.output_channel_r),
    );
    AudioSettings {
        revision,
        input_channel_count,
        output_channel_count,
        input_devices: devices.input,
        output_devices: devices.output,
        input_device_id: config.input_device_id.clone(),
        output_device_id: config.output_device_id.clone(),
        input_channels: ChannelPair {
            left: input_pair.0,
            right: input_pair.1,
        },
        output_channels: ChannelPair {
            left: output_pair.0,
            right: output_pair.1,
        },
        transmit_channels: config.transmit_channels,
        buffer_size: config.buffer_size,
        buffer_sizes: VALID_BUFFER_SIZES.to_vec(),
        sample_rate: config.sample_rate,
        sample_rates: config::sample_rates(),
    }
}

/// Orders the changes: one at a time from planning to announcing the result,
/// each numbered, so the settings windows can tell a newer announcement from
/// an older one.
///
/// The devices are listed before a change takes its turn, never while it
/// holds it: a driver that does not answer must not make every other setting
/// wait behind the listing.
#[derive(Default)]
pub struct SettingsState {
    changing: Mutex<()>,
    revision: AtomicU64,
}

impl SettingsState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Holds off changes while the guard lives, for a step that must read the
    /// settings and act on them before any change lands in between (a session
    /// starting).
    pub async fn hold_changes(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.changing.lock().await
    }
}

/// The devices on offer, listed off the async runtime's threads: the listing
/// waits for the driver.
async fn devices_on_offer() -> Devices {
    match tauri::async_runtime::spawn_blocking(Devices::list).await {
        Ok(devices) => devices,
        Err(e) => {
            tracing::warn!("Listing the devices failed: {}", e);
            Devices::default()
        }
    }
}

/// The audio settings in effect.
pub async fn current<R: Runtime>(app: &AppHandle<R>) -> Result<AudioSettings, SettingsError> {
    let devices = devices_on_offer().await;
    let settings_state = app.state::<SettingsState>();
    let _changing = settings_state.changing.lock().await;
    let config = app.state::<ConfigState>().get()?;
    Ok(snapshot(
        &config,
        devices,
        settings_state.revision.load(Ordering::SeqCst),
    ))
}

/// Applies `change`: saves it, tells a running session, and announces the new
/// settings to every window. Returns the settings now in effect.
pub async fn apply<R: Runtime>(
    app: &AppHandle<R>,
    change: &SettingChange,
) -> Result<AudioSettings, SettingsError> {
    let devices = devices_on_offer().await;
    let settings_state = app.state::<SettingsState>();
    let _changing = settings_state.changing.lock().await;

    let (config, session) = app.state::<ConfigState>().update_with(|current| {
        plan(current, &devices, change).map(|plan| (plan.config, plan.session))
    })?;
    let revision = settings_state.revision.fetch_add(1, Ordering::SeqCst) + 1;

    let streaming = app.state::<StreamingState>();
    for setting in session {
        streaming.apply_setting(setting).await;
    }
    if matches!(
        change,
        SettingChange::InputDevice { .. } | SettingChange::OutputDevice { .. }
    ) {
        app.state::<UsageState>().devices_selected(
            config.input_device_id.clone(),
            config.output_device_id.clone(),
        );
    }
    tracing::info!(
        "Audio setting changed: {}",
        change.describe(redaction_enabled())
    );

    let settings = snapshot(&config, devices, revision);
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
    Ok(current(&app).await?)
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
    use jamjam::audio::capture_attempts;

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

    fn input_channel(side: ChannelSide, channel: Option<u32>) -> SettingChange {
        SettingChange::InputChannel { side, channel }
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn switching_to_an_input_device_with_the_chosen_channels_plans_to_keep_them() {
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

    /// Channels 5 and 6 of the interface do not exist on a stereo device, so
    /// the device is opened with its first two - but the pair the user chose
    /// stays saved, and comes back with the interface.
    ///
    /// Verifies: REQ-AUD-122
    #[test]
    fn switching_to_an_input_device_without_the_chosen_channels_keeps_the_saved_pair() {
        let (config, devices) = setup();

        let to_builtin = plan(
            &config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:builtin".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            (
                to_builtin.config.input_channel_l,
                to_builtin.config.input_channel_r
            ),
            (5, Some(6)),
            "the choice is not rewritten to suit the device"
        );
        assert_eq!(
            to_builtin.session,
            vec![SessionSetting::InputDevice(Some("alsa:builtin".into()))],
            "the session reopens the device and fits the pair itself"
        );
        assert_eq!(
            pair_in_use(
                &devices.input,
                &to_builtin.config.input_device_id,
                (5, Some(6))
            ),
            (1, Some(2)),
            "the stereo device is opened with its first two channels"
        );

        let back = plan(
            &to_builtin.config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:interface".to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            pair_in_use(
                &devices.input,
                &back.config.input_device_id,
                (back.config.input_channel_l, back.config.input_channel_r)
            ),
            (5, Some(6)),
            "back on the interface, the chosen channels are in use again"
        );
    }

    /// A mono microphone is opened with its one channel, as a pair of the same
    /// channel (captured as mono). Choosing it must not leave that pair saved:
    /// the stereo interface used next would have lost its right side.
    ///
    /// Verifies: REQ-AUD-122
    #[test]
    fn a_mono_microphone_is_used_as_one_channel_without_changing_the_saved_stereo_pair() {
        let (mut config, mut devices) = setup();
        config.input_channel_l = 1;
        config.input_channel_r = Some(2);
        devices.input.push(device("alsa:mic", &[1], false));

        let plan = plan(
            &config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:mic".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            (plan.config.input_channel_l, plan.config.input_channel_r),
            (1, Some(2))
        );
        let in_use = pair_in_use(&devices.input, &plan.config.input_device_id, (1, Some(2)));
        assert_eq!(in_use, (1, Some(1)));
        assert_eq!(
            capture_attempts(in_use.0, in_use.1, 2)[0],
            [0],
            "the microphone is opened once, as one channel - the peer is told mono"
        );
    }

    /// Falling back one channel at a time could leave a doubled or reversed
    /// pair (2 and 3 on a stereo device becoming 2 and 2).
    ///
    /// Verifies: REQ-AUD-122
    #[test]
    fn when_only_one_channel_of_the_pair_is_missing_the_whole_pair_falls_back() {
        let (_, devices) = setup();

        assert_eq!(
            pair_in_use(&devices.input, &Some("alsa:builtin".into()), (2, Some(3))),
            (1, Some(2))
        );
    }

    /// The settings show the channels in use, so the choice on screen is the
    /// one a device that lacks the saved channels is actually opened with.
    ///
    /// Verifies: REQ-AUD-122
    #[test]
    fn the_settings_show_the_channels_in_use_while_the_saved_pair_stays() {
        let (mut config, devices) = setup();
        config.input_device_id = Some("alsa:builtin".to_string());
        config.output_device_id = Some("alsa:builtin".to_string());

        let shown = snapshot(&config, devices, 1);

        assert_eq!(
            (shown.input_channels.left, shown.input_channels.right),
            (1, Some(2))
        );
        assert_eq!(
            (shown.output_channels.left, shown.output_channels.right),
            (1, Some(2))
        );
        assert_eq!(
            (config.input_channel_l, config.input_channel_r),
            (5, Some(6)),
            "showing the pair does not change what is saved"
        );
    }

    /// A change to one channel starts from the pair on screen, not from the
    /// saved pair the device does not have.
    ///
    /// Verifies: REQ-AUD-122
    #[test]
    fn changing_one_channel_on_a_device_that_lacks_the_saved_pair_starts_from_the_pair_in_use() {
        let (mut config, devices) = setup();
        config.input_device_id = Some("alsa:builtin".to_string());

        let plan = plan(
            &config,
            &devices,
            &input_channel(ChannelSide::Left, Some(2)),
        )
        .unwrap();

        assert_eq!(
            (plan.config.input_channel_l, plan.config.input_channel_r),
            (2, Some(2))
        );
    }

    /// The same holds for a mono output device: it plays its only channel,
    /// and the saved pair stays.
    ///
    /// Verifies: REQ-AUD-122
    #[test]
    fn switching_to_a_mono_output_device_keeps_the_saved_pair_and_plays_its_only_channel() {
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
            (7, Some(8))
        );
        assert_eq!(
            pair_in_use(&devices.output, &plan.config.output_device_id, (7, Some(8))),
            (1, Some(1))
        );
    }

    /// A device that does not report its channels (a busy one, say) keeps the
    /// pair: refusing on a guess would drop an 8-channel interface's choice.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn a_device_that_does_not_say_how_many_channels_it_has_keeps_the_chosen_pair() {
        let (config, mut devices) = setup();
        devices.input.push(device("alsa:busy", &[], false));

        let plan = plan(
            &config,
            &devices,
            &SettingChange::InputDevice {
                device_id: "alsa:busy".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            (plan.config.input_channel_l, plan.config.input_channel_r),
            (5, Some(6))
        );
    }

    /// Verifies: REQ-GUI-011
    /// Verifies: REQ-GUI-024
    #[test]
    fn a_device_that_is_not_offered_is_refused() {
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

        assert_eq!(
            plan(
                &config,
                &devices,
                &input_channel(ChannelSide::Left, Some(9))
            ),
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

        assert_eq!(
            plan(
                &config,
                &devices,
                &input_channel(ChannelSide::Right, Some(3))
            ),
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
            &SettingChange::OutputChannel {
                side: ChannelSide::Left,
                channel: Some(0),
            },
        );

        assert!(matches!(
            result,
            Err(SettingsError::ChannelOutOfRange { channel: 0, .. })
        ));
    }

    /// A device that does not say how many channels it has still has no more
    /// than any device can: a huge number would be saved and then offered.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn a_channel_beyond_what_any_device_has_is_refused_even_when_the_device_does_not_say() {
        let (mut config, mut devices) = setup();
        devices.input.push(device("alsa:busy", &[], false));
        config.input_device_id = Some("alsa:busy".to_string());

        assert_eq!(
            plan(
                &config,
                &devices,
                &input_channel(ChannelSide::Left, Some(MAX_DEVICE_CHANNELS + 1))
            ),
            Err(SettingsError::ChannelOutOfRange {
                channel: MAX_DEVICE_CHANNELS + 1,
                available: MAX_DEVICE_CHANNELS
            })
        );
        assert!(plan(
            &config,
            &devices,
            &input_channel(ChannelSide::Left, Some(12))
        )
        .is_ok());
    }

    /// A misspelt field must not quietly mean mono.
    #[test]
    fn a_channel_change_without_the_channel_field_does_not_parse() {
        let missing = serde_json::json!({"setting": "input_channel", "side": "right", "chanel": 3});
        assert!(serde_json::from_value::<SettingChange>(missing).is_err());
        let mono =
            serde_json::json!({"setting": "input_channel", "side": "right", "channel": null});
        assert_eq!(
            serde_json::from_value::<SettingChange>(mono).unwrap(),
            input_channel(ChannelSide::Right, None)
        );
    }

    /// Changing one side keeps the other as it is in the config - not as a
    /// window last saw it - so two quick changes cannot undo each other.
    ///
    /// Verifies: REQ-GUI-024
    #[test]
    fn changing_one_channel_of_the_pair_plans_the_other_as_it_is() {
        let (config, devices) = setup();

        let plan = plan(
            &config,
            &devices,
            &SettingChange::OutputChannel {
                side: ChannelSide::Right,
                channel: Some(4),
            },
        )
        .unwrap();

        assert_eq!(
            (plan.config.output_channel_l, plan.config.output_channel_r),
            (7, Some(4))
        );
        assert_eq!(
            plan.session,
            vec![SessionSetting::OutputChannels(7, Some(4))]
        );
    }

    /// Verifies: REQ-GUI-024
    #[test]
    fn no_right_channel_plans_mono_and_no_left_channel_is_refused() {
        let (config, devices) = setup();

        let mono = plan(&config, &devices, &input_channel(ChannelSide::Right, None)).unwrap();
        assert_eq!(
            (mono.config.input_channel_l, mono.config.input_channel_r),
            (5, None)
        );
        assert_eq!(
            plan(&config, &devices, &input_channel(ChannelSide::Left, None)),
            Err(SettingsError::NoLeftChannel)
        );
    }

    /// Verifies: REQ-AUD-107
    #[test]
    fn choosing_mono_plans_to_save_it_and_to_tell_a_running_session() {
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
    fn a_new_buffer_size_is_planned_for_the_next_connect_and_the_session_is_left_alone() {
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

    /// Applying a preset writes its frame size as the buffer size, so every
    /// preset's frame size has to be one the app offers and can save.
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn every_presets_frame_size_is_a_buffer_size_on_offer() {
        let (config, devices) = setup();
        let offered = snapshot(&config, devices.clone(), 0).buffer_sizes;
        for preset in AudioPreset::all() {
            assert!(
                offered.contains(&preset.frame_size()),
                "{:?}'s frame size {} is not offered ({:?})",
                preset,
                preset.frame_size(),
                offered
            );
            plan(&config, &devices, &SettingChange::Preset { preset })
                .unwrap()
                .config
                .validate()
                .unwrap();
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
    fn applying_a_preset_plans_its_frame_size_and_a_new_jitter_depth_for_a_running_session() {
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

    #[test]
    fn the_settings_report_the_channels_of_the_devices_in_use() {
        let (mut config, devices) = setup();
        config.output_device_id = None;

        let settings = snapshot(&config, devices, 3);

        assert_eq!(settings.input_channel_count, Some(8));
        assert_eq!(settings.output_channel_count, Some(2), "the system default");
        assert_eq!(settings.revision, 3);
    }

    /// The name a change travels under is part of the protocol a helping
    /// peer speaks, so it is pinned.
    #[test]
    fn a_change_is_named_by_its_setting_on_the_wire() {
        let change = input_channel(ChannelSide::Right, None);
        let json = serde_json::to_value(&change).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"setting": "input_channel", "side": "right", "channel": null})
        );
        assert_eq!(
            serde_json::from_value::<SettingChange>(json).unwrap(),
            change
        );
    }

    #[test]
    fn a_device_change_and_its_refusal_mask_the_device_id() {
        let id = "coreaudio:AppleUSBAudioEngine:Yamaha:AG06:20221310:1,2";
        let line = SettingChange::InputDevice {
            device_id: id.to_string(),
        }
        .describe(true);
        assert!(!line.contains("20221310"), "{}", line);
        assert!(line.contains("AG06"), "{}", line);
        assert!(!SettingsError::DeviceNotOffered(id.to_string())
            .to_string()
            .contains("20221310"));
    }

    mod applying {
        use super::*;
        use std::sync::{Arc, Mutex as StdMutex};
        use tauri::Listener;

        use crate::streaming::StreamingCommand;

        /// A mock app with its config in `dir`, starting from `config`.
        fn app(dir: &tempfile::TempDir, config: AppConfig) -> tauri::App<tauri::test::MockRuntime> {
            let app = tauri::test::mock_app();
            app.manage(ConfigState::at(dir.path().join("config.toml"), config));
            app.manage(SettingsState::new());
            app.manage(StreamingState::new());
            app.manage(UsageState::with_reporter(
                jamjam::telemetry::UsageReporter::new(
                    None,
                    "test",
                    Arc::new(jamjam::telemetry::NoTransport),
                    false,
                ),
            ));
            app
        }

        fn saved(dir: &tempfile::TempDir) -> AppConfig {
            config::load_config_from(&dir.path().join("config.toml")).unwrap()
        }

        /// Verifies: REQ-AUD-107
        /// Verifies: REQ-GUI-024
        #[tokio::test]
        async fn a_change_during_a_session_is_saved_reaches_the_session_and_is_announced() {
            let dir = tempfile::tempdir().unwrap();
            let app = app(&dir, AppConfig::default());
            let session = app
                .state::<StreamingState>()
                .attach_session_for_test()
                .await;
            let announced = Arc::new(StdMutex::new(Vec::<AudioSettings>::new()));
            let heard = announced.clone();
            app.listen_any(CHANGED_EVENT, move |event| {
                heard
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(event.payload()).unwrap());
            });

            let settings = apply(app.handle(), &SettingChange::TransmitChannels { count: 1 })
                .await
                .unwrap();

            assert_eq!(settings.transmit_channels, 1);
            assert_eq!(saved(&dir).transmit_channels, 1);
            assert_eq!(
                session.try_recv().unwrap(),
                StreamingCommand::SetTransmitChannels(1)
            );
            assert_eq!(*announced.lock().unwrap(), vec![settings]);
        }

        /// The failure of the ticket: the driver stops answering, and every
        /// read and change of the settings waited for it, one listing at a time.
        ///
        /// Verifies: REQ-AUD-123
        #[tokio::test]
        async fn when_the_audio_driver_hangs_the_settings_still_read_and_change_within_the_limit() {
            use jamjam::audio::fault::{self, Call};
            use std::time::{Duration, Instant};

            let dir = tempfile::tempdir().unwrap();
            let app = app(&dir, AppConfig::default());
            // The listing the settings fall back on once the driver hangs
            let before = current(app.handle()).await.unwrap();
            fault::stall(Call::ListInputs, Duration::from_secs(10));
            fault::stall(Call::ListOutputs, Duration::from_secs(10));
            let started = Instant::now();

            let hung = current(app.handle()).await.unwrap();
            let changed = apply(app.handle(), &SettingChange::TransmitChannels { count: 1 })
                .await
                .unwrap();
            let read_again = current(app.handle()).await.unwrap();

            fault::stall(Call::ListInputs, Duration::ZERO);
            fault::stall(Call::ListOutputs, Duration::ZERO);
            assert_eq!(hung.input_devices, before.input_devices);
            assert_eq!(changed.transmit_channels, 1);
            assert_eq!(saved(&dir).transmit_channels, 1);
            assert_eq!(read_again.transmit_channels, 1);
            assert!(
                started.elapsed() < Duration::from_secs(6),
                "the settings waited {:?} for a driver that hangs for 10 s",
                started.elapsed()
            );
        }

        /// Verifies: REQ-AUD-107
        #[tokio::test]
        async fn a_change_with_no_session_running_is_saved_and_no_session_is_told() {
            let dir = tempfile::tempdir().unwrap();
            let app = app(&dir, AppConfig::default());
            let session = app
                .state::<StreamingState>()
                .detached_session_for_test()
                .await;

            apply(app.handle(), &SettingChange::TransmitChannels { count: 1 })
                .await
                .unwrap();

            assert_eq!(saved(&dir).transmit_channels, 1);
            assert!(session.try_recv().is_err(), "no session was running");
        }

        /// Verifies: REQ-GUI-024
        #[tokio::test]
        async fn a_refused_change_leaves_the_saved_settings_the_session_and_the_revision_alone() {
            let dir = tempfile::tempdir().unwrap();
            let app = app(&dir, AppConfig::default());
            let session = app
                .state::<StreamingState>()
                .attach_session_for_test()
                .await;
            let before = current(app.handle()).await.unwrap();

            let result = apply(app.handle(), &SettingChange::TransmitChannels { count: 3 }).await;

            assert_eq!(result, Err(SettingsError::InvalidTransmitChannels(3)));
            assert!(
                !dir.path().join("config.toml").exists(),
                "nothing was saved"
            );
            assert!(session.try_recv().is_err(), "the session was not told");
            assert_eq!(current(app.handle()).await.unwrap(), before);
        }

        /// A change that cannot be saved is not in effect either: the next
        /// connect must not use a value the file does not hold.
        ///
        /// Verifies: REQ-GUI-024
        #[tokio::test]
        async fn a_change_that_cannot_be_saved_is_not_in_effect() {
            let dir = tempfile::tempdir().unwrap();
            let blocked = dir.path().join("not-a-directory");
            std::fs::write(&blocked, "").unwrap();
            let app = tauri::test::mock_app();
            app.manage(ConfigState::at(
                blocked.join("config.toml"),
                AppConfig::default(),
            ));
            app.manage(SettingsState::new());
            app.manage(StreamingState::new());

            let result = apply(app.handle(), &SettingChange::BufferSize { samples: 32 }).await;

            assert!(matches!(result, Err(SettingsError::Unavailable(_))));
            assert_eq!(
                app.state::<ConfigState>().get().unwrap().buffer_size,
                AppConfig::default().buffer_size
            );
        }

        #[tokio::test]
        async fn every_applied_change_carries_a_later_revision() {
            let dir = tempfile::tempdir().unwrap();
            let app = app(&dir, AppConfig::default());

            let first = apply(app.handle(), &SettingChange::BufferSize { samples: 32 })
                .await
                .unwrap();
            let second = apply(app.handle(), &SettingChange::SampleRate { hz: 96000 })
                .await
                .unwrap();

            assert!(second.revision > first.revision);
            assert_eq!(
                current(app.handle()).await.unwrap().revision,
                second.revision
            );
        }
    }
}
