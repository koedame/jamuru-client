//! Calls into an audio driver that may never come back
//!
//! A driver that has hung (macOS's `coreaudiod`, an interface's driver) does
//! not fail a call, it never returns from it. Whatever thread made the call is
//! then gone with it. So the calls that can hang are made on threads of their
//! own, and the caller waits for them for a limited time instead of for good.
//!
//! A thread that is still inside a hung call cannot be stopped. It is left to
//! finish or to stay: what it produces if it ever comes back is dropped.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cpal::Stream;
use tracing::{debug, warn};

use super::error::AudioError;

/// How long opening a stream may take. Opening one takes a few hundred
/// milliseconds at most; the room above that is for an interface that wakes
/// up slowly.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(8);

/// How long listing the devices may take. It normally takes milliseconds, and
/// the settings window waits for it.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(3);

/// How long closing a stream is waited for. Closing a stream that hung is not
/// waited out: the next open must not sit behind it.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Runs `work` on a thread of its own and waits at most `timeout` for it.
///
/// `what` names the call in the error and the log.
///
/// # Errors
/// [`AudioError::DeviceUnresponsive`] when `work` had not returned by then.
/// It keeps running, and its result is dropped when it does return.
pub fn bounded<T: Send + 'static>(
    what: &str,
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, AudioError> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name(format!("audio-{}", what.replace(' ', "-")))
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| AudioError::DeviceOpenFailed(format!("{}: {}", what, e)))?;
    match rx.recv_timeout(timeout) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            warn!("{} did not return within {:?}", what, timeout);
            Err(AudioError::DeviceUnresponsive(what.to_string()))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(AudioError::StreamError(format!("{} stopped", what)))
        }
    }
}

/// A stream, alive on the thread that opened it
///
/// A stream cannot leave the thread that built it, so the thread stays until
/// the stream is to be closed. That is also what lets the caller stop waiting
/// for an open that hangs: the thread is what hangs, not the caller.
pub(crate) struct StreamHost {
    /// Dropping this tells the thread to close the stream
    close: Option<mpsc::Sender<()>>,
    /// Disconnects when the thread has closed the stream and ended
    ended: mpsc::Receiver<()>,
}

impl StreamHost {
    /// Builds a stream with `build`, on a thread of its own, and waits at
    /// most `timeout` for it.
    ///
    /// # Errors
    /// What `build` failed with, or [`AudioError::DeviceUnresponsive`] when it
    /// had not returned in time. A stream that is built after that is closed
    /// again by the thread that built it.
    pub(crate) fn open(
        what: &str,
        timeout: Duration,
        build: impl FnOnce() -> Result<Stream, AudioError> + Send + 'static,
    ) -> Result<Self, AudioError> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (close_tx, close_rx) = mpsc::channel::<()>();
        let (ended_tx, ended_rx) = mpsc::channel::<()>();
        thread::Builder::new()
            .name(format!("audio-{}", what.replace(' ', "-")))
            .spawn(move || {
                // Lives until the thread ends, so the receiving side sees the end
                let _ended = ended_tx;
                match build() {
                    Ok(stream) => {
                        if ready_tx.send(Ok(())).is_err() {
                            debug!("A stream that finished opening after it was given up on was closed");
                            return;
                        }
                        // Returns when the host is told to close, or dropped
                        let _ = close_rx.recv();
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| AudioError::DeviceOpenFailed(format!("{}: {}", what, e)))?;
        match ready_rx.recv_timeout(timeout) {
            Ok(Ok(())) => Ok(Self {
                close: Some(close_tx),
                ended: ended_rx,
            }),
            Ok(Err(e)) => Err(e),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                warn!("Opening the {} did not finish within {:?}", what, timeout);
                Err(AudioError::DeviceUnresponsive(what.to_string()))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(AudioError::StreamError(format!(
                "opening the {} stopped",
                what
            ))),
        }
    }

    /// Closes the stream, waiting a while for the driver to let go of it.
    pub(crate) fn close(mut self, what: &str) {
        self.close = None;
        if let Err(mpsc::RecvTimeoutError::Timeout) = self.ended.recv_timeout(CLOSE_WAIT) {
            warn!("The {} did not close within {:?}", what, CLOSE_WAIT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Verifies: REQ-AUD-123
    #[test]
    fn a_call_that_returns_in_time_gives_its_result() {
        let result = bounded("test call", Duration::from_secs(5), || 7);

        assert_eq!(result.unwrap(), 7);
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn a_call_that_never_returns_is_given_up_on_at_the_limit() {
        let started = Instant::now();

        let result = bounded("test call", Duration::from_millis(100), || {
            thread::sleep(Duration::from_secs(30));
        });

        assert!(matches!(result, Err(AudioError::DeviceUnresponsive(_))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
