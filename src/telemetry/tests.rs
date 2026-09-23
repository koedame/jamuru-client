use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::*;
use crate::config::{config_dir, AppConfig, ConnectionHistoryEntry};

// -- helpers ------------------------------------------------------------

/// Takes every batch and says whether the "server" accepted it.
struct RecordingTransport {
    sent: Mutex<Vec<String>>,
    accept: AtomicBool,
}

impl RecordingTransport {
    fn new(accept: bool) -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::new(Vec::new()),
            accept: AtomicBool::new(accept),
        })
    }

    fn batches(&self) -> Vec<String> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for RecordingTransport {
    fn send(&self, body: Vec<u8>) -> Delivery<'_> {
        Box::pin(async move {
            self.sent
                .lock()
                .unwrap()
                .push(String::from_utf8(body).unwrap());
            self.accept.load(Ordering::SeqCst)
        })
    }
}

fn reporter(dir: &Path, enabled: bool) -> (UsageReporter, Arc<RecordingTransport>) {
    let transport = RecordingTransport::new(true);
    let reporter = UsageReporter::new(Some(dir.to_path_buf()), "0.1.2", transport.clone(), enabled);
    (reporter, transport)
}

fn lines(ndjson: &str) -> Vec<Value> {
    ndjson
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn schema() -> jsonschema::Validator {
    jsonschema::validator_for(&serde_json::from_str(SCHEMA_JSON).unwrap()).unwrap()
}

fn assert_valid(line: &Value) {
    if let Err(e) = schema().validate(line) {
        panic!("{line} does not satisfy schema.json: {e}");
    }
}

fn is_valid(line: &Value) -> bool {
    schema().is_valid(line)
}

fn device(name: &str) -> Device {
    Device {
        name: name.to_string(),
        kind: DeviceKind::Usb,
        channels: 2,
        sample_rates: vec![44100, 48000, 96000],
        min_buffer_frames: Some(32),
        is_default: true,
    }
}

fn app_start() -> EventBody {
    EventBody::AppStart(AppStart {
        os_version: Some("14.6".into()),
        cpu_cores: Some(8),
        ram_gb: Some(16),
        webview_version: Some("128".into()),
        language: Some("ja".into()),
        audio_host: Some(AudioHost::Coreaudio),
        settings: settings::settings_for_report(&AppConfig::default()),
    })
}

fn audio_env() -> EventBody {
    EventBody::AudioEnv(AudioEnv {
        input: Some(device("Scarlett 2i2 USB")),
        output: None,
    })
}

// -- off by default -----------------------------------------------------

/// Verifies: REQ-TEL-001
#[tokio::test]
async fn when_usage_reporting_is_off_nothing_is_collected_or_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), false);

    reporter.record(app_start());
    reporter.record_error(Component::Signaling, ErrorCode::Timeout);
    reporter.begin_session(SessionMode::Create);
    reporter.end_session(EndReason::Left);
    reporter.flush().await;

    assert!(!reporter.is_enabled());
    assert_eq!(reporter.pending_count(), 0);
    assert_eq!(reporter.preview_ndjson(), "");
    assert!(transport.batches().is_empty(), "something was sent");
}

/// Verifies: REQ-TEL-001
#[test]
fn when_usage_reporting_is_off_no_install_id_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), false);
    reporter.record(app_start());

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// Verifies: REQ-TEL-001
#[test]
fn when_the_user_config_defaults_are_used_usage_reporting_is_off() {
    assert!(!AppConfig::default().usage_reporting);

    // A settings file from before the setting existed.
    let config: AppConfig = toml::from_str("buffer_size = 128").unwrap();
    assert!(!config.usage_reporting);
}

/// Verifies: REQ-TEL-001
#[test]
fn when_there_is_no_place_to_keep_the_install_id_reporting_stays_off() {
    let reporter = UsageReporter::new(None, "0.1.2", RecordingTransport::new(true), true);

    assert!(!reporter.is_enabled());
}

// -- install ID ---------------------------------------------------------

