//! Makes calls into the audio driver stall, the way a driver that has hung
//! does (macOS's `coreaudiod` or an interface's driver stops answering), and
//! makes open streams stop calling back, the way a stream the audio host has
//! left behind does (PipeWire's ALSA plugin after a run of xruns).
//!
//! The calls that can stall each pass through [`point`]. Nothing stalls unless
//! a test or a debug build asked for it with [`stall`] or [`stall_streams`]; a
//! build with neither has no fault code at all, so a release build cannot be
//! made to hang.

/// A call into the audio driver that can be made to stall
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    /// Listing the input devices
    ListInputs,
    /// Listing the output devices
    ListOutputs,
    /// Opening an input stream
    OpenInput,
    /// Opening an output stream
    OpenOutput,
}

impl Call {
    /// The name a debug caller gives the call
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "list_inputs" => Some(Self::ListInputs),
            "list_outputs" => Some(Self::ListOutputs),
            "open_input" => Some(Self::OpenInput),
            "open_output" => Some(Self::OpenOutput),
            _ => None,
        }
    }
}

/// Which streams stop calling back
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStall {
    /// None: lifts what was asked before
    Off,
    /// The streams open now. One opened after this works, as it does when a
    /// stalled stream is opened again and the host is well again.
    Open,
    /// Every stream, including those opened later: opening again does not help.
    All,
}

impl StreamStall {
    /// The name a debug caller gives the mode
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "off" => Some(Self::Off),
            "open" => Some(Self::Open),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

#[cfg(any(test, feature = "fault-injection"))]
mod armed {
    use super::{Call, StreamStall};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    static NEXT_SERIAL: AtomicU64 = AtomicU64::new(0);
    /// The streams with a serial below this stop calling back
    static STALLED_BELOW: AtomicU64 = AtomicU64::new(0);

    pub fn stall_streams(mode: StreamStall) {
        let below = match mode {
            StreamStall::Off => 0,
            StreamStall::Open => NEXT_SERIAL.load(Ordering::SeqCst),
            StreamStall::All => u64::MAX,
        };
        STALLED_BELOW.store(below, Ordering::SeqCst);
    }

    pub fn next_serial() -> u64 {
        NEXT_SERIAL.fetch_add(1, Ordering::SeqCst)
    }

    pub fn stream_stalled(serial: u64) -> bool {
        serial < STALLED_BELOW.load(Ordering::Relaxed)
    }

    static STALLS: Mutex<Vec<(Call, Duration)>> = Mutex::new(Vec::new());

    /// From now on `call` takes `duration` to return, each time it is made.
    /// A zero duration lifts the stall.
    pub fn stall(call: Call, duration: Duration) {
        let mut stalls = STALLS.lock().unwrap_or_else(|e| e.into_inner());
        stalls.retain(|(c, _)| *c != call);
        if !duration.is_zero() {
            stalls.push((call, duration));
        }
    }

    /// Lifts every stall.
    pub fn clear() {
        STALLS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn point(call: Call) {
        let wait = STALLS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(c, _)| *c == call)
            .map(|(_, d)| *d);
        if let Some(wait) = wait {
            std::thread::sleep(wait);
        }
    }
}

#[cfg(any(test, feature = "fault-injection"))]
pub use armed::{clear, stall, stall_streams};

/// A number for a stream being opened, to hand to [`stream_stalled`] from its
/// callback.
#[cfg(any(test, feature = "fault-injection"))]
pub(crate) fn stream_serial() -> u64 {
    armed::next_serial()
}

/// Whether the stream numbered `serial` has been made to stop calling back:
/// its callback returns at once, doing nothing.
#[cfg(any(test, feature = "fault-injection"))]
pub(crate) fn stream_stalled(serial: u64) -> bool {
    armed::stream_stalled(serial)
}

#[cfg(not(any(test, feature = "fault-injection")))]
#[inline(always)]
pub(crate) fn stream_serial() -> u64 {
    0
}

#[cfg(not(any(test, feature = "fault-injection")))]
#[inline(always)]
pub(crate) fn stream_stalled(_serial: u64) -> bool {
    false
}

/// Where a call into the driver begins: waits if the call has been made to
/// stall.
#[cfg(any(test, feature = "fault-injection"))]
pub(crate) fn point(call: Call) {
    armed::point(call);
}

#[cfg(not(any(test, feature = "fault-injection")))]
#[inline(always)]
pub(crate) fn point(_call: Call) {}
