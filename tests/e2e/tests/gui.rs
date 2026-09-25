//! GUI E2E scenarios: everything the user can operate, and the states they
//! can observe (ADR-025).
//!
//! These drive the real Tauri app - Rust backend, webview, React, IPC - via
//! the `e2e-control` channel. That is what separates them from the UI unit
//! tests and Storybook, which only exercise Pure components.
//!
//! Run with:
//! ```text
//! cargo build --manifest-path ../../src-tauri/Cargo.toml --features e2e-control
//! cargo test --features gui --test gui -- --test-threads=1
//! ```
//!
//! Serialized on [`APP_LOCK`]: each scenario launches a real desktop app that
//! opens audio devices, and two of those competing produces failures that
//! have nothing to do with the code under test.
#![cfg(feature = "gui")]

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use jamjam_e2e_tests::pom::fake_relay::{FakeRelay, Target};
use jamjam_e2e_tests::pom::loopback_audio;
use jamjam_e2e_tests::pom::screens::{ConnectionState, SettingsTab};
use jamjam_e2e_tests::pom::{App, UI_TIMEOUT};

static APP_LOCK: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    APP_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Launches the app and waits for the connection screen to settle, which is
/// the entry point of every user journey below.
///
/// Settling matters: the app auto-connects on start-up, and while that is in
/// flight the panel shows a loading variant with no form. Acting before it
/// resolves races the start-up sequence.
fn launch() -> (MutexGuard<'static, ()>, App) {
    let guard = exclusive();
    let app = App::launch().expect("app should launch with the e2e-control feature");
    app.connection_screen()
        .wait_until_interactive(LAUNCH_SETTLE)
        .expect("connection screen should settle into an operable state");
    (guard, app)
}

/// The start-up auto-connect has to time out against a signaling server that
/// is not there, which takes longer than an ordinary UI transition.
const LAUNCH_SETTLE: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Launch and the states the connection screen can be in
// ---------------------------------------------------------------------------

/// The user reaches the connection screen with no account step in between.
///
/// Verifies: REQ-GUI-001
/// Verifies: REQ-IDT-003
#[test]
fn launching_the_app_shows_the_connection_screen() {
    let (_guard, app) = launch();
    let screen = app.connection_screen();

    assert!(screen.is_displayed().unwrap());
    assert!(
        screen.create_room_button().is_visible().unwrap(),
        "the user should be able to start a room straight away"
    );
}

/// ADR-024 removed account registration. No email or verification-code UI
/// may appear anywhere on the first screen.
///
/// Verifies: REQ-GUI-001
/// Verifies: REQ-IDT-003
#[test]
fn the_first_screen_asks_for_no_account() {
    let (_guard, app) = launch();
    let html = app.main_window_html().unwrap().to_lowercase();

    for forbidden in [
        "メールアドレス",
        "認証コード",
        "サインイン",
        "verification code",
        "sign in",
        "email",
    ] {
        assert!(
            !html.contains(&forbidden.to_lowercase()),
            "sign-in UI leaked back in: found {:?}",
            forbidden
        );
    }
    assert!(
        !html.contains("type=\"email\"") && !html.contains("type='email'"),
        "an email input is present"
    );
}

/// The device identifier is internal (ADR-024 Decision 5). It must not be
/// rendered anywhere the user can see it.
///
/// Verifies: REQ-GUI-002
#[test]
fn the_device_identifier_is_never_shown_to_the_user() {
    let (_guard, app) = launch();
    let screen = app.connection_screen();

    // A device id is 26 base32 characters. Nothing on screen should look
    // like one.
    let text = screen.visible_text().unwrap();
    let suspicious: Vec<&str> = text
        .split_whitespace()
        .filter(|word| {
            word.len() == 26
                && word
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
        })
        .collect();

    assert!(
        suspicious.is_empty(),
        "something that looks like a device identifier is on screen: {:?}",
        suspicious
    );

    // The settings window has its own scenario below; this one is only
    // about the screen the user lands on.
}

/// The screen always reports one of its defined states, so later assertions
/// can rely on `state()` rather than guessing from which elements exist.
///
/// Verifies: REQ-GUI-001
#[test]
fn the_connection_screen_reports_a_defined_state() {
    let (_guard, app) = launch();
    let state = app.connection_screen().state().unwrap();

    // Without a signaling server on localhost the auto-connect fails, so
    // idle and error are both legitimate here; connecting is transient.
    assert!(
        matches!(
            state,
            ConnectionState::Idle | ConnectionState::Error | ConnectionState::Connecting
        ),
        "unexpected state {:?}",
        state
    );
}

// ---------------------------------------------------------------------------
// Operating the invite-code form
// ---------------------------------------------------------------------------

/// Typing a code updates the field the user sees.
///
/// Verifies: REQ-GUI-005
#[test]
fn typing_an_invite_code_updates_the_field() {
    let (_guard, app) = launch();
    let screen = app.connection_screen();

    screen.invite_code_input().type_text("ABC234").unwrap();

    assert_eq!(screen.invite_code_input().value().unwrap(), "ABC234");
}

// ---------------------------------------------------------------------------
// Reaching (or failing to reach) the signaling server
// ---------------------------------------------------------------------------

