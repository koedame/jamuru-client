//! Round-trip timing through the whole audio path, for `jamjam ... --input-bursts`.
//!
//! A continuous tone cannot say which sound came back for which one that was
//! sent. A short burst every [`BURST_INTERVAL`] can: each burst is stamped when
//! it goes out and again when it comes out of the play-out buffer, and the two
//! are paired. What that measures is everything the app adds and the network
//! adds: send task, codec, wire, receive, decode, jitter buffer, play-out.
//!
//! The peer may hold audio before sending it back (a peer that replays what it hears does, so the
//! bursts are not heard as themselves); the hold is given by the caller and
//! taken off.
//!
//! Resolution is one frame: a burst is noticed in the frame that carries it.

use std::f32::consts::TAU;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Time between the starts of two bursts. Longer than any round trip worth
/// measuring, because a burst is paired with the newest one sent before it.
pub const BURST_INTERVAL: Duration = Duration::from_millis(500);

/// How long one burst lasts.
const BURST_LENGTH: Duration = Duration::from_millis(10);

const BURST_HZ: f32 = 1000.0;

/// Below full scale so the centred stereo mix cannot clip.
const BURST_AMPLITUDE: f32 = 0.5;

/// A frame is part of a burst when a sample is louder than this. The burst is
/// sent centred, which puts each side at about 0.35.
const DETECT_THRESHOLD: f32 = 0.1;

/// After one burst is noticed, frames of the same burst (the sine crosses zero
/// inside it, and concealment can stretch it) are not counted again.
const HOLD_OFF: Duration = Duration::from_millis(100);

/// The signal `--input-bursts` sends in place of a device: silence with a short
/// tone burst at the start of every [`BURST_INTERVAL`].
pub struct BurstSignal {
    period_samples: u64,
    burst_samples: u64,
    sample_rate: f32,
    position: u64,
}

impl BurstSignal {
    pub fn new(sample_rate: u32) -> Self {
        let samples = |length: Duration| (length.as_secs_f64() * sample_rate as f64) as u64;
        Self {
            period_samples: samples(BURST_INTERVAL),
            burst_samples: samples(BURST_LENGTH),
            sample_rate: sample_rate as f32,
            position: 0,
        }
    }

    /// Fills `frame` with the next stretch of the signal.
    pub fn fill(&mut self, frame: &mut [f32]) {
        for sample in frame.iter_mut() {
            let in_period = self.position % self.period_samples;
            *sample = if in_period < self.burst_samples {
                BURST_AMPLITUDE * (TAU * BURST_HZ * in_period as f32 / self.sample_rate).sin()
            } else {
                0.0
            };
            self.position += 1;
        }
    }
}

/// Notes `at` in `edges` when `frame` starts a burst.
fn note_burst(edges: &mut Vec<Duration>, frame: &[f32], at: Duration) {
    if !frame.iter().any(|sample| sample.abs() > DETECT_THRESHOLD) {
        return;
    }
    if edges
        .last()
        .is_some_and(|last| at.saturating_sub(*last) < HOLD_OFF)
    {
        return;
    }
    edges.push(at);
}

/// Stamps bursts on their way out and on their way in. Shared between the send
/// task and the play-out callback; each side has its own lock.
pub struct BurstProbe {
    start: Instant,
    sent: Mutex<Vec<Duration>>,
    heard: Mutex<Vec<Duration>>,
}

impl Default for BurstProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl BurstProbe {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            sent: Mutex::new(Vec::new()),
            heard: Mutex::new(Vec::new()),
        }
    }

    /// Call with each frame about to be sent to the peer.
    pub fn note_sent(&self, frame: &[f32]) {
        let at = self.start.elapsed();
        if let Ok(mut sent) = self.sent.lock() {
            note_burst(&mut sent, frame, at);
        }
    }

    /// Call with each frame the play-out buffer hands to the output.
    pub fn note_played(&self, frame: &[f32]) {
        let at = self.start.elapsed();
        if let Ok(mut heard) = self.heard.lock() {
            note_burst(&mut heard, frame, at);
        }
    }

    /// Pairs what was sent with what was heard, as of now. `peer_hold` is how
    /// long the peer keeps audio before returning it.
    pub fn report(&self, peer_hold: Duration) -> RoundTripReport {
        let sent = self.sent.lock().map(|s| s.clone()).unwrap_or_default();
        let heard = self.heard.lock().map(|h| h.clone()).unwrap_or_default();
        RoundTripReport::pair(&sent, &heard, peer_hold, self.start.elapsed())
    }
}

