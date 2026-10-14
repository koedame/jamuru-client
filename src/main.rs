//! jamuru - Low-latency P2P audio communication for musicians

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use jamjam::audio::{
    choose_device, input_devices, output_devices, AudioDevice, AudioPreset, BurstProbe,
    ChooseDeviceError, PlayoutStats, RoundTripReport, ADAPT_INTERVAL,
};
use jamjam::audio_session::{self, Command, InputSignal, LinkTarget, Params, Running, Shared};
use jamjam::network::roster::{Audio, Roster};
use jamjam::network::{
    bind_std, gather_candidates_on, AddressCandidate, ConnectionStats, EnterError, EnterRequest,
    LatencyBreakdown, LinkHealth, LinkIdentity, LocalLatencyInfo, PeerLatencyInfo, RoomEntry,
    SignalingClient, SignalingConnection, SignalingMessage,
};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "jamuru")]
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
/// config file the GUI writes, so `jamuru` and the app behave the same way
/// without repeating the flags (ADR-027).
#[derive(clap::Args, Clone, Debug)]
struct AudioArgs {
    /// Preset name (see `jamuru preset list`)
    #[arg(long)]
    preset: Option<String>,

    /// Sample rate in Hz (44100, 48000, 96000)
    #[arg(long)]
    sample_rate: Option<u32>,

    /// Frame size in samples (32, 64, 128, 256)
    #[arg(long)]
    frame_size: Option<u32>,

    /// Input device id or name (use 'devices list' to see available devices)
    #[arg(long)]
    input_device: Option<String>,

    /// Output device id or name (use 'devices list' to see available devices)
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

/// How a room session ends and what it leaves behind.
#[derive(clap::Args, Clone, Debug)]
struct RunArgs {
    /// Stop after this many seconds connected (default: run until Ctrl+C)
    #[arg(long, value_name = "SECS")]
    duration: Option<u64>,

    /// Write what the session measured to this file as JSON when it ends
    #[arg(long, value_name = "PATH")]
    report_json: Option<PathBuf>,

    /// How long the peer keeps audio before sending it back, as a peer that
    /// replays what it hears does. Taken off the round trips `--input-bursts` measures
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

    /// Create a room on a signaling server and print its invite code
    CreateRoom {
        /// jamuru server URL (e.g., https://example.com). The CLI asks it
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

        #[command(flatten)]
        run: RunArgs,

        /// Skip audio (chat only mode)
        #[arg(long)]
        chat_only: bool,
    },