/// A server the app cannot reach - a leftover dev/test URL in `config.toml`,
/// for example - names the URL and the actual error, and does not leave
/// Create/Join looking clickable while they cannot do anything.
#[test]
fn an_unreachable_server_names_the_url_and_disables_the_buttons() {
    let _guard = exclusive();
    let app = App::launch_with_server_url("https://127.0.0.1:1")
        .expect("app should launch with the e2e-control feature");
    let screen = app.connection_screen();

    screen
        .wait_until_interactive(LAUNCH_SETTLE)
        .expect("connection screen should settle into an operable state");
    assert_eq!(
        screen.state().unwrap(),
        ConnectionState::Error,
        "an unreachable server should settle into the error state, not idle"
    );

    screen
        .server_error()
        .wait_until_visible(UI_TIMEOUT)
        .expect("the server error banner should be shown");
    assert!(
        screen
            .server_error_url()
            .text()
            .unwrap()
            .contains("127.0.0.1:1"),
        "the banner should name the URL the app tried to dial"
    );
    assert!(
        !screen
            .server_error_detail()
            .text()
            .unwrap()
            .trim()
            .is_empty(),
        "the banner should show the underlying error, not just a friendly summary"
    );

    assert!(
        !screen.create_room_button().is_enabled().unwrap(),
        "Create Room should be disabled while there is no signaling connection"
    );
    assert!(
        !screen.join_button().is_enabled().unwrap(),
        "Join should be disabled while there is no signaling connection, even with a complete code"
    );

    screen
        .retry_button()
        .click()
        .expect("retry should be clickable");
    // The same unreachable URL is still configured, so this settles back
    // into the error state rather than connecting - it should not hang or
    // crash.
    screen
        .wait_until_interactive(LAUNCH_SETTLE)
        .expect("retrying against the same URL should still settle, not hang");
}

// ---------------------------------------------------------------------------
// The settings window
// ---------------------------------------------------------------------------

/// The settings button opens a real second window.
///
/// Verifies: REQ-GUI-007
#[test]
fn the_settings_button_opens_the_settings_window() {
    let (_guard, app) = launch();

    assert!(
        !app.settings_screen().is_open().unwrap(),
        "settings should not be open before it is asked for"
    );

    app.connection_screen().settings_button().click().unwrap();
    app.settings_screen()
        .wait_until_open(UI_TIMEOUT)
        .expect("settings window should open");

    assert!(app
        .open_windows()
        .unwrap()
        .contains(&"settings".to_string()));
    assert!(app.settings_screen().is_displayed().unwrap());
}

/// Every tab the user can select actually selects, and the screen shows
/// which one is active.
///
/// Verifies: REQ-GUI-007
#[test]
fn every_settings_tab_can_be_selected() {
    let (_guard, app) = launch();

    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();

    for tab in SettingsTab::all() {
        assert!(
            settings.tab(tab).is_enabled().unwrap(),
            "{:?} tab should be operable",
            tab
        );
        settings.select_tab(tab).unwrap();

        jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
            settings.selected_tab().unwrap_or(None) == Some(tab)
        })
        .unwrap_or_else(|_| panic!("{:?} tab did not become the selected tab", tab));

        assert!(
            !settings.visible_text().unwrap().trim().is_empty(),
            "{:?} tab should render content",
            tab
        );
    }
}

/// The settings window must not expose the device identifier either.
///
/// Verifies: REQ-GUI-002
#[test]
fn the_settings_window_does_not_show_the_device_identifier() {
    let (_guard, app) = launch();

    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();

    for tab in SettingsTab::all() {
        settings.select_tab(tab).unwrap();
        let text = settings.visible_text().unwrap();
        let suspicious: Vec<&str> = text
            .split_whitespace()
            .filter(|word| {
                word.len() == 26
                    && word
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
            })
            .collect();
        assert!(
            suspicious.is_empty(),
            "{:?} tab shows something identifier-shaped: {:?}",
            tab,
            suspicious
        );
    }
}

/// Changing the language in the settings window updates the main window too.
///
/// Before this was fixed, the settings and main windows each ran their own
/// i18next instance, so only the settings window's own text changed - the
/// main window kept showing whatever it detected at launch until restarted.
///
/// Verifies: REQ-I18N-102
#[test]
fn changing_the_language_in_settings_updates_the_main_window_immediately() {
    let (_guard, app) = launch();
    let connection = app.connection_screen();

    connection.settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::General).unwrap();

    settings
        .general_tab()
        .language_select()
        .select_value("ja")
        .unwrap();

    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        connection
            .create_room_button()
            .text()
            .map(|text| text == "ルームを作成")
            .unwrap_or(false)
    })
    .unwrap_or_else(|_| {
        panic!(
            "main window should have switched to Japanese without being told \
             to itself; it still shows {:?}",
            connection.create_room_button().text()
        )
    });

    settings
        .general_tab()
        .language_select()
        .select_value("en")
        .unwrap();

    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        connection
            .create_room_button()
            .text()
            .map(|text| text == "Create Room")
            .unwrap_or(false)
    })
    .unwrap_or_else(|_| {
        panic!(
            "main window should have switched back to English; it still shows {:?}",
            connection.create_room_button().text()
        )
    });
}

// ---------------------------------------------------------------------------
// The diagnostic log file (ADR-036)
// ---------------------------------------------------------------------------
//
// A release build has no console and no developer tools, so `jamjam.log` is
// the only record of what a session did. These scenarios read that file from
// the throwaway `$HOME` the app was started with.

/// Waits until the log satisfies `ready`, then returns it. The Rust side
/// writes as it goes, but the webview's lines arrive after the screen loads.
fn wait_for_log(app: &App, ready: impl Fn(&str) -> bool) -> String {
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || ready(&app.log_text())).unwrap_or_else(|_| {
        panic!(
            "the log file never showed what the scenario expects. It holds:\n{}",
            app.log_text()
        )
    });
    app.log_text()
}

