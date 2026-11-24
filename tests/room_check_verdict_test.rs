//! What the room check's verdict says when GitHub-hosted runners do not reach each other, and how the
//! runners ask GitHub for what the others published.
//!
//! The logic lives in `scripts/room-check.py`; `summary` runs as the workflow's last job does, on result
//! files laid out as the runners write them, and `fetch_artifact` runs against a stand-in `gh`.

use std::path::PathBuf;
use std::process::Command;

/// Runs `body` in python with `room_check` (the script, loaded by path) and `script` (its path) in
/// scope, and returns what it prints.
fn python(body: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let script = root.join("scripts/room-check.py");
    let program = format!(
        "import importlib.util, json, os, subprocess, sys, tempfile, pathlib, stat, zipfile\n\
         script = {script:?}\n\
         spec = importlib.util.spec_from_file_location('room_check', script)\n\
         room_check = importlib.util.module_from_spec(spec)\n\
         spec.loader.exec_module(room_check)\n\
         {body}"
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

/// Four participants, two on each of two runners. `udp_between` is what the app-free UDP check found
/// between the runners ("open" or "lost"), and `across` the share of a turn the other runner's
/// participants were heard in.
fn verdict(udp_between: &str, across: f64) -> String {
    python(&format!(
        r#"
udp_between, across = {udp_between:?}, {across}
folder = pathlib.Path(tempfile.mkdtemp())
flows = 16
def udp(other):
    came = flows if udp_between == 'open' else 0
    return {{'checked': True, 'peers': {{str(other): {{'flows': flows, 'flows_received': came, 'during_flows_received': came}}}}}}
for index in range(4):
    runner = index // 2
    heard = {{str(other): (1.0 if other // 2 == runner else across) for other in range(4) if other != index}}
    result = {{
        'id': index, 'runner': runner, 'os': 'linux', 'offset': 0.0,
        'heard': heard, 'level': {{k: 0.3 for k in heard}}, 'leak': {{k: 0.0 for k in heard}},
        'links': [], 'elapsed_seconds': 100.0, 'cpu_seconds': 10.0, 'max_rss_mb': 40.0,
        'runner_facts': {{'udp': udp(1 - runner)}},
    }}
    (folder / f'p{{index:02d}}.json').write_text(json.dumps(result))
done = subprocess.run([sys.executable, script, 'summary', '--results', str(folder), '--total', '4'], capture_output=True, text=True)
verdict_line = [line for line in done.stdout.splitlines() if line.split(':')[0] in ('PASS', 'FAIL', 'NOT MEASURED')][0]
print(json.dumps({{'rc': done.returncode, 'verdict': verdict_line.split(':')[0]}}))
"#
    ))
}

#[test]
fn runners_that_lost_udp_between_them_even_without_the_app_are_not_measured_and_not_failed() {
    assert_eq!(verdict("lost", 0.0), r#"{"rc": 0, "verdict": "NOT MEASURED"}"#);
}

#[test]
fn runners_that_reach_each_other_without_the_app_but_not_with_it_fail() {
    assert_eq!(verdict("open", 0.0), r#"{"rc": 1, "verdict": "FAIL"}"#);
}

#[test]
fn runners_that_reach_each_other_and_are_heard_pass() {
    assert_eq!(verdict("open", 1.0), r#"{"rc": 0, "verdict": "PASS"}"#);
}

/// Runs `fetch_artifact` for `room-start-1` against a `gh` that answers as GitHub does, and prints
/// what each call to it was and what `fetch_artifact` returned. `answers` is the shell of the stand-in
/// for a call that lists the artifacts, with `$TAGGED` set when the call carries a tag of an answer.
fn fetching(answers: &str, asks: usize) -> String {
    python(&format!(
        r#"
folder = pathlib.Path(tempfile.mkdtemp())
with zipfile.ZipFile(folder / 'start.zip', 'w') as bundle:
    bundle.writestr('start.json', '{{"start_at": 1}}')
(folder / 'bin').mkdir()
gh = folder / 'bin' / 'gh'
gh.write_text('''#!/bin/sh
echo "$@" >> {{log}}
case "$*" in
  *artifacts/7/zip*) cat {{zip}} ;;
  *per_page=100*)
    TAGGED=
    case "$*" in *If-None-Match*) TAGGED=1 ;; esac
    {answers}
    ;;
esac
'''.replace('{{log}}', str(folder / 'calls')).replace('{{zip}}', str(folder / 'start.zip')))
gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
os.environ.update({{'PATH': str(folder / 'bin') + os.pathsep + os.environ['PATH'],
                    'GITHUB_REPOSITORY': 'o/r', 'GITHUB_RUN_ID': '9'}})
room_check.ARTIFACT_LIST_REUSE_SECONDS = 0
found = [room_check.fetch_artifact(folder / 'work', 'room-start-1', 'start.json', 0) for _ in range({asks})]
calls = (folder / 'calls').read_text().splitlines()
print(json.dumps({{'found': found, 'tagged': sum('If-None-Match' in call for call in calls),
                   'lists': sum('per_page=100' in call for call in calls), 'downloads': sum('/zip' in call for call in calls),
                   'why': room_check.why_not_fetched()}}))
"#
    ))
}

const LISTS_ONE_ARTIFACT: &str = r#"if [ -n "$TAGGED" ]; then printf 'HTTP/2.0 304 Not Modified\r\n\r\n'; exit 1; fi
    printf 'HTTP/2.0 200 OK\r\nEtag: W/"abc"\r\n\r\n{"artifacts":[{"name":"room-start-1","id":7}]}'"#;

#[test]
fn an_artifact_that_is_there_is_taken_and_a_list_asked_for_again_carries_the_tag_of_the_last() {
    // The second ask is answered "not modified" and costs nothing against GitHub's limit, yet the
    // artifact is still found (from the list kept) and downloaded again as asked.
    let found: serde_json::Value = serde_json::from_str(&fetching(LISTS_ONE_ARTIFACT, 2)).unwrap();
    assert_eq!(found["found"], serde_json::json!([true, true]));
    assert_eq!(found["lists"], 2);
    assert_eq!(found["tagged"], 1, "{found}");
    assert_eq!(found["why"], "");
}

#[test]
fn an_artifact_that_is_not_published_yet_is_not_found_and_nothing_is_downloaded() {
    let none = r#"if [ -n "$TAGGED" ]; then printf 'HTTP/2.0 304 Not Modified\r\n\r\n'; exit 1; fi
    printf 'HTTP/2.0 200 OK\r\nEtag: W/"abc"\r\n\r\n{"artifacts":[]}'"#;
    let found: serde_json::Value = serde_json::from_str(&fetching(none, 1)).unwrap();
    assert_eq!(found["found"], serde_json::json!([false]));
    assert_eq!(found["downloads"], 0);
    assert_eq!(found["why"], "");
}

#[test]
fn a_runner_that_gives_up_because_github_refused_says_what_github_said() {
    let refused = r#"printf 'HTTP/2.0 403 Forbidden\r\n\r\n'; echo 'gh: API rate limit exceeded for installation. (HTTP 403)' >&2; exit 1"#;
    let found: serde_json::Value = serde_json::from_str(&fetching(refused, 1)).unwrap();
    assert_eq!(found["found"], serde_json::json!([false]));
    assert!(
        found["why"].as_str().unwrap().contains("API rate limit exceeded"),
        "{found}"
    );
}
