//! Remote debugging: the debug portal of a beta build (ADR-044)
//!
//! A beta build asks the server, at start and every half hour, whether this
//! installation is enrolled for remote debugging. Enrolling is done on the
//! server; nothing on this machine turns it on. When it is, the app holds an
//! outbound WebSocket to the server's relay, where someone operating it from
//! elsewhere is paired with it, and serves their requests through the
//! permission table like any other portal.
//!
//! The whole module sits behind the `debug-remote` cargo feature, which is off
//! by default: a release build has no code that could open the connection.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;

use jamjam::network::{connect_remote, discover_remote_enrollment};

use crate::config::ConfigState;
use crate::device_identity::DeviceIdentityState;
use crate::rpc::link::{self, Session};
use crate::rpc::Portal;

/// What a beta build's binary contains and a release build's does not
/// (checked before a release is published).
pub const MARKER: &str = "jamjam-debug-remote/1";

/// How often an installation that is not enrolled asks again.
const RECHECK: Duration = Duration::from_secs(30 * 60);

/// Where the wait before reconnecting starts and stops growing.
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// A connection that lasted this long was a working one: the next wait starts
/// from the shortest again.
const STABLE_AFTER: Duration = Duration::from_secs(60);

/// Starts the loop that keeps the debug portal connected while enrolled. Call
/// from setup; returns immediately.
pub fn spawn(app: AppHandle) {
    // The server enrolls a device by this ID; the log is where its owner reads it from.
    tracing::info!(
        "Remote debugging is built in ({}); this device is {}",
        MARKER,
        app.state::<DeviceIdentityState>().identity().device_id()
    );
    crate::rpc::events::install(&app);
    tauri::async_runtime::spawn(async move {
        let mut wait = RETRY_MIN;
        loop {
            let pause = match round(&app).await {
                Round::NotEnrolled => {
                    wait = RETRY_MIN;
                    RECHECK
                }
                Round::Connected(lasted) => {
                    wait = next_wait(wait, lasted);
                    wait
                }
                Round::Failed => {
                    wait = next_wait(wait, Duration::ZERO);
                    wait
                }
            };
            tokio::time::sleep(pause).await;
        }
    });
}

/// What one pass of the loop came to.
enum Round {
    /// The server answered that this installation is not enrolled.
    NotEnrolled,
    /// The relay connection was made and has since ended, after this long.
    Connected(Duration),
    /// The server could not be asked, or the relay not reached.
    Failed,
}

/// How long to wait before the next try after one that ended. A connection
/// that held is a reason to come back soon; one that did not is a reason to
/// back off, up to [`RETRY_MAX`].
fn next_wait(previous: Duration, lasted: Duration) -> Duration {
    if lasted >= STABLE_AFTER {
        RETRY_MIN
    } else {
        (previous * 2).min(RETRY_MAX)
    }
}

async fn round(app: &AppHandle) -> Round {
    let server_url = app.state::<ConfigState>().server_url();
    let identity = app.state::<DeviceIdentityState>().identity();

    let enrollment = match discover_remote_enrollment(&server_url, &identity).await {
        Ok(enrollment) => enrollment,
        Err(e) => {
            tracing::debug!("Remote debugging: the server could not be asked: {}", e);
            return Round::Failed;
        }
    };
    if !enrollment.enrolled {
        tracing::debug!("Remote debugging: this installation is not enrolled");
        return Round::NotEnrolled;
    }

    let (mut writer, mut reader) = match connect_remote(&enrollment.url, &identity).await {
        Ok(halves) => halves,
        Err(e) => {
            tracing::warn!("Remote debugging: the relay could not be reached: {}", e);
            return Round::Failed;
        }
    };
    tracing::info!("Remote debugging: connected to the relay");
    let started = Instant::now();

    let (to_app, incoming) = mpsc::channel::<String>(32);
    let (outgoing, mut from_app) = mpsc::channel::<String>(32);
    let session = Session {
        portal: Portal::Debug,
        build: "beta",
        app_version: app.package_info().version.to_string(),
    };
    let serving = tauri::async_runtime::spawn(link::run(app.clone(), session, incoming, outgoing));

    // The relay's frames go to the link and the link's answers go back, until
    // either end is gone.
    loop {
        tokio::select! {
            frame = reader.recv_text() => match frame {
                Ok(Some(text)) => {
                    if to_app.send(text).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("Remote debugging: the relay connection failed: {}", e);
                    break;
                }
            },
            answer = from_app.recv() => match answer {
                Some(text) => {
                    if let Err(e) = writer.send_text(text).await {
                        tracing::warn!("Remote debugging: sending to the relay failed: {}", e);
                        break;
                    }
                }
                None => break,
            },
        }
    }
    serving.abort();
    writer.close().await;
    tracing::info!("Remote debugging: the relay connection ended");
    Round::Connected(started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-RMT-021
    #[test]
    fn after_a_failure_the_wait_doubles_up_to_a_minute() {
        let mut wait = RETRY_MIN;
        let mut seen = Vec::new();
        for _ in 0..6 {
            wait = next_wait(wait, Duration::ZERO);
            seen.push(wait.as_secs());
        }
        assert_eq!(seen, vec![10, 20, 40, 60, 60, 60]);
    }

    /// Verifies: REQ-RMT-021
    #[test]
    fn after_a_connection_that_held_the_wait_starts_over() {
        assert_eq!(next_wait(RETRY_MAX, STABLE_AFTER), RETRY_MIN);
        assert_eq!(next_wait(RETRY_MAX, Duration::from_secs(3)), RETRY_MAX);
    }
}
