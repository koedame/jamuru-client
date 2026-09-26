//! Play-out buffer: the one place received audio waits for its turn.
//!
//! The output callback reads from this buffer directly, at the device's clock
//! (ADR-028). That is what makes the configured delay the *actual* delay:
//!
//! - frames wait here, in sequence order, until their turn comes, so a late or
//!   reordered packet can still land in its slot;
//! - nothing sits in a second queue behind it, so the delay the app reports is
//!   the delay the listener hears;
//! - no worker thread has to be woken on time, because the thread that already
//!   runs at the audio clock does the reading.
//!
//! The previous design had two stages - a jitter buffer drained by a polling
//! loop into a playback FIFO - which moved frames out of the jitter buffer
//! before their play time and so gave up the reordering protection the depth
//! was supposed to buy.
//!
//! # Changing the delay while playing
//!
//! Moving the target while audio is playing moves what is held (ADR-031):
//! raising it inserts silence and does not advance the play position, so the
//! frames that arrive meanwhile become the extra delay; lowering it discards
//! frames from the play position until the buffer holds no more than the new
//! target. [`PlayoutBuffer::adapt`] moves the target from the loss rate and
//! [`PlayoutBuffer::set_delay`] moves it because the user chose a preset.
//!
//! # Real-time discipline
//!
//! The reader runs in the audio callback, so `read_into` allocates nothing and
//! never blocks: slots are preallocated and concealment writes into the
//! caller's buffer. Callers share the buffer through a mutex and use
//! `try_lock` on the reader side; a contended frame is concealed rather than
//! waited for, which costs one frame and cannot invert priorities.

use super::plc::PcmPlc;

/// Share of reads over an adaptation window that found their frame missing
/// (concealed or starved) above which the delay grows. One in a hundred is
/// already a gap every 1.3 seconds at 64-sample frames, and every gap is heard.
const GROW_ABOVE_MISS_RATE: f32 = 0.01;
/// Share of missing frames below which the window counts as clean: not one
/// missing frame in a window of up to a thousand reads.
const SHRINK_BELOW_MISS_RATE: f32 = 0.001;
/// Clean windows in a row before the delay gives a frame back. With a
/// one-second timer that is ten seconds.
const CLEAN_WINDOWS_BEFORE_SHRINK: u32 = 10;
/// A grow within this many windows of a shrink means the shrink was wrong.
const BOUNCE_WITHIN_WINDOWS: u32 = 3;
/// The clean stretch a shrink asks for at most, after it has been wrong
/// repeatedly: eighty seconds.
const MAX_CLEAN_WINDOWS_BEFORE_SHRINK: u32 = 80;
/// Fewest frames a window needs before its loss rate means anything.
const MIN_ADAPT_WINDOW_FRAMES: u64 = 20;

/// How the buffer is sized and how long it holds audio back.
///
/// The bounds decide how the delay behaves once [`PlayoutBuffer::adapt`] runs:
///
/// | Bounds | Behaviour |
/// |--------|-----------|
/// | `target = 0` | passthrough - the first frame plays as soon as it arrives |
/// | `min == max` | fixed - adaptation cannot move the delay (passthrough is) |
/// | `min < max` | adaptive - adaptation moves the delay between the two |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayoutConfig {
    /// Samples per frame, counting every channel (interleaved).
    pub frame_samples: usize,
    /// How many frames to keep ahead of playback. 0 is passthrough: the first
    /// frame plays as soon as it arrives (ADR-008 zero-latency).
    pub target_delay_frames: u32,
    /// Lowest delay adaptation may choose.
    pub min_delay_frames: u32,
    /// Highest delay adaptation may choose, and what sizes the slot ring.
    pub max_delay_frames: u32,
}

impl PlayoutConfig {
    /// Settings for a stream that is asked to hold `target_delay_frames`.
    ///
    /// This is what the receive path uses, so the delay a preset asks for is
    /// held here and nowhere else (ADR-028). Passthrough is fixed at zero:
    /// adaptation must not quietly give up the 0ms promise (ADR-031). Buffered
    /// delays keep at least one frame of protection and may grow to twice the
    /// target plus two.
    pub fn for_delay(frame_samples: usize, target_delay_frames: u32) -> Self {
        Self {
            frame_samples,
            target_delay_frames,
            min_delay_frames: if target_delay_frames == 0 { 0 } else { 1 },
            max_delay_frames: if target_delay_frames == 0 {
                0
            } else {
                target_delay_frames * 2 + 2
            },
        }
    }

    /// Slots the ring needs to hold `ring_delay_frames` of delay plus room for
    /// frames that arrive early, with a floor so a passthrough buffer can still
    /// reorder a little.
    fn capacity_frames(ring_delay_frames: u32) -> usize {
        ((ring_delay_frames as usize) * 2 + 2).max(4)
    }

    /// How large a slot has to be.
    ///
    /// A peer running at another sample rate is resampled before it gets here,
    /// and a rate conversion does not produce a whole number of frames: 44.1k
    /// to 48k is 1.088 frames' worth, 44.1k to 96k is 2.18. Frames are
    /// therefore stored at whatever length they are - padding or truncating
    /// them to a fixed size would add or remove samples, which is what changes
    /// pitch. The headroom covers the widest conversion the app offers
    /// (44100 -> 96000, ADR-013) with room to spare.
    fn slot_samples(&self) -> usize {
        self.frame_samples.max(1) * 3
    }
}

/// What a read produced, and how much of the caller's buffer it filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayoutRead {
    pub result: PlayoutResult,
    /// Samples written to the caller's buffer.
    pub samples: usize,
}

/// What a read produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayoutResult {
    /// The frame that was due had arrived.
    Played { sequence: u32 },
    /// It had not, and a later one proves it will not be used: concealed.
    Concealed { sequence: u32 },
    /// Not enough audio buffered to start yet. The output is silence.
    Priming,
    /// The delay is being raised: this read is silence and the play position
    /// stays where it is, so the frames that arrive meanwhile add to the
    /// delay (ADR-031). Not an underrun and not a loss.
    Padded,
    /// Playing, but the frame that is due has not arrived and nothing after it
    /// has either - the sender is simply not ahead of us. The output is
    /// silence and the stream position stays where it is (REQ-LAT-029).
    Starved,
}

/// What happened to a frame handed to the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Stored, waiting for its turn.
    Accepted,
    /// Its turn had already passed; dropped.
    Late,
    /// Already held; ignored.
    Duplicate,
    /// So far from the current sequence that the stream must have restarted.
    /// The buffer resynchronised to it.
    Resynced,
    /// Wrong length for the configured frame; dropped.
    WrongLength,
}

