//! Integration tests for the `jamuru` CLI (ADR-027).
//!
//! The CLI exists so a session can be driven without the GUI while debugging,
//! which means these tests run the real binary: they would have caught that
//! `create-room` did not exist at all, and that a joining peer had no invite
//! code to pass on.
//!
//! `$HOME` is redirected per test, because the CLI writes the same config file
//! the app uses and a test must not touch the developer's own settings.

mod common;

use common::fake_signaling::FakeSignaling;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// Held by a test while its CLIs run audio against the clock. Their stand-ins for the sound
/// card play and send at the pace of the frame, so a test that shares the machine with the
/// others' CLIs - three in a room, six at once - sees frames late and lost for no fault of
/// the CLI.
static AUDIO_SESSIONS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn one_audio_session_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    AUDIO_SESSIONS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The binary cargo just built for this test.
const JAMURU: &str = env!("CARGO_BIN_EXE_jamuru");

/// How long to wait for a line the CLI is expected to print. Generous: the
/// binary starts and opens its sockets before it prints anything.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(20);

/// Where the CLI writes `config.toml` under a redirected `$HOME`. Mirrors what
/// the `directories` crate derives for this platform.
fn config_path(home: &Path) -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/jamuru")
    } else if cfg!(target_os = "windows") {
        home.join("AppData/Roaming/jamuru/config")
    } else {
        home.join(".config/jamuru")
    };
    dir.join("config.toml")
}

/// Runs the CLI with an isolated `$HOME` and collects its output.
fn run(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(JAMURU)
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .output()
        .expect("the jamuru binary should run")
}

/// A CLI process whose stdout is read line by line in the background, so a
/// test can wait for one line while the process keeps running.
struct Running {
    child: Child,
    lines: Receiver<String>,
}

