//! ASIO backend (Windows only)
//!
//! ASIO has no independent capture and playback streams: a driver hands out
//! one set of buffers, half input and half output, and drives both with a
//! single `bufferSwitch` callback (see `Driver::create_buffers`). That does
//! not fit [`super::engine::AudioEngine`], which opens capture and playback as
//! two unrelated `cpal` streams, so an ASIO session is opened and driven
//! through this module instead, and the two paths converge again on the
//! `FnMut` callbacks a caller hands to either one
//! (see `AudioIo::start` in `main.rs`).
//!
//! This talks to the driver's COM interface directly, through the `azo`
//! crate, rather than through Steinberg's ASIO SDK: the SDK's proprietary
//! license needs a signed submission, and its GPLv3 option conflicts with
//! this app's source-available license.
//!
//! # Why the callback state is a global, not a closure capture
//!
//! `azo::sys::Callbacks` is four bare `unsafe extern "system" fn` pointers -
//! ASIO's C API has no user-data slot to carry a capture through. The state
//! the callback needs therefore lives in [`CALLBACK`], a process-wide static.
//! This is not a convenience shortcut: only one ASIO driver session runs at a
//! time (the app opens one audio session), so there is exactly one thing for
//! the static to hold, and no closure could have reached the callback anyway.
//!
//! # Scope not covered here
//!
//! - The driver must offer the session's sample rate exactly; unlike the
//!   `cpal` capture path, this does not resample a mismatched device rate.
//! - `direct_process == false` (the driver asking that processing move off
//!   its callback thread) is not handled: that buffer is left silent. Every
//!   interface this was written against reports `direct_process == true`.

use std::ffi::c_void;
use std::time::Duration;

use parking_lot::Mutex;

use azo::dto::ChannelId;
use azo::sys::{self, Bool, Callbacks, MessageSelector, SampleType};
use azo::utils::com::InitGuard;
use azo::{Driver, DriverMetadata};

use super::channels::{pick_channels, OutputRoute};
use super::device::AudioDevice;
use super::device::DeviceId;
use super::driver::StreamHost;
use super::engine::{FramePuller, RoutedPuller};
use super::error::AudioError;

/// Marks a [`DeviceId`] as an ASIO driver rather than a `cpal` device.
/// `cpal` never opens ASIO devices here (see the module doc), so this prefix
/// is the only way one is reachable at all.
pub(crate) const ID_PREFIX: &str = "asio:";

/// Whether `id` names an ASIO driver rather than a `cpal` device.
pub fn is_asio_id(id: &DeviceId) -> bool {
    id.0.starts_with(ID_PREFIX)
}

/// The driver name inside an ASIO [`DeviceId`], or `None` if `id` is not one.
pub fn driver_name_of(id: &DeviceId) -> Option<&str> {
    id.0.strip_prefix(ID_PREFIX)
}

/// Lists the ASIO drivers Windows has registered (`HKLM\SOFTWARE\ASIO`), one
/// [`AudioDevice`] per driver. Usable as both the input and the output
/// device: ASIO has one channel set per direction under a single driver, so
/// choosing the same one for both is what tells the caller to open it once
/// with [`AsioDuplex::start`] rather than twice through `cpal`.
///
/// A driver that fails to report anything useful (does not initialize, or
/// supports none of the common rates) is left out rather than shown as a
/// dead end.
pub(crate) fn list_drivers() -> Vec<AudioDevice> {
    let Ok(metas) = azo::get_drivers() else {
        return Vec::new();
    };
    metas.iter().filter_map(probe_driver).collect()
}

fn probe_driver(meta: &DriverMetadata) -> Option<AudioDevice> {
    let guard = meta.create_instance().ok()?;
    let driver: &Driver = &guard;
    if !driver.init(None) {
        return None;
    }
    let name = driver.name().to_string_lossy().into_owned();
    let counts = driver.channel_counts().ok()?;
    let supported_sample_rates: Vec<u32> = [44100u32, 48000, 96000, 192000]
        .into_iter()
        .filter(|rate| driver.can_sample_rate(f64::from(*rate)).is_ok())
        .collect();
    if supported_sample_rates.is_empty() {
        return None;
    }
    let supported_channels = [counts.in_, counts.out]
        .into_iter()
        .filter(|c| *c > 0)
        .map(|c| c as u16)
        .collect();
    Some(AudioDevice {
        id: DeviceId(format!("{ID_PREFIX}{name}")),
        name,
        supported_sample_rates,
        supported_channels,
        is_default: false,
        is_asio: true,
    })
}

