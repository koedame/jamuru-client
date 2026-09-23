//! A panic leaves the place it happened, and the next launch reports it.
//!
//! In its own test binary because the panic hook is process-wide: one test
//! here installs it and panics on purpose.

use std::sync::Arc;

use jamjam::telemetry::{EventBody, NoTransport, UsageReporter};

/// Verifies: REQ-TEL-008
#[test]
fn when_the_app_panics_only_the_place_is_kept_and_the_next_launch_sends_it() {
    let dir = tempfile::tempdir().unwrap();
    let crash_file = dir.path().join("crash.json");
    let first = UsageReporter::new(
        Some(dir.path().to_path_buf()),
        "0.1.2",
        Arc::new(NoTransport),
        false,
    );
    first.install_panic_hook();

    // Reporting is off: a panic leaves nothing behind.
    let _ = std::thread::spawn(|| panic!("room ZZTOP99 refused 10.0.0.5")).join();
    assert!(
        !crash_file.exists(),
        "a crash was kept while reporting is off"
    );

    // Reporting is on: it leaves where, and not what.
    first.set_enabled(true);
    first.record(EventBody::SessionStart(jamjam::telemetry::SessionStart {
        mode: jamjam::telemetry::SessionMode::Join,
    }));
    let crashed_launch: serde_json::Value =
        serde_json::from_str(first.preview_ndjson().lines().next().unwrap()).unwrap();
    let _ = std::thread::spawn(|| panic!("room ZZTOP99 refused 10.0.0.5")).join();

    let kept = std::fs::read_to_string(&crash_file).expect("no crash record was kept");
    assert!(kept.contains("usage_crash_test.rs"), "{kept}");
    assert!(
        !kept.contains("ZZTOP99") && !kept.contains("10.0.0.5"),
        "{kept}"
    );

    // The next launch sends it, under the launch that crashed.
    let next = UsageReporter::new(
        Some(dir.path().to_path_buf()),
        "0.1.3",
        Arc::new(NoTransport),
        true,
    );
    next.report_previous_crash();

    let sent: serde_json::Value =
        serde_json::from_str(next.preview_ndjson().lines().next().unwrap()).unwrap();
    assert_eq!(sent["event"], "crash");
    assert_eq!(sent["launch_id"], crashed_launch["launch_id"]);
    assert_eq!(sent["app_version"], "0.1.2");
    assert!(sent["file"]
        .as_str()
        .unwrap()
        .ends_with("usage_crash_test.rs"));
    assert!(sent["line"].as_u64().unwrap() > 0);
    assert!(!next.preview_ndjson().contains("ZZTOP99"));
    assert!(!crash_file.exists());
}