/// Counters for the UI and for the quality logic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayoutStats {
    pub frames_written: u64,
    pub frames_played: u64,
    pub frames_concealed: u64,
    pub frames_late: u64,
    /// Reads that found nothing at or after the frame due and played silence
    /// (`PlayoutResult::Starved`), while playing.
    pub frames_starved: u64,
    pub resyncs: u64,
    /// How far, in frames, the stream jumped at the last resynchronisation.
    pub last_resync_distance: u32,
}

/// One slot of the ring.
struct Slot {
    /// Sequence the samples belong to, or `None` when the slot is free.
    sequence: Option<u32>,
    /// Preallocated to the slot size; only `len` samples are meaningful.
    samples: Vec<f32>,
    /// How many samples this frame actually holds - resampling makes that
    /// vary from frame to frame.
    len: usize,
}

/// Received audio, waiting for the output callback to take it.
pub struct PlayoutBuffer {
    slots: Vec<Slot>,
    config: PlayoutConfig,
    /// Delay in force now; adaptation moves it between min and max.
    target_delay_frames: u32,
    /// Largest `max_delay_frames` the ring was sized for. `set_delay` may move
    /// to any delay up to it without allocating.
    ring_delay_frames: u32,
    /// Frames still to add (positive) or remove (negative) so that what is
    /// held catches up with `target_delay_frames` (ADR-031). Only meaningful
    /// while playing.
    pending_shift: i32,
    /// Played, concealed and starved reads at the last adaptation window.
    adapt_seen: u64,
    /// Concealed and starved reads at the last adaptation window.
    adapt_missed: u64,
    /// Consecutive adaptation windows with almost no missing frame.
    clean_windows: u32,
    /// Clean windows in a row the next shrink waits for. Doubles when a shrink
    /// is followed by a grow within [`BOUNCE_WITHIN_WINDOWS`].
    shrink_after_windows: u32,
    /// Windows since the last shrink, while a grow would still show it wrong.
    windows_since_shrink: Option<u32>,
    /// Sequence the next read will play.
    play_sequence: Option<u32>,
    /// Whether enough has been buffered to start.
    playing: bool,
    plc: PcmPlc,
    /// Length of the last frame played, so concealment lasts as long as the
    /// audio it stands in for.
    last_len: usize,
    stats: PlayoutStats,
}

impl PlayoutBuffer {
    pub fn new(config: PlayoutConfig) -> Self {
        let ring_delay_frames = config.max_delay_frames;
        Self::with_ring_for(config, ring_delay_frames)
    }

    /// Like [`Self::new`], with a ring big enough to hold delays up to
    /// `ring_delay_frames`, so [`Self::set_delay`] can move to any of them
    /// without allocating (ADR-031).
    pub fn with_ring_for(config: PlayoutConfig, ring_delay_frames: u32) -> Self {
        let ring_delay_frames = ring_delay_frames.max(config.max_delay_frames);
        let capacity = PlayoutConfig::capacity_frames(ring_delay_frames);
        let frame_samples = config.frame_samples.max(1);
        let slot_samples = config.slot_samples();
        let slots = (0..capacity)
            .map(|_| Slot {
                sequence: None,
                samples: vec![0.0; slot_samples],
                len: 0,
            })
            .collect();

        // The PLC works in frames, and this buffer is interleaved, so it is
        // told one "channel" of `frame_samples` - fading applies per sample
        // either way.
        let plc = PcmPlc::new(frame_samples as u32, 1);

        Self {
            slots,
            target_delay_frames: config.target_delay_frames,
            ring_delay_frames,
            pending_shift: 0,
            adapt_seen: 0,
            adapt_missed: 0,
            clean_windows: 0,
            shrink_after_windows: CLEAN_WINDOWS_BEFORE_SHRINK,
            windows_since_shrink: None,
            config,
            play_sequence: None,
            playing: false,
            plc,
            last_len: frame_samples,
            stats: PlayoutStats::default(),
        }
    }

    /// Hands a decoded frame to the buffer. Called from the network side.
    pub fn write(&mut self, sequence: u32, samples: &[f32]) -> WriteOutcome {
        // Any length up to the slot size is fine: a resampled frame is not
        // the nominal length, and forcing it to be would change pitch.
        if samples.is_empty() || samples.len() > self.config.slot_samples() {
            return WriteOutcome::WrongLength;
        }

        match self.play_sequence {
            // Nothing has been seen yet: this frame is where playback starts.
            None => self.play_sequence = Some(sequence),
            Some(next) => {
                let distance = sequence.wrapping_sub(next) as i32;

                if distance.unsigned_abs() as usize >= self.slots.len() {
                    // Too far from the current sequence to be a gap in the
                    // same stream - the peer restarted its numbering, or we
                    // ran past what it sent. Waiting for a sequence that will
                    // never come would be silence forever (a hole ADR-026
                    // left open).
                    self.resync_to(sequence);
                    self.store(sequence, samples);
                    self.stats.resyncs += 1;
                    self.stats.last_resync_distance = distance.unsigned_abs();
                    self.stats.frames_written += 1;
                    return WriteOutcome::Resynced;
                }

                if distance < 0 {
                    if self.playing {
                        // Its turn has passed. Dropping it is the only option:
                        // the listener has already heard that moment.
                        self.stats.frames_late += 1;
                        return WriteOutcome::Late;
                    }
                    // Still priming, so an older frame simply moves the start
                    // back - a burst that arrived out of order still plays in
                    // order.
                    self.play_sequence = Some(sequence);
                }
            }
        }

        let slot = self.slot_for(sequence);
        if self.slots[slot].sequence == Some(sequence) {
            return WriteOutcome::Duplicate;
        }

        self.store(sequence, samples);
        self.stats.frames_written += 1;
        WriteOutcome::Accepted
    }

