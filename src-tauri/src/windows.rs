//! Window management for Tauri multi-window application
//!
//! Provides programmatic window creation and management for:
//! - Connection Window: Session join/create UI (shown when disconnected)
//! - Mixer Window: DAW-style mixing console (shown when connected)
//! - Chat Window: Text communication (shown when connected)
//! - Settings Window: Audio/display settings (modal, on-demand)

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder};
use tracing::{debug, info, warn};

/// Window labels (used as unique identifiers)
pub mod labels {
    pub const MAIN: &str = "main";
    pub const CONNECTION: &str = "connection";
    pub const MIXER: &str = "mixer";
    pub const CHAT: &str = "chat";
    pub const SETTINGS: &str = "settings";
}

/// Global state to track if we're in a connected session
static IN_SESSION: AtomicBool = AtomicBool::new(false);

/// Check if currently in a session
pub fn is_in_session() -> bool {
    IN_SESSION.load(Ordering::Relaxed)
}

/// Set session state
fn set_session_state(in_session: bool) {
    IN_SESSION.store(in_session, Ordering::Relaxed);
}

/// Create the connection window
///
/// This is the initial window shown when the app starts.
/// Fixed size, non-resizable.
pub fn create_connection_window(app: &AppHandle) -> tauri::Result<()> {
    // Check if window already exists
    if app.get_webview_window(labels::CONNECTION).is_some() {
        debug!("Connection window already exists");
        return Ok(());
    }

    info!("Creating connection window");

    WebviewWindowBuilder::new(
        app,
        labels::CONNECTION,
        WebviewUrl::App("index.html".into()),
    )
    .title("jamjam")
    .inner_size(400.0, 300.0)
    .min_inner_size(320.0, 280.0)
    .resizable(false)
    .center()
    .build()?;

    Ok(())
}

/// Create the mixer window
///
/// DAW-style mixing console for volume/pan control.
/// Resizable with minimum size constraints.
pub fn create_mixer_window(app: &AppHandle) -> tauri::Result<()> {
    // Check if window already exists
    if app.get_webview_window(labels::MIXER).is_some() {
        debug!("Mixer window already exists");
        return Ok(());
    }

    info!("Creating mixer window");

    WebviewWindowBuilder::new(
        app,
        labels::MIXER,
        WebviewUrl::App("index.html#/mixer".into()),
    )
    .title("jamjam - Mixer")
    .inner_size(800.0, 600.0)
    .min_inner_size(600.0, 400.0)
    .resizable(true)
    .build()?;

    Ok(())
}

/// Create the chat window
///
/// Text communication window.
/// Resizable, can be hidden and re-shown.
pub fn create_chat_window(app: &AppHandle) -> tauri::Result<()> {
    // Check if window already exists
    if app.get_webview_window(labels::CHAT).is_some() {
        debug!("Chat window already exists");
        return Ok(());
    }

    info!("Creating chat window");

    WebviewWindowBuilder::new(
        app,
        labels::CHAT,
        WebviewUrl::App("index.html#/chat".into()),
    )
    .title("jamjam - Chat")
    .inner_size(400.0, 500.0)
    .min_inner_size(300.0, 400.0)
    .resizable(true)
    .build()?;

    Ok(())
}

/// Create the settings window
///
/// Settings window for audio devices, presets, and display options.
/// Non-modal, can be opened alongside other windows.
pub fn create_settings_window(app: &AppHandle) -> tauri::Result<()> {
    // Check if window already exists
    if let Some(window) = app.get_webview_window(labels::SETTINGS) {
        // Focus existing window
        debug!("Settings window already exists, focusing");
        let _ = window.set_focus();
        return Ok(());
    }

    info!("Creating settings window");

    WebviewWindowBuilder::new(
        app,
        labels::SETTINGS,
        WebviewUrl::App("index.html#/settings".into()),
    )
    .title("jamjam - Settings")
    .inner_size(720.0, 560.0)
    .resizable(false)
    .center()
    .build()?;

    Ok(())
}

/// Transition to connected state
///
/// - Closes the connection window
/// - Opens mixer and chat windows
pub fn transition_to_connected(app: &AppHandle) -> tauri::Result<()> {
    info!("Transitioning to connected state");
    set_session_state(true);

    // Close connection window
    if let Some(window) = app.get_webview_window(labels::CONNECTION) {
        debug!("Closing connection window");
        window.close()?;
    }

    // Create session windows
    create_mixer_window(app)?;
    create_chat_window(app)?;

    // Emit event to notify all windows
    app.emit("session:connected", ())?;

    Ok(())
}

/// Transition to disconnected state
///
/// - Closes mixer and chat windows
/// - Opens the connection window
pub fn transition_to_disconnected(app: &AppHandle, reason: Option<&str>) -> tauri::Result<()> {
    info!("Transitioning to disconnected state: {:?}", reason);
    set_session_state(false);

    // Emit event to notify all windows before closing
    app.emit(
        "session:disconnected",
        reason.unwrap_or("User left the session"),
    )?;

    // Close session windows
    if let Some(window) = app.get_webview_window(labels::MIXER) {
        debug!("Closing mixer window");
        let _ = window.close();
    }
    if let Some(window) = app.get_webview_window(labels::CHAT) {
        debug!("Closing chat window");
        let _ = window.close();
    }
    if let Some(window) = app.get_webview_window(labels::SETTINGS) {
        debug!("Closing settings window");
        let _ = window.close();
    }

    // Create connection window
    create_connection_window(app)?;

    Ok(())
}

