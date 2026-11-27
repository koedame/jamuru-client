//! Audio loopback tests
//!
//! These tests verify audio pipeline quality without network involvement.
//! Audio is pushed through [`SoftwarePipeline`], which is built from the
//! production codec, packet, jitter buffer and PLC types, so a regression in
//! any of them shows up here.

use crate::audio_injection::AudioInjector;
use crate::pipeline::SoftwarePipeline;
use crate::quality::{LatencyMeasurer, PesqEvaluator, QualityResult};
use crate::{TestConfig, TestResult, TestStatus};
use jamjam::audio::AudioPreset;
use std::time::Instant;
use tracing::{info, warn};

/// Channel count the loopback pipeline runs at
///
/// The injector and the pipeline must agree: interleaving stereo into a mono
/// pipeline would halve the apparent period of every test signal.
const PIPELINE_CHANNELS: u16 = 1;

/// Largest delay the latency measurement searches for, in milliseconds
///
/// Cross-correlation cost grows with the search range, and no preset is
/// specified to cost anywhere near this much.
const MAX_SEARCH_LAG_MS: f32 = 100.0;

/// Loopback test configuration
pub struct LoopbackTest {
    config: TestConfig,
    injector: AudioInjector,
}

impl LoopbackTest {
    /// Create a new loopback test
    pub fn new(config: TestConfig) -> Self {
        let injector = AudioInjector::new(config.sample_rate, PIPELINE_CHANNELS);
        Self { config, injector }
    }

    /// Resolve the preset named in the config, defaulting to balanced
    fn preset(&self) -> AudioPreset {
        AudioPreset::from_name(&self.config.preset).unwrap_or_default()
    }

    /// Send `reference` through the real pipeline and return what came out
    fn run_through_pipeline(&self, reference: &[f32]) -> (Vec<f32>, SoftwarePipeline) {
        let mut pipeline =
            SoftwarePipeline::new(self.preset(), self.config.sample_rate, PIPELINE_CHANNELS);
        let mut received = pipeline.process(reference, &[]);
        received.extend(pipeline.drain());
        (received, pipeline)
    }

    /// Send `reference` through the pipeline and drop the priming silence, so
    /// the result lines up sample-for-sample with the reference
    fn run_aligned(&self, reference: &[f32]) -> Vec<f32> {
        let (mut received, pipeline) = self.run_through_pipeline(reference);
        received.split_off(pipeline.output_delay_samples().min(received.len()))
    }

    /// Run a basic audio quality test with a sine wave
    ///
    /// Generates a reference tone, pushes it through the pipeline and scores
    /// the result against the preset's MOS threshold.
    pub fn run_sine_test(&self, frequency: f32) -> TestResult {
        let start = Instant::now();
        info!("Running sine wave loopback test at {} Hz", frequency);

        let reference = self
            .injector
            .generate_sine(frequency, self.config.duration_sec);
        let received = self.run_aligned(&reference);

        let evaluator = PesqEvaluator::new(self.config.sample_rate);
        let quality = evaluator
            .evaluate_with_threshold(&reference, &received, &self.config.preset)
            .unwrap_or_else(|e| {
                warn!("Quality evaluation failed: {}", e);
                QualityResult::failed(e.to_string())
            });

        TestResult {
            scenario: format!("loopback_sine_{}hz", frequency as u32),
            status: status_from(quality.meets_threshold),
            duration_ms: start.elapsed().as_millis() as u64,
            connection_time_ms: None,
            quality: Some(quality),
            error: None,
        }
    }

    /// Run a frequency sweep test to verify codec quality across the spectrum
    pub fn run_sweep_test(&self, start_freq: f32, end_freq: f32) -> TestResult {
        let start = Instant::now();
        info!(
            "Running frequency sweep test {} Hz - {} Hz",
            start_freq, end_freq
        );

        let reference =
            self.injector
                .generate_sweep(start_freq, end_freq, self.config.duration_sec);
        let received = self.run_aligned(&reference);

        let evaluator = PesqEvaluator::new(self.config.sample_rate);
        let quality = evaluator
            .evaluate_with_threshold(&reference, &received, &self.config.preset)
            .unwrap_or_else(|e| QualityResult::failed(e.to_string()));

        TestResult {
            scenario: format!("loopback_sweep_{}_{}", start_freq as u32, end_freq as u32),
            status: status_from(quality.meets_threshold),
            duration_ms: start.elapsed().as_millis() as u64,
            connection_time_ms: None,
            quality: Some(quality),
            error: None,
        }
    }

