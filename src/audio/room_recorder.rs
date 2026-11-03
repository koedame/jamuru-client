//! Recording the room to WAV files (ADR-095).
//!
//! The recording is a copy of what the audio callbacks already produce: the
//! mix the listener hears (`C`, the same frames as the virtual output) and, if
//! asked for, the microphone as captured (`A`). Each callback pushes into a
//! lock-free ring ([`FileTap`], no allocation, no waiting, one relaxed load
//! while nothing records) and a writer thread of its own drains the rings to
//! disk, so a slow disk can cost a recording some audio but never the session
//! a frame.
//!
//! The files are 24-bit PCM at the session's sample rate, stereo. They are
//! written as they come and the header is kept current once a second, so the
//! file of a crashed app still plays up to the last second.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};

/// Channels in a recording: what the audio path carries.
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 24;
const BYTES_PER_SAMPLE: usize = 3;
const HEADER_BYTES: u64 = 44;

/// How much audio a ring holds: a writer that stalls this long (a disk that
/// pauses) loses what comes after, rather than blocking the audio.
const RING_SECONDS: usize = 2;

/// How often the writer thread drains the rings.
const WRITE_INTERVAL: Duration = Duration::from_millis(20);

/// How often the header is made right, so a crash costs at most this much.
const HEADER_INTERVAL: Duration = Duration::from_secs(1);

/// The most audio bytes one file takes. A WAV's sizes are 32-bit, so the
/// recording ends a little before 4 GiB (about 3.8 hours at 48 kHz).
pub const MAX_DATA_BYTES: u64 = 4_000_000_000;

/// The sample rates a recording is written at have no fixed list: it is the
/// rate the session runs at.
fn ring_samples(sample_rate: u32) -> usize {
    sample_rate as usize * RING_SECONDS * CHANNELS as usize
}

/// Which audio a [`FileTap`] feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// What the listener hears: the others, after the faders, and the
    /// listener's own monitor if it is on
    Mix,
    /// The microphone as captured, before the fader and the mute
    SelfDry,
}

/// How a recording ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// The person stopped it
    Stopped,
    /// The audio session ended (left the room, the devices were reopened)
    SessionEnded,
    /// A file reached [`MAX_DATA_BYTES`]
    SizeLimit,
    /// Writing failed (the disk is full, the folder is gone)
    WriteFailed(String),
}

/// A recording that has ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    /// Counts up with every recording that ends, so a screen that polls can
    /// tell a new ending from one it has already shown
    pub id: u64,
    pub mix_path: PathBuf,
    pub self_path: Option<PathBuf>,
    pub seconds: f32,
    /// Stereo frames the callback could not hand over (the writer fell
    /// behind) and so are missing from the file
    pub dropped_frames: u64,
    pub reason: EndReason,
}

/// What a screen shows of the recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub recording: bool,
    /// Seconds of audio written to the mix file so far
    pub seconds: f32,
    /// Stereo frames missing from the mix file so far (see [`Finished`])
    pub dropped_frames: u64,
    pub mix_path: Option<PathBuf>,
    pub self_path: Option<PathBuf>,
    /// The latest recording that ended, until the next one starts
    pub last: Option<Finished>,
}

