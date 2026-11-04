//! Measures how well gaps that FEC could not repair are filled (ADR-105): the sound filled in is
//! compared with the sound that was lost, and the old way (the last frame repeated and faded,
//! silence while nothing has arrived) is run next to it on the same losses. Four kinds of sound
//! are made here from formulas, so there is no recording to license: a plucked string, a sung
//! vowel, a chord and drums, and a sustained bowed note.
//!
//! The runs go through [`ReceivePath`], the way received audio reaches the speakers, so the
//! cross-fades between a fill and the audio around it are part of what is measured.
//!
//! ```text
//! cargo test --release --test conceal_measure -- --ignored --nocapture
//! ```
//!
//! Listening copies are written to `$JAMUMU_CONCEAL_OUT` (default `target/conceal-wavs`).

use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::Instant;

use jamjam::audio::{
    AudioCodec, CodecConfig, CodecType, Concealer, PcmCodec, PlayoutResult, ReceivePath,
    WIRE_CHANNELS,
};

const RATE: u32 = 48_000;
const FRAME: usize = 64;
const SAMPLES_PER_FRAME: usize = FRAME * WIRE_CHANNELS;
const SECONDS: usize = 6;
/// The stall measurement is run over this many different stall patterns.
const STALL_SEEDS: u64 = 12;

// --- Sounds made from formulas -----------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn signed(&mut self) -> f64 {
        self.next() * 2.0 - 1.0
    }
}

fn frequency(semitones_from_a4: f64) -> f64 {
    440.0 * 2f64.powf(semitones_from_a4 / 12.0)
}

/// Plucked strings (Karplus-Strong), a note every 300 ms, panned around.
fn plucked_string() -> Vec<[f64; 2]> {
    let total = RATE as usize * SECONDS;
    let mut out = vec![[0.0; 2]; total];
    let mut rng = Rng(7);
    let scale = [0.0, 3.0, 5.0, 7.0, 10.0, 12.0, 7.0, 3.0];
    for (n, step) in scale.iter().cycle().take(SECONDS * 1000 / 300).enumerate() {
        let start = n * RATE as usize * 3 / 10;
        let hz = frequency(step - 12.0);
        let period = (RATE as f64 / hz) as usize;
        let mut line: Vec<f64> = (0..period).map(|_| rng.signed()).collect();
        let pan = 0.5 + 0.35 * (n as f64 * 1.3).sin();
        for i in 0..(RATE as usize * 8 / 10).min(total - start) {
            let at = i % period;
            let next = line[(at + 1) % period];
            let value = line[at];
            line[at] = 0.5 * (value + next) * 0.998;
            out[start + i][0] += value * (1.0 - pan);
            out[start + i][1] += value * pan;
        }
    }
    out
}

/// A sung vowel moving between /a/ and /i/ on a gliding pitch with vibrato: a glottal pulse
/// train through three resonators.
fn sung_vowel() -> Vec<[f64; 2]> {
    let total = RATE as usize * SECONDS;
    let mut rng = Rng(11);
    let formants_a = [730.0, 1090.0, 2440.0];
    let formants_i = [270.0, 2290.0, 3010.0];
    let mut phase = 0.0f64;
    let mut state = [[0.0f64; 2]; 3];
    let mut glottal = 0.0;
    let mut out = Vec::with_capacity(total);
    for n in 0..total {
        let t = n as f64 / RATE as f64;
        let f0 = 150.0 + 40.0 * (2.0 * PI * 0.4 * t).sin() + 4.0 * (2.0 * PI * 5.5 * t).sin();
        phase += f0 / RATE as f64;
        let pulse = if phase >= 1.0 {
            phase -= 1.0;
            1.0
        } else {
            0.0
        };
        glottal = glottal * 0.95 + pulse * 0.4;
        let excitation = glottal + 0.01 * rng.signed();
        let blend = 0.5 - 0.5 * (2.0 * PI * 0.5 * t).cos();
        let mut value = 0.0;
        for k in 0..3 {
            let hz = formants_a[k] * (1.0 - blend) + formants_i[k] * blend;
            let bandwidth = 60.0 + 20.0 * k as f64;
            let r = (-PI * bandwidth / RATE as f64).exp();
            let theta = 2.0 * PI * hz / RATE as f64;
            let y = excitation + 2.0 * r * theta.cos() * state[k][0] - r * r * state[k][1];
            state[k][1] = state[k][0];
            state[k][0] = y;
            value += y * (1.0 - 0.3 * k as f64) * (1.0 - r) * 3.0;
        }
        out.push([value, value]);
    }
    out
}

