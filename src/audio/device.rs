//! Audio device enumeration and management

use cpal::traits::{DeviceTrait, HostTrait};

use super::error::AudioError;

/// Unique identifier for an audio device
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

/// Identifier that survives restarts and reconnections: cpal's `DeviceId`
/// in its persistable form (for example `alsa:hw:0,0`).
///
/// Not the display name: on ALSA several devices share one name, so the name
/// cannot pick a device.
pub(crate) fn stable_device_id(device: &cpal::Device) -> Option<String> {
    device.id().ok().map(|id| id.to_string())
}

/// Name to show the user for a device
pub(crate) fn display_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// Information about an audio device
#[derive(Debug, Clone)]
pub struct AudioDevice {
    /// Device identifier
    pub id: DeviceId,
    /// Display name
    pub name: String,
    /// Supported sample rates (Hz)
    pub supported_sample_rates: Vec<u32>,
    /// Channel counts the device offers in this direction. Empty when the
    /// device reports none (unknown, not zero).
    pub supported_channels: Vec<u16>,
    /// Whether this is the default device
    pub is_default: bool,
    /// ASIO support (Windows only)
    pub is_asio: bool,
    /// Whether the device is a Bluetooth one (see [`is_bluetooth`])
    pub is_bluetooth: bool,
}

/// List available input (capture) devices
///
/// Returns all available audio input devices on the system.
///
/// # Errors
/// Returns `AudioError::DeviceOpenFailed` if device enumeration fails.
pub fn list_input_devices() -> Result<Vec<AudioDevice>, AudioError> {
    super::fault::point(super::fault::Call::ListInputs);
    let host = cpal::default_host();
    let default_device = host.default_input_device();
    let default_id = default_device.as_ref().and_then(stable_device_id);

    let devices = host.input_devices().map_err(|e| {
        AudioError::DeviceOpenFailed(format!("Failed to enumerate input devices: {}", e))
    })?;

    let mut result: Vec<AudioDevice> = devices
        .filter_map(|device| {
            let id = stable_device_id(&device)?;
            let name = display_name(&device)?;
            let is_default = default_id.as_ref() == Some(&id);
            let (sample_rates, channels) =
                capabilities(device.supported_input_configs().into_iter().flatten());
            Some(AudioDevice {
                id: DeviceId(id),
                is_bluetooth: is_bluetooth(&device, &name),
                name,
                supported_sample_rates: sample_rates,
                supported_channels: channels,
                is_default,
                is_asio: is_asio_device(&device),
            })
        })
        .collect();
    result.extend(asio_pseudo_devices());

    Ok(result)
}

/// List available output (playback) devices
///
/// Returns all available audio output devices on the system.
///
/// # Errors
/// Returns `AudioError::DeviceOpenFailed` if device enumeration fails.
pub fn list_output_devices() -> Result<Vec<AudioDevice>, AudioError> {
    super::fault::point(super::fault::Call::ListOutputs);
    let host = cpal::default_host();
    let default_device = host.default_output_device();
    let default_id = default_device.as_ref().and_then(stable_device_id);

    let devices = host.output_devices().map_err(|e| {
        AudioError::DeviceOpenFailed(format!("Failed to enumerate output devices: {}", e))
    })?;

    let mut result: Vec<AudioDevice> = devices
        .filter_map(|device| {
            let id = stable_device_id(&device)?;
            let name = display_name(&device)?;
            let is_default = default_id.as_ref() == Some(&id);
            let (sample_rates, channels) =
                capabilities(device.supported_output_configs().into_iter().flatten());
            Some(AudioDevice {
                id: DeviceId(id),
                is_bluetooth: is_bluetooth(&device, &name),
                name,
                supported_sample_rates: sample_rates,
                supported_channels: channels,
                is_default,
                is_asio: is_asio_device(&device),
            })
        })
        .collect();
    result.extend(asio_pseudo_devices());

    Ok(result)
}

