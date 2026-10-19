//! Whether the system lets the app use the microphone.
//!
//! macOS asks the person the first time an app opens an input stream, and
//! the stream's open does not return until they answer. Every driver call
//! queued behind it stalls too, so a session that opens its devices while
//! that question is on screen loses its output as well. The answer is asked
//! for explicitly here, before any device is opened, so the screen can say
//! what is being waited for.
//!
//! Only macOS has this question. Everywhere else the microphone is always
//! available to the app.

use std::time::Duration;

/// What the system says about the app's use of the microphone
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicAccess {
    /// Allowed (or the system never asks)
    Granted,
    /// Not asked yet, or asked and not answered: opening an input now would
    /// bring the question up and wait for it
    Undecided,
    /// Declined, or switched off for the app in the system's privacy settings
    Refused,
}

/// The system's answer about the microphone, and the way to ask for it
pub trait MicSystem {
    /// What the system has decided so far. Never waits.
    fn status(&self) -> MicAccess;

    /// Brings the system's question up if it has not been asked, and waits at
    /// most `wait` for the answer. [`MicAccess::Undecided`] when it did not come.
    fn ask(&self, wait: Duration) -> MicAccess;
}

/// The operating system's own answer
pub struct SystemMic;

#[cfg(target_os = "macos")]
impl MicSystem for SystemMic {
    fn status(&self) -> MicAccess {
        use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

        // SAFETY: `AVMediaTypeAudio` is a constant AVFoundation defines and
        // never changes; reading the authorization status has no side effect.
        let status = unsafe {
            let Some(audio) = AVMediaTypeAudio else {
                return MicAccess::Granted;
            };
            AVCaptureDevice::authorizationStatusForMediaType(audio)
        };
        match status {
            AVAuthorizationStatus::Authorized => MicAccess::Granted,
            AVAuthorizationStatus::NotDetermined => MicAccess::Undecided,
            _ => MicAccess::Refused,
        }
    }

    fn ask(&self, wait: Duration) -> MicAccess {
        use std::sync::mpsc;

        use block2::RcBlock;
        use objc2::runtime::Bool;
        use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

        if self.status() != MicAccess::Undecided {
            return self.status();
        }
        let (answered, answer) = mpsc::channel();
        let handler = RcBlock::new(move |granted: Bool| {
            let _ = answered.send(granted.as_bool());
        });
        // SAFETY: the handler is an owned block that AVFoundation copies; it
        // only sends on a channel, which is fine from whatever queue it is
        // called on.
        unsafe {
            let Some(audio) = AVMediaTypeAudio else {
                return MicAccess::Granted;
            };
            AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler);
        }
        match answer.recv_timeout(wait) {
            Ok(true) => MicAccess::Granted,
            Ok(false) => MicAccess::Refused,
            Err(_) => MicAccess::Undecided,
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl MicSystem for SystemMic {
    fn status(&self) -> MicAccess {
        MicAccess::Granted
    }

    fn ask(&self, _wait: Duration) -> MicAccess {
        MicAccess::Granted
    }
}
