//! Waveform concealment: filling the gaps the FEC could not repair with sound that continues
//! what was heard (ADR-105).
//!
//! The play-out buffer meets two kinds of gap, and they are different problems:
//!
//! - **A frame is missing but later ones have arrived** ([`Concealer::fill_gap`]). Both sides
//!   of the hole are known. A piece of the recent past is searched for whose beginning looks
//!   like the end of what was just played and whose end looks like the start of what comes
//!   next (normalised cross-correlation of the two), and copied in, joined to the frame that
//!   follows by a linear cross-fade. A prediction from both sides (linear prediction forwards
//!   from the past and backwards from the future, cross-faded) is made as well; the copy is
//!   used where it is close to the prediction or very well correlated, the prediction where
//!   it is not.
//! - **Nothing at or after the due frame has arrived** ([`Concealer::extrapolate`]). Only the
//!   past is known, so the sound is predicted from it: linear prediction (LPC) of the last
//!   few milliseconds, mixed with a repeat of the last pitch period when the sound is
//!   periodic, and faded out. When the real audio returns it is cross-faded in.
//!
//! Everything here runs in the audio callback: all buffers are allocated in
//! [`Concealer::new`], nothing is allocated, locked or waited for afterwards, and no frame is
//! delayed to wait for the next one (the "after" side of a gap is only looked at when the
//! play-out buffer already holds it).

use std::f32::consts::PI;

/// Pre-emphasis of the signal LPC is computed on and run in.
const PREEMPHASIS: f32 = 0.97;
/// Order of the linear predictor.
const LPC_ORDER: usize = 16;
/// Reflection coefficients are kept inside this, which keeps the predictor stable.
const REFLECTION_LIMIT: f64 = 0.999;
/// The k-th coefficient is scaled by this to the power k: the resonances get wider, so an
/// extrapolation does not ring on a narrow peak.
const BANDWIDTH_EXPANSION: f64 = 0.99;
/// A prediction that gets larger than this has diverged and is cut off.
const DIVERGED_ABOVE: f32 = 2.0;

/// Weakest normalised correlation of a pitch period that is used at all.
const PITCH_MIN_CORRELATION: f32 = 0.4;
/// Weakest correlation the coarse pitch search accepts as a starting point.
const PITCH_COARSE_MIN_CORRELATION: f32 = 0.3;
/// A shorter period (a sub-multiple of the best one) wins if it is at least this good, so the
/// double of the true period is not taken for it.
const PITCH_SUBMULTIPLE_KEEPS: f32 = 0.92;
/// Largest divisor tried for the sub-multiples.
const PITCH_MAX_DIVISOR: usize = 8;
/// Below this correlation the repeat of the last period is not mixed in; above
/// `PITCH_MIX_FULL` it is mixed in at its full weight.
const PITCH_MIX_START: f32 = 0.55;
const PITCH_MIX_FULL: f32 = 0.80;
/// Largest weight of the repeat of the last period against the linear prediction.
const PITCH_MAX_WEIGHT: f32 = 0.6;

/// How long an extrapolation is kept at full level, in milliseconds.
const HOLD_MS: f32 = 10.0;
/// How long it takes to fade out after that: shortest (not periodic) and longest (periodic).
const FADE_MIN_MS: f32 = 30.0;
const FADE_MAX_MS: f32 = 120.0;

/// Weakest correlation a gap fill takes from the past.
const FILL_MIN_CORRELATION: f32 = 0.6;
/// The fill is scaled to the level of what was played, but not by more than this either way.
const FILL_MAX_GAIN: f32 = 4.0;

/// Share of the reads missing above which extrapolation stops (the output would be mostly
/// predicted sound), and the share it has to fall below, for `RESUME_AFTER_SECS`, to start
/// again.
const SUPPRESS_ABOVE_MISS_RATE: f32 = 0.08;
const RESUME_BELOW_MISS_RATE: f32 = 0.02;
const RESUME_AFTER_SECS: f32 = 3.0;
/// The miss rate is an exponential average with this time constant.
const MISS_RATE_SECS: f32 = 1.0;

/// The copy of the past is kept over the two-sided prediction of a gap when it differs from
/// it by less than this share of the prediction's energy.
const COPY_AGREES_BELOW: f32 = 0.3;
/// A copy of the past this alike to the end of what was played (and the start of what follows)
/// is used without asking the prediction.
const COPY_SURE_ABOVE: f32 = 0.95;
/// Coefficients of the predictor used across a gap are scaled by this to the power k.
const INTERPOLATION_EXPANSION: f64 = 0.999;
/// A prediction across a gap that is louder than this times the loudest sample of the
/// stretch before the gap and of the frame after it is not used.
const INTERPOLATION_LOUDER_THAN_SURROUNDINGS: f32 = 1.5;

