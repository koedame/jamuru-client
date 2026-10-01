//! Guards `scripts/appimage-bundled-libraries.py` (REQ-DIST-008): the notice
//! for the system libraries (mostly LGPL) inside the Linux AppImage.
//!
//! The script runs for real, on a directory laid out like an unpacked
//! AppImage. `dpkg-query` and `/usr/share` are stand-ins (`DPKG_QUERY`,
//! `SHARE_DIR`) so the test does not depend on the machine's packages.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const STUB_DPKG_QUERY: &str = r#"#!/bin/sh
case "$1" in
  -S)
    case "$2" in
      "*/libfoo.so.1") echo "libfoo1:amd64: /usr/lib/x86_64-linux-gnu/libfoo.so.1" ;;
      "*/libbar.so.2") echo "libbar2:amd64: /usr/lib/x86_64-linux-gnu/libbar.so.2" ;;
      "*/libshared.so.3") echo "libfoo1:amd64, libbar2:amd64: /usr/lib/x86_64-linux-gnu/libshared.so.3" ;;
      "*/libstray.so") echo "stray:amd64: /opt/stray/libstray.so" ;;
      *) echo "dpkg-query: no path found matching pattern $2" >&2; exit 1 ;;
    esac ;;
  -W)
    case "$4" in
      libfoo1) printf '1.2-3\tfoo\t1.2-3' ;;
      libbar2) printf '4.5-6\tbar-src\t4.5-6build1' ;;
      *) exit 1 ;;
    esac ;;
esac
"#;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn write(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// An unpacked AppImage with these libraries (real ELF magic, no code).
fn appimage(dir: &Path, libraries: &[&str]) -> PathBuf {
    let root = dir.join("squashfs-root");
    for name in libraries {
        write(&root.join("usr/lib").join(name), b"\x7fELF fixture");
    }
    // A text file whose name looks like a library is not one.
    write(&root.join("usr/share/libnot.so.1"), b"just text");
    root
}

fn build_system(dir: &Path) -> PathBuf {
    let share = dir.join("share");
    write(
        &share.join("doc/libfoo1/copyright"),
        b"Files: *\nCopyright: 2001 Foo Authors\nLicense: LGPL-2.1+\n On Debian systems, see /usr/share/common-licenses/LGPL-2.1\n",
    );
    write(
        &share.join("doc/libbar2/copyright"),
        b"Copyright 2010 Bar Authors. Permission is hereby granted ...\n",
    );
    write(
        &share.join("common-licenses/LGPL-2.1"),
        b"GNU LESSER GENERAL PUBLIC LICENSE fixture text",
    );
    let stub = dir.join("dpkg-query");
    write(&stub, STUB_DPKG_QUERY.as_bytes());
    let status = Command::new("chmod").arg("+x").arg(&stub).status().unwrap();
    assert!(status.success());
    dir.to_path_buf()
}

fn run(dir: &Path, root: &Path) -> (Output, String) {
    build_system(dir);
    let out = dir.join("notice.txt");
    let output = Command::new("python3")
        .arg("scripts/appimage-bundled-libraries.py")
        .arg(root)
        .arg(&out)
        .current_dir(repo_root())
        .env("DPKG_QUERY", dir.join("dpkg-query"))
        .env("SHARE_DIR", dir.join("share"))
        .output()
        .expect("python3 is needed to run scripts/appimage-bundled-libraries.py");
    let text = std::fs::read_to_string(&out).unwrap_or_default();
    (output, text)
}

/// Verifies: REQ-DIST-008
#[test]
fn when_every_library_belongs_to_a_package_the_notice_names_each_with_its_copyright_and_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = appimage(dir.path(), &["libfoo.so.1", "libbar.so.2"]);

    let (output, notice) = run(dir.path(), &root);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(notice.contains("libfoo1 1.2-3"));
    assert!(notice.contains("https://launchpad.net/ubuntu/+source/foo/1.2-3"));
    assert!(notice.contains("https://launchpad.net/ubuntu/+source/bar-src/4.5-6build1"));
    assert!(notice.contains("Copyright: 2001 Foo Authors"));
    assert!(notice.contains("Copyright 2010 Bar Authors"));
    assert!(
        notice.contains("GNU LESSER GENERAL PUBLIC LICENSE fixture text"),
        "the license text a copyright file refers to is part of the notice"
    );
    assert!(!notice.contains("libnot"), "a text file is not a library");
}

/// Verifies: REQ-DIST-008
#[test]
fn when_a_library_belongs_to_two_packages_both_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let root = appimage(dir.path(), &["libshared.so.3"]);

    let (output, notice) = run(dir.path(), &root);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(notice.contains("libfoo1 1.2-3"));
    assert!(notice.contains("libbar2 4.5-6"));
}

/// Verifies: REQ-DIST-008
#[test]
fn when_a_library_matches_no_package_the_script_fails_and_the_notice_says_so() {
    let dir = tempfile::tempdir().unwrap();
    // libmissing: unknown to dpkg. libstray: owned by a package, but not under /usr/lib.
    let root = appimage(
        dir.path(),
        &["libfoo.so.1", "libmissing.so.9", "libstray.so"],
    );

    let (output, notice) = run(dir.path(), &root);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("libmissing.so.9") && stderr.contains("libstray.so"),
        "{stderr}"
    );
    assert!(notice.contains("this notice is incomplete"));
    assert!(notice.contains("libmissing.so.9"));
}
