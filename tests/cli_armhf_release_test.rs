//! Guards the ARM Linux builds: the command-line tool for 32-bit (ADR-126,
//! REQ-DIST-010) and, with the app, for 64-bit (ADR-127, REQ-DIST-011). That
//! every release carries them, that nothing is published without them, and that
//! the scripts behind them stop on a wrong call.
//!
//! Cross-building and running the real binary need the armhf toolchain and
//! qemu, or an arm64 machine, so those are done by the `build-cli` job itself.
//! What can be pinned on every `cargo test` is the shape of the workflows and
//! the scripts' refusals.

use std::path::PathBuf;
use std::process::Command;

fn repo_file(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn read(path: &str) -> String {
    std::fs::read_to_string(repo_file(path)).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

/// The text of the job `name` in a workflow: its header line to the next job's.
fn job(workflow: &str, name: &str) -> String {
    let header = format!("  {name}:");
    let mut lines = workflow.lines().skip_while(|line| *line != header);
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("the workflow has no job {name}"));
    std::iter::once(first)
        .chain(lines.take_while(|line| {
            line.is_empty() || line.starts_with("   ") || line.trim_start().starts_with('#')
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Verifies: REQ-DIST-010
#[test]
fn when_a_release_is_made_the_armhf_cli_is_built_and_published_with_its_checksum() {
    let workflow = read(".github/workflows/release.yml");

    let build = job(&workflow, "build-cli");
    assert!(
        build.contains("target: armv7-unknown-linux-gnueabihf")
            && build.contains("scripts/build-cli.sh ${{ matrix.target }}"),
        "the release does not build the armhf CLI:\n{build}"
    );
    assert!(
        build.contains("scripts/check-cli-runs.sh") && build.contains("/usr/arm-linux-gnueabihf"),
        "the armhf CLI is not started under qemu before it is released:\n{build}"
    );

    let release = job(&workflow, "release");
    let needs = release
        .lines()
        .find_map(|line| line.trim().strip_prefix("needs:"))
        .expect("the release job waits for nothing");
    assert!(
        needs.contains("build-cli"),
        "a release is published without waiting for the armhf CLI: needs {needs}"
    );
    for published in [
        "artifacts/jamuru-cli-linux-armv7/**/*.tar.gz\n",
        "artifacts/jamuru-cli-linux-armv7/**/*.tar.gz.sha256\n",
    ] {
        assert!(
            release.contains(published),
            "the release does not upload {published}"
        );
    }
    assert!(
        release.contains("\"artifacts/jamuru-cli-linux-armv7/**/*.tar.gz\""),
        "the release does not stop when the armhf CLI is missing"
    );
}

/// The download is neither signed nor updated by the app, so the job has no
/// reason to read a signing key, the server's address or any other secret, and
/// an unsigned build must not be able to take one.
///
/// Verifies: REQ-DIST-010
#[test]
fn when_the_armhf_cli_is_built_the_job_reads_no_secret_and_holds_no_write_access() {
    for file in [".github/workflows/release.yml", ".github/workflows/ci.yml"] {
        let build = job(&read(file), "build-cli");
        assert!(
            !build.contains("secrets."),
            "{file}: build-cli reads a secret"
        );
        assert!(
            !build.contains("environment:"),
            "{file}: build-cli enters an environment"
        );
        assert!(
            build.contains("persist-credentials: false"),
            "{file}: build-cli leaves the repository token in .git/config"
        );
    }
    let release = job(&read(".github/workflows/release.yml"), "build-cli");
    assert!(
        release.contains("permissions:\n      contents: read\n"),
        "build-cli inherits the release workflow's write access"
    );
}

/// Verifies: REQ-DIST-010
#[test]
fn when_a_change_is_pushed_the_armhf_cli_is_built_so_a_dependency_that_stops_building_is_caught_early(
) {
    let build = job(&read(".github/workflows/ci.yml"), "build-cli");

    assert!(build.contains("target: armv7-unknown-linux-gnueabihf"));
    assert!(build.contains("scripts/build-cli.sh ${{ matrix.target }}"));
    assert!(build.contains("scripts/check-cli-runs.sh"));
}

/// Verifies: REQ-DIST-010
#[test]
fn when_the_target_is_not_armv7_the_build_script_refuses_before_building_anything() {
    let out = Command::new("bash")
        .arg(repo_file("scripts/build-cli.sh"))
        .args(["x86_64-unknown-linux-gnu", "unused-output-directory"])
        .output()
        .expect("bash is needed to run scripts/build-cli.sh");

    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unsupported target"));
}

/// Verifies: REQ-DIST-010
#[test]
fn when_the_binary_does_not_exist_the_run_check_fails_instead_of_passing() {
    let out = Command::new("bash")
        .arg(repo_file("scripts/check-cli-runs.sh"))
        .args(["/nonexistent/jamuru", "/usr/arm-linux-gnueabihf"])
        .output()
        .expect("bash is needed to run scripts/check-cli-runs.sh");

    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not exist"));
}

/// Verifies: REQ-DIST-011
#[test]
fn when_a_release_is_made_the_arm64_cli_is_built_natively_started_and_published_with_its_checksum()
{
    let workflow = read(".github/workflows/release.yml");

    let build = job(&workflow, "build-cli");
    assert!(
        build.contains("runner: ubuntu-22.04-arm")
            && build.contains("target: aarch64-unknown-linux-gnu")
            && build.contains("artifact_name: jamuru-cli-linux-aarch64"),
        "the release does not build the arm64 CLI on an arm64 runner:\n{build}"
    );
    assert!(
        build.contains("scripts/check-cli-runs.sh target/${{ matrix.target }}/release/jamuru\n"),
        "the arm64 CLI is not started before it is released:\n{build}"
    );

    let release = job(&workflow, "release");
    for published in [
        "artifacts/jamuru-cli-linux-aarch64/**/*.tar.gz\n",
        "artifacts/jamuru-cli-linux-aarch64/**/*.tar.gz.sha256\n",
    ] {
        assert!(
            release.contains(published),
            "the release does not upload {published}"
        );
    }
    assert!(
        release.contains("\"artifacts/jamuru-cli-linux-aarch64/**/*.tar.gz\""),
        "the release does not stop when the arm64 CLI is missing"
    );
}

/// Verifies: REQ-DIST-011
#[test]
fn when_a_release_is_made_the_arm64_app_is_built_on_an_arm64_runner_and_published() {
    let workflow = read(".github/workflows/release.yml");

    let build = job(&workflow, "build");
    assert!(
        build.contains("runner: ubuntu-22.04-arm")
            && build.contains("target: aarch64-unknown-linux-gnu")
            && build.contains("artifact_name: jamuru-linux-arm64"),
        "the release does not build the app on an arm64 runner:\n{build}"
    );
    assert!(
        build.contains("scripts/check-glibc-floor.sh"),
        "the arm64 app is not held to the glibc floor:\n{build}"
    );

    let release = job(&workflow, "release");
    for published in [
        "artifacts/jamuru-linux-arm64/**/*.AppImage\n",
        "artifacts/jamuru-linux-arm64/**/*.AppImage.sig\n",
        "artifacts/jamuru-linux-arm64/**/*.deb\n",
        "artifacts/jamuru-linux-arm64/**/jamuru-linux-arm64-bundled-libraries.txt\n",
    ] {
        assert!(
            release.contains(published),
            "the release does not upload {published}"
        );
    }
    for required in [
        "\"artifacts/jamuru-linux-arm64/**/*.AppImage\"",
        "\"artifacts/jamuru-linux-arm64/**/*.deb\"",
    ] {
        assert!(
            release.contains(required),
            "the release does not stop when {required} is missing"
        );
    }
}

/// Two AppImages with one notice name would collide in the release.
///
/// Verifies: REQ-DIST-011
#[test]
fn when_the_app_is_built_for_two_architectures_each_has_its_own_notice_file_name() {
    let build = job(&read(".github/workflows/release.yml"), "build");

    assert!(build.contains("bundled_libraries: jamuru-linux-bundled-libraries.txt"));
    assert!(build.contains("bundled_libraries: jamuru-linux-arm64-bundled-libraries.txt"));
}

/// Verifies: REQ-DIST-011
#[test]
fn when_a_change_is_pushed_the_arm64_cli_is_built_so_a_dependency_that_stops_building_is_caught_early(
) {
    let build = job(&read(".github/workflows/ci.yml"), "build-cli");

    assert!(build.contains("runner: ubuntu-22.04-arm"));
    assert!(build.contains("target: aarch64-unknown-linux-gnu"));
}

/// The arm64 build is native: asked on another machine it refuses instead of
/// quietly building for that machine.
///
/// Verifies: REQ-DIST-011
#[test]
fn when_the_arm64_cli_is_asked_for_on_a_machine_that_is_not_arm64_the_build_script_refuses() {
    let machine = Command::new("uname").arg("-m").output().unwrap();
    if String::from_utf8_lossy(&machine.stdout).trim() == "aarch64" {
        return;
    }

    let out = Command::new("bash")
        .arg(repo_file("scripts/build-cli.sh"))
        .args(["aarch64-unknown-linux-gnu", "unused-output-directory"])
        .output()
        .expect("bash is needed to run scripts/build-cli.sh");

    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("built natively"));
}
