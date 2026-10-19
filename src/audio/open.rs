//! Opening the devices of an audio session: playback and capture on the channels chosen, and the
//! record of the devices that would not open.
//!
//! Shared by the app and the CLI, which open their devices the same way (ADR-077). Everything
//! that runs on an audio callback allocates nothing and never waits.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use rtrb::{Consumer, RingBuffer};
use serde::Serialize;

use super::mic_access::{MicAccess, MicSystem};
use super::taps::taps;
use super::{
    capture_attempts, input_devices, output_devices, pair_in_use, rms_level, AudioEngine,
    AudioError, BurstProbe, DeviceId, Devices, LocalMonitor, MonitorTap, OutputRoute, PeerMix,
    RecordingFeed, RecordingTap,
};
#[cfg(target_os = "windows")]
use super::{AsioDuplex, OPEN_TIMEOUT};

/// Which way audio goes through a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSide {
    Input,
    Output,
}

/// Why a device is not in use
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTrouble {
    /// The driver did not answer in time (REQ-AUD-123)
    Unresponsive,
    /// The device refused to open
    Failed,
    /// The device does not open at the session's sample rate, and no rate it
    /// offers helped. Its format can be changed in the system's sound settings.
    UnsupportedFormat,
    /// The stream is open but its callback is no longer called, and opening it
    /// again did not bring it back (REQ-AUD-128)
    Stalled,
    /// The system is asking whether the app may use the microphone, and the
    /// answer has not come. The input is not opened until it does: opening it
    /// waits on the same question and stalls the output with it.
    PermissionPending,
    /// The system does not let the app use the microphone. Changed in its
    /// privacy settings.
    PermissionRefused,
}

/// A device the audio thread could not open, for the UI to tell the user
///
/// Published by the audio thread and read by the IPC layer. Cleared when the
/// device of that side opens, and when a session starts or ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceProblem {
    pub side: DeviceSide,
    pub trouble: DeviceTrouble,
    /// The device's name, when the system's list has one for it
    pub device: Option<String>,
    /// The sample rate the session asked the device to open at
    pub sample_rate: u32,
}

/// Records that the `side` device could not be opened, replacing what was
/// recorded for that side
pub fn note_device_problem(
    problems: &RwLock<Vec<DeviceProblem>>,
    side: DeviceSide,
    device: Option<&DeviceId>,
    sample_rate: u32,
    error: &AudioError,
) {
    let trouble = match error {
        AudioError::DeviceUnresponsive(_) => DeviceTrouble::Unresponsive,
        AudioError::UnsupportedConfig(_) => DeviceTrouble::UnsupportedFormat,
        _ => DeviceTrouble::Failed,
    };
    note_trouble(problems, side, device, sample_rate, trouble);
}

/// Records `trouble` with the `side` device, replacing what was recorded for
/// that side
pub fn note_trouble(
    problems: &RwLock<Vec<DeviceProblem>>,
    side: DeviceSide,
    device: Option<&DeviceId>,
    sample_rate: u32,
    trouble: DeviceTrouble,
) {
    let listed = match side {
        DeviceSide::Input => input_devices(),
        DeviceSide::Output => output_devices(),
    }
    .unwrap_or_default();
    let name = device
        .and_then(|id| listed.iter().find(|d| d.id == *id))
        .or_else(|| {
            listed
                .iter()
                .find(|d| d.is_default)
                .filter(|_| device.is_none())
        })
        .map(|d| d.name.clone());
    if let Ok(mut problems) = problems.write() {
        problems.retain(|p| p.side != side);
        problems.push(DeviceProblem {
            side,
            trouble,
            device: name,
            sample_rate,
        });
    }
}

/// How long a session start waits for the person to answer the system's
/// question about the microphone
pub const MIC_ANSWER_WAIT: Duration = Duration::from_secs(60);

