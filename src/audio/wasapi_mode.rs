//! The three ways jamuru opens a device through WASAPI on Windows, what each
//! would do with the devices chosen, and which of them a person is advised to
//! use there (ADR-088).
//!
//! - [`WasapiMode::Shared`] goes through `cpal` and the system's mixer, like
//!   every other app. Its period is the mixer's (10 ms on nearly every
//!   device), whatever the buffer size.
//! - [`WasapiMode::LowLatency`] is shared too, but asks the mixer for the
//!   smallest period the driver allows (`IAudioClient3`). Only a driver that
//!   offers a smaller period than the standard one makes it any shorter.
//! - [`WasapiMode::Exclusive`] takes the device for itself and runs at the
//!   device's own smallest period. Nothing else plays on or records from that
//!   device while it is open.
//!
//! What a device offers is read without opening a stream (`wasapi::probe`,
//! Windows only). [`advise`] turns that into advice; it is a pure function, so
//! the advice is tested on every platform. The two ways `cpal` does not offer
//! are opened by `wasapi`. When one does not open, the device is opened the
//! standard way instead, and [`opened_ways`] says so.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::device::DeviceId;
use super::open::DeviceSide;

/// How a WASAPI device is opened
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasapiMode {
    /// Shared with every other app, at the mixer's period
    #[default]
    Shared,
    /// Shared, at the smallest period the driver offers
    LowLatency,
    /// The device to itself, at the device's smallest period
    Exclusive,
}

impl WasapiMode {
    /// Every way, in the order the settings show them
    pub const ALL: [WasapiMode; 3] = [
        WasapiMode::Shared,
        WasapiMode::LowLatency,
        WasapiMode::Exclusive,
    ];
}

/// The periods the shared mixer offers a stream in the device's format, in
/// frames at [`EndpointCaps::mix_rate`] (`IAudioClient3::GetSharedModeEnginePeriod`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedPeriods {
    /// The period a shared stream runs at unless it asks for another
    pub default: u32,
    /// Every period offered is `min` plus a whole number of these
    pub fundamental: u32,
    pub min: u32,
    pub max: u32,
}

/// What one WASAPI device offers, read from it without opening a stream
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointCaps {
    /// The rate the system mixes the device at (its format in the system's
    /// sound settings)
    pub mix_rate: u32,
    /// The period of a shared stream, in microseconds
    pub shared_period_us: u32,
    /// The periods of the low-latency shared mode. `None` when the system
    /// has no such mode (before Windows 10).
    pub low_latency: Option<SharedPeriods>,
    /// The smallest period of an exclusive stream, in microseconds
    pub exclusive_period_us: u32,
    /// The sample rates the device opens at in exclusive mode, of those a
    /// session runs at
    pub exclusive_rates: Vec<u32>,
}

/// A device of a session, as far as WASAPI is concerned
#[derive(Debug, Clone, Copy)]
pub enum Endpoint<'a> {
    /// Not opened through WASAPI (an ASIO driver)
    NotWasapi,
    /// A WASAPI device whose offer could not be read
    Unknown,
    Known(&'a EndpointCaps),
}

/// Whether a way of opening suits the devices chosen
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Recommended,
    NotRecommended,
    /// It does not open these devices. The settings do not offer it.
    Unavailable,
}

/// Why a way of opening got its [`Verdict`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Standard: no faster way opens these devices
    NothingFaster,
    /// Standard: a faster way opens these devices
    FasterAvailable,
    /// Low latency: the driver offers a shorter period than the standard one
    ShorterPeriod,
    /// Low latency: the driver's smallest period is the standard one, so it
    /// runs as the standard way does
    SamePeriod,
    /// Low latency: the system has no low-latency shared mode
    NoLowLatencyMode,
    /// Exclusive: the devices open in exclusive mode
    OpensExclusively,
    /// Exclusive: a device has no format it opens in exclusive mode
    NoExclusiveFormat,
    /// What a device offers could not be read. Choosing the way tries it; a
    /// device it does not open is opened the standard way.
    Unknown,
}

