//! Audio streaming IPC commands for Tauri
//!
//! Manages P2P audio streaming with a dedicated audio thread to handle
//! the non-Send+Sync AudioEngine.
//!
//! Performance optimizations for 32-sample buffers:
//! - Uses rtrb (real-time safe ring buffer) instead of tokio channels
//! - Zero allocation in audio callbacks
//! - Pre-allocated stereo conversion buffer

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, RwLock};
use std::thread;

use rtrb::RingBuffer;
use serde::Serialize;
use tokio::sync::Mutex;

use jamjam::audio::{
    mono_to_wire, AudioConfig, AudioEngine, AudioPreset, DeviceId, LocalMonitor, PeerRateChange,
    PlayoutResult, ReceivePath, WIRE_CHANNELS,
};
use jamjam::network::{
    required_bps, status_label, AudioEncodingConfig, BandwidthEstimator, BandwidthStatus,
    BandwidthVerdict, Connection, ConnectionState, ConnectionStats, LatencyBreakdown,
    LocalLatencyInfo, PeerLatencyInfo, QualityMonitor,
};
use jamjam::protocol::LatencyInfoMessage;

/// Default audio sample rate for latency calculations
/// Per ADR-013: 48kHz is recommended, but user can select 44100/48000/96000
const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// How long the receive loop waits when it has nothing to hand to playback.
///
/// Short enough to be irrelevant to the latency budget (ADR-008) and long
/// enough to hand the runtime back to the task that receives packets, which
/// shares this thread. The playback ring buffer holds eight frames, so a wake
/// that the OS timer rounds up to a millisecond still refills it in time.
const POP_IDLE: tokio::time::Duration = tokio::time::Duration::from_micros(250);

/// How often connection stats and the meter levels are refreshed for the UI,
/// which polls at the same interval.
const STATS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// How often the play-out delay follows the link (ADR-031). One second is the
/// window each decision judges, and ten of them in a row are needed to give
/// a frame back.
const ADAPT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Real-time thread priority for Linux (1-99, higher = more priority)
/// 99 = maximum, but risks system freeze if thread hangs
/// 90 = very high, leaves headroom for critical kernel threads
#[cfg(target_os = "linux")]
const REALTIME_PRIORITY: i32 = 90;