/// The first lines say which build ran on which system with which settings,
/// which is what a bug report needs before anything else.
///
/// Verifies: REQ-GUI-016
#[test]
fn launching_the_app_writes_a_log_file_that_names_the_build_and_the_settings() {
    let (_guard, app) = launch();

    let log = wait_for_log(&app, |log| log.contains("config: server="));

    assert!(
        log.contains(&format!(
            "starting (os={} arch={})",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        "the build and system are missing:\n{}",
        log
    );
    assert!(
        log.contains("log file: ") && log.contains("jamjam.log"),
        "the log does not say where it is:\n{}",
        log
    );
    assert!(
        log.contains("config: server=http://localhost:17890"),
        "the settings in effect are missing:\n{}",
        log
    );
}

/// By default the app's own code and the webview log at debug, so a report
/// carries the detail of what the app did.
///
/// Verifies: REQ-GUI-019
#[test]
fn by_default_the_apps_own_code_and_the_webview_log_at_debug() {
    let (_guard, app) = launch();

    let log = wait_for_log(&app, |log| log.contains(" DEBUG [webview] "));

    assert!(
        log.contains(" DEBUG [jamjam_app_lib] "),
        "the app's own debug lines are missing:\n{}",
        log
    );
}

/// `JAMJAM_LOG` overrides the levels: with `error`, the lines the app writes
/// at info and debug on every start-up are gone and the failed connection's
/// error remains.
///
/// Verifies: REQ-GUI-019
#[test]
fn jamjam_log_overrides_the_levels_the_file_is_written_at() {
    let _guard = exclusive();
    let app = App::launch_with_env(&[("JAMJAM_LOG", "error")])
        .expect("app should launch with the e2e-control feature");
    app.connection_screen()
        .wait_until_interactive(LAUNCH_SETTLE)
        .expect("connection screen should settle into an operable state");

    let log = wait_for_log(&app, |log| {
        log.contains(" ERROR [jamjam_app_lib::signaling] ")
    });

    for level in [" INFO ", " DEBUG ", " WARN "] {
        assert!(
            !log.contains(level),
            "JAMJAM_LOG=error still wrote{}lines:\n{}",
            level,
            log
        );
    }
}

/// The state the connection screen is in decides which buttons do anything,
/// so its transitions and the failure that caused them are in the file: the
/// backend's own account (the step it was at and the connection attempt with
/// its URL, error and duration), the screen's line for what it saw, and the
/// failed command when the person tries again.
///
/// The start-up auto-connect fails here because no signaling server is
/// listening (see [`launch`]).
///
/// Verifies: REQ-GUI-017
#[test]
fn a_failed_start_up_connection_is_written_to_the_log_file_from_both_sides() {
    let (_guard, app) = launch();

    let log = wait_for_log(&app, |log| {
        log.contains("[session] connecting_server -> error")
    });

    assert!(
        log.contains("Signaling connect to http://localhost:17890 failed after"),
        "the Rust side's account of the failed connection is missing:\n{}",
        log
    );
    assert!(
        log.contains("[main] [session] ") && log.contains("-> error"),
        "the screen's line for the failed connection is missing:\n{}",
        log
    );

    app.connection_screen().retry_button().click().unwrap();
    let log = wait_for_log(&app, |log| log.contains("invoke session_connect failed"));
    assert!(
        log.contains("invoke session_connect failed"),
        "the failed command is missing:\n{}",
        log
    );
}

/// The user finds the file from the settings window instead of hunting for the
/// OS-specific folder. The button is not clicked: that would open a real file
/// manager on the machine running the scenarios.
///
/// Verifies: REQ-GUI-020
#[test]
fn the_diagnostics_tab_offers_to_open_the_log_folder() {
    let (_guard, app) = launch();

    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::Diagnostics).unwrap();

    let button = settings.diagnostics_tab().open_log_folder_button();
    button
        .wait_until_visible(UI_TIMEOUT)
        .expect("the diagnostics tab should offer the log folder");
    assert!(button.is_enabled().unwrap());
}

/// Nobody is asked about usage reporting: it is off, nothing pops up on the
/// first launch, and the install ID does not exist until the user turns it on.
///
/// Verifies: REQ-TEL-001
/// Verifies: REQ-TEL-011
#[test]
fn on_the_first_launch_usage_reporting_is_off_and_nothing_asks_about_it() {
    let (_guard, app) = launch();

    let main = app.main_window_html().unwrap();
    assert!(
        !main.contains("role=\"dialog\"") && !main.contains("role=\"alertdialog\""),
        "the first launch should not show a dialog:\n{}",
        main
    );

    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::Diagnostics).unwrap();
    let diagnostics = settings.diagnostics_tab();
    diagnostics
        .usage_reporting_switch()
        .wait_until_visible(UI_TIMEOUT)
        .expect("the diagnostics tab should offer the usage reporting switch");

    assert!(!diagnostics.is_usage_reporting_on().unwrap());
    assert!(!settings.window_html().unwrap().contains("role=\"dialog\""));
    assert!(
        !app.usage_state_dir().join("install_id").exists(),
        "no install ID should exist while usage reporting is off"
    );
}

