//! Local monitoring: hearing your own input without the network in the way.
//!
//! The capture callback feeds a ring through [`MonitorTap`]; the output
//! callback mixes what is in it into the frame it is about to play with
//! [`LocalMonitor::mix_into`] (ADR-033). Nothing here touches the network or
//! the play-out buffer, so the delay does not depend on the connection or the
//! preset's jitter buffer.
//!
//! Capture and playback run on the clocks of two devices, so the ring is what
//! absorbs the offset between them. It keeps [`MONITOR_MARGIN_FRAMES`] of
//! headroom beyond what a callback asks for, and stays within a frame of that:
//! a longer backlog is skipped, a dry ring plays silence and refills.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use rtrb::{Consumer, Producer, RingBuffer};
use tracing::info;

use super::stream::WIRE_CHANNELS;

/// Frames of headroom the monitor keeps beyond what an output callback asks
/// for, so that a capture callback arriving a little late does not leave the
/// output with nothing to play.
///
/// With the capture frame and the playback frame this makes the monitored
/// delay `frame * (1 + MONITOR_MARGIN_FRAMES + 1)` (ADR-033).
pub const MONITOR_MARGIN_FRAMES: u32 = 1;

/// Ring capacity in frames. Sized like the capture ring: it only has to hold a
/// burst, the backlog is bounded by skipping, not by the capacity.
const RING_FRAMES: usize = 32;

struct Queue {
    consumer: Consumer<f32>,
    /// Whether the headroom has been built up. False until the ring holds a
    /// callback's worth plus the margin, and again after it runs dry.
    primed: bool,
}

impl Queue {
    fn discard(&mut self, samples: usize) {
        if samples == 0 {
            return;
        }
        if let Ok(chunk) = self.consumer.read_chunk(samples) {
            chunk.commit_all();
        }
    }
}

/// On/off, level and the ring between the capture and the output callback.
///
/// Cheap to clone: every clone controls the same monitor, so one goes to the
/// output callback and another stays with whoever operates the switch.
#[derive(Clone)]
pub struct LocalMonitor {
    enabled: Arc<AtomicBool>,
    /// Gain as `f32` bits, so the callback reads it without a lock.
    volume: Arc<AtomicU32>,
    queue: Arc<Mutex<Queue>>,
    frame_size: usize,
}

impl LocalMonitor {
    /// A monitor for a session whose capture callback delivers frames of
    /// `frame_size` mono samples. Off, at unity gain.
    pub fn new(frame_size: u32) -> Self {
        let frame_size = (frame_size as usize).max(1);
        let (_, consumer) = RingBuffer::new(frame_size * RING_FRAMES);
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            queue: Arc::new(Mutex::new(Queue {
                consumer,
                primed: false,
            })),
            frame_size,
        }
    }

    /// Turns monitoring on or off. Takes effect on the next callback.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        info!(
            "Local monitoring: {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Sets the monitored level, 0.0 (silent) to 1.0 (as captured). Values
    /// outside the range are clamped; NaN is ignored.
    pub fn set_volume(&self, volume: f32) {
        if volume.is_nan() {
            return;
        }
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::SeqCst))
    }

    /// The capture-side handle. Move it into the capture callback.
    ///
    /// Each call starts a new ring and retires the previous handle, which is
    /// what switching the input device does: the new stream gets its own
    /// handle and the output callback carries on with the same monitor.
    pub fn tap(&self) -> MonitorTap {
        let (producer, consumer) = RingBuffer::new(self.frame_size * RING_FRAMES);
        // Blocking here is fine - this runs on the thread that (re)starts
        // capture. The output callback only try-locks, so it skips a frame
        // while the swap is in progress.
        if let Ok(mut queue) = self.queue.lock() {
            queue.consumer = consumer;
            queue.primed = false;
        }
        MonitorTap {
            producer,
            enabled: self.enabled.clone(),
        }
    }

    /// Adds the monitored input to `out`, interleaved at [`WIRE_CHANNELS`]
    /// channels, on top of whatever is already in it.
    ///
    /// Runs on the output callback: no allocation, no waiting. When the capture
    /// side holds the ring (a device switch), this frame goes without the
    /// monitor rather than stall the device.
    pub fn mix_into(&self, out: &mut [f32]) {
        let enabled = self.enabled.load(Ordering::Relaxed);
        let Ok(mut queue) = self.queue.try_lock() else {
            return;
        };
        if !enabled {
            // Nothing is queued while off, but what was queued before it was
            // switched off must not play when it is switched on again.
            let queued = queue.consumer.slots();
            queue.discard(queued);
            queue.primed = false;
            return;
        }

        let wanted = out.len() / WIRE_CHANNELS;
        if wanted == 0 {
            return;
        }
        let margin = self.frame_size * MONITOR_MARGIN_FRAMES as usize;
        let mut queued = queue.consumer.slots();

        if !queue.primed {
            if queued < wanted + margin {
                return;
            }
            queue.primed = true;
        }

        // More than a frame beyond the margin means the two clocks have drifted
        // apart, or a callback stalled. Skip to the margin instead of carrying
        // the extra as delay.
        let keep = wanted + margin;
        if queued > keep + self.frame_size {
            queue.discard(queued - keep);
            queued = keep;
        }

        let taken = queued.min(wanted);
        if taken < wanted {
            queue.primed = false;
        }
        let Ok(chunk) = queue.consumer.read_chunk(taken) else {
            return;
        };
        let gain = self.volume();
        let (first, second) = chunk.as_slices();
        for (frame, &sample) in out
            .as_chunks_mut::<WIRE_CHANNELS>()
            .0
            .iter_mut()
            .zip(first.iter().chain(second))
        {
            for slot in frame {
                *slot += sample * gain;
            }
        }
        chunk.commit_all();
    }

    #[cfg(test)]
    fn queued_samples(&self) -> usize {
        self.queue.lock().unwrap().consumer.slots()
    }
}