fn error_energy(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// The lengths below are given for 48 kHz and scaled to the rate the stream runs at.
const REFERENCE_RATE: u64 = 48_000;

/// `position` taken modulo `period`, for a position that is usually already inside it.
fn wrap(position: usize, period: usize) -> usize {
    if position < period {
        position
    } else {
        position % period
    }
}

fn at_rate(rate: u32, samples_at_48k: usize) -> usize {
    ((samples_at_48k as u64 * u64::from(rate) + REFERENCE_RATE / 2) / REFERENCE_RATE).max(1)
        as usize
}

/// Every length the concealer works in, in samples per channel at the stream's rate.
#[derive(Debug, Clone, Copy)]
struct Lengths {
    /// Played sound kept for searching: 107 ms.
    history: usize,
    /// The part of the past that a gap fill has to look like: 1 ms.
    template: usize,
    /// Nearest and farthest a candidate may lie back from the end of the history.
    min_lag: usize,
    max_lag: usize,
    /// The first samples of a fill are moved to meet the last played sample over this many.
    onset: usize,
    /// Level of a copy is matched over this many samples before the copied stretch.
    gain_span: usize,
    /// A fill's cross-fade into the frame that follows.
    join: usize,
    /// LPC is computed over the last 10 ms.
    lpc_window: usize,
    /// Pitch is looked for over the last 38 ms, in periods of 60 to 800 Hz.
    pitch_window: usize,
    pitch_min_period: usize,
    pitch_max_period: usize,
    pitch_coarse_step: usize,
    /// The cross-fade into returning audio: shortest and longest.
    rejoin_min: usize,
    rejoin_max: usize,
}

impl Lengths {
    fn new(rate: u32) -> Self {
        let r = |n| at_rate(rate, n);
        Self {
            history: r(5120),
            template: r(48),
            min_lag: r(32),
            max_lag: r(512),
            onset: r(32),
            gain_span: r(48),
            join: r(64),
            lpc_window: r(480),
            pitch_window: r(1824),
            pitch_min_period: (rate as usize / 800).max(2),
            pitch_max_period: (rate as usize / 60).max(4),
            pitch_coarse_step: r(4),
            rejoin_min: r(64),
            rejoin_max: r(240),
        }
    }

    /// Longest stretch a gap fill runs for before the caller falls back: the history.
    fn max_fill(&self) -> usize {
        self.history
    }
}

/// Dot product written so that the compiler can vectorise it.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut lanes = [0.0f32; 8];
    let (chunks_a, rest_a) = a.as_chunks::<8>();
    let (chunks_b, rest_b) = b.as_chunks::<8>();
    for (x, y) in chunks_a.iter().zip(chunks_b) {
        for i in 0..8 {
            lanes[i] += x[i] * y[i];
        }
    }
    let mut sum: f32 = lanes.iter().sum();
    for (x, y) in rest_a.iter().zip(rest_b) {
        sum += x * y;
    }
    sum
}

/// What was played, per channel, kept contiguous so it can be searched as a slice. Appending
/// is amortised: the buffer is twice what is needed and the newest part moves to the front
/// when it is full.
struct History {
    channels: Vec<Vec<f32>>,
    len: usize,
    keep: usize,
}

