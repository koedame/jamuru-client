//! Audio engine module
//!
//! Handles audio capture, playback, and local monitoring.

#[cfg(target_os = "windows")]
mod asio;
mod channels;
pub(crate) mod codec;
mod device;
pub mod device_loop;
mod driver;
mod engine;
mod error;
pub mod fault;
mod flight;
mod heartbeat;
mod listing;
pub mod mic_access;
mod monitor;
mod open;
mod peer_mix;
mod playout;
mod plc;
mod preset;
pub(crate) mod priority;
mod probe;
mod resampler;
mod slot;
mod stream;
mod taps;
#[cfg(target_os = "macos")]
mod transport;
mod virtual_output;

#[cfg(target_os = "windows")]
pub use asio::AsioDuplex;
pub use channels::{capture_attempts, channel_count, pair_in_use, OutputRoute};
pub use codec::{
    create_codec, AudioCodec, CodecConfig, CodecError, CodecType, OpusCodec, PcmCodec,
};
pub(crate) use device::stable_device_id;
pub use device::{
    choose_device, device_in_use, driver_name_of, find_device, is_asio_id, list_input_devices,
    list_output_devices, resolve_input_device, resolve_output_device, shared_asio_driver,
    AudioDevice, ChooseDeviceError, DeviceId,
};
pub use driver::{bounded, LIST_TIMEOUT, OPEN_TIMEOUT};
pub use engine::{
    AudioBuffer, AudioConfig, AudioEngine, AudioEvent, BitDepth, CaptureConfig, PlaybackConfig,
    LINUX_MIN_OUTPUT_PERIOD,
};
pub use error::AudioError;
pub use flight::{
    Event as FlightEvent, FlightRecorder, Kind as FlightKind, Report as FlightReport,
    CAPACITY as FLIGHT_CAPACITY,
};
pub use heartbeat::{Heartbeat, StallGuard, Verdict, MAX_REOPENS, STALL_AFTER};
pub use listing::{
    a_listing_is_hung, input_devices, observe_listed_devices, output_devices, Devices,
};
pub use monitor::{LocalMonitor, MonitorTap, MONITOR_MARGIN_FRAMES};
#[cfg(target_os = "windows")]
pub use open::open_asio;
pub use open::{
    capture_or_silence, clear_device_problem, discard_queued, microphone_allows_input,
    note_device_problem, note_trouble, on_captured_frame, playout_source, start_capture_ring,
    start_playout, CaptureRing, DeviceProblem, DeviceSide, DeviceTrouble, MIC_ANSWER_WAIT,
};
pub use peer_mix::{PeerGain, PeerMeter, PeerMix, VoiceHandle};
pub use playout::{
    PlayoutBuffer, PlayoutConfig, PlayoutRead, PlayoutResult, PlayoutStats, WriteOutcome,
};
pub use plc::PcmPlc;
pub use preset::{AudioPreset, BUDGET_SAMPLE_RATE};
pub use probe::{BurstProbe, BurstSignal, DelayStats, RoundTripReport};
pub use resampler::{
    create_resampler, create_resampler_with_channels, AudioResampler, CaptureResampler,
    FastResampler, PassthroughResampler, ResamplerError,
};
#[cfg(target_os = "windows")]
pub use slot::AsioSwitch;
pub use slot::DeviceSlot;
pub use stream::{
    balance_gains, capture_to_wire, mono_to_wire, pan_received, rms_level, GainRamp,
    PeerRateChange, ReceivePath, ADAPT_INTERVAL, WIRE_CHANNELS,
};
pub(crate) use taps::taps;
pub use taps::{set_taps, Taps};
pub use virtual_output::{
    RecordingFeed, RecordingTap, VirtualOutputSink, CHANNELS as VIRTUAL_OUTPUT_CHANNELS,
    NODE_NAME_OUT,
};
