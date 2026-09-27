//! Round trip through an audio device that has its output cabled to its input.
//!
//! [`start`] arms a run: the output callback then writes a short burst on both
//! sides at every interval, stamping the time it hands each burst to the
//! device, and the input callback stamps the time it gets a burst back. What
//! comes out is how long a burst spends in the output buffer, the driver, the
//! converters, the cable, the driver again and the input buffer, which is all
//! the delay the device adds to a call and none of the network's.
//!
//! The stamp of a burst is the start of the callback that carries it plus the
//! burst's place in that callback, and a burst is noticed at the start of the
//! callback that delivers it, so the resolution is one callback: with a driver
//! that calls back every 10 ms the delays scatter over 10 ms around the true
//! figure. Read the smallest and the median, not one of them.
//!
//! Nothing is measured unless a debug build armed a run; a build without the
//! `device-loop` feature has no probe code at all, only two empty calls.

#[cfg(any(test, feature = "device-loop"))]
pub use armed::{finish, start, Params, Report};
#[cfg(any(test, feature = "device-loop"))]
pub(crate) use armed::{on_input, on_output};

/// Called with the stereo the output callback is about to hand to the device.
#[cfg(not(any(test, feature = "device-loop")))]
#[inline(always)]
pub(crate) fn on_output(_stereo: &mut [f32], _sample_rate: u32) {}

/// Called with what the input callback got from the device.
#[cfg(not(any(test, feature = "device-loop")))]
#[inline(always)]
pub(crate) fn on_input(_data: &[f32], _device_channels: usize) {}

#[cfg(any(test, feature = "device-loop"))]
mod armed {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use crate::audio::DelayStats;

    /// Frames in a burst: 0.67 ms at 48 kHz, long enough to be seen through a
    /// converter's filter and short enough to be one sound.
    const BURST_FRAMES: usize = 32;

    /// A burst alternates sign every this many frames (6 kHz at 48 kHz), so a
    /// device that blocks direct current still passes it.
    const HALF_CYCLE_FRAMES: usize = 4;

    /// Silence before the first burst, for the streams to settle.
    const WARM_UP: Duration = Duration::from_millis(100);

    /// After a burst is noticed the ones that follow it in the same sound are
    /// not counted again.
    const HOLD_OFF: Duration = Duration::from_millis(50);

