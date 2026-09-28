//! Recording/streaming virtual output device (Linux only; rm:1068).
//!
//! Publishes the self / peer / mix audio (`A` / `B` / `C`, 2ch each, 6ch
//! total) as a virtual device a DAW or OBS can pick as a recording input,
//! with no separate driver install. `pw-loopback` creates the device pair in
//! one child process: a sink face (`NODE_NAME_IN`, this app writes here) and
//! a source face (`NODE_NAME_OUT`, a DAW picks this), wired straight through.
//! Killing the process tears down both, so there is no node id to track or
//! `pw-cli destroy` to run.
//!
//! Getting the audio into the sink face goes through a second child process,
//! `pw-cat --playback`, fed over its stdin: linking `libpipewire` directly
//! (the `pipewire` crate) would need `libpipewire-0.3-dev` at build time,
//! which the desktop app's CI images do not carry today. `pw-cat` and
//! `pw-loopback` ship in the `pipewire` package already required for the
//! PipeWire backend used elsewhere (see `tests/e2e/scripts/setup-virtual-audio-linux.sh`).
//!
//! [`RecordingFeed`] is what actually gets the `A`/`B`/`C` frames from the
//! streaming session's audio callbacks to the sink: each source pushes into
//! its own lock-free ring ([`RecordingTap`], no allocation, no waiting), and
//! a dedicated writer thread polls the three rings and interleaves them into
//! the 6ch block this sink expects.
//!
//! # Scope not covered here
//!
//! - macOS and Windows have no user-mode way to create a device at runtime;
//!   `start` returns [`AudioError::UnsupportedConfig`] there. Plans.md tracks
//!   the follow-up (a bundled, installer-provisioned Core Audio HAL plugin /
//!   virtual audio driver).

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use super::error::AudioError;

/// How many stereo sources [`RecordingFeed`] combines: `A` (self), `B`
/// (peer), `C` (mix).
const SOURCES: usize = 3;

/// Frames of headroom each source's ring keeps beyond a writer poll, so a
/// callback arriving a little late does not starve the others (same
/// reasoning as [`super::monitor::MONITOR_MARGIN_FRAMES`]).
const RING_FRAMES: usize = 64;

/// How often the writer thread polls the three rings and writes a block to
/// the sink. Short enough that a DAW/OBS recording it hears no more than this
/// much extra delay; long enough not to busy-loop.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Channels this device exposes: `A` (self, dry) + `B` (peer, dry) + `C`
/// (mix, post-fader), 2ch each.
pub const CHANNELS: usize = 6;

/// Node name a DAW or OBS picks as a recording input.
pub const NODE_NAME_OUT: &str = "jamjam-recording-output";

/// Node name this app writes to (the same virtual cable's other end).
const NODE_NAME_IN: &str = "jamjam-recording-output-in";

/// Owns the `pw-loopback` device pair and the `pw-cat` writer feeding it.
/// Dropping this kills both child processes, which tears down the PipeWire
/// nodes with them.
pub struct VirtualOutputSink {
    loopback: Child,
    writer: Child,
}

impl VirtualOutputSink {
    /// Starts the virtual device pair and the writer process. `sample_rate`
    /// must match what the streaming session's mix runs at: `pw-cat` is not
    /// told the source rate and cannot resample.
    #[cfg(target_os = "linux")]
    pub fn start(sample_rate: u32) -> Result<Self, AudioError> {
        let loopback = Command::new("pw-loopback")
            .arg("--name")
            .arg(NODE_NAME_OUT)
            .arg("--channels")
            .arg(CHANNELS.to_string())
            .arg("--capture-props")
            .arg(format!(
                "media.class=Audio/Sink node.name={NODE_NAME_IN} \
                 node.description=\"JamJam Recording Output\""
            ))
            .arg("--playback-props")
            .arg(format!(
                "media.class=Audio/Source node.name={NODE_NAME_OUT}"
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AudioError::PluginError(format!("pw-loopback did not start: {e}")))?;

        let writer = Command::new("pw-cat")
            .arg("--playback")
            .arg("--target")
            .arg(NODE_NAME_IN)
            .arg("--channels")
            .arg(CHANNELS.to_string())
            .arg("--rate")
            .arg(sample_rate.to_string())
            .arg("--format")
            .arg("f32")
            .arg("--raw")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AudioError::PluginError(format!("pw-cat did not start: {e}")))?;

        Ok(Self { loopback, writer })
    }

