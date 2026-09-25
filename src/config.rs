//! Configuration persistence
//!
//! Provides TOML-based configuration file management for the jamjam application.
//! Lives in the core library because the CLI and the GUI share one config file:
//! a device or preset chosen from either is what the other starts with
//! (ADR-027). The Tauri command wrappers stay in `src-tauri/src/config.rs`.
//! Configuration is stored in platform-specific directories:
//! - Linux: ~/.config/jamjam/config.toml
//! - Windows: %APPDATA%\jamjam\config.toml
//! - macOS: ~/Library/Application Support/jamjam/config.toml

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Application name used for configuration directory
const APP_NAME: &str = "jamjam";

/// Default buffer size in samples (64 samples @ 48kHz = 1.33ms)
const DEFAULT_BUFFER_SIZE: u32 = 64;

/// Default sample rate in Hz (48kHz is the recommended value per ADR-013)
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Valid sample rates per ADR-013
pub const VALID_SAMPLE_RATES: [u32; 3] = [44100, 48000, 96000];

/// Audio buffer sizes (frame sizes, in samples) the app offers and accepts.
/// The same set as the presets' frame sizes (`AudioPreset::frame_size`).
pub const VALID_BUFFER_SIZES: [u32; 4] = [32, 64, 128, 256];

/// jamjam server a development build uses: one running on this machine on the
/// development port 17890 (ADR-030).
///
/// The app asks the server where its signaling server is each time it
/// connects (`jamjam::network::discover_signaling_url`), so this is the only
/// address the app holds.
pub const DEV_SERVER_URL: &str = "http://localhost:17890";

/// jamjam server a release build uses (ADR-030).
///
/// The source does not name it: whoever builds the release passes it in
/// `JAMJAM_SERVER_URL` at compile time. The app's build script refuses to
/// make a release without it, so `None` only reaches release builds of the
/// CLI and the library, which do not use a default server.
pub const RELEASE_SERVER_URL: Option<&str> = option_env!("JAMJAM_SERVER_URL");

/// jamjam server used when `server_url` is not set.
///
/// Chosen by the build profile at compile time, so a release build carries
/// only the production URL and `cargo tauri dev` keeps using the local
/// server (ADR-030).
pub const DEFAULT_SERVER_URL: &str = if cfg!(debug_assertions) {
    DEV_SERVER_URL
} else {
    match RELEASE_SERVER_URL {
        Some(url) => url,
        None => "",
    }
};

/// Maximum number of connection history entries to keep
pub const MAX_HISTORY_ENTRIES: usize = 10;

/// UI languages the app ships translations for (ADR-007).
pub const VALID_LANGUAGES: [&str; 2] = ["en", "ja"];

/// Connection history entry
///
/// Records a past connection for quick reconnection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectionHistoryEntry {
    /// Room code used for the connection
    pub room_code: String,

    /// Timestamp of the connection (ISO 8601 format)
    pub connected_at: DateTime<Utc>,

    /// Optional user-defined label for this connection
    #[serde(default)]
    pub label: Option<String>,
}

/// Available audio presets
///
/// Re-exported so that preset parameters and their latency budgets have
/// exactly one definition (see `audio::preset` and ADR-019). Defining them
/// here as well is what let the GUI, ADR-008 and the E2E thresholds drift
/// apart previously.
pub use crate::audio::AudioPreset;

/// Default peer name
const DEFAULT_PEER_NAME: &str = "User";

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_channel_l() -> u32 {
    1
}

fn default_channel_r() -> Option<u32> {
    Some(2)
}

fn default_transmit_channels() -> u32 {
    2
}

/// Application configuration
///
/// Contains all persistent settings for the jamjam application.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    /// Selected input device ID (None = system default)
    #[serde(default)]
    pub input_device_id: Option<String>,

    /// Selected output device ID (None = system default)
    #[serde(default)]
    pub output_device_id: Option<String>,

    /// Audio buffer size in samples. Valid values: 32, 64, 128, 256
    #[serde(default = "default_buffer_size")]
    pub buffer_size: u32,

    /// Custom jamjam server URL (None = use the build's default server)
    #[serde(default)]
    pub server_url: Option<String>,

    /// Selected audio preset
    #[serde(default)]
    pub preset: AudioPreset,

    /// Connection history (most recent first)
    #[serde(default)]
    pub connection_history: Vec<ConnectionHistoryEntry>,

    /// User's display name for sessions
    #[serde(default = "default_peer_name")]
    pub peer_name: String,

    /// Audio sample rate in Hz (44100, 48000, or 96000)
    /// Default: 48000 (recommended per ADR-013)
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Input channel L (1-based index, default: 1)
    #[serde(default = "default_channel_l")]
    pub input_channel_l: u32,

    /// Input channel R (1-based index, default: 2, None for mono)
    #[serde(default = "default_channel_r")]
    pub input_channel_r: Option<u32>,

    /// Output channel L (1-based index, default: 1)
    #[serde(default = "default_channel_l")]
    pub output_channel_l: u32,

    /// Output channel R (1-based index, default: 2, None for mono)
    #[serde(default = "default_channel_r")]
    pub output_channel_r: Option<u32>,

    /// Transmit channel count (1 for mono, 2 for stereo)
    #[serde(default = "default_transmit_channels")]
    pub transmit_channels: u32,

    /// UI language (None = not chosen yet; the GUI falls back to browser/OS
    /// detection). Set once the user picks one in Settings, so every window,
    /// not just the one that changed it, shows the same language.
    #[serde(default)]
    pub language: Option<String>,

    /// Whether the app may tell the jamjam server how it runs (`telemetry`).
    /// Off unless the user turns it on.
    #[serde(default)]
    pub usage_reporting: bool,

    /// Whether the app installs a new release by itself (ADR-041). On unless
    /// the user turns it off in `config.toml`.
    #[serde(default = "default_auto_update")]
    pub auto_update: bool,
}

