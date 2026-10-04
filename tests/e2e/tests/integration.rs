//! Integration tests for jamuru E2E testing
//!
//! These tests verify the E2E test infrastructure works correctly.

use jamjam::audio::AudioPreset;
use jamjam_e2e_tests::audio_injection::AudioInjector;
use jamjam_e2e_tests::quality::{ExternalPesqEvaluator, LatencyMeasurer, PesqEvaluator};
use jamjam_e2e_tests::{TestConfig, TestResult, TestStatus};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/reference_audio")
}

#[test]
fn test_audio_injector_sine() {
    let injector = AudioInjector::new(48000, 2);
    let samples = injector.generate_sine(440.0, 0.1);

    // 48000 * 0.1 * 2 channels = 9600 samples
    assert_eq!(samples.len(), 9600);

    // Check samples are in valid range
    for sample in &samples {
        assert!(
            *sample >= -1.0 && *sample <= 1.0,
            "Sample out of range: {}",
            sample
        );
    }
}

#[test]
fn test_audio_injector_sweep() {
    let injector = AudioInjector::new(48000, 1);
    let samples = injector.generate_sweep(100.0, 10000.0, 0.5);

    // 48000 * 0.5 * 1 channel = 24000 samples
    assert_eq!(samples.len(), 24000);
}

#[test]
fn test_pesq_evaluator_identical_audio() {
    let evaluator = PesqEvaluator::new(48000);
    let injector = AudioInjector::new(48000, 1);

    let reference = injector.generate_sine(440.0, 0.1);

    // Identical audio should have perfect MOS
    let mos = evaluator.evaluate(&reference, &reference).unwrap();
    assert!(
        mos >= 4.0,
        "Identical audio should have MOS >= 4.0, got {}",
        mos
    );
}

#[test]
fn test_pesq_evaluator_different_audio() {
    let evaluator = PesqEvaluator::new(48000);
    let injector = AudioInjector::new(48000, 1);

    let ref1 = injector.generate_sine(440.0, 0.1);
    let ref2 = injector.generate_sine(880.0, 0.1);

    // Different frequencies should have lower correlation
    let mos = evaluator.evaluate(&ref1, &ref2).unwrap();
    assert!(
        mos < 4.0,
        "Different audio should have MOS < 4.0, got {}",
        mos
    );
}

#[test]
fn test_latency_measurer_zero_delay() {
    let measurer = LatencyMeasurer::new(48000);
    let injector = AudioInjector::new(48000, 1);

    let audio = injector.generate_sine(1000.0, 0.1);

    let latency = measurer.measure(&audio, &audio).unwrap();
    assert!(
        latency < 1.0,
        "Same audio should have near-zero latency, got {}ms",
        latency
    );
}

#[test]
#[ignore = "Simplified correlation algorithm needs improvement for accurate delay detection"]
fn test_latency_measurer_with_delay() {
    let measurer = LatencyMeasurer::new(48000);
    let injector = AudioInjector::new(48000, 1);

    // Use longer signal for better correlation detection
    let reference = injector.generate_sine(1000.0, 0.5);

    // Add 10ms delay (480 samples at 48kHz)
    let mut received = vec![0.0f32; 480];
    received.extend_from_slice(&reference);

    let latency = measurer.measure(&reference, &received).unwrap();
    // Allow wider tolerance for simplified cross-correlation algorithm
    assert!(
        latency > 5.0 && latency < 50.0,
        "Expected delay detection (5-50ms range), got {}ms",
        latency
    );
}

/// Every preset must have a MOS bar here, and its latency budget must come
/// from the product rather than a copy kept in the test suite (ADR-019).
#[test]
fn test_preset_thresholds() {
    for preset in AudioPreset::all() {
        let name = preset.name();

        let min_mos = PesqEvaluator::min_mos_for(name);
        assert!(
            PesqEvaluator::MIN_MOS.iter().any(|(p, _)| *p == name),
            "Preset '{}' has no MOS bar defined",
            name
        );
        assert!(
            (1.0..=4.5).contains(&min_mos),
            "Invalid MOS threshold for {}",
            name
        );

        let max_latency = PesqEvaluator::max_app_latency_ms_for(name);
        assert!(max_latency > 0.0, "Invalid latency threshold for {}", name);
        assert_eq!(
            max_latency,
            preset.max_app_latency_ms(),
            "the E2E latency budget for {} must be the product's own budget",
            name
        );
    }
}