fn install_id_of(reporter: &UsageReporter) -> String {
    reporter.record(app_start());
    let body = reporter.preview_ndjson();
    lines(&body)[0]["install_id"].as_str().unwrap().to_string()
}

/// Verifies: REQ-TEL-002
#[test]
fn when_reporting_is_turned_on_a_random_16_byte_install_id_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), false);

    reporter.set_enabled(true);

    let id = install_id_of(&reporter);
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    let stored = std::fs::read_to_string(dir.path().join("install_id")).unwrap();
    assert_eq!(stored.trim(), id);
}

/// Verifies: REQ-TEL-002
#[test]
fn when_the_app_starts_with_reporting_still_on_the_same_install_id_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (first, _) = reporter(dir.path(), true);
    let (second, _) = reporter(dir.path(), true);

    assert_eq!(install_id_of(&first), install_id_of(&second));
}

/// Verifies: REQ-TEL-002
#[test]
fn when_the_install_id_is_kept_it_is_not_in_the_settings_file_directory() {
    let usage_dir = state_dir().expect("a data directory");

    assert_ne!(Some(usage_dir.clone()), config_dir());
    assert!(
        usage_dir.file_name().is_some_and(|name| name == "usage"),
        "{usage_dir:?}"
    );
}

/// Verifies: REQ-TEL-002
#[test]
fn when_reporting_is_turned_off_the_unsent_events_and_the_install_id_are_thrown_away() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    let before = install_id_of(&reporter);
    reporter.record_error(Component::Ice, ErrorCode::Timeout);
    assert!(reporter.pending_count() > 0);

    reporter.set_enabled(false);

    assert_eq!(reporter.pending_count(), 0);
    assert_eq!(reporter.preview_ndjson(), "");
    assert!(!dir.path().join("install_id").exists());

    // Turned on again, it is somebody new: the ID is not the old one.
    reporter.set_enabled(true);
    assert_ne!(install_id_of(&reporter), before);
}

/// Verifies: REQ-TEL-002
#[test]
fn when_the_app_starts_with_reporting_off_a_leftover_install_id_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (on, _) = reporter(dir.path(), true);
    on.record(app_start());
    assert!(dir.path().join("install_id").exists());

    let (_off, _) = reporter(dir.path(), false);

    assert!(!dir.path().join("install_id").exists());
}

// -- schema -------------------------------------------------------------

fn every_event() -> Vec<EventBody> {
    vec![
        app_start(),
        EventBody::AppStart(AppStart::default()),
        audio_env(),
        EventBody::AudioEnv(AudioEnv {
            input: None,
            output: Some(Device {
                min_buffer_frames: None,
                kind: DeviceKind::Bluetooth,
                ..device("太郎の AirPods")
            }),
        }),
        EventBody::SessionStart(SessionStart {
            mode: SessionMode::Join,
        }),
        EventBody::SessionEnd({
            let mut tally = SessionTally::new();
            tally.sample(Some(30.5), 0.004);
            tally.set_participants(3);
            tally.add_xruns(2);
            tally.finish(EndReason::Left)
        }),
        EventBody::SessionEnd(SessionTally::new().finish(EndReason::AppQuit)),
        EventBody::Error(ErrorEvent {
            component: Component::AudioInput,
            code: ErrorCode::DeviceOpenFailed,
            count: 3,
        }),
        EventBody::Crash(Crash {
            file: "src/network/connection.rs".into(),
            line: 42,
            function: Some("jamjam::network::connection::poll".into()),
        }),
        EventBody::Crash(Crash {
            file: "lib.rs".into(),
            line: 1,
            function: None,
        }),
    ]
}

/// Verifies: REQ-TEL-003
#[test]
fn when_any_event_is_recorded_its_line_satisfies_schema_json() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    let events = every_event();
    for event in &events {
        reporter.record(event.clone());
    }

    let parsed = lines(&reporter.preview_ndjson());

    assert_eq!(parsed.len(), events.len());
    for line in &parsed {
        assert_valid(line);
    }
}

