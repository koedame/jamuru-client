//! WASAPI streams jamuru opens itself (Windows only), for the two ways of
//! opening a device that `cpal` does not offer: exclusive mode, and the shared
//! mode at the smallest period the driver allows (`IAudioClient3`). What each
//! way does, and which is advised where: `wasapi_mode` and ADR-088.
//!
//! A stream runs on a thread of its own, woken by the device's event once a
//! period, and hands its callback interleaved `f32` samples the way `cpal`
//! does, so the engine's callbacks run on it unchanged. The thread opens the
//! device itself (the COM objects stay on the thread that made them) and says
//! whether it did before it starts.
//!
//! Also reads what a device offers without opening a stream ([`probe`]), for
//! the settings to advise on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use windows::core::{implement, Interface, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, PROPERTYKEY, S_OK, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, ERole, IAudioCaptureClient, IAudioClient,
    IAudioClient3, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_INVALIDATED,
    AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_ENGINE_PERIODICITY_LOCKED,
    AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, AUDCLNT_E_UNSUPPORTED_FORMAT,
    AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, WAVE_FORMAT_PCM,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::device::DeviceId;
use super::driver::{bounded, LIST_TIMEOUT};
use super::error::AudioError;
use super::open::DeviceSide;
use super::wasapi_mode::{
    low_latency_period, EndpointCaps, OpenFailure, SampleLayout, SharedPeriods, WasapiMode,
    WASAPI_ID_PREFIX,
};
use crate::config::VALID_SAMPLE_RATES;

/// The formats tried for an exclusive stream, best first: a layout and how
/// many of its bits carry the signal
const EXCLUSIVE_FORMATS: [(SampleLayout, u16); 5] = [
    (SampleLayout::F32, 32),
    (SampleLayout::I32, 24),
    (SampleLayout::I32, 32),
    (SampleLayout::I24, 24),
    (SampleLayout::I16, 16),
];

/// How long a stream's thread waits for the device's event before it looks
/// at whether it has been told to stop, and asks the device whether it is
/// still there
const EVENT_WAIT_MS: u32 = 100;

/// Which way a stream carries audio
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Flow {
    Capture,
    Render,
}

/// The format a stream was opened in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Format {
    pub rate: u32,
    pub channels: usize,
}

/// What is wanted of a stream
#[derive(Debug, Clone, Copy)]
pub(crate) struct Wanted {
    /// The session's rate, opened at when the device offers it
    pub sample_rate: u32,
    /// The fewest channels the stream must have
    pub channels: usize,
    /// The session's frame size, in frames at `sample_rate`
    pub buffer_frames: u32,
}

/// What went wrong with a stream that was running
pub(crate) enum Fault {
    /// The device went away (unplugged, or disabled in the sound settings)
    Lost,
    Failed(String),
}

/// A stream that did not open: why, and the callback it was given, for
/// opening the device another way. `callback` is `None` when the stream got
/// as far as using it.
pub(crate) struct NotOpened<F> {
    pub error: AudioError,
    pub failure: OpenFailure,
    pub callback: Option<F>,
}

/// A running stream. Dropping it stops the stream and waits for its thread.
pub(crate) struct Stream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens a capture stream on `endpoint` (`None`: the system default) the
/// `mode` way. `make` turns the format the stream opened in and `callback`
/// into what is handed each period's samples, on the stream's thread.
pub(crate) fn open_capture<F, C>(
    endpoint: Option<&DeviceId>,
    mode: WasapiMode,
    wanted: Wanted,
    callback: F,
    make: impl FnOnce(Format, F) -> Result<C, AudioError> + Send + 'static,
    on_fault: impl FnMut(Fault) + Send + 'static,
) -> Result<Stream, NotOpened<F>>
where
    F: Send + 'static,
    C: FnMut(&[f32]) + 'static,
{
    spawn(
        Flow::Capture,
        endpoint,
        mode,
        wanted,
        callback,
        make,
        on_fault,
        capture_loop,
    )
}

