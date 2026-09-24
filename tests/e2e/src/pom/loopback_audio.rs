//! Feeds a known signal into the app's capture device (ADR-025).
//!
//! Verifying that the level meter follows the input means controlling what
//! the input *is*, which needs a loopback audio device: one that presents
//! whatever is played to its output as its input. That is a per-platform
//! driver the developer installs once.
//!
//! Signal generation goes through `cpal` rather than shelling out to
//! `afplay`/`pw-play`. `cpal` is already a dependency of the workspace, it
//! can target a device by name (which `afplay` cannot), and the same code
//! then works on every platform the app supports.
//!
//! # The device keeps what it was given
//!
//! BlackHole mixes each writer into a ring buffer that nothing clears, and its
//! input keeps returning that buffer after every writer has stopped. Measured
//! behaviour on macOS 15 (`measure_input_peak`, 500ms windows):
//!
//! | Step | Reading |
//! |------|---------|
//! | fresh process, nothing played yet | 0.000 |
//! | while a 0.5 tone plays | 0.500 |
//! | after that tone stops | 0.499 |
//! | while zeros are written over it for 2s | 0.499 |
//! | after 10s with no client open at all | 0.499 |
//!
//! So a scenario cannot return the device to silence - not by stopping, not by
//! writing zeros, not by waiting. Two consequences for anything built on this
//! fixture:
//!
//! - **Assert a rise, not a level.** Read the floor first, then assert the
//!   meter climbs above it once a tone plays. A scenario that asserts silence
//!   passes only when it happens to run first.
//! - **Give each scenario its own frequency.** Leftover audio mixes with the
//!   new tone, and two sines of the *same* frequency can cancel to nothing.
//!   Detuned ones beat instead, so a short measurement window always catches a
//!   constructive moment.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::DriverResult;

/// Name of the loopback device on each platform. These are the names the
/// drivers register with the OS, so they are what `cpal` reports.
pub const DEVICE_NAME: &str = if cfg!(target_os = "macos") {
    "BlackHole 2ch"
} else if cfg!(target_os = "windows") {
    "CABLE Output"
} else {
    "jamjam-test-sink"
};

/// What to run to get the device, quoted in the error when it is missing so
/// the failure is actionable rather than just "not found".
const INSTALL_HINT: &str = if cfg!(target_os = "macos") {
    "brew install blackhole-2ch, then `sudo killall coreaudiod` (or reboot)"
} else if cfg!(target_os = "windows") {
    "install VB-Audio Virtual Cable from https://vb-audio.com/Cable/"
} else {
    "create a PipeWire null sink named jamjam-test-sink"
};

/// The device is 2ch on macOS; the app captures mono. CoreAudio converts, so
/// the tone is generated in stereo and arrives as mono.
const CHANNELS: u16 = 2;
const SAMPLE_RATE: u32 = 48000;

/// Name of the 8-channel loopback device (Linux only): a PipeWire null sink
/// with eight channels, reached through an ALSA PCM of this name. It stands in
/// for a multi-channel audio interface, so a scenario can put a signal on one
/// specific channel and see which channel the app reads or writes.
///
/// Unlike [`DEVICE_NAME`] it keeps no history: once a writer stops, every
/// channel is silent again.
pub const DEVICE_NAME_8CH: &str = "jamjam-test-8ch";

/// Channel count of [`DEVICE_NAME_8CH`].
pub const CHANNELS_8CH: u16 = 8;

/// A tone playing into the loopback device. Stops when dropped.
///
/// Holds the stream alive: `cpal` stops a stream as soon as its handle is
/// dropped, so a fixture that returned without keeping it would produce
/// silence and a mystifying test failure.
///
/// Dropping it stops *writing* but does not leave the device quiet - see
/// [`quiet_the_device`].
pub struct LoopbackTone {
    _stream: cpal::Stream,
}

/// Confirms the loopback device exists, with an actionable error if not.
///
/// Call this before a scenario that needs it, so a machine without the driver
/// reports what to install instead of failing on an empty level.
pub fn require_device() -> DriverResult<()> {
    let host = cpal::default_host();

    let has_output = host
        .output_devices()
        .map_err(|e| format!("could not enumerate output devices: {}", e))?
        .any(|d| d.description().is_ok_and(|desc| desc.name() == DEVICE_NAME));
    let has_input = host
        .input_devices()
        .map_err(|e| format!("could not enumerate input devices: {}", e))?
        .any(|d| d.description().is_ok_and(|desc| desc.name() == DEVICE_NAME));

    if has_output && has_input {
        return Ok(());
    }
    Err(format!(
        "loopback device {:?} is not available as both input and output \
         (input: {}, output: {}). To get it: {}",
        DEVICE_NAME, has_input, has_output, INSTALL_HINT
    ))
}