impl History {
    fn new(channels: usize, keep: usize) -> Self {
        Self {
            channels: vec![vec![0.0; keep * 2]; channels],
            len: 0,
            keep,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    /// Appends interleaved `samples`.
    fn push(&mut self, samples: &[f32]) {
        let ch = self.channels.len();
        let mut frames = samples.len() / ch;
        let mut samples = &samples[..frames * ch];
        if frames > self.keep {
            samples = &samples[(frames - self.keep) * ch..];
            frames = self.keep;
        }
        if self.len + frames > self.channels[0].len() {
            let from = self.len - (self.keep - frames).min(self.len);
            for buffer in &mut self.channels {
                buffer.copy_within(from..self.len, 0);
            }
            self.len -= from;
        }
        for (c, buffer) in self.channels.iter_mut().enumerate() {
            for (slot, frame) in buffer[self.len..self.len + frames]
                .iter_mut()
                .zip(samples.chunks_exact(ch))
            {
                *slot = frame[c];
            }
        }
        self.len += frames;
    }

    fn channel(&self, c: usize) -> &[f32] {
        &self.channels[c][..self.len]
    }
}

/// Which of the two kinds of gap an extrapolation is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SynthState {
    /// Nothing is being extrapolated.
    Idle,
    /// A prediction is being played.
    Running,
    /// It has faded out, diverged or could not start: silence, until audio returns.
    Done,
}

/// The state of one extrapolation: the predictor, the period to repeat and where the fade is.
struct Synth {
    state: SynthState,
    /// Per channel: predictor coefficients `a[1..=order]` (`a[0]` unused).
    lpc: Vec<[f32; LPC_ORDER + 1]>,
    /// Per channel: the pre-emphasised signal, the past followed by what the predictor has
    /// produced from it.
    signal: Vec<Vec<f32>>,
    /// Per channel: the past followed by its last period repeated on, which is mixed with
    /// the prediction. It is kept apart from the predictor, which it would make unstable.
    repeat: Vec<Vec<f32>>,
    signal_len: usize,
    /// Per channel: the last output sample, for undoing the pre-emphasis.
    last_out: Vec<f32>,
    /// Pitch period in samples (fractional), or 0 when the sound is not periodic.
    period: f32,
    /// Weight of the repeat of the last period against the linear prediction.
    pitch_weight: f32,
    /// Samples produced so far.
    produced: usize,
    /// Samples after which the output has faded to nothing.
    fade_start: usize,
    fade_len: usize,
    /// Correlation of the pitch (0 when there is none), which sets the cross-fade length.
    periodicity: f32,
}

/// Room kept ahead of the pitch repeat in the pre-emphasised signal: the longest period plus
/// the predictor order plus slack for interpolation.
fn signal_keep(lengths: &Lengths) -> usize {
    lengths.pitch_max_period + LPC_ORDER + 4
}

impl Synth {
    fn new(channels: usize, lengths: &Lengths) -> Self {
        let keep = signal_keep(lengths);
        Self {
            state: SynthState::Idle,
            lpc: vec![[0.0; LPC_ORDER + 1]; channels],
            signal: vec![vec![0.0; keep * 4]; channels],
            repeat: vec![vec![0.0; keep * 4]; channels],
            signal_len: 0,
            last_out: vec![0.0; channels],
            period: 0.0,
            pitch_weight: 0.0,
            produced: 0,
            fade_start: 0,
            fade_len: 1,
            periodicity: 0.0,
        }
    }

    /// Level of the sample number `n` of the extrapolation.
    fn envelope(&self, n: usize) -> f32 {
        if n < self.fade_start {
            1.0
        } else {
            (1.0 - (n - self.fade_start) as f32 / self.fade_len as f32).max(0.0)
        }
    }
}

/// Watches how much is missing, so extrapolation does not turn a bad link into a stream of
/// predicted sound.
struct MissGuard {
    rate: f32,
    suppressed: bool,
    clean_secs: f32,
}

impl MissGuard {
    fn new() -> Self {
        Self {
            rate: 0.0,
            suppressed: false,
            clean_secs: 0.0,
        }
    }

