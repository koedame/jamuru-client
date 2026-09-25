//! Listening to and feeding the audio path of a running session (ADR-044).
//!
//! `debug.audio_record` records what passes one of three points, and
//! `debug.audio_tone` puts a sine wave in place of what passes one of two, so
//! that the far end of a call can measure it. The audio paths call [`observe`]
//! and [`inject`] on every frame; both cost one failed `try_lock` when nothing
//! is armed, and never wait, so a device callback is not stalled by a debugger.
//!
//! Only builds with `debug-tools` contain this module.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

/// Where in the path a frame is seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// What the input device captured, in the device's picked channels.
    Input,
    /// What is about to be sent: stereo, with volume, pan and mute applied.
    Sent,
    /// What is about to be played: stereo, after the peers are mixed.
    Output,
}

impl Point {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "input" => Some(Self::Input),
            "sent" => Some(Self::Sent),
            "output" => Some(Self::Output),
            _ => None,
        }
    }
}

/// The longest recording. The method runs at most 25 seconds (ADR-044 §4).
pub const MAX_RECORD_SECONDS: f32 = 20.0;

/// The largest WAV a recording may come back as. A frame carries 1 MiB and the
/// WAV travels as base64.
pub const MAX_WAV_BYTES: usize = 600_000;

/// A run of near-silence at least this long, between signal, is a dropout.
const DROPOUT_SECONDS: f32 = 0.002;

/// Below this a sample counts as silence.
const SILENCE: f32 = 1e-4;

/// The FFT window, the most recent samples of a channel.
const FFT_WINDOW: usize = 1 << 16;

static SAMPLE_RATE: AtomicU32 = AtomicU32::new(48_000);

static TAP: Mutex<Tap> = Mutex::new(Tap {
    recording: None,
    tone: None,
});

struct Tap {
    recording: Option<Recording>,
    tone: Option<Tone>,
}

struct Recording {
    point: Point,
    channels: usize,
    sample_rate: u32,
    want_samples: usize,
    samples: Vec<f32>,
}

struct Tone {
    point: Point,
    frequency: f32,
    amplitude: f32,
    /// Frames still to fill.
    remaining: u64,
    phase: f32,
}

/// The session's sample rate, set when streaming starts. The paths do not each
/// carry it to the call sites.
pub fn set_sample_rate(rate: u32) {
    SAMPLE_RATE.store(rate, Ordering::Relaxed);
}

/// Records `samples` (interleaved, `channels` per frame) if a recording of
/// `point` is armed.
pub fn observe(point: Point, samples: &[f32], channels: usize) {
    let Ok(mut tap) = TAP.try_lock() else {
        return;
    };
    let Some(recording) = tap.recording.as_mut() else {
        return;
    };
    if recording.point != point {
        return;
    }
    if recording.channels == 0 {
        recording.channels = channels;
        recording.want_samples *= channels;
        // Stereo was reserved when the recording was armed; more channels grow it once.
        let missing = recording
            .want_samples
            .saturating_sub(recording.samples.capacity());
        recording.samples.reserve_exact(missing);
    }
    if recording.channels != channels {
        return;
    }
    let room = recording.want_samples - recording.samples.len();
    recording
        .samples
        .extend_from_slice(&samples[..samples.len().min(room)]);
}

/// Replaces `samples` (interleaved, `channels` per frame) with the armed tone
/// if one is armed for `point`.
pub fn inject(point: Point, samples: &mut [f32], channels: usize) {
    let Ok(mut tap) = TAP.try_lock() else {
        return;
    };
    let Some(tone) = tap.tone.as_mut() else {
        return;
    };
    if tone.point != point || channels == 0 {
        return;
    }
    let step = TAU * tone.frequency / SAMPLE_RATE.load(Ordering::Relaxed) as f32;
    for frame in samples.chunks_mut(channels) {
        if tone.remaining == 0 {
            break;
        }
        let value = tone.amplitude * tone.phase.sin();
        tone.phase = (tone.phase + step) % TAU;
        tone.remaining -= 1;
        frame.fill(value);
    }
    if tone.remaining == 0 {
        tap.tone = None;
    }
}