/// A chord of four notes that changes each second: a stack of harmonics with their own decay.
fn chord() -> Vec<[f64; 2]> {
    let total = RATE as usize * SECONDS;
    let mut out = vec![[0.0; 2]; total];
    let chords: [[f64; 4]; 4] = [
        [-9.0, -5.0, -2.0, 2.0],
        [-4.0, 0.0, 3.0, 7.0],
        [-2.0, 2.0, 5.0, 9.0],
        [0.0, 3.0, 7.0, 10.0],
    ];
    for (n, notes) in chords.iter().cycle().take(SECONDS).enumerate() {
        let start = n * RATE as usize;
        for (v, semitones) in notes.iter().enumerate() {
            let hz = frequency(*semitones);
            let detune = 1.0 + 0.0008 * (v as f64 - 1.5);
            for i in 0..(RATE as usize * 12 / 10).min(total - start) {
                let t = i as f64 / RATE as f64;
                let envelope = (-t / 0.9).exp() * (1.0 - (-t * 200.0).exp());
                let mut value = 0.0;
                for h in 1..=8 {
                    value += (2.0 * PI * hz * detune * h as f64 * t).sin() / h as f64;
                }
                let pan = 0.2 + 0.2 * v as f64;
                out[start + i][0] += value * envelope * (1.0 - pan);
                out[start + i][1] += value * envelope * pan;
            }
        }
    }
    out
}

/// Kick, snare and hi-hat at 120 bpm.
fn drums() -> Vec<[f64; 2]> {
    let total = RATE as usize * SECONDS;
    let mut out = vec![[0.0; 2]; total];
    let mut rng = Rng(23);
    let beat = RATE as usize / 2;
    for n in 0..SECONDS * 2 * 2 {
        let start = n * beat / 2;
        if start >= total {
            break;
        }
        let hat_length = (RATE as usize / 25).min(total - start);
        let mut high = 0.0;
        for i in 0..hat_length {
            let noise = rng.signed();
            let filtered = noise - high;
            high = noise * 0.9 + high * 0.1;
            let value = filtered * (-(i as f64) / (RATE as f64 * 0.008)).exp() * 0.25;
            out[start + i][0] += value;
            out[start + i][1] += value;
        }
        if n % 2 == 0 {
            let length = (RATE as usize * 3 / 10).min(total - start);
            if (n / 2) % 2 == 0 {
                let mut phase = 0.0f64;
                for i in 0..length {
                    let t = i as f64 / RATE as f64;
                    phase += (50.0 + 90.0 * (-t / 0.04).exp()) / RATE as f64;
                    let value = (2.0 * PI * phase).sin() * (-t / 0.09).exp();
                    out[start + i][0] += value * 0.9;
                    out[start + i][1] += value * 0.9;
                }
            } else {
                for i in 0..length {
                    let t = i as f64 / RATE as f64;
                    let value = (rng.signed() * 0.6 + (2.0 * PI * 190.0 * t).sin() * 0.5)
                        * (-t / 0.07).exp();
                    out[start + i][0] += value * 0.7;
                    out[start + i][1] += value * 0.7;
                }
            }
        }
    }
    out
}