/// The user turns it on, reads exactly what would be sent (with the install ID),
/// and turns it off again: the lines are gone, so is the ID.
///
/// Verifies: REQ-TEL-002
/// Verifies: REQ-TEL-013
/// Verifies: REQ-TEL-018
#[test]
fn turning_usage_reporting_on_shows_what_is_sent_and_turning_it_off_discards_it() {
    let (_guard, app) = launch();

    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::Diagnostics).unwrap();
    let diagnostics = settings.diagnostics_tab();
    diagnostics
        .usage_reporting_switch()
        .wait_until_visible(UI_TIMEOUT)
        .unwrap();

    diagnostics.usage_reporting_switch().click().unwrap();
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        diagnostics.is_usage_reporting_on().unwrap_or(false)
    })
    .expect("the switch should turn on");
    let install_id_file = app.usage_state_dir().join("install_id");
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || install_id_file.exists())
        .expect("turning it on should create the install ID");
    let install_id = std::fs::read_to_string(&install_id_file).unwrap();

    // The launch report is recorded in the background, so read until it is there.
    let mut shown = String::new();
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        diagnostics.show_usage_button().click().unwrap();
        shown = diagnostics.usage_preview().text().unwrap_or_default();
        shown.contains("\"event\":\"app_start\"")
    })
    .unwrap_or_else(|_| {
        panic!(
            "what is sent never showed the launch report. It shows: {}",
            shown
        )
    });
    assert!(
        shown.contains(install_id.trim()),
        "the lines should carry the install ID:\n{}",
        shown
    );
    for left_out in ["peer_name", "connection_history"] {
        assert!(
            !shown.contains(left_out),
            "{} must not be in what is sent:\n{}",
            left_out,
            shown
        );
    }
    assert!(
        shown.contains("\"server_is_default\":true"),
        "the launch report should say the server is the built-in one:\n{}",
        shown
    );

    diagnostics.usage_reporting_switch().click().unwrap();
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        !diagnostics.is_usage_reporting_on().unwrap_or(true)
    })
    .expect("the switch should turn off");
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        diagnostics.show_usage_button().click().unwrap();
        let text = diagnostics.usage_preview().text().unwrap_or_default();
        !text.contains("install_id") && !text.trim().is_empty()
    })
    .expect("after turning it off, nothing is left to show");
    assert!(
        !install_id_file.exists(),
        "turning it off should discard the install ID"
    );
}

/// Opens the settings window, turns usage reporting on and waits until the
/// launch report is what would be sent.
fn with_usage_reporting_on(app: &App) {
    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::Diagnostics).unwrap();
    let diagnostics = settings.diagnostics_tab();
    diagnostics
        .usage_reporting_switch()
        .wait_until_visible(UI_TIMEOUT)
        .unwrap();
    diagnostics.usage_reporting_switch().click().unwrap();
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        last_usage_line(app, "audio_env").is_some() && last_usage_line(app, "app_start").is_some()
    })
    .expect("turning reporting on should record the launch report");
}

/// The last line of one kind in what would be sent, read from the Diagnostics
/// tab (which it opens). What would be sent is the batch waiting, or the last
/// batch that went out, so a line that was sent earlier is no longer in it.
fn last_usage_line(app: &App, event: &str) -> Option<String> {
    let settings = app.settings_screen();
    settings.select_tab(SettingsTab::Diagnostics).unwrap();
    let diagnostics = settings.diagnostics_tab();
    diagnostics.show_usage_button().click().unwrap();
    let shown = diagnostics.usage_preview().text().unwrap_or_default();
    shown
        .lines()
        .rfind(|line| line.contains(&format!("\"event\":\"{event}\"")))
        .map(str::to_string)
}

/// The line's `seq`, which is new for every line the app records.
fn seq_of(line: &str) -> u64 {
    let json: serde_json::Value = serde_json::from_str(line).unwrap();
    json["seq"].as_u64().unwrap()
}

/// Changing a setting while reporting is on sends the settings again, once,
/// not once per save the app made.
///
/// Verifies: REQ-TEL-014
#[test]
fn changing_a_setting_while_usage_reporting_is_on_reports_the_new_settings_once() {
    let (_guard, app) = launch();
    with_usage_reporting_on(&app);

    let settings = app.settings_screen();
    settings.select_tab(SettingsTab::General).unwrap();
    settings
        .general_tab()
        .language_select()
        .select_value("ja")
        .unwrap();

    let mut reported = None;
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        reported = last_usage_line(&app, "app_start");
        reported
            .as_deref()
            .is_some_and(|line| line.contains("\"language\":\"ja\""))
    })
    .unwrap_or_else(|_| panic!("the new language never showed in what is sent: {reported:?}"));
    let reported = reported.unwrap();

    // Anything else the app saved after the change would have been sent by
    // now, as a newer line.
    std::thread::sleep(Duration::from_secs(4));
    let later = last_usage_line(&app, "app_start").unwrap();
    assert_eq!(
        seq_of(&later),
        seq_of(&reported),
        "the change should have been reported once: {reported}\nthen {later}"
    );
}

/// Choosing another audio device while reporting is on sends the devices
/// again, once. Needs two input devices, like the scenario that switches them.
///
/// Verifies: REQ-TEL-014
#[test]
fn choosing_another_input_device_while_usage_reporting_is_on_reports_the_devices() {
    let (_guard, app) = launch();
    with_usage_reporting_on(&app);

    let settings = app.settings_screen();
    settings.select_tab(SettingsTab::Devices).unwrap();
    let devices = settings.devices_tab();
    let values = devices.input_device_select().option_values().unwrap();
    let initial = devices.input_device_select().value().unwrap();
    let other = values
        .iter()
        .find(|v| !v.is_empty() && **v != initial)
        .unwrap_or_else(|| {
            panic!(
                "this scenario needs two input devices; the machine offers {:?}",
                devices.input_device_select().option_labels().unwrap()
            )
        });
    // Sending the device the panel picked on its own may still be under way.
    std::thread::sleep(Duration::from_secs(4));
    let before = last_usage_line(&app, "audio_env").map(|line| seq_of(&line));
    settings.select_tab(SettingsTab::Devices).unwrap();
    devices.input_device_select().select_value(other).unwrap();

    let mut reported = None;
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        reported = last_usage_line(&app, "audio_env");
        reported
            .as_deref()
            .is_some_and(|line| Some(seq_of(line)) != before && line.contains("\"input\":{"))
    })
    .unwrap_or_else(|_| {
        panic!("choosing {other:?} never reported the devices: {reported:?} (before: {before:?})")
    });
    let reported = reported.unwrap();

    std::thread::sleep(Duration::from_secs(4));
    let later = last_usage_line(&app, "audio_env").unwrap();
    assert_eq!(
        seq_of(&later),
        seq_of(&reported),
        "the change should have been reported once: {reported}\nthen {later}"
    );
}