/// Opens a render stream on `endpoint` the `mode` way, as [`open_capture`].
/// What `make` makes fills each period's samples.
pub(crate) fn open_render<F, C>(
    endpoint: Option<&DeviceId>,
    mode: WasapiMode,
    wanted: Wanted,
    callback: F,
    make: impl FnOnce(Format, F) -> Result<C, AudioError> + Send + 'static,
    on_fault: impl FnMut(Fault) + Send + 'static,
) -> Result<Stream, NotOpened<F>>
where
    F: Send + 'static,
    C: FnMut(&mut [f32]) + 'static,
{
    spawn(
        Flow::Render,
        endpoint,
        mode,
        wanted,
        callback,
        make,
        on_fault,
        render_loop,
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn<F, C, Fa>(
    flow: Flow,
    endpoint: Option<&DeviceId>,
    mode: WasapiMode,
    wanted: Wanted,
    callback: F,
    make: impl FnOnce(Format, F) -> Result<C, AudioError> + Send + 'static,
    on_fault: Fa,
    run: fn(&Opened, C, &AtomicBool, Fa),
) -> Result<Stream, NotOpened<F>>
where
    F: Send + 'static,
    C: 'static,
    Fa: FnMut(Fault) + Send + 'static,
{
    let endpoint = endpoint.map(|id| endpoint_id(&id.0));
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), NotOpened<F>>>();
    let thread_stop = stop.clone();
    let name = match flow {
        Flow::Capture => "audio-wasapi-capture",
        Flow::Render => "audio-wasapi-render",
    };
    let spawned = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let not_opened = |(error, failure): (AudioError, OpenFailure), callback| NotOpened {
                error,
                failure,
                callback,
            };
            let _com = match Com::enter() {
                Ok(com) => com,
                Err(e) => {
                    let _ = ready_tx.send(Err(not_opened(e, Some(callback))));
                    return;
                }
            };
            let opened = match open_client(flow, endpoint.as_deref(), mode, wanted) {
                Ok(opened) => opened,
                Err(e) => {
                    let _ = ready_tx.send(Err(not_opened(e, Some(callback))));
                    return;
                }
            };
            let process = match make(opened.format, callback) {
                Ok(process) => process,
                Err(error) => {
                    let _ = ready_tx.send(Err(not_opened((error, OpenFailure::Other), None)));
                    return;
                }
            };
            // SAFETY: the client was initialized above, on this thread
            if let Err(e) = unsafe { opened.client.Start() } {
                let _ = ready_tx.send(Err(not_opened(failure_of("start", &e), None)));
                return;
            }
            if ready_tx.send(Ok(())).is_ok() {
                super::priority::promote_callback_thread(match flow {
                    Flow::Capture => "capture",
                    Flow::Render => "playback",
                });
                run(&opened, process, &thread_stop, on_fault);
            }
            // SAFETY: as above
            let _ = unsafe { opened.client.Stop() };
        });
    let thread = match spawned {
        Ok(thread) => thread,
        Err(e) => {
            return Err(NotOpened {
                error: AudioError::DeviceOpenFailed(format!("{}: {}", name, e)),
                failure: OpenFailure::Other,
                callback: None,
            })
        }
    };
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Stream {
            stop,
            thread: Some(thread),
        }),
        Ok(Err(not_opened)) => {
            let _ = thread.join();
            Err(not_opened)
        }
        Err(_) => Err(NotOpened {
            error: AudioError::StreamError(format!("{} stopped while opening", name)),
            failure: OpenFailure::Other,
            callback: None,
        }),
    }
}

/// The endpoint id in a WASAPI device's id
fn endpoint_id(id: &str) -> String {
    id.strip_prefix(WASAPI_ID_PREFIX).unwrap_or(id).to_string()
}

/// The multithreaded COM apartment, entered on the current thread and left
/// on drop (on the same thread: it is not `Send`)
struct Com(std::marker::PhantomData<*const ()>);

impl Com {
    fn enter() -> Result<Self, (AudioError, OpenFailure)> {
        // SAFETY: paired with `CoUninitialize` in `drop`, on this thread
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|e| failure_of("COM", &e))?;
        Ok(Self(std::marker::PhantomData))
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: the apartment was entered on this thread in `enter`
        unsafe { CoUninitialize() };
    }
}

