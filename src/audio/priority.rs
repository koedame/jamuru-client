//! Real-time priority for the audio thread.

/// Real-time thread priority for Linux (1-99, higher = more priority)
/// 99 = maximum, but risks system freeze if thread hangs
/// 90 = very high, leaves headroom for critical kernel threads
#[cfg(target_os = "linux")]
const REALTIME_PRIORITY: i32 = 90;

/// Set real-time priority for the CURRENT thread (call from within audio thread)
/// This reduces buffer underruns by giving the audio thread scheduling priority
#[cfg(target_os = "macos")]
pub(crate) fn set_current_thread_realtime_priority(_role: &str) {
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

/// Asks for real-time priority for the thread that calls, and says in the log how it went.
/// `role` names the thread in the log.
///
/// Tried in turn, until one gives the thread priority over the ordinary ones:
/// 1. `SCHED_FIFO` directly. Needs an `RLIMIT_RTPRIO` above 0 (an audio group set up for it, a
///    container started with the limit raised). Threads started from this one inherit it.
/// 2. The real-time service of the desktop, rtkit (`rtkit`), which is how PipeWire and
///    PulseAudio get theirs on a stock desktop. What it gives is not inherited by threads
///    started later, so each audio thread asks for itself.
/// 3. A lower nice value, directly and then through rtkit. Not real-time, but better than none.
#[cfg(target_os = "linux")]
pub(crate) fn set_current_thread_realtime_priority(role: &str) {
    let tid = current_thread_id();
    // rtkit is a call over D-Bus that a stuck bus can leave unanswered; the thread
    // does not wait on it for long
    let asked = super::bounded("real-time priority request", RTKIT_WAIT, move || {
        promote_thread(tid)
    });
    log_outcome(role, asked.unwrap_or_else(|e| Err(e.to_string())));
}

/// Gives the thread that calls real-time priority the first time it is called, if it has none:
/// for the thread of an audio callback, which the audio library started and which cannot ask
/// for itself before it runs. The request goes through a thread of its own, so the callback does
/// not wait for D-Bus.
#[cfg(target_os = "linux")]
pub(crate) fn promote_callback_thread(role: &'static str) {
    thread_local! {
        static ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if ASKED.with(|asked| asked.replace(true)) || has_real_time_policy(0) {
        return;
    }
    let tid = current_thread_id();
    let _ = std::thread::Builder::new()
        .name("audio-priority".to_string())
        .spawn(move || log_outcome(role, promote_thread(tid)));
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn promote_callback_thread(_role: &'static str) {}

/// How long the thread that calls waits for the real-time priority to be settled
#[cfg(target_os = "linux")]
const RTKIT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// What a thread was given
#[cfg(target_os = "linux")]
enum Granted {
    Fifo(i32),
    Rtkit(i32),
    Nice {
        nice: i32,
        why_not_real_time: String,
    },
}

#[cfg(target_os = "linux")]
fn log_outcome(role: &str, outcome: Result<Granted, String>) {
    match outcome {
        Ok(Granted::Fifo(priority)) => tracing::info!(
            "Audio thread ({}): real-time priority enabled (SCHED_FIFO {})",
            role,
            priority
        ),
        Ok(Granted::Rtkit(priority)) => tracing::info!(
            "Audio thread ({}): real-time priority enabled through rtkit (SCHED_RR {})",
            role,
            priority
        ),
        Ok(Granted::Nice {
            nice,
            why_not_real_time,
        }) => tracing::warn!(
            "Audio thread ({}): no real-time priority, using nice {} ({})",
            role,
            nice,
            why_not_real_time
        ),
        Err(why) => tracing::warn!(
            "Audio thread ({}): no real-time priority, runs at normal priority ({})",
            role,
            why
        ),
    }
}

#[cfg(target_os = "linux")]
fn current_thread_id() -> libc::pid_t {
    // SAFETY: gettid takes no arguments and cannot fail
    unsafe { libc::syscall(libc::SYS_gettid) as libc::pid_t }
}

/// Whether the thread is scheduled `SCHED_FIFO` or `SCHED_RR`
#[cfg(target_os = "linux")]
fn has_real_time_policy(tid: libc::pid_t) -> bool {
    // SAFETY: only reads the policy of a thread
    let policy = unsafe { libc::sched_getscheduler(tid) };
    // SCHED_RESET_ON_FORK, which rtkit adds, is a flag on top of the policy
    policy >= 0
        && matches!(
            policy & !libc::SCHED_RESET_ON_FORK,
            libc::SCHED_FIFO | libc::SCHED_RR
        )
}

/// Raises the priority of the thread `tid` as far as the system lets this process
#[cfg(target_os = "linux")]
fn promote_thread(tid: libc::pid_t) -> Result<Granted, String> {
    let fifo = set_fifo(tid);
    let Err(fifo_error) = fifo else {
        return Ok(Granted::Fifo(REALTIME_PRIORITY));
    };
    let rtkit = match rtkit::RealtimeKit::connect() {
        Ok(rtkit) => rtkit,
        Err(e) => {
            return nice_instead(tid, None, format!("SCHED_FIFO: {fifo_error}; rtkit: {e}"));
        }
    };
    let real_time_error = match rtkit.make_thread_realtime(tid, REALTIME_PRIORITY) {
        Ok(priority) if has_real_time_policy(tid) => return Ok(Granted::Rtkit(priority)),
        Ok(_) => "rtkit said yes but the thread is not real-time".to_string(),
        Err(e) => e,
    };
    nice_instead(
        tid,
        Some(&rtkit),
        format!("SCHED_FIFO: {fifo_error}; rtkit: {real_time_error}"),
    )
}

#[cfg(target_os = "linux")]
fn set_fifo(tid: libc::pid_t) -> Result<(), String> {
    // SAFETY: a zeroed sched_param is valid, and the call only reads it
    let result = unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = REALTIME_PRIORITY;
        libc::sched_setscheduler(tid, libc::SCHED_FIFO, &param)
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

/// Real-time did not work out: a lower nice value, directly and then through rtkit
#[cfg(target_os = "linux")]
fn nice_instead(
    tid: libc::pid_t,
    rtkit: Option<&rtkit::RealtimeKit>,
    why_not_real_time: String,
) -> Result<Granted, String> {
    const NICE: i32 = -20;
    // SAFETY: only sets the nice value of a thread
    let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, NICE) };
    if result == 0 {
        return Ok(Granted::Nice {
            nice: NICE,
            why_not_real_time,
        });
    }
    let nice_error = std::io::Error::last_os_error();
    match rtkit.map(|rtkit| rtkit.make_thread_high_priority(tid)) {
        Some(Ok(nice)) => Ok(Granted::Nice {
            nice,
            why_not_real_time,
        }),
        Some(Err(e)) => Err(format!(
            "{why_not_real_time}; nice: {nice_error}; rtkit nice: {e}"
        )),
        None => Err(format!("{why_not_real_time}; nice: {nice_error}")),
    }
}

#[cfg(target_os = "linux")]
mod rtkit;

/// The MMCSS task the audio threads join. Windows ships "Pro Audio" for latency-critical work:
/// scheduling category High, active in the foreground. "Audio" is category Medium and background
/// only.
#[cfg(target_os = "windows")]
const MMCSS_TASK: &str = "Pro Audio";

/// Asks Windows for MMCSS scheduling for the thread that calls, and says in the log what it got.
/// `role` names the thread in the log.
///
/// MMCSS (the Multimedia Class Scheduler Service) is how Windows lets an ordinary process put a
/// thread in the real-time range (16-31) without any privilege. If the service refuses, the thread
/// is set to `THREAD_PRIORITY_TIME_CRITICAL`, which is the top of the normal range, and the log
/// says so.
#[cfg(target_os = "windows")]
pub(crate) fn set_current_thread_realtime_priority(role: &str) {
    log_outcome(role, join_mmcss());
}

/// Puts the thread that calls into the MMCSS task the first time it is called: for the thread of
/// an audio callback, which the audio library started and which cannot ask for itself before it
/// runs. MMCSS registers only the thread that calls, so this cannot be done from another thread.
///
/// ASIO is not covered: its callback runs on a thread the driver owns and schedules, and this
/// function is not called from there.
#[cfg(target_os = "windows")]
pub(crate) fn promote_callback_thread(role: &'static str) {
    thread_local! {
        static ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if ASKED.with(|asked| asked.replace(true)) {
        return;
    }
    log_outcome(role, join_mmcss());
}

/// What a thread was given
#[cfg(target_os = "windows")]
enum Granted {
    /// In the MMCSS task. `raised` says whether the priority inside the task was raised too
    Mmcss {
        task_index: u32,
        raised: Result<(), String>,
    },
    /// MMCSS said no; the top of the normal range instead
    TimeCritical { why_not_mmcss: String },
}

#[cfg(target_os = "windows")]
fn join_mmcss() -> Result<Granted, String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority, GetCurrentProcess, GetCurrentThread,
        SetPriorityClass, SetThreadPriority, AVRT_PRIORITY_CRITICAL, HIGH_PRIORITY_CLASS,
        THREAD_PRIORITY_TIME_CRITICAL,
    };

    // SAFETY: each call only acts on the calling thread or process
    unsafe {
        // The other threads of the process (network, timers) are not in the MMCSS task; the
        // class keeps them from falling behind the audio threads
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);

        let mut task_index = 0u32;
        match AvSetMmThreadCharacteristicsW(&HSTRING::from(MMCSS_TASK), &mut task_index) {
            Ok(task) => Ok(Granted::Mmcss {
                task_index,
                raised: AvSetMmThreadPriority(task, AVRT_PRIORITY_CRITICAL)
                    .map_err(|e| e.to_string()),
            }),
            Err(mmcss_error) => {
                let why_not_mmcss = mmcss_error.to_string();
                match SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) {
                    Ok(()) => Ok(Granted::TimeCritical { why_not_mmcss }),
                    Err(e) => Err(format!("MMCSS: {why_not_mmcss}; SetThreadPriority: {e}")),
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn log_outcome(role: &str, outcome: Result<Granted, String>) {
    match outcome {
        Ok(Granted::Mmcss {
            task_index,
            raised: Ok(()),
        }) => tracing::info!(
            "Audio thread ({}): real-time priority enabled through MMCSS (task \"{}\", index {})",
            role,
            MMCSS_TASK,
            task_index
        ),
        Ok(Granted::Mmcss {
            task_index,
            raised: Err(why),
        }) => tracing::warn!(
            "Audio thread ({}): in the MMCSS task \"{}\" (index {}), but its priority inside the task was not raised ({})",
            role,
            MMCSS_TASK,
            task_index,
            why
        ),
        Ok(Granted::TimeCritical { why_not_mmcss }) => tracing::warn!(
            "Audio thread ({}): no real-time priority, using the top of the normal range (TIME_CRITICAL) ({})",
            role,
            why_not_mmcss
        ),
        Err(why) => tracing::warn!(
            "Audio thread ({}): no real-time priority, runs at normal priority ({})",
            role,
            why
        ),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub(crate) fn set_current_thread_realtime_priority(_role: &str) {
    tracing::info!("Audio thread: real-time priority not supported on this platform");
}
