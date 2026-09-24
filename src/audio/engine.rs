//! Audio engine for capture and playback

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use tracing::{debug, error, info, warn};

use super::channels::{pick_channels, place_stereo, smallest_channel_count, OutputRoute};
use super::device::{
    display_name, offered_channel_counts, resolve_input_device, resolve_output_device, DeviceId,
};
use super::error::AudioError;

/// Events that can occur during audio streaming
#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// Input device was disconnected
    InputDeviceDisconnected,
    /// Output device was disconnected
    OutputDeviceDisconnected,
    /// Stream error occurred
    StreamError(String),
}

/// Bit depth for audio samples
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BitDepth {
    /// 16-bit signed integer
    I16,
    /// 24-bit signed integer
    I24,
    /// 32-bit floating point
    #[default]
    F32,
}

/// Audio configuration (shared base)
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u16,
    /// Frame size in samples
    pub frame_size: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            frame_size: 64,
        }
    }
}

/// Capture configuration
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u16,
    /// Frame size in samples
    pub frame_size: u32,
    /// Bit depth
    pub bit_depth: BitDepth,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            frame_size: 64,
            bit_depth: BitDepth::F32,
        }
    }
}

impl From<CaptureConfig> for AudioConfig {
    fn from(config: CaptureConfig) -> Self {
        AudioConfig {
            sample_rate: config.sample_rate,
            channels: config.channels,
            frame_size: config.frame_size,
        }
    }
}

/// Playback configuration
#[derive(Debug, Clone)]
pub struct PlaybackConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u16,
    /// Frame size in samples
    pub frame_size: u32,
    /// Bit depth
    pub bit_depth: BitDepth,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            frame_size: 64,
            bit_depth: BitDepth::F32,
        }
    }
}

impl From<PlaybackConfig> for AudioConfig {
    fn from(config: PlaybackConfig) -> Self {
        AudioConfig {
            sample_rate: config.sample_rate,
            channels: config.channels,
            frame_size: config.frame_size,
        }
    }
}

/// Audio buffer for samples
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// Sample data (interleaved format)
    pub data: Vec<f32>,
    /// Number of channels
    pub channels: u16,
    /// Number of samples per channel
    pub samples: u32,
}

impl AudioBuffer {
    /// Create a new audio buffer
    pub fn new(data: Vec<f32>, channels: u16) -> Self {
        let samples = if channels > 0 {
            (data.len() / channels as usize) as u32
        } else {
            0
        };
        Self {
            data,
            channels,
            samples,
        }
    }

    /// Create an empty buffer
    pub fn empty(channels: u16, samples: u32) -> Self {
        Self {
            data: vec![0.0; (channels as usize) * (samples as usize)],
            channels,
            samples,
        }
    }
}

/// Callback type for captured audio data
/// Now FnMut (not Sync) to allow non-Sync types like rtrb::Producer
#[allow(dead_code)]
pub type CaptureCallback = Box<dyn FnMut(&[f32], u64) + Send + 'static>;

/// Audio engine handles capture and playback
pub struct AudioEngine {
    config: AudioConfig,
    capture_stream: Option<Stream>,
    playback_stream: Option<Stream>,
    running: Arc<AtomicBool>,
    // Current device IDs (None = default device)
    current_input_device: Option<DeviceId>,
    current_output_device: Option<DeviceId>,
    // Device channels capture delivers, as indexes into a device frame
    capture_picks: Vec<usize>,
    // Device channels the stereo playback is written to
    playback_route: OutputRoute,
    // Event sender for device change notifications
    event_tx: Option<Sender<AudioEvent>>,
}

impl AudioEngine {
    /// Create a new audio engine with the given configuration
    pub fn new(config: AudioConfig) -> Self {
        Self {
            capture_picks: (0..config.channels as usize).collect(),
            playback_route: OutputRoute::default(),
            config,
            capture_stream: None,
            playback_stream: None,
            running: Arc::new(AtomicBool::new(false)),
            current_input_device: None,
            current_output_device: None,
            event_tx: None,
        }
    }

    /// Set event sender for device change notifications
    pub fn set_event_sender(&mut self, tx: Sender<AudioEvent>) {
        self.event_tx = Some(tx);
    }

    /// Get current input device ID
    pub fn current_input_device(&self) -> Option<&DeviceId> {
        self.current_input_device.as_ref()
    }

    /// Get current output device ID
    pub fn current_output_device(&self) -> Option<&DeviceId> {
        self.current_output_device.as_ref()
    }