fn default_peer_name() -> String {
    DEFAULT_PEER_NAME.to_string()
}

fn default_auto_update() -> bool {
    true
}

fn default_buffer_size() -> u32 {
    DEFAULT_BUFFER_SIZE
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            input_device_id: None,
            output_device_id: None,
            buffer_size: DEFAULT_BUFFER_SIZE,
            server_url: None,
            preset: AudioPreset::default(),
            connection_history: Vec::new(),
            peer_name: DEFAULT_PEER_NAME.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            input_channel_l: 1,
            input_channel_r: Some(2),
            output_channel_l: 1,
            output_channel_r: Some(2),
            transmit_channels: 2,
            language: None,
            usage_reporting: false,
            auto_update: true,
        }
    }
}

impl AppConfig {
    /// The jamjam server to use: the configured one, or
    /// [`DEFAULT_SERVER_URL`]
    pub fn effective_server_url(&self) -> &str {
        self.server_url.as_deref().unwrap_or(DEFAULT_SERVER_URL)
    }

    /// Validate the configuration values
    ///
    /// Returns an error message if any value is invalid.
    pub fn validate(&self) -> Result<(), String> {
        // Validate buffer size
        if !VALID_BUFFER_SIZES.contains(&self.buffer_size) {
            return Err(format!(
                "Invalid buffer size: {}. Valid values are {:?}",
                self.buffer_size, VALID_BUFFER_SIZES
            ));
        }

        // Validate sample rate (ADR-013)
        if !VALID_SAMPLE_RATES.contains(&self.sample_rate) {
            return Err(format!(
                "Invalid sample rate: {}. Valid values are 44100, 48000, 96000",
                self.sample_rate
            ));
        }

        // Validate server URL if provided
        if let Some(ref url) = self.server_url {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(format!(
                    "Invalid server URL: {}. Must start with http:// or https://",
                    url
                ));
            }
        }

        // Validate language if provided
        if let Some(ref language) = self.language {
            if !VALID_LANGUAGES.contains(&language.as_str()) {
                return Err(format!(
                    "Invalid language: {}. Valid values are {:?}",
                    language, VALID_LANGUAGES
                ));
            }
        }

        Ok(())
    }
}

/// Get the configuration directory path
///
/// Returns None if the configuration directory cannot be determined.
/// Shared with `identity_store`, which stores `device_identity.json`
/// alongside `config.toml`.
pub fn config_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", APP_NAME).map(|dirs| dirs.config_dir().to_path_buf())
}

/// Get the configuration file path
///
/// Returns None if the configuration directory cannot be determined.
fn config_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.toml"))
}

/// Load configuration from the config file
///
/// Returns the loaded configuration, or an error if loading fails.
/// If the file doesn't exist, returns an error (use unwrap_or_default for fallback).
pub fn load_config() -> Result<AppConfig, String> {
    let path = config_path().ok_or("Could not determine config path")?;

    if !path.exists() {
        return Err("Config file does not exist".to_string());
    }

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read config file at {:?}: {}", path, e))?;

    let config: AppConfig =
        toml::from_str(&content).map_err(|e| format!("Failed to parse config file: {}", e))?;

    // `signaling_server_url` (a ws:// URL) became `server_url` (the server the app asks for its
    // signaling server). The value cannot carry over, as its meaning changed, so say that the
    // default server is used instead of doing it silently.
    if content
        .parse::<toml::Table>()
        .is_ok_and(|table| table.contains_key("signaling_server_url"))
    {
        tracing::warn!(
            "config.toml sets signaling_server_url, which is no longer read; set server_url \
             (http:// or https://) to use another server"
        );
    }

    // Validate the loaded config
    config.validate()?;

    Ok(config)
}