/// Why a recording did not start.
#[derive(Debug)]
pub enum StartError {
    /// One is already running
    Already,
    /// The folder or the file could not be made
    Io(std::io::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Already => f.write_str("a recording is already running"),
            Self::Io(e) => write!(f, "the recording file could not be made: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// The ring a source's callback writes and the writer thread reads, swappable
/// when a device switch rebuilds the callback that owns the producer.
struct Slot {
    consumer: Arc<Mutex<Consumer<f32>>>,
    /// Pushes only while set
    armed: Arc<AtomicBool>,
    /// Samples a push could not fit in the ring, since the recording began
    dropped: Arc<AtomicU64>,
}

impl Slot {
    fn new() -> Self {
        let (_, consumer) = RingBuffer::new(1);
        Self {
            consumer: Arc::new(Mutex::new(consumer)),
            armed: Arc::new(AtomicBool::new(false)),
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct Active {
    dropped: Arc<AtomicU64>,
    mix_path: PathBuf,
    self_path: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    /// Frames written to the mix file
    frames: Arc<AtomicU64>,
    sample_rate: u32,
    thread: JoinHandle<EndReason>,
}

#[derive(Default)]
struct State {
    active: Option<Active>,
    last: Option<Finished>,
    ended: u64,
}

struct Inner {
    mix: Slot,
    self_dry: Slot,
    state: Mutex<State>,
}

/// Records the room. One lives as long as the app; each audio session wires
/// its callbacks to it through the taps.
#[derive(Clone)]
pub struct RoomRecorder {
    inner: Arc<Inner>,
}

impl Default for RoomRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl RoomRecorder {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                mix: Slot::new(),
                self_dry: Slot::new(),
                state: Mutex::new(State::default()),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn slot(&self, source: Source) -> &Slot {
        match source {
            Source::Mix => &self.inner.mix,
            Source::SelfDry => &self.inner.self_dry,
        }
    }

    /// The handle for the callback that plays the mix (`C`). Call again after
    /// a playback device switch rebuilds the callback.
    pub fn tap_mix(&self, sample_rate: u32) -> FileTap {
        self.tap(Source::Mix, sample_rate)
    }

    /// The handle for the callback that captures the microphone (`A`). Call
    /// again after a capture device switch rebuilds the callback.
    pub fn tap_self(&self, sample_rate: u32) -> FileTap {
        self.tap(Source::SelfDry, sample_rate)
    }

    fn tap(&self, source: Source, sample_rate: u32) -> FileTap {
        let slot = self.slot(source);
        let (producer, consumer) = RingBuffer::new(ring_samples(sample_rate));
        // Blocking is fine here: this runs on the thread that (re)starts the
        // callback, never on the callback itself.
        *slot.consumer.lock().unwrap_or_else(|e| e.into_inner()) = consumer;
        FileTap {
            producer,
            armed: slot.armed.clone(),
            dropped: slot.dropped.clone(),
        }
    }

    /// Starts writing `<dir>/<stem>.wav`, and `<dir>/<stem>-self.wav` if
    /// `with_self`, at `sample_rate` (the session's). `dir` is made if it is
    /// not there.
    pub fn start(
        &self,
        dir: &Path,
        stem: &str,
        sample_rate: u32,
        with_self: bool,
    ) -> Result<Report, StartError> {
        let mut state = self.state();
        if state.active.is_some() {
            return Err(StartError::Already);
        }
        std::fs::create_dir_all(dir).map_err(StartError::Io)?;
        let (mix_path, self_path) = free_names(dir, stem, with_self);
        let mut mix = WavFile::create(&mix_path, sample_rate).map_err(StartError::Io)?;
        let mut self_file = match &self_path {
            Some(path) => match WavFile::create(path, sample_rate) {
                Ok(file) => Some(file),
                Err(e) => {
                    let _ = std::fs::remove_file(&mix_path);
                    return Err(StartError::Io(e));
                }
            },
            None => None,
        };

        // What the rings hold now is from before the recording
        discard(&self.inner.mix.consumer);
        discard(&self.inner.self_dry.consumer);
        self.inner.mix.dropped.store(0, Ordering::SeqCst);

        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let thread = std::thread::Builder::new()
            .name("jamuru-room-recorder".into())
            .spawn({
                let stop = stop.clone();
                let frames = frames.clone();
                let mix_ring = self.inner.mix.consumer.clone();
                let self_ring = self.inner.self_dry.consumer.clone();
                move || {
                    write_loop(
                        &mut mix,
                        self_file.as_mut(),
                        &mix_ring,
                        &self_ring,
                        &stop,
                        &frames,
                    )
                }
            })
            .map_err(StartError::Io)?;

        self.inner.mix.armed.store(true, Ordering::SeqCst);
        self.inner.self_dry.armed.store(with_self, Ordering::SeqCst);
        state.active = Some(Active {
            dropped: self.inner.mix.dropped.clone(),
            mix_path,
            self_path,
            stop,
            frames,
            sample_rate,
            thread,
        });
        drop(state);
        Ok(self.report())
    }

    /// Ends the recording, if one runs, with `reason`, and says what was
    /// recorded. The files are finished (their headers made right) when this
    /// returns.
    pub fn stop(&self, reason: EndReason) -> Option<Finished> {
        let mut state = self.state();
        let active = state.active.take()?;
        self.finish(&mut state, active, Some(reason))
    }

    /// Ends the recording because the audio session ended.
    pub fn session_ended(&self) {
        self.stop(EndReason::SessionEnded);
    }

    fn finish(
        &self,
        state: &mut State,
        active: Active,
        reason: Option<EndReason>,
    ) -> Option<Finished> {
        self.inner.mix.armed.store(false, Ordering::SeqCst);
        self.inner.self_dry.armed.store(false, Ordering::SeqCst);
        active.stop.store(true, Ordering::SeqCst);
        let thread_reason = active.thread.join().unwrap_or_else(|_| {
            EndReason::WriteFailed("the recording thread stopped unexpectedly".into())
        });
        // The writer's own reason (a full disk) is the one that matters
        let reason = match (&thread_reason, reason) {
            (EndReason::Stopped, Some(asked)) => asked,
            _ => thread_reason,
        };
        state.ended += 1;
        let finished = Finished {
            id: state.ended,
            mix_path: active.mix_path,
            self_path: active.self_path,
            seconds: active.frames.load(Ordering::SeqCst) as f32 / active.sample_rate as f32,
            dropped_frames: active.dropped.load(Ordering::SeqCst) / 2,
            reason,
        };
        state.last = Some(finished.clone());
        Some(finished)
    }

    /// What there is to show. Also collects a recording that ended by itself.
    pub fn report(&self) -> Report {
        let mut state = self.state();
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.thread.is_finished())
        {
            if let Some(active) = state.active.take() {
                self.finish(&mut state, active, None);
            }
        }
        match &state.active {
            Some(active) => Report {
                recording: true,
                seconds: active.frames.load(Ordering::SeqCst) as f32 / active.sample_rate as f32,
                dropped_frames: active.dropped.load(Ordering::SeqCst) / 2,
                mix_path: Some(active.mix_path.clone()),
                self_path: active.self_path.clone(),
                last: None,
            },
            None => Report {
                recording: false,
                seconds: 0.0,
                dropped_frames: 0,
                mix_path: None,
                self_path: None,
                last: state.last.clone(),
            },
        }
    }
}

/// The producer-side handle of one source of the recording. Move it into the
/// audio callback that produces that source's frames.
pub struct FileTap {
    producer: Producer<f32>,
    armed: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}

impl FileTap {
    /// Pushes a stereo (2-channel interleaved) frame.
    ///
    /// No allocation, no waiting: safe to call from an audio callback. While
    /// nothing records it is one load; a full ring (the writer fell behind)
    /// drops the frame, and counts it, rather than block the device.
    pub fn push_stereo(&mut self, stereo: &[f32]) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut chunk) = self.producer.write_chunk(stereo.len()) else {
            self.dropped
                .fetch_add(stereo.len() as u64, Ordering::Relaxed);
            return;
        };
        let (first, second) = chunk.as_mut_slices();
        let split = first.len();
        first.copy_from_slice(&stereo[..split]);
        second.copy_from_slice(&stereo[split..]);
        chunk.commit_all();
    }

    /// Pushes a mono frame, heard on both sides.
    pub fn push_mono(&mut self, mono: &[f32]) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut chunk) = self.producer.write_chunk(mono.len() * 2) else {
            self.dropped
                .fetch_add(mono.len() as u64 * 2, Ordering::Relaxed);
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

/// The names the files of a recording get: `<stem>.wav`, and with the
/// microphone's `<stem>-self.wav`, with a number after the stem if either is
/// taken.
fn free_names(dir: &Path, stem: &str, with_self: bool) -> (PathBuf, Option<PathBuf>) {
    for attempt in 1u32.. {
        let stem = if attempt == 1 {
            stem.to_string()
        } else {
            format!("{stem}-{attempt}")
        };
        let mix = dir.join(format!("{stem}.wav"));
        let own = with_self.then(|| dir.join(format!("{stem}-self.wav")));
        if !mix.exists() && own.as_ref().is_none_or(|path| !path.exists()) {
            return (mix, own);
        }
    }
    unreachable!("a free name is found within u32 attempts")
}

fn discard(ring: &Arc<Mutex<Consumer<f32>>>) {
    let mut consumer = ring.lock().unwrap_or_else(|e| e.into_inner());
    let queued = consumer.slots();
    if let Ok(chunk) = consumer.read_chunk(queued) {
        chunk.commit_all();
    }
}

/// A WAV file being written: 24-bit PCM, stereo.
struct WavFile {
    file: File,
    sample_rate: u32,
    /// Audio bytes written
    data_bytes: u64,
    /// Reused for the bytes of one write
    scratch: Vec<u8>,
}

impl WavFile {
    fn create(path: &Path, sample_rate: u32) -> std::io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut wav = Self {
            file,
            sample_rate,
            data_bytes: 0,
            scratch: Vec::new(),
        };
        wav.write_header()?;
        Ok(wav)
    }

    /// Writes the 44-byte header for the audio written so far, and leaves the
    /// file positioned to append.
    fn write_header(&mut self) -> std::io::Result<()> {
        let block_align = CHANNELS as u32 * BYTES_PER_SAMPLE as u32;
        let data = self.data_bytes.min(u32::MAX as u64 - 36) as u32;
        let mut header = Vec::with_capacity(HEADER_BYTES as usize);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + data).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes()); // PCM
        header.extend_from_slice(&CHANNELS.to_le_bytes());
        header.extend_from_slice(&self.sample_rate.to_le_bytes());
        header.extend_from_slice(&(self.sample_rate * block_align).to_le_bytes());
        header.extend_from_slice(&(block_align as u16).to_le_bytes());
        header.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data.to_le_bytes());
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header)?;
        self.file
            .seek(SeekFrom::Start(HEADER_BYTES + self.data_bytes))?;
        Ok(())
    }

    /// Appends `samples` as 24-bit PCM, up to what the size limit leaves
    /// room for. Returns how many samples were written.
    fn append(&mut self, samples: &[f32]) -> std::io::Result<usize> {
        let room = (MAX_DATA_BYTES - self.data_bytes) as usize / BYTES_PER_SAMPLE;
        let samples = &samples[..samples.len().min(room)];
        self.scratch.clear();
        for &sample in samples {
            let value = (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            self.scratch.extend_from_slice(&value.to_le_bytes()[..3]);
        }
        self.file.write_all(&self.scratch)?;
        self.data_bytes += self.scratch.len() as u64;
        Ok(samples.len())
    }

    fn full(&self) -> bool {
        self.data_bytes + (CHANNELS as usize * BYTES_PER_SAMPLE) as u64 > MAX_DATA_BYTES
    }
}