/// Verifies: REQ-TEL-003
#[test]
fn when_a_line_is_written_it_carries_the_whole_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(audio_env());

    let line = &lines(&reporter.preview_ndjson())[0];

    for key in [
        "v",
        "ts",
        "seq",
        "event",
        "install_id",
        "launch_id",
        "session_id",
        "app_version",
        "os",
        "arch",
    ] {
        assert!(line.get(key).is_some(), "missing {key}: {line}");
    }
    assert_eq!(line["v"], 1);
    assert_eq!(line["event"], "audio_env");
    assert_eq!(line["session_id"], Value::Null);
    assert_eq!(line["app_version"], "0.1.2");
    assert_eq!(line["os"], std::env::consts::OS);
    assert_eq!(line["arch"], std::env::consts::ARCH);
}

/// Verifies: REQ-TEL-003
#[test]
fn when_events_are_recorded_seq_counts_up_within_the_launch() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(app_start());
    reporter.record(audio_env());
    reporter.record(audio_env());

    let seqs: Vec<u64> = lines(&reporter.preview_ndjson())
        .iter()
        .map(|line| line["seq"].as_u64().unwrap())
        .collect();

    assert_eq!(seqs, [0, 1, 2]);
}

/// Verifies: REQ-TEL-003
#[test]
fn when_a_line_has_a_value_outside_the_schema_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(EventBody::Error(ErrorEvent {
        component: Component::Signaling,
        code: ErrorCode::Timeout,
        count: 1,
    }));
    let good = lines(&reporter.preview_ndjson()).remove(0);
    assert_valid(&good);

    let with = |key: &str, value: Value| {
        let mut line = good.clone();
        line[key] = value;
        line
    };
    // Names and codes are closed lists.
    assert!(
        !is_valid(&with("event", "made_up".into())),
        "the schema accepted a line it must refuse"
    );
    assert!(
        !is_valid(&with("code", "connection to 10.0.0.5 refused".into())),
        "the schema accepted a line it must refuse"
    );
    assert!(
        !is_valid(&with("component", "chat".into())),
        "the schema accepted a line it must refuse"
    );
    // Nothing that is not in the schema may ride along.
    assert!(
        !is_valid(&with("message", "room ABC123".into())),
        "the schema accepted a line it must refuse"
    );
    assert!(
        !is_valid(&with("install_id", "not-hex".into())),
        "the schema accepted a line it must refuse"
    );
    assert!(
        !is_valid(&with("ts", "yesterday".into())),
        "the schema accepted a line it must refuse"
    );
    assert!(
        !is_valid(&with("v", 2.into())),
        "the schema accepted a line it must refuse"
    );
    assert!(
        !is_valid(&with("count", 0.into())),
        "the schema accepted a line it must refuse"
    );
}

/// Verifies: REQ-TEL-003
#[test]
fn when_a_device_or_a_session_end_has_a_value_outside_the_schema_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(audio_env());
    reporter.record(EventBody::SessionEnd(
        SessionTally::new().finish(EndReason::Left),
    ));
    let mut parsed = lines(&reporter.preview_ndjson());
    let session_end = parsed.pop().unwrap();
    let audio_env = parsed.pop().unwrap();
    assert_valid(&audio_env);
    assert_valid(&session_end);

    let mut bad = audio_env.clone();
    bad["input"]["kind"] = "wireless".into();
    assert!(!is_valid(&bad), "the schema accepted a line it must refuse");
    let mut bad = audio_env.clone();
    bad["input"]["device_id"] = "hw:0,0".into();
    assert!(!is_valid(&bad), "the schema accepted a line it must refuse");
    let mut bad = audio_env.clone();
    bad["input"]["name"] = "x".repeat(129).into();
    assert!(!is_valid(&bad), "the schema accepted a line it must refuse");

    let mut bad = session_end.clone();
    bad["end_reason"] = "crashed".into();
    assert!(!is_valid(&bad), "the schema accepted a line it must refuse");
    let mut bad = session_end.clone();
    bad["duration_s"] = "long".into();
    assert!(!is_valid(&bad), "the schema accepted a line it must refuse");
}

