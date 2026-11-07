//! What a session tells the operating system about itself: that it is audio which must not be
//! delayed. The system saves power by holding back background work, and an audio session that
//! sits in a hidden window looks like background work to it. Every request lasts as long as the
//! session does: [`OsHints::enter`] makes them, dropping the value takes them back.
//!
//! - macOS: an activity that is user-initiated and latency-critical, which keeps App Nap from
//!   slowing the timers and the network of an app whose window is hidden or minimized.
//! - Windows: a one-millisecond system timer (`timeBeginPeriod`), so that a wait of a few
//!   hundred microseconds is not rounded up to the next tick of the 15.6 ms default; and no
//!   EcoQoS (the power throttling of Windows 11) and no loss of the timer resolution for the
//!   process, both of which Windows applies to a process whose window is minimized.
//!
//! A request the system refuses is logged, and the session runs without it.

use std::net::UdpSocket;

use socket2::SockRef;
use tracing::{info, warn};

/// What can be asked for, one bit each
pub mod hint {
    /// macOS: the latency-critical activity
    pub const ACTIVITY: u32 = 1;
    /// Windows: the one-millisecond system timer
    pub const TIMER: u32 = 1 << 1;
    /// Windows: no power throttling for the process
    pub const THROTTLING: u32 = 1 << 2;
    /// Every system: larger buffers on the audio socket
    pub const SOCKET_BUFFERS: u32 = 1 << 3;
    /// All of them
    pub const ALL: u32 = ACTIVITY | TIMER | THROTTLING | SOCKET_BUFFERS;
}

/// What the buffers of the audio socket are asked to hold, each direction. At 750 packets per
/// second of about 400 bytes from each of 16 participants that is a little over 200 ms of what
/// they send together, so that a receive thread kept away for a few tens of milliseconds loses
/// no packet.
const SOCKET_BUFFER_BYTES: usize = 1 << 20;

#[cfg(any(test, feature = "fault-injection"))]
mod switched_off {
    use std::sync::atomic::{AtomicU32, Ordering};

    static OFF: AtomicU32 = AtomicU32::new(0);

    pub fn set(mask: u32) {
        OFF.store(mask, Ordering::SeqCst);
    }

    pub fn get() -> u32 {
        OFF.load(Ordering::SeqCst)
    }
}

/// Leaves out the hints in `mask` (bits of [`hint`]) from the next session on, so that a debug
/// build can measure with and without each of them. 0 puts all of them back.
#[cfg(any(test, feature = "fault-injection"))]
pub fn turn_off(mask: u32) {
    switched_off::set(mask);
}

fn wanted() -> u32 {
    #[cfg(any(test, feature = "fault-injection"))]
    {
        hint::ALL & !switched_off::get()
    }
    #[cfg(not(any(test, feature = "fault-injection")))]
    {
        hint::ALL
    }
}

/// The hints a session made. Dropping it takes them back (the fields do it as they drop).
pub(super) struct OsHints {
    #[cfg(target_os = "macos")]
    _activity: Option<macos::Activity>,
    #[cfg(windows)]
    _timer: Option<windows_hints::Timer>,
    #[cfg(windows)]
    _throttling: Option<windows_hints::NoThrottling>,
}