/// The settings window is a second webview of the same app. It once mounted
/// the connection screen for a frame before switching to the settings, and
/// that screen connects to the signaling server as soon as it mounts, so every
/// opening of the settings opened a second connection. The log showed it: a
/// `[settings] [session]` line and a second `Signaling connect`.
///
/// Verifies: REQ-GUI-007
#[test]
fn opening_the_settings_window_does_not_connect_to_the_signaling_server_again() {
    let (_guard, app) = launch();
    let connects = |log: &str| log.matches("Signaling connect: ").count();
    assert_eq!(connects(&app.log_text()), 1, "the start-up connection");

    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::Devices).unwrap();
    // The settings window has started up once it has listed the devices.
    let log = wait_for_log(&app, |log| log.contains("[settings] invoke settings_get"));

    assert!(
        !log.contains("[settings] [session]"),
        "the settings window showed the connection screen:\n{}",
        log
    );
    assert_eq!(connects(&log), 1, "the settings window connected:\n{}", log);
}

// ---------------------------------------------------------------------------
// Audio device selection
// ---------------------------------------------------------------------------
//
// These need real audio devices but no loopback driver: the machine's own
// input/output devices are enough to prove that the app enumerates them and
// that choosing one takes effect. What they cannot prove is that the chosen
// device's *signal* reaches the meter - that is what a loopback driver would
// add, and it is tracked separately in Plans.md.

/// Opens settings on the Devices tab, once its dropdowns have their devices.
///
/// The tab lists the devices after it opens and selects the default one when
/// none is saved, so reading a dropdown straight away sees it empty or
/// disabled. On a machine with no input device the value stays empty, so the
/// wait is bounded and the scenario carries on.
fn devices_tab(app: &App) -> jamjam_e2e_tests::pom::screens::DevicesTab<'_> {
    app.connection_screen().settings_button().click().unwrap();
    let settings = app.settings_screen();
    settings.wait_until_open(UI_TIMEOUT).unwrap();
    settings.select_tab(SettingsTab::Devices).unwrap();
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        settings.selected_tab().unwrap_or(None) == Some(SettingsTab::Devices)
    })
    .expect("the Devices tab should become selected");
    let devices = app.settings_screen().devices_tab();
    let _ = jamjam_e2e_tests::pom::wait_until(DEVICE_LIST_SETTLE, || {
        devices
            .input_device_select()
            .value()
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    });
    devices
}

/// How long the Devices tab may take to list the devices and pick the default.
const DEVICE_LIST_SETTLE: Duration = Duration::from_secs(3);

/// The app offers the machine's real devices, not an empty list.
///
/// Verifies: REQ-GUI-011
#[test]
fn the_devices_tab_lists_the_machines_audio_devices() {
    let (_guard, app) = launch();
    let devices = devices_tab(&app);

    let inputs = devices.input_device_select().option_labels().unwrap();
    let outputs = devices.output_device_select().option_labels().unwrap();

    assert!(
        !inputs.is_empty(),
        "the input device list should not be empty on a machine with a microphone"
    );
    assert!(
        !outputs.is_empty(),
        "the output device list should not be empty on a machine with speakers"
    );
    assert!(
        inputs.iter().all(|label| !label.trim().is_empty()),
        "every device should be named for the user, got {:?}",
        inputs
    );
}

/// Choosing a different input device takes effect, which is the part of
/// device switching that lives in our code rather than the OS's.
///
/// Verifies: REQ-GUI-011
#[test]
fn choosing_a_different_input_device_takes_effect() {
    let (_guard, app) = launch();
    let devices = devices_tab(&app);

    let values = devices.input_device_select().option_values().unwrap();
    if values.len() < 2 {
        // Reported rather than silently skipped: on a one-microphone machine
        // this scenario cannot distinguish "switching works" from "switching
        // is a no-op", and pretending otherwise would be a hollow pass.
        panic!(
            "this scenario needs at least two input devices; the machine offers {:?}. \
             Attach or enable a second input (any USB mic or loopback driver will do).",
            devices.input_device_select().option_labels().unwrap()
        );
    }

    let initial = devices.input_device_select().value().unwrap();
    let other = values
        .iter()
        .find(|v| **v != initial)
        .expect("a value other than the current one");

    devices.input_device_select().select_value(other).unwrap();

    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        devices.input_device_select().value().as_deref() == Ok(other.as_str())
    })
    .unwrap_or_else(|_| {
        panic!(
            "the dropdown should hold {:?} after choosing it, still shows {:?}",
            other,
            devices.input_device_select().value()
        )
    });

    assert_eq!(
        devices.input_device_select().value().unwrap(),
        *other,
        "the chosen input device should be the selected one"
    );
}