/// A running ASIO session: one driver, opened once for both directions.
///
/// Held on the thread that opened it, like [`StreamHost`] - the ASIO
/// session's COM apartment (`azo::utils::com::InitGuard`) cannot leave the
/// thread that initialized it, so the same "closes on request, from the
/// thread that owns it" shape is reused rather than re-invented.
pub struct AsioDuplex {
    host: StreamHost,
}

impl AsioDuplex {
    /// Opens `driver_name` for both capture and playback and starts it.
    ///
    /// `capture_picks` and `playback_route` mean the same as on
    /// [`super::engine::AudioEngine`]: which of the driver's channels
    /// capture delivers, and which ones playback is spread over. The
    /// channels requested from the driver are always `0..needed`, and the
    /// picks are applied in software afterwards, the same way the `cpal`
    /// path does it - so a device frame looks the same to `capture_callback`
    /// regardless of which backend opened it.
    ///
    /// # Errors
    /// [`AudioError::DeviceNotFound`] if no registered driver has this name,
    /// [`AudioError::UnsupportedConfig`] if it does not offer `sample_rate` or
    /// enough channels, [`AudioError::DeviceOpenFailed`] for other setup
    /// failures, and [`AudioError::DeviceUnresponsive`] if it does not answer
    /// within `open_timeout`.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors AudioEngine::start_capture / start_playback_with_source, opened together"
    )]
    pub fn start(
        driver_name: &str,
        sample_rate: u32,
        frame_size: u32,
        frame_samples: usize,
        open_timeout: Duration,
        capture_picks: Vec<usize>,
        playback_route: OutputRoute,
        capture_callback: impl FnMut(&[f32], u64) + Send + 'static,
        fill_frame: impl FnMut(&mut [f32]) -> usize + Send + 'static,
    ) -> Result<Self, AudioError> {
        let driver_name = driver_name.to_string();
        let host = StreamHost::open("ASIO duplex", open_timeout, move || {
            open_duplex(
                &driver_name,
                sample_rate,
                frame_size,
                frame_samples,
                capture_picks,
                playback_route,
                capture_callback,
                fill_frame,
            )
        })?;
        Ok(Self { host })
    }

    /// Stops the session, waiting a while for the driver to let go of it.
    pub fn stop(self) {
        self.host.close("ASIO duplex");
    }
}

/// Kept alive for as long as the session runs; dropping it (on the thread
/// that built it - see [`AsioDuplex`]) stops the driver, releases its
/// buffers, clears [`CALLBACK`] and finally uninitializes COM, in that order.
struct AsioResources {
    driver: InitGuard<Driver>,
}