/// Set real-time priority for the CURRENT thread (call from within audio thread)
/// This reduces buffer underruns by giving the audio thread scheduling priority
#[cfg(target_os = "macos")]
fn set_current_thread_realtime_priority() {
    // Use macOS Mach thread policy API - this is what professional DAWs use
    // (Logic Pro, Ableton, Pro Tools, etc.)
    // Does NOT require root, and is more effective than SCHED_FIFO on macOS

    #[repr(C)]
    struct ThreadTimeConstraintPolicy {
        period: u32,      // Interval between processing (in Mach absolute time units)
        computation: u32, // Time needed for computation
        constraint: u32,  // Maximum time before deadline
        preemptible: i32, // Can be preempted?
    }

    const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
    const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;

    extern "C" {
        fn mach_thread_self() -> u32;
        fn thread_policy_set(
            thread: u32,
            flavor: u32,
            policy_info: *const ThreadTimeConstraintPolicy,
            count: u32,
        ) -> i32;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe {
        // Get timebase info to convert nanoseconds to Mach absolute time
        let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
        mach_timebase_info(&mut timebase);

        // Convert nanoseconds to Mach absolute time units
        let ns_to_abs =
            |ns: u64| -> u32 { ((ns * timebase.denom as u64) / timebase.numer as u64) as u32 };

        // Audio timing constraints (for 48kHz, ~1ms period with small buffer)
        // period: how often the thread runs (e.g., every 1ms for audio callback)
        // computation: how much CPU time it needs per period
        // constraint: deadline (must complete within this time)
        let period_ns = 1_000_000; // 1ms period (audio callback interval)
        let computation_ns = 500_000; // 0.5ms computation time
        let constraint_ns = 1_000_000; // 1ms deadline

        let policy = ThreadTimeConstraintPolicy {
            period: ns_to_abs(period_ns),
            computation: ns_to_abs(computation_ns),
            constraint: ns_to_abs(constraint_ns),
            preemptible: 0, // Don't preempt during computation
        };

        let thread = mach_thread_self();
        let result = thread_policy_set(
            thread,
            THREAD_TIME_CONSTRAINT_POLICY,
            &policy,
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        );

        if result == 0 {
            println!("Audio thread: macOS real-time priority enabled (TIME_CONSTRAINT)");
        } else {
            // Fall back to nice value
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
            println!(
                "Audio thread: using nice -20 (TIME_CONSTRAINT failed: {})",
                result
            );
        }
    }
}

#[cfg(target_os = "linux")]
fn set_current_thread_realtime_priority() {
    unsafe {
        let policy = libc::SCHED_FIFO;
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = REALTIME_PRIORITY;
        let result = libc::pthread_setschedparam(libc::pthread_self(), policy, &param);
        if result != 0 {
            // Fall back to nice value if SCHED_FIFO fails (requires rtprio limit)
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
            println!("Audio thread: using nice -20 (SCHED_FIFO requires rtprio config)");
        } else {
            println!(
                "Audio thread: real-time priority enabled (SCHED_FIFO {})",
                REALTIME_PRIORITY
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn set_current_thread_realtime_priority() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, SetPriorityClass, SetThreadPriority,
        HIGH_PRIORITY_CLASS, THREAD_PRIORITY_TIME_CRITICAL,
    };

    unsafe {
        // First, elevate the process priority class
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);

        // Then set thread to TIME_CRITICAL (highest within the priority class)
        let result = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        if result.is_ok() {
            println!("Audio thread: real-time priority enabled (HIGH + TIME_CRITICAL)");
        } else {
            println!("Audio thread: failed to set thread priority");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn set_current_thread_realtime_priority() {
    println!("Audio thread: real-time priority not supported on this platform");
}

/// Streaming state managed by Tauri
pub struct StreamingState {
    /// Command sender to audio thread (None if not started)
    cmd_tx: Mutex<Option<std_mpsc::Sender<StreamingCommand>>>,
    /// Flag indicating if streaming is active
    is_active: Arc<AtomicBool>,
    /// Flag indicating if microphone is muted
    is_muted: Arc<AtomicBool>,
    /// Whether local monitoring is on (ADR-033)
    is_monitoring: Arc<AtomicBool>,
    /// Current input audio level (0-100, RMS normalized)
    input_level: Arc<AtomicU32>,
    /// Current output audio level (0-100, for master meter)
    output_level: Arc<AtomicU32>,
    /// Remote address currently connected to
    remote_addr: Mutex<Option<String>>,
    /// Connection statistics (updated by audio thread)
    stats: Arc<RwLock<Option<ConnectionStats>>>,
    /// Measured bandwidth against what the preset needs (updated by audio thread)
    bandwidth: Arc<RwLock<Option<BandwidthSnapshot>>>,
    /// Connection state as reported by the audio thread (ADR-022)
    connection_state: Arc<RwLock<ConnectionStateSnapshot>>,
    /// Jitter buffer delay in microseconds (updated by audio thread)
    ///
    /// Integer microseconds rather than a float, so it fits an atomic without
    /// bit-casting. Feeds the latency breakdown shown to the user.
    jitter_buffer_delay_us: Arc<AtomicU32>,
    /// Current buffer size (frame_size) for latency calculations
    buffer_size: Mutex<u32>,
    /// Buffer underrun count (audio glitches due to CPU/scheduling)
    underrun_count: Arc<AtomicU64>,
    /// How many times adaptation has moved the play-out delay this session
    delay_adjustments: Arc<AtomicU64>,
    /// How many times the connection dropped and began to reconnect
    reconnect_count: Arc<AtomicU64>,
    /// Peer (received) audio volume (0-200, 100 = unity gain)
    peer_volume: Arc<AtomicU32>,
    /// Master output volume (0-200, 100 = unity gain)
    master_volume: Arc<AtomicU32>,
    /// Peer (received) audio pan (-100 = full left, 0 = center, 100 = full right)
    peer_pan: Arc<std::sync::atomic::AtomicI32>,
    /// Local (microphone) input volume (0-200, 100 = unity gain)
    local_volume: Arc<AtomicU32>,
    /// Local (microphone) input pan (-100 = full left, 0 = center, 100 = full right)
    local_pan: Arc<std::sync::atomic::AtomicI32>,
    /// Current sample rate in Hz (ADR-013: user-selectable)
    sample_rate: Arc<AtomicU32>,
    /// Peer's latency info (received from remote peer via LatencyInfoMessage)
    peer_latency_info: Arc<RwLock<Option<PeerLatencyInfo>>>,
    /// UDP socket bound ahead of streaming so its port can be advertised to
    /// peers before any audio flows (ADR-026).
    ///
    /// Held as `std::net::UdpSocket`, not a `Connection`: the audio thread
    /// runs its own Tokio runtime, and a Tokio socket only receives wakeups on
    /// the runtime that registered it. The descriptor moves; the registration
    /// happens in `run_audio_streaming`.
    prepared_socket: Mutex<Option<std::net::UdpSocket>>,
    /// Address `prepared_socket` is bound to, kept so repeated prepares report
    /// the same one.
    prepared_addr: Mutex<Option<SocketAddr>>,
}

impl StreamingState {
    pub fn new() -> Self {
        Self {
            cmd_tx: Mutex::new(None),
            is_active: Arc::new(AtomicBool::new(false)),
            is_muted: Arc::new(AtomicBool::new(false)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            input_level: Arc::new(AtomicU32::new(0)),
            output_level: Arc::new(AtomicU32::new(0)),
            remote_addr: Mutex::new(None),
            stats: Arc::new(RwLock::new(None)),
            bandwidth: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(None)),
            jitter_buffer_delay_us: Arc::new(AtomicU32::new(0)),
            buffer_size: Mutex::new(64), // Default: 64 samples
            underrun_count: Arc::new(AtomicU64::new(0)),
            delay_adjustments: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            peer_volume: Arc::new(AtomicU32::new(100)), // 100 = unity gain
            master_volume: Arc::new(AtomicU32::new(100)), // 100 = unity gain
            peer_pan: Arc::new(std::sync::atomic::AtomicI32::new(0)), // 0 = center
            local_volume: Arc::new(AtomicU32::new(100)), // 100 = unity gain
            local_pan: Arc::new(std::sync::atomic::AtomicI32::new(0)), // 0 = center
            sample_rate: Arc::new(AtomicU32::new(DEFAULT_SAMPLE_RATE)), // ADR-013
            peer_latency_info: Arc::new(RwLock::new(None)),
            prepared_socket: Mutex::new(None),
            prepared_addr: Mutex::new(None),
        }
    }

    /// Bind the audio socket, or report the address of the one already bound.
    async fn prepare_socket(&self) -> Result<SocketAddr, String> {
        let mut addr_lock = self.prepared_addr.lock().await;
        if let Some(addr) = *addr_lock {
            return Ok(addr);
        }

        // Port 0 lets the OS choose; the bound address is what we advertise.
        let socket = jamjam::network::bind_std("0.0.0.0:0")
            .map_err(|e| format!("Failed to bind the audio socket: {}", e))?;
        let addr = socket
            .local_addr()
            .map_err(|e| format!("Failed to read the audio socket address: {}", e))?;

        *self.prepared_socket.lock().await = Some(socket);
        *addr_lock = Some(addr);

        tracing::info!("Audio socket prepared on {}", addr);
        Ok(addr)
    }

    /// Hand the prepared socket to the audio thread.
    ///
    /// The address is forgotten with it: the socket now belongs to the audio
    /// session, so the next prepare (a new peer after this session ended) has
    /// to bind a fresh one rather than report a port nothing listens on.
    async fn take_prepared_socket(&self) -> Option<std::net::UdpSocket> {
        let mut addr_lock = self.prepared_addr.lock().await;
        let socket = self.prepared_socket.lock().await.take();
        *addr_lock = None;
        socket
    }

    /// Address candidates for the prepared audio socket, with the public
    /// address STUN reports for that socket itself.
    ///
    /// Returns `None` if there is no prepared socket to ask through - none was
    /// prepared, or `streaming_start` already took it.
    ///
    /// The socket stays locked while STUN runs, so `streaming_start` waits for
    /// the answer instead of moving the socket into the receive loop, which
    /// would swallow the reply.
    pub async fn gather_candidates(&self) -> Option<Vec<jamjam::network::AddressCandidate>> {
        let prepared = self.prepared_socket.lock().await;
        let socket = prepared.as_ref()?;
        // A duplicate descriptor shares the socket - the same port, the same NAT
        // mapping - while leaving the original untouched for `streaming_start`.
        let asking = match socket.try_clone().and_then(tokio::net::UdpSocket::from_std) {
            Ok(socket) => Arc::new(socket),
            Err(e) => {
                tracing::warn!("Could not ask STUN through the audio socket: {}", e);
                return None;
            }
        };
        Some(jamjam::network::gather_candidates(&asking).await)
    }

    /// Get local latency info based on audio config
    fn local_latency_info(&self) -> LocalLatencyInfo {
        let frame_size = self.buffer_size.try_lock().map(|s| *s).unwrap_or(64);
        let sample_rate = self.sample_rate.load(Ordering::SeqCst);
        let mut info = LocalLatencyInfo::from_audio_config(frame_size, sample_rate, "pcm");

        // `from_audio_config` leaves the jitter buffer at 0 because it only knows
        // the audio configuration. Without this the breakdown shown to the user
        // omits the buffer entirely - up to 42.67ms on high-quality (ADR-019).
        let delay_us = self.jitter_buffer_delay_us.load(Ordering::SeqCst);
        info.set_jitter_buffer_ms(delay_us as f32 / 1000.0);
        info
    }
}

impl Default for StreamingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Commands sent to the audio thread
enum StreamingCommand {
    Stop,
    SetInputDevice(Option<String>),
    SetOutputDevice(Option<String>),
    SetMute(bool),
    SetMonitoring(bool),
    SetPeerVolume(f32),
    SetMasterVolume(f32),
    SetPeerPan(i32),
    SetLocalVolume(f32),
    SetLocalPan(i32),
    /// Rebuild the receive jitter buffer at a new depth (ADR-020)
    SetJitterBufferFrames(u32),
    /// Resume probing after automatic recovery gave up (REQ-CON-110)
    Reconnect,
}

impl StreamingState {
    /// The link's latest reading for the usage totals: the round-trip time
    /// (`None` before the first sample) and the packet loss rate (0.0 to
    /// 1.0). `None` while there is no connection.
    pub(crate) fn link_reading(&self) -> Option<(Option<f32>, f32)> {
        let stats = self.stats.read().ok()?;
        stats.as_ref().map(|s| (s.rtt_ms, s.packet_loss_rate))
    }

    /// Buffer underruns since streaming last started.
    pub(crate) fn underruns(&self) -> u64 {
        self.underrun_count.load(Ordering::Relaxed)
    }

    /// Times the connection began to reconnect since streaming last started.
    pub(crate) fn reconnects(&self) -> u64 {
        self.reconnect_count.load(Ordering::Relaxed)
    }

    /// Apply a new jitter buffer depth to a session that is already running
    ///
    /// Called when the preset changes mid-session (REQ-LAT-106). Only the depth
    /// is applied live: the frame size is fixed for the lifetime of the audio
    /// engines, so a preset's new frame size takes effect on the next connect.
    ///
    /// A no-op when no session is active.
    pub async fn apply_jitter_buffer_frames(&self, frames: u32) {
        if !self.is_active.load(Ordering::SeqCst) {
            return;
        }
        let tx = self.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetJitterBufferFrames(frames));
        }
    }
}

/// Connection state and, when it failed, why
///
/// Published by the audio thread and read by the IPC layer.
type ConnectionStateSnapshot = Option<(String, Option<String>)>;

/// What the link carries versus what the preset needs
///
/// Bitrate adaptation would need a variable-rate codec, which no preset uses, so
/// this is a warning signal rather than an input to adaptation (ADR-021).
#[derive(Debug, Clone, Copy)]
pub struct BandwidthSnapshot {
    /// Measured throughput in bits per second
    pub measured_bps: f64,
    /// Bits per second the preset needs in one direction
    pub required_bps: f64,
    /// How the measurement compares with the requirement, or `None` when the
    /// interval carried zero bytes (nothing is arriving from the peer, which is
    /// not the same as a narrow link - REQ-LAT-127)
    pub status: Option<BandwidthStatus>,
}

/// Network statistics for IPC
#[derive(Debug, Clone, Serialize)]
pub struct NetworkStats {
    /// Round-trip time in milliseconds, or `None` before the first sample
    /// arrives (REQ-LAT-130)
    pub rtt_ms: Option<f32>,
    /// Jitter in milliseconds
    pub jitter_ms: f32,
    /// Packet loss percentage (0-100)
    pub packet_loss_percent: f32,
    /// Connection quality band: "good" | "fair" | "poor", or `None` before
    /// the first RTT sample arrives (REQ-LAT-121, REQ-LAT-130)
    ///
    /// Classified in the core library so the UI cannot drift from the
    /// thresholds in latency.feature.
    pub quality: Option<String>,
    /// Measured downstream throughput in bits per second, 0 before the first
    /// measurement interval completes
    pub measured_bps: f64,
    /// Bits per second this preset needs in one direction
    pub required_bps: f64,
    /// "sufficient" | "marginal" | "insufficient", or None before measurement
    ///
    /// Bitrate adaptation needs a variable-rate codec, which no preset uses, so
    /// this is a warning signal rather than an input to adaptation (ADR-021).
    pub bandwidth_status: Option<String>,
    /// Connection uptime in seconds
    pub uptime_seconds: u64,
    /// Total packets sent
    pub packets_sent: u64,
    /// Total packets received
    pub packets_received: u64,
    /// Total bytes sent
    pub bytes_sent: u64,
    /// Total bytes received
    pub bytes_received: u64,
}

/// Audio quality metrics for IPC
#[derive(Debug, Clone, Serialize)]
pub struct AudioQuality {
    /// Number of buffer underruns (audio glitches due to CPU/scheduling)
    pub underrun_count: u64,
    /// Number of times the play-out delay was adjusted automatically. The UI
    /// tells the user when this goes up (REQ-LAT-108).
    pub delay_adjustments: u64,
}

/// Latency component breakdown for IPC
#[derive(Debug, Clone, Serialize)]
pub struct LatencyComponent {
    /// Component name
    pub name: String,
    /// Latency in milliseconds
    pub ms: f32,
    /// Additional info (e.g., "128 samples @ 48000 Hz")
    pub info: Option<String>,
}

/// Detailed latency breakdown for IPC
#[derive(Debug, Clone, Serialize)]
pub struct DetailedLatency {
    /// Upstream components (self -> peer)
    pub upstream: Vec<LatencyComponent>,
    /// Upstream total in ms
    pub upstream_total_ms: f32,
    /// Downstream components (peer -> self)
    pub downstream: Vec<LatencyComponent>,
    /// Downstream total in ms
    pub downstream_total_ms: f32,
    /// Round-trip total in ms
    pub roundtrip_total_ms: f32,
}

/// Peer audio configuration info for IPC
#[derive(Debug, Clone, Serialize)]
pub struct PeerAudioInfo {
    /// Peer's sample rate in Hz
    pub sample_rate: u32,
    /// Peer's frame size in samples
    pub frame_size: u32,
    /// Peer's codec name
    pub codec: String,
    /// Whether resampling is needed (peer sample rate != local sample rate)
    pub needs_resampling: bool,
    /// Peer's transmit channel count (1 = mono, 2 = stereo)
    pub channel_count: u32,
}

/// Streaming status for IPC
#[derive(Debug, Clone, Serialize)]
pub struct StreamingStatus {
    pub is_active: bool,
    pub remote_addr: Option<String>,
    /// Whether microphone is muted
    pub is_muted: bool,
    /// Whether the user hears their own input directly (ADR-033)
    pub is_monitoring: bool,
    /// Current input audio level (0-100)
    pub input_level: u32,
    /// Current output audio level (0-100, for master meter)
    pub output_level: u32,
    /// Network statistics
    pub network: Option<NetworkStats>,
    /// Detailed latency breakdown
    pub latency: Option<DetailedLatency>,
    /// Audio quality metrics
    pub audio_quality: Option<AudioQuality>,
    /// Peer's audio configuration (if received via LatencyInfoMessage)
    pub peer_audio: Option<PeerAudioInfo>,
    /// Connection state: "connected" | "reconnecting" | "failed" | ... (ADR-022)
    ///
    /// The UI shows reconnection progress from this, and asks whether to retry
    /// once it reads "failed" (REQ-CON-110).
    pub connection_state: Option<String>,
    /// Why the connection failed, when `connection_state` is "failed"
    pub connection_error: Option<String>,
}

/// Binds the audio socket and reports the address peers should send to.
///
/// Called when the user enters a room, before anyone knows where to send
/// audio. Two apps cannot start streaming to each other otherwise: each waits
/// for the other's address, and neither has one to give (ADR-026).
///
/// Idempotent - repeated calls report the same address, so a rejoin or a
/// double-invoke does not move the port out from under a peer that already
/// learned it.
#[tauri::command]
pub async fn streaming_prepare(state: tauri::State<'_, StreamingState>) -> Result<String, String> {
    state.prepare_socket().await.map(|addr| addr.to_string())
}

/// Address candidates to race for `addr` (REQ-CON-113): `others`, in the
/// order the caller ranked them (LAN before public), deduplicated, with
/// `addr` appended if it was not already among them. `addr` is always kept
/// as a fallback so a caller that could not name any other candidate (or an
/// older peer) still connects.
fn merge_candidate_addrs(addr: SocketAddr, others: &[SocketAddr]) -> Vec<SocketAddr> {
    let mut merged: Vec<SocketAddr> = Vec::new();
    for &candidate in others {
        if !merged.contains(&candidate) {
            merged.push(candidate);
        }
    }
    if !merged.contains(&addr) {
        merged.push(addr);
    }
    merged
}

/// Start audio streaming to a remote peer
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn streaming_start(
    remote_addr: String,
    // The peer's other address candidates (public and LAN), tried alongside
    // `remote_addr` so a peer on the same network is reached directly
    // instead of only through its public address (REQ-CON-113).
    remote_candidates: Option<Vec<String>>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    buffer_size: u32,
    sample_rate: Option<u32>,
    state: tauri::State<'_, StreamingState>,
    config_state: tauri::State<'_, crate::config::ConfigState>,
    usage: tauri::State<'_, crate::usage::UsageState>,
) -> Result<(), String> {
    // Check if already streaming
    if state.is_active.load(Ordering::SeqCst) {
        return Err("Streaming already active".to_string());
    }

    // Use provided sample rate or default (ADR-013)
    let sample_rate = sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

    // Jitter buffer depth comes from the selected preset (ADR-019/ADR-020).
    // The frame *duration* comes from the buffer_size actually in use, so the
    // resulting delay is correct even if the two were configured separately.
    let preset = config_state
        .get()
        .map(|config| config.preset)
        .unwrap_or_default();

    // Transmit channel count, told to the peer so its mixer can show whether
    // we're sending mono or stereo.
    let transmit_channels = config_state
        .get()
        .map(|config| config.transmit_channels)
        .unwrap_or(2);

    // Parse remote address
    let addr: SocketAddr = remote_addr
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    // Build the candidate list to race (REQ-CON-113): the peer's other
    // candidates, in the order the caller ranked them (LAN before public),
    // plus `addr` itself as a fallback.
    let mut parsed_candidates: Vec<SocketAddr> = Vec::new();
    for candidate in remote_candidates.into_iter().flatten() {
        parsed_candidates.push(
            candidate
                .parse()
                .map_err(|e| format!("Invalid candidate address: {}", e))?,
        );
    }
    let candidate_addrs = merge_candidate_addrs(addr, &parsed_candidates);

    let redact = crate::logging::redaction_enabled();
    tracing::info!(
        "Streaming start: remote={} candidates={:?} input={:?} output={:?} buffer={} sample_rate={} preset={:?}",
        addr,
        candidate_addrs,
        crate::logging::redact_device_id(input_device_id.as_deref().unwrap_or("system default"), redact),
        crate::logging::redact_device_id(output_device_id.as_deref().unwrap_or("system default"), redact),
        buffer_size,
        sample_rate,
        preset
    );

    // Create channel for commands to audio thread
    let (cmd_tx, cmd_rx) = std_mpsc::channel::<StreamingCommand>();

    // Store command sender
    {
        let mut tx = state.cmd_tx.lock().await;
        *tx = Some(cmd_tx);
    }

    // Store remote address
    {
        let mut addr_lock = state.remote_addr.lock().await;
        *addr_lock = Some(remote_addr.clone());
    }

    // Store buffer size for latency display
    {
        let mut bs = state.buffer_size.lock().await;
        *bs = buffer_size;
    }

    // Store sample rate (ADR-013)
    state.sample_rate.store(sample_rate, Ordering::SeqCst);

    let is_active = state.is_active.clone();
    let is_muted = state.is_muted.clone();
    let is_monitoring = state.is_monitoring.clone();
    let input_level = state.input_level.clone();
    let output_level = state.output_level.clone();
    let shared_stats = state.stats.clone();
    let shared_bandwidth = state.bandwidth.clone();
    let shared_jitter_delay_us = state.jitter_buffer_delay_us.clone();
    let shared_connection_state = state.connection_state.clone();
    let underrun_count = state.underrun_count.clone();
    let delay_adjustments = state.delay_adjustments.clone();
    let reconnect_count = state.reconnect_count.clone();
    let peer_volume = state.peer_volume.clone();
    let master_volume = state.master_volume.clone();
    let peer_pan = state.peer_pan.clone();
    let local_volume = state.local_volume.clone();
    let local_pan = state.local_pan.clone();
    let shared_peer_latency_info = state.peer_latency_info.clone();
    let usage_reporter = usage.reporter().clone();
    // Hand the pre-bound socket (if any) to the audio thread, which registers
    // it with its own runtime. Taking it means a later prepare rebinds rather
    // than handing out a socket already in use.
    let prepared_socket = state.take_prepared_socket().await;

    // Reset state on new connection
    state.is_muted.store(false, Ordering::SeqCst);
    // Off at the start of every session: with speakers, a monitored microphone
    // can feed back, so hearing yourself is something the user asks for.
    state.is_monitoring.store(false, Ordering::SeqCst);
    state.input_level.store(0, Ordering::SeqCst);
    state.output_level.store(0, Ordering::SeqCst);
    state.underrun_count.store(0, Ordering::SeqCst);
    state.delay_adjustments.store(0, Ordering::SeqCst);
    state.reconnect_count.store(0, Ordering::SeqCst);
    // Reset volumes to unity gain and pan to center
    state.peer_volume.store(100, Ordering::SeqCst);
    state.master_volume.store(100, Ordering::SeqCst);
    state.peer_pan.store(0, Ordering::SeqCst);
    state.local_volume.store(100, Ordering::SeqCst);
    state.local_pan.store(0, Ordering::SeqCst);
    // Clear peer latency info
    if let Ok(mut info) = state.peer_latency_info.write() {
        *info = None;
    }

    // Mark as active BEFORE spawning thread to avoid race condition
    state.is_active.store(true, Ordering::SeqCst);

    // Spawn audio thread with real-time priority
    thread::spawn(move || {
        // Set real-time priority immediately upon thread start
        set_current_thread_realtime_priority();

        // Create tokio runtime for this thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async move {
            if let Err(e) = run_audio_streaming(
                candidate_addrs,
                prepared_socket,
                input_device_id,
                output_device_id,
                buffer_size,
                preset,
                sample_rate,
                transmit_channels,
                cmd_rx,
                &is_active,
                &is_muted,
                &is_monitoring,
                &input_level,
                &output_level,
                &shared_stats,
                &shared_bandwidth,
                &shared_jitter_delay_us,
                &shared_connection_state,
                &underrun_count,
                &delay_adjustments,
                &reconnect_count,
                &peer_volume,
                &master_volume,
                &peer_pan,
                &local_volume,
                &local_pan,
                &shared_peer_latency_info,
            )
            .await
            {
                tracing::error!("Audio streaming failed: {}", e);
                crate::usage::record_streaming_failure(&usage_reporter, &e);
            } else {
                tracing::info!("Audio streaming ended");
            }
            is_active.store(false, Ordering::SeqCst);
            // Clear stats on disconnect
            if let Ok(mut stats) = shared_stats.write() {
                *stats = None;
            }
        });
    });

    Ok(())
}

/// Stop audio streaming
#[tauri::command]
pub async fn streaming_stop(state: tauri::State<'_, StreamingState>) -> Result<(), String> {
    if !state.is_active.load(Ordering::SeqCst) {
        return Ok(()); // Already stopped
    }

    tracing::info!("Streaming stop requested");

    // Send stop command
    {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::Stop);
        }
    }

    // Clear state
    {
        let mut tx = state.cmd_tx.lock().await;
        *tx = None;
    }
    {
        let mut addr = state.remote_addr.lock().await;
        *addr = None;
    }
    {
        if let Ok(mut stats) = state.stats.write() {
            *stats = None;
        }
    }
    {
        if let Ok(mut info) = state.peer_latency_info.write() {
            *info = None;
        }
    }

    // Wait briefly for thread to finish
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    state.is_active.store(false, Ordering::SeqCst);

    Ok(())
}

