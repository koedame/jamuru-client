//! Where this installation's device identity lives (ADR-024)
//!
//! jamuru has no account registration. The first time the app or the CLI
//! connects, it generates an Ed25519 key pair (see
//! [`crate::network::DeviceIdentity`]), stores the secret key in the config
//! directory, and reuses it from then on: the app and the CLI on one machine
//! are the same device, as they share `config.toml` (ADR-027).
//!
//! The identity lives in `device_identity.json`, deliberately separate from
//! `config.toml`: copying a settings file between machines must not clone a
//! device identifier.

use std::fs;
use std::path::{Path, PathBuf};

use data_encoding::BASE64;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::config_dir;
use crate::network::DeviceIdentity;

/// File name inside the config directory.
const IDENTITY_FILE_NAME: &str = "device_identity.json";

/// Schema version of [`StoredIdentity`], so a future key format can be told
/// apart from this one instead of being misread.
const IDENTITY_VERSION: u32 = 1;

/// On-disk form of a device identity.
#[derive(Debug, Serialize, Deserialize)]
struct StoredIdentity {
    version: u32,
    /// Base64-encoded 32-byte Ed25519 secret key.
    secret_key: String,
}

/// Loads the identity stored at `path`, generating and writing a new one if
/// the file is missing or unreadable.
///
/// Separate from [`load_installation_identity`] so it can be unit-tested
/// against a temporary directory.
///
/// A corrupt or unparseable file is replaced rather than treated as a fatal
/// error: the alternative is an app that refuses to start, and the identity
/// is not a credential the user can restore by hand.
pub fn load_or_create_at(path: &Path) -> Result<DeviceIdentity, String> {
    match read_identity(path) {
        Ok(Some(identity)) => return Ok(identity),
        Ok(None) => {}
        Err(e) => warn!(
            "Discarding unusable device identity at {:?} and generating a new one: {}",
            path, e
        ),
    }

    let identity = DeviceIdentity::generate();
    write_identity(path, &identity)?;
    // Not the identifier itself: it is internal (REQ-GUI-002) and this line
    // ends up in a log file users send to others (ADR-036).
    info!("Generated a new device identity");
    Ok(identity)
}

/// `Ok(None)` means "no identity stored yet"; `Err` means a file exists but
/// could not be turned into an identity.
fn read_identity(path: &Path) -> Result<Option<DeviceIdentity>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path).map_err(|e| format!("read failed: {}", e))?;
    let stored: StoredIdentity =
        serde_json::from_str(&content).map_err(|e| format!("parse failed: {}", e))?;

    if stored.version != IDENTITY_VERSION {
        return Err(format!("unsupported version {}", stored.version));
    }

    let bytes = BASE64
        .decode(stored.secret_key.as_bytes())
        .map_err(|e| format!("secret_key is not base64: {}", e))?;
    let secret: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "secret_key is not 32 bytes".to_string())?;

    Ok(Some(DeviceIdentity::from_secret_bytes(&secret)))
}

fn write_identity(path: &Path, identity: &DeviceIdentity) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create {:?}: {}", parent, e))?;
    }

    let stored = StoredIdentity {
        version: IDENTITY_VERSION,
        secret_key: BASE64.encode(&identity.secret_bytes()),
    };
    let json = serde_json::to_string_pretty(&stored)
        .map_err(|e| format!("Failed to serialize device identity: {}", e))?;

    // Written whole to a file only the owner can open, then moved into place: the key is never
    // readable by others, and the app and the CLI starting together never read half a file.
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    write_owner_only(&temp, json.as_bytes())
        .map_err(|e| format!("Failed to write {:?}: {}", temp, e))?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("Failed to move {:?} to {:?}: {}", temp, path, e)
    })
}

/// Creates `path` readable and writable by the owner only (0600) and writes
/// `contents`. Windows has no equivalent mode bits - NTFS ACLs already limit
/// the app data directory to the user.
#[cfg(unix)]
fn write_owner_only(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(contents)
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    fs::write(path, contents)
}

fn identity_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(IDENTITY_FILE_NAME))
}

