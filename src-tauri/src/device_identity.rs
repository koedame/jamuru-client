//! This installation's device identity, held for the app's lifetime (ADR-024)
//!
//! Where it is stored and how it is created is `jamjam::identity_store`,
//! shared with the CLI. The identity is presented to the signaling server by
//! [`crate::signaling::signaling_connect`].
//!
//! No Tauri command exposes the identifier - it is an internal identifier,
//! not something the UI shows.

use std::sync::Arc;

use jamjam::network::DeviceIdentity;

/// Tauri-managed state holding the identity for the process lifetime, so the
/// key is read from disk exactly once at startup.
pub struct DeviceIdentityState {
    identity: Arc<DeviceIdentity>,
}

impl DeviceIdentityState {
    /// Loads the stored identity, or generates and persists one on first
    /// launch (`jamjam::identity_store::load_installation_identity`).
    pub fn load() -> Self {
        Self {
            identity: Arc::new(jamjam::identity_store::load_installation_identity()),
        }
    }

    /// A fresh identity that is not stored, for tests.
    #[cfg(test)]
    pub fn generated() -> Self {
        Self {
            identity: Arc::new(DeviceIdentity::generate()),
        }
    }

    /// Shared handle for `SignalingClient::new`.
    pub fn identity(&self) -> Arc<DeviceIdentity> {
        self.identity.clone()
    }
}
