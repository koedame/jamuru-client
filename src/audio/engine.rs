//! Audio engine for capture and playback

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::StreamConfig;
use tracing::{debug, error, info, warn};

use super::channels::{pick_channels, place_stereo, smallest_channel_count, OutputRoute};
use super::device::{
    display_name, input_open_rate, offered_channel_counts, output_open_rate, resolve_input_device,
    resolve_output_device, DeviceId,
};
use super::driver::{StreamHost, OPEN_TIMEOUT};
use super::error::AudioError;
use super::fault::{self, Call};
use super::heartbeat::Heartbeat;
use super::open::DeviceSide;
use super::playout::widest_frame_samples;
use super::resampler::{CaptureResampler, ResamplerError};
use super::wasapi_mode::{
    next_note, note_opened, opened_through_wasapi, Noted, OpenedWay, WasapiMode,
};

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
    /// How a WASAPI device is opened (Windows; ADR-088)
    pub wasapi_mode: WasapiMode,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 1,
            frame_size: 64,
            wasapi_mode: WasapiMode::default(),
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
            ..AudioConfig::default()
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
            ..AudioConfig::default()
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
    capture_stream: Option<StreamHost>,
    playback_stream: Option<StreamHost>,
    // How long opening a stream is waited for
    open_timeout: Duration,
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
    // Counts the passes of the device callbacks of the streams this engine opens
    heartbeat: Heartbeat,
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
            open_timeout: OPEN_TIMEOUT,
            running: Arc::new(AtomicBool::new(false)),
            current_input_device: None,
            current_output_device: None,
            event_tx: None,
            heartbeat: Heartbeat::default(),
        }
    }

    /// The count of device callback passes, for telling a stream that has
    /// stopped calling back (see [`StallGuard`](super::heartbeat::StallGuard)).
    /// Every stream the engine opens counts on it, so one that is opened again
    /// goes on counting where the last one stopped.
    pub fn heartbeat(&self) -> Heartbeat {
        self.heartbeat.clone()
    }

    /// Makes the streams opened from now on count on `heartbeat`, so an engine
    /// made to replace another keeps the count the watcher is reading.
    pub fn set_heartbeat(&mut self, heartbeat: Heartbeat) {
        self.heartbeat = heartbeat;
    }

    /// Sets how long opening a stream is waited for before the device is
    /// reported as not responding.
    pub fn set_open_timeout(&mut self, timeout: Duration) {
        self.open_timeout = timeout;
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
        callback: F,
    ) -> Result<(), AudioError>
    where
        F: FnMut(&[f32], u64) + Send + 'static,
    {
        info!(
            "Starting capture on device: {}",
            device_id.map_or("system default", |id| id.0.as_str())
        );

        // Store current device ID
        self.current_input_device = device_id.cloned();

        let picks = self.capture_picks.clone();
        let needed = picks.iter().max().map_or(1, |highest| highest + 1);
        let sample_rate = self.config.sample_rate;
        let frame_size = self.config.frame_size;
        let mode = self.config.wasapi_mode;
        let wanted = device_id.cloned();
        let heartbeat = self.heartbeat.clone();
        let serial = fault::stream_serial();
        // Numbered now, so an open that finishes late is told from the one
        // that replaced it
        let note = next_note();

        // Error callback with device disconnection detection
        let event_tx = self.event_tx.clone();
        #[cfg(target_os = "windows")]
        let on_fault =
            wasapi_fault_handler(self.event_tx.clone(), AudioEvent::InputDeviceDisconnected);
        let err_fn = move |err: cpal::Error| {
            error!("Capture stream error: {:?}", err);
            if matches!(err.kind(), cpal::ErrorKind::Xrun) {
                crate::perf::count_input_xrun();
            }
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

        // Opened on a thread of its own: a driver that hangs takes that
        // thread and not this one (see `driver`).
        let host = StreamHost::open("input stream", self.open_timeout, move || {
            fault::point(Call::OpenInput);
            let through_wasapi = opened_through_wasapi(wanted.as_ref());
            let mut fell_back = None;
            // The two ways `cpal` does not open a WASAPI device are opened by
            // `wasapi`; a device they do not open is opened by `cpal` below
            #[cfg(target_os = "windows")]
            let callback = if through_wasapi && mode != WasapiMode::Shared {
                let (picks, heartbeat) = (picks.clone(), heartbeat.clone());
                let opened = super::wasapi::open_capture(
                    wanted.as_ref(),
                    mode,
                    super::wasapi::Wanted {
                        sample_rate,
                        channels: needed,
                        buffer_frames: frame_size,
                    },
                    callback,
                    move |format, callback| {
                        capture_delivery(
                            callback,
                            picks,
                            Shape {
                                device_rate: format.rate,
                                device_channels: format.channels,
                                sample_rate,
                                frame_size,
                            },
                            heartbeat,
                            serial,
                        )
                    },
                    on_fault,
                );
                match opened {
                    Ok(stream) => {
                        info!("Capture opened through WASAPI the {:?} way", mode);
                        return Ok(HeldStream::Wasapi {
                            _stream: stream,
                            _way: note_opened(
                                DeviceSide::Input,
                                note,
                                OpenedWay {
                                    asked: mode,
                                    opened: mode,
                                    failure: None,
                                },
                            ),
                        });
                    }
                    Err(super::wasapi::NotOpened {
                        error,
                        failure,
                        callback: Some(callback),
                    }) => {
                        warn!(
                            "Capture did not open the {:?} way ({}); it opens the standard way",
                            mode, error
                        );
                        fell_back = Some(failure);
                        callback
                    }
                    Err(super::wasapi::NotOpened { error, .. }) => return Err(error),
                }
            } else {
                callback
            };

            let device = resolve_input_device(wanted.as_ref())?;
            debug!(
                "Capture device: {}",
                display_name(&device).unwrap_or_default()
            );
            // A device that does not open at the session's rate is opened at
            // the rate it offers, and converted below
            let device_rate = input_open_rate(&device, sample_rate, needed);
            let device_channels = open_channel_count(&device, true, device_rate, needed) as usize;
            let stream_config = StreamConfig {
                channels: device_channels as u16,
                sample_rate: device_rate,
                buffer_size: cpal::BufferSize::Fixed(frame_size),
            };
            let mut deliver = capture_delivery(
                callback,
                picks,
                Shape {
                    device_rate,
                    device_channels,
                    sample_rate,
                    frame_size,
                },
                heartbeat,
                serial,
            )?;

            // Callback is moved directly (no Arc) - allows FnMut without Sync requirement
            let stream = device
                .build_input_stream(
                    stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        super::priority::promote_callback_thread("capture");
                        deliver(data);
                    },
                    err_fn,
                    None,
                )
                .map_err(stream_open_error)?;

            stream
                .play()
                .map_err(|e| AudioError::StreamError(e.to_string()))?;
            debug!("Capture opened with {} device channel(s)", device_channels);
            Ok(HeldStream::Cpal {
                _stream: stream,
                _way: through_wasapi.then(|| {
                    note_opened(
                        DeviceSide::Input,
                        note,
                        OpenedWay {
                            asked: mode,
                            opened: WasapiMode::Shared,
                            failure: fell_back.take(),
                        },
                    )
                }),
            })
        })?;
        self.capture_stream = Some(host);
        self.running.store(true, Ordering::SeqCst);

        debug!("Capture started with config: {:?}", self.config);
        Ok(())
    }

    /// Stream error callback: reports a disconnected device as an event so the
    /// app can tell the user, rather than going quiet.
    fn stream_error_handler(&self) -> impl FnMut(cpal::Error) + Send + 'static {
        let event_tx = self.event_tx.clone();
        move |err: cpal::Error| {
            error!("Playback stream error: {:?}", err);
            if matches!(err.kind(), cpal::ErrorKind::Xrun) {
                crate::perf::count_output_xrun();
            }
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
        info!(
            "Starting playback on device: {}",
            device_id.map_or("system default", |id| id.0.as_str())
        );
        self.current_output_device = device_id.cloned();

        let route = self.playback_route;
        let sample_rate = self.config.sample_rate;
        let mode = self.config.wasapi_mode;
        let period = output_period_frames(self.config.frame_size);
        if period != self.config.frame_size {
            info!(
                "Playback opens with a period of {} frames, not the frame size {}",
                period, self.config.frame_size
            );
        }
        let wanted = device_id.cloned();
        let err_fn = self.stream_error_handler();
        #[cfg(target_os = "windows")]
        let on_fault =
            wasapi_fault_handler(self.event_tx.clone(), AudioEvent::OutputDeviceDisconnected);
        let heartbeat = self.heartbeat.clone();
        let serial = fault::stream_serial();
        // Numbered now, so an open that finishes late is told from the one
        // that replaced it
        let note = next_note();

        // Opened on a thread of its own, like capture
        let host = StreamHost::open("output stream", self.open_timeout, move || {
            fault::point(Call::OpenOutput);
            let through_wasapi = opened_through_wasapi(wanted.as_ref());
            let mut fell_back = None;
            // As capture: the ways `cpal` does not open are opened by `wasapi`
            #[cfg(target_os = "windows")]
            let fill_frame = if through_wasapi && mode != WasapiMode::Shared {
                let heartbeat = heartbeat.clone();
                let opened = super::wasapi::open_render(
                    wanted.as_ref(),
                    mode,
                    super::wasapi::Wanted {
                        sample_rate,
                        channels: route.channels_needed(),
                        buffer_frames: period,
                    },
                    fill_frame,
                    move |format, fill_frame| {
                        playback_delivery(
                            fill_frame,
                            frame_samples,
                            route,
                            Shape {
                                device_rate: format.rate,
                                device_channels: format.channels,
                                sample_rate,
                                frame_size: period,
                            },
                            heartbeat,
                            serial,
                        )
                    },
                    on_fault,
                );
                match opened {
                    Ok(stream) => {
                        info!("Playback opened through WASAPI the {:?} way", mode);
                        return Ok(HeldStream::Wasapi {
                            _stream: stream,
                            _way: note_opened(
                                DeviceSide::Output,
                                note,
                                OpenedWay {
                                    asked: mode,
                                    opened: mode,
                                    failure: None,
                                },
                            ),
                        });
                    }
                    Err(super::wasapi::NotOpened {
                        error,
                        failure,
                        callback: Some(fill_frame),
                    }) => {
                        warn!(
                            "Playback did not open the {:?} way ({}); it opens the standard way",
                            mode, error
                        );
                        fell_back = Some(failure);
                        fill_frame
                    }
                    Err(super::wasapi::NotOpened { error, .. }) => return Err(error),
                }
            } else {
                fill_frame
            };

            let device = resolve_output_device(wanted.as_ref())?;
            debug!(
                "Playback device: {}",
                display_name(&device).unwrap_or_default()
            );
            // A device that runs at another rate is opened at its own, and
            // the stereo the source makes is converted to it below
            let device_rate = output_open_rate(&device, sample_rate, route.channels_needed());
            let device_channels =
                open_channel_count(&device, false, device_rate, route.channels_needed()) as usize;
            let stream_config = StreamConfig {
                channels: device_channels as u16,
                sample_rate: device_rate,
                buffer_size: cpal::BufferSize::Fixed(period),
            };
            let mut deliver = playback_delivery(
                fill_frame,
                frame_samples,
                route,
                Shape {
                    device_rate,
                    device_channels,
                    sample_rate,
                    frame_size: period,
                },
                heartbeat,
                serial,
            )?;

            let stream = device
                .build_output_stream(
                    stream_config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        super::priority::promote_callback_thread("playback");
                        deliver(data);
                    },
                    err_fn,
                    None,
                )
                .map_err(stream_open_error)?;

            stream
                .play()
                .map_err(|e| AudioError::StreamError(e.to_string()))?;
            Ok(HeldStream::Cpal {
                _stream: stream,
                _way: through_wasapi.then(|| {
                    note_opened(
                        DeviceSide::Output,
                        note,
                        OpenedWay {
                            asked: mode,
                            opened: WasapiMode::Shared,
                            failure: fell_back.take(),
                        },
                    )
                }),
            })
        })?;
        self.playback_stream = Some(host);

        debug!("Playback started from a frame source: {:?}", self.config);
        Ok(())
    }

    /// Stop capture
    pub fn stop_capture(&mut self) {
        if let Some(host) = self.capture_stream.take() {
            host.close("input stream");
        }
        info!("Capture stopped");
    }

    /// Stop playback
    pub fn stop_playback(&mut self) {
        if let Some(host) = self.playback_stream.take() {
            host.close("output stream");
        }
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

/// What a device refusing to build a stream means to the caller: a format it
/// will not open in (`UnsupportedConfig`) is told apart, because the user can
/// change it, from a stream that failed for any other reason.
fn stream_open_error(error: cpal::Error) -> AudioError {
    match error.kind() {
        cpal::ErrorKind::UnsupportedConfig => AudioError::UnsupportedConfig(error.to_string()),
        _ => AudioError::StreamError(error.to_string()),
    }
}

/// A stream the engine holds open. Dropping it closes the stream, and
/// forgets how its device was opened.
enum HeldStream {
    Cpal {
        _stream: cpal::Stream,
        _way: Option<Noted>,
    },
    #[cfg(target_os = "windows")]
    Wasapi {
        _stream: super::wasapi::Stream,
        _way: Noted,
    },
}

/// The format a device was opened in, and the session's
struct Shape {
    device_rate: u32,
    device_channels: usize,
    sample_rate: u32,
    /// Capture: the session's frame size. Playback: the device's period.
    frame_size: u32,
}

/// What capture does with each period a device delivers, whichever way it
/// was opened: takes the channels the session uses, converts them to the
/// session's rate, and hands them to `callback` with the count of samples
/// delivered before them.
fn capture_delivery<F>(
    mut callback: F,
    picks: Vec<usize>,
    shape: Shape,
    heartbeat: Heartbeat,
    serial: u64,
) -> Result<impl FnMut(&[f32]) + Send + 'static, AudioError>
where
    F: FnMut(&[f32], u64) + Send + 'static,
{
    let Shape {
        device_rate,
        device_channels,
        sample_rate,
        frame_size,
    } = shape;
    let mut resampler = if device_rate == sample_rate {
        None
    } else {
        let resampler = CaptureResampler::new(device_rate, sample_rate, picks.len())
            .map_err(|e| AudioError::UnsupportedConfig(e.to_string()))?;
        info!(
            "Capture opens at {} Hz and is converted to the session's {} Hz ({} frames of delay)",
            device_rate,
            sample_rate,
            resampler.delay_frames()
        );
        Some(resampler)
    };
    // A device frame that is already the frame wanted goes through as is
    let whole_frame = resampler.is_none() && picks.iter().copied().eq(0..device_channels);
    let mut picked = Vec::with_capacity(frame_size as usize * picks.len().max(1) * 4);
    let mut sample_count = 0u64;
    Ok(move |data: &[f32]| {
        if fault::stream_stalled(serial) {
            return;
        }
        heartbeat.beat();
        let started = crate::perf::start();
        super::device_loop::on_input(data, device_channels);
        if whole_frame {
            let timestamp = sample_count;
            sample_count += data.len() as u64;
            callback(data, timestamp);
        } else {
            pick_channels(data, device_channels, &picks, &mut picked);
            match resampler.as_mut() {
                None => {
                    let timestamp = sample_count;
                    sample_count += picked.len() as u64;
                    callback(&picked, timestamp);
                }
                Some(resampler) => resampler.process(&picked, |converted| {
                    let timestamp = sample_count;
                    sample_count += converted.len() as u64;
                    callback(converted, timestamp);
                }),
            }
        }
        crate::perf::INPUT_CALLBACK.stop(started);
    })
}

/// What playback does with each period a device asks for, whichever way it
/// was opened: pulls frames from `fill_frame`, converts them to the device's
/// rate, and writes them to the channels `route` names.
fn playback_delivery<F>(
    fill_frame: F,
    frame_samples: usize,
    route: OutputRoute,
    shape: Shape,
    heartbeat: Heartbeat,
    serial: u64,
) -> Result<impl FnMut(&mut [f32]) + Send + 'static, AudioError>
where
    F: FnMut(&mut [f32]) -> usize + Send + 'static,
{
    let Shape {
        device_rate,
        device_channels,
        sample_rate,
        frame_size: period,
    } = shape;
    let source = if device_rate == sample_rate {
        FramePuller::new(frame_samples, fill_frame)
    } else {
        let source = FramePuller::converting(frame_samples, fill_frame, sample_rate, device_rate)
            .map_err(|e| AudioError::UnsupportedConfig(e.to_string()))?;
        info!(
            "Playback opens at {} Hz and is converted from the session's {} Hz",
            device_rate, sample_rate
        );
        source
    };
    let mut puller = RoutedPuller {
        puller: source,
        stereo: vec![0.0; period as usize * 2 * 4],
        device_channels,
        route,
        sample_rate: device_rate,
    };
    Ok(move |data: &mut [f32]| {
        if fault::stream_stalled(serial) {
            data.fill(0.0);
            return;
        }
        heartbeat.beat();
        let started = crate::perf::start();
        puller.fill(data);
        crate::perf::OUTPUT_CALLBACK.stop(started);
    })
}

/// What a WASAPI stream jamuru opened itself reports when it stops: `lost`
/// when its device went away, a stream error otherwise
#[cfg(target_os = "windows")]
fn wasapi_fault_handler(
    event_tx: Option<Sender<AudioEvent>>,
    lost: AudioEvent,
) -> impl FnMut(super::wasapi::Fault) + Send + 'static {
    move |fault| {
        let event = match fault {
            super::wasapi::Fault::Lost => {
                warn!("A WASAPI device went away: {:?}", lost);
                lost.clone()
            }
            super::wasapi::Fault::Failed(message) => {
                error!("WASAPI stream error: {}", message);
                AudioEvent::StreamError(message)
            }
        };
        if let Some(ref tx) = event_tx {
            let _ = tx.send(event);
        }
    }
}

