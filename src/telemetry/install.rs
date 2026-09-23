//! The install ID and the small files kept next to it.
//!
//! Made when the user turns usage reporting on, and discarded when they turn
//! it off. It lives in its own directory, apart from `config.toml` (the
//! settings are sent whole, so nothing else may sit in that file) and from
//! `device_identity.json` (the ID is not tied to the device's key: turning
//! reporting off and on again gives a new one).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use data_encoding::HEXLOWER;
use directories::ProjectDirs;

const INSTALL_ID_FILE: &str = "install_id";
pub(crate) const CRASH_FILE: &str = "crash.json";

/// Where the install ID and the pending crash record are kept.
///
/// `None` when the OS gives no data directory, in which case nothing is
/// collected.
pub fn state_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "jamjam").map(|dirs| dirs.data_local_dir().join("usage"))
}

/// 16 random bytes as 32 lowercase hex characters.
pub(crate) fn random_id() -> String {
    HEXLOWER.encode(&rand::random::<[u8; 16]>())
}

pub(crate) fn is_valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The stored install ID, if there is a well-formed one.
pub(crate) fn load_install_id(dir: &Path) -> Option<String> {
    let id = fs::read_to_string(dir.join(INSTALL_ID_FILE)).ok()?;
    let id = id.trim();
    is_valid_id(id).then(|| id.to_string())
}

/// Makes a new install ID and stores it.
pub(crate) fn create_install_id(dir: &Path) -> io::Result<String> {
    fs::create_dir_all(dir)?;
    let id = random_id();
    fs::write(dir.join(INSTALL_ID_FILE), &id)?;
    Ok(id)
}

/// Removes everything kept for reporting: the install ID and a crash record
/// that has not been sent.
pub(crate) fn discard(dir: &Path) {
    for name in [INSTALL_ID_FILE, CRASH_FILE] {
        let _ = fs::remove_file(dir.join(name));
    }
}
