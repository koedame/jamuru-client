//! Measures the delay of the audio device pair the session runs on: from the
//! moment the app hands a sound to the output until the input has taken it in.
//!
//! The user cables the output to the input, or puts the speaker in front of
//! the microphone, and [`start`] plays a measuring sound from the output
//! callback and watches the input callback for it. The sound is made in the
//! output callback itself and the input is read into a ring, so what is
//! measured is the device and its driver, not the app's queues. The analysis
//! runs on a thread of its own: the callbacks only write and never wait.
//!
//! Where the sound arrives is decided in samples. The input callback stamps
//! the time it is called with, which is when the last sample of its buffer was
//! taken in, so a sample's time is that stamp less the samples after it in the
//! buffer. The wait for the rest of the buffer is not part of the figure.
//!
//! While a run is under way the output carries only the measuring sound and
//! the capture path is fed silence, so the room neither hears the sound nor
//! sends it on.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The two measuring sounds. The clicks come first; the tone is for a path
/// that passes too little of a click to hear (a speaker and a microphone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stimulus {
    /// One-sample pulses 1 ms apart for 30 ms.
    Clicks,
    /// A 1 kHz sine for 50 ms, heard through a band-pass filter.
    Tone,
}

impl Stimulus {
    pub fn name(self) -> &'static str {
        match self {
            Stimulus::Clicks => "clicks",
            Stimulus::Tone => "tone",
        }
    }
}

/// The volume steps, in dB below full scale. Each is double the one before.
pub const LEVELS_DB: [f32; 8] = [-48.0, -42.0, -36.0, -30.0, -24.0, -18.0, -12.0, -6.0];

/// How far above the noise a sound must rise to count, per step. The last
/// steps are louder but the path may be quiet, so they ask for less.
pub const MARGIN_DB: [f32; 8] = [20.0, 20.0, 20.0, 20.0, 20.0, 12.0, 9.0, 6.0];

/// Times the sound is played at each volume.
pub const REPEATS: usize = 5;

/// The quiet before the sound that sets the noise.
pub const NOISE_WINDOW: Duration = Duration::from_millis(300);

/// How long a sound is waited for.
pub const WAIT: Duration = Duration::from_millis(750);

/// The whole run is given up after this.
pub const TOTAL: Duration = Duration::from_secs(90);

/// A sound heard sooner than this has not been through the path: it is the
/// device leaking the output into the input.
pub const CROSSTALK: Duration = Duration::from_millis(2);

/// Results needed out of [`REPEATS`] for a figure.
pub const MIN_HITS: usize = 3;

/// The widest scatter, in ms, a figure is allowed to have.
pub const MAX_RANGE_MS: f64 = 25.0;

/// The longest delay, in ms, that is taken as a figure.
pub const MAX_DELAY_MS: f64 = 500.0;

/// A scatter up to this is shown as steady, in ms.
pub const STEADY_MS: f64 = 2.0;

/// A scatter up to this is shown as slightly shaky, in ms. Beyond it, shaky.
pub const SLIGHTLY_SHAKY_MS: f64 = 6.0;

/// A delay above this, in ms, is worth a second look at the settings.
pub const REVIEW_ABOVE_MS: f64 = 100.0;

/// Millisecond-wide outlier rule: a result further from the median than
/// `OUTLIER_SIGMAS` standard deviations, with the deviation taken from the
/// median absolute deviation (x 1.4826), and at least `OUTLIER_FLOOR_MS`.
const OUTLIER_SIGMAS: f64 = 3.0;
const MAD_TO_SIGMA: f64 = 1.4826;
const OUTLIER_FLOOR_MS: f64 = 3.0;

/// Noise below this (about -80 dBFS) is taken as this, so a path with digital
/// silence does not trigger on its own rounding.
const NOISE_MIN: f32 = 1e-4;

/// The tone's frequency and the band-pass filter's quality factor.
const TONE_HZ: f64 = 1000.0;
const FILTER_Q: f64 = 2.0;

const CLICK_COUNT: usize = 30;
const TONE_LEN: Duration = Duration::from_millis(50);
const TONE_FADE: Duration = Duration::from_millis(1);

/// Where a rise is traced back to: not more than this far before the crossing.
const BACKTRACK: Duration = Duration::from_millis(5);