    /// Fills `out` with the next frame. Called from the audio callback.
    ///
    /// Allocation-free and non-blocking by construction: every buffer it
    /// touches was allocated up front.
    pub fn read_into(&mut self, out: &mut [f32]) -> PlayoutRead {
        // Silence of the length the stream is running at, so the device clock
        // keeps its cadence while nothing is ready.
        let silence = |out: &mut [f32], len: usize| {
            let len = len.min(out.len());
            out[..len].fill(0.0);
            PlayoutRead {
                result: PlayoutResult::Priming,
                samples: len,
            }
        };

        if !self.playing {
            // Start once the configured delay is buffered *plus* the frame
            // about to play, or the delay would come out a frame short
            // (ADR-020).
            if self.ready_frames() as u32 <= self.target_delay_frames {
                return silence(out, self.last_len);
            }
            self.playing = true;
        }

        if self.pending_shift < 0 {
            self.trim_excess();
        }
        if self.pending_shift > 0 {
            self.pending_shift -= 1;
            let mut read = silence(out, self.last_len);
            read.result = PlayoutResult::Padded;
            return read;
        }

        let sequence = match self.play_sequence {
            Some(sequence) => sequence,
            None => return silence(out, self.last_len),
        };

        let slot = self.slot_for(sequence);
        if self.slots[slot].sequence != Some(sequence) && self.ready_frames() == 0 {
            // Nothing at or beyond the frame being waited for. Every buffered
            // frame is at or after `play_sequence` (older ones are refused as
            // late), so an empty buffer means no successor exists.
            //
            // Concealing here would step past a frame still in flight. The
            // reader runs on the device clock, so the position would pull a
            // frame further ahead of the sender on every read, every frame
            // would then arrive late, and the stream would not play again
            // (ADR-026). Waiting is the honest answer.
            let mut read = silence(out, self.last_len);
            read.result = PlayoutResult::Starved;
            self.stats.frames_starved += 1;
            return read;
        }
        self.play_sequence = Some(sequence.wrapping_add(1));

        if self.slots[slot].sequence == Some(sequence) {
            let len = self.slots[slot].len.min(out.len());
            out[..len].copy_from_slice(&self.slots[slot].samples[..len]);
            self.slots[slot].sequence = None;
            self.plc.store_frame(&out[..len]);
            self.last_len = len;
            self.stats.frames_played += 1;
            PlayoutRead {
                result: PlayoutResult::Played { sequence },
                samples: len,
            }
        } else {
            // Conceal for as long as the last frame lasted, so a loss does not
            // shift the timing of everything after it.
            let len = self.last_len.min(out.len());
            self.plc.conceal_into(&mut out[..len]);
            self.stats.frames_concealed += 1;
            PlayoutRead {
                result: PlayoutResult::Concealed { sequence },
                samples: len,
            }
        }
    }

