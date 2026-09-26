//! Connection management for P2P audio streaming

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::time::interval;
use tracing::{debug, info, trace, warn};

use crate::audio::{create_codec, AudioCodec, CodecConfig, CodecType};
use crate::protocol::{LatencyInfoMessage, LatencyPing, LatencyPong, Packet, PacketType};

use super::bandwidth::UDP_IP_OVERHEAD_BYTES;
use super::error::NetworkError;
use super::fec::{FecDecoder, FecEncoder, FecPacket};
use super::link_facts::LinkFacts;
use super::quality::ConnectionQuality;
use super::sequence_tracker::SequenceTracker;
use super::transport::UdpTransport;

/// Number of RTT samples to keep for averaging
const RTT_SAMPLE_COUNT: usize = 10;

/// Maximum pending pings before discarding old ones
const MAX_PENDING_PINGS: usize = 10;

/// Connection statistics
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    /// Round-trip time in milliseconds, or `None` before the first sample
    /// arrives (REQ-LAT-130). `0.0` is a real measurement, not a placeholder,
    /// so it cannot double as "unmeasured".
    pub rtt_ms: Option<f32>,
    /// Packet loss rate (0.0 - 1.0)
    pub packet_loss_rate: f32,
    /// Jitter in milliseconds (RTT variation)
    pub jitter_ms: f32,
    /// Total bytes sent
    pub bytes_sent: u64,
    /// Total bytes received
    pub bytes_received: u64,
    /// Total packets sent
    pub packets_sent: u64,
    /// Total packets received
    pub packets_received: u64,
    /// Audio packets that arrived, counting each arrival (cumulative)
    pub audio_packets_received: u64,
    /// Audio packets the peer's sequence numbers show as missing (cumulative).
    /// A late packet takes its loss back, so this can go down.
    pub audio_packets_lost: u64,
    /// Audio packets rebuilt from FEC because they never arrived, or `None`
    /// when this link sends no FEC
    pub fec_recovered: Option<u64>,
    /// Connection uptime in seconds
    pub uptime_seconds: u64,
}

impl ConnectionStats {
    /// Bytes received as the link carried them: every packet with the UDP and
    /// IP headers the socket does not count. What a bandwidth requirement, which
    /// counts them, is compared with.
    pub fn wire_bytes_received(&self) -> u64 {
        self.bytes_received + self.packets_received * UDP_IP_OVERHEAD_BYTES
    }

    /// How usable this connection currently is, or `None` before the first
    /// RTT sample arrives (REQ-LAT-121, REQ-LAT-130).
    ///
    /// Derived rather than stored, so it can never disagree with the RTT and
    /// loss figures it is computed from. Withheld rather than defaulted to
    /// `Poor` or `Good`: either would misrepresent a link nobody has measured
    /// yet.
    pub fn quality(&self) -> Option<ConnectionQuality> {
        self.rtt_ms
            .map(|rtt_ms| ConnectionQuality::classify(rtt_ms, self.packet_loss_rate))
    }
}

/// RTT measurement state
#[derive(Debug)]
struct RttMeasurement {
    /// Current smoothed RTT estimate (ms), or `None` before the first sample
    rtt_ms: Option<f32>,
    /// RTT jitter / variation (ms)
    jitter_ms: f32,
    /// Pending ping sequences with sent timestamps (monotonic instant)
    pending_pings: HashMap<u32, Instant>,
    /// Recent RTT samples for averaging
    rtt_samples: VecDeque<f32>,
    /// Next ping sequence number
    next_ping_seq: u32,
    /// Monotonic time reference for timestamps
    time_reference: Instant,
}

impl Default for RttMeasurement {
    fn default() -> Self {
        Self {
            rtt_ms: None,
            jitter_ms: 0.0,
            pending_pings: HashMap::new(),
            rtt_samples: VecDeque::with_capacity(RTT_SAMPLE_COUNT),
            next_ping_seq: 0,
            time_reference: Instant::now(),
        }
    }
}

impl RttMeasurement {
    /// Get current time in microseconds since reference
    fn now_us(&self) -> u64 {
        self.time_reference.elapsed().as_micros() as u64
    }

    /// Create a ping message and record the send time
    fn create_ping(&mut self) -> LatencyPing {
        let seq = self.next_ping_seq;
        self.next_ping_seq = self.next_ping_seq.wrapping_add(1);

        // Clean up old pending pings if too many
        if self.pending_pings.len() >= MAX_PENDING_PINGS {
            // Remove the oldest entry
            if let Some(&oldest_seq) = self.pending_pings.keys().min() {
                self.pending_pings.remove(&oldest_seq);
            }
        }

        self.pending_pings.insert(seq, Instant::now());

        LatencyPing {
            sent_time_us: self.now_us(),
            ping_sequence: seq,
        }
    }

    /// Process a pong response and update RTT statistics
    fn process_pong(&mut self, pong: &LatencyPong) {
        if let Some(sent_time) = self.pending_pings.remove(&pong.ping_sequence) {
            let rtt = sent_time.elapsed().as_secs_f32() * 1000.0; // Convert to ms

            // Update RTT samples
            if self.rtt_samples.len() >= RTT_SAMPLE_COUNT {
                self.rtt_samples.pop_front();
            }
            self.rtt_samples.push_back(rtt);

            // Calculate smoothed RTT (exponential moving average)
            let alpha = 0.125; // Standard TCP-like smoothing factor
            let smoothed = match self.rtt_ms {
                None => rtt,
                Some(current) => (1.0 - alpha) * current + alpha * rtt,
            };
            self.rtt_ms = Some(smoothed);

            // Calculate jitter (RTT variation)
            let beta = 0.25;
            let diff = (rtt - smoothed).abs();
            self.jitter_ms = (1.0 - beta) * self.jitter_ms + beta * diff;

            trace!(
                "RTT updated: rtt={:.2}ms, jitter={:.2}ms (sample={:.2}ms)",
                smoothed,
                self.jitter_ms,
                rtt
            );
        }
    }
}

/// Peer latency information received from remote peer
#[derive(Debug, Clone, Default)]
pub struct PeerLatencyInfo {
    /// Peer's capture buffer latency (ms)
    pub capture_buffer_ms: f32,
    /// Peer's playback buffer latency (ms)
    pub playback_buffer_ms: f32,
    /// Peer's encode latency (ms)
    pub encode_ms: f32,
    /// Peer's decode latency (ms)
    pub decode_ms: f32,
    /// Peer's current jitter buffer delay (ms)
    pub jitter_buffer_ms: f32,
    /// Peer's frame size (samples)
    pub frame_size: u32,
    /// Peer's sample rate (Hz)
    pub sample_rate: u32,
    /// Peer's codec name
    pub codec: String,
    /// Peer's transmit channel count (1 = mono, 2 = stereo)
    pub channel_count: u8,
}