/// An event the device sets once a period, closed on drop
struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self, (AudioError, OpenFailure)> {
        // SAFETY: an unnamed auto-reset event, owned by the value returned
        unsafe { CreateEventW(None, false, false, None) }
            .map(Event)
            .map_err(|e| failure_of("event", &e))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle was created in `new` and is closed only here
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A stream opened and ready to start
struct Opened {
    client: IAudioClient,
    io: Io,
    event: Event,
    layout: SampleLayout,
    format: Format,
    /// The frames the device's buffer holds
    buffer_frames: u32,
    /// Exclusive streams fill or empty the whole buffer each period
    exclusive: bool,
    /// For a stream on the system default device: whether Windows has
    /// changed its default since
    watch: Option<DefaultWatch>,
}

enum Io {
    Capture(IAudioCaptureClient),
    Render(IAudioRenderClient),
}

/// Tells a stream opened on the system default device that Windows has
/// changed its default. The stream then stops with an error and is reopened,
/// on the new default, as a `cpal` stream on the default device is.
struct DefaultWatch {
    enumerator: IMMDeviceEnumerator,
    client: IMMNotificationClient,
    changed: Arc<AtomicBool>,
}

impl DefaultWatch {
    fn start(flow: Flow) -> Result<Self, (AudioError, OpenFailure)> {
        let changed = Arc::new(AtomicBool::new(false));
        let client: IMMNotificationClient = DefaultChanged {
            flow: data_flow(flow),
            changed: changed.clone(),
        }
        .into();
        // SAFETY: COM calls on a thread in the apartment; the client stays
        // registered until `drop`
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| failure_of("device enumerator", &e))?;
            enumerator
                .RegisterEndpointNotificationCallback(&client)
                .map_err(|e| failure_of("default device notifications", &e))?;
            Ok(Self {
                enumerator,
                client,
                changed,
            })
        }
    }

    fn changed(&self) -> bool {
        self.changed.load(Ordering::Relaxed)
    }
}

impl Drop for DefaultWatch {
    fn drop(&mut self) {
        // SAFETY: unregisters what `start` registered. Waits for a
        // notification in progress, which only sets a flag.
        let _ = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.client)
        };
    }
}

#[implement(IMMNotificationClient)]
struct DefaultChanged {
    flow: EDataFlow,
    changed: Arc<AtomicBool>,
}

impl IMMNotificationClient_Impl for DefaultChanged_Impl {
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _id: &PCWSTR,
    ) -> windows::core::Result<()> {
        // The role the stream was opened for (`device_of`)
        if flow == self.flow && role == eConsole {
            self.changed.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    fn OnDeviceStateChanged(
        &self,
        _id: &PCWSTR,
        _state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

fn data_flow(flow: Flow) -> EDataFlow {
    match flow {
        Flow::Capture => eCapture,
        Flow::Render => eRender,
    }
}

/// What a WASAPI error means for the person, with the call that made it
fn failure_of(call: &str, error: &windows::core::Error) -> (AudioError, OpenFailure) {
    let code = error.code();
    let failure = if code == AUDCLNT_E_DEVICE_IN_USE {
        OpenFailure::InUse
    } else if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
        OpenFailure::NotAllowed
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
        OpenFailure::FormatRefused
    } else {
        OpenFailure::Other
    };
    let message = format!(
        "WASAPI {}: {} (0x{:08X})",
        call,
        error.message(),
        code.0 as u32
    );
    let error = if failure == OpenFailure::FormatRefused {
        AudioError::UnsupportedConfig(message)
    } else {
        AudioError::DeviceOpenFailed(message)
    };
    (error, failure)
}

fn device_of(flow: Flow, endpoint: Option<&str>) -> Result<IMMDevice, (AudioError, OpenFailure)> {
    // SAFETY: COM calls on a thread in the apartment
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| failure_of("device enumerator", &e))?;
        match endpoint {
            Some(id) => enumerator.GetDevice(&HSTRING::from(id)),
            None => enumerator.GetDefaultAudioEndpoint(data_flow(flow), eConsole),
        }
        .map_err(|e| {
            let (_, failure) = failure_of("device", &e);
            (AudioError::DeviceNotFound(e.message().to_string()), failure)
        })
    }
}

fn activate<T: Interface>(device: &IMMDevice) -> Result<T, (AudioError, OpenFailure)> {
    // SAFETY: COM call on a thread in the apartment
    unsafe { device.Activate::<T>(CLSCTX_ALL, None) }.map_err(|e| failure_of("activate", &e))
}

/// The format the system mixes a device in
struct MixFormat {
    rate: u32,
    channels: u16,
    channel_mask: u32,
    /// `None` when the mixer's format is not one a stream is opened in here
    layout: Option<(SampleLayout, u16)>,
}