/// A bowed note held with vibrato, moving to another every two seconds.
fn bowed_note() -> Vec<[f64; 2]> {
    let total = RATE as usize * SECONDS;
    let mut out = Vec::with_capacity(total);
    let notes = [-14.0, -9.0, -5.0];
    let mut phases = [0.0f64; 2];
    for n in 0..total {
        let t = n as f64 / RATE as f64;
        let hz = frequency(notes[(n / (RATE as usize * 2)) % 3])
            * (1.0 + 0.006 * (2.0 * PI * 5.0 * t).sin());
        let mut value = [0.0; 2];
        for (side, phase) in phases.iter_mut().enumerate() {
            *phase += hz * (1.0 + 0.002 * side as f64) / RATE as f64;
            *phase -= phase.floor();
            // A sawtooth with its top harmonics rolled off
            for h in 1..=14 {
                value[side] +=
                    (2.0 * PI * *phase * h as f64).sin() / h as f64 * (1.0 - h as f64 / 16.0);
            }
        }
        out.push(value);
    }
    out
}

fn normalised(sound: Vec<[f64; 2]>) -> Vec<f32> {
    let peak = sound
        .iter()
        .flat_map(|frame| frame.iter())
        .fold(0.0f64, |m, v| m.max(v.abs()));
    let gain = 0.7 / peak.max(1e-9);
    sound
        .iter()
        .flat_map(|frame| frame.iter().map(|v| (v * gain) as f32))
        .map(|v| (v * 8_388_608.0).round() / 8_388_608.0)
        .collect()
}

fn sounds() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("pluck", normalised(plucked_string())),
        ("voice", normalised(sung_vowel())),
        ("chord", normalised(chord())),
        ("drums", normalised(drums())),
        ("bowed", normalised(bowed_note())),
    ]
}

// --- Running the receive path ------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Block {
    Played(u32),
    Concealed(u32),
    Starved,
}

struct Heard {
    /// Everything the speaker was given, in order, interleaved
    samples: Vec<f32>,
    /// What each block of it was, with where it starts in `samples`
    blocks: Vec<(Block, usize, usize)>,
    filled: u64,
}

/// Sends `frames` to a receive path as 24-bit PCM, each arriving at the read number in
/// `arrives` (`None`: never), and reads one frame per read.
fn run(frames: &[f32], arrives: &[Option<usize>], delay: u32, concealment: bool) -> Heard {
    let path = ReceivePath::new(CodecType::Pcm, RATE, FRAME as u32, delay).expect("receive path");
    path.set_waveform_concealment(concealment);
    path.set_starved_prediction(concealment);
    let mut codec = PcmCodec::new(&CodecConfig {
        codec_type: CodecType::Pcm,
        sample_rate: RATE,
        channels: WIRE_CHANNELS as u16,
        frame_size: FRAME as u32,
        bitrate: 0,
    });
    let payloads: Vec<Vec<u8>> = frames
        .chunks(SAMPLES_PER_FRAME)
        .map(|frame| codec.encode(frame).expect("encode"))
        .collect();
    let last_read = arrives.iter().flatten().max().copied().unwrap_or(0) + delay as usize + 4;

    let mut order: Vec<usize> = (0..payloads.len())
        .filter(|i| arrives[*i].is_some())
        .collect();
    order.sort_by_key(|i| (arrives[*i], *i));
    let mut next = 0;

    let mut heard = Heard {
        samples: Vec::new(),
        blocks: Vec::new(),
        filled: 0,
    };
    let mut out = vec![0.0f32; path.widest_frame_samples()];
    for now in 0..=last_read {
        while next < order.len() && arrives[order[next]].unwrap() <= now {
            path.receive(order[next] as u32, &payloads[order[next]]);
            next += 1;
        }
        let read = path.read_into(&mut out);
        let kind = match read.result {
            PlayoutResult::Played { sequence } => Block::Played(sequence),
            PlayoutResult::Concealed { sequence } => Block::Concealed(sequence),
            PlayoutResult::Starved => Block::Starved,
            _ => continue,
        };
        heard.blocks.push((kind, heard.samples.len(), read.samples));
        heard.samples.extend_from_slice(&out[..read.samples]);
    }
    heard.filled = path.stats().frames_filled;
    heard
}

// --- Losses -----------------------------------------------------------------------------