/// Save configuration to the config file
///
/// Creates the config directory if it doesn't exist.
pub fn save_config(config: &AppConfig) -> Result<(), String> {
    let config_dir = config_dir().ok_or("Could not determine config directory")?;
    let config_path = config_dir.join("config.toml");

    // Create config directory if it doesn't exist
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)
            .map_err(|e| format!("Failed to create config directory {:?}: {}", config_dir, e))?;
    }

    // Serialize config to TOML
    let content =
        toml::to_string_pretty(config).map_err(|e| format!("Failed to serialize config: {}", e))?;

    // Write to file
    fs::write(&config_path, content)
        .map_err(|e| format!("Failed to write config file {:?}: {}", config_path, e))?;

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.input_device_id, None);
        assert_eq!(config.output_device_id, None);
        assert_eq!(config.buffer_size, 64);
        assert_eq!(config.server_url, None);
    }

    /// Verifies: REQ-UPD-001
    #[test]
    fn when_the_setting_is_absent_the_app_updates_itself() {
        assert!(AppConfig::default().auto_update);
        let config: AppConfig = toml::from_str("buffer_size = 64").unwrap();
        assert!(config.auto_update);
    }

    /// Verifies: REQ-UPD-002
    #[test]
    fn when_the_setting_is_off_in_the_file_the_app_does_not_update_itself() {
        let config: AppConfig = toml::from_str("auto_update = false").unwrap();
        assert!(!config.auto_update);
    }

    #[test]
    fn test_server_url_falls_back_to_the_build_default() {
        let config = AppConfig::default();
        assert_eq!(config.effective_server_url(), DEFAULT_SERVER_URL);
    }

    #[test]
    fn test_configured_server_url_overrides_the_build_default() {
        let config = AppConfig {
            server_url: Some("https://server.example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(config.effective_server_url(), "https://server.example.com");
    }

    #[test]
    fn test_config_validation_valid() {
        let config = AppConfig {
            input_device_id: Some("device1".to_string()),
            output_device_id: Some("device2".to_string()),
            buffer_size: 64,
            server_url: Some("https://example.com".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_buffer_size() {
        let config = AppConfig {
            buffer_size: 100, // Invalid
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_url() {
        let config = AppConfig {
            server_url: Some("wss://example.com".to_string()), // Invalid, should be http/https
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = AppConfig {
            input_device_id: Some("input-device".to_string()),
            output_device_id: Some("output-device".to_string()),
            buffer_size: 128,
            server_url: Some("https://server.example.com".to_string()),
            ..Default::default()
        };

        let toml_str = toml::to_string(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config, parsed);
    }

    /// ADR-024 removed the email-OTP account fields, but a config.toml
    /// written by an older build that had accounts still contains them. Upgrading must not
    /// fail to parse, and the surviving settings must be preserved.
    #[test]
    fn test_config_with_removed_account_fields_still_deserializes() {
        let toml_str = r#"
            buffer_size = 128
            peer_name = "TestUser"
            account_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            account_token_expires_at = 1800000000
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.buffer_size, 128);
        assert_eq!(config.peer_name, "TestUser");
        config
            .validate()
            .expect("a config carrying leftover account fields must still be valid");
    }

    #[test]
    fn test_config_deserialization_with_defaults() {
        // Test that missing fields get default values
        let toml_str = r#"
            input_device_id = "some-device"
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.input_device_id, Some("some-device".to_string()));
        assert_eq!(config.output_device_id, None);
        assert_eq!(config.buffer_size, 64); // Default
        assert_eq!(config.server_url, None);
        assert_eq!(config.sample_rate, 48000); // Default per ADR-013
    }

    #[test]
    fn test_default_sample_rate() {
        let config = AppConfig::default();
        assert_eq!(config.sample_rate, 48000);
    }

    #[test]
    fn test_sample_rate_validation_valid() {
        for &rate in &[44100, 48000, 96000] {
            let config = AppConfig {
                sample_rate: rate,
                ..Default::default()
            };
            assert!(config.validate().is_ok(), "Rate {} should be valid", rate);
        }
    }

    #[test]
    fn test_sample_rate_validation_invalid() {
        let config = AppConfig {
            sample_rate: 22050, // Invalid
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = AppConfig {
            sample_rate: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_sample_rate_deserialization() {
        let toml_str = r#"
            sample_rate = 96000
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.sample_rate, 96000);
    }

    #[test]
    fn test_sample_rate_backward_compatibility() {
        // Old config without sample_rate should use default
        let toml_str = r#"
            buffer_size = 128
            peer_name = "TestUser"
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.sample_rate, 48000); // Default
    }

    #[test]
    fn test_default_language_is_none() {
        // No language chosen yet: the GUI falls back to browser/OS detection.
        let config = AppConfig::default();
        assert_eq!(config.language, None);
    }

    #[test]
    fn test_language_backward_compatibility() {
        // A config.toml written before the `language` field existed has no `language` key.
        let toml_str = r#"
            buffer_size = 128
            peer_name = "TestUser"
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.language, None);
    }

    #[test]
    fn test_language_validation_valid() {
        for &language in &VALID_LANGUAGES {
            let config = AppConfig {
                language: Some(language.to_string()),
                ..Default::default()
            };
            assert!(
                config.validate().is_ok(),
                "language {} should be valid",
                language
            );
        }
    }

    #[test]
    fn test_language_validation_invalid() {
        let config = AppConfig {
            language: Some("fr".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_language_deserialization() {
        let toml_str = r#"
            language = "ja"
        "#;

        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.language, Some("ja".to_string()));
    }

    #[test]
    fn test_language_serialization_roundtrip() {
        let config = AppConfig {
            language: Some("ja".to_string()),
            ..Default::default()
        };

        let toml_str = toml::to_string(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config, parsed);
    }
}
