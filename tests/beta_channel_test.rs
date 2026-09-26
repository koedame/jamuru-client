//! Guards how a beta learns about the next beta (ADR-045): the version a beta is
//! built as (`scripts/build-version.sh`) and the file it reads
//! (`scripts/publish-beta-channel.sh`).
//!
//! Both scripts run for real. `gh` is a stand-in that keeps the channel's
//! release in a directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_version(tag: &str) -> Output {
    Command::new("bash")
        .arg("scripts/build-version.sh")
        .arg(tag)
        .current_dir(repo_root())
        .output()
        .expect("bash is needed to run scripts/build-version.sh")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Verifies: REQ-UPD-012
#[test]
fn when_the_tag_is_a_beta_the_app_is_built_as_the_version_with_the_beta_number() {
    let output = build_version("v0.1.0-beta.17");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "0.1.0-17");
}

/// Verifies: REQ-UPD-012
#[test]
fn when_the_tag_is_a_release_the_app_is_built_as_the_tag_version() {
    let output = build_version("v0.1.0");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "0.1.0");
}

/// Only a number is allowed after the dash: the Windows installer takes
/// nothing else, and anything else has no order against the betas.
///
/// Verifies: REQ-UPD-012
#[test]
fn when_the_tag_is_another_pre_release_no_version_is_given() {
    for tag in [
        "v0.1.0-rc.1",
        "v0.1.0-beta.x",
        "v0.1.0-beta",
        "v0.1.0-alpha-beta.3",
    ] {
        let output = build_version(tag);

        assert!(!output.status.success(), "{tag} was accepted");
        assert!(output.stdout.is_empty(), "{tag}: {}", stdout(&output));
    }
}

/// Verifies: REQ-UPD-012
#[test]
fn a_beta_is_older_than_the_next_beta_and_than_its_own_release() {
    let older = |a: &str, b: &str| {
        Command::new("dpkg")
            .args(["--compare-versions", a, "lt", b])
            .status()
            .unwrap()
            .success()
    };

    assert!(older("0.1.0~9", "0.1.0~17"));
    assert!(older("0.1.0~17", "0.1.0"));
    assert!(!older("0.1.0", "0.1.0~17"));
    assert!(older("0.1.0", "0.2.0~1"));
}

/// The channel's release, as `gh` would keep it: whether it exists, and the
/// `latest.json` asset.
struct Channel {
    dir: tempfile::TempDir,
}

impl Channel {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(
            &gh,
            r#"#!/usr/bin/env bash
set -eu
state=$(dirname "$0")
[ "$1" = release ] || exit 2
case "$2" in
  view) [ -e "$state/exists" ] ;;
  create) touch "$state/exists"; echo "$*" >> "$state/created" ;;
  download) [ -e "$state/latest.json" ] && cat "$state/latest.json" ;;
  upload) cp "$4" "$state/latest.json" ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        Self { dir }
    }

    fn published(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.dir.path().join("latest.json")).ok()?;
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        Some(manifest["version"].as_str().unwrap().to_string())
    }

    fn created(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("created"))
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn publish(&self, version: &str) -> Output {
        let manifest = self.dir.path().join(format!("manifest-{version}.json"));
        std::fs::write(&manifest, format!(r#"{{"version":"{version}"}}"#)).unwrap();
        publish(&manifest, self.dir.path())
    }
}

fn publish(manifest: &Path, state: &Path) -> Output {
    Command::new("bash")
        .arg("scripts/publish-beta-channel.sh")
        .arg(manifest)
        .current_dir(repo_root())
        .env("GH", state.join("gh"))
        .env("GITHUB_REPOSITORY", "koedame/jamjam-client")
        .env("GITHUB_SHA", "0000000")
        .output()
        .expect("bash is needed to run scripts/publish-beta-channel.sh")
}

/// Verifies: REQ-UPD-014
#[test]
fn when_the_channel_has_never_been_published_it_is_created_with_the_manifest() {
    let channel = Channel::new();

    let output = channel.publish("0.1.0-17");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(channel.published().as_deref(), Some("0.1.0-17"));
    assert_eq!(channel.created(), 1);
}

/// Verifies: REQ-UPD-014
#[test]
fn when_a_newer_beta_is_published_it_replaces_the_manifest() {
    let channel = Channel::new();
    channel.publish("0.1.0-9");

    let output = channel.publish("0.1.0-17");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(channel.published().as_deref(), Some("0.1.0-17"));
    assert_eq!(channel.created(), 1);
}

/// Verifies: REQ-UPD-014
#[test]
fn when_the_release_of_the_version_is_published_it_replaces_the_beta() {
    let channel = Channel::new();
    channel.publish("0.1.0-17");

    let output = channel.publish("0.1.0");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(channel.published().as_deref(), Some("0.1.0"));
}

/// A run that finishes late must not pull the betas back.
///
/// Verifies: REQ-UPD-014
#[test]
fn when_an_older_version_is_published_the_manifest_is_kept() {
    let channel = Channel::new();
    channel.publish("0.1.0");

    let output = channel.publish("0.1.0-18");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(channel.published().as_deref(), Some("0.1.0"));
}

/// Verifies: REQ-UPD-014
#[test]
fn when_the_beta_of_the_next_version_is_published_it_replaces_the_release() {
    let channel = Channel::new();
    channel.publish("0.1.0");

    let output = channel.publish("0.2.0-1");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(channel.published().as_deref(), Some("0.2.0-1"));
}