    /// Sets which device channels capture delivers, from the next
    /// `start_capture` on. The callback receives just those channels,
    /// interleaved in the order given. The device is opened with as many
    /// channels as it takes to reach the highest one.
    pub fn set_capture_picks(&mut self, picks: Vec<usize>) {
        self.capture_picks = picks;
    }

    /// Sets which device channels playback writes to, from the next
    /// `start_playback_with_source` on. Every other channel is silent.
    pub fn set_playback_route(&mut self, route: OutputRoute) {
        self.playback_route = route;
    }

    /// Start audio capture with a callback for captured samples
    ///
    /// The callback is `FnMut + Send` (not Sync) - this allows non-Sync types like
    /// rtrb::Producer to be moved directly into the callback for zero-allocation audio.
    pub fn start_capture<F>(
        &mut self,
        device_id: Option<&DeviceId>,
        mut callback: F,
    ) -> Result<(), AudioError>
    where
        F: FnMut(&[f32], u64) + Send + 'static,
    {
        let device = resolve_input_device(device_id)?;

        let device_name = display_name(&device).unwrap_or_default();
        info!("Starting capture on device: {}", device_name);

        // Store current device ID
        self.current_input_device = device_id.cloned();

        let picks = self.capture_picks.clone();
        let needed = picks.iter().max().map_or(1, |highest| highest + 1);
        let device_channels =
            open_channel_count(&device, true, self.config.sample_rate, needed) as usize;
        let stream_config = StreamConfig {
            channels: device_channels as u16,
            sample_rate: self.config.sample_rate,
            buffer_size: cpal::BufferSize::Fixed(self.config.frame_size),
        };
        // A device frame that is already the frame wanted goes through as is
        let whole_frame = picks.iter().copied().eq(0..device_channels);
        let mut picked =
            Vec::with_capacity(self.config.frame_size as usize * picks.len().max(1) * 4);

        let sample_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sample_count_clone = sample_count.clone();

        // Error callback with device disconnection detection
        let event_tx = self.event_tx.clone();
        let err_fn = move |err: cpal::Error| {
            error!("Capture stream error: {:?}", err);
            match err.kind() {
                cpal::ErrorKind::DeviceNotAvailable => {
                    warn!("Input device disconnected");
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(AudioEvent::InputDeviceDisconnected);
                    }
                }
                _ => {
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(AudioEvent::StreamError(err.to_string()));
                    }
                }
            }
        };

        // Callback is moved directly (no Arc) - allows FnMut without Sync requirement
        let stream = device
            .build_input_stream(
                stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if whole_frame {
                        let timestamp =
                            sample_count_clone.fetch_add(data.len() as u64, Ordering::Relaxed);
                        callback(data, timestamp);
                    } else {
                        pick_channels(data, device_channels, &picks, &mut picked);
                        let timestamp =
                            sample_count_clone.fetch_add(picked.len() as u64, Ordering::Relaxed);
                        callback(&picked, timestamp);
                    }
                },
                err_fn,
                None,
            )
            .map_err(|e| AudioError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::StreamError(e.to_string()))?;
        self.capture_stream = Some(stream);
        self.running.store(true, Ordering::SeqCst);

        debug!(
            "Capture started with config: {:?}, opened with {} device channel(s)",
            self.config, device_channels
        );
        Ok(())
    }

    /// Stream error callback: reports a disconnected device as an event so the
    /// app can tell the user, rather than going quiet.
    fn stream_error_handler(&self) -> impl FnMut(cpal::Error) + Send + 'static {
        let event_tx = self.event_tx.clone();
        move |err: cpal::Error| {
            error!("Playback stream error: {:?}", err);
            match err.kind() {
                cpal::ErrorKind::DeviceNotAvailable => {
                    warn!("Output device disconnected");
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(AudioEvent::OutputDeviceDisconnected);
                    }
                }
                _ => {
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(AudioEvent::StreamError(err.to_string()));
                    }
                }
            }
        }
    }

    /// Start playback that pulls each frame from `fill_frame` at the device's
    /// clock (ADR-028).
    ///
    /// The callback asks for exactly the audio it is about to play, when it is
    /// about to play it, rather than draining a queue another thread has to
    /// keep topped up. Nothing queues behind the source, so the delay the
    /// source holds is the delay heard.
    ///
    /// `fill_frame` runs on the audio callback: it must not allocate, block or
    /// lock without a fallback. It is handed a buffer to fill, interleaved,
    /// and returns how many samples it wrote - which varies when a peer at
    /// another sample rate is being resampled, and forcing it to be constant
    /// is what would change pitch.
    ///
    /// The device may ask for a different number of samples than a frame
    /// holds, so what is left over is carried to the next callback rather than
    /// dropped.
    pub fn start_playback_with_source<F>(
        &mut self,
        device_id: Option<&DeviceId>,
        frame_samples: usize,
        fill_frame: F,
    ) -> Result<(), AudioError>
    where
        F: FnMut(&mut [f32]) -> usize + Send + 'static,
    {
        let device = resolve_output_device(device_id)?;
        let device_name = display_name(&device).unwrap_or_default();
        info!("Starting playback on device: {}", device_name);
        self.current_output_device = device_id.cloned();

        let route = self.playback_route;
        let device_channels = open_channel_count(
            &device,
            false,
            self.config.sample_rate,
            route.channels_needed(),
        ) as usize;
        let stream_config = StreamConfig {
            channels: device_channels as u16,
            sample_rate: self.config.sample_rate,
            buffer_size: cpal::BufferSize::Fixed(self.config.frame_size),
        };

        let mut puller = RoutedPuller {
            puller: FramePuller::new(frame_samples, fill_frame),
            stereo: vec![0.0; self.config.frame_size as usize * 2 * 4],
            device_channels,
            route,
        };

        let err_fn = self.stream_error_handler();
        let stream = device
            .build_output_stream(
                stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| puller.fill(data),
                err_fn,
                None,
            )
            .map_err(|e| AudioError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::StreamError(e.to_string()))?;
        self.playback_stream = Some(stream);

        debug!("Playback started from a frame source: {:?}", self.config);
        Ok(())
    }

    /// Stop capture
    pub fn stop_capture(&mut self) {
        self.capture_stream = None;
        info!("Capture stopped");
    }

    /// Stop playback
    pub fn stop_playback(&mut self) {
        self.playback_stream = None;
        info!("Playback stopped");
    }

    /// Get current configuration
    pub fn config(&self) -> &AudioConfig {
        &self.config
    }

    /// Switch input device while running
    ///
    /// Thread: Must be called from non-realtime thread
    /// Blocking: Yes (until new stream is ready)
    ///
    /// This stops the existing capture stream and starts a new one with the new device.
    /// There will be a brief audio gap (~10-50ms) during the switch.
    pub fn set_input_device<F>(
        &mut self,
        device_id: Option<&DeviceId>,
        callback: F,
    ) -> Result<(), AudioError>
    where
        F: FnMut(&[f32], u64) + Send + 'static,
    {
        info!("Switching input device to: {:?}", device_id.map(|d| &d.0));

        // Stop existing capture stream
        self.stop_capture();

        // Start new capture with new device
        self.start_capture(device_id, callback)
    }

    /// Check if capture is currently running
    pub fn is_capture_running(&self) -> bool {
        self.capture_stream.is_some()
    }

    /// Check if playback is currently running
    pub fn is_playback_running(&self) -> bool {
        self.playback_stream.is_some()
    }
}

