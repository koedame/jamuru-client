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

use crate::audio::{
    DeviceProblem, FlightRecorder, PeerBufferControl, PeerGain, PeerMeter, PlayoutStats,
    RoomRecorder,
};
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
    /// The receive buffer of each participant: the owner's choice and what a participant
    /// asked of it, whether or not there is a link to them yet (ADR-097)
    pub peer_buffers: Arc<RwLock<HashMap<Uuid, Arc<PeerBufferControl>>>>,
    /// Devices the audio thread could not open, one per direction at most
    pub device_problems: Arc<RwLock<Vec<DeviceProblem>>>,
    /// Buffer underrun count (audio glitches due to CPU/scheduling)
    pub underrun_count: Arc<AtomicU64>,
    /// How many times adaptation has grown the play-out delay this session. A
    /// delay given back on a clean link is not counted: the user is told of
    /// the link getting worse, not of it settling
    pub delay_adjustments: Arc<AtomicU64>,
    /// The play-out delay, in microseconds, that the latest growth left the
    /// grown link with
    pub adjusted_delay_us: Arc<AtomicU32>,
    /// How many times a link dropped and began to reconnect, over every link
    pub reconnect_count: Arc<AtomicU64>,
    /// How many times an established link was given up for silence, over every link
    pub silence_giveups: Arc<AtomicU64>,
    /// How the session's first link came up, once the audio thread has one
    pub link_facts: Arc<RwLock<Option<Arc<LinkFacts>>>>,
    /// What went wrong with the audio received on each link and when. The last session's stay
    /// until the next one starts, so a debug call can still read them.
    pub flights: Arc<Flights>,
    /// How many frames the limiter on what is sent brought down this session (ADR-094)
    pub input_limited_frames: Arc<AtomicU64>,
    /// How many frames the limiter on what is played brought down this session (ADR-094)
    pub output_limited_frames: Arc<AtomicU64>,
    /// Master output volume (0-200, 100 = unity gain)
    pub master_volume: Arc<AtomicU32>,
    /// Local (microphone) input volume (0-200, 100 = unity gain)
    pub local_volume: Arc<AtomicU32>,
    /// Local (microphone) input pan (-100 = full left, 0 = centre, 100 = full right)
    pub local_pan: Arc<AtomicI32>,
    /// Records the room to files. It outlives a session: the session's callbacks are wired to it
    /// as they start, and the session's end ends the recording.
    pub recorder: RoomRecorder,
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
            peer_buffers: Arc::new(RwLock::new(HashMap::new())),
            device_problems: Arc::new(RwLock::new(Vec::new())),
            underrun_count: Arc::new(AtomicU64::new(0)),
            delay_adjustments: Arc::new(AtomicU64::new(0)),
            adjusted_delay_us: Arc::new(AtomicU32::new(0)),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            silence_giveups: Arc::new(AtomicU64::new(0)),
            link_facts: Arc::new(RwLock::new(None)),
            flights: Arc::new(RwLock::new(Vec::new())),
            input_limited_frames: Arc::new(AtomicU64::new(0)),
            output_limited_frames: Arc::new(AtomicU64::new(0)),
            master_volume: Arc::new(AtomicU32::new(100)), // 100 = unity gain
            local_volume: Arc::new(AtomicU32::new(100)),  // 100 = unity gain
            local_pan: Arc::new(AtomicI32::new(0)),       // 0 = centre
            recorder: RoomRecorder::new(),
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
        self.input_limited_frames.store(0, Ordering::SeqCst);
        self.output_limited_frames.store(0, Ordering::SeqCst);
        self.delay_adjustments.store(0, Ordering::SeqCst);
        self.adjusted_delay_us.store(0, Ordering::SeqCst);
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

    /// The receive buffer of `peer`, made as the preset has it the first time it is asked for.
    pub fn buffer_of(&self, peer: Uuid) -> Arc<PeerBufferControl> {
        let mut buffers = self.peer_buffers.write().unwrap_or_else(|e| e.into_inner());
        buffers.entry(peer).or_default().clone()
    }

    /// The participant `from` is now known as `to`: what is kept under their id (the link's
    /// report, the record of what went wrong with their audio, the fader) is kept under the
    /// new one, `gain` being the fader the link plays them through.
    pub fn rename_peer(&self, from: Uuid, to: Uuid, gain: Arc<PeerGain>) {
        if let Ok(mut links) = self.links.write() {
            for (peer, _) in links.iter_mut().filter(|(peer, _)| *peer == from) {
                *peer = to;
            }
        }
        if let Ok(mut flights) = self.flights.write() {
            for (peer, _) in flights.iter_mut().filter(|(peer, _)| *peer == from) {
                *peer = to;
            }
        }
        let mut gains = self.peer_gains.write().unwrap_or_else(|e| e.into_inner());
        gains.remove(&from);
        gains.insert(to, gain);
        let mut buffers = self.peer_buffers.write().unwrap_or_else(|e| e.into_inner());
        if let Some(buffer) = buffers.remove(&from) {
            buffers.insert(to, buffer);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::FlightRecorder;

    /// Verifies: REQ-CON-150
    #[test]
    fn a_participant_is_known_by_a_new_id_the_report_the_flights_and_the_fader_follow_the_id() {
        let shared = Shared::new();
        let (from, to, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let report = Arc::new(LinkReport::default());
        let other_report = Arc::new(LinkReport::default());
        shared
            .links
            .write()
            .unwrap()
            .extend([(from, report.clone()), (other, other_report.clone())]);
        shared
            .flights
            .write()
            .unwrap()
            .push((from, FlightRecorder::new()));
        let fader = shared.gain_of(from);
        fader.set_volume(150);
        fader.set_pan(-30);
        shared.gain_of(other);
        let buffer = shared.buffer_of(from);
        buffer.set_own(crate::audio::OwnBuffer::Fixed { ms10: 200 });

        shared.rename_peer(from, to, fader.clone());

        let links = shared.link_reports();
        assert_eq!(links.len(), 2, "no link is added or lost");
        assert_eq!(links[0].0, to, "the link keeps its place in the order");
        assert!(Arc::ptr_eq(&links[0].1, &report), "and its report");
        assert_eq!(links[1].0, other, "another link is left as it was");
        assert_eq!(shared.flights.read().unwrap()[0].0, to);
        let gains = shared.peer_gains.read().unwrap();
        assert!(!gains.contains_key(&from), "the old id has no fader left");
        let moved = &gains[&to];
        assert_eq!((moved.volume(), moved.pan()), (150, -30));
        assert!(gains.contains_key(&other));
        let buffers = shared.peer_buffers.read().unwrap();
        assert!(
            !buffers.contains_key(&from),
            "the old id has no buffer left"
        );
        assert!(
            Arc::ptr_eq(&buffers[&to], &buffer),
            "the buffer is the one they had"
        );
    }
}
