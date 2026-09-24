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
mod signaling;
mod streaming;
mod usage;
mod windows;

use audio::AudioState;
use config::ConfigState;
use device_identity::DeviceIdentityState;
use signaling::SignalingState;
use streaming::StreamingState;
use tauri::Manager;
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

    let app = tauri::Builder::default()
        // First: the logger exists before anything below can log (ADR-036).
        .plugin(logging::init(log_spec.clone()))
        .plugin(tauri_plugin_shell::init())
        // Hands `jamjam://join/<code>` links from the OS to the app (REQ-CON-103).
        // The scheme is declared in tauri.conf.json under plugins.deep-link.
        .plugin(tauri_plugin_deep_link::init())
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
            audio::audio_list_input_devices,
            audio::audio_list_output_devices,
            audio::audio_set_input_device,
            audio::audio_set_output_device,
            audio::audio_get_current_devices,
            audio::audio_get_buffer_size,
            audio::audio_set_buffer_size,
            audio::audio_get_device_channels,
            streaming::streaming_prepare,
            streaming::streaming_start,
            streaming::streaming_stop,
            streaming::streaming_status,
            streaming::streaming_reconnect,
            streaming::streaming_set_input_device,
            streaming::streaming_set_transmit_channels,
            streaming::streaming_set_output_device,
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
            config::config_save,
            config::config_get_server_url,
            config::config_set_server_url,
            config::config_get_effective_server_url,
            config::config_list_presets,
            config::config_get_preset,
            config::config_set_preset,
            config::config_get_connection_history,
            config::config_add_connection_history,
            config::config_remove_connection_history,
            config::config_clear_connection_history,
            config::config_update_connection_history_label,
            config::config_get_peer_name,
            config::config_set_peer_name,
            config::config_get_sample_rate,
            config::config_set_sample_rate,
            config::config_list_sample_rates,
            config::config_get_input_channels,
            config::config_set_input_channels,
            config::config_get_output_channels,
            config::config_set_output_channels,
            config::config_get_transmit_channels,
            config::config_set_transmit_channels,
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
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // State is built here, after the logger exists and before Tauri creates
    // the windows in `run`. Built earlier - as `Builder::manage` would - what
    // it logs while loading (an unreadable config, a discarded device
    // identity) would go nowhere.
    app.manage(SignalingState::new());
    // Device selection and buffer size come from config.toml, so the
    // interface the user chose is in effect from the first frame rather
    // than only after they open settings (ADR-026).
    let startup_config = load_startup_config();
    app.manage(AudioState::from_config(&startup_config));
    app.manage(StreamingState::new());
    app.manage(ConfigState::new());
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
    app.manage(usage);

    logging::log_startup(app.handle(), &log_spec);

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle
                .state::<UsageState>()
                .app_exiting(&handle.state::<StreamingState>());
        }
    });
}
