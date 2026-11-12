//! Takes one sample frame out of the played audio, or puts one in, slowly
//! (ADR-122).
//!
//! The receive buffer is kept at its target by playing a little faster when it
//! holds too much and a little slower when it holds too little. The speed is
//! changed one sample frame at a time: over [`SPAN_SECS`] the audio is read
//! from a position that runs ahead of the output by one sample frame (or falls
//! behind by one), by an even step. At 48 kHz that is 960 sample frames for one,
//! so the speed is off by 1/960 for those 20 ms - 1.8 cents, well under the
//! 5 cents at which a change of pitch is first noticed - and the pitch is never
//! off by more, however much the buffer has to be moved.
//!
//! Between two such stretches the audio is passed on untouched: no filter, no
//! delay. The interpolation (a windowed sinc of 16 points) runs only inside a
//! stretch.
//!
//! A stretch needs the eight sample frames after the one it is reading, so while
//! one is running the last few sample frames of a block are kept for the next
//! call. They are played at the end of the stretch, so nothing is lost, and the
//! audio always continues from where the previous call left off.

use std::f64::consts::PI;
use std::sync::OnceLock;

use super::stream::WIRE_CHANNELS;

/// How long one sample frame is taken or added over.
const SPAN_SECS: f64 = 0.020;

/// Points of the interpolation: the sample frame the read position is in, the
/// seven before it and the eight after it
const TAPS: usize = 16;
const LEFT: usize = TAPS / 2 - 1;
const RIGHT: usize = TAPS / 2;

/// Sample frames kept from one call to the next while a stretch is running: at
/// most those from the first the next read needs to the last that has come
const CARRY_FRAMES: usize = LEFT + RIGHT + 1;

/// Shortest block a stretch is started or carried on. Anything shorter is not a
/// frame of audio that was sent.
const MIN_BLOCK_FRAMES: usize = 2 * LEFT + 2;

/// Rows in the table of interpolation weights, by how far between two sample
/// frames the read position is; the weights between two rows are found on the
/// straight line between them
const PHASES: usize = 256;

/// Shape of the Kaiser window the weights are cut with. Chosen from the error of
/// the interpolation against a tone at 1 to 18 kHz (ADR-122).
const KAISER_BETA: f64 = 8.0;

/// The most that is owed at a time, in sample frames. A rate that asks for more
/// than a stretch can give in a moment is not made up for afterwards.
const MAX_OWED: f64 = 2.0;

/// How many stretches there have been.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StretchStats {
    /// Stretches that took a sample frame out (played faster)
    pub removals: u64,
    /// Stretches that put a sample frame in (played slower)
    pub insertions: u64,
    /// Stretches cut short because the audio was cut
    pub cut_short: u64,
}

/// A stretch in progress
#[derive(Debug, Clone, Copy)]
struct Stretch {
    /// +1 takes a sample frame out, -1 puts one in
    direction: f64,
    /// Sample frames made so far
    made: usize,
    /// Where the next one is read, in sample frames from the start of the window
    /// (the carried frames, then the block)
    position: f64,
}

/// Weights of the interpolation for a read position `phase / PHASES` of the way
/// between two sample frames: a sinc cut off by a Kaiser window and scaled to add
/// up to one. At 0 it is the sample frame itself, exactly.
///
/// Built once for the process. `Stretcher::new` builds it, so the audio callback
/// that calls `process` never has to.
static TABLE: OnceLock<Vec<[f32; TAPS]>> = OnceLock::new();

fn weights() -> &'static [[f32; TAPS]] {
    TABLE.get_or_init(|| {
        let bessel = |x: f64| {
            let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
            while term > 1e-14 * sum {
                term *= (x / (2.0 * k)).powi(2);
                sum += term;
                k += 1.0;
            }
            sum
        };
        (0..=PHASES)
            .map(|phase| {
                let t = phase as f64 / PHASES as f64;
                let mut row = [0.0f64; TAPS];
                for (j, weight) in row.iter_mut().enumerate() {
                    let x = j as f64 - LEFT as f64 - t;
                    let sinc = if x.abs() < 1e-12 {
                        1.0
                    } else {
                        (PI * x).sin() / (PI * x)
                    };
                    let r = x / RIGHT as f64;
                    let window = if r.abs() < 1.0 {
                        bessel(KAISER_BETA * (1.0 - r * r).sqrt()) / bessel(KAISER_BETA)
                    } else {
                        0.0
                    };
                    *weight = sinc * window;
                }
                let total: f64 = row.iter().sum();
                let mut out = [0.0f32; TAPS];
                for (weight, value) in out.iter_mut().zip(row) {
                    *weight = (value / total) as f32;
                }
                out
            })
            .collect()
    })
}