/// Verifies: REQ-TEL-003
#[test]
fn when_a_line_of_another_event_is_mixed_in_the_fields_of_one_event_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(EventBody::SessionStart(SessionStart {
        mode: SessionMode::Create,
    }));
    let mut line = lines(&reporter.preview_ndjson()).remove(0);
    assert_valid(&line);

    line["duration_s"] = 10.into();

    assert!(
        !is_valid(&line),
        "the schema accepted a line it must refuse"
    );
}

/// Verifies: REQ-TEL-003
#[test]
fn when_the_real_machine_is_described_the_lines_satisfy_schema_json() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(EventBody::AppStart(snapshot::app_start(
        &AppConfig::default(),
    )));
    reporter.record(EventBody::AudioEnv(snapshot::audio_env()));

    let parsed = lines(&reporter.preview_ndjson());

    assert_eq!(parsed.len(), 2);
    for line in &parsed {
        assert_valid(line);
    }
}

// -- settings -----------------------------------------------------------

fn settings_line_for(config: &AppConfig) -> (String, Value) {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    reporter.record(EventBody::AppStart(snapshot::app_start(config)));
    let body = reporter.preview_ndjson();
    let line = lines(&body).remove(0);
    (body, line)
}

/// Verifies: REQ-TEL-004
#[test]
fn when_settings_are_sent_the_five_left_out_items_are_not_in_the_line() {
    let config = AppConfig {
        peer_name: "Alice Anderson".into(),
        connection_history: vec![ConnectionHistoryEntry {
            room_code: "ROOMCODE123".into(),
            connected_at: chrono::Utc::now(),
            label: Some("band practice".into()),
        }],
        // Built from parts: the distribution test flags any literal server URL.
        server_url: Some(concat!("https", "://user:hunter2@my-own-server.example").into()),
        input_device_id: Some("alsa:hw:CARD=Alice,DEV=0".into()),
        output_device_id: Some("alsa:hw:CARD=Bob,DEV=1".into()),
        ..AppConfig::default()
    };

    let (body, line) = settings_line_for(&config);

    for name in settings::LEFT_OUT {
        assert!(
            line["settings"].get(name).is_none(),
            "{name} is in the settings: {body}"
        );
    }
    for value in [
        "Alice Anderson",
        "ROOMCODE123",
        "band practice",
        "hunter2",
        "my-own-server",
        "CARD=Alice",
        "CARD=Bob",
    ] {
        assert!(!body.contains(value), "{value} reached the line: {body}");
    }
    assert_valid(&line);
}

/// Verifies: REQ-TEL-004
#[test]
fn when_the_left_out_list_is_read_it_is_exactly_the_five_named_items() {
    let mut names = settings::LEFT_OUT.to_vec();
    names.sort_unstable();

    assert_eq!(
        names,
        [
            "connection_history",
            "input_device_id",
            "output_device_id",
            "peer_name",
            "server_url"
        ]
    );
}

/// Verifies: REQ-TEL-004
#[test]
fn when_a_setting_is_added_it_is_sent_unless_it_is_left_out() {
    // Whatever the settings file can hold (every item, set or not) is what is
    // sent, minus the five. A setting added to `AppConfig` tomorrow appears
    // here without this test or the reporter being touched.
    let everything = serde_json::to_value(AppConfig::default()).unwrap();
    let mut expected: Vec<&String> = everything
        .as_object()
        .unwrap()
        .keys()
        .filter(|name| !settings::LEFT_OUT.contains(&name.as_str()))
        .collect();
    expected.sort();

    let sent = settings::settings_for_report(&AppConfig::default());
    let mut sent_names: Vec<&String> = sent.keys().collect();
    sent_names.sort();

    assert_eq!(sent_names, expected);
    for name in [
        "buffer_size",
        "sample_rate",
        "preset",
        "transmit_channels",
        "usage_reporting",
    ] {
        assert!(sent.contains_key(name), "{name} is not sent");
    }
}

