//! How much faster or slower to play so that the receive buffer holds what its
//! target asks for (ADR-122).
//!
//! Two clocks that were set to the same rate never keep it: the other side's
//! sound card sends a little faster or slower than ours plays, by some tens of
//! parts per million (ppm). The buffer fills or empties by that much, for ever.
//! A change of the delay is the same thing for a moment: the buffer holds more or
//! less than the new target asks for. Both show as the difference between what
//! the buffer holds and what it should, and both are put right the same way, by
//! playing a little faster or slower until the difference is gone.
//!
//! What counts is the least the buffer held over half a second, not what it held
//! on average: the buffer swings by a frame with the phase of the reads and by
//! more with the jitter of the link, and the frames it holds above the least are
//! the ones that carry the link through its late packets. Playing so that the
//! average were at the target would eat them. The least of each window, smoothed
//! over a quarter of a second and less a dead band in which nothing is done, goes
//! to a proportional and integral controller. The integral learns the rate of
//! the clock, so once learned it goes on correcting while the difference is inside
//! the band.
//!
//! The answer is held to [`MAX_PPM`]: 1000 ppm is 1.7 cents of pitch, and the
//! slowest that matters is far below it. A clock off by more than that is not a
//! clock, and the buffer's other ways of giving back what it holds take over.

/// Length of the window the least depth is taken over, in seconds. Not shorter
/// than the buffer's own window for skipping what piled up (500 reads of 64 sample
/// frames at 48 kHz, two thirds of a second): the same depth is looked at.
const WINDOW_SECS: f32 = 0.5;

/// Time constant of the smoothing of the least depths, in seconds
const SMOOTHING_SECS: f32 = 0.25;

/// Natural frequency of the loop, in radians a second. Slow on purpose: the
/// least depth of a window is a whole number of frames, and a loop that answers
/// faster than the buffer drifts swings past the band and back.
const LOOP_OMEGA: f32 = 0.2;

/// The most the rate is moved from the one it was sent at, in ppm
pub const MAX_PPM: f32 = 1000.0;

/// How much deeper than asked the smoothed depth may be, in frames, before
/// anything is done
const BAND_ABOVE_FRAMES: f32 = 1.0;

/// How much shallower. Less than above: a buffer that is too shallow is the one
/// that runs dry, and one that is too deep only waits.
const BAND_BELOW_FRAMES: f32 = 0.25;

/// The band while the depth is being brought to a new target, in frames: the
/// delay that was asked for is held to within this
const BAND_MOVING_FRAMES: f32 = 0.2;

#[derive(Debug, Clone)]
pub struct DepthRate {
    sample_rate: f32,
    /// The least depth of the windows, smoothed, in frames; `None` until the
    /// first window after the buffer started or its target moved
    smoothed: Option<f32>,
    /// The least depth of the window being filled, and how long it has been
    window_least: usize,
    window_secs: f32,
    /// How long a frame of the buffer is, from the length of the last read
    frame_secs: f32,
    /// What the controller has learned of the clock, as a fraction
    integral: f32,
    /// Whether the depth is on its way to a target that just moved
    moving: bool,
    ppm: f32,
}

