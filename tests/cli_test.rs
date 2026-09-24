//! Integration tests for the `jamjam` CLI (ADR-027).
//!
//! The CLI exists so a session can be driven without the GUI while debugging,
//! which means these tests run the real binary: they would have caught that
//! `create-room` did not exist at all, and that a joining peer had no invite
//! code to pass on.
//!
//! `$HOME` is redirected per test, because the CLI writes the same config file
//! the app uses and a test must not touch the developer's own settings.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// The binary cargo just built for this test.
const JAMJAM: &str = env!("CARGO_BIN_EXE_jamjam");

/// How long to wait for a line the CLI is expected to print. Generous: the
/// binary starts and opens its sockets before it prints anything.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(20);

/// Where the CLI writes `config.toml` under a redirected `$HOME`. Mirrors what
/// the `directories` crate derives for this platform.
fn config_path(home: &Path) -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/jamjam")
    } else if cfg!(target_os = "windows") {
        home.join("AppData/Roaming/jamjam/config")
    } else {
        home.join(".config/jamjam")
    };
    dir.join("config.toml")
}

/// Runs the CLI with an isolated `$HOME` and collects its output.
fn run(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(JAMJAM)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()
        .expect("the jamjam binary should run")
}

/// A CLI process whose stdout is read line by line in the background, so a
/// test can wait for one line while the process keeps running.
struct Running {
    child: Child,
    lines: Receiver<String>,
}

impl Running {
    fn start(home: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(JAMJAM)
            .args(args)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the jamjam binary should start");

        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Self { child, lines }
    }

