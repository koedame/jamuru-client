//! A peak limiter that adds no delay (ADR-094).
//!
//! It keeps what it is given at or under [`CEILING`] (0 dBFS, the loudest a sample
//! can be on the wire and on a device). It does not look ahead: a peak is brought
//! down to the ceiling on the very sample it arrives, and the gain then climbs
//! back over [`RELEASE_MS`]. A signal that stays at or under the ceiling is left
//! as it is, bit for bit.
//!
//! The channels of a frame share one gain, so a loud left side does not move
//! the stereo image.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// The loudest a sample may be after the limiter: 0 dBFS
pub const CEILING: f32 = 1.0;

/// How long the gain takes to climb back after a peak. Long enough not to
/// pump on the low notes of a bass, short enough to be gone after a slammed
/// cable.
pub const RELEASE_MS: f32 = 100.0;

/// Below this much gain reduction the gain snaps back to 1.0, so that a
/// limiter that has done its work goes back to leaving the audio untouched
const SNAP_TO_UNITY: f32 = 1.0e-5;

pub struct Limiter {
    /// How far the gain is below 1.0, 0.0 when idle. Kept as the distance from
    /// 1.0 and not as the gain: next to 1.0 an `f32` is too coarse for the
    /// climb back to move it, and the gain would stall short of unity
    reduction: f32,
    /// How much of the reduction is left after one frame
    decay: f32,
    /// How many frames were brought down since the limiter was made
    limited_frames: u64,
    sample_rate: u32,
}

impl Limiter {
    pub fn new(sample_rate: u32) -> Self {
        let frames_per_release = RELEASE_MS / 1000.0 * sample_rate.max(1) as f32;
        Self {
            reduction: 0.0,
            decay: (-1.0 / frames_per_release).exp(),
            limited_frames: 0,
            sample_rate,
        }
    }

    /// Limits `samples` (interleaved, `channels` to a frame) in place.
    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        for frame in samples.chunks_exact_mut(channels.max(1)) {
            let peak = frame.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
            let needed = if peak > CEILING {
                1.0 - CEILING / peak
            } else {
                0.0
            };
            if needed > self.reduction {
                self.reduction = needed;
                self.limited_frames += 1;
            } else if self.reduction > 0.0 {
                let released = self.reduction * self.decay;
                self.reduction = if released < SNAP_TO_UNITY {
                    0.0
                } else {
                    released.max(needed)
                };
            }
            if self.reduction > 0.0 {
                let gain = 1.0 - self.reduction;
                for sample in frame {
                    // The clamp is for rounding: a sample scaled by CEILING / peak
                    // can land one step over the ceiling
                    *sample = (*sample * gain).clamp(-CEILING, CEILING);
                }
            }
        }
    }

    /// How many frames were brought down since the limiter was made
    pub fn limited_frames(&self) -> u64 {
        self.limited_frames
    }

    /// Forgets the gain reduction, so what comes next is not held down by a peak
    /// from before (switching the limiter off, a new device)
    pub fn reset(&mut self) {
        self.reduction = 0.0;
    }
}

/// A [`Limiter`] the user can switch on and off while the audio runs, and that
/// counts the frames it brought down where a session can read them
pub struct SwitchedLimiter {
    limiter: Limiter,
    on: Arc<AtomicBool>,
    counted: u64,
    limited_frames: Arc<AtomicU64>,
}

impl SwitchedLimiter {
    pub fn new(sample_rate: u32, on: Arc<AtomicBool>, limited_frames: Arc<AtomicU64>) -> Self {
        Self {
            limiter: Limiter::new(sample_rate),
            on,
            counted: 0,
            limited_frames,
        }
    }

    /// Switches the limiter on or off, for this one and every one made from it by [`fresh`](Self::fresh)
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    /// A limiter at unity gain with the same switch and the same count, for a
    /// stream that is opened again (the audio waiting in the old one is not held
    /// down by what it was playing)
    pub fn fresh(&self) -> Self {
        Self {
            limiter: Limiter::new(self.limiter.sample_rate),
            on: self.on.clone(),
            counted: 0,
            limited_frames: self.limited_frames.clone(),
        }
    }

