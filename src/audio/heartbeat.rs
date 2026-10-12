//! Tells a device stream that has stopped calling back from one that is quiet.
//!
//! A stream can stop without saying so: the audio host (PipeWire's ALSA plugin
//! after a run of xruns, a driver that hung) leaves the stream open and never
//! calls its callback again. No error comes, the session stays connected, and
//! the other side hears nothing. The callbacks are the one thing that cannot
//! be faked, so each pass of a callback counts on a [`Heartbeat`], and a
//! [`StallGuard`] watching the count says when it has not moved for too long.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a stream may go without a callback before it counts as stalled.
/// A device period is a few milliseconds, so this is a second of silence for
/// the listener and many missed passes for a stream that is merely slow.
pub const STALL_AFTER: Duration = Duration::from_secs(1);

/// How many times in a row a stalled stream is opened again before the
/// session says it cannot be brought back.
pub const MAX_REOPENS: u32 = 3;

/// A count of the passes of one device callback. Clones count together, so the
/// callback and the thread watching it hold one each, and a stream opened
/// again goes on counting where the last one stopped.
#[derive(Debug, Clone, Default)]
pub struct Heartbeat(Arc<AtomicU64>);

impl Heartbeat {
    /// Counts a pass. One relaxed add, safe on the audio callback.
    #[inline]
    pub fn beat(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// The passes so far.
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// What a [`StallGuard`] makes of the count this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The callback is being called, or has not been silent long enough to say.
    Flowing,
    /// Silent for [`STALL_AFTER`]: open the stream again.
    Reopen,
    /// Opened again [`MAX_REOPENS`] times and still silent: say so, and stop
    /// trying until the callback is heard again.
    GiveUp,
    /// The callback is heard again after a [`Verdict::GiveUp`].
    Resumed,
}

/// Watches the [`Heartbeat`] of one stream from the thread that runs the
/// session. The thread calls [`check`](Self::check) every turn.
#[derive(Debug)]
pub struct StallGuard {
    limit: Duration,
    max_reopens: u32,
    /// The count last seen.
    seen: u64,
    /// When the count last moved, or the wait last started over.
    since: Instant,
    /// When `check` was last called.
    checked: Instant,
    /// Reopens since the callback was last heard.
    reopens: u32,
    given_up: bool,
}

impl StallGuard {
    pub fn new(limit: Duration, max_reopens: u32, now: Instant) -> Self {
        Self {
            limit,
            max_reopens,
            seen: 0,
            since: now,
            checked: now,
            reopens: 0,
            given_up: false,
        }
    }

    /// A stream is being opened: the callback cannot be heard until it has
    /// been, so the wait starts again from now.
    pub fn hold(&mut self, now: Instant) {
        self.since = now;
        self.checked = now;
    }

