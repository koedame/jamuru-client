//! Restarting the app (ADR-055).
//!
//! Tauri restarts from a worker thread by asking the main thread's event loop
//! to exit and restarting when it has. A main thread that is stuck (a call into
//! a hung audio driver, ADR-050) never gets there: nothing restarts, the old
//! process stays and no new one starts. So the restart is given a time, and
//! when it has not happened by then the new instance is started from here and
//! the old process is ended without waiting for the main thread.

use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

use crate::streaming::{streaming_stop, StreamingState};

/// Audio that is still running is stopped before the restart, so that the
/// exit does not wait for a driver under a live stream. It is not waited for
/// longer than this.
const STOP_AUDIO_LIMIT: Duration = Duration::from_secs(3);

/// How long the exit through the main thread has to restart the app.
const GRACE: Duration = Duration::from_secs(10);

/// How long the old process has to end after the new instance is started.
#[cfg(unix)]
const EXIT_LIMIT: Duration = Duration::from_secs(3);

/// Restarts the app; does not return.
pub(crate) async fn restart<R: Runtime>(app: &AppHandle<R>) {
    stop_audio(app).await;
    let env = app.env();
    after(GRACE, move || start_new_instance(&env));
    tracing::info!("Restarting");
    app.restart()
}

async fn stop_audio<R: Runtime>(app: &AppHandle<R>) {
    let Some(streaming) = app.try_state::<StreamingState>() else {
        return;
    };
    match tokio::time::timeout(STOP_AUDIO_LIMIT, streaming_stop(streaming)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("Stopping the audio before the restart failed: {}", e),
        Err(_) => tracing::warn!(
            "Stopping the audio before the restart did not finish in {:?}",
            STOP_AUDIO_LIMIT
        ),
    }
}

/// Runs `action` on a thread of its own once `delay` has passed. A thread and
/// not a task: the runtime's workers may be the ones that are stuck.
fn after(delay: Duration, action: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        action();
    });
}

fn start_new_instance(env: &tauri::Env) -> ! {
    tracing::error!(
        "The restart did not happen within {:?}; starting the new instance and ending this one",
        GRACE
    );
    // Ending the process runs exit handlers, which can wait on the same thing
    // the main thread does.
    #[cfg(unix)]
    after(EXIT_LIMIT, || unsafe { libc::_exit(0) });
    tauri::process::restart(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Verifies: REQ-UPD-017
    #[test]
    fn when_the_time_has_passed_the_action_runs() {
        let (ran, waiting) = mpsc::channel();

        after(Duration::from_millis(20), move || ran.send(()).unwrap());

        assert!(waiting.recv_timeout(Duration::from_secs(5)).is_ok());
    }
}