/// Finds the input device streaming actually uses: the one matching
/// `device_id`, or the OS default when `device_id` is `None`. The
/// diagnostics tab resolves through this same function so it can't report on
/// a device other than the one streaming would open.
pub fn resolve_input_device(device_id: Option<&DeviceId>) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    match device_id {
        Some(id) => host
            .input_devices()
            .map_err(|e| AudioError::DeviceOpenFailed(e.to_string()))?
            .find(|d| stable_device_id(d).as_ref() == Some(&id.0))
            .ok_or_else(|| AudioError::DeviceNotFound(id.0.clone())),
        None => host
            .default_input_device()
            .ok_or_else(|| AudioError::DeviceNotFound("No default input device".into())),
    }
}

/// Finds the output device streaming actually uses. Same reasoning as
/// `resolve_input_device`.
pub fn resolve_output_device(device_id: Option<&DeviceId>) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    match device_id {
        Some(id) => host
            .output_devices()
            .map_err(|e| AudioError::DeviceOpenFailed(e.to_string()))?
            .find(|d| stable_device_id(d).as_ref() == Some(&id.0))
            .ok_or_else(|| AudioError::DeviceNotFound(id.0.clone())),
        None => host
            .default_output_device()
            .ok_or_else(|| AudioError::DeviceNotFound("No default output device".into())),
    }
}

/// Channel counts `device` offers for input (`input`) or output at
/// `sample_rate`. Empty when the device cannot say.
pub(crate) fn offered_channel_counts(
    device: &cpal::Device,
    input: bool,
    sample_rate: u32,
) -> Vec<u16> {
    let mut counts = Vec::new();
    let mut note = |config: cpal::SupportedStreamConfigRange| {
        if config.min_sample_rate() <= sample_rate && sample_rate <= config.max_sample_rate() {
            counts.push(config.channels());
        }
    };
    if input {
        device
            .supported_input_configs()
            .into_iter()
            .flatten()
            .for_each(&mut note);
    } else {
        device
            .supported_output_configs()
            .into_iter()
            .flatten()
            .for_each(&mut note);
    }
    counts
}

/// The sample rate to open `input` devices at: `wanted` when the device offers
/// it with at least `needed` channels, otherwise the offered rate nearest to
/// it (the higher one when two are equally near). A device that does not say
/// what it offers gets `wanted` and is left to refuse.
///
/// Windows opens a capture stream in shared mode only in the format the
/// system's sound settings give the device, which cheap interfaces often set
/// to 44100 Hz, so the session's rate is not always one the device opens at.
pub(crate) fn input_open_rate(device: &cpal::Device, wanted: u32, needed: usize) -> u32 {
    rate_to_open(
        device.supported_input_configs().into_iter().flatten(),
        needed,
        wanted,
    )
}

/// The sample rate to open `output` devices at: the rate the device runs at
/// now where opening a stream at another rate would rewrite the device's own
/// clock, otherwise what [`input_open_rate`] picks for an input.
///
/// macOS sets the device's nominal rate to the stream's and waits for the
/// change. A Bluetooth headset in its call mode (AirPods while their
/// microphone is in use) is fixed at 24000 Hz or so, never confirms the
/// change, and the open fails after a second. The rate is also the device's
/// for every other program, so it is left alone.
pub(crate) fn output_open_rate(device: &cpal::Device, wanted: u32, needed: usize) -> u32 {
    let current = device
        .default_output_config()
        .ok()
        .map(|config| config.sample_rate());
    pick_output_rate(
        rate_to_open(
            device.supported_output_configs().into_iter().flatten(),
            needed,
            wanted,
        ),
        current,
        cfg!(target_os = "macos"),
    )
}

fn pick_output_rate(offered: u32, current: Option<u32>, rewrites_device_rate: bool) -> u32 {
    match current {
        Some(current) if rewrites_device_rate => current,
        _ => offered,
    }
}