/// Verifies: REQ-TEL-004
#[test]
fn when_a_setting_is_a_string_the_value_is_sent_as_it_is() {
    let config = AppConfig {
        language: Some("ja".into()),
        ..AppConfig::default()
    };

    let sent = settings::settings_for_report(&config);

    assert_eq!(sent["language"], "ja");
    assert_eq!(sent["buffer_size"], 64);
}

// -- device names -------------------------------------------------------

/// Verifies: REQ-TEL-005
#[test]
fn when_a_device_is_reported_its_name_is_sent_exactly_as_the_os_gave_it() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    for name in ["Scarlett 2i2 USB", "太郎の AirPods", "Built-in Output (2)"] {
        reporter.record(EventBody::AudioEnv(AudioEnv {
            input: Some(device(name)),
            output: Some(device(name)),
        }));
    }

    let parsed = lines(&reporter.preview_ndjson());

    assert_eq!(parsed[0]["input"]["name"], "Scarlett 2i2 USB");
    assert_eq!(parsed[1]["output"]["name"], "太郎の AirPods");
    assert_eq!(parsed[2]["input"]["name"], "Built-in Output (2)");
    for line in &parsed {
        assert_valid(line);
        assert!(line["input"].get("device_id").is_none());
    }
}

// -- batches ------------------------------------------------------------

/// Verifies: REQ-TEL-006
#[tokio::test]
async fn when_more_events_wait_than_fit_in_one_send_only_200_lines_go_out() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), true);
    for _ in 0..250 {
        reporter.record(EventBody::SessionStart(SessionStart {
            mode: SessionMode::Join,
        }));
    }

    reporter.flush().await;

    let batches = transport.batches();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].lines().count(), MAX_BATCH_LINES);
    // What did not fit is gone, not kept for the next send.
    assert_eq!(reporter.pending_count(), 0);
}

/// Verifies: REQ-TEL-006
#[tokio::test]
async fn when_the_lines_are_long_one_send_stays_within_64_kilobytes() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), true);
    for _ in 0..200 {
        reporter.record(EventBody::AudioEnv(AudioEnv {
            input: Some(device(&"あ".repeat(128))),
            output: Some(device(&"い".repeat(128))),
        }));
    }

    reporter.flush().await;

    let body = &transport.batches()[0];
    assert!(body.len() <= MAX_BATCH_BYTES, "{} bytes", body.len());
    assert!(body.lines().count() < MAX_BATCH_LINES);
    assert!(body.ends_with('\n'));
    for line in lines(body) {
        assert_valid(&line);
    }
}

/// Verifies: REQ-TEL-006
#[tokio::test]
async fn when_the_server_does_not_take_a_batch_it_is_dropped_and_not_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), true);
    transport.accept.store(false, Ordering::SeqCst);
    reporter.record(app_start());

    reporter.flush().await;
    reporter.flush().await;

    assert_eq!(
        transport.batches().len(),
        1,
        "a failed batch was sent again"
    );
    assert_eq!(reporter.pending_count(), 0);
}

/// Verifies: REQ-TEL-006
#[tokio::test]
async fn when_nothing_is_waiting_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), true);

    reporter.flush().await;

    assert!(transport.batches().is_empty());
}

/// Verifies: REQ-TEL-006
#[tokio::test]
async fn when_an_error_happens_it_waits_for_the_next_send_and_goes_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), true);

    reporter.record_error(Component::Signaling, ErrorCode::ConnectFailed);
    assert!(transport.batches().is_empty(), "an error sent by itself");

    reporter.record(app_start());
    reporter.flush().await;

    let events: Vec<String> = lines(&transport.batches()[0])
        .iter()
        .map(|line| line["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, ["error", "app_start"]);
}

/// Verifies: REQ-TEL-006
#[test]
fn when_the_same_error_repeats_before_the_next_send_it_is_counted_once() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);

    for _ in 0..3 {
        reporter.record_error(Component::AudioOutput, ErrorCode::StreamFailed);
    }
    reporter.record_error(Component::AudioOutput, ErrorCode::DeviceNotFound);
    reporter.record_error(Component::Codec, ErrorCode::StreamFailed);

    let parsed = lines(&reporter.preview_ndjson());
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0]["count"], 3);
    assert_eq!(parsed[1]["count"], 1);
    assert_eq!(parsed[2]["count"], 1);
    for line in &parsed {
        assert_valid(line);
    }
}