/// What one way of opening would do with the devices chosen
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeAdvice {
    pub mode: WasapiMode,
    pub verdict: Verdict,
    pub reason: Reason,
    /// The period the input would run at, in microseconds. `None` when it
    /// is not known, or the input is not a WASAPI device.
    pub input_period_us: Option<u32>,
    /// The period the output would run at, as `input_period_us`
    pub output_period_us: Option<u32>,
}

/// The advice on every way of opening, for the devices chosen
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advice {
    /// Whether the choice matters at all: `false` when neither device is
    /// opened through WASAPI (both are an ASIO driver), and `modes` is empty
    pub applies: bool,
    /// One per way, in [`WasapiMode::ALL`]'s order
    pub modes: Vec<ModeAdvice>,
}

/// What one way of opening does with one device
#[derive(Debug, Clone, Copy, PartialEq)]
enum Outcome {
    /// Opens it, at this period
    Opens {
        period_us: u32,
    },
    Refused(Reason),
    Unknown,
}

/// The advice on every way of opening `input` and `output` for a session at
/// `sample_rate` with frames of `buffer_frames`.
///
/// Exclusive and low latency are each recommended when they open every
/// WASAPI device chosen and are faster than the standard way. The standard
/// way is recommended when neither is. A way that leaves a device it cannot
/// open to the standard way is not offered: it would not be what it says.
pub fn advise(input: Endpoint, output: Endpoint, sample_rate: u32, buffer_frames: u32) -> Advice {
    let sides = [input, output];
    if sides.iter().all(|side| matches!(side, Endpoint::NotWasapi)) {
        return Advice {
            applies: false,
            modes: Vec::new(),
        };
    }
    let outcomes = |way: fn(&EndpointCaps, u32, u32) -> Outcome| {
        sides.map(|side| match side {
            Endpoint::NotWasapi => None,
            Endpoint::Unknown => Some(Outcome::Unknown),
            Endpoint::Known(caps) => Some(way(caps, sample_rate, buffer_frames)),
        })
    };
    let standard = outcomes(|caps, _, _| Outcome::Opens {
        period_us: caps.shared_period_us,
    });
    let low_latency = outcomes(low_latency_outcome);
    let exclusive = outcomes(exclusive_outcome);

    let exclusive_advice = judge(WasapiMode::Exclusive, &exclusive, |_| {
        (Verdict::Recommended, Reason::OpensExclusively)
    });
    let low_latency_advice = judge(WasapiMode::LowLatency, &low_latency, |periods| {
        let shorter =
            periods
                .iter()
                .zip(&standard)
                .any(|(period, standard)| match (period, standard) {
                    (Some(period), Some(Outcome::Opens { period_us })) => period < period_us,
                    _ => false,
                });
        if shorter {
            (Verdict::Recommended, Reason::ShorterPeriod)
        } else {
            (Verdict::NotRecommended, Reason::SamePeriod)
        }
    });
    let faster = [&exclusive_advice, &low_latency_advice]
        .iter()
        .any(|advice| advice.verdict == Verdict::Recommended);
    let standard_advice = judge(WasapiMode::Shared, &standard, |_| {
        if faster {
            (Verdict::NotRecommended, Reason::FasterAvailable)
        } else {
            (Verdict::Recommended, Reason::NothingFaster)
        }
    });
    Advice {
        applies: true,
        modes: vec![standard_advice, low_latency_advice, exclusive_advice],
    }
}

/// The advice on `mode` from what it does with each side. A side it refuses
/// makes it unavailable, one not known makes it not recommended, and
/// otherwise `opens` decides from the periods it opens the sides at.
fn judge(
    mode: WasapiMode,
    outcomes: &[Option<Outcome>; 2],
    opens: impl FnOnce(&[Option<u32>; 2]) -> (Verdict, Reason),
) -> ModeAdvice {
    let periods = outcomes.map(|outcome| match outcome {
        Some(Outcome::Opens { period_us }) => Some(period_us),
        _ => None,
    });
    let refused = outcomes.iter().find_map(|outcome| match outcome {
        Some(Outcome::Refused(reason)) => Some(*reason),
        _ => None,
    });
    let unknown = outcomes.contains(&Some(Outcome::Unknown));
    let (verdict, reason) = match (refused, unknown) {
        (Some(reason), _) => (Verdict::Unavailable, reason),
        (None, true) if mode != WasapiMode::Shared => (Verdict::NotRecommended, Reason::Unknown),
        _ => opens(&periods),
    };
    ModeAdvice {
        mode,
        verdict,
        reason,
        input_period_us: periods[0],
        output_period_us: periods[1],
    }
}

