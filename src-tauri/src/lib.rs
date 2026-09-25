//! Tauri application library
//!
//! Provides IPC commands for signaling server, audio device, streaming management,
//! and multi-window management.

mod audio;
mod config;
mod device_identity;
mod diagnostics;
#[cfg(feature = "e2e-control")]
mod e2e_control;
mod logging;
mod settings;
mod settings_help;
mod signaling;
mod streaming;
mod updater;
mod usage;
mod windows;

use config::ConfigState;
use device_identity::DeviceIdentityState;
use settings::SettingsState;
use signaling::SignalingState;
use streaming::StreamingState;
use tauri::{Listener, Manager};
use usage::UsageState;

/// Config used to seed startup state.
///
/// A missing file is the normal first-run case and yields defaults quietly; a
/// file that exists but cannot be read or parsed is logged, because silently
/// falling back would present the user's saved settings as if they had never
/// been set.
fn load_startup_config() -> config::AppConfig {
    match config::load_config() {
        Ok(config) => config,
        Err(e) if e.contains("does not exist") => {
            // First launch. Defaults are correct and there is nothing to say.
            tracing::debug!("No config file yet; starting with defaults");
            config::AppConfig::default()
        }
        Err(e) => {
            // A file that exists but cannot be read or parsed is different:
            // falling back silently would present the user's saved settings as
            // if they had never been set.
            tracing::warn!("Ignoring unreadable config, starting with defaults: {}", e);
            config::AppConfig::default()
        }
    }
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let log_spec = logging::LogSpec::from_env();

    let context = tauri::generate_context!();
    // Only the release build is given updater settings (`tauri.updater.conf.json`,
    // ADR-041), so a build made from source never replaces itself with a release.
    let self_updating = context.config().plugins.0.contains_key("updater");

    let mut builder = tauri::Builder::default()
        // First: the logger exists before anything below can log (ADR-036).
        .plugin(logging::init(log_spec.clone()))
        .plugin(tauri_plugin_shell::init())
        // Hands `jamjam://join/<code>` links from the OS to the app (REQ-CON-103).
        // The scheme is declared in tauri.conf.json under plugins.deep-link.
        .plugin(tauri_plugin_deep_link::init());
    if self_updating {
        // Driven only from `updater.rs`, never the webview.
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let app = builder
        .setup(|_app| {
            // GUI E2E control channel (ADR-025). Compiled out entirely
            // without the `e2e-control` feature, and inert unless the
            // harness sets JAMJAM_E2E_CONTROL_PORT.
            #[cfg(feature = "e2e-control")]
            e2e_control::spawn(_app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            signaling::signaling_connect,
            signaling::signaling_disconnect,
            signaling::signaling_list_rooms,
            signaling::signaling_join_room,
            signaling::signaling_leave_room,
            signaling::signaling_create_room,
            signaling::signaling_send_chat,
            signaling::signaling_get_chat_messages,
            signaling::signaling_add_reaction,
            signaling::signaling_remove_reaction,
            signaling::signaling_toggle_reaction,
            signaling::signaling_publish_local_candidates,
            signaling::signaling_poll_events,
            signaling::settings_help_request,
            signaling::settings_help_answer,
            signaling::settings_help_propose,
            signaling::settings_help_decide,
            signaling::settings_help_stop,
            settings::settings_get,
            settings::settings_change,
            streaming::streaming_prepare,
            streaming::streaming_start,
            streaming::streaming_stop,
            streaming::streaming_status,
            streaming::streaming_reconnect,
            streaming::streaming_set_mute,
            streaming::streaming_get_mute,
            streaming::streaming_set_monitoring,
            streaming::streaming_get_input_level,
            streaming::streaming_set_peer_volume,
            streaming::streaming_get_peer_volume,
            streaming::streaming_set_master_volume,
            streaming::streaming_get_master_volume,
            streaming::streaming_set_peer_pan,
            streaming::streaming_get_peer_pan,
            streaming::streaming_set_local_volume,
            streaming::streaming_get_local_volume,
            streaming::streaming_set_local_pan,
            streaming::streaming_get_local_pan,
            config::config_load,
            config::config_set_usage_reporting,
            config::config_get_server_url,
            config::config_set_server_url,
            config::config_get_effective_server_url,
            config::config_list_presets,
            config::config_get_preset,
            config::config_get_connection_history,
            config::config_add_connection_history,
            config::config_remove_connection_history,
            config::config_clear_connection_history,
            config::config_update_connection_history_label,
            config::config_get_peer_name,
            config::config_set_peer_name,
            config::config_get_sample_rate,
            config::config_get_transmit_channels,
            config::config_get_language,
            config::config_set_language,
            diagnostics::diagnostics_run_complete,
            diagnostics::diagnostics_run_network,
            diagnostics::diagnostics_run_audio,
            diagnostics::diagnostics_run_cpu,
            diagnostics::diagnostics_get_recommended_preset,
            diagnostics::diagnostics_check_zero_latency,
            // Window management
            windows::window_open_settings,
            windows::window_close_settings,
            windows::window_toggle_chat,
            windows::window_show_chat,
            windows::window_hide_chat,
            windows::window_session_connected,
            windows::window_session_disconnected,
            windows::window_is_in_session,
            windows::window_focus,
            windows::window_resize_main,
            logging::log_frontend,
            logging::log_open_dir,
            usage::usage_preview,
        ])
        .build(context)
        .expect("error while building tauri application");

    // State is built here, after the logger exists and before Tauri creates
    // the windows in `run`. Built earlier - as `Builder::manage` would - what
    // it logs while loading (an unreadable config, a discarded device
    // identity) would go nowhere.
    app.manage(SignalingState::new());
    let startup_config = load_startup_config();
    app.manage(StreamingState::new());
    app.manage(SettingsState::new());
    let config_state = ConfigState::new();
    // Loads (or generates on first launch) this installation's device
    // identity once at startup - ADR-024, replaces account sign-in.
    app.manage(DeviceIdentityState::load());

    // Usage reporting is off unless the user turned it on (`usage_reporting`).
    // The panic hook goes in after the logger's, so both run on a panic.
    let usage = UsageState::new(&startup_config, &app.package_info().version.to_string());
    usage.reporter().install_panic_hook();
    if usage.reporter().is_enabled() {
        usage.report_launch(startup_config);
    }
    let saved_usage = usage.clone();
    config_state.on_saved(move |config| saved_usage.settings_saved(config));
    app.manage(config_state);
    app.manage(usage);

    // Whoever changed this app's audio settings, someone helping with them
    // sees them as they are now (ADR-043).
    let handle = app.handle().clone();
    app.listen_any(settings::CHANGED_EVENT, move |event| {
        let Ok(changed) = serde_json::from_str::<settings::AudioSettings>(event.payload()) else {
            return;
        };
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            signaling::settings_changed(&handle, changed).await;
        });
    });

    logging::log_startup(app.handle(), &log_spec);

    if self_updating {
        updater::spawn(app.handle().clone());
    }

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle
                .state::<UsageState>()
                .app_exiting(&handle.state::<StreamingState>());
        }
    });
}