/// Verifies: REQ-TEL-006
#[test]
fn when_the_same_error_happens_in_another_session_it_is_counted_apart() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);

    reporter.record_error(Component::Ice, ErrorCode::Timeout);
    reporter.begin_session(SessionMode::Create);
    reporter.record_error(Component::Ice, ErrorCode::Timeout);

    let errors: Vec<Value> = lines(&reporter.preview_ndjson())
        .into_iter()
        .filter(|line| line["event"] == "error")
        .collect();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["session_id"], Value::Null);
    assert!(errors[1]["session_id"].is_string());
}

// -- preview ------------------------------------------------------------

/// Verifies: REQ-TEL-009
#[tokio::test]
async fn when_the_preview_is_read_it_is_the_body_the_next_send_will_carry() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, transport) = reporter(dir.path(), true);
    reporter.record(app_start());
    reporter.record(audio_env());
    reporter.record_error(Component::Config, ErrorCode::InvalidValue);

    let preview = reporter.preview_ndjson();
    reporter.flush().await;

    assert_eq!(transport.batches().len(), 1);
    assert_eq!(transport.batches()[0], preview);
    // With nothing waiting it still shows what went out last.
    assert_eq!(reporter.preview_ndjson(), preview);
    assert!(preview.contains("\"install_id\""));
}

// -- sessions -----------------------------------------------------------

/// Verifies: REQ-TEL-010
#[test]
fn when_a_session_runs_its_events_carry_its_id_and_the_end_carries_the_totals() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);

    reporter.begin_session(SessionMode::Create);
    reporter.with_session(|tally| {
        tally.set_participants(2);
        tally.participant_joined();
        tally.participant_left();
        tally.add_reconnects(1);
        tally.add_xruns(3);
        tally.add_xruns(1);
        tally.sample(Some(20.0), 0.0);
        tally.sample(Some(40.0), 0.02);
    });
    reporter.end_session(EndReason::Left);
    reporter.record(audio_env());

    let parsed = lines(&reporter.preview_ndjson());
    assert_eq!(parsed[0]["event"], "session_start");
    assert_eq!(parsed[0]["mode"], "create");
    let session_id = parsed[0]["session_id"].as_str().unwrap();
    assert_eq!(parsed[1]["event"], "session_end");
    assert_eq!(parsed[1]["session_id"], session_id);
    assert_eq!(parsed[1]["end_reason"], "left");
    assert_eq!(parsed[1]["peers_max"], 3);
    assert_eq!(parsed[1]["reconnect_count"], 1);
    assert_eq!(parsed[1]["xrun_count"], 4);
    assert_eq!(parsed[1]["rtt_ms_p50"], 20.0);
    assert_eq!(parsed[1]["rtt_ms_p95"], 40.0);
    assert_eq!(parsed[1]["loss_pct_mean"], 1.0);
    assert_eq!(parsed[1]["loss_pct_max"], 2.0);
    // Outside a session again.
    assert_eq!(parsed[2]["session_id"], Value::Null);
    for line in &parsed {
        assert_valid(line);
    }
}

/// Verifies: REQ-TEL-010
#[test]
fn when_a_figure_was_never_measured_it_is_left_out_of_session_end() {
    let end = SessionTally::new().finish(EndReason::Disconnected);
    let json = serde_json::to_value(&end).unwrap();

    for key in [
        "rtt_ms_p50",
        "rtt_ms_p95",
        "loss_pct_mean",
        "loss_pct_max",
        "fec_active_pct",
    ] {
        assert!(json.get(key).is_none(), "{key} was written as {json}");
    }
}

