//! Guards which assets `scripts/prune-beta-packages.sh` deletes from a beta
//! release once a newer beta has shipped, and which it must never touch
//! (REQ-UPD-020): the updater and rollback (REQ-UPD-004, REQ-UPD-019) chain
//! stays intact, only the install-only packages disappear.
//!
//! The script runs for real. `gh` is a stand-in that keeps each release's
//! assets as files in its own directory.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The GitHub releases involved, as `gh` would keep them: each release has a
/// set of named assets.
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
sub=$2
shift 2
case "$sub" in
  list)
    if [ -d "$state/releases" ]; then
      find "$state/releases" -mindepth 1 -maxdepth 1 -type d -exec basename {} \;
    fi
    ;;
  view)
    tag=$1
    dir="$state/releases/$tag/assets"
    if [ -d "$dir" ]; then
      find "$dir" -mindepth 1 -maxdepth 1 -exec basename {} \;
    fi
    ;;
  delete-asset)
    tag=$1
    asset=$2
    file="$state/releases/$tag/assets/$asset"
    if [ ! -e "$file" ]; then
      echo "asset not found: $tag/$asset" >&2
      exit 1
    fi
    rm "$file"
    echo "$tag $asset" >> "$state/deleted"
    ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        Self { dir }
    }

    /// A release with the given assets already published.
    fn seed(&self, tag: &str, assets: &[&str]) {
        let assets_dir = self.dir.path().join("releases").join(tag).join("assets");
        std::fs::create_dir_all(&assets_dir).unwrap();
        for asset in assets {
            std::fs::write(assets_dir.join(asset), "").unwrap();
        }
    }

    fn assets_of(&self, tag: &str) -> Vec<String> {
        let assets_dir = self.dir.path().join("releases").join(tag).join("assets");
        let Ok(entries) = std::fs::read_dir(&assets_dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn deleted(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("deleted"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn prune(&self, tag: &str) -> Output {
        Command::new("bash")
            .arg("scripts/prune-beta-packages.sh")
            .arg(tag)
            .current_dir(repo_root())
            .env("GH", self.dir.path().join("gh"))
            .env("GITHUB_REPOSITORY", "koedame/jamjam-client")
            .output()
            .expect("bash is needed to run scripts/prune-beta-packages.sh")
    }
}

/// The full set of assets a beta release carries: the install-only packages
/// this script deletes, and the updater/rollback chain it must never touch.
const ALL_BETA_ASSETS: &[&str] = &[
    "jamjam_x64.dmg",
    "jamjam_aarch64.dmg",
    "jamjam_x64.app.tar.gz",
    "jamjam_x64.app.tar.gz.sig",
    "jamjam_aarch64.app.tar.gz",
    "jamjam_aarch64.app.tar.gz.sig",
    "jamjam.AppImage",
    "jamjam.AppImage.sig",
    "jamjam.deb",
    "jamjam-setup.exe",
    "jamjam-setup.exe.sig",
    "jamjam.msi",
    "jamjam.msi.sig",
    "latest.json",
];

const UPDATER_AND_ROLLBACK_ASSETS: &[&str] = &[
    "jamjam_x64.app.tar.gz",
    "jamjam_x64.app.tar.gz.sig",
    "jamjam_aarch64.app.tar.gz",
    "jamjam_aarch64.app.tar.gz.sig",
    "jamjam.AppImage",
    "jamjam.AppImage.sig",
    "jamjam-setup.exe",
    "jamjam-setup.exe.sig",
    "latest.json",
];

/// Verifies: REQ-UPD-020
#[test]
fn when_a_newer_beta_is_published_the_older_betas_lose_their_install_only_packages() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.9", ALL_BETA_ASSETS);
    releases.seed("v0.1.0-beta.10", ALL_BETA_ASSETS);

    let output = releases.prune("v0.1.0-beta.10");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        releases.assets_of("v0.1.0-beta.9"),
        {
            let mut kept: Vec<String> = UPDATER_AND_ROLLBACK_ASSETS
                .iter()
                .map(|s| s.to_string())
                .collect();
            kept.sort();
            kept
        },
        "the install-only packages of the older beta should be gone, everything the updater and rollback need should remain"
    );
}

/// Verifies: REQ-UPD-020
#[test]
fn when_a_beta_is_the_one_just_published_its_own_packages_are_kept() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.10", ALL_BETA_ASSETS);

    let output = releases.prune("v0.1.0-beta.10");

    assert!(output.status.success(), "{}", stderr(&output));
    let mut expected: Vec<String> = ALL_BETA_ASSETS.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(releases.assets_of("v0.1.0-beta.10"), expected);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_a_release_is_not_a_beta_nothing_is_pruned() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.9", ALL_BETA_ASSETS);

    let output = releases.prune("v0.1.0");

    assert!(output.status.success(), "{}", stderr(&output));
    let mut expected: Vec<String> = ALL_BETA_ASSETS.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(releases.assets_of("v0.1.0-beta.9"), expected);
    assert!(releases.deleted().is_empty());
}

/// A plain release (`vX.Y.Z`, no `-beta.N`) is never a beta, so it is never
/// listed as one to prune from, even once a new beta exists.
///
/// Verifies: REQ-UPD-020
#[test]
fn when_a_full_release_exists_its_packages_are_kept() {
    let releases = Releases::new();
    releases.seed("v0.1.0", ALL_BETA_ASSETS);
    releases.seed("v0.2.0-beta.1", ALL_BETA_ASSETS);

    let output = releases.prune("v0.2.0-beta.1");

    assert!(output.status.success(), "{}", stderr(&output));
    let mut expected: Vec<String> = ALL_BETA_ASSETS.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(releases.assets_of("v0.1.0"), expected);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_there_are_no_older_betas_nothing_happens() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1", ALL_BETA_ASSETS);

    let output = releases.prune("v0.1.0-beta.1");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(releases.deleted().is_empty());
}