    /// Join a room via signaling server
    JoinRoom {
        /// jamuru server URL (e.g., https://example.com). The CLI asks it
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

        #[command(flatten)]
        run: RunArgs,

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
        /// Input device id or name, or 'default' for the system default
        #[arg(long)]
        input: Option<String>,

        /// Output device id or name, or 'default' for the system default
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
    match input_devices() {
        Ok(devices) => {
            for device in devices {
                let default_marker = if device.is_default { " (default)" } else { "" };
                println!("  - {}  {}{}", device.id.0, device.name, default_marker);
            }
        }
        Err(e) => {
            println!("  Error: {}", e);
        }
    }

    println!("\nOutput devices:");
    match output_devices() {
        Ok(devices) => {
            for device in devices {
                let default_marker = if device.is_default { " (default)" } else { "" };
                println!("  - {}  {}{}", device.id.0, device.name, default_marker);
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
    /// Id of the device to capture from, as the app saves it (`None`: the
    /// system default)
    input_device: Option<String>,
    /// Id of the device to play to (`None`: the system default)
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
    /// How many channels are sent (1 mono, 2 stereo), as the app saves it
    transmit_channels: u32,
    /// Which channels of a multi-channel interface are read and played on (1-based), as
    /// the app saves them
    input_channels: (u32, Option<u32>),
    output_channels: (u32, Option<u32>),
}

impl AudioArgs {
    /// Resolves the settings: an explicit flag wins, then the named preset,
    /// then what the app saved, then the built-in default.
    fn resolve(&self) -> Result<AudioSettings> {
        let config = jamjam::config::load_config().unwrap_or_default();

        let preset = match &self.preset {
            Some(name) => AudioPreset::from_name(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown preset {:?}. `jamuru preset list` shows the available ones",
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

        let input_device = match &self.input_device {
            Some(wanted) => Some(device_id_of(
                wanted,
                &input_devices().map_err(anyhow::Error::msg)?,
                "input",
            )?),
            None => config.input_device_id,
        };
        let output_device = match &self.output_device {
            Some(wanted) => Some(device_id_of(
                wanted,
                &output_devices().map_err(anyhow::Error::msg)?,
                "output",
            )?),
            None => config.output_device_id,
        };

        Ok(AudioSettings {
            sample_rate: self.sample_rate.unwrap_or(config.sample_rate),
            frame_size,
            input_device,
            output_device,
            preset,
            input_tone: self.input_tone,
            input_bursts: self.input_bursts,
            output_file: self.output_file.clone(),
            monitor: self.monitor,
            transmit_channels: config.transmit_channels,
            input_channels: (config.input_channel_l, config.input_channel_r),
            output_channels: (config.output_channel_l, config.output_channel_r),
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
        let available = input_devices().map_err(anyhow::Error::msg)?;
        config.input_device_id = resolve_device_choice(&name, &available, "input")?;
    }
    if let Some(name) = output {
        let available = output_devices().map_err(anyhow::Error::msg)?;
        config.output_device_id = resolve_device_choice(&name, &available, "output")?;
    }

    jamjam::config::save_config(&config).map_err(anyhow::Error::msg)?;

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

/// What `devices set` saves for `wanted`: nothing for the default keyword,
/// otherwise the id of the device it names.
fn resolve_device_choice(
    wanted: &str,
    available: &[AudioDevice],
    kind: &str,
) -> Result<Option<String>> {
    if wanted == DEFAULT_DEVICE_KEYWORD {
        return Ok(None);
    }
    device_id_of(wanted, available, kind).map(Some)
}

/// The id of the device `wanted` (an id or a name) names. The id is what the
/// app saves and what a session opens, so a name is turned into it here rather
/// than saved: a name saved as is would match no device at session start.
/// Refuses a device the machine does not offer, and a name several devices share.
fn device_id_of(wanted: &str, available: &[AudioDevice], kind: &str) -> Result<String> {
    match choose_device(available, wanted) {
        Ok(device) => Ok(device.id.0.clone()),
        Err(ChooseDeviceError::NotFound(_)) => anyhow::bail!(
            "no {} device has the id or the name {:?}. `jamuru devices list` shows what this machine has",
            kind,
            wanted
        ),
        Err(refused @ ChooseDeviceError::Ambiguous { .. }) => {
            anyhow::bail!("{} device: {}", kind, refused)
        }
    }
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
            "unknown preset {:?}. `jamuru preset list` shows the available ones",
            name
        )
    })?;

    let mut config = jamjam::config::load_config().unwrap_or_default();
    config.preset = preset.clone();
    // Streaming reads buffer_size, so leaving it behind would run the session
    // at a frame size this preset never chose.
    config.buffer_size = preset.frame_size();
    jamjam::config::save_config(&config).map_err(anyhow::Error::msg)?;

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

/// Rounds to six decimals, so a report reads `0.495` and not `0.4950000047683716`
/// (an `f32` widened), and a loss rate of 0.04 % is not rounded away.
fn rounded(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

fn network_json(stats: &ConnectionStats) -> serde_json::Value {
    serde_json::json!({
        "rtt_ms": stats.rtt_ms.map(|rtt| rounded(f64::from(rtt))),
        "jitter_ms": rounded(f64::from(stats.jitter_ms)),
        "packet_loss_rate": rounded(f64::from(stats.packet_loss_rate)),
        "packets_sent": stats.packets_sent,
        "packets_received": stats.packets_received,
    })
}

fn playout_json(playout: &PlayoutStats, delay_frames: u32) -> serde_json::Value {
    serde_json::json!({
        "delay_frames": delay_frames,
        "frames_played": playout.frames_played,
        "frames_concealed": playout.frames_concealed,
        "frames_late": playout.frames_late,
        "resyncs": playout.resyncs,
    })
}

/// What a room session measured, as one JSON object.
///
/// `peer`, `network` and `playout` are those of the first peer that was linked, which is the
/// only one in a room of two; `peers` has every link, in the order they were opened.
/// `round_trip` is present only when the session sent `--input-bursts`.
fn session_report(
    settings: &AudioSettings,
    links: &[LinkSummary],
    connected_for: Duration,
    round_trip: Option<(u64, &RoundTripReport)>,
) -> serde_json::Value {
    let first = links
        .iter()
        .find(|link| link.addr.is_some())
        .or(links.first());
    let stats = first.map(|link| link.stats.clone()).unwrap_or_default();
    let playout = first.map(|link| link.playout).unwrap_or_default();
    let delay_frames = first.map_or(0, |link| link.delay_frames);
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
        "peer": first.and_then(|link| link.addr).map(|peer| peer.to_string()),
        "connected_seconds": rounded(connected_for.as_secs_f64()),
        "audio": {
            "preset": settings.preset.name(),
            "codec": format!("{:?}", settings.preset.codec_type()).to_lowercase(),
            "sample_rate": settings.sample_rate,
            "frame_size": settings.frame_size,
        },
        "network": network_json(&stats),
        "playout": playout_json(&playout, delay_frames),
        "peers": links
            .iter()
            .map(|link| {
                serde_json::json!({
                    "name": link.name,
                    "addr": link.addr.map(|addr| addr.to_string()),
                    "network": network_json(&link.stats),
                    "playout": playout_json(&link.playout, link.delay_frames),
                })
            })
            .collect::<Vec<_>>(),
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

/// What a line at the prompt can change of a running audio session
struct Controls {
    shared: Shared,
    commands: std_mpsc::Sender<Command>,
}

impl Controls {
    fn mute(&self, on: bool) {
        self.shared.is_muted.store(on, Ordering::SeqCst);
        let _ = self.commands.send(Command::SetMute(on));
    }

    fn monitor(&self, on: bool) {
        self.shared.is_monitoring.store(on, Ordering::SeqCst);
        let _ = self.commands.send(Command::SetMonitoring(on));
    }
}

/// Interprets a line as a command or as chat.
///
/// Slash commands exist because a debugging session needs the controls the
/// mixer offers - muting above all - without a second window (ADR-027).
fn handle_input_line(line: &str, audio: Option<&Controls>) -> Input {
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
            let Some(audio) = audio else {
                println!("(no audio in this session, nothing to mute)");
                return Input::Handled;
            };
            let mute = line == "/mute";
            audio.mute(mute);
            println!("{}", if mute { "🔇 muted" } else { "🔊 unmuted" });
            Input::Handled
        }
        "/monitor" | "/unmonitor" => {
            let Some(audio) = audio else {
                println!("(no audio in this session, nothing to monitor)");
                return Input::Handled;
            };
            let on = line == "/monitor";
            audio.monitor(on);
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

/// Runs a room session: create or join, then chat and (unless `chat_only`)
/// audio with everyone who publishes an address.
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
    settings: AudioSettings,
    run: RunArgs,
    message: Option<String>,
    timeout_secs: u64,
    chat_only: bool,
) -> Result<()> {
    if chat_only && run.report_json.is_some() {
        anyhow::bail!("--report-json reports the audio, which --chat-only does not open");
    }

    info!("Connecting through the jamuru server: {}", server);
    let client = signaling_client(&server);
    let mut conn = client.connect().await?;

    // The key the others in the room check our key exchange against
    let link_identity = LinkIdentity::generate();

    let entered = conn
        .enter_room(EnterRequest {
            entry,
            peer_name: peer_name.clone(),
            link_key: link_identity.public_key(),
            // The CLI takes no peer messages, so it announces no features.
            features: vec![],
            client_info: None,
        })
        .await
        .map_err(|e| match e {
            EnterError::Refused(message) => {
                anyhow::anyhow!("Signaling server refused the request: {}", message)
            }
            EnterError::Unexpected(other) => {
                anyhow::anyhow!("Unexpected response from server: {:?}", other)
            }
            EnterError::Connection(e) => anyhow::Error::new(e),
        })?;
    let (my_peer_id, peers) = (entered.peer_id, entered.peers);
    if entered.created {
        println!("\nRoom created: {}", entered.room_id);
    } else {
        println!("\nJoined room: {}", entered.room_id);
    }
    if let Some(invite_code) = &entered.invite_code {
        println!("Invite code:  {}", invite_code);
        if entered.created {
            println!(
                "Others join with:\n  jamuru join-room --server {} --room {}",
                server, invite_code
            );
        }
    }
    println!("Your peer ID: {}", my_peer_id);
    if !entered.created {
        println!("\nPeers in room ({}):", peers.len());
        for peer in &peers {
            println!(
                "  - {} (id: {}, addr: {:?})",
                peer.name, peer.id, peer.public_addr
            );
        }
    }

    let my_peer_id_str = my_peer_id.to_string();
    let signaling_conn_arc = Arc::new(tokio::sync::Mutex::new(conn));

    // Audio setup happens before any peer is known: the socket has to be bound
    // and published for the *other* side to have somewhere to send to.
    let probe = settings.input_bursts.then(|| Arc::new(BurstProbe::new()));
    let mut audio: Option<AudioSession> = if chat_only {
        println!("\n📝 Chat-only mode (no audio)");
        None
    } else {
        print_audio_settings(&settings);
        let session = AudioSession::start(&settings, link_identity, probe.clone()).await?;
        session.advertise(&signaling_conn_arc).await?;
        Some(session)
    };

    // Signaling events arrive on their own task so the main loop can select
    // over audio, chat and the keyboard at once.
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

    // Everyone in the room who has an address gets a link, whether they were
    // already there or turn up later.
    let mut roster = Roster::default();
    if let Some(session) = audio.as_mut() {
        let steps = roster.entered(my_peer_id, peers);
        run_audio(steps, &mut roster, session);
    }

    let mut connected_at: Option<Instant> = None;
    let mut follow = tokio::time::interval(ADAPT_INTERVAL);
    follow.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut announce = tokio::time::interval(Duration::from_millis(100));
    announce.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut stdin_open = true;
    loop {
        let session_ends = connected_at
            .zip(run.duration)
            .map(|(at, seconds)| at + Duration::from_secs(seconds));
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
                break;
            }
            _ = tokio::time::sleep_until(session_ends.unwrap_or_else(Instant::now).into()), if session_ends.is_some() => {
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

                if let Some(session) = audio.as_mut() {
                    let steps = match msg {
                        SignalingMessage::PeerJoined { peer } => {
                            roster.joined(peer);
                            Vec::new()
                        }
                        // A peer that just published an address is one we can stream to.
                        SignalingMessage::PeerUpdated { peer } => roster.updated(peer),
                        SignalingMessage::PeerLeft { peer_id } => roster.left(peer_id),
                        _ => Vec::new(),
                    };
                    run_audio(steps, &mut roster, session);
                }
            }
            _ = announce.tick(), if audio.is_some() => {
                if let Some(session) = audio.as_mut() {
                    for name in session.newly_connected() {
                        connected_at.get_or_insert_with(Instant::now);
                        println!("\nConnected to {}. Audio is flowing.\n", name);
                        print!("chat> ");
                        let _ = std::io::stdout().flush();
                    }
                }
            }
            _ = follow.tick(), if audio.is_some() => {
                if let Some(session) = audio.as_mut() {
                    let steps: Vec<Audio> = session
                        .link_healths()
                        .into_iter()
                        .flat_map(|(peer, health)| roster.link_is(peer, health))
                        .collect();
                    run_audio(steps, &mut roster, session);
                }
            }
            line_result = stdin_reader.next_line(), if stdin_open => {
                match line_result {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if !line.is_empty() {
                            match handle_input_line(line, audio.as_ref().map(|session| &session.controls)) {
                                Input::Quit => break,
                                Input::Handled => {}
                                Input::Stats => match audio.as_ref() {
                                    Some(session) => session.print_stats(),
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
                        stdin_open = false;
                    }
                    Err(e) => {
                        warn!("stdin error: {}", e);
                        stdin_open = false;
                    }
                }
            }
        }
    }

    signaling_recv_task.abort();

    if let Some(session) = audio {
        let reports = session.finish().await;

        if let Some(path) = &run.report_json {
            let round_trip =
                probe.map(|probe| probe.report(Duration::from_millis(run.echo_delay_ms)));
            let report = session_report(
                &settings,
                &reports,
                connected_at.map_or(Duration::ZERO, |at| at.elapsed()),
                round_trip
                    .as_ref()
                    .map(|report| (run.echo_delay_ms, report)),
            );
            std::fs::write(path, format!("{}\n", report))
                .with_context(|| format!("cannot write {}", path.display()))?;
        }
    }

    leave_room(&signaling_conn_arc).await;
    Ok(())
}

/// Carries out what the roster decided about the audio, the same decisions the app carries
/// out (ADR-077).
fn run_audio(steps: Vec<Audio>, roster: &mut Roster, session: &mut AudioSession) {
    for step in steps {
        match step {
            Audio::Stop => session.unlink_all(),
            // The socket outlives the links here, so what the room was told stays true
            Audio::Advertise => {}
            Audio::Rejoin => warn!(
                "A link gave up and nobody is heard. The app would enter the room again; \
                 the CLI does not - leave and start it again"
            ),
            Audio::End { peer } => session.unlink(peer),
            Audio::Start {
                peer,
                candidates,
                link_key,
            } => {
                let name = roster
                    .peers()
                    .iter()
                    .find(|known| known.id == peer)
                    .map_or_else(|| peer.to_string(), |known| known.name.clone());
                println!(
                    "\nConnecting to {} ({} address candidate(s))...",
                    name,
                    candidates.len()
                );
                if let Err(e) = session.link(peer, &name, candidates, link_key) {
                    // Allow a later update to retry rather than leaving them unheard for good
                    warn!("Failed to start the link to {}: {}", name, e);
                    roster.start_failed(peer);
                }
            }
        }
    }
}

/// What one link looked like when it was asked about or ended
#[derive(Clone)]
struct LinkSummary {
    name: String,
    addr: Option<SocketAddr>,
    stats: ConnectionStats,
    peer_info: Option<PeerLatencyInfo>,
    playout: PlayoutStats,
    /// The play-out delay in force, after any adaptation
    delay_frames: u32,
    /// The delay their play-out buffer holds, in microseconds
    jitter_buffer_delay_us: u32,
}

impl LinkSummary {
    fn of(name: String, report: &audio_session::LinkReport) -> Self {
        Self {
            name,
            addr: report.remote_addr.read().ok().and_then(|addr| *addr),
            stats: report
                .stats
                .read()
                .ok()
                .and_then(|stats| stats.clone())
                .unwrap_or_default(),
            peer_info: report
                .latency_info
                .read()
                .ok()
                .and_then(|info| info.clone()),
            playout: report
                .playout
                .read()
                .map(|playout| *playout)
                .unwrap_or_default(),
            delay_frames: report.delay_frames.load(Ordering::SeqCst),
            jitter_buffer_delay_us: report.jitter_buffer_delay_us.load(Ordering::SeqCst),
        }
    }
}

/// The audio half of a session: the audio session of the core library, which the app runs
/// too, steered from the prompt (ADR-077).
///
/// Received audio takes the app's path - decoded on arrival, held in the play-out buffer,
/// concealed when a frame is missing, FEC-recovered where the preset sends FEC - so a problem
/// with jitter or loss sounds here the way it sounds in the app (ADR-027, ADR-028). And, as in
/// the app, there is a link to everyone in the room who has an address, what is played is the
/// mix of them and what is captured goes to each of them.
struct AudioSession {
    controls: Controls,
    /// The audio thread, which ends after [`Command::Stop`]
    thread: std::thread::JoinHandle<()>,
    settings: AudioSettings,
    /// Our key in the room, which each peer's key exchange is checked against
    identity: LinkIdentity,
    /// What the room is told of the audio socket
    local_addr: SocketAddr,
    candidates: Vec<AddressCandidate>,
    /// The participants linked to, by the name the room knows them by
    names: HashMap<Uuid, String>,
    /// The links whose coming up has been told
    told_up: HashSet<Uuid>,
}

impl AudioSession {
    /// Binds the audio socket and starts the session on the devices, with no link yet.
    /// `identity` is our key in the room, which each peer's key exchange is checked against.
    ///
    /// With a `probe`, the bursts sent and the bursts played out are stamped on it.
    async fn start(
        settings: &AudioSettings,
        identity: LinkIdentity,
        probe: Option<Arc<BurstProbe>>,
    ) -> Result<Self> {
        // STUN is asked through the socket before anything reads it: the reply would be taken
        // for a datagram of a peer. One socket for the whole session, so every link shares
        // the address the room is told.
        let socket = bind_std("0.0.0.0:0")?;
        let local_addr = socket.local_addr()?;
        info!("Local UDP socket: {}", local_addr);
        let candidates = gather_candidates_on(&socket).await.unwrap_or_default();
        info!("Gathered {} address candidates", candidates.len());

        let shared = Shared::new();
        shared
            .is_monitoring
            .store(settings.monitor, Ordering::SeqCst);
        let input_signal = match (settings.input_tone, settings.input_bursts) {
            (Some(frequency), _) => Some(InputSignal::Tone(frequency)),
            (None, true) => Some(InputSignal::Bursts),
            (None, false) => None,
        };
        let Running {
            commands,
            started,
            thread,
        } = audio_session::start(
            Params {
                prepared_socket: socket,
                input_device_id: settings.input_device.clone(),
                output_device_id: settings.output_device.clone(),
                buffer_size: settings.frame_size,
                preset: settings.preset.clone(),
                sample_rate: settings.sample_rate,
                transmit_channels: settings.transmit_channels,
                input_channels: settings.input_channels,
                output_channels: settings.output_channels,
                input_signal,
                output_file: settings.output_file.clone(),
                probe,
            },
            &shared,
            |_| {},
        );
        match tokio::task::spawn_blocking(move || started.recv()).await? {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => anyhow::bail!(reason),
            Err(_) => anyhow::bail!("the audio session ended before it started"),
        }

        Ok(Self {
            controls: Controls { shared, commands },
            thread,
            settings: settings.clone(),
            identity,
            local_addr,
            candidates,
            names: HashMap::new(),
            told_up: HashSet::new(),
        })
    }

    /// Tells the room where to send audio. Done before any peer is known, because the peer needs
    /// an address to send to (ADR-026).
    async fn advertise(
        &self,
        signaling: &Arc<tokio::sync::Mutex<SignalingConnection>>,
    ) -> Result<()> {
        signaling
            .lock()
            .await
            .send(SignalingMessage::UpdatePeerInfo {
                candidates: self.candidates.clone(),
                public_addr: self.candidates.first().map(|c| c.address),
                local_addr: Some(self.local_addr),
            })
            .await?;
        Ok(())
    }

    /// Opens an audio link to `peer`, racing `candidates`. It is set up on the audio thread;
    /// [`AudioSession::link_healths`] says how it went.
    fn link(
        &mut self,
        peer: Uuid,
        name: &str,
        candidates: Vec<SocketAddr>,
        link_key: Option<String>,
    ) -> Result<()> {
        self.names.insert(peer, name.to_string());
        self.told_up.remove(&peer);
        self.controls
            .commands
            .send(Command::Link(Box::new(LinkTarget {
                peer,
                candidates,
                link: Some((self.identity.clone(), link_key)),
            })))
            .map_err(|_| anyhow::anyhow!("the audio session has ended"))
    }

    fn unlink(&mut self, peer: Uuid) {
        self.told_up.remove(&peer);
        let _ = self.controls.commands.send(Command::Unlink(peer));
    }

    fn unlink_all(&mut self) {
        for (peer, _) in self.controls.shared.link_reports() {
            self.unlink(peer);
        }
    }

    /// How the audio link to each participant is doing
    fn link_healths(&self) -> Vec<(Uuid, LinkHealth)> {
        self.controls.shared.link_healths()
    }

    /// The names of the participants whose link has come up since this was last asked
    fn newly_connected(&mut self) -> Vec<String> {
        let mut names = Vec::new();
        for (peer, report) in self.controls.shared.link_reports() {
            let connected = matches!(report.state(), Some((state, _)) if state == "connected");
            if connected && self.told_up.insert(peer) {
                names.push(named(&self.names, peer));
            }
        }
        names
    }

    /// What each link looks like, one block of statistics per peer
    fn print_stats(&self) {
        let reports = summaries(&self.names, self.controls.shared.link_reports());
        if reports.is_empty() {
            println!("(no peer connected yet, nothing to measure)");
        }
        for report in &reports {
            print_link_stats(report, &self.settings);
        }
    }

    /// Stops the audio, closes every link and prints what each looked like.
    async fn finish(self) -> Vec<LinkSummary> {
        let Self {
            controls,
            thread,
            settings,
            names,
            ..
        } = self;
        let _ = controls.commands.send(Command::Stop);
        let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        let reports = summaries(&names, controls.shared.ended_reports());
        for report in &reports {
            print_link_stats(report, &settings);
        }
        reports
    }
}

fn named(names: &HashMap<Uuid, String>, peer: Uuid) -> String {
    names
        .get(&peer)
        .cloned()
        .unwrap_or_else(|| peer.to_string())
}

fn summaries(
    names: &HashMap<Uuid, String>,
    reports: Vec<(Uuid, Arc<audio_session::LinkReport>)>,
) -> Vec<LinkSummary> {
    reports
        .into_iter()
        .map(|(peer, report)| LinkSummary::of(named(names, peer), &report))
        .collect()
}

fn print_link_stats(report: &LinkSummary, settings: &AudioSettings) {
    let mut local_info =
        LocalLatencyInfo::from_audio_config(settings.frame_size, settings.sample_rate, "pcm");
    local_info.set_jitter_buffer_ms(report.jitter_buffer_delay_us as f32 / 1000.0);
    print_session_stats(
        &report.stats,
        &local_info,
        report.peer_info.as_ref(),
        &report.playout,
        Some(&report.name),
    );
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
        Commands::CreateRoom {
            server,
            room_name,
            name,
            audio,
            run,
            chat_only,
        } => {
            run_room_session(
                server,
                RoomEntry::Create { room_name },
                name,
                audio.resolve()?,
                run,
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
            run,
            chat_only,
        } => {
            run_room_session(
                server,
                RoomEntry::Join { room_id: room },
                name,
                audio.resolve()?,
                run,
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
    use jamjam::audio::DeviceId;

    /// A session that is not running: the commands it is sent arrive on the returned receiver.
    fn controls() -> (Controls, std_mpsc::Receiver<Command>) {
        let (commands, sent) = std_mpsc::channel();
        (
            Controls {
                shared: Shared::new(),
                commands,
            },
            sent,
        )
    }

    /// The session commands are what make the CLI usable for debugging without
    /// a second window (ADR-027): they have to be recognised, act, and not be
    /// mistaken for chat.
    ///
    /// Verifies: REQ-CLI-002
    #[test]
    fn session_commands_are_recognised_and_act() {
        let (audio, sent) = controls();

        assert!(matches!(
            handle_input_line("/mute", Some(&audio)),
            Input::Handled
        ));
        assert!(
            audio.shared.is_muted.load(Ordering::SeqCst),
            "/mute should mute"
        );
        assert_eq!(sent.try_recv(), Ok(Command::SetMute(true)));

        assert!(matches!(
            handle_input_line("/unmute", Some(&audio)),
            Input::Handled
        ));
        assert!(
            !audio.shared.is_muted.load(Ordering::SeqCst),
            "/unmute should unmute"
        );
        assert_eq!(sent.try_recv(), Ok(Command::SetMute(false)));

        assert!(matches!(
            handle_input_line("/stats", Some(&audio)),
            Input::Stats
        ));
        assert!(matches!(
            handle_input_line("/help", Some(&audio)),
            Input::Handled
        ));
        assert!(matches!(
            handle_input_line("/quit", Some(&audio)),
            Input::Quit
        ));
    }

    /// Anything that is not a command is chat, and an unrecognised command is
    /// not sent to the room by accident.
    ///
    /// Verifies: REQ-CLI-002
    #[test]
    fn only_commands_are_intercepted() {
        let (audio, sent) = controls();

        match handle_input_line("hello everyone", Some(&audio)) {
            Input::Chat(text) => assert_eq!(text, "hello everyone"),
            _ => panic!("plain text should be chat"),
        }

        assert!(
            matches!(handle_input_line("/mutee", Some(&audio)), Input::Handled),
            "a mistyped command must not be broadcast as chat"
        );
        assert!(
            !audio.shared.is_muted.load(Ordering::SeqCst) && sent.try_recv().is_err(),
            "a mistyped command must not change anything"
        );
    }

    /// Muting is meaningless in a chat-only session; saying so beats silently
    /// setting a flag nothing reads.
    ///
    /// Verifies: REQ-CLI-002
    #[test]
    fn muting_without_audio_reports_that_there_is_nothing_to_mute() {
        assert!(matches!(handle_input_line("/mute", None), Input::Handled));
    }

    /// `/monitor` is the CLI's switch for hearing yourself; it has to reach the
    /// session the audio callbacks read, not just print a line.
    ///
    /// Verifies: REQ-CLI-006
    #[test]
    fn monitor_commands_switch_local_monitoring() {
        let (audio, sent) = controls();

        assert!(matches!(
            handle_input_line("/monitor", Some(&audio)),
            Input::Handled
        ));
        assert!(
            audio.shared.is_monitoring.load(Ordering::SeqCst),
            "/monitor should turn monitoring on"
        );
        assert_eq!(sent.try_recv(), Ok(Command::SetMonitoring(true)));

        assert!(matches!(
            handle_input_line("/unmonitor", Some(&audio)),
            Input::Handled
        ));
        assert!(
            !audio.shared.is_monitoring.load(Ordering::SeqCst),
            "/unmonitor should turn monitoring off"
        );
        assert_eq!(sent.try_recv(), Ok(Command::SetMonitoring(false)));
        assert!(
            !audio.shared.is_muted.load(Ordering::SeqCst),
            "monitoring is what you hear; it does not touch what is sent"
        );
    }

    fn settings() -> AudioSettings {
        AudioSettings {
            sample_rate: 48_000,
            frame_size: 128,
            input_device: None,
            output_device: None,
            preset: AudioPreset::Balanced,
            input_tone: None,
            input_bursts: false,
            output_file: None,
            monitor: false,
            transmit_channels: 1,
            input_channels: (1, Some(2)),
            output_channels: (1, Some(2)),
        }
    }

    fn link(
        name: &str,
        addr: Option<&str>,
        packets_received: u64,
        frames_played: u64,
    ) -> LinkSummary {
        LinkSummary {
            name: name.to_string(),
            addr: addr.map(|addr| addr.parse().unwrap()),
            stats: ConnectionStats {
                packets_received,
                ..Default::default()
            },
            peer_info: None,
            playout: PlayoutStats {
                frames_played,
                ..Default::default()
            },
            delay_frames: 3,
            jitter_buffer_delay_us: 0,
        }
    }

    /// A session with a link to each of the others measured each of them. What a report of a
    /// room of two read - `peer`, `network`, `playout` - is the first one that came up, so the
    /// tools that read it go on reading the same thing; `peers` has all of them.
    ///
    /// Verifies: REQ-CLI-011
    #[test]
    fn a_session_linked_to_two_peers_reports_each_and_the_first_that_came_up_at_the_top() {
        let links = [
            link("never-up", None, 0, 0),
            link("ben", Some("192.0.2.2:5000"), 100, 90),
            link("cho", Some("192.0.2.3:6000"), 200, 190),
        ];

        let report = session_report(&settings(), &links, Duration::from_secs(5), None);

        assert_eq!(report["peer"], "192.0.2.2:5000");
        assert_eq!(report["network"]["packets_received"], 100);
        assert_eq!(report["playout"]["frames_played"], 90);
        assert_eq!(report["playout"]["delay_frames"], 3);
        let peers = report["peers"].as_array().expect("a list of peers");
        let names: Vec<&str> = peers
            .iter()
            .map(|peer| peer["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["never-up", "ben", "cho"]);
        assert_eq!(peers[2]["network"]["packets_received"], 200);
        assert_eq!(peers[0]["addr"], serde_json::Value::Null);
    }

    /// Verifies: REQ-CLI-007
    #[test]
    fn a_session_nobody_linked_to_reports_no_peer_and_empty_counters() {
        let report = session_report(&settings(), &[], Duration::ZERO, None);

        assert_eq!(report["peer"], serde_json::Value::Null);
        assert_eq!(report["network"]["packets_received"], 0);
        assert_eq!(report["playout"]["frames_played"], 0);
        assert_eq!(report["peers"], serde_json::json!([]));
    }

    /// Verifies: REQ-CLI-006
    #[test]
    fn monitoring_without_audio_reports_that_there_is_nothing_to_monitor() {
        assert!(matches!(
            handle_input_line("/monitor", None),
            Input::Handled
        ));
    }

    fn device(id: &str, name: &str) -> AudioDevice {
        AudioDevice {
            id: DeviceId(id.to_string()),
            name: name.to_string(),
            supported_sample_rates: vec![48000],
            supported_channels: vec![2],
            is_default: false,
            is_asio: false,
            is_bluetooth: false,
        }
    }

    /// The app reads a saved device as an id and opens the device whose id it
    /// is. A name saved as it was typed matched no device when a session started.
    ///
    /// Verifies: REQ-CLI-012
    #[test]
    fn a_device_chosen_by_name_is_saved_as_its_id() {
        let available = vec![
            device("alsa:hw:0,0", "Built-in Audio"),
            device("alsa:hw:1,0", "USB Interface"),
        ];

        assert_eq!(
            resolve_device_choice("USB Interface", &available, "input").unwrap(),
            Some("alsa:hw:1,0".to_string())
        );
        assert_eq!(
            resolve_device_choice("alsa:hw:1,0", &available, "input").unwrap(),
            Some("alsa:hw:1,0".to_string())
        );
    }

    /// Verifies: REQ-CLI-012
    #[test]
    fn the_default_keyword_saves_no_choice() {
        let available = vec![device("alsa:hw:0,0", "Built-in Audio")];

        assert_eq!(
            resolve_device_choice("default", &available, "output").unwrap(),
            None
        );
    }

    /// Verifies: REQ-CLI-004, REQ-CLI-012
    #[test]
    fn a_device_the_machine_does_not_have_is_refused_naming_how_to_list_the_real_ones() {
        let available = vec![device("alsa:hw:0,0", "Built-in Audio")];

        let message = resolve_device_choice("Missing", &available, "input")
            .unwrap_err()
            .to_string();

        assert!(
            message.contains("Missing") && message.contains("devices list"),
            "got {message:?}"
        );
    }

    /// Verifies: REQ-CLI-012
    #[test]
    fn a_name_that_two_devices_share_is_refused_naming_their_ids() {
        let available = vec![device("alsa:hw:2,0", "Twin"), device("alsa:hw:3,0", "Twin")];

        let message = resolve_device_choice("Twin", &available, "output")
            .unwrap_err()
            .to_string();

        assert!(
            message.contains("alsa:hw:2,0") && message.contains("alsa:hw:3,0"),
            "got {message:?}"
        );
    }
}
