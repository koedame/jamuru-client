//! Filling the gaps FEC could not repair with sound that continues what was heard (ADR-105).
//!
//! The sounds are made from formulas here, so there is no recording to license. How much better
//! the fill is than the old repeat-and-fade, across kinds of sound, is measured in
//! `conceal_measure.rs`; these tests hold the properties that must not be lost.

use jamjam::audio::{Concealer, PcmPlc, PlayoutBuffer, PlayoutConfig, PlayoutResult};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const FRAME: usize = 64;
const SAMPLES: usize = FRAME * CHANNELS;

/// A held note with overtones, slightly different in the two channels: `frames` frames of it,
/// from frame number `from`.
fn note(hz: f64, from: usize, frames: usize) -> Vec<f32> {
    (from * FRAME..(from + frames) * FRAME)
        .flat_map(|n| {
            let t = n as f64 / f64::from(RATE);
            let wave = |k: f64| (2.0 * std::f64::consts::PI * hz * k * t).sin();
            let v = 0.4 * wave(1.0) + 0.2 * wave(2.0) + 0.1 * wave(3.0);
            [v as f32, (v * 0.7) as f32]
        })
        .collect()
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

fn heard(concealer: &mut Concealer, sound: &[f32]) {
    for frame in sound.chunks(SAMPLES) {
        let mut copy = frame.to_vec();
        concealer.on_played(&mut copy);
    }
}

/// Where a frame is missing and the one after it is here, the fill is closer to the lost sound
/// than the old repeat-and-fade is, for notes of different pitch.
///
/// Verifies: REQ-AUD-070
#[test]
fn a_lost_frame_with_a_later_one_held_is_filled_closer_than_the_old_repeat() {
    for hz in [110.0, 196.0, 330.0, 523.0] {
        let sound = note(hz, 0, 80);
        let frames: Vec<&[f32]> = sound.chunks(SAMPLES).collect();
        let mut concealer = Concealer::new(CHANNELS, RATE);
        let mut plc = PcmPlc::new(FRAME as u32, CHANNELS as u16);
        for frame in &frames[..60] {
            let mut copy = frame.to_vec();
            concealer.on_played(&mut copy);
            plc.store_frame(frame);
        }
        let mut filled = vec![0.0f32; SAMPLES];
        assert!(
            concealer.fill_gap(&mut filled, Some(frames[61])),
            "{hz} Hz was not filled"
        );
        let mut repeated = vec![0.0f32; SAMPLES];
        plc.conceal_into(&mut repeated);

        let lost = frames[60];
        let (new, old) = (error_energy(lost, &filled), error_energy(lost, &repeated));
        assert!(
            new < 0.5 * old,
            "{hz} Hz: the fill is off by {new:.4}, the repeat by {old:.4}"
        );
        assert!(
            new < 0.5 * energy(lost),
            "{hz} Hz: the fill is no closer than silence"
        );
    }
}

/// A fill is the length of the gap, finite, and never louder than the sound around it, even on
/// noise that nothing can be predicted from.
///
/// Verifies: REQ-AUD-070
#[test]
fn a_fill_is_never_louder_than_the_sound_around_the_gap() {
    let mut seed = 12345u64;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        ((seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * 0.5
    };
    let sound: Vec<f32> = (0..80 * SAMPLES).map(|_| noise()).collect();
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES).collect();
    for hole in 55..75 {
        let mut concealer = Concealer::new(CHANNELS, RATE);
        heard(&mut concealer, &sound[..hole * SAMPLES]);
        let mut filled = vec![0.0f32; SAMPLES];
        if concealer.fill_gap(&mut filled, Some(frames[hole + 1])) {
            let around = sound[(hole - 4) * SAMPLES..(hole + 5) * SAMPLES]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()));
            let peak = filled.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(filled.iter().all(|v| v.is_finite()));
            assert!(
                peak <= 1.5 * around + 1e-6,
                "hole {hole}: {peak} against {around}"
            );
        }
    }
}

/// With nothing heard yet, or after a reset, there is nothing to continue and the caller is told
/// so (it falls back to the old way).
///
/// Verifies: REQ-AUD-070
#[test]
fn a_gap_is_not_filled_without_a_past_to_continue() {
    let sound = note(220.0, 0, 70);
    let mut concealer = Concealer::new(CHANNELS, RATE);
    let mut out = vec![0.0f32; SAMPLES];
    assert!(!concealer.fill_gap(&mut out, Some(&sound[..SAMPLES])));
    heard(&mut concealer, &sound[..60 * SAMPLES]);
    concealer.reset();
    assert!(!concealer.fill_gap(&mut out, Some(&sound[..SAMPLES])));
    assert!(!concealer.extrapolate(&mut out));
}

/// When nothing later has arrived either, the sound is predicted from the past: close to what
/// was lost on a held note, and gone within the length cap (130 ms), never a drone.
///
/// Verifies: REQ-AUD-071
#[test]
fn a_held_note_is_predicted_from_its_past_and_fades_out_within_the_cap() {
    let sound = note(196.0, 0, 200);
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES).collect();
    let mut concealer = Concealer::new(CHANNELS, RATE);
    heard(&mut concealer, &sound[..60 * SAMPLES]);

    let mut first = vec![0.0f32; SAMPLES];
    assert!(concealer.extrapolate(&mut first));
    let error = error_energy(frames[60], &first);
    assert!(
        error < 0.25 * energy(frames[60]),
        "predicted {error:.4} off a {:.4}",
        energy(frames[60])
    );

    let mut produced = FRAME;
    let mut out = vec![0.0f32; SAMPLES];
    while concealer.extrapolate(&mut out) {
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        produced += FRAME;
        assert!(
            produced <= RATE as usize * 130 / 1000 + FRAME,
            "still going after {produced} frames"
        );
    }
    assert!(
        produced >= RATE as usize * 30 / 1000,
        "gone after only {produced} frames"
    );
}