fn low_latency_outcome(caps: &EndpointCaps, sample_rate: u32, buffer_frames: u32) -> Outcome {
    match caps.low_latency {
        None => Outcome::Refused(Reason::NoLowLatencyMode),
        Some(periods) => Outcome::Opens {
            period_us: frames_to_us(
                low_latency_period(&periods, caps.mix_rate, sample_rate, buffer_frames),
                caps.mix_rate,
            ),
        },
    }
}

fn exclusive_outcome(caps: &EndpointCaps, sample_rate: u32, buffer_frames: u32) -> Outcome {
    if caps.exclusive_rates.is_empty() {
        return Outcome::Refused(Reason::NoExclusiveFormat);
    }
    Outcome::Opens {
        period_us: exclusive_period_us(caps.exclusive_period_us, buffer_frames, sample_rate),
    }
}

/// The period, in frames at `mix_rate`, a low-latency shared stream asks for
/// to carry frames of `buffer_frames` at `sample_rate`: the smallest period
/// the mixer offers that holds a whole frame, or the largest it offers when
/// none does.
pub fn low_latency_period(
    periods: &SharedPeriods,
    mix_rate: u32,
    sample_rate: u32,
    buffer_frames: u32,
) -> u32 {
    let wanted =
        (u64::from(buffer_frames) * u64::from(mix_rate)).div_ceil(u64::from(sample_rate.max(1)));
    let wanted = u32::try_from(wanted).unwrap_or(u32::MAX);
    if wanted <= periods.min || periods.fundamental == 0 {
        return periods.min;
    }
    let steps = (wanted - periods.min).div_ceil(periods.fundamental);
    periods
        .min
        .saturating_add(steps.saturating_mul(periods.fundamental))
        .min(periods.max.max(periods.min))
}

/// The period, in microseconds, an exclusive stream asks for to carry frames
/// of `buffer_frames` at `sample_rate`: a whole frame, and never less than
/// the device's smallest period.
pub fn exclusive_period_us(smallest_us: u32, buffer_frames: u32, sample_rate: u32) -> u32 {
    frames_to_us(buffer_frames, sample_rate).max(smallest_us)
}

/// `frames` at `rate` as microseconds, rounded up
pub fn frames_to_us(frames: u32, rate: u32) -> u32 {
    let us = (u64::from(frames) * 1_000_000).div_ceil(u64::from(rate.max(1)));
    u32::try_from(us).unwrap_or(u32::MAX)
}

/// Why a way of opening did not open a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFailure {
    /// Another app has the device to itself
    InUse,
    /// The system's sound settings do not let apps take the device for
    /// themselves
    NotAllowed,
    /// The device refused every format it was asked to open in
    FormatRefused,
    Other,
}

/// How a device of the session was opened
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenedWay {
    /// The way the settings asked for
    pub asked: WasapiMode,
    /// The way it was opened. The standard way when the one asked for did
    /// not open it, and `failure` says why.
    pub opened: WasapiMode,
    pub failure: Option<OpenFailure>,
}

/// How the devices of the session are open now. A side that is not open, or
/// not open through WASAPI, is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenedWays {
    pub input: Option<OpenedWay>,
    pub output: Option<OpenedWay>,
}

/// How the devices are open now, with the note each came with. There is one
/// audio session in a process, so one pair. A note is told apart from the
/// next one by its number, so a stream closed after its replacement opened
/// does not take the replacement's note with it.
static OPENED: Mutex<[Option<(u64, OpenedWay)>; 2]> = Mutex::new([None, None]);
static NEXT_NOTE: AtomicU64 = AtomicU64::new(1);