    /// Limits `samples` when the limiter is on, and leaves them alone when it is off
    pub fn process(&mut self, samples: &mut [f32], channels: usize) {
        if !self.on.load(Ordering::Relaxed) {
            // So that a peak from before does not hold the audio down when it is switched on again
            self.limiter.reset();
            return;
        }
        self.limiter.process(samples, channels);
        let limited = self.limiter.limited_frames();
        if limited != self.counted {
            self.limited_frames
                .fetch_add(limited - self.counted, Ordering::Relaxed);
            self.counted = limited;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(amplitude: f32, frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| {
                let t = (i / channels) as f32 / RATE as f32;
                amplitude * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect()
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn audio_at_or_under_the_ceiling_comes_out_bit_for_bit() {
        let mut limiter = Limiter::new(RATE);
        let original = sine(1.0, 4800, 2);
        let mut samples = original.clone();
        limiter.process(&mut samples, 2);
        assert_eq!(samples, original);
        assert_eq!(limiter.limited_frames(), 0);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn audio_over_the_ceiling_never_comes_out_over_it() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = sine(8.0, 4800, 2);
        limiter.process(&mut samples, 2);
        assert!(samples.iter().all(|s| s.abs() <= CEILING));
        assert!(limiter.limited_frames() > 0);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn a_single_loud_sample_is_brought_down_on_that_sample() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = vec![0.0f32; 64 * 2];
        samples[10 * 2] = 4.0;
        limiter.process(&mut samples, 2);
        assert_eq!(samples[10 * 2], CEILING);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn the_channels_of_a_frame_keep_their_balance() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = vec![4.0f32, 1.0];
        limiter.process(&mut samples, 2);
        assert_eq!(samples[0], CEILING);
        assert!((samples[1] - 0.25).abs() < 1e-6);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn after_a_peak_the_gain_comes_back_to_leave_the_audio_untouched() {
        let mut limiter = Limiter::new(RATE);
        let mut peak = vec![4.0f32, 4.0];
        limiter.process(&mut peak, 2);
        // Twenty release times of quiet audio
        let quiet = sine(0.5, (RATE as f32 * RELEASE_MS / 1000.0 * 20.0) as usize, 2);
        let mut samples = quiet.clone();
        limiter.process(&mut samples, 2);
        let mut again = quiet.clone();
        limiter.process(&mut again, 2);
        assert_eq!(again, quiet);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn the_gain_climbs_back_within_the_release_time() {
        let mut limiter = Limiter::new(RATE);
        let mut peak = vec![2.0f32, 2.0];
        limiter.process(&mut peak, 2);
        let release_frames = (RATE as f32 * RELEASE_MS / 1000.0) as usize;
        let mut samples = vec![0.5f32; release_frames * 2];
        limiter.process(&mut samples, 2);
        let last = samples[samples.len() - 1];
        // After one release time 1/e of the 6 dB reduction is left
        assert!(
            last > 0.5 * 0.5 + 0.5 * 0.5 * 0.6,
            "gain at the end: {last}"
        );
        assert!(last <= 0.5);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn a_switched_off_limiter_leaves_loud_audio_as_it_is() {
        let on = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicU64::new(0));
        let mut limiter = SwitchedLimiter::new(RATE, on.clone(), count.clone());
        let mut samples = vec![3.0f32, -2.0];
        limiter.process(&mut samples, 2);
        assert_eq!(samples, vec![3.0, -2.0]);
        assert_eq!(count.load(Ordering::Relaxed), 0);

        on.store(true, Ordering::Relaxed);
        limiter.process(&mut samples, 2);
        assert_eq!(samples[0], CEILING);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn switching_the_limiter_off_and_on_forgets_the_peak_before() {
        let on = Arc::new(AtomicBool::new(true));
        let mut limiter = SwitchedLimiter::new(RATE, on.clone(), Arc::new(AtomicU64::new(0)));
        let mut loud = vec![8.0f32, 8.0];
        limiter.process(&mut loud, 2);
        on.store(false, Ordering::Relaxed);
        limiter.process(&mut [0.1f32, 0.1], 2);
        on.store(true, Ordering::Relaxed);
        let mut quiet = vec![0.5f32, -0.5];
        limiter.process(&mut quiet, 2);
        assert_eq!(quiet, vec![0.5, -0.5]);
    }

    // Verifies: REQ-AUD-048
    #[test]
    fn a_mono_stream_is_limited_too() {
        let mut limiter = Limiter::new(RATE);
        let mut samples = vec![2.0f32, -3.0, 0.5];
        limiter.process(&mut samples, 1);
        assert!(samples.iter().all(|s| s.abs() <= CEILING));
    }
}