/// A prediction starts where the sound was heading: no step at the join with what was played,
/// and none where the real sound returns.
///
/// Verifies: REQ-AUD-041, REQ-AUD-071
#[test]
fn a_prediction_joins_the_sound_on_both_sides_without_a_step() {
    let sound = note(262.0, 0, 120);
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES).collect();
    let mut concealer = Concealer::new(CHANNELS, RATE);
    heard(&mut concealer, &sound[..60 * SAMPLES]);
    let mut stream: Vec<f32> = sound[..60 * SAMPLES].to_vec();
    for _ in 0..3 {
        let mut out = vec![0.0f32; SAMPLES];
        assert!(concealer.extrapolate(&mut out));
        stream.extend_from_slice(&out);
    }
    let mut returned = frames[63].to_vec();
    concealer.on_played(&mut returned);
    stream.extend_from_slice(&returned);

    // The sound itself moves by at most this between neighbouring samples
    let natural = sound
        .chunks(CHANNELS)
        .zip(sound.chunks(CHANNELS).skip(1))
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0f32, f32::max);
    let worst = stream
        .chunks(CHANNELS)
        .zip(stream.chunks(CHANNELS).skip(1))
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst <= 1.5 * natural,
        "a step of {worst} where the sound moves {natural}"
    );
}

fn buffer(on: bool) -> PlayoutBuffer {
    let mut buffer = PlayoutBuffer::new(PlayoutConfig {
        frame_samples: SAMPLES,
        target_delay_frames: 2,
        min_delay_frames: 0,
        max_delay_frames: 8,
    });
    if on {
        buffer.enable_waveform_concealment(CHANNELS, RATE);
    }
    buffer
}

/// Plays `frames` frames of `sound` through a buffer with the frames in `lost` never arriving,
/// and returns what each read said and what it produced.
fn play(
    on: bool,
    sound: &[f32],
    lost: &[u32],
    predict_starved: bool,
    stall_from: Option<u32>,
) -> Vec<(PlayoutResult, Vec<f32>)> {
    let mut buffer = buffer(on);
    buffer.set_starved_prediction(predict_starved);
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES).collect();
    let mut reads = Vec::new();
    let mut out = vec![0.0f32; SAMPLES];
    for (n, frame) in frames.iter().enumerate() {
        let n = n as u32;
        let arrives = !lost.contains(&n) && stall_from.is_none_or(|s| n < s || n >= s + 6);
        if arrives {
            buffer.write(n, frame);
        }
        let read = buffer.read_into(&mut out);
        reads.push((read.result, out[..read.samples].to_vec()));
    }
    reads
}

/// Filling does not change when anything plays: the same reads give the same results in the
/// same order and the same length, filled or not. The buffer is not made longer to wait for a
/// frame that follows a gap.
///
/// Verifies: REQ-AUD-072
#[test]
fn filling_gaps_changes_neither_the_timing_nor_the_length_of_the_reads() {
    let sound = note(220.0, 0, 150);
    let lost = [70, 71, 90, 112];
    let off = play(false, &sound, &lost, false, None);
    let on = play(true, &sound, &lost, false, None);
    assert_eq!(off.len(), on.len());
    for (n, (a, b)) in off.iter().zip(&on).enumerate() {
        assert_eq!(a.0, b.0, "read {n} came out differently");
        assert_eq!(a.1.len(), b.1.len(), "read {n} has another length");
    }
    for sequence in lost {
        assert!(
            on.iter()
                .any(|(r, _)| *r == PlayoutResult::Concealed { sequence }),
            "frame {sequence} was not met as a gap"
        );
    }
}

/// A frame that is played is left exactly as it arrived, where no gap came before it.
///
/// Verifies: REQ-AUD-072
#[test]
fn frames_with_no_gap_before_them_play_bit_for_bit() {
    let sound = note(330.0, 0, 120);
    let frames: Vec<&[f32]> = sound.chunks(SAMPLES).collect();
    let on = play(true, &sound, &[], true, None);
    for (n, (result, samples)) in on.iter().enumerate() {
        if let PlayoutResult::Played { sequence } = result {
            assert_eq!(
                samples.as_slice(),
                frames[*sequence as usize],
                "frame {n} was changed"
            );
        }
    }
}

/// A stall of the link, when nothing at all has arrived, is answered with silence as before
/// unless prediction is asked for; asked for, it is answered with sound that fades out.
///
/// Verifies: REQ-AUD-071
#[test]
fn a_stall_is_silent_unless_prediction_is_switched_on() {
    let sound = note(220.0, 0, 150);
    let quiet = play(true, &sound, &[], false, Some(80));
    let predicted = play(true, &sound, &[], true, Some(80));
    let starved = |reads: &[(PlayoutResult, Vec<f32>)]| -> Vec<f32> {
        reads
            .iter()
            .filter(|(r, _)| *r == PlayoutResult::Starved)
            .flat_map(|(_, s)| s.iter().copied())
            .collect()
    };
    let (quiet, predicted) = (starved(&quiet), starved(&predicted));
    assert!(!quiet.is_empty());
    assert!(
        quiet.iter().all(|v| *v == 0.0),
        "a stall is not silent by default"
    );
    assert!(
        predicted.iter().any(|v| v.abs() > 0.05),
        "prediction made no sound"
    );
}
