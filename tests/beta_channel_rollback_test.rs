//! Guards the deliberate way of moving the beta update channel backwards
//! (ADR-057): `scripts/publish-beta-channel-rollback.sh` republishes a past
//! release's own `latest.json` as beta-channel's manifest, bypassing the
//! "only a newer version" guard in `scripts/publish-beta-channel.sh`.
//!
//! The script runs for real. `gh` is a stand-in that keeps every release
//! (including beta-channel) in its own directory, so it can serve both the
//! release being rolled back to and the channel being rolled back.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The GitHub releases involved, as `gh` would keep them: each release has its
/// own `latest.json` asset (or none, for a release cut before ADR-041).
struct Releases {
    dir: tempfile::TempDir,
}

impl Releases {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            r#"#!/usr/bin/env bash
set -eu
state=$(dirname "$0")
[ "$1" = release ] || exit 2
tag=$3
dir="$state/releases/$tag"
case "$2" in
  view) [ -e "$dir/exists" ] ;;
  create) mkdir -p "$dir"; touch "$dir/exists"; echo "$*" >> "$state/created" ;;
  download) [ -e "$dir/latest.json" ] && cat "$dir/latest.json" ;;
  upload) mkdir -p "$dir"; touch "$dir/exists"; cp "$4" "$dir/latest.json" ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        Self { dir }
    }

    /// A past release with an update manifest, as `make-update-manifest.sh`
    /// would have built it at release time.
    fn seed(&self, tag: &str, version: &str) {
        let release_dir = self.dir.path().join("releases").join(tag);
        std::fs::create_dir_all(&release_dir).unwrap();
        std::fs::write(release_dir.join("exists"), "").unwrap();
        std::fs::write(
            release_dir.join("latest.json"),
            format!(r#"{{"version":"{version}"}}"#),
        )
        .unwrap();
    }

    /// A release that exists but predates ADR-041: it has no manifest asset.
    fn seed_without_manifest(&self, tag: &str) {
        let release_dir = self.dir.path().join("releases").join(tag);
        std::fs::create_dir_all(&release_dir).unwrap();
        std::fs::write(release_dir.join("exists"), "").unwrap();
    }

    fn channel_version(&self) -> Option<String> {
        let text =
            std::fs::read_to_string(self.dir.path().join("releases/beta-channel/latest.json"))
                .ok()?;
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        Some(manifest["version"].as_str().unwrap().to_string())
    }

    fn rollback(&self, tag: &str) -> Output {
        Command::new("bash")
            .arg("scripts/publish-beta-channel-rollback.sh")
            .arg(tag)
            .current_dir(repo_root())
            .env("GH", self.dir.path().join("gh"))
            .env("GITHUB_REPOSITORY", "koedame/jamjam-client")
            .env("GITHUB_SHA", "0000000") // needed only if beta-channel does not exist yet
            .output()
            .expect("bash is needed to run scripts/publish-beta-channel-rollback.sh")
    }
}

/// Verifies: REQ-UPD-019
#[test]
fn when_rolling_back_to_a_past_release_the_channel_gets_that_releases_manifest() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.9", "0.1.0-9");
    releases.seed("beta-channel", "0.1.0-17"); // the bad build, currently live

    let output = releases.rollback("v0.1.0-beta.9");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.channel_version().as_deref(), Some("0.1.0-9"));
}

/// Verifies: REQ-UPD-019
#[test]
fn when_there_is_no_channel_yet_the_rollback_creates_it() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.9", "0.1.0-9");

    let output = releases.rollback("v0.1.0-beta.9");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.channel_version().as_deref(), Some("0.1.0-9"));
}

/// Verifies: REQ-UPD-019
#[test]
fn when_the_release_does_not_exist_the_rollback_fails() {
    let releases = Releases::new();

    let output = releases.rollback("v9.9.9-beta.1");

    assert!(!output.status.success());
    assert!(releases.channel_version().is_none());
}

/// A release cut before ADR-041 never got an update manifest uploaded to it,
/// so there is nothing to roll back to.
///
/// Verifies: REQ-UPD-019
#[test]
fn when_the_release_has_no_update_manifest_the_rollback_fails() {
    let releases = Releases::new();
    releases.seed_without_manifest("v0.0.9");

    let output = releases.rollback("v0.0.9");

    assert!(!output.status.success());
    assert!(releases.channel_version().is_none());
}
