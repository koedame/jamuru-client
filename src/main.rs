//! jamuru - Low-latency P2P audio communication for musicians

mod links;

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use jamjam::link::Senders;
use links::{Checked, LinkReport, LinkSettings, Links};

use jamjam::audio::{
    bounded, choose_device, list_input_devices, list_output_devices, mono_to_wire, AudioConfig,
    AudioDevice, AudioEngine, AudioError, AudioPreset, BurstProbe, BurstSignal, ChooseDeviceError,
    DeviceId, GainRamp, LocalMonitor, PeerMix, PlayoutStats, RoundTripReport, ADAPT_INTERVAL,
    LIST_TIMEOUT, WIRE_CHANNELS,
};
#[cfg(target_os = "windows")]
use jamjam::audio::{shared_asio_driver, AsioDuplex, OutputRoute, OPEN_TIMEOUT};
use jamjam::network::roster::{Audio, Roster};
use jamjam::network::{
    audio_packet_duration_ms, bind_std, gather_candidates_on, AudioEncodingConfig, ConnectionStats,
    EnterError, EnterRequest, LatencyBreakdown, LinkIdentity, LocalLatencyInfo, PeerLatencyInfo,
    RoomEntry, SignalingClient, SignalingConnection, SignalingMessage, UdpMux, MAX_PEERS_PER_ROOM,
};
use jamjam::protocol::LatencyInfoMessage;

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

/// Lists devices on a thread of its own and gives up at `timeout`, so a driver
/// that has hung does not hang the command with it (REQ-AUD-123).
fn listed_within(
    what: &str,
    timeout: Duration,
    list: fn() -> Result<Vec<AudioDevice>, AudioError>,
) -> Result<Vec<AudioDevice>, AudioError> {
    bounded(what, timeout, list)?
}

fn list_input_devices_bounded() -> Result<Vec<AudioDevice>, AudioError> {
    listed_within("input device listing", LIST_TIMEOUT, list_input_devices)
}

fn list_output_devices_bounded() -> Result<Vec<AudioDevice>, AudioError> {
    listed_within("output device listing", LIST_TIMEOUT, list_output_devices)
}