/// The channel count to open `device` with to reach channel number `needed`:
/// the smallest one it offers that does. A device that does not say what it
/// offers is asked for `needed` and left to refuse.
fn open_channel_count(device: &cpal::Device, input: bool, sample_rate: u32, needed: usize) -> u16 {
    let offered = offered_channel_counts(device, input, sample_rate);
    smallest_channel_count(&offered, needed).unwrap_or(needed as u16)
}

/// The smallest output period PipeWire's ALSA plugin starts playback with.
/// It raises a smaller period to this internally and raises no POLLOUT until
/// that much is free, so a stream opened at two periods of less (cpal's start
/// threshold) never gets past its first write and plays nothing.
pub const LINUX_MIN_OUTPUT_PERIOD: u32 = 64;

/// How many frames the output device is asked for at a time. The frame source
/// carries the remainder of a request over to the next (`FramePuller`), so a
/// period longer than the session's frame costs no samples; PipeWire's period
/// floor is this value already, so it costs no latency there either.
fn output_period_frames(frame_size: u32) -> u32 {
    if cfg!(target_os = "linux") {
        frame_size.max(LINUX_MIN_OUTPUT_PERIOD)
    } else {
        frame_size
    }
}

/// Hands the device the stereo a frame source produces, on the channels the
/// output route names.
pub(crate) struct RoutedPuller<F> {
    puller: FramePuller<F>,
    /// The stereo for one request, before it is spread over the device's channels.
    stereo: Vec<f32>,
    device_channels: usize,
    route: OutputRoute,
    sample_rate: u32,
}