/// Verifies: REQ-TEL-010
#[test]
fn when_readings_arrive_the_percentiles_are_nearest_rank_and_the_duration_is_the_elapsed_time() {
    let mut tally = SessionTally::started_at(Instant::now());
    for rtt in 1..=100 {
        tally.sample(Some(rtt as f32), 0.0);
    }
    // No round-trip time yet: it counts for loss, not for the round trip.
    tally.sample(None, 0.5);

    let end = tally.finish_after(Duration::from_secs(1832), EndReason::Left);

    assert_eq!(end.duration_s, 1832);
    assert_eq!(end.rtt_ms_p50, Some(50.0));
    assert_eq!(end.rtt_ms_p95, Some(95.0));
    assert_eq!(end.loss_pct_max, Some(50.0));
}

/// Verifies: REQ-TEL-010
#[test]
fn when_a_session_is_still_open_a_new_one_closes_it_first() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);

    reporter.begin_session(SessionMode::Create);
    reporter.begin_session(SessionMode::Join);

    let events: Vec<String> = lines(&reporter.preview_ndjson())
        .iter()
        .map(|line| line["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(events, ["session_start", "session_end", "session_start"]);
}

/// Verifies: REQ-TEL-010
#[test]
fn when_no_session_is_open_ending_one_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);

    reporter.end_session(EndReason::Left);

    assert_eq!(reporter.pending_count(), 0);
}

// -- crash --------------------------------------------------------------

fn saved_crash(dir: &Path, launch_id: &str, file: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("crash.json"),
        serde_json::json!({
            "ts": "2026-09-23T10:00:00Z",
            "launch_id": launch_id,
            "app_version": "0.1.1",
            "file": file,
            "line": 77,
            "function": "jamjam::audio::engine::run",
        })
        .to_string(),
    )
    .unwrap();
}

/// Verifies: REQ-TEL-008
#[test]
fn when_the_last_launch_crashed_the_next_one_records_where_with_the_old_launch_id() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    let crashed_launch = "0123456789abcdef0123456789abcdef";
    saved_crash(dir.path(), crashed_launch, "src/audio/engine.rs");

    reporter.report_previous_crash();

    let parsed = lines(&reporter.preview_ndjson());
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["event"], "crash");
    assert_eq!(parsed[0]["file"], "src/audio/engine.rs");
    assert_eq!(parsed[0]["line"], 77);
    assert_eq!(parsed[0]["function"], "jamjam::audio::engine::run");
    assert_eq!(parsed[0]["launch_id"], crashed_launch);
    assert_eq!(parsed[0]["app_version"], "0.1.1");
    assert_valid(&parsed[0]);
    // Sent once.
    assert!(!dir.path().join("crash.json").exists());
    reporter.report_previous_crash();
    assert_eq!(reporter.pending_count(), 1);
}

/// Verifies: REQ-TEL-008
#[test]
fn when_a_crash_record_names_something_that_is_not_a_source_location_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (reporter, _) = reporter(dir.path(), true);
    saved_crash(
        dir.path(),
        "0123456789abcdef0123456789abcdef",
        "panicked: room ABC123 refused 10.0.0.5",
    );

    reporter.report_previous_crash();

    assert_eq!(reporter.pending_count(), 0);
    assert!(!dir.path().join("crash.json").exists());
}

/// Verifies: REQ-TEL-008
#[test]
fn when_reporting_is_off_a_crash_record_is_removed_and_not_sent() {
    let dir = tempfile::tempdir().unwrap();
    saved_crash(dir.path(), "0123456789abcdef0123456789abcdef", "src/lib.rs");

    let (reporter, transport) = reporter(dir.path(), false);
    reporter.report_previous_crash();

    assert!(!dir.path().join("crash.json").exists());
    assert_eq!(reporter.pending_count(), 0);
    assert!(transport.batches().is_empty());
}

