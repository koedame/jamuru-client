//! Guards the 32-bit ARM Linux build of the app (ADR-128, REQ-DIST-012): that
//! every release carries its AppImage and deb, that nothing is published
//! without them, and that the release checks cover it the way they cover the
//! x64 app.
//!
//! Cross-building and bundling need the armhf toolchain and qemu, so those are
//! done by the `build` job itself. What can be pinned on every `cargo test` is
//! the shape of the workflows and of the update manifest.

use std::path::PathBuf;

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

/// Verifies: REQ-DIST-012
#[test]
fn when_a_release_is_made_the_armv7_app_goes_through_the_same_build_job_as_the_x64_app() {
    let release = read(".github/workflows/release.yml");
    let build = job(&release, "build");

    assert!(
        build.contains(
            "target: armv7-unknown-linux-gnueabihf\n            artifact_name: jamuru-linux-armv7"
        ),
        "the build matrix has no armv7 app:\n{build}"
    );
    assert!(
        build.contains("cross: armhf"),
        "the armv7 row does not say it is cross-built"
    );
    assert!(
        build.contains("scripts/install-armhf-cross-deps.sh gui"),
        "the armhf libraries of the window are not installed"
    );
    assert!(
        build.contains("qemu-user-static") && build.contains("scripts/register-armhf-binfmt.sh"),
        "the AppImage bundler cannot run the armhf AppImage tools"
    );
    assert!(
        build.contains("scripts/armhf-ldd.sh"),
        "linuxdeploy bundles none of the armv7 app's libraries with the amd64 ldd"
    );
    assert!(
        build.contains("grep -q 'libwebkit2gtk-4.1\\.so'"),
        "an AppImage without WebKitGTK is released"
    );
    assert!(
        build.contains("OBJDUMP: ${{ matrix.cross == 'armhf' && 'arm-linux-gnueabihf-objdump'"),
        "the glibc floor of the armv7 app is read with the amd64 objdump"
    );
    assert!(
        build.contains("PACKAGE_ARCH: ${{ matrix.cross == 'armhf' && 'armhf'"),
        "the bundled-libraries notice of the armv7 AppImage lists the amd64 packages"
    );
}

/// Verifies: REQ-DIST-012
#[test]
fn when_a_release_is_made_nothing_is_published_without_the_armv7_app() {
    let release = job(&read(".github/workflows/release.yml"), "release");

    for required in [
        "[\"Linux armv7 AppImage\"]=\"artifacts/jamuru-linux-armv7/**/*.AppImage\"",
        "[\"Linux armv7 deb\"]=\"artifacts/jamuru-linux-armv7/**/*.deb\"",
    ] {
        assert!(
            release.contains(required),
            "the release does not stop when {required} is missing"
        );
    }
    for published in [
        "artifacts/jamuru-linux-armv7/**/*.AppImage\n",
        "artifacts/jamuru-linux-armv7/**/*.AppImage.sig\n",
        "artifacts/jamuru-linux-armv7/**/jamuru-linux-armv7-bundled-libraries.txt\n",
        "artifacts/jamuru-linux-armv7/**/*.deb\n",
    ] {
        assert!(
            release.contains(published),
            "the release does not upload {published}"
        );
    }
}

/// Without its entry in the manifest an installed armv7 app would never be
/// offered an update.
///
/// Verifies: REQ-DIST-012
#[test]
fn when_the_update_manifest_is_made_it_names_the_armv7_appimage() {
    let script = read("scripts/make-update-manifest.sh");

    assert!(script.contains(
        "add_platform linux-armv7-appimage \"$artifacts/jamuru-linux-armv7/**/*.AppImage\""
    ));
}

/// The two libraries notices (x64 and armv7) are both named in the notice that
/// goes into the installers, or the armv7 one would be published unannounced.
///
/// Verifies: REQ-DIST-012
#[test]
fn when_the_installers_name_the_bundled_libraries_notice_both_architectures_are_named() {
    let notice = read("src-tauri/resources/LICENSES.txt");

    assert!(notice.contains("jamuru-linux-bundled-libraries.txt"));
    assert!(notice.contains("jamuru-linux-armv7-bundled-libraries.txt"));
}

/// Verifies: REQ-DIST-012
#[test]
fn when_the_install_script_is_given_an_unknown_argument_it_refuses_before_changing_the_system() {
    let out = std::process::Command::new("bash")
        .arg(repo_file("scripts/install-armhf-cross-deps.sh"))
        .arg("bogus")
        .output()
        .expect("bash is needed to run scripts/install-armhf-cross-deps.sh");

    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage"));
}

/// Verifies: REQ-DIST-012
#[test]
fn when_ldd_is_given_a_native_program_it_lists_the_libraries_as_the_real_ldd_does() {
    let out = std::process::Command::new("bash")
        .arg(repo_file("scripts/armhf-ldd.sh"))
        .arg("/bin/sh")
        .output()
        .expect("bash is needed to run scripts/armhf-ldd.sh");

    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("libc.so.6"));
}
