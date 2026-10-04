//! CPU performance diagnostics for jamuru
//!
//! Provides CPU performance checks including:
//! - Processing capability benchmark
//! - Real-time factor calculation
//! - System load monitoring

use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{DiagnosticGrade, DiagnosticProblem, ProblemCode, ProblemSeverity};

/// CPU performance benchmark result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuBenchmarkResult {
    /// Time to process one frame of audio (in microseconds)
    pub processing_time_us: f64,
    /// Frame duration at 48kHz with given buffer size (in microseconds)
    pub frame_duration_us: f64,
    /// Real-time factor (< 1.0 means can process faster than real-time)
    pub realtime_factor: f64,
    /// Buffer size used for benchmark
    pub buffer_size: u32,
}

/// System resource information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemResources {
    /// Number of CPU cores
    pub cpu_cores: usize,
    /// Current CPU usage estimate (0.0 - 1.0), if available
    pub cpu_usage: Option<f64>,
    /// Available memory in MB, if available
    pub available_memory_mb: Option<u64>,
}

/// Complete CPU diagnostics result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuDiagnosticsResult {
    /// Benchmark results for different buffer sizes
    pub benchmarks: Vec<CpuBenchmarkResult>,
    /// System resource information
    pub system: SystemResources,
    /// Overall CPU grade
    pub grade: DiagnosticGrade,
    /// Whether real-time processing is achievable
    pub realtime_capable: bool,
    /// Detected problems
    pub problems: Vec<DiagnosticProblem>,
}

/// CPU diagnostics runner
pub struct CpuDiagnostics;