/// This installation's identity: the stored one, or one generated and stored
/// now.
///
/// If the config directory can't be determined or written to, falls back to an
/// in-memory identity: an identifier that changes between launches is better
/// than refusing to connect.
pub fn load_installation_identity() -> DeviceIdentity {
    match identity_path() {
        Some(path) => load_or_create_at(&path).unwrap_or_else(|e| {
            warn!(
                "Could not persist a device identity ({}); using a temporary one for this session",
                e
            );
            DeviceIdentity::generate()
        }),
        None => {
            warn!("Could not determine the config directory; using a temporary device identity");
            DeviceIdentity::generate()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-IDT-001
    #[test]
    fn test_first_launch_generates_and_persists_an_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);

        let identity = load_or_create_at(&path).unwrap();
        assert!(path.exists(), "the identity file should have been written");
        assert_eq!(identity.device_id().len(), crate::network::DEVICE_ID_LEN);
    }

    /// Verifies: REQ-IDT-001
    #[test]
    fn test_later_launches_reuse_the_same_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);

        let first = load_or_create_at(&path).unwrap();
        let second = load_or_create_at(&path).unwrap();
        let third = load_or_create_at(&path).unwrap();

        assert_eq!(first.device_id(), second.device_id());
        assert_eq!(first.device_id(), third.device_id());
        assert_eq!(first.secret_bytes(), second.secret_bytes());
    }

    /// Verifies: REQ-IDT-001
    #[test]
    fn test_separate_installations_get_separate_device_ids() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        let a = load_or_create_at(&dir_a.path().join(IDENTITY_FILE_NAME)).unwrap();
        let b = load_or_create_at(&dir_b.path().join(IDENTITY_FILE_NAME)).unwrap();

        assert_ne!(a.device_id(), b.device_id());
    }

    /// Verifies: REQ-IDT-001
    #[test]
    fn test_missing_parent_directory_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(IDENTITY_FILE_NAME);

        let first = load_or_create_at(&path).unwrap();
        assert!(path.exists());
        assert_eq!(
            load_or_create_at(&path).unwrap().device_id(),
            first.device_id()
        );
    }

    /// Verifies: REQ-IDT-006
    #[cfg(unix)]
    #[test]
    fn test_identity_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);
        load_or_create_at(&path).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    /// The key is written to a temporary file and moved into place, so a
    /// reader never sees half of it; nothing is left behind.
    #[test]
    fn test_writing_an_identity_leaves_only_the_identity_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);
        load_or_create_at(&path).unwrap();
        fs::write(&path, "corrupt").unwrap();
        load_or_create_at(&path).unwrap();

        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![IDENTITY_FILE_NAME.to_string()]);
    }

    #[test]
    fn test_corrupt_file_is_replaced_with_a_fresh_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);
        fs::write(&path, "this is not json").unwrap();

        let identity = load_or_create_at(&path).expect("a corrupt file must not be fatal");
        // The replacement is itself persisted, so the id is stable from here.
        assert_eq!(
            load_or_create_at(&path).unwrap().device_id(),
            identity.device_id()
        );
    }

    #[test]
    fn test_unknown_version_is_replaced_with_a_fresh_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);
        fs::write(
            &path,
            r#"{"version":999,"secret_key":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#,
        )
        .unwrap();

        let identity = load_or_create_at(&path).expect("an unknown version must not be fatal");
        let reread = load_or_create_at(&path).unwrap();
        assert_eq!(reread.device_id(), identity.device_id());
    }

    #[test]
    fn test_wrong_length_secret_key_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);
        fs::write(&path, r#"{"version":1,"secret_key":"AAAA"}"#).unwrap();

        let identity = load_or_create_at(&path).expect("a short key must not be fatal");
        assert_eq!(identity.device_id().len(), crate::network::DEVICE_ID_LEN);
    }

    #[test]
    fn test_stored_file_is_json_with_a_version_and_base64_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(IDENTITY_FILE_NAME);
        let identity = load_or_create_at(&path).unwrap();

        let stored: StoredIdentity =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.version, IDENTITY_VERSION);
        assert_eq!(
            BASE64.decode(stored.secret_key.as_bytes()).unwrap(),
            identity.secret_bytes()
        );
    }
}