/// The capture-side end of a [`LocalMonitor`].
pub struct MonitorTap {
    producer: Producer<f32>,
    enabled: Arc<AtomicBool>,
}

impl MonitorTap {
    /// Hands a captured mono frame to the monitor.
    ///
    /// Called from the capture callback: no allocation, no waiting. While the
    /// monitor is off nothing is queued, so there is nothing stale to play
    /// when it is switched on. A full ring drops the frame.
    pub fn push(&mut self, mono: &[f32]) {
        if mono.is_empty() || !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut chunk) = self.producer.write_chunk(mono.len()) {
            let (first, second) = chunk.as_mut_slices();
            let split = first.len();
            first.copy_from_slice(&mono[..split]);
            second.copy_from_slice(&mono[split..]);
            chunk.commit_all();
        }
    }

    /// Hands a captured frame of `channels` interleaved channels to the monitor.
    ///
    /// The monitor is mono (ADR-033), so a stereo capture is heard as the mean
    /// of its two sides. Same constraints as [`MonitorTap::push`].
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        if channels <= 1 {
            self.push(samples);
            return;
        }
        let frames = samples.len() / channels;
        if frames == 0 || !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut chunk) = self.producer.write_chunk(frames) {
            let (first, second) = chunk.as_mut_slices();
            let split = first.len();
            for (slot, frame) in first
                .iter_mut()
                .zip(samples.chunks_exact(channels).take(split))
            {
                *slot = frame.iter().sum::<f32>() / channels as f32;
            }
            for (slot, frame) in second
                .iter_mut()
                .zip(samples.chunks_exact(channels).skip(split))
            {
                *slot = frame.iter().sum::<f32>() / channels as f32;
            }
            chunk.commit_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: usize = 8;

    /// One stereo output frame's worth of what a peer is playing.
    fn peers(value: f32) -> Vec<f32> {
        vec![value; FRAME * WIRE_CHANNELS]
    }

    /// A ramp counting up from `start + 1`, so what was heard can be read off
    /// the values: which samples, in what order, with none lost or repeated.
    /// Never 0, which is what silence reads as.
    fn ramp(start: usize) -> Vec<f32> {
        (start..start + FRAME).map(|n| (n + 1) as f32).collect()
    }

    fn left(out: &[f32]) -> Vec<f32> {
        out.iter().step_by(WIRE_CHANNELS).copied().collect()
    }

    /// Runs `frames` capture/output rounds, capture first, and returns what the
    /// left channel of the output carried.
    fn run(monitor: &LocalMonitor, tap: &mut MonitorTap, frames: usize) -> Vec<f32> {
        let mut heard = Vec::new();
        for n in 0..frames {
            tap.push(&ramp(n * FRAME));
            let mut out = peers(0.0);
            monitor.mix_into(&mut out);
            heard.extend(left(&out));
        }
        heard
    }

    /// Verifies: REQ-AUD-111
    #[test]
    fn test_enabled_monitor_plays_the_captured_input_in_order_and_unaltered() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);

        let heard = run(&monitor, &mut tap, 12);

        let played: Vec<f32> = heard.iter().copied().skip_while(|s| *s == 0.0).collect();
        assert!(
            played.len() >= 8 * FRAME,
            "monitoring never started: {heard:?}"
        );
        let first = played[0] as usize;
        let expected: Vec<f32> = (first..first + played.len()).map(|n| n as f32).collect();
        assert_eq!(played, expected, "no sample lost, repeated or reordered");
    }

    /// Verifies: REQ-AUD-111
    #[test]
    fn test_monitor_adds_to_what_the_peers_are_playing_rather_than_replacing_it() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        for _ in 0..3 {
            tap.push(&[0.25; FRAME]);
        }

        let mut out = peers(0.5);
        monitor.mix_into(&mut out);

        assert!(
            out.iter().all(|s| (*s - 0.75).abs() < 1e-6),
            "both channels carry the peers plus the monitor: {out:?}"
        );
    }

    /// Verifies: REQ-AUD-111
    #[test]
    fn test_monitor_level_scales_what_is_heard() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        monitor.set_volume(0.5);
        for _ in 0..3 {
            tap.push(&[0.8; FRAME]);
        }

        let mut out = peers(0.0);
        monitor.mix_into(&mut out);

        assert!(out.iter().all(|s| (*s - 0.4).abs() < 1e-6), "{out:?}");
    }

    /// Verifies: REQ-AUD-111
    #[test]
    fn test_monitor_plays_a_stereo_capture_as_the_mean_of_its_sides() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        let stereo: Vec<f32> = (0..FRAME).flat_map(|_| [0.5, 0.25]).collect();
        for _ in 0..3 {
            tap.push_interleaved(&stereo, 2);
        }

        let mut out = peers(0.0);
        monitor.mix_into(&mut out);

        assert!(out.iter().all(|s| (*s - 0.375).abs() < 1e-6), "{out:?}");
    }

    #[test]
    fn test_monitor_level_stays_within_zero_and_one() {
        let monitor = LocalMonitor::new(FRAME as u32);
        monitor.set_volume(3.0);
        assert_eq!(monitor.volume(), 1.0);
        monitor.set_volume(-1.0);
        assert_eq!(monitor.volume(), 0.0);
        monitor.set_volume(f32::NAN);
        assert_eq!(monitor.volume(), 0.0, "NaN leaves the level as it was");
    }

    /// Verifies: REQ-AUD-112
    #[test]
    fn test_disabled_monitor_leaves_the_output_to_the_peers() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();

        let heard = run(&monitor, &mut tap, 12);

        assert!(heard.iter().all(|s| *s == 0.0), "{heard:?}");
    }

    /// Verifies: REQ-AUD-112
    #[test]
    fn test_switching_the_monitor_off_stops_it_on_the_next_frame() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        run(&monitor, &mut tap, 6);

        monitor.set_enabled(false);
        let heard = run(&monitor, &mut tap, 3);

        assert!(heard.iter().all(|s| *s == 0.0), "{heard:?}");
    }

    /// What was captured before the monitor was switched off is old by the time
    /// it is switched on again, and must not be heard.
    ///
    /// Verifies: REQ-AUD-112
    #[test]
    fn test_input_captured_before_switching_off_is_not_heard_after_switching_on() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        for _ in 0..3 {
            tap.push(&[0.9; FRAME]);
        }
        monitor.set_enabled(false);
        monitor.mix_into(&mut peers(0.0));

        monitor.set_enabled(true);
        for _ in 0..3 {
            tap.push(&[0.1; FRAME]);
        }
        let mut out = peers(0.0);
        monitor.mix_into(&mut out);

        assert!(out.iter().all(|s| (*s - 0.1).abs() < 1e-6), "{out:?}");
    }

    /// The delay is the capture frame, the margin and the playback frame
    /// (ADR-033). The margin is what the ring holds once the monitor is
    /// running, and it must stay that when the two clocks are in step.
    ///
    /// Verifies: REQ-AUD-032
    #[test]
    fn test_monitor_holds_the_margin_and_no_more_when_the_clocks_agree() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);

        for n in 0..40 {
            tap.push(&ramp(n * FRAME));
            monitor.mix_into(&mut peers(0.0));
            if n >= 4 {
                assert_eq!(
                    monitor.queued_samples(),
                    MONITOR_MARGIN_FRAMES as usize * FRAME,
                    "after frame {n}"
                );
            }
        }
    }

    /// A capture clock running fast against the output clock would otherwise
    /// add a little delay every frame for as long as the session lasts.
    ///
    /// Verifies: REQ-AUD-032
    #[test]
    fn test_monitor_does_not_grow_a_backlog_when_capture_runs_ahead_of_output() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        let bound = (MONITOR_MARGIN_FRAMES as usize + 1) * FRAME;

        for n in 0..500 {
            tap.push(&ramp(n * FRAME));
            // A second capture frame every fourth round: 25% fast.
            if n % 4 == 0 {
                tap.push(&ramp(n * FRAME));
            }
            monitor.mix_into(&mut peers(0.0));
            assert!(
                monitor.queued_samples() <= bound,
                "backlog {} after round {n}",
                monitor.queued_samples()
            );
        }
    }

    /// Verifies: REQ-AUD-032
    #[test]
    fn test_monitor_recovers_after_capture_stalls() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        run(&monitor, &mut tap, 8);

        // Capture stops delivering for a while; the output keeps asking.
        for _ in 0..5 {
            monitor.mix_into(&mut peers(0.0));
        }
        let heard = run(&monitor, &mut tap, 8);

        assert!(
            heard.iter().any(|s| *s > 0.0),
            "monitoring did not resume: {heard:?}"
        );
    }

    #[test]
    fn test_a_dry_ring_adds_silence_rather_than_repeating_what_it_played() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        run(&monitor, &mut tap, 8);

        let mut out = peers(0.0);
        for _ in 0..4 {
            out.fill(0.0);
            monitor.mix_into(&mut out);
        }

        assert!(out.iter().all(|s| *s == 0.0), "{out:?}");
    }

    #[test]
    fn test_replacing_the_tap_keeps_the_monitor_running_on_the_new_input() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut old = monitor.tap();
        monitor.set_enabled(true);
        run(&monitor, &mut old, 6);

        // The input device is switched: a new tap, the old one is gone.
        drop(old);
        let mut new = monitor.tap();
        let mut heard = Vec::new();
        for _ in 0..6 {
            new.push(&[0.3; FRAME]);
            let mut out = peers(0.0);
            monitor.mix_into(&mut out);
            heard.extend(left(&out));
        }

        assert!(
            heard.iter().any(|s| (*s - 0.3).abs() < 1e-6),
            "the new input is not heard: {heard:?}"
        );
    }

    #[test]
    fn test_output_frames_of_any_length_are_served_in_order() {
        let monitor = LocalMonitor::new(FRAME as u32);
        let mut tap = monitor.tap();
        monitor.set_enabled(true);
        tap.push(&ramp(0));
        tap.push(&ramp(FRAME));

        // What a resampled peer makes the output ask for varies in length.
        let mut heard = Vec::new();
        for (round, frames) in [5usize, 9, 3].into_iter().enumerate() {
            let mut out = vec![0.0; frames * WIRE_CHANNELS];
            monitor.mix_into(&mut out);
            heard.extend(left(&out));
            tap.push(&ramp((round + 2) * FRAME));
        }

        let expected: Vec<f32> = (0..heard.len()).map(|n| (n + 1) as f32).collect();
        assert_eq!(heard, expected);
    }
}