    /// Same signature on every platform; unsupported ones fail to start
    /// rather than silently doing nothing, so a caller logs it once instead
    /// of it going unnoticed (`AudioError::UnsupportedConfig`).
    #[cfg(not(target_os = "linux"))]
    pub fn start(_sample_rate: u32) -> Result<Self, AudioError> {
        Err(AudioError::UnsupportedConfig(
            "recording/streaming virtual output is Linux-only for now".into(),
        ))
    }

    /// Writes one block of interleaved `CHANNELS`-wide `f32` frames.
    /// Blocking: call this from a dedicated writer thread, never from an
    /// audio callback (see the audio thread rules in `docs-spec/architecture.md`).
    pub fn write_frames(&mut self, interleaved: &[f32]) -> Result<(), AudioError> {
        debug_assert_eq!(interleaved.len() % CHANNELS, 0);
        let stdin = self.writer.stdin.as_mut().expect("stdin piped at spawn");
        let bytes = f32_to_le_bytes(interleaved);
        stdin
            .write_all(&bytes)
            .map_err(|e| AudioError::StreamError(format!("virtual output write failed: {e}")))
    }
}

impl Drop for VirtualOutputSink {
    fn drop(&mut self) {
        // Order does not matter: each is its own process and neither waits
        // on the other to exit.
        let _ = self.writer.kill();
        let _ = self.writer.wait();
        let _ = self.loopback.kill();
        let _ = self.loopback.wait();
    }
}

/// One of `A`/`B`/`C`'s ring, swappable without disturbing the writer thread.
///
/// Works like [`super::monitor::LocalMonitor`]'s tap: [`RecordingSource::tap`]
/// starts a fresh ring and retires the previous producer, which is what a
/// device switch does when it rebuilds the audio callback that owns the tap.
/// The writer thread keeps reading from the same slot throughout.
struct RecordingSource {
    consumer: Arc<Mutex<Consumer<f32>>>,
}

impl RecordingSource {
    fn new() -> Self {
        let (_, consumer) = RingBuffer::new(RING_FRAMES * 2);
        Self {
            consumer: Arc::new(Mutex::new(consumer)),
        }
    }

    /// The producer-side handle. Move it into the audio callback that
    /// produces this source's stereo frames.
    fn tap(&self) -> RecordingTap {
        let (producer, consumer) = RingBuffer::new(RING_FRAMES * 2);
        // Blocking here is fine: this runs on the thread that (re)starts the
        // callback, never on the callback itself.
        if let Ok(mut guard) = self.consumer.lock() {
            *guard = consumer;
        }
        RecordingTap { producer }
    }
}

/// The producer-side handle for one of [`RecordingFeed`]'s `A`/`B`/`C`
/// sources. Move it into the audio callback that captures or renders that
/// source.
pub struct RecordingTap {
    producer: Producer<f32>,
}

impl RecordingTap {
    /// A tap paired with the consumer that reads what is pushed to it,
    /// without a [`RecordingFeed`] (which needs a real virtual device).
    ///
    /// For a caller that wires up an audio callback with a tap it obtained
    /// from a `RecordingFeed`: this lets it verify that wiring - which frames
    /// reach `A`/`B`/`C`, and in what state (pre- or post-fader) - without a
    /// PipeWire daemon.
    pub fn for_wiring_tests() -> (Self, Consumer<f32>) {
        let (producer, consumer) = RingBuffer::new(RING_FRAMES * 2);
        (Self { producer }, consumer)
    }