/// The filter is run this long before the window it is read in, to settle.
const FILTER_SETTLE: Duration = Duration::from_millis(30);

/// The poll of the analysis thread.
const POLL: Duration = Duration::from_millis(5);

/// How long the output callback is given to start a sound.
const OUTPUT_START_WAIT: Duration = Duration::from_millis(1500);

const RING_FRAMES: usize = 1 << 19;
const ANCHORS: usize = 1 << 13;

// ---------------------------------------------------------------------
// What the callbacks and the analysis thread share

static ARMED: AtomicBool = AtomicBool::new(false);

/// The meter is one per process: tests that run it, or that play sound
/// through the callbacks it hooks, take turns.
#[cfg(test)]
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}
static SHARED: OnceLock<Shared> = OnceLock::new();
static BASE: OnceLock<Instant> = OnceLock::new();

fn base() -> Instant {
    *BASE.get_or_init(Instant::now)
}

fn nanos_of(at: Instant) -> u64 {
    at.saturating_duration_since(base()).as_nanos() as u64
}

fn instant_of(nanos: u64) -> Instant {
    base() + Duration::from_nanos(nanos)
}

struct Shared {
    // Output side. The play_* fields belong to the output callback.
    request_gen: AtomicU64,
    request_stimulus: AtomicU32,
    request_level: AtomicU32,
    started_gen: AtomicU64,
    started_nanos: AtomicU64,
    out_rate: AtomicU32,
    out_callbacks: AtomicU64,
    play_pos: AtomicU64,
    play_len: AtomicU64,
    play_stimulus: AtomicU32,
    play_level: AtomicU32,
    // Input side
    ring: Box<[AtomicU32]>,
    written: AtomicU64,
    in_rate: AtomicU32,
    anchor_end: Box<[AtomicU64]>,
    anchor_nanos: Box<[AtomicU64]>,
    anchor_head: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        fn zeros32(n: usize) -> Box<[AtomicU32]> {
            (0..n).map(|_| AtomicU32::new(0)).collect()
        }
        fn zeros64(n: usize) -> Box<[AtomicU64]> {
            (0..n).map(|_| AtomicU64::new(0)).collect()
        }
        Self {
            request_gen: AtomicU64::new(0),
            request_stimulus: AtomicU32::new(0),
            request_level: AtomicU32::new(0),
            started_gen: AtomicU64::new(0),
            started_nanos: AtomicU64::new(0),
            out_rate: AtomicU32::new(0),
            out_callbacks: AtomicU64::new(0),
            play_pos: AtomicU64::new(0),
            play_len: AtomicU64::new(0),
            play_stimulus: AtomicU32::new(0),
            play_level: AtomicU32::new(0),
            ring: zeros32(RING_FRAMES),
            written: AtomicU64::new(0),
            in_rate: AtomicU32::new(0),
            anchor_end: zeros64(ANCHORS),
            anchor_nanos: zeros64(ANCHORS),
            anchor_head: AtomicU64::new(0),
        }
    }

    /// Forgets the last run. Called with the callbacks disarmed.
    fn reset(&self) {
        self.started_gen.store(0, Ordering::Relaxed);
        self.request_gen.store(0, Ordering::Relaxed);
        self.play_pos.store(0, Ordering::Relaxed);
        self.play_len.store(0, Ordering::Relaxed);
        self.out_callbacks.store(0, Ordering::Relaxed);
        self.written.store(0, Ordering::Relaxed);
        self.anchor_head.store(0, Ordering::Relaxed);
    }
}

fn stimulus_of(code: u32) -> Stimulus {
    if code == 0 {
        Stimulus::Clicks
    } else {
        Stimulus::Tone
    }
}

fn frames_of(duration: Duration, rate: u32) -> usize {
    (duration.as_secs_f64() * f64::from(rate)).round() as usize
}

fn click_spacing(rate: u32) -> usize {
    (rate as usize / 1000).max(1)
}

fn stimulus_frames(stimulus: Stimulus, rate: u32) -> usize {
    match stimulus {
        Stimulus::Clicks => CLICK_COUNT * click_spacing(rate),
        Stimulus::Tone => frames_of(TONE_LEN, rate),
    }
}