fn rate_to_open(
    configs: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
    needed: usize,
    wanted: u32,
) -> u32 {
    let mut nearest: Option<u32> = None;
    for config in configs
        .into_iter()
        .filter(|config| config.channels() as usize >= needed)
    {
        let rate = wanted.clamp(config.min_sample_rate(), config.max_sample_rate());
        if rate == wanted {
            return wanted;
        }
        nearest = nearest
            .into_iter()
            .chain([rate])
            .min_by_key(|rate| (rate.abs_diff(wanted), u32::MAX - rate));
    }
    nearest.unwrap_or(wanted)
}

/// Sample rates and channel counts offered by `configs`, the stream
/// configurations of one direction of a device.
///
/// Only that direction: an interface with 2 inputs and 8 outputs must not
/// offer input channel 7. A device that reports nothing (one that is busy, or
/// whose driver cannot say) gets no channel counts - unknown, not stereo - so
/// a channel chosen earlier is not refused on a guess.
fn capabilities(
    configs: impl IntoIterator<Item = cpal::SupportedStreamConfigRange>,
) -> (Vec<u32>, Vec<u16>) {
    let mut sample_rates = Vec::new();
    let mut channels = Vec::new();

    for config in configs {
        // Add common sample rates that fall within the supported range
        for rate in &[44100u32, 48000, 96000, 192000] {
            if *rate >= config.min_sample_rate()
                && *rate <= config.max_sample_rate()
                && !sample_rates.contains(rate)
            {
                sample_rates.push(*rate);
            }
        }
        let ch = config.channels();
        if !channels.contains(&ch) {
            channels.push(ch);
        }
    }

    sample_rates.sort();
    channels.sort();

    // Provide defaults if nothing was detected
    if sample_rates.is_empty() {
        sample_rates = vec![44100, 48000];
    }

    (sample_rates, channels)
}

/// Whether `device` is a Bluetooth one, which adds tens of milliseconds of
/// its own and drops to a narrow-band call mode while its microphone is in
/// use, so it does not suit a session that is after the lowest latency and
/// the best quality.
///
/// `cpal` names the connection on Windows (WASAPI), PipeWire and Android, but
/// not on macOS, where it leaves AirPods as an unknown interface. There
/// CoreAudio is asked for the device's transport type; the device's name is
/// the last resort for a device whose transport cannot be read.
pub(crate) fn is_bluetooth(device: &cpal::Device, name: &str) -> bool {
    let interface = device
        .description()
        .map(|description| description.interface_type())
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    if let Ok(id) = device.id() {
        match super::transport::is_bluetooth(id.id()) {
            Some(bluetooth) => return bluetooth,
            None => tracing::warn!("{}", transport_unknown_warning(interface)),
        }
    }
    looks_bluetooth(interface, name)
}

/// The log line for a device whose transport type CoreAudio would not give.
///
/// It names neither the device's ID (a Bluetooth device's UID looks like its
/// address) nor its name (it may hold its owner's): the line is written while
/// the devices are being listed, before the log's redaction knows them, so
/// whatever it held would reach the file, and the report, as it is.
#[cfg(any(target_os = "macos", test))]
fn transport_unknown_warning(interface: cpal::InterfaceType) -> String {
    format!(
        "CoreAudio did not report the transport type of a device (cpal: {interface:?}); going by the device name"
    )
}

/// Whether a CoreAudio transport type (`kAudioDevicePropertyTransportType`,
/// a four-character code) is Bluetooth or Bluetooth LE.
#[cfg(any(target_os = "macos", test))]
pub(super) fn is_bluetooth_transport(transport: u32) -> bool {
    const BLUETOOTH: u32 = u32::from_be_bytes(*b"blue");
    const BLUETOOTH_LE: u32 = u32::from_be_bytes(*b"blea");
    transport == BLUETOOTH || transport == BLUETOOTH_LE
}