impl MixFormat {
    /// Reads the format at `format`, which the mixer handed out
    ///
    /// # Safety
    /// `format` points to a `WAVEFORMATEX`, and to a `WAVEFORMATEXTENSIBLE`
    /// when its tag says so.
    unsafe fn read(format: *const WAVEFORMATEX) -> Self {
        let base = unsafe { std::ptr::read_unaligned(format) };
        let (tag, bits, channels, rate) = (
            u32::from(base.wFormatTag),
            base.wBitsPerSample,
            base.nChannels,
            base.nSamplesPerSec,
        );
        let (float, valid, channel_mask) = if tag == WAVE_FORMAT_EXTENSIBLE {
            let ext = unsafe { std::ptr::read_unaligned(format as *const WAVEFORMATEXTENSIBLE) };
            let sub = ext.SubFormat;
            let valid = unsafe { ext.Samples.wValidBitsPerSample };
            let float = if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                Some(true)
            } else if sub == KSDATAFORMAT_SUBTYPE_PCM {
                Some(false)
            } else {
                None
            };
            (float, valid, ext.dwChannelMask)
        } else {
            let float = match tag {
                WAVE_FORMAT_IEEE_FLOAT => Some(true),
                WAVE_FORMAT_PCM => Some(false),
                _ => None,
            };
            (float, bits, default_channel_mask(channels))
        };
        let layout = match (float, bits) {
            (Some(true), 32) => Some((SampleLayout::F32, 32)),
            (Some(false), 32) => Some((SampleLayout::I32, valid)),
            (Some(false), 24) => Some((SampleLayout::I24, 24)),
            (Some(false), 16) => Some((SampleLayout::I16, 16)),
            _ => None,
        };
        Self {
            rate,
            channels,
            channel_mask,
            layout,
        }
    }
}

fn default_channel_mask(channels: u16) -> u32 {
    match channels {
        // Front centre
        1 => 0x4,
        n if n >= 32 => u32::MAX,
        n => (1u32 << n) - 1,
    }
}

/// The mixer's format of `client`'s device
fn mix_format(client: &IAudioClient) -> Result<MixFormat, (AudioError, OpenFailure)> {
    // SAFETY: the mixer allocates the format; it is read and then freed here
    unsafe {
        let format = client
            .GetMixFormat()
            .map_err(|e| failure_of("mix format", &e))?;
        let mix = MixFormat::read(format);
        CoTaskMemFree(Some(format as *const _));
        Ok(mix)
    }
}

/// A format to ask a device for in exclusive mode
fn exclusive_format(
    rate: u32,
    channels: u16,
    channel_mask: u32,
    (layout, valid_bits): (SampleLayout, u16),
) -> WAVEFORMATEXTENSIBLE {
    let bits = (layout.bytes() * 8) as u16;
    let block = channels * bits / 8;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: bits,
            cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                - std::mem::size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: valid_bits,
        },
        dwChannelMask: channel_mask,
        SubFormat: if layout == SampleLayout::F32 {
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            KSDATAFORMAT_SUBTYPE_PCM
        },
    }
}

/// The first of `rates` and of [`EXCLUSIVE_FORMATS`] the device opens in
/// exclusive mode, `None` when it opens none of them. An error when the
/// device does not say (another app or this one has it exclusively, or
/// Windows does not let apps take it): that is not an answer about formats.
fn first_exclusive_format(
    client: &IAudioClient,
    rates: &[u32],
    mix: &MixFormat,
) -> windows::core::Result<Option<(u32, (SampleLayout, u16))>> {
    for &rate in rates {
        for format in EXCLUSIVE_FORMATS {
            let asked = exclusive_format(rate, mix.channels, mix.channel_mask, format);
            // SAFETY: `asked` lives across the call; no closest match is asked for
            let answer: HRESULT = unsafe {
                client.IsFormatSupported(
                    AUDCLNT_SHAREMODE_EXCLUSIVE,
                    &asked as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX,
                    None,
                )
            };
            if answer == S_OK {
                return Ok(Some((rate, format)));
            }
            if answer.is_err() && answer != AUDCLNT_E_UNSUPPORTED_FORMAT {
                return Err(answer.into());
            }
        }
    }
    Ok(None)
}

/// 100-nanosecond units for `frames` at `rate`
fn frames_to_hns(frames: u32, rate: u32) -> i64 {
    (10_000_000.0 * f64::from(frames) / f64::from(rate.max(1)) + 0.5) as i64
}