/// Toggle settings window visibility
#[allow(dead_code)]
pub fn toggle_settings_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(labels::SETTINGS) {
        // Close if already open
        debug!("Closing settings window");
        window.close()?;
    } else {
        // Open if not
        create_settings_window(app)?;
    }
    Ok(())
}

/// Toggle chat window visibility (for session state only)
pub fn toggle_chat_window(app: &AppHandle) -> tauri::Result<()> {
    if !is_in_session() {
        warn!("Cannot toggle chat window: not in session");
        return Ok(());
    }

    if let Some(window) = app.get_webview_window(labels::CHAT) {
        if window.is_visible().unwrap_or(false) {
            debug!("Hiding chat window");
            window.hide()?;
        } else {
            debug!("Showing chat window");
            window.show()?;
            window.set_focus()?;
        }
    } else {
        create_chat_window(app)?;
    }
    Ok(())
}

// ============================================================================
// Tauri Commands for window management
// ============================================================================

/// Open settings window
///
/// The commands that can create a window are `async`: WebView2 deadlocks the
/// main thread when a window is built from a synchronous command.
#[tauri::command]
pub async fn window_open_settings(app: AppHandle) -> Result<(), String> {
    create_settings_window(&app).map_err(|e| e.to_string())
}

/// Close settings window
#[tauri::command]
pub fn window_close_settings(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(labels::SETTINGS) {
        window.close().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

/// Toggle chat window visibility
#[tauri::command]
pub async fn window_toggle_chat(app: AppHandle) -> Result<(), String> {
    toggle_chat_window(&app).map_err(|e| e.to_string())
}

/// Show chat window
#[tauri::command]
pub async fn window_show_chat(app: AppHandle) -> Result<(), String> {
    if !is_in_session() {
        return Err("Not in session".to_string());
    }

    if let Some(window) = app.get_webview_window(labels::CHAT) {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())
    } else {
        create_chat_window(&app).map_err(|e| e.to_string())
    }
}

/// Hide chat window
#[tauri::command]
pub fn window_hide_chat(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(labels::CHAT) {
        window.hide().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

/// Transition to connected state (called when session starts)
#[tauri::command]
pub async fn window_session_connected(app: AppHandle) -> Result<(), String> {
    transition_to_connected(&app).map_err(|e| e.to_string())
}

/// Transition to disconnected state (called when session ends)
#[tauri::command]
pub async fn window_session_disconnected(
    app: AppHandle,
    reason: Option<String>,
) -> Result<(), String> {
    transition_to_disconnected(&app, reason.as_deref()).map_err(|e| e.to_string())
}

/// Get current session state
#[tauri::command]
pub fn window_is_in_session() -> bool {
    is_in_session()
}

/// Focus a specific window by label
#[tauri::command]
pub fn window_focus(app: AppHandle, label: String) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(&label) {
        window.set_focus().map_err(|e| e.to_string())
    } else {
        Err(format!("Window '{}' not found", label))
    }
}

/// Resize the main window, and update its enforced minimum size to match.
///
/// The app currently renders the connection screen and the connected
/// (mixer/chat) view inside a single main window rather than the separate
/// Connection/Mixer windows described above, so it resizes itself between
/// the two instead: compact to match ui.pen's JoinRoom frame while
/// disconnected, wider once a session is active.
///
/// `min_width`/`min_height` must be passed in (rather than hardcoded here)
/// because the connected 3-column layout's real floor is a UI-layer fact
/// (sidebar + chat column widths) that the frontend already owns via
/// `ui/src/lib/windowSizes.ts` — without updating the minimum here, a user
/// could shrink the connected window below what the layout can render and
/// clip the mixer with no way to recover (`tauri.conf.json`'s `minWidth`/
/// `minHeight` only apply to the window's initial disconnected state).
#[tauri::command]
pub fn window_resize_main(
    app: AppHandle,
    width: f64,
    height: f64,
    min_width: f64,
    min_height: f64,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(labels::MAIN) {
        // Lower the floor before resizing: on a shrink (connected -> disconnected),
        // resizing first while the old, larger minimum is still in effect risks the
        // OS/webview clamping the shrink to that stale floor.
        window
            .set_min_size(Some(LogicalSize::new(min_width, min_height)))
            .map_err(|e| e.to_string())?;
        window
            .set_size(LogicalSize::new(width, height))
            .map_err(|e| e.to_string())
    } else {
        Err("Main window not found".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_state() {
        // Reset state
        set_session_state(false);
        assert!(!is_in_session());

        set_session_state(true);
        assert!(is_in_session());

        set_session_state(false);
        assert!(!is_in_session());
    }
}
