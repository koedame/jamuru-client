//! Configuration Tauri commands
//!
//! The configuration model and its file I/O live in the core library
//! (`jamjam::config`) so the CLI shares one config file with the GUI
//! (ADR-027). This module is the IPC surface over it.

use std::sync::Mutex;

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub use jamjam::config::{
    load_config, save_config, AppConfig, AudioPreset, ConnectionHistoryEntry, DEFAULT_SAMPLE_RATE,
    DEFAULT_SERVER_URL, MAX_HISTORY_ENTRIES, VALID_SAMPLE_RATES,
};

/// State for configuration management
pub struct ConfigState {
    config: Mutex<AppConfig>,
}

impl ConfigState {
    /// Create a new ConfigState, loading existing config or using defaults
    pub fn new() -> Self {
        let config = load_config().unwrap_or_default();
        Self {
            config: Mutex::new(config),
        }
    }

    /// Get a clone of the current configuration
    pub fn get(&self) -> Result<AppConfig, String> {
        self.config
            .lock()
            .map(|guard| guard.clone())
            .map_err(|e| format!("Failed to lock config: {}", e))
    }

    /// The jamjam server to connect through (ADR-030)
    ///
    /// Falls back to the build default when the config cannot be read, so the
    /// connection screen and diagnostics always use the same server.
    pub fn server_url(&self) -> String {
        self.get()
            .map(|c| c.effective_server_url().to_string())
            .unwrap_or_else(|_| DEFAULT_SERVER_URL.to_string())
    }

    /// Update the configuration and save to disk
    pub fn update(&self, new_config: AppConfig) -> Result<(), String> {
        new_config.validate()?;

        let mut config = self
            .config
            .lock()
            .map_err(|e| format!("Failed to lock config: {}", e))?;

        *config = new_config.clone();
        drop(config);

        save_config(&new_config)
    }
}

impl Default for ConfigState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tauri Commands
// =============================================================================

/// Load configuration from disk
///
/// Returns the current configuration (from file or defaults if file doesn't exist).
#[tauri::command]
pub fn config_load(state: tauri::State<'_, ConfigState>) -> Result<AppConfig, String> {
    state.get()
}

/// Save configuration to disk
///
/// Validates and saves the provided configuration.
#[tauri::command]
pub fn config_save(
    config: AppConfig,
    state: tauri::State<'_, ConfigState>,
    usage: tauri::State<'_, crate::usage::UsageState>,
) -> Result<(), String> {
    state.update(config.clone())?;
    usage.apply_setting(&config);
    Ok(())
}

/// Get the jamjam server URL from configuration
///
/// Returns None if using the default server.
#[tauri::command]
pub fn config_get_server_url(
    state: tauri::State<'_, ConfigState>,
) -> Result<Option<String>, String> {
    let config = state.get()?;
    Ok(config.server_url)
}

/// Set the jamjam server URL in configuration
///
/// Pass None to use the default server.
#[tauri::command]
pub fn config_set_server_url(
    url: Option<String>,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let mut config = state.get()?;
    config.server_url = url;
    state.update(config)
}

/// Get the jamjam server URL the app will actually use.
///
/// Unlike [`config_get_server_url`], never `None`: resolves to the build
/// default when no override is configured, so the UI can always show what it
/// is (or will be) connecting to.
#[tauri::command]
pub fn config_get_effective_server_url(state: tauri::State<'_, ConfigState>) -> String {
    state.server_url()
}

/// Preset information returned to the frontend
#[derive(Debug, Clone, Serialize)]
pub struct PresetInfo {
    /// Preset identifier (e.g., "zero-latency")
    pub id: String,
    /// Recommended buffer size in samples
    pub buffer_size: u32,
    /// Recommended jitter buffer frames
    pub jitter_buffer_frames: u32,
}

/// Get the current preset
#[tauri::command]
pub fn config_get_preset(state: tauri::State<'_, ConfigState>) -> Result<String, String> {
    let config = state.get()?;
    Ok(config.preset.name().to_string())
}

/// Set the preset and apply its recommended settings
#[tauri::command]
pub async fn config_set_preset(
    preset_name: String,
    state: tauri::State<'_, ConfigState>,
    streaming_state: tauri::State<'_, crate::streaming::StreamingState>,
) -> Result<PresetInfo, String> {
    let preset = AudioPreset::from_name(&preset_name)
        .ok_or_else(|| format!("Unknown preset: {}", preset_name))?;

    let mut config = state.get()?;
    config.preset = preset.clone();
    config.buffer_size = preset.frame_size();
    state.update(config)?;

    // Apply the new jitter buffer depth to a running session (REQ-LAT-106).
    // The frame size cannot change mid-session - the audio engines own it - so
    // it takes effect on the next connect.
    streaming_state
        .apply_jitter_buffer_frames(preset.jitter_buffer_frames())
        .await;

    Ok(PresetInfo {
        id: preset.name().to_string(),
        buffer_size: preset.frame_size(),
        jitter_buffer_frames: preset.jitter_buffer_frames(),
    })
}

