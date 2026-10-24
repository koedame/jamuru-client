//! Updating before the app opens (ADR-088).
//!
//! A start of a build that installs releases itself first looks for a newer
//! release, before the main window opens. One that is found is downloaded and
//! installed, and the app restarts into it. Whatever does not go - no network,
//! no newer release, a download that is too slow, an install that fails, the
//! person skipping it - leaves the app to open as it is.
//!
//! The version an install replaces is kept until the new one's screen has been
//! up for [`STABLE`] (`trial`): one that does not get there is replaced by it
//! again, and is not installed again by itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;

use super::trial::{self, Booted, Candidate, Decision};
use super::{Support, GATE};
use crate::windows::labels;

/// How long a start waits to hear whether a newer release exists.
const CHECK_LIMIT: Duration = Duration::from_secs(10);
/// How long a start waits for the download. The person can skip it sooner.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(3 * 60);
/// A check that answers within this opens no window: most starts find nothing.
const QUIET_CHECK: Duration = Duration::from_secs(1);
/// How long the new version's screen has been up when it counts as started.
const STABLE: Duration = Duration::from_secs(10);

/// Heard by the window shown while updating; the payload is a [`Progress`].
pub(crate) const PROGRESS_EVENT: &str = "update:startup";

/// What a start found before anything of the app ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Began {
    /// A build that does not install releases itself.
    Off,
    Normal,
    /// The first start of a version that was just installed.
    Trying(String),
    /// Another start finishes what this one found; this one ends.
    Exit,
}

/// The trial this start takes part in, until it is decided. Held for as long
/// as the process lives: its lock tells the watcher the new version runs.
static CANDIDATE: Mutex<Option<Candidate>> = Mutex::new(None);
/// The main window's screen has said it is up.
static READY: AtomicBool = AtomicBool::new(false);

/// Looks for an update being tried, before the app's own start. In a build
/// that installs releases itself, this comes before anything else: a start
/// during a put-back must end before it has touched anything.
pub(crate) fn begin(version: &str) -> Began {
    if super::is_development_build() {
        return Began::Off;
    }
    let Some(store) = trial::Store::locate() else {
        return Began::Normal;
    };
    match trial::boot_app(&store, version, trial::start_watcher) {
        Booted::Normal => Began::Normal,
        Booted::Exit => Began::Exit,
        Booted::Candidate(candidate) => {
            let version = candidate.version().to_string();
            *lock(&CANDIDATE) = Some(*candidate);
            Began::Trying(version)
        }
    }
}

/// Says in the app's log what [`begin`] found, which ran before the log.
pub(crate) fn log_began(began: &Began) {
    match began {
        Began::Trying(version) => tracing::info!(
            "Trying {} after an update; the previous version is kept until it has started",
            version
        ),
        Began::Normal | Began::Off | Began::Exit => {}
    }
}

/// Whether this start updates before the app opens, or why not.
fn plan(
    began: &Began,
    support: Support,
    auto_update: bool,
    terms_accepted: bool,
    opened_by_link: bool,
) -> Result<(), &'static str> {
    if *began != Began::Normal {
        Err("an update is being tried")
    } else if support != Support::Install {
        Err("this build or package does not install releases itself")
    } else if !auto_update {
        Err("turned off in config.toml")
    } else if !terms_accepted {
        // Nothing talks to the network by itself before the terms of use are
        // agreed to.
        Err("the terms of use are not agreed to yet")
    } else if opened_by_link {
        Err("an invite link opened the app")
    } else {
        Ok(())
    }
}

/// Runs once the event loop runs: updates first where [`plan`] says so, then
/// `open` opens the app. `open` is told whether a check found what there is
/// to find, so the background loop does not look again at once.
pub(crate) fn run(app: AppHandle, began: Began, open: impl FnOnce(&AppHandle, bool) + Send + 'static) {
    let terms = app
        .try_state::<crate::terms::TermsState>()
        .is_some_and(|terms| terms.is_accepted());
    let planned = plan(
        &began,
        super::support_of(&app),
        crate::config::load_config().unwrap_or_default().auto_update,
        terms,
        opened_by_link(&app),
    );
    if let Err(reason) = planned {
        tracing::debug!("Not updating before the app opens: {}", reason);
        open(&app, false);
        return;
    }
    // An invite link that comes while updating is followed rather than kept
    // waiting.
    app.deep_link().on_open_url(|_| skip());
    tauri::async_runtime::spawn(async move {
        let checked = update_first(&app).await;
        open(&app, checked);
        close_window(&app);
    });
}