/// Get streaming status
#[tauri::command]
pub async fn streaming_status(
    state: tauri::State<'_, StreamingState>,
) -> Result<StreamingStatus, String> {
    let is_active = state.is_active.load(Ordering::SeqCst);
    let is_muted = state.is_muted.load(Ordering::SeqCst);
    let is_monitoring = state.is_monitoring.load(Ordering::SeqCst);
    let input_level = state.input_level.load(Ordering::SeqCst);
    let output_level = state.output_level.load(Ordering::SeqCst);
    let remote_addr = state.remote_addr.lock().await.clone();
    let stats = state.stats.read().ok().and_then(|s| s.clone());
    let bandwidth = state.bandwidth.read().ok().and_then(|b| *b);
    let connection_state = state.connection_state.read().ok().and_then(|c| c.clone());

    let (network, latency) = if let Some(ref s) = stats {
        let local_info = state.local_latency_info();
        let breakdown =
            LatencyBreakdown::calculate(&local_info, None, s.rtt_ms.unwrap_or(0.0), s.jitter_ms);
        let frame_size = state.buffer_size.lock().await;

        let network = NetworkStats {
            rtt_ms: s.rtt_ms,
            jitter_ms: s.jitter_ms,
            packet_loss_percent: s.packet_loss_rate * 100.0,
            quality: s.quality().map(|q| q.as_str().to_string()),
            measured_bps: bandwidth.map(|b| b.measured_bps).unwrap_or(0.0),
            required_bps: bandwidth.map(|b| b.required_bps).unwrap_or(0.0),
            bandwidth_status: bandwidth.map(|b| status_label(b.status).to_string()),
            uptime_seconds: s.uptime_seconds,
            packets_sent: s.packets_sent,
            packets_received: s.packets_received,
            bytes_sent: s.bytes_sent,
            bytes_received: s.bytes_received,
        };

        let upstream = vec![
            LatencyComponent {
                name: "Capture buffer".to_string(),
                ms: breakdown.upstream.capture_buffer_ms,
                info: Some(format!(
                    "{} samples @ {} Hz",
                    *frame_size,
                    state.sample_rate.load(Ordering::SeqCst)
                )),
            },
            LatencyComponent {
                name: "Encode (pcm)".to_string(),
                ms: breakdown.upstream.encode_ms,
                info: None,
            },
            LatencyComponent {
                name: "Network".to_string(),
                ms: breakdown.upstream.network_ms,
                info: Some("RTT/2".to_string()),
            },
        ];

        let downstream = vec![
            LatencyComponent {
                name: "Network".to_string(),
                ms: breakdown.downstream.network_ms,
                info: Some("RTT/2".to_string()),
            },
            LatencyComponent {
                name: "Jitter buffer".to_string(),
                ms: breakdown.downstream.jitter_buffer_ms,
                info: None,
            },
            LatencyComponent {
                name: "Decode (pcm)".to_string(),
                ms: breakdown.downstream.decode_ms,
                info: None,
            },
            LatencyComponent {
                name: "Playback buffer".to_string(),
                ms: breakdown.downstream.playback_buffer_ms,
                info: Some(format!("{} samples", *frame_size)),
            },
        ];

        let latency = DetailedLatency {
            upstream,
            upstream_total_ms: breakdown.upstream_total_ms,
            downstream,
            downstream_total_ms: breakdown.downstream_total_ms,
            roundtrip_total_ms: breakdown.roundtrip_total_ms,
        };

        (Some(network), Some(latency))
    } else {
        (None, None)
    };

    // Audio quality metrics
    let audio_quality = if is_active {
        Some(AudioQuality {
            underrun_count: state.underrun_count.load(Ordering::Relaxed),
            delay_adjustments: state.delay_adjustments.load(Ordering::Relaxed),
        })
    } else {
        None
    };

    // Peer audio info (ADR-013: sample rate communication)
    let peer_audio = state.peer_latency_info.read().ok().and_then(|info| {
        info.as_ref().map(|p| {
            let local_sample_rate = state.sample_rate.load(Ordering::SeqCst);
            PeerAudioInfo {
                sample_rate: p.sample_rate,
                frame_size: p.frame_size,
                codec: p.codec.clone(),
                needs_resampling: p.sample_rate != local_sample_rate,
                channel_count: p.channel_count as u32,
            }
        })
    });

    Ok(StreamingStatus {
        is_active,
        remote_addr,
        is_muted,
        is_monitoring,
        input_level,
        output_level,
        network,
        latency,
        audio_quality,
        peer_audio,
        connection_state: connection_state.as_ref().map(|(state, _)| state.clone()),
        connection_error: connection_state.and_then(|(_, error)| error),
    })
}