impl Drop for AsioResources {
    fn drop(&mut self) {
        *CALLBACK.lock() = None;
        let _ = self.driver.stop();
        let _ = self.driver.dispose_all_buffers();
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "internal setup helper, not a public API"
)]
fn open_duplex(
    driver_name: &str,
    sample_rate: u32,
    frame_size: u32,
    frame_samples: usize,
    capture_picks: Vec<usize>,
    playback_route: OutputRoute,
    capture_callback: impl FnMut(&[f32], u64) + Send + 'static,
    fill_frame: impl FnMut(&mut [f32]) -> usize + Send + 'static,
) -> Result<AsioResources, AudioError> {
    let metas = azo::get_drivers()
        .map_err(|e| AudioError::DeviceOpenFailed(format!("listing ASIO drivers: {e}")))?;
    let meta = metas
        .iter()
        .find(|m| m.description == driver_name)
        .ok_or_else(|| AudioError::DeviceNotFound(driver_name.to_string()))?;
    let guard = meta
        .create_instance()
        .map_err(|e| AudioError::DeviceOpenFailed(format!("creating {driver_name}: {e}")))?;
    let driver: &Driver = &guard;
    if !driver.init(None) {
        return Err(AudioError::DeviceOpenFailed(format!(
            "{driver_name} failed to initialize: {:?}",
            driver.last_error()
        )));
    }

    driver
        .can_sample_rate(f64::from(sample_rate))
        .map_err(|_| {
            AudioError::UnsupportedConfig(format!("{driver_name} does not offer {sample_rate} Hz"))
        })?;
    driver
        .set_sample_rate(f64::from(sample_rate))
        .map_err(|e| {
            AudioError::UnsupportedConfig(format!(
                "cannot set {driver_name} to {sample_rate} Hz: {e}"
            ))
        })?;

    let counts = driver
        .channel_counts()
        .map_err(|e| AudioError::DeviceOpenFailed(format!("{driver_name} channel count: {e}")))?;
    let needed_in = capture_picks.iter().max().map_or(1, |highest| highest + 1);
    let needed_out = playback_route.channels_needed();
    if needed_in > counts.in_ as usize {
        return Err(AudioError::UnsupportedConfig(format!(
            "{driver_name} has {} input channel(s), {needed_in} needed",
            counts.in_
        )));
    }
    if needed_out > counts.out as usize {
        return Err(AudioError::UnsupportedConfig(format!(
            "{driver_name} has {} output channel(s), {needed_out} needed",
            counts.out
        )));
    }

    let channel_types = |input: bool, count: usize| -> Result<Vec<SampleType>, AudioError> {
        (0..count as i32)
            .map(|index| {
                let info = driver
                    .channel_info(ChannelId { input, index })
                    .map_err(|e| {
                        AudioError::DeviceOpenFailed(format!(
                            "{driver_name} channel info ({}, {index}): {e}",
                            if input { "in" } else { "out" }
                        ))
                    })?;
                if !is_supported(info.sample_type) {
                    return Err(AudioError::UnsupportedConfig(format!(
                        "{driver_name} channel ({}, {index}) uses an unsupported sample format",
                        if input { "in" } else { "out" }
                    )));
                }
                Ok(info.sample_type)
            })
            .collect()
    };
    let input_types = channel_types(true, needed_in)?;
    let output_types = channel_types(false, needed_out)?;

    let buffer_size_info = driver
        .buffer_size()
        .map_err(|e| AudioError::DeviceOpenFailed(format!("{driver_name} buffer size: {e}")))?;
    let requested = frame_size as i32;
    let buffer_size = if requested >= buffer_size_info.min && requested <= buffer_size_info.max {
        requested
    } else {
        buffer_size_info.preferred
    };

    let channel_ids = (0..needed_in as i32)
        .map(|index| ChannelId { input: true, index })
        .chain((0..needed_out as i32).map(|index| ChannelId {
            input: false,
            index,
        }));
    // Safety: `CALLBACKS` outlives the buffers (it is a `'static`), and the
    // buffer pointers this returns are stored in `CALLBACK` before
    // `driver.start()` runs the callback that first dereferences them, and
    // are cleared (in `AsioResources::drop`) before the buffers are disposed.
    let buffers: Vec<[*mut c_void; 2]> =
        unsafe { driver.create_buffers(channel_ids, buffer_size, &raw const CALLBACKS) }
            .map_err(|e| {
                AudioError::DeviceOpenFailed(format!("{driver_name} create_buffers: {e}"))
            })?
            .collect();
    let (input_bufs, output_bufs) = buffers.split_at(needed_in);

    let input: Vec<ChannelBuf> = input_bufs
        .iter()
        .zip(input_types)
        .map(|(&ptrs, sample_type)| ChannelBuf { ptrs, sample_type })
        .collect();
    let output: Vec<ChannelBuf> = output_bufs
        .iter()
        .zip(output_types)
        .map(|(&ptrs, sample_type)| ChannelBuf { ptrs, sample_type })
        .collect();

    let buffer_size = buffer_size as usize;
    let mut picked = Vec::with_capacity(buffer_size * capture_picks.len().max(1));
    // A dry run against a zeroed decoded buffer grows `picked` to its steady
    // size once, here, rather than on the first real callback.
    pick_channels(
        &vec![0.0; buffer_size * needed_in.max(1)],
        needed_in.max(1),
        &capture_picks,
        &mut picked,
    );

    let state = CallbackState {
        buffer_size,
        capture_picks,
        capture_callback: Box::new(capture_callback),
        puller: RoutedPuller::new(
            FramePuller::new(frame_samples, Box::new(fill_frame)),
            buffer_size * 2,
            output.len(),
            playback_route,
            sample_rate,
        ),
        decoded: vec![0.0; buffer_size * input.len().max(1)],
        picked,
        device_out: vec![0.0; buffer_size * output.len().max(1)],
        sample_count: 0,
        input,
        output,
    };
    *CALLBACK.lock() = Some(state);

    driver
        .start()
        .map_err(|e| AudioError::StreamError(format!("starting {driver_name}: {e}")))?;
    // ASIO4ALL withholds `bufferSwitch` until the host signals this, even
    // though nothing else observed needed it (see the module doc). Devices
    // without internal buffering report `NOT_PRESENT`, which the crate says
    // to ignore rather than treat as failure.
    let _ = driver.output_ready();

    Ok(AsioResources { driver: guard })
}

