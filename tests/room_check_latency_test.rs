//! The round trip to the fixed participant of the latency room that the room check keeps for
//! every runner: when each runner takes its turn, who is counted as the far end and
//! who as company, and which stays are left out of the numbers.
//!
//! The logic lives in `scripts/room-check.py`; these run it as the workflow does, through python.

use std::path::PathBuf;
use std::process::Command;

/// Runs `body` in python with `room_check` (the script, loaded by path) in scope, and returns what it prints.
fn python(body: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let program = format!(
        "import importlib.util, json\n\
         spec = importlib.util.spec_from_file_location('room_check', {:?})\n\
         room_check = importlib.util.module_from_spec(spec)\n\
         spec.loader.exec_module(room_check)\n\
         {}",
        root.join("scripts/room-check.py"),
        body
    );
    let output = Command::new("python3")
        .arg("-c")
        .arg(program)
        .output()
        .expect("python3 is needed to run scripts/room-check.py");
    assert!(
        output.status.success(),
        "python failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn runners_take_their_turns_in_the_latency_room_one_after_another_after_the_room_has_ended() {
    let turns = python(
        "start = 1000.0\n\
         _, end_at = room_check.schedule(start, 6, 3)\n\
         enters = [room_check.latency_enter_at(start, 6, 3, runner) for runner in range(4)]\n\
         print(json.dumps({'end': end_at, 'enters': enters, 'stay': room_check.LATENCY_STAY_SECONDS}))",
    );
    let turns: serde_json::Value = serde_json::from_str(&turns).unwrap();
    let end = turns["end"].as_f64().unwrap();
    let stay = turns["stay"].as_f64().unwrap();
    let enters: Vec<f64> = turns["enters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|at| at.as_f64().unwrap())
        .collect();
    assert!(
        enters[0] > end,
        "the first runner enters at {} before the room's run ends at {}",
        enters[0],
        end
    );
    for pair in enters.windows(2) {
        assert!(
            pair[1] - pair[0] > stay,
            "two runners' stays overlap: {:?}",
            enters
        );
    }
}

#[test]
fn only_the_fixed_participant_is_in_the_room_when_a_runner_is_alone_with_it() {
    let found = python(
        "lines = ['Joined room: x', 'Invite code:  AAAAAAAAA', 'Peers in room (1):',\n\
                  '  - Fixed Peer (id: 1, addr: None)']\n\
         far, others = room_check.names_in_room(lines)\n\
         print(json.dumps({'far': far, 'others': sorted(others)}))",
    );
    assert_eq!(found, r#"{"far": "Fixed Peer", "others": []}"#);
}

#[test]
fn another_runner_is_counted_as_company_whether_it_was_there_at_the_start_or_came_later() {
    let found = python(
        "lines = ['Peers in room (2):',\n\
                  '  - Fixed Peer (id: 1, addr: None)',\n\
                  '  - r1a1-q00 (id: 2, addr: None)',\n\
                  '',\n\
                  '📥 r1a1-q02 joined the room']\n\
         far, others = room_check.names_in_room(lines)\n\
         print(json.dumps({'far': far, 'others': sorted(others)}))",
    );
    assert_eq!(
        found,
        r#"{"far": "Fixed Peer", "others": ["r1a1-q00", "r1a1-q02"]}"#
    );
}

#[test]
fn a_room_without_the_fixed_participant_has_no_far_end() {
    let found = python(
        "far, others = room_check.names_in_room(['Peers in room (0):'])\n\
         print(json.dumps({'far': far, 'others': sorted(others)}))",
    );
    assert_eq!(found, r#"{"far": null, "others": []}"#);
}

fn status_of(report: &str, far: &str, others: &str) -> String {
    python(&format!(
        "report = json.loads({report:?})\n\
         print(room_check.latency_of(report, {far}, set({others}))['status'])"
    ))
}

const REPORT: &str = r#"{"peers": [{"name": "Fixed Peer", "rtt_spread_ms": {"samples": 28, "min": 140.0, "median": 151.5, "p95": 160.0, "max": 170.0, "stdev": 4.0}, "network": {"packet_loss_rate": 0.0}, "link": {"route": "public", "family": "ipv4"}}]}"#;

#[test]
fn a_stay_alone_with_round_trips_timed_is_measured() {
    assert_eq!(status_of(REPORT, "'Fixed Peer'", "[]"), "measured");
}

#[test]
fn a_stay_with_company_is_shared_and_left_out_of_the_numbers() {
    assert_eq!(status_of(REPORT, "'Fixed Peer'", "['r1a1-q01']"), "shared");
}

#[test]
fn a_stay_without_a_round_trip_back_is_unreached() {
    let silent = r#"{"peers": [{"name": "Fixed Peer", "rtt_spread_ms": null, "link": {}}]}"#;
    assert_eq!(status_of(silent, "'Fixed Peer'", "[]"), "unreached");
}

#[test]
fn a_stay_in_a_room_without_the_fixed_participant_has_no_far_end() {
    assert_eq!(status_of(r#"{"peers": []}"#, "None", "[]"), "no_far_end");
}

#[test]
fn the_numbers_of_a_measured_stay_are_the_apps_own_spread_of_round_trips() {
    let numbers = python(&format!(
        "report = json.loads({REPORT:?})\n\
         found = room_check.latency_of(report, 'Fixed Peer', set())\n\
         print(json.dumps([found['rtt_ms']['median'], found['rtt_ms']['p95'], found['rtt_ms']['samples'], found['route'], found['family']]))"
    ));
    assert_eq!(numbers, r#"[151.5, 160.0, 28, "public", "ipv4"]"#);
}
