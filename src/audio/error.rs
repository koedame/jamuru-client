//! Audio error types

use thiserror::Error;

/// Errors that can occur in the audio subsystem
#[derive(Error, Debug)]
pub enum AudioError {
    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Failed to open device: {0}")]
    DeviceOpenFailed(String),

    /// The driver did not answer in time. The call may still be running: the
    /// device is stuck, not necessarily gone.
    #[error("Device is not responding: {0}")]
    DeviceUnresponsive(String),

    #[error("Unsupported configuration: {0}")]
    UnsupportedConfig(String),

    #[error("Stream error: {0}")]
    StreamError(String),

    #[error("Buffer overflow")]
    BufferOverflow,

    #[error("Buffer underrun")]
    BufferUnderrun,

    #[error("Recording error: {0}")]
    RecordingError(String),

    #[error("Plugin error: {0}")]
    PluginError(String),
}