    /// Reads the count at `now`.
    pub fn check(&mut self, beats: u64, now: Instant) -> Verdict {
        // A thread that was away longer than the limit (the machine slept, the
        // process was stopped) knows nothing about how long the stream was silent
        let away = now.duration_since(self.checked) > self.limit;
        self.checked = now;
        if beats != self.seen {
            self.seen = beats;
            self.since = now;
            self.reopens = 0;
            return if std::mem::take(&mut self.given_up) {
                Verdict::Resumed
            } else {
                Verdict::Flowing
            };
        }
        if away {
            self.since = now;
            return Verdict::Flowing;
        }
        if self.given_up || now.duration_since(self.since) < self.limit {
            return Verdict::Flowing;
        }
        self.since = now;
        if self.reopens >= self.max_reopens {
            self.given_up = true;
            Verdict::GiveUp
        } else {
            self.reopens += 1;
            Verdict::Reopen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: Duration = Duration::from_secs(1);

    fn guard(t0: Instant) -> StallGuard {
        StallGuard::new(LIMIT, 3, t0)
    }

    fn ms(t0: Instant, millis: u64) -> Instant {
        t0 + Duration::from_millis(millis)
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn clones_of_a_heartbeat_count_together() {
        let heartbeat = Heartbeat::default();
        let watcher = heartbeat.clone();

        heartbeat.beat();
        heartbeat.beat();

        assert_eq!(watcher.count(), 2);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn when_the_callback_keeps_being_called_the_stream_is_flowing() {
        let t0 = Instant::now();
        let mut guard = guard(t0);

        for step in 1..=50u64 {
            assert_eq!(guard.check(step, ms(t0, step * 100)), Verdict::Flowing);
        }
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn when_the_callback_is_silent_for_the_limit_the_stream_is_reopened() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        assert_eq!(guard.check(100, ms(t0, 10)), Verdict::Flowing);

        assert_eq!(guard.check(100, ms(t0, 500)), Verdict::Flowing);
        assert_eq!(guard.check(100, ms(t0, 1009)), Verdict::Flowing);
        assert_eq!(guard.check(100, ms(t0, 1010)), Verdict::Reopen);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_stream_that_is_reopened_is_given_the_limit_again_before_the_next_reopen() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        guard.check(5, ms(t0, 0));
        assert_eq!(guard.check(5, ms(t0, 1000)), Verdict::Reopen);

        assert_eq!(guard.check(5, ms(t0, 1500)), Verdict::Flowing);
        assert_eq!(guard.check(5, ms(t0, 2000)), Verdict::Reopen);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn when_every_reopen_stays_silent_the_guard_gives_up_once() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        guard.check(5, ms(t0, 0));

        let verdicts: Vec<Verdict> = (1..=6)
            .map(|second| guard.check(5, ms(t0, second * 1000)))
            .collect();

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
    fn when_the_callback_is_heard_again_after_giving_up_that_is_said_once() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        guard.check(5, ms(t0, 0));
        for second in 1..=4 {
            guard.check(5, ms(t0, second * 1000));
        }

        assert_eq!(guard.check(6, ms(t0, 4500)), Verdict::Resumed);
        assert_eq!(guard.check(7, ms(t0, 4600)), Verdict::Flowing);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_callback_heard_after_a_reopen_gives_the_next_stall_its_reopens_back() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        guard.check(5, ms(t0, 0));
        for second in 1..=3 {
            assert_eq!(guard.check(5, ms(t0, second * 1000)), Verdict::Reopen);
        }

        guard.check(6, ms(t0, 3100));

        assert_eq!(guard.check(6, ms(t0, 4100)), Verdict::Reopen);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn while_a_stream_is_being_opened_the_wait_does_not_run() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        guard.check(5, ms(t0, 0));

        // An open that takes four seconds, held every turn
        for turn in 1..=40 {
            guard.hold(ms(t0, turn * 100));
        }

        assert_eq!(guard.check(5, ms(t0, 4000)), Verdict::Flowing);
        assert_eq!(guard.check(5, ms(t0, 4999)), Verdict::Flowing);
        assert_eq!(guard.check(5, ms(t0, 5000)), Verdict::Reopen);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn when_the_watching_thread_was_away_longer_than_the_limit_the_stream_is_not_blamed() {
        let t0 = Instant::now();
        let mut guard = guard(t0);
        guard.check(5, ms(t0, 0));
        guard.check(5, ms(t0, 100));

        // The machine slept for a minute
        assert_eq!(guard.check(5, ms(t0, 60_000)), Verdict::Flowing);
        assert_eq!(guard.check(5, ms(t0, 60_900)), Verdict::Flowing);
        assert_eq!(guard.check(5, ms(t0, 61_000)), Verdict::Reopen);
    }

    /// Verifies: REQ-AUD-128
    #[test]
    fn a_session_that_waits_to_connect_before_its_first_check_is_not_blamed() {
        let t0 = Instant::now();
        let mut guard = guard(t0);

        // The streams were opened at t0 and the loop starts 20 s later
        assert_eq!(guard.check(0, ms(t0, 20_000)), Verdict::Flowing);
        assert_eq!(guard.check(0, ms(t0, 20_500)), Verdict::Flowing);
    }
}
