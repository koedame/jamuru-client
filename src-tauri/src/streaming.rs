//! Audio streaming IPC commands for Tauri
//!
//! Manages P2P audio streaming with a dedicated audio thread that also
//! carries the network. Opening a device, which a hung driver can make take
//! as long as it likes, is done off that thread (`audio_slot`).
//!
//! Performance optimizations for 32-sample buffers:
//! - Uses rtrb (real-time safe ring buffer) instead of tokio channels
//! - Zero allocation in audio callbacks
//! - Pre-allocated stereo conversion buffer

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, RwLock};
use std::thread;

use rtrb::{Consumer, RingBuffer};
use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use jamjam::audio::{
    capture_attempts, capture_to_wire, rms_level, AudioConfig, AudioEngine, AudioError,
    AudioPreset, DeviceId, FlightKind, FlightRecorder, GainRamp, LocalMonitor, MonitorTap,
    OutputRoute, PeerGain, PeerMeter, PeerMix, PeerRateChange, ReceivePath, RecordingFeed,
    RecordingTap, Verdict, VoiceHandle, ADAPT_INTERVAL, WIRE_CHANNELS,
};
#[cfg(target_os = "windows")]
use jamjam::audio::{driver_name_of, AsioDuplex, OPEN_TIMEOUT};
use jamjam::network::{
    audio_packet_duration_ms, required_bps, status_label, AudioEncodingConfig, BandwidthEstimator,
    BandwidthStatus, BandwidthVerdict, Connection, ConnectionState, ConnectionStats,
    LatencyBreakdown, LinkFacts, LinkHealth, LinkIdentity, LinkSnapshot, LocalLatencyInfo,
    PeerLatencyInfo, QualityMonitor, UdpMux, MAX_PEERS_PER_ROOM,
};
use jamjam::protocol::LatencyInfoMessage;

#[cfg(target_os = "windows")]
use crate::audio_slot::AsioSwitch;
use crate::audio_slot::DeviceSlot;
use crate::settings::{pair_in_use, Devices};

/// How many audio links a session holds at most: one to each of the others in
/// a full room.
const MAX_LINKS: usize = MAX_PEERS_PER_ROOM - 1;

/// What the audio thread publishes about each link, by the participant the
/// link is to, in the order the links were opened
type LinkReports = RwLock<Vec<(Uuid, Arc<LinkReport>)>>;

/// What went wrong with the audio received on each link and when, by the
/// participant the link is to
type Flights = RwLock<Vec<(Uuid, Arc<FlightRecorder>)>>;

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
            tracing::info!("Audio thread: macOS real-time priority enabled (TIME_CONSTRAINT)");
        } else {
            // Fall back to nice value
            libc::setpriority(libc::PRIO_PROCESS, 0, -20);
            tracing::info!(
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
            tracing::info!("Audio thread: using nice -20 (SCHED_FIFO requires rtprio config)");
        } else {
            tracing::info!(
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
            tracing::info!("Audio thread: real-time priority enabled (HIGH + TIME_CRITICAL)");
        } else {
            tracing::warn!("Audio thread: failed to set thread priority");
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn set_current_thread_realtime_priority() {
    tracing::info!("Audio thread: real-time priority not supported on this platform");
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
    /// The audio links of the running session, in the order they were opened,
    /// with what the audio thread publishes about each
    links: Arc<LinkReports>,
    /// Where the fader and pan of each participant stand, whether or not
    /// there is a link to them yet. The output callback reads them as it mixes.
    peer_gains: Arc<RwLock<HashMap<Uuid, Arc<PeerGain>>>>,
    /// Devices the audio thread could not open, one per direction at most
    device_problems: Arc<RwLock<Vec<DeviceProblem>>>,
    /// Current buffer size (frame_size) for latency calculations
    buffer_size: Mutex<u32>,
    /// Buffer underrun count (audio glitches due to CPU/scheduling)
    underrun_count: Arc<AtomicU64>,
    /// How many times adaptation has moved the play-out delay this session
    delay_adjustments: Arc<AtomicU64>,
    /// How many times a link dropped and began to reconnect, over every link
    reconnect_count: Arc<AtomicU64>,
    /// How many times an established link was given up for silence, over
    /// every link
    silence_giveups: Arc<AtomicU64>,
    /// How the session's first link came up, once the audio thread has one
    link_facts: Arc<RwLock<Option<Arc<LinkFacts>>>>,
    /// What went wrong with the audio received on each link and when, by the
    /// participant the link is to. The last session's stay until the next one
    /// starts, so a debug call can still read them.
    flights: Arc<Flights>,
    /// Master output volume (0-200, 100 = unity gain)
    master_volume: Arc<AtomicU32>,
    /// Local (microphone) input volume (0-200, 100 = unity gain)
    local_volume: Arc<AtomicU32>,
    /// Local (microphone) input pan (-100 = full left, 0 = center, 100 = full right)
    local_pan: Arc<std::sync::atomic::AtomicI32>,
    /// Current sample rate in Hz (ADR-013: user-selectable)
    sample_rate: Arc<AtomicU32>,
    /// UDP socket bound ahead of streaming so its port can be advertised to
    /// peers before any audio flows (ADR-026).
    ///
    /// Held as `std::net::UdpSocket`, not a `UdpMux`: the audio thread
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
            links: Arc::new(RwLock::new(Vec::new())),
            peer_gains: Arc::new(RwLock::new(HashMap::new())),
            device_problems: Arc::new(RwLock::new(Vec::new())),
            buffer_size: Mutex::new(jamjam::config::DEFAULT_BUFFER_SIZE),
            underrun_count: Arc::new(AtomicU64::new(0)),
            delay_adjustments: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            silence_giveups: Arc::new(AtomicU64::new(0)),
            link_facts: Arc::new(RwLock::new(None)),
            flights: Arc::new(RwLock::new(Vec::new())),
            master_volume: Arc::new(AtomicU32::new(100)), // 100 = unity gain
            local_volume: Arc::new(AtomicU32::new(100)),  // 100 = unity gain
            local_pan: Arc::new(std::sync::atomic::AtomicI32::new(0)), // 0 = center
            // Until a session starts from the saved settings (ADR-013)
            sample_rate: Arc::new(AtomicU32::new(jamjam::config::DEFAULT_SAMPLE_RATE)),
            prepared_socket: Mutex::new(None),
            prepared_addr: Mutex::new(None),
        }
    }

    /// The fader and pan of `peer`, made at unity gain and centre the first
    /// time they are asked for.
    fn gain_of(&self, peer: Uuid) -> Arc<PeerGain> {
        let mut gains = self.peer_gains.write().unwrap_or_else(|e| e.into_inner());
        gains.entry(peer).or_default().clone()
    }

    /// Sets the volume `peer` is played at, in percent of unity gain (0-200).
    /// Heard on the next frame when there is a link to them, and kept for when
    /// there is.
    pub fn set_peer_volume(&self, peer: Uuid, percent: u32) {
        self.gain_of(peer).set_volume(percent);
    }

    /// Sets where `peer` sits, from -100 (left) to 100 (right).
    pub fn set_peer_pan(&self, peer: Uuid, pan: i32) {
        self.gain_of(peer).set_pan(pan);
    }

    /// Forgets the fader and pan of everyone but `peers`, so one who left
    /// leaves nothing behind.
    pub fn keep_peer_gains(&self, peers: &[Uuid]) {
        self.peer_gains
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|peer, _| peers.contains(peer));
    }

    /// Sets the volume of the microphone, in percent of unity gain (0-200).
    pub async fn set_local_volume(&self, percent: u32) {
        let clamped = percent.min(200);
        self.local_volume.store(clamped, Ordering::SeqCst);
        self.send(StreamingCommand::SetLocalVolume(clamped as f32 / 100.0))
            .await;
    }

    /// Sets where the microphone sits, from -100 (left) to 100 (right).
    pub async fn set_local_pan(&self, pan: i32) {
        let clamped = pan.clamp(-100, 100);
        self.local_pan.store(clamped, Ordering::SeqCst);
        self.send(StreamingCommand::SetLocalPan(clamped)).await;
    }

    /// The volume (percent of unity gain) and pan `peer` is played at, or
    /// `None` when nothing is kept for them.
    #[cfg(test)]
    pub(crate) fn peer_gain(&self, peer: Uuid) -> Option<(u32, i32)> {
        let gains = self.peer_gains.read().unwrap();
        gains.get(&peer).map(|gain| (gain.volume(), gain.pan()))
    }

    /// The microphone's volume (percent of unity gain) and pan, as stored.
    #[cfg(test)]
    pub(crate) fn local_gain(&self) -> (u32, i32) {
        (
            self.local_volume.load(Ordering::SeqCst),
            self.local_pan.load(Ordering::SeqCst),
        )
    }

    /// Tells the audio thread, if a session is running; a value set before one
    /// starts is picked up from the stored one.
    async fn send(&self, command: StreamingCommand) {
        if self.is_active.load(Ordering::SeqCst) {
            if let Some(sender) = self.cmd_tx.lock().await.as_ref() {
                let _ = sender.send(command);
            }
        }
    }

    /// Bind the audio socket, or report the address of the one already bound.
    async fn prepare_socket(&self) -> Result<SocketAddr, String> {
        let mut addr_lock = self.prepared_addr.lock().await;
        if let Some(addr) = *addr_lock {
            return Ok(addr);
        }

        // Port 0 lets the OS choose; the bound address is what we advertise.
        self.bind_prepared(&mut addr_lock, "0.0.0.0:0").await
    }

    async fn bind_prepared(
        &self,
        addr_lock: &mut Option<SocketAddr>,
        bind: &str,
    ) -> Result<SocketAddr, String> {
        let socket = jamjam::network::bind_std(bind)
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
    /// session, so the next prepare (after this session ended) has to bind a
    /// fresh one rather than report a port nothing listens on.
    async fn take_prepared_socket(&self) -> Option<std::net::UdpSocket> {
        let mut addr_lock = self.prepared_addr.lock().await;
        let socket = self.prepared_socket.lock().await.take();
        *addr_lock = None;
        socket
    }

    /// Whether an audio session is running
    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::SeqCst)
    }

    /// Whether a socket is bound and waiting for the next audio session: the
    /// one whose address the room was told.
    pub async fn has_prepared_socket(&self) -> bool {
        self.prepared_addr.lock().await.is_some()
    }

    fn link_reports(&self) -> Vec<(Uuid, Arc<LinkReport>)> {
        self.links
            .read()
            .map(|links| links.clone())
            .unwrap_or_default()
    }

    /// How the audio link to each participant is doing, in the order the
    /// links were opened.
    pub fn link_healths(&self) -> Vec<(Uuid, LinkHealth)> {
        self.link_reports()
            .into_iter()
            .map(|(peer, report)| (peer, report.health()))
            .collect()
    }

    /// Address candidates for the prepared audio socket, with the public
    /// address STUN reports for that socket itself.
    ///
    /// Returns `None` if there is no prepared socket to ask through - none was
    /// prepared, or the audio session already took it.
    ///
    /// The socket stays locked while STUN runs, so starting the session waits
    /// for the answer instead of moving the socket into the receive loop,
    /// which would swallow the reply.
    pub async fn gather_candidates(&self) -> Option<Vec<jamjam::network::AddressCandidate>> {
        let prepared = self.prepared_socket.lock().await;
        jamjam::network::gather_candidates_on(prepared.as_ref()?).await
    }

    /// Local latency info based on the audio config, for the link whose
    /// play-out buffer holds `jitter_buffer_delay_us`
    fn local_latency_info(&self, jitter_buffer_delay_us: u32) -> LocalLatencyInfo {
        let frame_size = self
            .buffer_size
            .try_lock()
            .map(|s| *s)
            .unwrap_or(jamjam::config::DEFAULT_BUFFER_SIZE);
        let sample_rate = self.sample_rate.load(Ordering::SeqCst);
        let mut info = LocalLatencyInfo::from_audio_config(frame_size, sample_rate, "pcm");

        // `from_audio_config` leaves the jitter buffer at 0 because it only knows
        // the audio configuration. Without this the breakdown shown to the user
        // omits the buffer entirely - up to 21.33ms on high-quality (ADR-019, ADR-070).
        info.set_jitter_buffer_ms(jitter_buffer_delay_us as f32 / 1000.0);
        info
    }
}

impl Default for StreamingState {
    fn default() -> Self {
        Self::new()
    }
}

/// An audio setting a running session can take on without reconnecting
/// ([`StreamingState::apply_setting`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSetting {
    InputDevice(Option<String>),
    OutputDevice(Option<String>),
    /// Input device channels to capture from (1-based)
    InputChannels(u32, Option<u32>),
    /// Output device channels to play on (1-based)
    OutputChannels(u32, Option<u32>),
    /// 1 (mono) or 2 (stereo)
    TransmitChannels(u32),
    /// Receive jitter buffer depth (ADR-020)
    JitterBufferFrames(u32),
}

/// Commands sent to the audio thread
#[derive(Debug, PartialEq)]
pub(crate) enum StreamingCommand {
    Stop,
    SetInputDevice(Option<String>),
    /// Capture 1 (mono) or 2 (stereo) channels from now on
    SetTransmitChannels(u32),
    /// Capture from these input device channels (1-based settings) from now on
    SetInputChannels(u32, Option<u32>),
    /// Play on these output device channels (1-based settings) from now on
    SetOutputChannels(u32, Option<u32>),
    SetOutputDevice(Option<String>),
    SetMute(bool),
    SetMonitoring(bool),
    SetMasterVolume(f32),
    SetLocalVolume(f32),
    SetLocalPan(i32),
    /// Rebuild the receive jitter buffer at a new depth (ADR-020)
    SetJitterBufferFrames(u32),
    /// Resume probing on every link automatic recovery gave up on (REQ-CON-110)
    Reconnect,
    /// Open an audio link to a participant
    Link(Box<LinkTarget>),
    /// Close the audio link to a participant. The others go on.
    Unlink(Uuid),
}