    /// Waits for a line containing `needle`, returning it.
    fn wait_for(&self, needle: &str) -> Option<String> {
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        while Instant::now() < deadline {
            match self.lines.recv_timeout(Duration::from_millis(200)) {
                Ok(line) if line.contains(needle) => return Some(line),
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        None
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The preset chosen from the CLI is the one the app starts with: same file,
/// same field, and the buffer size moves with it.
///
/// Verifies: REQ-CLI-003
#[test]
fn preset_use_saves_the_choice_where_the_app_reads_it() {
    let home = tempfile::tempdir().expect("temp HOME");

    let output = run(home.path(), &["preset", "use", "zero-latency"]);
    assert!(
        output.status.success(),
        "preset use failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let saved = std::fs::read_to_string(config_path(home.path())).expect("config.toml is written");
    assert!(
        saved.contains("preset = \"zero-latency\""),
        "the preset should be saved, got:\n{}",
        saved
    );
    assert!(
        saved.contains("buffer_size = 32"),
        "the buffer size should follow the preset (32 samples), got:\n{}",
        saved
    );

    // And the listing reports it as the one in use.
    let listed = run(home.path(), &["preset", "list"]);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    let marked: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with('*'))
        .collect();
    assert_eq!(
        marked.len(),
        1,
        "exactly one preset should be marked as in use, got:\n{}",
        stdout
    );
    assert!(
        marked[0].contains("zero-latency"),
        "the marked preset should be the one just chosen, got {:?}",
        marked[0]
    );
}

/// An unknown preset is refused rather than saved, so a typo cannot leave the
/// app configured for something that does not exist.
///
/// Verifies: REQ-CLI-003
#[test]
fn preset_use_refuses_a_name_that_is_not_a_preset() {
    let home = tempfile::tempdir().expect("temp HOME");

    let output = run(home.path(), &["preset", "use", "zero-latencyy"]);

    assert!(!output.status.success(), "a typo should not be accepted");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("zero-latencyy") && message.contains("preset list"),
        "the error should name the typo and how to see the real ones, got {:?}",
        message
    );
    assert!(
        !config_path(home.path()).exists(),
        "nothing should have been saved"
    );
}

/// Choosing a device the machine does not offer is refused, the same way the
/// settings UI refuses it (REQ-GUI-011) - saving the name would silently fall
/// back to the default device at session start.
///
/// Verifies: REQ-CLI-004
#[test]
fn devices_set_refuses_a_device_this_machine_does_not_have() {
    let home = tempfile::tempdir().expect("temp HOME");

    let output = run(
        home.path(),
        &["devices", "set", "--input", "no-such-device-42"],
    );

    assert!(!output.status.success(), "an absent device is not settable");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("no-such-device-42") && message.contains("devices list"),
        "the error should name the device and how to list the real ones, got {:?}",
        message
    );
    assert!(
        !config_path(home.path()).exists(),
        "a refused choice must not be written"
    );
}

fn find_available_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .expect("bind an ephemeral UDP port")
        .local_addr()
        .expect("read the bound address")
        .port()
}

/// What a CLI wrote with `--output-file`: 32-bit float, little-endian,
/// stereo interleaved. Returned as (left, right).
fn read_played(path: &Path) -> (Vec<f32>, Vec<f32>) {
    let bytes = std::fs::read(path).expect("the output file is written");
    let samples: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    let left = samples.iter().step_by(2).copied().collect();
    let right = samples.iter().skip(1).step_by(2).copied().collect();
    (left, right)
}

/// Frequency of a tone, from the typical distance between the points where it
/// crosses zero going up. The median, so a gap - the peer stopping, a frame
/// concealed - does not drag the estimate down.
fn tone_frequency(samples: &[f32], sample_rate: u32) -> f32 {
    let crossings: Vec<usize> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
        .map(|(i, _)| i)
        .collect();
    let mut periods: Vec<usize> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(!periods.is_empty(), "no tone to measure");
    periods.sort_unstable();
    sample_rate as f32 / periods[periods.len() / 2] as f32
}

/// Two CLIs connected directly each hear the other's tone - at the pitch and
/// level it was sent, in both channels - after it has gone through the
/// play-out path the app uses (a buffered preset, so FEC and the play-out
/// delay are both in play).
///
/// The sound card is replaced by `--input-tone` and `--output-file`, so this
/// runs wherever `cargo test` runs.
///
/// Verifies: REQ-CLI-005
#[test]
fn two_clis_connected_directly_hear_each_other() {
    const SAMPLE_RATE: u32 = 48000;
    let port = find_available_udp_port().to_string();
    let address = format!("127.0.0.1:{}", port);
    let host_home = tempfile::tempdir().expect("temp HOME");
    let join_home = tempfile::tempdir().expect("temp HOME");
    let host_out = host_home.path().join("played.f32");
    let join_out = join_home.path().join("played.f32");
    let audio = |tone: &'static str, out: &Path| -> Vec<String> {
        [
            "--preset",
            "balanced",
            "--sample-rate",
            "48000",
            "--input-tone",
            tone,
            "--output-file",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .chain([out.display().to_string()])
        .collect()
    };

    let host_args: Vec<String> = ["host".to_string(), "--port".to_string(), port.clone()]
        .into_iter()
        .chain(audio("440", &host_out))
        .collect();
    let host = Running::start(
        host_home.path(),
        &host_args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    host.wait_for("Listening on port")
        .expect("host should start listening");

    let join_args: Vec<String> = ["join".to_string(), address]
        .into_iter()
        .chain(audio("660", &join_out))
        .collect();
    let join = Running::start(
        join_home.path(),
        &join_args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    join.wait_for("Session active")
        .expect("join should connect");
    host.wait_for("Session active")
        .expect("host should accept the joining peer");

    std::thread::sleep(Duration::from_millis(1500));
    drop(join);
    drop(host);

    for (who, path, sent_hz) in [("host", &host_out, 660.0), ("join", &join_out, 440.0)] {
        let (left, right) = read_played(path);

        // Priming and the moments before the peer connected are silence;
        // what follows is the peer's tone.
        let start = left
            .iter()
            .position(|s| s.abs() > 0.05)
            .unwrap_or_else(|| panic!("{} played nothing but silence", who));
        let (left, right) = (&left[start..], &right[start..]);
        assert!(
            left.len() >= SAMPLE_RATE as usize / 2,
            "{} should have played at least half a second of the peer, got {} samples",
            who,
            left.len()
        );

        let heard = tone_frequency(left, SAMPLE_RATE);
        assert!(
            (heard - sent_hz).abs() < sent_hz * 0.05,
            "{} should hear the peer's {} Hz tone at its pitch, heard {:.1} Hz",
            who,
            sent_hz,
            heard
        );

        // The tone is sent centred at half scale: each side carries it at
        // 0.5 * cos(pi/4). Louder would mean frames played twice over each
        // other; quieter, or unequal sides, a channel mix-up.
        let peak = left.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(
            (0.30..=0.40).contains(&peak),
            "{} should hear the tone at about 0.35, peak was {}",
            who,
            peak
        );
        assert_eq!(
            left, right,
            "{} should hear the centred tone equally on both sides",
            who
        );
    }
}

/// Runs a host with nobody connected, capturing a 440 Hz tone and writing what
/// it plays to a file, and returns the left channel.
///
/// With no peer the only thing the output can carry is the monitored input, so
/// what comes out is what monitoring does.
fn host_alone_playing(extra_args: &[&str]) -> Vec<f32> {
    let home = tempfile::tempdir().expect("temp HOME");
    let out = home.path().join("played.f32");
    let port = find_available_udp_port().to_string();
    let out_arg = out.display().to_string();
    let mut args = vec![
        "host",
        "--port",
        &port,
        "--preset",
        "ultra-low-latency",
        "--sample-rate",
        "48000",
        "--input-tone",
        "440",
        "--output-file",
        &out_arg,
    ];
    args.extend_from_slice(extra_args);

    let host = Running::start(home.path(), &args);
    host.wait_for("Listening on port")
        .expect("host should start listening");
    std::thread::sleep(Duration::from_millis(1500));
    drop(host);

    read_played(&out).0
}

/// With `--monitor` the host hears its own tone at the pitch and level it was
/// captured, before any peer exists to send it anywhere - which is what makes
/// it local, not a round trip.
///
/// Verifies: REQ-AUD-111
/// Verifies: REQ-CLI-006
#[test]
fn a_monitoring_cli_hears_its_own_input_without_a_peer() {
    const SAMPLE_RATE: u32 = 48000;
    let left = host_alone_playing(&["--monitor"]);

    let start = left
        .iter()
        .position(|s| s.abs() > 0.05)
        .expect("monitoring played nothing but silence");
    let left = &left[start..];
    assert!(
        left.len() >= SAMPLE_RATE as usize / 2,
        "expected at least half a second of monitoring, got {} samples",
        left.len()
    );

    let heard = tone_frequency(left, SAMPLE_RATE);
    assert!(
        (heard - 440.0).abs() < 440.0 * 0.05,
        "the monitor should play the input at its pitch, heard {:.1} Hz",
        heard
    );
    // The tone is captured at half scale and monitored at unity, in both
    // channels equally (no pan on the local monitor).
    let peak = left.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!(
        (0.45..=0.55).contains(&peak),
        "the monitor should play the input at its captured level (0.5), peak was {}",
        peak
    );
}

/// Verifies: REQ-AUD-112
#[test]
fn a_cli_without_monitoring_does_not_play_its_own_input() {
    let left = host_alone_playing(&[]);

    assert!(!left.is_empty(), "the output file should have been written");
    assert!(
        left.iter().all(|s| s.abs() < 1e-6),
        "with monitoring off the output carries only the peers, and there are none"
    );
}

/// A UDP peer that sends every datagram straight back, except audio, which it
/// holds for `hold` first - what a peer that replays what it hears does. Runs until dropped.
struct EchoPeer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EchoPeer {
    /// Byte 1 of a jamjam packet is its type; 0x01 is audio.
    const AUDIO_TYPE_BYTE: usize = 1;
    const AUDIO_TYPE: u8 = 0x01;

    fn start(socket: std::net::UdpSocket, hold: Duration) -> Self {
        use std::sync::atomic::Ordering;

        socket
            .set_read_timeout(Some(Duration::from_millis(2)))
            .expect("set a read timeout");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut buffer = [0u8; 2048];
            let mut held: std::collections::VecDeque<(Instant, std::net::SocketAddr, Vec<u8>)> =
                Default::default();
            while !stopping.load(Ordering::SeqCst) {
                if let Ok((length, from)) = socket.recv_from(&mut buffer) {
                    let datagram = buffer[..length].to_vec();
                    if datagram.get(Self::AUDIO_TYPE_BYTE) == Some(&Self::AUDIO_TYPE) {
                        held.push_back((Instant::now() + hold, from, datagram));
                    } else {
                        let _ = socket.send_to(&datagram, from);
                    }
                }
                while held
                    .front()
                    .is_some_and(|(due, _, _)| *due <= Instant::now())
                {
                    let (_, to, datagram) = held.pop_front().expect("front was just seen");
                    let _ = socket.send_to(&datagram, to);
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for EchoPeer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `join` against a peer that echoes audio after a hold ends by itself after
/// `--duration`, leaves a JSON report, and that report holds the round trip of
/// the bursts sent - the hold taken off - so a change that slows the audio path
/// shows up as a number. The sound card is replaced by `--input-bursts` and
/// `--output-file`, so this runs wherever `cargo test` runs.
///
/// Verifies: REQ-CLI-007
#[test]
fn a_cli_joined_to_an_echo_reports_the_round_trip_of_its_bursts() {
    const HOLD_MS: u64 = 200;
    let echo_socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind the echo peer");
    let address = echo_socket.local_addr().expect("echo address").to_string();
    let _echo = EchoPeer::start(echo_socket, Duration::from_millis(HOLD_MS));

    let home = tempfile::tempdir().expect("temp HOME");
    let report_path = home.path().join("report.json");
    let played = home.path().join("played.f32");
    let started = Instant::now();

    let output = run(
        home.path(),
        &[
            "join",
            &address,
            "--preset",
            "balanced",
            "--sample-rate",
            "48000",
            "--input-bursts",
            "--output-file",
            played.to_str().expect("utf-8 path"),
            "--duration",
            "4",
            "--echo-delay-ms",
            &HOLD_MS.to_string(),
            "--report-json",
            report_path.to_str().expect("utf-8 path"),
        ],
    );

    assert!(
        output.status.success(),
        "the session should end by itself, got {:?}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "--duration 4 should not run for {:?}",
        started.elapsed()
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).expect("the report is written"))
            .expect("the report is JSON");
    let round_trip = &report["round_trip"];

    let sent = round_trip["bursts_sent"].as_u64().expect("bursts_sent");
    let expected = round_trip["bursts_expected"]
        .as_u64()
        .expect("bursts_expected");
    let heard = round_trip["bursts_heard"].as_u64().expect("bursts_heard");
    assert!(sent >= 6, "about 8 bursts go out in 4 s, got {}", sent);
    assert!(
        heard >= 3 && heard >= expected.saturating_sub(1),
        "the bursts due back should come back, heard {} of {} expected",
        heard,
        expected
    );

    // The echo returns audio the moment its hold is up, over loopback, so what
    // is left is the app's own path: the jitter buffer above all (about 16 ms).
    // Failing to take the hold off leaves 200 ms or more, and taking off the
    // wrong hold leaves a delay that is off by as much.
    let median = round_trip["median_ms"].as_f64().expect("median_ms");
    let min = round_trip["min_ms"].as_f64().expect("min_ms");
    let max = round_trip["max_ms"].as_f64().expect("max_ms");
    assert!(
        (0.0..100.0).contains(&median) && min <= median && median <= max,
        "the round trip should be a small positive time with the hold taken off, got min {} median {} max {}",
        min,
        median,
        max
    );

    assert_eq!(report["peer"], address.as_str());
    assert_eq!(report["audio"]["preset"], "balanced");
    assert!(
        report["playout"]["frames_played"].as_u64().unwrap_or(0) > 0,
        "the play-out counters should be in the report: {}",
        report
    );
}