/// Frames that never arrive: each frame starts a burst with probability `p`, of one frame
/// (70 %), two (20 %) or three (10 %).
fn lose_frames(count: usize, p: f64, seed: u64, keep_first: usize) -> Vec<Option<usize>> {
    let mut rng = Rng(seed);
    let mut arrives: Vec<Option<usize>> = (0..count).map(Some).collect();
    let mut i = keep_first;
    while i + 4 < count {
        if rng.next() < p {
            let r = rng.next();
            let length = if r < 0.7 {
                1
            } else if r < 0.9 {
                2
            } else {
                3
            };
            for slot in arrives.iter_mut().skip(i).take(length) {
                *slot = None;
            }
            i += length + 6;
        } else {
            i += 1;
        }
    }
    arrives
}

/// Frames that are held up together: with probability `p` per frame the link stalls for
/// `1..=max_stall` frame times and what was sent meanwhile arrives at once afterwards.
fn stall_link(
    count: usize,
    p: f64,
    max_stall: usize,
    seed: u64,
    keep_first: usize,
) -> Vec<Option<usize>> {
    let mut rng = Rng(seed);
    let mut arrives: Vec<Option<usize>> = (0..count).map(Some).collect();
    let mut i = keep_first;
    while i + 2 < count {
        if rng.next() < p {
            let stall = 1 + (rng.next() * max_stall as f64) as usize;
            for slot in arrives.iter_mut().take((i + stall).min(count)).skip(i) {
                *slot = Some(i + stall);
            }
            i += stall + 40;
        } else {
            i += 1;
        }
    }
    arrives
}

// --- Measuring --------------------------------------------------------------------------

fn snr_db(reference_energy: f64, error_energy: f64) -> f64 {
    10.0 * (reference_energy.max(1e-30) / error_energy.max(1e-30)).log10()
}

/// Log-spectral distance in dB between two stretches (mean over 64-sample Hann windows and
/// over the bins up to 16 kHz of the difference of their log magnitudes, left channel).
fn spectral_distance(a: &[f32], b: &[f32]) -> f64 {
    let frames = a.len().min(b.len()) / WIRE_CHANNELS;
    let window = frames.min(512);
    let bins = window / 2 * 2 / 3;
    let spectrum = |x: &[f32], start: usize| -> Vec<f64> {
        (1..bins)
            .map(|bin| {
                let (mut re, mut im) = (0.0, 0.0);
                for n in 0..window {
                    let w = 0.5 - 0.5 * (2.0 * PI * (n as f64 + 0.5) / window as f64).cos();
                    let v = f64::from(x[(start + n) * WIRE_CHANNELS]) * w;
                    let angle = 2.0 * PI * bin as f64 * n as f64 / window as f64;
                    re += v * angle.cos();
                    im -= v * angle.sin();
                }
                10.0 * (re * re + im * im + 1e-12).log10()
            })
            .collect()
    };
    let mut total = 0.0;
    let mut count = 0usize;
    let mut start = 0;
    while start + window <= frames {
        let (sa, sb) = (spectrum(a, start), spectrum(b, start));
        // Nothing below 70 dB under the loudest part of the original counts: that is under the
        // noise of any recording
        let floor = sa.iter().cloned().fold(f64::MIN, f64::max) - 70.0;
        let distance: f64 = sa
            .iter()
            .zip(&sb)
            .map(|(x, y)| (x.max(floor) - y.max(floor)).powi(2))
            .sum::<f64>()
            / sa.len() as f64;
        total += distance.sqrt();
        count += 1;
        start += window;
    }
    total / count.max(1) as f64
}

