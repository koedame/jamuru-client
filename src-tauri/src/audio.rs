//! Audio device IPC for Tauri
//!
//! Lists the audio devices and reports the selection in effect. Choosing a
//! device, like every other audio setting, goes through
//! [`crate::settings::settings_change`] (ADR-043).

use serde::{Deserialize, Serialize};

use jamjam::audio::{list_input_devices, list_output_devices};

use crate::config::ConfigState;

/// Audio device information for IPC
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub supported_sample_rates: Vec<u32>,
    pub supported_channels: Vec<u16>,
    pub is_default: bool,
    pub is_asio: bool,
}

/// Current device selection
#[derive(Debug, Clone, Serialize)]
pub struct CurrentDevices {
    pub input_device_id: Option<String>,
    pub output_device_id: Option<String>,
}

/// Available input (microphone) devices
pub fn input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    let devices = list_input_devices().map_err(|e| {
        tracing::error!("Listing input devices failed: {}", e);
        e.to_string()
    })?;

    let devices: Vec<AudioDeviceInfo> = devices
        .into_iter()
        .map(|d| AudioDeviceInfo {
            id: d.id.0,
            name: d.name,
            supported_sample_rates: d.supported_sample_rates,
            supported_channels: d.supported_channels,
            is_default: d.is_default,
            is_asio: d.is_asio,
        })
        .collect();
    log_devices("input", &devices);
    Ok(devices)
}

/// Available output (speaker) devices
pub fn output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    let devices = list_output_devices().map_err(|e| {
        tracing::error!("Listing output devices failed: {}", e);
        e.to_string()
    })?;

    let devices: Vec<AudioDeviceInfo> = devices
        .into_iter()
        .map(|d| AudioDeviceInfo {
            id: d.id.0,
            name: d.name,
            supported_sample_rates: d.supported_sample_rates,
            supported_channels: d.supported_channels,
            is_default: d.is_default,
            is_asio: d.is_asio,
        })
        .collect();
    log_devices("output", &devices);
    Ok(devices)
}

/// What the OS offered, so a report can show "no interface was found" apart
/// from "the interface was found but not selected".
fn log_devices(kind: &str, devices: &[AudioDeviceInfo]) {
    let names: Vec<String> = devices
        .iter()
        .map(|d| {
            if d.is_default {
                format!("{} (default)", d.name)
            } else {
                d.name.clone()
            }
        })
        .collect();
    tracing::info!(
        "Found {} {} device(s): {}",
        devices.len(),
        kind,
        names.join(", ")
    );
}

/// The device selection in effect, as saved in `config.toml` (ADR-026)
#[tauri::command]
pub fn audio_get_current_devices(
    state: tauri::State<'_, ConfigState>,
) -> Result<CurrentDevices, String> {
    let config = state.get()?;
    Ok(CurrentDevices {
        input_device_id: config.input_device_id,
        output_device_id: config.output_device_id,
    })
}

/// The buffer size in effect (frame size, in samples), as saved in `config.toml`
#[tauri::command]
pub fn audio_get_buffer_size(state: tauri::State<'_, ConfigState>) -> Result<u32, String> {
    Ok(state.get()?.buffer_size)
}
