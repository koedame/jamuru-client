//! One direction of a running session's audio, and the device being switched to
//!
//! Opening a device can take as long as a hung driver likes, and the session
//! thread also carries the network (`streaming`), so it must not do the
//! opening itself. The engine goes to a thread of its own for the open, and
//! the session thread looks in on it every turn. Asking for another device
//! while one is still being opened gives up on that open: the engine it holds
//! is left behind, and a new one is made at once, so the switch is never
//! queued behind a driver that does not answer.

use std::sync::mpsc;
use std::thread;

use jamjam::audio::{AudioConfig, AudioEngine, AudioError, DeviceId};

/// What an open leaves behind: the engine, back from the thread that used it,
/// and how the open went
type Opened<T> = (AudioEngine, Result<T, AudioError>);

pub(crate) struct DeviceSlot<T> {
    /// "input" or "output", for the log
    what: &'static str,
    config: AudioConfig,
    /// The engine, when no open holds it
    engine: Option<AudioEngine>,
    /// The open in progress
    opening: Option<mpsc::Receiver<Opened<T>>>,
    /// The device last asked for; `None` is the system default
    device: Option<DeviceId>,
}

impl<T: Send + 'static> DeviceSlot<T> {
    /// A slot around `engine`, which has `device` open (or has just failed to)
    pub(crate) fn new(
        what: &'static str,
        config: AudioConfig,
        engine: AudioEngine,
        device: Option<DeviceId>,
    ) -> Self {
        Self {
            what,
            engine: Some(engine),
            config,
            opening: None,
            device,
        }
    }

    /// The device last asked for; `None` is the system default
    pub(crate) fn device(&self) -> Option<&DeviceId> {
        self.device.as_ref()
    }

    /// Starts opening `device`: `open` runs on a thread of its own, on the
    /// engine. Returns at once; [`poll`](Self::poll) says how it went.
    pub(crate) fn switch(
        &mut self,
        device: Option<DeviceId>,
        open: impl FnOnce(&mut AudioEngine) -> Result<T, AudioError> + Send + 'static,
    ) {
        self.device = device;
        let engine = match self.engine.take() {
            Some(engine) => engine,
            None => {
                // An open is still out. It keeps its engine, and hands nothing
                // back that anyone reads.
                tracing::warn!(
                    "Switching the {} device again before the last switch finished",
                    self.what
                );
                self.opening = None;
                AudioEngine::new(self.config.clone())
            }
        };
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name(format!("audio-switch-{}", self.what))
            .spawn(move || {
                let mut engine = engine;
                let result = open(&mut engine);
                // When the open was given up on, the receiver is gone and
                // the engine is closed here instead.
                let _ = tx.send((engine, result));
            });
        match spawned {
            Ok(_) => self.opening = Some(rx),
            Err(e) => {
                tracing::error!("Could not start the {} device switch: {}", self.what, e);
                self.engine = Some(AudioEngine::new(self.config.clone()));
            }
        }
    }

    /// How the open that was started went, once it has finished
    pub(crate) fn poll(&mut self) -> Option<Result<T, AudioError>> {
        let opening = self.opening.as_ref()?;
        match opening.try_recv() {
            Ok((engine, result)) => {
                self.engine = Some(engine);
                self.opening = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.engine = Some(AudioEngine::new(self.config.clone()));
                self.opening = None;
                Some(Err(AudioError::StreamError(format!(
                    "the {} switch stopped",
                    self.what
                ))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn slot() -> DeviceSlot<&'static str> {
        DeviceSlot::new(
            "input",
            AudioConfig::default(),
            AudioEngine::new(AudioConfig::default()),
            None,
        )
    }

    fn wait_for<T: Send + 'static>(slot: &mut DeviceSlot<T>) -> Result<T, AudioError> {
        let started = Instant::now();
        loop {
            if let Some(result) = slot.poll() {
                return result;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the open never finished"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn switching_returns_at_once_and_the_result_comes_when_the_open_finishes() {
        let mut slot = slot();
        let started = Instant::now();

        slot.switch(Some(DeviceId("b".into())), |_| {
            thread::sleep(Duration::from_millis(200));
            Ok("opened")
        });

        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(slot.poll().is_none(), "the open has not finished yet");
        assert_eq!(wait_for(&mut slot).unwrap(), "opened");
    }

    /// The failure of the ticket: the device that was chosen never answers,
    /// and the user chooses another.
    ///
    /// Verifies: REQ-AUD-123
    #[test]
    fn switching_to_another_device_while_one_hangs_does_not_wait_for_the_hung_open() {
        let mut slot = slot();
        slot.switch(Some(DeviceId("hangs".into())), |_| {
            thread::sleep(Duration::from_secs(30));
            Ok("late")
        });
        let started = Instant::now();

        slot.switch(Some(DeviceId("works".into())), |_| Ok("opened"));

        assert_eq!(wait_for(&mut slot).unwrap(), "opened");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(slot.device(), Some(&DeviceId("works".into())));
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn what_a_given_up_open_returns_when_it_finally_does_is_not_reported() {
        let mut slot = slot();
        slot.switch(None, |_| {
            thread::sleep(Duration::from_millis(300));
            Ok("late")
        });
        slot.switch(None, |_| Ok("current"));

        assert_eq!(wait_for(&mut slot).unwrap(), "current");
        thread::sleep(Duration::from_millis(500));
        assert!(slot.poll().is_none(), "the late open reports nothing");
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn a_failed_open_is_reported_and_the_next_switch_still_works() {
        let mut slot = slot();

        slot.switch(None, |_| {
            Err(AudioError::DeviceUnresponsive("input".into()))
        });
        assert!(matches!(
            wait_for(&mut slot),
            Err(AudioError::DeviceUnresponsive(_))
        ));

        slot.switch(None, |_| Ok("opened"));
        assert_eq!(wait_for(&mut slot).unwrap(), "opened");
    }
}
