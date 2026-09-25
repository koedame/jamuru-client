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
}

/// List available input (capture) devices
///
/// Returns all available audio input devices on the system.
///
/// # Errors
/// Returns `AudioError::DeviceOpenFailed` if device enumeration fails.
pub fn list_input_devices() -> Result<Vec<AudioDevice>, AudioError> {
    let host = cpal::default_host();
    let default_device = host.default_input_device();
    let default_id = default_device.as_ref().and_then(stable_device_id);

    let devices = host.input_devices().map_err(|e| {
        AudioError::DeviceOpenFailed(format!("Failed to enumerate input devices: {}", e))
    })?;

    let result = devices
        .filter_map(|device| {
            let id = stable_device_id(&device)?;
            let name = display_name(&device)?;
            let is_default = default_id.as_ref() == Some(&id);
            let (sample_rates, channels) =
                capabilities(device.supported_input_configs().into_iter().flatten());
            Some(AudioDevice {
                id: DeviceId(id),
                name,
                supported_sample_rates: sample_rates,
                supported_channels: channels,
                is_default,
                is_asio: is_asio_device(&device),
            })
        })
        .collect();

    Ok(result)
}

/// List available output (playback) devices
///
/// Returns all available audio output devices on the system.
///
/// # Errors
/// Returns `AudioError::DeviceOpenFailed` if device enumeration fails.
pub fn list_output_devices() -> Result<Vec<AudioDevice>, AudioError> {
    let host = cpal::default_host();
    let default_device = host.default_output_device();
    let default_id = default_device.as_ref().and_then(stable_device_id);

    let devices = host.output_devices().map_err(|e| {
        AudioError::DeviceOpenFailed(format!("Failed to enumerate output devices: {}", e))
    })?;

    let result = devices
        .filter_map(|device| {
            let id = stable_device_id(&device)?;
            let name = display_name(&device)?;
            let is_default = default_id.as_ref() == Some(&id);
            let (sample_rates, channels) =
                capabilities(device.supported_output_configs().into_iter().flatten());
            Some(AudioDevice {
                id: DeviceId(id),
                name,
                supported_sample_rates: sample_rates,
                supported_channels: channels,
                is_default,
                is_asio: is_asio_device(&device),
            })
        })
        .collect();

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

/// Check if device is an ASIO device (Windows only)
#[cfg(target_os = "windows")]
fn is_asio_device(device: &cpal::Device) -> bool {
    display_name(device).is_some_and(|n| n.contains("ASIO"))
}

#[cfg(not(target_os = "windows"))]
fn is_asio_device(_device: &cpal::Device) -> bool {
    false
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