/// Whether the input may be opened now, asking the system about the
/// microphone first and telling the screen what is being waited for.
///
/// `false` means the input stays closed and `problems` says why: the person
/// has not answered within `wait` (it stays on screen as pending), or the
/// system does not let the app use the microphone. The output does not
/// depend on any of this and is opened either way.
pub fn microphone_allows_input(
    mic: &impl MicSystem,
    problems: &RwLock<Vec<DeviceProblem>>,
    sample_rate: u32,
    wait: Duration,
) -> bool {
    let mut access = mic.status();
    let asked = access == MicAccess::Undecided;
    if asked {
        tracing::info!("Waiting for the answer to the system's microphone question");
        note_permission(problems, DeviceTrouble::PermissionPending, sample_rate);
        access = mic.ask(wait);
    }
    match access {
        MicAccess::Granted => {
            if asked {
                clear_device_problem(problems, DeviceSide::Input);
            }
            true
        }
        MicAccess::Refused => {
            tracing::warn!("The system does not let the app use the microphone");
            note_permission(problems, DeviceTrouble::PermissionRefused, sample_rate);
            false
        }
        MicAccess::Undecided => {
            tracing::warn!("The microphone question was not answered within {:?}", wait);
            false
        }
    }
}

/// Records a microphone permission trouble for the input. The device's name
/// is left out: the system's list is behind the same question.
fn note_permission(
    problems: &RwLock<Vec<DeviceProblem>>,
    trouble: DeviceTrouble,
    sample_rate: u32,
) {
    if let Ok(mut problems) = problems.write() {
        problems.retain(|p| p.side != DeviceSide::Input);
        problems.push(DeviceProblem {
            side: DeviceSide::Input,
            trouble,
            device: None,
            sample_rate,
        });
    }
}

pub fn clear_device_problem(problems: &RwLock<Vec<DeviceProblem>>, side: DeviceSide) {
    if let Ok(mut problems) = problems.write() {
        problems.retain(|p| p.side != side);
    }
}

/// The frame source the output callback pulls from.
///
/// Runs on the audio callback, so it allocates nothing and never waits: a
/// contended buffer costs one concealed frame rather than a stalled device.
/// The mixer gains are applied here rather than on the way in, so a volume or
/// pan change is heard on the very next frame.
pub fn playout_source(
    mix: PeerMix,
    master_volume: Arc<AtomicU32>,
    output_level: Arc<AtomicU32>,
    monitor: LocalMonitor,
    recording: Option<(RecordingTap, RecordingTap)>,
    probe: Option<Arc<BurstProbe>>,
) -> impl FnMut(&mut [f32]) -> usize + Send + 'static {
    let mut recording = recording;
    // Everyone else as they sent it, before the listener's faders and pans:
    // what recording/streaming output `B` carries
    let mut dry = vec![0.0f32; mix.frame_samples()];
    move |out: &mut [f32]| {
        let (b_tap, c_tap) = match recording.as_mut() {
            Some((b, c)) => (Some(b), Some(c)),
            None => (None, None),
        };
        let samples = mix.mix_into(
            out,
            master_volume.load(Ordering::Relaxed),
            b_tap.is_some().then_some(&mut dry[..]),
        );
        if let Some(tap) = b_tap {
            tap.push_stereo(&dry[..samples]);
        }
        output_level.store(rms_level(&out[..samples]), Ordering::Relaxed);
        // Before the monitor mix: what the peer sent, not our own input.
        if let Some(probe) = &probe {
            probe.note_played(&out[..samples]);
        }
        // Added after the peers, whatever they did this frame: hearing yourself
        // must not wait for, or depend on, anyone being connected (ADR-033).
        monitor.mix_into(&mut out[..samples]);
        // C is what is actually heard: post-fader, self included.
        if let Some(tap) = c_tap {
            tap.push_stereo(&out[..samples]);
        }
        if let Some(taps) = taps() {
            taps.output(&mut out[..samples]);
        }
        samples
    }
}

