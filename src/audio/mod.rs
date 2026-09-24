//! Audio engine module
//!
//! Handles audio capture, playback, and local monitoring.

pub(crate) mod codec;
mod device;
mod engine;
mod error;
mod monitor;
mod playout;
mod plc;
mod preset;
mod probe;
mod resampler;
mod stream;

pub use codec::{
    create_codec, AudioCodec, CodecConfig, CodecError, CodecType, OpusCodec, PcmCodec,
};
pub(crate) use device::stable_device_id;
pub use device::{
    list_input_devices, list_output_devices, resolve_input_device, resolve_output_device,
    AudioDevice, DeviceId,
};
pub use engine::{
    AudioBuffer, AudioConfig, AudioEngine, AudioEvent, BitDepth, CaptureConfig, PlaybackConfig,
};
pub use error::AudioError;
pub use monitor::{LocalMonitor, MonitorTap, MONITOR_MARGIN_FRAMES};
pub use playout::{
    PlayoutBuffer, PlayoutConfig, PlayoutRead, PlayoutResult, PlayoutStats, WriteOutcome,
};
pub use plc::PcmPlc;
pub use preset::{AudioPreset, BUDGET_SAMPLE_RATE};
pub use probe::{BurstProbe, BurstSignal, DelayStats, RoundTripReport};
pub use resampler::{
    create_resampler, create_resampler_with_channels, AudioResampler, FastResampler,
    PassthroughResampler, ResamplerError,
};
pub use stream::{mono_to_wire, PeerRateChange, ReceivePath, ADAPT_INTERVAL, WIRE_CHANNELS};