impl StreamingState {
    /// The latest reading of every link that is up, for the usage totals:
    /// the round-trip time (`None` before the first sample), the packet loss
    /// rate (0.0 to 1.0) and the packets FEC has rebuilt so far (`None` when
    /// the link sends no FEC).
    pub(crate) fn link_readings(&self) -> Vec<(Option<f32>, f32, Option<u64>)> {
        self.link_reports()
            .iter()
            .filter_map(|(_, report)| report.stats.read().ok()?.clone())
            .map(|s| (s.rtt_ms, s.packet_loss_rate, s.fec_recovered))
            .collect()
    }

    /// Buffer underruns since streaming last started.
    pub(crate) fn underruns(&self) -> u64 {
        self.underrun_count.load(Ordering::Relaxed)
    }

    /// Times a link began to reconnect since streaming last started.
    pub(crate) fn reconnects(&self) -> u64 {
        self.reconnect_count.load(Ordering::Relaxed)
    }

    /// Times an established link was given up for silence since streaming
    /// last started.
    pub(crate) fn silence_giveups(&self) -> u64 {
        self.silence_giveups.load(Ordering::Relaxed)
    }

    /// How the session's first link came up. Empty before there is one.
    pub(crate) fn link_snapshot(&self) -> LinkSnapshot {
        self.link_facts
            .read()
            .ok()
            .and_then(|facts| facts.as_ref().map(|facts| facts.snapshot()))
            .unwrap_or_default()
    }