/// The sample `pos` frames into a measuring sound, before the volume.
fn stimulus_sample(stimulus: Stimulus, pos: usize, len: usize, rate: u32) -> f32 {
    match stimulus {
        Stimulus::Clicks => {
            if pos.is_multiple_of(click_spacing(rate)) {
                1.0
            } else {
                0.0
            }
        }
        Stimulus::Tone => {
            let phase = 2.0 * std::f64::consts::PI * TONE_HZ * pos as f64 / f64::from(rate);
            let fade = frames_of(TONE_FADE, rate).max(1);
            let left = len.saturating_sub(pos);
            let envelope = if left < fade {
                left as f32 / fade as f32
            } else {
                1.0
            };
            phase.sin() as f32 * envelope
        }
    }
}

/// Called with the stereo the output callback is about to hand to the device.
/// While a run is under way it is replaced by silence and the measuring sound.
pub(crate) fn on_output(stereo: &mut [f32], sample_rate: u32) {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    output_at(stereo, sample_rate, Instant::now());
}

/// Called with what the input callback got from the device. True while a run
/// is under way: what the callback delivers to the session is then silence.
pub(crate) fn on_input(data: &[f32], device_channels: usize, sample_rate: u32) -> bool {
    if !ARMED.load(Ordering::Acquire) {
        return false;
    }
    input_at(data, device_channels, sample_rate, Instant::now());
    true
}

