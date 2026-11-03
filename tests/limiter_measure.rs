//! Measures what the limiter (ADR-094) costs: the time one call takes in the audio callback and
//! send thread, and how much it distorts a signal that goes over 0 dBFS, next to the hard clip
//! the device would have made of the same signal. Run by hand to compare a change to the limiter
//! against the version before it:
//!
//! ```text
//! cargo test --release --test limiter_measure -- --ignored --nocapture
//! ```
//!
//! The delay it adds is not measured here: it is none, and `a_single_loud_sample_is_brought_down_on_that_sample`
//! (in `src/audio/limiter.rs`) holds that.

use std::f64::consts::PI;
use std::time::Instant;

use jamjam::audio::Limiter;

const RATE: u32 = 48_000;
const FRAME: usize = 64;
const HZ: f64 = 440.0;

fn sine(amplitude: f64, seconds: usize) -> Vec<f32> {
    (0..RATE as usize * seconds)
        .flat_map(|n| {
            let s = (amplitude * (2.0 * PI * HZ * n as f64 / RATE as f64).sin()) as f32;
            [s, s]
        })
        .collect()
}

/// What is left of `samples` (left channel) once the 440 Hz sine it was made from is taken
/// out, against that sine, in dB: the distortion and noise the processing added
fn residual_db(samples: &[f32]) -> f64 {
    let n = samples.len() / 2;
    let (mut sin_sum, mut cos_sum) = (0.0, 0.0);
    for i in 0..n {
        let phase = 2.0 * PI * HZ * i as f64 / RATE as f64;
        sin_sum += samples[i * 2] as f64 * phase.sin();
        cos_sum += samples[i * 2] as f64 * phase.cos();
    }
    let (a, b) = (2.0 * sin_sum / n as f64, 2.0 * cos_sum / n as f64);
    let mut residual = 0.0;
    for i in 0..n {
        let phase = 2.0 * PI * HZ * i as f64 / RATE as f64;
        let fitted = a * phase.sin() + b * phase.cos();
        residual += (samples[i * 2] as f64 - fitted).powi(2);
    }
    let fundamental = (a * a + b * b) / 2.0;
    10.0 * ((residual / n as f64) / fundamental).log10()
}

#[test]
#[ignore = "a measurement, run by hand"]
fn how_much_the_limiter_distorts_a_signal_that_goes_over_full_scale() {
    println!("amplitude  limited(dB)  hard clip(dB)   (residual against the 440 Hz sine; lower is cleaner)");
    for amplitude in [0.5, 1.0, 1.1, 1.5, 2.0, 4.0] {
        let original = sine(amplitude, 2);
        let mut limited = original.clone();
        let mut limiter = Limiter::new(RATE);
        for chunk in limited.chunks_mut(FRAME * 2) {
            limiter.process(chunk, 2);
        }
        let clipped: Vec<f32> = original.iter().map(|s| s.clamp(-1.0, 1.0)).collect();
        // The first half second is the limiter finding its gain; the steady state is what is heard
        let from = RATE as usize;
        let show = |db: f64| {
            if db < -200.0 {
                "  exact".to_string()
            } else {
                format!("{db:7.1}")
            }
        };
        let limited_db = if amplitude <= 1.0 {
            f64::NEG_INFINITY
        } else {
            residual_db(&limited[from * 2..])
        };
        let clipped_db = if amplitude <= 1.0 {
            f64::NEG_INFINITY
        } else {
            residual_db(&clipped[from * 2..])
        };
        println!(
            "{amplitude:9.1}  {}      {}",
            show(limited_db),
            show(clipped_db)
        );
        if amplitude <= 1.0 {
            assert_eq!(
                limited, original,
                "at or under 0 dBFS the audio is left as it is"
            );
        }
    }
}

#[test]
#[ignore = "a measurement, run by hand"]
fn how_long_one_call_takes() {
    let mut buffer = sine(0.5, 1);
    let mut loud = sine(3.0, 1);
    for (name, samples) in [
        ("at or under 0 dBFS", &mut buffer),
        ("over 0 dBFS", &mut loud),
    ] {
        let mut limiter = Limiter::new(RATE);
        let calls = samples.len() / (FRAME * 2);
        let started = Instant::now();
        for _ in 0..20 {
            let mut work = samples.clone();
            for chunk in work.chunks_mut(FRAME * 2) {
                limiter.process(chunk, 2);
            }
            std::hint::black_box(&work);
        }
        let per_call = started.elapsed().as_nanos() as f64 / (20 * calls) as f64;
        println!(
            "{name}: {per_call:.0} ns per {FRAME}-frame stereo call (the frame lasts {:.0} us)",
            FRAME as f64 / RATE as f64 * 1e6
        );
    }
}
