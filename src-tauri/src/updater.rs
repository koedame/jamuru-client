//! Self-update (ADR-041).
//!
//! A release published after the running version is downloaded, checked
//! against the public key in `tauri.conf.json` (`plugins.updater`), and
//! installed without the user doing anything. Nothing here can install or
//! change anything from the webview: no command, no event, no permission for
//! that. The one thing that reaches it is a version number, for a package that
//! cannot replace itself (ADR-075): the screen is told that a newer release
//! exists, and the user installs it by hand.
//!
//! Two rules keep it out of the way of the app's purpose. It never installs
//! while the user is in a session (an install ends with a restart, which would
//! cut the audio), and it never runs in a development build.
//!
//! An update never takes the app down with it. Only one runs at a time (the
//! background loop and `debug.update_apply` would otherwise both replace the
//! same bundle), and each step has a time limit, so a download that stalls or
//! an install that never returns is given up on while the app keeps running.

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::config;
use crate::watchdog::Watchdog;
use crate::windows;

/// Left alone so the first frames and the audio device setup are not competing
/// with a network request.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(30);
const CHECK_LIMIT: Duration = Duration::from_secs(30);
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(10 * 60);
const INSTALL_LIMIT: Duration = Duration::from_secs(2 * 60);

/// Why an update did not happen.
#[derive(Debug)]
pub(crate) enum UpdateError {
    /// Another update is running.
    Busy,
    /// An update is installed and the app is about to restart into it.
    Restarting,
    /// A step did not finish within its time limit.
    TimedOut(&'static str),
    Failed(tauri_plugin_updater::Error),
}

impl From<tauri_plugin_updater::Error> for UpdateError {
    fn from(error: tauri_plugin_updater::Error) -> Self {
        Self::Failed(error)
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => write!(f, "another update is already running"),
            Self::Restarting => write!(f, "an update is installed and the app is restarting"),
            Self::TimedOut(step) => write!(f, "{} did not finish in time", step),
            Self::Failed(error) => write!(f, "{}", error),
        }
    }
}

/// Lets one update at a time run, and none once one has been installed: the
/// installed app only takes over at the restart, so a second update before it
/// would check against the old version and install over the new one.
struct Gate {
    running: AtomicBool,
    installed: AtomicBool,
}

static GATE: Gate = Gate::new();

impl Gate {
    const fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            installed: AtomicBool::new(false),
        }
    }

    /// Refuses at once, never waits, when it is not free.
    fn begin(&self) -> Result<Turn<'_>, UpdateError> {
        if self.installed.load(Ordering::SeqCst) {
            return Err(UpdateError::Restarting);
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(UpdateError::Busy);
        }
        Ok(Turn(self))
    }
}

/// The gate held by an update; freed when it is dropped.
struct Turn<'a>(&'a Gate);

impl Turn<'_> {
    fn installed(&self) {
        self.0.installed.store(true, Ordering::SeqCst);
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

/// What a round of the background loop does.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Round {
    /// Download and install a newer release.
    Install,
    /// Only look whether a newer release exists, and tell the screen.
    Notify,
}

/// What this run does about a newer release, or why it does nothing.
pub(crate) fn plan(
    auto_update: bool,
    development_build: bool,
    package_updates_itself: bool,
) -> Result<Round, &'static str> {
    if development_build {
        Err("development build")
    } else if !auto_update {
        Err("turned off in config.toml")
    } else if !package_updates_itself {
        Ok(Round::Notify)
    } else {
        Ok(Round::Install)
    }
}

/// A build made with `cargo tauri dev`, or the E2E build, must never replace
/// itself with a release: the E2E harness would find the app restarted under
/// it, and a developer would lose the checkout's build.
fn is_development_build() -> bool {
    cfg!(debug_assertions) || cfg!(feature = "e2e-control")
}

/// Only an AppImage can replace itself on Linux, and an `.msi` cannot on
/// Windows. A `.deb` install would need `sudo` and a password prompt, and the
/// `.msi` is always per-machine: run without elevation, `msiexec` fails with
/// Error 1730 after the app has already exited, and nothing starts it again.
/// Neither is "without the user doing anything", so they stay with the package
/// manager and with whoever installed them; the app only tells them that a
/// newer release exists ([`Round::Notify`]). The per-user `-setup.exe` (NSIS)
/// updates itself.
pub(crate) fn package_updates_itself() -> bool {
    bundle_updates_itself(std::env::consts::OS, tauri::utils::platform::bundle_type())
}

fn bundle_updates_itself(os: &str, bundle: Option<tauri::utils::config::BundleType>) -> bool {
    use tauri::utils::config::BundleType;
    match os {
        "linux" => matches!(bundle, Some(BundleType::AppImage)),
        "windows" => !matches!(bundle, Some(BundleType::Msi)),
        _ => true,
    }
}