fn list_devices() {
    println!("Input devices:");
    match list_input_devices_bounded() {
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
    match list_output_devices_bounded() {
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
                &list_input_devices_bounded().map_err(|e| anyhow::anyhow!("{}", e))?,
                "input",
            )?),
            None => config.input_device_id,
        };
        let output_device = match &self.output_device {
            Some(wanted) => Some(device_id_of(
                wanted,
                &list_output_devices_bounded().map_err(|e| anyhow::anyhow!("{}", e))?,
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
        let available = list_input_devices_bounded().map_err(|e| anyhow::anyhow!("{}", e))?;
        config.input_device_id = resolve_device_choice(&name, &available, "input")?;
    }
    if let Some(name) = output {
        let available = list_output_devices_bounded().map_err(|e| anyhow::anyhow!("{}", e))?;
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
    links: &[LinkReport],
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
    let is_muted = Arc::new(AtomicBool::new(false));
    let probe = settings.input_bursts.then(|| Arc::new(BurstProbe::new()));
    let (mut audio, mut rx_checks, _no_checks): (Option<AudioSession>, _, _) = if chat_only {
        println!("\n📝 Chat-only mode (no audio)");
        let (idle, rx_checks) = tokio::sync::mpsc::unbounded_channel();
        (None, rx_checks, Some(idle))
    } else {
        print_audio_settings(&settings);
        let (session, rx_checks) =
            AudioSession::start(&settings, link_identity, is_muted.clone(), probe.clone()).await?;
        session.advertise(&signaling_conn_arc).await?;
        (Some(session), rx_checks, None)
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
        run_audio(steps, &mut roster, session).await;
    }

    let mut connected_at: Option<Instant> = None;
    let mut follow = tokio::time::interval(ADAPT_INTERVAL);
    follow.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
                    run_audio(steps, &mut roster, session).await;
                }
            }
            Some(checked) = rx_checks.recv() => {
                if let Some(session) = audio.as_mut() {
                    if let Some(name) = session.links.checked(checked).await {
                        connected_at.get_or_insert_with(Instant::now);
                        println!("\nConnected to {}. Audio is flowing.\n", name);
                        print!("chat> ");
                        let _ = std::io::stdout().flush();
                    }
                }
            }
            _ = follow.tick(), if audio.is_some() => {
                if let Some(session) = audio.as_mut() {
                    session.links.adapt();
                    let steps: Vec<Audio> = session
                        .links
                        .healths()
                        .into_iter()
                        .flat_map(|(peer, health)| roster.link_is(peer, health))
                        .collect();
                    run_audio(steps, &mut roster, session).await;
                }
            }
            line_result = stdin_reader.next_line(), if stdin_open => {
                match line_result {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if !line.is_empty() {
                            match handle_input_line(line, &is_muted, audio.as_ref().map(AudioSession::monitor)) {
                                Input::Quit => break,
                                Input::Handled => {}
                                Input::Stats => match audio.as_ref() {
                                    Some(session) => session.print_stats().await,
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
async fn run_audio(steps: Vec<Audio>, roster: &mut Roster, session: &mut AudioSession) {
    for step in steps {
        match step {
            Audio::Stop => session.links.end_all().await,
            // The socket outlives the links here, so what the room was told stays true
            Audio::Advertise => {}
            Audio::Rejoin => warn!(
                "A link gave up and nobody is heard. The app would enter the room again; \
                 the CLI does not - leave and start it again"
            ),
            Audio::End { peer } => session.links.end(peer).await,
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
                if let Err(e) = session.links.start(peer, &name, candidates, link_key) {
                    // Allow a later update to retry rather than leaving them unheard for good
                    warn!("Failed to start the link to {}: {}", name, e);
                    roster.start_failed(peer);
                }
            }
        }
    }
}

/// The audio half of a session.
///
/// Received audio takes the app's path - decoded on arrival, held in the
/// play-out buffer, concealed when a frame is missing, FEC-recovered where the
/// preset sends FEC - so a problem with jitter or loss sounds here the way it
/// sounds in the app (ADR-027, ADR-028). What is sent matches the app too:
/// stereo, centred, in the preset's codec. And, as in the app, there is a link
/// to everyone in the room who has an address, what is played is the mix of
/// them and what is captured goes to each of them (ADR-077).
struct AudioSession {
    links: Links,
    local_info: LocalLatencyInfo,
    /// What the room is told of the audio socket
    local_addr: SocketAddr,
    candidates: Vec<jamjam::network::AddressCandidate>,
    send_task: tokio::task::JoinHandle<()>,
    monitor: LocalMonitor,
    /// Held for as long as the session runs; dropping it stops the audio.
    io: AudioIo,
}

impl AudioSession {
    /// Binds the audio socket and starts capture and playback, with no link yet. `identity` is
    /// our key in the room, which each peer's key exchange is checked against.
    ///
    /// With a `probe`, the bursts sent and the bursts played out are stamped
    /// on it.
    async fn start(
        settings: &AudioSettings,
        identity: LinkIdentity,
        is_muted: Arc<AtomicBool>,
        probe: Option<Arc<BurstProbe>>,
    ) -> Result<(Self, tokio::sync::mpsc::UnboundedReceiver<Checked>)> {
        let preset = &settings.preset;
        let codec_type = preset.codec_type();

        // STUN is asked through the socket before anything reads it: the reply would be taken
        // for a datagram of a peer. One socket for the whole session, so every link shares
        // the address the room is told.
        let socket = bind_std("0.0.0.0:0")?;
        let local_addr = socket.local_addr()?;
        info!("Local UDP socket: {}", local_addr);
        let candidates = gather_candidates_on(&socket).await.unwrap_or_default();
        info!("Gathered {} address candidates", candidates.len());
        let mux = UdpMux::from_std(socket)?;

        let mix = PeerMix::new(
            MAX_PEERS_PER_ROOM - 1,
            settings.sample_rate,
            settings.frame_size,
            Arc::new(AtomicU64::new(0)),
        );
        let monitor = LocalMonitor::new(settings.frame_size);
        monitor.set_enabled(settings.monitor);

        let (tx_capture, mut rx_capture) = tokio::sync::mpsc::channel::<(Vec<f32>, u32)>(64);
        let io = AudioIo::start(
            settings,
            &mix,
            &monitor,
            probe.clone(),
            move |samples, timestamp| {
                let _ = tx_capture.try_send((samples.to_vec(), timestamp));
            },
        )?;

        let codec = format!("{:?}", codec_type).to_lowercase();
        let mut local_info =
            LocalLatencyInfo::from_audio_config(settings.frame_size, settings.sample_rate, &codec);
        local_info.set_jitter_buffer_ms(jitter_buffer_ms(preset, settings));

        let senders: Senders = Arc::new(RwLock::new(Arc::from(Vec::new())));
        let (links, checks) = Links::new(
            mux,
            mix,
            link_settings(settings, &local_info),
            identity,
            senders.clone(),
        );

        let sample_rate = settings.sample_rate;
        let send_task = tokio::spawn(async move {
            let mut stereo = Vec::new();
            // Muting fades the microphone out (and unmuting in) rather than
            // cutting it, so the other side does not hear a click.
            let mut mute_ramp = GainRamp::new(sample_rate);
            while let Some((samples, timestamp)) = rx_capture.recv().await {
                let mute_target = if is_muted.load(Ordering::SeqCst) {
                    0.0
                } else {
                    1.0
                };
                if mute_ramp.is_silent_at(mute_target) {
                    continue;
                }
                stereo.resize(samples.len() * WIRE_CHANNELS, 0.0);
                mono_to_wire(&samples, 1.0, 0, &mut stereo);
                mute_ramp.apply(&mut stereo, WIRE_CHANNELS, mute_target);
                // Each link seals the frame with its own key, so each is sent its own
                let up = senders.read().unwrap_or_else(|e| e.into_inner()).clone();
                let mut noted = false;
                for connection in up.iter() {
                    let conn = connection.lock().await;
                    if !conn.is_connected() {
                        continue;
                    }
                    if let (Some(probe), false) = (&probe, noted) {
                        probe.note_sent(&stereo);
                        noted = true;
                    }
                    if let Err(e) = conn.send_audio(&stereo, timestamp).await {
                        warn!("Failed to send audio: {}", e);
                    }
                }
            }
        });

        Ok((
            Self {
                links,
                local_info,
                local_addr,
                candidates,
                send_task,
                monitor,
                io,
            },
            checks,
        ))
    }

    fn monitor(&self) -> &LocalMonitor {
        &self.monitor
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

    /// What each link looks like, one block of statistics per peer
    async fn print_stats(&self) {
        let reports = self.links.reports().await;
        if reports.is_empty() {
            println!("(no peer connected yet, nothing to measure)");
        }
        for report in &reports {
            print_link_stats(report, &self.local_info);
        }
    }

    /// Stops the audio, closes every link and prints what each looked like.
    async fn finish(self) -> Vec<LinkReport> {
        let Self {
            links,
            local_info,
            send_task,
            io,
            ..
        } = self;
        send_task.abort();
        let reports = links.finish().await;
        drop(io);
        for report in &reports {
            print_link_stats(report, &local_info);
        }
        reports
    }
}

fn print_link_stats(report: &LinkReport, local_info: &LocalLatencyInfo) {
    print_session_stats(
        &report.stats,
        local_info,
        report.peer_info.as_ref(),
        &report.playout,
        Some(&report.name),
    );
}

/// How every link of the session carries audio
fn link_settings(settings: &AudioSettings, local_info: &LocalLatencyInfo) -> LinkSettings {
    let preset = &settings.preset;
    LinkSettings {
        encoding: AudioEncodingConfig {
            codec_type: preset.codec_type(),
            sample_rate: settings.sample_rate,
            channels: WIRE_CHANNELS as u16,
            frame_size: settings.frame_size,
            bitrate: 0,
            fec_group_size: preset.fec_group_size(),
        },
        sample_rate: settings.sample_rate,
        frame_size: settings.frame_size,
        jitter_buffer_frames: preset.jitter_buffer_frames(),
        announce: latency_info_message(local_info),
    }
}

/// What a link tells its peer of our audio, so it can convert ours and show the latency we add
/// (ADR-013).
fn latency_info_message(info: &LocalLatencyInfo) -> LatencyInfoMessage {
    LatencyInfoMessage {
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
    }
}

/// The play-out delay told to the peer, in ms. The buffer counts packets, and
/// a frame too big for one packet goes as several, so its depth lasts packets
/// long, not frames long (ADR-070).
fn jitter_buffer_ms(preset: &AudioPreset, settings: &AudioSettings) -> f32 {
    preset.jitter_buffer_frames() as f32
        * audio_packet_duration_ms(settings.frame_size, WIRE_CHANNELS, settings.sample_rate)
}

/// Peak level of `--input-tone`, below full scale so the centred stereo mix
/// cannot clip.
const TONE_AMPLITUDE: f32 = 0.5;

/// Fills a captured frame with a stand-in signal.
type SignalGenerator = Box<dyn FnMut(&mut [f32]) + Send>;

/// Hands off each captured frame and its timestamp. Boxed so `AudioIo::start`
/// can choose at runtime which of capture's backends ends up calling it.
type OnCapture = Box<dyn FnMut(&[f32], u32) + Send>;

/// Fills a device-sized request with what a peer sent. Boxed for the same
/// reason as [`OnCapture`].
type FrameSource = Box<dyn FnMut(&mut [f32]) -> usize + Send>;

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
    /// Set instead of running `capture`/`playback` when the input and output
    /// device name the same ASIO driver (Windows only): one ASIO session
    /// serves both directions, so there is nothing for `capture`/`playback`
    /// to open in that case (see `audio::asio`).
    #[cfg(target_os = "windows")]
    asio: Option<AsioDuplex>,
}

impl AudioIo {
    /// Starts capture, handing each mono frame and its sample timestamp to
    /// `on_capture`, and playback, taking frames from `mix`. What is
    /// captured also goes to `monitor`, which mixes it into what is played
    /// while it is on. What is played is also shown to `probe`.
    fn start<F>(
        settings: &AudioSettings,
        mix: &PeerMix,
        monitor: &LocalMonitor,
        probe: Option<Arc<BurstProbe>>,
        on_capture: F,
    ) -> Result<Self>
    where
        F: FnMut(&[f32], u32) + Send + 'static,
    {
        // The CLI has no input or output channel choice: the app's pickers
        // (`set_capture_picks` / `set_playback_route`) are left out on purpose.
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
            #[cfg(target_os = "windows")]
            asio: None,
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

        // Boxed, and taken by whichever of the three branches below ends up
        // using it, so the choice can be made with an `Option`s runtime
        // check rather than needing the compiler to see one static branch -
        // `asio_driver`'s branch only exists on Windows, and stays absent
        // (never `Some`) everywhere else.
        let mut on_capture: Option<OnCapture> = Some(Box::new(on_capture));
        let for_output = mix.clone();
        let for_monitor = monitor.clone();
        let mut source: Option<FrameSource> = Some(Box::new(move |out: &mut [f32]| {
            let samples = for_output.mix_into(out, 100, None);
            // Before the monitor mix: what the peer sent, not our own input.
            if let Some(probe) = &probe {
                probe.note_played(&out[..samples]);
            }
            for_monitor.mix_into(&mut out[..samples]);
            samples
        }));

        // The input and output device name the same ASIO driver: one ASIO
        // session serves both directions (`audio::asio`), so it takes over
        // from the two independent opens below. Not considered together
        // with a stand-in (`--input-tone` etc.) - those exist to run without
        // real hardware, which an ASIO driver is not.
        #[cfg(target_os = "windows")]
        if stand_in.is_none() && settings.output_file.is_none() {
            let asio_driver = shared_asio_driver(
                settings.input_device.clone().map(DeviceId).as_ref(),
                settings.output_device.clone().map(DeviceId).as_ref(),
            );
            if let Some(driver_name) = asio_driver {
                let mut tap = monitor.tap();
                let mut capture_cb = on_capture.take().expect("not yet taken");
                let fill_frame = source.take().expect("not yet taken");
                io.asio = Some(AsioDuplex::start(
                    &driver_name,
                    settings.sample_rate,
                    settings.frame_size,
                    mix.frame_samples(),
                    OPEN_TIMEOUT,
                    vec![0],
                    OutputRoute::default(),
                    move |samples, timestamp| {
                        tap.push(samples);
                        capture_cb(samples, timestamp as u32);
                    },
                    fill_frame,
                )?);
            }
        }

        if let Some(mut on_capture) = on_capture.take() {
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
        }

        if let Some(mut source) = source.take() {
            match &settings.output_file {
                Some(path) => {
                    let mut file = std::fs::File::create(path)
                        .with_context(|| format!("cannot write {}", path.display()))?;
                    let mut frame = vec![0.0f32; mix.frame_samples()];
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
                    mix.frame_samples(),
                    source,
                )?,
            }
        }

        Ok(io)
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        #[cfg(target_os = "windows")]
        if let Some(asio) = self.asio.take() {
            asio.stop();
        }
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
        }
    }

    fn link(
        name: &str,
        addr: Option<&str>,
        packets_received: u64,
        frames_played: u64,
    ) -> LinkReport {
        LinkReport {
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
        let muted = AtomicBool::new(false);

        assert!(matches!(
            handle_input_line("/monitor", &muted, None),
            Input::Handled
        ));
    }

    /// Verifies: REQ-CLI-008
    #[test]
    fn listing_devices_with_a_hung_driver_gives_up_at_the_timeout() {
        fn hung() -> Result<Vec<AudioDevice>, AudioError> {
            std::thread::sleep(Duration::from_secs(3));
            Ok(Vec::new())
        }

        let started = Instant::now();
        let outcome = listed_within("test listing", Duration::from_millis(100), hung);

        assert!(
            matches!(outcome, Err(AudioError::DeviceUnresponsive(_))),
            "a listing that does not return should end as unresponsive, got {outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the caller should not have waited for the driver"
        );
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

    /// A link that is still being checked is not one the captured audio goes to, so what is
    /// captured meanwhile is not held for it and cannot reach the peer as a burst of old audio
    /// when the check ends. It is sent to from the moment its check has ended.
    ///
    /// Verifies: REQ-CLI-009
    #[tokio::test]
    async fn audio_captured_while_a_link_is_checked_is_not_held_for_it_and_goes_to_it_once_it_is_up(
    ) {
        let settings = settings();
        let local_info = LocalLatencyInfo::from_audio_config(128, 48_000, "pcm");
        let (ours, theirs) = (LinkIdentity::generate(), LinkIdentity::generate());
        let (our_key, their_key) = (ours.public_key(), theirs.public_key());
        let (our_mux, their_mux) = (
            UdpMux::bind("127.0.0.1:0").await.expect("our socket"),
            UdpMux::bind("127.0.0.1:0").await.expect("their socket"),
        );
        let (our_addr, their_addr) = (our_mux.local_addr(), their_mux.local_addr());
        let senders: Senders = Arc::new(RwLock::new(Arc::from(Vec::new())));
        let (mut links, mut checks) = Links::new(
            our_mux,
            PeerMix::new(
                MAX_PEERS_PER_ROOM - 1,
                settings.sample_rate,
                settings.frame_size,
                Arc::new(AtomicU64::new(0)),
            ),
            link_settings(&settings, &local_info),
            ours,
            senders.clone(),
        );
        let sent_to = || senders.read().unwrap().len();

        links
            .start(
                uuid::Uuid::new_v4(),
                "ben",
                vec![their_addr],
                Some(their_key),
            )
            .expect("the link starts");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(sent_to(), 0, "nobody has answered, so the link is not up");

        let mut ben = jamjam::network::Connection::on(&their_mux).expect("their link");
        ben.verify_peer(&theirs, Some(&our_key)).expect("our key");
        ben.connect(our_addr).await.expect("they answer");
        let checked = tokio::time::timeout(Duration::from_secs(10), checks.recv())
            .await
            .expect("the check ends")
            .expect("the check reports");

        assert_eq!(links.checked(checked).await.as_deref(), Some("ben"));
        assert_eq!(sent_to(), 1, "the link is sent to once its check has ended");
    }

    /// Verifies: REQ-CLI-010
    #[test]
    fn jitter_buffer_told_to_the_peer_counts_packets_not_frames() {
        let settings = |frame_size| AudioSettings {
            frame_size,
            preset: AudioPreset::HighQuality,
            ..settings()
        };
        let depth = AudioPreset::HighQuality.jitter_buffer_frames() as f32;

        // 256 frames go as two packets of 128: a packet lasts 2.667 ms, not
        // the frame's 5.333 ms.
        let big = jitter_buffer_ms(&AudioPreset::HighQuality, &settings(256));
        assert!(
            (big - depth * 128.0 / 48.0).abs() < 0.01,
            "256 frames: got {big} ms"
        );

        let small = jitter_buffer_ms(&AudioPreset::HighQuality, &settings(64));
        assert!(
            (small - depth * 64.0 / 48.0).abs() < 0.01,
            "64 frames: got {small} ms"
        );
    }
}
