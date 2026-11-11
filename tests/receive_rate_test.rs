//! The receive buffer is kept at its target by playing a little faster or slower (ADR-122).
//!
//! A sender whose clock runs fast or slow against ours is played for a minute or two in virtual
//! time (one step is one read of the device), once as the app did before and once as it does now,
//! and the skips, gaps and depth of the buffer are put next to each other. The pitch that comes out
//! is measured over the whole run, the stretches included.

use std::f64::consts::PI;

use jamjam::audio::{encode_packed_exact, CodecType, PlayoutResult, ReceivePath};

const RATE: u32 = 48_000;

/// What a run of the receive path came to
#[derive(Debug, Default)]
struct Run {
    trimmed: u64,
    concealed: u64,
    starved: u64,
    resyncs: u64,
    removals: u64,
    insertions: u64,
    /// The shallowest and deepest the buffer was after the first twenty seconds, in frames
    shallowest: usize,
    deepest: usize,
    /// The delay the buffer ended at, in frames
    target: u32,
    /// What the left channel played, every read
    played: Vec<f32>,
    /// The rate asked for at the end, in ppm
    ppm: f32,
}

/// Tones the sender plays, in Hz, with the amplitude of each
struct Tones(&'static [(f64, f64)]);

const A_SINE: Tones = Tones(&[(1000.0, 0.5)]);
/// C major: C4, E4, G4
const A_CHORD: Tones = Tones(&[(261.63, 0.2), (329.63, 0.2), (392.0, 0.2)]);

impl Tones {
    fn at(&self, n: u64) -> f32 {
        self.0
            .iter()
            .map(|(hz, amplitude)| amplitude * (2.0 * PI * hz * n as f64 / f64::from(RATE)).sin())
            .sum::<f64>() as f32
    }
}

/// Plays `seconds` of `tones` from a sender whose clock runs `skew_ppm` faster than ours (negative:
/// slower), through a receive path with `frame` sample frames per packet and the delay `delays`
/// ask for: frames from a second on, the first from second 0. Packets are late by a pseudo-random `jitter` of a frame at most.
fn run(
    skew_ppm: f64,
    follow: bool,
    tones: &Tones,
    seconds: f64,
    frame: u32,
    delays: &[(f64, u32)],
    jitter: f64,
) -> Run {
    let path = ReceivePath::new(CodecType::Pcm, RATE, frame, delays[0].1).expect("PCM");
    path.set_rate_following(follow);
    let frame_secs = f64::from(frame) / f64::from(RATE);
    let send_secs = frame_secs / (1.0 + skew_ppm * 1e-6);
    let reads = (seconds / frame_secs) as u64;

    let mut out = vec![0.0f32; path.widest_frame_samples()];
    let mut held: Vec<f32> = Vec::new();
    let mut next_packet = 0u64;
    let mut seed = 12345u64;
    // The packets on their way: (arrival time, sequence)
    let mut flight: Vec<(f64, u64)> = Vec::new();
    let mut run = Run {
        shallowest: usize::MAX,
        ..Run::default()
    };

    for read in 0..reads {
        let now = (read as f64 + 0.5) * frame_secs;
        for (from, frames) in delays.iter().skip(1) {
            if (now - frame_secs..now).contains(from) {
                path.set_delay_frames(*frames);
            }
        }
        while next_packet as f64 * send_secs <= now {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let late = (seed >> 33) as f64 / (1u64 << 31) as f64 * jitter * frame_secs;
            flight.push((next_packet as f64 * send_secs + late, next_packet));
            next_packet += 1;
        }
        flight.sort_by(|a, b| a.0.total_cmp(&b.0));
        let arrived = flight.partition_point(|(time, _)| *time <= now);
        for (_, sequence) in flight.drain(..arrived) {
            let samples: Vec<f32> = (0..frame as u64)
                .flat_map(|i| {
                    let value = tones.at(sequence * u64::from(frame) + i);
                    [value, value]
                })
                .collect();
            let mut bytes = Vec::new();
            encode_packed_exact(&samples, 2, &mut bytes);
            path.receive(sequence as u32, &bytes);
        }

        // The device takes a frame of ours at a time; the buffer is asked for another only
        // when what was taken from it is used up, as the mix of the room does
        let mut result = None;
        while held.len() < frame as usize * 2 {
            let read = path.read_into(&mut out);
            if read.samples == 0 {
                break;
            }
            held.extend_from_slice(&out[..read.samples]);
            result = Some(read.result);
        }
        run.played.extend(
            held.drain(..(frame as usize * 2).min(held.len()))
                .step_by(2),
        );
        if matches!(result, Some(PlayoutResult::Played { .. })) && now > 20.0 {
            let depth = path.held_frames();
            run.shallowest = run.shallowest.min(depth);
            run.deepest = run.deepest.max(depth);
        }
        if read % (1.0 / frame_secs) as u64 == 0 && read > 0 {
            path.adapt();
        }
    }

    let stats = path.stats();
    run.trimmed = stats.frames_trimmed;
    run.concealed = stats.frames_concealed;
    run.starved = stats.frames_starved;
    run.resyncs = stats.resyncs;
    run.removals = path.stretch_stats().removals;
    run.insertions = path.stretch_stats().insertions;
    run.target = path.delay_frames();
    run.ppm = path.rate_ppm();
    run
}

/// The pitch of `tone_hz` in `played` over each window of `window` seconds, every `hop` seconds
/// after `from` seconds, as cents away from `tone_hz`: from how far the phase of the tone turns
/// between the two halves of the window
fn pitch_cents(played: &[f32], tone_hz: f64, from: f64, window: f64, hop: f64) -> Vec<f64> {
    let rate = f64::from(RATE);
    let half = (window * rate / 2.0) as usize;
    let phase = |start: usize| -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for i in 0..half {
            let weight = 0.5 - 0.5 * (2.0 * PI * (i as f64 + 0.5) / half as f64).cos();
            let angle = 2.0 * PI * tone_hz * (start + i) as f64 / rate;
            re += weight * f64::from(played[start + i]) * angle.cos();
            im -= weight * f64::from(played[start + i]) * angle.sin();
        }
        im.atan2(re)
    };
    let mut cents = Vec::new();
    let mut start = (from * rate) as usize;
    while start + 2 * half <= played.len() {
        let mut turn = phase(start + half) - phase(start);
        while turn > PI {
            turn -= 2.0 * PI;
        }
        while turn < -PI {
            turn += 2.0 * PI;
        }
        // The tone at tone_hz turns by 2 pi tone_hz half / rate between the halves on its own;
        // phase() has taken that out
        let hz = tone_hz + turn / (2.0 * PI * half as f64 / rate);
        cents.push(1200.0 * (hz / tone_hz).log2());
        start += (hop * rate) as usize;
    }
    cents
}