/// Retry the connection after automatic recovery gave up
///
/// Backs the "reconnect?" prompt the UI shows when `connection_state` reads
/// "failed" (REQ-CON-110). A no-op when no session is active.
#[tauri::command]
pub async fn streaming_reconnect(state: tauri::State<'_, StreamingState>) -> Result<(), String> {
    if !state.is_active.load(Ordering::SeqCst) {
        return Err("Streaming is not active".to_string());
    }
    let tx = state.cmd_tx.lock().await;
    match *tx {
        Some(ref sender) => sender
            .send(StreamingCommand::Reconnect)
            .map_err(|e| format!("Failed to request reconnection: {}", e)),
        None => Err("No active session".to_string()),
    }
}

/// Set input device during streaming
#[tauri::command]
pub async fn streaming_set_input_device(
    device_id: Option<String>,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    if !state.is_active.load(Ordering::SeqCst) {
        return Err("Streaming is not active".to_string());
    }

    let tx = state.cmd_tx.lock().await;
    if let Some(ref sender) = *tx {
        sender
            .send(StreamingCommand::SetInputDevice(device_id))
            .map_err(|e| format!("Failed to send command: {}", e))?;
    }

    Ok(())
}

/// Set output device during streaming
#[tauri::command]
pub async fn streaming_set_output_device(
    device_id: Option<String>,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    if !state.is_active.load(Ordering::SeqCst) {
        return Err("Streaming is not active".to_string());
    }

    let tx = state.cmd_tx.lock().await;
    if let Some(ref sender) = *tx {
        sender
            .send(StreamingCommand::SetOutputDevice(device_id))
            .map_err(|e| format!("Failed to send command: {}", e))?;
    }

    Ok(())
}

/// Set mute state
#[tauri::command]
pub async fn streaming_set_mute(
    muted: bool,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    // Can set mute state even when not streaming
    state.is_muted.store(muted, Ordering::SeqCst);

    // If streaming, also send command to audio thread
    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetMute(muted));
        }
    }

    Ok(())
}

/// Switch local monitoring: hearing your own input directly, without the
/// network delay (ADR-033). Applies to the running session, if there is one.
#[tauri::command]
pub async fn streaming_set_monitoring(
    enabled: bool,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    state.is_monitoring.store(enabled, Ordering::SeqCst);

    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetMonitoring(enabled));
        }
    }

    Ok(())
}

/// Get mute state
#[tauri::command]
pub async fn streaming_get_mute(state: tauri::State<'_, StreamingState>) -> Result<bool, String> {
    Ok(state.is_muted.load(Ordering::SeqCst))
}

/// Get current input audio level (0-100)
#[tauri::command]
pub async fn streaming_get_input_level(
    state: tauri::State<'_, StreamingState>,
) -> Result<u32, String> {
    Ok(state.input_level.load(Ordering::SeqCst))
}

/// Set peer (received audio) volume
/// Volume is 0-200 where 100 = unity gain (1.0x), 200 = 2.0x
#[tauri::command]
pub async fn streaming_set_peer_volume(
    volume: u32,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let clamped = volume.min(200);
    state.peer_volume.store(clamped, Ordering::SeqCst);

    // If streaming, also send command to audio thread
    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetPeerVolume(clamped as f32 / 100.0));
        }
    }

    Ok(())
}

/// Get peer volume (0-200, 100 = unity)
#[tauri::command]
pub async fn streaming_get_peer_volume(
    state: tauri::State<'_, StreamingState>,
) -> Result<u32, String> {
    Ok(state.peer_volume.load(Ordering::SeqCst))
}

/// Set master output volume
/// Volume is 0-200 where 100 = unity gain (1.0x), 200 = 2.0x
#[tauri::command]
pub async fn streaming_set_master_volume(
    volume: u32,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let clamped = volume.min(200);
    state.master_volume.store(clamped, Ordering::SeqCst);

    // If streaming, also send command to audio thread
    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetMasterVolume(clamped as f32 / 100.0));
        }
    }

    Ok(())
}

/// Get master volume (0-200, 100 = unity)
#[tauri::command]
pub async fn streaming_get_master_volume(
    state: tauri::State<'_, StreamingState>,
) -> Result<u32, String> {
    Ok(state.master_volume.load(Ordering::SeqCst))
}