/// The largest jump between neighbouring samples within a few samples of `at` (a sample frame
/// index), against the typical jump in the original around it: 1 is as smooth as the sound
/// itself, and a click is several times that.
fn step_at(heard: &[f32], at: usize, original: &[f32], original_at: usize) -> f64 {
    let jump = |x: &[f32], from: usize, to: usize| -> f64 {
        let mut largest = 0.0f64;
        for n in from.max(1)..to.min(x.len() / WIRE_CHANNELS) {
            for c in 0..WIRE_CHANNELS {
                let d = f64::from(x[n * WIRE_CHANNELS + c] - x[(n - 1) * WIRE_CHANNELS + c]).abs();
                largest = largest.max(d);
            }
        }
        largest
    };
    let typical = {
        let from = original_at.saturating_sub(240);
        let to = (original_at + 240).min(original.len() / WIRE_CHANNELS);
        let mut sum = 0.0f64;
        let mut n = 0usize;
        for i in from.max(1)..to {
            let d = f64::from(original[i * WIRE_CHANNELS] - original[(i - 1) * WIRE_CHANNELS]);
            sum += d * d;
            n += 1;
        }
        (sum / n.max(1) as f64).sqrt().max(1e-6)
    };
    jump(heard, at.saturating_sub(3), at + 4) / typical
}

#[derive(Default, Debug)]
struct Scores {
    lost_energy: f64,
    lost_error: f64,
    all_energy: f64,
    all_error: f64,
    distance_sum: f64,
    distance_n: usize,
    steps: Vec<f64>,
    lost_frames: usize,
}

impl Scores {
    fn add(&mut self, other: Scores) {
        self.lost_energy += other.lost_energy;
        self.lost_error += other.lost_error;
        self.all_energy += other.all_energy;
        self.all_error += other.all_error;
        self.distance_sum += other.distance_sum;
        self.distance_n += other.distance_n;
        self.steps.extend(other.steps);
        self.lost_frames += other.lost_frames;
    }
    fn lost_snr(&self) -> f64 {
        snr_db(self.lost_energy, self.lost_error)
    }
    fn overall_snr(&self) -> f64 {
        snr_db(self.all_energy, self.all_error)
    }
    fn distance(&self) -> f64 {
        self.distance_sum / self.distance_n.max(1) as f64
    }
    fn step_p95(&self) -> f64 {
        let mut steps = self.steps.clone();
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        steps
            .get(steps.len().saturating_sub(1) * 95 / 100)
            .copied()
            .unwrap_or(0.0)
    }
}

fn energy(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v).powi(2)).sum()
}

fn error_energy(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2))
        .sum()
}

/// The spectral distance over the stretch of `span` samples that begins at `original_at` in the
/// original and `heard_at` in what was heard, with four frames either side: a gap is judged
/// among the sound that surrounds it, as the ear does.
fn context_distance(
    original: &[f32],
    heard: &Heard,
    original_at: usize,
    heard_at: usize,
    span: usize,
) -> f64 {
    let around = 4 * SAMPLES_PER_FRAME;
    if original_at < around || heard_at < around {
        return 0.0;
    }
    let end = span + 2 * around;
    if original_at - around + end > original.len() || heard_at - around + end > heard.samples.len()
    {
        return 0.0;
    }
    spectral_distance(
        &original[original_at - around..][..end],
        &heard.samples[heard_at - around..][..end],
    )
}

/// Scores a run where frames were lost but later ones were there: the speaker's output lines
/// up with the original frame for frame.
fn score_lost_frames(original: &[f32], heard: &Heard) -> Scores {
    let mut scores = Scores::default();
    let mut in_run = false;
    for (block, start, len) in heard.blocks.iter() {
        let (Block::Played(seq) | Block::Concealed(seq)) = block else {
            continue;
        };
        let reference = &original[*seq as usize * SAMPLES_PER_FRAME..][..*len];
        let got = &heard.samples[*start..*start + len];
        scores.all_energy += energy(reference);
        scores.all_error += error_energy(reference, got);
        match block {
            Block::Concealed(_) => {
                scores.lost_energy += energy(reference);
                scores.lost_error += error_energy(reference, got);
                scores.distance_sum += context_distance(
                    original,
                    heard,
                    *seq as usize * SAMPLES_PER_FRAME,
                    *start,
                    SAMPLES_PER_FRAME,
                );
                scores.distance_n += 1;
                scores.lost_frames += 1;
                if !in_run {
                    in_run = true;
                    scores.steps.push(step_at(
                        &heard.samples,
                        start / WIRE_CHANNELS,
                        original,
                        *seq as usize * FRAME,
                    ));
                }
            }
            _ => {
                if std::mem::take(&mut in_run) {
                    scores.steps.push(step_at(
                        &heard.samples,
                        start / WIRE_CHANNELS,
                        original,
                        *seq as usize * FRAME,
                    ));
                }
            }
        }
    }
    scores
}