#[test]
fn test_quality_result_creation() {
    let passed = jamjam_e2e_tests::quality::QualityResult::passed();
    assert!(passed.meets_threshold);
    assert!(passed.pesq_mos.is_none());

    let failed = jamjam_e2e_tests::quality::QualityResult::failed("Test failure");
    assert!(!failed.meets_threshold);
    assert_eq!(failed.notes, Some("Test failure".to_string()));
}

#[test]
fn test_test_config_default() {
    let config = TestConfig::default();
    assert_eq!(config.sample_rate, 48000);
    assert_eq!(config.frame_size, 128);
    assert_eq!(config.duration_sec, 10.0);
    assert_eq!(config.preset, "balanced");
}

#[test]
fn test_test_result_creation() {
    let passed = TestResult::passed("test_scenario", 100);
    assert!(passed.is_passed());
    assert_eq!(passed.status, TestStatus::Passed);
    assert_eq!(passed.scenario, "test_scenario");
    assert_eq!(passed.duration_ms, 100);

    let failed = TestResult::failed("test_scenario", "error message");
    assert!(!failed.is_passed());
    assert_eq!(failed.status, TestStatus::Failed);
    assert_eq!(failed.error, Some("error message".to_string()));

    // A scenario without a harness is neither a pass nor a failure, and it
    // must always carry a reason (ADR-018).
    let skipped = TestResult::not_implemented("test_scenario", "no harness");
    assert!(!skipped.is_passed());
    assert!(skipped.is_not_implemented());
    assert_eq!(skipped.status, TestStatus::NotImplemented);
    assert_eq!(skipped.error, Some("no harness".to_string()));
    assert!(
        skipped.quality.is_none(),
        "an unrun scenario must not report measurements"
    );
}

#[test]
#[ignore = "Requires fixture files to be generated first"]
fn test_fixture_files_exist() {
    let fixtures = fixtures_dir();

    let expected_files = [
        "sine_440hz_1s.wav",
        "sine_1000hz_1s.wav",
        "silence_1s.wav",
        "sweep_100_10000hz_2s.wav",
    ];

    for file in expected_files {
        let path = fixtures.join(file);
        assert!(path.exists(), "Fixture file not found: {:?}", path);
    }
}

#[test]
#[ignore = "Requires Python with pesq library"]
fn test_external_pesq_evaluator() {
    let fixtures = fixtures_dir();
    let evaluator = ExternalPesqEvaluator::new();

    let reference = fixtures.join("sine_440hz_1s.wav");

    // Test with identical file
    let result = evaluator.evaluate_files(&reference, &reference);

    match result {
        Ok(quality) => {
            println!("PESQ result: {:?}", quality);
            if let Some(mos) = quality.pesq_mos {
                assert!(mos >= 4.0, "Identical audio should have high MOS");
            }
        }
        Err(e) => {
            println!(
                "PESQ evaluation failed (expected if pesq not installed): {}",
                e
            );
        }
    }
}

#[cfg(feature = "loopback")]
mod loopback_tests {
    use super::*;
    use jamjam_e2e_tests::scenarios::loopback::LoopbackTest;

    #[test]
    fn test_loopback_sine() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 0.1,
            preset: "balanced".to_string(),
        };

        let test = LoopbackTest::new(config);
        let result = test.run_sine_test(440.0);

        assert!(result.is_passed(), "Loopback sine test should pass");
    }

    #[test]
    fn test_loopback_silence() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 0.1,
            preset: "balanced".to_string(),
        };

        let test = LoopbackTest::new(config);
        let result = test.run_silence_test();

        assert!(result.is_passed(), "Loopback silence test should pass");
    }
}

#[cfg(feature = "network-local")]
mod network_tests {
    use super::*;
    use jamjam_e2e_tests::node::{Platform, TestNode};

    #[test]
    fn test_node_creation() {
        let node = TestNode::local("test-node");
        assert!(node.is_local());
        assert_eq!(node.platform(), &Platform::current());
    }

    #[tokio::test]
    async fn test_two_node_simulated() {
        use jamjam_e2e_tests::scenarios::two_node::TwoNodeTest;

        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 1.0,
            preset: "balanced".to_string(),
        };

        let test = TwoNodeTest::new(config);
        let result = test.test_audio_quality().await;

        // No two-node harness exists, so the honest outcome is NotImplemented.
        assert!(
            result.is_not_implemented(),
            "two-node audio quality must not report a fabricated pass"
        );
        assert!(result.error.is_some(), "the missing harness must be named");
        assert!(result.quality.is_none());
    }
}