/// The newer release a package that cannot replace itself has found, if any.
#[derive(Default)]
pub struct UpdateNotice(Mutex<Option<String>>);

/// Heard by the screen when [`UpdateNotice`] gets a version; the payload is the
/// version.
pub(crate) const NOTICE_EVENT: &str = "update:available";

/// The version of a newer release this package cannot install by itself, or
/// `None` when none was found (yet).
#[tauri::command]
pub fn update_notice_get(notice: State<'_, UpdateNotice>) -> Option<String> {
    notice.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Starts the background loop: check shortly after launch, then every six
/// hours. The setting is read again on every round, so turning it off in
/// `config.toml` takes effect without a restart.
pub(crate) fn spawn(app: AppHandle) {
    if is_development_build() {
        tracing::debug!("Self-update is off in a development build");
        return;
    }
    tauri::async_runtime::spawn(async move {
        crate::terms::wait_accepted(&app).await;
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let auto_update = config::load_config().unwrap_or_default().auto_update;
            match plan(auto_update, false, package_updates_itself()) {
                Err(reason) => tracing::debug!("Not checking for an update: {}", reason),
                Ok(Round::Install) => match update_once(&app).await {
                    Ok(()) => {}
                    Err(e @ (UpdateError::Busy | UpdateError::Restarting)) => {
                        tracing::debug!("Not updating: {}", e)
                    }
                    Err(e) => tracing::warn!("Self-update did not finish: {}", e),
                },
                Ok(Round::Notify) => {
                    if let Err(e) = notify_once(&app).await {
                        tracing::warn!("Checking for a newer release did not finish: {}", e)
                    }
                }
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn update_once(app: &AppHandle) -> Result<(), UpdateError> {
    if let Some(version) = install_newer(app, true).await? {
        tracing::info!("Installed {}; restarting", version);
        // Windows: the installer has already ended this process. Elsewhere the
        // new version runs from the next start.
        crate::restart::restart(app).await;
    }
    Ok(())
}

/// The platform the update information is read for when the package has none
/// of its own: the information carries one version for every platform, and
/// only the AppImage has an entry on Linux (`scripts/make-update-manifest.sh`).
/// Release builds exist for x86_64 only.
fn notice_target() -> Option<&'static str> {
    (std::env::consts::OS == "linux").then_some("linux-x86_64-appimage")
}

/// Looks whether a release newer than the running one exists, and tells the
/// screen. Nothing is downloaded or installed.
async fn notify_once<R: Runtime>(app: &AppHandle<R>) -> Result<(), UpdateError> {
    let mut builder = app.updater_builder();
    if let Some(target) = notice_target() {
        builder = builder.target(target);
    }
    let updater = builder.build()?;
    let Some(update) = within("checking for a release", CHECK_LIMIT, updater.check()).await? else {
        tracing::debug!("No newer release");
        return Ok(());
    };
    tracing::info!(
        "Release {} is out (running {}); this package cannot install it by itself",
        update.version,
        update.current_version
    );
    announce(app, update.version);
    Ok(())
}

/// Keeps the version for a screen that opens later, and tells the screens
/// that are open.
fn announce<R: Runtime>(app: &AppHandle<R>, version: String) {
    if let Err(e) = app.emit(NOTICE_EVENT, &version) {
        tracing::warn!("Could not announce the newer release to the screen: {}", e);
    }
    *app.state::<UpdateNotice>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(version);
}

/// Installs a newer release now, without waiting for the user to leave a
/// session: for someone debugging the app remotely (ADR-044), who restarts it
/// afterwards. Returns the version installed, or `None` when there was none.
#[cfg(feature = "debug-tools")]
pub(crate) async fn install_now<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<String>, UpdateError> {
    install_newer(app, false).await
}

/// Downloads and installs a release newer than the running one, and returns
/// its version. With `wait_for_session` it holds back the install until the
/// user is out of a session, since the install ends in a restart. The wait is
/// spent without holding the gate, so an update asked for by hand is not
/// refused for as long as the user stays in the session.
async fn install_newer<R: Runtime>(
    app: &AppHandle<R>,
    wait_for_session: bool,
) -> Result<Option<String>, UpdateError> {
    let turn = GATE.begin()?;
    // The Windows updater ends the process itself once the installer has
    // started, without `RunEvent::Exit`; without this the next launch would
    // report every update as a launch that did not end cleanly.
    let hook_app = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            if let Some(watchdog) = hook_app.try_state::<Watchdog>() {
                watchdog.handing_over_to_installer();
            }
        })
        .build()?;
    let checked = within("checking for a release", CHECK_LIMIT, updater.check()).await?;
    let Some(update) = checked else {
        tracing::debug!("No newer release");
        return Ok(None);
    };
    tracing::info!(
        "Release {} is out (running {}); downloading",
        update.version,
        update.current_version
    );
    // `download` returns only bytes whose signature matches the public key
    // the app was built with.
    let bytes = within(
        "the download",
        DOWNLOAD_LIMIT,
        update.download(|_, _| {}, || {}),
    )
    .await?;

    let turn = if wait_for_session {
        drop(turn);
        wait_until_no_session(windows::is_in_session, SESSION_POLL_INTERVAL).await;
        if !config::load_config().unwrap_or_default().auto_update {
            tracing::info!("Self-update was turned off while downloading; not installing");
            return Ok(None);
        }
        GATE.begin()?
    } else {
        turn
    };

    let version = update.version.clone();
    if let Err(e) = install(update, bytes, turn).await {
        if let Some(watchdog) = app.try_state::<Watchdog>() {
            watchdog.handover_failed();
        }
        return Err(e);
    }
    Ok(Some(version))
}

/// The install writes into the app's own bundle and, on macOS, may wait for an
/// administrator's password on the main thread; so it runs off the async
/// workers, and the gate goes with it, staying shut until the install has
/// really ended even when it is given up on.
async fn install(update: Update, bytes: Vec<u8>, turn: Turn<'static>) -> Result<(), UpdateError> {
    blocking_within("the install", INSTALL_LIMIT, move || {
        update.install(bytes)?;
        turn.installed();
        Ok(())
    })
    .await
}

async fn within<T>(
    step: &'static str,
    limit: Duration,
    work: impl Future<Output = tauri_plugin_updater::Result<T>>,
) -> Result<T, UpdateError> {
    match tokio::time::timeout(limit, work).await {
        Ok(done) => Ok(done?),
        Err(_) => Err(UpdateError::TimedOut(step)),
    }
}

/// Runs `work` on a blocking thread and stops waiting for it after `limit`.
/// The thread cannot be stopped, so one that never returns is left behind.
async fn blocking_within<T: Send + 'static>(
    step: &'static str,
    limit: Duration,
    work: impl FnOnce() -> tauri_plugin_updater::Result<T> + Send + 'static,
) -> Result<T, UpdateError> {
    let running = tauri::async_runtime::spawn_blocking(work);
    match tokio::time::timeout(limit, running).await {
        Ok(Ok(done)) => Ok(done?),
        Ok(Err(e)) => Err(UpdateError::Failed(
            std::io::Error::other(e.to_string()).into(),
        )),
        Err(_) => Err(UpdateError::TimedOut(step)),
    }
}

/// Returns once `in_session` is false. The install ends in a restart, so it
/// waits for the user to leave the session rather than cutting the audio.
async fn wait_until_no_session(in_session: impl Fn() -> bool, poll: Duration) {
    while in_session() {
        tokio::time::sleep(poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Verifies: REQ-UPD-001
    #[test]
    fn when_the_setting_is_on_in_a_release_build_the_app_updates_itself() {
        assert_eq!(plan(true, false, true), Ok(Round::Install));
    }

    /// Verifies: REQ-UPD-002
    #[test]
    fn when_the_setting_is_off_the_app_does_not_update_itself() {
        assert!(plan(false, false, true).is_err());
        assert!(plan(false, false, false).is_err());
    }

    /// Verifies: REQ-UPD-003
    #[test]
    fn when_it_is_a_development_build_the_app_does_not_update_itself() {
        assert!(plan(true, true, true).is_err());
        assert!(plan(true, true, false).is_err());
    }

    /// Verifies: REQ-UPD-004
    #[test]
    fn when_the_package_cannot_replace_itself_the_app_only_looks_for_a_newer_release() {
        assert_eq!(plan(true, false, false), Ok(Round::Notify));
    }

    /// Verifies: REQ-UPD-004
    #[test]
    fn when_it_is_a_windows_msi_the_package_cannot_replace_itself() {
        use tauri::utils::config::BundleType;
        assert!(!bundle_updates_itself("windows", Some(BundleType::Msi)));
    }

    /// Verifies: REQ-UPD-004
    #[test]
    fn when_it_is_a_windows_nsis_installer_the_package_replaces_itself() {
        use tauri::utils::config::BundleType;
        assert!(bundle_updates_itself("windows", Some(BundleType::Nsis)));
    }

    /// Verifies: REQ-UPD-004
    #[test]
    fn when_it_is_a_linux_deb_the_package_cannot_replace_itself() {
        use tauri::utils::config::BundleType;
        assert!(!bundle_updates_itself("linux", Some(BundleType::Deb)));
        assert!(bundle_updates_itself("linux", Some(BundleType::AppImage)));
    }

    /// Verifies: REQ-UPD-021
    #[test]
    fn when_a_newer_release_was_found_the_screen_can_read_its_version() {
        let app = tauri::test::mock_app();
        app.manage(UpdateNotice::default());
        assert_eq!(update_notice_get(app.state()), None);

        announce(app.handle(), "1.2.3".to_string());

        assert_eq!(update_notice_get(app.state()), Some("1.2.3".to_string()));
    }

    /// Verifies: REQ-UPD-021
    #[test]
    fn when_a_newer_release_was_found_the_open_screens_are_told_its_version() {
        use tauri::Listener;
        let app = tauri::test::mock_app();
        app.manage(UpdateNotice::default());
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        app.listen(NOTICE_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_string())
        });

        announce(app.handle(), "1.2.3".to_string());

        assert_eq!(*heard.lock().unwrap(), vec!["\"1.2.3\"".to_string()]);
    }

    /// Verifies: REQ-UPD-005
    #[tokio::test]
    async fn when_the_user_is_in_a_session_the_install_waits_until_it_ends() {
        let asked = AtomicUsize::new(0);
        let in_session = || asked.fetch_add(1, Ordering::SeqCst) < 3;

        wait_until_no_session(in_session, Duration::from_millis(1)).await;

        assert_eq!(asked.load(Ordering::SeqCst), 4);
    }

    /// Verifies: REQ-UPD-005
    #[tokio::test]
    async fn when_the_user_is_not_in_a_session_the_install_does_not_wait() {
        let asked = AtomicUsize::new(0);
        let in_session = || {
            asked.fetch_add(1, Ordering::SeqCst);
            false
        };

        wait_until_no_session(in_session, Duration::from_secs(3600)).await;

        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    /// Verifies: REQ-UPD-015
    #[test]
    fn when_an_update_is_running_another_is_refused_at_once() {
        let gate = Gate::new();
        let _running = gate.begin().unwrap();

        assert!(matches!(gate.begin(), Err(UpdateError::Busy)));
    }

    /// Verifies: REQ-UPD-015
    #[test]
    fn when_an_update_has_ended_without_installing_the_next_may_start() {
        let gate = Gate::new();
        drop(gate.begin().unwrap());

        assert!(gate.begin().is_ok());
    }

    /// Verifies: REQ-UPD-015
    #[test]
    fn when_an_update_is_installed_no_other_starts_before_the_restart() {
        let gate = Gate::new();
        gate.begin().unwrap().installed();

        assert!(matches!(gate.begin(), Err(UpdateError::Restarting)));
    }

    /// Verifies: REQ-UPD-016
    #[tokio::test(start_paused = true)]
    async fn when_a_step_does_not_finish_it_is_given_up_after_its_limit() {
        let stalled = std::future::pending::<tauri_plugin_updater::Result<()>>();

        let result = within("the download", Duration::from_secs(600), stalled).await;

        assert!(matches!(result, Err(UpdateError::TimedOut("the download"))));
    }

    /// Verifies: REQ-UPD-016
    #[tokio::test]
    async fn when_an_install_never_returns_it_is_given_up_and_the_gate_stays_shut_until_it_ends() {
        static GATE: Gate = Gate::new();
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let turn = GATE.begin().unwrap();

        let result = blocking_within("the install", Duration::from_millis(50), move || {
            let _turn = turn;
            let _ = blocked.recv();
            Ok(())
        })
        .await;

        assert!(matches!(result, Err(UpdateError::TimedOut("the install"))));
        assert!(matches!(GATE.begin(), Err(UpdateError::Busy)));
        release.send(()).unwrap();
        for _ in 0..100 {
            if GATE.begin().is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the gate was not freed after the install ended");
    }

    /// The background update is downloading when `debug.update_apply` is
    /// called: the call is refused at once instead of joining it, and the
    /// background update ends at its limit without holding the app.
    ///
    /// Verifies: REQ-UPD-015, REQ-UPD-016
    #[cfg(feature = "debug-tools")]
    #[tokio::test(start_paused = true)]
    async fn when_an_update_is_running_update_apply_is_refused_and_both_end() {
        let background = tokio::spawn(async {
            let _turn = GATE.begin().unwrap();
            within(
                "the download",
                DOWNLOAD_LIMIT,
                std::future::pending::<tauri_plugin_updater::Result<()>>(),
            )
            .await
        });
        tokio::task::yield_now().await;
        let app = tauri::test::mock_app();

        let refused = tokio::time::timeout(Duration::from_secs(1), install_now(app.handle()))
            .await
            .expect("update_apply must answer while another update runs");

        assert!(matches!(refused, Err(UpdateError::Busy)));
        assert!(matches!(
            background.await.unwrap(),
            Err(UpdateError::TimedOut("the download"))
        ));
        assert!(GATE.begin().is_ok());
    }
}