impl<F: FnMut(&mut [f32]) -> usize> RoutedPuller<F> {
    /// `stereo_len` is the exact size the stereo scratch buffer needs to be
    /// for a `fill` call of `device_channels`-wide requests, so the caller
    /// sizes it once and `fill` never has to grow it (see the field doc on
    /// `stereo`, and `AsioDuplex` in `asio.rs` for such a caller).
    ///
    /// Only used by `asio.rs` (Windows only); the `cpal` path below builds
    /// `RoutedPuller` as a struct literal directly.
    #[cfg(target_os = "windows")]
    pub(crate) fn new(
        puller: FramePuller<F>,
        stereo_len: usize,
        device_channels: usize,
        route: OutputRoute,
        sample_rate: u32,
    ) -> Self {
        Self {
            puller,
            stereo: vec![0.0; stereo_len],
            device_channels,
            route,
            sample_rate,
        }
    }

    pub(crate) fn fill(&mut self, data: &mut [f32]) {
        if self.device_channels == 2 && self.route.is_default() {
            self.puller.fill(data);
            super::device_loop::on_output(data, self.sample_rate);
            return;
        }
        let stereo_len = data.len() / self.device_channels * 2;
        if self.stereo.len() < stereo_len {
            self.stereo.resize(stereo_len, 0.0);
        }
        let stereo = &mut self.stereo[..stereo_len];
        self.puller.fill(stereo);
        super::device_loop::on_output(stereo, self.sample_rate);
        place_stereo(stereo, data, self.device_channels, self.route);
    }
}