/// Arms a tone. It ends by itself after `seconds`; a second call replaces it.
pub fn arm_tone(point: Point, frequency: f32, amplitude: f32, seconds: f32) {
    let frames = (seconds * SAMPLE_RATE.load(Ordering::Relaxed) as f32) as u64;
    let mut tap = TAP.lock().unwrap_or_else(|e| e.into_inner());
    tap.tone = Some(Tone {
        point,
        frequency,
        amplitude,
        remaining: frames,
        phase: 0.0,
    });
}

/// Records `point` for `seconds` and analyzes it. The recording starts with the
/// first frame that arrives, so what comes back is `seconds` of a running path;
/// a path that delivers nothing within the time is an error.
pub async fn record(point: Point, seconds: f32, keep_wav: bool) -> Result<Value, String> {
    {
        let mut tap = TAP.lock().unwrap_or_else(|e| e.into_inner());
        if tap.recording.is_some() {
            return Err("another recording is running".into());
        }
        let rate = SAMPLE_RATE.load(Ordering::Relaxed);
        tap.recording = Some(Recording {
            point,
            channels: 0,
            sample_rate: rate,
            // Frames until the first callback says how many channels there are.
            want_samples: (seconds * rate as f32) as usize,
            samples: Vec::with_capacity((seconds * rate as f32) as usize * 2),
        });
    }
    tokio::time::sleep(std::time::Duration::from_secs_f32(seconds + 0.5)).await;
    let recording = TAP
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .recording
        .take()
        .ok_or("the recording was lost")?;
    if recording.channels == 0 {
        return Err(format!(
            "nothing passed the {:?} point: is a session running?",
            recording.point
        ));
    }
    Ok(analyze(&recording, keep_wav))
}

fn analyze(recording: &Recording, keep_wav: bool) -> Value {
    let channels = recording.channels;
    let rate = recording.sample_rate as f32;
    let frames = recording.samples.len() / channels;
    let per_channel: Vec<Value> = (0..channels)
        .map(|channel| {
            let signal: Vec<f32> = recording
                .samples
                .iter()
                .skip(channel)
                .step_by(channels)
                .copied()
                .collect();
            let peak = signal.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let rms =
                (signal.iter().map(|s| s * s).sum::<f32>() / signal.len().max(1) as f32).sqrt();
            json!({
                "channel": channel + 1,
                "peak": peak,
                "peak_dbfs": dbfs(peak),
                "rms": rms,
                "rms_dbfs": dbfs(rms),
                "dominant_hz": dominant_frequency(&signal, rate),
                "dropouts": dropouts(&signal, rate),
            })
        })
        .collect();
    let mut result = json!({
        "point": format!("{:?}", recording.point).to_lowercase(),
        "sample_rate": recording.sample_rate,
        "channels": channels,
        "frames": frames,
        "seconds": frames as f32 / rate,
        "per_channel": per_channel,
    });
    if keep_wav {
        result["wav_base64"] = match wav(recording) {
            Some(wav) => Value::String(data_encoding::BASE64.encode(&wav)),
            None => Value::Null,
        };
    }
    result
}

fn dbfs(level: f32) -> Option<f32> {
    (level > 0.0).then(|| 20.0 * level.log10())
}

/// The number of runs of near-silence lasting [`DROPOUT_SECONDS`] or more
/// between the first and the last sample that is not silent.
fn dropouts(signal: &[f32], rate: f32) -> u32 {
    let Some(first) = signal.iter().position(|s| s.abs() > SILENCE) else {
        return 0;
    };
    let last = signal
        .iter()
        .rposition(|s| s.abs() > SILENCE)
        .unwrap_or(first);
    let shortest = (DROPOUT_SECONDS * rate) as usize;
    let (mut run, mut count) = (0usize, 0u32);
    for sample in &signal[first..=last] {
        if sample.abs() <= SILENCE {
            run += 1;
        } else {
            if run >= shortest {
                count += 1;
            }
            run = 0;
        }
    }
    count
}