/// Confirms [`DEVICE_NAME_8CH`] exists as both input and output, with an
/// actionable error if not. Linux only: no 8-channel loopback driver is
/// assumed on the other platforms.
pub fn require_device_8ch() -> DriverResult<()> {
    let host = cpal::default_host();
    let has_output = host
        .output_devices()
        .map_err(|e| format!("could not enumerate output devices: {}", e))?
        .any(|d| {
            d.description()
                .is_ok_and(|desc| desc.name() == DEVICE_NAME_8CH)
        });
    let has_input = host
        .input_devices()
        .map_err(|e| format!("could not enumerate input devices: {}", e))?
        .any(|d| {
            d.description()
                .is_ok_and(|desc| desc.name() == DEVICE_NAME_8CH)
        });

    if has_output && has_input {
        return Ok(());
    }
    Err(format!(
        "8-channel loopback device {:?} is not available as both input and output \
         (input: {}, output: {}). To get it: tests/e2e/scripts/setup-virtual-audio-linux.sh create, \
         and the ALSA PCM it prints",
        DEVICE_NAME_8CH, has_input, has_output
    ))
}

/// Resolves a cpal display name (the form `DEVICE_NAME` uses, as reported by
/// `description().name()`) into cpal's own **stable id** (`Device::id()`,
/// prefixed by host - `alsa:jamjam-test-sink` on Linux, for example). That
/// stable id is the form the app itself stores as a device id (see
/// `audio::device::stable_device_id` in the app crate); the display name is
/// not usable there, since on ALSA several devices can share one name.
///
/// Searches input devices, then output devices, so it resolves either side
/// of a duplex loopback device.
pub fn resolve_device_id(name: &str) -> DriverResult<String> {
    let host = cpal::default_host();

    let device = host
        .input_devices()
        .map_err(|e| format!("could not enumerate input devices: {}", e))?
        .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
        .or_else(|| {
            host.output_devices()
                .ok()?
                .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
        })
        .ok_or_else(|| format!("no audio device named {:?} found", name))?;

    device
        .id()
        .map(|id| id.to_string())
        .map_err(|e| format!("could not read the stable id of {:?}: {}", name, e))
}

/// Starts a sine tone on the loopback device's output, which then appears on
/// its input - and so on the app's capture stream.
///
/// `amplitude` is 0.0-1.0. Kept well below full scale by callers: the app
/// plays received audio back through the real speakers, so the tone is
/// briefly audible while a scenario runs.
pub fn play_tone(frequency: f32, amplitude: f32) -> DriverResult<LoopbackTone> {
    open_output(DEVICE_NAME, CHANNELS, None, frequency, amplitude)
}

/// Starts a sine tone on one channel of [`DEVICE_NAME_8CH`] and silence on the
/// other seven. `channel` is 1-based, the way the app's channel settings count.
pub fn play_tone_on_channel_8ch(
    channel: u16,
    frequency: f32,
    amplitude: f32,
) -> DriverResult<LoopbackTone> {
    if channel == 0 || channel > CHANNELS_8CH {
        return Err(format!(
            "channel {} is outside 1..={} of {:?}",
            channel, CHANNELS_8CH, DEVICE_NAME_8CH
        ));
    }
    open_output(
        DEVICE_NAME_8CH,
        CHANNELS_8CH,
        Some(channel as usize - 1),
        frequency,
        amplitude,
    )
}