pub struct Stretcher {
    span: usize,
    /// Sample frames to take out (positive) or put in (negative) that the rate
    /// has asked for and no stretch has been started for
    owed: f64,
    /// The last sample frames of the audio so far, which a stretch that starts
    /// with the next block reads from
    history: [f32; LEFT * WIRE_CHANNELS],
    stretch: Option<Stretch>,
    carry: [f32; CARRY_FRAMES * WIRE_CHANNELS],
    carry_frames: usize,
    /// The carried frames and the block, laid out for reading. Allocated once.
    window: Vec<f32>,
    stats: StretchStats,
}

impl Stretcher {
    /// A stretcher for audio at `sample_rate`, for blocks of up to
    /// `widest_block_samples` interleaved samples.
    pub fn new(sample_rate: u32, widest_block_samples: usize) -> Self {
        let span = ((f64::from(sample_rate) * SPAN_SECS).round() as usize).max(MIN_BLOCK_FRAMES);
        weights();
        Self {
            span,
            owed: 0.0,
            history: [0.0; LEFT * WIRE_CHANNELS],
            stretch: None,
            carry: [0.0; CARRY_FRAMES * WIRE_CHANNELS],
            carry_frames: 0,
            window: vec![0.0; widest_block_samples + CARRY_FRAMES * WIRE_CHANNELS],
            stats: StretchStats::default(),
        }
    }

    pub fn stats(&self) -> StretchStats {
        self.stats
    }

    /// Whether a stretch is running now
    #[cfg(test)]
    fn is_stretching(&self) -> bool {
        self.stretch.is_some()
    }

    /// Forgets the stretch in progress and what is owed. For a stream that starts
    /// over.
    pub fn reset(&mut self) {
        self.cancel();
        self.owed = 0.0;
        self.history = [0.0; LEFT * WIRE_CHANNELS];
    }

    fn cancel(&mut self) {
        if self.stretch.take().is_some() {
            self.stats.cut_short += 1;
        }
        self.carry_frames = 0;
    }

    /// Passes on the `len` samples at the start of `block`, stretched as `ppm`
    /// asks, and returns how many samples that makes (up to a few sample frames
    /// more or fewer than `len`). `block` has to have room for those.
    ///
    /// `ppm` is how much faster than it was sent the audio is to be played, in
    /// parts per million: positive takes sample frames out, negative puts them
    /// in. `continuous` says `block` follows the audio before it; when it does
    /// not (silence, a concealed frame, a skip) a stretch in progress is given
    /// up and the block is passed on as it is.
    ///
    /// Allocation-free, for the audio callback.
    pub fn process(&mut self, block: &mut [f32], len: usize, ppm: f32, continuous: bool) -> usize {
        let len = len.min(block.len()) / WIRE_CHANNELS * WIRE_CHANNELS;
        let frames = len / WIRE_CHANNELS;
        let room = block.len() / WIRE_CHANNELS;
        if !continuous
            || frames < MIN_BLOCK_FRAMES
            || len + CARRY_FRAMES * WIRE_CHANNELS > self.window.len()
            || room < frames + CARRY_FRAMES
        {
            let flushed = self.flush_into(block, len);
            self.cancel();
            self.owed = 0.0;
            self.remember_history(&block[flushed..flushed + len]);
            return flushed + len;
        }

        self.owed = (self.owed + f64::from(ppm) * 1e-6 * frames as f64).clamp(-MAX_OWED, MAX_OWED);

        if self.stretch.is_none() {
            if self.owed.abs() < 1.0 {
                self.remember_history(&block[..len]);
                return len;
            }
            let direction = self.owed.signum();
            self.owed -= direction;
            if direction > 0.0 {
                self.stats.removals += 1;
            } else {
                self.stats.insertions += 1;
            }
            self.carry[..LEFT * WIRE_CHANNELS].copy_from_slice(&self.history);
            self.carry_frames = LEFT;
            self.stretch = Some(Stretch {
                direction,
                made: 0,
                position: LEFT as f64,
            });
        }

        let carried = self.carry_frames * WIRE_CHANNELS;
        self.window[..carried].copy_from_slice(&self.carry[..carried]);
        self.window[carried..carried + len].copy_from_slice(&block[..len]);
        let total = self.carry_frames + frames;

        let mut stretch = self.stretch.expect("a stretch is running");
        let step = 1.0 + stretch.direction / self.span as f64;
        let table = weights();
        let mut made = 0usize;

        loop {
            let index = stretch.position.floor() as usize;
            if index + RIGHT >= total {
                break;
            }
            let row = (stretch.position - index as f64) * PHASES as f64;
            let nearer = row.floor() as usize;
            let between = (row - nearer as f64) as f32;
            let mut taps = [0.0f32; TAPS];
            for (tap, (a, b)) in taps
                .iter_mut()
                .zip(table[nearer].iter().zip(&table[nearer + 1]))
            {
                *tap = a + (b - a) * between;
            }
            let first = (index - LEFT) * WIRE_CHANNELS;
            for channel in 0..WIRE_CHANNELS {
                let mut sum = 0.0f32;
                for (j, tap) in taps.iter().enumerate() {
                    sum += tap * self.window[first + j * WIRE_CHANNELS + channel];
                }
                block[made * WIRE_CHANNELS + channel] = sum;
            }
            made += 1;
            stretch.made += 1;

            if stretch.made == self.span {
                // The stretch is done: the read position is now a whole number
                // of sample frames from where it started, one more or less than
                // the output has come to, and the rest passes on as it is.
                let from = (stretch.position + step).round() as usize;
                let rest = total.saturating_sub(from);
                block[made * WIRE_CHANNELS..(made + rest) * WIRE_CHANNELS].copy_from_slice(
                    &self.window[from * WIRE_CHANNELS..(from + rest) * WIRE_CHANNELS],
                );
                made += rest;
                self.stretch = None;
                self.carry_frames = 0;
                self.history.copy_from_slice(
                    &self.window[(total - LEFT) * WIRE_CHANNELS..total * WIRE_CHANNELS],
                );
                return made * WIRE_CHANNELS;
            }
            stretch.position += step;
        }

        // Out of audio to read from: keep what the next sample frame needs
        let keep_from = stretch.position.floor() as usize - LEFT;
        self.carry_frames = total - keep_from;
        self.carry[..self.carry_frames * WIRE_CHANNELS]
            .copy_from_slice(&self.window[keep_from * WIRE_CHANNELS..total * WIRE_CHANNELS]);
        stretch.position -= keep_from as f64;
        self.stretch = Some(stretch);
        made * WIRE_CHANNELS
    }