fn output_at(stereo: &mut [f32], rate: u32, now: Instant) {
    let Some(shared) = SHARED.get() else {
        return;
    };
    shared.out_rate.store(rate, Ordering::Relaxed);
    shared.out_callbacks.fetch_add(1, Ordering::Relaxed);
    let wanted = shared.request_gen.load(Ordering::Acquire);
    let mut pos = shared.play_pos.load(Ordering::Relaxed);
    let mut len = shared.play_len.load(Ordering::Relaxed);
    if wanted != shared.started_gen.load(Ordering::Relaxed) && pos >= len {
        let stimulus = shared.request_stimulus.load(Ordering::Relaxed);
        shared.play_stimulus.store(stimulus, Ordering::Relaxed);
        shared.play_level.store(
            shared.request_level.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        pos = 0;
        len = stimulus_frames(stimulus_of(stimulus), rate) as u64;
        shared.play_len.store(len, Ordering::Relaxed);
        shared.started_nanos.store(nanos_of(now), Ordering::Relaxed);
        shared.started_gen.store(wanted, Ordering::Release);
    }
    let stimulus = stimulus_of(shared.play_stimulus.load(Ordering::Relaxed));
    let level = f32::from_bits(shared.play_level.load(Ordering::Relaxed));
    for frame in stereo.as_chunks_mut::<2>().0.iter_mut() {
        let value = if pos < len {
            let sample = stimulus_sample(stimulus, pos as usize, len as usize, rate) * level;
            pos += 1;
            sample
        } else {
            0.0
        };
        frame.fill(value);
    }
    shared.play_pos.store(pos, Ordering::Relaxed);
}

fn input_at(data: &[f32], channels: usize, rate: u32, now: Instant) {
    let Some(shared) = SHARED.get() else {
        return;
    };
    let channels = channels.max(1);
    shared.in_rate.store(rate, Ordering::Relaxed);
    let mut at = shared.written.load(Ordering::Relaxed);
    for frame in data.chunks_exact(channels) {
        // The channel that carries the sound is the loudest one
        let sample =
            frame.iter().copied().fold(
                0.0f32,
                |kept, s| if s.abs() > kept.abs() { s } else { kept },
            );
        shared.ring[(at as usize) & (RING_FRAMES - 1)].store(sample.to_bits(), Ordering::Relaxed);
        at += 1;
    }
    let head = shared.anchor_head.load(Ordering::Relaxed);
    let slot = (head as usize) & (ANCHORS - 1);
    shared.anchor_nanos[slot].store(nanos_of(now), Ordering::Relaxed);
    shared.anchor_end[slot].store(at, Ordering::Relaxed);
    shared.anchor_head.store(head + 1, Ordering::Release);
    shared.written.store(at, Ordering::Release);
}

// ---------------------------------------------------------------------
// Signal handling

/// A band-pass filter (constant 0 dB peak gain) around the tone.
struct BandPass {
    b0: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl BandPass {
    fn new(rate: u32) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * TONE_HZ / f64::from(rate);
        let alpha = w0.sin() / (2.0 * FILTER_Q);
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            a1: -2.0 * w0.cos() / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        // b1 = 0 and b2 = -b0
        let y = self.b0 * (x - self.x2) - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y as f32
    }
}

/// The signal the sound is looked for in, made as the input arrives: the
/// input as it is for clicks, the input through the band-pass filter for the tone.
struct Scan {
    filter: Option<BandPass>,
    signal: Vec<f32>,
}

impl Scan {
    fn new(stimulus: Stimulus, rate: u32) -> Self {
        Self {
            filter: match stimulus {
                Stimulus::Clicks => None,
                Stimulus::Tone => Some(BandPass::new(rate)),
            },
            signal: Vec::new(),
        }
    }

    fn feed(&mut self, raw: impl Iterator<Item = f32>) {
        match self.filter.as_mut() {
            None => self.signal.extend(raw),
            Some(filter) => self.signal.extend(raw.map(|x| filter.process(x))),
        }
    }
}

/// The amplitude 90 % of the samples stay under.
fn noise_of(window: &[f32]) -> f32 {
    if window.is_empty() {
        return NOISE_MIN;
    }
    let mut amplitudes: Vec<f32> = window.iter().map(|s| s.abs()).collect();
    let index = ((amplitudes.len() - 1) as f64 * 0.9).round() as usize;
    let (_, p90, _) = amplitudes.select_nth_unstable_by(index, f32::total_cmp);
    (*p90).max(NOISE_MIN)
}

/// How a search for the sound ended.
#[derive(Debug, PartialEq, Eq)]
enum Seek {
    /// The sound starts at this sample.
    Found(usize),
    /// Not yet: look again from this sample when there is more signal.
    Pending(usize),
}

/// The signal a crossing has to go on for to be the sound and not a spike of
/// noise.
const PERSISTENCE: Duration = Duration::from_millis(10);

/// Where the sound starts in `signal`, looking from `from`: a sample over
/// `noise` raised by `margin_db`, that the sound goes on from (three samples
/// over it within the next 10 ms for clicks, one 6 to 10 ms on for the tone),
/// traced back to where the rise began.
fn seek_arrival(
    signal: &[f32],
    from: usize,
    noise: f32,
    margin_db: f32,
    rate: u32,
    stimulus: Stimulus,
) -> Seek {
    let threshold = noise * 10f32.powf(margin_db / 20.0);
    let span = frames_of(PERSISTENCE, rate);
    let backtrack = frames_of(BACKTRACK, rate);
    let mut at = from;
    while let Some(offset) = signal
        .get(at..)
        .and_then(|rest| rest.iter().position(|s| s.abs() > threshold))
    {
        let crossing = at + offset;
        if crossing + span > signal.len() {
            return Seek::Pending(crossing);
        }
        let ahead = &signal[crossing..crossing + span];
        let goes_on = match stimulus {
            Stimulus::Clicks => ahead.iter().filter(|s| s.abs() > threshold).count() >= 3,
            Stimulus::Tone => ahead[span * 6 / 10..].iter().any(|s| s.abs() > threshold),
        };
        if goes_on {
            let floor = noise * 2.0;
            let earliest = from.max(crossing.saturating_sub(backtrack));
            let mut start = crossing;
            while start > earliest && signal[start - 1].abs() > floor {
                start -= 1;
            }
            return Seek::Found(start);
        }
        at = crossing + 1;
    }
    Seek::Pending(signal.len().max(from))
}

fn median_of(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

/// How steady the figure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    Steady,
    SlightlyShaky,
    Shaky,
}

impl Stability {
    pub fn of_spread(spread_ms: f64) -> Self {
        if spread_ms <= STEADY_MS {
            Stability::Steady
        } else if spread_ms <= SLIGHTLY_SHAKY_MS {
            Stability::SlightlyShaky
        } else {
            Stability::Shaky
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Stability::Steady => "steady",
            Stability::SlightlyShaky => "slightly_shaky",
            Stability::Shaky => "shaky",
        }
    }
}

/// What a run found.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// The median of the results kept, in ms.
    pub delay_ms: f64,
    /// The largest minus the smallest result kept, in ms.
    pub spread_ms: f64,
    pub stability: Stability,
    /// Results kept out of those heard.
    pub kept: usize,
    pub heard: usize,
    pub stimulus: Stimulus,
    /// The volume of the step it was measured at, dB below full scale.
    pub level_db: f32,
}

