//! E2E Test Infrastructure for jamuru
//!
//! This crate provides end-to-end testing capabilities for the jamuru
//! P2P audio communication application.
//!
//! ## Test Layers
//!
//! - **Loopback tests**: Test audio path without real devices (software pipeline)
//! - **Network tests**: Test P2P communication on localhost
//! - **Remote tests**: Test multi-node scenarios across machines
//!
//! ## Features
//!
//! - `loopback`: Enable loopback tests through the software pipeline
//! - `network-local`: Enable localhost network tests
//! - `remote`: Enable remote multi-node tests
//! - `full`: Enable all test features

pub mod audio_injection;
#[path = "../../common/fake_signaling.rs"]
pub mod fake_signaling;
pub mod node;
pub mod orchestrator;
pub mod pipeline;
pub mod pom;
pub mod quality;
pub mod scenarios;

// Re-exports for convenience
pub use audio_injection::AudioInjector;
pub use node::{Platform, TestNode};
pub use orchestrator::TestOrchestrator;
pub use pipeline::SoftwarePipeline;
pub use quality::{LatencyMeasurer, PesqEvaluator, QualityResult};

/// Test configuration
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// Sample rate for audio tests
    pub sample_rate: u32,
    /// Frame size in samples
    pub frame_size: u32,
    /// Test duration in seconds
    pub duration_sec: f32,
    /// Preset to test
    pub preset: String,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 10.0,
            preset: "balanced".to_string(),
        }
    }
}

/// Outcome of an E2E test scenario
///
/// `NotImplemented` exists so that a scenario whose infrastructure does not
/// exist yet reports honestly instead of fabricating a passing measurement.
/// It is not a failure - the code under test was never exercised - but it is
/// also not a pass, and `tests/traceability_test.rs` counts the requirement as
/// unverified (ADR-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    /// The scenario ran and met its thresholds
    Passed,
    /// The scenario ran and did not meet its thresholds
    Failed,
    /// The scenario could not run because its infrastructure is missing
    NotImplemented,
}

impl TestStatus {
    /// Short label for report tables
    pub fn as_str(&self) -> &'static str {
        match self {
            TestStatus::Passed => "PASS",
            TestStatus::Failed => "FAIL",
            TestStatus::NotImplemented => "N/I",
        }
    }
}

/// Result of an E2E test scenario
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestResult {
    /// Test scenario name
    pub scenario: String,
    /// Outcome of the scenario
    pub status: TestStatus,
    /// Test duration in milliseconds
    pub duration_ms: u64,
    /// Connection establishment time in milliseconds
    pub connection_time_ms: Option<u64>,
    /// Audio quality metrics
    pub quality: Option<QualityResult>,
    /// Error message if failed, or the reason if not implemented
    pub error: Option<String>,
}

impl TestResult {
    /// Create a passed result
    pub fn passed(scenario: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            scenario: scenario.into(),
            status: TestStatus::Passed,
            duration_ms,
            connection_time_ms: None,
            quality: None,
            error: None,
        }
    }

    /// Create a failed result
    pub fn failed(scenario: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            scenario: scenario.into(),
            status: TestStatus::Failed,
            duration_ms: 0,
            connection_time_ms: None,
            quality: None,
            error: Some(error.into()),
        }
    }

    /// Create a result for a scenario whose infrastructure does not exist yet
    ///
    /// `reason` must say what is missing, so the report explains why the
    /// requirement is unverified rather than leaving a blank.
    pub fn not_implemented(scenario: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            scenario: scenario.into(),
            status: TestStatus::NotImplemented,
            duration_ms: 0,
            connection_time_ms: None,
            quality: None,
            error: Some(reason.into()),
        }
    }

    /// Whether the scenario ran and passed
    pub fn is_passed(&self) -> bool {
        self.status == TestStatus::Passed
    }

    /// Whether the scenario could not run
    pub fn is_not_implemented(&self) -> bool {
        self.status == TestStatus::NotImplemented
    }
}
