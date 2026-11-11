//! Measures what playing a little faster or slower costs (ADR-122): the level and the
//! distortion of a tone during a stretch, next to the straight-line interpolation that a stretch
//! could have been made with, and the time one call takes in the audio callback. Run by hand to
//! compare a change to the stretch against the version before it:
//!
//! ```text
//! cargo test --release --test stretch_measure -- --ignored --nocapture
//! ```

use std::f64::consts::PI;
use std::time::Instant;

use jamjam::audio::Stretcher;

const RATE: u32 = 48_000;
const BLOCK: usize = 64;
/// Sample frames a stretch takes: 20 ms
const SPAN: usize = 960;

/// What the stretch of `tone_hz` made: the level against the level sent (dB), and what is left
/// of the signal once the tone it became (the same tone, off by 1/960) is taken out of it, against
/// that tone (dB)
struct Quality {
    gain_db: f64,
    residual_db: f64,
}

/// The tone fitted to `stretched` (left channel), whose frequency is `hz`
fn fit(stretched: &[f64], hz: f64, first_input_index: f64, step: f64) -> Quality {
    let phase = |k: usize| 2.0 * PI * hz * (first_input_index + k as f64 * step) / f64::from(RATE);
    let n = stretched.len();
    let (mut sin_sum, mut cos_sum) = (0.0, 0.0);
    for (k, value) in stretched.iter().enumerate() {
        sin_sum += value * phase(k).sin();
        cos_sum += value * phase(k).cos();
    }
    // Least squares for a sine of a known frequency over a stretch that is not a whole number of
    // its periods: solve the two-by-two normal equations
    let (mut ss, mut cc, mut sc) = (0.0, 0.0, 0.0);
    for k in 0..n {
        let (s, c) = phase(k).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
    }
    let det = ss * cc - sc * sc;
    let a = (sin_sum * cc - cos_sum * sc) / det;
    let b = (cos_sum * ss - sin_sum * sc) / det;
    let mut residual = 0.0;
    for (k, value) in stretched.iter().enumerate() {
        let fitted = a * phase(k).sin() + b * phase(k).cos();
        residual += (value - fitted).powi(2);
    }
    let level = (a * a + b * b) / 2.0;
    Quality {
        gain_db: 10.0 * (level / 0.5).log10(),
        residual_db: 10.0 * ((residual / n as f64) / level).log10(),
    }
}

/// A stretch of a full-scale `hz` tone through the receive path's stretcher; `direction` +1 takes a
/// sample frame out, -1 puts one in
fn cubic(hz: f64, direction: f64) -> Quality {
    let mut stretcher = Stretcher::new(RATE, 4096);
    let ppm = (1000.0 * direction) as f32;
    let mut next = 0usize;
    let mut started_at = None;
    let mut out_left: Vec<f64> = Vec::new();
    let mut seen = 0;
    for _ in 0..200 {
        let mut block = vec![0.0f32; 4096];
        for i in 0..BLOCK {
            let s = (2.0 * PI * hz * (next + i) as f64 / f64::from(RATE)).sin() as f32;
            block[2 * i] = s;
            block[2 * i + 1] = s;
        }
        let len = stretcher.process(&mut block, BLOCK * 2, ppm, true);
        let stats = stretcher.stats();
        let begun = stats.removals + stats.insertions;
        if started_at.is_none() && begun > seen {
            started_at = Some((next, out_left.len()));
        }
        seen = begun;
        if started_at.is_some() {
            out_left.extend(block[..len].iter().step_by(2).map(|&s| f64::from(s)));
        }
        next += BLOCK;
    }
    let (input_at, output_at) = started_at.expect("a stretch started");
    let _ = output_at;
    let step = 1.0 + direction / SPAN as f64;
    fit(&out_left[..SPAN], hz, input_at as f64, step)
}

/// The same stretch made with the straight line between the two sample frames around the position
fn linear(hz: f64, direction: f64) -> Quality {
    let step = 1.0 + direction / SPAN as f64;
    let input = |n: f64| (2.0 * PI * hz * n / f64::from(RATE)).sin();
    let first = 100.0;
    let stretched: Vec<f64> = (0..SPAN)
        .map(|k| {
            let q = first + k as f64 * step;
            let i = q.floor();
            let t = q - i;
            input(i) * (1.0 - t) + input(i + 1.0) * t
        })
        .collect();
    fit(&stretched, hz, first, step)
}

#[test]
#[ignore = "a measurement: run by hand"]
fn measure_the_level_and_the_distortion_of_a_tone_during_a_stretch() {
    println!("tone      cubic: level  residual    straight line: level  residual");
    for hz in [
        100.0, 1000.0, 5000.0, 10_000.0, 15_000.0, 18_000.0, 20_000.0,
    ] {
        for direction in [1.0, -1.0] {
            let c = cubic(hz, direction);
            let l = linear(hz, direction);
            println!(
                "{hz:>7.0} Hz ({}) {:+7.3} dB {:7.1} dB      {:+7.3} dB {:7.1} dB",
                if direction > 0.0 { "out" } else { "in " },
                c.gain_db,
                c.residual_db,
                l.gain_db,
                l.residual_db
            );
        }
    }
}

#[test]
#[ignore = "a measurement: run by hand"]
fn measure_the_time_a_call_takes_in_the_audio_callback() {
    let mut block = vec![0.3f32; 4096];
    for (name, ppm) in [("untouched", 0.0f32), ("stretching", 1000.0)] {
        let mut stretcher = Stretcher::new(RATE, 4096);
        let calls = 2_000_000;
        let start = Instant::now();
        let mut checksum = 0usize;
        for _ in 0..calls {
            checksum += stretcher.process(&mut block, BLOCK * 2, ppm, true);
        }
        let per_call = start.elapsed().as_nanos() as f64 / f64::from(calls);
        println!(
            "{name}: {per_call:.0} ns per call of {BLOCK} stereo sample frames ({:.4} % of the {:.0} us the frame lasts) [{checksum}]",
            per_call / (BLOCK as f64 / f64::from(RATE) * 1e9) * 100.0,
            BLOCK as f64 / f64::from(RATE) * 1e6
        );
    }
}
