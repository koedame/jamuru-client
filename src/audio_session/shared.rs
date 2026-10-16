//! What a running audio session publishes and takes, for whoever shows or steers it: the app's
//! IPC layer and the CLI's prompt.
//!
//! The audio thread writes the readings (levels, link reports, device problems, counters); the
//! controls (mute, volumes, pans) are written by the caller and read by the audio thread on
//! its next frame. Every field is an `Arc`, so a clone of [`Shared`] is the same session's.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use uuid::Uuid;

use crate::audio::{DeviceProblem, FlightRecorder, PeerGain, PeerMeter, PlayoutStats};
use crate::network::{
    BandwidthStatus, ConnectionStats, LinkFacts, LinkHealth, PeerLatencyInfo, MAX_PEERS_PER_ROOM,
};

/// How many audio links a session holds at most: one to each of the others in a full room.
pub const MAX_LINKS: usize = MAX_PEERS_PER_ROOM - 1;

/// What the audio thread publishes about each link, by the participant the link is to, in the
/// order the links were opened
pub type LinkReports = RwLock<Vec<(Uuid, Arc<LinkReport>)>>;

/// What went wrong with the audio received on each link and when, by the participant the link
/// is to
pub type Flights = RwLock<Vec<(Uuid, Arc<FlightRecorder>)>>;

/// Connection state and, when it failed, why
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

/// What the audio thread publishes about the link to one participant
#[derive(Default)]
pub struct LinkReport {
    /// Connection statistics, once the link is up
    pub stats: RwLock<Option<ConnectionStats>>,
    /// Measured bandwidth against what the preset needs
    pub bandwidth: RwLock<Option<BandwidthSnapshot>>,
    /// Connection state (ADR-022)
    connection_state: RwLock<ConnectionStateSnapshot>,
    /// Delay their play-out buffer holds, in microseconds
    ///
    /// Integer microseconds rather than a float, so it fits an atomic without
    /// bit-casting. Feeds the latency breakdown shown to the user.
    pub jitter_buffer_delay_us: AtomicU32,
    /// What their app told about its audio (`LatencyInfoMessage`)
    pub latency_info: RwLock<Option<PeerLatencyInfo>>,
    /// What the mix measured of them
    pub meter: Arc<PeerMeter>,
    /// The address the link runs on, once it is up
    pub remote_addr: RwLock<Option<SocketAddr>>,
    /// What their play-out buffer did
    pub playout: RwLock<PlayoutStats>,
    /// The play-out delay in force, after any adaptation, in frames
    pub delay_frames: AtomicU32,
}

impl LinkReport {
    pub fn state(&self) -> ConnectionStateSnapshot {
        self.connection_state.read().ok().and_then(|c| c.clone())
    }

    pub fn set_state(&self, state: &str, error: Option<String>) {
        if let Ok(mut published) = self.connection_state.write() {
            *published = Some((state.to_string(), error));
        }
    }