/// 100-nanosecond units that hold `frames` at `rate` whole (rounded up)
fn frames_to_hns_up(frames: u32, rate: u32) -> i64 {
    (10_000_000u64 * u64::from(frames)).div_ceil(u64::from(rate.max(1))) as i64
}

fn open_client(
    flow: Flow,
    endpoint: Option<&str>,
    mode: WasapiMode,
    wanted: Wanted,
) -> Result<Opened, (AudioError, OpenFailure)> {
    let device = device_of(flow, endpoint)?;
    let (client, layout, format) = match mode {
        WasapiMode::Exclusive => open_exclusive(&device, wanted)?,
        WasapiMode::LowLatency => open_low_latency(&device, wanted)?,
        WasapiMode::Shared => {
            return Err((
                AudioError::DeviceOpenFailed("the standard way is opened through cpal".into()),
                OpenFailure::Other,
            ))
        }
    };
    let watch = match endpoint {
        None => Some(DefaultWatch::start(flow)?),
        Some(_) => None,
    };
    let event = Event::new()?;
    // SAFETY: COM calls on the client initialized above, on this thread
    unsafe {
        client
            .SetEventHandle(event.0)
            .map_err(|e| failure_of("event handle", &e))?;
        let buffer_frames = client
            .GetBufferSize()
            .map_err(|e| failure_of("buffer size", &e))?;
        let io = match flow {
            Flow::Capture => Io::Capture(
                client
                    .GetService::<IAudioCaptureClient>()
                    .map_err(|e| failure_of("capture client", &e))?,
            ),
            Flow::Render => {
                let render = client
                    .GetService::<IAudioRenderClient>()
                    .map_err(|e| failure_of("render client", &e))?;
                // Silence in the buffer before the start, so the first
                // period does not play whatever was in it
                if render.GetBuffer(buffer_frames).is_ok() {
                    let _ =
                        render.ReleaseBuffer(buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
                }
                Io::Render(render)
            }
        };
        Ok(Opened {
            client,
            io,
            event,
            layout,
            format,
            buffer_frames,
            exclusive: mode == WasapiMode::Exclusive,
            watch,
        })
    }
}

/// Opens `device` for itself, at its smallest period or the session's frame
/// when that is longer
fn open_exclusive(
    device: &IMMDevice,
    wanted: Wanted,
) -> Result<(IAudioClient, SampleLayout, Format), (AudioError, OpenFailure)> {
    let client: IAudioClient = activate(device)?;
    let mix = mix_format(&client)?;
    if usize::from(mix.channels) < wanted.channels {
        return Err((
            AudioError::UnsupportedConfig(format!(
                "the device has {} channel(s), {} are needed",
                mix.channels, wanted.channels
            )),
            OpenFailure::FormatRefused,
        ));
    }
    // The session's rate, else the mixer's, else any a session runs at; a
    // rate other than the session's is converted by the caller
    let mut rates = vec![wanted.sample_rate, mix.rate];
    rates.extend(VALID_SAMPLE_RATES);
    let (rate, format) = first_exclusive_format(&client, &rates, &mix)
        .map_err(|e| failure_of("exclusive format", &e))?
        .ok_or_else(|| {
            (
                AudioError::UnsupportedConfig(
                    "the device opens in none of the formats asked for in exclusive mode".into(),
                ),
                OpenFailure::FormatRefused,
            )
        })?;
    let mut smallest = 0i64;
    // SAFETY: COM call on a thread in the apartment
    unsafe { client.GetDevicePeriod(None, Some(&mut smallest)) }
        .map_err(|e| failure_of("device period", &e))?;
    // A whole frame, never less than the device's smallest period (in its
    // own units: a hair less is refused)
    let period = smallest.max(frames_to_hns_up(wanted.buffer_frames, wanted.sample_rate));
    let asked = exclusive_format(rate, mix.channels, mix.channel_mask, format);
    let initialize = |client: &IAudioClient, period: i64| {
        // SAFETY: `asked` lives across the call
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_EXCLUSIVE,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                period,
                period,
                &asked as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX,
                None,
            )
        }
    };
    let client = match initialize(&client, period) {
        Ok(()) => client,
        Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
            // The device rounds the buffer to a size of its own; the period
            // is asked again at that size, on a new client
            // SAFETY: the size is still read after a failed initialize
            let frames = unsafe { client.GetBufferSize() }
                .map_err(|e| failure_of("aligned buffer size", &e))?;
            let client: IAudioClient = activate(device)?;
            initialize(&client, frames_to_hns(frames, rate))
                .map_err(|e| failure_of("initialize", &e))?;
            client
        }
        Err(e) => return Err(failure_of("initialize", &e)),
    };
    Ok((
        client,
        format.0,
        Format {
            rate,
            channels: usize::from(mix.channels),
        },
    ))
}

