//! One page object per screen the user sees.
//!
//! Each exposes the operations available on that screen and the states it
//! can be observed in. Selectors live here and nowhere else.

mod connection;
mod helper;
mod session;
mod settings;

pub use connection::{ConnectionScreen, ConnectionState};
pub use helper::HelperScreen;
pub use session::SessionScreen;
pub use settings::{DevicesTab, DiagnosticsTab, SettingsScreen, SettingsTab};

/// Tauri window labels, mirroring `src-tauri/src/windows.rs`'s `labels`.
pub(crate) mod windows {
    pub const SETTINGS: &str = "settings";
}

/// The start of the label of a window someone helping works in, mirroring
/// `src-tauri/src/help_link.rs`.
pub(crate) const HELP_WINDOW_PREFIX: &str = "help-";