fn slot(side: DeviceSide) -> usize {
    match side {
        DeviceSide::Input => 0,
        DeviceSide::Output => 1,
    }
}

/// How one device was opened, recorded until this is dropped (with the
/// stream it was recorded for)
pub(crate) struct Noted {
    side: DeviceSide,
    note: u64,
}

/// Records how the `side` device was opened, replacing what was recorded
pub(crate) fn note_opened(side: DeviceSide, way: OpenedWay) -> Noted {
    let note = NEXT_NOTE.fetch_add(1, Ordering::Relaxed);
    let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    opened[slot(side)] = Some((note, way));
    Noted { side, note }
}

impl Drop for Noted {
    /// Forgets how the device was opened, if what is recorded is still this
    fn drop(&mut self) {
        let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &mut opened[slot(self.side)];
        if slot.is_some_and(|(current, _)| current == self.note) {
            *slot = None;
        }
    }
}

/// How the devices of the session are open now
pub fn opened_ways() -> OpenedWays {
    let opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    OpenedWays {
        input: opened[0].map(|(_, way)| way),
        output: opened[1].map(|(_, way)| way),
    }
}

/// The prefix of a WASAPI device's id (`cpal`'s, before the endpoint id)
pub(crate) const WASAPI_ID_PREFIX: &str = "wasapi:";

/// Whether `id` is the id of a device opened through WASAPI
pub fn is_wasapi_id(id: &str) -> bool {
    id.starts_with(WASAPI_ID_PREFIX)
}

/// Whether the device `id` (`None`: the system default) is opened through
/// WASAPI: on Windows, every device but an ASIO driver
pub(crate) fn opened_through_wasapi(id: Option<&DeviceId>) -> bool {
    cfg!(target_os = "windows") && id.is_none_or(|id| is_wasapi_id(&id.0))
}

/// What the WASAPI devices read offer
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Endpoints {
    /// By device id. A WASAPI device that is not here could not be read.
    pub caps: HashMap<String, EndpointCaps>,
}

impl Endpoints {
    /// The device `id` as far as WASAPI is concerned (`None`: no device)
    pub fn endpoint(&self, id: Option<&str>) -> Endpoint<'_> {
        match id {
            Some(id) if is_wasapi_id(id) => {
                self.caps.get(id).map_or(Endpoint::Unknown, Endpoint::Known)
            }
            _ => Endpoint::NotWasapi,
        }
    }
}

/// Reads what the devices `ids` offer through WASAPI, without opening a
/// stream. Waits for the drivers to answer (at most `LIST_TIMEOUT`), so it is
/// called off the async runtime. A device that is not a WASAPI one is
/// skipped, and off Windows nothing is read.
pub fn read_endpoints(ids: &[(DeviceSide, String)]) -> Endpoints {
    let ids: Vec<_> = ids
        .iter()
        .filter(|(_, id)| is_wasapi_id(id))
        .cloned()
        .collect();
    // A driver that does not answer leaves its device unknown, as the
    // listing leaves it out
    #[cfg(target_os = "windows")]
    let caps = super::driver::bounded(
        "reading the WASAPI devices",
        super::driver::LIST_TIMEOUT,
        move || super::wasapi::probe(&ids),
    )
    .unwrap_or_default();
    #[cfg(not(target_os = "windows"))]
    let caps = {
        let _ = ids;
        HashMap::new()
    };
    Endpoints { caps }
}

/// What the settings show of the ways of opening a WASAPI device. How the
/// session's devices did open is in the session's status ([`opened_ways`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasapiSettings {
    /// The way chosen
    pub mode: WasapiMode,
    /// What each way would do with the devices chosen
    pub advice: Advice,
}

/// How the samples of a stream jamuru opens itself are laid out
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleLayout {
    F32,
    /// 32-bit integers, of which the driver may use only the top 24 bits
    I32,
    /// 24-bit integers packed in 3 bytes
    I24,
    I16,
}