/// Set peer (received audio) pan
/// Pan is -100 (full left) to 100 (full right), 0 = center
#[tauri::command]
pub async fn streaming_set_peer_pan(
    pan: i32,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let clamped = pan.clamp(-100, 100);
    state.peer_pan.store(clamped, Ordering::SeqCst);

    // If streaming, also send command to audio thread
    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetPeerPan(clamped));
        }
    }

    Ok(())
}

/// Get peer pan (-100 to 100, 0 = center)
#[tauri::command]
pub async fn streaming_get_peer_pan(
    state: tauri::State<'_, StreamingState>,
) -> Result<i32, String> {
    Ok(state.peer_pan.load(Ordering::SeqCst))
}

/// Set local (microphone input) volume
/// Volume is 0-200 where 100 = unity gain (1.0x), 200 = 2.0x
#[tauri::command]
pub async fn streaming_set_local_volume(
    volume: u32,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let clamped = volume.min(200);
    state.local_volume.store(clamped, Ordering::SeqCst);

    // If streaming, also send command to audio thread
    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetLocalVolume(clamped as f32 / 100.0));
        }
    }

    Ok(())
}

/// Get local volume (0-200, 100 = unity)
#[tauri::command]
pub async fn streaming_get_local_volume(
    state: tauri::State<'_, StreamingState>,
) -> Result<u32, String> {
    Ok(state.local_volume.load(Ordering::SeqCst))
}

/// Set local (microphone input) pan
/// Pan is -100 (full left) to 100 (full right), 0 = center
#[tauri::command]
pub async fn streaming_set_local_pan(
    pan: i32,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let clamped = pan.clamp(-100, 100);
    state.local_pan.store(clamped, Ordering::SeqCst);

    // If streaming, also send command to audio thread
    if state.is_active.load(Ordering::SeqCst) {
        let tx = state.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(StreamingCommand::SetLocalPan(clamped));
        }
    }

    Ok(())
}

/// Get local pan (-100 to 100, 0 = center)
#[tauri::command]
pub async fn streaming_get_local_pan(
    state: tauri::State<'_, StreamingState>,
) -> Result<i32, String> {
    Ok(state.local_pan.load(Ordering::SeqCst))
}

/// The frame source the output callback pulls from.
///
/// Runs on the audio callback, so it allocates nothing and never waits: a
/// contended buffer costs one concealed frame rather than a stalled device.
/// The mixer gains are applied here rather than on the way in, so a volume or
/// pan change is heard on the very next frame.
fn playout_source(
    receive: ReceivePath,
    peer_volume: Arc<AtomicU32>,
    master_volume: Arc<AtomicU32>,
    peer_pan: Arc<std::sync::atomic::AtomicI32>,
    output_level: Arc<AtomicU32>,
    underrun_count: Arc<AtomicU64>,
    monitor: LocalMonitor,
) -> impl FnMut(&mut [f32]) -> usize + Send + 'static {
    move |out: &mut [f32]| {
        let samples = mix_peers(
            &receive,
            &peer_volume,
            &master_volume,
            &peer_pan,
            &output_level,
            &underrun_count,
            out,
        );
        // Added after the peers, whatever they did this frame: hearing yourself
        // must not wait for, or depend on, anyone being connected (ADR-033).
        monitor.mix_into(&mut out[..samples]);
        samples
    }
}

/// Fills `out` from the play-out buffer with the mixer gains applied, and
/// returns how many samples it wrote.
fn mix_peers(
    receive: &ReceivePath,
    peer_volume: &AtomicU32,
    master_volume: &AtomicU32,
    peer_pan: &std::sync::atomic::AtomicI32,
    output_level: &AtomicU32,
    underrun_count: &AtomicU64,
    out: &mut [f32],
) -> usize {
    let read = receive.read_into(out);

    match read.result {
        PlayoutResult::Priming | PlayoutResult::Starved => {
            underrun_count.fetch_add(1, Ordering::Relaxed);
            output_level.store(0, Ordering::Relaxed);
            return read.samples;
        }
        // Silence that makes room for a deeper delay is a decision, not an
        // underrun (ADR-031).
        PlayoutResult::Padded => {
            output_level.store(0, Ordering::Relaxed);
            return read.samples;
        }
        PlayoutResult::Played { .. } | PlayoutResult::Concealed { .. } => {}
    }

    let combined_vol = (peer_volume.load(Ordering::Relaxed) as f32 / 100.0)
        * (master_volume.load(Ordering::Relaxed) as f32 / 100.0);

    // Constant-power pan on top of whatever the sender already applied.
    let pan = peer_pan.load(Ordering::Relaxed);
    let angle = ((pan + 100) as f32 / 200.0) * std::f32::consts::FRAC_PI_2;
    let (left_gain, right_gain) = (angle.cos(), angle.sin());

    let samples = &mut out[..read.samples];
    for frame in samples.chunks_mut(WIRE_CHANNELS) {
        if frame.len() < WIRE_CHANNELS {
            for slot in frame.iter_mut() {
                *slot *= combined_vol;
            }
            continue;
        }
        let left = frame[0] * combined_vol;
        let right = frame[1] * combined_vol;
        frame[0] = left * left_gain + right * (1.0 - right_gain);
        frame[1] = right * right_gain + left * (1.0 - left_gain);
    }

    output_level.store(rms_level(samples), Ordering::Relaxed);
    read.samples
}

/// Converts a frame of samples to the 0-100 level the meters display.
///
/// RMS, not peak: a full-scale sine reads 71, not 100. That is deliberate -
/// the meter is meant to track perceived loudness, and a peak meter would sit
/// pinned at 100 for any normalised signal.
///
/// Extracted because the identical calculation previously appeared at three
/// call sites (capture, device switch, playback), so a correction to one would
/// silently have missed the others. It is also the only part of input
/// metering that can be verified without real audio hardware: whether
/// CoreAudio/WASAPI/ALSA delivers samples to the callback is the OS's
/// business, but whether a delivered frame turns into the right number is
/// ours.
fn rms_level(samples: &[f32]) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_squares / samples.len() as f32).sqrt();
    // NaN would make the comparison below false and round() yield 0, so clamp
    // explicitly rather than relying on that.
    if !rms.is_finite() {
        return 0;
    }
    ((rms * 100.0).min(100.0)).round() as u32
}