fn looks_bluetooth(interface: cpal::InterfaceType, name: &str) -> bool {
    match interface {
        cpal::InterfaceType::Bluetooth => true,
        cpal::InterfaceType::Unknown => {
            let name = name.to_lowercase();
            BLUETOOTH_NAME_MARKS.iter().any(|mark| name.contains(mark))
        }
        _ => false,
    }
}

/// Lower-case parts of the names Bluetooth audio devices go by when the
/// system does not say how they are connected (macOS when CoreAudio cannot be
/// read, and any host that reports an unknown interface).
const BLUETOOTH_NAME_MARKS: &[&str] = &["bluetooth", "airpods", "hands-free", "handsfree", "beats"];

/// Check if device is an ASIO device (Windows only)
#[cfg(target_os = "windows")]
fn is_asio_device(device: &cpal::Device) -> bool {
    display_name(device).is_some_and(|n| n.contains("ASIO"))
}

#[cfg(not(target_os = "windows"))]
fn is_asio_device(_device: &cpal::Device) -> bool {
    false
}

/// ASIO drivers, one [`AudioDevice`] each, usable as both an input and an
/// output device (see `asio::list_drivers`). `cpal` never sees these: on
/// Windows it only opens WASAPI, so this is the only place they are listed.
#[cfg(target_os = "windows")]
fn asio_pseudo_devices() -> Vec<AudioDevice> {
    super::asio::list_drivers()
}