    fn note(&mut self, missed: bool, secs: f32) {
        let weight = (secs / MISS_RATE_SECS).min(1.0);
        self.rate += weight * (if missed { 1.0 } else { 0.0 } - self.rate);
        if self.rate > SUPPRESS_ABOVE_MISS_RATE {
            self.suppressed = true;
            self.clean_secs = 0.0;
        } else if self.suppressed {
            if self.rate < RESUME_BELOW_MISS_RATE {
                self.clean_secs += secs;
                if self.clean_secs >= RESUME_AFTER_SECS {
                    self.suppressed = false;
                }
            } else {
                self.clean_secs = 0.0;
            }
        }
    }
}

/// Scratch space for the analyses, so nothing is allocated when a gap is met.
struct Scratch {
    /// Mono mix of the history, for the pitch search.
    mono: Vec<f32>,
    /// Running sum of squares of `mono`, for the energy of any stretch of it.
    energy: Vec<f64>,
    /// The windowed, pre-emphasised stretch LPC is computed on.
    window: Vec<f32>,
    /// Predictions run forwards from the past and backwards from the future of a gap.
    forward: Vec<f32>,
    backward: Vec<f32>,
    /// What [`Concealer::interpolate`] made, interleaved.
    interp: Vec<f32>,
}

/// Fits a linear predictor to the last `window.len()` samples of `history` (pre-emphasised,
/// Hann window, autocorrelation method, Levinson-Durbin), into `coefficients`. False when the
/// stretch is silent.
fn fit_lpc(
    history: &[f32],
    window: &mut [f32],
    emphasis: f32,
    expansion: f64,
    coefficients: &mut [f32; LPC_ORDER + 1],
) -> bool {
    let n = window.len();
    let end = history.len();
    for (i, slot) in window.iter_mut().enumerate() {
        let at = end - n + i;
        let emphasised = history[at] - emphasis * history[at - 1];
        let hann = 0.5 - 0.5 * (2.0 * PI * (i as f32 + 0.5) / n as f32).cos();
        *slot = emphasised * hann;
    }
    let mut r = [0.0f64; LPC_ORDER + 1];
    for (lag, slot) in r.iter_mut().enumerate() {
        *slot = f64::from(dot(&window[lag..], &window[..n - lag]));
    }
    if r[0] < 1e-9 {
        return false;
    }
    // A little white noise keeps the system well conditioned
    r[0] *= 1.0 + 1e-4;

    let mut a = [0.0f64; LPC_ORDER + 1];
    a[0] = 1.0;
    let mut error = r[0];
    for i in 1..=LPC_ORDER {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = (-acc / error).clamp(-REFLECTION_LIMIT, REFLECTION_LIMIT);
        let previous = a;
        for j in 1..i {
            a[j] = previous[j] + k * previous[i - j];
        }
        a[i] = k;
        error *= 1.0 - k * k;
        if error <= 0.0 {
            return false;
        }
    }
    let mut factor = 1.0;
    for (k, slot) in coefficients.iter_mut().enumerate().skip(1) {
        factor *= expansion;
        *slot = (a[k] * factor) as f32;
    }
    true
}

/// Fills gaps in a stream of interleaved frames. One per received stream.
pub struct Concealer {
    channels: usize,
    rate: u32,
    lengths: Lengths,
    history: History,
    /// Samples per channel concealed since a real frame last played.
    run: usize,
    /// Per channel: the continuation of a fill or extrapolation, to be cross-faded into the
    /// audio that follows it. `tail_pos..tail_len` is what is still to be used.
    tail: Vec<Vec<f32>>,
    tail_pos: usize,
    tail_len: usize,
    synth: Synth,
    guard: MissGuard,
    scratch: Scratch,
}

impl Concealer {
    /// A concealer for interleaved frames of `channels` channels at `sample_rate`.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let channels = channels.max(1);
        let lengths = Lengths::new(sample_rate);
        let tail_room = lengths.rejoin_max.max(lengths.join);
        let mono_room = lengths.pitch_window + lengths.pitch_max_period;
        Self {
            channels,
            rate: sample_rate,
            history: History::new(channels, lengths.history),
            run: 0,
            tail: vec![vec![0.0; tail_room]; channels],
            tail_pos: 0,
            tail_len: 0,
            synth: Synth::new(channels, &lengths),
            guard: MissGuard::new(),
            scratch: Scratch {
                mono: vec![0.0; mono_room],
                energy: vec![0.0; mono_room + 1],
                window: vec![0.0; lengths.lpc_window + 1],
                forward: vec![0.0; LPC_ORDER + lengths.history],
                backward: vec![0.0; LPC_ORDER + lengths.history],
                interp: vec![0.0; lengths.history * channels],
            },
            lengths,
        }
    }

    /// Forgets everything heard: a connection started over, or the sender came back after
    /// being silent.
    pub fn reset(&mut self) {
        self.history.clear();
        self.run = 0;
        self.tail_len = 0;
        self.tail_pos = 0;
        self.synth.state = SynthState::Idle;
        self.guard = MissGuard::new();
    }

    /// Something other than a gap and other than audio to play came out of the buffer
    /// (priming, or silence for raising the delay): what was being continued is no longer
    /// where the sound is.
    pub fn interrupt(&mut self) {
        self.run = 0;
        self.tail_len = 0;
        self.tail_pos = 0;
        self.synth.state = SynthState::Idle;
    }

    /// Notes one read of the buffer while playing, `missed` when the frame was not there.
    pub fn note_read(&mut self, missed: bool, frame_samples: usize) {
        let secs = (frame_samples / self.channels) as f32 / self.rate as f32;
        self.guard.note(missed, secs);
    }

    /// A real frame is about to play: joins it to what came before, and remembers it.
    pub fn on_played(&mut self, frame: &mut [f32]) {
        let ch = self.channels;
        if frame.len() < ch {
            return;
        }
        if self.synth.state == SynthState::Running {
            self.start_tail_from_synth();
        }
        self.synth.state = SynthState::Idle;
        self.run = 0;

        if self.tail_pos < self.tail_len {
            let frames = frame.len() / ch;
            let usable = (self.tail_len - self.tail_pos).min(frames);
            let span = self.tail_len as f32 + 1.0;
            for i in 0..usable {
                let weight = (self.tail_pos + i + 1) as f32 / span;
                for c in 0..ch {
                    let slot = &mut frame[i * ch + c];
                    *slot = self.tail[c][self.tail_pos + i] * (1.0 - weight) + *slot * weight;
                }
            }
            self.tail_pos += usable;
        }
        self.history.push(frame);
    }

