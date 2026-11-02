//! Guards which releases `scripts/prune-old-releases.sh` deletes once a new
//! release has shipped (REQ-UPD-020): every older release of the same
//! channel, and nothing else. In particular never the new release, the
//! other channel's releases, `beta-channel`, or a git tag.
//!
//! The script runs for real. `gh` is a stand-in that keeps each release as a
//! directory and records what was deleted.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The GitHub releases involved, as `gh` would keep them.
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
    find "$state/releases" -mindepth 1 -maxdepth 1 -type d -exec basename {} \;
    ;;
  view)
    [ -d "$state/releases/$1" ]
    ;;
  download)
    [ -e "$state/releases/$1/latest.json" ] && cat "$state/releases/$1/latest.json"
    ;;
  delete)
    tag=$1
    case " $* " in
      *" --cleanup-tag "*) echo "$tag" >> "$state/tags-deleted" ;;
    esac
    [ -d "$state/releases/$tag" ] || { echo "release not found: $tag" >&2; exit 1; }
    rm -r "$state/releases/$tag"
    echo "$tag" >> "$state/deleted"
    ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        std::fs::create_dir_all(dir.path().join("releases")).unwrap();
        Self { dir }
    }

    /// A release that is already published.
    fn seed(&self, tag: &str) {
        std::fs::create_dir_all(self.dir.path().join("releases").join(tag)).unwrap();
    }

    /// The update manifest of the `beta-channel` release, which points at the
    /// assets of `tag`.
    fn seed_channel_pointing_at(&self, tag: &str) {
        self.seed("beta-channel");
        std::fs::write(
            self.dir.path().join("releases/beta-channel/latest.json"),
            format!(
                r#"{{"platforms":{{"linux-x86_64":{{"url":"https://github.com/koedame/jamuru-client/releases/download/{tag}/jamuru.AppImage"}}}}}}"#
            ),
        )
        .unwrap();
    }

    fn remaining(&self) -> Vec<String> {
        let mut tags: Vec<String> = std::fs::read_dir(self.dir.path().join("releases"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        tags.sort();
        tags
    }

    fn deleted(&self) -> Vec<String> {
        self.lines("deleted")
    }

    fn tags_deleted(&self) -> Vec<String> {
        self.lines("tags-deleted")
    }

    fn lines(&self, file: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join(file))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn prune(&self, tag: &str) -> Output {
        self.run(tag, false)
    }

    fn run(&self, tag: &str, dry_run: bool) -> Output {
        let mut command = Command::new("bash");
        command
            .arg("scripts/prune-old-releases.sh")
            .arg(tag)
            .current_dir(repo_root())
            .env("GH", self.dir.path().join("gh"))
            .env("GITHUB_REPOSITORY", "koedame/jamuru-client");
        if dry_run {
            command.env("DRY_RUN", "1");
        }
        command
            .output()
            .expect("bash is needed to run scripts/prune-old-releases.sh")
    }
}

/// Verifies: REQ-UPD-020
#[test]
fn when_a_newer_beta_is_published_every_older_beta_release_is_deleted() {
    let releases = Releases::new();
    for n in [1, 2, 9, 10, 11] {
        releases.seed(&format!("v0.1.0-beta.{n}"));
    }

    let output = releases.prune("v0.1.0-beta.11");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.remaining(), ["v0.1.0-beta.11"]);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_betas_are_compared_the_number_counts_not_the_text() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.9");
    releases.seed("v0.1.0-beta.10");
    releases.seed("v0.1.0-beta.100");
    releases.seed("v0.2.0-beta.1");

    let output = releases.prune("v0.1.0-beta.100");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        releases.remaining(),
        ["v0.1.0-beta.100", "v0.2.0-beta.1"],
        "beta.9 and beta.10 are older than beta.100; the beta of the next version is newer and stays"
    );
}