    /// Frames currently waiting, which is the delay in force.
    pub fn ready_frames(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.sequence.is_some())
            .count()
    }

    /// Delay the buffer is holding to, in frames.
    pub fn target_delay_frames(&self) -> u32 {
        self.target_delay_frames
    }

    /// Moves the delay, clamped to the configured range. Used by adaptation.
    ///
    /// While playing, what is held follows: see the module docs.
    pub fn set_target_delay_frames(&mut self, frames: u32) {
        let frames = frames.clamp(
            self.config.min_delay_frames,
            self.config
                .max_delay_frames
                .max(self.config.min_delay_frames),
        );
        if self.playing {
            self.pending_shift += frames as i32 - self.target_delay_frames as i32;
        }
        self.target_delay_frames = frames;
    }

    /// Chooses a new delay - the user switched preset - and the range
    /// adaptation may move it in.
    ///
    /// Unlike [`Self::set_target_delay_frames`] this is not held to the range
    /// the buffer started with: leaving passthrough for a buffered preset, or
    /// the other way round, has to land on the delay that was asked for. The
    /// delay is also what [`Self::reset`] returns to. It is limited only by
    /// the ring, which was sized up front.
    pub fn set_delay(&mut self, frames: u32) {
        let bounds = PlayoutConfig::for_delay(self.config.frame_samples, frames);
        self.config.min_delay_frames = bounds.min_delay_frames;
        self.config.max_delay_frames = bounds.max_delay_frames.min(self.ring_delay_frames);
        self.config.target_delay_frames = frames.min(self.config.max_delay_frames);
        self.clean_windows = 0;
        self.shrink_after_windows = CLEAN_WINDOWS_BEFORE_SHRINK;
        self.windows_since_shrink = None;
        self.set_target_delay_frames(self.config.target_delay_frames);
    }

    /// Follows the link over the time since the last call: grows the delay
    /// after a stretch with missing frames, gives it back after a long clean
    /// one. Returns the new delay when it moved.
    ///
    /// A frame is missing when it was concealed (a later one had arrived) or
    /// when the read found the buffer empty and played silence (`Starved`).
    /// Both are heard as a gap. Starved reads count because a frame that is
    /// merely late - the common case on Wi-Fi - starves the read and is never
    /// concealed, so leaving them out let a link drop a frame in a hundred
    /// without the delay ever moving.
    ///
    /// Call it on a timer, not per frame: the point is to follow the link, not
    /// to react to one packet. It judges the stretch since its previous
    /// decision rather than the whole session - a burst of loss early on must
    /// not keep pushing the delay up for minutes.
    ///
    /// Growing takes one bad stretch; shrinking takes `shrink_after_windows`
    /// clean ones in a row, starting at [`CLEAN_WINDOWS_BEFORE_SHRINK`]. Every
    /// move costs a gap or a skip (ADR-031), so the delay must not flip back
    /// and forth: a shrink that is followed by a grow within
    /// [`BOUNCE_WITHIN_WINDOWS`] was wrong, and the next one waits twice as long.
    pub fn adapt(&mut self) -> Option<u32> {
        let seen =
            self.stats.frames_played + self.stats.frames_concealed + self.stats.frames_starved;
        let window = seen.saturating_sub(self.adapt_seen);
        if window < MIN_ADAPT_WINDOW_FRAMES {
            return None;
        }
        let missed_total = self.stats.frames_concealed + self.stats.frames_starved;
        let missed = missed_total.saturating_sub(self.adapt_missed);
        self.adapt_seen = seen;
        self.adapt_missed = missed_total;
        self.windows_since_shrink = self.windows_since_shrink.map(|n| n + 1);

        let before = self.target_delay_frames;
        let miss_rate = missed as f32 / window as f32;
        if miss_rate > GROW_ABOVE_MISS_RATE {
            self.clean_windows = 0;
            if self
                .windows_since_shrink
                .is_some_and(|n| n <= BOUNCE_WITHIN_WINDOWS)
            {
                self.shrink_after_windows =
                    (self.shrink_after_windows * 2).min(MAX_CLEAN_WINDOWS_BEFORE_SHRINK);
            }
            self.windows_since_shrink = None;
            self.set_target_delay_frames(before + 1);
        } else if miss_rate < SHRINK_BELOW_MISS_RATE {
            self.clean_windows += 1;
            if self.clean_windows >= self.shrink_after_windows {
                self.clean_windows = 0;
                self.set_target_delay_frames(before.saturating_sub(1));
                if self.target_delay_frames != before {
                    self.windows_since_shrink = Some(0);
                }
            }
        } else {
            self.clean_windows = 0;
        }

        (self.target_delay_frames != before).then_some(self.target_delay_frames)
    }

    pub fn stats(&self) -> PlayoutStats {
        self.stats
    }

    /// Drops everything and starts over. Used when a connection is
    /// (re-)established.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.sequence = None;
        }
        self.play_sequence = None;
        self.playing = false;
        self.target_delay_frames = self.config.target_delay_frames;
        self.last_len = self.config.frame_samples;
        self.plc.reset();
        self.stats = PlayoutStats::default();
        self.pending_shift = 0;
        self.adapt_seen = 0;
        self.adapt_missed = 0;
        self.clean_windows = 0;
        self.shrink_after_windows = CLEAN_WINDOWS_BEFORE_SHRINK;
        self.windows_since_shrink = None;
    }

    fn slot_for(&self, sequence: u32) -> usize {
        sequence as usize % self.slots.len()
    }

    fn store(&mut self, sequence: u32, samples: &[f32]) {
        let slot = self.slot_for(sequence);
        self.slots[slot].samples[..samples.len()].copy_from_slice(samples);
        self.slots[slot].len = samples.len();
        self.slots[slot].sequence = Some(sequence);
    }

    fn resync_to(&mut self, sequence: u32) {
        for slot in &mut self.slots {
            slot.sequence = None;
        }
        self.play_sequence = Some(sequence);
        self.playing = false;
        self.pending_shift = 0;
        self.plc.reset();
    }

    /// Discards frames from the play position while the buffer holds more than
    /// the target asks for, up to what was requested.
    ///
    /// What is left of the request afterwards is dropped: the buffer already
    /// holds no more than the target, so jitter has done the work.
    fn trim_excess(&mut self) {
        while self.pending_shift < 0 && self.ready_frames() as u32 > self.target_delay_frames + 1 {
            if let Some(sequence) = self.play_sequence {
                let slot = self.slot_for(sequence);
                if self.slots[slot].sequence == Some(sequence) {
                    self.slots[slot].sequence = None;
                }
                self.play_sequence = Some(sequence.wrapping_add(1));
            }
            self.pending_shift += 1;
        }
        self.pending_shift = self.pending_shift.max(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(target: u32) -> PlayoutConfig {
        PlayoutConfig {
            frame_samples: 4,
            target_delay_frames: target,
            min_delay_frames: 0,
            max_delay_frames: 8,
        }
    }

    fn frame(value: f32) -> Vec<f32> {
        vec![value; 4]
    }

    /// The delay the caller configures is the delay that is actually held, and
    /// the frames stay in this buffer rather than moving to a second queue -
    /// which is what lets a late frame still find its slot (ADR-028).
    ///
    /// Verifies: REQ-LAT-025
    #[test]
    fn playback_starts_once_the_configured_delay_is_buffered() {
        for target in 0..=4u32 {
            let mut buffer = PlayoutBuffer::new(config(target));
            let mut out = frame(0.0);

            // Exactly `target` frames must not start playback: doing so would
            // leave the delay one frame short of what was asked for.
            for sequence in 0..target {
                assert_eq!(
                    buffer.write(sequence, &frame(sequence as f32)),
                    WriteOutcome::Accepted
                );
                assert_eq!(
                    buffer.read_into(&mut out).result,
                    PlayoutResult::Priming,
                    "target {}: must not start with only {} frame(s)",
                    target,
                    sequence + 1
                );
                assert!(out.iter().all(|&s| s == 0.0), "priming must be silent");
            }

            // One more starts it, from the oldest frame.
            buffer.write(target, &frame(target as f32));
            assert_eq!(
                buffer.read_into(&mut out).result,
                PlayoutResult::Played { sequence: 0 },
                "target {}: playback starts from the oldest frame",
                target
            );
            assert_eq!(out, frame(0.0));
            assert_eq!(
                buffer.ready_frames() as u32,
                target,
                "target {}: the configured delay stays in the buffer",
                target
            );
        }
    }

    /// Passthrough keeps its 0ms promise: the first frame plays immediately
    /// and nothing is held back.
    ///
    /// Verifies: REQ-LAT-026
    #[test]
    fn passthrough_plays_the_first_frame_immediately() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = frame(0.0);

        buffer.write(0, &frame(0.5));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 0 }
        );
        assert_eq!(out, frame(0.5));
        assert_eq!(buffer.ready_frames(), 0, "passthrough retains nothing");
    }

    /// Frames that arrive out of order play in order, because each waits in
    /// the slot its sequence names.
    #[test]
    fn out_of_order_arrivals_play_in_order() {
        let mut buffer = PlayoutBuffer::new(config(2));
        let mut out = frame(0.0);

        for sequence in [2u32, 0, 3, 1] {
            buffer.write(sequence, &frame(sequence as f32));
        }

        for expected in 0..4u32 {
            assert_eq!(
                buffer.read_into(&mut out).result,
                PlayoutResult::Played { sequence: expected }
            );
            assert_eq!(out, frame(expected as f32));
        }
    }

    /// A frame that arrives after its turn cannot be played - the listener has
    /// already heard that moment - and must not disturb what follows.
    #[test]
    fn a_frame_that_arrives_after_its_turn_is_dropped() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = frame(0.0);

        buffer.write(0, &frame(0.0));
        buffer.write(1, &frame(1.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 0 }
        );
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 1 }
        );

        assert_eq!(buffer.write(0, &frame(9.0)), WriteOutcome::Late);
        assert_eq!(buffer.stats().frames_late, 1);

        buffer.write(2, &frame(2.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 2 },
            "a late arrival must not shift what comes next"
        );
        assert_eq!(out, frame(2.0));
    }

    /// A missing frame is concealed from the last good one rather than
    /// stalling playback, and the stream carries on afterwards.
    #[test]
    fn a_missing_frame_is_concealed_and_playback_continues() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = frame(0.0);

        buffer.write(0, &frame(1.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 0 }
        );

        // Frame 1 never arrives; 2 does.
        buffer.write(2, &frame(2.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Concealed { sequence: 1 }
        );
        assert!(
            out.iter().all(|&s| s != 0.0 && s.abs() <= 1.0),
            "concealment should fade the last good frame, got {:?}",
            out
        );

        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 2 }
        );
        assert_eq!(out, frame(2.0));
        assert_eq!(buffer.stats().frames_concealed, 1);
    }

    /// A peer that restarts its numbering must not silence the session
    /// forever. The buffer resynchronises instead of waiting for a sequence
    /// that will never arrive.
    #[test]
    fn a_restarted_stream_resynchronises() {
        let mut buffer = PlayoutBuffer::new(config(1));
        let mut out = frame(0.0);

        for sequence in 0..5u32 {
            buffer.write(sequence, &frame(sequence as f32));
        }
        assert!(matches!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { .. }
        ));

        // The peer reconnects and starts at 0 again, far from where we are.
        assert_eq!(buffer.write(9_000, &frame(7.0)), WriteOutcome::Resynced);
        assert_eq!(buffer.stats().resyncs, 1);

        buffer.write(9_001, &frame(8.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 9_000 },
            "playback should follow the stream to its new numbering"
        );
        assert_eq!(out, frame(7.0));
    }

    /// Feeds `count` periods to the buffer: each one delivers a frame (unless
    /// `lose_every_other` drops the odd ones) and takes one out. `next` is the
    /// sequence to deliver next and is advanced.
    fn drive(buffer: &mut PlayoutBuffer, next: &mut u32, count: u32, lose_every_other: bool) {
        let mut out = frame(0.0);
        for _ in 0..count {
            if !(lose_every_other && *next % 2 == 1) {
                buffer.write(*next, &frame(*next as f32));
            }
            *next += 1;
            let _ = buffer.read_into(&mut out);
        }
    }

    /// Reads in a window that is meant to be clean. Long enough that the one
    /// frame a growth leaves concealed at its start stays under the limit for
    /// a growing window (one in a hundred).
    const CLEAN_WINDOW: u32 = 300;

    fn adaptive(target: u32, min: u32, max: u32) -> PlayoutBuffer {
        PlayoutBuffer::new(PlayoutConfig {
            frame_samples: 4,
            target_delay_frames: target,
            min_delay_frames: min,
            max_delay_frames: max,
        })
    }

    /// A lossy stretch buys more delay, one frame per decision, and never past
    /// the ceiling.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn a_lossy_stretch_grows_the_delay_up_to_its_ceiling() {
        let mut buffer = adaptive(2, 1, 4);
        let mut next = 0;
        drive(&mut buffer, &mut next, 4, false);

        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), Some(3), "concealment should buy a frame");

        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), Some(4));

        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), None, "the ceiling holds");
        assert_eq!(buffer.target_delay_frames(), 4);
    }

    /// The loss rate is the rate over the stretch since the last decision. A
    /// burst of loss early on must not keep pushing the delay up afterwards.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn adaptation_judges_the_last_stretch_and_not_the_whole_session() {
        let mut buffer = adaptive(2, 1, 8);
        let mut next = 0;
        drive(&mut buffer, &mut next, 4, false);
        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), Some(3));

        // The link has been clean since. The cumulative rate is still high,
        // but the delay must hold.
        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            assert_eq!(buffer.adapt(), None);
        }
        assert_eq!(buffer.target_delay_frames(), 3);
    }

    /// Too few frames to say anything: the decision waits for more instead of
    /// acting on noise, and nothing is lost by waiting.
    #[test]
    fn adaptation_waits_until_the_stretch_has_enough_frames() {
        let mut buffer = adaptive(2, 1, 8);
        let mut next = 0;
        drive(&mut buffer, &mut next, 4, false);

        drive(&mut buffer, &mut next, 12, true);
        assert_eq!(buffer.adapt(), None, "a handful of frames is not a rate");
        assert_eq!(buffer.adapt(), None, "asking again changes nothing");

        drive(&mut buffer, &mut next, 20, true);
        assert_eq!(
            buffer.adapt(),
            Some(3),
            "the earlier frames count once there are enough"
        );
    }

    /// A long clean run gives a frame back, but never below the floor, and a
    /// single clean stretch is not enough.
    ///
    /// Verifies: REQ-LAT-109
    #[test]
    fn a_long_clean_run_gives_the_delay_back_down_to_its_floor() {
        let mut buffer = adaptive(3, 2, 8);
        let mut next = 0;
        drive(&mut buffer, &mut next, 6, false);

        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, 100, false);
            assert_eq!(buffer.adapt(), None, "one clean stretch is not enough");
        }
        drive(&mut buffer, &mut next, 100, false);
        assert_eq!(buffer.adapt(), Some(2));

        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK * 3) {
            drive(&mut buffer, &mut next, 100, false);
            assert_eq!(buffer.adapt(), None, "the floor holds");
        }
        assert_eq!(buffer.target_delay_frames(), 2);
    }

    /// A lossy stretch in the middle of a clean run starts the count again, so
    /// the delay does not flip back and forth.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn a_lossy_stretch_restarts_the_count_towards_shrinking() {
        let mut buffer = adaptive(3, 1, 8);
        let mut next = 0;
        drive(&mut buffer, &mut next, 6, false);

        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            buffer.adapt();
        }
        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), Some(4));

        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            assert_eq!(buffer.adapt(), None);
        }
    }

    /// A read that finds the buffer empty plays silence and leaves the position
    /// where it is: it is starved, not concealed. Adaptation has to see it, or a
    /// link that leaves the buffer empty one read in ten never moves the delay
    /// (a Mac on Wi-Fi played 1546 starved reads and never adjusted).
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn reads_that_find_the_buffer_empty_grow_the_delay() {
        let mut buffer = adaptive(1, 1, 4);
        let mut out = frame(0.0);
        let mut next = 0;
        drive(&mut buffer, &mut next, 4, false);

        // The device asks ten times for what the peer sends nine times: one
        // read in ten finds the buffer empty and plays silence.
        let mut starved = 0;
        for period in 0..100u32 {
            if period % 10 != 5 {
                buffer.write(next, &frame(next as f32));
                next += 1;
            }
            if buffer.read_into(&mut out).result == PlayoutResult::Starved {
                starved += 1;
            }
        }

        assert!(
            starved > 5,
            "the scenario has to starve reads, got {}",
            starved
        );
        assert_eq!(buffer.stats().frames_concealed, 0, "nothing was concealed");
        assert_eq!(buffer.stats().frames_starved, starved);
        assert_eq!(buffer.adapt(), Some(2), "starved reads buy a frame");
    }

    /// One read in a hundred finding its frame missing is a gap every second
    /// and a half: enough to grow the delay, where the old limit of one in
    /// twenty left it alone.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn about_one_missing_frame_in_a_hundred_grows_the_delay() {
        let mut buffer = adaptive(1, 1, 4);
        let mut next = 0;
        drive(&mut buffer, &mut next, 4, false);

        // 750 reads a second at 64 samples, 15 of them concealed: 2%.
        let mut out = frame(0.0);
        for period in 0..750u32 {
            if period % 50 != 25 {
                buffer.write(next, &frame(next as f32));
            }
            next += 1;
            let _ = buffer.read_into(&mut out);
        }

        assert_eq!(buffer.adapt(), Some(2));
    }

    /// A delay that was given back and had to be taken again was given back
    /// too early. The next attempt waits twice as long, so a link that needs
    /// the frame does not lose and regain it every eleven seconds.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn a_shrink_that_bounces_makes_the_next_one_wait_twice_as_long() {
        let mut buffer = adaptive(3, 1, 8);
        let mut next = 0;
        drive(&mut buffer, &mut next, 6, false);

        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            assert_eq!(buffer.adapt(), None);
        }
        drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
        assert_eq!(
            buffer.adapt(),
            Some(2),
            "ten clean windows give a frame back"
        );

        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), Some(3), "the frame was needed after all");

        // The window right after a grow still carries the frame it left
        // concealed, so the count of clean windows starts with the next.
        drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
        buffer.adapt();
        for _ in 0..(2 * CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            assert_eq!(
                buffer.adapt(),
                None,
                "the second attempt waits twice as long"
            );
        }
        drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
        assert_eq!(buffer.adapt(), Some(2));
    }

    /// A shrink that stood for a while was right, and does not slow the next.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn a_shrink_that_holds_does_not_change_how_long_the_next_one_waits() {
        let mut buffer = adaptive(4, 1, 8);
        let mut next = 0;
        drive(&mut buffer, &mut next, 8, false);

        for _ in 0..CLEAN_WINDOWS_BEFORE_SHRINK {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            buffer.adapt();
        }
        assert_eq!(buffer.target_delay_frames(), 3);

        for _ in 0..(BOUNCE_WITHIN_WINDOWS + 2) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            buffer.adapt();
        }
        drive(&mut buffer, &mut next, 40, true);
        assert_eq!(buffer.adapt(), Some(4));

        drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
        buffer.adapt();
        for _ in 0..(CLEAN_WINDOWS_BEFORE_SHRINK - 1) {
            drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
            assert_eq!(buffer.adapt(), None);
        }
        drive(&mut buffer, &mut next, CLEAN_WINDOW, false);
        assert_eq!(buffer.adapt(), Some(3), "still ten windows");
    }

    /// Every fifth frame comes in a period and a half late, behind the frame
    /// after it. Its read has to conceal it, and it is dropped when it turns
    /// up. The buffer keeps missing at that spot until the delay is deep
    /// enough to have the late frame in its slot when the read comes; the
    /// adaptation is what gets it there.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn adaptation_finds_the_delay_a_link_with_late_frames_needs() {
        // One frame is `PERIOD` ticks; the device reads once per period.
        const PERIOD: u64 = 1_000;
        const LATENESS: u64 = 1_500;
        const READS_PER_WINDOW: u64 = 750;
        const WINDOWS: u64 = 30;

        let miss_rates = |adaptive_delay: bool| -> Vec<f32> {
            let mut buffer = adaptive(1, 1, 4);
            let total = READS_PER_WINDOW * WINDOWS;
            let mut writes: Vec<(u64, u32)> = (0..total + 8)
                .map(|n| {
                    let late = if n % 5 == 2 { LATENESS } else { 0 };
                    (n * PERIOD + late, n as u32)
                })
                .collect();
            writes.sort_by_key(|&(time, _)| time);
            let mut writes = writes.into_iter().peekable();

            let mut out = frame(0.0);
            let mut rates = Vec::new();
            let (mut before_missed, mut before_reads) = (0u64, 0u64);
            for read in 0..total {
                let now = (read + 1) * PERIOD;
                while let Some(&(time, sequence)) = writes.peek() {
                    if time > now {
                        break;
                    }
                    buffer.write(sequence, &frame(sequence as f32));
                    writes.next();
                }
                let _ = buffer.read_into(&mut out);
                if (read + 1) % READS_PER_WINDOW == 0 {
                    let stats = buffer.stats();
                    let reads = stats.frames_played + stats.frames_concealed + stats.frames_starved;
                    let missed = stats.frames_concealed + stats.frames_starved;
                    rates.push(
                        (missed - before_missed) as f32 / (reads - before_reads).max(1) as f32,
                    );
                    before_missed = missed;
                    before_reads = reads;
                    if adaptive_delay {
                        buffer.adapt();
                    }
                }
            }
            rates
        };

        let fixed = miss_rates(false);
        let adapted = miss_rates(true);
        let last = |rates: &[f32]| rates[rates.len() - 10..].iter().sum::<f32>() / 10.0;

        assert!(
            last(&fixed) > 0.05,
            "without adaptation the buffer keeps missing: {:?}",
            &fixed[fixed.len() - 10..]
        );
        assert!(
            last(&adapted) < GROW_ABOVE_MISS_RATE,
            "with adaptation the delay settles where the late frames fit: {:?}",
            &adapted[adapted.len() - 10..]
        );
    }

    /// Fixed bounds and passthrough are not moved by adaptation.
    ///
    /// Verifies: REQ-LAT-110
    #[test]
    fn a_fixed_delay_does_not_adapt() {
        for (target, min, max) in [(3, 3, 3), (0, 0, 0)] {
            let mut buffer = adaptive(target, min, max);
            let mut next = 0;
            drive(&mut buffer, &mut next, target + 2, false);

            drive(&mut buffer, &mut next, 200, true);
            assert_eq!(buffer.adapt(), None);
            for _ in 0..CLEAN_WINDOWS_BEFORE_SHRINK {
                drive(&mut buffer, &mut next, 100, false);
                assert_eq!(buffer.adapt(), None);
            }
            assert_eq!(buffer.target_delay_frames(), target);
        }
    }

    /// Adaptation moves the delay within its bounds, and a caller asking for
    /// less than the floor gets the floor.
    ///
    /// Verifies: REQ-LAT-109
    #[test]
    fn the_target_stays_within_the_adaptation_bounds() {
        let mut buffer = adaptive(2, 1, 4);
        buffer.set_target_delay_frames(0);
        assert_eq!(buffer.target_delay_frames(), 1);
        buffer.set_target_delay_frames(99);
        assert_eq!(buffer.target_delay_frames(), 4);
    }

    /// Delivers `frames` in a row, taking one out after each, and returns
    /// what each read produced.
    fn periods(buffer: &mut PlayoutBuffer, next: &mut u32, frames: u32) -> Vec<PlayoutResult> {
        let mut out = frame(0.0);
        (0..frames)
            .map(|_| {
                buffer.write(*next, &frame(*next as f32));
                *next += 1;
                buffer.read_into(&mut out).result
            })
            .collect()
    }

    /// Raising the delay while playing really holds more: silence goes out
    /// while the position waits, and the frames that arrive meanwhile are the
    /// extra delay. Nothing is lost or reordered.
    ///
    /// Verifies: REQ-LAT-025
    #[test]
    fn raising_the_delay_while_playing_holds_more_audio() {
        let mut buffer = PlayoutBuffer::new(config(2));
        let mut next = 0;
        periods(&mut buffer, &mut next, 6);
        assert_eq!(buffer.ready_frames(), 2, "steady state holds the target");

        buffer.set_target_delay_frames(4);
        let results = periods(&mut buffer, &mut next, 4);

        assert_eq!(
            results,
            vec![
                PlayoutResult::Padded,
                PlayoutResult::Padded,
                PlayoutResult::Played { sequence: 4 },
                PlayoutResult::Played { sequence: 5 },
            ],
            "two frames of silence, then the stream carries on without a skip"
        );
        assert_eq!(
            buffer.ready_frames(),
            4,
            "the delay in force is the new one"
        );
        let stats = buffer.stats();
        assert_eq!(
            stats.frames_concealed, 0,
            "inserted silence is not a loss - adaptation must not read it as one"
        );
    }

    /// The silence lasts as long as the audio it makes room for, and is silent.
    #[test]
    fn padding_is_silent_and_keeps_the_device_cadence() {
        let mut buffer = PlayoutBuffer::new(config(1));
        let mut next = 0;
        periods(&mut buffer, &mut next, 4);

        buffer.set_target_delay_frames(2);
        let mut out = frame(9.0);
        buffer.write(next, &frame(next as f32));
        let read = buffer.read_into(&mut out);

        assert_eq!(read.result, PlayoutResult::Padded);
        assert_eq!(read.samples, 4);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    /// Lowering the delay while playing releases what is held beyond the new
    /// target, from the play position, so the latency drops at once.
    ///
    /// Verifies: REQ-LAT-025
    #[test]
    fn lowering_the_delay_while_playing_drops_the_excess() {
        let mut buffer = PlayoutBuffer::new(config(4));
        let mut next = 0;
        periods(&mut buffer, &mut next, 8);
        assert_eq!(buffer.ready_frames(), 4);

        buffer.set_target_delay_frames(2);
        let results = periods(&mut buffer, &mut next, 2);

        assert_eq!(
            results[0],
            PlayoutResult::Played { sequence: 6 },
            "two frames were skipped: 4 and 5"
        );
        assert_eq!(results[1], PlayoutResult::Played { sequence: 7 });
        assert_eq!(buffer.ready_frames(), 2, "what is held is the new target");
        assert_eq!(buffer.stats().frames_concealed, 0);
    }

    /// If the buffer already holds no more than the new target - jitter got
    /// there first - nothing is thrown away.
    #[test]
    fn lowering_the_delay_does_not_drop_what_is_already_gone() {
        let mut buffer = PlayoutBuffer::new(config(4));
        let mut next = 0;
        periods(&mut buffer, &mut next, 8);

        // The network stalls for three periods: the buffer drains to one.
        let mut out = frame(0.0);
        for _ in 0..3 {
            buffer.read_into(&mut out);
        }
        assert_eq!(buffer.ready_frames(), 1);

        buffer.set_target_delay_frames(2);
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 7 },
            "nothing is skipped when there is nothing in excess"
        );
    }

    /// Before playback starts the start condition reads the target directly,
    /// so changing it must not also queue a shift.
    #[test]
    fn changing_the_delay_before_playback_needs_no_shift() {
        let mut buffer = PlayoutBuffer::new(config(2));
        buffer.set_target_delay_frames(3);

        let mut out = frame(0.0);
        for sequence in 0..3u32 {
            buffer.write(sequence, &frame(sequence as f32));
            assert_eq!(buffer.read_into(&mut out).result, PlayoutResult::Priming);
        }
        buffer.write(3, &frame(3.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 0 },
            "it starts on the new target, with no silence in front"
        );
        assert_eq!(buffer.ready_frames(), 3);
    }

    /// A change that is reverted before it takes effect cancels out.
    #[test]
    fn opposite_changes_cancel_out() {
        let mut buffer = PlayoutBuffer::new(config(2));
        let mut next = 0;
        periods(&mut buffer, &mut next, 6);

        buffer.set_target_delay_frames(4);
        buffer.set_target_delay_frames(2);

        let results = periods(&mut buffer, &mut next, 2);
        assert_eq!(
            results,
            vec![
                PlayoutResult::Played { sequence: 4 },
                PlayoutResult::Played { sequence: 5 }
            ]
        );
    }

    /// Choosing a preset lands on that preset's delay whichever way the switch
    /// goes. The range it started with must not cut the choice short.
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn choosing_a_delay_is_not_cut_short_by_the_starting_range() {
        let frame_samples = 4;
        let ring = PlayoutConfig::for_delay(frame_samples, 8).max_delay_frames;
        let mut buffer =
            PlayoutBuffer::with_ring_for(PlayoutConfig::for_delay(frame_samples, 4), ring);

        buffer.set_delay(0);
        assert_eq!(buffer.target_delay_frames(), 0, "balanced to zero-latency");

        buffer.set_delay(8);
        assert_eq!(
            buffer.target_delay_frames(),
            8,
            "zero-latency to high-quality"
        );

        buffer.set_delay(4);
        assert_eq!(buffer.target_delay_frames(), 4);
        buffer.reset();
        assert_eq!(
            buffer.target_delay_frames(),
            4,
            "a reconnect returns to the delay that was chosen, not the first one"
        );
    }

    /// Adaptation after a preset switch works inside the new preset's range,
    /// and `reset` gives back what adaptation took.
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn a_chosen_delay_sets_the_range_adaptation_moves_in() {
        let ring = PlayoutConfig::for_delay(4, 8).max_delay_frames;
        let mut buffer = PlayoutBuffer::with_ring_for(PlayoutConfig::for_delay(4, 4), ring);

        buffer.set_delay(0);
        let mut next = 0;
        drive(&mut buffer, &mut next, 3, false);
        drive(&mut buffer, &mut next, 200, true);
        assert_eq!(buffer.adapt(), None, "zero-latency stays at zero");

        buffer.set_delay(4);
        buffer.set_target_delay_frames(99);
        assert_eq!(
            buffer.target_delay_frames(),
            PlayoutConfig::for_delay(4, 4).max_delay_frames
        );
        buffer.reset();
        assert_eq!(buffer.target_delay_frames(), 4);
    }

    /// The ring caps the delay: asking for more than it was sized for must not
    /// index past it.
    #[test]
    fn a_delay_beyond_the_ring_is_capped() {
        let mut buffer = PlayoutBuffer::new(config(2));
        buffer.set_delay(100);
        assert!(buffer.target_delay_frames() <= 8);
        buffer.write(0, &frame(1.0));
    }

    /// A read with nothing buffered must wait, not declare a loss: the frame
    /// has not been sent yet, and stepping past it would leave the buffer
    /// ahead of the sender for good (ADR-026).
    ///
    /// Verifies: REQ-LAT-029
    #[test]
    fn a_read_with_nothing_buffered_waits_instead_of_concealing() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = frame(0.0);

        buffer.write(0, &frame(0.5));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 0 }
        );

        // Drained. The sender has not sent frame 1 yet.
        for _ in 0..10 {
            out.fill(9.0);
            let read = buffer.read_into(&mut out);
            assert_eq!(
                read.result,
                PlayoutResult::Starved,
                "an empty buffer must wait rather than consume sequence numbers"
            );
            assert_eq!(read.samples, 4, "the device keeps its cadence");
            assert!(out.iter().all(|&s| s == 0.0), "waiting must be silent");
        }
        assert_eq!(
            buffer.stats().frames_concealed,
            0,
            "nothing was lost - the frame had not been sent"
        );

        // When it does arrive, it plays.
        assert_eq!(buffer.write(1, &frame(1.0)), WriteOutcome::Accepted);
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 1 }
        );
        assert_eq!(out, frame(1.0), "the frame must survive the wait");
    }

    /// A gap is only a gap once a later frame proves it. Concealment must still
    /// happen then, or a real loss would stall playback forever.
    ///
    /// Verifies: REQ-LAT-029
    #[test]
    fn a_gap_is_concealed_once_a_later_frame_arrives() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = frame(0.0);

        buffer.write(0, &frame(0.5));
        buffer.read_into(&mut out);
        assert_eq!(buffer.read_into(&mut out).result, PlayoutResult::Starved);

        // Frame 1 never arrives, but 2 does.
        buffer.write(2, &frame(2.0));
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Concealed { sequence: 1 },
            "a buffered successor is what makes frame 1 a loss"
        );
        assert_eq!(
            buffer.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 2 }
        );
        assert_eq!(buffer.stats().frames_concealed, 1);
    }

    /// Resetting is what a reconnect does; nothing may survive it.
    #[test]
    fn reset_clears_everything() {
        let mut buffer = PlayoutBuffer::new(config(1));
        buffer.write(0, &frame(1.0));
        buffer.write(1, &frame(2.0));

        buffer.reset();

        assert_eq!(buffer.ready_frames(), 0);
        assert_eq!(buffer.stats(), PlayoutStats::default());
        assert_eq!(buffer.target_delay_frames(), 1);
    }

    /// A frame longer than a slot cannot be stored, and an empty one is a
    /// decoder bug; both are refused rather than played as something else.
    #[test]
    fn a_frame_that_cannot_be_stored_is_refused() {
        let mut buffer = PlayoutBuffer::new(config(0));

        assert_eq!(buffer.write(0, &[]), WriteOutcome::WrongLength);
        assert_eq!(
            buffer.write(0, &[0.5; 4 * 3 + 1]),
            WriteOutcome::WrongLength,
            "a frame larger than a slot has nowhere to go"
        );
        assert_eq!(buffer.ready_frames(), 0);
        assert_eq!(buffer.stats().frames_written, 0);
    }

    /// Frames keep whatever length they arrive with.
    ///
    /// A peer at another sample rate is resampled before it gets here, and a
    /// rate conversion does not yield a whole number of frames - 44.1k to 48k
    /// is 1.088 frames' worth. Padding or truncating to a fixed size would add
    /// or drop samples, and that is what changes pitch. Playing each frame at
    /// its own length is what keeps the sound at the pitch it was played at
    /// (ADR-028, ADR-013).
    #[test]
    fn frames_keep_the_length_they_arrived_with() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = vec![0.0f32; 12];

        // What a 44.1k -> 48k conversion of four-sample frames looks like:
        // alternating 4 and 5 samples, never a constant length.
        let arrivals: [&[f32]; 3] = [&[0.1, 0.2, 0.3, 0.4], &[0.5; 5], &[0.6; 4]];
        for (sequence, samples) in arrivals.iter().enumerate() {
            assert_eq!(
                buffer.write(sequence as u32, samples),
                WriteOutcome::Accepted
            );
        }

        for (sequence, expected) in arrivals.iter().enumerate() {
            let read = buffer.read_into(&mut out);
            assert_eq!(
                read.result,
                PlayoutResult::Played {
                    sequence: sequence as u32
                }
            );
            assert_eq!(
                read.samples,
                expected.len(),
                "frame {} must play at the length it arrived with",
                sequence
            );
            assert_eq!(
                &out[..read.samples],
                *expected,
                "no sample may be added or dropped"
            );
        }
    }

    /// Concealment lasts as long as the frame it stands in for, so a loss does
    /// not shift the timing of everything after it.
    #[test]
    fn concealment_matches_the_length_of_what_it_replaces() {
        let mut buffer = PlayoutBuffer::new(config(0));
        let mut out = vec![0.0f32; 12];

        buffer.write(0, &[0.5; 5]);
        let played = buffer.read_into(&mut out);
        assert_eq!(played.samples, 5);

        buffer.write(2, &[0.5; 5]);
        let concealed = buffer.read_into(&mut out);
        assert_eq!(concealed.result, PlayoutResult::Concealed { sequence: 1 });
        assert_eq!(
            concealed.samples, 5,
            "the gap must be as long as the audio it replaces"
        );
    }
}