/// What came of the bursts sent during a run.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundTripReport {
    /// Bursts that went out.
    pub bursts_sent: usize,
    /// Bursts sent early enough that their return was due before the run
    /// ended. The figure `bursts_heard` is judged against.
    pub bursts_expected: usize,
    /// Bursts heard coming back, each counted once.
    pub bursts_heard: usize,
    /// Timing of the bursts heard, or `None` when none was.
    pub delay: Option<DelayStats>,
}

/// Round-trip delay in milliseconds over the bursts heard.
#[derive(Debug, Clone, PartialEq)]
pub struct DelayStats {
    pub min_ms: f64,
    pub mean_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl RoundTripReport {
    /// `sent` and `heard` are times since the probe started, oldest first;
    /// `elapsed` is how long the run lasted.
    ///
    /// A burst heard belongs to the newest one sent at least `peer_hold`
    /// earlier. It is not counted when that burst is [`BURST_INTERVAL`] or more
    /// older than the peer hold (its own burst was lost), nor when the burst
    /// was already paired.
    fn pair(sent: &[Duration], heard: &[Duration], peer_hold: Duration, elapsed: Duration) -> Self {
        let mut paired = vec![false; sent.len()];
        let mut delays_ms = Vec::new();

        for &at in heard {
            let Some(returned_from) = at.checked_sub(peer_hold) else {
                continue;
            };
            let newer_than = sent.partition_point(|&sent_at| sent_at <= returned_from);
            let Some(index) = newer_than.checked_sub(1) else {
                continue;
            };
            let delay = returned_from - sent[index];
            if delay >= BURST_INTERVAL || paired[index] {
                continue;
            }
            paired[index] = true;
            delays_ms.push(delay.as_nanos() as f64 / 1_000_000.0);
        }

        let due_by = elapsed.saturating_sub(peer_hold + BURST_INTERVAL);
        let bursts_expected = sent.iter().filter(|&&at| at <= due_by).count();

        Self {
            bursts_sent: sent.len(),
            bursts_expected,
            bursts_heard: delays_ms.len(),
            delay: DelayStats::of(delays_ms),
        }
    }
}

impl DelayStats {
    fn of(mut delays_ms: Vec<f64>) -> Option<Self> {
        if delays_ms.is_empty() {
            return None;
        }
        delays_ms.sort_by(f64::total_cmp);
        let count = delays_ms.len();
        let nearest_rank =
            |fraction: f64| delays_ms[((fraction * count as f64).ceil() as usize).max(1) - 1];
        Some(Self {
            min_ms: delays_ms[0],
            mean_ms: delays_ms.iter().sum::<f64>() / count as f64,
            median_ms: nearest_rank(0.5),
            p95_ms: nearest_rank(0.95),
            max_ms: delays_ms[count - 1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Verifies: REQ-CLI-007
    #[test]
    fn a_burst_signal_starts_every_interval_with_silence_between() {
        let sample_rate = 48_000;
        let mut signal = BurstSignal::new(sample_rate);
        let mut samples = vec![0.0f32; sample_rate as usize]; // one second
        for frame in samples.chunks_mut(64) {
            signal.fill(frame);
        }

        let interval = sample_rate as usize / 2;
        let burst = sample_rate as usize / 100;
        for start in [0, interval] {
            let peak = |range: std::ops::Range<usize>| {
                samples[range]
                    .iter()
                    .fold(0.0f32, |peak, s| peak.max(s.abs()))
            };
            assert!(
                peak(start..start + burst) > 0.4,
                "a burst should start at sample {}",
                start
            );
            assert_eq!(
                peak(start + burst + 64..start + interval),
                0.0,
                "the rest of the interval should be silent"
            );
        }
    }

    /// Verifies: REQ-CLI-007
    #[test]
    fn a_burst_is_noticed_once_however_many_frames_carry_it() {
        let mut edges = Vec::new();
        let loud = [0.3f32; 8];
        let quiet = [0.0f32; 8];

        note_burst(&mut edges, &quiet, ms(0));
        note_burst(&mut edges, &loud, ms(10));
        note_burst(&mut edges, &loud, ms(12));
        note_burst(&mut edges, &quiet, ms(14));
        note_burst(&mut edges, &loud, ms(16));
        note_burst(&mut edges, &loud, ms(510));

        assert_eq!(edges, vec![ms(10), ms(510)]);
    }

    /// Verifies: REQ-CLI-007
    #[test]
    fn when_the_peer_holds_audio_the_hold_is_taken_off_the_delay() {
        let sent = [ms(1000), ms(1500), ms(2000)];
        // Returned 3000 ms + 20 ms, 30 ms and 40 ms later.
        let heard = [ms(4020), ms(4530), ms(5040)];

        let report = RoundTripReport::pair(&sent, &heard, ms(3000), ms(6000));

        assert_eq!(report.bursts_sent, 3);
        assert_eq!(report.bursts_heard, 3);
        let delay = report.delay.expect("three bursts were heard");
        assert!((delay.min_ms - 20.0).abs() < 1e-6);
        assert!((delay.median_ms - 30.0).abs() < 1e-6);
        assert!((delay.max_ms - 40.0).abs() < 1e-6);
        assert!((delay.mean_ms - 30.0).abs() < 1e-6);
    }

    /// A lost burst must not shift the pairing of the ones after it: each is
    /// paired by when it was due, not by counting off.
    ///
    /// Verifies: REQ-CLI-007
    #[test]
    fn when_a_burst_is_lost_the_bursts_after_it_still_pair_with_their_own() {
        let sent = [ms(0), ms(500), ms(1000), ms(1500)];
        // The second never came back.
        let heard = [ms(25), ms(1035), ms(1540)];

        let report = RoundTripReport::pair(&sent, &heard, ms(0), ms(3000));

        assert_eq!(report.bursts_expected, 4);
        assert_eq!(report.bursts_heard, 3);
        let delay = report.delay.expect("bursts were heard");
        assert!((delay.min_ms - 25.0).abs() < 1e-6);
        assert!((delay.max_ms - 40.0).abs() < 1e-6);
    }

    /// Verifies: REQ-CLI-007
    #[test]
    fn a_sound_with_no_burst_of_ours_before_it_is_not_counted() {
        let sent = [ms(1000)];
        // Before anything was sent, and a second sound for a burst already
        // paired.
        let heard = [ms(500), ms(1010), ms(1030)];

        let report = RoundTripReport::pair(&sent, &heard, ms(0), ms(3000));

        assert_eq!(report.bursts_heard, 1);
        assert_eq!(report.delay.expect("one was heard").min_ms, 10.0);
    }

    /// The burst that a sound belongs to is the newest one sent before it. When
    /// that one is a whole interval or more back, its own burst was lost, and
    /// pairing it with an older one would report a delay that never happened.
    ///
    /// Verifies: REQ-CLI-007
    #[test]
    fn a_sound_more_than_an_interval_after_the_newest_burst_is_not_counted() {
        let sent = [ms(0), ms(500)];

        let report = RoundTripReport::pair(&sent, &[ms(1200)], ms(0), ms(3000));

        assert_eq!(report.bursts_heard, 0);
        assert_eq!(report.delay, None);
    }

    /// A sound that arrives before the peer's hold could have passed cannot be
    /// an echo of anything.
    ///
    /// Verifies: REQ-CLI-007
    #[test]
    fn a_sound_heard_before_the_peer_hold_has_passed_is_not_counted() {
        let sent = [ms(0)];

        let report = RoundTripReport::pair(&sent, &[ms(2000)], ms(3000), ms(9000));

        assert_eq!(report.bursts_heard, 0);
    }

    /// Bursts sent in the last stretch of a run had no time to come back and
    /// must not read as loss.
    ///
    /// Verifies: REQ-CLI-007
    #[test]
    fn bursts_sent_too_late_to_return_are_not_expected() {
        let sent = [ms(0), ms(500), ms(9000), ms(9500)];

        let report = RoundTripReport::pair(&sent, &[], ms(3000), ms(10_000));

        assert_eq!(report.bursts_sent, 4);
        assert_eq!(report.bursts_expected, 2, "only the first two are due back");
        assert_eq!(report.bursts_heard, 0);
        assert_eq!(report.delay, None);
    }

    /// Verifies: REQ-CLI-007
    #[test]
    fn the_95th_percentile_is_the_nearest_rank() {
        let sent: Vec<Duration> = (0..20).map(|n| ms(n * 500)).collect();
        // Delays of 1, 2, ... 20 ms.
        let heard: Vec<Duration> = (0..20).map(|n| ms(n * 500 + n + 1)).collect();

        let report = RoundTripReport::pair(&sent, &heard, ms(0), ms(20_000));

        let delay = report.delay.expect("bursts were heard");
        assert_eq!(delay.p95_ms, 19.0);
        assert_eq!(delay.median_ms, 10.0);
    }
}