/// A device the app does not offer must be refused rather than silently
/// leaving the old selection - otherwise a test could "switch" to a device
/// that does not exist and still pass.
///
/// Verifies: REQ-GUI-011
#[test]
fn choosing_a_device_that_is_not_offered_fails() {
    let (_guard, app) = launch();
    let devices = devices_tab(&app);

    let before = devices.input_device_select().value().unwrap();
    let result = devices.input_device_select().select_value("no-such-device");

    assert!(
        result.is_err(),
        "choosing an unavailable device should fail, not silently no-op"
    );
    assert_eq!(
        devices.input_device_select().value().unwrap(),
        before,
        "a refused choice must leave the selection untouched"
    );
}

/// The "Select device" placeholder is shown but is not a device. It once
/// counted as a second choice, so on a machine with one microphone
/// `choosing_a_different_input_device_takes_effect` "switched" to it and
/// passed without switching anything.
///
/// Verifies: REQ-GUI-011
#[test]
fn the_select_device_placeholder_cannot_be_chosen() {
    let (_guard, app) = launch();
    let devices = devices_tab(&app);
    let before = devices.input_device_select().value().unwrap();

    assert!(
        !devices
            .input_device_select()
            .option_values()
            .unwrap()
            .contains(&String::new()),
        "the placeholder should not be offered as a choice"
    );
    assert!(
        devices.input_device_select().select_value("").is_err(),
        "choosing the placeholder should fail like any value that is not a device"
    );
    assert_eq!(devices.input_device_select().value().unwrap(), before);
}

// ---------------------------------------------------------------------------
// Calling the app's commands (ADR-043)
// ---------------------------------------------------------------------------
//
// Every feature is reachable as a command, the same way the UI reaches it.
// A scenario uses that to set the app up quickly or to reach what no page
// object covers; a peer helping with the settings uses a few of the same
// commands.

/// A change made without touching the settings window - here through the
/// control channel, as a helping peer's would arrive - shows in the window
/// while it stays open.
///
/// Verifies: REQ-GUI-024
/// Verifies: REQ-GUI-025
#[test]
fn a_setting_changed_by_command_shows_in_the_open_settings_window() {
    let (_guard, app) = launch();
    let devices = devices_tab(&app);
    let before = devices.buffer_size_select().value().unwrap();
    let wanted = if before == "128" { "64" } else { "128" };

    let settings = app
        .change_audio_setting(serde_json::json!({
            "setting": "buffer_size",
            "samples": wanted.parse::<u32>().unwrap(),
        }))
        .unwrap()
        .expect("an offered buffer size should be accepted");

    assert_eq!(settings["buffer_size"].to_string(), wanted);
    jamjam_e2e_tests::pom::wait_until(UI_TIMEOUT, || {
        devices.buffer_size_select().value().as_deref() == Ok(wanted)
    })
    .unwrap_or_else(|_| {
        panic!(
            "the open settings window should show {} samples, still shows {:?}",
            wanted,
            devices.buffer_size_select().value()
        )
    });
    assert_eq!(
        app.audio_settings().unwrap()["buffer_size"].to_string(),
        wanted
    );
}

/// A change the app refuses comes back as the app's answer and leaves the
/// settings as they were.
///
/// Verifies: REQ-GUI-024
/// Verifies: REQ-GUI-025
#[test]
fn a_refused_setting_change_comes_back_as_the_apps_answer_and_changes_nothing() {
    let (_guard, app) = launch();
    let before = app.audio_settings().unwrap()["buffer_size"].clone();

    let answer = app
        .change_audio_setting(serde_json::json!({ "setting": "buffer_size", "samples": 8 }))
        .unwrap();

    let error = answer.expect_err("8 samples is not a size the app offers");
    assert!(error.contains("Invalid buffer size"), "{}", error);
    assert_eq!(app.audio_settings().unwrap()["buffer_size"], before);
}

/// Any command the app registers is callable, not only those a page object
/// wraps; a name the app does not have is an error, not a silent nothing. The
/// permission table that decides this is the one every other portal uses
/// (ADR-044), so an unlisted command is refused before anything runs.
///
/// Verifies: REQ-GUI-025
/// Verifies: REQ-RMT-023
#[test]
fn any_command_the_app_registers_can_be_called_and_an_unknown_one_is_refused() {
    let (_guard, app) = launch();

    let name = app
        .invoke("config_get_peer_name", serde_json::json!({}))
        .unwrap()
        .expect("config_get_peer_name should answer");
    assert!(
        name.as_str().is_some_and(|n| !n.is_empty()),
        "the display name should come back as text, got {}",
        name
    );

    let unknown = app
        .invoke("no_such_command", serde_json::json!({}))
        .unwrap();
    assert!(
        unknown.is_err(),
        "an unknown command should be refused, got {:?}",
        unknown
    );
}

// ---------------------------------------------------------------------------
// Remote debugging through the relay (ADR-044)
// ---------------------------------------------------------------------------
//
// The app is built with `debug-remote` as well as `e2e-control`. The relay is
// a stand-in on loopback that plays the operator; how the real server pairs and
// authenticates is that server's own tests.

/// How long the app may take to ask about enrollment and connect. It asks as
/// soon as it starts, so this is process start-up, not a timer.
const RELAY_CONNECT: Duration = Duration::from_secs(30);

