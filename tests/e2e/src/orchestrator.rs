//! Test orchestration for multi-node E2E tests
//!
//! Coordinates test execution across multiple nodes.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::fake_signaling::FakeSignaling;
use crate::node::{NodeProcess, TestNode};
use crate::{TestConfig, TestResult};

/// Test orchestrator for coordinating multi-node tests
pub struct TestOrchestrator {
    /// Test configuration
    config: TestConfig,
    /// Active node processes
    processes: HashMap<String, NodeProcess>,
}

impl TestOrchestrator {
    /// Create a new test orchestrator
    pub fn new(config: TestConfig) -> Self {
        Self {
            config,
            processes: HashMap::new(),
        }
    }

    /// Run a two-node connection test
    ///
    /// Verifies that both nodes start and stay alive for the test duration. It
    /// does **not** verify that a P2P connection was established or that audio
    /// flowed - that needs the harness described in Plans.md, under REQ-LAT-102.
    pub async fn run_two_node_test(
        &mut self,
        host_node: TestNode,
        client_node: TestNode,
    ) -> TestResult {
        let scenario = format!(
            "two-node-{:?}-{:?}",
            host_node.platform, client_node.platform
        );
        let start = Instant::now();

        info!("Starting two-node test: {}", scenario);

        // Step 1: Start the signaling server both nodes meet through
        let server = FakeSignaling::start();

        // Step 2: Start host node, which creates the room
        let (host_process, invite_code) =
            match NodeProcess::start_create_room(host_node.clone(), server.url()).await {
                Ok(started) => started,
                Err(e) => {
                    error!("Failed to start host: {}", e);
                    return TestResult::failed(&scenario, format!("Host start failed: {}", e));
                }
            };
        self.processes.insert(host_node.id.clone(), host_process);

        // Step 3: Start client node, which joins it
        let client_process =
            match NodeProcess::start_join_room(client_node.clone(), server.url(), &invite_code)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to start client: {}", e);
                    self.cleanup().await;
                    return TestResult::failed(&scenario, format!("Client start failed: {}", e));
                }
            };
        self.processes
            .insert(client_node.id.clone(), client_process);

        let startup_time = start.elapsed();
        info!("Both nodes spawned in {}ms", startup_time.as_millis());

        // Step 4: Wait for test duration
        tokio::time::sleep(Duration::from_secs(self.config.duration_sec as u64)).await;

        // Step 5: Confirm neither node died while we waited. Without this the
        // test would pass for two processes that exited immediately.
        let died: Vec<String> = self
            .processes
            .iter_mut()
            .filter_map(|(id, process)| {
                process
                    .exit_status()
                    .map(|status| format!("{} exited with {}", id, status))
            })
            .collect();

        // Step 6: Cleanup
        self.cleanup().await;

        let duration = start.elapsed();
        if !died.is_empty() {
            return TestResult::failed(&scenario, died.join("; "));
        }

        let mut result = TestResult::passed(&scenario, duration.as_millis() as u64);
        // Time to spawn both processes, not a confirmed P2P connection.
        result.connection_time_ms = Some(startup_time.as_millis() as u64);

        info!(
            "Two-node test completed: {} ({}ms)",
            scenario,
            duration.as_millis()
        );

        result
    }

    /// Run a loopback test through a real jamjam process on `node`
    ///
    /// Driving the audio path of a separate process needs virtual audio devices
    /// bound to it. The in-process audio path is already covered by
    /// `scenarios::loopback`, which runs the production codec, packet, jitter
    /// buffer and PLC types.
    pub async fn run_loopback_test(&mut self, node: TestNode) -> TestResult {
        let scenario = format!("loopback-{:?}", node.platform);
        info!("Starting loopback test: {}", scenario);

        TestResult::not_implemented(
            scenario,
            "needs virtual audio devices bound to a spawned jamjam process; \
             the in-process audio path is covered by scenarios::loopback",
        )
    }

    /// Cleanup all running processes
    async fn cleanup(&mut self) {
        for (id, mut process) in self.processes.drain() {
            debug!("Cleaning up node: {}", id);
            if let Err(e) = process.stop().await {
                warn!("Failed to stop node {}: {}", id, e);
            }
        }
    }
}

impl Drop for TestOrchestrator {
    fn drop(&mut self) {
        // Best-effort cleanup - processes will be killed in their own Drop impl
        self.processes.clear();
    }
}

/// Scenario definition for declarative test configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestScenario {
    /// Scenario name
    pub name: String,
    /// Description
    pub description: String,
    /// Number of nodes required
    pub node_count: usize,
    /// Platform requirements
    pub platforms: Vec<String>,
    /// Test duration in seconds
    pub duration_sec: u32,
    /// Assertions to verify
    pub assertions: Vec<Assertion>,
}

/// Test assertion
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Assertion {
    /// Assertion type
    #[serde(rename = "type")]
    pub assertion_type: String,
    /// Expected value or threshold
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let config = TestConfig::default();
        let orchestrator = TestOrchestrator::new(config);
        assert!(orchestrator.processes.is_empty());
    }
}
