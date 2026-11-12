//! Stand-ins for the sound card, so a session can run where there is none, and so what was
//! sent and what was heard is reproducible.
//!
//! They run at the session's own frame clock, as a device would.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rtrb::RingBuffer;

use super::InputSignal;
use crate::audio::{on_captured_frame, BurstSignal, CaptureRing, LocalMonitor};

/// Peak level of a tone, below full scale so the centred stereo mix cannot clip.
const TONE_AMPLITUDE: f32 = 0.5;

/// Peak level of the noise, as for a tone.
const NOISE_AMPLITUDE: f32 = 0.5;

/// Fills a captured frame with a stand-in signal.
type SignalGenerator = Box<dyn FnMut(&mut [f32]) + Send>;

fn generator(signal: &InputSignal, sample_rate: u32) -> SignalGenerator {
    match *signal {
        InputSignal::Tone(frequency) => {
            let step = std::f64::consts::TAU * frequency as f64 / sample_rate as f64;
            let mut angle = 0.0f64;
            Box::new(move |frame: &mut [f32]| {
                for sample in frame.iter_mut() {
                    *sample = TONE_AMPLITUDE * angle.sin() as f32;
                    angle = (angle + step) % std::f64::consts::TAU;
                }
            })
        }
        InputSignal::Bursts => {
            let mut signal = BurstSignal::new(sample_rate);
            Box::new(move |frame: &mut [f32]| signal.fill(frame))
        }
        InputSignal::Noise => {
            // xorshift64*, with a fixed start, so every run sends the same noise
            let mut state = 0x9E37_79B9_7F4A_7C15u64;
            Box::new(move |frame: &mut [f32]| {
                for sample in frame.iter_mut() {
                    state ^= state >> 12;
                    state ^= state << 25;
                    state ^= state >> 27;
                    let bits = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32;
                    let unit = bits / (1u64 << 24) as f32;
                    *sample = NOISE_AMPLITUDE * (2.0 * unit - 1.0);
                }
            })
        }
    }
}

/// The threads that stand in for the devices of a session
pub(super) struct StandIns {
    /// Cleared to stop the threads.
    running: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    sample_rate: u32,
    frame_size: u32,
}

impl StandIns {
    pub(super) fn new(sample_rate: u32, frame_size: u32) -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            threads: Vec::new(),
            sample_rate,
            frame_size,
        }
    }

    /// Captures `signal` in place of an input device: one mono frame per frame period, handed
    /// over as a device's capture is.
    pub(super) fn input(
        &mut self,
        signal: &InputSignal,
        monitor: &LocalMonitor,
        level: &Arc<AtomicU32>,
    ) -> CaptureRing {
        let (mut producer, consumer) = RingBuffer::<f32>::new(32 * self.frame_size as usize);
        let mut fill = generator(signal, self.sample_rate);
        let mut frame = vec![0.0f32; self.frame_size as usize];
        let mut tap = monitor.tap();
        let level = level.clone();
        self.run_at_frame_rate(move || {
            fill(&mut frame);
            on_captured_frame(&frame, 1, &mut producer, &mut tap, &level, None);
        });
        CaptureRing {
            consumer,
            channels: 1,
        }
    }

    /// Plays into `path` in place of an output device: what `source` gives, as raw 32-bit
    /// float, little-endian, stereo interleaved.
    pub(super) fn output_file(
        &mut self,
        path: &Path,
        frame_samples: usize,
        mut source: impl FnMut(&mut [f32]) -> usize + Send + 'static,
    ) -> Result<(), String> {
        let mut file = std::fs::File::create(path)
            .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
        let mut frame = vec![0.0f32; frame_samples];
        let mut bytes = Vec::with_capacity(frame.len() * 4);
        let path = path.to_path_buf();
        self.run_at_frame_rate(move || {
            let samples = source(&mut frame);
            bytes.clear();
            bytes.extend(frame[..samples].iter().flat_map(|s| s.to_le_bytes()));
            // Unbuffered on purpose: whatever was played is on disk even if the process is
            // killed.
            if let Err(e) = file.write_all(&bytes) {
                tracing::warn!("Failed to write {}: {}", path.display(), e);
            }
        });
        Ok(())
    }

    /// Calls `tick` once per frame period on a thread of its own until [`StandIns::stop`] -
    /// the clock a device would otherwise provide. Deadlines are counted from the start, so a
    /// late wake-up does not slow the stream down.
    fn run_at_frame_rate(&mut self, mut tick: impl FnMut() + Send + 'static) {
        let running = self.running.clone();
        let period = Duration::from_secs_f64(self.frame_size as f64 / self.sample_rate as f64);
        self.threads.push(std::thread::spawn(move || {
            let start = Instant::now();
            let mut frames = 0u32;
            while running.load(Ordering::SeqCst) {
                tick();
                frames = frames.wrapping_add(1);
                let due = start + period * frames;
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
        }));
    }

    /// Stops the threads and waits for them
    pub(super) fn stop(mut self) {
        self.running.store(false, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for StandIns {
    /// A session that fails to start leaves no thread running
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(frames: usize) -> Vec<f32> {
        let mut fill = generator(&InputSignal::Noise, 48_000);
        let mut out = vec![0.0f32; 128 * frames];
        for frame in out.chunks_mut(128) {
            fill(frame);
        }
        out
    }

    #[test]
    fn noise_is_the_same_every_run() {
        assert_eq!(noise(4), noise(4));
    }

    #[test]
    fn noise_stays_within_its_peak_level_and_has_no_bias() {
        let samples = noise(400);
        assert!(samples.iter().all(|s| s.abs() <= NOISE_AMPLITUDE));
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        assert!(mean.abs() < 0.01, "mean {mean}");
    }

    #[test]
    fn noise_fills_the_whole_range_and_changes_from_one_sample_to_the_next() {
        let samples = noise(400);
        let (low, high) = samples
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), s| (l.min(*s), h.max(*s)));
        assert!(low < -0.45 && high > 0.45, "range {low}..{high}");
        assert!(samples.windows(2).all(|w| w[0] != w[1]));
    }
}