impl From<LatencyInfoMessage> for PeerLatencyInfo {
    fn from(msg: LatencyInfoMessage) -> Self {
        Self {
            capture_buffer_ms: msg.capture_buffer_ms,
            playback_buffer_ms: msg.playback_buffer_ms,
            encode_ms: msg.encode_ms,
            decode_ms: msg.decode_ms,
            jitter_buffer_ms: msg.jitter_buffer_ms,
            frame_size: msg.frame_size,
            sample_rate: msg.sample_rate,
            codec: msg.codec,
            channel_count: msg.channel_count,
        }
    }
}

/// Connection state
///
/// State machine for P2P connections with ICE/NAT traversal support.
///
/// ```text
/// [*] --> Disconnected
/// Disconnected --> Connecting: connect()
/// Connecting --> GatheringCandidates: ICE start
/// GatheringCandidates --> CheckingConnectivity: candidates ready
/// CheckingConnectivity --> Connected: ICE success
/// CheckingConnectivity --> Failed: ICE failed
/// Connected --> Reconnecting: connection lost
/// Reconnecting --> Connected: reconnect success
/// Reconnecting --> Failed: timeout
/// Connected --> Disconnected: disconnect()
/// Failed --> Disconnected: reset()
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ConnectionState {
    /// Not connected
    #[default]
    Disconnected = 0,
    /// Initiating connection
    Connecting = 1,
    /// Gathering ICE candidates
    GatheringCandidates = 2,
    /// Checking connectivity with candidates
    CheckingConnectivity = 3,
    /// Successfully connected
    Connected = 4,
    /// Attempting to reconnect after connection loss
    Reconnecting = 5,
    /// Connection failed
    Failed = 6,
}

impl ConnectionState {
    /// Convert from u8 value
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Disconnected,
            1 => Self::Connecting,
            2 => Self::GatheringCandidates,
            3 => Self::CheckingConnectivity,
            4 => Self::Connected,
            5 => Self::Reconnecting,
            6 => Self::Failed,
            _ => Self::Disconnected,
        }
    }

    /// Check if the connection is in a connected state
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }

    /// Check if the connection is in progress
    pub fn is_connecting(&self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::GatheringCandidates | Self::CheckingConnectivity
        )
    }

    /// Check if the connection can send/receive data
    pub fn can_transmit(&self) -> bool {
        matches!(self, Self::Connected | Self::Reconnecting)
    }
}

/// Callback for received audio data
/// How audio is encoded on the wire, and whether FEC is sent (ADR-021)
#[derive(Debug, Clone)]
pub struct AudioEncodingConfig {
    /// Codec to use. Must be available in this build.
    pub codec_type: CodecType,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Channel count
    pub channels: u16,
    /// Samples per channel per frame
    pub frame_size: u32,
    /// Opus bitrate in bits per second; ignored for PCM
    pub bitrate: u32,
    /// FEC group size, or `None` to disable FEC
    ///
    /// Must not exceed the receiver's jitter buffer depth, or recovered packets
    /// arrive after they were due (ADR-021).
    pub fec_group_size: Option<usize>,
}

impl Default for AudioEncodingConfig {
    fn default() -> Self {
        Self {
            codec_type: CodecType::Pcm,
            sample_rate: 48_000,
            channels: 1,
            frame_size: 128,
            bitrate: 0,
            fec_group_size: None,
        }
    }
}

/// When a silent link counts as lost, and when it counts as gone
///
/// UDP has no connection to re-establish, so recovery means continuing to probe
/// until packets flow again - the keep-alive loop already does that. What this
/// configures is when to tell the user, which `connection.feature` specifies:
/// a 3-second outage recovers on its own, a 10-second one asks the user
/// (REQ-CON-109, REQ-CON-110).
#[derive(Debug, Clone, Copy)]
pub struct ReconnectConfig {
    /// How often keep-alives are sent
    ///
    /// Keep-alives are what prove the link is alive when no audio is flowing, so
    /// this sets the granularity of every threshold below it.
    pub keep_alive_interval: Duration,
    /// Silence after which the connection is considered lost and reconnection
    /// starts. Shorter than the 3 seconds REQ-CON-109 describes, so that outage
    /// is detected well inside it.
    pub detect_after: Duration,
    /// Silence after which automatic recovery gives up and the user is asked
    pub give_up_after: Duration,
    /// How often liveness is checked
    pub check_interval: Duration,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            keep_alive_interval: Duration::from_secs(1),
            detect_after: Duration::from_secs(2),
            give_up_after: Duration::from_secs(10),
            check_interval: Duration::from_millis(250),
        }
    }
}

impl ReconnectConfig {
    /// Smallest ratio of `detect_after` to `keep_alive_interval` that does not flap
    ///
    /// A healthy link is silent between keep-alives. If loss is declared sooner
    /// than two of them, an idle-but-working link oscillates between Connected
    /// and Reconnecting, which is worse than not detecting loss at all.
    const MIN_DETECT_INTERVALS: u32 = 2;

    /// Return a config whose thresholds cannot produce flapping
    ///
    /// `detect_after` is raised to at least two keep-alive intervals, and
    /// `give_up_after` to at least `detect_after`, so a caller cannot accidentally
    /// configure a connection that reports loss on a working link.
    pub fn validated(self) -> Self {
        let keep_alive_interval = if self.keep_alive_interval.is_zero() {
            Duration::from_secs(1)
        } else {
            self.keep_alive_interval
        };

        let detect_after = self
            .detect_after
            .max(keep_alive_interval * Self::MIN_DETECT_INTERVALS);

        Self {
            keep_alive_interval,
            detect_after,
            give_up_after: self.give_up_after.max(detect_after),
            check_interval: if self.check_interval.is_zero() {
                Duration::from_millis(250)
            } else {
                self.check_interval
            },
        }
    }
}

/// Callback invoked when the connection state changes
///
/// Lets a caller react to loss and recovery - resetting a jitter buffer, or
/// prompting the user once automatic recovery has given up.
pub type StateChangeCallback = Box<dyn Fn(ConnectionState) + Send + Sync + 'static>;

/// Callback invoked for each received audio packet
///
/// Arguments are `(sequence, payload, timestamp)`. The sequence number is
/// required to reorder packets and detect loss, so a receiver can feed a jitter
/// buffer (ADR-020). The payload is passed by value, so a receiver that keeps
/// it does not have to copy it out of the transport.
///
/// `participant_id` and the FEC recovery flags described in
/// docs-spec/api/network.md are not supplied yet; see that document.
pub type AudioCallback = Box<dyn Fn(u32, Vec<u8>, u32) + Send + Sync + 'static>;