impl DepthRate {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as f32,
            smoothed: None,
            window_least: usize::MAX,
            window_secs: 0.0,
            frame_secs: 0.0,
            integral: 0.0,
            moving: false,
            ppm: 0.0,
        }
    }

    /// The depth, smoothed, in frames
    pub fn smoothed(&self) -> Option<f32> {
        self.smoothed
    }

    /// Forgets the depth, which moved by itself (frames discarded, a stream that
    /// started over, a delay that was raised). What was learned of the clock stays.
    pub fn depth_moved(&mut self) {
        self.smoothed = None;
        self.window_least = usize::MAX;
        self.window_secs = 0.0;
        self.ppm = 0.0;
    }

    /// The target moved by itself: the depth is brought to it with a narrow band
    /// instead of the wide one.
    pub fn target_moved(&mut self) {
        self.depth_moved();
        self.moving = true;
    }

    /// Forgets everything, the clock too.
    pub fn reset(&mut self) {
        self.depth_moved();
        self.integral = 0.0;
        self.moving = false;
    }

    /// One read: the buffer held `depth` frames, the target is `target` frames,
    /// and the read covers `read_sample_frames` sample frames. Returns the rate
    /// to play at, in ppm; it is worked out again at the end of each window.
    pub fn update(&mut self, depth: usize, target: u32, read_sample_frames: usize) -> f32 {
        self.window_least = self.window_least.min(depth);
        self.frame_secs = read_sample_frames.max(1) as f32 / self.sample_rate;
        self.window_secs += self.frame_secs;
        if self.window_secs < WINDOW_SECS {
            return self.ppm;
        }
        let secs = self.window_secs;
        let least = self.window_least as f32;
        self.window_least = usize::MAX;
        self.window_secs = 0.0;

        let smoothed = match self.smoothed {
            None => least,
            Some(smoothed) => {
                let weight = 1.0 - (-secs / SMOOTHING_SECS).exp();
                smoothed + weight * (least - smoothed)
            }
        };
        self.smoothed = Some(smoothed);

        // A frame being read is held until it is, so a buffer at its target
        // holds one more than that when it is read
        let difference = smoothed - (target as f32 + 1.0);
        let (above, below) = if self.moving {
            (BAND_MOVING_FRAMES, BAND_MOVING_FRAMES)
        } else {
            (BAND_ABOVE_FRAMES, BAND_BELOW_FRAMES)
        };
        if self.moving && difference.abs() <= BAND_MOVING_FRAMES {
            self.moving = false;
        }
        let outside = if difference > above {
            difference - above
        } else if difference < -below {
            difference + below
        } else {
            0.0
        };

        // The difference in seconds of audio
        let error = outside * self.frame_secs;

        let proportional = 2.0 * LOOP_OMEGA * error;
        let limit = MAX_PPM * 1e-6;
        self.integral =
            (self.integral + LOOP_OMEGA * LOOP_OMEGA * error * secs).clamp(-limit, limit);
        self.ppm = ((proportional + self.integral) * 1e6).clamp(-MAX_PPM, MAX_PPM);
        self.ppm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const FRAME: usize = 64;

    /// Reads at the device's pace for `secs` seconds while the buffer drifts by
    /// `skew_ppm` of the time (positive: it fills), and returns the rate asked
    /// for at the end and the largest difference seen after `settle` seconds
    fn drift(skew_ppm: f32, secs: f32, settle: f32, target: u32) -> (f32, f32) {
        let mut rate = DepthRate::new(RATE);
        let reads = (secs * RATE as f32 / FRAME as f32) as usize;
        let mut held = (target + 1) as f32;
        let mut worst = 0.0f32;
        let mut ppm = 0.0;
        for read in 0..reads {
            // One frame is read; the sender made (1 + skew - correction) of one
            held += (skew_ppm - ppm) * 1e-6;
            let depth = held.round() as usize;
            ppm = rate.update(depth, target, FRAME);
            if read as f32 * FRAME as f32 / RATE as f32 > settle {
                worst = worst.max((held - (target + 1) as f32).abs());
            }
        }
        (ppm, worst)
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_the_buffer_holds_what_it_should_no_rate_is_asked_for() {
        let mut rate = DepthRate::new(RATE);
        for _ in 0..2000 {
            assert_eq!(rate.update(3, 2, FRAME), 0.0);
        }
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_the_buffer_holds_more_than_it_should_a_faster_rate_is_asked_for() {
        let mut rate = DepthRate::new(RATE);
        let mut ppm = 0.0;
        for _ in 0..2000 {
            ppm = rate.update(6, 2, FRAME);
        }
        assert!(ppm > 0.0, "{ppm}");
        assert!(ppm <= MAX_PPM);
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_the_buffer_holds_less_than_it_should_a_slower_rate_is_asked_for() {
        let mut rate = DepthRate::new(RATE);
        let mut ppm = 0.0;
        for _ in 0..2000 {
            ppm = rate.update(1, 4, FRAME);
        }
        assert!(ppm < 0.0, "{ppm}");
        assert!(ppm >= -MAX_PPM);
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn the_rate_never_goes_beyond_the_limit_however_far_off_the_buffer_is() {
        let mut rate = DepthRate::new(RATE);
        for _ in 0..20_000 {
            let ppm = rate.update(60, 2, FRAME);
            assert!(ppm.abs() <= MAX_PPM);
        }
        let mut rate = DepthRate::new(RATE);
        for _ in 0..20_000 {
            let ppm = rate.update(0, 30, FRAME);
            assert!(ppm.abs() <= MAX_PPM);
        }
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn a_clock_off_by_a_few_hundred_ppm_is_learned_and_the_depth_stays_within_the_band() {
        for skew in [-500.0f32, -100.0, 100.0, 500.0] {
            let (ppm, worst) = drift(skew, 120.0, 30.0, 4);
            assert!((ppm - skew).abs() < 60.0, "learned {ppm} of {skew}");
            assert!(worst < 2.0, "{worst} frames off at {skew} ppm");
        }
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn a_clock_off_by_more_than_the_limit_is_corrected_as_far_as_the_limit_goes() {
        let (ppm, _) = drift(2500.0, 60.0, 60.0, 4);
        assert_eq!(ppm, MAX_PPM);
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn after_the_target_moves_the_depth_is_brought_to_it_without_the_wide_band() {
        let mut rate = DepthRate::new(RATE);
        rate.update(3, 2, FRAME);
        // The target goes down by two frames: the buffer holds two more than asked
        rate.target_moved();
        let mut ppm = 0.0;
        for _ in 0..2000 {
            ppm = rate.update(3, 0, FRAME);
        }
        assert!(ppm > 0.0, "{ppm}");
    }
}