fn largest(cents: &[f64]) -> f64 {
    cents.iter().fold(0.0f64, |worst, c| worst.max(c.abs()))
}

/// Verifies: REQ-LAT-134
#[test]
fn when_the_senders_clock_is_off_the_buffer_is_held_at_its_target_without_a_skip_or_a_gap() {
    for skew in [-500.0, -100.0, 100.0, 500.0] {
        let before = run(skew, false, &A_SINE, 90.0, 64, &[(0.0, 2)], 0.0);
        let after = run(skew, true, &A_SINE, 90.0, 64, &[(0.0, 2)], 0.0);
        eprintln!(
            "{skew:+} ppm  before: trimmed {} concealed {} starved {} resyncs {} target {} depth {}..{}   \
             after: trimmed {} concealed {} starved {} resyncs {} target {} depth {}..{} \
             stretches {}/{} rate {:.0} ppm",
            before.trimmed, before.concealed, before.starved, before.resyncs, before.target,
            before.shallowest, before.deepest,
            after.trimmed, after.concealed, after.starved, after.resyncs, after.target,
            after.shallowest, after.deepest, after.removals, after.insertions, after.ppm,
        );
        assert_eq!(after.trimmed, 0, "{skew} ppm: no skip");
        assert_eq!(after.concealed + after.starved, 0, "{skew} ppm: no gap");
        assert_eq!(after.resyncs, 0, "{skew} ppm");
        assert!(
            after.deepest <= after.target as usize + 3
                && after.shallowest + 2 >= after.target as usize,
            "{skew} ppm: depth {}..{} against a target of {}",
            after.shallowest,
            after.deepest,
            after.target
        );
    }
}

/// Verifies: REQ-LAT-134
#[test]
fn a_sine_and_a_chord_never_change_pitch_by_more_than_two_cents_with_the_clock_off() {
    for skew in [-500.0, -100.0, 100.0, 500.0] {
        let sine = run(skew, true, &A_SINE, 60.0, 64, &[(0.0, 2)], 0.0);
        // The sender's own clock being off moves the pitch by that much before anything is done
        // to it; what is measured is the pitch against the one the sender meant
        let cents = pitch_cents(&sine.played, 1000.0, 5.0, 0.050, 0.050);
        eprintln!(
            "{skew:+} ppm 1 kHz: largest {:.2} cents over {} windows",
            largest(&cents),
            cents.len()
        );
        assert!(
            largest(&cents) <= 2.0,
            "{skew} ppm sine: {:.2}",
            largest(&cents)
        );

        let chord = run(skew, true, &A_CHORD, 60.0, 64, &[(0.0, 2)], 0.0);
        for (hz, _) in A_CHORD.0 {
            let cents = pitch_cents(&chord.played, *hz, 5.0, 0.200, 0.050);
            eprintln!("{skew:+} ppm {hz} Hz: largest {:.2} cents", largest(&cents));
            assert!(
                largest(&cents) <= 2.0,
                "{skew} ppm {hz} Hz: {:.2}",
                largest(&cents)
            );
        }
    }
}