/// Opens `device_name` for output with `channels` channels and plays a sine at
/// `amplitude` - on every channel, or only on `hot` (0-based) when given.
fn open_output(
    device_name: &str,
    channels: u16,
    hot: Option<usize>,
    frequency: f32,
    amplitude: f32,
) -> DriverResult<LoopbackTone> {
    let host = cpal::default_host();
    let device = host
        .output_devices()
        .map_err(|e| format!("could not enumerate output devices: {}", e))?
        .find(|d| d.description().is_ok_and(|desc| desc.name() == device_name))
        .ok_or_else(|| {
            format!(
                "loopback output {:?} not found. {}",
                device_name, INSTALL_HINT
            )
        })?;

    let config = cpal::StreamConfig {
        channels,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    let mut phase = 0f32;
    let step = frequency / SAMPLE_RATE as f32;
    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels as usize) {
                    let sample = (phase * std::f32::consts::TAU).sin() * amplitude;
                    phase = (phase + step).fract();
                    for (index, slot) in frame.iter_mut().enumerate() {
                        *slot = if hot.is_none_or(|h| h == index) {
                            sample
                        } else {
                            0.0
                        };
                    }
                }
            },
            |e| eprintln!("loopback tone error: {}", e),
            None,
        )
        .map_err(|e| format!("could not open {:?} for output: {}", device_name, e))?;

    stream
        .play()
        .map_err(|e| format!("could not start the tone: {}", e))?;

    Ok(LoopbackTone { _stream: stream })
}

/// Measures the peak amplitude arriving on the loopback device's input.
///
/// Used to confirm the fixture itself works before blaming the app: if this
/// reads silence, the tone never reached the device and no assertion about
/// the app's meter would mean anything.
///
/// Reads whatever the device currently holds, which after a tone stops is
/// still that tone ([`quiet_the_device`]).
pub fn measure_input_peak(duration: std::time::Duration) -> DriverResult<f32> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .map_err(|e| format!("could not enumerate input devices: {}", e))?
        .find(|d| d.description().is_ok_and(|desc| desc.name() == DEVICE_NAME))
        .ok_or_else(|| {
            format!(
                "loopback input {:?} not found. {}",
                DEVICE_NAME, INSTALL_HINT
            )
        })?;

    let config = cpal::StreamConfig {
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    // Peak as milli-units in an atomic, because the callback cannot borrow a
    // float from this stack frame.
    let peak = Arc::new(AtomicU32::new(0));
    let writer = peak.clone();
    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let frame_peak = data.iter().fold(0f32, |acc, s| acc.max(s.abs()));
                writer.fetch_max((frame_peak * 1000.0) as u32, Ordering::SeqCst);
            },
            |e| eprintln!("loopback capture error: {}", e),
            None,
        )
        .map_err(|e| format!("could not open {:?} for input: {}", DEVICE_NAME, e))?;

    stream
        .play()
        .map_err(|e| format!("could not start capture: {}", e))?;
    std::thread::sleep(duration);

    Ok(peak.load(Ordering::SeqCst) as f32 / 1000.0)
}

/// Measures the peak amplitude arriving on each channel of [`DEVICE_NAME_8CH`]'s
/// input, in channel order (index 0 is channel 1).
///
/// The counterpart of [`play_tone_on_channel_8ch`]: it says which channels a
/// signal came back on. Also how a scenario sees which channels the app *wrote*
/// to when it plays to this device.
pub fn measure_input_channel_peaks_8ch(duration: std::time::Duration) -> DriverResult<Vec<f32>> {
    let host = cpal::default_host();
    let device = host
        .input_devices()
        .map_err(|e| format!("could not enumerate input devices: {}", e))?
        .find(|d| {
            d.description()
                .is_ok_and(|desc| desc.name() == DEVICE_NAME_8CH)
        })
        .ok_or_else(|| {
            format!(
                "loopback input {:?} not found. {}",
                DEVICE_NAME_8CH, INSTALL_HINT
            )
        })?;

    let config = cpal::StreamConfig {
        channels: CHANNELS_8CH,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    // Milli-units in atomics, for the same reason as `measure_input_peak`.
    let peaks: Arc<Vec<AtomicU32>> =
        Arc::new((0..CHANNELS_8CH).map(|_| AtomicU32::new(0)).collect());
    let writer = peaks.clone();
    let stream = device
        .build_input_stream(
            config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                for frame in data.chunks(CHANNELS_8CH as usize) {
                    for (peak, sample) in writer.iter().zip(frame) {
                        peak.fetch_max((sample.abs() * 1000.0) as u32, Ordering::SeqCst);
                    }
                }
            },
            |e| eprintln!("loopback capture error: {}", e),
            None,
        )
        .map_err(|e| format!("could not open {:?} for input: {}", DEVICE_NAME_8CH, e))?;

    stream
        .play()
        .map_err(|e| format!("could not start capture: {}", e))?;
    std::thread::sleep(duration);

    Ok(peaks
        .iter()
        .map(|peak| peak.load(Ordering::SeqCst) as f32 / 1000.0)
        .collect())
}
