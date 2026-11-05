//! Measures what packing the audio without loss (ADR-111) saves and what it costs: the size of a packet
//! next to the 24-bit samples it holds, for a plucked string, a voice, a recorded voice, the noise floor
//! of an audio interface and full-scale noise, and the time one packet takes to pack and to unpack. Run by
//! hand:
//!
//! ```text
//! cargo test --release --test packed_measure -- --ignored --nocapture
//! ```
//!
//! `JAMUARU_SPEECH_WAV` may name a 16-bit mono WAV file of speech to measure as well. The interface noise
//! is added to it, because a 16-bit file has eight low bits that are zero and a 24-bit interface does not.
//!
//! The instruments are made up (a plucked string, a voice-like source through formants) and the noise
//! floor is a Gaussian one at the level an interface of that class resolves: they stand for the signal,
//! and the numbers are not a measurement of any one device.

use std::f64::consts::PI;
use std::time::Instant;

use jamjam::audio::{decode_packed, encode_packed, encode_packed_exact, verbatim_len, SIZE_STEPS};

const RATE: f64 = 48_000.0;
const SECONDS: usize = 20;

struct Noise(u64);

impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        (-2.0 * self.uniform().ln()).sqrt() * (2.0 * PI * self.uniform()).cos()
    }
}

fn dbfs(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Plucked strings (Karplus-Strong), a chord picked a string at a time and played again
fn guitar(noise: &mut Noise) -> Vec<f64> {
    let notes = [82.4, 110.0, 146.8, 196.0, 246.9, 329.6];
    let n = RATE as usize * SECONDS;
    let mut out = vec![0.0; n];
    let mut at = 0;
    let mut round = 0;
    while at < n {
        for (i, f) in notes.iter().enumerate() {
            let f = f * if round % 2 == 0 { 1.0 } else { 1.122 };
            let start = at + i * 2400;
            let len = (RATE / f) as usize;
            let mut line: Vec<f64> = (0..len).map(|_| noise.uniform() * 2.0 - 1.0).collect();
            let mut last = 0.0;
            for k in 0..(RATE as usize * 2) {
                let slot = k % len;
                let value = line[slot];
                line[slot] = 0.5 * (value + last) * 0.998;
                last = value;
                if start + k < n {
                    out[start + k] += value * 0.12;
                }
            }
        }
        at += RATE as usize;
        round += 1;
    }
    out
}

/// A voice-like source: a pulse train with a wandering pitch through three formants, in syllables
fn voice(noise: &mut Noise) -> Vec<f64> {
    let n = RATE as usize * SECONDS;
    let formants = [(700.0, 90.0), (1200.0, 100.0), (2600.0, 140.0)];
    let mut state = [[0.0f64; 2]; 3];
    let mut phase = 0.0;
    let mut lowpass = [0.0f64; 2];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f64 / RATE;
        let f0 = 130.0 + 25.0 * (2.0 * PI * 0.7 * t).sin() + 8.0 * (2.0 * PI * 5.0 * t).sin();
        phase = (phase + f0 / RATE) % 1.0;
        let pulse = if phase < 0.4 {
            (phase / 0.4 * PI).sin().powi(2)
        } else {
            0.0
        };
        let breath = noise.gaussian() * 0.02;
        let source = pulse - 0.15 + breath;
        lowpass[0] += 0.3 * (source - lowpass[0]);
        lowpass[1] += 0.3 * (lowpass[0] - lowpass[1]);
        let mut x = lowpass[1];
        for (state, (freq, width)) in state.iter_mut().zip(formants) {
            let r = (-PI * width / RATE).exp();
            let theta = 2.0 * PI * freq / RATE;
            let y = x + 2.0 * r * theta.cos() * state[0] - r * r * state[1];
            state[1] = state[0];
            state[0] = y;
            x = y * (1.0 - r);
        }
        let syllable = (0.5 - 0.5 * (2.0 * PI * 3.5 * t).cos()).powf(0.6);
        let phrase = if (t % 5.0) < 4.0 { 1.0 } else { 0.0 };
        out.push(x * syllable * phrase * 1.6);
    }
    out
}

fn recorded_speech() -> Option<Vec<f64>> {
    let bytes = std::fs::read(std::env::var("JAMUARU_SPEECH_WAV").ok()?).ok()?;
    let data = bytes.windows(4).position(|w| w == b"data")? + 8;
    Some(
        bytes[data..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b) as f64 / 32768.0)
            .collect(),
    )
}