    /// What went wrong with the audio received from each participant in this
    /// session, or the last one, and when, in the order the links were
    /// opened. Empty before any session has opened a link.
    #[cfg(feature = "debug-tools")]
    pub(crate) fn flight_reports(&self) -> Vec<(Uuid, jamjam::audio::FlightReport)> {
        self.flights
            .read()
            .map(|flights| {
                flights
                    .iter()
                    .map(|(peer, flight)| (*peer, flight.report()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stands in for the audio thread: a link to `peer` in `state`
    /// ("connected", "reconnecting", ...) that has received `packets_received`
    /// packets and measured a round trip of `rtt_ms`. With `None`, a link
    /// that is not up yet and has no statistics.
    #[cfg(test)]
    pub(crate) fn set_link_for_test(&self, peer: Uuid, state: &str, up: Option<(u64, f32)>) {
        let report = Arc::new(LinkReport::default());
        report.set_state(state, None);
        if let Some((packets_received, rtt_ms)) = up {
            *report.stats.write().unwrap() = Some(ConnectionStats {
                packets_received,
                rtt_ms: Some(rtt_ms),
                ..Default::default()
            });
        }
        let mut links = self.links.write().unwrap();
        links.retain(|(linked, _)| *linked != peer);
        links.push((peer, report));
    }

    /// Stands in for the audio thread: a link that came up as `facts` says.
    #[cfg(test)]
    pub(crate) fn set_link_facts_for_test(&self, facts: Arc<LinkFacts>) {
        *self.link_facts.write().unwrap() = Some(facts);
    }

    /// Stands in for the audio thread giving up an established link.
    #[cfg(test)]
    pub(crate) fn add_silence_giveup_for_test(&self) {
        self.silence_giveups.fetch_add(1, Ordering::Relaxed);
    }

    /// Stands in for a running session: what it is told arrives on the
    /// returned receiver instead of an audio thread.
    #[cfg(test)]
    pub(crate) async fn attach_session_for_test(&self) -> std_mpsc::Receiver<StreamingCommand> {
        let rx = self.detached_session_for_test().await;
        self.is_active.store(true, Ordering::SeqCst);
        rx
    }

    /// A receiver for commands while no session is running, to show that
    /// nothing is sent to one.
    #[cfg(test)]
    pub(crate) async fn detached_session_for_test(&self) -> std_mpsc::Receiver<StreamingCommand> {
        let (tx, rx) = std_mpsc::channel();
        *self.cmd_tx.lock().await = Some(tx);
        rx
    }

    /// Tells a running session about a changed audio setting, so it follows
    /// the change straight away. A no-op when no session is active: the next
    /// session starts from the saved settings.
    ///
    /// Only what can change live is here. The frame size is fixed for the
    /// lifetime of the audio engines, so a new buffer size (or a preset's new
    /// frame size) takes effect on the next connect (REQ-LAT-106).
    pub async fn apply_setting(&self, setting: SessionSetting) {
        if !self.is_active.load(Ordering::SeqCst) {
            return;
        }
        let command = match setting {
            SessionSetting::InputDevice(id) => StreamingCommand::SetInputDevice(id),
            SessionSetting::OutputDevice(id) => StreamingCommand::SetOutputDevice(id),
            SessionSetting::InputChannels(left, right) => {
                StreamingCommand::SetInputChannels(left, right)
            }
            SessionSetting::OutputChannels(left, right) => {
                StreamingCommand::SetOutputChannels(left, right)
            }
            SessionSetting::TransmitChannels(count) => StreamingCommand::SetTransmitChannels(count),
            SessionSetting::JitterBufferFrames(frames) => {
                StreamingCommand::SetJitterBufferFrames(frames)
            }
        };
        let tx = self.cmd_tx.lock().await;
        if let Some(ref sender) = *tx {
            let _ = sender.send(command);
        }
    }
}

/// Connection state and, when it failed, why
///
/// Published by the audio thread and read by the IPC layer.
type ConnectionStateSnapshot = Option<(String, Option<String>)>;

/// A participant to open an audio link to
#[derive(Debug, Clone)]
pub struct LinkTarget {
    pub peer: Uuid,
    /// Their addresses, best first. All are raced, so one on the same network
    /// is reached directly instead of only through its public address
    /// (REQ-CON-113).
    pub candidates: Vec<SocketAddr>,
    /// Our key in the room and the key the server gave for them: with them
    /// the link is checked to be theirs (`Connection::verify_peer`). `None`
    /// for a peer reached without a room.
    pub link: Option<(LinkIdentity, Option<String>)>,
}

impl PartialEq for LinkTarget {
    fn eq(&self, other: &Self) -> bool {
        let keys = |target: &Self| {
            target
                .link
                .as_ref()
                .map(|(ours, theirs)| (ours.public_key(), theirs.clone()))
        };
        self.peer == other.peer && self.candidates == other.candidates && keys(self) == keys(other)
    }
}

/// What the audio thread publishes about the link to one participant, for
/// the IPC layer to read
#[derive(Default)]
struct LinkReport {
    /// Connection statistics, once the link is up
    stats: RwLock<Option<ConnectionStats>>,
    /// Measured bandwidth against what the preset needs
    bandwidth: RwLock<Option<BandwidthSnapshot>>,
    /// Connection state (ADR-022)
    connection_state: RwLock<ConnectionStateSnapshot>,
    /// Delay their play-out buffer holds, in microseconds
    ///
    /// Integer microseconds rather than a float, so it fits an atomic without
    /// bit-casting. Feeds the latency breakdown shown to the user.
    jitter_buffer_delay_us: AtomicU32,
    /// What their app told about its audio (`LatencyInfoMessage`)
    latency_info: RwLock<Option<PeerLatencyInfo>>,
    /// What the mix measured of them
    meter: Arc<PeerMeter>,
}

impl LinkReport {
    fn state(&self) -> ConnectionStateSnapshot {
        self.connection_state.read().ok().and_then(|c| c.clone())
    }

    fn set_state(&self, state: &str, error: Option<String>) {
        if let Ok(mut published) = self.connection_state.write() {
            *published = Some((state.to_string(), error));
        }
    }

    fn health(&self) -> LinkHealth {
        let heard = self
            .stats
            .read()
            .ok()
            .and_then(|stats| stats.as_ref().map(|s| s.packets_received > 0))
            .unwrap_or(false);
        match self.state().as_ref().map(|(state, _)| state.as_str()) {
            Some("failed") => LinkHealth::Failed,
            Some("reconnecting") => LinkHealth::Silent,
            Some("connected") if heard => LinkHealth::Heard,
            _ => LinkHealth::Starting,
        }
    }
}

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
    /// Whether the audio to this peer is encrypted: "encrypted", "negotiating" while the
    /// keys are being agreed, or "unencrypted" when the peer's app cannot encrypt
    pub encryption: String,
    /// Whether the keys of the link are bound to the peer's key from the server, so the peer is
    /// known to be who the server said. False for a peer whose app predates the check.
    pub peer_checked: bool,
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

/// Which way audio goes through a device
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSide {
    Input,
    Output,
}

/// Why a device is not in use
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTrouble {
    /// The driver did not answer in time (REQ-AUD-123)
    Unresponsive,
    /// The device refused to open
    Failed,
    /// The device does not open at the session's sample rate, and no rate it
    /// offers helped. Its format can be changed in the system's sound settings.
    UnsupportedFormat,
    /// The stream is open but its callback is no longer called, and opening it
    /// again did not bring it back (REQ-AUD-128)
    Stalled,
}

/// A device the audio thread could not open, for the UI to tell the user
///
/// Published by the audio thread and read by the IPC layer. Cleared when the
/// device of that side opens, and when a session starts or ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceProblem {
    pub side: DeviceSide,
    pub trouble: DeviceTrouble,
    /// The device's name, when the system's list has one for it
    pub device: Option<String>,
    /// The sample rate the session asked the device to open at
    pub sample_rate: u32,
}

/// Records that the `side` device could not be opened, replacing what was
/// recorded for that side
fn note_device_problem(
    problems: &RwLock<Vec<DeviceProblem>>,
    side: DeviceSide,
    device: Option<&DeviceId>,
    sample_rate: u32,
    error: &AudioError,
) {
    let trouble = match error {
        AudioError::DeviceUnresponsive(_) => DeviceTrouble::Unresponsive,
        AudioError::UnsupportedConfig(_) => DeviceTrouble::UnsupportedFormat,
        _ => DeviceTrouble::Failed,
    };
    note_trouble(problems, side, device, sample_rate, trouble);
}

/// Records `trouble` with the `side` device, replacing what was recorded for
/// that side
fn note_trouble(
    problems: &RwLock<Vec<DeviceProblem>>,
    side: DeviceSide,
    device: Option<&DeviceId>,
    sample_rate: u32,
    trouble: DeviceTrouble,
) {
    let listed = match side {
        DeviceSide::Input => crate::audio::input_devices(),
        DeviceSide::Output => crate::audio::output_devices(),
    }
    .unwrap_or_default();
    let name = device
        .and_then(|id| listed.iter().find(|d| d.id == id.0))
        .or_else(|| {
            listed
                .iter()
                .find(|d| d.is_default)
                .filter(|_| device.is_none())
        })
        .map(|d| d.name.clone());
    if let Ok(mut problems) = problems.write() {
        problems.retain(|p| p.side != side);
        problems.push(DeviceProblem {
            side,
            trouble,
            device: name,
            sample_rate,
        });
    }
}

fn clear_device_problem(problems: &RwLock<Vec<DeviceProblem>>, side: DeviceSide) {
    if let Ok(mut problems) = problems.write() {
        problems.retain(|p| p.side != side);
    }
}

/// The audio link to one participant, for IPC
#[derive(Debug, Clone, Serialize)]
pub struct PeerLinkStatus {
    /// The participant's id in the room
    pub peer_id: String,
    /// Connection state: "connecting" | "checkingconnectivity" | "connected" |
    /// "reconnecting" | "failed" (ADR-022)
    pub connection_state: Option<String>,
    /// Why the link failed, when `connection_state` is "failed" and the
    /// reason is known
    pub connection_error: Option<String>,
    /// Network statistics, once the link is up
    pub network: Option<NetworkStats>,
    /// Detailed latency breakdown, once the link is up
    pub latency: Option<DetailedLatency>,
    /// Their audio configuration (if received via LatencyInfoMessage)
    pub peer_audio: Option<PeerAudioInfo>,
    /// Level of what is played of them, after their fader (0-100)
    pub level: u32,
}

/// Streaming status for IPC
///
/// `network`, `latency`, `peer_audio`, `connection_state` and
/// `connection_error` are those of one link: the one doing best (connected
/// before reconnecting before failed), the earliest opened among equals. In a
/// room of two that is the link to the other participant, which is what a
/// reader written for such a room expects. `peers` has every link.
#[derive(Debug, Clone, Serialize)]
pub struct StreamingStatus {
    pub is_active: bool,
    /// The audio link to each participant there is one to, in the order the
    /// links were opened
    pub peers: Vec<PeerLinkStatus>,
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
    pub connection_state: Option<String>,
    /// Why the connection failed, when `connection_state` is "failed"
    pub connection_error: Option<String>,
    /// Devices that could not be opened. Read whether or not a session is
    /// active: one that could not open its device never became active.
    pub device_problems: Vec<DeviceProblem>,
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
pub async fn streaming_prepare(state: tauri::State<'_, StreamingState>) -> Result<String, String> {
    state.prepare_socket().await.map(|addr| addr.to_string())
}

/// Opens an audio link to `target`, starting the audio session - the devices
/// and the socket the room was told - if none is running.
///
/// The link is set up on the audio thread: this returns once it has been
/// asked for, and [`StreamingState::link_healths`] says how it went.
pub async fn streaming_link(
    target: LinkTarget,
    state: tauri::State<'_, StreamingState>,
    config_state: tauri::State<'_, crate::config::ConfigState>,
    settings_state: tauri::State<'_, crate::settings::SettingsState>,
    usage: tauri::State<'_, crate::usage::UsageState>,
) -> Result<(), String> {
    // An address published twice is checked once
    let mut target = target;
    let mut candidates: Vec<SocketAddr> = Vec::new();
    for candidate in target.candidates {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    target.candidates = candidates;
    if target.candidates.is_empty() {
        return Err("The participant has published no address".to_string());
    }
    if !state.is_active() {
        start_session(&state, &config_state, &settings_state, &usage).await?;
    }
    tracing::info!(
        "Audio link to {} asked for ({} address candidate(s))",
        target.peer,
        target.candidates.len()
    );
    let tx = state.cmd_tx.lock().await;
    match tx.as_ref() {
        Some(sender) => sender
            .send(StreamingCommand::Link(Box::new(target)))
            .map_err(|_| "The audio session has ended".to_string()),
        None => Err("No audio session is running".to_string()),
    }
}

/// Closes the audio link to `peer`. The session and its other links go on.
pub async fn streaming_unlink(
    peer: Uuid,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    if !state.is_active() {
        return Ok(());
    }
    tracing::info!("Audio link to {} to be closed", peer);
    if let Some(sender) = state.cmd_tx.lock().await.as_ref() {
        let _ = sender.send(StreamingCommand::Unlink(peer));
    }
    Ok(())
}

/// Starts the audio session: opens the devices from the saved settings and
/// takes the prepared socket. It has no links yet.
async fn start_session(
    state: &StreamingState,
    config_state: &crate::config::ConfigState,
    settings_state: &crate::settings::SettingsState,
    usage: &crate::usage::UsageState,
) -> Result<(), String> {
    // Hand the pre-bound socket to the audio thread, which registers it with
    // its own runtime. Taking it means a later prepare rebinds rather than
    // handing out a socket already in use.
    //
    // Everyone in the room sends to the address of this socket. A session
    // with a socket nobody was told of would send and hear no one, so there
    // is none without it.
    let prepared_socket = state
        .take_prepared_socket()
        .await
        .ok_or("No audio socket was prepared: the room has no address to send to")?;

    // Create channel for commands to audio thread
    let (cmd_tx, cmd_rx) = std_mpsc::channel::<StreamingCommand>();

    // Store command sender
    {
        let mut tx = state.cmd_tx.lock().await;
        *tx = Some(cmd_tx);
    }

    let is_active = state.is_active.clone();
    let is_muted = state.is_muted.clone();
    let is_monitoring = state.is_monitoring.clone();
    let input_level = state.input_level.clone();
    let output_level = state.output_level.clone();
    let shared_links = state.links.clone();
    let peer_gains = state.peer_gains.clone();
    let shared_device_problems = state.device_problems.clone();
    let underrun_count = state.underrun_count.clone();
    let delay_adjustments = state.delay_adjustments.clone();
    let reconnect_count = state.reconnect_count.clone();
    let silence_giveups = state.silence_giveups.clone();
    let shared_link_facts = state.link_facts.clone();
    let shared_flights = state.flights.clone();
    let master_volume = state.master_volume.clone();
    let local_volume = state.local_volume.clone();
    let local_pan = state.local_pan.clone();
    let usage_reporter = usage.reporter().clone();

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
    state.silence_giveups.store(0, Ordering::SeqCst);
    if let Ok(mut facts) = state.link_facts.write() {
        *facts = None;
    }
    if let Ok(mut flights) = state.flights.write() {
        flights.clear();
    }
    if let Ok(mut links) = state.links.write() {
        links.clear();
    }
    // The master volume goes back to unity gain. The faders' own values are
    // the mixer's, which sets them as they change (`mixer`).
    state.master_volume.store(100, Ordering::SeqCst);
    if let Ok(mut problems) = state.device_problems.write() {
        problems.clear();
    }

    // Read the settings and become active as one step, with changes held
    // off: a change made before this is in the config read here, and one made
    // after it finds the session active and is sent to it. Read any earlier -
    // before the prepared socket, which can wait on STUN - and a change made
    // meanwhile would be saved and shown but miss this session.
    let changes_held = settings_state.hold_changes().await;
    // The session runs with the audio settings as saved: the one store every
    // change goes through (ADR-043), so what the settings show is what the
    // session gets - the sample rate once never reached it, because the
    // caller had to pass it and did not.
    let config = config_state.get()?;
    let input_device_id = config.input_device_id.clone();
    let output_device_id = config.output_device_id.clone();
    let buffer_size = config.buffer_size;
    // User-selectable (ADR-013)
    let sample_rate = config.sample_rate;
    // Jitter buffer depth comes from the selected preset (ADR-019/ADR-020).
    // The frame *duration* comes from the buffer_size actually in use, so the
    // resulting delay is correct even if the two were configured separately.
    let preset = config.preset.clone();
    // Transmit channel count, told to the peers so their mixers can show
    // whether we're sending mono or stereo.
    let transmit_channels = config.transmit_channels;
    // Which channels of a multi-channel interface are read and played on
    let input_channels = (config.input_channel_l, config.input_channel_r);
    let output_channels = (config.output_channel_l, config.output_channel_r);

    let redact = crate::logging::redaction_enabled();
    tracing::info!(
        "Streaming start: input={:?} output={:?} buffer={} sample_rate={} preset={:?}",
        crate::logging::redact_device_id(
            input_device_id.as_deref().unwrap_or("system default"),
            redact
        ),
        crate::logging::redact_device_id(
            output_device_id.as_deref().unwrap_or("system default"),
            redact
        ),
        buffer_size,
        sample_rate,
        preset
    );

    // Store buffer size for latency display
    {
        let mut bs = state.buffer_size.lock().await;
        *bs = buffer_size;
    }

    // Store sample rate (ADR-013)
    state.sample_rate.store(sample_rate, Ordering::SeqCst);
    #[cfg(feature = "debug-tools")]
    crate::audio_tap::set_sample_rate(sample_rate);

    // Mark as active BEFORE spawning thread to avoid race condition
    state.is_active.store(true, Ordering::SeqCst);
    drop(changes_held);

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
                prepared_socket,
                input_device_id,
                output_device_id,
                buffer_size,
                preset,
                sample_rate,
                transmit_channels,
                input_channels,
                output_channels,
                cmd_rx,
                &is_active,
                &is_muted,
                &is_monitoring,
                &input_level,
                &output_level,
                &shared_links,
                &peer_gains,
                &shared_device_problems,
                &underrun_count,
                &delay_adjustments,
                &reconnect_count,
                &silence_giveups,
                &shared_link_facts,
                &shared_flights,
                &master_volume,
                &local_volume,
                &local_pan,
            )
            .await
            {
                tracing::error!("Audio streaming failed: {}", e);
                crate::usage::record_streaming_failure(&usage_reporter, &e);
            } else {
                tracing::info!("Audio streaming ended");
            }
            // The links ended with the session
            if let Ok(mut links) = shared_links.write() {
                links.clear();
            }
            is_active.store(false, Ordering::SeqCst);
        });
    });

    Ok(())
}

/// Stop audio streaming
pub async fn streaming_stop(state: tauri::State<'_, StreamingState>) -> Result<(), String> {
    if let Ok(mut problems) = state.device_problems.write() {
        problems.clear();
    }
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

    // The audio thread clears `is_active` as the last thing it does. Waiting
    // for that, not for a fixed time, is what keeps the end of this session
    // from clearing the flag of the one that starts right after it.
    let deadline = std::time::Instant::now() + STOP_WAIT_LIMIT;
    while state.is_active.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    if state.is_active.swap(false, Ordering::SeqCst) {
        tracing::warn!(
            "The audio thread did not end within {:?} of being told to stop",
            STOP_WAIT_LIMIT
        );
    }

    Ok(())
}

/// How long [`streaming_stop`] waits for the audio thread to end.
const STOP_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// How a link in `state` compares with others when one has to stand for the
/// session: lower is better.
fn state_rank(state: Option<&str>) -> u8 {
    match state {
        Some("connected") => 0,
        Some("reconnecting") => 1,
        Some("failed") => 3,
        // Not up yet
        _ => 2,
    }
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
    let frame_size = *state.buffer_size.lock().await;
    let local_sample_rate = state.sample_rate.load(Ordering::SeqCst);

    let peers: Vec<PeerLinkStatus> = state
        .link_reports()
        .iter()
        .map(|(peer, report)| {
            let stats = report.stats.read().ok().and_then(|s| s.clone());
            let bandwidth = report.bandwidth.read().ok().and_then(|b| *b);
            let connection_state = report.state();
            let (network, latency) = match &stats {
                Some(s) => {
                    let local_info = state
                        .local_latency_info(report.jitter_buffer_delay_us.load(Ordering::SeqCst));
                    (
                        Some(network_stats(s, bandwidth)),
                        Some(detailed_latency(
                            s,
                            &local_info,
                            frame_size,
                            local_sample_rate,
                        )),
                    )
                }
                None => (None, None),
            };
            // Peer audio info (ADR-013: sample rate communication)
            let peer_audio = report.latency_info.read().ok().and_then(|info| {
                info.as_ref().map(|p| PeerAudioInfo {
                    sample_rate: p.sample_rate,
                    frame_size: p.frame_size,
                    codec: p.codec.clone(),
                    needs_resampling: p.sample_rate != local_sample_rate,
                    channel_count: p.channel_count as u32,
                })
            });
            PeerLinkStatus {
                peer_id: peer.to_string(),
                connection_state: connection_state.as_ref().map(|(state, _)| state.clone()),
                connection_error: connection_state.and_then(|(_, error)| error),
                network,
                latency,
                peer_audio,
                level: report.meter.level(),
            }
        })
        .collect();

    // Audio quality metrics
    let audio_quality = if is_active {
        Some(AudioQuality {
            underrun_count: state.underrun_count.load(Ordering::Relaxed),
            delay_adjustments: state.delay_adjustments.load(Ordering::Relaxed),
        })
    } else {
        None
    };

    // `min_by_key` keeps the first of equals: the earliest opened
    let best = peers
        .iter()
        .min_by_key(|peer| state_rank(peer.connection_state.as_deref()))
        .cloned();

    Ok(StreamingStatus {
        is_active,
        is_muted,
        is_monitoring,
        input_level,
        output_level,
        network: best.as_ref().and_then(|peer| peer.network.clone()),
        latency: best.as_ref().and_then(|peer| peer.latency.clone()),
        audio_quality,
        peer_audio: best.as_ref().and_then(|peer| peer.peer_audio.clone()),
        connection_state: best.as_ref().and_then(|peer| peer.connection_state.clone()),
        connection_error: best.and_then(|peer| peer.connection_error),
        peers,
        device_problems: state
            .device_problems
            .read()
            .map(|problems| problems.clone())
            .unwrap_or_default(),
    })
}

fn network_stats(s: &ConnectionStats, bandwidth: Option<BandwidthSnapshot>) -> NetworkStats {
    NetworkStats {
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
        encryption: s.security.as_str().to_string(),
        peer_checked: s.peer_checked,
        bytes_sent: s.bytes_sent,
        bytes_received: s.bytes_received,
    }
}

fn detailed_latency(
    s: &ConnectionStats,
    local_info: &LocalLatencyInfo,
    frame_size: u32,
    sample_rate: u32,
) -> DetailedLatency {
    let breakdown =
        LatencyBreakdown::calculate(local_info, None, s.rtt_ms.unwrap_or(0.0), s.jitter_ms);

    let upstream = vec![
        LatencyComponent {
            name: "Capture buffer".to_string(),
            ms: breakdown.upstream.capture_buffer_ms,
            info: Some(format!("{} samples @ {} Hz", frame_size, sample_rate)),
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
            info: Some(format!("{} samples", frame_size)),
        },
    ];

    DetailedLatency {
        upstream,
        upstream_total_ms: breakdown.upstream_total_ms,
        downstream,
        downstream_total_ms: breakdown.downstream_total_ms,
        roundtrip_total_ms: breakdown.roundtrip_total_ms,
    }
}

/// Retry every link automatic recovery gave up on
///
/// Backs the "reconnect?" prompt the UI shows when a link's
/// `connection_state` reads "failed" (REQ-CON-110). A no-op when no session
/// is active.
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

/// The frame source the output callback pulls from.
///
/// Runs on the audio callback, so it allocates nothing and never waits: a
/// contended buffer costs one concealed frame rather than a stalled device.
/// The mixer gains are applied here rather than on the way in, so a volume or
/// pan change is heard on the very next frame.
fn playout_source(
    mix: PeerMix,
    master_volume: Arc<AtomicU32>,
    output_level: Arc<AtomicU32>,
    monitor: LocalMonitor,
    recording: Option<(RecordingTap, RecordingTap)>,
) -> impl FnMut(&mut [f32]) -> usize + Send + 'static {
    let mut recording = recording;
    // Everyone else as they sent it, before the listener's faders and pans:
    // what recording/streaming output `B` carries
    let mut dry = vec![0.0f32; mix.frame_samples()];
    move |out: &mut [f32]| {
        let (b_tap, c_tap) = match recording.as_mut() {
            Some((b, c)) => (Some(b), Some(c)),
            None => (None, None),
        };
        let samples = mix.mix_into(
            out,
            master_volume.load(Ordering::Relaxed),
            b_tap.is_some().then_some(&mut dry[..]),
        );
        if let Some(tap) = b_tap {
            tap.push_stereo(&dry[..samples]);
        }
        output_level.store(rms_level(&out[..samples]), Ordering::Relaxed);
        // Added after the peers, whatever they did this frame: hearing yourself
        // must not wait for, or depend on, anyone being connected (ADR-033).
        monitor.mix_into(&mut out[..samples]);
        // C is what is actually heard: post-fader, self included.
        if let Some(tap) = c_tap {
            tap.push_stereo(&out[..samples]);
        }
        #[cfg(feature = "debug-tools")]
        {
            use crate::audio_tap::{inject, observe, Point};
            inject(Point::Output, &mut out[..samples], WIRE_CHANNELS);
            observe(Point::Output, &out[..samples], WIRE_CHANNELS);
        }
        samples
    }
}

/// Starts playback on `device_id`, on the output channels `(left, right)` the
/// user selected (1-based). `make_source` builds the frame source, once per
/// attempt.
///
/// A device that has no such channels is played on its first ones
/// (`pair_in_use`), so a stale setting never leaves a session without sound.
/// The saved pair is not touched.
fn start_playout<S>(
    engine: &mut AudioEngine,
    device_id: Option<&DeviceId>,
    saved: (u32, Option<u32>),
    frame_samples: usize,
    make_source: impl Fn() -> S,
) -> Result<(), AudioError>
where
    S: FnMut(&mut [f32]) -> usize + Send + 'static,
{
    let chosen = device_id.map(|id| id.0.clone());
    let (left, right) = pair_in_use(&Devices::list().output, &chosen, saved);
    let selected = OutputRoute::from_settings(left, right);
    engine.set_playback_route(selected);
    match engine.start_playback_with_source(device_id, frame_samples, make_source()) {
        Err(e) if !selected.is_default() => {
            tracing::warn!(
                "Playback on {:?} to device channel(s) {:?} failed: {}",
                device_id,
                selected,
                e
            );
            engine.set_playback_route(OutputRoute::default());
            engine.start_playback_with_source(device_id, frame_samples, make_source())
        }
        result => result,
    }
}

