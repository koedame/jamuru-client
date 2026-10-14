//! Where a debugger listens to, or feeds, the audio path of a running session (ADR-044).
//!
//! The paths call the taps on every frame, on the audio threads, so a tap never waits. Nothing is
//! set in a release build and each call is then one load of an empty cell.

use std::sync::OnceLock;

/// What a debugger does at the three points of the audio path
pub trait Taps: Send + Sync {
    /// What the input device captured, in the channels picked from it
    fn input(&self, samples: &[f32], channels: usize);
    /// What is about to be sent: stereo, with volume, pan and mute applied
    fn sent(&self, stereo: &mut [f32]);
    /// What is about to be played: stereo, after the peers are mixed
    fn output(&self, stereo: &mut [f32]);
}

static TAPS: OnceLock<&'static dyn Taps> = OnceLock::new();

/// Puts `taps` in the audio path. Only the first call counts.
pub fn set_taps(taps: &'static dyn Taps) {
    let _ = TAPS.set(taps);
}

pub(crate) fn taps() -> Option<&'static dyn Taps> {
    TAPS.get().copied()
}
