//! Notices a critical operation that does not finish, and a launch that
//! never reached a clean exit at all.
//!
//! Two independent local records feed `telemetry::Hang`, written to disk by
//! `UsageReporter::write_hang` regardless of the `usage_reporting` setting -
//! only sending what is found needs the user's consent, decided at read time
//! in `usage.rs`:
//!
//! - **A stage stuck**: [`Watchdog::enter_stage`] marks a critical operation
//!   (closing, restarting, applying an update, opening an audio device) as
//!   running. A plain OS thread of its own - not a task of the async
//!   runtime, which the same stall can be blocking - checks once a second
//!   whether the stage is still the one entered and, once it has run longer
//!   than its own limit, writes a record naming it and how long it had run.
//!   Dropping the guard ([`StageGuard`]) - however the scope is left, a
//!   panic included - clears the stage, so only one that genuinely never
//!   returns is ever seen as stale.
//! - **No stage, no clean exit**: [`Watchdog::install`] writes a small
//!   marker naming this launch as soon as it starts, and
//!   [`Watchdog::mark_clean_exit`] removes it once `RunEvent::Exit` has run.
//!   A marker still there at the next startup means the launch before it
//!   was ended some other way (closed from outside, the OS reclaiming
//!   memory, power loss) while no stage was being watched - `previous_incident`
//!   folds that into the same shape as a stalled stage, `stage: Unknown` and
//!   no `stalled_ms`, so the rest of the app does not need to know which of
//!   the two happened.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::telemetry::{Hang, HangStage, UsageReporter};
use serde::{Deserialize, Serialize};

/// How often a running stage is checked against its limit.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

const RUNNING_FILE: &str = "running.json";

/// How long a stage may run before it counts as stuck. Chosen well inside
/// anything else that would end the process on its own - restarting has its
/// own 10s grace before ADR-055 starts a new instance regardless, so this
/// fires before that and gives a name to what ADR-055 would otherwise only
/// log.
fn limit_for(stage: HangStage) -> Duration {
    match stage {
        HangStage::AppExit => Duration::from_secs(5),
        HangStage::Restart => Duration::from_secs(8),
        HangStage::UpdateApply => Duration::from_secs(20),
        HangStage::DeviceOpen => Duration::from_secs(3),
        HangStage::Unknown => Duration::from_secs(10),
    }
}

#[derive(Serialize, Deserialize)]
struct RunningMarker {
    launch_id: String,
    app_version: String,
}

fn running_marker_path(dir: &Path) -> PathBuf {
    dir.join(RUNNING_FILE)
}

fn write_running_marker(dir: &Path, launch_id: &str, app_version: &str) {
    let marker = RunningMarker {
        launch_id: launch_id.to_string(),
        app_version: app_version.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&marker) {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(running_marker_path(dir), json);
    }
}

fn take_running_marker(dir: &Path) -> Option<RunningMarker> {
    let path = running_marker_path(dir);
    let content = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    serde_json::from_str(&content).ok()
}

fn clear_running_marker(dir: &Path) {
    let _ = fs::remove_file(running_marker_path(dir));
}

/// This launch's evidence that the one before it did not end cleanly.
/// Prefers what `reporter.previous_hang()` found (a stage the watchdog
/// caught stuck names itself); failing that, a start marker the previous
/// launch never got to remove means it was killed while no stage was being
/// watched.
///
/// Must run before [`Watchdog::install`] writes this launch's own marker -
/// otherwise the leftover this reads would already be this launch's.
pub fn previous_incident(reporter: &UsageReporter) -> Option<(Hang, String, String)> {
    if let Some(found) = reporter.previous_hang() {
        return Some(found);
    }
    let dir = reporter.state_dir()?;
    let marker = take_running_marker(dir)?;
    Some((
        Hang {
            stage: HangStage::Unknown,
            stalled_ms: None,
        },
        marker.launch_id,
        marker.app_version,
    ))
}

struct ActiveStage {
    stage: HangStage,
    started: Instant,
    recorded: bool,
}

struct Shared {
    stage: Mutex<Option<ActiveStage>>,
    stopped: AtomicBool,
}

