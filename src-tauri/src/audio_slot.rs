//! One direction of a running session's audio, and the device being switched to
//!
//! Opening a device can take as long as a hung driver likes, and the session
//! thread also carries the network (`streaming`), so it must not do the
//! opening itself. The engine goes to a thread of its own for the open, and
//! the session thread looks in on it every turn. Asking for another device
//! while one is still being opened gives up on that open: the engine it holds
//! is left behind, and a new one is made at once, so the switch is never
//! queued behind a driver that does not answer.
//!
//! The slot also watches the stream it holds: one that stops calling back
//! without an error (see `jamjam::audio::StallGuard`) is said so by
//! [`watch`](DeviceSlot::watch), and the session thread opens it again with
//! the same [`switch`](DeviceSlot::switch) it uses for a new device.

use std::sync::mpsc;
use std::thread;
use std::time::Instant;

#[cfg(target_os = "windows")]
use jamjam::audio::AsioDuplex;
use jamjam::audio::{
    AudioConfig, AudioEngine, AudioError, DeviceId, Heartbeat, StallGuard, Verdict, MAX_REOPENS,
    STALL_AFTER,
};

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
    /// Counts the passes of the callback of the stream the engine has open,
    /// and of every stream opened after it
    heartbeat: Heartbeat,
    guard: StallGuard,
    /// Whether a stream is open for the guard to watch
    streaming: bool,
    /// An open has just finished: the next [`watch`](Self::watch) starts the
    /// wait over from its own clock
    just_opened: bool,
}

impl<T: Send + 'static> DeviceSlot<T> {
    /// A slot around `engine`, which has `device` open
    pub(crate) fn new(
        what: &'static str,
        config: AudioConfig,
        engine: AudioEngine,
        device: Option<DeviceId>,
    ) -> Self {
        Self {
            what,
            heartbeat: engine.heartbeat(),
            engine: Some(engine),
            config,
            opening: None,
            device,
            guard: StallGuard::new(STALL_AFTER, MAX_REOPENS, Instant::now()),
            streaming: true,
            just_opened: false,
        }
    }

    /// For a slot whose engine has just failed to open its device: nothing is
    /// running, so nothing is watched until a switch opens a stream.
    pub(crate) fn without_stream(mut self) -> Self {
        self.streaming = false;
        self
    }

    /// An engine to open the next stream on, counting on this slot's heartbeat
    fn fresh_engine(&self) -> AudioEngine {
        let mut engine = AudioEngine::new(self.config.clone());
        engine.set_heartbeat(self.heartbeat.clone());
        engine
    }

    /// What the session thread does about the stream this turn: nothing,
    /// unless its callback has been silent for too long (a [`Verdict`]).
    ///
    /// While a stream is being opened it is not watched, and the wait starts
    /// again once the open is done.
    pub(crate) fn watch(&mut self, now: Instant) -> Verdict {
        if self.opening.is_some() || std::mem::take(&mut self.just_opened) {
            self.guard.hold(now);
            return Verdict::Flowing;
        }
        if !self.streaming {
            return Verdict::Flowing;
        }
        self.guard.check(self.heartbeat.count(), now)
    }

    /// The device last asked for; `None` is the system default
    pub(crate) fn device(&self) -> Option<&DeviceId> {
        self.device.as_ref()
    }

    /// Drops the engine (stopping its stream, if any) and gives up on any
    /// switch in progress, without opening anything in its place: for when
    /// ASIO takes over this direction (`AsioSwitch`), which opens both
    /// directions itself. `device` becomes what `device()` reports and what
    /// the next real [`switch`](Self::switch) sees, the same as if this slot
    /// had opened it.
    #[cfg(target_os = "windows")]
    pub(crate) fn park(&mut self, device: Option<DeviceId>) {
        self.opening = None;
        self.engine = None;
        self.device = device;
        self.streaming = false;
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
            None if self.opening.is_some() => {
                // An open is still out. It keeps its engine, and hands nothing
                // back that anyone reads.
                tracing::warn!(
                    "Switching the {} device again before the last switch finished",
                    self.what
                );
                self.fresh_engine()
            }
            // Parked (ASIO took over this direction) rather than mid-open:
            // nothing to warn about, just build the engine `park` dropped.
            None => self.fresh_engine(),
        };
        self.opening = None;
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
                self.engine = Some(self.fresh_engine());
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
                self.streaming = result.is_ok();
                self.just_opened = true;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.engine = Some(self.fresh_engine());
                self.opening = None;
                self.streaming = false;
                Some(Err(AudioError::StreamError(format!(
                    "the {} switch stopped",
                    self.what
                ))))
            }
        }
    }
}