fn is_supported(sample_type: SampleType) -> bool {
    matches!(
        sample_type,
        SampleType::PCM_I16_LSB
            | SampleType::PCM_I16_MSB
            | SampleType::PCM_I24_LSB
            | SampleType::PCM_I24_MSB
            | SampleType::PCM_I32_LSB
            | SampleType::PCM_I32_MSB
            | SampleType::PCM_F32_LSB
            | SampleType::PCM_F32_MSB
            | SampleType::PCM_F64_LSB
            | SampleType::PCM_F64_MSB
            | SampleType::PCM_I32_LSB_16
            | SampleType::PCM_I32_MSB_16
            | SampleType::PCM_I32_LSB_18
            | SampleType::PCM_I32_MSB_18
            | SampleType::PCM_I32_LSB_20
            | SampleType::PCM_I32_MSB_20
            | SampleType::PCM_I32_LSB_24
            | SampleType::PCM_I32_MSB_24
    )
}

/// Full-scale magnitudes for the ASIO "shifted" sample types, whose 32-bit
/// container only has this many significant bits (MSB-aligned).
const I18_MAX: f32 = 0x0001_FFFF as f32;
const I20_MAX: f32 = 0x0007_FFFF as f32;
const I24_MAX: f32 = 0x007F_FFFF as f32;

/// One ASIO channel's pair of half-buffers, and the native format they hold.
struct ChannelBuf {
    ptrs: [*mut c_void; 2],
    sample_type: SampleType,
}

// The pointers are plain process memory the driver allocated for this
// session; nothing about them is thread-affine. What *is* thread-affine
// (the COM apartment) is `AsioResources::driver`, which is never put in
// `CALLBACK`.
unsafe impl Send for ChannelBuf {}

/// A captured frame and its timestamp, handed to whatever `AsioDuplex::start`
/// was given as `capture_callback`.
type CaptureCallback = Box<dyn FnMut(&[f32], u64) + Send>;

/// What `RoutedPuller` pulls the stereo it spreads over the output channels
/// from - boxed so [`CallbackState`] does not need to be generic.
type FrameSource = Box<dyn FnMut(&mut [f32]) -> usize + Send>;

struct CallbackState {
    buffer_size: usize,
    input: Vec<ChannelBuf>,
    output: Vec<ChannelBuf>,
    capture_picks: Vec<usize>,
    capture_callback: CaptureCallback,
    puller: RoutedPuller<FrameSource>,
    /// Scratch: the input half decoded to interleaved f32, `buffer_size *
    /// input.len()` long. Sized once at open, never resized in the callback.
    decoded: Vec<f32>,
    /// Scratch for `pick_channels`'s output.
    picked: Vec<f32>,
    /// Scratch: what `puller` produced, `buffer_size * output.len()` long,
    /// before it is encoded into the output half.
    device_out: Vec<f32>,
    sample_count: u64,
}