/// Captured audio waiting to be sent, and how many interleaved channels each
/// frame of it has.
struct CaptureRing {
    consumer: Consumer<f32>,
    channels: usize,
}

impl CaptureRing {
    /// A ring nothing is ever written to, for a session whose input did not open
    fn silent(channels: usize) -> Self {
        let (_, consumer) = RingBuffer::<f32>::new(1);
        Self { consumer, channels }
    }
}

/// Throws away what is waiting in a capture ring.
fn discard_queued(consumer: &mut Consumer<f32>) {
    let queued = consumer.slots();
    if let Ok(chunk) = consumer.read_chunk(queued) {
        chunk.commit_all();
    }
}

/// Delivers one just-captured, already channel-picked frame: feeds the "hear
/// yourself" monitor, updates the input level meter, and queues it for the
/// network send thread. Shared between the two capture backends - `cpal`'s
/// ring-buffer callback and ASIO's `bufferSwitch` - which hand over samples
/// the same way (interleaved, already reduced to the picked channels).
#[allow(clippy::too_many_arguments)]
fn on_captured_frame(
    samples: &[f32],
    channels: usize,
    producer: &mut rtrb::Producer<f32>,
    monitor_tap: &mut MonitorTap,
    level: &AtomicU32,
    recording_a: Option<&mut RecordingTap>,
) {
    monitor_tap.push_interleaved(samples, channels);
    #[cfg(feature = "debug-tools")]
    crate::audio_tap::observe(crate::audio_tap::Point::Input, samples, channels);
    // A is dry: no fader/pan applied, whatever the transmit channel count.
    if let Some(tap) = recording_a {
        if channels >= 2 {
            tap.push_stereo(samples);
        } else {
            tap.push_mono(samples);
        }
    }
    // Calculate RMS level (0-100)
    if !samples.is_empty() {
        level.store(rms_level(samples), Ordering::SeqCst);
    }
    if let Ok(mut chunk) = producer.write_chunk_uninit(samples.len()) {
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
}

/// Starts capture on `device_id` with `wanted` channels (1 or 2), taken from
/// the input channels `(left, right)` the user selected (1-based), and returns
/// the ring the captured audio lands in.
///
/// A device that has no such channels is opened with its first ones instead
/// (`pair_in_use`; a mono microphone gets its one channel, which is captured
/// as mono), and one that will not open with two falls back to its first
/// channel, so a stale setting never leaves a session without input. The
/// saved pair is not touched. The ring reports how many channels it actually
/// has.
#[allow(clippy::too_many_arguments)]
fn start_capture_ring(
    engine: &mut AudioEngine,
    device_id: Option<&DeviceId>,
    wanted: u16,
    selected: (u32, Option<u32>),
    frame_size: usize,
    input_level: &Arc<AtomicU32>,
    monitor: &LocalMonitor,
    recording: Option<&RecordingFeed>,
) -> Result<CaptureRing, AudioError> {
    let chosen = device_id.map(|id| id.0.clone());
    let (left, right) = pair_in_use(&Devices::list().input, &chosen, selected);
    let mut failure = AudioError::DeviceNotFound("no way to open the input was tried".into());
    for picks in capture_attempts(left, right, wanted) {
        let channels = picks.len() as u16;
        let (mut producer, consumer) = RingBuffer::<f32>::new(32 * frame_size * channels as usize);
        let mut monitor_tap = monitor.tap();
        let mut recording_a = recording.map(|feed| feed.tap_a());
        let level = input_level.clone();

        engine.stop_capture();
        engine.set_capture_picks(picks.clone());
        // Zero-allocation: write directly to the rtrb producer (FnMut, no Sync needed)
        let started = engine.start_capture(device_id, move |samples, _timestamp| {
            on_captured_frame(
                samples,
                channels as usize,
                &mut producer,
                &mut monitor_tap,
                &level,
                recording_a.as_mut(),
            );
        });
        match started {
            Ok(()) => {
                tracing::info!(
                    "Capture started on {:?} from device channel(s) {:?}",
                    device_id,
                    picks
                );
                return Ok(CaptureRing {
                    consumer,
                    channels: channels as usize,
                });
            }
            Err(e) => {
                tracing::warn!(
                    "Capture on {:?} from device channel(s) {:?} failed: {}",
                    device_id,
                    picks,
                    e
                );
                // Another way of opening a driver that does not answer would
                // wait for the same silence again
                let unresponsive = matches!(e, AudioError::DeviceUnresponsive(_));
                failure = e;
                if unresponsive {
                    break;
                }
            }
        }
    }
    Err(failure)
}

/// The ring the session sends from: the one the input opened, or, when it
/// did not open, one that stays empty. What others say is still heard, so the
/// session goes on without sending, and the screen says why the input is not
/// in use.
fn capture_or_silence(
    opened: Result<CaptureRing, AudioError>,
    wanted_channels: u16,
    device_problems: &RwLock<Vec<DeviceProblem>>,
    device: Option<&DeviceId>,
    sample_rate: u32,
) -> CaptureRing {
    match opened {
        Ok(ring) => ring,
        Err(e) => {
            tracing::warn!("The session goes on without input: {}", e);
            note_device_problem(device_problems, DeviceSide::Input, device, sample_rate, &e);
            CaptureRing::silent(wanted_channels as usize)
        }
    }
}

/// The ASIO driver `input` and `output` should open as one session, if they
/// name the same one: ASIO has one channel set per direction under a single
/// driver, so an input and output that agree on the driver both mean that
/// driver, opened once, rather than two independent streams (see
/// `audio::asio`). `None` when either is unset, they differ, or neither
/// names an ASIO driver at all.
#[cfg(target_os = "windows")]
fn shared_asio_driver(input: Option<&DeviceId>, output: Option<&DeviceId>) -> Option<String> {
    let (input, output) = (input?, output?);
    if input != output {
        return None;
    }
    driver_name_of(input).map(str::to_string)
}

/// Opens `driver_name` for both directions at once: what `start_capture_ring`
/// and `start_playout` do together, through a single [`AsioDuplex`] session
/// instead of two `cpal` streams (Windows only, see `audio::asio`).
///
/// Picks and route come from the same settings the two-engine path uses
/// (`pair_in_use`), fit to the driver's reported channels the same way -
/// `asio::list_drivers` is what the settings screen's channel counts for an
/// ASIO device come from in the first place, so unlike `start_capture_ring`
/// this does not retry with narrower channels when they don't fit: a channel
/// setting that does not fit the driver's own reported channels is a stale
/// setting to report, not a device that might still open some other way.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn open_asio(
    driver_name: &str,
    input_id: Option<&DeviceId>,
    output_id: Option<&DeviceId>,
    sample_rate: u32,
    frame_size: u32,
    frame_samples: usize,
    wanted_channels: u16,
    input_channels: (u32, Option<u32>),
    output_channels: (u32, Option<u32>),
    input_level: Arc<AtomicU32>,
    monitor: LocalMonitor,
    mix: PeerMix,
    master_volume: Arc<AtomicU32>,
    output_level: Arc<AtomicU32>,
    recording: Option<&RecordingFeed>,
) -> Result<(AsioDuplex, CaptureRing), AudioError> {
    let chosen_in = input_id.map(|id| id.0.clone());
    let (in_left, in_right) = pair_in_use(&Devices::list().input, &chosen_in, input_channels);
    let picks = capture_attempts(in_left, in_right, wanted_channels)
        .into_iter()
        .next()
        .unwrap_or_default();

    let chosen_out = output_id.map(|id| id.0.clone());
    let (out_left, out_right) = pair_in_use(&Devices::list().output, &chosen_out, output_channels);
    let route = OutputRoute::from_settings(out_left, out_right);

    let channels = picks.len().max(1);
    let (mut producer, consumer) = RingBuffer::<f32>::new(32 * frame_size as usize * channels);
    let mut monitor_tap = monitor.tap();
    let mut recording_a = recording.map(|feed| feed.tap_a());
    let level = input_level;

    let session = AsioDuplex::start(
        driver_name,
        sample_rate,
        frame_size,
        frame_samples,
        OPEN_TIMEOUT,
        picks,
        route,
        move |samples, _timestamp| {
            on_captured_frame(
                samples,
                channels,
                &mut producer,
                &mut monitor_tap,
                &level,
                recording_a.as_mut(),
            );
        },
        playout_source(
            mix,
            master_volume,
            output_level,
            monitor,
            recording.map(|feed| (feed.tap_b(), feed.tap_c())),
        ),
    )?;

    Ok((session, CaptureRing { consumer, channels }))
}

/// The connections the send thread sends each captured frame to: those of the
/// links that are up. Replaced whole when a link comes up or goes, so the
/// send thread takes it without waiting for whoever changes it.
type Senders = Arc<RwLock<Arc<[Arc<tokio::sync::Mutex<Connection>>]>>>;

/// Where setting up the link to a participant has got
enum LinkStage {
    /// The connectivity check is running. It hands the connection over when
    /// it ends.
    Checking(tokio::task::JoinHandle<Result<Connection, String>>),
    Up(Arc<tokio::sync::Mutex<Connection>>),
    /// The link could not be set up
    Down,
}

/// The audio link to one participant, as the audio thread holds it
struct Link {
    peer: Uuid,
    report: Arc<LinkReport>,
    /// Their audio, from payload to the mix
    receive: ReceivePath,
    /// Their place in the mix: they are heard for as long as the link is held
    _voice: VoiceHandle,
    stage: LinkStage,
    /// Reports quality changes on the edge rather than on every stats poll
    quality_monitor: QualityMonitor,
    /// Measures what the link actually carries, so one too narrow for PCM can
    /// be reported instead of silently dropping audio (REQ-LAT-120's manual mode)
    bandwidth_estimator: BandwidthEstimator,
    bandwidth_verdict: BandwidthVerdict,
    last_bandwidth_status: Option<BandwidthStatus>,
    /// The play-out delay the display and the peer have been told about. The
    /// loop compares it with the buffer's own and reports the difference, so
    /// a preset switch, an adaptation and a reconnect all reach the display
    /// the same way (ADR-031).
    announced_delay_frames: u32,
    /// The channel count the peer has been told we send
    announced_channel_count: u8,
}

/// What every link of a session is set up with
struct LinkSetup<'a> {
    mux: &'a UdpMux,
    mix: &'a PeerMix,
    encoding: &'a AudioEncodingConfig,
    sample_rate: u32,
    buffer_size: u32,
    packet_duration_ms: f32,
    peer_gains: &'a RwLock<HashMap<Uuid, Arc<PeerGain>>>,
    reconnect_count: &'a Arc<AtomicU64>,
    silence_giveups: &'a Arc<AtomicU64>,
    shared_link_facts: &'a RwLock<Option<Arc<LinkFacts>>>,
    shared_flights: &'a Flights,
}

impl LinkSetup<'_> {
    /// Starts an audio link to `target`: their place in the mix, and the
    /// connectivity check, which runs on its own so that the others' audio
    /// and the session's commands do not wait for it.
    fn open(&self, target: LinkTarget, jitter_buffer_frames: u32) -> Result<Link, String> {
        let peer = target.peer;
        let mut connection = Connection::on(self.mux)
            .map_err(|e| format!("Failed to create the connection: {}", e))?;
        if let Some((ours, peer_key)) = &target.link {
            connection
                .verify_peer(ours, peer_key.as_deref())
                .map_err(|e| format!("Failed to set up the link to the peer: {}", e))?;
        }
        // The codec and FEC come from the preset (ADR-021)
        connection
            .set_audio_encoding(self.encoding.clone())
            .map_err(|e| format!("Failed to configure audio encoding: {}", e))?;

        // Receive path: jitter buffer keyed by sequence number (ADR-020).
        // Received audio waits in the play-out buffer, and only there, until
        // the output callback takes it (ADR-028). The CLI uses the same path.
        let receive = ReceivePath::new(
            self.encoding.codec_type,
            self.sample_rate,
            self.buffer_size,
            jitter_buffer_frames,
        )
        .map_err(|e| format!("Failed to create audio decoder: {}", e))?;
        if let Ok(mut flights) = self.shared_flights.write() {
            flights.push((peer, receive.flight().clone()));
        }

        let report = Arc::new(LinkReport::default());
        report.jitter_buffer_delay_us.store(
            (jitter_buffer_frames as f32 * self.packet_duration_ms * 1000.0) as u32,
            Ordering::SeqCst,
        );
        report.set_state("connecting", None);

        let gain = {
            let mut gains = self.peer_gains.write().unwrap_or_else(|e| e.into_inner());
            gains.entry(peer).or_default().clone()
        };
        let voice = self
            .mix
            .add(receive.clone(), gain, report.meter.clone())
            .ok_or_else(|| "The session has no room for another link".to_string())?;

        // Rebuild the jitter buffer when the link recovers: sequence numbers
        // carry on from before the outage, so stale packets would play out of
        // order (ADR-022).
        let receive_for_state = receive.clone();
        let report_for_state = report.clone();
        let reconnect_count = self.reconnect_count.clone();
        let silence_giveups = self.silence_giveups.clone();
        let link_facts = connection.link_facts();
        if let Ok(mut shared) = self.shared_link_facts.write() {
            shared.get_or_insert_with(|| link_facts.clone());
        }
        connection.set_state_change_callback(move |state| {
            tracing::info!("Audio connection to {} state: {:?}", peer, state);
            if state == ConnectionState::Connected {
                receive_for_state.reset();
            }
            if state == ConnectionState::Reconnecting {
                reconnect_count.fetch_add(1, Ordering::Relaxed);
            }
            // Failing to connect at all is reported by the connect call itself;
            // a failure after the link came up is the peer going silent.
            if state == ConnectionState::Failed && link_facts.snapshot().route.is_some() {
                silence_giveups.fetch_add(1, Ordering::Relaxed);
            }

            // Publish the transition so the UI can show reconnection progress
            // and prompt once recovery gives up (REQ-CON-110).
            report_for_state.set_state(&format!("{:?}", state).to_lowercase(), None);
        });

        // Runs on the network task, not the realtime audio callback, so
        // locking and taking ownership of the payload are both acceptable here.
        let receive_for_callback = receive.clone();
        connection.set_audio_callback(move |sequence, payload, _timestamp| {
            if !receive_for_callback.receive(sequence, &payload) {
                tracing::trace!("Play-out buffer refused frame {}", sequence);
            }
        });

        // Race all their candidates, so a peer on the same network is reached
        // directly rather than only through its public address (REQ-CON-113).
        let candidates = target.candidates;
        let checking = tokio::spawn(async move {
            connection
                .connect_with_candidates(&candidates)
                .await
                .map_err(|e| e.to_string())?;
            Ok(connection)
        });

        Ok(Link {
            peer,
            report,
            receive,
            _voice: voice,
            stage: LinkStage::Checking(checking),
            quality_monitor: QualityMonitor::new(),
            bandwidth_estimator: BandwidthEstimator::default(),
            bandwidth_verdict: BandwidthVerdict::default(),
            last_bandwidth_status: None,
            announced_delay_frames: jitter_buffer_frames,
            // Told when the link comes up
            announced_channel_count: 0,
        })
    }
}

