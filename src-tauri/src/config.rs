//! Configuration Tauri commands
//!
//! The configuration model and its file I/O live in the core library
//! (`jamjam::config`) so the CLI shares one config file with the GUI
//! (ADR-027). This module is the IPC surface over it.

use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

pub use jamjam::config::{
    load_config, save_config, AppConfig, AudioPreset, ConnectionHistoryEntry, DEFAULT_SAMPLE_RATE,
    DEFAULT_SERVER_URL, MAX_HISTORY_ENTRIES, VALID_SAMPLE_RATES,
};

/// Called with the new configuration after each successful save
type SavedHook = Box<dyn Fn(&AppConfig) + Send + Sync>;

/// State for configuration management
pub struct ConfigState {
    config: Mutex<AppConfig>,
    on_saved: Mutex<Option<SavedHook>>,
}

impl ConfigState {
    /// Create a new ConfigState, loading existing config or using defaults
    pub fn new() -> Self {
        let config = load_config().unwrap_or_default();
        Self {
            config: Mutex::new(config),
            on_saved: Mutex::new(None),
        }
    }

    /// Calls `hook` with the new configuration after each successful save.
    pub fn on_saved(&self, hook: impl Fn(&AppConfig) + Send + Sync + 'static) {
        if let Ok(mut on_saved) = self.on_saved.lock() {
            *on_saved = Some(Box::new(hook));
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

        save_config(&new_config)?;
        if let Ok(on_saved) = self.on_saved.lock() {
            if let Some(hook) = on_saved.as_ref() {
                hook(&new_config);
            }
        }
        Ok(())
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
pub fn config_save(config: AppConfig, state: tauri::State<'_, ConfigState>) -> Result<(), String> {
    state.update(config)
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

/// A sample rate the app offers (ADR-013)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleRateInfo {
    /// Sample rate in Hz
    pub rate: u32,
    /// Human-readable label
    pub label: String,
    /// Whether this is the recommended rate
    pub recommended: bool,
}

pub fn sample_rates() -> Vec<SampleRateInfo> {
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

/// Get transmit channel count
///
/// Returns 1 for mono, 2 for stereo
#[tauri::command]
pub fn config_get_transmit_channels(state: tauri::State<'_, ConfigState>) -> Result<u32, String> {
    let config = state.get()?;
    Ok(config.transmit_channels)
}