/// Hands device-sized requests what a frame source produces, in frames.
///
/// The source is asked for a frame only when the device has used up the last
/// one, so nothing is fetched ahead of the clock. When the device runs at
/// another rate than the source makes audio at, each frame is converted to the
/// device's rate before it is handed on.
pub(crate) struct FramePuller<F> {
    fill_frame: F,
    frame: Vec<f32>,
    /// Present when the device's rate is not the source's
    conversion: Option<Conversion>,
    filled: usize,
    taken: usize,
}

/// A source frame converted to the device's rate, waiting to be taken
struct Conversion {
    resampler: CaptureResampler,
    converted: Vec<f32>,
}

/// Interleaved channels of what a frame source makes
const SOURCE_CHANNELS: usize = 2;

impl<F: FnMut(&mut [f32]) -> usize> FramePuller<F> {
    pub(crate) fn new(frame_samples: usize, fill_frame: F) -> Self {
        // Frames vary in length - a peer runs at its own buffer size and
        // sample rate - so the buffer is sized for the widest of them and the
        // source reports how much of it it filled.
        Self {
            fill_frame,
            frame: vec![0.0f32; widest_frame_samples(frame_samples)],
            conversion: None,
            filled: 0,
            taken: 0,
        }
    }

    /// A puller whose requests are in `device_rate`, from a source that makes
    /// stereo at `source_rate`.
    pub(crate) fn converting(
        frame_samples: usize,
        fill_frame: F,
        source_rate: u32,
        device_rate: u32,
    ) -> Result<Self, ResamplerError> {
        let resampler = CaptureResampler::new(source_rate, device_rate, SOURCE_CHANNELS)?;
        let mut puller = Self::new(frame_samples, fill_frame);
        // The widest frame, converted, with room for the chunk the resampler
        // holds back: the callback never grows it
        let widest_frames = puller.frame.len() / SOURCE_CHANNELS + CaptureResampler::CHUNK_FRAMES;
        let reserve = (widest_frames * device_rate as usize).div_ceil(source_rate as usize)
            + CaptureResampler::CHUNK_FRAMES;
        puller.conversion = Some(Conversion {
            resampler,
            converted: Vec::with_capacity(reserve * SOURCE_CHANNELS),
        });
        Ok(puller)
    }

