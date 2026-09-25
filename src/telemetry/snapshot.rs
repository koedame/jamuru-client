//! What the machine and its audio devices look like right now, as the
//! `app_start` and `audio_env` events.
//!
//! The facts come from `crate::environment` (OS version, installed memory,
//! audio host, a device's smallest buffer) and from what the app already
//! reads. A fact the platform cannot tell is left out of the event: one
//! missing fact never costs the others.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::audio::{resolve_input_device, resolve_output_device, DeviceId};
use crate::config::{AppConfig, DEFAULT_SERVER_URL, VALID_SAMPLE_RATES};
use crate::environment;

use super::event::{AppStart, AudioEnv, AudioHost, Device, DeviceKind};
use super::settings::settings_for_report;

/// The `app_start` for a launch with `config`.
pub fn app_start(config: &AppConfig) -> AppStart {
    AppStart {
        os_version: environment::os_version(),
        cpu_cores: std::thread::available_parallelism()
            .ok()
            .and_then(|cores| u32::try_from(cores.get()).ok()),
        ram_gb: environment::ram_gb(),
        audio_host: Some(audio_host_of(&environment::audio_host())),
        language: config.language.clone(),
        server_is_default: Some(config.effective_server_url() == DEFAULT_SERVER_URL),
        settings: settings_for_report(config),
        ..AppStart::default()
    }
}

/// The `audio_env` for the devices in use: the ones `input_id` and
/// `output_id` name, or the OS default for a side with no id. A side whose
/// device cannot be found is `None`.
pub fn audio_env(input_id: Option<&str>, output_id: Option<&str>) -> AudioEnv {
    let host = cpal::default_host();
    let default_input = host.default_input_device();
    let default_output = host.default_output_device();
    AudioEnv {
        input_id: input_id.map(str::to_string),
        output_id: output_id.map(str::to_string),
        input: resolve_input_device(input_id.map(|id| DeviceId(id.to_string())).as_ref())
            .ok()
            .and_then(|device| describe(&device, Direction::Input, default_input.as_ref())),
        output: resolve_output_device(output_id.map(|id| DeviceId(id.to_string())).as_ref())
            .ok()
            .and_then(|device| describe(&device, Direction::Output, default_output.as_ref())),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Input,
    Output,
}

/// `default` is the OS default device for the same direction; the device is
/// the default one when it has that device's name.
fn describe(
    device: &cpal::Device,
    direction: Direction,
    default: Option<&cpal::Device>,
) -> Option<Device> {
    let description = device.description().ok()?;
    let name = description.name().to_string();
    if name.is_empty() {
        return None;
    }
    let is_default = default
        .and_then(|default| default.description().ok())
        .is_some_and(|default| default.name() == name);

    let min_buffer_frames = match direction {
        Direction::Input => environment::min_input_buffer_frames(device),
        Direction::Output => environment::min_output_buffer_frames(device),
    };
    let mut channels = 0u32;
    let mut sample_rates: Vec<u32> = Vec::new();
    let mut note = |config: cpal::SupportedStreamConfigRange| {
        channels = channels.max(u32::from(config.channels()));
        for rate in VALID_SAMPLE_RATES {
            if (config.min_sample_rate()..=config.max_sample_rate()).contains(&rate)
                && !sample_rates.contains(&rate)
            {
                sample_rates.push(rate);
            }
        }
    };
    match direction {
        Direction::Input => device
            .supported_input_configs()
            .into_iter()
            .flatten()
            .for_each(&mut note),
        Direction::Output => device
            .supported_output_configs()
            .into_iter()
            .flatten()
            .for_each(&mut note),
    }
    sample_rates.sort_unstable();

    Some(Device {
        name,
        kind: kind_of(&description),
        channels: channels.max(1),
        sample_rates,
        min_buffer_frames,
        is_default,
    })
}

/// The schema's name for the audio host `cpal` reports.
fn audio_host_of(name: &str) -> AudioHost {
    match name.to_ascii_lowercase().as_str() {
        "coreaudio" => AudioHost::Coreaudio,
        "wasapi" => AudioHost::Wasapi,
        "asio" => AudioHost::Asio,
        "alsa" => AudioHost::Alsa,
        "jack" => AudioHost::Jack,
        _ => AudioHost::Other,
    }
}

fn kind_of(description: &cpal::DeviceDescription) -> DeviceKind {
    if description.device_type() == cpal::DeviceType::Virtual {
        return DeviceKind::Virtual;
    }
    match description.interface_type() {
        cpal::InterfaceType::Usb => DeviceKind::Usb,
        cpal::InterfaceType::BuiltIn => DeviceKind::Builtin,
        cpal::InterfaceType::Bluetooth => DeviceKind::Bluetooth,
        _ => DeviceKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_cpal_names_a_known_audio_host_the_schema_name_is_used() {
        for (reported, expected) in [
            ("CoreAudio", AudioHost::Coreaudio),
            ("coreaudio", AudioHost::Coreaudio),
            ("WASAPI", AudioHost::Wasapi),
            ("ASIO", AudioHost::Asio),
            ("alsa", AudioHost::Alsa),
            ("JACK", AudioHost::Jack),
        ] {
            assert_eq!(audio_host_of(reported), expected, "{reported}");
        }
    }

    #[test]
    fn when_cpal_names_an_unknown_audio_host_it_is_reported_as_other() {
        assert_eq!(audio_host_of("Emscripten"), AudioHost::Other);
        assert_eq!(audio_host_of(""), AudioHost::Other);
    }

    #[test]
    fn when_the_machine_is_described_the_environment_facts_are_filled_in() {
        let start = app_start(&AppConfig::default());

        assert!(start.cpu_cores.is_some_and(|cores| cores >= 1));
        assert!(start.audio_host.is_some());
        // Whatever the platform could tell is a power of two, or absent.
        if let Some(ram) = start.ram_gb {
            assert!(ram.is_power_of_two(), "{ram}");
        }
        // The version keeps at most major.minor.
        if let Some(version) = start.os_version {
            assert!(version.split('.').count() <= 2, "{version}");
        }
    }
}
