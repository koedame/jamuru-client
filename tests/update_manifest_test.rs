//! Guards `scripts/make-update-manifest.sh` (ADR-041): the file installed apps
//! read to learn about a release must never be written for a release that
//! would not update anyone.
//!
//! The script runs for real, on fixture bundles laid out the way
//! `actions/download-artifact` lays them out. The signatures are fixtures too:
//! the script reads only the trusted comment inside them (the apps' own
//! verification of the signature is the updater plugin's).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BUNDLES: [(&str, &str); 5] = [
    ("jamjam-macos-arm64/macos", "jamjam_aarch64.app.tar.gz"),
    ("jamjam-macos-x64/macos", "jamjam_x64.app.tar.gz"),
    ("jamjam-linux-x64/bundle/appimage", "jamjam_amd64.AppImage"),
    ("jamjam-windows-x64/bundle/nsis", "jamjam_x64-setup.exe"),
    ("jamjam-windows-x64/bundle/msi", "jamjam_x64_en-US.msi"),
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn built_version() -> String {
    let conf = std::fs::read_to_string(repo_root().join("src-tauri/tauri.conf.json")).unwrap();
    let conf: serde_json::Value = serde_json::from_str(&conf).unwrap();
    conf["version"].as_str().unwrap().to_string()
}

/// A signature file as the Tauri CLI writes it: the base64 of a minisign
/// signature whose trusted comment names the version it was signed for.
fn signature(trusted_comment: &str) -> String {
    let minisign = format!(
        "untrusted comment: signature from tauri secret key\nRUQfixture\ntrusted comment: {trusted_comment}\nfixture\n"
    );
    let encoded = Command::new("base64")
        .arg("-w0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(minisign.as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();
    String::from_utf8(encoded.stdout).unwrap()
}

/// Lays out every platform's bundle and signature. `signed_for` is the version
/// each signature names.
fn artifacts(dir: &Path, signed_for: &str) {
    for (folder, file) in BUNDLES {
        let folder = dir.join(folder);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(file), b"bundle").unwrap();
        std::fs::write(
            folder.join(format!("{file}.sig")),
            signature(&format!("timestamp:1\tfile:{file}\tversion:{signed_for}")),
        )
        .unwrap();
    }
}

fn run(tag: &str, dir: &Path) -> Output {
    Command::new("bash")
        .arg("scripts/make-update-manifest.sh")
        .arg(tag)
        .arg(dir)
        .current_dir(repo_root())
        .env("GITHUB_REPOSITORY", "koedame/jamjam-client")
        .output()
        .expect("bash is needed to run scripts/make-update-manifest.sh")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Verifies: REQ-UPD-009
#[test]
fn when_every_platform_is_signed_for_the_built_version_the_manifest_lists_them_all() {
    let version = built_version();
    let dir = tempfile::tempdir().unwrap();
    artifacts(dir.path(), &version);

    let output = run(&format!("v{version}"), dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(manifest["version"], version.as_str());
    let platforms = manifest["platforms"].as_object().unwrap();
    let mut keys: Vec<&String> = platforms.keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "darwin-aarch64-app",
            "darwin-x86_64-app",
            "linux-x86_64-appimage",
            "windows-x86_64-msi",
            "windows-x86_64-nsis",
        ]
    );
    assert_eq!(
        platforms["darwin-aarch64-app"]["url"],
        format!(
            "https://github.com/koedame/jamjam-client/releases/download/v{version}/jamjam_aarch64.app.tar.gz"
        )
    );
    // The apps decode this exact text, so it is the file's content, untouched.
    let on_disk = std::fs::read_to_string(
        dir.path()
            .join("jamjam-macos-arm64/macos/jamjam_aarch64.app.tar.gz.sig"),
    )
    .unwrap();
    assert_eq!(
        platforms["darwin-aarch64-app"]["signature"],
        on_disk.trim_end()
    );
}

/// Verifies: REQ-UPD-010
#[test]
fn when_the_tag_is_another_version_than_the_one_built_no_manifest_is_written() {
    let version = built_version();
    let dir = tempfile::tempdir().unwrap();
    artifacts(dir.path(), &version);

    let output = run("v99.0.0", dir.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("v99.0.0"), "{}", stderr(&output));
}

/// A beta is tagged `v<built version>-beta.<n>`: the same version, so the
/// same check passes.
///
/// Verifies: REQ-UPD-010
#[test]
fn when_the_tag_is_a_beta_of_the_built_version_the_manifest_is_written() {
    let version = built_version();
    let dir = tempfile::tempdir().unwrap();
    artifacts(dir.path(), &version);

    let output = run(&format!("v{version}-beta.7"), dir.path());

    assert!(output.status.success(), "{}", stderr(&output));
}

/// Verifies: REQ-UPD-011
#[test]
fn when_a_signature_names_another_version_no_manifest_is_written() {
    let version = built_version();
    let dir = tempfile::tempdir().unwrap();
    artifacts(dir.path(), "0.0.1");

    let output = run(&format!("v{version}"), dir.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains("was not signed for version"),
        "{}",
        stderr(&output)
    );
}

/// Verifies: REQ-UPD-011
#[test]
fn when_a_signature_names_no_version_no_manifest_is_written() {
    let version = built_version();
    let dir = tempfile::tempdir().unwrap();
    artifacts(dir.path(), &version);
    std::fs::write(
        dir.path()
            .join("jamjam-linux-x64/bundle/appimage/jamjam_amd64.AppImage.sig"),
        signature("timestamp:1\tfile:jamjam_amd64.AppImage"),
    )
    .unwrap();

    let output = run(&format!("v{version}"), dir.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// Verifies: REQ-UPD-011
#[test]
fn when_a_platforms_bundle_is_missing_no_manifest_is_written() {
    let version = built_version();
    let dir = tempfile::tempdir().unwrap();
    artifacts(dir.path(), &version);
    std::fs::remove_file(
        dir.path()
            .join("jamjam-windows-x64/bundle/msi/jamjam_x64_en-US.msi.sig"),
    )
    .unwrap();

    let output = run(&format!("v{version}"), dir.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("msi"), "{}", stderr(&output));
}