    /// Pushes a stereo (2-channel interleaved) frame.
    ///
    /// No allocation, no waiting: safe to call from an audio callback. A full
    /// ring (the writer thread fell behind) drops the frame rather than
    /// block the device.
    pub fn push_stereo(&mut self, stereo: &[f32]) {
        let Ok(mut chunk) = self.producer.write_chunk(stereo.len()) else {
            return;
        };
        let (first, second) = chunk.as_mut_slices();
        let split = first.len();
        first.copy_from_slice(&stereo[..split]);
        second.copy_from_slice(&stereo[split..]);
        chunk.commit_all();
    }

    /// Pushes a mono frame, heard on both sides: dry audio has no pan applied
    /// yet, so a mono source plays the same on `left` and `right` rather than
    /// picking one arbitrarily.
    pub fn push_mono(&mut self, mono: &[f32]) {
        let Ok(mut chunk) = self.producer.write_chunk(mono.len() * 2) else {
            return;
        };
        let (first, second) = chunk.as_mut_slices();
        let mut out = first.iter_mut().chain(second.iter_mut());
        for &sample in mono {
            *out.next().expect("chunk sized for 2 samples per input") = sample;
            *out.next().expect("chunk sized for 2 samples per input") = sample;
        }
        chunk.commit_all();
    }
}

/// Feeds the self (`A`) / peer (`B`) / mix (`C`) audio into a
/// [`VirtualOutputSink`] on a dedicated writer thread, so a DAW or OBS can
/// record or monitor them independently (rm:1068).
///
/// Each source is dry-run push-only from wherever it is produced; this type
/// owns none of the audio pipeline, only the combination into 6ch and the
/// write to the sink.
pub struct RecordingFeed {
    stop: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
    a: RecordingSource,
    b: RecordingSource,
    c: RecordingSource,
}

impl RecordingFeed {
    /// Starts the virtual device and its writer thread. Returns `None` where
    /// the platform or environment cannot provide a virtual device (logged
    /// once), so a caller runs the session without recording/streaming
    /// output rather than failing the session over it.
    pub fn start(sample_rate: u32) -> Option<Self> {
        let sink = match VirtualOutputSink::start(sample_rate) {
            Ok(sink) => sink,
            Err(e) => {
                tracing::info!("Recording/streaming virtual output not started: {e}");
                return None;
            }
        };

        let a = RecordingSource::new();
        let b = RecordingSource::new();
        let c = RecordingSource::new();
        let stop = Arc::new(AtomicBool::new(false));

        let block_frames = (sample_rate as f32 * POLL_INTERVAL.as_secs_f32()).round() as usize;
        let writer = std::thread::Builder::new()
            .name("jamjam-recording-writer".into())
            .spawn({
                let stop = stop.clone();
                let sources = [a.consumer.clone(), b.consumer.clone(), c.consumer.clone()];
                move || writer_loop(sink, sources, block_frames.max(1), stop)
            })
            .ok();

        Some(Self {
            stop,
            writer,
            a,
            b,
            c,
        })
    }

    /// A fresh handle for the self (dry) audio. Call again after a capture
    /// device switch rebuilds the callback.
    pub fn tap_a(&self) -> RecordingTap {
        self.a.tap()
    }

    /// A fresh handle for the peer (dry, pre-fader) audio. Call again after a
    /// playback device switch rebuilds the callback.
    pub fn tap_b(&self) -> RecordingTap {
        self.b.tap()
    }

    /// A fresh handle for the mix (post-fader, what is actually heard) audio.
    /// Call again after a playback device switch rebuilds the callback.
    pub fn tap_c(&self) -> RecordingTap {
        self.c.tap()
    }
}

impl Drop for RecordingFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.writer.take() {
            let _ = handle.join();
        }
    }
}