/// Scores a run with stalls: what stood in for the stalled time is compared with what the
/// sender had sent for it (which plays next), and the joins on both sides are measured.
fn score_stalls(original: &[f32], heard: &Heard) -> Scores {
    let mut scores = Scores::default();
    let blocks = &heard.blocks;
    let mut i = 0;
    while i < blocks.len() {
        if blocks[i].0 != Block::Starved {
            i += 1;
            continue;
        }
        let begin = i;
        while i < blocks.len() && blocks[i].0 == Block::Starved {
            i += 1;
        }
        let Some((Block::Played(next), after, _)) = blocks.get(i).copied() else {
            break;
        };
        let (from, span) = (blocks[begin].1, after - blocks[begin].1);
        let reference_start = next as usize * SAMPLES_PER_FRAME;
        if reference_start + span > original.len() {
            break;
        }
        let reference = &original[reference_start..reference_start + span];
        let got = &heard.samples[from..from + span];
        scores.lost_energy += energy(reference);
        scores.lost_error += error_energy(reference, got);
        scores.distance_sum += context_distance(original, heard, reference_start, from, span);
        scores.distance_n += 1;
        scores.lost_frames += i - begin;
        let (s0, s1) = (
            step_at(
                &heard.samples,
                from / WIRE_CHANNELS,
                original,
                next as usize * FRAME,
            ),
            step_at(
                &heard.samples,
                after / WIRE_CHANNELS,
                original,
                next as usize * FRAME,
            ),
        );
        scores.steps.push(s0);
        scores.steps.push(s1);
    }
    scores
}

fn write_wav(path: &PathBuf, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    for v in samples {
        writer
            .write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16)
            .expect("write wav");
    }
    writer.finalize().expect("finalize wav");
}

fn output_directory() -> PathBuf {
    let dir = std::env::var("JAMUMU_CONCEAL_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/conceal-wavs"));
    std::fs::create_dir_all(&dir).expect("output directory");
    dir
}