impl CpuDiagnostics {
    /// Run all CPU diagnostics
    pub fn run() -> CpuDiagnosticsResult {
        let mut problems = Vec::new();

        // Get system information
        let system = Self::get_system_resources();

        // Run benchmarks for different buffer sizes
        let buffer_sizes = [32, 64, 128, 256];
        let benchmarks: Vec<CpuBenchmarkResult> = buffer_sizes
            .iter()
            .map(|&size| Self::run_benchmark(size))
            .collect();

        // Check if real-time processing is achievable
        let realtime_capable = benchmarks.iter().any(|b| b.realtime_factor < 0.5); // Should have 50% headroom

        // Calculate grade based on benchmark results
        let grade = Self::calculate_grade(&benchmarks, &system);

        // Add problems for performance issues
        if !realtime_capable {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Warning,
                category: "cpu".to_string(),
                code: ProblemCode::InsufficientRealtimeHeadroom,
            });
        }

        // Check for high CPU usage
        if let Some(usage) = system.cpu_usage {
            if usage > 0.8 {
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Warning,
                    category: "cpu".to_string(),
                    code: ProblemCode::HighCpuUsage {
                        usage_percent: usage * 100.0,
                    },
                });
            }
        }

        // Check for low memory
        if let Some(mem_mb) = system.available_memory_mb {
            if mem_mb < 512 {
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Warning,
                    category: "cpu".to_string(),
                    code: ProblemCode::LowMemory {
                        available_mb: mem_mb,
                    },
                });
            }
        }

        // Check benchmark results for smallest buffer
        if let Some(smallest) = benchmarks.first() {
            if smallest.realtime_factor > 0.8 {
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Warning,
                    category: "cpu".to_string(),
                    code: ProblemCode::SmallBufferGlitchRisk,
                });
            }
        }

        CpuDiagnosticsResult {
            benchmarks,
            system,
            grade,
            realtime_capable,
            problems,
        }
    }

    /// Get system resource information
    fn get_system_resources() -> SystemResources {
        let cpu_cores = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1);

        // Note: Getting accurate CPU usage and memory requires platform-specific APIs
        // or external crates like sysinfo. For now, we provide basic info.
        // TODO: Add sysinfo crate for better system monitoring

        SystemResources {
            cpu_cores,
            cpu_usage: None,           // Would need sysinfo crate
            available_memory_mb: None, // Would need sysinfo crate
        }
    }

    /// Run a processing benchmark for a given buffer size
    fn run_benchmark(buffer_size: u32) -> CpuBenchmarkResult {
        const SAMPLE_RATE: u32 = 48000;
        const ITERATIONS: u32 = 1000;

        // Frame duration in microseconds
        let frame_duration_us = (buffer_size as f64 / SAMPLE_RATE as f64) * 1_000_000.0;

        // Create a buffer to simulate audio processing
        let mut buffer: Vec<f32> = vec![0.0; buffer_size as usize];

        // Fill with test data
        for (i, sample) in buffer.iter_mut().enumerate() {
            *sample = (i as f32 / buffer_size as f32) * 2.0 - 1.0;
        }

        // Benchmark: simulate typical audio processing operations
        let start = Instant::now();

        for _ in 0..ITERATIONS {
            // Simulate typical audio processing:
            // 1. Apply gain
            for sample in buffer.iter_mut() {
                *sample *= 0.8;
            }

            // 2. Apply simple low-pass filter (single pole)
            let mut prev = 0.0f32;
            let alpha = 0.1f32;
            for sample in buffer.iter_mut() {
                *sample = prev + alpha * (*sample - prev);
                prev = *sample;
            }

            // 3. Apply limiting
            for sample in buffer.iter_mut() {
                *sample = sample.clamp(-0.99, 0.99);
            }

            // Prevent optimization from removing the loop
            std::hint::black_box(&buffer);
        }

        let elapsed = start.elapsed();
        let processing_time_us = elapsed.as_secs_f64() * 1_000_000.0 / ITERATIONS as f64;
        let realtime_factor = processing_time_us / frame_duration_us;

        debug!(
            "CPU benchmark: buffer_size={}, processing_time={:.2}us, frame_duration={:.2}us, rt_factor={:.4}",
            buffer_size, processing_time_us, frame_duration_us, realtime_factor
        );

        CpuBenchmarkResult {
            processing_time_us,
            frame_duration_us,
            buffer_size,
            realtime_factor,
        }
    }

    /// Calculate overall grade based on benchmarks
    fn calculate_grade(
        benchmarks: &[CpuBenchmarkResult],
        system: &SystemResources,
    ) -> DiagnosticGrade {
        // Check if we can handle smallest buffer with headroom
        let smallest_buffer_ok = benchmarks
            .first()
            .map(|b| b.realtime_factor < 0.3) // 70% headroom
            .unwrap_or(false);

        let medium_buffer_ok = benchmarks
            .iter()
            .find(|b| b.buffer_size == 64)
            .map(|b| b.realtime_factor < 0.5) // 50% headroom
            .unwrap_or(false);

        let large_buffer_ok = benchmarks
            .iter()
            .find(|b| b.buffer_size == 128)
            .map(|b| b.realtime_factor < 0.7) // 30% headroom
            .unwrap_or(false);

        // Bonus for multi-core systems
        let core_bonus = if system.cpu_cores >= 4 { 10 } else { 0 };

        let base_score = if smallest_buffer_ok {
            100
        } else if medium_buffer_ok {
            75
        } else if large_buffer_ok {
            50
        } else {
            25
        };

        let total_score = base_score + core_bonus;

        if total_score >= 90 {
            DiagnosticGrade::A
        } else if total_score >= 65 {
            DiagnosticGrade::B
        } else {
            DiagnosticGrade::C
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_runs() {
        let result = CpuDiagnostics::run_benchmark(128);
        assert!(result.processing_time_us > 0.0);
        assert!(result.frame_duration_us > 0.0);
        assert!(result.realtime_factor > 0.0);
        assert_eq!(result.buffer_size, 128);
    }

    #[test]
    fn test_system_resources() {
        let system = CpuDiagnostics::get_system_resources();
        assert!(system.cpu_cores >= 1);
    }

    #[test]
    fn test_full_diagnostics() {
        let result = CpuDiagnostics::run();
        assert!(!result.benchmarks.is_empty());
        // Should have benchmarks for 32, 64, 128, 256
        assert_eq!(result.benchmarks.len(), 4);
    }
}