impl Measurement {
    /// A delay this long is worth a second look at the device settings.
    pub fn needs_review(&self) -> bool {
        self.delay_ms > REVIEW_ABOVE_MS
    }
}

/// Why a run gave no figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The sound never came back loud enough.
    NoSound,
    /// It came back, but at times too far apart.
    Unsteady,
    /// It came back after more than [`MAX_DELAY_MS`].
    TooLong,
    /// The output or the input is not running.
    NoStream,
    /// The 90 seconds ran out.
    TimedOut,
    Cancelled,
}

impl Failure {
    pub fn name(self) -> &'static str {
        match self {
            Failure::NoSound => "no_sound",
            Failure::Unsteady => "unsteady",
            Failure::TooLong => "too_long",
            Failure::NoStream => "no_stream",
            Failure::TimedOut => "timed_out",
            Failure::Cancelled => "cancelled",
        }
    }
}

/// The figure out of the results of one step: the outliers go, and what is
/// left must be enough, close enough and short enough.
pub fn evaluate(delays_ms: &[f64]) -> Result<(f64, f64, usize), Failure> {
    if delays_ms.len() < MIN_HITS {
        return Err(Failure::NoSound);
    }
    let all = sorted(delays_ms);
    let median = median_of(&all);
    let deviations = sorted(&all.iter().map(|d| (d - median).abs()).collect::<Vec<_>>());
    let limit = (OUTLIER_SIGMAS * MAD_TO_SIGMA * median_of(&deviations)).max(OUTLIER_FLOOR_MS);
    let kept: Vec<f64> = all
        .iter()
        .copied()
        .filter(|d| (d - median).abs() <= limit)
        .collect();
    if kept.len() < MIN_HITS {
        return Err(Failure::Unsteady);
    }
    let spread = kept[kept.len() - 1] - kept[0];
    if spread > MAX_RANGE_MS {
        return Err(Failure::Unsteady);
    }
    let figure = median_of(&kept);
    if figure > MAX_DELAY_MS {
        return Err(Failure::TooLong);
    }
    Ok((figure, spread, kept.len()))
}

// ---------------------------------------------------------------------
// The run

/// Time for the analysis thread, so a test can run a whole run in virtual time.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub stimulus: Stimulus,
    /// 1-based volume step.
    pub step: usize,
    pub steps: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Running(Step),
    Done(Result<Measurement, Failure>),
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static CANCEL: AtomicBool = AtomicBool::new(false);

fn set_state(state: State) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

/// Where the run is now.
pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Asks the run under way to stop.
pub fn cancel() {
    CANCEL.store(true, Ordering::Release);
}

/// Starts a run on a thread of its own, which calls `on_done` with how it
/// ended. Fails when one is under way.
pub fn start(
    on_done: impl FnOnce(&Result<Measurement, Failure>) + Send + 'static,
) -> Result<(), &'static str> {
    begin(std::sync::Arc::new(RealClock), true, Box::new(on_done))
}

type Done = Box<dyn FnOnce(&Result<Measurement, Failure>) + Send>;

fn begin(clock: std::sync::Arc<dyn Clock>, spawn: bool, on_done: Done) -> Result<(), &'static str> {
    {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*state, State::Running(_)) {
            return Err("a delay measurement is already under way");
        }
        *state = State::Running(Step {
            stimulus: Stimulus::Clicks,
            step: 1,
            steps: LEVELS_DB.len(),
        });
    }
    CANCEL.store(false, Ordering::Release);
    let shared = SHARED.get_or_init(Shared::new);
    ARMED.store(false, Ordering::Release);
    shared.reset();
    ARMED.store(true, Ordering::Release);
    let work = move || {
        let result = run(&*clock, shared);
        ARMED.store(false, Ordering::Release);
        set_state(State::Done(result.clone()));
        on_done(&result);
    };
    if spawn {
        std::thread::Builder::new()
            .name("delay-meter".into())
            .spawn(work)
            .map_err(|_| "could not start the delay measurement")?;
    } else {
        work();
    }
    Ok(())
}

