//! Audio IPC commands for Tauri
//!
//! Provides commands to list and select audio devices.

use std::sync::Mutex;

use serde::Serialize;

use jamjam::audio::{list_input_devices, list_output_devices};

/// Audio state managed by Tauri
///
/// Note: AudioEngine is not stored here because cpal's Stream is not Send+Sync.
/// This state only tracks the selected device IDs and buffer settings.
/// Actual audio engine initialization will be done separately when needed.
pub struct AudioState {
    current_input_id: Mutex<Option<String>>,
    current_output_id: Mutex<Option<String>>,
    /// Buffer size (frame_size) in samples. Valid values: 32, 64, 128, 256
    buffer_size: Mutex<u32>,
}

impl AudioState {
    /// Restores the device selection saved in `config.toml`.
    ///
    /// Without this the saved selection is display-only: the settings panel
    /// reads it to fill its dropdowns, but only pushes a device into this
    /// state when none is saved (`SettingsPanelAdapter`'s `if (!inputId)`), so
    /// after a restart streaming asks for "no device" and silently captures
    /// from the OS default instead of the interface the user chose. Seeding at
    /// startup makes the saved choice take effect everywhere (ADR-026).
    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        Self {
            current_input_id: Mutex::new(config.input_device_id.clone()),
            current_output_id: Mutex::new(config.output_device_id.clone()),
            buffer_size: Mutex::new(config.buffer_size),
        }
    }

    pub fn new() -> Self {
        Self {
            current_input_id: Mutex::new(None),
            current_output_id: Mutex::new(None),
            buffer_size: Mutex::new(64), // Default: 64 samples @ 48kHz = 1.33ms
        }
    }
}

impl Default for AudioState {
    fn default() -> Self {
        Self::new()
    }
}

/// Audio device information for IPC
#[derive(Debug, Clone, Serialize)]
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

/// List available input (microphone) devices
#[tauri::command]
pub fn audio_list_input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
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

/// List available output (speaker) devices
#[tauri::command]
pub fn audio_list_output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
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

/// Set the input device
///
/// Note: This currently only stores the device ID selection.
/// Actual audio engine device switching will be implemented when
/// the audio pipeline is integrated.
#[tauri::command]
pub fn audio_set_input_device(
    device_id: Option<String>,
    state: tauri::State<'_, AudioState>,
    usage: tauri::State<'_, crate::usage::UsageState>,
) -> Result<(), String> {
    let mut current = state.current_input_id.lock().map_err(|e| e.to_string())?;
    tracing::info!("Input device selected: {:?}", device_id);
    *current = device_id.clone();
    drop(current);

    let output = state.current_output_id.lock().map_err(|e| e.to_string())?;
    usage.devices_selected(device_id, output.clone());
    Ok(())
}

/// Set the output device
///
/// Note: This currently only stores the device ID selection.
/// Actual audio engine device switching will be implemented when
/// the audio pipeline is integrated.
#[tauri::command]
pub fn audio_set_output_device(
    device_id: Option<String>,
    state: tauri::State<'_, AudioState>,
    usage: tauri::State<'_, crate::usage::UsageState>,
) -> Result<(), String> {
    let mut current = state.current_output_id.lock().map_err(|e| e.to_string())?;
    tracing::info!("Output device selected: {:?}", device_id);
    *current = device_id.clone();
    drop(current);

    let input = state.current_input_id.lock().map_err(|e| e.to_string())?;
    usage.devices_selected(input.clone(), device_id);
    Ok(())
}

/// Get current device selection
#[tauri::command]
pub fn audio_get_current_devices(
    state: tauri::State<'_, AudioState>,
) -> Result<CurrentDevices, String> {
    let input = state
        .current_input_id
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let output = state
        .current_output_id
        .lock()
        .map_err(|e| e.to_string())?
        .clone();

    Ok(CurrentDevices {
        input_device_id: input,
        output_device_id: output,
    })
}

/// Get current buffer size (frame_size in samples)
#[tauri::command]
pub fn audio_get_buffer_size(state: tauri::State<'_, AudioState>) -> Result<u32, String> {
    let size = state.buffer_size.lock().map_err(|e| e.to_string())?;
    Ok(*size)
}

/// Set buffer size (frame_size in samples)
///
/// Valid values: 8, 16, 32, 64, 128, 256
/// Lower values = less latency but may cause audio crackling
/// Higher values = more stable but higher latency
#[tauri::command]
pub fn audio_set_buffer_size(size: u32, state: tauri::State<'_, AudioState>) -> Result<(), String> {
    // Validate buffer size
    if ![8, 16, 32, 64, 128, 256].contains(&size) {
        return Err(format!(
            "Invalid buffer size: {}. Valid values are 8, 16, 32, 64, 128, 256",
            size
        ));
    }

    let mut current = state.buffer_size.lock().map_err(|e| e.to_string())?;
    tracing::info!("Buffer size set to {}", size);
    *current = size;

    Ok(())
}

/// Get the number of channels supported by a device
///
/// Returns the list of supported channel counts for the specified device.
/// If device_id is None, uses the default device.
#[tauri::command]
pub fn audio_get_device_channels(device_id: String, is_input: bool) -> Result<Vec<u16>, String> {
    let devices = if is_input {
        list_input_devices().map_err(|e| e.to_string())?
    } else {
        list_output_devices().map_err(|e| e.to_string())?
    };

    let device = devices
        .into_iter()
        .find(|d| d.id.0 == device_id)
        .ok_or_else(|| format!("Device not found: {}", device_id))?;

    Ok(device.supported_channels)
}