/// Held for as long as a critical operation runs. Entering a stage while
/// another is already active replaces it (nesting is not expected: each
/// critical operation is its own scope), and dropping the guard clears
/// whichever stage it entered.
#[must_use]
pub struct StageGuard {
    shared: Arc<Shared>,
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        *self.shared.stage.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Watches for a stage that does not return. Cheap to clone: clones share
/// one watchdog.
#[derive(Clone)]
pub struct Watchdog {
    shared: Arc<Shared>,
    dir: Option<PathBuf>,
}

impl Watchdog {
    /// Marks this launch as running (so a startup that never reaches
    /// `mark_clean_exit` can be told apart from one that did) and starts the
    /// background check. Call `previous_incident` with the same `reporter`
    /// first: this overwrites the marker the previous launch may have left.
    pub fn install(reporter: UsageReporter) -> Self {
        let dir = reporter.state_dir().map(Path::to_path_buf);
        if let Some(dir) = &dir {
            write_running_marker(dir, reporter.launch_id(), reporter.app_version());
        }
        let shared = Arc::new(Shared {
            stage: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        spawn_loop(shared.clone(), reporter);
        Watchdog { shared, dir }
    }

    /// Marks a critical operation as running until the returned guard is
    /// dropped.
    pub fn enter_stage(&self, stage: HangStage) -> StageGuard {
        *self.shared.stage.lock().unwrap_or_else(|e| e.into_inner()) = Some(ActiveStage {
            stage,
            started: Instant::now(),
            recorded: false,
        });
        StageGuard {
            shared: self.shared.clone(),
        }
    }

    /// This launch reached a clean exit: removes the marker `install` wrote
    /// and stops the background check (nothing is left to watch).
    pub fn mark_clean_exit(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Some(dir) = &self.dir {
            clear_running_marker(dir);
        }
    }
}

fn spawn_loop(shared: Arc<Shared>, reporter: UsageReporter) {
    std::thread::spawn(move || loop {
        std::thread::sleep(CHECK_INTERVAL);
        if shared.stopped.load(Ordering::SeqCst) {
            return;
        }
        let mut stage = shared.stage.lock().unwrap_or_else(|e| e.into_inner());
        let Some(active) = stage.as_mut() else {
            continue;
        };
        if active.recorded {
            continue;
        }
        let elapsed = active.started.elapsed();
        if elapsed < limit_for(active.stage) {
            continue;
        }
        active.recorded = true;
        let stalled_ms = u32::try_from(elapsed.as_millis()).unwrap_or(u32::MAX);
        tracing::error!(
            "Watchdog: stage {:?} has not finished in {:?}",
            active.stage,
            elapsed
        );
        reporter.write_hang(active.stage, Some(stalled_ms));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reporter(dir: &Path) -> UsageReporter {
        UsageReporter::new(
            Some(dir.to_path_buf()),
            "0.1.2",
            std::sync::Arc::new(jamjam::telemetry::NoTransport),
            false,
        )
    }

    /// Verifies: REQ-TEL-019
    #[test]
    fn a_stage_that_runs_past_its_limit_is_written_with_how_long_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let r = reporter(dir.path());
        let shared = Arc::new(Shared {
            stage: Mutex::new(Some(ActiveStage {
                stage: HangStage::DeviceOpen,
                started: Instant::now() - Duration::from_secs(10),
                recorded: false,
            })),
            stopped: AtomicBool::new(false),
        });

        // One tick of the loop's own body, without the sleep or the thread.
        {
            let mut stage = shared.stage.lock().unwrap();
            let active = stage.as_mut().unwrap();
            let elapsed = active.started.elapsed();
            assert!(elapsed >= limit_for(active.stage));
            active.recorded = true;
            r.write_hang(active.stage, Some(elapsed.as_millis() as u32));
        }

        let (hang, _, _) = r.previous_hang().unwrap();
        assert_eq!(hang.stage, HangStage::DeviceOpen);
        assert!(hang.stalled_ms.unwrap() >= 10_000);
    }

    /// The real path: a stage held past its limit is caught by the running
    /// background thread and left for the next launch.
    #[test]
    fn the_watchdog_thread_records_a_stage_still_active_past_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let r = reporter(dir.path());
        let watchdog = Watchdog::install(r.clone());

        {
            // Backdate the start so the 3s DeviceOpen limit is already past,
            // instead of sleeping the test out for it.
            let guard = watchdog.enter_stage(HangStage::DeviceOpen);
            watchdog
                .shared
                .stage
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .started = Instant::now() - Duration::from_secs(4);
            std::thread::sleep(Duration::from_millis(1200));
            drop(guard);
        }

        let (hang, _, _) = r.previous_hang().expect("the stalled stage was written");
        assert_eq!(hang.stage, HangStage::DeviceOpen);
    }

    /// A stage that clears well inside its limit leaves no record, and a
    /// clean exit removes the running marker `install` wrote - so the next
    /// launch's `previous_incident` finds nothing at all.
    #[test]
    fn a_stage_that_clears_before_its_limit_and_a_clean_exit_leave_no_record() {
        let dir = tempfile::tempdir().unwrap();
        let r = reporter(dir.path());
        let watchdog = Watchdog::install(r.clone());

        {
            let _guard = watchdog.enter_stage(HangStage::DeviceOpen);
            std::thread::sleep(Duration::from_millis(1200));
        }
        std::thread::sleep(Duration::from_millis(1200));
        watchdog.mark_clean_exit();

        assert!(r.previous_hang().is_none());
        assert!(previous_incident(&reporter(dir.path())).is_none());
    }

    /// Verifies: REQ-TEL-019
    #[test]
    fn a_launch_killed_with_no_stage_active_is_found_as_an_unknown_stage_by_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let killed = reporter(dir.path());
        let watchdog = Watchdog::install(killed.clone());
        // The launch ends here without ever calling `mark_clean_exit` -
        // killed, or closed some other way that skips `RunEvent::Exit`.
        drop(watchdog);

        let next_launch = reporter(dir.path());
        let (hang, launch_id, app_version) = previous_incident(&next_launch).unwrap();

        assert_eq!(hang.stage, HangStage::Unknown);
        assert_eq!(hang.stalled_ms, None);
        assert_eq!(launch_id, killed.launch_id());
        assert_eq!(app_version, "0.1.2");
        // Found once: a third launch sees nothing left over.
        assert!(previous_incident(&reporter(dir.path())).is_none());
    }

    /// A stage the watchdog itself caught stuck is the richer record, and is
    /// preferred over the plain "did not exit cleanly" marker even though
    /// both exist for the same launch.
    #[test]
    fn a_known_stalled_stage_is_preferred_over_the_running_marker() {
        let dir = tempfile::tempdir().unwrap();
        let r = reporter(dir.path());
        r.write_hang(HangStage::Restart, Some(9000));
        write_running_marker(dir.path(), r.launch_id(), r.app_version());

        let (hang, _, _) = previous_incident(&reporter(dir.path())).unwrap();

        assert_eq!(hang.stage, HangStage::Restart);
        assert_eq!(hang.stalled_ms, Some(9000));
    }
}