/// Opens `device` shared, at the smallest period the mixer offers that holds
/// the session's frame
fn open_low_latency(
    device: &IMMDevice,
    wanted: Wanted,
) -> Result<(IAudioClient, SampleLayout, Format), (AudioError, OpenFailure)> {
    let client: IAudioClient3 = activate(device)?;
    // SAFETY: the mixer allocates the format; it is used for the calls below
    // and freed before returning
    unsafe {
        let format = client
            .GetMixFormat()
            .map_err(|e| failure_of("mix format", &e))?;
        let opened = (|| {
            let mix = MixFormat::read(format);
            let (layout, _) = mix.layout.ok_or_else(|| {
                (
                    AudioError::UnsupportedConfig(
                        "the mixer's format is not one opened here".into(),
                    ),
                    OpenFailure::FormatRefused,
                )
            })?;
            if usize::from(mix.channels) < wanted.channels {
                return Err((
                    AudioError::UnsupportedConfig(format!(
                        "the device has {} channel(s), {} are needed",
                        mix.channels, wanted.channels
                    )),
                    OpenFailure::FormatRefused,
                ));
            }
            let (mut default, mut fundamental, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
            client
                .GetSharedModeEnginePeriod(
                    format,
                    &mut default,
                    &mut fundamental,
                    &mut min,
                    &mut max,
                )
                .map_err(|e| failure_of("shared periods", &e))?;
            let period = low_latency_period(
                &SharedPeriods {
                    default,
                    fundamental,
                    min,
                    max,
                },
                mix.rate,
                wanted.sample_rate,
                wanted.buffer_frames,
            );
            let client = match client.InitializeSharedAudioStream(
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                period,
                format,
                None,
            ) {
                Ok(()) => client.clone(),
                // Another app runs the mixer at a period of its own; a stream
                // runs at that period, on a new client (one whose initialize
                // failed is not initialized again)
                Err(e) if e.code() == AUDCLNT_E_ENGINE_PERIODICITY_LOCKED => {
                    let mut current_format = std::ptr::null_mut();
                    let mut current = 0u32;
                    client
                        .GetCurrentSharedModeEnginePeriod(&mut current_format, &mut current)
                        .map_err(|e| failure_of("current shared period", &e))?;
                    CoTaskMemFree(Some(current_format as *const _));
                    let retry: IAudioClient3 = activate(device)?;
                    retry
                        .InitializeSharedAudioStream(
                            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                            current,
                            format,
                            None,
                        )
                        .map_err(|e| failure_of("initialize", &e))?;
                    retry
                }
                Err(e) => return Err(failure_of("initialize", &e)),
            };
            Ok((
                client,
                layout,
                Format {
                    rate: mix.rate,
                    channels: usize::from(mix.channels),
                },
            ))
        })();
        CoTaskMemFree(Some(format as *const _));
        let (client, layout, format) = opened?;
        let client: IAudioClient = client.cast().map_err(|e| failure_of("client", &e))?;
        Ok((client, layout, format))
    }
}

/// Waits for the device's next period: `false` when the stream is to stop,
/// or can no longer run (told to `on_fault`). A device whose event stops
/// coming is asked whether it is still there, which notices one unplugged;
/// a stream on the system default stops when Windows changes the default.
fn next_period(opened: &Opened, stop: &AtomicBool, on_fault: &mut impl FnMut(Fault)) -> bool {
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        if opened.watch.as_ref().is_some_and(DefaultWatch::changed) {
            on_fault(Fault::Failed(
                "WASAPI: Windows changed its default device".into(),
            ));
            return false;
        }
        // SAFETY: the event lives as long as `opened`; COM call on the
        // stream's thread
        let gone = match unsafe { WaitForSingleObject(opened.event.0, EVENT_WAIT_MS) } {
            WAIT_OBJECT_0 => return true,
            WAIT_TIMEOUT => unsafe { opened.client.GetCurrentPadding() }.err(),
            _ => Some(windows::core::Error::from_thread()),
        };
        if let Some(error) = gone {
            report(on_fault, &error);
            return false;
        }
    }
}

