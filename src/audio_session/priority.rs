//! Real-time priority for the audio thread.

/// Real-time thread priority for Linux (1-99, higher = more priority)
/// 99 = maximum, but risks system freeze if thread hangs
/// 90 = very high, leaves headroom for critical kernel threads
#[cfg(target_os = "linux")]
const REALTIME_PRIORITY: i32 = 90;

/// Set real-time priority for the CURRENT thread (call from within audio thread)
/// This reduces buffer underruns by giving the audio thread scheduling priority
#[cfg(target_os = "macos")]
pub(crate) fn set_current_thread_realtime_priority() {
    // Use macOS Mach thread policy API - this is what professional DAWs use
    // (Logic Pro, Ableton, Pro Tools, etc.)
    // Does NOT require root, and is more effective than SCHED_FIFO on macOS

    #[repr(C)]
    struct ThreadTimeConstraintPolicy {
        period: u32,      // Interval between processing (in Mach absolute time units)
        computation: u32, // Time needed for computation
        constraint: u32,  // Maximum time before deadline
        preemptible: i32, // Can be preempted?
    }

    const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
    const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;

    extern "C" {
        fn mach_thread_self() -> u32;
        fn thread_policy_set(
            thread: u32,
            flavor: u32,
            policy_info: *const ThreadTimeConstraintPolicy,
            count: u32,
        ) -> i32;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe {
        // Get timebase info to convert nanoseconds to Mach absolute time
        let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
        mach_timebase_info(&mut timebase);

        // Convert nanoseconds to Mach absolute time units
        let ns_to_abs =
            |ns: u64| -> u32 { ((ns * timebase.denom as u64) / timebase.numer as u64) as u32 };

        // Audio timing constraints (for 48kHz, ~1ms period with small buffer)
        // period: how often the thread runs (e.g., every 1ms for audio callback)
        // computation: how much CPU time it needs per period
        // constraint: deadline (must complete within this time)
        let period_ns = 1_000_000; // 1ms period (audio callback interval)
        let computation_ns = 500_000; // 0.5ms computation time
        let constraint_ns = 1_000_000; // 1ms deadline

        let policy = ThreadTimeConstraintPolicy {
            period: ns_to_abs(period_ns),
            computation: ns_to_abs(computation_ns),
            constraint: ns_to_abs(constraint_ns),
            preemptible: 0, // Don't preempt during computation
        };

        let thread = mach_thread_self();
        let result = thread_policy_set(
            thread,
            THREAD_TIME_CONSTRAINT_POLICY,
            &policy,
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        );

        if result == 0 {
            tracing::info!("Audio thread: macOS real-time priority enabled (TIME_CONSTRAINT)");
        } else {
            // Fall back to nice value
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
            tracing::info!(
                "Audio thread: using nice -20 (TIME_CONSTRAINT failed: {})",
                result
            );
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn set_current_thread_realtime_priority() {
    unsafe {
        let policy = libc::SCHED_FIFO;
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = REALTIME_PRIORITY;
        let result = libc::pthread_setschedparam(libc::pthread_self(), policy, &param);
        if result != 0 {
            // Fall back to nice value if SCHED_FIFO fails (requires rtprio limit)
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
            tracing::info!("Audio thread: using nice -20 (SCHED_FIFO requires rtprio config)");
        } else {
            tracing::info!(
                "Audio thread: real-time priority enabled (SCHED_FIFO {})",
                REALTIME_PRIORITY
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn set_current_thread_realtime_priority() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
        HIGH_PRIORITY_CLASS, THREAD_PRIORITY_TIME_CRITICAL,
    };

    unsafe {
        // First, elevate the process priority class
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);

        // Then set thread to TIME_CRITICAL (highest within the priority class)
        let result = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        if result.is_ok() {
            tracing::info!("Audio thread: real-time priority enabled (HIGH + TIME_CRITICAL)");
        } else {
            tracing::warn!("Audio thread: failed to set thread priority");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub(crate) fn set_current_thread_realtime_priority() {
    tracing::info!("Audio thread: real-time priority not supported on this platform");
}