    /// Puts the audio a stretch in progress has read but not yet passed on in front
    /// of the `len` samples at the start of `block`, which follow it in time but
    /// are not stretched. Returns how many samples went in front (0 without a
    /// stretch, or when `block` has no room). Without this the sample frames the
    /// stretch holds back are lost and what follows jumps ahead of what was played.
    fn flush_into(&mut self, block: &mut [f32], len: usize) -> usize {
        let Some(stretch) = self.stretch else {
            return 0;
        };
        let from = (stretch.position.round() as usize).min(self.carry_frames);
        let flushed = (self.carry_frames - from) * WIRE_CHANNELS;
        if flushed == 0 || flushed + len > block.len() {
            return 0;
        }
        block.copy_within(..len, flushed);
        block[..flushed]
            .copy_from_slice(&self.carry[from * WIRE_CHANNELS..self.carry_frames * WIRE_CHANNELS]);
        flushed
    }

    /// Keeps the last [`LEFT`] sample frames of `played` for a stretch that
    /// starts with the next block
    fn remember_history(&mut self, played: &[f32]) {
        let keep = LEFT * WIRE_CHANNELS;
        if played.len() >= keep {
            self.history.copy_from_slice(&played[played.len() - keep..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const BLOCK: usize = 64;
    /// The ramp rises by this much a sample frame: small, so single precision
    /// has room for the step between neighbours at the end of a long ramp
    const RISE: f32 = 0.001;

    /// Feeds `blocks` blocks of a ramp (sample frame n has the value n) through
    /// a stretcher at `ppm`, and returns what came out
    fn through(ppm: f32, blocks: usize) -> (Vec<f32>, Stretcher) {
        let mut stretcher = Stretcher::new(RATE, 4096);
        let mut played = Vec::new();
        let mut next = 0u64;
        for _ in 0..blocks {
            let mut block = vec![0.0f32; 4096];
            for i in 0..BLOCK {
                block[2 * i] = (next + i as u64) as f32 * RISE;
                block[2 * i + 1] = -((next + i as u64) as f32 * RISE);
            }
            next += BLOCK as u64;
            let len = stretcher.process(&mut block, BLOCK * 2, ppm, true);
            played.extend_from_slice(&block[..len]);
        }
        (played, stretcher)
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_a_stretcher_is_made_the_interpolation_weights_are_already_built() {
        let _stretcher = Stretcher::new(RATE, 4096);
        assert!(
            TABLE.get().is_some(),
            "the weights were left to the first block that stretches, in the audio callback"
        );
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_no_rate_is_asked_for_the_audio_passes_on_untouched() {
        let (played, stretcher) = through(0.0, 100);
        let expected: Vec<f32> = (0..100 * BLOCK)
            .flat_map(|n| [n as f32 * RISE, -(n as f32 * RISE)])
            .collect();
        assert_eq!(played, expected);
        assert_eq!(stretcher.stats(), StretchStats::default());
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_the_rate_asks_to_play_faster_a_sample_frame_goes_out_every_so_often() {
        // 500 ppm is one sample frame in 2000: over 60000 sample frames, 30 out
        let blocks = 60_000 / BLOCK;
        let (played, stretcher) = through(500.0, blocks);
        let frames = played.len() / 2;
        let lost = (blocks * BLOCK) as i64 - frames as i64;
        assert!(
            (28..=32).contains(&lost),
            "a sample frame goes out every 2000: {lost} went"
        );
        assert!(stretcher.stats().removals >= 28);
        assert_eq!(stretcher.stats().insertions, 0);
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn when_the_rate_asks_to_play_slower_a_sample_frame_comes_in_every_so_often() {
        let blocks = 60_000 / BLOCK;
        let (played, stretcher) = through(-500.0, blocks);
        let frames = played.len() / 2;
        let gained = frames as i64 - (blocks * BLOCK) as i64;
        assert!(
            (28..=32).contains(&gained),
            "a sample frame comes in every 2000: {gained} came"
        );
        assert!(stretcher.stats().insertions >= 28);
        assert_eq!(stretcher.stats().removals, 0);
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn a_ramp_stays_a_ramp_and_no_step_in_it_is_bigger_than_the_stretch_makes() {
        // The ramp rises by RISE per sample frame, so the stretch can only make the
        // step between neighbours RISE +- 1/960 of it. The rest of the tolerance is
        // the resolution of an f32 at the top of the ramp, divided by RISE.
        for ppm in [1000.0f32, -1000.0] {
            let (played, _) = through(ppm, 400);
            let left: Vec<f32> = played.iter().step_by(2).copied().collect();
            let mut previous = left[0];
            for (i, &value) in left.iter().enumerate().skip(1) {
                let step = (value - previous) / RISE;
                assert!(
                    (step - 1.0).abs() <= 1.0 / 960.0 + 1e-2,
                    "at {i}: step {step} at {ppm} ppm"
                );
                previous = value;
            }
        }
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn every_sample_frame_of_the_ramp_is_played_once_in_order_after_a_stretch_ends() {
        // After the stretch the audio is the ramp again, shifted by one sample frame
        let (played, _) = through(1000.0, 100);
        let left: Vec<f32> = played.iter().step_by(2).copied().collect();
        let last = *left.last().expect("played");
        assert!(left.windows(2).all(|pair| pair[1] > pair[0]));
        // The last sample frame played is one of the ramp's, to within what is
        // still carried for the next block
        assert!(last > 6.0 && last <= 6.4, "last {last}");
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn a_stretch_given_up_on_a_cut_passes_on_what_it_held_back_and_then_the_block_as_it_is() {
        let mut stretcher = Stretcher::new(RATE, 4096);
        let next = std::cell::Cell::new(0u64);
        let ramp = |block: &mut Vec<f32>, frames: usize| {
            block.clear();
            for _ in 0..frames {
                let v = next.get() as f32 * RISE;
                block.extend_from_slice(&[v, v]);
                next.set(next.get() + 1);
            }
            block.resize(4096, 0.0);
        };
        let mut block = Vec::new();
        for _ in 0..40 {
            ramp(&mut block, BLOCK);
            stretcher.process(&mut block, BLOCK * 2, 1000.0, true);
        }
        assert!(stretcher.is_stretching());
        // The next block follows the ramp but is not stretched (a concealed frame)
        let before = next.get();
        ramp(&mut block, BLOCK);
        let len = stretcher.process(&mut block, BLOCK * 2, 1000.0, false);
        assert!(!stretcher.is_stretching());
        assert_eq!(stretcher.stats().cut_short, 1);
        assert!(len > BLOCK * 2, "what was held back goes first: {len}");
        let left: Vec<f32> = block[..len].iter().step_by(2).copied().collect();
        // The block ends the way it came, and what went before it carries on
        // from the last sample frame passed on, to within the part of a sample
        // frame the stretch had come to
        let last = *left.last().unwrap();
        assert!(
            (last - (next.get() - 1) as f32 * RISE).abs() < 1e-3,
            "last {last}"
        );
        let held_back = len / WIRE_CHANNELS - BLOCK;
        let first = left[held_back];
        assert!(
            (first - before as f32 * RISE).abs() < 1e-3,
            "block starts {first}"
        );
        for pair in left[..=held_back].windows(2) {
            let step = pair[1] - pair[0];
            assert!((step - RISE).abs() < RISE * 0.6, "step {step}");
        }
    }

    /// Verifies: REQ-LAT-134
    #[test]
    fn a_block_with_no_room_for_a_stretch_passes_as_it_is() {
        let mut stretcher = Stretcher::new(RATE, 4096);
        let mut block = vec![0.25f32; BLOCK * 2];
        for _ in 0..100 {
            let len = stretcher.process(&mut block, BLOCK * 2, 1000.0, true);
            assert_eq!(len, BLOCK * 2);
        }
        assert_eq!(stretcher.stats(), StretchStats::default());
    }
}
