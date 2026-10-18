//! One running app per person.
//!
//! Opening an invite link (`jamuru://join/<code>`) while the app is running
//! starts the program again on Windows and Linux. Two copies then share one
//! config file but each keeps its own copy of the settings in memory, so a
//! setting saved in one is not what the other joins a room with, and a save
//! from the older copy writes the stale values back over the newer file. They
//! also fight over the one remote-debugging registration and the audio
//! devices.
//!
//! The second copy hands its arguments (the link) to the first, which brings
//! its window forward, and exits (ADR-085).

use std::ffi::OsStr;

use tauri::{AppHandle, Manager, Wry};
use tracing::warn;

use crate::windows::labels;

/// Set by the GUI E2E harness (ADR-025), which starts several apps at once,
/// each with its own `$HOME` and control port.
const E2E_PORT_ENV: &str = "JAMJAM_E2E_CONTROL_PORT";

/// Whether a second copy should hand over to the one running.
fn wanted(e2e_control_port: Option<&OsStr>) -> bool {
    e2e_control_port.is_none()
}

/// The plugin that makes a second copy hand over, or `None` while the E2E
/// harness runs several copies on purpose. It must be the first plugin
/// registered: the second copy exits while the plugins start.
pub fn plugin() -> Option<tauri::plugin::TauriPlugin<Wry>> {
    wanted(std::env::var_os(E2E_PORT_ENV).as_deref())
        .then(|| tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)))
}

/// The link itself reaches the page through the deep-link plugin; this only
/// brings the window the person is looking for to the front.
fn show_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window(labels::MAIN) else {
        return;
    };
    for result in [window.unminimize(), window.show(), window.set_focus()] {
        if let Err(e) = result {
            warn!("Could not bring the window forward: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-CON-143
    #[test]
    fn the_e2e_harness_starts_several_apps_at_once_the_app_does_not_hand_over_to_one_running() {
        assert!(!wanted(Some(OsStr::new("17891"))));
    }

    /// Verifies: REQ-CON-143
    #[test]
    fn nothing_asks_for_several_copies_a_second_copy_hands_over_to_the_one_running() {
        assert!(wanted(None));
    }
}
