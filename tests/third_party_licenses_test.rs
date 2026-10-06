//! Guards the third-party license notices the installers carry (REQ-DIST-007).
//!
//! MIT, Apache-2.0, BSD and MPL-2.0 code may be shipped only with its
//! copyright notice and license text (MPL-2.0 also with where its source is).
//! The notices are generated from the lock files by
//! `scripts/third-party-licenses.py`, which was introduced after the
//! hand-written table had fallen behind the real dependencies.
//!
//! The generator itself needs cargo-about and minutes; `--check` only compares
//! the hash of every input it was built from, so it runs here on every
//! `cargo test`. The dependency licenses themselves are checked by
//! `cargo deny check licenses` (deny.toml) in CI.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(file: &str) -> String {
    let path = repo_root().join(file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {}", path.display(), e))
}

/// Verifies: REQ-DIST-007
#[test]
fn the_notices_are_the_ones_the_current_lock_files_produce() {
    let output = Command::new("python3")
        .arg("scripts/third-party-licenses.py")
        .arg("--check")
        .current_dir(repo_root())
        .output()
        .expect("python3 is needed to run scripts/third-party-licenses.py");

    assert!(
        output.status.success(),
        "the license notices are out of date; run scripts/third-party-licenses.py\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Verifies: REQ-DIST-007
#[test]
fn the_installers_bundle_the_generated_notices() {
    let conf: serde_json::Value = serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    let resources = conf["bundle"]["resources"].as_array().unwrap();
    assert!(
        resources.iter().any(|r| r == "resources/LICENSES.txt"),
        "src-tauri/tauri.conf.json no longer bundles resources/LICENSES.txt"
    );

    let bundled = read("src-tauri/resources/LICENSES.txt");
    assert!(bundled.contains("jamuru Source Available License"));
    // Rust crates, npm packages and the code copied into the source are all in it.
    assert!(bundled.contains("Mozilla Public License Version 2.0"));
    assert!(bundled.contains("SIL OPEN FONT LICENSE Version 1.1"));
    assert!(bundled.contains("Lucide"));
}

/// MPL-2.0 asks that anyone who gets the binary can find the source of the
/// covered files.
///
/// Verifies: REQ-DIST-007
#[test]
fn every_mpl_licensed_crate_comes_with_where_its_source_is() {
    let lock = read("src-tauri/Cargo.lock");
    let notices = read("THIRD_PARTY_LICENSES.md");

    for crate_name in [
        "cssparser",
        "cssparser-macros",
        "dtoa-short",
        "option-ext",
        "selectors",
    ] {
        assert!(
            lock.contains(&format!("name = \"{crate_name}\"")),
            "{crate_name} is no longer a dependency; update this list"
        );
        assert!(
            notices.contains(&format!("https://crates.io/crates/{crate_name}/")),
            "{crate_name} (MPL-2.0) has no source location in THIRD_PARTY_LICENSES.md"
        );
    }
}
