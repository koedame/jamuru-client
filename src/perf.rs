//! What the audio path costs the machine, measured while it runs.
//!
//! The audio callbacks and the receive loop record how long each pass took, and
//! the streams count the xruns the audio host reports. Timing is off until
//! [`enable`] is called (only the debug tools do), so an ordinary build pays one
//! relaxed load per pass. A reader takes a [`Window`] to see what happened over
//! a stretch of time, and a [`ProcessSampler`] for the process's CPU and memory
//! over the same stretch.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Passes of the input (capture) callback: encode and hand to the sender.
pub static INPUT_CALLBACK: Timer = Timer::new();
/// Passes of the output (playback) callback: pull, decode, resample, mix.
pub static OUTPUT_CALLBACK: Timer = Timer::new();
/// Passes of the streaming loop that carries commands and statistics.
pub static RECEIVE_LOOP: Timer = Timer::new();

static INPUT_XRUNS: AtomicU64 = AtomicU64::new(0);
static OUTPUT_XRUNS: AtomicU64 = AtomicU64::new(0);

/// Starts timing passes. Until this is called nothing is timed.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// The start of a pass to hand to [`Timer::stop`]; `None` while timing is off.
pub fn start() -> Option<Instant> {
    ENABLED.load(Ordering::Relaxed).then(Instant::now)
}

/// Counts an xrun the audio host reported on the input stream.
pub fn count_input_xrun() {
    INPUT_XRUNS.fetch_add(1, Ordering::Relaxed);
}

/// Counts an xrun the audio host reported on the output stream.
pub fn count_output_xrun() {
    OUTPUT_XRUNS.fetch_add(1, Ordering::Relaxed);
}

/// How long passes of one kind took: how many, their total and the longest.
pub struct Timer {
    count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl Timer {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
        }
    }

    /// Records the pass that began at `started` (from [`start`]).
    pub fn stop(&self, started: Option<Instant>) {
        if let Some(started) = started {
            self.record(started.elapsed());
        }
    }

    fn record(&self, took: Duration) {
        let ns = took.as_nanos().min(u64::MAX as u128) as u64;
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(ns, Ordering::Relaxed);
        self.max_ns.fetch_max(ns, Ordering::Relaxed);
    }
}

/// What one kind of pass did over a [`Window`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PassStats {
    pub count: u64,
    pub avg_us: f64,
    pub max_us: f64,
}

/// Everything counted over a stretch of time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Report {
    pub input_callback: PassStats,
    pub output_callback: PassStats,
    pub receive_loop: PassStats,
    pub input_xruns: u64,
    pub output_xruns: u64,
}

/// A stretch of time that ends with [`Window::finish`].
pub struct Window {
    input: (u64, u64),
    output: (u64, u64),
    receive: (u64, u64),
    input_xruns: u64,
    output_xruns: u64,
}

impl Window {
    /// Starts a window. The longest pass is forgotten, so `max_us` is the
    /// longest within this window.
    pub fn start() -> Self {
        let mark = |timer: &Timer| {
            timer.max_ns.store(0, Ordering::Relaxed);
            (
                timer.count.load(Ordering::Relaxed),
                timer.total_ns.load(Ordering::Relaxed),
            )
        };
        Self {
            input: mark(&INPUT_CALLBACK),
            output: mark(&OUTPUT_CALLBACK),
            receive: mark(&RECEIVE_LOOP),
            input_xruns: INPUT_XRUNS.load(Ordering::Relaxed),
            output_xruns: OUTPUT_XRUNS.load(Ordering::Relaxed),
        }
    }

    /// What happened since [`Window::start`]. Two windows open at once share
    /// the longest pass: the later start clears it for the earlier one too.
    pub fn finish(&self) -> Report {
        let stats = |timer: &Timer, from: (u64, u64)| {
            let count = timer.count.load(Ordering::Relaxed).saturating_sub(from.0);
            let total = timer
                .total_ns
                .load(Ordering::Relaxed)
                .saturating_sub(from.1);
            PassStats {
                count,
                avg_us: if count == 0 {
                    0.0
                } else {
                    total as f64 / count as f64 / 1000.0
                },
                max_us: timer.max_ns.load(Ordering::Relaxed) as f64 / 1000.0,
            }
        };
        Report {
            input_callback: stats(&INPUT_CALLBACK, self.input),
            output_callback: stats(&OUTPUT_CALLBACK, self.output),
            receive_loop: stats(&RECEIVE_LOOP, self.receive),
            input_xruns: INPUT_XRUNS
                .load(Ordering::Relaxed)
                .saturating_sub(self.input_xruns),
            output_xruns: OUTPUT_XRUNS
                .load(Ordering::Relaxed)
                .saturating_sub(self.output_xruns),
        }
    }
}

/// This process's CPU and memory over a stretch of time, and those of the
/// processes it started (on Linux the webview that draws the window is one).
///
/// The audio path runs in this process; the window is drawn by the children.
/// Both are counted because a machine has to carry both.
pub struct ProcessSampler {
    system: sysinfo::System,
    pid: sysinfo::Pid,
}

/// One process the app started, by what it used.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChildUsage {
    pub name: String,
    pub cpu_percent: f32,
    pub memory_mb: f64,
}

/// What [`ProcessSampler::finish`] read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessUsage {
    /// Of one core, so a process using two full cores reads 200.
    pub cpu_percent: f32,
    pub memory_mb: f64,
    /// Logical cores of the machine, to turn `cpu_percent` into a share of it.
    pub cpu_cores: usize,
    /// Everything the app started, all levels down, the busiest first (at most
    /// [`MAX_CHILDREN`]).
    pub children: Vec<ChildUsage>,
    /// Their sum, not limited to the ones listed.
    pub children_cpu_percent: f32,
    pub children_memory_mb: f64,
}

