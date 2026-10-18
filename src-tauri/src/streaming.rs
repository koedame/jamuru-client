//! Audio streaming IPC commands for Tauri
//!
//! Steers the audio session of the core library (`jamjam::audio_session`), which runs on a
//! dedicated audio thread that also carries the network, and publishes what the UI shows.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;
use uuid::Uuid;

use jamjam::audio::DeviceProblem;
pub use jamjam::audio_session::LinkTarget;
use jamjam::audio_session::{self, BandwidthSnapshot, LinkReport, Params, Shared};
use jamjam::network::{
    status_label, ConnectionStats, LatencyBreakdown, LinkHealth, LinkSnapshot, LocalLatencyInfo,
};

pub(crate) use jamjam::audio_session::Command as StreamingCommand;

/// Streaming state managed by Tauri
pub struct StreamingState {
    /// Command sender to audio thread (None if not started)
    cmd_tx: Mutex<Option<std_mpsc::Sender<StreamingCommand>>>,
    /// What the audio session publishes and takes
    shared: Shared,
    /// Current buffer size (frame_size) for latency calculations
    buffer_size: Mutex<u32>,
    /// Current sample rate in Hz (ADR-013: user-selectable)
    sample_rate: Arc<AtomicU32>,
    /// UDP socket bound ahead of streaming so its port can be advertised to
    /// peers before any audio flows (ADR-026).
    ///
    /// Held as `std::net::UdpSocket`, not a `UdpMux`: the audio thread
    /// runs its own Tokio runtime, and a Tokio socket only receives wakeups on
    /// the runtime that registered it. The descriptor moves; the registration
    /// happens when the session starts.
    prepared_socket: Mutex<Option<std::net::UdpSocket>>,
    /// Address `prepared_socket` is bound to, kept so repeated prepares report
    /// the same one.
    prepared_addr: Mutex<Option<SocketAddr>>,
    /// While the microphone is held muted behind a question put to the person
    /// (REQ-CON-138), what lifting the hold goes back to. `None` when nothing
    /// holds it.
    mute_hold: std::sync::Mutex<Option<MuteHold>>,
    /// What a session started by a test captures and plays in place of the
    /// devices: the machine the tests run on may have none, and a test must
    /// not open the microphone of the one that has.
    #[cfg(test)]
    stand_ins: std::sync::Mutex<Option<(jamjam::audio_session::InputSignal, std::path::PathBuf)>>,
}

/// What the person had before a question silenced their room (REQ-CON-138).
struct MuteHold {
    /// Whether the microphone was muted
    microphone_muted: bool,
    /// The master volume, in percent. A change made while the hold stands
    /// lands here and is heard when the hold is lifted.
    master_volume: u32,
}

impl StreamingState {
    pub fn new() -> Self {
        Self {
            cmd_tx: Mutex::new(None),
            shared: Shared::new(),
            buffer_size: Mutex::new(jamjam::config::DEFAULT_BUFFER_SIZE),
            // Until a session starts from the saved settings (ADR-013)
            sample_rate: Arc::new(AtomicU32::new(jamjam::config::DEFAULT_SAMPLE_RATE)),
            prepared_socket: Mutex::new(None),
            prepared_addr: Mutex::new(None),
            mute_hold: std::sync::Mutex::new(None),
            #[cfg(test)]
            stand_ins: std::sync::Mutex::new(None),
        }
    }

    /// Mutes the microphone, with the session running or not.
    async fn set_mute(&self, muted: bool) {
        self.shared.is_muted.store(muted, Ordering::SeqCst);
        if self.shared.is_active.load(Ordering::SeqCst) {
            let tx = self.cmd_tx.lock().await;
            if let Some(ref sender) = *tx {
                let _ = sender.send(StreamingCommand::SetMute(muted));
            }
        }
    }

    /// Sets the master volume, in percent of unity gain, with the session
    /// running or not. While a hold stands the room stays silent and the new
    /// volume is heard when the hold is lifted.
    async fn set_master_volume(&self, percent: u32) {
        {
            let mut held = self.mute_hold.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(hold) = held.as_mut() {
                hold.master_volume = percent;
                return;
            }
        }
        self.apply_master_volume(percent).await;
    }