fn opened_by_link(app: &AppHandle) -> bool {
    match app.deep_link().get_current() {
        Ok(urls) => urls.is_some_and(|urls| !urls.is_empty()),
        Err(e) => {
            tracing::debug!("Could not read the link that opened the app: {}", e);
            false
        }
    }
}

/// What the window shown while updating says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Progress {
    Checking,
    Downloading { received: u64, total: Option<u64> },
    /// From here it is not skipped.
    Installing,
    Restarting,
}

/// Where the update before the app opens has got to; `None` when there is
/// none.
static PROGRESS: Mutex<Option<Progress>> = Mutex::new(None);
static SKIPPED: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The person does not wait for the update: the app opens as it is. Only
/// while checking or downloading; an install, once begun, is let finish.
fn skip() {
    let progress = lock(&PROGRESS);
    if matches!(
        *progress,
        Some(Progress::Checking | Progress::Downloading { .. })
    ) {
        SKIPPED.store(true, Ordering::SeqCst);
    }
}

fn skipped() -> bool {
    SKIPPED.load(Ordering::SeqCst)
}

fn report(app: &AppHandle, progress: Progress) {
    *lock(&PROGRESS) = Some(progress);
    if let Err(e) = app.emit_to(labels::UPDATING, PROGRESS_EVENT, progress) {
        tracing::debug!("Could not tell the updating window: {}", e);
    }
}

/// Moves on to the install unless the person has skipped the update; from
/// then on [`skip`] does nothing.
fn begin_install(app: &AppHandle) -> bool {
    {
        let mut progress = lock(&PROGRESS);
        if skipped() {
            return false;
        }
        *progress = Some(Progress::Installing);
    }
    report(app, Progress::Installing);
    true
}

/// Runs `work` until it ends or the person skips. `None` when skipped.
async fn unless_skipped<T>(work: impl std::future::Future<Output = T>) -> Option<T> {
    tokio::select! {
        done = work => Some(done),
        _ = async {
            while !skipped() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } => None,
    }
}

/// Updates if a newer release is found. Returns whether the check answered.
/// Does not return when the update is installed: the app restarts.
async fn update_first(app: &AppHandle) -> bool {
    let turn = match GATE.begin() {
        Ok(turn) => turn,
        Err(e) => {
            tracing::debug!("Not updating before the app opens: {}", e);
            return false;
        }
    };
    *lock(&PROGRESS) = Some(Progress::Checking);
    let updater = match super::updater_for_install(app) {
        Ok(updater) => updater,
        Err(e) => {
            tracing::warn!("Could not look for an update before the app opens: {}", e);
            return false;
        }
    };
    let shown = app.clone();
    let window = tauri::async_runtime::spawn(async move {
        tokio::time::sleep(QUIET_CHECK).await;
        show_window(&shown);
    });
    let found = unless_skipped(super::within(
        "checking for a release",
        CHECK_LIMIT,
        updater.check(),
    ))
    .await;
    window.abort();
    let update = match found {
        None => {
            tracing::info!("The update before the app opens was skipped while checking");
            return false;
        }
        Some(Err(e)) => {
            tracing::warn!("Could not look for an update before the app opens: {}", e);
            return false;
        }
        Some(Ok(None)) => {
            tracing::debug!("No newer release");
            return true;
        }
        Some(Ok(Some(update))) => update,
    };
    if let Some(reason) = super::held_back(&update.version) {
        tracing::info!("Not installing {} by itself: {}", update.version, reason);
        return true;
    }
    tracing::info!(
        "Release {} is out (running {}); updating before the app opens",
        update.version,
        update.current_version
    );
    show_window(app);
    report(
        app,
        Progress::Downloading {
            received: 0,
            total: None,
        },
    );
    let mut meter = Meter::default();
    let downloaded = unless_skipped(super::within(
        "the download",
        DOWNLOAD_LIMIT,
        update.download(
            |chunk, total| {
                if let Some(progress) = meter.add(chunk, total) {
                    report(app, progress);
                }
            },
            || {},
        ),
    ))
    .await;
    let bytes = match downloaded {
        None => {
            tracing::info!("The update before the app opens was skipped while downloading");
            return true;
        }
        Some(Err(e)) => {
            tracing::warn!("The update before the app opens did not download: {}", e);
            return true;
        }
        Some(Ok(bytes)) => bytes,
    };
    if !begin_install(app) {
        tracing::info!("The update before the app opens was skipped");
        return true;
    }
    let version = update.version.clone();
    match super::install(update, bytes, turn, super::Asked::ByItself).await {
        Ok(()) => {
            tracing::info!("Installed {}; restarting into it", version);
            report(app, Progress::Restarting);
            crate::restart::restart(app).await;
            true
        }
        Err(e) => {
            tracing::warn!("The update before the app opens did not install: {}", e);
            if let Some(watchdog) = app.try_state::<crate::watchdog::Watchdog>() {
                watchdog.handover_failed();
            }
            true
        }
    }
}

