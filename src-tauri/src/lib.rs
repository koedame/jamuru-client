//! Tauri application library
//!
//! Provides IPC commands for signaling server, audio device, streaming management,
//! and multi-window management.

mod app_info;
#[cfg(feature = "debug-tools")]
mod audio_tap;
mod chat_link;
mod config;
#[cfg(feature = "debug-remote")]
mod debug_remote;
mod device_identity;
mod diagnostics;
#[cfg(feature = "e2e-control")]
mod e2e_control;
mod help_link;
mod help_presence;
mod help_record;
mod logging;
mod menu;
mod mixer;
mod report_problem;
mod restart;
mod rpc;
mod session;
mod settings;
mod signaling;
mod single_instance;
mod software_render;
mod streaming;
mod terms;
#[cfg(feature = "update-drill")]
mod update_drill;
mod updater;
mod usage;
mod watchdog;
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

/// `config` as the usage reporter should read it: reporting is on only for a
/// user who has agreed to the terms of use, whatever the saved setting says.
fn with_usage_reporting_only_after_the_terms(mut config: config::AppConfig) -> config::AppConfig {
    config.usage_reporting &= config.terms_accepted();
    config
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Opens the app: the main window, and what runs on its own once it is open.
/// `checked_at_start` is whether a start that updates first has already looked
/// for a newer release.
fn open_app(app: &tauri::AppHandle, self_updating: bool, checked_at_start: bool) {
    let main = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == windows::labels::MAIN);
    let opened = match main {
        Some(config) => {
            tauri::WebviewWindowBuilder::from_config(app, config).and_then(|b| b.build())
        }
        None => Err(tauri::Error::WindowNotFound),
    };
    if let Err(e) = opened {
        // Nothing would be left for the person to see or quit.
        tracing::error!("Could not open the main window: {}", e);
        updater::startup::could_not_open();
        app.exit(1);
        return;
    }

    if self_updating {
        updater::spawn(app.clone(), checked_at_start);
    }

    // After the state is managed: the connection is made as soon as it
    // starts, and the screen finds the session as it is when it asks.
    session::spawn(app.clone());

    // After the state is managed: the loop reads it as soon as it starts.
    #[cfg(feature = "debug-remote")]
    debug_remote::spawn(app.clone());
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // A watcher of an update being tried is this program run from the copy
    // of the version the update replaced (ADR-088). It does nothing else, so
    // it comes before everything.
    if let Some((store, id)) = updater::trial::watcher_args(std::env::args_os()) {
        std::process::exit(updater::trial::run_watcher(store, id));
    }

    // First: the web process inherits this, and it must be set before any other thread exists.
    let software_render_limits = software_render::limit_software_rendering();
    let log_spec = logging::LogSpec::from_env();
    #[cfg(feature = "debug-tools")]
    {
        jamjam::perf::enable();
        audio_tap::install();
    }
    // The log writes a device by what the person is shown of it, so each device a listing finds is
    // told to it before anything is logged about the device.
    jamjam::audio::observe_listed_devices(|device| {
        logging::register_device(&device.id.0, &device.name)
    });

    let context = tauri::generate_context!();
    // Only the release build is given updater settings (`tauri.updater.conf.json`,
    // ADR-041), so a build made from source never replaces itself with a release.
    let self_updating = context.config().plugins.0.contains_key("updater");
    let version = context.package_info().version.to_string();
    #[cfg(feature = "update-drill")]
    update_drill::at(update_drill::Stage::BeforeStart, &version);
    // Before anything of the app runs: a start while the version an update
    // replaced is put back ends here (ADR-088).
    let began = if self_updating {
        updater::startup::begin(&version)
    } else {
        updater::startup::Began::Off
    };
    if began == updater::startup::Began::Exit {
        std::process::exit(0);
    }
    #[cfg(feature = "update-drill")]
    update_drill::at(update_drill::Stage::Started, &version);

    let mut builder = tauri::Builder::default();
    // Before every other plugin: a second copy exits while they start (ADR-085).
    if let Some(plugin) = single_instance::plugin() {
        builder = builder.plugin(plugin);
    }
    let mut builder = builder
        // The logger exists before anything below can log (ADR-036).
        .plugin(logging::init(log_spec.clone()))
        .plugin(tauri_plugin_shell::init())
        // Hands `jamuru://join/<code>` links from the OS to the app (REQ-CON-103).
        // The scheme is declared in tauri.conf.json under plugins.deep-link.
        .plugin(tauri_plugin_deep_link::init())
        // The menu is made in `menu::install`, in the app's language.
        .enable_macos_default_menu(false)
        .on_menu_event(|app, event| menu::handle_event(app, event.id().as_ref()))
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(true) => {
                menu::window_focused(window.app_handle(), window.label());
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                windows::close_requested(window.app_handle(), window.label(), api);
            }
            _ => {}
        });
    if self_updating {
        // Driven only from `updater.rs`, never the webview.
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let app = builder
        .setup(move |_app| {
            if !software_render_limits.is_empty() {
                tracing::info!(
                    "No GPU render node; limiting software rendering: {:?}",
                    software_render_limits
                );
            }
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
    // Until the user agrees to the terms of use, nothing below that talks to
    // the network on its own starts (`terms::wait_accepted`).
    app.manage(terms::TermsState::new(startup_config.terms_accepted()));
    app.manage(StreamingState::new());
    app.manage(SettingsState::new());
    let config_state = ConfigState::new();
    // Loads (or generates on first launch) this installation's device
    // identity once at startup - ADR-024, replaces account sign-in.
    app.manage(DeviceIdentityState::load());

    // Usage reporting is off unless the user turned it on (`usage_reporting`).
    // The panic hook goes in after the logger's, so both run on a panic.
    let usage = UsageState::new(
        &with_usage_reporting_only_after_the_terms(startup_config.clone()),
        &app.package_info().version.to_string(),
    );
    usage.reporter().install_panic_hook();
    // Read before the watchdog below writes this launch's own running
    // marker, which would otherwise overwrite what the previous one left.
    // Local and unconditional: whether it is sent depends on
    // `usage_reporting`, decided inside (usage.rs, ADR-056).
    let previous_incident = watchdog::previous_incident(usage.reporter());
    usage.apply_previous_incident(previous_incident);
    let watchdog = watchdog::Watchdog::install(usage.reporter().clone());
    app.manage(watchdog.clone());
    if usage.reporter().is_enabled() {
        usage.report_launch(startup_config);
    }
    let saved_usage = usage.clone();
    config_state.on_saved(move |config| {
        saved_usage.settings_saved(&with_usage_reporting_only_after_the_terms(config.clone()))
    });
    app.manage(config_state);
    app.manage(usage);
    app.manage(mixer::MixerState::new());
    app.manage(session::SessionState::new());
    app.manage(help_link::HelpLinks::default());
    app.manage(help_presence::PresenceState::default());
    app.manage(updater::UpdateNotice::default());
    // What a portal over the relay hears (a helper, and the debug portal of a
    // beta build) comes through this.
    rpc::events::install(app.handle());

    logging::log_startup(app.handle(), &log_spec);
    updater::startup::log_began(&began);

    // After the state above is managed: the menu reads it.
    if let Err(e) = menu::install(app.handle()) {
        tracing::warn!("Could not set up the menu: {}", e);
    }

    let mut began = Some(began);
    app.run(move |handle, event| match event {
        // Tauri does not make the main window (`create: false`): a start that
        // updates first opens it afterwards, or never, when it restarts into
        // the update (ADR-088).
        tauri::RunEvent::Ready => {
            if let Some(began) = began.take() {
                updater::startup::run(handle.clone(), began, move |app, checked| {
                    open_app(app, self_updating, checked)
                });
            }
        }
        tauri::RunEvent::ExitRequested { code, .. } => {
            tracing::info!("Exit requested (code {:?})", code);
        }
        tauri::RunEvent::Exit => {
            let _stage = watchdog.enter_stage(jamjam::telemetry::HangStage::AppExit);
            handle
                .state::<UsageState>()
                .app_exiting(&handle.state::<StreamingState>());
            drop(_stage);
            updater::startup::exiting();
            watchdog.mark_clean_exit();
        }
        _ => {}
    });
}