/// Callback for received peer latency info
pub type LatencyInfoCallback = Box<dyn Fn(PeerLatencyInfo) + Send + Sync + 'static>;

/// A P2P connection to a remote peer
pub struct Connection {
    transport: Arc<UdpTransport>,
    remote_addr: SocketAddr,
    state: Arc<AtomicU8>,
    /// Last error message that caused connection failure (if any)
    last_error: Arc<std::sync::Mutex<Option<String>>>,
    /// Sequence for control packets (keep-alive, latency info).
    sequence: AtomicU32,
    /// Sequence for audio packets, kept apart from `sequence` so audio is
    /// numbered without gaps: the receiver counts a gap as loss, and derives
    /// the FEC group from the number (ADR-021).
    audio_sequence: AtomicU32,
    packets_sent: Arc<AtomicU64>,
    packets_received: Arc<AtomicU64>,
    fec_recovered: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    last_received: Arc<std::sync::Mutex<Instant>>,
    audio_callback: Option<Arc<AudioCallback>>,
    /// Encoder for outgoing audio. `None` until `set_audio_encoding` is called,
    /// in which case PCM is used inline for backward compatibility.
    encoder: Option<Arc<std::sync::Mutex<Box<dyn AudioCodec>>>>,
    /// FEC encoder for outgoing audio, when the preset enables FEC
    fec_encoder: Option<Arc<std::sync::Mutex<FecEncoder>>>,
    /// FEC decoder for incoming audio, shared with the receive loop
    fec_decoder: Option<Arc<std::sync::Mutex<FecDecoder>>>,
    /// FEC group size in use, needed to map sequence numbers to groups
    fec_group_size: Option<usize>,
    /// Reconnection thresholds
    reconnect_config: ReconnectConfig,
    /// Notified when the connection state changes
    state_change_callback: Option<Arc<StateChangeCallback>>,
    /// Liveness monitor task
    liveness_handle: Option<tokio::task::JoinHandle<()>>,
    /// Tracks received audio sequence numbers so loss can be reported
    ///
    /// Shared with the receive loop, which is where sequence numbers arrive.
    sequence_tracker: Arc<std::sync::Mutex<SequenceTracker>>,
    receive_handle: Option<tokio::task::JoinHandle<()>>,
    keepalive_handle: Option<tokio::task::JoinHandle<()>>,
    /// RTT measurement state
    rtt_measurement: Arc<RwLock<RttMeasurement>>,
    /// Peer latency information (received from remote peer)
    peer_latency_info: Arc<RwLock<Option<PeerLatencyInfo>>>,
    /// Callback for when peer latency info is received
    latency_info_callback: Option<Arc<LatencyInfoCallback>>,
    /// Connection start time for uptime tracking
    connection_start: Arc<std::sync::Mutex<Option<Instant>>>,
    /// How the link came up, for the usage log
    link_facts: Arc<LinkFacts>,
}

impl Connection {
    /// Create a new connection (not yet connected)
    pub async fn new(local_addr: &str) -> Result<Self, NetworkError> {
        Self::from_transport(UdpTransport::bind(local_addr).await?)
    }

    /// Create a connection over an already-bound standard socket.
    ///
    /// Lets a caller bind early to learn and advertise its port, then hand the
    /// socket to the runtime that will drive the audio path. Must be called
    /// from within that runtime - see [`UdpTransport::from_std`] and ADR-026.
    pub fn from_std(std_socket: std::net::UdpSocket) -> Result<Self, NetworkError> {
        Self::from_transport(UdpTransport::from_std(std_socket)?)
    }

    fn from_transport(transport: UdpTransport) -> Result<Self, NetworkError> {
        Ok(Self {
            transport: Arc::new(transport),
            remote_addr: "0.0.0.0:0".parse().unwrap(),
            state: Arc::new(AtomicU8::new(ConnectionState::Disconnected as u8)),
            last_error: Arc::new(std::sync::Mutex::new(None)),
            sequence: AtomicU32::new(0),
            audio_sequence: AtomicU32::new(0),
            packets_sent: Arc::new(AtomicU64::new(0)),
            packets_received: Arc::new(AtomicU64::new(0)),
            fec_recovered: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
            last_received: Arc::new(std::sync::Mutex::new(Instant::now())),
            audio_callback: None,
            encoder: None,
            fec_encoder: None,
            fec_decoder: None,
            fec_group_size: None,
            reconnect_config: ReconnectConfig::default().validated(),
            state_change_callback: None,
            liveness_handle: None,
            sequence_tracker: Arc::new(std::sync::Mutex::new(SequenceTracker::new())),
            receive_handle: None,
            keepalive_handle: None,
            rtt_measurement: Arc::new(RwLock::new(RttMeasurement::default())),
            peer_latency_info: Arc::new(RwLock::new(None)),
            latency_info_callback: None,
            connection_start: Arc::new(std::sync::Mutex::new(None)),
            link_facts: Arc::new(LinkFacts::new()),
        })
    }

