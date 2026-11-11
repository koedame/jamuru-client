//! Tries the app on bad lines and prints what it did, one row per line (ADR-124).
//!
//! The CLI joins a room whose only other member is a peer that sends back the audio it gets,
//! with a simulated bad line in front of that peer (`common::line`), and measures the round trip
//! of its own bursts as `--input-bursts --report-json` does (ADR-039). The row is that report and
//! what the line did to the datagrams. To judge a change to the receive buffer, the concealment
//! or the like, run the same conditions with the app before it and with the app after it and put
//! the rows side by side:
//!
//! ```text
//! cargo test --release --test line_trial -- 4g-busy burst-loss       # these two
//! cargo test --release --test line_trial -- all --duration 30        # every condition
//! cargo test --release --test line_trial -- --list                   # the conditions
//! cargo test --release --test line_trial -- all --app /path/to/jamuru-before
//! ```
//!
//! With no condition named it tries the short set that every `cargo test` runs.
//!
//! The exit code is 0 if every condition was measured, 1 if one could not be (the app did not
//! run to its report, nothing went through the line, or the round trip is shorter than the line
//! allows, which means the line was not in the way), and 2 for a command line that is not
//! understood. The figures themselves are not judged: whether a figure is good is for whoever
//! compares the rows.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::fake_signaling::FakeSignaling;
use common::line::{condition, BadLine, Condition, Counts, CONDITIONS};
use common::{EchoPeer, Peer};
use serde_json::{json, Value};

/// What `cargo test` tries: the line that adds nothing, to see the harness adds nothing either,
/// and one with loss, a narrow rate and a wide spread together
const CHECK_SET: &[&str] = &["fiber", "4g-busy"];
const CHECK_SECONDS: u64 = 6;
const DEFAULT_SECONDS: u64 = 20;

const SAMPLE_RATE: u32 = 48_000;
const TONE_HZ: f64 = 1000.0;

struct Options {
    app: PathBuf,
    seconds: u64,
    seed: u64,
    preset: String,
    json: Option<PathBuf>,
    quality: bool,
    conditions: Vec<&'static Condition>,
}

enum Parsed {
    Run(Options),
    List,
    Help,
}

const USAGE: &str = "usage: line_trial [<condition>... | all] [--duration <seconds>] [--seed <n>] \
[--preset <name>] [--app <jamuru binary>] [--json <path>] [--quality] [--list]";

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut app = PathBuf::from(env!("CARGO_BIN_EXE_jamuru"));
    let (mut seconds, mut seed, mut preset) = (None, 1u64, "balanced".to_string());
    let (mut json, mut quality) = (None, false);
    let mut names: Vec<String> = Vec::new();

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--list" => return Ok(Parsed::List),
            "--help" | "-h" => return Ok(Parsed::Help),
            "--duration" => {
                seconds = Some(
                    value("--duration")?
                        .parse::<u64>()
                        .map_err(|_| "--duration is a whole number of seconds".to_string())?,
                )
            }
            "--seed" => {
                seed = value("--seed")?
                    .parse()
                    .map_err(|_| "--seed is a whole number".to_string())?
            }
            "--preset" => preset = value("--preset")?,
            "--app" => app = PathBuf::from(value("--app")?),
            "--json" => json = Some(PathBuf::from(value("--json")?)),
            "--quality" => quality = true,
            // What `cargo test` and people pass to a test binary
            "--nocapture" | "--quiet" | "-q" | "--show-output" => {}
            flag if flag.starts_with("--test-threads") || flag.starts_with("--color") => {
                if !flag.contains('=') {
                    args.next();
                }
            }
            flag if flag.starts_with('-') => return Err(format!("{flag} is not an option")),
            name => names.push(name.to_string()),
        }
    }

    let check = names.is_empty();
    let conditions = if check {
        CHECK_SET
            .iter()
            .map(|name| condition(name).expect("a condition of the check set"))
            .collect()
    } else if names == ["all"] {
        CONDITIONS.iter().collect()
    } else {
        names
            .iter()
            .map(|name| condition(name).ok_or_else(|| format!("no condition called {name}")))
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(Parsed::Run(Options {
        app,
        seconds: seconds.unwrap_or(if check {
            CHECK_SECONDS
        } else {
            DEFAULT_SECONDS
        }),
        seed,
        preset,
        json,
        quality,
        conditions,
    }))
}

/// One session of the CLI through a line
struct Session {
    report: Value,
    played: PathBuf,
    up: Counts,
    down: Counts,
    _home: tempfile::TempDir,
}