    /// Measure the latency the pipeline actually introduces
    ///
    /// The delay is recovered by cross-correlating the played signal against
    /// the reference. The device capture and playback buffers cannot exist
    /// without hardware, so they are added analytically before the
    /// result is compared with the preset budget.
    pub fn run_latency_test(&self) -> TestResult {
        let start = Instant::now();
        info!("Running latency measurement test");

        let preset = self.preset();
        // A sweep, not a tone: a steady tone correlates at every multiple of its
        // period, which makes the measured lag ambiguous.
        let reference = self
            .injector
            .generate_sweep(200.0, 8000.0, self.config.duration_sec);
        let (received, pipeline) = self.run_through_pipeline(&reference);

        let measurer = LatencyMeasurer::new(self.config.sample_rate);
        let max_lag_samples =
            (self.config.sample_rate as f32 * MAX_SEARCH_LAG_MS / 1000.0) as usize;
        let measured_pipeline_ms =
            match measurer.measure_within(&reference, &received, max_lag_samples) {
                Ok(ms) => ms,
                Err(e) => {
                    return TestResult::failed(
                        "loopback_latency",
                        format!("could not measure latency: {}", e),
                    )
                }
            };

        let total_app_ms = pipeline.total_app_latency_ms(measured_pipeline_ms);
        let budget_ms = preset.max_app_latency_ms();
        let meets_threshold = total_app_ms <= budget_ms;

        let quality = QualityResult {
            pesq_mos: None,
            latency_ms: Some(total_app_ms),
            packet_loss_percent: Some(0.0),
            meets_threshold,
            notes: Some(format!(
                "pipeline {:.2}ms measured + device buffers {:.2}ms = {:.2}ms (budget {:.2}ms)",
                measured_pipeline_ms,
                pipeline.device_buffer_latency_ms(),
                total_app_ms,
                budget_ms
            )),
        };

        TestResult {
            scenario: "loopback_latency".to_string(),
            status: status_from(meets_threshold),
            duration_ms: start.elapsed().as_millis() as u64,
            connection_time_ms: None,
            quality: Some(quality),
            error: if meets_threshold {
                None
            } else {
                Some(format!(
                    "{} latency {:.2}ms exceeds its {:.2}ms budget",
                    preset.name(),
                    total_app_ms,
                    budget_ms
                ))
            },
        }
    }

    /// Run silence test to verify no noise is introduced
    pub fn run_silence_test(&self) -> TestResult {
        let start = Instant::now();
        info!("Running silence test");

        let reference = self.injector.generate_silence(self.config.duration_sec);
        let received = self.run_aligned(&reference);

        // Check that silence is preserved (RMS should be near zero)
        let rms: f32 = if received.is_empty() {
            0.0
        } else {
            (received.iter().map(|s| s * s).sum::<f32>() / received.len() as f32).sqrt()
        };
        let noise_floor_db = if rms > 0.0 {
            20.0 * rms.log10()
        } else {
            -120.0
        };

        let passed = noise_floor_db < -60.0; // Should be below -60 dB

        let mut quality = QualityResult::passed();
        quality.notes = Some(format!("Noise floor: {:.1} dB", noise_floor_db));
        quality.meets_threshold = passed;

        TestResult {
            scenario: "loopback_silence".to_string(),
            status: status_from(passed),
            duration_ms: start.elapsed().as_millis() as u64,
            connection_time_ms: None,
            quality: Some(quality),
            error: if passed {
                None
            } else {
                Some(format!(
                    "Noise floor too high: {:.1} dB (max -60 dB)",
                    noise_floor_db
                ))
            },
        }
    }