/// The channel count to open `device` with to reach channel number `needed`:
/// the smallest one it offers that does. A device that does not say what it
/// offers is asked for `needed` and left to refuse.
fn open_channel_count(device: &cpal::Device, input: bool, sample_rate: u32, needed: usize) -> u16 {
    let offered = offered_channel_counts(device, input, sample_rate);
    smallest_channel_count(&offered, needed).unwrap_or(needed as u16)
}

/// Hands the device the stereo a frame source produces, on the channels the
/// output route names.
struct RoutedPuller<F> {
    puller: FramePuller<F>,
    /// The stereo for one request, before it is spread over the device's channels.
    stereo: Vec<f32>,
    device_channels: usize,
    route: OutputRoute,
}

impl<F: FnMut(&mut [f32]) -> usize> RoutedPuller<F> {
    fn fill(&mut self, data: &mut [f32]) {
        if self.device_channels == 2 && self.route.is_default() {
            self.puller.fill(data);
            return;
        }
        let stereo_len = data.len() / self.device_channels * 2;
        if self.stereo.len() < stereo_len {
            self.stereo.resize(stereo_len, 0.0);
        }
        let stereo = &mut self.stereo[..stereo_len];
        self.puller.fill(stereo);
        place_stereo(stereo, data, self.device_channels, self.route);
    }
}

/// Hands device-sized requests what a frame source produces, in frames.
///
/// The source is asked for a frame only when the device has used up the last
/// one, so nothing is fetched ahead of the clock.
struct FramePuller<F> {
    fill_frame: F,
    frame: Vec<f32>,
    filled: usize,
    taken: usize,
}