/// A snapshot of the stamps of the input callbacks that delivered frames
/// from `frame` on, oldest first.
fn anchors_since(shared: &Shared, frame: u64) -> Vec<(u64, u64)> {
    let head = shared.anchor_head.load(Ordering::Acquire);
    let oldest = head.saturating_sub(ANCHORS as u64);
    let mut table = Vec::new();
    for n in (oldest..head).rev() {
        let slot = (n as usize) & (ANCHORS - 1);
        let end = shared.anchor_end[slot].load(Ordering::Relaxed);
        table.push((end, shared.anchor_nanos[slot].load(Ordering::Relaxed)));
        if end <= frame {
            break;
        }
    }
    table.reverse();
    table
}

/// The time, in ns on the shared base, the input took in frame `frame`.
fn time_of_frame(anchors: &[(u64, u64)], rate: u32, frame: u64) -> Option<i64> {
    let index = anchors.partition_point(|&(end, _)| end <= frame);
    let &(end, nanos) = anchors.get(index)?;
    let after = (end - 1 - frame) as f64 * 1e9 / f64::from(rate);
    Some(nanos as i64 - after.round() as i64)
}

fn read_ring(shared: &Shared, from: u64, to: u64) -> impl Iterator<Item = f32> + '_ {
    (from..to).map(|n| {
        f32::from_bits(shared.ring[(n as usize) & (RING_FRAMES - 1)].load(Ordering::Relaxed))
    })
}

enum Trial {
    Heard(f64),
    Missed,
    Leak,
}

struct Context<'a> {
    clock: &'a dyn Clock,
    shared: &'a Shared,
    began: Instant,
}

