//! Remote test node management
//!
//! Manages test nodes (local or remote machines) for E2E testing.

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{debug, info};

/// Supported platforms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    MacOS,
    Windows,
}

impl Platform {
    /// Detect the current platform
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        return Platform::Linux;
        #[cfg(target_os = "macos")]
        return Platform::MacOS;
        #[cfg(target_os = "windows")]
        return Platform::Windows;
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        panic!("Unsupported platform");
    }

    /// Get string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::MacOS => "macos",
            Platform::Windows => "windows",
        }
    }
}

/// Parses a platform name, as `FromStr` rather than an inherent method so
/// `"macos".parse()` works and nothing shadows the standard trait.
///
/// An unrecognised name is an error, not Linux. Silently defaulting would run
/// a suite against the wrong platform's audio stack and report the results as
/// if they came from the requested one.
impl std::str::FromStr for Platform {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "linux" => Ok(Platform::Linux),
            "macos" | "darwin" => Ok(Platform::MacOS),
            "windows" | "win" => Ok(Platform::Windows),
            other => Err(format!(
                "unknown platform {:?}; expected linux, macos/darwin or windows/win",
                other
            )),
        }
    }
}

/// A test node (local or remote machine)
#[derive(Debug, Clone)]
pub struct TestNode {
    /// Unique identifier for this node
    pub id: String,
    /// Platform of this node
    pub platform: Platform,
    /// Address for SSH connection (None for local)
    pub ssh_address: Option<String>,
    /// Path to jamjam binary on this node
    pub binary_path: String,
}

impl TestNode {
    /// Create a local test node with default settings
    pub fn local(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            platform: Platform::current(),
            ssh_address: None,
            binary_path: "target/release/jamjam".to_string(),
        }
    }

    /// Create a local test node with specific settings
    pub fn local_with_config(id: impl Into<String>, binary_path: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            platform: Platform::current(),
            ssh_address: None,
            binary_path: binary_path.into(),
        }
    }

    /// Get the platform of this node
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// Create a remote test node
    pub fn remote(
        id: impl Into<String>,
        platform: Platform,
        ssh_address: impl Into<String>,
        binary_path: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            platform,
            ssh_address: Some(ssh_address.into()),
            binary_path: binary_path.into(),
        }
    }

    /// Check if this is a local node
    pub fn is_local(&self) -> bool {
        self.ssh_address.is_none()
    }
}

/// Handle to a running jamjam process
pub struct NodeProcess {
    /// The node this process belongs to
    pub node: TestNode,
    /// The child process (only for local nodes)
    child: Option<Child>,
    /// Whether the process is running
    running: bool,
    /// The process's `$HOME` and the file its played audio goes to, removed with it
    _scratch: Option<tempfile::TempDir>,
}

/// `jamjam <args>` with no sound card and no settings of the machine's: a tone stands in
/// for the input, a file in `scratch` for the output, and `$HOME` is `scratch`.
fn session_command(node: &TestNode, scratch: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(&node.binary_path);
    command
        .args(args)
        .args(["--input-tone", "440", "--output-file"])
        .arg(scratch.join("played.f32"))
        .env("HOME", scratch)
        .env("XDG_CONFIG_HOME", scratch.join(".config"));
    command
}

impl NodeProcess {
    /// Start jamjam on a local node, creating a room on the signaling `server`.
    /// Returns the process and the invite code the others join with.
    pub async fn start_create_room(
        node: TestNode,
        server: &str,
    ) -> Result<(Self, String), NodeError> {
        if !node.is_local() {
            return Err(NodeError::RemoteNotSupported);
        }

        info!("Starting jamjam on node {} to create a room", node.id);

        let scratch = tempfile::tempdir().map_err(|e| NodeError::SpawnFailed(e.to_string()))?;
        let mut child =
            session_command(&node, scratch.path(), &["create-room", "--server", server])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| NodeError::SpawnFailed(e.to_string()))?;