    /// Predicts a gap from both sides: forwards from what was played and backwards from
    /// `next`, with one predictor (the statistics of a signal read backwards are those of the
    /// signal), cross-fading from the first to the second across the gap. The result goes to
    /// `scratch.interp`; how much the two predictions disagree is returned, 0 for none and 2
    /// for opposite.
    fn interpolate(&mut self, hole: usize, next: &[f32]) -> Option<f32> {
        let ch = self.channels;
        let l = self.lengths;
        let available = self.history.len;
        if hole == 0
            || hole > l.history
            || available < l.lpc_window + 1
            || next.len() / ch < LPC_ORDER
        {
            return None;
        }
        let mut coefficients = [0.0f32; LPC_ORDER + 1];
        let (mut apart, mut total) = (0.0f32, 0.0f32);
        // What a predictor starting from a few samples may not do is to be louder than what is
        // around the gap: it can swell on the way when its resonances are sharp.
        let mut bound = 0.0f32;
        for c in 0..ch {
            let h = self.history.channel(c);
            for v in &h[available - hole.min(available)..] {
                bound = bound.max(v.abs());
            }
        }
        for v in &next[..(hole.min(next.len() / ch)) * ch] {
            bound = bound.max(v.abs());
        }
        bound *= INTERPOLATION_LOUDER_THAN_SURROUNDINGS;
        for c in 0..ch {
            let h = self.history.channel(c);
            let window = &mut self.scratch.window[..l.lpc_window];
            if !fit_lpc(h, window, 0.0, INTERPOLATION_EXPANSION, &mut coefficients) {
                return None;
            }
            let (forward, backward) = (&mut self.scratch.forward, &mut self.scratch.backward);
            forward[..LPC_ORDER].copy_from_slice(&h[available - LPC_ORDER..]);
            for j in 0..LPC_ORDER {
                backward[j] = next[(LPC_ORDER - 1 - j) * ch + c];
            }
            for n in LPC_ORDER..LPC_ORDER + hole {
                let (mut f, mut b) = (0.0f32, 0.0f32);
                for k in 1..=LPC_ORDER {
                    f -= coefficients[k] * forward[n - k];
                    b -= coefficients[k] * backward[n - k];
                }
                forward[n] = f;
                backward[n] = b;
            }
            for i in 0..hole {
                let f = forward[LPC_ORDER + i];
                let b = backward[LPC_ORDER + hole - 1 - i];
                let weight = 1.0 - (i as f32 + 0.5) / hole as f32;
                let v = f * weight + b * (1.0 - weight);
                if v.is_nan() || v.abs() > bound {
                    return None;
                }
                self.scratch.interp[i * ch + c] = v;
                apart += (f - b) * (f - b);
                total += f * f + b * b;
            }
        }
        Some(apart / (total + 1e-12))
    }

    /// The lag at which the recent past, followed by the periodic continuation of the stretch
    /// that follows it, best resembles the end of what was played and (if `next` is known) the
    /// start of what follows the gap. Positive score: it is copied as it is; negative: inverted.
    fn find_match(&self, hole: usize, next: Option<&[f32]>) -> Option<(usize, f32)> {
        let ch = self.channels;
        let l = self.lengths;
        let available = self.history.len;
        if available < l.template + l.min_lag + 1 {
            return None;
        }
        let max_lag = l.max_lag.min(available - l.template);
        if max_lag < l.min_lag {
            return None;
        }
        let after = next.map_or(0, |n| (n.len() / ch).min(l.template));

        let mut target_energy = 0.0f32;
        for c in 0..ch {
            let t = &self.history.channel(c)[available - l.template..];
            target_energy += dot(t, t);
        }
        if let Some(next) = next {
            target_energy += dot(&next[..after * ch], &next[..after * ch]);
        }
        if target_energy < 1e-12 {
            return None;
        }

        let mut best = (0usize, 0.0f32);
        for lag in l.min_lag..=max_lag {
            let start = available - lag;
            let mut cross = 0.0f32;
            let mut energy = 0.0f32;
            for c in 0..ch {
                let h = self.history.channel(c);
                let candidate = &h[start - l.template..start];
                cross += dot(candidate, &h[available - l.template..]);
                energy += dot(candidate, candidate);
                if let Some(next) = next {
                    for j in 0..after {
                        let v = h[start + wrap(hole + j, lag)];
                        cross += v * next[j * ch + c];
                        energy += v * v;
                    }
                }
            }
            let score = cross / (target_energy * energy + 1e-20).sqrt();
            if score.abs() > best.1.abs() {
                best = (lag, score);
            }
        }
        (best.0 != 0 && best.1.abs() >= FILL_MIN_CORRELATION).then_some(best)
    }