    /// Verify that a dropped packet is concealed rather than dropped outright
    pub fn run_packet_loss_test(&self, lost_frames: &[usize]) -> TestResult {
        let start = Instant::now();
        info!("Running packet loss test, dropping {:?}", lost_frames);

        let preset = self.preset();
        let reference = self.injector.generate_sine(440.0, self.config.duration_sec);

        let mut pipeline =
            SoftwarePipeline::new(preset, self.config.sample_rate, PIPELINE_CHANNELS);
        let mut received = pipeline.process(&reference, lost_frames);
        received.extend(pipeline.drain());

        let concealed = pipeline.concealed_frames() as usize;
        let passed = concealed == lost_frames.len() && !received.is_empty();

        let mut quality = QualityResult::passed();
        quality.packet_loss_percent = Some(if reference.is_empty() {
            0.0
        } else {
            lost_frames.len() as f32
                / (reference.len() as f32 / pipeline.preset().frame_size() as f32)
                * 100.0
        });
        quality.meets_threshold = passed;
        quality.notes = Some(format!(
            "{} dropped frame(s), {} concealed",
            lost_frames.len(),
            concealed
        ));

        TestResult {
            scenario: "loopback_packet_loss".to_string(),
            status: status_from(passed),
            duration_ms: start.elapsed().as_millis() as u64,
            connection_time_ms: None,
            quality: Some(quality),
            error: if passed {
                None
            } else {
                Some(format!(
                    "expected {} concealed frame(s), got {}",
                    lost_frames.len(),
                    concealed
                ))
            },
        }
    }
}

fn status_from(passed: bool) -> TestStatus {
    if passed {
        TestStatus::Passed
    } else {
        TestStatus::Failed
    }
}

/// Run all loopback tests for a given preset
pub fn run_all_loopback_tests(preset: &str) -> Vec<TestResult> {
    let config = TestConfig {
        sample_rate: 48000,
        frame_size: 128,
        duration_sec: 1.0,
        preset: preset.to_string(),
    };

    let test = LoopbackTest::new(config);

    vec![
        test.run_sine_test(440.0),
        test.run_sine_test(1000.0),
        test.run_sweep_test(100.0, 10000.0),
        test.run_latency_test(),
        test.run_silence_test(),
        test.run_packet_loss_test(&[3, 11]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_for(preset: &str) -> TestConfig {
        TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 0.2,
            preset: preset.to_string(),
        }
    }

    #[test]
    fn test_sine_loopback() {
        let test = LoopbackTest::new(config_for("balanced"));
        let result = test.run_sine_test(440.0);

        assert!(
            result.is_passed(),
            "Sine loopback test should pass: {:?}",
            result
        );
        let mos = result.quality.and_then(|q| q.pesq_mos).expect("MOS score");
        assert!(mos >= 3.5, "PCM loopback should score well, got {:.2}", mos);
    }

    #[test]
    fn test_sweep_loopback() {
        let test = LoopbackTest::new(config_for("balanced"));
        let result = test.run_sweep_test(100.0, 10000.0);

        assert!(
            result.is_passed(),
            "Sweep loopback test should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_silence_loopback() {
        let test = LoopbackTest::new(config_for("balanced"));
        let result = test.run_silence_test();

        assert!(result.is_passed(), "Silence loopback test should pass");
    }

    /// The measured pipeline delay plus the derived device buffers must fit
    /// inside every preset's budget, and must reproduce the value the latency
    /// budget derives from the preset parameters.
    #[test]
    fn test_all_presets_meet_their_latency_budget() {
        for preset in AudioPreset::all() {
            let test = LoopbackTest::new(config_for(preset.name()));
            let result = test.run_latency_test();

            assert!(
                result.is_passed(),
                "preset {} exceeded its latency budget: {:?}",
                preset.name(),
                result.error
            );

            let measured = result
                .quality
                .and_then(|q| q.latency_ms)
                .expect("latency measurement");
            assert!(
                measured <= preset.max_app_latency_ms(),
                "preset {} measured {:.2}ms against a {:.2}ms budget",
                preset.name(),
                measured,
                preset.max_app_latency_ms()
            );

            // With the jitter buffer on the receive path, the measurement should
            // not merely fit the budget, it should reproduce the value the budget
            // derives from the preset parameters. A mismatch means the model and
            // the code disagree.
            let designed = preset.designed_app_latency_ms(48_000);
            assert!(
                (measured - designed).abs() < 0.1,
                "preset {} measured {:.2}ms but its budget derives {:.2}ms",
                preset.name(),
                measured,
                designed
            );
        }
    }

    #[test]
    fn test_all_presets() {
        for preset in ["zero-latency", "balanced", "high-quality"] {
            let test = LoopbackTest::new(config_for(preset));
            let result = test.run_sine_test(440.0);

            assert!(
                result.is_passed(),
                "Preset {} should pass sine test",
                preset
            );
        }
    }

    #[test]
    fn test_packet_loss_is_concealed() {
        let test = LoopbackTest::new(config_for("ultra-low-latency"));
        let result = test.run_packet_loss_test(&[2, 7]);

        assert!(
            result.is_passed(),
            "dropped frames should be concealed: {:?}",
            result.error
        );
    }
}
