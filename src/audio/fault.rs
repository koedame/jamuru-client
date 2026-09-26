//! Makes calls into the audio driver stall, the way a driver that has hung
//! does (macOS's `coreaudiod` or an interface's driver stops answering).
//!
//! The calls that can stall each pass through [`point`]. Nothing stalls unless
//! a test or a debug build asked for it with [`stall`]; a build with neither
//! has no fault code at all, so a release build cannot be made to hang.

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

#[cfg(any(test, feature = "fault-injection"))]
mod armed {
    use super::Call;
    use std::sync::Mutex;
    use std::time::Duration;

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
pub use armed::{clear, stall};

/// Where a call into the driver begins: waits if the call has been made to
/// stall.
#[cfg(any(test, feature = "fault-injection"))]
pub(crate) fn point(call: Call) {
    armed::point(call);
}

#[cfg(not(any(test, feature = "fault-injection")))]
#[inline(always)]
pub(crate) fn point(_call: Call) {}