    async fn apply_master_volume(&self, percent: u32) {
        self.shared.master_volume.store(percent, Ordering::SeqCst);
        if self.shared.is_active.load(Ordering::SeqCst) {
            let tx = self.cmd_tx.lock().await;
            if let Some(ref sender) = *tx {
                let _ = sender.send(StreamingCommand::SetMasterVolume(percent as f32 / 100.0));
            }
        }
    }

    /// Holds the microphone muted, and the room silent, while the person is
    /// asked something that they may walk away from: nothing of their room is
    /// sent while the question stands, and since a muted microphone looks the
    /// same as an open one to the person, hearing nothing of the others
    /// keeps them from talking on as if they were heard (REQ-CON-138).
    /// Lifting the hold puts both back as they were before. Holding twice, or
    /// lifting what is not held, does nothing.
    pub async fn hold_mute(&self, hold: bool) {
        let (muted, master_volume) = {
            let mut held = self.mute_hold.lock().unwrap_or_else(|e| e.into_inner());
            if hold {
                if held.is_some() {
                    return;
                }
                *held = Some(MuteHold {
                    microphone_muted: self.shared.is_muted.load(Ordering::SeqCst),
                    master_volume: self.shared.master_volume.load(Ordering::SeqCst),
                });
                (true, 0)
            } else {
                match held.take() {
                    Some(before) => (before.microphone_muted, before.master_volume),
                    None => return,
                }
            }
        };
        self.set_mute(muted).await;
        self.apply_master_volume(master_volume).await;
    }

    /// Sets the volume `peer` is played at, in percent of unity gain (0-200).
    /// Heard on the next frame when there is a link to them, and kept for when
    /// there is.
    pub fn set_peer_volume(&self, peer: Uuid, percent: u32) {
        self.shared.gain_of(peer).set_volume(percent);
    }

    /// Sets where `peer` sits, from -100 (left) to 100 (right).
    pub fn set_peer_pan(&self, peer: Uuid, pan: i32) {
        self.shared.gain_of(peer).set_pan(pan);
    }