/// Counts the download and says when there is more to show: each whole
/// percent, or each mebibyte when the size is not known.
#[derive(Default)]
struct Meter {
    received: u64,
    shown: Option<u64>,
}

impl Meter {
    fn add(&mut self, chunk: usize, total: Option<u64>) -> Option<Progress> {
        self.received += chunk as u64;
        let step = match total {
            Some(total) if total > 0 => self.received.min(total) * 100 / total,
            _ => self.received >> 20,
        };
        if self.shown == Some(step) {
            return None;
        }
        self.shown = Some(step);
        Some(Progress::Downloading {
            received: self.received,
            total,
        })
    }
}

fn show_window(app: &AppHandle) {
    if app.get_webview_window(labels::UPDATING).is_some() {
        return;
    }
    let built = WebviewWindowBuilder::new(
        app,
        labels::UPDATING,
        WebviewUrl::App("index.html#/updating".into()),
    )
    .title("jamuru")
    .inner_size(380.0, 200.0)
    .resizable(false)
    .center()
    .build();
    if let Err(e) = built {
        tracing::warn!("Could not show the updating window: {}", e);
    }
}

/// After the main window is open: the app does not end for having no window
/// in between.
fn close_window(app: &AppHandle) {
    *lock(&PROGRESS) = None;
    if let Some(window) = app.get_webview_window(labels::UPDATING) {
        if let Err(e) = window.destroy() {
            tracing::warn!("Could not close the updating window: {}", e);
        }
    }
}

/// Closing the updating window skips the update. Returns whether the close
/// is held back: the window goes once the app has opened.
pub(crate) fn close_requested(label: &str) -> bool {
    if label != labels::UPDATING {
        return false;
    }
    skip();
    true
}

/// Where the update before the app opens has got to, for a window that opens
/// after it began.
#[tauri::command]
pub fn update_startup_progress() -> Option<Progress> {
    *lock(&PROGRESS)
}

/// Opens the app without waiting for the update.
#[tauri::command]
pub fn update_startup_skip() {
    skip();
}

/// The main window's screen is up. A version being tried counts as started
/// once it has stayed up for [`STABLE`].
#[tauri::command]
pub fn update_app_ready(window: WebviewWindow) {
    if window.label() != labels::MAIN || READY.swap(true, Ordering::SeqCst) {
        return;
    }
    if lock(&CANDIDATE).is_none() {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(STABLE);
        confirm();
    });
}

fn confirm() {
    let mut candidate = lock(&CANDIDATE);
    let Some(trying) = candidate.as_ref() else {
        return;
    };
    match trying.confirm() {
        Ok(Decision::Confirmed) => {
            tracing::info!("{} has started; the previous version is let go", trying.version());
            // Its lock goes with it: the trial is over.
            candidate.take();
        }
        // Still held: the watcher ends this process before it puts the
        // previous version back, which it must not do under a running app.
        Ok(decided) => tracing::warn!(
            "{} was {:?} before its screen had been up long enough; the previous version is put back",
            trying.version(),
            decided
        ),
        Err(e) => tracing::warn!("Could not say that {} has started: {}", trying.version(), e),
    }
}

