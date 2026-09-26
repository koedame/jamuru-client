//! Tauri application library
//!
//! Provides IPC commands for signaling server, audio device, streaming management,
//! and multi-window management.

mod audio;
#[cfg(feature = "debug-tools")]
mod audio_tap;
mod config;
#[cfg(feature = "debug-remote")]
mod debug_remote;
mod device_identity;
mod diagnostics;
#[cfg(feature = "e2e-control")]
mod e2e_control;
mod help_link;
mod logging;
mod mixer;
mod rpc;
mod session;
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
    #[cfg(feature = "debug-tools")]
    jamjam::perf::enable();

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
        .invoke_handler(rpc::spec::invoke_handler())
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
    app.manage(mixer::MixerState::new());
    app.manage(session::SessionState::new());
    app.manage(help_link::HelpLinks::default());
    // What a portal over the relay hears (a helper, and the debug portal of a
    // beta build) comes through this.
    rpc::events::install(app.handle());

    logging::log_startup(app.handle(), &log_spec);

    if self_updating {
        updater::spawn(app.handle().clone());
    }

    // After the state above is managed: the connection is made as soon as it
    // starts, and the screen finds the session as it is when it asks.
    session::spawn(app.handle().clone());

    // After the state above is managed: the loop reads it as soon as it starts.
    #[cfg(feature = "debug-remote")]
    debug_remote::spawn(app.handle().clone());

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle
                .state::<UsageState>()
                .app_exiting(&handle.state::<StreamingState>());
        }
    });
}