#[test]
#[ignore = "a measurement, run by hand"]
fn how_well_gaps_are_filled_next_to_the_old_way() {
    let dir = output_directory();
    println!("\nFRAME LOSS - frames lost, later frames already held (delay 2 frames)");
    println!("sound   loss%  | lost-frame SNR dB (old -> new) | whole SNR dB (old -> new) | spectral dist dB (old -> new) | join step p95 (old -> new; the sound undamaged)");
    for (name, sound) in sounds() {
        let count = sound.len() / SAMPLES_PER_FRAME;
        for (label, p) in [("light", 0.02), ("heavy", 0.07)] {
            let arrives = lose_frames(count, p, 5, 40);
            let old = run(&sound, &arrives, 2, false);
            let new = run(&sound, &arrives, 2, true);
            let (so, sn) = (
                score_lost_frames(&sound, &old),
                score_lost_frames(&sound, &new),
            );
            let mut undamaged_samples = sound.clone();
            undamaged_samples.resize(new.samples.len().max(sound.len()), 0.0);
            let undamaged = Heard {
                samples: undamaged_samples,
                blocks: new.blocks.clone(),
                filled: 0,
            };
            let intact = score_lost_frames(&sound, &undamaged);
            println!(
                "{name:<7} {label:<5}{:>4.1}% | {:>7.2} -> {:>7.2}            | {:>6.2} -> {:>6.2}          | {:>6.2} -> {:>6.2}              | {:>5.2} -> {:>5.2}  (undamaged {:>5.2}) (filled {} of {})",
                100.0 * so.lost_frames as f64 / count as f64,
                so.lost_snr(), sn.lost_snr(), so.overall_snr(), sn.overall_snr(),
                so.distance(), sn.distance(), so.step_p95(), sn.step_p95(), intact.step_p95(), new.filled, sn.lost_frames
            );
            if label == "light" {
                write_wav(&dir.join(format!("{name}-lost-original.wav")), &sound);
                write_wav(&dir.join(format!("{name}-lost-old.wav")), &old.samples);
                write_wav(&dir.join(format!("{name}-lost-new.wav")), &new.samples);
            }
        }
    }

    println!(
        "\nSTALL - the link stalls, nothing has arrived (delay 1 frame, stalls of up to 8 frames)"
    );
    println!("sound   starved%| stall SNR dB (old -> new) | spectral dist dB (old -> new) | join step p95 (old -> new)");
    for (name, sound) in sounds() {
        let count = sound.len() / SAMPLES_PER_FRAME;
        for (label, p, longest) in [("short", 0.02, 4), ("long", 0.02, 8)] {
            let (mut so, mut sn) = (Scores::default(), Scores::default());
            let mut filled = 0;
            for seed in 1..=STALL_SEEDS {
                let arrives = stall_link(count, p, longest, seed, 40);
                let old = run(&sound, &arrives, 1, false);
                let new = run(&sound, &arrives, 1, true);
                so.add(score_stalls(&sound, &old));
                sn.add(score_stalls(&sound, &new));
                filled += new.filled;
                if seed == 9 && label == "short" {
                    write_wav(&dir.join(format!("{name}-stall-old.wav")), &old.samples);
                    write_wav(&dir.join(format!("{name}-stall-new.wav")), &new.samples);
                }
            }
            println!(
                "{name:<7} {label:<5}{:>4.1}%| {:>7.2} -> {:>7.2}      | {:>6.2} -> {:>6.2}              | {:>5.2} -> {:>5.2}  (filled {} of {}, {} stalls)",
                100.0 * so.lost_frames as f64 / (count * STALL_SEEDS as usize) as f64,
                so.lost_snr(), sn.lost_snr(), so.distance(), sn.distance(), so.step_p95(), sn.step_p95(), filled, sn.lost_frames, so.distance_n
            );
        }
    }
}

#[test]
#[ignore = "a measurement, run by hand"]
fn how_long_filling_a_gap_takes_next_to_the_time_a_frame_lasts() {
    // A frame of 64 samples at 48 kHz lasts 1333 us
    let sound = &sounds()[0].1;
    let mut concealer = Concealer::new(WIRE_CHANNELS, RATE);
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES_PER_FRAME).collect();
    for frame in &frames[..200] {
        let mut copy = frame.to_vec();
        concealer.on_played(&mut copy);
    }
    for (label, extrapolate) in [
        ("gap filled from both sides", false),
        ("sound predicted from the past", true),
    ] {
        let mut worst = std::time::Duration::ZERO;
        let mut total = std::time::Duration::ZERO;
        let rounds = 2000;
        for round in 0..rounds {
            let mut copy = frames[200 + round % 100].to_vec();
            concealer.on_played(&mut copy);
            let mut out = vec![0.0f32; SAMPLES_PER_FRAME];
            let started = Instant::now();
            if extrapolate {
                concealer.extrapolate(&mut out);
            } else {
                concealer.fill_gap(&mut out, Some(frames[201 + round % 100]));
            }
            let took = started.elapsed();
            total += took;
            worst = worst.max(took);
            concealer.interrupt();
        }
        println!(
            "{label}: mean {:.1} us, worst {:.1} us (a 64-sample frame lasts 1333 us)",
            total.as_secs_f64() * 1e6 / rounds as f64,
            worst.as_secs_f64() * 1e6
        );
    }
}

