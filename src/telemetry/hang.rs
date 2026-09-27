//! Where the app did not end cleanly, kept for the next launch.
//!
//! Unlike a panic (`crash.rs`), a stall or a kill leaves no code running
//! that could ask the user anything. So this file is written unconditionally,
//! regardless of the `usage_reporting` setting, by whichever of the app's
//! two sources noticed the trouble, and it is the *next* launch that
//! decides what to do with it: fold it into the usual send if reporting is
//! already on, or, if it is off, offer to send this one record on its own
//! (`UsageReporter::previous_hang`, `send_one_off_hang`).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::event::{Hang, HangStage};
use super::install::{is_valid_id, HANG_FILE};

/// The hang record on disk.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct HangFile {
    pub ts: String,
    /// The launch that did not end cleanly, so its `app_start` and this line
    /// can be joined, and so per-version counts stay attached to the version
    /// that actually stalled (which can differ from the version that goes on
    /// to report it, e.g. after an update).
    pub launch_id: String,
    pub app_version: String,
    pub stage: HangStage,
    pub stalled_ms: Option<u32>,
}

impl HangFile {
    pub(crate) fn event(&self) -> Hang {
        Hang {
            stage: self.stage,
            stalled_ms: self.stalled_ms,
        }
    }

    pub(crate) fn has_valid_ids(&self) -> bool {
        is_valid_id(&self.launch_id)
            && !self.app_version.is_empty()
            && self.app_version.len() <= 32
            && self
                .app_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    }
}

/// Replaces any record already there: only the most recent stage stuck (or
/// the fact that the process left no other record) is worth keeping.
pub(crate) fn write(dir: &Path, hang: &HangFile) {
    let Ok(json) = serde_json::to_string(hang) else {
        return;
    };
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(dir.join(HANG_FILE), json);
}

/// Reads and removes the pending hang record.
pub(crate) fn take(dir: &Path) -> Option<HangFile> {
    let path = dir.join(HANG_FILE);
    let content = fs::read_to_string(&path).ok();
    let _ = fs::remove_file(&path);
    serde_json::from_str(&content?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> HangFile {
        HangFile {
            ts: "2026-09-27T12:00:00Z".to_string(),
            launch_id: "a".repeat(32),
            app_version: "0.1.0".to_string(),
            stage: HangStage::AppExit,
            stalled_ms: Some(5000),
        }
    }

    #[test]
    fn a_written_record_is_read_back_and_then_gone() {
        let dir = tempfile::tempdir().unwrap();

        write(dir.path(), &sample());
        let read = take(dir.path()).unwrap();

        assert_eq!(read.launch_id, sample().launch_id);
        assert_eq!(read.stage, HangStage::AppExit);
        assert_eq!(read.stalled_ms, Some(5000));
        assert!(
            take(dir.path()).is_none(),
            "the record is removed once read"
        );
    }

    #[test]
    fn a_record_with_no_stalled_time_reads_back_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut hang = sample();
        hang.stage = HangStage::Unknown;
        hang.stalled_ms = None;

        write(dir.path(), &hang);

        let read = take(dir.path()).unwrap();
        assert_eq!(read.stage, HangStage::Unknown);
        assert_eq!(read.stalled_ms, None);
    }

    #[test]
    fn ids_are_valid_only_when_well_formed() {
        assert!(sample().has_valid_ids());

        let mut bad_launch = sample();
        bad_launch.launch_id = "not-hex".to_string();
        assert!(!bad_launch.has_valid_ids());

        let mut bad_version = sample();
        bad_version.app_version = "".to_string();
        assert!(!bad_version.has_valid_ids());
    }
}