#[cfg(any(target_os = "windows", test))]
impl SampleLayout {
    pub(crate) fn bytes(self) -> usize {
        match self {
            SampleLayout::F32 | SampleLayout::I32 => 4,
            SampleLayout::I24 => 3,
            SampleLayout::I16 => 2,
        }
    }

    /// Reads the little-endian samples in `bytes` into `out`, as many as fit
    pub(crate) fn decode(self, bytes: &[u8], out: &mut [f32]) {
        let samples = bytes.chunks_exact(self.bytes()).zip(out.iter_mut());
        match self {
            SampleLayout::F32 => {
                for (b, o) in samples {
                    *o = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                }
            }
            SampleLayout::I32 => {
                for (b, o) in samples {
                    *o = i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0;
                }
            }
            SampleLayout::I24 => {
                for (b, o) in samples {
                    *o = (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0;
                }
            }
            SampleLayout::I16 => {
                for (b, o) in samples {
                    *o = i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0;
                }
            }
        }
    }

    /// Writes `samples` into `bytes` little-endian, as many as fit. Samples
    /// beyond full scale are clipped.
    pub(crate) fn encode(self, samples: &[f32], bytes: &mut [u8]) {
        let pairs = bytes.chunks_exact_mut(self.bytes()).zip(samples);
        match self {
            SampleLayout::F32 => {
                for (b, s) in pairs {
                    b.copy_from_slice(&s.to_le_bytes());
                }
            }
            SampleLayout::I32 => {
                for (b, s) in pairs {
                    let v = (f64::from(s.clamp(-1.0, 1.0)) * 2_147_483_647.0) as i32;
                    b.copy_from_slice(&v.to_le_bytes());
                }
            }
            SampleLayout::I24 => {
                for (b, s) in pairs {
                    let v = (s.clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                    b.copy_from_slice(&v.to_le_bytes()[..3]);
                }
            }
            SampleLayout::I16 => {
                for (b, s) in pairs {
                    let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
                    b.copy_from_slice(&v.to_le_bytes());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The interface the device round trip was measured on: a USB 1.1 codec
    /// whose driver keeps the shared period at 10 ms, and whose exclusive
    /// mode runs down to 3 ms at 16 bits
    fn fixed_period_codec() -> EndpointCaps {
        EndpointCaps {
            mix_rate: 48000,
            shared_period_us: 10_000,
            low_latency: Some(SharedPeriods {
                default: 480,
                fundamental: 480,
                min: 480,
                max: 480,
            }),
            exclusive_period_us: 3_000,
            exclusive_rates: vec![44100, 48000],
        }
    }

    /// A device whose driver offers shared periods down to 2.67 ms (128
    /// frames), in steps of 32 frames
    fn small_period_device() -> EndpointCaps {
        EndpointCaps {
            low_latency: Some(SharedPeriods {
                default: 480,
                fundamental: 32,
                min: 128,
                max: 480,
            }),
            ..fixed_period_codec()
        }
    }

    fn advice_on(advice: &Advice, mode: WasapiMode) -> &ModeAdvice {
        advice.modes.iter().find(|m| m.mode == mode).unwrap()
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_the_driver_keeps_the_period_at_10_ms_low_latency_is_not_recommended_and_exclusive_is() {
        let codec = fixed_period_codec();

        let advice = advise(Endpoint::Known(&codec), Endpoint::Known(&codec), 48000, 64);

        assert!(advice.applies);
        let low_latency = advice_on(&advice, WasapiMode::LowLatency);
        assert_eq!(low_latency.verdict, Verdict::NotRecommended);
        assert_eq!(low_latency.reason, Reason::SamePeriod);
        assert_eq!(low_latency.input_period_us, Some(10_000));
        let exclusive = advice_on(&advice, WasapiMode::Exclusive);
        assert_eq!(exclusive.verdict, Verdict::Recommended);
        assert_eq!(exclusive.reason, Reason::OpensExclusively);
        assert_eq!(exclusive.output_period_us, Some(3_000));
        let standard = advice_on(&advice, WasapiMode::Shared);
        assert_eq!(standard.verdict, Verdict::NotRecommended);
        assert_eq!(standard.reason, Reason::FasterAvailable);
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_the_driver_offers_a_smaller_shared_period_low_latency_is_recommended_at_it() {
        let device = small_period_device();

        let advice = advise(
            Endpoint::Known(&device),
            Endpoint::Known(&device),
            48000,
            64,
        );

        let low_latency = advice_on(&advice, WasapiMode::LowLatency);
        assert_eq!(low_latency.verdict, Verdict::Recommended);
        assert_eq!(low_latency.reason, Reason::ShorterPeriod);
        assert_eq!(low_latency.input_period_us, Some(2_667));
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_no_faster_way_opens_the_devices_the_standard_way_is_recommended() {
        let device = EndpointCaps {
            low_latency: None,
            exclusive_rates: Vec::new(),
            ..fixed_period_codec()
        };

        let advice = advise(
            Endpoint::Known(&device),
            Endpoint::Known(&device),
            48000,
            64,
        );

        let standard = advice_on(&advice, WasapiMode::Shared);
        assert_eq!(standard.verdict, Verdict::Recommended);
        assert_eq!(standard.reason, Reason::NothingFaster);
        let low_latency = advice_on(&advice, WasapiMode::LowLatency);
        assert_eq!(low_latency.verdict, Verdict::Unavailable);
        assert_eq!(low_latency.reason, Reason::NoLowLatencyMode);
        let exclusive = advice_on(&advice, WasapiMode::Exclusive);
        assert_eq!(exclusive.verdict, Verdict::Unavailable);
        assert_eq!(exclusive.reason, Reason::NoExclusiveFormat);
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_one_device_does_not_open_exclusively_exclusive_is_not_offered() {
        let codec = fixed_period_codec();
        let microphone = EndpointCaps {
            exclusive_rates: Vec::new(),
            ..fixed_period_codec()
        };

        let advice = advise(
            Endpoint::Known(&microphone),
            Endpoint::Known(&codec),
            48000,
            64,
        );

        let exclusive = advice_on(&advice, WasapiMode::Exclusive);
        assert_eq!(exclusive.verdict, Verdict::Unavailable);
        assert_eq!(exclusive.reason, Reason::NoExclusiveFormat);
        assert_eq!(
            advice_on(&advice, WasapiMode::Shared).verdict,
            Verdict::Recommended
        );
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_a_device_could_not_be_asked_the_faster_ways_are_not_recommended_and_still_offered() {
        let codec = fixed_period_codec();

        let advice = advise(Endpoint::Unknown, Endpoint::Known(&codec), 48000, 64);

        for mode in [WasapiMode::LowLatency, WasapiMode::Exclusive] {
            let way = advice_on(&advice, mode);
            assert_eq!(way.verdict, Verdict::NotRecommended, "{mode:?}");
            assert_eq!(way.reason, Reason::Unknown, "{mode:?}");
        }
        let standard = advice_on(&advice, WasapiMode::Shared);
        assert_eq!(standard.verdict, Verdict::Recommended);
        assert_eq!(standard.input_period_us, None);
        assert_eq!(standard.output_period_us, Some(10_000));
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_both_devices_are_an_asio_driver_the_choice_does_not_apply() {
        let advice = advise(Endpoint::NotWasapi, Endpoint::NotWasapi, 48000, 64);

        assert!(!advice.applies);
        assert!(advice.modes.is_empty());
    }

    /// Verifies: REQ-GUI-031
    #[test]
    fn when_only_one_device_is_wasapi_the_advice_is_for_that_device() {
        let codec = fixed_period_codec();

        let advice = advise(Endpoint::NotWasapi, Endpoint::Known(&codec), 48000, 64);

        let exclusive = advice_on(&advice, WasapiMode::Exclusive);
        assert_eq!(exclusive.verdict, Verdict::Recommended);
        assert_eq!(exclusive.input_period_us, None);
        assert_eq!(exclusive.output_period_us, Some(3_000));
    }

    /// Verifies: REQ-AUD-042
    #[test]
    fn when_the_buffer_is_longer_than_the_smallest_period_an_exclusive_stream_runs_at_the_buffer() {
        // 256 frames at 48 kHz is 5.33 ms, longer than the 3 ms the device allows
        assert_eq!(exclusive_period_us(3_000, 256, 48000), 5_334);
        assert_eq!(exclusive_period_us(3_000, 64, 48000), 3_000);
    }

    /// Verifies: REQ-AUD-042
    #[test]
    fn when_the_buffer_falls_between_two_shared_periods_the_larger_one_is_asked_for() {
        let periods = SharedPeriods {
            default: 480,
            fundamental: 32,
            min: 128,
            max: 480,
        };

        assert_eq!(low_latency_period(&periods, 48000, 48000, 64), 128);
        assert_eq!(low_latency_period(&periods, 48000, 48000, 129), 160);
        assert_eq!(low_latency_period(&periods, 48000, 48000, 256), 256);
        // A 44.1 kHz mixer carrying a 48 kHz session's frames of 256 needs 236
        assert_eq!(low_latency_period(&periods, 44100, 48000, 256), 256);
        assert_eq!(low_latency_period(&periods, 48000, 48000, 1024), 480);
    }

    /// Verifies: REQ-AUD-042
    #[test]
    fn when_a_driver_offers_one_shared_period_that_period_is_asked_for() {
        let fixed = SharedPeriods {
            default: 441,
            fundamental: 441,
            min: 441,
            max: 441,
        };

        assert_eq!(low_latency_period(&fixed, 44100, 48000, 32), 441);
        assert_eq!(low_latency_period(&fixed, 44100, 48000, 1024), 441);
    }

    /// Verifies: REQ-AUD-042
    #[test]
    fn every_sample_layout_carries_a_signal_through_encode_and_decode() {
        let signal = [0.0f32, 0.5, -0.5, 0.999, -1.0, 0.123_456];
        for layout in [
            SampleLayout::F32,
            SampleLayout::I32,
            SampleLayout::I24,
            SampleLayout::I16,
        ] {
            let mut bytes = vec![0u8; signal.len() * layout.bytes()];
            layout.encode(&signal, &mut bytes);
            let mut back = [0.0f32; 6];
            layout.decode(&bytes, &mut back);

            let step = match layout {
                SampleLayout::I16 => 1.0 / 32_768.0,
                SampleLayout::I24 => 1.0 / 8_388_608.0,
                _ => 1.0e-6,
            };
            for (sent, got) in signal.iter().zip(back) {
                assert!(
                    (sent - got).abs() <= step * 2.0,
                    "{layout:?}: {sent} came back as {got}"
                );
            }
        }
    }

    /// Verifies: REQ-AUD-042
    #[test]
    fn a_sample_beyond_full_scale_is_clipped_rather_than_wrapped_around() {
        for layout in [SampleLayout::I32, SampleLayout::I24, SampleLayout::I16] {
            let mut bytes = vec![0u8; 2 * layout.bytes()];
            layout.encode(&[1.5, -1.5], &mut bytes);
            let mut back = [0.0f32; 2];
            layout.decode(&bytes, &mut back);

            assert!(back[0] > 0.99, "{layout:?}: {}", back[0]);
            assert!(back[1] < -0.99, "{layout:?}: {}", back[1]);
        }
    }

    /// Verifies: REQ-AUD-042
    #[test]
    fn when_a_stream_closes_after_its_replacement_opened_the_replacement_stays_recorded() {
        let way = OpenedWay {
            asked: WasapiMode::Exclusive,
            opened: WasapiMode::Exclusive,
            failure: None,
        };
        let old = note_opened(DeviceSide::Output, way);
        let replacement = note_opened(
            DeviceSide::Output,
            OpenedWay {
                opened: WasapiMode::Shared,
                failure: Some(OpenFailure::InUse),
                ..way
            },
        );

        drop(old);
        assert_eq!(
            opened_ways().output.map(|w| w.failure),
            Some(Some(OpenFailure::InUse))
        );
        drop(replacement);
        assert_eq!(opened_ways().output, None);
    }
}