fn run_session(
    options: &Options,
    condition: &Condition,
    input: &[&str],
    extra_seconds: u64,
) -> Result<Session, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("a runtime for the line: {e}"))?;

    let echo = Peer::bind();
    let (echo_addr, echo_key) = (echo.local_addr(), echo.link_key());
    let _echo = EchoPeer::start(echo, Duration::ZERO);
    let line = runtime.block_on(BadLine::start(
        None,
        echo_addr,
        (condition.each_way)(),
        (condition.each_way)(),
        options.seed,
    ));
    let server = FakeSignaling::start_with_resident_at("echo", line.addr(), echo_key);

    let home = tempfile::tempdir().map_err(|e| format!("a home for the app: {e}"))?;
    let report_path = home.path().join("report.json");
    let played = home.path().join("played.f32");
    let seconds = options.seconds + extra_seconds;
    let mut child = Command::new(&options.app)
        .args(["join-room", "--server", server.url(), "--room", "echo"])
        .args(["--preset", &options.preset, "--sample-rate"])
        .arg(SAMPLE_RATE.to_string())
        .args(input)
        .arg("--output-file")
        .arg(&played)
        .args(["--duration", &seconds.to_string()])
        .arg("--report-json")
        .arg(&report_path)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", options.app.display()))?;

    let deadline = Instant::now() + Duration::from_secs(seconds + 60);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the app did not end within {} s", seconds + 60));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        return Err(format!("the app ended with {status}"));
    }
    let report = std::fs::read(&report_path)
        .map_err(|e| format!("no report: {e}"))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| format!("bad report: {e}")))?;
    let (up, down) = (line.up(), line.down());
    Ok(Session {
        report,
        played,
        up,
        down,
        _home: home,
    })
}

/// What a tone that came through the line is like, from the output file: the share of 10 ms
/// windows in which it is gone, and of those in which it is there the share that does not look
/// like the tone (less than 30 dB of the window is the sine of the tone, found by least squares).
/// Counted from the first sound, less the first half second.
fn tone_quality(played: &Path) -> Option<(f64, f64)> {
    let bytes = std::fs::read(played).ok()?;
    let left: Vec<f64> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|frame| f64::from(f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]])))
        .collect();
    let first_sound = left.iter().position(|s| s.abs() > 0.01)?;
    let from = first_sound + SAMPLE_RATE as usize / 2;
    let window = SAMPLE_RATE as usize / 100;
    let w = 2.0 * std::f64::consts::PI * TONE_HZ / f64::from(SAMPLE_RATE);

    // (amplitude, share of the energy that is the tone) of each window
    let fits: Vec<(f64, f64)> = left
        .get(from..)?
        .chunks_exact(window)
        .map(|samples| {
            // 10 whole cycles, so sine and cosine are orthogonal over the window
            let (mut s, mut c, mut energy) = (0.0, 0.0, 0.0);
            for (i, x) in samples.iter().enumerate() {
                s += x * (w * i as f64).sin();
                c += x * (w * i as f64).cos();
                energy += x * x;
            }
            let (a, b) = (2.0 * s / window as f64, 2.0 * c / window as f64);
            let tone = (a * a + b * b) / 2.0 * window as f64;
            ((a * a + b * b).sqrt(), tone / energy.max(1e-12))
        })
        .collect();
    if fits.len() < 100 {
        return None;
    }
    let mut amplitudes: Vec<f64> = fits.iter().map(|f| f.0).collect();
    amplitudes.sort_by(f64::total_cmp);
    let usual = amplitudes[amplitudes.len() / 2];
    let gone = fits.iter().filter(|f| f.0 < 0.1 * usual).count();
    let there: Vec<&(f64, f64)> = fits.iter().filter(|f| f.0 >= 0.1 * usual).collect();
    // 30 dB is 0.1 % of the energy outside the tone
    let damaged = there.iter().filter(|f| f.1 < 0.999).count();
    Some((
        100.0 * gone as f64 / fits.len() as f64,
        100.0 * damaged as f64 / there.len().max(1) as f64,
    ))
}

struct Row {
    cells: Vec<String>,
    json: Value,
}

const HEADER: [&str; 13] = [
    "condition",
    "heard/expected",
    "RTT min (ms)",
    "p50",
    "p95",
    "max",
    "buffer (frames)",
    "played",
    "concealed",
    "late",
    "route moves",
    "lost up/down (%)",
    "sent up/down (kbit/s)",
];

fn number(value: &Value, digits: usize) -> String {
    value
        .as_f64()
        .map_or("-".to_string(), |v| format!("{v:.digits$}"))
}

fn share(counts: &Counts) -> String {
    if counts.offered == 0 {
        return "-".into();
    }
    format!(
        "{:.1}",
        100.0 * counts.dropped() as f64 / counts.offered as f64
    )
}

/// How much the app sent into each way of the line while it was connected, in kbit/s: whether
/// a rate limit set on the line was ever in the way depends on it
fn rate(session: &Session) -> String {
    let seconds = session.report["connected_seconds"].as_f64().unwrap_or(0.0);
    if seconds <= 0.0 {
        return "-".into();
    }
    let kbit = |counts: &Counts| counts.bytes_offered as f64 * 8.0 / 1000.0 / seconds;
    format!("{:.0} / {:.0}", kbit(&session.up), kbit(&session.down))
}