/// A switch to a new ASIO session in progress (Windows only).
///
/// ASIO opens capture and playback together, as a single session
/// ([`AsioDuplex`]), not as two `AudioEngine`s, so this does not reuse an
/// engine across switches the way [`DeviceSlot`] does: each switch tears down
/// whatever session is running (on the same thread that opens the next one -
/// only one ASIO session runs at a time) and builds an entirely new one.
/// Otherwise the shape is the same as `DeviceSlot`: opening happens off the
/// calling thread, since a driver can hang, and asking for another switch
/// before [`poll`](Self::poll) has reported the last one abandons it rather
/// than queuing behind it. The live session itself is the caller's to hold
/// between switches (there is no persistent resource here to hand back, the
/// way `DeviceSlot` hands back its engine).
#[cfg(target_os = "windows")]
pub(crate) struct AsioSwitch<T> {
    opening: Option<mpsc::Receiver<Result<(AsioDuplex, T), AudioError>>>,
}

#[cfg(target_os = "windows")]
impl<T: Send + 'static> AsioSwitch<T> {
    pub(crate) fn idle() -> Self {
        Self { opening: None }
    }

    /// Starts closing `previous` (when given) and opening a new session with
    /// `open`, on a thread of its own. Returns at once; [`poll`](Self::poll)
    /// reports how it went.
    pub(crate) fn switch(
        &mut self,
        previous: Option<AsioDuplex>,
        open: impl FnOnce() -> Result<(AsioDuplex, T), AudioError> + Send + 'static,
    ) {
        if self.opening.is_some() {
            // The open it was mid this stopped: it keeps running until its
            // driver call returns, then hands its result to nobody (the
            // receiver below is dropped here), the same as `DeviceSlot`.
            tracing::warn!("Switching the ASIO driver again before the last switch finished");
        }
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("audio-switch-asio".to_string())
            .spawn(move || {
                if let Some(previous) = previous {
                    previous.stop();
                }
                let _ = tx.send(open());
            });
        match spawned {
            Ok(_) => self.opening = Some(rx),
            Err(e) => tracing::error!("Could not start the ASIO switch: {}", e),
        }
    }

    /// How the switch that was started went, once it has finished.
    pub(crate) fn poll(&mut self) -> Option<Result<(AsioDuplex, T), AudioError>> {
        let opening = self.opening.as_ref()?;
        match opening.try_recv() {
            Ok(result) => {
                self.opening = None;
                Some(result)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.opening = None;
                Some(Err(AudioError::StreamError(
                    "the ASIO switch stopped".to_string(),
                )))
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

    fn watched() -> (DeviceSlot<&'static str>, Heartbeat) {
        let engine = AudioEngine::new(AudioConfig::default());
        let heartbeat = engine.heartbeat();
        let slot = DeviceSlot::new("input", AudioConfig::default(), engine, None);
        (slot, heartbeat)
    }

    fn after(from: Instant, millis: u64) -> Instant {
        from + Duration::from_millis(millis)
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_stream_whose_callback_keeps_being_called_is_never_reopened() {
        let (mut slot, heartbeat) = watched();
        let t0 = Instant::now();

        for turn in 1..=50u64 {
            heartbeat.beat();
            assert_eq!(slot.watch(after(t0, turn * 100)), Verdict::Flowing);
        }
    }

    /// The failure of the ticket: PipeWire left the stream open and quit
    /// calling it, and nothing said so.
    ///
    /// Verifies: REQ-AUD-128
    #[test]
    fn a_stream_that_stops_calling_back_is_opened_again_and_goes_on_when_the_new_one_calls() {
        let (mut slot, heartbeat) = watched();
        let t0 = Instant::now();
        heartbeat.beat();
        assert_eq!(slot.watch(after(t0, 100)), Verdict::Flowing);

        assert_eq!(slot.watch(after(t0, 1100)), Verdict::Reopen);
        let beats = heartbeat.clone();
        slot.switch(None, move |engine| {
            // The open takes a while, and the new stream counts on the same heartbeat
            thread::sleep(Duration::from_millis(100));
            engine.heartbeat().beat();
            Ok("opened again")
        });
        // The opening engine counts on the heartbeat the slot watches
        assert_eq!(wait_for(&mut slot).unwrap(), "opened again");
        assert_eq!(beats.count(), 2);
        assert_eq!(slot.watch(after(t0, 1200)), Verdict::Flowing);
        assert_eq!(slot.watch(after(t0, 5000)), Verdict::Flowing);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn while_the_stream_is_being_opened_again_it_is_not_judged() {
        let (mut slot, heartbeat) = watched();
        let t0 = Instant::now();
        heartbeat.beat();
        slot.watch(after(t0, 0));
        assert_eq!(slot.watch(after(t0, 1000)), Verdict::Reopen);
        slot.switch(None, |_| {
            thread::sleep(Duration::from_millis(400));
            Ok("late")
        });

        // The open takes ten seconds by the clock the watch is given
        for turn in 1..=100u64 {
            assert_eq!(slot.watch(after(t0, 1000 + turn * 100)), Verdict::Flowing);
        }
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_stream_that_stays_silent_after_every_reopen_is_given_up_on() {
        let (mut slot, heartbeat) = watched();
        let t0 = Instant::now();
        heartbeat.beat();
        slot.watch(after(t0, 0));
        let mut verdicts = Vec::new();

        for second in 1..=6u64 {
            let verdict = slot.watch(after(t0, second * 1000));
            verdicts.push(verdict);
            if verdict == Verdict::Reopen {
                slot.switch(None, |_| Ok("opened, but silent"));
                assert_eq!(wait_for(&mut slot).unwrap(), "opened, but silent");
                // The turn after an open starts the wait over
                assert_eq!(slot.watch(after(t0, second * 1000)), Verdict::Flowing);
            }
        }

        assert_eq!(
            verdicts,
            vec![
                Verdict::Reopen,
                Verdict::Reopen,
                Verdict::Reopen,
                Verdict::GiveUp,
                Verdict::Flowing,
                Verdict::Flowing,
            ]
        );
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_reopen_that_fails_is_reported_by_the_open_and_the_stream_is_watched_no_more() {
        let (mut slot, heartbeat) = watched();
        let t0 = Instant::now();
        heartbeat.beat();
        slot.watch(after(t0, 0));
        assert_eq!(slot.watch(after(t0, 1000)), Verdict::Reopen);

        slot.switch(None, |_| {
            Err(AudioError::DeviceUnresponsive("input".into()))
        });

        assert!(matches!(
            wait_for(&mut slot),
            Err(AudioError::DeviceUnresponsive(_))
        ));
        for second in 2..=10u64 {
            assert_eq!(slot.watch(after(t0, second * 1000)), Verdict::Flowing);
        }
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_slot_whose_device_never_opened_has_no_stream_to_reopen() {
        let engine = AudioEngine::new(AudioConfig::default());
        let mut slot: DeviceSlot<&'static str> =
            DeviceSlot::new("input", AudioConfig::default(), engine, None).without_stream();
        let t0 = Instant::now();

        for second in 1..=10u64 {
            assert_eq!(slot.watch(after(t0, second * 1000)), Verdict::Flowing);
        }

        slot.switch(None, |_| Ok("opened"));
        wait_for(&mut slot).unwrap();
        slot.watch(after(t0, 11_000));
        assert_eq!(
            slot.watch(after(t0, 12_000)),
            Verdict::Reopen,
            "once a switch has opened a stream, it is watched"
        );
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn an_engine_made_while_another_open_is_out_counts_on_the_same_heartbeat() {
        let (mut slot, heartbeat) = watched();
        slot.switch(None, |_| {
            thread::sleep(Duration::from_millis(300));
            Ok("late")
        });

        slot.switch(None, |engine| {
            engine.heartbeat().beat();
            Ok("current")
        });

        assert_eq!(wait_for(&mut slot).unwrap(), "current");
        assert_eq!(heartbeat.count(), 1);
    }
}
