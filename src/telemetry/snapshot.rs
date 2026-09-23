//! What the machine and its audio devices look like right now, as the
//! `app_start` and `audio_env` events.
//!
//! Only what the app already reads is taken here. The operating system's
//! version, installed memory, the audio host's name and a device's smallest
//! buffer are left out of the events until the environment probe provides
//! them.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::config::{AppConfig, VALID_SAMPLE_RATES};

use super::event::{AppStart, AudioEnv, Device, DeviceKind};
use super::settings::settings_for_report;

/// The `app_start` for a launch with `config`.
pub fn app_start(config: &AppConfig) -> AppStart {
    AppStart {
        cpu_cores: std::thread::available_parallelism()
            .ok()
            .and_then(|cores| u32::try_from(cores.get()).ok()),
        language: config.language.clone(),
        settings: settings_for_report(config),
        ..AppStart::default()
    }
}

/// The `audio_env` for the devices the OS has as default.
pub fn audio_env() -> AudioEnv {
    let host = cpal::default_host();
    AudioEnv {
        input: host
            .default_input_device()
            .and_then(|device| describe(&device, Direction::Input)),
        output: host
            .default_output_device()
            .and_then(|device| describe(&device, Direction::Output)),
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Input,
    Output,
}

fn describe(device: &cpal::Device, direction: Direction) -> Option<Device> {
    let description = device.description().ok()?;
    let name = description.name().to_string();
    if name.is_empty() {
        return None;
    }

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
        min_buffer_frames: None,
        is_default: true,
    })
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