/// Verifies: REQ-LAT-134
#[test]
fn when_the_delay_is_lowered_the_buffer_drains_by_playing_faster_without_a_skip_or_a_gap() {
    // From 6 frames to 3 at ten seconds: 3 frames of 64 are 4 ms, far inside what is given back by
    // playing faster. A skip would show as `trimmed`, a gap as `concealed` or `starved`
    let before = run(0.0, false, &A_SINE, 40.0, 64, &[(0.0, 6), (10.0, 3)], 0.0);
    eprintln!(
        "lowered 6 -> 3, as before: trimmed {} concealed {} starved {} target {} depth {}..{}",
        before.trimmed,
        before.concealed,
        before.starved,
        before.target,
        before.shallowest,
        before.deepest
    );
    let run = run(0.0, true, &A_SINE, 40.0, 64, &[(0.0, 6), (10.0, 3)], 0.0);
    eprintln!(
        "lowered 6 -> 3: trimmed {} concealed {} starved {} resyncs {} target {} depth {}..{} \
         stretches {}/{}",
        run.trimmed,
        run.concealed,
        run.starved,
        run.resyncs,
        run.target,
        run.shallowest,
        run.deepest,
        run.removals,
        run.insertions
    );
    assert_eq!(run.trimmed, 0, "no skip");
    assert_eq!(run.concealed + run.starved, 0, "no gap");
    assert!(run.removals > 0, "it played faster to drain");
    assert!(
        run.deepest <= run.target as usize + 3,
        "depth {}..{} against a target of {}",
        run.shallowest,
        run.deepest,
        run.target
    );
    let cents = pitch_cents(&run.played, 1000.0, 5.0, 0.050, 0.050);
    eprintln!("lowered 6 -> 3: largest {:.2} cents", largest(&cents));
    assert!(largest(&cents) <= 2.0, "{:.2}", largest(&cents));
}

/// Verifies: REQ-LAT-134
#[test]
fn when_packets_arrive_unevenly_and_the_clock_is_off_it_skips_and_drops_out_no_more_than_before() {
    for (skew, jitter) in [(100.0, 0.5), (-100.0, 0.5), (300.0, 1.0), (-300.0, 1.0)] {
        let before = run(skew, false, &A_SINE, 90.0, 64, &[(0.0, 3)], jitter);
        let after = run(skew, true, &A_SINE, 90.0, 64, &[(0.0, 3)], jitter);
        eprintln!(
            "{skew:+} ppm, packets late by up to {jitter} frame: \
             before: trimmed {} concealed {} starved {} target {}   \
             after: trimmed {} concealed {} starved {} target {} rate {:.0} ppm",
            before.trimmed,
            before.concealed,
            before.starved,
            before.target,
            after.trimmed,
            after.concealed,
            after.starved,
            after.target,
            after.ppm,
        );
        assert!(
            after.trimmed + after.concealed + after.starved
                <= before.trimmed + before.concealed + before.starved,
            "{skew} ppm: after {} {} {}, before {} {} {}",
            after.trimmed,
            after.concealed,
            after.starved,
            before.trimmed,
            before.concealed,
            before.starved,
        );
    }
}

/// Tones of whole cycles to a 64-sample frame, as a room of three plays them
const TWO_OTHERS: Tones = Tones(&[(3000.0, 0.35), (6000.0, 0.35)]);

fn bin_amplitude(samples: &[f32], hz: f64) -> f32 {
    let step = 2.0 * PI * hz / f64::from(RATE);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, sample) in samples.iter().enumerate() {
        re += f64::from(*sample) * (step * i as f64).cos();
        im -= f64::from(*sample) * (step * i as f64).sin();
    }
    (2.0 * (re * re + im * im).sqrt() / samples.len() as f64) as f32
}

/// Verifies: REQ-LAT-134
#[test]
fn when_packets_arrive_unevenly_a_tone_that_is_not_played_stays_out_of_the_output() {
    for (skew, jitter) in [
        (0.0, 1.0),
        (100.0, 1.0),
        (-100.0, 1.0),
        (0.0, 3.0),
        (-300.0, 3.0),
        (300.0, 3.0),
    ] {
        for follow in [false, true] {
            let r = run(skew, follow, &TWO_OTHERS, 30.0, 64, &[(0.0, 2)], jitter);
            let worst = r.played[48_000 * 5..]
                .as_chunks::<64>()
                .0
                .iter()
                .map(|c| bin_amplitude(c, 1500.0))
                .fold(0.0f32, f32::max);
            if follow {
                assert!(
                    worst < 0.02,
                    "{skew} ppm, late by up to {jitter} frame: a tone that is not played came out at {worst}"
                );
            }
        }
    }
}