/// Starts playback on `device_id`, on the output channels `(left, right)` the
/// user selected (1-based). `make_source` builds the frame source, once per
/// attempt.
///
/// A device that has no such channels is played on its first ones
/// (`pair_in_use`), so a stale setting never leaves a session without sound.
/// The saved pair is not touched.
pub fn start_playout<S>(
    engine: &mut AudioEngine,
    device_id: Option<&DeviceId>,
    saved: (u32, Option<u32>),
    frame_samples: usize,
    make_source: impl Fn() -> S,
) -> Result<(), AudioError>
where
    S: FnMut(&mut [f32]) -> usize + Send + 'static,
{
    let chosen = device_id.map(|id| id.0.clone());
    let (left, right) = pair_in_use(&Devices::list().output, &chosen, saved);
    let selected = OutputRoute::from_settings(left, right);
    engine.set_playback_route(selected);
    match engine.start_playback_with_source(device_id, frame_samples, make_source()) {
        Err(e) if !selected.is_default() => {
            tracing::warn!(
                "Playback on {:?} to device channel(s) {:?} failed: {}",
                device_id,
                selected,
                e
            );
            engine.set_playback_route(OutputRoute::default());
            engine.start_playback_with_source(device_id, frame_samples, make_source())
        }
        result => result,
    }
}

/// Captured audio waiting to be sent, and how many interleaved channels each
/// frame of it has.
pub struct CaptureRing {
    pub consumer: Consumer<f32>,
    pub channels: usize,
}

impl CaptureRing {
    /// A ring nothing is ever written to, for a session whose input did not open
    pub fn silent(channels: usize) -> Self {
        let (_, consumer) = RingBuffer::<f32>::new(1);
        Self { consumer, channels }
    }
}

/// Throws away what is waiting in a capture ring.
pub fn discard_queued(consumer: &mut Consumer<f32>) {
    let queued = consumer.slots();
    if let Ok(chunk) = consumer.read_chunk(queued) {
        chunk.commit_all();
    }
}

/// Delivers one just-captured, already channel-picked frame: feeds the "hear
/// yourself" monitor, updates the input level meter, and queues it for the
/// network send thread. Shared between the two capture backends - `cpal`'s
/// ring-buffer callback and ASIO's `bufferSwitch` - which hand over samples
/// the same way (interleaved, already reduced to the picked channels).
#[allow(clippy::too_many_arguments)]
pub fn on_captured_frame(
    samples: &[f32],
    channels: usize,
    producer: &mut rtrb::Producer<f32>,
    monitor_tap: &mut MonitorTap,
    level: &AtomicU32,
    recording_a: Option<&mut RecordingTap>,
) {
    monitor_tap.push_interleaved(samples, channels);
    if let Some(taps) = taps() {
        taps.input(samples, channels);
    }
    // A is dry: no fader/pan applied, whatever the transmit channel count.
    if let Some(tap) = recording_a {
        if channels >= 2 {
            tap.push_stereo(samples);
        } else {
            tap.push_mono(samples);
        }
    }
    // Calculate RMS level (0-100)
    if !samples.is_empty() {
        level.store(rms_level(samples), Ordering::SeqCst);
    }
    if let Ok(mut chunk) = producer.write_chunk_uninit(samples.len()) {
        let slices = chunk.as_mut_slices();
        // Copy samples directly to ring buffer slices
        let first_len = slices.0.len().min(samples.len());
        for (i, &sample) in samples[..first_len].iter().enumerate() {
            slices.0[i].write(sample);
        }
        if first_len < samples.len() {
            for (i, &sample) in samples[first_len..].iter().enumerate() {
                slices.1[i].write(sample);
            }
        }
        // SAFETY: we just initialized all elements
        unsafe {
            chunk.commit_all();
        }
    }
    // If buffer full, samples are dropped (backpressure)
}