    /// Writes the copy of the past found by [`Self::find_match`] into `out`, and the
    /// continuation after it into the tail.
    fn copy_from_past(&mut self, out: &mut [f32], lag: usize, score: f32) {
        let ch = self.channels;
        let l = self.lengths;
        let hole = out.len() / ch;
        let available = self.history.len;
        let sign = if score < 0.0 { -1.0f32 } else { 1.0 };
        let start = available - lag;
        let onset = l.onset.min(hole);
        let fill_run = self.run;
        let max_fill = l.max_fill();
        let hold = at_rate(self.rate, (HOLD_MS * 48.0) as usize);
        let level = move |position: usize| -> f32 {
            if position < hold {
                1.0
            } else {
                (1.0 - (position - hold) as f32 / (max_fill - hold).max(1) as f32).max(0.0)
            }
        };
        let join = l.join.min(self.tail[0].len());

        for c in 0..ch {
            let h = self.history.channel(c);
            // The level of what was played, against the level of the stretch it is copied from
            let span = l.gain_span.min(start);
            let (e_cand, e_tgt) = (
                dot(&h[start - span..start], &h[start - span..start]),
                dot(&h[available - span..], &h[available - span..]),
            );
            let gain = if e_cand > 1e-20 {
                (e_tgt / e_cand).sqrt().min(FILL_MAX_GAIN)
            } else {
                0.0
            } * sign;

            // The sample that would have come before the fill, against the one that did
            let step = h[available - 1] - h[start - 1] * gain;
            for i in 0..hole {
                let mut v = h[start + wrap(i, lag)] * gain;
                if i < onset {
                    v += step * (1.0 - i as f32 / onset as f32);
                }
                out[i * ch + c] = v * level(fill_run + i);
            }
            for j in 0..join {
                self.tail[c][j] =
                    h[start + wrap(hole + j, lag)] * gain * level(fill_run + hole + j);
            }
        }
        self.tail_pos = 0;
        self.tail_len = join;
    }

    /// Writes into `out` sound to stand for a frame that is missing while `next`, the first
    /// samples of the frame after it, is already held. Returns false when nothing in the past
    /// is similar enough, or the gap has gone on too long; `out` is then untouched.
    pub fn fill_gap(&mut self, out: &mut [f32], next: Option<&[f32]>) -> bool {
        let ch = self.channels;
        if out.is_empty() || !out.len().is_multiple_of(ch) {
            return false;
        }
        let hole = out.len() / ch;
        if self.run + hole > self.lengths.max_fill() {
            return false;
        }
        let predicted = next.and_then(|next| self.interpolate(hole, next));
        match (self.find_match(hole, next), predicted) {
            (None, None) => return false,
            (None, Some(_)) => self.use_prediction(out),
            (Some((lag, score)), None) => self.copy_from_past(out, lag, score),
            (Some((lag, score)), Some(_)) => {
                // The copy is the more exact where it is right, the prediction where it is
                // not; they agree where the copy is right.
                self.copy_from_past(out, lag, score);
                let interp = &self.scratch.interp[..out.len()];
                let apart = error_energy(out, interp) / (dot(interp, interp) + 1e-12);
                if score.abs() < COPY_SURE_ABOVE && apart >= COPY_AGREES_BELOW {
                    self.use_prediction(out);
                }
            }
        }
        self.synth.state = SynthState::Idle;
        self.run += hole;
        self.history.push(out);
        true
    }