impl<F: FnMut(&mut [f32]) -> usize> FramePuller<F> {
    fn new(frame_samples: usize, fill_frame: F) -> Self {
        // Frames vary in length once a peer at another sample rate is
        // resampled, so the buffer is sized for the widest conversion and the
        // source reports how much of it it filled.
        Self {
            fill_frame,
            frame: vec![0.0f32; frame_samples.max(1) * 3],
            filled: 0,
            taken: 0,
        }
    }

    fn fill(&mut self, data: &mut [f32]) {
        let mut written = 0;
        while written < data.len() {
            if self.taken == self.filled {
                self.filled = (self.fill_frame)(&mut self.frame).min(self.frame.len());
                self.taken = 0;
                if self.filled == 0 {
                    // A source with nothing to say still has to leave the
                    // device with samples.
                    data[written..].fill(0.0);
                    return;
                }
            }
            let n = (data.len() - written).min(self.filled - self.taken);
            data[written..written + n].copy_from_slice(&self.frame[self.taken..self.taken + n]);
            written += n;
            self.taken += n;
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop_capture();
        self.stop_playback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_config_default() {
        let config = AudioConfig::default();
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.frame_size, 64);
    }

    #[test]
    fn test_engine_creation() {
        let config = AudioConfig::default();
        let engine = AudioEngine::new(config.clone());
        assert_eq!(engine.config().sample_rate, config.sample_rate);
    }

    /// A source that writes `frame_len` samples per call, counting up from 0,
    /// so the order and completeness of what reaches the device can be read
    /// off the values. The counter is shared to see how often it was asked.
    fn counting_source(
        frame_len: usize,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    ) -> impl FnMut(&mut [f32]) -> usize {
        let mut next = 0.0f32;
        move |out: &mut [f32]| {
            calls.fetch_add(1, Ordering::Relaxed);
            for sample in &mut out[..frame_len] {
                *sample = next;
                next += 1.0;
            }
            frame_len
        }
    }

    /// The device's request size has no reason to match the frame length. What
    /// a request leaves over of a frame must open the next request, or samples
    /// are lost or repeated - which is an audible glitch.
    ///
    /// Verifies: REQ-AUD-031
    #[test]
    fn test_device_requests_of_any_size_receive_the_source_in_order() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut puller = FramePuller::new(5, counting_source(5, calls));

        let mut heard = Vec::new();
        for request in [3, 7, 1, 8, 2] {
            let mut data = vec![f32::NAN; request];
            puller.fill(&mut data);
            heard.extend(data);
        }

        let expected: Vec<f32> = (0..heard.len()).map(|n| n as f32).collect();
        assert_eq!(heard, expected);
    }

    /// Audio fetched before the device needs it is audio that waits, and
    /// waiting is the delay the play-out buffer was meant to remove (ADR-028).
    ///
    /// Verifies: REQ-AUD-031
    #[test]
    fn test_source_is_asked_only_when_the_device_has_used_up_the_last_frame() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut puller = FramePuller::new(5, counting_source(5, calls.clone()));
        let mut data = [0.0f32; 3];

        puller.fill(&mut data);
        assert_eq!(calls.load(Ordering::Relaxed), 1, "3 of 5 samples played");
        puller.fill(&mut data);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "6 samples played: 2 frames"
        );
        puller.fill(&mut data);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "9 samples played: still 2"
        );
        puller.fill(&mut data);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "12 samples played: 3 frames"
        );
    }

    /// A source that has nothing must still leave the device with samples, and
    /// they must be silence rather than what an earlier frame left behind.
    ///
    /// Verifies: REQ-AUD-031
    #[test]
    fn test_a_source_with_nothing_to_say_plays_silence() {
        let mut puller = FramePuller::new(4, |_: &mut [f32]| 0);
        let mut data = [1.0f32; 6];

        puller.fill(&mut data);

        assert_eq!(data, [0.0; 6]);
    }

    /// What the device asks for is stereo spread over its own channels: the
    /// source is asked for two samples per device frame, whatever the device's
    /// width, and they land on the channels the route names.
    ///
    /// Verifies: REQ-AUD-119
    #[test]
    fn test_a_device_wider_than_stereo_is_played_on_the_selected_channels() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut puller = RoutedPuller {
            puller: FramePuller::new(4, counting_source(4, calls)),
            stereo: Vec::new(),
            device_channels: 8,
            route: OutputRoute::from_settings(5, Some(6)),
        };
        let mut data = vec![f32::NAN; 16];

        puller.fill(&mut data);

        let mut expected = vec![0.0; 16];
        expected[4..6].copy_from_slice(&[0.0, 1.0]);
        expected[12..14].copy_from_slice(&[2.0, 3.0]);
        assert_eq!(data, expected);
    }
}