// -- transport ----------------------------------------------------------

struct Captured {
    request_line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// A server on this machine that answers every request with `status` and
/// records what it was sent.
async fn capturing_server(status: &'static str) -> (String, Arc<Mutex<Vec<Captured>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let store = captured.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(at + 4);
                }
            };
            let Some(header_end) = header_end else {
                continue;
            };
            let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
            let mut head_lines = head.split("\r\n");
            let request_line = head_lines.next().unwrap().to_string();
            let headers: Vec<(String, String)> = head_lines
                .filter_map(|line| line.split_once(": "))
                .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
                .collect();
            let length: usize = headers
                .iter()
                .find(|(k, _)| k == "content-length")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            while raw.len() < header_end + length {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            store.lock().unwrap().push(Captured {
                request_line,
                headers,
                body: raw[header_end..].to_vec(),
            });
            let response = format!(
                "HTTP/1.1 {status}\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (url, captured)
}

/// Verifies: REQ-TEL-007
#[tokio::test]
async fn when_a_batch_is_sent_it_is_one_anonymous_ndjson_post_to_the_usage_logs_path() {
    let (server, captured) = capturing_server("204 No Content").await;
    let dir = tempfile::tempdir().unwrap();
    let reporter = UsageReporter::new(
        Some(dir.path().to_path_buf()),
        "0.1.2",
        Arc::new(HttpTransport::new(&format!("{server}/")).unwrap()),
        true,
    );
    reporter.record(app_start());
    reporter.record(audio_env());
    let preview = reporter.preview_ndjson();

    reporter.flush().await;

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.request_line, "POST /api/v1/usage-logs HTTP/1.1");
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(header("content-type"), Some("application/x-ndjson"));
    assert_eq!(String::from_utf8(request.body.clone()).unwrap(), preview);
    // Anonymous: nothing that identifies the device beyond the lines.
    for (name, _) in &request.headers {
        assert!(
            !name.starts_with("x-device") && name != "authorization" && name != "cookie",
            "{name} was sent"
        );
    }
}

/// Verifies: REQ-TEL-007
#[tokio::test]
async fn when_the_server_answers_204_the_batch_counts_as_delivered() {
    let (server, _) = capturing_server("204 No Content").await;

    let delivered = HttpTransport::new(&server)
        .unwrap()
        .send(b"{}\n".to_vec())
        .await;

    assert!(delivered);
}

/// Verifies: REQ-TEL-007
#[tokio::test]
async fn when_the_server_answers_anything_but_204_the_batch_does_not_count_as_delivered() {
    for status in ["200 OK", "400 Bad Request", "503 Service Unavailable"] {
        let (server, _) = capturing_server(status).await;

        let delivered = HttpTransport::new(&server)
            .unwrap()
            .send(b"{}\n".to_vec())
            .await;

        assert!(!delivered, "{status} counted as delivered");
    }
}

/// Verifies: REQ-TEL-007
#[tokio::test]
async fn when_the_server_redirects_the_batch_is_not_sent_on() {
    let (server, captured) = capturing_server("307 Temporary Redirect").await;

    let delivered = HttpTransport::new(&server)
        .unwrap()
        .send(b"{}\n".to_vec())
        .await;

    assert!(!delivered);
    assert_eq!(captured.lock().unwrap().len(), 1);
}

/// Verifies: REQ-TEL-007
#[tokio::test]
async fn when_nobody_answers_the_batch_does_not_count_as_delivered() {
    // Bound and dropped, so the port is closed.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };

    let delivered = HttpTransport::new(&closed)
        .unwrap()
        .send(b"{}\n".to_vec())
        .await;

    assert!(!delivered);
}

/// Verifies: REQ-TEL-007
#[test]
fn when_the_server_url_is_not_http_there_is_no_transport() {
    assert!(HttpTransport::new("").is_none());
    assert!(HttpTransport::new("ws://server.example.com").is_none());
    assert!(HttpTransport::new("https://server.example.com").is_some());
}
