//! jamjam - Low-latency P2P audio communication for musicians

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use jamjam::audio::{
    list_input_devices, list_output_devices, mono_to_wire, AudioConfig, AudioEngine, AudioPreset,
    BurstProbe, BurstSignal, DeviceId, LocalMonitor, PeerRateChange, PlayoutStats, ReceivePath,
    RoundTripReport, ADAPT_INTERVAL, WIRE_CHANNELS,
};
use jamjam::network::{
    candidates_to_addrs, gather_candidates, AudioEncodingConfig, Connection, ConnectionState,
    ConnectionStats, LatencyBreakdown, LocalLatencyInfo, PeerInfo, PeerLatencyInfo,
    SignalingClient, SignalingConnection, SignalingMessage,
};
use jamjam::protocol::LatencyInfoMessage;

#[derive(Parser)]
#[command(name = "jamjam")]
#[command(about = "Low-latency P2P audio communication for musicians")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

/// Audio settings shared by every command that opens a session.
///
/// All optional: an unset flag falls back to the preset and then to the
/// config file the GUI writes, so `jamjam` and the app behave the same way
/// without repeating the flags (ADR-027).
#[derive(clap::Args, Clone, Debug)]
struct AudioArgs {
    /// Preset name (see `jamjam preset list`)
    #[arg(long)]
    preset: Option<String>,

    /// Sample rate in Hz (44100, 48000, 96000)
    #[arg(long)]
    sample_rate: Option<u32>,

    /// Frame size in samples (32, 64, 128, 256)
    #[arg(long)]
    frame_size: Option<u32>,

    /// Input device name (use 'devices list' to see available devices)
    #[arg(long)]
    input_device: Option<String>,

    /// Output device name (use 'devices list' to see available devices)
    #[arg(long)]
    output_device: Option<String>,

    /// Send a sine tone of this frequency instead of capturing from a device
    #[arg(long, value_name = "HZ", conflicts_with = "input_device")]
    input_tone: Option<f32>,

    /// Send a short tone burst every 500 ms instead of capturing from a
    /// device, so the round trip through the whole audio path can be timed
    /// (see `--report-json`)
    #[arg(long, conflicts_with_all = ["input_device", "input_tone"])]
    input_bursts: bool,

    /// Write what would be played to a file instead of an output device
    /// (raw 32-bit float, little-endian, stereo interleaved)
    #[arg(long, value_name = "PATH", conflicts_with = "output_device")]
    output_file: Option<PathBuf>,

    /// Hear your own input as it is captured, without the network delay
    /// (`/monitor` and `/unmonitor` switch it in a room session)
    #[arg(long)]
    monitor: bool,
}

/// How a `host` or `join` session ends and what it leaves behind.
#[derive(clap::Args, Clone, Debug)]
struct RunArgs {
    /// Stop after this many seconds connected (default: run until Ctrl+C)
    #[arg(long, value_name = "SECS")]
    duration: Option<u64>,

    /// Write what the session measured to this file as JSON when it ends
    #[arg(long, value_name = "PATH")]
    report_json: Option<PathBuf>,

    /// How long the peer keeps audio before sending it back, as an echo server
    /// does. Taken off the round trips `--input-bursts` measures
    #[arg(long, value_name = "MS", default_value_t = 0)]
    echo_delay_ms: u64,
}

#[derive(Subcommand)]
enum Commands {
    /// List or choose audio devices
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
    },

    /// List or choose the audio preset
    Preset {
        #[command(subcommand)]
        action: PresetAction,
    },

    /// Host a session on a port, without a signaling server
    Host {
        /// Port to listen on
        #[arg(short, long, default_value = "5000")]
        port: u16,

        #[command(flatten)]
        audio: AudioArgs,

        #[command(flatten)]
        run: RunArgs,
    },

    /// Join a session by address, without a signaling server
    Join {
        /// Remote address (IP:PORT)
        address: String,

        #[command(flatten)]
        audio: AudioArgs,

        #[command(flatten)]
        run: RunArgs,
    },

    /// Create a room on a signaling server and print its invite code
    CreateRoom {
        /// jamjam server URL (e.g., https://example.com). The CLI asks it
        /// where its signaling server is
        #[arg(short, long)]
        server: String,

        /// Room name shown in listings
        #[arg(long, default_value = "CLI Room")]
        room_name: String,

        /// Your display name
        #[arg(short, long, default_value = "CLI User")]
        name: String,

        #[command(flatten)]
        audio: AudioArgs,

        /// Skip audio (chat only mode)
        #[arg(long)]
        chat_only: bool,
    },

    /// Join a room via signaling server
    JoinRoom {
        /// jamjam server URL (e.g., https://example.com). The CLI asks it
        /// where its signaling server is
        #[arg(short, long)]
        server: String,

        /// Room ID or invite code to join
        #[arg(short, long)]
        room: String,

        /// Your display name
        #[arg(short, long, default_value = "CLI User")]
        name: String,

        #[command(flatten)]
        audio: AudioArgs,

        /// Send a single message and exit (non-interactive mode)
        #[arg(short = 'm', long)]
        message: Option<String>,

        /// Timeout in seconds for non-interactive mode (default: 5)
        #[arg(long, default_value = "5")]
        timeout: u64,

        /// Skip audio (chat only mode)
        #[arg(long)]
        chat_only: bool,
    },
}

#[derive(Subcommand)]
enum DevicesAction {
    /// List all devices
    List,

    /// Choose the devices sessions use, saving them for next time
    Set {
        /// Input device name, or 'default' for the system default
        #[arg(long)]
        input: Option<String>,

        /// Output device name, or 'default' for the system default
        #[arg(long)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
enum PresetAction {
    /// List the presets and their latency budgets
    List,

    /// Choose the preset sessions use, saving it for next time
    Use {
        /// Preset name (zero-latency, ultra-low-latency, balanced, high-quality)
        name: String,
    },
}

fn setup_logging(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::INFO };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");
}

fn list_devices() {
    println!("Input devices:");
    match list_input_devices() {
        Ok(devices) => {
            for device in devices {
                let default_marker = if device.is_default { " (default)" } else { "" };
                println!("  - {}{}", device.name, default_marker);
            }
        }
        Err(e) => {
            println!("  Error: {}", e);
        }
    }

    println!("\nOutput devices:");
    match list_output_devices() {
        Ok(devices) => {
            for device in devices {
                let default_marker = if device.is_default { " (default)" } else { "" };
                println!("  - {}{}", device.name, default_marker);
            }
        }
        Err(e) => {
            println!("  Error: {}", e);
        }
    }
}

/// Audio settings after flags, preset and the saved config have been folded
/// together.
#[derive(Debug, Clone)]
struct AudioSettings {
    sample_rate: u32,
    frame_size: u32,
    input_device: Option<String>,
    output_device: Option<String>,
    preset: AudioPreset,
    /// Stand-in for the input device: a sine tone at this frequency.
    input_tone: Option<f32>,
    /// Stand-in for the input device: a tone burst every 500 ms, whose round
    /// trip is timed.
    input_bursts: bool,
    /// Stand-in for the output device: the file played audio is written to.
    output_file: Option<PathBuf>,
    /// Whether the session starts with local monitoring on.
    monitor: bool,
}

impl AudioArgs {
    /// Resolves the settings: an explicit flag wins, then the named preset,
    /// then what the app saved, then the built-in default.
    fn resolve(&self) -> Result<AudioSettings> {
        let config = jamjam::config::load_config().unwrap_or_default();

        let preset = match &self.preset {
            Some(name) => AudioPreset::from_name(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown preset {:?}. `jamjam preset list` shows the available ones",
                    name
                )
            })?,
            None => config.preset,
        };