    /// Makes the next frame ready to take. False when the source has nothing
    /// to say.
    fn refill(&mut self) -> bool {
        self.taken = 0;
        loop {
            let made = (self.fill_frame)(&mut self.frame).min(self.frame.len());
            let Some(conversion) = self.conversion.as_mut() else {
                self.filled = made;
                return made > 0;
            };
            if made == 0 {
                self.filled = 0;
                return false;
            }
            conversion.converted.clear();
            let converted = &mut conversion.converted;
            conversion
                .resampler
                .process(&self.frame[..made], |out| converted.extend_from_slice(out));
            // The resampler works in chunks, so a short frame can yield nothing
            // yet: take the next one rather than call that silence
            if !conversion.converted.is_empty() {
                self.filled = conversion.converted.len();
                return true;
            }
        }
    }

    fn fill(&mut self, data: &mut [f32]) {
        let mut written = 0;
        while written < data.len() {
            if self.taken == self.filled && !self.refill() {
                // A source with nothing to say still has to leave the
                // device with samples.
                data[written..].fill(0.0);
                return;
            }
            let ready = match &self.conversion {
                Some(conversion) => &conversion.converted,
                None => &self.frame,
            };
            let n = (data.len() - written).min(self.filled - self.taken);
            data[written..written + n].copy_from_slice(&ready[self.taken..self.taken + n]);
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

    /// The engine is handed between threads (the session thread and the
    /// threads that open a device off it), so it must stay `Send`. Checked
    /// when this compiles; there is nothing to assert at run time.
    #[test]
    fn the_engine_can_be_sent_to_another_thread() {
        fn is_send<T: Send>() {}
        is_send::<AudioEngine>();
    }

    /// A source making a 1 kHz tone on the left channel and silence on the
    /// right, `frame_samples` interleaved samples at a time, at 48000 Hz.
    fn left_tone(frame_samples: usize) -> impl FnMut(&mut [f32]) -> usize {
        let mut frame_index = 0usize;
        move |out: &mut [f32]| {
            for frame in out[..frame_samples].as_chunks_mut::<2>().0 {
                frame[0] = (std::f32::consts::TAU * 1000.0 * frame_index as f32 / 48000.0).sin();
                frame[1] = 0.0;
                frame_index += 1;
            }
            frame_samples
        }
    }

    /// Plays `seconds` of the source through `puller`, `request` samples per
    /// device callback, and returns what the device was handed.
    fn play(
        puller: &mut FramePuller<impl FnMut(&mut [f32]) -> usize>,
        device_rate: u32,
        seconds: u32,
        request: usize,
    ) -> Vec<f32> {
        let total = device_rate as usize * 2 * seconds as usize;
        let mut played = Vec::with_capacity(total);
        let mut data = vec![0.0f32; request];
        while played.len() < total {
            puller.fill(&mut data);
            played.extend_from_slice(&data);
        }
        played
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_the_device_runs_at_half_the_session_rate_the_tone_keeps_its_pitch_and_its_channel() {
        let mut puller = FramePuller::converting(128, left_tone(128), 48000, 24000).unwrap();

        let played = play(&mut puller, 24000, 1, 240);

        let left: Vec<f32> = played.iter().step_by(2).copied().collect();
        let right = played.iter().skip(1).step_by(2);
        let rising = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (999..=1001).contains(&rising),
            "{rising} cycles in a second of a 1 kHz tone"
        );
        assert!(right.into_iter().all(|x| x.abs() < 1e-6));
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_the_device_runs_above_the_session_rate_the_tone_keeps_its_pitch() {
        let mut puller = FramePuller::converting(128, left_tone(128), 48000, 96000).unwrap();

        let played = play(&mut puller, 96000, 1, 960);

        let left: Vec<f32> = played.iter().step_by(2).copied().collect();
        let rising = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (999..=1001).contains(&rising),
            "{rising} cycles in a second of a 1 kHz tone"
        );
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_the_source_makes_less_than_a_conversion_chunk_the_device_still_gets_the_audio() {
        // 3 frames a call: the resampler needs 8 to produce anything
        let mut puller = FramePuller::converting(6, left_tone(6), 48000, 24000).unwrap();

        let played = play(&mut puller, 24000, 1, 240);

        let silent = played.iter().filter(|x| x.abs() < 1e-6).count();
        assert!(
            silent < played.len() * 3 / 4,
            "{silent} of {} samples are silence",
            played.len()
        );
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_the_source_has_nothing_to_say_the_converted_device_is_left_silent() {
        let mut puller = FramePuller::converting(128, |_: &mut [f32]| 0, 48000, 24000).unwrap();
        let mut data = vec![1.0f32; 240];

        puller.fill(&mut data);

        assert!(data.iter().all(|x| *x == 0.0));
    }

    /// Verifies: REQ-AUD-127
    #[test]
    fn when_playing_through_a_conversion_the_device_callback_does_not_grow_its_buffers() {
        let mut puller = FramePuller::converting(128, left_tone(128), 48000, 44100).unwrap();
        let reserved = puller.conversion.as_ref().unwrap().converted.capacity();

        play(&mut puller, 44100, 1, 441 * 2);

        assert_eq!(
            puller.conversion.as_ref().unwrap().converted.capacity(),
            reserved
        );
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn opening_capture_on_a_driver_that_hangs_gives_up_at_the_limit_and_says_so() {
        let mut engine = AudioEngine::new(AudioConfig::default());
        engine.set_open_timeout(Duration::from_millis(100));
        fault::stall(Call::OpenInput, Duration::from_secs(3));
        let started = std::time::Instant::now();

        let result = engine.start_capture(None, |_, _| {});

        fault::stall(Call::OpenInput, Duration::ZERO);
        assert!(matches!(result, Err(AudioError::DeviceUnresponsive(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!engine.is_capture_running());
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn opening_playback_on_a_driver_that_hangs_gives_up_at_the_limit_and_says_so() {
        let mut engine = AudioEngine::new(AudioConfig::default());
        engine.set_open_timeout(Duration::from_millis(100));
        fault::stall(Call::OpenOutput, Duration::from_secs(3));
        let started = std::time::Instant::now();

        let result = engine.start_playback_with_source(None, 64, |_| 0);

        fault::stall(Call::OpenOutput, Duration::ZERO);
        assert!(matches!(result, Err(AudioError::DeviceUnresponsive(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!engine.is_playback_running());
    }

    /// PipeWire's ALSA plugin raises no POLLOUT for a period under 64 frames,
    /// so a stream opened at 32 or 48 never plays (cpal 0.18, pipewire 1.4.2).
    ///
    /// Verifies: REQ-AUD-036
    #[cfg(target_os = "linux")]
    #[test]
    fn on_linux_an_output_period_is_never_under_64_frames() {
        assert_eq!(output_period_frames(32), 64);
        assert_eq!(output_period_frames(48), 64);
        assert_eq!(output_period_frames(64), 64);
        assert_eq!(output_period_frames(128), 128);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn off_linux_the_output_period_is_the_frame_size() {
        assert_eq!(output_period_frames(32), 32);
    }

    /// The period alone proves nothing about the device: this opens the
    /// system's default output at a frame size of 32 and counts how often the
    /// device asks for samples. Skipped where there is no output device to
    /// open (CI without PipeWire).
    ///
    /// Verifies: REQ-AUD-036
    #[cfg(target_os = "linux")]
    #[test]
    fn on_linux_playback_at_a_frame_size_of_32_is_asked_for_samples() {
        use cpal::traits::HostTrait;
        if cpal::default_host().default_output_device().is_none() {
            eprintln!("skipped: no default output device");
            return;
        }
        for frame_size in [32u32, 48] {
            let mut engine = AudioEngine::new(AudioConfig {
                frame_size,
                ..AudioConfig::default()
            });
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = calls.clone();
            let opened =
                engine.start_playback_with_source(None, frame_size as usize * 2, move |_| {
                    counted.fetch_add(1, Ordering::Relaxed);
                    0
                });
            if opened.is_err() {
                eprintln!("skipped: the default output device does not open");
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
            engine.stop_playback();
            let asked = calls.load(Ordering::Relaxed);
            assert!(
                asked > 10,
                "frame size {frame_size}: the device asked the source {asked} times in 1 s"
            );
        }
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

    /// A peer runs at its own buffer size, so its frames can be several times
    /// the length of ours. The source has to be given room for one, or the
    /// end of every frame is cut off.
    ///
    /// Verifies: REQ-AUD-038
    #[test]
    fn a_source_frame_several_times_the_nominal_length_reaches_the_device_whole() {
        const NOMINAL: usize = 4;
        const THEIRS: usize = NOMINAL * 8;
        let mut next = 0.0f32;
        let mut puller = FramePuller::new(NOMINAL, move |out: &mut [f32]| {
            assert!(out.len() >= THEIRS, "the source has room for its frame");
            for sample in &mut out[..THEIRS] {
                *sample = next;
                next += 1.0;
            }
            THEIRS
        });

        let mut heard = vec![f32::NAN; THEIRS + 8];
        puller.fill(&mut heard);

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
            sample_rate: 48_000,
        };
        let mut data = vec![f32::NAN; 16];

        puller.fill(&mut data);

        let mut expected = vec![0.0; 16];
        expected[4..6].copy_from_slice(&[0.0, 1.0]);
        expected[12..14].copy_from_slice(&[2.0, 3.0]);
        assert_eq!(data, expected);
    }
}