/// A run that finishes late must not delete a newer release.
///
/// Verifies: REQ-UPD-020
#[test]
fn when_a_newer_beta_already_exists_it_is_not_deleted() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.5");
    releases.seed("v0.1.0-beta.6");
    releases.seed("v0.1.0-beta.7");

    let output = releases.prune("v0.1.0-beta.6");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.remaining(), ["v0.1.0-beta.6", "v0.1.0-beta.7"]);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_a_beta_is_published_the_beta_channel_release_is_kept() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");
    releases.seed("v0.1.0-beta.2");
    releases.seed_channel_pointing_at("v0.1.0-beta.2");

    let output = releases.prune("v0.1.0-beta.2");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.remaining(), ["beta-channel", "v0.1.0-beta.2"]);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_a_beta_is_published_the_stable_releases_are_kept() {
    let releases = Releases::new();
    releases.seed("v0.1.0");
    releases.seed("v0.1.0-beta.1");
    releases.seed("v0.2.0-beta.1");

    let output = releases.prune("v0.2.0-beta.1");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.remaining(), ["v0.1.0", "v0.2.0-beta.1"]);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_a_stable_release_is_published_the_older_stable_releases_go_and_the_betas_stay() {
    let releases = Releases::new();
    releases.seed("v0.9.0");
    releases.seed("v0.10.0");
    releases.seed("v1.0.0");
    releases.seed("v1.0.0-beta.3");

    let output = releases.prune("v1.0.0");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.remaining(), ["v1.0.0", "v1.0.0-beta.3"]);
}

/// The git tag is what the update drill builds "the release before" from.
///
/// Verifies: REQ-UPD-020
#[test]
fn when_a_release_is_deleted_its_git_tag_is_left_alone() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");
    releases.seed("v0.1.0-beta.2");

    let output = releases.prune("v0.1.0-beta.2");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(releases.deleted(), ["v0.1.0-beta.1"]);
    assert!(releases.tags_deleted().is_empty());
}

/// Verifies: REQ-UPD-020
#[test]
fn when_the_tag_is_not_a_beta_or_a_stable_release_nothing_is_deleted() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");
    releases.seed("v0.1.0-rc.1");
    releases.seed("beta-channel");

    for tag in ["v0.1.0-rc.1", "beta-channel", "main"] {
        let output = releases.prune(tag);
        assert!(output.status.success(), "{}", stderr(&output));
    }

    assert!(releases.deleted().is_empty());
}

/// Nothing may be deleted on the strength of a release that was not made.
///
/// Verifies: REQ-UPD-020
#[test]
fn when_the_new_release_does_not_exist_nothing_is_deleted() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");

    let output = releases.prune("v0.1.0-beta.2");

    assert!(!output.status.success());
    assert!(releases.deleted().is_empty());
}

/// Installed betas fetch their update from where the channel manifest points.
///
/// Verifies: REQ-UPD-020
#[test]
fn when_the_beta_channel_still_points_at_an_older_release_that_release_is_kept() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");
    releases.seed("v0.1.0-beta.2");
    releases.seed("v0.1.0-beta.3");
    releases.seed_channel_pointing_at("v0.1.0-beta.2");

    let output = releases.prune("v0.1.0-beta.3");

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        releases.remaining(),
        ["beta-channel", "v0.1.0-beta.2", "v0.1.0-beta.3"]
    );
}

/// Verifies: REQ-UPD-020
#[test]
fn when_it_is_a_dry_run_nothing_is_deleted_and_the_list_is_printed() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");
    releases.seed("v0.1.0-beta.2");

    let output = releases.run("v0.1.0-beta.2", true);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("would delete release v0.1.0-beta.1"));
    assert!(releases.deleted().is_empty());
    assert_eq!(releases.remaining(), ["v0.1.0-beta.1", "v0.1.0-beta.2"]);
}

/// Verifies: REQ-UPD-020
#[test]
fn when_there_is_no_older_release_nothing_happens() {
    let releases = Releases::new();
    releases.seed("v0.1.0-beta.1");

    let output = releases.prune("v0.1.0-beta.1");

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(releases.deleted().is_empty());
}