        // A preset named on the command line sets the frame size with it;
        // otherwise the saved buffer size stands, so the CLI runs at what the
        // GUI is configured for.
        let frame_size = match (self.frame_size, &self.preset) {
            (Some(explicit), _) => explicit,
            (None, Some(_)) => preset.frame_size(),
            (None, None) => config.buffer_size,
        };

        Ok(AudioSettings {
            sample_rate: self.sample_rate.unwrap_or(config.sample_rate),
            frame_size,
            input_device: self.input_device.clone().or(config.input_device_id),
            output_device: self.output_device.clone().or(config.output_device_id),
            preset,
            input_tone: self.input_tone,
            input_bursts: self.input_bursts,
            output_file: self.output_file.clone(),
            monitor: self.monitor,
        })
    }
}

/// Name that means "no explicit choice", so a device can be un-set without
/// editing config.toml by hand.
const DEFAULT_DEVICE_KEYWORD: &str = "default";

/// Saves the device choice both the CLI and the GUI start from.
fn set_devices(input: Option<String>, output: Option<String>) -> Result<()> {
    if input.is_none() && output.is_none() {
        anyhow::bail!(
            "nothing to set - pass --input and/or --output ('{}' clears the choice)",
            DEFAULT_DEVICE_KEYWORD
        );
    }

    let mut config = jamjam::config::load_config().unwrap_or_default();

    if let Some(name) = input {
        let available = list_input_devices().map_err(|e| anyhow::anyhow!("{}", e))?;
        config.input_device_id = resolve_device_choice(&name, &available, "input")?;
    }
    if let Some(name) = output {
        let available = list_output_devices().map_err(|e| anyhow::anyhow!("{}", e))?;
        config.output_device_id = resolve_device_choice(&name, &available, "output")?;
    }

    jamjam::config::save_config(&config).map_err(|e| anyhow::anyhow!("{}", e))?;

    println!(
        "input:  {}",
        config
            .input_device_id
            .as_deref()
            .unwrap_or("(system default)")
    );
    println!(
        "output: {}",
        config
            .output_device_id
            .as_deref()
            .unwrap_or("(system default)")
    );
    Ok(())
}

/// Refuses a device the machine does not offer, rather than saving a name that
/// silently falls back to the default at session start.
fn resolve_device_choice(
    name: &str,
    available: &[jamjam::audio::AudioDevice],
    kind: &str,
) -> Result<Option<String>> {
    if name == DEFAULT_DEVICE_KEYWORD {
        return Ok(None);
    }
    if !available.iter().any(|device| device.name == name) {
        anyhow::bail!(
            "no {} device named {:?}. `jamjam devices list` shows what this machine has",
            kind,
            name
        );
    }
    Ok(Some(name.to_string()))
}

/// Prints the presets with the numbers ADR-019 fixes for them.
fn list_presets() {
    let current = jamjam::config::load_config().unwrap_or_default().preset;

    println!(
        "  {:<18}{:>7}{:>8}{:>12}{:>10}  CODEC",
        "PRESET", "FRAME", "JITTER", "LATENCY", "BUDGET"
    );
    for preset in AudioPreset::all() {
        println!(
            "{} {:<18}{:>7}{:>8}{:>10.2}ms{:>8.2}ms  {:?}",
            if preset == current { "*" } else { " " },
            preset.name(),
            preset.frame_size(),
            preset.jitter_buffer_frames(),
            preset.designed_app_latency_ms(48000),
            preset.max_app_latency_ms(),
            preset.codec_type()
        );
    }
    println!(
        "\nThe preset in use is marked with *. LATENCY is the design value at 48kHz, \
         BUDGET its ceiling (ADR-019)."
    );
}

/// Saves the preset both the CLI and the GUI start from.
fn use_preset(name: &str) -> Result<()> {
    let preset = AudioPreset::from_name(name).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown preset {:?}. `jamjam preset list` shows the available ones",
            name
        )
    })?;

    let mut config = jamjam::config::load_config().unwrap_or_default();
    config.preset = preset.clone();
    // Streaming reads buffer_size, so leaving it behind would run the session
    // at a frame size this preset never chose.
    config.buffer_size = preset.frame_size();
    jamjam::config::save_config(&config).map_err(|e| anyhow::anyhow!("{}", e))?;

    println!(
        "preset: {} ({} samples/frame, {} jitter frame(s), {:.2}ms designed at 48kHz)",
        preset.name(),
        preset.frame_size(),
        preset.jitter_buffer_frames(),
        preset.designed_app_latency_ms(48000)
    );
    Ok(())
}