impl Running {
    fn start(home: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(JAMURU)
            .args(args)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the jamuru binary should start");

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

impl Running {
    /// Waits until `count` lines containing `needle` have come, the last of which is returned.
    fn wait_for_count(&self, needle: &str, count: usize) -> Option<String> {
        let mut last = None;
        for _ in 0..count {
            last = Some(self.wait_for(needle)?);
        }
        last
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

/// Two CLIs in one room each hear the other's tone - at the pitch and
/// level it was sent, in both channels - after it has gone through the
/// play-out path the app uses (a buffered preset, so FEC and the play-out
/// delay are both in play).
///
/// The sound card is replaced by `--input-tone` and `--output-file`, so this
/// runs wherever `cargo test` runs.
///
/// Verifies: REQ-CLI-005
#[test]
fn two_clis_in_a_room_hear_each_other() {
    let _alone = one_audio_session_at_a_time();
    const SAMPLE_RATE: u32 = 48000;
    let server = FakeSignaling::start();
    let creator_home = tempfile::tempdir().expect("temp HOME");
    let joiner_home = tempfile::tempdir().expect("temp HOME");
    let creator_out = creator_home.path().join("played.f32");
    let joiner_out = joiner_home.path().join("played.f32");
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

    let creator_args: Vec<String> = ["create-room", "--server", server.url()]
        .iter()
        .map(|arg| arg.to_string())
        .chain(audio("440", &creator_out))
        .collect();
    let creator = Running::start(
        creator_home.path(),
        &creator_args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let invite_code = creator
        .wait_for("Invite code:")
        .expect("the room should be created")
        .split_whitespace()
        .last()
        .expect("the invite code follows its label")
        .to_string();

    let joiner_args: Vec<String> = [
        "join-room",
        "--server",
        server.url(),
        "--room",
        &invite_code,
    ]
    .iter()
    .map(|arg| arg.to_string())
    .chain(audio("660", &joiner_out))
    .collect();
    let joiner = Running::start(
        joiner_home.path(),
        &joiner_args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    joiner
        .wait_for("Audio is flowing")
        .expect("the joining CLI should connect");
    creator
        .wait_for("Audio is flowing")
        .expect("the creating CLI should connect to the joining peer");

    std::thread::sleep(Duration::from_millis(1500));
    drop(joiner);
    drop(creator);

    for (who, path, sent_hz) in [
        ("creator", &creator_out, 660.0),
        ("joiner", &joiner_out, 440.0),
    ] {
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

/// Size of the component of `samples` at `hz`, as the amplitude of a sine that has it.
fn tone_amplitude(samples: &[f32], hz: f64, sample_rate: u32) -> f32 {
    let step = std::f64::consts::TAU * hz / f64::from(sample_rate);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, sample) in samples.iter().enumerate() {
        let angle = step * i as f64;
        re += f64::from(*sample) * angle.cos();
        im -= f64::from(*sample) * angle.sin();
    }
    (2.0 * (re * re + im * im).sqrt() / samples.len() as f64) as f32
}

/// The preset the CLIs of a room run
const ROOM_PRESET: &str = "balanced";

/// The play-out frame of [`ROOM_PRESET`], in samples per channel. What a CLI plays is written
/// to its file a frame at a time, so a frame of one frame is what was played in one beat.
fn frame_samples() -> usize {
    jamjam::AudioPreset::from_name(ROOM_PRESET)
        .expect("a preset")
        .frame_size() as usize
}

/// The tones the CLIs of a room send: 2, 4 and 8 whole cycles to a frame, so that each is
/// measured in one frame without leaking into the others
fn tones() -> [u32; 3] {
    [2, 4, 8].map(|cycles| cycles * 48_000 / frame_samples() as u32)
}

/// What a CLI of a room played, a frame at a time: how loud each of [`tones`] was in each. A
/// frame the machine was too busy to play is silent in every tone, and a frame of one of the
/// others may be missing while another's is there, so what is asked of the frames is
/// "somewhere" and "nowhere", not "throughout".
struct Heard(Vec<[f32; 3]>);

impl Heard {
    /// The frames after the first `count`
    fn after(&self, count: usize) -> &[[f32; 3]] {
        &self.0[count.min(self.0.len())..]
    }
}

/// The loudest `tone` is in any of `frames`
fn loudest(frames: &[[f32; 3]], tone: usize) -> f32 {
    frames.iter().fold(0.0f32, |top, f| top.max(f[tone]))
}

/// The loudest the quieter of two tones is in one frame: the most that both were played at the
/// same time
fn loudest_together(frames: &[[f32; 3]], a: usize, b: usize) -> f32 {
    frames.iter().fold(0.0f32, |top, f| top.max(f[a].min(f[b])))
}

/// One CLI of a room of several, with the file its played audio goes to
struct Member {
    _home: tempfile::TempDir,
    played: PathBuf,
    cli: Running,
}

impl Member {
    /// Creates a room, or joins `invite_code`, sending the tone of number `who` of [`tones`]
    fn enter(server: &FakeSignaling, who: usize, invite_code: Option<&str>) -> Self {
        let home = tempfile::tempdir().expect("temp HOME");
        let played = home.path().join("played.f32");
        let mut args: Vec<String> = match invite_code {
            None => vec!["create-room".to_string()],
            Some(code) => vec![
                "join-room".to_string(),
                "--room".to_string(),
                code.to_string(),
            ],
        };
        args.extend(
            [
                "--server",
                server.url(),
                "--name",
                &format!("cli-{who}"),
                "--preset",
                ROOM_PRESET,
                "--sample-rate",
                "48000",
                "--input-tone",
                &tones()[who].to_string(),
                "--output-file",
                played.to_str().expect("utf-8 path"),
            ]
            .map(String::from),
        );
        let cli = Running::start(
            home.path(),
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        Self {
            _home: home,
            played,
            cli,
        }
    }

    /// The invite code a CLI that created a room printed
    fn invite_code(&self) -> String {
        self.cli
            .wait_for("Invite code:")
            .expect("the room should be created")
            .split_whitespace()
            .last()
            .expect("the invite code follows its label")
            .to_string()
    }

    /// What it has played so far
    fn heard(&self) -> Heard {
        let (left, _) = read_played(&self.played);
        Heard(
            left.chunks_exact(frame_samples())
                .map(|frame| tones().map(|tone| tone_amplitude(frame, f64::from(tone), 48_000)))
                .collect(),
        )
    }
}

/// Three CLIs enter a room one after the other; each is linked to both of the others when
/// "Audio is flowing" has been printed twice.
fn room_of_three(server: &FakeSignaling) -> [Member; 3] {
    let first = Member::enter(server, 0, None);
    let code = first.invite_code();
    let second = Member::enter(server, 1, Some(&code));
    let third = Member::enter(server, 2, Some(&code));
    for member in [&first, &second, &third] {
        member
            .cli
            .wait_for_count("Audio is flowing", 2)
            .expect("each CLI should be linked to both of the others");
    }
    [first, second, third]
}

/// Sent centred at half scale, a tone is 0.5 * cos(pi/4) in each channel
const HEARD_AT: std::ops::RangeInclusive<f32> = 0.28..=0.42;

/// What a tone that is not played still measures. A tone that is played is above [`HEARD_AT`]'s start.
const NOT_HEARD: f32 = 0.1;

/// Three CLIs in a room each hear both of the others, at the same time, and not themselves:
/// the third to enter a room was linked to one of the others only, who was linked to the other,
/// so the third heard nobody and nobody heard the third.
///
/// Verifies: REQ-CLI-011
#[test]
fn three_clis_in_a_room_each_hear_both_of_the_others() {
    let _alone = one_audio_session_at_a_time();
    let server = FakeSignaling::start();
    let room = room_of_three(&server);
    std::thread::sleep(Duration::from_millis(4000));
    let heard: Vec<Heard> = room.iter().map(Member::heard).collect();
    drop(room);

    for (me, heard) in heard.iter().enumerate() {
        let (a, b) = match me {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let together = loudest_together(&heard.0, a, b);
        assert!(
            HEARD_AT.contains(&together),
            "CLI {me} should hear the tones of CLI {a} and CLI {b} at once at about 0.35 each, \
             the quieter was {together} at best"
        );
        let itself = loudest(&heard.0, me);
        assert!(
            itself < NOT_HEARD,
            "CLI {me} should not hear its own tone, it was {itself} at most"
        );
    }
}

/// When one of three leaves, the two who stay go on hearing each other and stop hearing the one
/// who left.
///
/// Verifies: REQ-CLI-011
#[test]
fn a_cli_leaves_a_room_of_three_the_other_two_go_on_hearing_each_other() {
    let _alone = one_audio_session_at_a_time();
    let server = FakeSignaling::start();
    let [first, second, third] = room_of_three(&server);
    std::thread::sleep(Duration::from_millis(1500));

    drop(third);
    first
        .cli
        .wait_for("left the room")
        .expect("the room should tell that the third left");
    // What the third sent is still in the play-out buffers for a moment
    std::thread::sleep(Duration::from_millis(2500));
    let after_leaving = first.heard().0.len();
    std::thread::sleep(Duration::from_millis(3000));
    let (heard_by_first, heard_by_second) = (first.heard(), second.heard());
    drop((first, second));

    for (me, heard) in [(0, &heard_by_first), (1, &heard_by_second)] {
        let later = heard.after(after_leaving);
        let stays = 1 - me;
        let still = loudest(later, stays);
        assert!(
            HEARD_AT.contains(&still),
            "CLI {me} should still hear the tone of CLI {stays} at about 0.35, it was {still} at best"
        );
        let gone = loudest(later, 2);
        assert!(
            gone < NOT_HEARD,
            "CLI {me} should no longer hear the tone of the one who left, it was {gone} at most"
        );
    }
}

/// Runs a CLI alone in a room, capturing a 440 Hz tone and writing what it
/// plays to a file, and returns the left channel.
///
/// With no peer the only thing the output can carry is the monitored input, so
/// what comes out is what monitoring does.
fn cli_alone_playing(extra_args: &[&str]) -> Vec<f32> {
    let _alone = one_audio_session_at_a_time();
    let server = FakeSignaling::start();
    let home = tempfile::tempdir().expect("temp HOME");
    let out = home.path().join("played.f32");
    let out_arg = out.display().to_string();
    let mut args = vec![
        "create-room",
        "--server",
        server.url(),
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

    let cli = Running::start(home.path(), &args);
    cli.wait_for("Chat enabled")
        .expect("the CLI should be in its room");
    std::thread::sleep(Duration::from_millis(1500));
    drop(cli);

    read_played(&out).0
}

/// With `--monitor` the CLI hears its own tone at the pitch and level it was
/// captured, before any peer exists to send it anywhere - which is what makes
/// it local, not a round trip.
///
/// Verifies: REQ-AUD-111
/// Verifies: REQ-CLI-006
#[test]
fn a_monitoring_cli_hears_its_own_input_without_a_peer() {
    const SAMPLE_RATE: u32 = 48000;
    let left = cli_alone_playing(&["--monitor"]);

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
    let left = cli_alone_playing(&[]);

    assert!(!left.is_empty(), "the output file should have been written");
    assert!(
        left.iter().all(|s| s.abs() < 1e-6),
        "with monitoring off the output carries only the peers, and there are none"
    );
}

/// A peer that sends every packet straight back, except audio, which it holds for `hold`
/// first - what a peer that replays what it hears does. It agrees keys with whoever talks to it,
/// as an app does, and opens and seals what it carries. Runs until dropped.
struct EchoPeer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EchoPeer {
    fn start(peer: common::Peer, hold: Duration) -> Self {
        use std::sync::atomic::Ordering;

        peer.socket
            .set_read_timeout(Some(Duration::from_millis(2)))
            .expect("set a read timeout");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut held: std::collections::VecDeque<(
                Instant,
                std::net::SocketAddr,
                jamjam::protocol::Packet,
            )> = Default::default();
            while !stopping.load(Ordering::SeqCst) {
                if let Ok(Some((packet, from))) = peer.recv() {
                    if packet.packet_type == jamjam::protocol::PacketType::Audio {
                        held.push_back((Instant::now() + hold, from, packet));
                    } else {
                        peer.send(packet, from);
                    }
                }
                while held
                    .front()
                    .is_some_and(|(due, _, _)| *due <= Instant::now())
                {
                    let (_, to, packet) = held.pop_front().expect("front was just seen");
                    peer.send(packet, to);
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

/// `join-room` into a room with a peer that echoes audio after a hold ends by
/// itself after `--duration`, leaves a JSON report, and that report holds the
/// round trip of the bursts sent - the hold taken off - so a change that slows
/// the audio path shows up as a number. The sound card is replaced by
/// `--input-bursts` and `--output-file`, so this runs wherever `cargo test`
/// runs.
///
/// Verifies: REQ-CLI-007
#[test]
fn a_cli_joined_to_an_echo_reports_the_round_trip_of_its_bursts() {
    let _alone = one_audio_session_at_a_time();
    const HOLD_MS: u64 = 200;
    let echo_peer = common::Peer::bind();
    let address = echo_peer.local_addr().to_string();
    let server = FakeSignaling::start_with_resident_at("echo", echo_peer.local_addr());
    let _echo = EchoPeer::start(echo_peer, Duration::from_millis(HOLD_MS));

    let home = tempfile::tempdir().expect("temp HOME");
    let report_path = home.path().join("report.json");
    let played = home.path().join("played.f32");
    let started = Instant::now();

    let output = run(
        home.path(),
        &[
            "join-room",
            "--server",
            server.url(),
            "--room",
            "echo",
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