fn report(on_fault: &mut impl FnMut(Fault), error: &windows::core::Error) {
    if error.code() == AUDCLNT_E_DEVICE_INVALIDATED {
        on_fault(Fault::Lost);
    } else {
        on_fault(Fault::Failed(format!(
            "WASAPI: {} (0x{:08X})",
            error.message(),
            error.code().0 as u32
        )));
    }
}

fn capture_loop<C: FnMut(&[f32])>(
    opened: &Opened,
    mut deliver: C,
    stop: &AtomicBool,
    mut on_fault: impl FnMut(Fault),
) {
    let Io::Capture(capture) = &opened.io else {
        return;
    };
    let channels = opened.format.channels;
    let bytes_per_frame = channels * opened.layout.bytes();
    let mut samples = vec![0.0f32; opened.buffer_frames as usize * channels];
    while next_period(opened, stop, &mut on_fault) {
        loop {
            match next_packet(capture, opened.layout, bytes_per_frame, &mut samples) {
                Ok(Some(n)) => deliver(&samples[..n]),
                Ok(None) => break,
                Err(e) => {
                    report(&mut on_fault, &e);
                    return;
                }
            }
        }
    }
}

/// Reads the next packet the device captured into `samples`. The number of
/// samples read, or `None` when there is no packet yet.
fn next_packet(
    capture: &IAudioCaptureClient,
    layout: SampleLayout,
    bytes_per_frame: usize,
    samples: &mut [f32],
) -> windows::core::Result<Option<usize>> {
    // SAFETY: COM calls on the stream's thread; the buffer handed out is
    // read before it is released, and holds `frames` whole frames
    unsafe {
        if capture.GetNextPacketSize()? == 0 {
            return Ok(None);
        }
        let mut data = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
        let bytes = frames as usize * bytes_per_frame;
        let n = (bytes / layout.bytes()).min(samples.len());
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
            samples[..n].fill(0.0);
        } else {
            layout.decode(std::slice::from_raw_parts(data, bytes), &mut samples[..n]);
        }
        if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
            crate::perf::count_input_xrun();
        }
        capture.ReleaseBuffer(frames)?;
        Ok(Some(n))
    }
}

fn render_loop<C: FnMut(&mut [f32])>(
    opened: &Opened,
    mut fill: C,
    stop: &AtomicBool,
    mut on_fault: impl FnMut(Fault),
) {
    let Io::Render(render) = &opened.io else {
        return;
    };
    let channels = opened.format.channels;
    let bytes_per_frame = channels * opened.layout.bytes();
    let mut samples = vec![0.0f32; opened.buffer_frames as usize * channels];
    while next_period(opened, stop, &mut on_fault) {
        // SAFETY: COM calls on the stream's thread; the buffer handed out is
        // written before it is released
        let result = unsafe {
            let free = if opened.exclusive {
                Ok(opened.buffer_frames)
            } else {
                opened
                    .client
                    .GetCurrentPadding()
                    .map(|queued| opened.buffer_frames.saturating_sub(queued))
            };
            free.and_then(|frames| {
                if frames == 0 {
                    return Ok(());
                }
                let data = render.GetBuffer(frames)?;
                let n = frames as usize * channels;
                fill(&mut samples[..n]);
                let bytes = std::slice::from_raw_parts_mut(data, frames as usize * bytes_per_frame);
                opened.layout.encode(&samples[..n], bytes);
                render.ReleaseBuffer(frames, 0)
            })
        };
        if let Err(e) = result {
            report(&mut on_fault, &e);
            return;
        }
    }
}

/// What each device last told [`probe`], kept for when it cannot answer
/// (this app has it open exclusively, or its driver is not answering)
static LAST_READ: Mutex<Option<HashMap<(Flow, String), EndpointCaps>>> = Mutex::new(None);

/// Set while a read of the devices is out. A driver that has not answered
/// would hold the next read the same way, so none is made until it does.
static READING: AtomicBool = AtomicBool::new(false);

/// Clears [`READING`] when the read it was made for ends, late or not
struct Read;

impl Drop for Read {
    fn drop(&mut self) {
        READING.store(false, Ordering::SeqCst);
    }
}