/// Print session statistics with latency breakdown
fn print_session_stats(
    stats: &ConnectionStats,
    local_info: &LocalLatencyInfo,
    peer_info: Option<&PeerLatencyInfo>,
    playout: &PlayoutStats,
    peer_name: Option<&str>,
) {
    let breakdown = LatencyBreakdown::calculate(
        local_info,
        peer_info,
        stats.rtt_ms.unwrap_or(0.0),
        stats.jitter_ms,
    );
    let peer_label = peer_name.unwrap_or("Peer");

    println!("\n═══════════════════════════════════════════════════════════════");
    println!(" Session Statistics");
    println!("═══════════════════════════════════════════════════════════════");

    // Network stats
    println!("\n Network:");
    match stats.rtt_ms {
        Some(rtt_ms) => println!("   RTT:          {:>7.2} ms", rtt_ms),
        None => println!("   RTT:          measuring..."),
    }
    println!("   Jitter:       {:>7.2} ms", stats.jitter_ms);
    println!("   Packet Loss:  {:>7.1} %", stats.packet_loss_rate * 100.0);
    println!("   Uptime:       {:>7} sec", stats.uptime_seconds);

    // Latency breakdown
    println!("\n Latency Breakdown:");

    // Upstream (You -> Peer)
    println!("\n   Upstream (You → {}):", peer_label);
    println!(
        "     Capture buffer:    {:>6.2} ms  ({} samples @ {} Hz)",
        breakdown.upstream.capture_buffer_ms, local_info.frame_size, local_info.sample_rate
    );
    println!(
        "     Encode ({}):    {:>6.2} ms",
        local_info.codec, breakdown.upstream.encode_ms
    );
    println!(
        "     Network:           {:>6.2} ms  (RTT/2)",
        breakdown.upstream.network_ms
    );

    if breakdown.has_peer_info() {
        println!(
            "     [{}] Jitter buf: {:>6.2} ms",
            peer_label, breakdown.upstream.peer_jitter_buffer_ms
        );
        println!(
            "     [{}] Decode:     {:>6.2} ms",
            peer_label, breakdown.upstream.peer_decode_ms
        );
        println!(
            "     [{}] Playback:   {:>6.2} ms",
            peer_label, breakdown.upstream.peer_playback_buffer_ms
        );
    } else {
        println!("     [{}] (info not available)", peer_label);
    }
    println!("     ─────────────────────────────");
    println!(
        "     Total:             {:>6.2} ms",
        breakdown.upstream_total_ms
    );

    // Downstream (Peer -> You)
    println!("\n   Downstream ({} → You):", peer_label);
    if breakdown.has_peer_info() {
        println!(
            "     [{}] Capture:    {:>6.2} ms",
            peer_label, breakdown.downstream.peer_capture_buffer_ms
        );
        println!(
            "     [{}] Encode:     {:>6.2} ms",
            peer_label, breakdown.downstream.peer_encode_ms
        );
    } else {
        println!("     [{}] (info not available)", peer_label);
    }
    println!(
        "     Network:           {:>6.2} ms  (RTT/2)",
        breakdown.downstream.network_ms
    );
    println!(
        "     Jitter buffer:     {:>6.2} ms",
        breakdown.downstream.jitter_buffer_ms
    );
    println!(
        "     Decode ({}):    {:>6.2} ms",
        local_info.codec, breakdown.downstream.decode_ms
    );
    println!(
        "     Playback buffer:   {:>6.2} ms  ({} samples)",
        breakdown.downstream.playback_buffer_ms, local_info.frame_size
    );
    println!("     ─────────────────────────────");
    println!(
        "     Total:             {:>6.2} ms",
        breakdown.downstream_total_ms
    );

    // Summary
    println!("\n Summary:");
    println!(
        "   Upstream total:    {:>7.2} ms",
        breakdown.upstream_total_ms
    );
    println!(
        "   Downstream total:  {:>7.2} ms",
        breakdown.downstream_total_ms
    );
    println!(
        "   Round-trip total:  {:>7.2} ms",
        breakdown.roundtrip_total_ms
    );

    // What happened to received audio in the play-out buffer - the numbers
    // that show jitter and loss the way the listener heard them.
    println!("\n Play-out:");
    println!("   Played:    {:>10}", playout.frames_played);
    println!("   Concealed: {:>10}", playout.frames_concealed);
    println!("   Late:      {:>10}", playout.frames_late);
    println!("   Resyncs:   {:>10}", playout.resyncs);

    // Packet stats
    println!("\n Packets:");
    println!("   Sent:     {:>10}", stats.packets_sent);
    println!("   Received: {:>10}", stats.packets_received);
    println!("   Bytes sent:     {:>10}", stats.bytes_sent);
    println!("   Bytes received: {:>10}", stats.bytes_received);

    println!("\n═══════════════════════════════════════════════════════════════\n");
}

/// Where a session without a signaling server finds its peer.
enum Direct {
    /// Listen on a port until a peer reaches it.
    Host { port: u16 },
    /// Reach a peer at a known address.
    Join { address: SocketAddr },
}