/// Launches an app whose server says it is enrolled, and returns the
/// operator's end of the relay connection it opens.
fn launch_enrolled() -> (MutexGuard<'static, ()>, FakeRelay, App, Target) {
    let guard = exclusive();
    let relay = FakeRelay::start(true).expect("the fake relay should start");
    let app = App::launch_with_server_url(&relay.server_url())
        .expect("app should launch with the e2e-control feature");
    // The screen has to be up before a call reaches it; the relay connection
    // is independent of it.
    app.connection_screen()
        .wait_until_interactive(LAUNCH_SETTLE)
        .expect("connection screen should settle into an operable state");
    let target = relay
        .wait_for_target(RELAY_CONNECT)
        .expect("an enrolled app should open the relay connection");
    (guard, relay, app, target)
}

/// An enrolled app connects by itself, proves which device it is, and speaks
/// first: what it is and which methods this portal may call.
///
/// Verifies: REQ-RMT-021
/// Verifies: REQ-RMT-024
#[test]
fn an_enrolled_app_opens_the_relay_and_says_who_it_is() {
    let (_guard, relay, _app, target) = launch_enrolled();

    let hello = target.recv(Duration::from_secs(10)).unwrap();

    assert_eq!(hello["hello"]["protocol"], "jamjam-rpc/1");
    assert_eq!(hello["hello"]["portal"], "debug");
    assert_eq!(hello["hello"]["build"], "beta");
    let methods: Vec<&str> = hello["hello"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.as_str())
        .collect();
    for expected in ["debug.info", "debug.logs", "ui.query", "settings_change"] {
        assert!(
            methods.contains(&expected),
            "{} is not offered: {:?}",
            expected,
            methods
        );
    }
    assert_eq!(
        relay.device_ids().first().map(String::len),
        Some(26),
        "the app should present its 26-character device id on the handshake"
    );
}

/// An installation the server does not know opens nothing: enrolling is the
/// server's decision, and the app only asks.
///
/// Verifies: REQ-RMT-021
#[test]
fn an_app_that_is_not_enrolled_never_opens_the_relay() {
    let _guard = exclusive();
    let relay = FakeRelay::start(false).expect("the fake relay should start");
    let _app = App::launch_with_server_url(&relay.server_url())
        .expect("app should launch with the e2e-control feature");

    jamjam_e2e_tests::pom::wait_until(RELAY_CONNECT, || relay.enrollment_requests() >= 1)
        .expect("the app should ask whether it is enrolled");
    std::thread::sleep(Duration::from_secs(3));

    assert_eq!(relay.connections(), 0, "a relay connection was opened");
}

/// After the relay drops the connection the app opens a new one by itself.
///
/// Verifies: REQ-RMT-021
#[test]
fn an_app_reopens_the_relay_connection_after_it_drops() {
    let (_guard, relay, _app, target) = launch_enrolled();

    target.hang_up();

    relay
        .wait_for_target(RELAY_CONNECT)
        .expect("the app should reconnect after the relay hung up");
    assert!(relay.connections() >= 2);
}