/// The app is ending. A version being tried whose screen came up has started;
/// one quit before then is replaced by the previous version, which is not
/// started again.
pub(crate) fn exiting() {
    if lock(&CANDIDATE).is_none() {
        return;
    }
    if READY.load(Ordering::SeqCst) {
        confirm();
    } else if !crate::restart::is_restarting() {
        if let Some(trying) = lock(&CANDIDATE).as_ref() {
            if let Err(e) = trying.quitting() {
                tracing::warn!("Could not say that {} was quit: {}", trying.version(), e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-UPD-025
    #[test]
    fn when_a_build_that_installs_releases_starts_as_usual_it_updates_before_it_opens() {
        assert_eq!(plan(&Began::Normal, Support::Install, true, true, false), Ok(()));
    }

    /// Verifies: REQ-UPD-025
    #[test]
    fn when_nothing_may_update_the_app_opens_at_once() {
        let normal = Began::Normal;
        for (began, support, auto, terms, link) in [
            (&Began::Off, Support::Install, true, true, false),
            (&Began::Trying("0.2.0".into()), Support::Install, true, true, false),
            (&normal, Support::Notice, true, true, false),
            (&normal, Support::None, true, true, false),
            (&normal, Support::Install, false, true, false),
            (&normal, Support::Install, true, false, false),
            (&normal, Support::Install, true, true, true),
        ] {
            assert!(
                plan(began, support, auto, terms, link).is_err(),
                "{:?} {:?} {} {} {}",
                began,
                support,
                auto,
                terms,
                link
            );
        }
    }

    /// The statics are the process's own, so the tests that touch them run
    /// one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn at(progress: Option<Progress>) {
        *lock(&PROGRESS) = progress;
        SKIPPED.store(false, Ordering::SeqCst);
    }

    /// Verifies: REQ-UPD-025
    #[test]
    fn while_checking_or_downloading_the_person_can_skip_the_update() {
        let _serial = lock(&SERIAL);
        for progress in [
            Progress::Checking,
            Progress::Downloading {
                received: 10,
                total: Some(100),
            },
        ] {
            at(Some(progress));
            skip();
            assert!(skipped(), "{:?}", progress);
        }
        at(None);
    }

    /// Verifies: REQ-UPD-025
    #[test]
    fn once_the_install_has_begun_or_when_nothing_is_updating_a_skip_does_nothing() {
        let _serial = lock(&SERIAL);
        for progress in [None, Some(Progress::Installing), Some(Progress::Restarting)] {
            at(progress);
            skip();
            assert!(!skipped(), "{:?}", progress);
        }
        at(None);
    }

    /// Verifies: REQ-UPD-025
    #[test]
    fn closing_the_updating_window_skips_the_update_and_other_windows_close_as_usual() {
        let _serial = lock(&SERIAL);
        at(Some(Progress::Checking));
        assert!(!close_requested(labels::MAIN));
        assert!(!skipped());
        assert!(close_requested(labels::UPDATING));
        assert!(skipped());
        at(None);
    }

    /// Verifies: REQ-UPD-025
    #[test]
    fn a_skip_ends_the_wait_for_what_is_being_waited_for() {
        let _serial = lock(&SERIAL);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            at(Some(Progress::Checking));
            let waited = tokio::spawn(unless_skipped(std::future::pending::<()>()));
            tokio::time::sleep(Duration::from_millis(50)).await;
            skip();
            let waited = tokio::time::timeout(Duration::from_secs(2), waited)
                .await
                .expect("the skip did not end the wait");
            assert_eq!(waited.unwrap(), None);
            assert_eq!(unless_skipped(async { 7 }).await, None, "skipped stays skipped");
            at(None);
            assert_eq!(unless_skipped(async { 7 }).await, Some(7));
        });
    }

    /// Verifies: REQ-UPD-025
    #[test]
    fn the_download_is_shown_by_whole_percent_or_by_mebibyte() {
        let mut meter = Meter::default();
        assert_eq!(
            meter.add(0, Some(1000)),
            Some(Progress::Downloading {
                received: 0,
                total: Some(1000)
            })
        );
        assert_eq!(meter.add(5, Some(1000)), None);
        assert_eq!(
            meter.add(5, Some(1000)),
            Some(Progress::Downloading {
                received: 10,
                total: Some(1000)
            })
        );
        let mut unknown = Meter::default();
        assert!(unknown.add(1, None).is_some());
        assert_eq!(unknown.add(1000, None), None);
        assert_eq!(
            unknown.add(1 << 20, None),
            Some(Progress::Downloading {
                received: 1001 + (1 << 20),
                total: None
            })
        );
    }

    /// The window shown while updating reads this shape
    /// (`ui/src/lib/tauri.ts`).
    ///
    /// Verifies: REQ-UPD-025
    #[test]
    fn the_updating_window_reads_a_phase_and_the_download_counts() {
        use serde_json::json;
        assert_eq!(
            serde_json::to_value(Progress::Checking).unwrap(),
            json!({ "phase": "checking" })
        );
        assert_eq!(
            serde_json::to_value(Progress::Downloading {
                received: 3,
                total: Some(9)
            })
            .unwrap(),
            json!({ "phase": "downloading", "received": 3, "total": 9 })
        );
        assert_eq!(
            serde_json::to_value(Progress::Installing).unwrap(),
            json!({ "phase": "installing" })
        );
    }
}