/// List all available presets
#[tauri::command]
pub fn config_list_presets() -> Vec<PresetInfo> {
    AudioPreset::all()
        .into_iter()
        .map(|p| PresetInfo {
            id: p.name().to_string(),
            buffer_size: p.frame_size(),
            jitter_buffer_frames: p.jitter_buffer_frames(),
        })
        .collect()
}

// =============================================================================
// Connection History Commands
// =============================================================================

/// Get connection history
///
/// Returns the list of past connections, most recent first.
#[tauri::command]
pub fn config_get_connection_history(
    state: tauri::State<'_, ConfigState>,
) -> Result<Vec<ConnectionHistoryEntry>, String> {
    let config = state.get()?;
    Ok(config.connection_history)
}

/// Add a connection to history
///
/// Adds a new entry to the connection history. If the room code already exists,
/// it updates the timestamp and moves it to the top. Limits history to MAX_HISTORY_ENTRIES.
#[tauri::command]
pub fn config_add_connection_history(
    room_code: String,
    label: Option<String>,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let mut config = state.get()?;

    // Remove existing entry with same room code (if any)
    config
        .connection_history
        .retain(|e| e.room_code != room_code);

    // Add new entry at the beginning
    config.connection_history.insert(
        0,
        ConnectionHistoryEntry {
            room_code,
            connected_at: Utc::now(),
            label,
        },
    );

    // Trim to max entries
    config.connection_history.truncate(MAX_HISTORY_ENTRIES);

    state.update(config)
}

/// Remove a connection from history
///
/// Removes the entry with the specified room code from history.
#[tauri::command]
pub fn config_remove_connection_history(
    room_code: String,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let mut config = state.get()?;
    config
        .connection_history
        .retain(|e| e.room_code != room_code);
    state.update(config)
}

/// Clear all connection history
#[tauri::command]
pub fn config_clear_connection_history(state: tauri::State<'_, ConfigState>) -> Result<(), String> {
    let mut config = state.get()?;
    config.connection_history.clear();
    state.update(config)
}

/// Update a connection history entry label
#[tauri::command]
pub fn config_update_connection_history_label(
    room_code: String,
    label: Option<String>,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let mut config = state.get()?;
    if let Some(entry) = config
        .connection_history
        .iter_mut()
        .find(|e| e.room_code == room_code)
    {
        entry.label = label;
        state.update(config)
    } else {
        Err(format!("Room code not found in history: {}", room_code))
    }
}

// =============================================================================
// Peer Name Commands
// =============================================================================

/// Get the user's display name
///
/// Returns the configured peer name for use in sessions.
#[tauri::command]
pub fn config_get_peer_name(state: tauri::State<'_, ConfigState>) -> Result<String, String> {
    let config = state.get()?;
    Ok(config.peer_name)
}

/// Set the user's display name
///
/// Updates and persists the peer name used in sessions.
#[tauri::command]
pub fn config_set_peer_name(
    name: String,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Peer name cannot be empty".to_string());
    }
    if trimmed.len() > 32 {
        return Err("Peer name cannot exceed 32 characters".to_string());
    }

    let mut config = state.get()?;
    config.peer_name = trimmed.to_string();
    state.update(config)
}

// =============================================================================
// Language Commands
// =============================================================================

/// Get the persisted UI language
///
/// Returns None if the user has not chosen one yet (the GUI falls back to
/// browser/OS detection in that case).
#[tauri::command]
pub fn config_get_language(state: tauri::State<'_, ConfigState>) -> Result<Option<String>, String> {
    let config = state.get()?;
    Ok(config.language)
}

/// Set the UI language and notify every open window
///
/// Persists the choice so a restarted window shows it too, and emits
/// `i18n:language-changed` so windows other than the one the user changed it
/// in (e.g. the main window while the settings window is open) switch
/// immediately. A Tauri event rather than the localStorage `storage` event,
/// because separate webviews don't fire `storage` consistently.
#[tauri::command]
pub fn config_set_language(
    language: String,
    app: AppHandle,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    let mut config = state.get()?;
    config.language = Some(language.clone());
    state.update(config)?;

    app.emit("i18n:language-changed", language)
        .map_err(|e| e.to_string())
}

// =============================================================================
// Sample Rate Commands (ADR-013)
// =============================================================================

/// Get the configured sample rate
///
/// Returns the current sample rate in Hz.
#[tauri::command]
pub fn config_get_sample_rate(state: tauri::State<'_, ConfigState>) -> Result<u32, String> {
    let config = state.get()?;
    Ok(config.sample_rate)
}