impl OsHints {
    /// Makes the hints the system takes
    pub(super) fn enter() -> Self {
        Self::enter_with(wanted())
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_variables))]
    fn enter_with(wanted: u32) -> Self {
        Self {
            #[cfg(target_os = "macos")]
            _activity: (wanted & hint::ACTIVITY != 0)
                .then(macos::Activity::begin)
                .and_then(log_refusal("latency-critical activity")),
            #[cfg(windows)]
            _timer: (wanted & hint::TIMER != 0)
                .then(windows_hints::Timer::begin)
                .and_then(log_refusal("one-millisecond system timer")),
            #[cfg(windows)]
            _throttling: (wanted & hint::THROTTLING != 0)
                .then(windows_hints::NoThrottling::begin)
                .and_then(log_refusal("no power throttling")),
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
fn log_refusal<T>(what: &'static str) -> impl FnOnce(Result<T, String>) -> Option<T> {
    move |outcome| match outcome {
        Ok(taken) => {
            info!("Told the system the session is latency-critical: {what}");
            Some(taken)
        }
        Err(why) => {
            warn!("The system did not take the request ({what}): {why}");
            None
        }
    }
}

/// Asks for larger buffers on the audio socket and says in the log what it was given. A system
/// that keeps a smaller size (Linux caps it at `net.core.rmem_max`) is not an error: the socket
/// runs with what it got.
pub fn enlarge_socket_buffers(socket: &UdpSocket) {
    enlarge(socket, wanted());
}

fn enlarge(socket: &UdpSocket, wanted: u32) {
    if wanted & hint::SOCKET_BUFFERS == 0 {
        return;
    }
    let sock = SockRef::from(socket);
    let (receive_before, send_before) = (
        sock.recv_buffer_size().unwrap_or(0),
        sock.send_buffer_size().unwrap_or(0),
    );
    let asked_receive = sock.set_recv_buffer_size(SOCKET_BUFFER_BYTES);
    let asked_send = sock.set_send_buffer_size(SOCKET_BUFFER_BYTES);
    let (receive_after, send_after) = (
        sock.recv_buffer_size().unwrap_or(0),
        sock.send_buffer_size().unwrap_or(0),
    );
    match (asked_receive, asked_send) {
        (Ok(()), Ok(())) => info!(
            "Audio socket buffers: receive {receive_before} -> {receive_after} bytes, send {send_before} -> {send_after} bytes"
        ),
        (receive, send) => warn!(
            "The system did not take the audio socket buffer sizes (receive: {}, send: {}); they stay {receive_after} and {send_after} bytes",
            receive.err().map_or("ok".to_string(), |e| e.to_string()),
            send.err().map_or("ok".to_string(), |e| e.to_string()),
        ),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    /// An activity of the process: user-initiated (the system does not nap the app, and does not
    /// idle-sleep for it) and latency-critical (timers are not coalesced)
    pub struct Activity {
        token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    }

    impl Activity {
        pub fn begin() -> Result<Self, String> {
            let options = NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical;
            let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
                options,
                &NSString::from_str("Live audio session"),
            );
            Ok(Self { token })
        }
    }

    impl Drop for Activity {
        fn drop(&mut self) {
            // SAFETY: the token is what `beginActivityWithOptions` returned
            unsafe { NSProcessInfo::processInfo().endActivity(&self.token) };
        }
    }
}

#[cfg(windows)]
mod windows_hints {
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
    };

    /// The system timer at one millisecond. Windows 10 and later give it to this process only,
    /// and take it away while the process's window is minimized unless the process opts out
    /// (`NoThrottling`).
    pub struct Timer;

    const PERIOD_MS: u32 = 1;

    impl Timer {
        pub fn begin() -> Result<Self, String> {
            // SAFETY: no pointers; balanced by `timeEndPeriod` with the same value
            let result = unsafe { timeBeginPeriod(PERIOD_MS) };
            if result == 0 {
                Ok(Self)
            } else {
                Err(format!("timeBeginPeriod returned {result}"))
            }
        }
    }

    impl Drop for Timer {
        fn drop(&mut self) {
            // SAFETY: matches the `timeBeginPeriod` of `begin`
            unsafe { timeEndPeriod(PERIOD_MS) };
        }
    }

    /// The process opted out of EcoQoS (execution speed throttling) and of losing the timer
    /// resolution, for as long as it lives. Applies to all of its threads.
    pub struct NoThrottling;

    fn set(control_mask: u32) -> Result<(), String> {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: control_mask,
            // Control bit set and state bit clear: the throttling is off. No control bit at
            // all: the system decides again.
            StateMask: 0,
        };
        // SAFETY: a valid state of the size given, read during the call only
        unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                std::ptr::addr_of!(state).cast(),
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
        }
        .map_err(|e| e.to_string())
    }

    impl NoThrottling {
        pub fn begin() -> Result<Self, String> {
            set(PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION)?;
            Ok(Self)
        }
    }

    impl Drop for NoThrottling {
        fn drop(&mut self) {
            let _ = set(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-AUD-134
    #[test]
    fn when_the_socket_buffers_are_enlarged_then_they_are_not_smaller_than_before() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sock = SockRef::from(&socket);
        let before = sock.recv_buffer_size().unwrap();
        enlarge(&socket, hint::ALL);
        assert!(sock.recv_buffer_size().unwrap() >= before);
        assert!(sock.send_buffer_size().unwrap() > 0);
    }

    /// Verifies: REQ-AUD-134
    #[test]
    fn when_the_socket_buffer_hint_is_left_out_then_the_buffers_stay_as_they_were() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sock = SockRef::from(&socket);
        let before = (
            sock.recv_buffer_size().unwrap(),
            sock.send_buffer_size().unwrap(),
        );
        enlarge(&socket, hint::ALL & !hint::SOCKET_BUFFERS);
        let after = (
            sock.recv_buffer_size().unwrap(),
            sock.send_buffer_size().unwrap(),
        );
        assert_eq!(after, before);
    }

    #[test]
    fn when_the_session_enters_and_leaves_with_all_or_no_hints_then_nothing_panics() {
        {
            let _all = OsHints::enter_with(hint::ALL);
        }
        let _none = OsHints::enter_with(0);
    }

    /// What the system holds as the power throttling of this process: the bits the process
    /// controls, and what it set them to
    #[cfg(windows)]
    fn throttling_of_this_process() -> (u32, u32) {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, GetProcessInformation, ProcessPowerThrottling,
            PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE,
        };
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: 0,
            StateMask: 0,
        };
        // SAFETY: a valid state of the size given, written during the call
        unsafe {
            GetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                std::ptr::addr_of_mut!(state).cast(),
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            )
            .unwrap();
        }
        (state.ControlMask, state.StateMask)
    }

    /// Verifies: REQ-AUD-134
    #[cfg(windows)]
    #[test]
    fn when_a_session_is_in_progress_then_windows_holds_no_throttling_for_the_process_and_lets_go_after(
    ) {
        let before = throttling_of_this_process();
        let hints = OsHints::enter_with(hint::THROTTLING);
        let (control, state) = throttling_of_this_process();
        let during = (control != 0, state);
        drop(hints);
        let after = throttling_of_this_process();
        assert_eq!(
            before,
            (0, 0),
            "the test process starts with the system's choice"
        );
        assert_eq!(
            during,
            (true, 0),
            "the process controls the bits and has them off"
        );
        assert_eq!(
            after,
            (0, 0),
            "dropping the hints gives the choice back to the system"
        );
    }
}