impl CallbackState {
    /// Runs one `bufferSwitch`: decodes `half` of the input buffers, hands
    /// the picked channels to the capture callback, pulls what to play from
    /// the frame source, and encodes it into `half` of the output buffers.
    fn process(&mut self, half: usize) {
        let n = self.buffer_size;
        let in_channels = self.input.len();
        for (channel, buf) in self.input.iter().enumerate() {
            let base = buf.ptrs[half].cast::<u8>();
            for frame in 0..n {
                // Safety: `base` is this channel's live ASIO buffer half,
                // `n` samples wide in `buf.sample_type`'s native width (the
                // size `create_buffers` was called with).
                self.decoded[frame * in_channels + channel] =
                    unsafe { read_sample(buf.sample_type, base, frame) };
            }
        }
        if in_channels > 0 {
            super::device_loop::on_input(&self.decoded, in_channels);
            pick_channels(
                &self.decoded,
                in_channels,
                &self.capture_picks,
                &mut self.picked,
            );
            let timestamp = self.sample_count;
            self.sample_count += self.picked.len() as u64;
            (self.capture_callback)(&self.picked, timestamp);
        }

        let out_channels = self.output.len();
        if out_channels > 0 {
            self.puller.fill(&mut self.device_out);
            for (channel, buf) in self.output.iter().enumerate() {
                let base = buf.ptrs[half].cast::<u8>();
                for frame in 0..n {
                    // Safety: same as the input loop above, for the output half.
                    unsafe {
                        write_sample(
                            buf.sample_type,
                            base,
                            frame,
                            self.device_out[frame * out_channels + channel],
                        );
                    }
                }
            }
        }
    }
}

static CALLBACK: Mutex<Option<CallbackState>> = Mutex::new(None);

static CALLBACKS: Callbacks = Callbacks {
    buffer_switch,
    sample_rate_did_change,
    asio_message,
    buffer_switch_time_info,
};

unsafe extern "system" fn buffer_switch(buffer_index: sys::ChannelIndex, _direct_process: Bool) {
    if let Some(state) = CALLBACK.lock().as_mut() {
        state.process(buffer_index as usize);
    }
}

unsafe extern "system" fn sample_rate_did_change(_sample_rate: sys::SampleRate) {
    // The driver changed rate out from under us (usually the user changing
    // it in the driver's control panel). Nothing safe to do about it from
    // inside this callback; the next session open will pick up the new rate.
}

const unsafe extern "system" fn asio_message(
    selector: MessageSelector,
    _value: std::ffi::c_long,
    _message: *const c_void,
    _opt: *const f64,
) -> std::ffi::c_long {
    match selector {
        MessageSelector::ENGINE_VERSION => 2,
        _ => Bool::FALSE.0,
    }
}

unsafe extern "system" fn buffer_switch_time_info(
    params: *mut sys::Time,
    _double_buffer_index: std::ffi::c_long,
    _direct_process: Bool,
) -> *mut sys::Time {
    // Never called: `asio_message` never claims `CanTimeInfo`, so drivers
    // call `buffer_switch` instead. Kept as a valid no-op should one call it
    // anyway.
    params
}

