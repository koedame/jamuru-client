//! Audio devices for the settings (ADR-043)
//!
//! Lists the audio devices on offer. Choosing one, like every other audio
//! setting, goes through [`crate::settings::settings_change`]; the choice in
//! effect is the saved config.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use jamjam::audio::{list_input_devices, list_output_devices};

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
/// from "the interface was found but not selected". Written when the list
/// differs from the one written last: the settings list the devices on every
/// change, and the same list again says nothing new.
fn log_devices(kind: &str, devices: &[AudioDeviceInfo]) {
    static LAST: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

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
    let line = format!("{} {} device(s): {}", devices.len(), kind, names.join(", "));
    if let Ok(mut last) = LAST.lock() {
        match last.iter_mut().find(|(k, _)| k == kind) {
            Some((_, previous)) if *previous == line => return,
            Some((_, previous)) => *previous = line.clone(),
            None => last.push((kind.to_string(), line.clone())),
        }
    }
    tracing::info!("Found {}", line);
}
