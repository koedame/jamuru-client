//! Cross-platform test scenarios
//!
//! Tests P2P audio communication across different operating systems.
//! Requires one machine per operating system.

use crate::node::{Platform, TestNode};
use crate::{TestConfig, TestResult, TestStatus};
use std::collections::HashMap;
use std::time::Instant;
use tracing::info;

/// Cross-platform test matrix
pub struct CrossPlatformTest {
    /// Retained for when the cross-platform harness is built; see the
    /// NotImplemented reasons on the scenarios below.
    #[allow(dead_code)]
    config: TestConfig,
    /// Available remote nodes by platform
    nodes: HashMap<Platform, Vec<TestNode>>,
}

impl CrossPlatformTest {
    /// Create a new cross-platform test
    pub fn new(config: TestConfig) -> Self {
        Self {
            config,
            nodes: HashMap::new(),
        }
    }

    /// Register a remote test node
    pub fn add_node(&mut self, node: TestNode) {
        self.nodes.entry(*node.platform()).or_default().push(node);
    }

    /// Generate all platform pairs for testing
    fn get_platform_pairs(&self) -> Vec<(Platform, Platform)> {
        let platforms = vec![Platform::Linux, Platform::MacOS, Platform::Windows];
        let mut pairs = Vec::new();

        for host in &platforms {
            for client in &platforms {
                pairs.push((*host, *client));
            }
        }

        pairs
    }

    /// Run test for a specific platform pair
    pub async fn test_pair(
        &self,
        host_platform: &Platform,
        client_platform: &Platform,
    ) -> TestResult {
        let start = Instant::now();
        let scenario = format!(
            "cross_platform_{}_to_{}",
            host_platform.as_str(),
            client_platform.as_str()
        );

        info!(
            "Testing cross-platform: {} -> {}",
            host_platform.as_str(),
            client_platform.as_str()
        );

        // Get available nodes for each platform
        let host_nodes = self.nodes.get(host_platform);
        let client_nodes = self.nodes.get(client_platform);

        match (host_nodes, client_nodes) {
            (Some(hosts), Some(clients)) if !hosts.is_empty() && !clients.is_empty() => {
                // Driving a registered remote node needs an SSH control channel
                // to start jamuru there and collect the captured audio back.
                TestResult::not_implemented(
                    scenario,
                    "nodes are registered but there is no remote control channel to run jamuru on them",
                )
            }
            _ => TestResult {
                scenario,
                status: TestStatus::Failed,
                duration_ms: start.elapsed().as_millis() as u64,
                connection_time_ms: None,
                quality: None,
                error: Some(format!(
                    "Missing nodes for {} and/or {}",
                    host_platform.as_str(),
                    client_platform.as_str()
                )),
            },
        }
    }

    /// Run full cross-platform matrix test
    pub async fn run_matrix(&self) -> Vec<TestResult> {
        let pairs = self.get_platform_pairs();
        let mut results = Vec::new();

        for (host, client) in pairs {
            let result = self.test_pair(&host, &client).await;
            results.push(result);
        }

        results
    }
}

/// Platform compatibility matrix result
pub struct MatrixResult {
    /// Results indexed by (host_platform, client_platform)
    pub results: HashMap<(Platform, Platform), TestResult>,
    /// Overall pass rate
    pub pass_rate: f32,
}

impl MatrixResult {
    /// Create from test results
    pub fn from_results(results: Vec<TestResult>) -> Self {
        let mut map = HashMap::new();
        let mut passed = 0;
        let total = results.len();

        for result in results {
            // Parse scenario name to extract platforms
            // Format: "cross_platform_{host}_to_{client}"
            if let Some(parts) = result.scenario.strip_prefix("cross_platform_") {
                let platforms: Vec<&str> = parts.split("_to_").collect();
                if let [host, client] = platforms[..] {
                    // Names that are not a platform are skipped, like names
                    // that do not follow the format at all.
                    let (Ok(host), Ok(client)) = (host.parse::<Platform>(), client.parse()) else {
                        continue;
                    };
                    if result.is_passed() {
                        passed += 1;
                    }
                    map.insert((host, client), result);
                }
            }
        }

        let pass_rate = if total > 0 {
            passed as f32 / total as f32
        } else {
            0.0
        };

        Self {
            results: map,
            pass_rate,
        }
    }

    /// Print matrix as ASCII table
    pub fn print_matrix(&self) {
        let platforms = [Platform::Linux, Platform::MacOS, Platform::Windows];

        // Header
        print!("{:10}", "");
        for client in &platforms {
            print!("{:10}", client.as_str());
        }
        println!();

        // Rows
        for host in &platforms {
            print!("{:10}", host.as_str());
            for client in &platforms {
                let status = self
                    .results
                    .get(&(*host, *client))
                    .map(|r| r.status.as_str())
                    .unwrap_or("N/A");
                print!("{:10}", status);
            }
            println!();
        }

        println!("\nPass rate: {:.1}%", self.pass_rate * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_platform_pairs() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 1.0,
            preset: "balanced".to_string(),
        };

        let test = CrossPlatformTest::new(config);
        let pairs = test.get_platform_pairs();

        // 3 platforms x 3 = 9 pairs (including same-platform)
        assert_eq!(pairs.len(), 9);
    }

    #[tokio::test]
    async fn test_empty_matrix() {
        let config = TestConfig {
            sample_rate: 48000,
            frame_size: 128,
            duration_sec: 1.0,
            preset: "balanced".to_string(),
        };

        let test = CrossPlatformTest::new(config);
        let results = test.run_matrix().await;

        // All should fail due to no nodes registered
        assert_eq!(results.len(), 9);
        for result in &results {
            assert!(!result.is_passed(), "Should fail without nodes");
            assert_eq!(result.status, TestStatus::Failed);
        }
    }
}