/// The measurement of one condition, or why it could not be made
fn measure(options: &Options, condition: &'static Condition) -> Result<Row, String> {
    let session = run_session(options, condition, &["--input-bursts"], 0)?;
    let report = &session.report;
    let round_trip = &report["round_trip"];
    let playout = &report["playout"];
    let peer = &report["peers"][0];

    if session.up.offered == 0 || session.down.offered == 0 {
        return Err(format!(
            "nothing went through the line (sent {}, came back {})",
            session.up.offered, session.down.offered
        ));
    }
    // Audio cannot come back faster than the line takes it twice, less its spread
    let line = (condition.each_way)();
    let shortest = 2.0 * (line.delay_ms - line.jitter_ms).max(0.0);
    if let Some(median) = round_trip["median_ms"].as_f64() {
        if median < shortest {
            return Err(format!(
                "the round trip is {median} ms, shorter than the {shortest} ms the line takes: the line is not in the way"
            ));
        }
    }

    let mut cells = vec![
        condition.name.to_string(),
        format!(
            "{}/{}",
            round_trip["bursts_heard"], round_trip["bursts_expected"]
        ),
        number(&round_trip["min_ms"], 1),
        number(&round_trip["median_ms"], 1),
        number(&round_trip["p95_ms"], 1),
        number(&round_trip["max_ms"], 1),
        playout["delay_frames"].to_string(),
        playout["frames_played"].to_string(),
        playout["frames_concealed"].to_string(),
        playout["frames_late"].to_string(),
        peer["link"]["route_moves"].to_string(),
        format!("{} / {}", share(&session.up), share(&session.down)),
        rate(&session),
    ];

    let mut json = json!({
        "condition": condition.name,
        "about": condition.about,
        "seed": options.seed,
        "seconds": options.seconds,
        "preset": options.preset,
        "app_version": report["jamjam_version"],
        "round_trip": round_trip,
        "playout": playout,
        "buffer_ms": peer["buffer_ms"],
        "route_moves": peer["link"]["route_moves"],
        "network": report["network"],
        "line": {
            "up": counts_json(&session.up),
            "down": counts_json(&session.down),
        },
    });

    if options.quality {
        let tone = run_session(options, condition, &["--input-tone", "1000"], 3)?;
        let (gone, damaged) = tone_quality(&tone.played)
            .ok_or("the tone could not be measured: too little of it was played")?;
        cells.push(format!("{gone:.1}"));
        cells.push(format!("{damaged:.1}"));
        cells.push(format!(
            "{} / {}; {}",
            share(&tone.up),
            share(&tone.down),
            rate(&tone)
        ));
        json["tone"] = json!({
            "gone_percent": gone,
            "damaged_percent": damaged,
            "playout": tone.report["playout"],
            "line": {
                "up": counts_json(&tone.up),
                "down": counts_json(&tone.down),
            },
        });
    }
    Ok(Row { cells, json })
}

fn counts_json(counts: &Counts) -> Value {
    json!({
        "offered": counts.offered,
        "bytes_offered": counts.bytes_offered,
        "delivered": counts.delivered,
        "lost": counts.lost,
        "queue_full": counts.queue_full,
        "outage": counts.outage,
        "duplicated": counts.duplicated,
        "reordered": counts.reordered,
        "longest_queue_ms": counts.longest_queue_ms,
    })
}

fn print_table(header: &[&str], rows: &[Row]) {
    let line = |cells: Vec<&str>| println!("| {} |", cells.join(" | "));
    line(header.to_vec());
    line(header.iter().map(|_| "---").collect());
    for row in rows {
        line(row.cells.iter().map(String::as_str).collect());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(Parsed::Run(options)) => options,
        Ok(Parsed::List) => {
            for c in CONDITIONS {
                println!("{:<12} {}", c.name, c.about);
            }
            return;
        }
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return;
        }
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            std::process::exit(2);
        }
    };

    println!(
        "app {}, preset {}, {} s per condition, seed {}",
        options.app.display(),
        options.preset,
        options.seconds,
        options.seed
    );
    let started = Instant::now();
    let (mut rows, mut failed) = (Vec::new(), 0);
    for condition in &options.conditions {
        match measure(&options, condition) {
            Ok(row) => rows.push(row),
            Err(why) => {
                eprintln!("{}: could not be measured: {why}", condition.name);
                failed += 1;
            }
        }
    }

    let mut header = HEADER.to_vec();
    if options.quality {
        header.extend([
            "tone gone (%)",
            "tone damaged (%)",
            "tone: lost up/down (%); sent up/down (kbit/s)",
        ]);
    }
    print_table(&header, &rows);
    println!(
        "{} of {} conditions measured in {:.0} s",
        rows.len(),
        options.conditions.len(),
        started.elapsed().as_secs_f64()
    );

    if let Some(path) = &options.json {
        let all: Vec<&Value> = rows.iter().map(|row| &row.json).collect();
        let written = serde_json::to_vec_pretty(&all)
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(path, bytes).map_err(|e| e.to_string()));
        if let Err(why) = written {
            eprintln!("{} could not be written: {why}", path.display());
            failed += 1;
        }
    }
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