/// How long one audio packet of a peer lasts, in ms: what the peer `told` of
/// its frame size and sample rate says, and until it has, as long as ours.
/// The play-out buffer counts its delay in the peer's packets.
fn peer_packet_ms(told: Option<&PeerLatencyInfo>, ours: f32) -> f32 {
    match told {
        Some(info) if info.frame_size > 0 && info.sample_rate > 0 => {
            audio_packet_duration_ms(info.frame_size, WIRE_CHANNELS, info.sample_rate)
        }
        _ => ours,
    }
}

impl Link {
    fn connection(&self) -> Option<&Arc<tokio::sync::Mutex<Connection>>> {
        match &self.stage {
            LinkStage::Up(connection) => Some(connection),
            _ => None,
        }
    }

    /// The delay their play-out buffer holds, in microseconds, where a packet
    /// of ours lasts `our_packet_ms`.
    fn delay_us(&self, our_packet_ms: f32) -> u32 {
        let told = self.report.latency_info.read().ok();
        let packet_ms = peer_packet_ms(told.as_ref().and_then(|info| info.as_ref()), our_packet_ms);
        (self.receive.delay_frames() as f32 * packet_ms * 1000.0) as u32
    }

    /// Ends the link: the check if it is still running, the connection if it
    /// is up.
    async fn close(self) {
        match self.stage {
            LinkStage::Checking(checking) => checking.abort(),
            LinkStage::Up(connection) => connection.lock().await.disconnect(),
            LinkStage::Down => {}
        }
    }
}

