//! Measures how long the short waits of the audio path really take: the send thread's
//! `thread::sleep(100 µs)` and the streaming loop's `tokio::time::sleep(250 µs)`, with the
//! system timer at its default and at one millisecond (Windows, `timeBeginPeriod`). Run by hand:
//!
//! ```text
//! cargo test --release --test timer_wait_measure -- --ignored --nocapture
//! ```

#[cfg(windows)]
mod windows_only {
    use std::time::{Duration, Instant};
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};

    const PASSES: usize = 4000;

    fn percentile(sorted: &[f64], p: f64) -> f64 {
        sorted[((sorted.len() - 1) as f64 * p) as usize]
    }

    fn row(name: &str, mut took_us: Vec<f64>) {
        took_us.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = took_us.iter().sum::<f64>() / took_us.len() as f64;
        println!(
            "  {name:<28} mean {mean:>8.0} us  p50 {:>8.0}  p99 {:>8.0}  max {:>8.0}",
            percentile(&took_us, 0.5),
            percentile(&took_us, 0.99),
            took_us[took_us.len() - 1]
        );
    }

    fn waits() {
        let thread: Vec<f64> = (0..PASSES)
            .map(|_| {
                let started = Instant::now();
                std::thread::sleep(Duration::from_micros(100));
                started.elapsed().as_secs_f64() * 1e6
            })
            .collect();
        row("thread::sleep(100 us)", thread);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let tokio_waits: Vec<f64> = runtime.block_on(async {
            let mut took = Vec::new();
            for _ in 0..PASSES {
                let started = Instant::now();
                tokio::time::sleep(Duration::from_micros(250)).await;
                took.push(started.elapsed().as_secs_f64() * 1e6);
            }
            took
        });
        row("tokio::time::sleep(250 us)", tokio_waits);
    }

    #[test]
    #[ignore = "a measurement to run by hand"]
    fn measure_short_waits_with_and_without_the_one_millisecond_timer() {
        println!("system timer as the system has it:");
        waits();
        // SAFETY: balanced by timeEndPeriod
        unsafe { timeBeginPeriod(1) };
        println!("timeBeginPeriod(1):");
        waits();
        // SAFETY: matches the call above
        unsafe { timeEndPeriod(1) };
    }
}
