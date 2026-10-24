//! Guards that no release is published before the update drill passed, and
//! that what the drill breaks on purpose cannot ship (ADR-089).
//!
//! The drill itself (`scripts/update-drill.py`) installs real packages and
//! runs on CI only (`.github/workflows/update-drill.yml`). These read what
//! decides that it runs, and run the check of the binaries for real.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_file(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo_file(path)).unwrap_or_else(|e| panic!("{}: {}", path, e))
}

/// The lines of the job `name` in a workflow, up to the next job.
fn job(workflow: &str, name: &str) -> String {
    let head = format!("  {}:", name);
    let mut lines = workflow.lines().skip_while(|line| *line != head);
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("the workflow has no job {}", name));
    std::iter::once(first)
        .chain(lines.take_while(|line| {
            line.is_empty() || line.starts_with("   ") || line.trim_start().starts_with('#')
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Verifies: REQ-UPD-032
#[test]
fn no_release_is_published_before_the_update_drill_passed() {
    let workflow = read(".github/workflows/release.yml");

    let drill = job(&workflow, "update-drill");
    assert!(
        drill.contains("uses: ./.github/workflows/update-drill.yml"),
        "the release does not run the drill:\n{}",
        drill
    );
    let release = job(&workflow, "release");
    let needs = release
        .lines()
        .find_map(|line| line.trim().strip_prefix("needs:"))
        .unwrap_or_else(|| panic!("the release job waits for nothing:\n{}", release));
    assert!(
        needs.contains("update-drill"),
        "the release is published without waiting for the drill: needs {}",
        needs
    );
}

/// The drill builds and runs each kind of package that installs an update
/// itself, and each is run where it would be installed.
///
/// Verifies: REQ-UPD-032
#[test]
fn the_update_drill_runs_on_every_system_that_installs_updates_itself() {
    let workflow = read(".github/workflows/update-drill.yml");

    assert!(
        workflow.contains("workflow_call:"),
        "the release cannot run the drill"
    );
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ] {
        assert!(
            workflow.contains(target),
            "the drill does not run for {}",
            target
        );
    }
    assert!(workflow.contains("scripts/update-drill.py build"));
    assert!(workflow.contains("scripts/update-drill.py run"));
    assert!(
        !workflow.contains("continue-on-error"),
        "a failing drill would not stop the release"
    );
}

/// Verifies: REQ-UPD-032
#[test]
fn the_drills_breaking_is_not_a_default_feature() {
    let manifest = read("src-tauri/Cargo.toml");
    let features = manifest
        .split_once("[features]")
        .expect("src-tauri/Cargo.toml has no [features]")
        .1;
    let default = features
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("default"))
        .expect("src-tauri/Cargo.toml has no default feature");

    assert!(
        features
            .lines()
            .any(|line| line.trim().starts_with("update-drill = ")),
        "the drill's feature is gone, so the guard below guards nothing"
    );
    assert!(
        !default.contains("update-drill"),
        "every build could be broken on purpose: {}",
        default
    );
}

/// Verifies: REQ-UPD-032
#[test]
fn each_release_build_is_checked_to_have_nothing_of_the_drill() {
    let workflow = read(".github/workflows/release.yml");
    let build = job(&workflow, "build");

    assert!(
        build
            .lines()
            .any(|line| line.contains("scripts/check-update-drill.sh")
                && line.trim_end().ends_with("absent")),
        "the release's binaries are not checked for the drill:\n{}",
        build
    );
}

fn check(binary: &Path, expected: &str) -> std::process::Output {
    Command::new("bash")
        .arg(repo_file("scripts/check-update-drill.sh"))
        .arg(binary)
        .arg(expected)
        .output()
        .expect("bash is needed to run scripts/check-update-drill.sh")
}

/// Verifies: REQ-UPD-032
#[test]
fn a_build_that_can_be_broken_on_purpose_does_not_pass_as_a_release() {
    let dir = tempfile::tempdir().unwrap();
    let drill = dir.path().join("drill");
    std::fs::write(&drill, b"\x7fELF\0...jamjam-update-drill-break\0...").unwrap();
    let release = dir.path().join("release");
    std::fs::write(&release, b"\x7fELF\0...JAMJAM_SERVER_URL\0...").unwrap();

    assert!(!check(&drill, "absent").status.success());
    assert!(check(&drill, "present").status.success());
    assert!(check(&release, "absent").status.success());
    assert!(!check(&release, "present").status.success());
    assert!(!check(&dir.path().join("missing"), "absent")
        .status
        .success());
}