    /// Get the local address
    pub fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }

    /// The address currently in use for the remote peer: the candidate
    /// [`Self::connect_with_candidates`] selected, or the single address
    /// [`Self::connect`] was given.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// How the link came up (route, connect time, first audio). Stays valid
    /// after the connection moves elsewhere.
    pub fn link_facts(&self) -> Arc<LinkFacts> {
        self.link_facts.clone()
    }

    /// The audio socket, for gathering address candidates through it before
    /// [`Self::connect`] starts the receive loop.
    pub fn socket(&self) -> &Arc<tokio::net::UdpSocket> {
        self.transport.socket()
    }

    /// Connect to a remote peer
    pub async fn connect(&mut self, remote_addr: SocketAddr) -> Result<(), NetworkError> {
        if self.is_connected() {
            return Err(NetworkError::AlreadyConnected);
        }

        let started = Instant::now();
        self.remote_addr = remote_addr;
        self.set_state(ConnectionState::Connecting);
        info!("Connecting to {}", remote_addr);

        // Send initial keep-alive to establish connection
        let packet = Packet::keep_alive(self.next_sequence());
        self.transport.send_to(&packet, remote_addr).await?;

        // Record connection start time
        if let Ok(mut start) = self.connection_start.lock() {
            *start = Some(Instant::now());
        }

        self.link_facts.link_up(remote_addr, false, started);
        self.set_state(ConnectionState::Connected);
        self.start_receive_loop();
        self.start_keepalive_loop();
        self.start_liveness_monitor();

        info!("Connected to {}", remote_addr);
        Ok(())
    }

    /// Wait for a peer to reach this socket, then connect back to it
    ///
    /// For a listener that does not know its peer's address in advance
    /// (`jamjam host`). Whatever arrives first names the peer: a joining side
    /// opens with a keep-alive, so nothing it cares about is consumed here.
    pub async fn accept(&mut self) -> Result<SocketAddr, NetworkError> {
        if self.is_connected() {
            return Err(NetworkError::AlreadyConnected);
        }

        let (_, peer) = self.transport.recv_raw().await?;
        info!("Peer {} reached us", peer);
        self.connect(peer).await?;
        Ok(peer)
    }

    /// Connect to a remote peer using multiple address candidates (Happy Eyeballs style)
    ///
    /// This function tries multiple candidates in parallel and uses the first one that responds.
    /// Candidates are tried in order of priority, with a small delay between starting each attempt.
    pub async fn connect_with_candidates(
        &mut self,
        candidates: &[SocketAddr],
    ) -> Result<(), NetworkError> {
        if self.is_connected() {
            return Err(NetworkError::AlreadyConnected);
        }

        if candidates.is_empty() {
            return Err(NetworkError::NoCandidates);
        }

        // If only one candidate, use simple connect
        if candidates.len() == 1 {
            return self.connect(candidates[0]).await;
        }

        let started = Instant::now();
        self.set_state(ConnectionState::CheckingConnectivity);
        info!("Checking connectivity with {} candidates", candidates.len());

        // Try candidates with Happy Eyeballs approach:
        // - Send probes to all candidates
        // - Use the first one that responds
        const PROBE_TIMEOUT: Duration = Duration::from_millis(1000);
        const CANDIDATE_DELAY: Duration = Duration::from_millis(50);

        // Send probes to all candidates with small delays between each
        for (i, &addr) in candidates.iter().enumerate() {
            let packet = Packet::keep_alive(self.next_sequence());
            if let Err(e) = self.transport.send_to(&packet, addr).await {
                debug!("Failed to send probe to candidate {}: {}", addr, e);
                continue;
            }
            debug!("Sent connectivity probe to candidate {} ({})", i, addr);

            // Small delay before next candidate (Happy Eyeballs style)
            if i < candidates.len() - 1 {
                tokio::time::sleep(CANDIDATE_DELAY).await;
            }
        }

        // Wait for first response
        let transport = self.transport.clone();
        let timeout = tokio::time::timeout(PROBE_TIMEOUT, async {
            loop {
                match transport.recv_raw().await {
                    Ok((buf, from_addr)) => {
                        // Check if response is from one of our candidates
                        if candidates.contains(&from_addr) {
                            if let Some(packet) = Packet::from_bytes(&buf) {
                                if matches!(packet.packet_type, PacketType::KeepAlive) {
                                    return Some(from_addr);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Error receiving probe response: {}", e);
                        return None;
                    }
                }
            }
        })
        .await;

        match timeout {
            Ok(Some(selected_addr)) => {
                info!("Selected candidate: {} (first to respond)", selected_addr);
                self.remote_addr = selected_addr;

                // Record connection start time
                if let Ok(mut start) = self.connection_start.lock() {
                    *start = Some(Instant::now());
                }

                self.link_facts.link_up(selected_addr, true, started);
                self.set_state(ConnectionState::Connected);
                self.start_receive_loop();
                self.start_keepalive_loop();
                self.start_liveness_monitor();

                Ok(())
            }
            Ok(None) => {
                self.set_state(ConnectionState::Failed);
                Err(NetworkError::ConnectionFailed(
                    "No candidate responded".to_string(),
                ))
            }
            Err(_) => {
                // Timeout - try fallback to first candidate
                warn!("No candidate responded in time, falling back to first candidate");
                self.set_state(ConnectionState::Connecting);
                self.remote_addr = candidates[0];

                // Record connection start time
                if let Ok(mut start) = self.connection_start.lock() {
                    *start = Some(Instant::now());
                }

                self.link_facts.link_up(candidates[0], false, started);
                self.set_state(ConnectionState::Connected);
                self.start_receive_loop();
                self.start_keepalive_loop();
                self.start_liveness_monitor();

                Ok(())
            }
        }
    }

    /// Disconnect from the remote peer
    pub fn disconnect(&mut self) {
        self.set_state(ConnectionState::Disconnected);

        if let Some(handle) = self.receive_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.keepalive_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.liveness_handle.take() {
            handle.abort();
        }

        info!("Disconnected from {}", self.remote_addr);
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.state().is_connected()
    }

    /// Get current connection state
    pub fn state(&self) -> ConnectionState {
        ConnectionState::from_u8(self.state.load(Ordering::SeqCst))
    }

    /// Set connection state
    fn set_state(&self, state: ConnectionState) {
        // Clear last_error when transitioning to a non-failed state
        if state != ConnectionState::Failed {
            if let Ok(mut err) = self.last_error.lock() {
                *err = None;
            }
        }
        let previous = self.state.swap(state as u8, Ordering::SeqCst);

        // Only an actual transition is worth reporting, so a caller can act on
        // the edge rather than filtering repeats itself.
        if previous != state as u8 {
            if let Some(ref callback) = self.state_change_callback {
                callback(state);
            }
        }
    }

    /// Set connection to failed state with error information
    #[allow(dead_code)]
    fn set_failed(&self, error: &NetworkError) {
        if let Ok(mut err) = self.last_error.lock() {
            *err = Some(error.to_string());
        }
        self.state
            .store(ConnectionState::Failed as u8, Ordering::SeqCst);
    }

    /// Get the last error message that caused connection failure
    ///
    /// Returns `None` if the connection has not failed or if no error was recorded.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    /// Configure how outgoing audio is encoded, and whether FEC is sent
    ///
    /// Must be called before [`Self::connect`]: the receive loop captures the
    /// FEC decoder when it starts. Without this call audio is sent as PCM with
    /// no FEC.
    ///
    /// # Errors
    /// Returns [`NetworkError::Configuration`] if the codec is unavailable in
    /// this build. Callers should resolve availability first - see
    /// [`crate::audio::AudioPreset::resolved_codec`].
    pub fn set_audio_encoding(&mut self, config: AudioEncodingConfig) -> Result<(), NetworkError> {
        let codec_config = CodecConfig {
            codec_type: config.codec_type,
            sample_rate: config.sample_rate,
            channels: config.channels,
            frame_size: config.frame_size,
            bitrate: config.bitrate,
        };
        let codec = create_codec(&codec_config)
            .map_err(|e| NetworkError::Configuration(format!("Audio codec: {}", e)))?;

        self.encoder = Some(Arc::new(std::sync::Mutex::new(codec)));

        match config.fec_group_size {
            Some(group) if group >= 2 => {
                self.fec_encoder = Some(Arc::new(std::sync::Mutex::new(
                    FecEncoder::with_group_size(group),
                )));
                self.fec_decoder = Some(Arc::new(std::sync::Mutex::new(
                    FecDecoder::with_group_size(group),
                )));
                self.fec_group_size = Some(group);
            }
            _ => {
                self.fec_encoder = None;
                self.fec_decoder = None;
                self.fec_group_size = None;
            }
        }

        Ok(())
    }

    /// Set callback for received audio data
    ///
    /// The callback receives `(sequence, payload, timestamp)`. See [`AudioCallback`].
    pub fn set_audio_callback<F>(&mut self, callback: F)
    where
        F: Fn(u32, Vec<u8>, u32) + Send + Sync + 'static,
    {
        self.audio_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Set callback for received peer latency info
    pub fn set_latency_info_callback<F>(&mut self, callback: F)
    where
        F: Fn(PeerLatencyInfo) + Send + Sync + 'static,
    {
        self.latency_info_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Set the reconnection thresholds
    ///
    /// Must be called before [`Self::connect`]; the monitor captures them when it
    /// starts.
    pub fn set_reconnect_config(&mut self, config: ReconnectConfig) {
        self.reconnect_config = config.validated();
    }

    /// Set the callback invoked when the connection state changes
    ///
    /// Must be called before [`Self::connect`].
    pub fn set_state_change_callback<F>(&mut self, callback: F)
    where
        F: Fn(ConnectionState) + Send + Sync + 'static,
    {
        self.state_change_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Resume probing after automatic recovery gave up
    ///
    /// `connection.feature` has the user choose whether to retry after a long
    /// outage (REQ-CON-110); this is what that choice calls. Returns an error if
    /// the connection was deliberately closed rather than lost.
    pub fn reconnect(&mut self) -> Result<(), NetworkError> {
        match self.state() {
            ConnectionState::Disconnected => Err(NetworkError::NotConnected),
            ConnectionState::Connected => Ok(()),
            _ => {
                // Treat the link as alive again for the purposes of the
                // give-up deadline, so the user gets a full attempt.
                if let Ok(mut last) = self.last_received.lock() {
                    *last = Instant::now();
                }
                self.reset_receive_state();
                self.set_state(ConnectionState::Reconnecting);
                self.start_liveness_monitor();
                self.start_keepalive_loop();
                Ok(())
            }
        }
    }

    /// Discard receive state that cannot survive an outage
    ///
    /// Groups that were in flight will never complete, and a stale group could
    /// pair a pre-outage packet with a post-outage FEC packet and "recover"
    /// nonsense (ADR-022).
    fn reset_receive_state(&self) {
        if let Some(ref decoder) = self.fec_decoder {
            if let Ok(mut decoder) = decoder.lock() {
                decoder.reset();
            }
        }
        // Packets missed during the outage are not loss the user can act on, and
        // leaving them counted would pin the quality classification to Poor for
        // the rest of the session.
        if let Ok(mut tracker) = self.sequence_tracker.lock() {
            tracker.reset();
        }
    }

    /// Send latency info to the remote peer
    pub async fn send_latency_info(&self, info: &LatencyInfoMessage) -> Result<(), NetworkError> {
        if !self.state().can_transmit() {
            return Err(NetworkError::NotConnected);
        }

        let packet = Packet::latency_info(self.next_sequence(), info);
        self.transport.send_to(&packet, self.remote_addr).await?;

        debug!("Sent latency info to {}", self.remote_addr);
        Ok(())
    }

    /// Get the current RTT estimate in milliseconds, or `None` before the
    /// first sample arrives (REQ-LAT-130)
    pub fn rtt_ms(&self) -> Option<f32> {
        self.rtt_measurement.read().rtt_ms
    }

    /// Get the current jitter estimate in milliseconds
    pub fn jitter_ms(&self) -> f32 {
        self.rtt_measurement.read().jitter_ms
    }

    /// Get the peer's latency information (if received)
    pub fn peer_latency_info(&self) -> Option<PeerLatencyInfo> {
        self.peer_latency_info.read().clone()
    }

    /// Send audio data to the remote peer
    pub async fn send_audio(&self, data: &[f32], timestamp: u32) -> Result<(), NetworkError> {
        if !self.state().can_transmit() {
            return Err(NetworkError::NotConnected);
        }

        // Encode with the configured codec. Without `set_audio_encoding` this
        // falls back to inline PCM so existing callers keep working.
        let bytes = match self.encoder {
            Some(ref encoder) => {
                let mut codec = encoder
                    .lock()
                    .map_err(|_| NetworkError::Configuration("Codec lock poisoned".into()))?;
                codec
                    .encode(data)
                    .map_err(|e| NetworkError::Configuration(format!("Audio encode: {}", e)))?
            }
            None => data.iter().flat_map(|&s| s.to_le_bytes()).collect(),
        };

        // The receiver files a packet under group `sequence / group_size`, and
        // the encoder groups packets in the order it is given them. The two
        // agree only if every audio sequence reaches the encoder, in order - so
        // the number is taken under the encoder's lock, before anything that
        // could fail.
        let (sequence, generated) = match self.fec_encoder {
            Some(ref fec_encoder) => {
                let mut encoder = fec_encoder
                    .lock()
                    .map_err(|_| NetworkError::Configuration("FEC lock poisoned".into()))?;
                let sequence = self.next_audio_sequence();
                (sequence, encoder.add_packet(&bytes))
            }
            None => (self.next_audio_sequence(), None),
        };

        let packet = Packet::audio(sequence, timestamp, bytes);
        let packet_bytes = packet.to_bytes();
        let len = packet_bytes.len() as u64;

        self.transport.send_to(&packet, self.remote_addr).await?;

        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(len, Ordering::Relaxed);

        // FEC covers the group this packet completes.
        if let Some(fec) = generated {
            let fec_packet = Packet::fec(sequence, timestamp, fec.to_bytes());
            let fec_len = fec_packet.to_bytes().len() as u64;
            self.transport
                .send_to(&fec_packet, self.remote_addr)
                .await?;
            self.bytes_sent.fetch_add(fec_len, Ordering::Relaxed);
            trace!("Sent FEC packet for group {}", fec.group_sequence);
        }

        Ok(())
    }

    /// Send a pre-built packet to an arbitrary address
    ///
    /// `send_audio` assigns sequence numbers itself, which makes it impossible to
    /// reproduce a gap. This exists so loss handling can be exercised with an
    /// exact sequence pattern.
    pub async fn send_raw_to(&self, packet: &Packet, addr: SocketAddr) -> Result<(), NetworkError> {
        self.transport.send_to(packet, addr).await
    }

    /// Get connection statistics
    pub fn stats(&self) -> ConnectionStats {
        let rtt = self.rtt_measurement.read();
        let uptime = self
            .connection_start
            .lock()
            .ok()
            .and_then(|start| start.map(|s| s.elapsed().as_secs()))
            .unwrap_or(0);

        let (packet_loss_rate, audio_packets_received, audio_packets_lost) = self
            .sequence_tracker
            .lock()
            .map(|tracker| {
                (
                    tracker.loss_rate(),
                    tracker.packets_received(),
                    tracker.packets_lost(),
                )
            })
            .unwrap_or((0.0, 0, 0));

        ConnectionStats {
            rtt_ms: rtt.rtt_ms,
            packet_loss_rate,
            jitter_ms: rtt.jitter_ms,
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            packets_received: self.packets_received.load(Ordering::Relaxed),
            audio_packets_received,
            audio_packets_lost,
            fec_recovered: self
                .fec_decoder
                .as_ref()
                .map(|_| self.fec_recovered.load(Ordering::Relaxed)),
            uptime_seconds: uptime,
        }
    }

    fn next_sequence(&self) -> u32 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    fn next_audio_sequence(&self) -> u32 {
        self.audio_sequence.fetch_add(1, Ordering::Relaxed)
    }

    fn start_receive_loop(&mut self) {
        let transport = self.transport.clone();
        let state = self.state.clone();
        let last_received = self.last_received.clone();
        let packets_received = self.packets_received.clone();
        let fec_recovered = self.fec_recovered.clone();
        let bytes_received = self.bytes_received.clone();
        let audio_callback = self.audio_callback.clone();
        let fec_decoder = self.fec_decoder.clone();
        let fec_group_size = self.fec_group_size;
        let sequence_tracker = self.sequence_tracker.clone();
        let rtt_measurement = self.rtt_measurement.clone();
        let peer_latency_info = self.peer_latency_info.clone();
        let latency_info_callback = self.latency_info_callback.clone();
        let link_facts = self.link_facts.clone();
        let remote_addr = self.remote_addr;
        let sequence = Arc::new(AtomicU32::new(1_000_000)); // Separate sequence for pong responses

        let handle = tokio::spawn(async move {
            let (mut rx, _recv_handle) = transport.clone().start_receive_loop();

            while let Some((packet, _addr)) = rx.recv().await {
                let current_state = ConnectionState::from_u8(state.load(Ordering::SeqCst));
                if !current_state.can_transmit() {
                    break;
                }

                *last_received.lock().unwrap() = Instant::now();
                packets_received.fetch_add(1, Ordering::Relaxed);
                bytes_received.fetch_add(packet.payload.len() as u64 + 12, Ordering::Relaxed);

                match packet.packet_type {
                    PacketType::Audio => {
                        link_facts.audio_received();

                        // Loss is measured from the gaps in the audio sequence.
                        // Without this `ConnectionStats::packet_loss_rate` would
                        // be a constant, and the quality classification that
                        // reads it could never report loss (REQ-CON-023).
                        if let Ok(mut tracker) = sequence_tracker.lock() {
                            tracker.record(packet.sequence);
                        }

                        // Register the packet with FEC before handing it on, so
                        // the group is complete when its FEC packet arrives.
                        if let (Some(ref decoder), Some(group_size)) =
                            (&fec_decoder, fec_group_size)
                        {
                            if let Ok(mut decoder) = decoder.lock() {
                                let group = packet.sequence / group_size as u32;
                                let index = (packet.sequence % group_size as u32) as usize;
                                decoder.add_packet(group, index, &packet.payload);
                            }
                        }

                        if let Some(ref callback) = audio_callback {
                            // Move the payload out: the receiver stores it in a
                            // jitter buffer, so a copy here would be wasted.
                            callback(packet.sequence, packet.payload, packet.timestamp);
                        }
                    }
                    PacketType::Fec => {
                        let recovered = match (&fec_decoder, fec_group_size) {
                            (Some(decoder), Some(group_size)) => {
                                FecPacket::from_bytes(&packet.payload).and_then(|fec| {
                                    let group = fec.group_sequence;
                                    decoder
                                        .lock()
                                        .ok()
                                        .and_then(|mut decoder| decoder.add_fec(fec))
                                        .map(|recovered| {
                                            let sequence = group * group_size as u32
                                                + recovered.packet_index as u32;
                                            (sequence, recovered.data)
                                        })
                                })
                            }
                            _ => None,
                        };

                        if let Some((sequence, data)) = recovered {
                            trace!("FEC recovered packet seq={}", sequence);
                            fec_recovered.fetch_add(1, Ordering::Relaxed);
                            if let Some(ref callback) = audio_callback {
                                // Timestamp is not recoverable from FEC; the
                                // receive path keys off sequence (ADR-021).
                                callback(sequence, data, 0);
                            }
                        }
                    }
                    PacketType::KeepAlive => {
                        debug!("Received keep-alive");
                    }
                    PacketType::LatencyPing => {
                        // Respond with pong
                        if let Some(ping) = LatencyPing::from_bytes(&packet.payload) {
                            let pong = LatencyPong {
                                original_sent_time_us: ping.sent_time_us,
                                ping_sequence: ping.ping_sequence,
                            };
                            let pong_packet = Packet::latency_pong(
                                sequence.fetch_add(1, Ordering::Relaxed),
                                &pong,
                            );
                            if let Err(e) = transport.send_to(&pong_packet, remote_addr).await {
                                warn!("Failed to send latency pong: {}", e);
                            }
                            trace!("Responded to latency ping seq={}", ping.ping_sequence);
                        }
                    }
                    PacketType::LatencyPong => {
                        // Update RTT measurement
                        if let Some(pong) = LatencyPong::from_bytes(&packet.payload) {
                            rtt_measurement.write().process_pong(&pong);
                        }
                    }
                    PacketType::LatencyInfo => {
                        // Store peer's latency info
                        if let Some(info) = LatencyInfoMessage::from_bytes(&packet.payload) {
                            let peer_info: PeerLatencyInfo = info.into();
                            debug!(
                                "Received peer latency info: capture={:.2}ms, playback={:.2}ms, jitter={:.2}ms",
                                peer_info.capture_buffer_ms,
                                peer_info.playback_buffer_ms,
                                peer_info.jitter_buffer_ms
                            );
                            if let Some(ref callback) = latency_info_callback {
                                callback(peer_info.clone());
                            }
                            *peer_latency_info.write() = Some(peer_info);
                        }
                    }
                    _ => {}
                }
            }
        });

        self.receive_handle = Some(handle);
    }

    /// Watch for a link that has gone quiet, and for one that came back
    ///
    /// UDP gives no disconnect event, so silence is the only signal. Recovery
    /// needs no handshake: the keep-alive loop keeps probing throughout, which is
    /// also what refreshes a NAT mapping that expired during the outage
    /// (ADR-022).
    fn start_liveness_monitor(&mut self) {
        if let Some(handle) = self.liveness_handle.take() {
            handle.abort();
        }

        let state = self.state.clone();
        let last_received = self.last_received.clone();
        let config = self.reconnect_config;
        let fec_decoder = self.fec_decoder.clone();
        let sequence_tracker_for_monitor = self.sequence_tracker.clone();
        let state_change_callback = self.state_change_callback.clone();
        let last_error = self.last_error.clone();
        let remote_addr = self.remote_addr;

        let handle = tokio::spawn(async move {
            let mut ticker = interval(config.check_interval);

            loop {
                ticker.tick().await;

                let current = ConnectionState::from_u8(state.load(Ordering::SeqCst));
                if matches!(
                    current,
                    ConnectionState::Disconnected | ConnectionState::Failed
                ) {
                    break;
                }

                let silence = match last_received.lock() {
                    Ok(last) => last.elapsed(),
                    Err(_) => continue,
                };

                let next = if silence >= config.give_up_after {
                    // Automatic recovery has had long enough. Hand the decision
                    // to the user (REQ-CON-110).
                    if let Ok(mut err) = last_error.lock() {
                        *err = Some(format!(
                            "No packets from {} for {:.0}s",
                            remote_addr,
                            silence.as_secs_f32()
                        ));
                    }
                    Some(ConnectionState::Failed)
                } else if silence >= config.detect_after {
                    (current == ConnectionState::Connected).then_some(ConnectionState::Reconnecting)
                } else {
                    // Packets are flowing. If we had given up on the link,
                    // it is usable again.
                    (current == ConnectionState::Reconnecting).then_some(ConnectionState::Connected)
                };

                let Some(next) = next else { continue };

                if next == ConnectionState::Connected {
                    // Groups that spanned the outage can never complete, and
                    // mixing across it would fabricate audio.
                    if let Some(ref decoder) = fec_decoder {
                        if let Ok(mut decoder) = decoder.lock() {
                            decoder.reset();
                        }
                    }
                    if let Ok(mut tracker) = sequence_tracker_for_monitor.lock() {
                        tracker.reset();
                    }
                    info!("Reconnected to {}", remote_addr);
                } else if next == ConnectionState::Reconnecting {
                    warn!(
                        "No packets from {} for {:.1}s, reconnecting",
                        remote_addr,
                        silence.as_secs_f32()
                    );
                } else {
                    warn!(
                        "Giving up on {} after {:.0}s of silence",
                        remote_addr,
                        silence.as_secs_f32()
                    );
                }

                // Mirrors `set_state`, which the monitor cannot reach from here.
                let previous = state.swap(next as u8, Ordering::SeqCst);
                if previous != next as u8 {
                    if let Some(ref callback) = state_change_callback {
                        callback(next);
                    }
                }

                if next == ConnectionState::Failed {
                    break;
                }
            }
        });

        self.liveness_handle = Some(handle);
    }

    fn start_keepalive_loop(&mut self) {
        let transport = self.transport.clone();
        let state = self.state.clone();
        let remote_addr = self.remote_addr;
        let sequence = AtomicU32::new(0);
        let rtt_measurement = self.rtt_measurement.clone();
        let keep_alive_interval = self.reconnect_config.keep_alive_interval;

        let handle = tokio::spawn(async move {
            let mut interval = interval(keep_alive_interval);

            loop {
                interval.tick().await;

                let current_state = ConnectionState::from_u8(state.load(Ordering::SeqCst));
                if !current_state.can_transmit() {
                    break;
                }

                // Send keep-alive
                let packet = Packet::keep_alive(sequence.fetch_add(1, Ordering::Relaxed));
                if let Err(e) = transport.send_to(&packet, remote_addr).await {
                    warn!("Failed to send keep-alive: {}", e);
                }

                // Send latency ping for RTT measurement
                let ping = rtt_measurement.write().create_ping();
                let ping_packet =
                    Packet::latency_ping(sequence.fetch_add(1, Ordering::Relaxed), &ping);
                if let Err(e) = transport.send_to(&ping_packet, remote_addr).await {
                    warn!("Failed to send latency ping: {}", e);
                }
                trace!("Sent latency ping seq={}", ping.ping_sequence);
            }
        });

        self.keepalive_handle = Some(handle);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connection_creation() {
        let conn = Connection::new("127.0.0.1:0").await.unwrap();
        assert!(!conn.is_connected());
        assert!(conn.local_addr().port() > 0);
    }

    #[tokio::test]
    async fn test_connection_stats_initial() {
        let conn = Connection::new("127.0.0.1:0").await.unwrap();
        let stats = conn.stats();
        assert_eq!(stats.packets_sent, 0);
        assert_eq!(stats.packets_received, 0);
    }

    #[tokio::test]
    async fn test_connect_with_candidates_empty() {
        let mut conn = Connection::new("127.0.0.1:0").await.unwrap();
        let result = conn.connect_with_candidates(&[]).await;

        assert!(result.is_err());
        match result {
            Err(NetworkError::NoCandidates) => {}
            _ => panic!("Expected NoCandidates error"),
        }
    }

    #[tokio::test]
    async fn test_connect_with_candidates_already_connected() {
        let mut conn1 = Connection::new("127.0.0.1:0").await.unwrap();
        let conn2 = Connection::new("127.0.0.1:0").await.unwrap();

        // First connection
        conn1.connect(conn2.local_addr()).await.unwrap();
        assert!(conn1.is_connected());

        // Try to connect again
        let candidates = vec![conn2.local_addr()];
        let result = conn1.connect_with_candidates(&candidates).await;

        assert!(result.is_err());
        match result {
            Err(NetworkError::AlreadyConnected) => {}
            _ => panic!("Expected AlreadyConnected error"),
        }
    }

    #[tokio::test]
    async fn test_connect_with_single_candidate() {
        // Single candidate should fall back to regular connect
        let mut conn1 = Connection::new("127.0.0.1:0").await.unwrap();
        let conn2 = Connection::new("127.0.0.1:0").await.unwrap();

        let candidates = vec![conn2.local_addr()];
        conn1.connect_with_candidates(&candidates).await.unwrap();

        assert!(conn1.is_connected());
    }

    #[tokio::test]
    async fn test_connect_with_multiple_candidates_loopback() {
        // Test with multiple loopback candidates
        let mut conn1 = Connection::new("127.0.0.1:0").await.unwrap();
        let conn2 = Connection::new("127.0.0.1:0").await.unwrap();

        // Create fake candidates, only the real one will respond
        let candidates = vec![
            "127.0.0.1:59999".parse().unwrap(), // Fake - won't respond
            conn2.local_addr(),                 // Real - will respond
        ];

        // This should succeed by finding the working candidate
        conn1.connect_with_candidates(&candidates).await.unwrap();

        assert!(conn1.is_connected());
    }

    /// Verifies: REQ-TEL-015
    #[tokio::test]
    async fn when_a_candidate_answers_the_link_facts_say_the_route_was_confirmed() {
        let mut conn1 = Connection::new("127.0.0.1:0").await.unwrap();
        let mut conn2 = Connection::new("127.0.0.1:0").await.unwrap();
        // conn2 opens with a keep-alive, which conn1's probe wait takes as the answer.
        conn2.connect(conn1.local_addr()).await.unwrap();
        let candidates = vec!["127.0.0.1:59999".parse().unwrap(), conn2.local_addr()];

        conn1.connect_with_candidates(&candidates).await.unwrap();

        let snapshot = conn1.link_facts().snapshot();
        assert_eq!(conn1.remote_addr(), conn2.local_addr());
        assert_eq!(snapshot.route, Some(super::super::LinkRoute::Loopback));
        assert_eq!(snapshot.route_confirmed, Some(true));
        assert!(snapshot.connect_ms.is_some());
        assert_eq!(snapshot.first_audio_ms, None);
    }

    /// Verifies: REQ-TEL-015
    #[tokio::test]
    async fn when_no_candidate_answers_the_link_facts_say_the_route_was_not_confirmed() {
        let mut conn1 = Connection::new("127.0.0.1:0").await.unwrap();
        let candidates = vec![
            "127.0.0.1:59998".parse().unwrap(),
            "127.0.0.1:59999".parse().unwrap(),
        ];

        conn1.connect_with_candidates(&candidates).await.unwrap();

        let snapshot = conn1.link_facts().snapshot();
        assert_eq!(snapshot.route, Some(super::super::LinkRoute::Loopback));
        assert_eq!(snapshot.route_confirmed, Some(false));
        assert!(
            snapshot.connect_ms.is_some_and(|ms| ms >= 900),
            "the fallback waits out the probe timeout: {snapshot:?}"
        );
    }

    /// Verifies: REQ-TEL-015
    #[tokio::test]
    async fn when_audio_arrives_the_link_facts_have_a_first_audio_time() {
        let mut conn1 = Connection::new("127.0.0.1:0").await.unwrap();
        let mut conn2 = Connection::new("127.0.0.1:0").await.unwrap();
        conn1.connect(conn2.local_addr()).await.unwrap();
        conn2.connect(conn1.local_addr()).await.unwrap();
        assert_eq!(conn1.link_facts().snapshot().first_audio_ms, None);

        conn2.send_audio(&[0.0f32; 64], 0).await.unwrap();
        for _ in 0..50 {
            if conn1.link_facts().snapshot().first_audio_ms.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(conn1.link_facts().snapshot().first_audio_ms.is_some());
    }

    /// A `detect_after` below two keep-alive intervals would declare loss on a
    /// link that is merely idle, so it must be raised rather than honoured.
    ///
    /// Verifies: REQ-CON-022
    #[test]
    fn reconnect_config_cannot_be_configured_to_flap() {
        let flapping = ReconnectConfig {
            keep_alive_interval: Duration::from_secs(1),
            // Shorter than a single keep-alive: a healthy link looks dead.
            detect_after: Duration::from_millis(100),
            give_up_after: Duration::from_millis(50),
            check_interval: Duration::from_millis(10),
        }
        .validated();

        assert!(
            flapping.detect_after >= flapping.keep_alive_interval * 2,
            "detect_after {:?} must span at least two keep-alives",
            flapping.detect_after
        );
        assert!(
            flapping.give_up_after >= flapping.detect_after,
            "giving up before detecting loss makes no sense"
        );

        // Zero intervals would spin or divide the schedule by nothing.
        let degenerate = ReconnectConfig {
            keep_alive_interval: Duration::ZERO,
            detect_after: Duration::ZERO,
            give_up_after: Duration::ZERO,
            check_interval: Duration::ZERO,
        }
        .validated();
        assert!(!degenerate.keep_alive_interval.is_zero());
        assert!(!degenerate.check_interval.is_zero());
        assert!(degenerate.detect_after >= degenerate.keep_alive_interval * 2);

        // A sane configuration is left alone.
        let sane = ReconnectConfig::default().validated();
        assert_eq!(sane.detect_after, Duration::from_secs(2));
        assert_eq!(sane.give_up_after, Duration::from_secs(10));
    }

    /// Before any pong arrives, RTT must stay unmeasured rather than default
    /// to a real-looking `0.0` - `0.0` is a value the link could actually
    /// report, so it cannot double as "nothing measured yet".
    ///
    /// Verifies: REQ-LAT-130
    #[test]
    fn rtt_measurement_starts_unmeasured_and_records_the_first_sample() {
        let mut measurement = RttMeasurement::default();
        assert_eq!(
            measurement.rtt_ms, None,
            "a fresh measurement has not observed a pong yet"
        );

        let ping = measurement.create_ping();
        measurement.process_pong(&LatencyPong {
            original_sent_time_us: ping.sent_time_us,
            ping_sequence: ping.ping_sequence,
        });

        assert!(
            measurement.rtt_ms.is_some(),
            "the first pong must record a sample instead of leaving None"
        );
    }

    /// What the link carried counts the UDP and IP headers of every packet, or
    /// a stream at its preset's requirement (which counts them) would read as
    /// short of it.
    ///
    /// Verifies: REQ-NET-024
    #[test]
    fn wire_bytes_add_the_udp_and_ip_headers_of_every_packet() {
        let stats = ConnectionStats {
            bytes_received: 1_000,
            packets_received: 10,
            ..Default::default()
        };

        assert_eq!(stats.wire_bytes_received(), 1_000 + 10 * 28);
    }

    /// `ConnectionStats::quality()` must withhold judgment until RTT has been
    /// measured, and classify normally once it has (the same 0.0-boundary
    /// case `classification_matches_the_specified_bands` in `quality.rs`
    /// covers, but reached through the `Option` gate this struct adds).
    ///
    /// Verifies: REQ-LAT-121, REQ-LAT-130
    #[test]
    fn connection_stats_quality_waits_for_the_first_rtt_sample() {
        let unmeasured = ConnectionStats {
            rtt_ms: None,
            packet_loss_rate: 0.0,
            ..Default::default()
        };
        assert_eq!(
            unmeasured.quality(),
            None,
            "an unmeasured link must not be reported as good, fair, or poor"
        );

        let measured = ConnectionStats {
            rtt_ms: Some(10.0),
            ..unmeasured
        };
        assert_eq!(measured.quality(), Some(ConnectionQuality::Good));
    }
}