        // The invite code is on the first lines it prints. The rest is read
        // and dropped for as long as the process runs: a closed pipe would
        // make its next print fail.
        let stdout = child.stdout.take().expect("stdout is piped");
        let (code_tx, code_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut code_tx = Some(code_tx);
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(code) = line.strip_prefix("Invite code:") {
                    if let Some(code_tx) = code_tx.take() {
                        let _ = code_tx.send(code.trim().to_string());
                    }
                }
            }
        });
        let invite_code = tokio::time::timeout(std::time::Duration::from_secs(20), code_rx)
            .await
            .map_err(|_| NodeError::ConnectionFailed("no invite code within 20 s".to_string()))?
            .map_err(|_| {
                NodeError::ConnectionFailed("the process ended without one".to_string())
            })?;

        Ok((
            Self {
                node,
                child: Some(child),
                running: true,
                _scratch: Some(scratch),
            },
            invite_code,
        ))
    }

    /// Start jamjam on a local node and join the room `invite_code` on the signaling `server`
    pub async fn start_join_room(
        node: TestNode,
        server: &str,
        invite_code: &str,
    ) -> Result<Self, NodeError> {
        if !node.is_local() {
            return Err(NodeError::RemoteNotSupported);
        }

        info!(
            "Starting jamjam on node {} to join {}",
            node.id, invite_code
        );

        let scratch = tempfile::tempdir().map_err(|e| NodeError::SpawnFailed(e.to_string()))?;
        let child = session_command(
            &node,
            scratch.path(),
            &["join-room", "--server", server, "--room", invite_code],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| NodeError::SpawnFailed(e.to_string()))?;

        Ok(Self {
            node,
            child: Some(child),
            running: true,
            _scratch: Some(scratch),
        })
    }

    /// Stop the process
    pub async fn stop(&mut self) -> Result<(), NodeError> {
        if let Some(ref mut child) = self.child {
            debug!("Stopping node {}", self.node.id);
            child.kill().await.ok();
            self.running = false;
        }
        Ok(())
    }

    /// Check if the process is still running
    ///
    /// Reflects whether [`Self::stop`] has been called; use
    /// [`Self::exit_status`] to find out whether the process died on its own.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Exit status if the process has already terminated, `None` if still alive
    ///
    /// Unlike [`Self::is_running`] this asks the operating system, so a node
    /// that crashed on startup is reported as dead.
    pub fn exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
    }
}

impl Drop for NodeProcess {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            // Try to kill the process synchronously
            // This is a best-effort cleanup
            let _ = child.start_kill();
        }
    }
}

/// Errors that can occur during node operations
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    #[error("Remote node operations not yet supported")]
    RemoteNotSupported,

    #[error("Node not running")]
    NotRunning,

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = Platform::current();
        // Should not panic
        assert!(matches!(
            platform,
            Platform::Linux | Platform::MacOS | Platform::Windows
        ));
    }

    /// Every accepted spelling maps to its platform, and an unknown one is
    /// rejected rather than silently becoming Linux.
    #[test]
    fn test_platform_parsing() {
        for (name, expected) in [
            ("linux", Platform::Linux),
            ("LINUX", Platform::Linux),
            ("macos", Platform::MacOS),
            ("darwin", Platform::MacOS),
            ("windows", Platform::Windows),
            ("win", Platform::Windows),
        ] {
            assert_eq!(
                name.parse::<Platform>().unwrap(),
                expected,
                "{:?} should parse as {:?}",
                name,
                expected
            );
        }

        let error = "windwos".parse::<Platform>().unwrap_err();
        assert!(
            error.contains("windwos") && error.contains("windows"),
            "the error should name what was given and what is accepted, got {:?}",
            error
        );
    }

    #[test]
    fn test_local_node_creation() {
        let node = TestNode::local_with_config("test-node", "/usr/bin/jamjam");
        assert!(node.is_local());
        assert_eq!(node.binary_path, "/usr/bin/jamjam");
    }

    #[test]
    fn test_local_node_simple() {
        let node = TestNode::local("simple-node");
        assert!(node.is_local());
        assert_eq!(node.binary_path, "target/release/jamjam");
    }

    #[test]
    fn test_remote_node_creation() {
        let node = TestNode::remote(
            "remote-node",
            Platform::Linux,
            "user@192.168.1.100",
            "/home/user/jamjam",
        );
        assert!(!node.is_local());
        assert_eq!(node.ssh_address.as_deref(), Some("user@192.168.1.100"));
    }
}
