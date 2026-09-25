//! Guards that the GUI E2E control channel cannot ship (ADR-025).
//!
//! The channel can read whatever the app is displaying and can click and
//! type on the user's behalf. It is confined by a cargo feature that must
//! stay out of the default set - if it ever lands there, every release build
//! would carry it. This test runs on every `cargo test`, with no feature
//! flags, precisely so nobody has to remember to check.
//!
//! Reading the manifest rather than scanning a built artifact is deliberate:
//! this must be cheap enough to run always, and the manifest is what decides
//! the outcome.

use std::path::PathBuf;

fn tauri_manifest() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src-tauri/Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {}", path.display(), e))
}

/// Returns the entries of `[features] default = [...]`.
fn default_features(manifest: &str) -> Vec<String> {
    let features = manifest
        .split_once("[features]")
        .unwrap_or_else(|| panic!("src-tauri/Cargo.toml has no [features] section"))
        .1;
    let line = features
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("default"))
        .unwrap_or_else(|| panic!("src-tauri/Cargo.toml has no `default` feature"));
    let list = line
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(inner, _)| inner)
        .unwrap_or_else(|| panic!("could not parse the default feature list from {:?}", line));

    list.split(',')
        .map(|entry| entry.trim().trim_matches('"').to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Verifies: REQ-GUI-003
#[test]
fn e2e_control_is_not_a_default_feature() {
    let defaults = default_features(&tauri_manifest());

    assert!(
        !defaults.iter().any(|f| f == "e2e-control"),
        "e2e-control is enabled by default, so it would ship in release builds. \
         Default features: {:?}",
        defaults
    );
}

/// Remote debugging and the methods that read and drive the screen exist only
/// in beta and E2E builds (ADR-044). A release build must carry none of them,
/// so none may be in the default set.
///
/// Verifies: REQ-RMT-020
#[test]
fn the_debug_features_are_not_default_features() {
    let defaults = default_features(&tauri_manifest());

    for feature in ["e2e-control", "debug-tools", "debug-remote"] {
        assert!(
            !defaults.iter().any(|f| f == feature),
            "{} is enabled by default, so it would ship in release builds. \
             Default features: {:?}",
            feature,
            defaults
        );
    }
}

/// The guard above is only meaningful if it is reading the real list; a
/// parser that silently returned nothing would let anything through.
///
/// Verifies: REQ-GUI-003
#[test]
fn the_guard_actually_reads_the_default_feature_list() {
    let defaults = default_features(&tauri_manifest());

    assert!(
        defaults.iter().any(|f| f == "custom-protocol"),
        "expected custom-protocol among the default features, got {:?} - \
         the guard is not parsing the manifest correctly",
        defaults
    );
}

/// The feature has to exist and be declared separately, otherwise a build
/// with `--features e2e-control` would fail and the GUI suite could never
/// run.
#[test]
fn e2e_control_is_declared_as_an_opt_in_feature() {
    let manifest = tauri_manifest();
    let features = manifest
        .split_once("[features]")
        .expect("src-tauri/Cargo.toml has no [features] section")
        .1;

    assert!(
        features
            .lines()
            .map(str::trim)
            .any(|line| line.starts_with("e2e-control")),
        "e2e-control is not declared in [features]"
    );
}

/// A parser check with a hand-written manifest, so a change to the real one
/// cannot quietly make the assertions above vacuous.
#[test]
fn default_features_are_parsed_from_the_declared_list() {
    let manifest = r#"
[package]
name = "example"

[features]
default = ["custom-protocol", "other"]
custom-protocol = []
e2e-control = ["dep:axum"]
"#;

    assert_eq!(
        default_features(manifest),
        vec!["custom-protocol".to_string(), "other".to_string()]
    );
}