/// What the WASAPI devices `ids` offer, read without opening a stream and
/// waited for at most `LIST_TIMEOUT`: the ones that answer, by their id. A
/// device that does not answer is given what it said the last time, if it
/// said anything; so is every device while a read that has not returned is
/// still out.
pub(crate) fn probe(ids: Vec<(DeviceSide, String)>) -> HashMap<String, EndpointCaps> {
    if !READING.swap(true, Ordering::SeqCst) {
        let read = Read;
        let asked = ids.clone();
        let found = bounded("reading the WASAPI devices", LIST_TIMEOUT, move || {
            let _read = read;
            read_now(&asked)
        });
        if let Ok(found) = found {
            return found;
        }
    }
    last_read(&ids)
}

fn key_of(side: DeviceSide, id: &str) -> (Flow, String) {
    let flow = match side {
        DeviceSide::Input => Flow::Capture,
        DeviceSide::Output => Flow::Render,
    };
    (flow, endpoint_id(id))
}

fn read_now(ids: &[(DeviceSide, String)]) -> HashMap<String, EndpointCaps> {
    let Ok(_com) = Com::enter() else {
        return last_read(ids);
    };
    let mut found = HashMap::new();
    for (side, id) in ids {
        let key = key_of(*side, id);
        match probe_one(key.0, &key.1) {
            Ok(caps) => {
                LAST_READ
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(HashMap::new)
                    .insert(key, caps.clone());
                found.insert(id.clone(), caps);
            }
            Err((error, _)) => {
                tracing::debug!("What a WASAPI device offers could not be read: {}", error);
                found.extend(last_read(&[(*side, id.clone())]));
            }
        }
    }
    found
}

/// What the devices `ids` said the last time they were read
fn last_read(ids: &[(DeviceSide, String)]) -> HashMap<String, EndpointCaps> {
    let last = LAST_READ.lock().unwrap_or_else(|e| e.into_inner());
    let Some(last) = last.as_ref() else {
        return HashMap::new();
    };
    ids.iter()
        .filter_map(|(side, id)| {
            last.get(&key_of(*side, id))
                .map(|caps| (id.clone(), caps.clone()))
        })
        .collect()
}

fn probe_one(flow: Flow, endpoint: &str) -> Result<EndpointCaps, (AudioError, OpenFailure)> {
    let device = device_of(flow, Some(endpoint))?;
    let client: IAudioClient = activate(&device)?;
    let (mut shared, mut smallest) = (0i64, 0i64);
    // SAFETY: COM calls on a thread in the apartment
    unsafe { client.GetDevicePeriod(Some(&mut shared), Some(&mut smallest)) }
        .map_err(|e| failure_of("device period", &e))?;
    let mix = mix_format(&client)?;
    let low_latency = client.cast::<IAudioClient3>().ok().and_then(|client| {
        // SAFETY: the mixer allocates the format; it is freed after the call
        unsafe {
            let format = client.GetMixFormat().ok()?;
            let (mut default, mut fundamental, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
            let read = client.GetSharedModeEnginePeriod(
                format,
                &mut default,
                &mut fundamental,
                &mut min,
                &mut max,
            );
            CoTaskMemFree(Some(format as *const _));
            read.ok().map(|()| SharedPeriods {
                default,
                fundamental,
                min,
                max,
            })
        }
    });
    // The rates an exclusive stream is opened at (`open_exclusive`)
    let mut exclusive_rates = Vec::new();
    let mut exclusive_allowed = true;
    for rate in std::iter::once(mix.rate).chain(
        VALID_SAMPLE_RATES
            .into_iter()
            .filter(|&rate| rate != mix.rate),
    ) {
        match first_exclusive_format(&client, &[rate], &mix) {
            Ok(Some(_)) => exclusive_rates.push(rate),
            Ok(None) => {}
            // An answer, not a device that is silent: Windows keeps every
            // app from taking it until the person allows it
            Err(e) if e.code() == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED => {
                exclusive_allowed = false;
                break;
            }
            Err(e) => return Err(failure_of("exclusive format", &e)),
        }
    }
    Ok(EndpointCaps {
        mix_rate: mix.rate,
        shared_period_us: (shared / 10) as u32,
        low_latency,
        exclusive_period_us: (smallest / 10) as u32,
        exclusive_rates,
        exclusive_allowed,
    })
}