/// How many child processes [`ProcessUsage::children`] names.
const MAX_CHILDREN: usize = 6;

impl ProcessSampler {
    /// Starts sampling. Wait at least [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`]
    /// before [`ProcessSampler::finish`], or the CPU reads as 0.
    pub fn start() -> Result<Self, String> {
        let pid = sysinfo::get_current_pid().map_err(|e| e.to_string())?;
        let mut system = sysinfo::System::new();
        refresh(&mut system);
        Ok(Self { system, pid })
    }

    pub fn finish(mut self) -> Result<ProcessUsage, String> {
        refresh(&mut self.system);
        let process = self
            .system
            .process(self.pid)
            .ok_or_else(|| "this process is not in the process list".to_string())?;
        let mut descendants = vec![self.pid];
        loop {
            let before = descendants.len();
            for (pid, candidate) in self.system.processes() {
                // Linux lists a process's threads as processes of their own, with the
                // process as parent; they are already in the process's own reading
                if candidate.thread_kind().is_none()
                    && !descendants.contains(pid)
                    && candidate.parent().is_some_and(|p| descendants.contains(&p))
                {
                    descendants.push(*pid);
                }
            }
            if descendants.len() == before {
                break;
            }
        }
        let mut children: Vec<ChildUsage> = descendants
            .iter()
            .filter(|pid| **pid != self.pid)
            .filter_map(|pid| self.system.process(*pid))
            .map(|child| ChildUsage {
                name: child.name().to_string_lossy().into_owned(),
                cpu_percent: child.cpu_usage(),
                memory_mb: child.memory() as f64 / (1024.0 * 1024.0),
            })
            .collect();
        let children_cpu_percent = children.iter().map(|c| c.cpu_percent).sum();
        let children_memory_mb = children.iter().map(|c| c.memory_mb).sum();
        children.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        children.truncate(MAX_CHILDREN);
        Ok(ProcessUsage {
            cpu_percent: process.cpu_usage(),
            memory_mb: process.memory() as f64 / (1024.0 * 1024.0),
            cpu_cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
            children,
            children_cpu_percent,
            children_memory_mb,
        })
    }
}

/// The processor's brand string and its logical core count, for a debug
/// report on what a measurement ran on. `None` where the OS does not say.
pub fn cpu_description() -> (Option<String>, usize) {
    let mut system = sysinfo::System::new();
    system.refresh_cpu_specifics(sysinfo::CpuRefreshKind::nothing());
    let brand = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_string())
        .filter(|brand| !brand.is_empty());
    (brand, system.cpus().len())
}

fn refresh(system: &mut sysinfo::System) {
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_stopped_while_timing_is_off_records_nothing() {
        let timer = Timer::new();

        timer.stop(None);

        assert_eq!(timer.count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn passes_of_two_lengths_report_their_average_and_longest() {
        let timer = Timer::new();
        timer.record(Duration::from_micros(100));
        timer.record(Duration::from_micros(300));

        let count = timer.count.load(Ordering::Relaxed);
        let total = timer.total_ns.load(Ordering::Relaxed);

        assert_eq!(count, 2);
        assert_eq!(total as f64 / count as f64 / 1000.0, 200.0);
        assert_eq!(timer.max_ns.load(Ordering::Relaxed), 300_000);
    }

    #[test]
    fn a_window_reports_only_what_happened_after_it_started() {
        // The statics are shared with every other test in this process, so
        // this one uses counts no other test touches: RECEIVE_LOOP.
        RECEIVE_LOOP.record(Duration::from_micros(900));
        let window = Window::start();
        RECEIVE_LOOP.record(Duration::from_micros(40));
        RECEIVE_LOOP.record(Duration::from_micros(60));

        let report = window.finish();

        assert_eq!(report.receive_loop.count, 2);
        assert_eq!(report.receive_loop.avg_us, 50.0);
        assert_eq!(
            report.receive_loop.max_us, 60.0,
            "the 900us pass came before the window"
        );
    }

    #[test]
    fn xruns_are_counted_from_the_start_of_the_window() {
        count_input_xrun();
        let window = Window::start();
        count_input_xrun();
        count_output_xrun();
        count_output_xrun();

        let report = window.finish();

        assert_eq!(report.input_xruns, 1);
        assert_eq!(report.output_xruns, 2);
    }

    #[test]
    fn the_cpu_description_counts_at_least_one_core() {
        let (_, cores) = cpu_description();

        assert!(cores >= 1);
    }

    #[test]
    fn the_threads_of_this_process_are_not_counted_as_children() {
        let (stop, wait) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = wait.recv();
        });
        let sampler = ProcessSampler::start().unwrap();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);

        let usage = sampler.finish().unwrap();
        stop.send(()).unwrap();
        worker.join().unwrap();

        assert!(usage.children.is_empty(), "{:?}", usage);
    }

    #[cfg(unix)]
    #[test]
    fn a_child_process_is_counted_with_what_it_used() {
        let mut child = std::process::Command::new("sleep")
            .arg("3")
            .spawn()
            .unwrap();
        let sampler = ProcessSampler::start().unwrap();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);

        let usage = sampler.finish().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();

        assert!(
            usage.children.iter().any(|c| c.name == "sleep"),
            "{:?}",
            usage
        );
        assert!(usage.children_memory_mb > 0.0, "{:?}", usage);
    }

    #[test]
    fn the_process_sampler_reads_this_process_memory() {
        let sampler = ProcessSampler::start().unwrap();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);

        let usage = sampler.finish().unwrap();

        assert!(usage.memory_mb > 1.0, "{:?}", usage);
        assert!(usage.cpu_cores >= 1);
    }
}