    /// Forgets the fader and pan of everyone but `peers`, so one who left
    /// leaves nothing behind.
    pub fn keep_peer_gains(&self, peers: &[Uuid]) {
        self.shared
            .peer_gains
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|peer, _| peers.contains(peer));
    }

    /// Sets the volume of the microphone, in percent of unity gain (0-200).
    pub async fn set_local_volume(&self, percent: u32) {
        let clamped = percent.min(200);
        self.shared.local_volume.store(clamped, Ordering::SeqCst);
        self.send(StreamingCommand::SetLocalVolume(clamped as f32 / 100.0))
            .await;
    }

    /// Sets where the microphone sits, from -100 (left) to 100 (right).
    pub async fn set_local_pan(&self, pan: i32) {
        let clamped = pan.clamp(-100, 100);
        self.shared.local_pan.store(clamped, Ordering::SeqCst);
        self.send(StreamingCommand::SetLocalPan(clamped)).await;
    }

    /// The volume (percent of unity gain) and pan `peer` is played at, or
    /// `None` when nothing is kept for them.
    #[cfg(test)]
    pub(crate) fn peer_gain(&self, peer: Uuid) -> Option<(u32, i32)> {
        let gains = self.shared.peer_gains.read().unwrap();
        gains.get(&peer).map(|gain| (gain.volume(), gain.pan()))
    }

    /// The microphone's volume (percent of unity gain) and pan, as stored.
    #[cfg(test)]
    pub(crate) fn local_gain(&self) -> (u32, i32) {
        (
            self.shared.local_volume.load(Ordering::SeqCst),
            self.shared.local_pan.load(Ordering::SeqCst),
        )
    }

    /// Tells the audio thread, if a session is running; a value set before one
    /// starts is picked up from the stored one.
    async fn send(&self, command: StreamingCommand) {
        if self.shared.is_active.load(Ordering::SeqCst) {
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
        self.shared.is_active.load(Ordering::SeqCst)
    }

    /// Whether a socket is bound and waiting for the next audio session: the
    /// one whose address the room was told.
    pub async fn has_prepared_socket(&self) -> bool {
        self.prepared_addr.lock().await.is_some()
    }

    fn link_reports(&self) -> Vec<(Uuid, Arc<LinkReport>)> {
        self.shared.link_reports()
    }

    /// How the audio link to each participant is doing, in the order the
    /// links were opened.
    pub fn link_healths(&self) -> Vec<(Uuid, LinkHealth)> {
        self.shared.link_healths()
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

/// A session a test started does not outlive the state it was started for.
#[cfg(test)]
impl Drop for StreamingState {
    fn drop(&mut self) {
        if let Ok(tx) = self.cmd_tx.try_lock() {
            if let Some(tx) = tx.as_ref() {
                let _ = tx.send(StreamingCommand::Stop);
            }
        }
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

impl StreamingState {
    /// Mutes the microphone for a room entered by an invite link from another
    /// app, and for the session that starts in it: the person pressed nothing
    /// in this app, so nothing of theirs is sent until they unmute
    /// (REQ-CON-139).
    pub(crate) async fn mute_on_entry(&self) {
        self.shared.muted_at_start.store(true, Ordering::SeqCst);
        self.set_mute(true).await;
    }

    /// The next session starts unmuted again: the room it was muted for was
    /// not entered, or has been left.
    pub(crate) fn forget_mute_at_start(&self) {
        self.shared.muted_at_start.store(false, Ordering::SeqCst);
    }

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
        self.shared.underrun_count.load(Ordering::Relaxed)
    }

    /// Times a link began to reconnect since streaming last started.
    pub(crate) fn reconnects(&self) -> u64 {
        self.shared.reconnect_count.load(Ordering::Relaxed)
    }

    /// Times an established link was given up for silence since streaming
    /// last started.
    pub(crate) fn silence_giveups(&self) -> u64 {
        self.shared.silence_giveups.load(Ordering::Relaxed)
    }

    /// How the session's first link came up. Empty before there is one.
    pub(crate) fn link_snapshot(&self) -> LinkSnapshot {
        self.shared
            .link_facts
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
        self.shared
            .flights
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
        let mut links = self.shared.links.write().unwrap();
        links.retain(|(linked, _)| *linked != peer);
        links.push((peer, report));
    }

    /// Stands in for the audio thread: a link that came up as `facts` says.
    #[cfg(test)]
    pub(crate) fn set_link_facts_for_test(&self, facts: Arc<jamjam::network::LinkFacts>) {
        *self.shared.link_facts.write().unwrap() = Some(facts);
    }

    /// Stands in for the audio thread giving up an established link.
    #[cfg(test)]
    pub(crate) fn add_silence_giveup_for_test(&self) {
        self.shared.silence_giveups.fetch_add(1, Ordering::Relaxed);
    }

    /// Sessions this starts capture `signal` and play into `played`, as the
    /// devices would.
    #[cfg(test)]
    pub(crate) fn stand_in_devices_for_test(
        &self,
        signal: jamjam::audio_session::InputSignal,
        played: std::path::PathBuf,
    ) {
        *self.stand_ins.lock().unwrap() = Some((signal, played));
    }

    /// Stands in for a running session: what it is told arrives on the
    /// returned receiver instead of an audio thread.
    #[cfg(test)]
    pub(crate) async fn attach_session_for_test(&self) -> std_mpsc::Receiver<StreamingCommand> {
        let rx = self.detached_session_for_test().await;
        self.shared.is_active.store(true, Ordering::SeqCst);
        rx
    }

    /// A receiver for commands while no session is running, to show that
    /// nothing is sent to one.
    /// The frame size the running (or last) session was started with.
    #[cfg(test)]
    pub(crate) async fn frame_size_for_test(&self) -> u32 {
        *self.buffer_size.lock().await
    }

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
    /// Only what can change live is here. The frame size and the sample rate
    /// are fixed for the lifetime of the audio engines, so a new one replaces
    /// the session instead (`Plan::new_session`, REQ-GUI-024).
    pub async fn apply_setting(&self, setting: SessionSetting) {
        if !self.shared.is_active.load(Ordering::SeqCst) {
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
    /// The play-out delay, in milliseconds, the latest of those adjustments
    /// left the adjusted link with
    pub adjusted_delay_ms: f32,
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
    /// Peer's capture buffer latency (ms)
    pub capture_buffer_ms: f32,
    /// Peer's playback buffer latency (ms)
    pub playback_buffer_ms: f32,
    /// Peer's current jitter buffer delay (ms)
    pub jitter_buffer_ms: f32,
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

/// Starts the audio session - the devices and the socket the room was told -
/// if none is running, with no link. Done on entering a room, so the
/// microphone is heard (the meter moves) while the person is alone in it.
pub async fn streaming_open(
    state: &StreamingState,
    config_state: &crate::config::ConfigState,
    settings_state: &crate::settings::SettingsState,
    usage: &crate::usage::UsageState,
) -> Result<(), String> {
    if state.is_active() {
        return Ok(());
    }
    start_session(state, config_state, settings_state, usage).await
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
    streaming_open(&state, &config_state, &settings_state, &usage).await?;
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

    let usage_reporter = usage.reporter().clone();

    // Reset state on new connection
    state.shared.start_over();

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

    #[cfg(test)]
    let (input_signal, output_file) = match state.stand_ins.lock().ok().and_then(|s| s.clone()) {
        Some((signal, file)) => (Some(signal), Some(file)),
        None => (None, None),
    };
    #[cfg(not(test))]
    let (input_signal, output_file) = (None, None);

    // Becomes active as the audio thread is spawned, before changes are let through again
    let session = audio_session::start(
        Params {
            prepared_socket,
            input_device_id,
            output_device_id,
            buffer_size,
            preset,
            sample_rate,
            transmit_channels,
            input_channels,
            output_channels,
            input_signal,
            output_file,
            probe: None,
        },
        &state.shared,
        move |result| match result {
            Err(e) => {
                tracing::error!("Audio streaming failed: {}", e);
                crate::usage::record_streaming_failure(&usage_reporter, e);
            }
            Ok(()) => tracing::info!("Audio streaming ended"),
        },
    );
    *state.cmd_tx.lock().await = Some(session.commands);
    drop(changes_held);

    Ok(())
}

/// Stop audio streaming
pub async fn streaming_stop(state: tauri::State<'_, StreamingState>) -> Result<(), String> {
    if let Ok(mut problems) = state.shared.device_problems.write() {
        problems.clear();
    }
    if !state.shared.is_active.load(Ordering::SeqCst) {
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
    while state.shared.is_active.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    if state.shared.is_active.swap(false, Ordering::SeqCst) {
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
    let is_active = state.shared.is_active.load(Ordering::SeqCst);
    let is_muted = state.shared.is_muted.load(Ordering::SeqCst);
    let is_monitoring = state.shared.is_monitoring.load(Ordering::SeqCst);
    let input_level = state.shared.input_level.load(Ordering::SeqCst);
    let output_level = state.shared.output_level.load(Ordering::SeqCst);
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
                    capture_buffer_ms: p.capture_buffer_ms,
                    playback_buffer_ms: p.playback_buffer_ms,
                    jitter_buffer_ms: p.jitter_buffer_ms,
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
            underrun_count: state.shared.underrun_count.load(Ordering::Relaxed),
            delay_adjustments: state.shared.delay_adjustments.load(Ordering::Relaxed),
            adjusted_delay_ms: state.shared.adjusted_delay_us.load(Ordering::Relaxed) as f32
                / 1000.0,
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
            .shared
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
    if !state.shared.is_active.load(Ordering::SeqCst) {
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
    // Can set mute state even when not streaming. A session that starts later
    // starts with it: the person's choice before anyone is linked stands.
    if !state.shared.is_active.load(Ordering::SeqCst) {
        state.shared.muted_at_start.store(muted, Ordering::SeqCst);
    }
    state.set_mute(muted).await;
    Ok(())
}

/// Holds the microphone muted and the room silent while a question is on the
/// screen, or lifts the hold and puts both back as they were (REQ-CON-138).
#[tauri::command]
pub async fn streaming_hold_mute(
    hold: bool,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    state.hold_mute(hold).await;
    Ok(())
}

/// Switch local monitoring: hearing your own input directly, without the
/// network delay (ADR-033). Applies to the running session, if there is one.
#[tauri::command]
pub async fn streaming_set_monitoring(
    enabled: bool,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    state.shared.is_monitoring.store(enabled, Ordering::SeqCst);

    if state.shared.is_active.load(Ordering::SeqCst) {
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
    Ok(state.shared.is_muted.load(Ordering::SeqCst))
}

/// Get current input audio level (0-100)
#[tauri::command]
pub async fn streaming_get_input_level(
    state: tauri::State<'_, StreamingState>,
) -> Result<u32, String> {
    Ok(state.shared.input_level.load(Ordering::SeqCst))
}

/// Set master output volume
/// Volume is 0-200 where 100 = unity gain (1.0x), 200 = 2.0x
#[tauri::command]
pub async fn streaming_set_master_volume(
    volume: u32,
    state: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    state.set_master_volume(volume.min(200)).await;
    Ok(())
}

/// Get master volume (0-200, 100 = unity)
#[tauri::command]
pub async fn streaming_get_master_volume(
    state: tauri::State<'_, StreamingState>,
) -> Result<u32, String> {
    Ok(state.shared.master_volume.load(Ordering::SeqCst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jamjam::audio::{AudioPreset, PlayoutConfig};
    use tauri::Manager;

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_the_mute_is_held_the_microphone_is_muted_until_the_hold_is_lifted() {
        let state = StreamingState::new();
        let sent = state.attach_session_for_test().await;

        state.hold_mute(true).await;
        assert!(state.shared.is_muted.load(Ordering::SeqCst));
        assert!(matches!(
            sent.try_recv(),
            Ok(StreamingCommand::SetMute(true))
        ));

        state.hold_mute(false).await;
        assert!(!state.shared.is_muted.load(Ordering::SeqCst));
        assert!(sent
            .try_iter()
            .any(|c| matches!(c, StreamingCommand::SetMute(false))));
    }

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_the_mute_is_held_the_room_is_silent_until_the_hold_is_lifted() {
        let state = StreamingState::new();
        let sent = state.attach_session_for_test().await;
        state.set_master_volume(80).await;
        sent.try_iter().for_each(drop);

        state.hold_mute(true).await;
        assert_eq!(state.shared.master_volume.load(Ordering::SeqCst), 0);
        assert!(sent
            .try_iter()
            .any(|c| matches!(c, StreamingCommand::SetMasterVolume(v) if v == 0.0)));

        state.hold_mute(false).await;
        assert_eq!(state.shared.master_volume.load(Ordering::SeqCst), 80);
        assert!(sent
            .try_iter()
            .any(|c| matches!(c, StreamingCommand::SetMasterVolume(v) if v == 0.8)));
    }

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_the_volume_is_changed_during_the_hold_the_room_stays_silent_until_it_is_lifted() {
        let state = StreamingState::new();
        state.hold_mute(true).await;

        state.set_master_volume(150).await;
        assert_eq!(state.shared.master_volume.load(Ordering::SeqCst), 0);

        state.hold_mute(false).await;
        assert_eq!(state.shared.master_volume.load(Ordering::SeqCst), 150);
    }

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_the_mute_is_held_twice_the_room_goes_back_to_the_volume_before_the_first() {
        let state = StreamingState::new();
        state.set_master_volume(60).await;

        state.hold_mute(true).await;
        state.hold_mute(true).await;
        state.hold_mute(false).await;

        assert_eq!(state.shared.master_volume.load(Ordering::SeqCst), 60);
    }

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_the_microphone_was_muted_before_the_hold_lifting_it_keeps_it_muted() {
        let state = StreamingState::new();
        state.set_mute(true).await;

        state.hold_mute(true).await;
        state.hold_mute(false).await;

        assert!(state.shared.is_muted.load(Ordering::SeqCst));
    }

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_the_mute_is_held_twice_lifting_it_goes_back_to_what_it_was_before_the_first() {
        let state = StreamingState::new();

        state.hold_mute(true).await;
        state.hold_mute(true).await;
        state.hold_mute(false).await;

        assert!(!state.shared.is_muted.load(Ordering::SeqCst));
    }

    /// Verifies: REQ-CON-138
    #[tokio::test]
    async fn when_nothing_is_held_lifting_the_hold_leaves_the_mute_as_it_is() {
        let state = StreamingState::new();
        state.set_mute(true).await;

        state.hold_mute(false).await;

        assert!(state.shared.is_muted.load(Ordering::SeqCst));
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
        state.shared.is_active.store(true, Ordering::SeqCst);
        let is_active = state.shared.is_active.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            is_active.store(false, Ordering::SeqCst);
        });
        let started = std::time::Instant::now();

        streaming_stop(app.state()).await.unwrap();

        assert!(started.elapsed() >= std::time::Duration::from_millis(400));
        assert!(!state.shared.is_active.load(Ordering::SeqCst));
    }

    fn peer(n: u128) -> Uuid {
        Uuid::from_u128(n)
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

    /// The status says what each participant told of their own audio side, so
    /// the screen can show the layers beyond the network per participant.
    ///
    /// Verifies: REQ-LAT-102
    #[tokio::test]
    async fn the_status_has_the_buffers_each_participant_told_of_their_own_side() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        let state = app.state::<StreamingState>();
        state.set_link_for_test(peer(2), "connected", Some((100, 8.0)));
        state.set_link_for_test(peer(3), "connected", Some((100, 8.0)));
        for (n, capture_ms) in [(2, 0.67_f32), (3, 2.67_f32)] {
            let links = state.shared.links.read().unwrap();
            let (_, report) = links.iter().find(|(p, _)| *p == peer(n)).unwrap();
            *report.latency_info.write().unwrap() = Some(jamjam::network::PeerLatencyInfo {
                capture_buffer_ms: capture_ms,
                playback_buffer_ms: 1.0,
                jitter_buffer_ms: 4.0,
                sample_rate: 48000,
                frame_size: 32,
                codec: "pcm".to_string(),
                channel_count: 2,
                ..Default::default()
            });
        }

        let status = streaming_status(app.state()).await.unwrap();

        let told = |n: u128| {
            status
                .peers
                .iter()
                .find(|p| p.peer_id == peer(n).to_string())
                .and_then(|p| p.peer_audio.clone())
                .expect("they told of their audio")
        };
        assert_eq!(told(2).capture_buffer_ms, 0.67);
        assert_eq!(told(3).capture_buffer_ms, 2.67);
        assert_eq!(told(3).playback_buffer_ms, 1.0);
        assert_eq!(told(3).jitter_buffer_ms, 4.0);
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

    /// A room entered by a link from another app is entered muted, and the
    /// audio session that starts in it starts muted too: the reset a new
    /// session makes must not unmute the person.
    ///
    /// Verifies: REQ-CON-139
    #[tokio::test]
    async fn a_room_is_entered_by_a_link_mutes_the_microphone_and_the_session_that_starts_in_it() {
        let state = StreamingState::new();

        state.mute_on_entry().await;
        assert!(state.shared.is_muted.load(Ordering::SeqCst));
        state.shared.start_over();

        assert!(state.shared.is_muted.load(Ordering::SeqCst));
    }

    /// Verifies: REQ-CON-139
    #[tokio::test]
    async fn the_person_unmutes_before_the_session_starts_the_session_starts_unmuted() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        let state = app.state::<StreamingState>();
        state.mute_on_entry().await;

        streaming_set_mute(false, app.state()).await.unwrap();
        state.shared.start_over();

        assert!(!state.shared.is_muted.load(Ordering::SeqCst));
    }

    /// The room that was muted for was not entered, or has been left: the next
    /// one, entered by typing a code, is not muted for it.
    ///
    /// Verifies: REQ-CON-139
    #[tokio::test]
    async fn the_room_muted_for_is_not_entered_the_next_session_starts_unmuted() {
        let state = StreamingState::new();
        state.mute_on_entry().await;

        state.forget_mute_at_start();
        state.shared.start_over();

        assert!(!state.shared.is_muted.load(Ordering::SeqCst));
    }

    /// A session that is running keeps the mute the person gave it: choosing
    /// while it runs is not a choice for the next one.
    ///
    /// Verifies: REQ-CON-139
    #[tokio::test]
    async fn the_person_mutes_while_a_session_runs_the_next_session_starts_unmuted() {
        let app = tauri::test::mock_app();
        app.manage(StreamingState::new());
        let state = app.state::<StreamingState>();
        state.shared.is_active.store(true, Ordering::SeqCst);

        streaming_set_mute(true, app.state()).await.unwrap();
        assert!(state.shared.is_muted.load(Ordering::SeqCst));
        state.shared.is_active.store(false, Ordering::SeqCst);
        state.shared.start_over();

        assert!(!state.shared.is_muted.load(Ordering::SeqCst));
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