impl Link {
    /// Publishes the link's statistics and what the peer told about its
    /// audio, and follows the peer's sample rate and channels. Does nothing
    /// while the link is not up, or while its connection is busy sending.
    fn refresh(&mut self, preset: &AudioPreset, required_bps: f64, sample_rate: u32) {
        let Some(connection) = self.connection().cloned() else {
            return;
        };
        let Ok(conn) = connection.try_lock() else {
            return;
        };
        // The log keeps the start of an id, which tells the links apart
        let peer = self.peer;
        let conn_stats = conn.stats();

        // Log quality transitions once, on the edge (REQ-LAT-107). Held
        // back until the first RTT sample arrives, so a link nobody has
        // measured yet is never logged as "good" (REQ-LAT-130).
        if let Some(rtt_ms) = conn_stats.rtt_ms {
            if let Some(change) = self
                .quality_monitor
                .update(rtt_ms, conn_stats.packet_loss_rate)
            {
                if change.is_degradation() {
                    tracing::warn!(
                        "Connection quality to {} degraded to {} (RTT {:.1}ms, loss {:.2}%)",
                        peer,
                        change.to.as_str(),
                        rtt_ms,
                        conn_stats.packet_loss_rate * 100.0
                    );
                } else {
                    tracing::info!(
                        "Connection quality to {} is now {} (RTT {:.1}ms, loss {:.2}%)",
                        peer,
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
        self.bandwidth_estimator.note_audio(
            conn_stats.audio_packets_received,
            conn_stats.audio_packets_lost,
        );
        if self
            .bandwidth_estimator
            .sample(conn_stats.wire_bytes_received())
            .is_some()
        {
            let raw_status = self.bandwidth_estimator.status();
            let status = self.bandwidth_verdict.confirm(raw_status);
            let measured_bps = self.bandwidth_estimator.current_bps();
            if let Ok(mut snapshot) = self.report.bandwidth.write() {
                *snapshot = Some(BandwidthSnapshot {
                    measured_bps,
                    required_bps,
                    status,
                });
            }

            if status != self.last_bandwidth_status {
                match status {
                    Some(BandwidthStatus::Insufficient) => tracing::warn!(
                        "Link drops the audio of {}: {:.0} kbps arrive where {} needs {:.0} kbps",
                        peer,
                        measured_bps / 1000.0,
                        preset.name(),
                        required_bps / 1000.0
                    ),
                    Some(BandwidthStatus::Marginal) => tracing::warn!(
                        "Link loses some of the audio packets of {} ({:.0} kbps arrive)",
                        peer,
                        measured_bps / 1000.0
                    ),
                    Some(BandwidthStatus::Sufficient) => tracing::info!(
                        "Link bandwidth to {} is sufficient for {}",
                        peer,
                        preset.name()
                    ),
                    None => tracing::warn!("No bytes received from {} in the last interval", peer),
                }
                self.last_bandwidth_status = status;
            }
        }

        if let Ok(mut stats) = self.report.stats.write() {
            *stats = Some(conn_stats);
        }
        // Update peer latency info (ADR-013: sample rate from remote peer)
        if let Some(peer_info) = conn.peer_latency_info() {
            match self.receive.follow_peer_rate(peer_info.sample_rate) {
                PeerRateChange::Unchanged => {}
                PeerRateChange::Resampling {
                    from,
                    to,
                    latency_ms,
                } => tracing::info!(
                    "Resampler created for {}: {} Hz -> {} Hz (latency: {:.2} ms)",
                    peer,
                    from,
                    to,
                    latency_ms
                ),
                PeerRateChange::Passthrough => tracing::info!(
                    "Resampler disabled for {}: peer rate matches local rate ({} Hz)",
                    peer,
                    sample_rate
                ),
                PeerRateChange::Failed(e) => {
                    tracing::warn!("Failed to create resampler: {}", e)
                }
            }
            self.receive.follow_peer_channels(peer_info.channel_count);
            if let Ok(mut info) = self.report.latency_info.write() {
                *info = Some(peer_info);
            }
        }
    }
}

/// The connections of the links that are up, for the send thread
fn senders_of(links: &[Link]) -> Arc<[Arc<tokio::sync::Mutex<Connection>>]> {
    links
        .iter()
        .filter_map(|link| link.connection().cloned())
        .collect()
}

/// Run audio streaming in the audio thread
#[allow(clippy::too_many_arguments)]
async fn run_audio_streaming(
    prepared_socket: std::net::UdpSocket,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    buffer_size: u32,
    preset: AudioPreset,
    sample_rate: u32,
    transmit_channels: u32,
    mut input_channels: (u32, Option<u32>),
    mut output_channels: (u32, Option<u32>),
    cmd_rx: std_mpsc::Receiver<StreamingCommand>,
    is_active: &AtomicBool,
    is_muted: &AtomicBool,
    is_monitoring: &AtomicBool,
    input_level: &AtomicU32,
    output_level: &Arc<AtomicU32>,
    shared_links: &LinkReports,
    peer_gains: &RwLock<HashMap<Uuid, Arc<PeerGain>>>,
    device_problems: &RwLock<Vec<DeviceProblem>>,
    underrun_count: &Arc<AtomicU64>,
    delay_adjustments: &AtomicU64,
    reconnect_count: &Arc<AtomicU64>,
    silence_giveups: &Arc<AtomicU64>,
    shared_link_facts: &RwLock<Option<Arc<LinkFacts>>>,
    shared_flights: &Flights,
    master_volume: &Arc<AtomicU32>,
    local_volume: &AtomicU32,
    local_pan: &std::sync::atomic::AtomicI32,
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

    // Everyone in the room sends to the one address this app told it, so every
    // link goes through that socket (the one `streaming_prepare` bound).
    let mux = UdpMux::from_std(prepared_socket)
        .map_err(|e| format!("Failed to adopt the prepared audio socket: {}", e))?;

    // Create separate audio engines for capture (mono) and playback (stereo)
    let mut capture_engine = AudioEngine::new(capture_config.clone());
    let mut playback_engine = AudioEngine::new(playback_config.clone());

    let input_id = input_device_id.map(DeviceId);
    let output_id = output_device_id.map(DeviceId);

    // Zero-allocation audio pipeline using rtrb for both capture and playback:
    // - Capture: rtrb ring buffer (FnMut callback, no Sync requirement)
    // - Playback: rtrb ring buffer (wait-free network callback)
    // - Send thread: dedicated std::thread (no tokio overhead)

    // Configure the codec and FEC from the preset (ADR-021).
    let encoding = AudioEncodingConfig {
        codec_type: preset.codec_type(),
        sample_rate,
        channels: 2, // stereo on the wire, with local_pan already applied
        frame_size: buffer_size,
        bitrate: 0, // unused for PCM
        fec_group_size: preset.fec_group_size(),
    };

    // The play-out delay a link starts with. A preset switch moves it for the
    // links there are and for those opened later.
    let mut jitter_buffer_frames = preset.jitter_buffer_frames();
    // The buffer counts packets, and a frame too big for one packet goes as
    // several (ADR-070), so its depth lasts packets long, not frames long.
    let packet_duration_ms = audio_packet_duration_ms(buffer_size, WIRE_CHANNELS, sample_rate);
    // Everyone the session has a link to is mixed here for the output callback
    let mix = PeerMix::new(MAX_LINKS, sample_rate, buffer_size, underrun_count.clone());
    let stereo_frame_size = mix.frame_samples();

    let bandwidth_required_bps = required_bps(&preset, sample_rate, 2);

    let input_level_for_capture = Arc::new(AtomicU32::new(0));

    // Local monitoring: what the capture callback hears goes straight to the
    // output callback, never through the network (ADR-033).
    let monitor = LocalMonitor::new(buffer_size);
    monitor.set_enabled(is_monitoring.load(Ordering::SeqCst));

    // Recording/streaming virtual output (rm:1068): self (A) / peer (B) / mix
    // (C) as one 6ch device a DAW or OBS can pick as a recording input.
    // `None` where the platform or environment cannot provide one; the
    // session runs the same either way.
    let recording_feed = RecordingFeed::start(sample_rate).map(Arc::new);

    // Start audio capture with level metering. Mono or stereo follows the
    // transmit channel setting (REQ-AUD-107/108).
    let mut wanted_channels = transmit_channels.clamp(1, WIRE_CHANNELS as u32) as u16;

    // The input and output settings name the same ASIO driver (Windows
    // only): one ASIO session serves both directions instead of the two
    // engines below (see `audio::asio`, and the CLI's `AudioIo::start`,
    // which the same check comes from).
    #[cfg(target_os = "windows")]
    let asio_driver = shared_asio_driver(input_id.as_ref(), output_id.as_ref());
    #[cfg(not(target_os = "windows"))]
    let asio_driver: Option<String> = None;
    #[cfg(target_os = "windows")]
    let mut asio_session: Option<AsioDuplex> = None;

    // The ring, and whether the input opened: one that did not has no stream
    // to watch
    let (capture_ring, capture_opened) = match &asio_driver {
        #[cfg(target_os = "windows")]
        Some(driver_name) => match open_asio(
            driver_name,
            input_id.as_ref(),
            output_id.as_ref(),
            sample_rate,
            buffer_size,
            stereo_frame_size,
            wanted_channels,
            input_channels,
            output_channels,
            input_level_for_capture.clone(),
            monitor.clone(),
            mix.clone(),
            master_volume.clone(),
            output_level.clone(),
            recording_feed.as_deref(),
        ) {
            Ok((session, ring)) => {
                tracing::info!("ASIO session started on {}", driver_name);
                clear_device_problem(device_problems, DeviceSide::Input);
                clear_device_problem(device_problems, DeviceSide::Output);
                asio_session = Some(session);
                (ring, true)
            }
            Err(e) => {
                note_device_problem(
                    device_problems,
                    DeviceSide::Input,
                    input_id.as_ref(),
                    sample_rate,
                    &e,
                );
                note_device_problem(
                    device_problems,
                    DeviceSide::Output,
                    output_id.as_ref(),
                    sample_rate,
                    &e,
                );
                return Err(format!("Failed to start the ASIO session: {}", e));
            }
        },
        #[cfg(not(target_os = "windows"))]
        Some(_) => unreachable!("asio_driver is always None off Windows"),
        None => {
            let opened = start_capture_ring(
                &mut capture_engine,
                input_id.as_ref(),
                wanted_channels,
                input_channels,
                buffer_size as usize,
                &input_level_for_capture,
                &monitor,
                recording_feed.as_deref(),
            );
            let opened_ok = opened.is_ok();
            let ring = capture_or_silence(
                opened,
                wanted_channels,
                device_problems,
                input_id.as_ref(),
                sample_rate,
            );

            // Start audio playback (stereo). The callback takes frames from
            // the play-out buffer itself and applies the mixer gains, so a
            // volume change is heard on the next frame and nothing queues
            // behind the buffer (ADR-028).
            if let Err(e) = start_playout(
                &mut playback_engine,
                output_id.as_ref(),
                output_channels,
                stereo_frame_size,
                || {
                    playout_source(
                        mix.clone(),
                        master_volume.clone(),
                        output_level.clone(),
                        monitor.clone(),
                        recording_feed
                            .as_deref()
                            .map(|feed| (feed.tap_b(), feed.tap_c())),
                    )
                },
            ) {
                note_device_problem(
                    device_problems,
                    DeviceSide::Output,
                    output_id.as_ref(),
                    sample_rate,
                    &e,
                );
                return Err(format!("Failed to start playback: {}", e));
            }
            tracing::info!("Playback started on {:?}", output_id);
            (ring, opened_ok)
        }
    };
    let capture_channels = capture_ring.channels;
    let capture_ring = Arc::new(std::sync::Mutex::new(capture_ring));

    let mut capture = DeviceSlot::new("input", capture_config, capture_engine, input_id.clone());
    if !capture_opened {
        capture = capture.without_stream();
    }
    let mut playback: DeviceSlot<()> = DeviceSlot::new(
        "output",
        playback_config,
        playback_engine,
        output_id.clone(),
    );

    // What the thread that opens the input device runs, for a new device, new
    // channels, and a stream that stopped calling back: the ring is made
    // anew, and the send thread takes it once the open has finished.
    let capture_job = |device: Option<DeviceId>, wanted: u16, picks: (u32, Option<u32>)| {
        let input_level = input_level_for_capture.clone();
        let monitor = monitor.clone();
        let recording_feed = recording_feed.clone();
        move |engine: &mut AudioEngine| {
            start_capture_ring(
                engine,
                device.as_ref(),
                wanted,
                picks,
                buffer_size as usize,
                &input_level,
                &monitor,
                recording_feed.as_deref(),
            )
        }
    };

    // What the thread that switches the output device runs: the stream is
    // rebuilt around the same play-out buffer, so the audio already waiting
    // in it survives the switch.
    let playout_job = |device: Option<DeviceId>, channels: (u32, Option<u32>)| {
        let mix = mix.clone();
        let master_volume = master_volume.clone();
        let output_level = output_level.clone();
        let monitor = monitor.clone();
        let recording_feed = recording_feed.clone();
        move |engine: &mut AudioEngine| {
            engine.stop_playback();
            start_playout(engine, device.as_ref(), channels, stereo_frame_size, || {
                playout_source(
                    mix.clone(),
                    master_volume.clone(),
                    output_level.clone(),
                    monitor.clone(),
                    recording_feed
                        .as_deref()
                        .map(|feed| (feed.tap_b(), feed.tap_c())),
                )
            })
        }
    };

    tracing::info!("Streaming active");

    // What every peer is told about our audio when its link comes up (ADR-013:
    // sample rate communication). The delay is that of the peer's own
    // play-out buffer, filled in as it is sent.
    let mut local_latency_info = LatencyInfoMessage {
        capture_buffer_ms: (buffer_size as f32 / sample_rate as f32) * 1000.0,
        playback_buffer_ms: (buffer_size as f32 / sample_rate as f32) * 1000.0,
        encode_ms: 0.0, // PCM, no encoding delay
        decode_ms: 0.0, // PCM, no decoding delay
        jitter_buffer_ms: jitter_buffer_frames as f32 * packet_duration_ms,
        frame_size: buffer_size,
        sample_rate,
        codec: "pcm".to_string(),
        channel_count: capture_channels as u8,
    };

    let mut links: Vec<Link> = Vec::new();
    let link_setup = LinkSetup {
        mux: &mux,
        mix: &mix,
        encoding: &encoding,
        sample_rate,
        buffer_size,
        packet_duration_ms,
        peer_gains,
        reconnect_count,
        silence_giveups,
        shared_link_facts,
        shared_flights,
    };
    let senders: Senders = Arc::new(RwLock::new(Arc::from(Vec::new())));
    let senders_for_send = senders.clone();

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
    let capture_ring_for_send = capture_ring.clone();

    // Spawn dedicated thread to send captured audio (no tokio overhead)
    // This thread reads from rtrb consumer and sends via connection
    let send_thread = std::thread::spawn(move || {
        // Create a mini tokio runtime just for async send operations
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create send thread runtime");

        let mut timestamp: u32 = 0;
        // Room for the widest capture, so a switch between mono and stereo
        // needs no new allocation
        let mut send_buffer = vec![0.0f32; send_buffer_size * WIRE_CHANNELS];
        // Stereo buffer for pan-converted output (2x mono size)
        let mut stereo_send_buffer = vec![0.0f32; send_buffer_size * 2];
        // Muting fades the microphone out (and unmuting in) rather than cutting
        // it, so the other side does not hear a click.
        let mut mute_ramp = GainRamp::new(sample_rate);

        // Capture opened before this thread started, so the ring holds what
        // was captured meanwhile. Sent, that would be a burst of old audio
        // ahead of the live frames, and the other side's play-out buffer
        // overflows on it. From here on the ring is read as it fills whether
        // or not a link is up, so nothing old waits for the next link either.
        if let Ok(mut ring) = capture_ring_for_send.lock() {
            discard_queued(&mut ring.consumer);
        }

        while send_thread_running_ref.load(Ordering::SeqCst) {
            // Try to read a frame from capture ring buffer. Yields the channel
            // count of the frame read: a frame is `send_buffer_size` samples
            // per channel.
            let channels_read = {
                if let Ok(mut ring) = capture_ring_for_send.try_lock() {
                    let channels = ring.channels;
                    let frame_samples = send_buffer_size * channels;
                    let available = ring.consumer.slots();
                    if available >= frame_samples {
                        if let Ok(chunk) = ring.consumer.read_chunk(frame_samples) {
                            let slices = chunk.as_slices();
                            send_buffer[..slices.0.len()].copy_from_slice(slices.0);
                            if !slices.1.is_empty() {
                                send_buffer[slices.0.len()..frame_samples]
                                    .copy_from_slice(slices.1);
                            }
                            chunk.commit_all();
                            Some(channels)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(channels) = channels_read {
                let mute_target = if is_muted_send_ref.load(Ordering::SeqCst) {
                    0.0
                } else {
                    1.0
                };
                // Nothing is sent once the fade-out has finished
                if !mute_ramp.is_silent_at(mute_target) {
                    let volume = local_volume_send_ref.load(Ordering::SeqCst) as f32 / 100.0;
                    let pan = local_pan_send_ref.load(Ordering::SeqCst);
                    capture_to_wire(
                        &send_buffer[..send_buffer_size * channels],
                        channels,
                        volume,
                        pan,
                        &mut stereo_send_buffer,
                    );
                    mute_ramp.apply(&mut stereo_send_buffer, WIRE_CHANNELS, mute_target);
                    #[cfg(feature = "debug-tools")]
                    {
                        use crate::audio_tap::{inject, observe, Point};
                        inject(Point::Sent, &mut stereo_send_buffer, WIRE_CHANNELS);
                        observe(Point::Sent, &stereo_send_buffer, WIRE_CHANNELS);
                    }

                    // Everyone gets the same frame, each through their own link
                    let targets = senders_for_send
                        .read()
                        .map(|senders| senders.clone())
                        .unwrap_or_else(|e| e.into_inner().clone());
                    rt.block_on(async {
                        for connection in targets.iter() {
                            let conn = connection.lock().await;
                            if conn.is_connected() {
                                if let Err(e) =
                                    conn.send_audio(&stereo_send_buffer, timestamp).await
                                {
                                    tracing::warn!("Failed to send audio: {}", e);
                                }
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

    // This thread carries the network task as well as this loop, so a turn of
    // the loop that comes late is a stretch in which no packet was taken off
    // the socket either.
    let mut last_turn = std::time::Instant::now();
    let stall_after = std::time::Duration::from_micros(2 * mix.frame_us());

    // The ASIO driver currently in use, if any - what the reconciliation
    // below compares a device or channel change against to decide whether
    // the path in use (ASIO, or the two engines) needs to change.
    #[cfg(target_os = "windows")]
    let mut asio_driver_in_use = asio_driver;
    #[cfg(target_os = "windows")]
    let mut asio_switch: AsioSwitch<CaptureRing> = AsioSwitch::idle();

    loop {
        let pass_started = jamjam::perf::start();
        let turn_started = std::time::Instant::now();
        let away = turn_started.duration_since(last_turn);
        if away > stall_after {
            for link in &links {
                link.receive
                    .flight()
                    .note(FlightKind::ThreadStall, away.as_micros() as u64);
            }
        }
        last_turn = turn_started;

        // Set by a command that has capture, or playback, start over: a new
        // device, or a new channel count on the current one. Deferred to
        // after the match below (rather than switched right away, the way
        // this loop used to handle playback) so the two can be reconciled
        // with ASIO together: whichever changes, the pair might now share
        // (or stop sharing) an ASIO driver, which takes over both directions
        // at once.
        let mut reopen_capture: Option<Option<DeviceId>> = None;
        let mut reopen_playback: Option<Option<DeviceId>> = None;

        // Check for commands (non-blocking)
        match cmd_rx.try_recv() {
            Ok(StreamingCommand::Stop) => {
                tracing::info!("Stopping streaming...");
                break;
            }
            Ok(StreamingCommand::SetInputDevice(device_id)) => {
                tracing::info!("Switching input device to: {:?}", device_id);
                reopen_capture = Some(device_id.map(DeviceId));
            }
            Ok(StreamingCommand::SetTransmitChannels(count)) => {
                tracing::info!("Setting transmit channels to: {}", count);
                wanted_channels = count.clamp(1, WIRE_CHANNELS as u32) as u16;
                reopen_capture = Some(capture.device().cloned());
            }
            Ok(StreamingCommand::SetOutputDevice(device_id)) => {
                tracing::info!("Switching output device to: {:?}", device_id);
                reopen_playback = Some(device_id.map(DeviceId));
            }
            Ok(StreamingCommand::SetOutputChannels(left, right)) => {
                tracing::info!("Setting output channels to: {} / {:?}", left, right);
                output_channels = (left, right);
                reopen_playback = Some(playback.device().cloned());
            }
            Ok(StreamingCommand::SetInputChannels(left, right)) => {
                tracing::info!("Setting input channels to: {} / {:?}", left, right);
                input_channels = (left, right);
                reopen_capture = Some(capture.device().cloned());
            }
            Ok(StreamingCommand::SetMute(muted)) => {
                tracing::info!("Setting mute state to: {}", muted);
                is_muted_for_send.store(muted, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetMonitoring(enabled)) => {
                monitor.set_enabled(enabled);
            }
            Ok(StreamingCommand::SetMasterVolume(vol)) => {
                tracing::debug!("Setting master volume to: {}", vol);
                master_volume.store((vol * 100.0) as u32, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetLocalVolume(vol)) => {
                tracing::debug!("Setting local volume to: {}", vol);
                let volume_u32 = (vol * 100.0) as u32;
                local_volume.store(volume_u32, Ordering::SeqCst);
                // Also update the send thread's copy
                local_volume_for_send.store(volume_u32, Ordering::SeqCst);
            }
            Ok(StreamingCommand::SetLocalPan(pan)) => {
                tracing::debug!("Setting local pan to: {}", pan);
                local_pan.store(pan, Ordering::SeqCst);
                // Also update the send thread's copy
                local_pan_for_send.store(pan, Ordering::SeqCst);
            }
            Ok(StreamingCommand::Reconnect) => {
                tracing::info!("User requested reconnection");
                // A link that is up, or not up yet, has no loss to recover from
                for link in &links {
                    if let Some(connection) = link.connection() {
                        if let Err(e) = connection.lock().await.reconnect() {
                            tracing::warn!("Reconnect refused: {}", e);
                        }
                    }
                }
            }
            Ok(StreamingCommand::SetJitterBufferFrames(frames)) => {
                tracing::info!("Setting the play-out delay to {} frame(s)", frames);
                jitter_buffer_frames = frames;
                // What is held moves to the new delay (ADR-031). The display
                // and the peers hear about it from the stats tick below.
                for link in &links {
                    link.receive.set_delay_frames(frames);
                }
            }
            Ok(StreamingCommand::Link(target)) => {
                let peer = target.peer;
                // Asked for twice: the link there is is the one to keep
                if links.iter().any(|link| link.peer == peer) {
                    tracing::debug!("There is a link to {} already", peer);
                } else {
                    // What is left of an attempt that could not be started
                    if let Ok(mut shared) = shared_links.write() {
                        shared.retain(|(linked, _)| *linked != peer);
                    }
                    match link_setup.open(*target, jitter_buffer_frames) {
                        Ok(link) => {
                            if let Ok(mut shared) = shared_links.write() {
                                shared.push((peer, link.report.clone()));
                            }
                            links.push(link);
                        }
                        Err(e) => {
                            tracing::error!(
                                "The audio link to {} could not be started: {}",
                                peer,
                                e
                            );
                            // Told like a link that gave up, so whoever
                            // decides about the links hears of it
                            let report = Arc::new(LinkReport::default());
                            report.set_state("failed", None);
                            if let Ok(mut shared) = shared_links.write() {
                                shared.push((peer, report));
                            }
                        }
                    }
                }
            }
            Ok(StreamingCommand::Unlink(peer)) => {
                if let Ok(mut shared) = shared_links.write() {
                    shared.retain(|(linked, _)| *linked != peer);
                }
                if let Some(at) = links.iter().position(|link| link.peer == peer) {
                    let link = links.remove(at);
                    if let Ok(mut senders) = senders.write() {
                        *senders = senders_of(&links);
                    }
                    link.close().await;
                    tracing::info!("Audio link to {} closed", peer);
                }
            }
            Err(std_mpsc::TryRecvError::Disconnected) => {
                tracing::info!("Command channel disconnected");
                break;
            }
            Err(std_mpsc::TryRecvError::Empty) => {
                // No command, continue
            }
        }

        // Whether this turn's command could put the pair on, or take it off,
        // a shared ASIO driver: a device pick on either side, or - while
        // ASIO is active - a channel setting, since ASIO needs the new picks
        // and route from the moment it reopens (unlike `start_capture_ring`,
        // it does not retry with a narrower channel selection afterwards).
        // Windows only: off Windows there is no ASIO path, and
        // `reopen_capture`/`reopen_playback` proceed exactly as before.
        #[cfg(target_os = "windows")]
        if reopen_capture.is_some() || reopen_playback.is_some() {
            let wanted_input = reopen_capture
                .clone()
                .unwrap_or_else(|| capture.device().cloned());
            let wanted_output = reopen_playback
                .clone()
                .unwrap_or_else(|| playback.device().cloned());
            let wanted_asio_driver =
                shared_asio_driver(wanted_input.as_ref(), wanted_output.as_ref());

            if let Some(driver_name) = wanted_asio_driver {
                capture.park(wanted_input.clone());
                playback.park(wanted_output.clone());
                let previous = asio_session.take();
                let input_level = input_level_for_capture.clone();
                let monitor = monitor.clone();
                let mix = mix.clone();
                let master_volume = master_volume.clone();
                let output_level = output_level.clone();
                let recording_feed = recording_feed.clone();
                let frame_samples = mix.frame_samples();
                let wanted = wanted_channels;
                let input_ch = input_channels;
                let output_ch = output_channels;
                let open_input = wanted_input.clone();
                let open_output = wanted_output.clone();
                let open_driver_name = driver_name.clone();
                asio_switch.switch(previous, move || {
                    open_asio(
                        &open_driver_name,
                        open_input.as_ref(),
                        open_output.as_ref(),
                        sample_rate,
                        buffer_size,
                        frame_samples,
                        wanted,
                        input_ch,
                        output_ch,
                        input_level,
                        monitor,
                        mix,
                        master_volume,
                        output_level,
                        recording_feed.as_deref(),
                    )
                });
                asio_driver_in_use = Some(driver_name);
                reopen_capture = None;
                reopen_playback = None;
            } else if asio_driver_in_use.is_some() {
                // Leaving ASIO: it governed both directions, so both need a
                // normal open now, even if only one side's command fired
                // this turn.
                if let Some(previous) = asio_session.take() {
                    let _ = thread::Builder::new()
                        .name("audio-switch-asio".to_string())
                        .spawn(move || previous.stop());
                }
                reopen_capture.get_or_insert(wanted_input);
                reopen_playback.get_or_insert(wanted_output);
                asio_driver_in_use = None;
            }
        }

        if let Some(device_id) = reopen_capture {
            let job = capture_job(device_id.clone(), wanted_channels, input_channels);
            capture.switch(device_id, job);
        }
        if let Some(device_id) = reopen_playback {
            let device = device_id.clone();
            playback.switch(device_id, playout_job(device, output_channels));
        }

        // How the last switch of each device went, if it has finished. Until
        // it has, the session goes on: a device that does not answer holds
        // up only its own switch.
        if let Some(opened) = capture.poll() {
            match opened {
                Ok(ring) => {
                    // The peer is told below if the channel count changed
                    local_latency_info.channel_count = ring.channels as u8;
                    // Replace the ring in the shared reference (send thread will pick this up)
                    if let Ok(mut guard) = capture_ring.lock() {
                        *guard = ring;
                    }
                    clear_device_problem(device_problems, DeviceSide::Input);
                }
                Err(e) => {
                    tracing::warn!("Failed to restart capture: {}", e);
                    note_device_problem(
                        device_problems,
                        DeviceSide::Input,
                        capture.device(),
                        sample_rate,
                        &e,
                    );
                }
            }
        }
        if let Some(opened) = playback.poll() {
            match opened {
                Ok(()) => clear_device_problem(device_problems, DeviceSide::Output),
                Err(e) => {
                    tracing::warn!("Failed to switch output device: {}", e);
                    note_device_problem(
                        device_problems,
                        DeviceSide::Output,
                        playback.device(),
                        sample_rate,
                        &e,
                    );
                }
            }
        }
        // How the last switch to an ASIO driver went, if it has finished
        // (Windows only; see the reconciliation above).
        #[cfg(target_os = "windows")]
        if let Some(result) = asio_switch.poll() {
            match result {
                Ok((session, ring)) => {
                    tracing::info!("ASIO session started on {:?}", asio_driver_in_use);
                    asio_session = Some(session);
                    local_latency_info.channel_count = ring.channels as u8;
                    if let Ok(mut guard) = capture_ring.lock() {
                        *guard = ring;
                    }
                    clear_device_problem(device_problems, DeviceSide::Input);
                    clear_device_problem(device_problems, DeviceSide::Output);
                }
                Err(e) => {
                    tracing::warn!("Failed to switch the ASIO driver: {}", e);
                    note_device_problem(
                        device_problems,
                        DeviceSide::Input,
                        capture.device(),
                        sample_rate,
                        &e,
                    );
                    note_device_problem(
                        device_problems,
                        DeviceSide::Output,
                        playback.device(),
                        sample_rate,
                        &e,
                    );
                }
            }
        }

        // A stream that has stopped calling back raises no error and leaves
        // the session connected, so the callbacks are watched: one silent
        // for a second is opened again, and one that stays silent is told to
        // the user (REQ-AUD-128). ASIO has its own session, not watched here.
        #[cfg(target_os = "windows")]
        let streams_watched = asio_driver_in_use.is_none();
        #[cfg(not(target_os = "windows"))]
        let streams_watched = true;
        if streams_watched {
            let now = std::time::Instant::now();
            match capture.watch(now) {
                Verdict::Flowing => {}
                Verdict::Reopen => {
                    tracing::warn!("The input stopped calling back; opening it again");
                    jamjam::perf::count_input_reopen();
                    let device = capture.device().cloned();
                    let job = capture_job(device.clone(), wanted_channels, input_channels);
                    capture.switch(device, job);
                }
                Verdict::GiveUp => {
                    tracing::error!("The input is still silent after being opened again");
                    note_trouble(
                        device_problems,
                        DeviceSide::Input,
                        capture.device(),
                        sample_rate,
                        DeviceTrouble::Stalled,
                    );
                }
                Verdict::Resumed => {
                    tracing::info!("The input calls back again");
                    clear_device_problem(device_problems, DeviceSide::Input);
                }
            }
            match playback.watch(now) {
                Verdict::Flowing => {}
                Verdict::Reopen => {
                    tracing::warn!("The output stopped calling back; opening it again");
                    jamjam::perf::count_output_reopen();
                    let device = playback.device().cloned();
                    playback.switch(device.clone(), playout_job(device, output_channels));
                }
                Verdict::GiveUp => {
                    tracing::error!("The output is still silent after being opened again");
                    note_trouble(
                        device_problems,
                        DeviceSide::Output,
                        playback.device(),
                        sample_rate,
                        DeviceTrouble::Stalled,
                    );
                }
                Verdict::Resumed => {
                    tracing::info!("The output calls back again");
                    clear_device_problem(device_problems, DeviceSide::Output);
                }
            }
        }

        // Check if we should still be active
        if !is_active.load(Ordering::SeqCst) {
            break;
        }

        // A link whose connectivity check has ended is up: its peer is told
        // about our audio, and the send thread starts sending to it.
        let mut came_up = false;
        for link in &mut links {
            let LinkStage::Checking(checking) = &mut link.stage else {
                continue;
            };
            if !checking.is_finished() {
                continue;
            }
            link.stage = match checking.await {
                Ok(Ok(connection)) => {
                    let info = LatencyInfoMessage {
                        jitter_buffer_ms: link.delay_us(packet_duration_ms) as f32 / 1000.0,
                        ..local_latency_info.clone()
                    };
                    if let Err(e) = connection.send_latency_info(&info).await {
                        tracing::warn!("Failed to send initial latency info: {}", e);
                    }
                    link.announced_channel_count = info.channel_count;
                    came_up = true;
                    LinkStage::Up(Arc::new(tokio::sync::Mutex::new(connection)))
                }
                Ok(Err(e)) => {
                    tracing::warn!("The audio link to {} could not be set up: {}", link.peer, e);
                    link.report.set_state("failed", None);
                    LinkStage::Down
                }
                Err(e) => {
                    tracing::warn!(
                        "The check of the audio link to {} ended early: {}",
                        link.peer,
                        e
                    );
                    link.report.set_state("failed", None);
                    LinkStage::Down
                }
            };
        }
        if came_up {
            if let Ok(mut senders) = senders.write() {
                *senders = senders_of(&links);
            }
        }

        // Refresh stats and the meters on a wall-clock interval. Counting loop
        // iterations would tie the cadence to how fast this loop happens to
        // spin, which is a property of the audio clock, not of anything the UI
        // cares about.
        if std::time::Instant::now() >= next_adapt_at {
            next_adapt_at = std::time::Instant::now() + ADAPT_INTERVAL;
            for link in &links {
                if let Some(frames) = link.receive.adapt() {
                    tracing::info!(
                        "Play-out delay of {} adjusted to {} frame(s)",
                        link.peer,
                        frames
                    );
                    delay_adjustments.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Whatever moved a delay, or the channels we send, the display and the
        // peer follow. The peer shows a total latency that includes our
        // buffer, so it has to hear about the change - otherwise its display
        // keeps the value from connect time. If the connection is busy the
        // peer is told on the next tick.
        let channel_count = local_latency_info.channel_count;
        for link in &mut links {
            let delay_frames = link.receive.delay_frames();
            if delay_frames == link.announced_delay_frames
                && channel_count == link.announced_channel_count
            {
                continue;
            }
            let Some(connection) = link.connection().cloned() else {
                // Told when the link comes up
                link.announced_delay_frames = delay_frames;
                continue;
            };
            let told = match connection.try_lock() {
                Ok(conn) => {
                    let updated = LatencyInfoMessage {
                        jitter_buffer_ms: link.delay_us(packet_duration_ms) as f32 / 1000.0,
                        ..local_latency_info.clone()
                    };
                    if let Err(e) = conn.send_latency_info(&updated).await {
                        tracing::warn!("Failed to send updated latency info: {}", e);
                    }
                    true
                }
                Err(_) => false,
            };
            if told {
                link.announced_delay_frames = delay_frames;
                link.announced_channel_count = channel_count;
            }
        }

        if std::time::Instant::now() >= next_stats_at {
            next_stats_at = std::time::Instant::now() + STATS_INTERVAL;
            for link in &mut links {
                link.refresh(&preset, bandwidth_required_bps, sample_rate);
                // Whatever moved the delay - a preset switch, an adaptation,
                // a reconnect, the peer saying how long its packets are - the
                // display follows (ADR-031)
                link.report
                    .jitter_buffer_delay_us
                    .store(link.delay_us(packet_duration_ms), Ordering::SeqCst);
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
        jamjam::perf::RECEIVE_LOOP.stop(pass_started);
        tokio::time::sleep(POP_IDLE).await;
    }

    // Cleanup: signal send thread to stop and wait for it
    send_thread_running.store(false, Ordering::SeqCst);
    let _ = send_thread.join();

    if let Ok(mut senders) = senders.write() {
        *senders = Arc::from(Vec::new());
    }
    for link in links {
        link.close().await;
    }

    drop(capture);
    drop(playback);
    #[cfg(target_os = "windows")]
    if let Some(session) = asio_session.take() {
        session.stop();
    }

    tracing::info!("Streaming stopped.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jamjam::audio::PlayoutConfig;
    use tauri::Manager;

    /// Verifies: REQ-AUD-126
    #[cfg(target_os = "windows")]
    #[test]
    fn the_same_asio_driver_on_both_sides_is_the_shared_driver() {
        let asio = DeviceId("asio:Focusrite USB ASIO".to_string());
        assert_eq!(
            shared_asio_driver(Some(&asio), Some(&asio)),
            Some("Focusrite USB ASIO".to_string())
        );
    }

    /// Verifies: REQ-AUD-126
    #[cfg(target_os = "windows")]
    #[test]
    fn two_different_asio_drivers_are_not_a_shared_driver() {
        let a = DeviceId("asio:Focusrite USB ASIO".to_string());
        let b = DeviceId("asio:RME Fireface".to_string());
        assert_eq!(shared_asio_driver(Some(&a), Some(&b)), None);
    }

    /// Verifies: REQ-AUD-126
    #[cfg(target_os = "windows")]
    #[test]
    fn a_non_asio_device_on_either_side_is_not_a_shared_driver() {
        let asio = DeviceId("asio:Focusrite USB ASIO".to_string());
        let wasapi = DeviceId("wasapi:{some-guid}".to_string());
        assert_eq!(shared_asio_driver(Some(&asio), Some(&wasapi)), None);
        assert_eq!(shared_asio_driver(Some(&wasapi), Some(&wasapi)), None);
    }

    /// Verifies: REQ-AUD-126
    #[cfg(target_os = "windows")]
    #[test]
    fn with_no_device_on_either_side_there_is_no_shared_driver() {
        let asio = DeviceId("asio:Focusrite USB ASIO".to_string());
        assert_eq!(shared_asio_driver(None, Some(&asio)), None);
        assert_eq!(shared_asio_driver(Some(&asio), None), None);
        assert_eq!(shared_asio_driver(None, None), None);
    }

    /// The bug this guards: stopping waited a fixed 100 ms, and a thread that
    /// took longer to end cleared `is_active` after the next session had set
    /// it, which ended that session as soon as it connected.
    ///
    /// Verifies: REQ-CON-134
    #[tokio::test]
    async fn stopping_waits_until_the_audio_thread_has_ended() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        let state = app.state::<StreamingState>();
        state.is_active.store(true, Ordering::SeqCst);
        let is_active = state.is_active.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            is_active.store(false, Ordering::SeqCst);
        });
        let started = std::time::Instant::now();

        streaming_stop(app.state()).await.unwrap();

        assert!(started.elapsed() >= std::time::Duration::from_millis(400));
        assert!(!state.is_active.load(Ordering::SeqCst));
    }

    fn peer(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// The delay shown for a peer is its play-out buffer's depth in packets
    /// times how long a packet of *theirs* lasts. Counted in ours, a peer at
    /// twice our buffer size showed half the delay it had.
    ///
    /// Verifies: REQ-AUD-038
    #[test]
    fn a_peers_packet_lasts_as_long_as_its_own_buffer_size_says_and_as_ours_until_it_has_told() {
        let ours = audio_packet_duration_ms(64, WIRE_CHANNELS, 48_000);
        let told = |frame_size, sample_rate| PeerLatencyInfo {
            capture_buffer_ms: 0.0,
            playback_buffer_ms: 0.0,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 0.0,
            frame_size,
            sample_rate,
            codec: "pcm".to_string(),
            channel_count: 2,
        };

        assert_eq!(peer_packet_ms(None, ours), ours);
        assert_eq!(peer_packet_ms(Some(&told(64, 48_000)), ours), ours);
        assert_eq!(peer_packet_ms(Some(&told(128, 48_000)), ours), ours * 2.0);
        // 256 frames go as two packets of 128 (ADR-070)
        assert_eq!(peer_packet_ms(Some(&told(256, 48_000)), ours), ours * 2.0);
        assert_eq!(peer_packet_ms(Some(&told(0, 0)), ours), ours);
    }

    /// A link that came up on an address that never answered reports itself
    /// connected. Only packets from the peer make it one that hears them.
    ///
    /// Verifies: REQ-CON-134
    #[test]
    fn a_link_is_heard_only_once_packets_of_its_peer_have_arrived() {
        let state = StreamingState::new();
        state.set_link_for_test(peer(2), "checkingconnectivity", None);
        state.set_link_for_test(peer(3), "connected", Some((0, 0.0)));
        state.set_link_for_test(peer(4), "connected", Some((12, 5.0)));
        state.set_link_for_test(peer(5), "reconnecting", Some((12, 5.0)));
        state.set_link_for_test(peer(6), "failed", Some((12, 5.0)));

        assert_eq!(
            state.link_healths(),
            vec![
                (peer(2), LinkHealth::Starting),
                (peer(3), LinkHealth::Starting),
                (peer(4), LinkHealth::Heard),
                (peer(5), LinkHealth::Silent),
                (peer(6), LinkHealth::Failed),
            ]
        );
    }

    /// The bug this guards: the status had the state and statistics of one
    /// link, so the screen showed every participant the same latency and
    /// could not say whom the app was not connected to.
    ///
    /// Verifies: REQ-LAT-102
    #[tokio::test]
    async fn the_status_has_the_state_and_the_latency_of_the_link_to_each_participant() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        let state = app.state::<StreamingState>();
        state.set_link_for_test(peer(2), "connected", Some((100, 8.0)));
        state.set_link_for_test(peer(3), "reconnecting", Some((100, 40.0)));
        state.set_link_for_test(peer(4), "checkingconnectivity", None);

        let status = streaming_status(app.state()).await.unwrap();

        let of = |n: u128| {
            status
                .peers
                .iter()
                .find(|p| p.peer_id == peer(n).to_string())
                .expect("the link is listed")
        };
        assert_eq!(status.peers.len(), 3);
        assert_eq!(of(2).connection_state.as_deref(), Some("connected"));
        assert_eq!(of(2).network.as_ref().unwrap().rtt_ms, Some(8.0));
        assert_eq!(of(3).connection_state.as_deref(), Some("reconnecting"));
        assert_eq!(of(3).network.as_ref().unwrap().rtt_ms, Some(40.0));
        assert!(of(3).latency.as_ref().unwrap().roundtrip_total_ms > 40.0);
        assert_eq!(
            of(4).connection_state.as_deref(),
            Some("checkingconnectivity")
        );
        assert!(of(4).network.is_none() && of(4).latency.is_none());
    }

    /// What a reader written for a room of two reads beside `peers`.
    #[tokio::test]
    async fn the_status_fields_of_one_link_are_those_of_the_link_doing_best_the_earliest_among_equals(
    ) {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        let state = app.state::<StreamingState>();
        state.set_link_for_test(peer(2), "failed", Some((100, 90.0)));
        state.set_link_for_test(peer(3), "connected", Some((100, 8.0)));
        state.set_link_for_test(peer(4), "connected", Some((100, 3.0)));

        let status = streaming_status(app.state()).await.unwrap();

        assert_eq!(status.connection_state.as_deref(), Some("connected"));
        assert_eq!(status.network.unwrap().rtt_ms, Some(8.0));
    }

    #[tokio::test]
    async fn the_status_of_a_session_with_no_link_has_no_peers_and_no_connection_state() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());

        let status = streaming_status(app.state()).await.unwrap();

        assert!(status.peers.is_empty());
        assert_eq!(status.connection_state, None);
        assert!(status.network.is_none());
    }

    /// The status goes to the screen of someone who helps, too. Nothing in
    /// it may be the address of a participant; a field added to it has to be
    /// added here, which is the moment to ask whether it is one.
    ///
    /// Verifies: REQ-RMT-006
    #[tokio::test]
    async fn the_status_names_the_fields_it_always_has_and_none_of_them_is_an_address() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        app.state::<StreamingState>()
            .set_link_for_test(peer(2), "connected", Some((100, 8.0)));

        let status = streaming_status(app.state()).await.unwrap();
        let status = serde_json::to_value(status).unwrap();

        let keys = |value: &serde_json::Value| -> Vec<String> {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        assert_eq!(
            keys(&status),
            [
                "audio_quality",
                "connection_error",
                "connection_state",
                "device_problems",
                "input_level",
                "is_active",
                "is_monitoring",
                "is_muted",
                "latency",
                "network",
                "output_level",
                "peer_audio",
                "peers",
            ]
        );
        assert_eq!(
            keys(&status["peers"][0]),
            [
                "connection_error",
                "connection_state",
                "latency",
                "level",
                "network",
                "peer_audio",
                "peer_id",
            ]
        );
        assert_eq!(
            keys(&status["peers"][0]["network"]),
            [
                "bandwidth_status",
                "bytes_received",
                "bytes_sent",
                "encryption",
                "jitter_ms",
                "measured_bps",
                "packet_loss_percent",
                "packets_received",
                "packets_sent",
                "peer_checked",
                "quality",
                "required_bps",
                "rtt_ms",
                "uptime_seconds",
            ]
        );
    }

    /// Verifies: REQ-RMT-031
    #[test]
    fn the_fader_of_a_participant_is_kept_until_they_are_no_longer_in_the_room() {
        let state = StreamingState::new();
        state.set_peer_volume(peer(2), 50);
        state.set_peer_pan(peer(2), -30);
        state.set_peer_volume(peer(3), 150);

        state.keep_peer_gains(&[peer(3)]);

        assert_eq!(state.peer_gain(peer(2)), None);
        assert_eq!(state.peer_gain(peer(3)), Some((150, 0)));
    }

    /// A session started on a socket the room was never told of would send
    /// and hear no one, with nothing on the screen to say why.
    ///
    /// Verifies: REQ-CON-024
    #[tokio::test]
    async fn a_link_asked_for_when_no_socket_was_prepared_is_refused_and_starts_no_session() {
        let app = tauri::test::mock_app();
        let dir = tempfile::tempdir().unwrap();
        app.manage(StreamingState::new());
        app.manage(crate::config::ConfigState::at(
            dir.path().join("config.toml"),
            crate::config::AppConfig::default(),
        ));
        app.manage(crate::settings::SettingsState::new());
        app.manage(crate::usage::UsageState::with_reporter(
            jamjam::telemetry::UsageReporter::new(
                None,
                "test",
                Arc::new(jamjam::telemetry::NoTransport),
                false,
            ),
        ));
        assert!(!app.state::<StreamingState>().has_prepared_socket().await);

        let refused = streaming_link(
            LinkTarget {
                peer: peer(2),
                candidates: vec!["192.0.2.2:5000".parse().unwrap()],
                link: None,
            },
            app.state(),
            app.state(),
            app.state(),
            app.state(),
        )
        .await;

        assert!(refused.is_err());
        assert!(!app.state::<StreamingState>().is_active());
    }

    #[tokio::test]
    async fn a_link_to_a_participant_with_no_address_is_refused_and_starts_no_session() {
        let app = tauri::test::mock_app();
        let dir = tempfile::tempdir().unwrap();
        app.manage(StreamingState::new());
        app.manage(crate::config::ConfigState::at(
            dir.path().join("config.toml"),
            crate::config::AppConfig::default(),
        ));
        app.manage(crate::settings::SettingsState::new());
        app.manage(crate::usage::UsageState::with_reporter(
            jamjam::telemetry::UsageReporter::new(
                None,
                "test",
                Arc::new(jamjam::telemetry::NoTransport),
                false,
            ),
        ));

        let refused = streaming_link(
            LinkTarget {
                peer: peer(2),
                candidates: Vec::new(),
                link: None,
            },
            app.state(),
            app.state(),
            app.state(),
            app.state(),
        )
        .await;

        assert!(refused.is_err());
        assert!(!app.state::<StreamingState>().is_active());
    }

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

    /// Verifies: REQ-AUD-125
    #[test]
    fn when_the_input_does_not_open_at_the_session_rate_the_session_goes_on_and_says_which_rate() {
        let problems = RwLock::new(Vec::new());
        let refused = AudioError::UnsupportedConfig("Failed to initialize audio client".into());

        let ring = capture_or_silence(Err(refused), 2, &problems, None, 48000);

        assert_eq!(ring.channels, 2);
        assert_eq!(
            ring.consumer.slots(),
            0,
            "nothing is sent from a closed input"
        );
        let problems = problems.read().unwrap();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].side, DeviceSide::Input);
        assert_eq!(problems[0].trouble, DeviceTrouble::UnsupportedFormat);
        assert_eq!(problems[0].sample_rate, 48000);
    }

    /// Verifies: REQ-AUD-125
    #[test]
    fn when_the_input_refuses_for_any_other_reason_the_session_goes_on_too() {
        let problems = RwLock::new(Vec::new());
        let refused = AudioError::StreamError("device busy".into());

        let ring = capture_or_silence(Err(refused), 1, &problems, None, 48000);

        assert_eq!(ring.channels, 1);
        assert_eq!(problems.read().unwrap()[0].trouble, DeviceTrouble::Failed);
    }

    /// Verifies: REQ-AUD-125
    #[test]
    fn when_the_input_opens_nothing_is_reported() {
        let problems = RwLock::new(Vec::new());
        let (_, consumer) = RingBuffer::<f32>::new(8);

        let ring = capture_or_silence(
            Ok(CaptureRing {
                consumer,
                channels: 2,
            }),
            2,
            &problems,
            None,
            48000,
        );

        assert_eq!(ring.channels, 2);
        assert!(problems.read().unwrap().is_empty());
    }

    /// Verifies: REQ-AUD-035
    #[test]
    fn audio_captured_before_the_connection_is_not_sent_once_it_is_up() {
        let (mut producer, mut consumer) = RingBuffer::<f32>::new(8);
        for sample in [1.0_f32, 2.0, 3.0, 4.0] {
            producer.push(sample).unwrap();
        }

        discard_queued(&mut consumer);
        assert_eq!(consumer.slots(), 0, "the old audio is gone");

        producer.push(5.0).unwrap();
        assert_eq!(
            consumer.pop().unwrap(),
            5.0,
            "what follows is the live audio"
        );
    }

    /// A PCM payload as `ReceivePath::receive` expects it: `f32` samples as
    /// little-endian bytes (matches `PcmCodec::encode`).
    fn pcm(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    /// A mix with one participant in it, who sends stereo frames of `frame`
    /// samples per channel and is played at `volume` percent
    fn mix_of_one(frame: u32, volume: u32) -> (PeerMix, ReceivePath, VoiceHandle) {
        let receive = ReceivePath::new(
            AudioPreset::ZeroLatency.codec_type(),
            48_000,
            frame,
            AudioPreset::ZeroLatency.jitter_buffer_frames(),
        )
        .expect("a PCM receive path");
        receive.follow_peer_channels(2);
        let mix = PeerMix::new(MAX_LINKS, 48_000, frame, Arc::new(AtomicU64::new(0)));
        let gain = Arc::new(PeerGain::default());
        gain.set_volume(volume);
        let voice = mix
            .add(receive.clone(), gain, Arc::new(PeerMeter::default()))
            .expect("a free place");
        (mix, receive, voice)
    }

    /// `C` (recording/streaming output) must carry exactly what is played:
    /// post-fader, with the local monitor mixed in, same as what a listener
    /// on the output device hears.
    #[test]
    fn playout_source_feeds_its_c_tap_the_same_audio_it_plays() {
        const FRAME: u32 = 4;
        let (mix, receive, _voice) = mix_of_one(FRAME, 50);
        let sent: Vec<f32> = (0..FRAME).flat_map(|_| [0.8_f32, 0.2_f32]).collect();
        assert!(receive.receive(0, &pcm(&sent)));

        let monitor = LocalMonitor::new(FRAME);
        let mut monitor_tap = monitor.tap();
        monitor.set_enabled(true);
        for _ in 0..3 {
            monitor_tap.push(&[0.1; FRAME as usize]);
        }

        let (b_tap, mut b_consumer) = jamjam::audio::RecordingTap::for_wiring_tests();
        let (c_tap, mut c_consumer) = jamjam::audio::RecordingTap::for_wiring_tests();
        let mut source = playout_source(
            mix,
            Arc::new(AtomicU32::new(100)),
            Arc::new(AtomicU32::new(0)),
            monitor,
            Some((b_tap, c_tap)),
        );
        let mut out = vec![0.0f32; receive.frame_samples() * 3];
        let samples = source(&mut out);
        out.truncate(samples);

        let b_chunk = b_consumer.read_chunk(samples).expect("B got a frame");
        let (b_first, _) = b_chunk.as_slices();
        assert_eq!(
            b_first,
            &sent[..samples],
            "B stays dry even though C mixes in the monitor"
        );

        let c_chunk = c_consumer.read_chunk(samples).expect("C got a frame");
        let (c_first, _) = c_chunk.as_slices();
        assert_eq!(c_first, &out[..], "C matches exactly what is played");
        assert_ne!(
            c_first,
            &sent[..samples],
            "C is not dry: the fader and monitor were applied"
        );
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
        let (mix, receive, _voice) = mix_of_one(FRAME, 100);
        let monitor = LocalMonitor::new(FRAME);
        let mut tap = monitor.tap();
        monitor.set_enabled(monitoring);
        for _ in 0..3 {
            tap.push(&[0.25; FRAME as usize]);
        }

        let mut source = playout_source(
            mix,
            Arc::new(AtomicU32::new(100)),
            Arc::new(AtomicU32::new(0)),
            monitor,
            None,
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