/// The frequency with the most energy in the last [`FFT_WINDOW`] samples, or
/// none for silence.
fn dominant_frequency(signal: &[f32], rate: f32) -> Option<f32> {
    let take = signal.len().min(FFT_WINDOW);
    let n = take.next_power_of_two() / if take.is_power_of_two() { 1 } else { 2 };
    if n < 64 {
        return None;
    }
    let window = &signal[signal.len() - n..];
    let mut re: Vec<f32> = window
        .iter()
        .enumerate()
        .map(|(i, s)| s * (0.5 - 0.5 * (TAU * i as f32 / n as f32).cos()))
        .collect();
    let mut im = vec![0.0f32; n];
    fft(&mut re, &mut im);
    let (bin, power) =
        (1..n / 2)
            .map(|k| (k, re[k] * re[k] + im[k] * im[k]))
            .fold(
                (0, 0.0f32),
                |best, next| if next.1 > best.1 { next } else { best },
            );
    (power > 1e-9).then(|| bin as f32 * rate / n as f32)
}

/// In-place radix-2 FFT; `re.len()` is a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -TAU / len as f32;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

/// 16-bit PCM WAV of the recording, or none if it is over [`MAX_WAV_BYTES`].
fn wav(recording: &Recording) -> Option<Vec<u8>> {
    let data_bytes = recording.samples.len() * 2;
    if data_bytes + 44 > MAX_WAV_BYTES {
        return None;
    }
    let channels = recording.channels as u16;
    let mut out = Vec::with_capacity(data_bytes + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&recording.sample_rate.to_le_bytes());
    out.extend_from_slice(&(recording.sample_rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for sample in &recording.samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| 0.5 * (TAU * hz * i as f32 / rate).sin())
            .collect()
    }

    #[test]
    fn a_sine_wave_reports_its_frequency() {
        let signal = sine(1000.0, 48_000.0, 48_000);

        let found = dominant_frequency(&signal, 48_000.0).unwrap();

        assert!((found - 1000.0).abs() < 2.0, "{}", found);
    }

    #[test]
    fn silence_has_no_dominant_frequency_and_no_dropouts() {
        let signal = vec![0.0; 4800];

        assert_eq!(dominant_frequency(&signal, 48_000.0), None);
        assert_eq!(dropouts(&signal, 48_000.0), 0);
    }

    #[test]
    fn a_gap_inside_a_tone_is_one_dropout() {
        let mut signal = sine(440.0, 48_000.0, 24_000);
        signal[10_000..10_500].fill(0.0);

        assert_eq!(dropouts(&signal, 48_000.0), 1);
    }

    #[test]
    fn a_clean_tone_has_no_dropouts() {
        assert_eq!(dropouts(&sine(100.0, 48_000.0, 48_000), 48_000.0), 0);
    }

    #[test]
    fn a_wav_holds_the_samples_it_was_given() {
        let recording = Recording {
            point: Point::Sent,
            channels: 2,
            sample_rate: 48_000,
            want_samples: 4,
            samples: vec![0.5, -0.5, 1.0, -1.0],
        };

        let bytes = wav(&recording).unwrap();

        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 16383);
        assert_eq!(i16::from_le_bytes([bytes[48], bytes[49]]), 32767);
    }

    #[test]
    fn a_recording_too_big_for_a_frame_comes_back_without_a_wav() {
        let recording = Recording {
            point: Point::Input,
            channels: 2,
            sample_rate: 48_000,
            want_samples: 0,
            samples: vec![0.0; MAX_WAV_BYTES],
        };

        assert!(wav(&recording).is_none());
    }
}