/// Run audio streaming in the audio thread
#[allow(clippy::too_many_arguments)]
async fn run_audio_streaming(
    remote_candidates: Vec<SocketAddr>,
    prepared_socket: Option<std::net::UdpSocket>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    buffer_size: u32,
    preset: AudioPreset,
    sample_rate: u32,
    transmit_channels: u32,
    cmd_rx: std_mpsc::Receiver<StreamingCommand>,
    is_active: &AtomicBool,
    is_muted: &AtomicBool,
    is_monitoring: &AtomicBool,
    input_level: &AtomicU32,
    output_level: &Arc<AtomicU32>,
    shared_stats: &RwLock<Option<ConnectionStats>>,
    shared_bandwidth: &RwLock<Option<BandwidthSnapshot>>,
    shared_jitter_delay_us: &AtomicU32,
    shared_connection_state: &Arc<RwLock<ConnectionStateSnapshot>>,
    underrun_count: &Arc<AtomicU64>,
    delay_adjustments: &AtomicU64,
    reconnect_count: &Arc<AtomicU64>,
    peer_volume: &Arc<AtomicU32>,
    master_volume: &Arc<AtomicU32>,
    peer_pan: &Arc<std::sync::atomic::AtomicI32>,
    local_volume: &AtomicU32,
    local_pan: &std::sync::atomic::AtomicI32,
    shared_peer_latency_info: &RwLock<Option<PeerLatencyInfo>>,
) -> Result<(), String> {
    // Capture config: mono (for network transmission)
    // Sample rate from config (ADR-013: user-selectable)
    let capture_config = AudioConfig {
        sample_rate,
        channels: 1,
        frame_size: buffer_size,
    };

    // Playback config: stereo (for pan support)
    let playback_config = AudioConfig {
        sample_rate,
        channels: 2,
        frame_size: buffer_size,
    };

    // Create connection
    // Adopt the socket bound by `streaming_prepare` so audio arrives on the
    // port already advertised to peers. Falling back to a fresh bind keeps
    // callers that advertise their own port (the CLI) working.
    let mut connection = match prepared_socket {
        Some(socket) => Connection::from_std(socket)
            .map_err(|e| format!("Failed to adopt the prepared audio socket: {}", e))?,
        None => Connection::new("0.0.0.0:0")
            .await
            .map_err(|e| format!("Failed to create connection: {}", e))?,
    };

    // Create separate audio engines for capture (mono) and playback (stereo)
    let mut capture_engine = AudioEngine::new(capture_config);
    let mut playback_engine = AudioEngine::new(playback_config);

    let input_id = input_device_id.map(DeviceId);
    let output_id = output_device_id.map(DeviceId);

    // Zero-allocation audio pipeline using rtrb for both capture and playback:
    // - Capture: rtrb ring buffer (FnMut callback, no Sync requirement)
    // - Playback: rtrb ring buffer (wait-free network callback)
    // - Send thread: dedicated std::thread (no tokio overhead)

    // Capture path: rtrb for zero-allocation capture
    let capture_rb_size = 32 * buffer_size as usize;
    let (mut capture_producer, capture_consumer) = RingBuffer::<f32>::new(capture_rb_size);
    let capture_consumer = Arc::new(std::sync::Mutex::new(capture_consumer));

    // Receive path: jitter buffer keyed by sequence number (ADR-020).
    //
    // This replaces a plain ring buffer, which played packets in arrival order
    // and could not tell a lost packet from a reordered one. Depth comes from
    // the preset; frame duration from the buffer size actually in use.
    // Configure the codec and FEC from the preset (ADR-021).
    let codec_type = preset.codec_type();
    connection
        .set_audio_encoding(AudioEncodingConfig {
            codec_type,
            sample_rate,
            channels: 2, // stereo on the wire, with local_pan already applied
            frame_size: buffer_size,
            bitrate: 0, // unused for PCM
            fec_group_size: preset.fec_group_size(),
        })
        .map_err(|e| format!("Failed to configure audio encoding: {}", e))?;

    let jitter_buffer_frames = preset.jitter_buffer_frames();
    let frame_duration_ms = (buffer_size as f32 / sample_rate as f32) * 1000.0;
    // Received audio waits in the play-out buffer, and only there, until the
    // output callback takes it (ADR-028). The CLI uses the same path.
    let receive = ReceivePath::new(codec_type, sample_rate, buffer_size, jitter_buffer_frames)
        .map_err(|e| format!("Failed to create audio decoder: {}", e))?;
    let stereo_frame_size = receive.frame_samples();
    // The delay the display and the peer have been told about. The loop below
    // compares it with the buffer's own and reports the difference, so a preset
    // switch, an adaptation and a reconnect all reach the display the same way
    // (ADR-031).
    let mut announced_delay_frames = jitter_buffer_frames;
    let jitter_buffer_delay_ms = jitter_buffer_frames as f32 * frame_duration_ms;
    shared_jitter_delay_us.store((jitter_buffer_delay_ms * 1000.0) as u32, Ordering::SeqCst);

    // Reports quality changes on the edge rather than on every stats poll.
    let mut quality_monitor = QualityMonitor::new();

    // Measures what the link actually carries, so a link too narrow for PCM can
    // be reported instead of silently dropping audio (REQ-LAT-120's manual mode).
    let mut bandwidth_estimator = BandwidthEstimator::default();
    let bandwidth_required_bps = required_bps(&preset, sample_rate, 2);
    let mut bandwidth_verdict = BandwidthVerdict::default();
    let mut last_bandwidth_status: Option<BandwidthStatus> = None;

    // Clone atomics for capture callback
    let input_level_for_capture = Arc::new(AtomicU32::new(0));
    let input_level_capture_ref = input_level_for_capture.clone();

    // Local monitoring: what the capture callback hears goes straight to the
    // output callback, never through the network (ADR-033).
    let monitor = LocalMonitor::new(buffer_size);
    monitor.set_enabled(is_monitoring.load(Ordering::SeqCst));
    let mut monitor_tap = monitor.tap();

    // Start audio capture with level metering (mono)
    // Zero-allocation: write directly to rtrb producer (FnMut, no Sync needed)
    capture_engine
        .start_capture(input_id.as_ref(), move |samples, _timestamp| {
            monitor_tap.push(samples);
            // Calculate RMS level (0-100)
            if !samples.is_empty() {
                input_level_capture_ref.store(rms_level(samples), Ordering::SeqCst);
            }
            // Write directly to rtrb - zero allocation
            if let Ok(mut chunk) = capture_producer.write_chunk_uninit(samples.len()) {
                let slices = chunk.as_mut_slices();
                // Copy samples directly to ring buffer slices
                let first_len = slices.0.len().min(samples.len());
                for (i, &sample) in samples[..first_len].iter().enumerate() {
                    slices.0[i].write(sample);
                }
                if first_len < samples.len() {
                    for (i, &sample) in samples[first_len..].iter().enumerate() {
                        slices.1[i].write(sample);
                    }
                }
                // SAFETY: we just initialized all elements
                unsafe {
                    chunk.commit_all();
                }
            }
            // If buffer full, samples are dropped (backpressure)
        })
        .map_err(|e| format!("Failed to start capture: {}", e))?;
    tracing::info!("Capture started on {:?}", input_id);

    // Start audio playback (stereo). The callback takes frames from the
    // play-out buffer itself and applies the mixer gains, so a volume change
    // is heard on the next frame and nothing queues behind the buffer
    // (ADR-028).
    playback_engine
        .start_playback_with_source(
            output_id.as_ref(),
            stereo_frame_size,
            playout_source(
                receive.clone(),
                peer_volume.clone(),
                master_volume.clone(),
                peer_pan.clone(),
                output_level.clone(),
                underrun_count.clone(),
                monitor.clone(),
            ),
        )
        .map_err(|e| format!("Failed to start playback: {}", e))?;
    tracing::info!("Playback started on {:?}", output_id);

    // Rebuild the jitter buffer when the link recovers: sequence numbers carry
    // on from before the outage, so stale packets would play out of order
    // (ADR-022).
    let receive_for_state = receive.clone();
    let shared_connection_state_for_cb = shared_connection_state.clone();
    let reconnect_count_for_cb = reconnect_count.clone();
    connection.set_state_change_callback(move |state| {
        tracing::info!("Audio connection state: {:?}", state);
        if state == ConnectionState::Connected {
            receive_for_state.reset();
        }
        if state == ConnectionState::Reconnecting {
            reconnect_count_for_cb.fetch_add(1, Ordering::Relaxed);
        }

        // Publish the transition so the UI can show reconnection progress and
        // prompt once recovery gives up (REQ-CON-110).
        if let Ok(mut published) = shared_connection_state_for_cb.write() {
            *published = Some((format!("{:?}", state).to_lowercase(), None));
        }
    });

    // Set up audio receive callback BEFORE connect
    //
    // Runs on the network task, not the realtime audio callback, so locking and
    // taking ownership of the payload are both acceptable here.
    let receive_for_callback = receive.clone();
    connection.set_audio_callback(move |sequence, payload, _timestamp| {
        if !receive_for_callback.receive(sequence, &payload) {
            tracing::trace!("Play-out buffer refused frame {}", sequence);
        }
    });

    // Connect to remote peer, racing all its candidates so a peer on the
    // same network is reached directly rather than only through its public
    // address (REQ-CON-113).
    connection
        .connect_with_candidates(&remote_candidates)
        .await
        .map_err(|e| format!("Failed to connect: {}", e))?;

    let remote_addr = connection.remote_addr();
    println!("Connected to {}. Streaming active.", remote_addr);

    // Send our latency info to the peer (ADR-013: sample rate communication)
    let local_latency_info = LatencyInfoMessage {
        capture_buffer_ms: (buffer_size as f32 / sample_rate as f32) * 1000.0,
        playback_buffer_ms: (buffer_size as f32 / sample_rate as f32) * 1000.0,
        encode_ms: 0.0, // PCM, no encoding delay
        decode_ms: 0.0, // PCM, no decoding delay
        jitter_buffer_ms: jitter_buffer_delay_ms,
        frame_size: buffer_size,
        sample_rate,
        codec: "pcm".to_string(),
        channel_count: transmit_channels as u8,
    };
    if let Err(e) = connection.send_latency_info(&local_latency_info).await {
        eprintln!("Failed to send initial latency info: {}", e);
    }

    // Wrap connection for shared access
    let connection_arc = Arc::new(tokio::sync::Mutex::new(connection));
    let connection_for_send = connection_arc.clone();

    // Create muted state flag for send thread
    let is_muted_for_send = Arc::new(AtomicBool::new(is_muted.load(Ordering::SeqCst)));
    let is_muted_send_ref = is_muted_for_send.clone();

    // Flag to signal send thread to stop
    let send_thread_running = Arc::new(AtomicBool::new(true));
    let send_thread_running_ref = send_thread_running.clone();

    // Clone local_volume for send thread
    let local_volume_for_send = Arc::new(AtomicU32::new(local_volume.load(Ordering::SeqCst)));
    let local_volume_send_ref = local_volume_for_send.clone();

    // Clone local_pan for send thread
    let local_pan_for_send = Arc::new(std::sync::atomic::AtomicI32::new(
        local_pan.load(Ordering::SeqCst),
    ));
    let local_pan_send_ref = local_pan_for_send.clone();

    // Clone buffer_size for send thread
    let send_buffer_size = buffer_size as usize;
    let capture_consumer_for_send = capture_consumer.clone();

    // Spawn dedicated thread to send captured audio (no tokio overhead)
    // This thread reads from rtrb consumer and sends via connection
    let send_thread = std::thread::spawn(move || {
        // Create a mini tokio runtime just for async send operations
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create send thread runtime");

        let mut timestamp: u32 = 0;
        let mut send_buffer = vec![0.0f32; send_buffer_size];
        // Stereo buffer for pan-converted output (2x mono size)
        let mut stereo_send_buffer = vec![0.0f32; send_buffer_size * 2];

        while send_thread_running_ref.load(Ordering::SeqCst) {
            // Try to read a frame from capture ring buffer
            let samples_read = {
                if let Ok(mut consumer) = capture_consumer_for_send.try_lock() {
                    let available = consumer.slots();
                    if available >= send_buffer_size {
                        if let Ok(chunk) = consumer.read_chunk(send_buffer_size) {
                            let slices = chunk.as_slices();
                            send_buffer[..slices.0.len()].copy_from_slice(slices.0);
                            if !slices.1.is_empty() {
                                send_buffer[slices.0.len()..send_buffer_size]
                                    .copy_from_slice(slices.1);
                            }
                            chunk.commit_all();
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if samples_read {
                // Skip sending if muted
                if !is_muted_send_ref.load(Ordering::SeqCst) {
                    let volume = local_volume_send_ref.load(Ordering::SeqCst) as f32 / 100.0;
                    let pan = local_pan_send_ref.load(Ordering::SeqCst);
                    mono_to_wire(&send_buffer, volume, pan, &mut stereo_send_buffer);

                    rt.block_on(async {
                        let conn = connection_for_send.lock().await;
                        if conn.is_connected() {
                            // Send stereo audio data
                            if let Err(e) = conn.send_audio(&stereo_send_buffer, timestamp).await {
                                eprintln!("Failed to send audio: {}", e);
                            }
                        }
                    });
                }
                timestamp = timestamp.wrapping_add(send_buffer_size as u32);
            } else {
                // No data available, sleep briefly (100µs for low latency)
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
        }
    });

    // Main loop: process received audio and check for stop command
    let mut next_stats_at = std::time::Instant::now();
    let mut next_adapt_at = std::time::Instant::now() + ADAPT_INTERVAL;

    // Pre-allocate buffers to avoid allocation in hot path
    // Receive buffer is stereo (frame_size * 2) since sender transmits stereo with local_pan applied

    loop {
        // Check for commands (non-blocking)
        match cmd_rx.try_recv() {
            Ok(StreamingCommand::Stop) => {
                println!("Stopping streaming...");
                break;
            }
            Ok(StreamingCommand::SetInputDevice(device_id)) => {
                println!("Switching input device to: {:?}", device_id);
                let new_device_id = device_id.map(DeviceId);

                // Create new rtrb ring buffer for the new capture
                let (mut new_producer, new_consumer) = RingBuffer::<f32>::new(capture_rb_size);

                // Replace consumer in shared reference (send thread will pick this up)
                if let Ok(mut consumer_guard) = capture_consumer.lock() {
                    *consumer_guard = new_consumer;
                }

                let level_ref = input_level_for_capture.clone();
                let mut new_monitor_tap = monitor.tap();
                if let Err(e) = capture_engine.set_input_device(
                    new_device_id.as_ref(),
                    move |samples, _timestamp| {
                        new_monitor_tap.push(samples);
                        if !samples.is_empty() {
                            level_ref.store(rms_level(samples), Ordering::SeqCst);
                        }
                        // Write directly to rtrb - zero allocation
                        if let Ok(mut chunk) = new_producer.write_chunk_uninit(samples.len()) {
                            let slices = chunk.as_mut_slices();
                            let first_len = slices.0.len().min(samples.len());
                            for (i, &sample) in samples[..first_len].iter().enumerate() {
                                slices.0[i].write(sample);
                            }
                            if first_len < samples.len() {
                                for (i, &sample) in samples[first_len..].iter().enumerate() {
                                    slices.1[i].write(sample);
                                }
                            }
                            unsafe {
                                chunk.commit_all();
                            }
                        }
                    },
                ) {
                    eprintln!("Failed to switch input device: {}", e);
                }
            }
            Ok(StreamingCommand::SetOutputDevice(device_id)) => {
                println!("Switching output device to: {:?}", device_id);
                let new_device_id = device_id.map(DeviceId);
                // Rebuild the stream around the same play-out buffer: the
                // audio already waiting in it survives the switch.
                playback_engine.stop_playback();
                if let Err(e) = playback_engine.start_playback_with_source(
                    new_device_id.as_ref(),
                    stereo_frame_size,
                    playout_source(
                        receive.clone(),
                        peer_volume.clone(),
                        master_volume.clone(),
                        peer_pan.clone(),
                        output_level.clone(),
                        underrun_count.clone(),
                        monitor.clone(),
                    ),
                ) {
                    eprintln!("Failed to switch output device: {}", e);
                }
            }
            Ok(StreamingCommand::SetMute(muted)) => {
                println!("Setting mute state to: {}", muted);
                is_muted_for_send.store(muted, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetMonitoring(enabled)) => {
                monitor.set_enabled(enabled);
            }
            Ok(StreamingCommand::SetPeerVolume(vol)) => {
                println!("Setting peer volume to: {}", vol);
                peer_volume.store((vol * 100.0) as u32, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetMasterVolume(vol)) => {
                println!("Setting master volume to: {}", vol);
                master_volume.store((vol * 100.0) as u32, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetPeerPan(pan)) => {
                println!("Setting peer pan to: {}", pan);
                peer_pan.store(pan, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetLocalVolume(vol)) => {
                println!("Setting local volume to: {}", vol);
                let volume_u32 = (vol * 100.0) as u32;
                local_volume.store(volume_u32, Ordering::SeqCst);
                // Also update the send thread's copy
                local_volume_for_send.store(volume_u32, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetLocalPan(pan)) => {
                println!("Setting local pan to: {}", pan);
                local_pan.store(pan, Ordering::SeqCst);
                // Also update the send thread's copy
                local_pan_for_send.store(pan, Ordering::SeqCst);
            }
            Ok(StreamingCommand::Reconnect) => {
                println!("User requested reconnection");
                let mut conn = connection_arc.lock().await;
                if let Err(e) = conn.reconnect() {
                    eprintln!("Reconnect refused: {}", e);
                }
            }
            Ok(StreamingCommand::SetJitterBufferFrames(frames)) => {
                println!("Setting the play-out delay to {} frame(s)", frames);
                // What is held moves to the new delay (ADR-031). The display
                // and the peer hear about it from the stats tick below.
                receive.set_delay_frames(frames);
            }
            Err(std_mpsc::TryRecvError::Disconnected) => {
                println!("Command channel disconnected");
                break;
            }
            Err(std_mpsc::TryRecvError::Empty) => {
                // No command, continue
            }
        }

        // Check if we should still be active
        if !is_active.load(Ordering::SeqCst) {
            break;
        }

        // Refresh stats and the meters on a wall-clock interval. Counting loop
        // iterations would tie the cadence to how fast this loop happens to
        // spin, which is a property of the audio clock, not of anything the UI
        // cares about.
        if std::time::Instant::now() >= next_adapt_at {
            next_adapt_at = std::time::Instant::now() + ADAPT_INTERVAL;
            if let Some(frames) = receive.adapt() {
                tracing::info!("Play-out delay adjusted to {} frame(s)", frames);
                delay_adjustments.fetch_add(1, Ordering::Relaxed);
            }
        }

        // Whatever moved the delay, the display and the peer follow. The peer
        // shows a total latency that includes our buffer, so it has to hear
        // about the change - otherwise its display keeps the value from
        // connect time. If the connection is busy the peer is told on the next
        // tick.
        let delay_frames = receive.delay_frames();
        if delay_frames != announced_delay_frames {
            let delay_ms = delay_frames as f32 * frame_duration_ms;
            shared_jitter_delay_us.store((delay_ms * 1000.0) as u32, Ordering::SeqCst);

            if let Ok(conn) = connection_arc.try_lock() {
                let updated = LatencyInfoMessage {
                    jitter_buffer_ms: delay_ms,
                    ..local_latency_info.clone()
                };
                if let Err(e) = conn.send_latency_info(&updated).await {
                    eprintln!("Failed to send updated latency info: {}", e);
                }
                announced_delay_frames = delay_frames;
            }
        }

        if std::time::Instant::now() >= next_stats_at {
            next_stats_at = std::time::Instant::now() + STATS_INTERVAL;
            // Update connection stats and peer latency info
            if let Ok(conn) = connection_arc.try_lock() {
                let conn_stats = conn.stats();

                // Log quality transitions once, on the edge (REQ-LAT-107). Held
                // back until the first RTT sample arrives, so a link nobody has
                // measured yet is never logged as "good" (REQ-LAT-130).
                if let Some(rtt_ms) = conn_stats.rtt_ms {
                    if let Some(change) =
                        quality_monitor.update(rtt_ms, conn_stats.packet_loss_rate)
                    {
                        if change.is_degradation() {
                            tracing::warn!(
                                "Connection quality degraded to {} (RTT {:.1}ms, loss {:.2}%)",
                                change.to.as_str(),
                                rtt_ms,
                                conn_stats.packet_loss_rate * 100.0
                            );
                        } else {
                            tracing::info!(
                                "Connection quality is now {} (RTT {:.1}ms, loss {:.2}%)",
                                change.to.as_str(),
                                rtt_ms,
                                conn_stats.packet_loss_rate * 100.0
                            );
                        }
                    }
                }

                // Warn once when the link stops carrying what the preset needs, and once
                // when the peer stops sending anything at all (REQ-LAT-127). The snapshot
                // is published on every completed interval, not only when a status is
                // classified, so a zero-byte interval overwrites (rather than leaves in
                // place) whatever verdict was measured before the peer went quiet.
                //
                // The raw per-interval classification goes through `bandwidth_verdict`
                // before it reaches the snapshot or the log: PCM's requirement has no
                // headroom built in, so a single interval can read insufficient on a
                // perfectly healthy link, and a peer that has only just connected (a
                // test peer that replies after a deliberate delay, or plain connection
                // setup) reads as insufficient rather than idle for its first few
                // seconds (REQ-LAT-128, REQ-LAT-129).
                if bandwidth_estimator
                    .sample(conn_stats.bytes_received)
                    .is_some()
                {
                    let raw_status = bandwidth_estimator.status_for(&preset, sample_rate, 2);
                    let status = bandwidth_verdict.confirm(raw_status);
                    if let Ok(mut snapshot) = shared_bandwidth.write() {
                        *snapshot = Some(BandwidthSnapshot {
                            measured_bps: bandwidth_estimator.current_bps(),
                            required_bps: bandwidth_required_bps,
                            status,
                        });
                    }

                    if status != last_bandwidth_status {
                        match status {
                            Some(BandwidthStatus::Insufficient) => tracing::warn!(
                                "Link carries {:.0} kbps but {} needs {:.0} kbps",
                                bandwidth_estimator.current_bps() / 1000.0,
                                preset.name(),
                                bandwidth_required_bps / 1000.0
                            ),
                            Some(BandwidthStatus::Marginal) => tracing::warn!(
                                "Link has under 20% of bandwidth headroom for {}",
                                preset.name()
                            ),
                            Some(BandwidthStatus::Sufficient) => {
                                tracing::info!("Link bandwidth is sufficient for {}", preset.name())
                            }
                            None => tracing::warn!(
                                "No bytes received from the peer in the last interval"
                            ),
                        }
                        last_bandwidth_status = status;
                    }
                }

                if let Ok(mut stats) = shared_stats.write() {
                    *stats = Some(conn_stats);
                }
                // Update peer latency info (ADR-013: sample rate from remote peer)
                if let Some(peer_info) = conn.peer_latency_info() {
                    match receive.follow_peer_rate(peer_info.sample_rate) {
                        PeerRateChange::Unchanged => {}
                        PeerRateChange::Resampling {
                            from,
                            to,
                            latency_ms,
                        } => println!(
                            "Resampler created: {} Hz -> {} Hz (latency: {:.2} ms)",
                            from, to, latency_ms
                        ),
                        PeerRateChange::Passthrough => println!(
                            "Resampler disabled: peer rate matches local rate ({} Hz)",
                            sample_rate
                        ),
                        PeerRateChange::Failed(e) => {
                            eprintln!("Failed to create resampler: {}", e)
                        }
                    }
                    if let Ok(mut info) = shared_peer_latency_info.write() {
                        *info = Some(peer_info);
                    }
                }
            }
            // Update shared input level from capture callback
            input_level.store(
                input_level_for_capture.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
        }

        // Nothing to do here for audio: the output callback takes frames
        // from the play-out buffer itself, at the device's clock (ADR-028).
        // This loop only carries commands and statistics, so it waits.
        tokio::time::sleep(POP_IDLE).await;
    }

    // Cleanup: signal send thread to stop and wait for it
    send_thread_running.store(false, Ordering::SeqCst);
    let _ = send_thread.join();

    {
        let mut conn = connection_arc.lock().await;
        conn.disconnect();
    }

    capture_engine.stop_capture();
    playback_engine.stop_playback();

    println!("Streaming stopped.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jamjam::audio::PlayoutConfig;

    /// After audio starts the socket belongs to the audio session, so the
    /// next prepare must bind a new one. Reporting the old address would
    /// advertise a port nothing listens on: the audio session that follows a
    /// peer leaving would then bind an unadvertised port and hear nobody.
    #[tokio::test]
    async fn prepare_socket_after_the_socket_was_taken_binds_a_new_one() {
        let state = StreamingState::new();
        let first = state.prepare_socket().await.unwrap();
        let socket = state
            .take_prepared_socket()
            .await
            .expect("a prepared socket");
        assert_eq!(socket.local_addr().unwrap(), first);

        let second = state.prepare_socket().await.unwrap();

        assert_ne!(second, first, "the taken socket still holds the first port");
        assert!(state.take_prepared_socket().await.is_some());
    }

    /// A repeated prepare (re-entering a room, a double invoke) must not move
    /// the port out from under a peer that already learned it.
    #[tokio::test]
    async fn prepare_socket_when_already_prepared_reports_the_same_address() {
        let state = StreamingState::new();

        let first = state.prepare_socket().await.unwrap();
        let second = state.prepare_socket().await.unwrap();

        assert_eq!(first, second);
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn merge_candidate_addrs_puts_the_lan_candidate_before_the_public_one() {
        let public: SocketAddr = "203.0.113.7:5000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.20:5000".parse().unwrap();

        // The caller (the frontend) ranks LAN before public.
        let merged = merge_candidate_addrs(public, &[lan, public]);

        assert_eq!(merged, vec![lan, public]);
    }

    /// Verifies: REQ-CON-113
    #[test]
    fn merge_candidate_addrs_when_no_other_candidates_falls_back_to_addr() {
        let public: SocketAddr = "203.0.113.7:5000".parse().unwrap();

        assert_eq!(merge_candidate_addrs(public, &[]), vec![public]);
    }

    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_is_zero_for_silence() {
        assert_eq!(rms_level(&[0.0; 128]), 0);
    }

    /// An empty frame must not divide by zero or report a stale level.
    ///
    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_is_zero_for_an_empty_frame() {
        assert_eq!(rms_level(&[]), 0);
    }

    /// The meter has to move with the signal - a constant would make it
    /// useless (.claude/rules/traceability.md, "指標は定数化させない").
    ///
    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_rises_with_amplitude() {
        let quiet = rms_level(&[0.1; 128]);
        let medium = rms_level(&[0.5; 128]);
        let loud = rms_level(&[1.0; 128]);

        assert_eq!(quiet, 10);
        assert_eq!(medium, 50);
        assert_eq!(loud, 100);
        assert!(quiet < medium && medium < loud);
    }

    /// RMS, not peak: a full-scale sine is 1/sqrt(2) of full scale.
    ///
    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_of_a_full_scale_sine_is_about_71() {
        let samples: Vec<f32> = (0..4800)
            .map(|i| (i as f32 / 48000.0 * 440.0 * std::f32::consts::TAU).sin())
            .collect();

        let level = rms_level(&samples);
        assert!(
            (69..=73).contains(&level),
            "expected about 71 for a full-scale sine, got {}",
            level
        );
    }

    /// Sign must not matter: the meter shows magnitude.
    ///
    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_ignores_polarity() {
        assert_eq!(rms_level(&[0.5; 64]), rms_level(&[-0.5; 64]));
    }

    /// Samples beyond full scale are clamped rather than reported above 100,
    /// which the meter's 0-100 range could not display.
    ///
    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_clamps_above_full_scale() {
        assert_eq!(rms_level(&[4.0; 64]), 100);
        assert_eq!(rms_level(&[-4.0; 64]), 100);
    }

    /// A non-finite sample must not surface as a nonsense level. `f32::NAN`
    /// would otherwise fall through `min` and round to 0 by accident rather
    /// than by decision.
    ///
    /// Verifies: REQ-AUD-029
    #[test]
    fn test_rms_level_rejects_non_finite_samples() {
        assert_eq!(rms_level(&[f32::NAN, 0.5, 0.5]), 0);
        assert_eq!(rms_level(&[f32::INFINITY, 0.5]), 0);
    }

    /// Switching preset changes the play-out delay, and zero-latency must
    /// resolve to no delay at all - the preset promises 0ms (ADR-008).
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn switching_to_zero_latency_selects_a_passthrough_delay() {
        let zero_latency = AudioPreset::ZeroLatency;
        let balanced = AudioPreset::Balanced;

        let config = PlayoutConfig::for_delay(64, zero_latency.jitter_buffer_frames());
        assert_eq!(config.target_delay_frames, 0);
        assert_eq!(
            config.min_delay_frames, 0,
            "passthrough must be allowed to stay at zero"
        );

        // Coming from balanced, the delay really does change.
        let previous = PlayoutConfig::for_delay(64, balanced.jitter_buffer_frames());
        assert_eq!(previous.target_delay_frames, 4);
        assert_ne!(config.target_delay_frames, previous.target_delay_frames);
    }

    /// A session with nobody to hear yet: the play-out buffer has nothing, and
    /// the output callback is the only thing that can put the monitored input
    /// on the speakers.
    fn monitored_output(monitoring: bool) -> Vec<f32> {
        const FRAME: u32 = 32;
        let receive = ReceivePath::new(
            AudioPreset::ZeroLatency.codec_type(),
            48_000,
            FRAME,
            AudioPreset::ZeroLatency.jitter_buffer_frames(),
        )
        .expect("a PCM receive path");
        let monitor = LocalMonitor::new(FRAME);
        let mut tap = monitor.tap();
        monitor.set_enabled(monitoring);
        for _ in 0..3 {
            tap.push(&[0.25; FRAME as usize]);
        }

        let mut source = playout_source(
            receive.clone(),
            Arc::new(AtomicU32::new(100)),
            Arc::new(AtomicU32::new(100)),
            Arc::new(std::sync::atomic::AtomicI32::new(0)),
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU64::new(0)),
            monitor,
        );
        let mut out = vec![0.0f32; receive.frame_samples() * 3];
        let samples = source(&mut out);
        assert_eq!(samples, receive.frame_samples(), "one nominal frame");
        out.truncate(samples);
        out
    }

    /// Verifies: REQ-AUD-111
    #[test]
    fn test_monitored_input_is_played_before_any_peer_has_sent_audio() {
        let out = monitored_output(true);

        assert!(
            out.iter().all(|s| (*s - 0.25).abs() < 1e-6),
            "the input is mixed into both channels: {out:?}"
        );
    }

    /// Verifies: REQ-AUD-112
    #[test]
    fn test_output_is_silent_with_monitoring_off_and_no_peer_audio() {
        let out = monitored_output(false);

        assert!(out.iter().all(|s| *s == 0.0), "{out:?}");
    }

    /// Every preset must map to a play-out delay that matches what it
    /// promises, so a switch cannot silently land on a different latency.
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn every_preset_maps_to_a_matching_delay() {
        for preset in AudioPreset::all() {
            let config = PlayoutConfig::for_delay(64, preset.jitter_buffer_frames());

            assert_eq!(
                config.target_delay_frames,
                preset.jitter_buffer_frames(),
                "{} play-out delay",
                preset.name()
            );
            assert!(
                config.max_delay_frames >= config.target_delay_frames,
                "{} must be allowed to hold the delay it asks for",
                preset.name()
            );
        }
    }
}