/// Starts capture on `device_id` with `wanted` channels (1 or 2), taken from
/// the input channels `(left, right)` the user selected (1-based), and returns
/// the ring the captured audio lands in.
///
/// A device that has no such channels is opened with its first ones instead
/// (`pair_in_use`; a mono microphone gets its one channel, which is captured
/// as mono), and one that will not open with two falls back to its first
/// channel, so a stale setting never leaves a session without input. The
/// saved pair is not touched. The ring reports how many channels it actually
/// has.
#[allow(clippy::too_many_arguments)]
pub fn start_capture_ring(
    engine: &mut AudioEngine,
    device_id: Option<&DeviceId>,
    wanted: u16,
    selected: (u32, Option<u32>),
    frame_size: usize,
    input_level: &Arc<AtomicU32>,
    monitor: &LocalMonitor,
    recording: Option<&RecordingFeed>,
) -> Result<CaptureRing, AudioError> {
    let chosen = device_id.map(|id| id.0.clone());
    let (left, right) = pair_in_use(&Devices::list().input, &chosen, selected);
    let mut failure = AudioError::DeviceNotFound("no way to open the input was tried".into());
    for picks in capture_attempts(left, right, wanted) {
        let channels = picks.len() as u16;
        let (mut producer, consumer) = RingBuffer::<f32>::new(32 * frame_size * channels as usize);
        let mut monitor_tap = monitor.tap();
        let mut recording_a = recording.map(|feed| feed.tap_a());
        let level = input_level.clone();

        engine.stop_capture();
        engine.set_capture_picks(picks.clone());
        // Zero-allocation: write directly to the rtrb producer (FnMut, no Sync needed)
        let started = engine.start_capture(device_id, move |samples, _timestamp| {
            on_captured_frame(
                samples,
                channels as usize,
                &mut producer,
                &mut monitor_tap,
                &level,
                recording_a.as_mut(),
            );
        });
        match started {
            Ok(()) => {
                tracing::info!(
                    "Capture started on {:?} from device channel(s) {:?}",
                    device_id,
                    picks
                );
                return Ok(CaptureRing {
                    consumer,
                    channels: channels as usize,
                });
            }
            Err(e) => {
                tracing::warn!(
                    "Capture on {:?} from device channel(s) {:?} failed: {}",
                    device_id,
                    picks,
                    e
                );
                // Another way of opening a driver that does not answer would
                // wait for the same silence again
                let unresponsive = matches!(e, AudioError::DeviceUnresponsive(_));
                failure = e;
                if unresponsive {
                    break;
                }
            }
        }
    }
    Err(failure)
}

/// The ring the session sends from: the one the input opened, or, when it
/// did not open, one that stays empty. What others say is still heard, so the
/// session goes on without sending, and the screen says why the input is not
/// in use.
pub fn capture_or_silence(
    opened: Result<CaptureRing, AudioError>,
    wanted_channels: u16,
    device_problems: &RwLock<Vec<DeviceProblem>>,
    device: Option<&DeviceId>,
    sample_rate: u32,
) -> CaptureRing {
    match opened {
        Ok(ring) => ring,
        Err(e) => {
            tracing::warn!("The session goes on without input: {}", e);
            note_device_problem(device_problems, DeviceSide::Input, device, sample_rate, &e);
            CaptureRing::silent(wanted_channels as usize)
        }
    }
}