/// Reads one sample at frame `i` from a raw ASIO buffer half in `base`.
///
/// # Safety
/// `base` must point to a live ASIO buffer half at least `i + 1` samples
/// wide in `sample_type`'s native width.
unsafe fn read_sample(sample_type: SampleType, base: *const u8, i: usize) -> f32 {
    unsafe fn bytes<const N: usize>(base: *const u8, i: usize, stride: usize) -> [u8; N] {
        let mut buf = [0u8; N];
        std::ptr::copy_nonoverlapping(base.add(i * stride), buf.as_mut_ptr(), N);
        buf
    }
    fn i24_le(b: [u8; 3]) -> i32 {
        i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8
    }
    fn i24_be(b: [u8; 3]) -> i32 {
        i32::from_be_bytes([b[0], b[1], b[2], 0]) >> 8
    }

    match sample_type {
        SampleType::PCM_I16_LSB => i16::from_le_bytes(bytes(base, i, 2)) as f32 / i16::MAX as f32,
        SampleType::PCM_I16_MSB => i16::from_be_bytes(bytes(base, i, 2)) as f32 / i16::MAX as f32,
        SampleType::PCM_I24_LSB => i24_le(bytes(base, i, 3)) as f32 / I24_MAX,
        SampleType::PCM_I24_MSB => i24_be(bytes(base, i, 3)) as f32 / I24_MAX,
        SampleType::PCM_I32_LSB => i32::from_le_bytes(bytes(base, i, 4)) as f32 / i32::MAX as f32,
        SampleType::PCM_I32_MSB => i32::from_be_bytes(bytes(base, i, 4)) as f32 / i32::MAX as f32,
        SampleType::PCM_F32_LSB => f32::from_le_bytes(bytes(base, i, 4)),
        SampleType::PCM_F32_MSB => f32::from_be_bytes(bytes(base, i, 4)),
        SampleType::PCM_F64_LSB => f64::from_le_bytes(bytes(base, i, 8)) as f32,
        SampleType::PCM_F64_MSB => f64::from_be_bytes(bytes(base, i, 8)) as f32,
        SampleType::PCM_I32_LSB_16 => {
            i32::from_le_bytes(bytes(base, i, 4)) as f32 / i16::MAX as f32
        }
        SampleType::PCM_I32_MSB_16 => {
            i32::from_be_bytes(bytes(base, i, 4)) as f32 / i16::MAX as f32
        }
        SampleType::PCM_I32_LSB_18 => i32::from_le_bytes(bytes(base, i, 4)) as f32 / I18_MAX,
        SampleType::PCM_I32_MSB_18 => i32::from_be_bytes(bytes(base, i, 4)) as f32 / I18_MAX,
        SampleType::PCM_I32_LSB_20 => i32::from_le_bytes(bytes(base, i, 4)) as f32 / I20_MAX,
        SampleType::PCM_I32_MSB_20 => i32::from_be_bytes(bytes(base, i, 4)) as f32 / I20_MAX,
        SampleType::PCM_I32_LSB_24 => i32::from_le_bytes(bytes(base, i, 4)) as f32 / I24_MAX,
        SampleType::PCM_I32_MSB_24 => i32::from_be_bytes(bytes(base, i, 4)) as f32 / I24_MAX,
        // Rejected earlier (`is_supported`, checked when the channel was
        // opened): DSD or an ASIO sample type this app does not know.
        _ => 0.0,
    }
}