/// Fills a single frame cut out of the sound at many places, straight through the concealer,
/// and reports how close to the frame that was cut out the fill is.
#[test]
#[ignore = "a measurement, run by hand"]
fn single_holes_filled_straight_from_the_concealer() {
    use jamjam::audio::PcmPlc;
    for (name, sound) in sounds() {
        let frames: Vec<&[f32]> = sound.chunks(SAMPLES_PER_FRAME).collect();
        let (mut e_ref, mut e_old, mut e_new) = (0.0, 0.0, 0.0);
        let (mut better, mut total, mut filled) = (0, 0, 0);
        let mut lsd = (0.0, 0.0);
        let mut level = (0.0, 0.0);
        let (mut e_lpc, mut lsd_lpc) = (0.0, 0.0);
        let mut not_predicted = 0;
        for hole in (60..frames.len() - 4).step_by(7) {
            let mut concealer = Concealer::new(WIRE_CHANNELS, RATE);
            let mut plc = PcmPlc::new(FRAME as u32, WIRE_CHANNELS as u16);
            for frame in &frames[hole - 50..hole] {
                let mut copy = frame.to_vec();
                concealer.on_played(&mut copy);
                plc.store_frame(frame);
            }
            let mut new = vec![0.0f32; SAMPLES_PER_FRAME];
            let ok = concealer.fill_gap(&mut new, Some(frames[hole + 1]));
            let mut old = vec![0.0f32; SAMPLES_PER_FRAME];
            plc.conceal_into(&mut old);
            if !ok {
                new.copy_from_slice(&old);
            }
            let reference = frames[hole];
            let context = |fill: &[f32]| -> Vec<f32> {
                let mut v: Vec<f32> = frames[hole - 4..hole].concat();
                v.extend_from_slice(fill);
                v.extend_from_slice(&frames[hole + 1..hole + 5].concat());
                v
            };
            let truth_context = frames[hole - 4..hole + 5].concat();
            let mut predicted = vec![0.0f32; SAMPLES_PER_FRAME];
            let mut other = Concealer::new(WIRE_CHANNELS, RATE);
            for frame in &frames[hole - 50..hole] {
                let mut copy = frame.to_vec();
                other.on_played(&mut copy);
            }
            let predicted_ok = other.extrapolate(&mut predicted);
            if !predicted_ok {
                not_predicted += 1;
            }
            e_lpc += error_energy(reference, &predicted);
            lsd_lpc += spectral_distance(&truth_context, &context(&predicted));
            let (a, b) = (error_energy(reference, &old), error_energy(reference, &new));
            e_ref += energy(reference);
            e_old += a;
            e_new += b;
            total += 1;
            filled += usize::from(ok);
            better += usize::from(b < a);
            level.0 += 10.0 * ((energy(&old) + 1e-12) / (energy(reference) + 1e-12)).log10();
            level.1 += 10.0 * ((energy(&new) + 1e-12) / (energy(reference) + 1e-12)).log10();
            lsd.0 += spectral_distance(&truth_context, &context(&old));
            lsd.1 += spectral_distance(&truth_context, &context(&new));
        }
        println!(
            "{name:<6} SNR old {:>6.2} new {:>6.2} dB | LSD old {:>5.2} new {:>5.2} | filled {filled}/{total}, closer than the old way in {better} | PREDICT SNR {:>6.2} LSD {:>5.2} (none {not_predicted}) | level vs truth old {:>5.2} new {:>5.2} dB",
            snr_db(e_ref, e_old), snr_db(e_ref, e_new), lsd.0 / total as f64, lsd.1 / total as f64, snr_db(e_ref, e_lpc), lsd_lpc / total as f64, level.0 / total as f64, level.1 / total as f64
        );
    }
}

#[test]
#[ignore = "a measurement, run by hand"]
fn debug_one_prediction() {
    let (_, sound) = sounds().into_iter().find(|(n, _)| *n == "chord").unwrap();
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES_PER_FRAME).collect();
    for hole in [200usize, 207, 214, 221] {
        let mut c = Concealer::new(WIRE_CHANNELS, RATE);
        for frame in &frames[hole - 50..hole] {
            let mut copy = frame.to_vec();
            c.on_played(&mut copy);
        }
        let mut out = vec![0.0f32; SAMPLES_PER_FRAME];
        c.extrapolate(&mut out);
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let truth = frames[hole].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!(
            "hole {hole}: peak out {peak:.3} truth {truth:.3}  energy out {:.4} truth {:.4}",
            energy(&out),
            energy(frames[hole])
        );
    }
}