/// The operator learns what the app is: version, build, OS, devices and the
/// audio settings in effect, and where it logs.
///
/// Verifies: REQ-RMT-026
#[test]
fn the_operator_reads_what_the_app_is() {
    let (_guard, _relay, _app, target) = launch_enrolled();

    let info = target
        .call(1, "debug.info", serde_json::json!({}))
        .unwrap()
        .expect("debug.info should answer");

    assert!(info["app_version"].as_str().is_some_and(|v| !v.is_empty()));
    assert_eq!(info["build"], "e2e");
    assert!(info["os"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(info["audio"]["buffer_size"].is_number(), "{}", info);
    assert!(info["log_file"]
        .as_str()
        .is_some_and(|p| p.ends_with("jamjam.log")));
    assert_eq!(info["device_id"].as_str().map(str::len), Some(26));
}

/// The operator reads the app's log, and can follow it from where the last
/// read ended.
///
/// Verifies: REQ-RMT-026
#[test]
fn the_operator_reads_the_log_and_follows_it() {
    let (_guard, _relay, _app, target) = launch_enrolled();

    let tail = target
        .call(1, "debug.logs", serde_json::json!({}))
        .unwrap()
        .expect("debug.logs should answer");
    assert!(
        tail["text"]
            .as_str()
            .unwrap()
            .contains("Remote debugging: connected to the relay"),
        "the log should say the relay connected: {}",
        tail["text"]
    );

    let next = tail["next_offset"].as_u64().unwrap();
    let more = target
        .call(2, "debug.logs", serde_json::json!({ "offset": next }))
        .unwrap()
        .unwrap();
    assert_eq!(more["offset"].as_u64(), Some(next));
    assert!(more["next_offset"].as_u64().unwrap() >= next);

    let few = target
        .call(
            3,
            "debug.logs",
            serde_json::json!({ "offset": 0, "bytes": 10 }),
        )
        .unwrap()
        .unwrap();
    assert!(few["text"].as_str().unwrap().len() <= 10);
}

/// The crash record and the panics the log holds come back in a fixed shape,
/// empty when nothing crashed.
///
/// Verifies: REQ-RMT-026
#[test]
fn the_operator_asks_for_crashes_and_gets_none_from_a_healthy_app() {
    let (_guard, _relay, _app, target) = launch_enrolled();

    let crashes = target
        .call(1, "debug.crashes", serde_json::json!({}))
        .unwrap()
        .expect("debug.crashes should answer");

    assert!(crashes["record"].is_null());
    assert_eq!(crashes["panics_in_log"], serde_json::json!([]));
}

/// The operator sees and drives the screen the way the E2E channel does.
///
/// Verifies: REQ-RMT-026
#[test]
fn the_operator_looks_at_the_screen_through_ui_methods() {
    let (_guard, _relay, _app, target) = launch_enrolled();

    let windows = target
        .call(1, "ui.windows", serde_json::json!({}))
        .unwrap()
        .unwrap();
    assert_eq!(windows, serde_json::json!(["main"]));

    let body = target
        .call(2, "ui.query", serde_json::json!({ "selector": "body" }))
        .unwrap()
        .unwrap();
    assert_eq!(body["exists"], true);
    assert_eq!(body["visible"], true);

    let missing = target
        .call(
            3,
            "ui.click",
            serde_json::json!({ "selector": "#nothing-here" }),
        )
        .unwrap()
        .unwrap();
    assert_eq!(missing["performed"], false);
}

/// An app command called over the relay does what the same call from the screen
/// does: the change is saved and the settings in effect are the new ones.
///
/// Verifies: REQ-RMT-026
#[test]
fn the_operator_changes_a_setting_the_way_the_screen_does() {
    let (_guard, _relay, app, target) = launch_enrolled();
    let before = app.audio_settings().unwrap()["buffer_size"]
        .as_u64()
        .unwrap();
    let wanted = if before == 128 { 64 } else { 128 };

    let answer = target
        .call(
            1,
            "settings_change",
            serde_json::json!({ "change": { "setting": "buffer_size", "samples": wanted } }),
        )
        .unwrap()
        .expect("settings_change should be accepted");

    assert_eq!(answer["buffer_size"], wanted);
    assert_eq!(app.audio_settings().unwrap()["buffer_size"], wanted);
}

/// A method the table does not list is refused over the relay, with a
/// code the operator can act on, and nothing runs.
///
/// Verifies: REQ-RMT-022
/// Verifies: REQ-RMT-024
#[test]
fn a_method_that_is_not_listed_is_refused_over_the_relay() {
    let (_guard, _relay, _app, target) = launch_enrolled();

    let refused = target
        .call(1, "no_such_command", serde_json::json!({}))
        .unwrap()
        .expect_err("an unlisted method should be refused");

    assert!(refused.contains("unknown_method"), "{}", refused);
}

// ---------------------------------------------------------------------------
// Real audio through the loopback device
// ---------------------------------------------------------------------------
//
// These need a loopback audio driver (macOS: BlackHole, Windows: VB-Cable,
// Linux: a PipeWire null sink). Without it they fail with the install command
// rather than passing vacuously - a meter test that cannot control the input
// would assert nothing.
//
// They are audible: the app plays received audio through the real output
// device, so a tone sounds briefly while they run. Amplitude is kept low.

/// Amplitude of every tone. Well below full scale because the app plays
/// received audio through the real output device, so this is audible.
const TONE_AMPLITUDE: f32 = 0.3;

/// One frequency per scenario, spaced far enough apart to beat rather than
/// cancel: the loopback device keeps what earlier scenarios played into it and
/// mixes the new tone with it, and two sines at the same frequency can sum to
/// nothing (see `pom::loopback_audio`).
const FIXTURE_HZ: f32 = 440.0;

/// How long to watch the loopback input for. Long enough to span several beat
/// periods against leftover audio, so a constructive moment is always caught.
const MEASURE: Duration = Duration::from_millis(500);

/// Establishes that the fixture works before any assertion about the app.
/// If a tone played into the loopback device does not come back on its input,
/// nothing downstream would mean anything.
///
/// Phrased as a rise over whatever the device already holds, because it cannot
/// be returned to silence - the module doc records the measurements.
///
/// Verifies: REQ-GUI-012
#[test]
fn the_loopback_device_returns_what_is_played_into_it() {
    let _guard = exclusive();
    loopback_audio::require_device().expect("loopback device");

    let floor = loopback_audio::measure_input_peak(MEASURE).unwrap();

    let _tone = loopback_audio::play_tone(FIXTURE_HZ, TONE_AMPLITUDE).unwrap();
    let observed = loopback_audio::measure_input_peak(MEASURE).unwrap();

    assert!(
        observed >= floor + TONE_AMPLITUDE * 0.5,
        "a tone at {:.2} should raise the input level well above the {:.3} \
         already in the device, saw {:.3}",
        TONE_AMPLITUDE,
        floor,
        observed
    );
}

/// The 8-channel fixture keeps channels apart: a tone put on one channel comes
/// back on that channel and on no other. This is what lets a scenario ask
/// "which channel did the app read or write?" and trust the answer. Checked
/// before any assertion about the app, like [`the_loopback_device_returns_what_is_played_into_it`].
///
/// The Linux devices hold no history (BlackHole on macOS does, see the module
/// doc of `loopback_audio`), so this asserts levels rather than a rise.
///
/// Verifies: REQ-GUI-012
#[test]
fn the_8ch_loopback_device_returns_each_channel_where_it_was_played() {
    let _guard = exclusive();
    loopback_audio::require_device_8ch().expect("8ch loopback device");

    for channel in [1u16, 2, 5, 6, 8] {
        let peaks = loopback_audio::measure_tone_on_channel_8ch(
            channel,
            FIXTURE_HZ,
            TONE_AMPLITUDE,
            MEASURE,
        )
        .unwrap();

        for (index, peak) in peaks.iter().enumerate() {
            if index + 1 == channel as usize {
                assert!(
                    *peak >= TONE_AMPLITUDE * 0.5,
                    "a tone on channel {} should come back on channel {}, but the peaks are {:?}",
                    channel,
                    channel,
                    peaks
                );
            } else {
                assert!(
                    *peak <= TONE_AMPLITUDE * 0.1,
                    "a tone on channel {} leaked into channel {}: peaks are {:?}",
                    channel,
                    index + 1,
                    peaks
                );
            }
        }
    }
}