/// Interleaved stereo `signal` with an interface's noise floor. `centred` is one microphone
/// placed in the middle, which is what the app sends for a mono input: the same sample on both
/// sides. Otherwise it is two inputs, each with a floor of its own.
fn with_floor(
    signal: &[f64],
    floor_db: Option<f64>,
    peak: f64,
    centred: bool,
    noise: &mut Noise,
) -> Vec<f32> {
    let top = signal.iter().fold(1e-9f64, |m, s| m.max(s.abs()));
    let gain = if peak > 0.0 { peak / top } else { 0.0 };
    let floor = floor_db.map_or(0.0, dbfs);
    signal
        .iter()
        .flat_map(|s| {
            let left = s * gain + noise.gaussian() * floor;
            if centred {
                let side = (left * std::f64::consts::FRAC_1_SQRT_2) as f32;
                return [side, side];
            }
            let right = s * gain * 0.9 + noise.gaussian() * floor;
            [left as f32, right as f32]
        })
        .collect()
}

struct Row {
    packed: f64,
    padded: f64,
    classes: Vec<usize>,
    encode_us: f64,
    decode_us: f64,
}

fn measure(samples: &[f32], frames: usize) -> Row {
    let per_packet = frames * 2;
    let mut exact = 0usize;
    let mut padded = 0usize;
    let mut raw = 0usize;
    let mut classes = vec![0usize; SIZE_STEPS + 1];
    let mut buffer = Vec::new();
    let mut back = vec![0.0f32; per_packet];
    let (mut enc, mut dec) = (0.0, 0.0);
    let mut packets = 0;
    for chunk in samples.chunks_exact(per_packet) {
        buffer.clear();
        encode_packed_exact(chunk, 2, &mut buffer);
        exact += buffer.len();
        buffer.clear();
        let start = Instant::now();
        encode_packed(chunk, 2, &mut buffer);
        enc += start.elapsed().as_secs_f64();
        padded += buffer.len();
        raw += 3 * per_packet;
        classes[(buffer.len() * SIZE_STEPS)
            .div_ceil(verbatim_len(per_packet))
            .min(SIZE_STEPS)] += 1;
        let start = Instant::now();
        decode_packed(&buffer, &mut back).unwrap();
        dec += start.elapsed().as_secs_f64();
        for (a, b) in chunk.iter().zip(&back) {
            assert!((a - b).abs() <= 0.5 / 8_388_608.0 + 1e-9 || a.abs() > 1.0);
        }
        packets += 1;
    }
    Row {
        packed: exact as f64 / raw as f64,
        padded: padded as f64 / raw as f64,
        classes,
        encode_us: enc / packets as f64 * 1e6,
        decode_us: dec / packets as f64 * 1e6,
    }
}

/// Bits of the packet size a watcher learns per packet
fn entropy(classes: &[usize]) -> f64 {
    let total = classes.iter().sum::<usize>() as f64;
    classes
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / total;
            -p * p.log2()
        })
        .sum::<f64>()
        .abs()
}

#[test]
#[ignore = "measurement: run by hand"]
fn how_much_packing_saves() {
    let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
    let guitar_signal = guitar(&mut noise);
    let voice_signal = voice(&mut noise);
    let speech = recorded_speech();
    let silent = vec![0.0; RATE as usize * SECONDS];

    println!("share of the 24-bit samples (3 bytes each) the packet takes; exact / rounded up to one of {SIZE_STEPS} sizes");
    for frames in [64usize, 128] {
        println!("\n== {frames} frames of stereo per packet ==");
        println!(
            "{:<44} {:>7} {:>8} {:>8} {:>10} {:>10}",
            "source", "exact", "rounded", "bits/pkt", "pack µs", "unpack µs"
        );
        let mut cases: Vec<(String, Vec<f32>)> = Vec::new();
        for centred in [true, false] {
            let layout = if centred {
                "mono, centred"
            } else {
                "two inputs"
            };
            for floor in [-110.0, -100.0] {
                let mut add = |name: &str, signal: &[f64], peak: f64| {
                    cases.push((
                        format!("{name}, {layout}, {floor} dBFS"),
                        with_floor(signal, Some(floor), peak, centred, &mut noise),
                    ));
                };
                add("guitar", &guitar_signal, 0.5);
                add("voice", &voice_signal, 0.5);
                if let Some(speech) = &speech {
                    add("recorded voice", speech, 0.5);
                }
                add("silence", &silent, 0.0);
            }
        }
        cases.push((
            "digital silence".into(),
            with_floor(&silent, None, 0.0, false, &mut noise),
        ));
        let white: Vec<f64> = (0..RATE as usize * SECONDS)
            .map(|_| noise.gaussian() * 0.25)
            .collect();
        cases.push((
            "loud noise (Gaussian, 0.25 RMS)".into(),
            with_floor(&white, None, 0.9, false, &mut noise),
        ));
        for (name, samples) in cases {
            let row = measure(&samples, frames);
            println!(
                "{:<44} {:>6.1}% {:>7.1}% {:>8.2} {:>10.1} {:>10.1}",
                name,
                row.packed * 100.0,
                row.padded * 100.0,
                entropy(&row.classes),
                row.encode_us,
                row.decode_us
            );
        }
    }
}
