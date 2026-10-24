//! Self-update (ADR-041).
//!
//! A release published after the running version is downloaded, checked
//! against the public key in `tauri.conf.json` (`plugins.updater`), and
//! installed without the user doing anything. Nothing here can install or
//! change anything from the webview: no command, no event, no permission for
//! that. The one thing that reaches it is a version number, for a package that
//! cannot replace itself (ADR-076): the screen is told that a newer release
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
//!
//! An update never leaves the app unable to start either (ADR-089). A start
//! looks for one before the app opens (`startup`), and every install keeps the
//! version it replaces until the new one has started (`trial`): a new version
//! that does not start is replaced by the kept one again, and is not installed
//! again by itself. Where the kept version could not be put back, nothing is
//! installed by itself.

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::config;
use crate::watchdog::Watchdog;
use crate::windows;

pub(crate) mod startup;
pub(crate) mod trial;

/// Left alone so the first frames and the audio device setup are not competing
/// with a network request.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(30);
const CHECK_LIMIT: Duration = Duration::from_secs(30);
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(10 * 60);
const INSTALL_LIMIT: Duration = Duration::from_secs(2 * 60);
/// A release the app installed by itself that is not what runs afterwards
/// (it reports another version) is not installed by itself again for this
/// long: otherwise every start would install it and restart.
const REINSTALL_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Why an update did not happen.
#[derive(Debug)]
pub(crate) enum UpdateError {
    /// Another update is running.
    Busy,
    /// An update is installed and the app is about to restart into it.
    Restarting,
    /// The user is in a session, and this update would cut the audio.
    InSession,
    /// A step did not finish within its time limit.
    TimedOut(&'static str),
    /// The running version could not be kept to go back to, so the update is
    /// not installed.
    NoWayBack(String),
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
            Self::InSession => write!(f, "the user is in a session"),
            Self::TimedOut(step) => write!(f, "{} did not finish in time", step),
            Self::NoWayBack(why) => write!(f, "the running version cannot be kept: {}", why),
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
    installs_itself: bool,
) -> Result<Round, &'static str> {
    if development_build {
        Err("development build")
    } else if !auto_update {
        Err("turned off in config.toml")
    } else if !installs_itself {
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

/// Whether this package installs releases itself: one that replaces itself,
/// where the running version can be kept to go back to (ADR-089). One that
/// cannot keep it - installed where the app cannot write, say - is told of a
/// newer release instead, as a package that cannot replace itself is.
fn installs_itself() -> bool {
    package_updates_itself() && previous_can_be_kept().is_ok()
}

fn previous_can_be_kept() -> Result<(), String> {
    trial::Store::locate().ok_or("there is no place to keep it")?;
    let package = trial::Package::running()?;
    package
        .writable()
        .map_err(|e| format!("cannot write where it is installed: {}", e))
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
/// hours. A start that has already looked (`startup`) waits the six hours
/// first. The setting is read again on every round, so turning it off in
/// `config.toml` takes effect without a restart.
pub(crate) fn spawn(app: AppHandle, checked_at_start: bool) {
    if is_development_build() {
        tracing::debug!("Self-update is off in a development build");
        return;
    }
    if !installs_itself() && package_updates_itself() {
        if let Err(why) = previous_can_be_kept() {
            tracing::info!("Not installing releases by itself: {}", why);
        }
    }
    let first_delay = if checked_at_start {
        CHECK_INTERVAL
    } else {
        FIRST_CHECK_DELAY
    };
    tauri::async_runtime::spawn(async move {
        crate::terms::wait_accepted(&app).await;
        tokio::time::sleep(first_delay).await;
        loop {
            let auto_update = config::load_config().unwrap_or_default().auto_update;
            match plan(auto_update, false, installs_itself()) {
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
    if let Some(version) = install_newer(app, InSession::Wait).await? {
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

/// Looks for a release newer than the running one. Nothing is downloaded.
/// `notice_only` is for a package that cannot replace itself, which reads the
/// update information as [`notice_target`] does.
async fn find_newer<R: Runtime>(
    app: &AppHandle<R>,
    notice_only: bool,
) -> Result<Option<Update>, UpdateError> {
    let mut builder = app.updater_builder();
    if let Some(target) = notice_target().filter(|_| notice_only) {
        builder = builder.target(target);
    }
    let updater = builder.build()?;
    within("checking for a release", CHECK_LIMIT, updater.check()).await
}

/// Looks whether a release newer than the running one exists, and tells the
/// screen. Nothing is downloaded or installed.
async fn notify_once<R: Runtime>(app: &AppHandle<R>) -> Result<(), UpdateError> {
    let Some(update) = find_newer(app, true).await? else {
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

/// What the app can do about a newer release, as far as this build goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// A development build, or one made from source: it never looks for a
    /// release (ADR-041).
    None,
    /// A package that cannot replace itself (ADR-076), or that cannot keep the
    /// running version to go back to (ADR-089): it can say that a newer
    /// release exists, and the person installs it by hand.
    Notice,
    /// Downloads, installs and restarts into the newer release.
    Install,
}

fn support(development_build: bool, configured: bool, installs_itself: bool) -> Support {
    if development_build || !configured {
        Support::None
    } else if installs_itself {
        Support::Install
    } else {
        Support::Notice
    }
}

fn support_of<R: Runtime>(app: &AppHandle<R>) -> Support {
    let configured = app.config().plugins.0.contains_key("updater");
    if is_development_build() || !configured {
        return Support::None;
    }
    support(false, true, installs_itself())
}

/// What the settings screen shows before anything is checked.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct UpdateStatus {
    /// The running version.
    pub current: String,
    pub support: Support,
}

#[tauri::command]
pub fn update_status<R: Runtime>(app: AppHandle<R>) -> UpdateStatus {
    UpdateStatus {
        current: app.package_info().version.to_string(),
        support: support_of(&app),
    }
}

/// What a check the person asked for found.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Checked {
    Unsupported,
    UpToDate,
    Available { version: String },
}

/// Looks for a newer release now, whatever `auto_update` says: the person
/// asked. Nothing is downloaded. A package that cannot install it by itself
/// also tells the top of the screen, as its background check does (ADR-076).
#[tauri::command]
pub async fn update_check<R: Runtime>(app: AppHandle<R>) -> Result<Checked, String> {
    let support = support_of(&app);
    if support == Support::None {
        return Ok(Checked::Unsupported);
    }
    let found = find_newer(&app, support == Support::Notice)
        .await
        .map_err(|e| e.to_string())?;
    Ok(match found {
        None => Checked::UpToDate,
        Some(update) => {
            if support == Support::Notice {
                announce(&app, update.version.clone());
            }
            Checked::Available {
                version: update.version,
            }
        }
    })
}

/// What an install the person asked for came to. Anything that went wrong is
/// an `Err` instead.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Installed {
    /// This build or package cannot install a release by itself.
    Unsupported,
    /// The user is in a session: the install ends in a restart, which would
    /// cut the audio.
    InSession,
    /// Another update is running, or one is installed and about to restart.
    Busy,
    UpToDate,
    /// Installed; the app is restarting into it.
    Restarting {
        version: String,
    },
}

/// Downloads and installs the newer release, then restarts the app. Never
/// during a session, whatever `auto_update` says. Only the app's own screen
/// can ask (ADR-082); the restart ends the call, so an answer means it did not
/// happen.
#[tauri::command]
pub async fn update_install<R: Runtime>(app: AppHandle<R>) -> Result<Installed, String> {
    if support_of(&app) != Support::Install {
        return Ok(Installed::Unsupported);
    }
    match install_newer(&app, InSession::Refuse).await {
        Ok(None) => Ok(Installed::UpToDate),
        Ok(Some(version)) => {
            tracing::info!("Installed {}; restarting", version);
            crate::restart::restart(&app).await;
            Ok(Installed::Restarting { version })
        }
        Err(UpdateError::InSession) => Ok(Installed::InSession),
        Err(UpdateError::Busy | UpdateError::Restarting) => Ok(Installed::Busy),
        Err(e) => Err(e.to_string()),
    }
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
    install_newer(app, InSession::Install).await
}

/// What an install does when the user is in a session, since the install ends
/// in a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InSession {
    /// Installs anyway: someone debugging the app remotely asked for it.
    #[cfg_attr(not(feature = "debug-tools"), allow(dead_code))]
    Install,
    /// Downloads, then holds back the install until the user is out. The wait
    /// is spent without holding the gate, so an update asked for by hand is not
    /// refused for as long as the user stays in the session.
    Wait,
    /// Gives up with [`UpdateError::InSession`], before the download and again
    /// before the install: the person asked for it by hand, and can ask again
    /// once out of the session.
    Refuse,
}

impl InSession {
    fn refuses(self, in_session_now: bool) -> bool {
        self == InSession::Refuse && in_session_now
    }

    fn asked(self) -> Asked {
        match self {
            InSession::Wait => Asked::ByItself,
            InSession::Install | InSession::Refuse => Asked::ByPerson,
        }
    }
}

/// Who wants an update installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// The app, by itself: it holds back a release that went wrong here
    /// ([`held_back`]).
    ByItself,
    /// A person, by hand or debugging remotely, who installs what they ask for.
    ByPerson,
}

/// Why the app does not install `version` by itself, if it does not.
fn held_back(version: &str) -> Option<&'static str> {
    let store = trial::Store::locate()?;
    if store.is_rejected(version) {
        Some("it did not start here before")
    } else if store.tried_by_itself_lately(version, REINSTALL_AFTER) {
        Some("it was tried a moment ago and is not what runs")
    } else {
        None
    }
}

/// The trial of the update this run installed, until the app hands over to
/// it (`hand_over`).
static OPENED: Mutex<Option<trial::Opened>> = Mutex::new(None);

/// The app is about to end so that the update it installed starts: from here
/// the watcher counts the time to the new version's start. Called by the
/// restart, and on Windows by the updater just before it runs the installer.
pub(crate) fn handing_over() {
    let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(opened) = opened.as_mut() {
        if let Err(e) = opened.hand_over() {
            tracing::warn!("Could not hand over to the installed update: {}", e);
        }
    }
}

/// The updater an install uses. The Windows updater ends the process itself
/// once the installer has started, without `RunEvent::Exit`: the hook says so
/// first, or the next launch would report every update as a launch that did
/// not end cleanly, and the new version would be given no time to start.
fn updater_for_install<R: Runtime>(app: &AppHandle<R>) -> Result<Updater, UpdateError> {
    let hook_app = app.clone();
    Ok(app
        .updater_builder()
        .on_before_exit(move || {
            if let Some(watchdog) = hook_app.try_state::<Watchdog>() {
                watchdog.handing_over_to_installer();
            }
            handing_over();
        })
        .build()?)
}

/// Downloads and installs a release newer than the running one, and returns
/// its version.
async fn install_newer<R: Runtime>(
    app: &AppHandle<R>,
    in_session: InSession,
) -> Result<Option<String>, UpdateError> {
    if in_session.refuses(windows::is_in_session()) {
        return Err(UpdateError::InSession);
    }
    let turn = GATE.begin()?;
    let updater = updater_for_install(app)?;
    let checked = within("checking for a release", CHECK_LIMIT, updater.check()).await?;
    let Some(update) = checked else {
        tracing::debug!("No newer release");
        return Ok(None);
    };
    if in_session.asked() == Asked::ByItself {
        if let Some(reason) = held_back(&update.version) {
            tracing::info!("Not installing {} by itself: {}", update.version, reason);
            return Ok(None);
        }
    }
    // Not downloaded only to be refused: no update is installed over one
    // still being tried.
    if trial::Store::locate().is_some_and(|store| store.is_open()) {
        return Err(UpdateError::NoWayBack(
            "an earlier update is still being tried".into(),
        ));
    }
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

    let turn = match in_session {
        InSession::Wait => {
            drop(turn);
            wait_until_no_session(windows::is_in_session, SESSION_POLL_INTERVAL).await;
            if !config::load_config().unwrap_or_default().auto_update {
                tracing::info!("Self-update was turned off while downloading; not installing");
                return Ok(None);
            }
            GATE.begin()?
        }
        InSession::Install | InSession::Refuse => {
            if in_session.refuses(windows::is_in_session()) {
                return Err(UpdateError::InSession);
            }
            turn
        }
    };

    let version = update.version.clone();
    if let Err(e) = install(update, bytes, turn, in_session.asked()).await {
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
///
/// The running version is kept first (`trial::open`); without it nothing is
/// installed. What the install came to is written down on the same thread,
/// once it has returned, so that an install given up on still leaves the
/// trial as it really ended.
async fn install(
    update: Update,
    bytes: Vec<u8>,
    turn: Turn<'static>,
    asked: Asked,
) -> Result<(), UpdateError> {
    blocking_within("the install", INSTALL_LIMIT, move || {
        let store = trial::Store::locate()
            .ok_or_else(|| UpdateError::NoWayBack("there is no place to keep it".into()))?;
        let package = trial::Package::running().map_err(UpdateError::NoWayBack)?;
        if asked == Asked::ByItself {
            // Noted before anything can fail: a release that cannot be
            // installed here is not downloaded again at every start.
            if let Err(e) = store.tried_by_itself(&update.version) {
                tracing::warn!("Could not note the update tried by itself: {}", e);
            }
        }
        let opened = trial::open(
            &store,
            package,
            &update.current_version,
            &update.version,
            trial::start_watcher,
        )
        .map_err(|e| UpdateError::NoWayBack(e.to_string()))?;
        *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some(opened);
        // On Windows this does not return once the installer has started.
        let installed = update.install(bytes);
        let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
        match installed {
            Ok(()) => {
                if let Some(opened) = opened.as_ref() {
                    // Without it, the update is tried when it next starts,
                    // or put back if the app ends without restarting.
                    if let Err(e) = opened.installed() {
                        tracing::warn!("Could not note the install: {}", e);
                    }
                }
                turn.installed();
                Ok(())
            }
            Err(e) => {
                if let Some(trying) = opened.as_mut() {
                    // On Windows the installer runs only once the app has
                    // ended, so nothing of the package was touched; putting
                    // it back would write over the program that runs.
                    match trying.abandon(cfg!(not(windows))) {
                        Ok(()) => {
                            opened.take();
                        }
                        // Kept, with its lock: the watcher puts the running
                        // version back once the app has ended.
                        Err(put_back) => tracing::error!(
                            "The install failed and the running version could not be put back: {}",
                            put_back
                        ),
                    }
                }
                Err(e.into())
            }
        }
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
    work: impl FnOnce() -> Result<T, UpdateError> + Send + 'static,
) -> Result<T, UpdateError> {
    let running = tauri::async_runtime::spawn_blocking(work);
    match tokio::time::timeout(limit, running).await {
        Ok(Ok(done)) => done,
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

    /// Verifies: REQ-UPD-022
    #[test]
    fn when_it_is_a_release_build_that_updates_itself_the_person_can_install_by_hand() {
        assert_eq!(support(false, true, true), Support::Install);
    }

    /// Verifies: REQ-UPD-022
    #[test]
    fn when_the_package_cannot_replace_itself_the_person_is_only_told_of_a_newer_release() {
        assert_eq!(support(false, true, false), Support::Notice);
    }

    /// Verifies: REQ-UPD-022
    #[test]
    fn when_it_is_a_development_or_self_built_build_there_is_nothing_to_check() {
        assert_eq!(support(true, true, true), Support::None);
        assert_eq!(support(false, false, true), Support::None);
        assert_eq!(support(false, false, false), Support::None);
    }

    /// Verifies: REQ-UPD-022
    #[test]
    fn when_the_screen_asks_for_the_status_it_gets_the_running_version_and_the_support() {
        let app = tauri::test::mock_app();

        let status = update_status(app.handle().clone());

        assert_eq!(status.current, app.package_info().version.to_string());
        assert_eq!(status.support, Support::None);
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({ "current": status.current, "support": "none" })
        );
    }

    /// The screen reads these shapes (`ui/src/lib/tauri.ts`).
    ///
    /// Verifies: REQ-UPD-022
    #[test]
    fn when_a_check_or_an_install_is_answered_the_screen_reads_a_kind_and_a_version() {
        use serde_json::json;
        assert_eq!(
            serde_json::to_value(&Checked::UpToDate).unwrap(),
            json!({ "kind": "up_to_date" })
        );
        assert_eq!(
            serde_json::to_value(&Checked::Available {
                version: "1.2.3".into()
            })
            .unwrap(),
            json!({ "kind": "available", "version": "1.2.3" })
        );
        assert_eq!(
            serde_json::to_value(&Installed::InSession).unwrap(),
            json!({ "kind": "in_session" })
        );
        assert_eq!(
            serde_json::to_value(&Installed::Restarting {
                version: "1.2.3".into()
            })
            .unwrap(),
            json!({ "kind": "restarting", "version": "1.2.3" })
        );
    }

    /// Verifies: REQ-UPD-022
    #[tokio::test]
    async fn when_the_build_cannot_check_a_check_by_hand_says_so_without_the_network() {
        let app = tauri::test::mock_app();

        assert_eq!(
            update_check(app.handle().clone()).await,
            Ok(Checked::Unsupported)
        );
        assert_eq!(
            update_install(app.handle().clone()).await,
            Ok(Installed::Unsupported)
        );
    }

    /// Verifies: REQ-UPD-024
    #[test]
    fn when_the_person_asked_by_hand_and_is_in_a_session_the_install_is_refused() {
        assert!(InSession::Refuse.refuses(true));
        assert!(!InSession::Refuse.refuses(false));
    }

    /// Verifies: REQ-UPD-024
    #[test]
    fn when_the_update_is_automatic_or_remote_it_is_not_refused_for_a_session_here() {
        assert!(!InSession::Wait.refuses(true));
        assert!(!InSession::Install.refuses(true));
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
