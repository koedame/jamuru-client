//! Two-node (peer-to-peer) test scenarios
//!
//! Tests basic P2P audio communication between two local nodes.

use crate::node::TestNode;
use crate::orchestrator::TestOrchestrator;
use crate::{TestConfig, TestResult};
use jamjam::audio::AudioPreset;
use tracing::info;

/// Two-node test suite
pub struct TwoNodeTest {
    config: TestConfig,
}

impl TwoNodeTest {
    /// Create a new two-node test
    pub fn new(config: TestConfig) -> Self {
        Self { config }
    }

    /// Test basic connection establishment
    pub async fn test_connection(&self) -> TestResult {
        info!("Testing two-node connection");

        let mut orchestrator = TestOrchestrator::new(self.config.clone());

        // Create two local nodes
        let host = TestNode::local("host".to_string());
        let client = TestNode::local("client".to_string());

        orchestrator.run_two_node_test(host, client).await
    }

    /// Test audio quality between two nodes
    ///
    /// Requires two processes with virtual audio devices bound to them, which
    /// only dedicated test machines provide. Until that harness exists this
    /// reports NotImplemented rather than a fabricated measurement.
    pub async fn test_audio_quality(&self) -> TestResult {
        info!("Testing two-node audio quality");

        TestResult::not_implemented(
            "two_node_audio_quality",
            "needs two jamuru processes bound to virtual audio devices; \
             the single-process audio path is covered by the loopback layer",
        )
    }

    /// Test reconnection after network interruption
    ///
    /// Requires the ability to sever and restore the link between two running
    /// nodes, which needs the multi-process harness.
    pub async fn test_reconnection(&self) -> TestResult {
        info!("Testing reconnection capability");

        TestResult::not_implemented(
            "two_node_reconnection",
            "needs a harness that can sever and restore the link between two running nodes",
        )
    }

    /// Test every preset across two nodes
    ///
    /// Shares the multi-process prerequisite of [`Self::test_audio_quality`].
    /// Per-preset budgets are verified today by the loopback layer.
    pub async fn test_all_presets(&self) -> Vec<TestResult> {
        AudioPreset::all()
            .iter()
            .map(|preset| {
                TestResult::not_implemented(
                    format!("two_node_preset_{}", preset.name()),
                    "needs two jamuru processes bound to virtual audio devices",
                )
            })
            .collect()
    }
}

/// Run all two-node tests
pub async fn run_all_two_node_tests(preset: &str) -> Vec<TestResult> {
    let config = TestConfig {
        sample_rate: 48000,
        frame_size: 128,
        duration_sec: 5.0,
        preset: preset.to_string(),
    };

    let test = TwoNodeTest::new(config);

    vec![
        test.test_connection().await,
        test.test_audio_quality().await,
        test.test_reconnection().await,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestStatus;

    /// Without a release binary at `target/release/jamuru` the orchestrator
    /// cannot spawn nodes, and the result must say so rather than pass silently.
    /// On a machine where the binary exists the nodes must survive the run.
    #[tokio::test]
    async fn test_two_node_connection() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 1.0,
            preset: "balanced".to_string(),
        };
        let test = TwoNodeTest::new(config);
        let result = test.test_connection().await;

        assert!(
            result.scenario.starts_with("two-node-"),
            "unexpected scenario name: {}",
            result.scenario
        );
        match result.status {
            TestStatus::Passed => assert!(
                result.connection_time_ms.is_some(),
                "a passing run must report how long the nodes took to spawn"
            ),
            TestStatus::Failed => assert!(
                result.error.is_some(),
                "a failed run must say why: {:?}",
                result
            ),
            TestStatus::NotImplemented => panic!(
                "the orchestrator does spawn processes, so this path should not \
                 report NotImplemented: {:?}",
                result
            ),
        }
    }

    /// The two-node audio path has no harness yet, so it must report
    /// NotImplemented with a reason - never a pass.
    #[tokio::test]
    async fn test_audio_quality_reports_missing_harness() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 2.0,
            preset: "balanced".to_string(),
        };
        let test = TwoNodeTest::new(config);
        let result = test.test_audio_quality().await;

        assert!(
            result.is_not_implemented(),
            "must not report a fabricated pass"
        );
        assert!(!result.is_passed());
        assert!(
            result.error.is_some(),
            "a NotImplemented result must say what is missing"
        );
        assert!(
            result.quality.is_none(),
            "no measurement was taken, so no quality figures may be reported"
        );
    }

    #[tokio::test]
    async fn test_presets() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 1.0,
            preset: "balanced".to_string(),
        };
        let test = TwoNodeTest::new(config);
        let results = test.test_all_presets().await;

        assert_eq!(results.len(), 4, "Should cover 4 presets");
        for result in &results {
            assert!(
                result.is_not_implemented(),
                "{} must not report a fabricated pass",
                result.scenario
            );
        }
    }
}