/// Set the sample rate
///
/// Updates and persists the sample rate, and notifies every open window so
/// the main window's mixer reflects it without a restart (mirrors
/// `config_set_language`'s `i18n:language-changed` broadcast).
///
/// Valid values: 44100, 48000, 96000. 48000 Hz is recommended per ADR-013.
#[tauri::command]
pub fn config_set_sample_rate(
    sample_rate: u32,
    app: AppHandle,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    if !VALID_SAMPLE_RATES.contains(&sample_rate) {
        return Err(format!(
            "Invalid sample rate: {}. Valid values are 44100, 48000, 96000",
            sample_rate
        ));
    }

    let mut config = state.get()?;
    config.sample_rate = sample_rate;
    state.update(config)?;

    app.emit("audio:config-changed", ())
        .map_err(|e| e.to_string())
}

/// Get available sample rates
///
/// Returns the list of valid sample rates with metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SampleRateInfo {
    /// Sample rate in Hz
    pub rate: u32,
    /// Human-readable label
    pub label: String,
    /// Whether this is the recommended rate
    pub recommended: bool,
}

#[tauri::command]
pub fn config_list_sample_rates() -> Vec<SampleRateInfo> {
    VALID_SAMPLE_RATES
        .iter()
        .map(|&rate| SampleRateInfo {
            rate,
            label: format!("{} Hz", rate),
            recommended: rate == DEFAULT_SAMPLE_RATE,
        })
        .collect()
}

// =============================================================================
// Channel Configuration Commands
// =============================================================================

/// Channel configuration for input or output
#[derive(Debug, Clone, Serialize)]
pub struct ChannelConfig {
    /// Left (or mono) channel (1-based index)
    pub channel_l: u32,
    /// Right channel (1-based index, None for mono)
    pub channel_r: Option<u32>,
}

/// Get input channel configuration
#[tauri::command]
pub fn config_get_input_channels(
    state: tauri::State<'_, ConfigState>,
) -> Result<ChannelConfig, String> {
    let config = state.get()?;
    Ok(ChannelConfig {
        channel_l: config.input_channel_l,
        channel_r: config.input_channel_r,
    })
}

/// Set input channel configuration
///
/// channel_l: 1-based index for left/mono channel
/// channel_r: 1-based index for right channel, or None for mono
#[tauri::command]
pub async fn config_set_input_channels(
    channel_l: u32,
    channel_r: Option<u32>,
    state: tauri::State<'_, ConfigState>,
    streaming_state: tauri::State<'_, crate::streaming::StreamingState>,
) -> Result<(), String> {
    if channel_l == 0 {
        return Err("Channel index must be >= 1".to_string());
    }
    if let Some(r) = channel_r {
        if r == 0 {
            return Err("Channel index must be >= 1".to_string());
        }
    }

    let mut config = state.get()?;
    config.input_channel_l = channel_l;
    config.input_channel_r = channel_r;
    state.update(config)?;

    // A running session follows the setting straight away
    streaming_state
        .apply_input_channels(channel_l, channel_r)
        .await;
    Ok(())
}

/// Get output channel configuration
#[tauri::command]
pub fn config_get_output_channels(
    state: tauri::State<'_, ConfigState>,
) -> Result<ChannelConfig, String> {
    let config = state.get()?;
    Ok(ChannelConfig {
        channel_l: config.output_channel_l,
        channel_r: config.output_channel_r,
    })
}

/// Set output channel configuration
///
/// channel_l: 1-based index for left/mono channel
/// channel_r: 1-based index for right channel, or None for mono
#[tauri::command]
pub async fn config_set_output_channels(
    channel_l: u32,
    channel_r: Option<u32>,
    state: tauri::State<'_, ConfigState>,
    streaming_state: tauri::State<'_, crate::streaming::StreamingState>,
) -> Result<(), String> {
    if channel_l == 0 {
        return Err("Channel index must be >= 1".to_string());
    }
    if let Some(r) = channel_r {
        if r == 0 {
            return Err("Channel index must be >= 1".to_string());
        }
    }

    let mut config = state.get()?;
    config.output_channel_l = channel_l;
    config.output_channel_r = channel_r;
    state.update(config)?;

    // A running session follows the setting straight away
    streaming_state
        .apply_output_channels(channel_l, channel_r)
        .await;
    Ok(())
}

/// Get transmit channel count
///
/// Returns 1 for mono, 2 for stereo
#[tauri::command]
pub fn config_get_transmit_channels(state: tauri::State<'_, ConfigState>) -> Result<u32, String> {
    let config = state.get()?;
    Ok(config.transmit_channels)
}

/// Set transmit channel count, and notify every open window so the main
/// window's mixer reflects it without a restart (mirrors
/// `config_set_language`'s `i18n:language-changed` broadcast).
///
/// count: 1 for mono, 2 for stereo
#[tauri::command]
pub fn config_set_transmit_channels(
    count: u32,
    app: AppHandle,
    state: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    if count != 1 && count != 2 {
        return Err("Transmit channels must be 1 (mono) or 2 (stereo)".to_string());
    }

    let mut config = state.get()?;
    config.transmit_channels = count;
    state.update(config)?;

    app.emit("audio:config-changed", ())
        .map_err(|e| e.to_string())
}