    /// Takes the two-sided prediction as the fill. It is anchored to the frame after the gap
    /// and needs no cross-fade into it.
    fn use_prediction(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.scratch.interp[..out.len()]);
        self.tail_len = 0;
        self.tail_pos = 0;
    }

    /// Writes into `out` a prediction of the sound that should come next, for a frame that is
    /// due while nothing at or after it has arrived. Returns false when nothing sensible can
    /// be predicted (or it has faded out, or there is too much missing): the caller plays
    /// silence.
    pub fn extrapolate(&mut self, out: &mut [f32]) -> bool {
        let ch = self.channels;
        if out.is_empty() || !out.len().is_multiple_of(ch) {
            return false;
        }
        let mut fresh = false;
        match self.synth.state {
            SynthState::Done => return false,
            SynthState::Idle => {
                fresh = true;
                self.tail_len = 0;
                self.tail_pos = 0;
                if self.guard.suppressed || !self.start_synth() {
                    self.synth.state = SynthState::Done;
                    return false;
                }
            }
            SynthState::Running => {}
        }
        let frames = out.len() / ch;
        let audible = self.generate(Some(out), frames);
        if fresh {
            self.meet_last_played(out);
        }
        if !audible {
            self.synth.state = SynthState::Done;
        }
        self.history.push(out);
        true
    }

    /// The first sample of a prediction is moved to where the played sound was heading, and the
    /// move is let go of over the first few samples. The predictor knows the shape of the sound,
    /// not the exact level it was at.
    fn meet_last_played(&mut self, out: &mut [f32]) {
        let ch = self.channels;
        let available = self.history.len;
        if available < 2 {
            return;
        }
        let onset = self.lengths.onset.min(out.len() / ch);
        for c in 0..ch {
            let h = self.history.channel(c);
            let heading = 2.0 * h[available - 1] - h[available - 2];
            let step = heading - out[c];
            for i in 0..onset {
                out[i * ch + c] += step * (1.0 - i as f32 / onset as f32);
            }
        }
    }

    /// Cross-fade into returning audio: runs the extrapolation on over the cross-fade, into
    /// the tail.
    fn start_tail_from_synth(&mut self) {
        let span = self.lengths.rejoin_min as f32
            + (self.lengths.rejoin_max - self.lengths.rejoin_min) as f32
                * self.synth.periodicity.clamp(0.0, 1.0);
        let span = (span as usize).min(self.tail[0].len());
        self.generate(None, span);
        self.tail_pos = 0;
        self.tail_len = span;
    }

    /// Produces `frames` more samples of the extrapolation, into `out` (interleaved) or into
    /// the tail. Returns whether it is still audible after them.
    fn generate(&mut self, mut out: Option<&mut [f32]>, frames: usize) -> bool {
        let ch = self.channels;
        let synth = &mut self.synth;
        let keep = signal_keep(&self.lengths);
        let produced_before = synth.produced;
        let mut diverged = false;
        let into_tail = out.is_none();
        for i in 0..frames {
            if synth.signal_len == synth.signal[0].len() {
                let from = synth.signal_len - keep;
                for buffer in synth.signal.iter_mut().chain(synth.repeat.iter_mut()) {
                    buffer.copy_within(from..synth.signal_len, 0);
                }
                synth.signal_len = keep;
            }
            let n = synth.signal_len;
            let level = synth.envelope(produced_before + i);
            for c in 0..ch {
                let s = &synth.signal[c];
                let a = &synth.lpc[c];
                let mut predicted = 0.0f32;
                for k in 1..=LPC_ORDER {
                    predicted -= a[k] * s[n - k];
                }
                synth.signal[c][n] = predicted;
                let mut sample = predicted + PREEMPHASIS * synth.last_out[c];
                synth.last_out[c] = sample;
                if synth.period > 0.0 {
                    let r = &synth.repeat[c];
                    let position = n as f32 - synth.period;
                    let index = position.floor();
                    let fraction = position - index;
                    let index = index as usize;
                    let repeated = r[index] * (1.0 - fraction) + r[index + 1] * fraction;
                    synth.repeat[c][n] = repeated;
                    sample = sample * (1.0 - synth.pitch_weight) + repeated * synth.pitch_weight;
                }
                if sample.is_nan() || sample.abs() > DIVERGED_ABOVE {
                    diverged = true;
                }
                let sample = if diverged { 0.0 } else { sample * level };
                match out.as_deref_mut() {
                    Some(out) => out[i * ch + c] = sample,
                    None => self.tail[c][i] = sample,
                }
            }
            synth.signal_len += 1;
            if diverged {
                // Whatever is left of this call is silence
                for rest in i + 1..frames {
                    for c in 0..ch {
                        match out.as_deref_mut() {
                            Some(out) => out[rest * ch + c] = 0.0,
                            None => self.tail[c][rest] = 0.0,
                        }
                    }
                }
                return false;
            }
        }
        // A tail runs on from where the audio stopped, so it does not move the extrapolation
        // forward for a next read: the extrapolation is over once it has been used.
        if !into_tail {
            synth.produced = produced_before + frames;
        }
        synth.envelope(produced_before + frames) > 0.0
    }

    /// Analyses the past and sets an extrapolation up. False when there is nothing to
    /// continue: too little heard, or silence.
    fn start_synth(&mut self) -> bool {
        let ch = self.channels;
        let l = self.lengths;
        let available = self.history.len;
        let keep = signal_keep(&l);
        if available < l.lpc_window + 1 || available < keep + 1 {
            return false;
        }

        for c in 0..ch {
            let h = self.history.channel(c);
            let window = &mut self.scratch.window[..l.lpc_window];
            if !fit_lpc(
                h,
                window,
                PREEMPHASIS,
                BANDWIDTH_EXPANSION,
                &mut self.synth.lpc[c],
            ) {
                return false;
            }
        }
        let (period, correlation) = self.analyse_pitch().unwrap_or((0.0, 0.0));

        let synth = &mut self.synth;
        synth.period = period;
        synth.periodicity = correlation;
        let mix =
            ((correlation - PITCH_MIX_START) / (PITCH_MIX_FULL - PITCH_MIX_START)).clamp(0.0, 1.0);
        synth.pitch_weight = mix * (correlation * 0.7).min(PITCH_MAX_WEIGHT);
        if synth.pitch_weight <= 0.0 {
            synth.period = 0.0;
        }
        synth.produced = 0;
        let to_samples = |ms: f32| (ms * self.rate as f32 / 1000.0) as usize;
        synth.fade_start = to_samples(HOLD_MS);
        synth.fade_len =
            to_samples(FADE_MIN_MS + (FADE_MAX_MS - FADE_MIN_MS) * correlation.clamp(0.0, 1.0))
                .max(1);

        // The past, pre-emphasised, is what the predictor and the repeat run on
        synth.signal_len = keep;
        for c in 0..ch {
            let h = self.history.channel(c);
            let tail = &h[available - keep - 1..];
            for (i, slot) in synth.signal[c][..keep].iter_mut().enumerate() {
                *slot = tail[i + 1] - PREEMPHASIS * tail[i];
            }
            synth.last_out[c] = h[available - 1];
            synth.repeat[c][..keep].copy_from_slice(&h[available - keep..]);
        }
        synth.state = SynthState::Running;
        true
    }

    /// Finds the pitch period of the last 38 ms (of the mix of the channels), as a fractional
    /// number of samples, with its correlation.
    fn analyse_pitch(&mut self) -> Option<(f32, f32)> {
        let l = self.lengths;
        let available = self.history.len;
        let window = l.pitch_window.min(available / 2);
        let max_period = l.pitch_max_period.min(available - window);
        if max_period < l.pitch_min_period + 2 || window < l.pitch_min_period * 2 {
            return None;
        }
        let total = window + max_period;
        let scale = 1.0 / self.channels as f32;
        let mono = &mut self.scratch.mono[..total];
        mono.fill(0.0);
        for c in 0..self.channels {
            let h = self.history.channel(c);
            for (slot, v) in mono.iter_mut().zip(&h[available - total..]) {
                *slot += v * scale;
            }
        }
        let energy = &mut self.scratch.energy[..total + 1];
        energy[0] = 0.0;
        for i in 0..total {
            energy[i + 1] = energy[i] + f64::from(mono[i]) * f64::from(mono[i]);
        }
        let mono = &self.scratch.mono[..total];
        let energy = &self.scratch.energy[..total + 1];
        let recent = &mono[total - window..];
        let recent_energy = (energy[total] - energy[total - window]) as f32;
        if recent_energy < 1e-9 {
            return None;
        }
        // Correlation of the last `window` samples with the same stretch `period` earlier
        let correlation = |period: usize| -> f32 {
            let from = total - window - period;
            let earlier = &mono[from..from + window];
            let earlier_energy = (energy[from + window] - energy[from]) as f32;
            dot(recent, earlier) / (recent_energy * earlier_energy + 1e-20).sqrt()
        };

        // Coarsely first
        let mut best = (0usize, 0.0f32);
        let mut period = l.pitch_min_period;
        while period <= max_period {
            let value = correlation(period);
            if value > best.1 {
                best = (period, value);
            }
            period += l.pitch_coarse_step;
        }
        if best.0 == 0 || best.1 < PITCH_COARSE_MIN_CORRELATION {
            return None;
        }

        // Then the best of each sub-multiple of that period, looked for closely
        let refine = |centre: usize| -> (usize, f32) {
            let low = centre
                .saturating_sub(l.pitch_coarse_step)
                .max(l.pitch_min_period);
            let high = (centre + l.pitch_coarse_step).min(max_period);
            let mut local = (0usize, f32::MIN);
            for p in low..=high {
                let value = correlation(p);
                if value > local.1 {
                    local = (p, value);
                }
            }
            local
        };
        let mut candidates = [(0usize, 0.0f32); PITCH_MAX_DIVISOR];
        let mut strongest = 0.0f32;
        for divisor in 1..=PITCH_MAX_DIVISOR {
            let centre = (best.0 + divisor / 2) / divisor;
            if centre < l.pitch_min_period {
                break;
            }
            let found = refine(centre);
            candidates[divisor - 1] = found;
            strongest = strongest.max(found.1);
        }
        // The shortest period that is nearly as good as the best
        let mut chosen = (0usize, 0.0f32);
        for &(p, value) in candidates.iter() {
            if p != 0
                && value >= PITCH_SUBMULTIPLE_KEEPS * strongest
                && (chosen.0 == 0 || p < chosen.0)
            {
                chosen = (p, value);
            }
        }
        if chosen.0 == 0 || chosen.1 < PITCH_MIN_CORRELATION {
            return None;
        }

        // A parabola through the peak and its neighbours gives the period between samples
        let (p, peak) = chosen;
        let mut fractional = p as f32;
        if p > l.pitch_min_period && p < max_period {
            let (left, right) = (correlation(p - 1), correlation(p + 1));
            let curve = left - 2.0 * peak + right;
            if curve < 0.0 {
                fractional += (0.5 * (left - right) / curve).clamp(-1.0, 1.0);
            }
        }
        Some((fractional, peak.min(1.0)))
    }
}