/// Moves what is in `ring` into `file`. Returns the frames written.
fn drain(
    ring: &Arc<Mutex<Consumer<f32>>>,
    file: &mut WavFile,
    block: &mut Vec<f32>,
) -> std::io::Result<u64> {
    {
        let mut consumer = ring.lock().unwrap_or_else(|e| e.into_inner());
        // Whole frames only
        let available = consumer.slots() / CHANNELS as usize * CHANNELS as usize;
        block.clear();
        if available > 0 {
            if let Ok(chunk) = consumer.read_chunk(available) {
                let (first, second) = chunk.as_slices();
                block.extend_from_slice(first);
                block.extend_from_slice(second);
                chunk.commit_all();
            }
        }
    }
    let written = file.append(block)?;
    Ok((written / CHANNELS as usize) as u64)
}

fn write_loop(
    mix: &mut WavFile,
    mut self_file: Option<&mut WavFile>,
    mix_ring: &Arc<Mutex<Consumer<f32>>>,
    self_ring: &Arc<Mutex<Consumer<f32>>>,
    stop: &AtomicBool,
    frames: &AtomicU64,
) -> EndReason {
    let mut block = Vec::new();
    let mut header_due = Instant::now() + HEADER_INTERVAL;
    let result = (|| -> Result<EndReason, std::io::Error> {
        loop {
            // Read the flag before draining, so what the callbacks pushed
            // before it was set is still written
            let stopping = stop.load(Ordering::SeqCst);
            let written = drain(mix_ring, mix, &mut block)?;
            frames.fetch_add(written, Ordering::SeqCst);
            if let Some(file) = self_file.as_deref_mut() {
                drain(self_ring, file, &mut block)?;
            }
            if mix.full() || self_file.as_deref().is_some_and(WavFile::full) {
                return Ok(EndReason::SizeLimit);
            }
            if stopping {
                return Ok(EndReason::Stopped);
            }
            if Instant::now() >= header_due {
                mix.write_header()?;
                if let Some(file) = self_file.as_deref_mut() {
                    file.write_header()?;
                }
                header_due = Instant::now() + HEADER_INTERVAL;
            }
            std::thread::sleep(WRITE_INTERVAL);
        }
    })();
    let reason = match result {
        Ok(reason) => reason,
        Err(e) => EndReason::WriteFailed(e.to_string()),
    };
    // A last header, whatever ended it
    let _ = mix.write_header();
    if let Some(file) = self_file {
        let _ = file.write_header();
    }
    reason
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn read_wav(path: &Path) -> (hound::WavSpec, Vec<i32>) {
        let mut reader = hound::WavReader::open(path).expect("a WAV that opens");
        let spec = reader.spec();
        let samples = reader.samples::<i32>().map(|s| s.unwrap()).collect();
        (spec, samples)
    }

    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_the_mix_is_pushed_while_recording_the_wav_holds_it_as_24_bit_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        recorder.start(dir.path(), "room", RATE, false).unwrap();

        let frames: Vec<f32> = (0..480)
            .flat_map(|i| [i as f32 / 1000.0, -(i as f32) / 1000.0])
            .collect();
        tap.push_stereo(&frames);
        wait_until("the writer to take the frames", || {
            recorder.report().seconds > 0.0
        });
        let finished = recorder.stop(EndReason::Stopped).unwrap();

        let (spec, samples) = read_wav(&finished.mix_path);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, RATE);
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        assert_eq!(samples.len(), 960);
        // 0.123 of full scale, in 24-bit
        let expected = (0.123_f32 * 8_388_607.0).round() as i32;
        assert!((samples[2 * 123] - expected).abs() <= 1);
        assert!((samples[2 * 123 + 1] + expected).abs() <= 1);
        assert_eq!(finished.reason, EndReason::Stopped);
        assert!((finished.seconds - 0.01).abs() < 1e-4);
    }

    /// Verifies: REQ-AUD-048
    #[test]
    fn when_nothing_records_a_tap_takes_nothing_so_a_later_recording_starts_clean() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        tap.push_stereo(&[0.5; 960]);

        recorder.start(dir.path(), "room", RATE, false).unwrap();
        let finished = recorder.stop(EndReason::Stopped).unwrap();

        let (_, samples) = read_wav(&finished.mix_path);
        assert!(samples.is_empty());
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_the_microphone_is_asked_for_it_is_written_to_a_second_file() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut mix = recorder.tap_mix(RATE);
        let mut own = recorder.tap_self(RATE);
        let report = recorder.start(dir.path(), "room", RATE, true).unwrap();
        assert_eq!(
            report.self_path.as_deref(),
            Some(dir.path().join("room-self.wav").as_path())
        );

        mix.push_stereo(&[0.25; 200]);
        own.push_mono(&[-0.5; 50]);
        wait_until("both files", || recorder.report().seconds > 0.0);
        let finished = recorder.stop(EndReason::Stopped).unwrap();

        let (_, mixed) = read_wav(&finished.mix_path);
        let (_, own) = read_wav(&finished.self_path.unwrap());
        assert_eq!(mixed.len(), 200);
        // A mono microphone is heard on both sides
        assert_eq!(own.len(), 100);
        assert!(own.iter().all(|&s| s < 0));
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_the_microphone_is_not_asked_for_its_ring_is_not_fed() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut own = recorder.tap_self(RATE);
        recorder.start(dir.path(), "room", RATE, false).unwrap();

        own.push_stereo(&[0.5; 960]);

        assert!(!dir.path().join("room-self.wav").exists());
        assert_eq!(recorder.inner.self_dry.consumer.lock().unwrap().slots(), 0);
        recorder.stop(EndReason::Stopped);
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_samples_are_beyond_full_scale_they_are_clipped_not_wrapped() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        recorder.start(dir.path(), "room", RATE, false).unwrap();

        tap.push_stereo(&[2.0, -2.0, f32::NAN, 0.0]);
        wait_until("the frames", || recorder.report().seconds > 0.0);
        let finished = recorder.stop(EndReason::Stopped).unwrap();

        let (_, samples) = read_wav(&finished.mix_path);
        assert_eq!(samples, vec![8_388_607, -8_388_607, 0, 0]);
    }

    /// Verifies: REQ-AUD-047
    #[test]
    fn when_a_recording_runs_a_second_one_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        recorder.start(dir.path(), "room", RATE, false).unwrap();

        let second = recorder.start(dir.path(), "again", RATE, false);

        assert!(matches!(second, Err(StartError::Already)));
        recorder.stop(EndReason::Stopped);
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_the_name_is_taken_the_new_recording_gets_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let first = recorder.start(dir.path(), "room", RATE, true).unwrap();
        recorder.stop(EndReason::Stopped);

        let second = recorder.start(dir.path(), "room", RATE, true).unwrap();
        recorder.stop(EndReason::Stopped);

        assert_eq!(first.mix_path.unwrap(), dir.path().join("room.wav"));
        assert_eq!(second.mix_path.unwrap(), dir.path().join("room-2.wav"));
        assert_eq!(
            second.self_path.unwrap(),
            dir.path().join("room-2-self.wav")
        );
    }

    /// Verifies: REQ-AUD-047
    #[test]
    fn when_the_session_ends_the_recording_ends_with_the_file_finished() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        recorder.start(dir.path(), "room", RATE, false).unwrap();
        tap.push_stereo(&[0.1; 96]);
        wait_until("the frames", || recorder.report().seconds > 0.0);

        recorder.session_ended();

        let report = recorder.report();
        assert!(!report.recording);
        let last = report.last.unwrap();
        assert_eq!(last.reason, EndReason::SessionEnded);
        assert_eq!(read_wav(&last.mix_path).1.len(), 96);
    }

    /// Verifies: REQ-AUD-047
    #[test]
    fn when_the_file_cannot_be_written_the_recording_ends_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("room.wav");
        WavFile::create(&path, RATE).unwrap();
        // A handle that cannot write stands for a disk that is gone
        let mut wav = WavFile {
            file: File::open(&path).unwrap(),
            sample_rate: RATE,
            data_bytes: 0,
            scratch: Vec::new(),
        };
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        recorder.inner.mix.armed.store(true, Ordering::SeqCst);
        tap.push_stereo(&[0.5; 96]);
        let frames = AtomicU64::new(0);

        let reason = write_loop(
            &mut wav,
            None,
            &recorder.inner.mix.consumer,
            &recorder.inner.self_dry.consumer,
            &AtomicBool::new(false),
            &frames,
        );

        assert!(matches!(reason, EndReason::WriteFailed(_)), "{reason:?}");
    }

    /// Verifies: REQ-AUD-047
    #[test]
    fn when_a_file_reaches_the_size_limit_nothing_more_is_written_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.wav");
        let mut wav = WavFile::create(&path, RATE).unwrap();
        wav.data_bytes = MAX_DATA_BYTES - 3 * 2;

        let written = wav.append(&[0.5; 8]).unwrap();

        assert_eq!(written, 2);
        assert!(wav.full());
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_the_header_is_made_right_midway_the_file_plays_up_to_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.wav");
        let mut wav = WavFile::create(&path, RATE).unwrap();
        wav.append(&[0.5; 200]).unwrap();
        wav.write_header().unwrap();
        wav.append(&[0.5; 200]).unwrap();
        // The app dies here: the second append is on disk, the header is not

        let (spec, samples) = read_wav(&path);
        assert_eq!(spec.bits_per_sample, 24);
        assert_eq!(samples.len(), 200);
    }

    /// Verifies: REQ-AUD-048
    #[test]
    fn when_the_writer_falls_behind_the_callback_does_not_wait_and_the_loss_is_counted() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        recorder.start(dir.path(), "behind", RATE, false).unwrap();

        // The writer is stuck: a ring's worth and then some goes in
        let frames_in_ring = RATE as usize * RING_SECONDS;
        let second = vec![0.1_f32; RATE as usize * 2];
        let started = Instant::now();
        {
            let _stuck = recorder.inner.mix.consumer.lock().unwrap();
            for _ in 0..RING_SECONDS + 1 {
                tap.push_stereo(&second);
            }
            assert!(
                started.elapsed() < Duration::from_millis(500),
                "a push waited for the writer"
            );
        }
        let finished = recorder.stop(EndReason::Stopped).unwrap();

        // 3 seconds in a 2-second ring: at least the last second is lost
        assert!(
            finished.dropped_frames >= RATE as u64 / 2,
            "{} dropped of {frames_in_ring}",
            finished.dropped_frames
        );
    }

    /// What the recording costs the audio callback, measured (ADR-095):
    /// `cargo test --release --lib room_recorder_cost -- --ignored --nocapture`.
    /// It feeds 64-frame stereo callbacks at the real pace for 20 s, once with
    /// nothing recording and once recording, and prints the time a push takes
    /// and what the file got.
    #[test]
    #[ignore = "a measurement, 40 s of real time"]
    fn room_recorder_cost() {
        const FRAMES: usize = 64;
        let period = Duration::from_secs_f64(FRAMES as f64 / 48_000.0);
        let block: Vec<f32> = (0..FRAMES * 2)
            .map(|i| (i as f32 * 0.01).sin() * 0.3)
            .collect();

        for recording in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let recorder = RoomRecorder::new();
            let mut tap = recorder.tap_mix(48_000);
            if recording {
                recorder.start(dir.path(), "cost", 48_000, true).unwrap();
            }
            let mut worst = Duration::ZERO;
            let mut total = Duration::ZERO;
            let mut pushes = 0u32;
            let begin = Instant::now();
            let mut next = begin;
            while begin.elapsed() < Duration::from_secs(20) {
                let t0 = Instant::now();
                tap.push_stereo(&block);
                let spent = t0.elapsed();
                worst = worst.max(spent);
                total += spent;
                pushes += 1;
                next += period;
                while Instant::now() < next {
                    std::hint::spin_loop();
                }
            }
            let finished = recorder.stop(EndReason::Stopped);
            println!(
                "recording={recording}: {pushes} pushes, mean {:?}, worst {:?}, written {:?} s, dropped {:?}",
                total / pushes,
                worst,
                finished.as_ref().map(|f| f.seconds),
                finished.as_ref().map(|f| f.dropped_frames),
            );
        }
    }

    /// The cost on the audio callback while nothing records is what the
    /// session pays for having the feature: one relaxed load.
    ///
    /// Verifies: REQ-AUD-048
    #[test]
    fn when_nothing_records_a_push_is_cheap_and_leaves_no_trace() {
        let recorder = RoomRecorder::new();
        let mut tap = recorder.tap_mix(RATE);
        let frame = vec![0.1_f32; 128];

        let before = Instant::now();
        for _ in 0..100_000 {
            tap.push_stereo(&frame);
        }
        let per_push = before.elapsed() / 100_000;

        assert!(
            recorder.inner.mix.consumer.lock().unwrap().slots() == 0,
            "an idle tap pushed"
        );
        // Generous: an optimised build measures in single-digit nanoseconds
        assert!(per_push < Duration::from_micros(5), "{per_push:?} per push");
    }
}