    pub fn health(&self) -> LinkHealth {
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

/// The state of one audio session, shared between the audio thread and whoever controls it
#[derive(Clone)]
pub struct Shared {
    /// Whether the audio thread runs. Cleared by it as the very last thing it does.
    pub is_active: Arc<AtomicBool>,
    /// Whether the microphone is muted
    pub is_muted: Arc<AtomicBool>,
    /// The mute the next session starts with: what the room was entered with, or what the
    /// person chose since while no session ran
    pub muted_at_start: Arc<AtomicBool>,
    /// Whether local monitoring is on (ADR-033)
    pub is_monitoring: Arc<AtomicBool>,
    /// Current input audio level (0-100, RMS normalized)
    pub input_level: Arc<AtomicU32>,
    /// Current output audio level (0-100, for the master meter)
    pub output_level: Arc<AtomicU32>,
    /// The audio links of the running session, in the order they were opened, with what the
    /// audio thread publishes about each
    pub links: Arc<LinkReports>,
    /// The links that ended during this session, as they looked when they ended
    pub ended: Arc<LinkReports>,
    /// Where the fader and pan of each participant stand, whether or not there is a link to
    /// them yet. The output callback reads them as it mixes.
    pub peer_gains: Arc<RwLock<HashMap<Uuid, Arc<PeerGain>>>>,
    /// Devices the audio thread could not open, one per direction at most
    pub device_problems: Arc<RwLock<Vec<DeviceProblem>>>,
    /// Buffer underrun count (audio glitches due to CPU/scheduling)
    pub underrun_count: Arc<AtomicU64>,
    /// How many times adaptation has moved the play-out delay this session
    pub delay_adjustments: Arc<AtomicU64>,
    /// How many times a link dropped and began to reconnect, over every link
    pub reconnect_count: Arc<AtomicU64>,
    /// How many times an established link was given up for silence, over every link
    pub silence_giveups: Arc<AtomicU64>,
    /// How the session's first link came up, once the audio thread has one
    pub link_facts: Arc<RwLock<Option<Arc<LinkFacts>>>>,
    /// What went wrong with the audio received on each link and when. The last session's stay
    /// until the next one starts, so a debug call can still read them.
    pub flights: Arc<Flights>,
    /// Master output volume (0-200, 100 = unity gain)
    pub master_volume: Arc<AtomicU32>,
    /// Local (microphone) input volume (0-200, 100 = unity gain)
    pub local_volume: Arc<AtomicU32>,
    /// Local (microphone) input pan (-100 = full left, 0 = centre, 100 = full right)
    pub local_pan: Arc<AtomicI32>,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self {
            is_active: Arc::new(AtomicBool::new(false)),
            is_muted: Arc::new(AtomicBool::new(false)),
            muted_at_start: Arc::new(AtomicBool::new(false)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            input_level: Arc::new(AtomicU32::new(0)),
            output_level: Arc::new(AtomicU32::new(0)),
            links: Arc::new(RwLock::new(Vec::new())),
            ended: Arc::new(RwLock::new(Vec::new())),
            peer_gains: Arc::new(RwLock::new(HashMap::new())),
            device_problems: Arc::new(RwLock::new(Vec::new())),
            underrun_count: Arc::new(AtomicU64::new(0)),
            delay_adjustments: Arc::new(AtomicU64::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            silence_giveups: Arc::new(AtomicU64::new(0)),
            link_facts: Arc::new(RwLock::new(None)),
            flights: Arc::new(RwLock::new(Vec::new())),
            master_volume: Arc::new(AtomicU32::new(100)), // 100 = unity gain
            local_volume: Arc::new(AtomicU32::new(100)),  // 100 = unity gain
            local_pan: Arc::new(AtomicI32::new(0)),       // 0 = centre
        }
    }

    /// Puts the readings and the controls of a session back to where a new one starts. The
    /// faders of the participants are the mixer's, which sets them as they change.
    pub fn start_over(&self) {
        self.is_muted
            .store(self.muted_at_start.load(Ordering::SeqCst), Ordering::SeqCst);
        // Off at the start of every session: with speakers, a monitored microphone can feed
        // back, so hearing yourself is something the user asks for.
        self.is_monitoring.store(false, Ordering::SeqCst);
        self.input_level.store(0, Ordering::SeqCst);
        self.output_level.store(0, Ordering::SeqCst);
        self.underrun_count.store(0, Ordering::SeqCst);
        self.delay_adjustments.store(0, Ordering::SeqCst);
        self.reconnect_count.store(0, Ordering::SeqCst);
        self.silence_giveups.store(0, Ordering::SeqCst);
        if let Ok(mut facts) = self.link_facts.write() {
            *facts = None;
        }
        if let Ok(mut flights) = self.flights.write() {
            flights.clear();
        }
        if let Ok(mut links) = self.links.write() {
            links.clear();
        }
        if let Ok(mut ended) = self.ended.write() {
            ended.clear();
        }
        // The master volume goes back to unity gain
        self.master_volume.store(100, Ordering::SeqCst);
        if let Ok(mut problems) = self.device_problems.write() {
            problems.clear();
        }
    }

    /// The fader and pan of `peer`, made at unity gain and centre the first time they are
    /// asked for.
    pub fn gain_of(&self, peer: Uuid) -> Arc<PeerGain> {
        let mut gains = self.peer_gains.write().unwrap_or_else(|e| e.into_inner());
        gains.entry(peer).or_default().clone()
    }

    /// The links of the running session, in the order they were opened
    pub fn link_reports(&self) -> Vec<(Uuid, Arc<LinkReport>)> {
        self.links
            .read()
            .map(|links| links.clone())
            .unwrap_or_default()
    }

    /// The links that ended during this session, in the order they ended
    pub fn ended_reports(&self) -> Vec<(Uuid, Arc<LinkReport>)> {
        self.ended
            .read()
            .map(|links| links.clone())
            .unwrap_or_default()
    }

    /// How the audio link to each participant is doing, in the order the links were opened.
    pub fn link_healths(&self) -> Vec<(Uuid, LinkHealth)> {
        self.link_reports()
            .into_iter()
            .map(|(peer, report)| (peer, report.health()))
            .collect()
    }
}