impl Context<'_> {
    fn check(&self) -> Result<(), Failure> {
        if CANCEL.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        if self.clock.now().saturating_duration_since(self.began) > TOTAL {
            return Err(Failure::TimedOut);
        }
        Ok(())
    }

    fn pause(&self, duration: Duration) -> Result<(), Failure> {
        let until = self.clock.now() + duration;
        while self.clock.now() < until {
            self.check()?;
            self.clock
                .sleep(POLL.min(until.saturating_duration_since(self.clock.now())));
        }
        Ok(())
    }

    /// Plays the sound once and says when it came back.
    fn trial(
        &self,
        stimulus: Stimulus,
        level_db: f32,
        margin_db: f32,
    ) -> Result<(Trial, Duration), Failure> {
        let shared = self.shared;
        // Both streams must be calling back before anything can be measured
        let waiting_since = self.clock.now();
        while shared.in_rate.load(Ordering::Relaxed) == 0
            || shared.out_callbacks.load(Ordering::Relaxed) == 0
        {
            self.check()?;
            if self.clock.now().saturating_duration_since(waiting_since) > OUTPUT_START_WAIT {
                return Err(Failure::NoStream);
            }
            self.clock.sleep(POLL);
        }
        let rate = shared.in_rate.load(Ordering::Relaxed);
        let noise_frames = frames_of(NOISE_WINDOW, rate) as u64;
        let settle = frames_of(FILTER_SETTLE, rate) as u64;
        // Wait for the quiet the noise is taken from
        while shared.written.load(Ordering::Acquire) < noise_frames + settle {
            self.check()?;
            if self.clock.now().saturating_duration_since(waiting_since) > OUTPUT_START_WAIT {
                return Err(Failure::NoStream);
            }
            self.clock.sleep(POLL);
        }
        let noise_end = shared.written.load(Ordering::Acquire);
        let noise_start = noise_end - noise_frames;
        let window_start = noise_start - settle;

        let generation = shared.request_gen.load(Ordering::Relaxed) + 1;
        shared.request_stimulus.store(
            match stimulus {
                Stimulus::Clicks => 0,
                Stimulus::Tone => 1,
            },
            Ordering::Relaxed,
        );
        shared
            .request_level
            .store(10f32.powf(level_db / 20.0).to_bits(), Ordering::Relaxed);
        shared.request_gen.store(generation, Ordering::Release);
        let requested = self.clock.now();
        while shared.started_gen.load(Ordering::Acquire) != generation {
            self.check()?;
            if self.clock.now().saturating_duration_since(requested) > OUTPUT_START_WAIT {
                return Err(Failure::NoStream);
            }
            self.clock.sleep(POLL);
        }
        let started_nanos = shared.started_nanos.load(Ordering::Relaxed);
        let started = instant_of(started_nanos);
        let sound_len = Duration::from_secs_f64(
            stimulus_frames(stimulus, shared.out_rate.load(Ordering::Relaxed).max(1)) as f64
                / f64::from(shared.out_rate.load(Ordering::Relaxed).max(1)),
        );

        let mut scan = Scan::new(stimulus, rate);
        let mut noise = None;
        let mut first: Option<usize> = None;
        let mut resume = 0;
        let mut verdict = Trial::Missed;
        loop {
            let end = shared.written.load(Ordering::Acquire);
            let have = window_start + scan.signal.len() as u64;
            scan.feed(read_ring(shared, have, end));
            let noise = *noise.get_or_insert_with(|| {
                noise_of(&scan.signal[settle as usize..(settle + noise_frames) as usize])
            });
            let table = anchors_since(shared, window_start);
            if first.is_none() {
                // The first frame the sound can be in
                let (mut low, mut high) = (noise_end, end);
                while low < high {
                    let middle = low + (high - low) / 2;
                    let late = time_of_frame(&table, rate, middle)
                        .is_some_and(|t| t >= started_nanos as i64);
                    if late {
                        high = middle;
                    } else {
                        low = middle + 1;
                    }
                }
                if low < end {
                    first = Some((low - window_start) as usize);
                    resume = (low - window_start) as usize;
                }
            }
            if let Some(from) = first {
                match seek_arrival(&scan.signal, resume, noise, margin_db, rate, stimulus) {
                    Seek::Found(at) => {
                        let frame = window_start + at as u64;
                        let t = time_of_frame(&table, rate, frame).unwrap_or(started_nanos as i64);
                        let delay = Duration::from_nanos((t - started_nanos as i64).max(0) as u64);
                        verdict = if delay < CROSSTALK {
                            Trial::Leak
                        } else {
                            Trial::Heard(delay.as_secs_f64() * 1000.0)
                        };
                        break;
                    }
                    Seek::Pending(next) => resume = next.max(from),
                }
            }
            if self.clock.now().saturating_duration_since(started) >= WAIT {
                break;
            }
            self.check()?;
            self.clock.sleep(POLL);
        }
        let waited = match verdict {
            Trial::Heard(ms) => Duration::from_secs_f64(ms / 1000.0),
            _ => WAIT,
        };
        // Let the sound and its echo die out before the next quiet is taken
        self.pause(
            sound_len
                + NOISE_WINDOW
                + waited.saturating_sub(self.clock.now().saturating_duration_since(started)),
        )?;
        Ok((verdict, waited))
    }
}

fn run(clock: &dyn Clock, shared: &Shared) -> Result<Measurement, Failure> {
    let context = Context {
        clock,
        shared,
        began: clock.now(),
    };
    let mut failure = Failure::NoSound;
    for stimulus in [Stimulus::Clicks, Stimulus::Tone] {
        for (index, (&level, &margin)) in LEVELS_DB.iter().zip(MARGIN_DB.iter()).enumerate() {
            set_state(State::Running(Step {
                stimulus,
                step: index + 1,
                steps: LEVELS_DB.len(),
            }));
            let mut delays = Vec::new();
            let mut misses = 0;
            for _ in 0..REPEATS {
                context.check()?;
                match context.trial(stimulus, level, margin)?.0 {
                    Trial::Heard(ms) => delays.push(ms),
                    Trial::Missed | Trial::Leak => misses += 1,
                }
                if REPEATS - misses < MIN_HITS {
                    break;
                }
            }
            match evaluate(&delays) {
                Ok((delay_ms, spread_ms, kept)) => {
                    return Ok(Measurement {
                        delay_ms,
                        spread_ms,
                        stability: Stability::of_spread(spread_ms),
                        kept,
                        heard: delays.len(),
                        stimulus,
                        level_db: level,
                    })
                }
                Err(Failure::NoSound) => {}
                Err(other) => failure = other,
            }
        }
    }
    Err(failure)
}

#[cfg(test)]
mod tests;