#[cfg(not(target_os = "windows"))]
fn asio_pseudo_devices() -> Vec<AudioDevice> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Device enumeration must not panic, and whatever it returns must be
    /// usable: a device with an empty id cannot be selected later.
    ///
    /// The list itself may legitimately be empty (CI runners have no audio
    /// hardware), so its length is not asserted.
    #[test]
    fn test_listed_devices_are_selectable() {
        let inputs = list_input_devices().unwrap_or_default();
        let outputs = list_output_devices().unwrap_or_default();

        for device in inputs.iter().chain(outputs.iter()) {
            assert!(
                !device.id.0.is_empty(),
                "device {:?} has no id and could not be selected",
                device
            );
            assert!(
                !device.name.is_empty(),
                "device {:?} has no name to show the user",
                device
            );
        }
    }

    fn config(channels: u16) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(
            channels,
            44100,
            48000,
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )
    }

    #[test]
    fn a_direction_offers_the_channel_counts_of_its_own_configs() {
        let (_, channels) = capabilities([config(8), config(2), config(8)]);
        assert_eq!(channels, vec![2, 8]);
    }

    /// A device that cannot say what it offers must not look like a stereo
    /// one: the settings would then refuse a channel chosen on it earlier.
    #[test]
    fn a_device_that_reports_no_configs_has_unknown_channel_counts() {
        let (_, channels) = capabilities([]);
        assert!(channels.is_empty());
    }

    fn config_at(channels: u16, min: u32, max: u32) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(
            channels,
            min,
            max,
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        )
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn when_the_device_offers_the_wanted_rate_it_is_opened_at_that_rate() {
        assert_eq!(rate_to_open([config_at(2, 44100, 96000)], 2, 48000), 48000);
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn when_the_device_offers_only_44100_it_is_opened_at_44100() {
        assert_eq!(rate_to_open([config_at(2, 44100, 44100)], 2, 48000), 44100);
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn when_two_offered_rates_are_equally_near_the_higher_one_is_used() {
        let configs = [config_at(2, 44000, 44000), config_at(2, 52000, 52000)];
        assert_eq!(rate_to_open(configs, 2, 48000), 52000);
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_opening_would_rewrite_the_device_rate_the_output_opens_at_the_rate_it_runs_at() {
        assert_eq!(pick_output_rate(48000, Some(24000), true), 24000);
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_the_device_already_runs_at_the_wanted_rate_the_output_opens_at_it() {
        assert_eq!(pick_output_rate(48000, Some(48000), true), 48000);
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_opening_does_not_rewrite_the_device_rate_the_output_opens_at_an_offered_rate() {
        assert_eq!(pick_output_rate(48000, Some(44100), false), 48000);
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_the_device_does_not_say_its_rate_the_output_opens_at_an_offered_rate() {
        assert_eq!(pick_output_rate(44100, None, true), 44100);
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn a_rate_offered_only_with_too_few_channels_is_not_chosen() {
        let configs = [config_at(2, 48000, 48000), config_at(8, 44100, 44100)];
        assert_eq!(rate_to_open(configs, 6, 48000), 44100);
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn a_device_that_offers_nothing_is_asked_for_the_wanted_rate() {
        assert_eq!(rate_to_open([], 2, 48000), 48000);
    }

    /// Verifies: REQ-GUI-026
    #[test]
    fn when_the_system_says_the_device_is_bluetooth_it_is_bluetooth() {
        assert!(looks_bluetooth(
            cpal::InterfaceType::Bluetooth,
            "Headphones"
        ));
    }

    /// Verifies: REQ-GUI-026
    #[test]
    fn when_coreaudio_says_the_transport_is_bluetooth_or_bluetooth_le_it_is_bluetooth() {
        assert!(is_bluetooth_transport(0x626c_7565)); // 'blue'
        assert!(is_bluetooth_transport(0x626c_6561)); // 'blea'
    }

    /// Verifies: REQ-GUI-026
    #[test]
    fn when_coreaudio_says_the_transport_is_usb_built_in_or_virtual_it_is_not_bluetooth() {
        assert!(!is_bluetooth_transport(0x7573_6220)); // 'usb '
        assert!(!is_bluetooth_transport(0x626c_746e)); // 'bltn'
        assert!(!is_bluetooth_transport(0x7669_7274)); // 'virt'
        assert!(!is_bluetooth_transport(0));
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn when_coreaudio_does_not_report_the_transport_the_log_line_names_no_device() {
        let line = transport_unknown_warning(cpal::InterfaceType::Unknown);

        assert!(line.contains("Unknown"), "{line}");
        assert!(!line.contains("coreaudio:"), "{line}");
    }

    /// Verifies: REQ-GUI-026
    #[test]
    fn when_the_system_does_not_say_but_the_name_is_airpods_the_device_is_bluetooth() {
        assert!(looks_bluetooth(
            cpal::InterfaceType::Unknown,
            "Taro's AirPods Pro"
        ));
        assert!(looks_bluetooth(
            cpal::InterfaceType::Unknown,
            "Hands-Free AG Audio"
        ));
    }

    /// Verifies: REQ-GUI-026
    #[test]
    fn when_the_system_says_the_device_is_usb_a_bluetooth_looking_name_does_not_make_it_bluetooth()
    {
        assert!(!looks_bluetooth(
            cpal::InterfaceType::Usb,
            "Bluetooth Dongle Interface"
        ));
    }

    /// Verifies: REQ-GUI-026
    #[test]
    fn when_the_device_is_unknown_and_its_name_is_ordinary_it_is_not_bluetooth() {
        assert!(!looks_bluetooth(
            cpal::InterfaceType::Unknown,
            "Scarlett 2i2 USB"
        ));
        assert!(!looks_bluetooth(
            cpal::InterfaceType::Unknown,
            "MacBook Pro Microphone"
        ));
    }

    /// A device id that no longer exists must be an error, not a silent
    /// fallback to the OS default - otherwise a disconnected configured
    /// device would look like it's still in use.
    #[test]
    fn test_resolve_input_device_errors_on_unknown_id_instead_of_falling_back() {
        let unknown = DeviceId("this-device-id-does-not-exist".to_string());
        assert!(resolve_input_device(Some(&unknown)).is_err());
    }

    #[test]
    fn test_resolve_output_device_errors_on_unknown_id_instead_of_falling_back() {
        let unknown = DeviceId("this-device-id-does-not-exist".to_string());
        assert!(resolve_output_device(Some(&unknown)).is_err());
    }
}