/// Polls the three sources every [`POLL_INTERVAL`] and writes one interleaved
/// 6ch block to `sink`. A source with nothing (yet) available contributes
/// silence for the frames it is short, so one quiet source (no peer
/// connected, mic muted) never stalls the other two.
fn writer_loop(
    mut sink: VirtualOutputSink,
    sources: [Arc<Mutex<Consumer<f32>>>; SOURCES],
    block_frames: usize,
    stop: Arc<AtomicBool>,
) {
    let mut block = vec![0.0_f32; block_frames * CHANNELS];
    while !stop.load(Ordering::Relaxed) {
        for (source_index, source) in sources.iter().enumerate() {
            take_stereo_or_silence(source, &mut block, source_index * 2, block_frames);
        }
        if sink.write_frames(&block).is_err() {
            // The writer process died (e.g. PipeWire restarted); nothing to
            // recover into on this thread, so stop rather than spin.
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Reads up to `block_frames` stereo frames from `source` into `block` at
/// device-channel offset `channel_offset` (stride [`CHANNELS`]), padding with
/// silence for frames not (yet) available.
fn take_stereo_or_silence(
    source: &Arc<Mutex<Consumer<f32>>>,
    block: &mut [f32],
    channel_offset: usize,
    block_frames: usize,
) {
    let Ok(mut consumer) = source.lock() else {
        return;
    };
    let available = (consumer.slots() / 2).min(block_frames);
    if available > 0 {
        if let Ok(chunk) = consumer.read_chunk(available * 2) {
            let (first, second) = chunk.as_slices();
            for (frame_index, pair) in first
                .as_chunks::<2>()
                .0
                .iter()
                .chain(second.as_chunks::<2>().0)
                .enumerate()
            {
                let at = frame_index * CHANNELS + channel_offset;
                block[at] = pair[0];
                block[at + 1] = pair[1];
            }
            chunk.commit_all();
        }
    }
    for frame_index in available..block_frames {
        let at = frame_index * CHANNELS + channel_offset;
        block[at] = 0.0;
        block[at + 1] = 0.0;
    }
}

/// `f32` samples as their little-endian bytes, the wire format `pw-cat
/// --format f32 --raw` expects.
fn f32_to_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_f32_samples_as_little_endian_bytes() {
        let samples = [1.0_f32, -1.0, 0.0];
        let bytes = f32_to_le_bytes(&samples);
        assert_eq!(bytes.len(), 12);
        assert_eq!(&bytes[0..4], &1.0_f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &(-1.0_f32).to_le_bytes());
        assert_eq!(&bytes[8..12], &0.0_f32.to_le_bytes());
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn refuses_to_start_on_an_unsupported_platform() {
        let result = VirtualOutputSink::start(48_000);
        assert!(matches!(result, Err(AudioError::UnsupportedConfig(_))));
    }

    /// Only runs where `pw-loopback` / `pw-cat` are actually installed and a
    /// PipeWire daemon is reachable (`PIPEWIRE_REMOTE` or the default
    /// `XDG_RUNTIME_DIR/pipewire-0` socket); skips everywhere else rather
    /// than failing CI machines with no audio server.
    #[test]
    #[cfg(target_os = "linux")]
    fn starts_and_stops_against_a_real_pipewire_daemon() {
        if which("pw-loopback").is_none() || which("pw-cat").is_none() {
            eprintln!("skipping: pw-loopback/pw-cat not on PATH");
            return;
        }
        if !pipewire_reachable() {
            eprintln!("skipping: no PipeWire daemon reachable");
            return;
        }

        let mut sink = match VirtualOutputSink::start(48_000) {
            Ok(sink) => sink,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        // Give pw-loopback a moment to register its nodes before writing.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let silence = vec![0.0_f32; CHANNELS * 480];
        sink.write_frames(&silence).expect("write to a live pw-cat");
        drop(sink);
    }

    #[cfg(target_os = "linux")]
    fn which(bin: &str) -> Option<()> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(bin))
                .find(|path| path.is_file())
                .map(|_| ())
        })
    }

    #[cfg(target_os = "linux")]
    fn pipewire_reachable() -> bool {
        std::process::Command::new("pw-cli")
            .arg("info")
            .arg("0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Only runs where a real PipeWire daemon is reachable, like
    /// [`starts_and_stops_against_a_real_pipewire_daemon`] above.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_recording_feed_writes_a_b_and_c_to_a_real_pipewire_daemon() {
        if which("pw-loopback").is_none() || which("pw-cat").is_none() {
            eprintln!("skipping: pw-loopback/pw-cat not on PATH");
            return;
        }
        if !pipewire_reachable() {
            eprintln!("skipping: no PipeWire daemon reachable");
            return;
        }

        let Some(feed) = RecordingFeed::start(48_000) else {
            eprintln!("skipping: RecordingFeed did not start");
            return;
        };
        let mut a = feed.tap_a();
        let mut b = feed.tap_b();
        let mut c = feed.tap_c();
        // Give pw-loopback a moment to register its nodes before writing.
        std::thread::sleep(Duration::from_millis(300));
        for _ in 0..20 {
            a.push_stereo(&[0.1, 0.1]);
            b.push_stereo(&[0.2, 0.2]);
            c.push_stereo(&[0.3, 0.3]);
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(feed);
    }

    #[test]
    fn test_take_stereo_or_silence_copies_available_frames_and_pads_the_rest_with_silence() {
        let (mut producer, consumer) = RingBuffer::<f32>::new(16);
        for sample in [0.1, 0.2, 0.3, 0.4] {
            producer.push(sample).unwrap();
        }
        let source = Arc::new(Mutex::new(consumer));
        let mut block = vec![9.0_f32; 4 * CHANNELS];

        take_stereo_or_silence(&source, &mut block, 2, 4);

        assert_eq!((block[2], block[3]), (0.1, 0.2), "frame 0, channels B");
        assert_eq!(
            (block[CHANNELS + 2], block[CHANNELS + 3]),
            (0.3, 0.4),
            "frame 1, channels B"
        );
        assert_eq!(
            (block[2 * CHANNELS + 2], block[2 * CHANNELS + 3]),
            (0.0, 0.0),
            "frame 2 has nothing available: silence"
        );
        assert_eq!(
            (block[3 * CHANNELS + 2], block[3 * CHANNELS + 3]),
            (0.0, 0.0),
            "frame 3 has nothing available: silence"
        );
    }

    #[test]
    fn test_take_stereo_or_silence_leaves_other_channels_untouched() {
        let (_producer, consumer) = RingBuffer::<f32>::new(16);
        let source = Arc::new(Mutex::new(consumer));
        let mut block = vec![9.0_f32; CHANNELS];

        take_stereo_or_silence(&source, &mut block, 2, 1);

        assert_eq!(block, [9.0, 9.0, 0.0, 0.0, 9.0, 9.0]);
    }

    #[test]
    fn test_pushing_a_mono_frame_is_heard_the_same_on_both_sides() {
        let (producer, mut consumer) = RingBuffer::<f32>::new(16);
        let mut tap = RecordingTap { producer };

        tap.push_mono(&[0.5, -0.25]);

        let chunk = consumer.read_chunk(4).unwrap();
        let (first, _) = chunk.as_slices();
        assert_eq!(first, &[0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn test_pushing_a_stereo_frame_keeps_left_and_right_separate() {
        let (producer, mut consumer) = RingBuffer::<f32>::new(16);
        let mut tap = RecordingTap { producer };

        tap.push_stereo(&[0.5, -0.5, 0.25, -0.25]);

        let chunk = consumer.read_chunk(4).unwrap();
        let (first, _) = chunk.as_slices();
        assert_eq!(first, &[0.5, -0.5, 0.25, -0.25]);
    }

    #[test]
    fn test_pushing_to_a_full_tap_drops_the_frame_rather_than_panicking() {
        let (producer, _consumer) = RingBuffer::<f32>::new(2);
        let mut tap = RecordingTap { producer };
        tap.push_stereo(&[1.0, 1.0]);

        tap.push_stereo(&[2.0, 2.0]);
    }

    #[test]
    fn test_retapping_a_recording_source_moves_the_writer_to_the_new_ring() {
        let source = RecordingSource::new();
        let mut old_tap = source.tap();
        old_tap.push_stereo(&[9.0, 9.0]);

        let mut new_tap = source.tap();
        new_tap.push_stereo(&[1.0, 2.0]);

        let mut block = vec![0.0_f32; CHANNELS];
        take_stereo_or_silence(&source.consumer, &mut block, 0, 1);

        assert_eq!(
            (block[0], block[1]),
            (1.0, 2.0),
            "reads the new ring, not the old one"
        );
    }
}