/// Opens `driver_name` for both directions at once: what `start_capture_ring`
/// and `start_playout` do together, through a single [`AsioDuplex`] session
/// instead of two `cpal` streams (Windows only, see `audio::asio`).
///
/// Picks and route come from the same settings the two-engine path uses
/// (`pair_in_use`), fit to the driver's reported channels the same way -
/// `asio::list_drivers` is what the settings screen's channel counts for an
/// ASIO device come from in the first place, so unlike `start_capture_ring`
/// this does not retry with narrower channels when they don't fit: a channel
/// setting that does not fit the driver's own reported channels is a stale
/// setting to report, not a device that might still open some other way.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
pub fn open_asio(
    driver_name: &str,
    input_id: Option<&DeviceId>,
    output_id: Option<&DeviceId>,
    sample_rate: u32,
    frame_size: u32,
    frame_samples: usize,
    wanted_channels: u16,
    input_channels: (u32, Option<u32>),
    output_channels: (u32, Option<u32>),
    input_level: Arc<AtomicU32>,
    monitor: LocalMonitor,
    mix: PeerMix,
    master_volume: Arc<AtomicU32>,
    output_level: Arc<AtomicU32>,
    recording: Option<&RecordingFeed>,
) -> Result<(AsioDuplex, CaptureRing), AudioError> {
    let chosen_in = input_id.map(|id| id.0.clone());
    let (in_left, in_right) = pair_in_use(&Devices::list().input, &chosen_in, input_channels);
    let picks = capture_attempts(in_left, in_right, wanted_channels)
        .into_iter()
        .next()
        .unwrap_or_default();

    let chosen_out = output_id.map(|id| id.0.clone());
    let (out_left, out_right) = pair_in_use(&Devices::list().output, &chosen_out, output_channels);
    let route = OutputRoute::from_settings(out_left, out_right);

    let channels = picks.len().max(1);
    let (mut producer, consumer) = RingBuffer::<f32>::new(32 * frame_size as usize * channels);
    let mut monitor_tap = monitor.tap();
    let mut recording_a = recording.map(|feed| feed.tap_a());
    let level = input_level;

    let session = AsioDuplex::start(
        driver_name,
        sample_rate,
        frame_size,
        frame_samples,
        OPEN_TIMEOUT,
        picks,
        route,
        move |samples, _timestamp| {
            on_captured_frame(
                samples,
                channels,
                &mut producer,
                &mut monitor_tap,
                &level,
                recording_a.as_mut(),
            );
        },
        playout_source(
            mix,
            master_volume,
            output_level,
            monitor,
            recording.map(|feed| (feed.tap_b(), feed.tap_c())),
            None,
        ),
    )?;

    Ok((session, CaptureRing { consumer, channels }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;
    use crate::audio::{AudioPreset, PeerGain, PeerMeter, ReceivePath, VoiceHandle};
    use crate::network::MAX_PEERS_PER_ROOM;

    /// A system whose microphone question is in the given state, and which
    /// answers `answer` when asked (`Undecided` for a person who does not)
    struct FakeMic {
        status: MicAccess,
        answer: MicAccess,
        asked: std::sync::atomic::AtomicBool,
    }

    impl FakeMic {
        fn new(status: MicAccess, answer: MicAccess) -> Self {
            Self {
                status,
                answer,
                asked: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn was_asked(&self) -> bool {
            self.asked.load(Ordering::SeqCst)
        }
    }

    impl MicSystem for FakeMic {
        fn status(&self) -> MicAccess {
            self.status
        }

        fn ask(&self, _wait: Duration) -> MicAccess {
            self.asked.store(true, Ordering::SeqCst);
            self.answer
        }
    }

    fn input_trouble(problems: &RwLock<Vec<DeviceProblem>>) -> Option<DeviceTrouble> {
        problems
            .read()
            .unwrap()
            .iter()
            .find(|p| p.side == DeviceSide::Input)
            .map(|p| p.trouble)
    }

    /// Verifies: REQ-AUD-129
    #[test]
    fn when_the_system_already_allows_the_microphone_the_input_opens_without_asking() {
        let mic = FakeMic::new(MicAccess::Granted, MicAccess::Granted);
        let problems = RwLock::new(Vec::new());

        let allowed = microphone_allows_input(&mic, &problems, 48000, Duration::from_secs(1));

        assert!(allowed);
        assert!(!mic.was_asked());
        assert_eq!(input_trouble(&problems), None);
    }

    /// Verifies: REQ-AUD-129
    #[test]
    fn when_the_person_has_not_been_asked_yet_the_input_opens_after_they_allow_it() {
        let mic = FakeMic::new(MicAccess::Undecided, MicAccess::Granted);
        let problems = RwLock::new(Vec::new());

        let allowed = microphone_allows_input(&mic, &problems, 48000, Duration::from_secs(1));

        assert!(allowed);
        assert!(mic.was_asked());
        assert_eq!(input_trouble(&problems), None);
    }

    /// Verifies: REQ-AUD-129
    #[test]
    fn when_the_person_declines_the_question_the_input_stays_closed_and_the_screen_says_so() {
        let mic = FakeMic::new(MicAccess::Undecided, MicAccess::Refused);
        let problems = RwLock::new(Vec::new());

        let allowed = microphone_allows_input(&mic, &problems, 48000, Duration::from_secs(1));

        assert!(!allowed);
        assert_eq!(
            input_trouble(&problems),
            Some(DeviceTrouble::PermissionRefused)
        );
    }

    /// Verifies: REQ-AUD-129
    #[test]
    fn when_the_microphone_is_switched_off_for_the_app_the_input_is_not_opened_or_asked_about() {
        let mic = FakeMic::new(MicAccess::Refused, MicAccess::Refused);
        let problems = RwLock::new(Vec::new());

        let allowed = microphone_allows_input(&mic, &problems, 48000, Duration::from_secs(1));

        assert!(!allowed);
        assert!(!mic.was_asked());
        assert_eq!(
            input_trouble(&problems),
            Some(DeviceTrouble::PermissionRefused)
        );
    }

    /// Verifies: REQ-AUD-129
    #[test]
    fn when_nobody_answers_in_time_the_input_stays_closed_and_the_screen_says_it_is_waiting() {
        let mic = FakeMic::new(MicAccess::Undecided, MicAccess::Undecided);
        let problems = RwLock::new(Vec::new());

        let allowed = microphone_allows_input(&mic, &problems, 48000, Duration::from_millis(10));

        assert!(!allowed);
        assert_eq!(
            input_trouble(&problems),
            Some(DeviceTrouble::PermissionPending)
        );
    }

    /// Verifies: REQ-AUD-129
    #[test]
    fn when_the_answer_comes_after_the_screen_said_it_was_waiting_the_notice_goes_away() {
        let mic = FakeMic::new(MicAccess::Undecided, MicAccess::Granted);
        let problems = RwLock::new(Vec::new());
        note_permission(&problems, DeviceTrouble::PermissionPending, 48000);

        let allowed = microphone_allows_input(&mic, &problems, 48000, Duration::from_secs(1));

        assert!(allowed);
        assert_eq!(input_trouble(&problems), None);
    }

    /// Verifies: REQ-AUD-125
    #[test]
    fn when_the_input_does_not_open_at_the_session_rate_the_session_goes_on_and_says_which_rate() {
        let problems = RwLock::new(Vec::new());
        let refused = AudioError::UnsupportedConfig("Failed to initialize audio client".into());

        let ring = capture_or_silence(Err(refused), 2, &problems, None, 48000);

        assert_eq!(ring.channels, 2);
        assert_eq!(
            ring.consumer.slots(),
            0,
            "nothing is sent from a closed input"
        );
        let problems = problems.read().unwrap();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].side, DeviceSide::Input);
        assert_eq!(problems[0].trouble, DeviceTrouble::UnsupportedFormat);
        assert_eq!(problems[0].sample_rate, 48000);
    }

    /// Verifies: REQ-AUD-125
    #[test]
    fn when_the_input_refuses_for_any_other_reason_the_session_goes_on_too() {
        let problems = RwLock::new(Vec::new());
        let refused = AudioError::StreamError("device busy".into());

        let ring = capture_or_silence(Err(refused), 1, &problems, None, 48000);

        assert_eq!(ring.channels, 1);
        assert_eq!(problems.read().unwrap()[0].trouble, DeviceTrouble::Failed);
    }

    /// Verifies: REQ-AUD-125
    #[test]
    fn when_the_input_opens_nothing_is_reported() {
        let problems = RwLock::new(Vec::new());
        let (_, consumer) = RingBuffer::<f32>::new(8);

        let ring = capture_or_silence(
            Ok(CaptureRing {
                consumer,
                channels: 2,
            }),
            2,
            &problems,
            None,
            48000,
        );

        assert_eq!(ring.channels, 2);
        assert!(problems.read().unwrap().is_empty());
    }

    /// Verifies: REQ-AUD-035
    #[test]
    fn audio_captured_before_the_connection_is_not_sent_once_it_is_up() {
        let (mut producer, mut consumer) = RingBuffer::<f32>::new(8);
        for sample in [1.0_f32, 2.0, 3.0, 4.0] {
            producer.push(sample).unwrap();
        }

        discard_queued(&mut consumer);
        assert_eq!(consumer.slots(), 0, "the old audio is gone");

        producer.push(5.0).unwrap();
        assert_eq!(
            consumer.pop().unwrap(),
            5.0,
            "what follows is the live audio"
        );
    }

    /// A PCM payload as `ReceivePath::receive` expects it: `f32` samples as
    /// little-endian bytes (matches `PcmCodec::encode`).
    fn pcm(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    /// A mix with one participant in it, who sends stereo frames of `frame`
    /// samples per channel and is played at `volume` percent
    fn mix_of_one(frame: u32, volume: u32) -> (PeerMix, ReceivePath, VoiceHandle) {
        let receive = ReceivePath::new(
            AudioPreset::ZeroLatency.codec_type(),
            48_000,
            frame,
            AudioPreset::ZeroLatency.jitter_buffer_frames(),
        )
        .expect("a PCM receive path");
        receive.follow_peer_channels(2);
        let mix = PeerMix::new(
            MAX_PEERS_PER_ROOM - 1,
            48_000,
            frame,
            Arc::new(AtomicU64::new(0)),
        );
        let gain = Arc::new(PeerGain::default());
        gain.set_volume(volume);
        let voice = mix
            .add(receive.clone(), gain, Arc::new(PeerMeter::default()))
            .expect("a free place");
        (mix, receive, voice)
    }

    /// `C` (recording/streaming output) must carry exactly what is played:
    /// post-fader, with the local monitor mixed in, same as what a listener
    /// on the output device hears.
    #[test]
    fn playout_source_feeds_its_c_tap_the_same_audio_it_plays() {
        const FRAME: u32 = 4;
        let (mix, receive, _voice) = mix_of_one(FRAME, 50);
        let sent: Vec<f32> = (0..FRAME).flat_map(|_| [0.8_f32, 0.2_f32]).collect();
        assert!(receive.receive(0, &pcm(&sent)));

        let monitor = LocalMonitor::new(FRAME);
        let mut monitor_tap = monitor.tap();
        monitor.set_enabled(true);
        for _ in 0..3 {
            monitor_tap.push(&[0.1; FRAME as usize]);
        }

        let (b_tap, mut b_consumer) = RecordingTap::for_wiring_tests();
        let (c_tap, mut c_consumer) = RecordingTap::for_wiring_tests();
        let mut source = playout_source(
            mix,
            Arc::new(AtomicU32::new(100)),
            Arc::new(AtomicU32::new(0)),
            monitor,
            Some((b_tap, c_tap)),
            None,
        );
        let mut out = vec![0.0f32; receive.frame_samples() * 3];
        let samples = source(&mut out);
        out.truncate(samples);

        let b_chunk = b_consumer.read_chunk(samples).expect("B got a frame");
        let (b_first, _) = b_chunk.as_slices();
        assert_eq!(
            b_first,
            &sent[..samples],
            "B stays dry even though C mixes in the monitor"
        );

        let c_chunk = c_consumer.read_chunk(samples).expect("C got a frame");
        let (c_first, _) = c_chunk.as_slices();
        assert_eq!(c_first, &out[..], "C matches exactly what is played");
        assert_ne!(
            c_first,
            &sent[..samples],
            "C is not dry: the fader and monitor were applied"
        );
    }

    /// A session with nobody to hear yet: the play-out buffer has nothing, and
    /// the output callback is the only thing that can put the monitored input
    /// on the speakers.
    fn monitored_output(monitoring: bool) -> Vec<f32> {
        const FRAME: u32 = 32;
        let (mix, receive, _voice) = mix_of_one(FRAME, 100);
        let monitor = LocalMonitor::new(FRAME);
        let mut tap = monitor.tap();
        monitor.set_enabled(monitoring);
        for _ in 0..3 {
            tap.push(&[0.25; FRAME as usize]);
        }

        let mut source = playout_source(
            mix,
            Arc::new(AtomicU32::new(100)),
            Arc::new(AtomicU32::new(0)),
            monitor,
            None,
            None,
        );
        let mut out = vec![0.0f32; receive.frame_samples() * 3];
        let samples = source(&mut out);
        assert_eq!(samples, receive.frame_samples(), "one nominal frame");
        out.truncate(samples);
        out
    }

    /// Verifies: REQ-AUD-111
    #[test]
    fn test_monitored_input_is_played_before_any_peer_has_sent_audio() {
        let out = monitored_output(true);

        assert!(
            out.iter().all(|s| (*s - 0.25).abs() < 1e-6),
            "the input is mixed into both channels: {out:?}"
        );
    }

    /// Verifies: REQ-AUD-112
    #[test]
    fn test_output_is_silent_with_monitoring_off_and_no_peer_audio() {
        let out = monitored_output(false);

        assert!(out.iter().all(|s| *s == 0.0), "{out:?}");
    }
}