    /// What a run sends and how it tells a burst.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Params {
        /// Bursts to send.
        pub count: usize,
        /// Time between the starts of two bursts. Longer than any round trip
        /// worth measuring: a burst heard is paired with the newest one sent.
        pub interval: Duration,
        /// Peak of a burst, 0 to 1.
        pub amplitude: f32,
        /// An input sample louder than this is a burst.
        pub threshold: f32,
    }

    /// What a run found.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Report {
        pub bursts_sent: usize,
        pub bursts_heard: usize,
        /// Sounds that were not the answer to a burst just sent.
        pub other_sounds: usize,
        /// Every delay heard, in the order heard, in milliseconds.
        pub delays_ms: Vec<f64>,
        pub delay: Option<DelayStats>,
        /// Frames the last output callback asked for.
        pub output_callback_frames: usize,
        /// Frames the last input callback delivered.
        pub input_callback_frames: usize,
        /// Channels the input device delivers.
        pub input_device_channels: usize,
    }

    struct Run {
        params: Params,
        primed: bool,
        /// Frames left before the next burst starts.
        until_next: u64,
        /// Frames of the burst under way still to write.
        burst_left: usize,
        sent: Vec<Instant>,
        /// The bursts before this index are paired or given up.
        paired_upto: usize,
        delays: Vec<Duration>,
        hold_until: Option<Instant>,
        other_sounds: usize,
        output_frames: usize,
        input_frames: usize,
        input_channels: usize,
    }

    impl Run {
        fn new(params: Params) -> Self {
            Self {
                params,
                primed: false,
                until_next: 0,
                burst_left: 0,
                sent: Vec::with_capacity(params.count),
                paired_upto: 0,
                delays: Vec::with_capacity(params.count),
                hold_until: None,
                other_sounds: 0,
                output_frames: 0,
                input_frames: 0,
                input_channels: 0,
            }
        }

        /// Replaces the stereo about to be played with silence and the bursts.
        fn output(&mut self, stereo: &mut [f32], sample_rate: u32, now: Instant) {
            let rate = sample_rate as f64;
            if !self.primed {
                self.primed = true;
                self.until_next = (WARM_UP.as_secs_f64() * rate) as u64;
            }
            let interval_frames = (self.params.interval.as_secs_f64() * rate) as u64;
            self.output_frames = stereo.len() / 2;
            let (frames, _) = stereo.as_chunks_mut::<2>();
            for (index, frame) in frames.iter_mut().enumerate() {
                if self.burst_left == 0
                    && self.sent.len() < self.params.count
                    && self.until_next == 0
                {
                    self.sent
                        .push(now + Duration::from_secs_f64(index as f64 / rate));
                    self.burst_left = BURST_FRAMES;
                    self.until_next = interval_frames;
                }
                let value = if self.burst_left > 0 {
                    let place = BURST_FRAMES - self.burst_left;
                    self.burst_left -= 1;
                    if (place / HALF_CYCLE_FRAMES).is_multiple_of(2) {
                        self.params.amplitude
                    } else {
                        -self.params.amplitude
                    }
                } else {
                    0.0
                };
                self.until_next = self.until_next.saturating_sub(1);
                frame.fill(value);
            }
        }

        /// Pairs a burst heard with the newest one sent before now.
        fn input(&mut self, data: &[f32], channels: usize, now: Instant) {
            self.input_frames = data.len() / channels.max(1);
            self.input_channels = channels;
            if self.hold_until.is_some_and(|until| now < until) {
                return;
            }
            if !data
                .iter()
                .any(|sample| sample.abs() > self.params.threshold)
            {
                return;
            }
            self.hold_until = Some(now + HOLD_OFF);
            let due = self.sent.partition_point(|&at| at <= now);
            if due <= self.paired_upto {
                self.other_sounds += 1;
                return;
            }
            let delay = now - self.sent[due - 1];
            if delay >= self.params.interval {
                self.other_sounds += 1;
                return;
            }
            self.paired_upto = due;
            self.delays.push(delay);
        }

        fn report(self) -> Report {
            let delays_ms: Vec<f64> = self
                .delays
                .iter()
                .map(|delay| delay.as_nanos() as f64 / 1_000_000.0)
                .collect();
            Report {
                bursts_sent: self.sent.len(),
                bursts_heard: self.delays.len(),
                other_sounds: self.other_sounds,
                delay: DelayStats::of(delays_ms.clone()),
                delays_ms,
                output_callback_frames: self.output_frames,
                input_callback_frames: self.input_frames,
                input_device_channels: self.input_channels,
            }
        }
    }

    static ARMED: AtomicBool = AtomicBool::new(false);
    static RUN: Mutex<Option<Run>> = Mutex::new(None);

    /// Arms a run. Fails when one is already under way.
    pub fn start(params: Params) -> Result<(), &'static str> {
        let mut run = RUN.lock().unwrap_or_else(|e| e.into_inner());
        if run.is_some() {
            return Err("another round-trip run is under way");
        }
        *run = Some(Run::new(params));
        ARMED.store(true, Ordering::Release);
        Ok(())
    }

    /// Ends the run and says what it found; `None` when none was armed.
    pub fn finish() -> Option<Report> {
        ARMED.store(false, Ordering::Release);
        RUN.lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(Run::report)
    }

    /// Never waits: a callback that finds the run being read skips its turn.
    pub(crate) fn on_output(stereo: &mut [f32], sample_rate: u32) {
        if !ARMED.load(Ordering::Acquire) {
            return;
        }
        let now = Instant::now();
        if let Ok(mut run) = RUN.try_lock() {
            if let Some(run) = run.as_mut() {
                run.output(stereo, sample_rate, now);
            }
        }
    }

    pub(crate) fn on_input(data: &[f32], device_channels: usize) {
        if !ARMED.load(Ordering::Acquire) {
            return;
        }
        let now = Instant::now();
        if let Ok(mut run) = RUN.try_lock() {
            if let Some(run) = run.as_mut() {
                run.input(data, device_channels, now);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const RATE: u32 = 48_000;

        fn params(count: usize) -> Params {
            Params {
                count,
                interval: Duration::from_millis(250),
                amplitude: 0.5,
                threshold: 0.05,
            }
        }

        /// A device pair: the output callback hands `callback_frames` at a
        /// time, the cable and the drivers hold each sample for `loop_delay`,
        /// and the input callback delivers what has come through by then. The
        /// input runs on the same period, half a period behind.
        fn run_through_a_device(
            run: &mut Run,
            callback_frames: usize,
            loop_delay: Duration,
            gain: f32,
            seconds: f64,
        ) {
            let t0 = Instant::now();
            let period = Duration::from_secs_f64(callback_frames as f64 / RATE as f64);
            let mut played: Vec<(Instant, f32)> = Vec::new();
            let callbacks = (seconds / period.as_secs_f64()) as u32;
            let mut delivered_to = t0;
            for n in 0..callbacks {
                let at = t0 + period * n;
                let mut stereo = vec![1.0f32; callback_frames * 2];
                run.output(&mut stereo, RATE, at);
                for (index, frame) in stereo.as_chunks::<2>().0.iter().enumerate() {
                    let plays_at = at + Duration::from_secs_f64(index as f64 / RATE as f64);
                    played.push((plays_at + loop_delay, frame[0] * gain));
                }
                let input_at = at + period / 2;
                let data: Vec<f32> = played
                    .iter()
                    .filter(|(arrives, _)| *arrives > delivered_to && *arrives <= input_at)
                    .flat_map(|(_, sample)| [*sample, *sample])
                    .collect();
                delivered_to = input_at;
                if !data.is_empty() {
                    run.input(&data, 2, input_at);
                }
            }
        }

        /// Verifies: REQ-RMT-027
        #[test]
        fn when_the_output_is_cabled_to_the_input_every_burst_is_heard_after_the_loop_delay() {
            let mut run = Run::new(params(10));
            let loop_delay = Duration::from_millis(23);

            run_through_a_device(&mut run, 480, loop_delay, 0.45, 4.0);

            let report = run.report();
            assert_eq!(report.bursts_sent, 10);
            assert_eq!(report.bursts_heard, 10, "each burst comes back once");
            assert_eq!(report.other_sounds, 0);
            let period_ms = 10.0;
            for delay in &report.delays_ms {
                assert!(
                    *delay >= 23.0 - 0.1 && *delay <= 23.0 + period_ms,
                    "{} ms is the 23 ms loop plus at most one callback",
                    delay
                );
            }
        }

        /// Verifies: REQ-RMT-027
        #[test]
        fn when_the_callbacks_are_smaller_the_delay_is_read_closer_to_the_loop_delay() {
            let mut run = Run::new(params(10));

            run_through_a_device(&mut run, 64, Duration::from_millis(7), 0.45, 4.0);

            let delay = run.report().delay.expect("bursts were heard");
            assert!(delay.min_ms >= 7.0 - 0.1, "min {}", delay.min_ms);
            assert!(delay.max_ms <= 7.0 + 1.4, "max {}", delay.max_ms);
        }

        /// Verifies: REQ-RMT-027
        #[test]
        fn when_the_cable_is_not_connected_no_burst_is_heard() {
            let mut run = Run::new(params(10));

            run_through_a_device(&mut run, 480, Duration::from_millis(23), 0.0, 4.0);

            let report = run.report();
            assert_eq!(report.bursts_sent, 10);
            assert_eq!(report.bursts_heard, 0);
            assert_eq!(report.delay, None);
        }

        /// Verifies: REQ-RMT-027
        #[test]
        fn a_sound_that_was_not_a_burst_just_sent_is_counted_apart_from_the_delays() {
            let mut run = Run::new(params(1));
            let t0 = Instant::now();

            run.input(&[0.3, 0.3], 2, t0);

            let report = run.report();
            assert_eq!(report.bursts_heard, 0);
            assert_eq!(report.other_sounds, 1);
        }

        /// Verifies: REQ-RMT-027
        #[test]
        fn the_output_is_silence_apart_from_the_bursts_and_no_more_are_sent_than_asked() {
            let mut run = Run::new(params(2));
            let t0 = Instant::now();
            let mut loud_frames = 0;
            for n in 0..400u32 {
                let mut stereo = vec![1.0f32; 128];
                run.output(
                    &mut stereo,
                    RATE,
                    t0 + Duration::from_millis(u64::from(n) * 2),
                );
                let frames = stereo.as_chunks::<2>().0;
                loud_frames += frames.iter().filter(|frame| frame[0] != 0.0).count();
                assert!(
                    frames.iter().all(|frame| frame[0] == frame[1]),
                    "a burst is on both sides"
                );
            }
            assert_eq!(run.sent.len(), 2);
            assert_eq!(loud_frames, 2 * BURST_FRAMES);
        }

        /// Verifies: REQ-RMT-027
        #[test]
        fn a_burst_that_straddles_two_callbacks_is_stamped_once_in_the_callback_it_starts_in() {
            let mut run = Run::new(params(1));
            let t0 = Instant::now();
            run.primed = true;
            run.until_next = 0;
            // The burst starts at frame 0 and has 16 of its 32 frames to go after this callback.
            run.output(&mut vec![0.0f32; 16 * 2][..], RATE, t0);
            let started_at = run.sent[0];
            let mut rest = vec![0.0f32; 64 * 2];
            run.output(&mut rest, RATE, t0 + Duration::from_micros(333));

            assert_eq!(run.sent.len(), 1);
            let first = rest.as_chunks::<2>().0.iter().position(|f| f[0] != 0.0);
            assert_eq!(first, Some(0), "the burst goes on in the next callback");
            assert_eq!(
                started_at, t0,
                "stamped at frame 0 of the callback it starts in"
            );
        }
    }
}