/// Writes one sample at frame `i` into a raw ASIO buffer half in `base`.
/// Safety and format support mirror [`read_sample`].
unsafe fn write_sample(sample_type: SampleType, base: *mut u8, i: usize, value: f32) {
    unsafe fn put(base: *mut u8, i: usize, stride: usize, bytes: &[u8]) {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(i * stride), bytes.len());
    }

    let v = value.clamp(-1.0, 1.0);
    match sample_type {
        SampleType::PCM_I16_LSB => put(base, i, 2, &((v * i16::MAX as f32) as i16).to_le_bytes()),
        SampleType::PCM_I16_MSB => put(base, i, 2, &((v * i16::MAX as f32) as i16).to_be_bytes()),
        SampleType::PCM_I24_LSB => {
            put(base, i, 3, &((v * I24_MAX) as i32).to_le_bytes()[..3]);
        }
        SampleType::PCM_I24_MSB => {
            put(base, i, 3, &((v * I24_MAX) as i32).to_be_bytes()[1..]);
        }
        SampleType::PCM_I32_LSB => put(
            base,
            i,
            4,
            &((v as f64 * i32::MAX as f64) as i32).to_le_bytes(),
        ),
        SampleType::PCM_I32_MSB => put(
            base,
            i,
            4,
            &((v as f64 * i32::MAX as f64) as i32).to_be_bytes(),
        ),
        SampleType::PCM_F32_LSB => put(base, i, 4, &v.to_le_bytes()),
        SampleType::PCM_F32_MSB => put(base, i, 4, &v.to_be_bytes()),
        SampleType::PCM_F64_LSB => put(base, i, 8, &(v as f64).to_le_bytes()),
        SampleType::PCM_F64_MSB => put(base, i, 8, &(v as f64).to_be_bytes()),
        SampleType::PCM_I32_LSB_16 => {
            put(base, i, 4, &((v * i16::MAX as f32) as i32).to_le_bytes())
        }
        SampleType::PCM_I32_MSB_16 => {
            put(base, i, 4, &((v * i16::MAX as f32) as i32).to_be_bytes())
        }
        SampleType::PCM_I32_LSB_18 => put(base, i, 4, &((v * I18_MAX) as i32).to_le_bytes()),
        SampleType::PCM_I32_MSB_18 => put(base, i, 4, &((v * I18_MAX) as i32).to_be_bytes()),
        SampleType::PCM_I32_LSB_20 => put(base, i, 4, &((v * I20_MAX) as i32).to_le_bytes()),
        SampleType::PCM_I32_MSB_20 => put(base, i, 4, &((v * I20_MAX) as i32).to_be_bytes()),
        SampleType::PCM_I32_LSB_24 => put(base, i, 4, &((v * I24_MAX) as i32).to_le_bytes()),
        SampleType::PCM_I32_MSB_24 => put(base, i, 4, &((v * I24_MAX) as i32).to_be_bytes()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_id_with_the_asio_prefix_is_recognized_as_one() {
        assert!(is_asio_id(&DeviceId(format!(
            "{ID_PREFIX}Focusrite USB ASIO"
        ))));
        assert!(!is_asio_id(&DeviceId("wasapi:{gu-id}".to_string())));
    }

    #[test]
    fn the_driver_name_is_read_back_out_of_the_device_id() {
        let id = DeviceId(format!("{ID_PREFIX}Focusrite USB ASIO"));
        assert_eq!(driver_name_of(&id), Some("Focusrite USB ASIO"));
        assert_eq!(driver_name_of(&DeviceId("wasapi:x".to_string())), None);
    }

    /// A sample written and read back through the same format must survive
    /// (within rounding), for every format this backend claims to support -
    /// otherwise `is_supported` and the two conversion functions have
    /// drifted apart.
    #[test]
    fn every_supported_sample_format_round_trips_through_encode_and_decode() {
        let formats = [
            SampleType::PCM_I16_LSB,
            SampleType::PCM_I16_MSB,
            SampleType::PCM_I24_LSB,
            SampleType::PCM_I24_MSB,
            SampleType::PCM_I32_LSB,
            SampleType::PCM_I32_MSB,
            SampleType::PCM_F32_LSB,
            SampleType::PCM_F32_MSB,
            SampleType::PCM_F64_LSB,
            SampleType::PCM_F64_MSB,
            SampleType::PCM_I32_LSB_16,
            SampleType::PCM_I32_MSB_16,
            SampleType::PCM_I32_LSB_18,
            SampleType::PCM_I32_MSB_18,
            SampleType::PCM_I32_LSB_20,
            SampleType::PCM_I32_MSB_20,
            SampleType::PCM_I32_LSB_24,
            SampleType::PCM_I32_MSB_24,
        ];
        for sample_type in formats {
            assert!(is_supported(sample_type));
            let mut buf = [0u8; 8];
            for value in [-1.0f32, -0.5, 0.0, 0.25, 0.9] {
                unsafe {
                    write_sample(sample_type, buf.as_mut_ptr(), 0, value);
                    let back = read_sample(sample_type, buf.as_ptr(), 0);
                    assert!(
                        (back - value).abs() < 0.01,
                        "{sample_type:?}: wrote {value}, read back {back}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_unsupported_sample_format_is_rejected_rather_than_silently_misread() {
        assert!(!is_supported(SampleType::DSD_I8_LSB_1));
    }
}