/// Runs `host` or `join`: one peer, no signaling server, until Ctrl+C or the
/// end of `--duration`.
async fn run_direct(target: Direct, settings: AudioSettings, run: RunArgs) -> Result<()> {
    let bind = match &target {
        Direct::Host { port } => format!("0.0.0.0:{}", port),
        Direct::Join { .. } => "0.0.0.0:0".to_string(),
    };
    let connection = Connection::new(&bind).await?;
    info!("Audio socket: {}", connection.local_addr());

    print_audio_settings(&settings);
    let probe = settings.input_bursts.then(|| Arc::new(BurstProbe::new()));
    let session = AudioSession::start(
        connection,
        &settings,
        Arc::new(AtomicBool::new(false)),
        probe.clone(),
    )?;

    let peer = match target {
        Direct::Host { port } => {
            println!("\nHost started. Listening on port {}.", port);
            println!("Press Ctrl+C to stop.\n");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => None,
                peer = session.accept() => {
                    let peer = peer?;
                    println!("Connected to {}. Session active.", peer);
                    Some(peer)
                }
            }
        }
        Direct::Join { address } => {
            session.connect(&[address]).await?;
            println!("\nConnected to {}. Session active.", address);
            println!("Press Ctrl+C to stop.\n");
            Some(address)
        }
    };

    let connected_at = Instant::now();
    if peer.is_some() {
        match run.duration {
            Some(seconds) => tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
            },
            None => tokio::signal::ctrl_c().await?,
        }
    }
    info!("Shutting down...");
    let outcome = session.finish(None).await;

    if let Some(path) = &run.report_json {
        let round_trip = probe.map(|probe| probe.report(Duration::from_millis(run.echo_delay_ms)));
        let report = session_report(
            &settings,
            peer,
            &outcome,
            connected_at.elapsed(),
            round_trip
                .as_ref()
                .map(|report| (run.echo_delay_ms, report)),
        );
        std::fs::write(path, format!("{}\n", report))
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// Rounds to six decimals, so a report reads `0.495` and not `0.4950000047683716`
/// (an `f32` widened), and a loss rate of 0.04 % is not rounded away.
fn rounded(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// What a `host` / `join` session measured, as one JSON object.
///
/// `round_trip` is present only when the session sent `--input-bursts`.
fn session_report(
    settings: &AudioSettings,
    peer: Option<SocketAddr>,
    outcome: &SessionOutcome,
    connected_for: Duration,
    round_trip: Option<(u64, &RoundTripReport)>,
) -> serde_json::Value {
    let stats = &outcome.stats;
    let round_trip = round_trip.map(|(echo_delay_ms, report)| {
        let delay = report.delay.as_ref();
        serde_json::json!({
            "echo_delay_ms": echo_delay_ms,
            "bursts_sent": report.bursts_sent,
            "bursts_expected": report.bursts_expected,
            "bursts_heard": report.bursts_heard,
            "min_ms": delay.map(|d| rounded(d.min_ms)),
            "mean_ms": delay.map(|d| rounded(d.mean_ms)),
            "median_ms": delay.map(|d| rounded(d.median_ms)),
            "p95_ms": delay.map(|d| rounded(d.p95_ms)),
            "max_ms": delay.map(|d| rounded(d.max_ms)),
        })
    });

    serde_json::json!({
        "jamjam_version": env!("CARGO_PKG_VERSION"),
        "finished_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "peer": peer.map(|peer| peer.to_string()),
        "connected_seconds": rounded(connected_for.as_secs_f64()),
        "audio": {
            "preset": settings.preset.name(),
            "codec": format!("{:?}", settings.preset.codec_type()).to_lowercase(),
            "sample_rate": settings.sample_rate,
            "frame_size": settings.frame_size,
        },
        "network": {
            "rtt_ms": stats.rtt_ms.map(|rtt| rounded(f64::from(rtt))),
            "jitter_ms": rounded(f64::from(stats.jitter_ms)),
            "packet_loss_rate": rounded(f64::from(stats.packet_loss_rate)),
            "packets_sent": stats.packets_sent,
            "packets_received": stats.packets_received,
        },
        "playout": {
            "delay_frames": outcome.delay_frames,
            "frames_played": outcome.playout.frames_played,
            "frames_concealed": outcome.playout.frames_concealed,
            "frames_late": outcome.playout.frames_late,
            "resyncs": outcome.playout.resyncs,
        },
        "round_trip": round_trip,
    })
}

fn print_audio_settings(settings: &AudioSettings) {
    println!(
        "\nAudio: {} preset, {} Hz, {} samples/frame",
        settings.preset.name(),
        settings.sample_rate,
        settings.frame_size
    );
}

/// Process signaling events and print them to stdout
fn handle_signaling_event(msg: &SignalingMessage) {
    match msg {
        SignalingMessage::PeerJoined { peer } => {
            println!("\n📥 {} joined the room", peer.name);
            print!("chat> ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        SignalingMessage::PeerLeft { peer_id } => {
            println!("\n📤 Peer {} left the room", peer_id);
            print!("chat> ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        SignalingMessage::ChatMessage {
            sender_name,
            content,
            ..
        } => {
            println!("\n💬 {}: {}", sender_name, content);
            print!("chat> ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        _ => {}
    }
}

/// Send a chat message via signaling connection
async fn send_chat_message(
    conn: &mut SignalingConnection,
    peer_id: &str,
    peer_name: &str,
    content: &str,
) -> Result<()> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    conn.send(SignalingMessage::ChatMessage {
        sender_id: peer_id.to_string(),
        sender_name: peer_name.to_string(),
        content: content.to_string(),
        timestamp,
    })
    .await?;

    Ok(())
}

/// A signaling client for `server`, proving this installation's device
/// identity - the one the GUI on this machine uses too.
fn signaling_client(server: &str) -> SignalingClient {
    let identity = jamjam::identity_store::load_installation_identity();
    SignalingClient::new(server, Arc::new(identity))
}

#[allow(clippy::too_many_arguments)]
/// How the CLI enters a room.
enum RoomEntry {
    /// Create a room and print the invite code others join with.
    Create { room_name: String },
    /// Join an existing room by id or invite code.
    Join { room_id: String },
}

/// What the user typed at the `chat>` prompt.
enum Input {
    /// Send it to the room.
    Chat(String),
    /// A command that was carried out here; nothing to send.
    Handled,
    /// Print the connection statistics so far.
    Stats,
    /// Leave the session.
    Quit,
}

/// Interprets a line as a command or as chat.
///
/// Slash commands exist because a debugging session needs the controls the
/// mixer offers - muting above all - without a second window (ADR-027).
fn handle_input_line(line: &str, is_muted: &AtomicBool, monitor: Option<&LocalMonitor>) -> Input {
    let audio_active = monitor.is_some();
    match line {
        "/help" | "/?" => {
            println!("  /mute      stop sending audio");
            println!("  /unmute    resume sending audio");
            println!("  /monitor   hear your own input, without the network delay");
            println!("  /unmonitor stop hearing your own input");
            println!("  /stats     show the connection so far");
            println!("  /quit      leave the room");
            println!("  anything else is sent to the room as chat");
            Input::Handled
        }
        "/mute" | "/unmute" => {
            if !audio_active {
                println!("(no audio in this session, nothing to mute)");
                return Input::Handled;
            }
            let mute = line == "/mute";
            is_muted.store(mute, Ordering::SeqCst);
            println!("{}", if mute { "🔇 muted" } else { "🔊 unmuted" });
            Input::Handled
        }
        "/monitor" | "/unmonitor" => {
            let Some(monitor) = monitor else {
                println!("(no audio in this session, nothing to monitor)");
                return Input::Handled;
            };
            let on = line == "/monitor";
            monitor.set_enabled(on);
            println!(
                "{}",
                if on {
                    "🎧 monitoring on"
                } else {
                    "🎧 monitoring off"
                }
            );
            Input::Handled
        }
        "/stats" => Input::Stats,
        "/quit" | "/exit" => Input::Quit,
        other if other.starts_with('/') => {
            println!("unknown command {:?} - /help lists them", other);
            Input::Handled
        }
        other => Input::Chat(other.to_string()),
    }
}

/// Addresses a peer can be reached at, preferring gathered candidates over the
/// legacy single address.
fn peer_addrs(peer: &PeerInfo) -> Vec<std::net::SocketAddr> {
    if !peer.candidates.is_empty() {
        candidates_to_addrs(&peer.candidates)
    } else if let Some(addr) = peer.public_addr {
        vec![addr]
    } else {
        vec![]
    }
}

/// Runs a room session: create or join, then chat and (unless `chat_only`)
/// audio with the first peer that publishes an address.
///
/// One function for both entry points because everything after the first
/// message is identical, and because both need to start streaming when a
/// peer's address arrives *later* - the creator is alone in the room until
/// someone joins (the defect ADR-026 fixed in the GUI).
#[allow(clippy::too_many_arguments)]
async fn run_room_session(
    server: String,
    entry: RoomEntry,
    peer_name: String,
    audio: AudioSettings,
    message: Option<String>,
    timeout_secs: u64,
    chat_only: bool,
) -> Result<()> {
    info!("Connecting through the jamjam server: {}", server);
    let client = signaling_client(&server);
    let mut conn = client.connect().await?;

    let request = match &entry {
        RoomEntry::Create { room_name } => SignalingMessage::CreateRoom {
            room_name: room_name.clone(),
            password: None,
            peer_name: peer_name.clone(),
        },
        RoomEntry::Join { room_id } => SignalingMessage::JoinRoom {
            room_id: room_id.clone(),
            password: None,
            peer_name: peer_name.clone(),
        },
    };
    conn.send(request).await?;

    let (my_peer_id, mut peers) = match conn.recv().await? {
        SignalingMessage::RoomCreated {
            room_id,
            peer_id,
            invite_code,
        } => {
            println!("\nRoom created: {}", room_id);
            println!("Invite code:  {}", invite_code);
            println!(
                "Others join with:\n  jamjam join-room --server {} --room {}",
                server, invite_code
            );
            println!("Your peer ID: {}", peer_id);
            (peer_id, Vec::new())
        }
        SignalingMessage::RoomJoined {
            room_id,
            peer_id,
            invite_code,
            peers,
        } => {
            println!("\nJoined room: {}", room_id);
            if !invite_code.is_empty() {
                println!("Invite code:  {}", invite_code);
            }
            println!("Your peer ID: {}", peer_id);
            println!("\nPeers in room ({}):", peers.len());
            for peer in &peers {
                println!(
                    "  - {} (id: {}, addr: {:?})",
                    peer.name, peer.id, peer.public_addr
                );
            }
            (peer_id, peers)
        }
        SignalingMessage::Error { message } => {
            anyhow::bail!("Signaling server refused the request: {}", message);
        }
        other => anyhow::bail!("Unexpected response from server: {:?}", other),
    };

    let my_peer_id_str = my_peer_id.to_string();

    // Audio setup happens before any peer is known: the socket has to be bound
    // and published for the *other* side to have somewhere to send to.
    let is_muted = Arc::new(AtomicBool::new(false));
    let audio: Option<AudioSession> = if chat_only {
        println!("\n📝 Chat-only mode (no audio)");
        None
    } else {
        print_audio_settings(&audio);
        let connection = Connection::new("0.0.0.0:0").await?;
        publish_address(&mut conn, &connection).await?;
        Some(AudioSession::start(
            connection,
            &audio,
            is_muted.clone(),
            None,
        )?)
    };

    // Signaling events arrive on their own task so the main loop can select
    // over audio, chat and the keyboard at once.
    let signaling_conn_arc = Arc::new(tokio::sync::Mutex::new(conn));
    let signaling_for_recv = signaling_conn_arc.clone();
    let (tx_signaling, mut rx_signaling) = tokio::sync::mpsc::channel::<SignalingMessage>(32);
    let signaling_recv_task = tokio::spawn(async move {
        loop {
            let msg = {
                let mut conn_guard = signaling_for_recv.lock().await;
                match tokio::time::timeout(Duration::from_millis(100), conn_guard.recv()).await {
                    Ok(Ok(msg)) => Some(msg),
                    Ok(Err(_)) => None,
                    Err(_) => None,
                }
            };
            if let Some(msg) = msg {
                if tx_signaling.send(msg).await.is_err() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    // Non-interactive: say one thing, wait for a reply, leave. Used by scripts
    // and smoke tests.
    if let Some(msg_to_send) = message {
        let result = run_single_message(
            &signaling_conn_arc,
            &mut rx_signaling,
            &my_peer_id_str,
            &peer_name,
            &msg_to_send,
            timeout_secs,
        )
        .await;
        signaling_recv_task.abort();
        leave_room(&signaling_conn_arc).await;
        return result;
    }

    println!("\n💬 Chat enabled. Type a message and press Enter to send.");
    println!("   /help lists the session commands. Ctrl+C to stop.\n");
    print!("chat> ");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    let stdin = tokio::io::stdin();
    let mut stdin_reader = BufReader::new(stdin).lines();

    // Connect to the first peer that has an address, whether it was already in
    // the room or turns up later.
    let mut peer_display_name: Option<String> = None;
    for peer in peers.drain(..) {
        if let Some(session) = audio.as_ref() {
            if maybe_connect(session, &peer).await? {
                peer_display_name = Some(peer.name.clone());
                break;
            }
        }
    }

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
                break;
            }
            Some(msg) = rx_signaling.recv() => {
                if let SignalingMessage::ChatMessage { sender_id, .. } = &msg {
                    if sender_id == &my_peer_id_str {
                        continue;
                    }
                }
                handle_signaling_event(&msg);

                // A peer that just published an address is one we can stream to.
                let peer = match &msg {
                    SignalingMessage::PeerJoined { peer } | SignalingMessage::PeerUpdated { peer } => Some(peer.clone()),
                    _ => None,
                };
                if let (Some(peer), Some(session)) = (peer, audio.as_ref()) {
                    if maybe_connect(session, &peer).await? {
                        peer_display_name = Some(peer.name.clone());
                    }
                }
            }
            line_result = stdin_reader.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if !line.is_empty() {
                            match handle_input_line(line, &is_muted, audio.as_ref().map(AudioSession::monitor)) {
                                Input::Quit => break,
                                Input::Handled => {}
                                Input::Stats => match audio.as_ref() {
                                    Some(session) => session.print_stats(peer_display_name.as_deref()).await,
                                    None => println!("(no audio in this session, nothing to measure)"),
                                },
                                Input::Chat(text) => {
                                    let mut conn_guard = signaling_conn_arc.lock().await;
                                    if let Err(e) = send_chat_message(
                                        &mut conn_guard,
                                        &my_peer_id_str,
                                        &peer_name,
                                        &text,
                                    ).await {
                                        warn!("Failed to send chat: {}", e);
                                    } else {
                                        println!("💬 You: {}", text);
                                    }
                                }
                            }
                        }
                        print!("chat> ");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    Ok(None) => {
                        info!("stdin closed");
                    }
                    Err(e) => {
                        warn!("stdin error: {}", e);
                    }
                }
            }
        }
    }

    signaling_recv_task.abort();

    if let Some(session) = audio {
        session.finish(peer_display_name.as_deref()).await;
    }

    leave_room(&signaling_conn_arc).await;
    Ok(())
}

/// Publishes where `connection` can be reached. Done before any peer is
/// known, because the peer needs an address to send to (ADR-026).
async fn publish_address(conn: &mut SignalingConnection, connection: &Connection) -> Result<()> {
    let local_addr = connection.local_addr();
    info!("Local UDP socket: {}", local_addr);

    let candidates = gather_candidates(connection.socket()).await;
    info!("Gathered {} address candidates", candidates.len());
    conn.send(SignalingMessage::UpdatePeerInfo {
        candidates: candidates.clone(),
        public_addr: candidates.first().map(|c| c.address),
        local_addr: Some(local_addr),
    })
    .await?;
    Ok(())
}

/// The audio half of a session.
///
/// Received audio takes the app's path - decoded on arrival, held in the
/// play-out buffer, concealed when a frame is missing, FEC-recovered where the
/// preset sends FEC - so a problem with jitter or loss sounds here the way it
/// sounds in the app (ADR-027, ADR-028). What is sent matches the app too:
/// stereo, centred, in the preset's codec.
struct AudioSession {
    connection: Arc<tokio::sync::Mutex<Connection>>,
    receive: ReceivePath,
    local_info: LocalLatencyInfo,
    send_task: tokio::task::JoinHandle<()>,
    /// Lets the play-out delay follow the link, as the app does (ADR-031).
    adapt_task: tokio::task::JoinHandle<()>,
    monitor: LocalMonitor,
    /// Held for as long as the session runs; dropping it stops the audio.
    io: AudioIo,
}

impl AudioSession {
    /// Configures `connection` and starts capture and playback.
    ///
    /// The connection must not be connected yet: connecting starts the receive
    /// loop, which takes the callbacks installed here.
    ///
    /// With a `probe`, the bursts sent and the bursts played out are stamped
    /// on it.
    fn start(
        mut connection: Connection,
        settings: &AudioSettings,
        is_muted: Arc<AtomicBool>,
        probe: Option<Arc<BurstProbe>>,
    ) -> Result<Self> {
        let preset = &settings.preset;
        let codec_type = preset.codec_type();
        connection.set_audio_encoding(AudioEncodingConfig {
            codec_type,
            sample_rate: settings.sample_rate,
            channels: WIRE_CHANNELS as u16,
            frame_size: settings.frame_size,
            bitrate: 0,
            fec_group_size: preset.fec_group_size(),
        })?;

        let receive = ReceivePath::new(
            codec_type,
            settings.sample_rate,
            settings.frame_size,
            preset.jitter_buffer_frames(),
        )?;

        let for_audio = receive.clone();
        connection.set_audio_callback(move |sequence, payload, _timestamp| {
            if !for_audio.receive(sequence, &payload) {
                tracing::trace!("Play-out buffer refused frame {}", sequence);
            }
        });

        let for_rate = receive.clone();
        connection.set_latency_info_callback(move |peer| {
            match for_rate.follow_peer_rate(peer.sample_rate) {
                PeerRateChange::Unchanged => {}
                PeerRateChange::Resampling {
                    from,
                    to,
                    latency_ms,
                } => info!(
                    "Converting the peer's audio: {} Hz -> {} Hz (+{:.2} ms)",
                    from, to, latency_ms
                ),
                PeerRateChange::Passthrough => info!("Peer runs at our sample rate"),
                PeerRateChange::Failed(e) => {
                    warn!(
                        "Cannot convert the peer's sample rate, playing as is: {}",
                        e
                    )
                }
            }
        });

        // Sequence numbers carry on across an outage, so what was waiting
        // before it must not play after it (ADR-022).
        let for_state = receive.clone();
        connection.set_state_change_callback(move |state| {
            if state == ConnectionState::Connected {
                for_state.reset();
            }
        });

        let monitor = LocalMonitor::new(settings.frame_size);
        monitor.set_enabled(settings.monitor);

        let (tx_capture, mut rx_capture) = tokio::sync::mpsc::channel::<(Vec<f32>, u32)>(64);
        let io = AudioIo::start(
            settings,
            &receive,
            &monitor,
            probe.clone(),
            move |samples, timestamp| {
                let _ = tx_capture.try_send((samples.to_vec(), timestamp));
            },
        )?;

        let connection = Arc::new(tokio::sync::Mutex::new(connection));
        let connection_for_send = connection.clone();
        let send_task = tokio::spawn(async move {
            let mut stereo = Vec::new();
            while let Some((samples, timestamp)) = rx_capture.recv().await {
                if is_muted.load(Ordering::SeqCst) {
                    continue;
                }
                stereo.resize(samples.len() * WIRE_CHANNELS, 0.0);
                mono_to_wire(&samples, 1.0, 0, &mut stereo);
                let conn = connection_for_send.lock().await;
                if conn.is_connected() {
                    if let Some(probe) = &probe {
                        probe.note_sent(&stereo);
                    }
                    if let Err(e) = conn.send_audio(&stereo, timestamp).await {
                        warn!("Failed to send audio: {}", e);
                    }
                }
            }
        });

        let for_adapt = receive.clone();
        let adapt_task = tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(
                tokio::time::Instant::now() + ADAPT_INTERVAL,
                ADAPT_INTERVAL,
            );
            loop {
                tick.tick().await;
                if let Some(frames) = for_adapt.adapt() {
                    info!("Play-out delay adjusted to {} frame(s)", frames);
                }
            }
        });

        let codec = format!("{:?}", codec_type).to_lowercase();
        let mut local_info =
            LocalLatencyInfo::from_audio_config(settings.frame_size, settings.sample_rate, &codec);
        local_info.set_jitter_buffer_ms(preset.jitter_buffer_frames() as f32 * frame_ms(settings));

        Ok(Self {
            connection,
            receive,
            local_info,
            send_task,
            adapt_task,
            monitor,
            io,
        })
    }

    fn monitor(&self) -> &LocalMonitor {
        &self.monitor
    }

    /// Waits for a peer to reach the socket, and connects back to it.
    async fn accept(&self) -> Result<SocketAddr> {
        let peer = self.connection.lock().await.accept().await?;
        self.announce().await;
        Ok(peer)
    }

    /// Connects to the first of `addrs` that answers.
    async fn connect(&self, addrs: &[SocketAddr]) -> Result<()> {
        self.connection
            .lock()
            .await
            .connect_with_candidates(addrs)
            .await?;
        self.announce().await;
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        self.connection.lock().await.is_connected()
    }

    /// Tells the peer our sample rate and buffering, so it can convert our
    /// audio and show the latency we add (ADR-013).
    async fn announce(&self) {
        let info = &self.local_info;
        let message = LatencyInfoMessage {
            capture_buffer_ms: info.capture_buffer_ms,
            playback_buffer_ms: info.playback_buffer_ms,
            encode_ms: info.encode_ms,
            decode_ms: info.decode_ms,
            jitter_buffer_ms: info.jitter_buffer_ms,
            frame_size: info.frame_size,
            sample_rate: info.sample_rate,
            codec: info.codec.clone(),
            // The CLI always upmixes its mono capture to WIRE_CHANNELS before
            // sending (see `mono_to_wire` above), so that's what's actually
            // on the wire regardless of any per-user setting.
            channel_count: WIRE_CHANNELS as u8,
        };
        if let Err(e) = self
            .connection
            .lock()
            .await
            .send_latency_info(&message)
            .await
        {
            warn!("Failed to send latency info: {}", e);
        }
    }

    async fn print_stats(&self, peer_name: Option<&str>) {
        let connection = self.connection.lock().await;
        print_session_stats(
            &connection.stats(),
            &self.local_info,
            connection.peer_latency_info().as_ref(),
            &self.receive.stats(),
            peer_name,
        );
    }

    /// Stops the audio, disconnects and prints what the session looked like.
    async fn finish(self, peer_name: Option<&str>) -> SessionOutcome {
        self.send_task.abort();
        self.adapt_task.abort();
        let (stats, peer_info) = {
            let mut connection = self.connection.lock().await;
            let stats = connection.stats();
            let peer_info = connection.peer_latency_info();
            connection.disconnect();
            (stats, peer_info)
        };
        drop(self.io);

        let playout = self.receive.stats();
        print_session_stats(
            &stats,
            &self.local_info,
            peer_info.as_ref(),
            &playout,
            peer_name,
        );
        SessionOutcome {
            stats,
            playout,
            delay_frames: self.receive.delay_frames(),
        }
    }
}

/// What a session looked like when it ended.
struct SessionOutcome {
    stats: ConnectionStats,
    playout: PlayoutStats,
    /// The play-out delay in force at the end, after any adaptation.
    delay_frames: u32,
}

fn frame_ms(settings: &AudioSettings) -> f32 {
    settings.frame_size as f32 / settings.sample_rate as f32 * 1000.0
}

/// Peak level of `--input-tone`, below full scale so the centred stereo mix
/// cannot clip.
const TONE_AMPLITUDE: f32 = 0.5;

/// Fills a captured frame with a stand-in signal.
type SignalGenerator = Box<dyn FnMut(&mut [f32]) + Send>;

/// Capture and playback, on the devices or on their stand-ins.
///
/// The stand-ins (`--input-tone`, `--output-file`) let a session run where
/// there is no sound card, and make what was sent and what was heard
/// reproducible. They run at the session's own frame clock, as a device would.
struct AudioIo {
    capture: AudioEngine,
    playback: AudioEngine,
    /// Cleared to stop the stand-in threads.
    running: Arc<AtomicBool>,
    stand_ins: Vec<std::thread::JoinHandle<()>>,
}

impl AudioIo {
    /// Starts capture, handing each mono frame and its sample timestamp to
    /// `on_capture`, and playback, taking frames from `receive`. What is
    /// captured also goes to `monitor`, which mixes it into what is played
    /// while it is on. What is played is also shown to `probe`.
    fn start<F>(
        settings: &AudioSettings,
        receive: &ReceivePath,
        monitor: &LocalMonitor,
        probe: Option<Arc<BurstProbe>>,
        mut on_capture: F,
    ) -> Result<Self>
    where
        F: FnMut(&[f32], u32) + Send + 'static,
    {
        let config = |channels: usize| AudioConfig {
            sample_rate: settings.sample_rate,
            channels: channels as u16,
            frame_size: settings.frame_size,
        };
        let mut io = Self {
            capture: AudioEngine::new(config(1)),
            playback: AudioEngine::new(config(WIRE_CHANNELS)),
            running: Arc::new(AtomicBool::new(true)),
            stand_ins: Vec::new(),
        };

        let stand_in: Option<SignalGenerator> = if let Some(frequency) = settings.input_tone {
            let step = std::f64::consts::TAU * frequency as f64 / settings.sample_rate as f64;
            let mut angle = 0.0f64;
            Some(Box::new(move |frame: &mut [f32]| {
                for sample in frame.iter_mut() {
                    *sample = TONE_AMPLITUDE * angle.sin() as f32;
                    angle = (angle + step) % std::f64::consts::TAU;
                }
            }))
        } else if settings.input_bursts {
            let mut signal = BurstSignal::new(settings.sample_rate);
            Some(Box::new(move |frame: &mut [f32]| signal.fill(frame)))
        } else {
            None
        };

        match stand_in {
            Some(mut fill) => {
                let mut timestamp = 0u32;
                let mut frame = vec![0.0f32; settings.frame_size as usize];
                let mut tap = monitor.tap();
                io.stand_ins
                    .push(run_at_frame_rate(settings, io.running.clone(), move || {
                        fill(&mut frame);
                        tap.push(&frame);
                        on_capture(&frame, timestamp);
                        timestamp = timestamp.wrapping_add(frame.len() as u32);
                    }));
            }
            None => {
                let mut tap = monitor.tap();
                io.capture.start_capture(
                    settings.input_device.clone().map(DeviceId).as_ref(),
                    move |samples, timestamp| {
                        tap.push(samples);
                        on_capture(samples, timestamp as u32)
                    },
                )?
            }
        }

        let for_output = receive.clone();
        let for_monitor = monitor.clone();
        let source = move |out: &mut [f32]| {
            let samples = for_output.read_into(out).samples;
            // Before the monitor mix: what the peer sent, not our own input.
            if let Some(probe) = &probe {
                probe.note_played(&out[..samples]);
            }
            for_monitor.mix_into(&mut out[..samples]);
            samples
        };
        match &settings.output_file {
            Some(path) => {
                let mut file = std::fs::File::create(path)
                    .with_context(|| format!("cannot write {}", path.display()))?;
                let mut frame = vec![0.0f32; receive.frame_samples() * 3];
                let mut bytes = Vec::with_capacity(frame.len() * 4);
                let path = path.to_path_buf();
                io.stand_ins
                    .push(run_at_frame_rate(settings, io.running.clone(), move || {
                        let samples = source(&mut frame);
                        bytes.clear();
                        bytes.extend(frame[..samples].iter().flat_map(|s| s.to_le_bytes()));
                        // Unbuffered on purpose: whatever was played is on disk
                        // even if the process is killed.
                        if let Err(e) = file.write_all(&bytes) {
                            warn!("Failed to write {}: {}", path.display(), e);
                        }
                    }));
            }
            None => io.playback.start_playback_with_source(
                settings.output_device.clone().map(DeviceId).as_ref(),
                receive.frame_samples(),
                source,
            )?,
        }

        Ok(io)
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.capture.stop_capture();
        self.playback.stop_playback();
        for stand_in in self.stand_ins.drain(..) {
            let _ = stand_in.join();
        }
    }
}

/// Calls `tick` once per frame period on its own thread until `running`
/// clears - the clock a device would otherwise provide. Deadlines are counted
/// from the start, so a late wake-up does not slow the stream down.
fn run_at_frame_rate(
    settings: &AudioSettings,
    running: Arc<AtomicBool>,
    mut tick: impl FnMut() + Send + 'static,
) -> std::thread::JoinHandle<()> {
    let period = Duration::from_secs_f64(settings.frame_size as f64 / settings.sample_rate as f64);
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut frames = 0u32;
        while running.load(Ordering::SeqCst) {
            tick();
            frames = frames.wrapping_add(1);
            let due = start + period * frames;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
    })
}

/// Connects to `peer` if it has an address and we are not connected yet.
/// Returns whether this call established the connection.
async fn maybe_connect(session: &AudioSession, peer: &PeerInfo) -> Result<bool> {
    let addrs = peer_addrs(peer);
    if addrs.is_empty() || session.is_connected().await {
        return Ok(false);
    }

    println!(
        "\nConnecting to {} ({} address candidate(s))...",
        peer.name,
        addrs.len()
    );
    session.connect(&addrs).await?;
    println!("Connected. Audio is flowing.\n");
    print!("chat> ");
    let _ = std::io::stdout().flush();
    Ok(true)
}

/// Sends one message, waits for a reply, and reports whether one came.
async fn run_single_message(
    signaling: &Arc<tokio::sync::Mutex<SignalingConnection>>,
    rx_signaling: &mut tokio::sync::mpsc::Receiver<SignalingMessage>,
    my_peer_id: &str,
    peer_name: &str,
    message: &str,
    timeout_secs: u64,
) -> Result<()> {
    println!("📤 Sending: {}", message);
    {
        let mut conn_guard = signaling.lock().await;
        send_chat_message(&mut conn_guard, my_peer_id, peer_name, message).await?;
    }

    let deadline = Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();
    let mut received_response = false;
    println!("⏳ Waiting for response (timeout: {}s)...", timeout_secs);

    while start.elapsed() <= deadline {
        tokio::select! {
            Some(msg) = rx_signaling.recv() => {
                if let SignalingMessage::ChatMessage { sender_id, sender_name, content, .. } = &msg {
                    if sender_id != my_peer_id {
                        println!("📥 {}: {}", sender_name, content);
                        received_response = true;
                        // A small window for anything that follows.
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                } else {
                    handle_signaling_event(&msg);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    if !received_response {
        println!("⚠️  Timeout: no response received");
    }
    Ok(())
}

async fn leave_room(signaling: &Arc<tokio::sync::Mutex<SignalingConnection>>) {
    let mut conn_guard = signaling.lock().await;
    let _ = conn_guard.send(SignalingMessage::LeaveRoom).await;
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    setup_logging(cli.verbose);

    match cli.command {
        Commands::Devices { action } => match action {
            DevicesAction::List => list_devices(),
            DevicesAction::Set { input, output } => set_devices(input, output)?,
        },
        Commands::Preset { action } => match action {
            PresetAction::List => list_presets(),
            PresetAction::Use { name } => use_preset(&name)?,
        },
        Commands::Host { port, audio, run } => {
            run_direct(Direct::Host { port }, audio.resolve()?, run).await?;
        }
        Commands::Join {
            address,
            audio,
            run,
        } => {
            let address = address
                .parse()
                .with_context(|| format!("{:?} is not an IP:PORT address", address))?;
            run_direct(Direct::Join { address }, audio.resolve()?, run).await?;
        }
        Commands::CreateRoom {
            server,
            room_name,
            name,
            audio,
            chat_only,
        } => {
            run_room_session(
                server,
                RoomEntry::Create { room_name },
                name,
                audio.resolve()?,
                None,
                0,
                chat_only,
            )
            .await?;
        }
        Commands::JoinRoom {
            server,
            room,
            name,
            audio,
            message,
            timeout,
            chat_only,
        } => {
            run_room_session(
                server,
                RoomEntry::Join { room_id: room },
                name,
                audio.resolve()?,
                message,
                timeout,
                chat_only,
            )
            .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session commands are what make the CLI usable for debugging without
    /// a second window (ADR-027): they have to be recognised, act, and not be
    /// mistaken for chat.
    ///
    /// Verifies: REQ-CLI-002
    #[test]
    fn session_commands_are_recognised_and_act() {
        let muted = AtomicBool::new(false);
        let monitor = LocalMonitor::new(64);

        assert!(matches!(
            handle_input_line("/mute", &muted, Some(&monitor)),
            Input::Handled
        ));
        assert!(muted.load(Ordering::SeqCst), "/mute should mute");

        assert!(matches!(
            handle_input_line("/unmute", &muted, Some(&monitor)),
            Input::Handled
        ));
        assert!(!muted.load(Ordering::SeqCst), "/unmute should unmute");

        assert!(matches!(
            handle_input_line("/stats", &muted, Some(&monitor)),
            Input::Stats
        ));
        assert!(matches!(
            handle_input_line("/help", &muted, Some(&monitor)),
            Input::Handled
        ));
        assert!(matches!(
            handle_input_line("/quit", &muted, Some(&monitor)),
            Input::Quit
        ));
    }

    /// Anything that is not a command is chat, and an unrecognised command is
    /// not sent to the room by accident.
    ///
    /// Verifies: REQ-CLI-002
    #[test]
    fn only_commands_are_intercepted() {
        let muted = AtomicBool::new(false);
        let monitor = LocalMonitor::new(64);

        match handle_input_line("hello everyone", &muted, Some(&monitor)) {
            Input::Chat(text) => assert_eq!(text, "hello everyone"),
            _ => panic!("plain text should be chat"),
        }

        assert!(
            matches!(
                handle_input_line("/mutee", &muted, Some(&monitor)),
                Input::Handled
            ),
            "a mistyped command must not be broadcast as chat"
        );
        assert!(
            !muted.load(Ordering::SeqCst),
            "a mistyped command must not change anything"
        );
    }

    /// Muting is meaningless in a chat-only session; saying so beats silently
    /// setting a flag nothing reads.
    ///
    /// Verifies: REQ-CLI-002
    #[test]
    fn muting_without_audio_reports_that_there_is_nothing_to_mute() {
        let muted = AtomicBool::new(false);

        assert!(matches!(
            handle_input_line("/mute", &muted, None),
            Input::Handled
        ));
        assert!(
            !muted.load(Ordering::SeqCst),
            "there is no audio to mute, so the flag stays put"
        );
    }

    /// `/monitor` is the CLI's switch for hearing yourself; it has to reach the
    /// monitor the audio callbacks read, not just print a line.
    ///
    /// Verifies: REQ-CLI-006
    #[test]
    fn monitor_commands_switch_local_monitoring() {
        let muted = AtomicBool::new(false);
        let monitor = LocalMonitor::new(64);

        assert!(matches!(
            handle_input_line("/monitor", &muted, Some(&monitor)),
            Input::Handled
        ));
        assert!(monitor.is_enabled(), "/monitor should turn monitoring on");

        assert!(matches!(
            handle_input_line("/unmonitor", &muted, Some(&monitor)),
            Input::Handled
        ));
        assert!(
            !monitor.is_enabled(),
            "/unmonitor should turn monitoring off"
        );
        assert!(
            !muted.load(Ordering::SeqCst),
            "monitoring is what you hear; it does not touch what is sent"
        );
    }

    /// Verifies: REQ-CLI-006
    #[test]
    fn monitoring_without_audio_reports_that_there_is_nothing_to_monitor() {
        let muted = AtomicBool::new(false);

        assert!(matches!(
            handle_input_line("/monitor", &muted, None),
            Input::Handled
        ));
    }
}
