//! Connection management for P2P audio streaming

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use tokio::time::interval;
use tracing::{debug, info, trace, warn};

use crate::audio::{create_codec, AudioCodec, CodecConfig, CodecType, PcmCodec};
use crate::protocol::{
    LatencyInfoMessage, LatencyPing, LatencyPong, Packet, PacketType, PongWall, HEADER_SIZE,
};

use super::bandwidth::{samples_per_audio_packet, UDP_IP_OVERHEAD_BYTES};
use super::clock::now_unix_us;
use super::encryption::{LinkIdentity, LinkSecurity, Opened, SecureLink};
use super::error::NetworkError;
use super::fec::{FecDecoder, FecEncoder, FecPacket, RecoveredPacket};
use super::link_facts::{
    route_handicap, route_preference, LinkFacts, LinkRoute, RouteOverride, RouteReason,
};
use super::port_prediction::PortPrediction;
use super::quality::ConnectionQuality;
use super::sequence_tracker::SequenceTracker;
use super::signaling::usable_candidates;
use super::transport::UdpTransport;
use super::udp_mux::UdpMux;

/// How long the connectivity check waits for the first answer after the last probe went out
const PROBE_TIMEOUT: Duration = Duration::from_millis(1000);

/// How long the connectivity check waits between probing one candidate and the next
const CANDIDATE_DELAY: Duration = Duration::from_millis(50);

/// How many times the connectivity check probes each candidate. One round trip is a single
/// sample of a path that jitters (a relayed Tailscale path swings between 43 and 187 ms),
/// so a route is judged by the median of several.
const PROBE_ROUNDS: usize = 5;

/// How long the connectivity check waits between one round of probes and the next
const ROUND_INTERVAL: Duration = Duration::from_millis(80);

/// How many probes the connectivity check sends to a candidate, beyond the scheduled
/// rounds, because the peer's probe came from it while none of ours was answered. Behind a
/// NAT that lets in only what we have sent to, ours were dropped until the peer's own
/// first probe opened its side, and the scheduled rounds are over by then.
const TRIGGERED_PROBES: usize = 3;

/// How many times a probe that could not be sent is sent again, and how long apart. A send that
/// fails for want of room in the socket's buffer is likely to go through a moment later.
const SEND_RETRIES: usize = 3;
const SEND_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// How long the keep-alive goes to the peer's other addresses, once the peer has gone
/// unheard long enough to doubt the one in use. An address the peer published is not
/// proof it is the peer's, so the probing cannot go on for as long as the silence does.
const PROBE_OTHERS_FOR: Duration = Duration::from_secs(30);

/// When a link whose peer has not been heard at all sends again, and in which order
///
/// A NAT that keeps what reaches it before its own side has sent anything to that address (a
/// router that accepts what is addressed to itself, and records it as a flow) gives the first
/// packet its side then sends to that address another port than the one the peer was told. The
/// peer keeps sending to the old port, which keeps the record alive, so sending on does not
/// help: nothing gets through, however long both ends go on. It does when both ends stop for
/// the time such a record lasts, and the end behind that NAT then sends first. Which end that
/// is cannot be told, so the two take turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PunchSchedule {
    /// How long after the check found nothing the first quiet begins
    pub settle: Duration,
    /// How long both ends send nothing, so that what the peer's packets left in a NAT runs out
    /// (a Linux NAT keeps a flow nobody answered for 30 seconds, and removes it within a minute
    /// after that; until it does, a new flow from the same port gets the port the old one had)
    pub quiet: Duration,
    /// How long the end that is second waits after the quiet: longer than the two ends can be
    /// apart in starting, so that the first is first
    pub second_waits: Duration,
    /// How long the packets go to every address of the peer after that, before the next quiet
    pub window: Duration,
}

impl Default for PunchSchedule {
    fn default() -> Self {
        Self {
            settle: Duration::from_secs(5),
            quiet: Duration::from_secs(100),
            second_waits: Duration::from_secs(2),
            window: Duration::from_secs(6),
        }
    }
}

/// Number of RTT samples to keep for averaging
const RTT_SAMPLE_COUNT: usize = 10;

/// Maximum pending pings before discarding old ones
const MAX_PENDING_PINGS: usize = 10;

/// The longest a ping that has not come back counts as the round trip of the link
///
/// A round trip that is growing is not in the samples until it ends, so a ping still out
/// is the only sign of it. Capped so that a peer that never answers is still given up on.
const UNANSWERED_PING_CAP: Duration = Duration::from_millis(2500);

/// How often our key is sent while the peer has not shown it has it
const KEY_EXCHANGE_INTERVAL: Duration = Duration::from_millis(250);

/// How long a packet takes in each direction, from the sender's clock to the receiver's
///
/// Each is the difference of two clocks, so each carries the gap between them: `up_ms` is too
/// large by it and `down_ms` too small by it. Their sum is the round trip whatever the gap,
/// and their difference is the real difference plus twice the gap, so it is known only as far
/// as the clocks are known to agree. Either can come out negative when the receiver's clock is
/// behind.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OneWayDelays {
    /// This computer to the peer
    pub up_ms: f32,
    /// The peer to this computer
    pub down_ms: f32,
}

/// Connection statistics
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    /// Round-trip time in milliseconds, or `None` before the first sample
    /// arrives (REQ-LAT-130). `0.0` is a real measurement, not a placeholder,
    /// so it cannot double as "unmeasured".
    pub rtt_ms: Option<f32>,
    /// The two ways of the round trip timed by the clocks of both ends, or `None` before the
    /// first answer that carried them (REQ-LAT-133)
    pub one_way: Option<OneWayDelays>,
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
    /// When a packet last arrived, `None` while none has (REQ-CON-162). An instant rather than
    /// an age, so a reader of the published statistics knows how long ago it was at the moment
    /// it asks, not at the moment they were taken.
    pub last_received_at: Option<Instant>,
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
    /// Whether what this link carries is encrypted
    pub security: LinkSecurity,
    /// Packets turned away because they were forged, repeated or not encrypted
    pub packets_refused: u64,
    /// Samples sent that did not fit the audio format and were clipped (cumulative):
    /// the input went past 0 dBFS
    pub samples_clipped: u64,
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

/// Where outgoing packets go, and the peer's other addresses
///
/// The address picked while connecting only shows that the peer's packets
/// reach us from there. Whether ours reach the peer is known only once it
/// answers a ping, so the route can move to an address the peer is heard
/// from when it has not answered (REQ-CON-114).
#[derive(Debug)]
struct Route {
    current: SocketAddr,
    /// The peer's other addresses, in the order they were offered, with
    /// whether the last send to each went out
    others: Vec<(SocketAddr, bool)>,
    /// When `current` was taken up, or the peer last answered a ping
    answered_at: Instant,
    /// The ports guessed for a peer none of whose offered addresses answered: it is behind a NAT
    /// that gives each destination a port of its own (ADR-115). What comes from one of them is
    /// the peer's, and the route follows an answer from there.
    predicted: Option<PortPrediction>,
    /// Until when the keep-alive goes to the peer's other addresses whatever the silence, while
    /// the sending starts over after a quiet ([`PunchSchedule`])
    reprobe_until: Option<Instant>,
    /// The checks sent to addresses the peer was heard from that it did not offer. The route
    /// moves to one only when the answer to its check comes back from there ([`PathCheck`]).
    checks: Vec<PathCheck>,
}

/// A check sent to an address a packet came from that is not one of the peer's
///
/// The source address of a datagram proves nothing: a packet the peer sent can be sent again
/// from anywhere. So the route moves to a new address only after a sealed probe carrying a
/// token nobody could have guessed was sent there and the sealed answer to it came back from
/// there (the way a QUIC endpoint validates a new path). Only the peer holds the keys that
/// seal the answer, and the token ties the answer to this check.
#[derive(Debug)]
struct PathCheck {
    addr: SocketAddr,
    token: u32,
    sent_at: Instant,
}

/// How many checks may be out at once. A packet from an address that would be the
/// fifth is not checked until one has been answered or has expired.
const MAX_PATH_CHECKS: usize = 4;
/// How long after a check went out to an address it is sent there again, if it was not answered
const PATH_CHECK_RETRY: Duration = Duration::from_secs(1);
/// How long an unanswered check stays valid: the answer must come back within this
const PATH_CHECK_LIFETIME: Duration = Duration::from_secs(5);

/// Whether the keep-alive goes to the peer's other addresses too, when the peer has
/// not answered for `silent_for` and is doubted after `after`
fn probes_others(silent_for: Duration, after: Duration) -> bool {
    silent_for >= after && silent_for < after + PROBE_OTHERS_FOR
}

/// Where a link goes when no candidate answered the check: the first public address, else the
/// first one that could be sent to
///
/// A private address answers at once when the peer is on the same network, so one that did not
/// answer is not the way to the peer; going to it leaves a peer on another network unheard.
fn fallback_candidate(candidates: &[SocketAddr], sendable: impl Fn(usize) -> bool) -> SocketAddr {
    let pick = |wanted: &dyn Fn(SocketAddr) -> bool| {
        candidates
            .iter()
            .enumerate()
            .find_map(|(i, &addr)| (sendable(i) && wanted(addr)).then_some(addr))
    };
    pick(&|addr| LinkRoute::of(addr) == LinkRoute::Public)
        .or_else(|| pick(&|_| true))
        .unwrap_or(candidates[0])
}

impl Route {
    fn new(current: SocketAddr, others: Vec<(SocketAddr, bool)>) -> Self {
        Self {
            current,
            others,
            answered_at: Instant::now(),
            predicted: None,
            reprobe_until: None,
            checks: Vec::new(),
        }
    }

    /// Whether the sending that starts over after a quiet is still on
    fn reprobing(&self) -> bool {
        self.reprobe_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Whether `addr` is one of the peer's addresses
    fn knows(&self, addr: SocketAddr) -> bool {
        addr == self.current
            || self.others.iter().any(|&(other, _)| other == addr)
            || self.guessed(addr)
    }

    /// Whether `addr` is one of the ports guessed for the peer
    fn guessed(&self, addr: SocketAddr) -> bool {
        self.predicted.as_ref().is_some_and(|p| p.covers(addr))
    }

    /// Whether `token` is what a probe to `addr` carried, among the ones sent to guessed ports
    fn is_answer_from_guessed(&self, token: u32, addr: SocketAddr) -> bool {
        self.predicted
            .as_ref()
            .is_some_and(|p| p.covers(addr) && p.is_token_for(token, addr))
    }

    /// The peer answered a ping, so what we send reaches it
    fn answered(&mut self) {
        self.answered_at = Instant::now();
    }

    /// Starts the wait for the peer's first answer `after` from now instead of at once
    ///
    /// No ping goes out before the keys are agreed, which takes a round trip, so on a
    /// slow link the peer cannot be blamed for silence in that time.
    fn start_waiting_for_an_answer_in(&mut self, after: Duration) {
        self.answered_at = Instant::now() + after;
    }

    /// Moves to `from` when a packet arrived from there, we can send to it,
    /// and the peer has not answered for `after`. Returns the address left.
    ///
    /// An address that is only guessed at is moved to when `proven` says the packet shows the
    /// peer is there: the answer to a probe sent to it, or a packet opened by an encrypted link.
    fn follow(&mut self, from: SocketAddr, after: Duration, proven: bool) -> Option<SocketAddr> {
        if from == self.current || self.answered_at.elapsed() < after {
            return None;
        }
        let index = self
            .others
            .iter()
            .position(|&(addr, sendable)| addr == from && sendable);
        if index.is_none() && !(proven && self.guessed(from)) {
            return None;
        }
        let left = std::mem::replace(&mut self.current, from);
        match index {
            Some(index) => self.others[index] = (left, true),
            None => self.others.push((left, true)),
        }
        self.answered_at = Instant::now();
        Some(left)
    }

    /// The token of a check to send to `from`, when a packet opened by an encrypted link came
    /// from an address that is not one of the peer's offered ones and the peer has not answered
    /// for `after`. `None` when no check is due.
    ///
    /// This is for a network that changes the port or address the peer is seen from after it
    /// offered its addresses. The address is not taken up here: a packet that got through the
    /// link may be one the peer sent earlier, sent again from an address of someone else's, so
    /// the route moves only once the answer to the check comes from there
    /// ([`Route::check_answered`]).
    fn start_check(&mut self, from: SocketAddr, after: Duration) -> Option<u32> {
        if from == self.current
            || self.others.iter().any(|&(other, _)| other == from)
            || self.answered_at.elapsed() < after
        {
            return None;
        }
        self.checks
            .retain(|check| check.sent_at.elapsed() < PATH_CHECK_LIFETIME);
        let token = rand::random::<u32>();
        let check = PathCheck {
            addr: from,
            token,
            sent_at: Instant::now(),
        };
        match self.checks.iter().position(|check| check.addr == from) {
            Some(i) if self.checks[i].sent_at.elapsed() < PATH_CHECK_RETRY => return None,
            Some(i) => self.checks[i] = check,
            None if self.checks.len() >= MAX_PATH_CHECKS => return None,
            None => self.checks.push(check),
        }
        Some(token)
    }

    /// Whether `token` answers the check sent to `from`. An answer counts once, and the
    /// address becomes one of the peer's, so that [`Route::follow`] can move to it.
    fn check_answered(&mut self, token: u32, from: SocketAddr) -> bool {
        let Some(index) = self
            .checks
            .iter()
            .position(|check| check.addr == from && check.token == token)
        else {
            return false;
        };
        let check = self.checks.swap_remove(index);
        if check.sent_at.elapsed() >= PATH_CHECK_LIFETIME {
            return false;
        }
        if !self.knows(from) {
            self.others.push((from, true));
        }
        true
    }

    /// Notes whether the last send to `addr` went out
    fn note_send(&mut self, addr: SocketAddr, sent: bool) {
        if let Some(entry) = self.others.iter_mut().find(|(other, _)| *other == addr) {
            entry.1 = sent;
        }
    }
}

/// Whether `bytes`, received from `from`, answer a probe sent to `candidates`
///
/// `sendable[i]` says whether the probe to `candidates[i]` went out. A packet
/// that reaches us from an address we cannot send to shows a one-way path, and
/// choosing it leaves the peer hearing nothing from us (REQ-CON-114).
fn is_probe_answer(
    candidates: &[SocketAddr],
    sendable: &[bool],
    from: SocketAddr,
    bytes: &[u8],
) -> bool {
    let Some(index) = candidates.iter().position(|&addr| addr == from) else {
        return false;
    };
    sendable[index]
        && Packet::from_bytes(bytes)
            .is_some_and(|packet| matches!(packet.packet_type, PacketType::KeepAlive))
}

/// How much faster a route must be to beat one that is nearer by kind: round trips
/// closer than this are noise.
const NEAR_ENOUGH: Duration = Duration::from_millis(2);

/// The number a probe carries, so that its reply names the candidate and the round it
/// answers
fn probe_token(round: usize, index: usize) -> u32 {
    ((round as u32) << 16) | index as u32
}

/// What the connectivity check has heard so far, candidate by candidate
///
/// Each probe is a keep-alive that asks for one back ([`Packet::route_probe`]), so the check
/// can time the round trip on every address, [`PROBE_ROUNDS`] times each.
struct Probing<'a> {
    candidates: &'a [SocketAddr],
    /// When the probe of each round went out to the candidate. A probe that did not go out
    /// is not a route, however its packets reach us (REQ-CON-114). The rounds after the
    /// scheduled ones are the probes sent in answer to the peer's ([`TRIGGERED_PROBES`])
    sent_at: Vec<Vec<Option<Instant>>>,
    /// The round trip of each probe, once its reply came
    rtt: Vec<Vec<Option<Duration>>>,
    /// How many probes in answer to the peer's went to each candidate
    triggered: Vec<usize>,
}

/// What to send back for a probe the peer sent us
#[derive(Debug, PartialEq)]
struct ProbeAnswer {
    /// The token the reply to the peer's probe carries
    reply_token: u32,
    /// The token of a probe of our own to send to the peer at once, when its probe came from
    /// an address none of ours has been answered by (see [`TRIGGERED_PROBES`])
    probe_back: Option<u32>,
}

/// What the check has settled on, or how long it has to wait
enum Verdict {
    /// The candidate with the shortest median round trip (within [`NEAR_ENOUGH`], the
    /// nearer kind)
    Fastest {
        index: usize,
        rtt: Duration,
        replies: usize,
    },
    /// Nothing is settled before this time
    WaitUntil(Instant),
    /// Nothing has answered
    NoAnswer,
}

impl<'a> Probing<'a> {
    fn new(candidates: &'a [SocketAddr]) -> Self {
        let n = candidates.len();
        Self {
            candidates,
            sent_at: vec![vec![None; PROBE_ROUNDS + TRIGGERED_PROBES]; n],
            rtt: vec![vec![None; PROBE_ROUNDS + TRIGGERED_PROBES]; n],
            triggered: vec![0; n],
        }
    }

    fn sent(&mut self, index: usize, round: usize, at: Instant) {
        self.sent_at[index][round] = Some(at);
    }

    /// Whether any probe went out to the candidate
    fn sendable(&self, index: usize) -> bool {
        self.sent_at[index].iter().any(Option::is_some)
    }

    fn sent_count(&self, index: usize) -> usize {
        self.sent_at[index].iter().flatten().count()
    }

    fn reply_count(&self, index: usize) -> usize {
        self.rtt[index].iter().flatten().count()
    }

    /// The median round trip of the replies that came: one lucky or unlucky probe does not
    /// move it
    fn median(&self, index: usize) -> Option<Duration> {
        let mut replies: Vec<Duration> = self.rtt[index].iter().flatten().copied().collect();
        replies.sort();
        replies.get(replies.len().checked_sub(1)? / 2).copied()
    }

    /// Take in a datagram that reached the socket. Returns what to send back to `from` when
    /// the peer asked for a reply.
    ///
    /// A probe that reaches us shows that the path works from the peer to us, and that the
    /// peer's NAT, if any, now lets us in: our own probe sent back at once is the first to
    /// get through when the scheduled ones were all dropped.
    fn hear(&mut self, from: SocketAddr, bytes: &[u8], now: Instant) -> Option<ProbeAnswer> {
        let sendable: Vec<bool> = (0..self.candidates.len())
            .map(|i| self.sendable(i))
            .collect();
        if !is_probe_answer(self.candidates, &sendable, from, bytes) {
            return None;
        }
        let index = self.candidates.iter().position(|&addr| addr == from)?;
        let (reply, token) = Packet::from_bytes(bytes)?.route_probe_token()?;
        if !reply {
            let probe_back = (self.reply_count(index) == 0
                && self.triggered[index] < TRIGGERED_PROBES)
                .then(|| {
                    let round = PROBE_ROUNDS + self.triggered[index];
                    self.triggered[index] += 1;
                    probe_token(round, index)
                });
            return Some(ProbeAnswer {
                reply_token: token,
                probe_back,
            });
        }
        // A reply counts only for the address and the round its probe went to
        let (round, named) = ((token >> 16) as usize, (token & 0xFFFF) as usize);
        if named == index
            && round < PROBE_ROUNDS + TRIGGERED_PROBES
            && self.rtt[index][round].is_none()
        {
            self.rtt[index][round] =
                self.sent_at[index][round].map(|sent| now.saturating_duration_since(sent));
        }
        None
    }

    /// Settle on the candidate whose median round trip is shortest, once no probe still
    /// out could change that. A candidate that answered fewer than half of its probes is a
    /// lossy path and is passed over, unless nothing else answered.
    fn assess(&self, now: Instant) -> Verdict {
        let answered: Vec<usize> = (0..self.candidates.len())
            .filter(|&i| self.reply_count(i) > 0)
            .collect();
        let reliable: Vec<usize> = answered
            .iter()
            .copied()
            .filter(|&i| self.reply_count(i) * 2 >= self.sent_count(i))
            .collect();
        let pool = if reliable.is_empty() {
            answered
        } else {
            reliable
        };
        let Some(fastest) = pool.iter().filter_map(|&i| self.median(i)).min() else {
            return Verdict::NoAnswer;
        };

        // A probe still out cannot come back faster than `fastest` once that long has
        // passed since it went out
        let settled = (0..self.candidates.len())
            .flat_map(|i| {
                (0..PROBE_ROUNDS + TRIGGERED_PROBES)
                    .filter(move |&round| self.rtt[i][round].is_none())
                    .filter_map(move |round| self.sent_at[i][round])
            })
            .map(|sent| sent + fastest + NEAR_ENOUGH)
            .max();
        if let Some(settled) = settled.filter(|&settled| now < settled) {
            return Verdict::WaitUntil(settled);
        }

        // Routes are compared by the median plus what the kind of route is held to
        let score = |i: usize| {
            self.median(i)
                .map(|m| m + route_handicap(self.candidates[i]))
        };
        let best = pool
            .iter()
            .filter_map(|&i| score(i))
            .min()
            .expect("the fastest candidate has a score");
        let index = pool
            .iter()
            .copied()
            .filter(|&i| score(i).is_some_and(|s| s <= best + NEAR_ENOUGH))
            .max_by_key(|&i| {
                (
                    route_preference(self.candidates[i]),
                    std::cmp::Reverse(score(i)),
                )
            })
            .expect("the best candidate is among them");
        Verdict::Fastest {
            index,
            rtt: self.median(index).expect("it has a round trip"),
            replies: self.reply_count(index),
        }
    }
}

/// Hand a packet the FEC decoder rebuilt to the audio callback and count it.
///
/// Both ways a group can complete come through here: the FEC packet arriving
/// last, and - when packets are reordered - a data packet arriving after it.
fn deliver_recovered(
    recovered: Option<RecoveredPacket>,
    group_size: usize,
    fec_recovered: &AtomicU64,
    callback: &Option<Arc<AudioCallback>>,
) {
    if let Some(recovered) = recovered {
        let sequence = recovered.group_sequence * group_size as u32 + recovered.packet_index as u32;
        trace!("FEC recovered packet seq={}", sequence);
        fec_recovered.fetch_add(1, Ordering::Relaxed);
        if let Some(callback) = callback {
            // Timestamp is not recoverable from FEC; the receive path keys off
            // sequence (ADR-021).
            callback(sequence, recovered.data, 0);
        }
    }
}

/// Store the peer's latency info and tell the callback about it.
fn note_peer_latency_info(
    payload: &[u8],
    callback: &Option<Arc<LatencyInfoCallback>>,
    store: &RwLock<Option<PeerLatencyInfo>>,
) {
    if let Some(info) = LatencyInfoMessage::from_bytes(payload) {
        let peer_info: PeerLatencyInfo = info.into();
        debug!(
            "Received peer latency info: capture={:.2}ms, playback={:.2}ms, jitter={:.2}ms",
            peer_info.capture_buffer_ms, peer_info.playback_buffer_ms, peer_info.jitter_buffer_ms
        );
        if let Some(callback) = callback {
            callback(peer_info.clone());
        }
        *store.write() = Some(peer_info);
    }
}

/// RTT measurement state
#[derive(Debug)]
struct RttMeasurement {
    /// Current smoothed RTT estimate (ms), or `None` before the first sample
    rtt_ms: Option<f32>,
    /// RTT jitter / variation (ms)
    jitter_ms: f32,
    /// Smoothed time of each way, from the clocks of both ends
    one_way: Option<OneWayDelays>,
    /// Pending ping sequences with sent timestamps (monotonic instant)
    pending_pings: HashMap<u32, Instant>,
    /// When the newest ping that came back was sent: pings sent before it that did not come
    /// back were lost, and say nothing of how long an answer takes
    last_answered_sent_at: Option<Instant>,
    /// Recent RTT samples for averaging
    rtt_samples: VecDeque<f32>,
    /// The round trip the connectivity check timed on the route taken up: all that is known of
    /// the link until the first ping is answered
    route_round_trip: Option<Duration>,
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
            one_way: None,
            pending_pings: HashMap::new(),
            last_answered_sent_at: None,
            rtt_samples: VecDeque::with_capacity(RTT_SAMPLE_COUNT),
            route_round_trip: None,
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

    /// The slowest of the recent round trips, counting the pings sent since the last answer
    /// for as long as they have been out, or `None` before there is any
    ///
    /// Until a ping has been answered, the round trip the connectivity check timed stands in.
    fn slowest_recent(&self) -> Option<Duration> {
        let answered = self
            .rtt_samples
            .iter()
            .copied()
            .reduce(f32::max)
            .map(|ms| Duration::from_secs_f32(ms / 1000.0))
            .or(self.route_round_trip);
        let unanswered = self
            .pending_pings
            .values()
            .filter(|&&sent| {
                self.last_answered_sent_at
                    .is_none_or(|newest| sent > newest)
            })
            .map(|sent| sent.elapsed().min(UNANSWERED_PING_CAP))
            .max();
        answered.max(unanswered)
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
            sent_wall_us: Some(now_unix_us()),
        }
    }

    /// Process a pong response, which arrived at `received_wall_us` by this computer's clock,
    /// and update RTT statistics
    fn process_pong(&mut self, pong: &LatencyPong, received_wall_us: u64) {
        if let Some(sent_time) = self.pending_pings.remove(&pong.ping_sequence) {
            let rtt = sent_time.elapsed().as_secs_f32() * 1000.0; // Convert to ms
            if self
                .last_answered_sent_at
                .is_none_or(|newest| sent_time > newest)
            {
                self.last_answered_sent_at = Some(sent_time);
            }

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

            if let Some(wall) = pong.wall {
                self.note_one_way(&wall, received_wall_us);
            }

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

impl RttMeasurement {
    /// Smooth into the estimate the two ways of a round trip, from the clock times its ping
    /// and pong carried and the time the pong arrived
    fn note_one_way(&mut self, wall: &PongWall, received_wall_us: u64) {
        let ms =
            |later: u64, earlier: u64| (i128::from(later) - i128::from(earlier)) as f32 / 1000.0;
        let sample = OneWayDelays {
            up_ms: ms(wall.ping_received_wall_us, wall.original_sent_wall_us),
            down_ms: ms(received_wall_us, wall.sent_wall_us),
        };
        let alpha = 0.125;
        self.one_way = Some(match self.one_way {
            None => sample,
            Some(current) => OneWayDelays {
                up_ms: (1.0 - alpha) * current.up_ms + alpha * sample.up_ms,
                down_ms: (1.0 - alpha) * current.down_ms + alpha * sample.down_ms,
            },
        });
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
/// Failed --> Connected: packets arrive again
/// Failed --> Reconnecting: reconnect()
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
/// until packets flow again - the keep-alive loop already does that, also after
/// the user was asked. What this configures is when to tell the user, which
/// `connection.feature` specifies: a 3-second outage recovers on its own, a
/// 10-second one asks the user, and a link that comes back is taken up again
/// either way (REQ-CON-109, REQ-CON-110).
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

    /// How long the peer may go without answering a ping before the route looks past
    /// the address in use
    ///
    /// `detect_after` suits a link that answers within a keep-alive interval. On a link
    /// whose round trip is long, the answer to the first ping sent after the route
    /// moved cannot be back before one interval (the wait for the next ping) plus the
    /// round trip, so a fixed wait moves the route again before that answer can arrive.
    /// The wait spans the next ping and the slowest recent round trip with the same
    /// margin again, and never exceeds `give_up_after`.
    fn answer_wait(&self, slowest_round_trip: Option<Duration>) -> Duration {
        let slowest = slowest_round_trip.unwrap_or(Duration::ZERO);
        self.detect_after
            .max(self.keep_alive_interval + slowest * 2)
            .min(self.give_up_after.max(self.detect_after))
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
    route: Arc<Mutex<Route>>,
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
    /// When the user last asked for a new attempt: the give-up time is counted from here
    /// when that is later than the last packet
    retry_at: Arc<std::sync::Mutex<Instant>>,
    audio_callback: Option<Arc<AudioCallback>>,
    /// Encoder for outgoing audio. `None` until `set_audio_encoding` is called,
    /// in which case PCM is used inline for backward compatibility.
    encoder: Arc<std::sync::Mutex<Box<dyn AudioCodec>>>,
    /// FEC encoder for outgoing audio, when the preset enables FEC
    fec_encoder: Option<Arc<std::sync::Mutex<FecEncoder>>>,
    /// FEC decoder for incoming audio, shared with the receive loop
    fec_decoder: Option<Arc<std::sync::Mutex<FecDecoder>>>,
    /// FEC group size in use, needed to map sequence numbers to groups
    fec_group_size: Option<usize>,
    /// Channels interleaved in the audio handed to `send_audio`
    audio_channels: usize,
    /// Reconnection thresholds
    reconnect_config: ReconnectConfig,
    /// Whether 127/8 counts as a public address for guessing a peer's port (tests only)
    predict_on_loopback: bool,
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
    /// The encryption of this link: every packet goes out through it and comes in through it
    secure_link: Arc<SecureLink>,
    /// The latest latency info, kept while the link is still agreeing keys and sent when it
    /// settles
    pending_latency_info: Arc<Mutex<Option<LatencyInfoMessage>>>,
    /// Sends our key until the peer has it, and what waited for the keys
    secure_link_handle: Option<tokio::task::JoinHandle<()>>,
    /// Sends to the ports guessed for a peer behind a NAT that changes its port per destination
    prediction_handle: Option<tokio::task::JoinHandle<()>>,
    /// While set, nothing is sent to the peer that it has not asked for: a quiet of the
    /// [`PunchSchedule`]
    sending_paused: Arc<AtomicBool>,
    /// Datagrams from the peer that the link let through, the key exchange too. It counts what
    /// shows the way to the peer works, which `packets_received` does not before the keys.
    heard_any: Arc<AtomicU64>,
    punch_schedule: PunchSchedule,
    /// Starts the sending over while the peer has not been heard
    repunch_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Sends `packet` as the link allows: encrypted when the keys are agreed, and not at all
/// while they are not, apart from keep-alives. Returns the bytes put on the wire, `0` for a
/// packet that had to wait.
async fn send_sealed(
    transport: &UdpTransport,
    link: &SecureLink,
    packet: Packet,
    addr: SocketAddr,
) -> Result<usize, NetworkError> {
    let Some(sealed) = link.seal(packet) else {
        return Ok(0);
    };
    let len = HEADER_SIZE + sealed.payload.len();
    transport.send_to(&sealed, addr).await?;
    Ok(len)
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

    /// Create a connection to one peer over a socket shared with the
    /// connections to the others in the room.
    ///
    /// Everyone in a room sends to the one address this app told it, so the
    /// connections to them all go through that socket, and `mux` gives each
    /// the packets of its own peer. Must be called from within the runtime
    /// that drives `mux`.
    pub fn on(mux: &UdpMux) -> Result<Self, NetworkError> {
        Self::from_transport(mux.transport())
    }

    fn from_transport(transport: UdpTransport) -> Result<Self, NetworkError> {
        Ok(Self {
            transport: Arc::new(transport),
            route: Arc::new(Mutex::new(Route::new(
                "0.0.0.0:0".parse().unwrap(),
                Vec::new(),
            ))),
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
            retry_at: Arc::new(std::sync::Mutex::new(Instant::now())),
            audio_callback: None,
            encoder: Arc::new(std::sync::Mutex::new(Box::new(PcmCodec::new(
                &CodecConfig::default(),
            )))),
            fec_encoder: None,
            fec_decoder: None,
            fec_group_size: None,
            audio_channels: 1,
            reconnect_config: ReconnectConfig::default().validated(),
            predict_on_loopback: false,
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
            secure_link: Arc::new(SecureLink::unbound()),
            pending_latency_info: Arc::new(Mutex::new(None)),
            secure_link_handle: None,
            prediction_handle: None,
            sending_paused: Arc::new(AtomicBool::new(false)),
            heard_any: Arc::new(AtomicU64::new(0)),
            punch_schedule: PunchSchedule::default(),
            repunch_handle: None,
        })
    }

    /// Has the link to the peer check who the peer is: the key exchange it sends has to be
    /// signed with `peer_key`, the key the signaling server gave for it, and ours is signed
    /// with `ours`. A connection does not connect before this is called, and a `peer_key`
    /// that is not a key (the peer's app predates it, or the server gave none) is refused with
    /// [`NetworkError::PeerKeyMissing`]. Called before connecting.
    pub fn verify_peer(&mut self, ours: &LinkIdentity, peer_key: &str) -> Result<(), NetworkError> {
        if self.is_connected() {
            return Err(NetworkError::AlreadyConnected);
        }
        let link = SecureLink::for_peer(ours, peer_key).ok_or(NetworkError::PeerKeyMissing)?;
        self.secure_link = Arc::new(link);
        Ok(())
    }

    /// When and in which order a link whose peer is not heard sends again. Called before
    /// connecting.
    pub fn set_punch_schedule(&mut self, schedule: PunchSchedule) {
        self.punch_schedule = schedule;
    }

    /// Whether the keys of the link are bound to the peer's key from the server
    pub fn checks_peer(&self) -> bool {
        self.secure_link.checks_peer()
    }

    /// Get the local address
    pub fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }

    /// The address currently in use for the remote peer: the candidate
    /// [`Self::connect_with_candidates`] selected, or the single address
    /// [`Self::connect`] was given.
    pub fn remote_addr(&self) -> SocketAddr {
        self.route.lock().current
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
        if !self.checks_peer() {
            return Err(NetworkError::PeerKeyMissing);
        }

        let started = Instant::now();
        self.transport.expect_peer_at(&[remote_addr]);
        *self.route.lock() = Route::new(remote_addr, Vec::new());
        self.set_state(ConnectionState::Connecting);
        info!("Connecting to {}", remote_addr);

        // Send initial keep-alive to establish connection
        let packet = Packet::keep_alive(self.next_sequence());
        send_sealed(&self.transport, &self.secure_link, packet, remote_addr).await?;
        self.link_facts.sent_first();

        // Record connection start time
        if let Ok(mut start) = self.connection_start.lock() {
            *start = Some(Instant::now());
        }
        self.start_silence_count();

        self.link_facts.link_up(remote_addr, false, started);
        self.link_facts.chosen_because(RouteReason::OnlyCandidate);
        self.set_state(ConnectionState::Connected);
        self.start_receive_loop();
        self.start_keepalive_loop();
        self.start_liveness_monitor();
        self.start_secure_link_task();

        info!("Connected to {}", remote_addr);
        Ok(())
    }

    /// Connect to a remote peer using multiple address candidates (Happy Eyeballs style)
    ///
    /// Every candidate is probed several times with a keep-alive that asks for one back, and
    /// the one whose median round trip is shortest is used. A peer that answers no probe is
    /// not connected to. `JAMJAM_ROUTE` pins the kind of route for debugging.
    /// Candidates are probed in order of priority, with a small delay between each.
    pub async fn connect_with_candidates(
        &mut self,
        candidates: &[SocketAddr],
    ) -> Result<(), NetworkError> {
        if self.is_connected() {
            return Err(NetworkError::AlreadyConnected);
        }
        if !self.checks_peer() {
            return Err(NetworkError::PeerKeyMissing);
        }

        let usable = usable_candidates(candidates.to_vec());
        let candidates = &usable[..];
        if candidates.is_empty() {
            return Err(NetworkError::NoCandidates);
        }

        let pinned: Vec<SocketAddr>;
        let candidates = match RouteOverride::from_env().map_err(NetworkError::ConnectionFailed)? {
            Some(route) => {
                pinned = candidates
                    .iter()
                    .copied()
                    .filter(|&addr| route.allows(addr))
                    .collect();
                warn!(
                    "JAMJAM_ROUTE pins the route: {} of {} candidates allowed",
                    pinned.len(),
                    candidates.len()
                );
                if pinned.is_empty() {
                    return Err(NetworkError::NoCandidates);
                }
                &pinned[..]
            }
            None => candidates,
        };

        // If only one candidate, use simple connect
        if candidates.len() == 1 {
            return self.connect(candidates[0]).await;
        }

        let started = Instant::now();
        self.transport.expect_peer_at(candidates);
        self.link_facts.candidates_offered(candidates);
        self.set_state(ConnectionState::CheckingConnectivity);
        info!("Checking connectivity with {} candidates", candidates.len());

        // Probe every candidate PROBE_ROUNDS times, a little apart, hearing the peer's probes
        // in between so that its requests are answered at once: a request left in the socket
        // until the loop ends would be timed by the peer as a long round trip.
        let mut probing = Probing::new(candidates);
        let mut due: Vec<(Duration, usize, usize)> = (0..PROBE_ROUNDS)
            .flat_map(|round| {
                (0..candidates.len()).map(move |i| {
                    (
                        ROUND_INTERVAL * round as u32 + CANDIDATE_DELAY * i as u32,
                        round,
                        i,
                    )
                })
            })
            .collect();
        due.sort();
        for (after, round, i) in due {
            let at = started + after;
            while Instant::now() < at {
                self.hear_probe(&mut probing, at - Instant::now()).await?;
            }
            let addr = candidates[i];
            let packet = Packet::route_probe(self.next_sequence(), probe_token(round, i));
            if let Err(e) = send_sealed(&self.transport, &self.secure_link, packet, addr).await {
                debug!("Failed to send probe to candidate {}: {}", addr, e);
                continue;
            }
            probing.sent(i, round, Instant::now());
            self.link_facts.sent_first();
            debug!(
                "Sent connectivity probe {} to candidate {} ({})",
                round, i, addr
            );
        }

        let give_up_at = Instant::now() + PROBE_TIMEOUT;
        let verdict = loop {
            let now = Instant::now();
            match probing.assess(now) {
                Verdict::WaitUntil(until) => {
                    self.hear_probe(&mut probing, until.saturating_duration_since(now))
                        .await?
                }
                Verdict::NoAnswer if now < give_up_at => {
                    self.hear_probe(&mut probing, give_up_at - now).await?
                }
                verdict => break verdict,
            }
        };

        let other_candidates = |selected: SocketAddr| -> Vec<(SocketAddr, bool)> {
            candidates
                .iter()
                .enumerate()
                .filter(|&(_, &addr)| addr != selected)
                .map(|(i, &addr)| (addr, probing.sendable(i)))
                .collect()
        };

        let mut selected_rtt = None;
        let selected = match verdict {
            Verdict::Fastest {
                index,
                rtt,
                replies,
            } => {
                selected_rtt = Some(rtt);
                for (i, addr) in candidates.iter().enumerate() {
                    if probing.reply_count(i) > 0 {
                        let round_trips: Vec<String> = probing.rtt[i]
                            .iter()
                            .map(|rtt| match rtt {
                                Some(rtt) => format!("{:.1}", rtt.as_secs_f32() * 1000.0),
                                None => "-".to_string(),
                            })
                            .collect();
                        debug!(
                            "Candidate {} answered {} of {} probes, round trips (ms): {}",
                            addr,
                            probing.reply_count(i),
                            probing.sent_count(i),
                            round_trips.join(" ")
                        );
                    }
                }
                info!(
                    "Selected candidate: {} (median round trip {:.1} ms of {} replies, the shortest of {} candidates)",
                    candidates[index],
                    rtt.as_secs_f32() * 1000.0,
                    replies,
                    (0..candidates.len())
                        .filter(|&i| probing.reply_count(i) > 0)
                        .count()
                );
                Some(candidates[index])
            }
            Verdict::WaitUntil(_) | Verdict::NoAnswer => None,
        };

        match selected {
            Some(selected_addr) => {
                let mut route = Route::new(selected_addr, other_candidates(selected_addr));
                if let Some(rtt) = selected_rtt {
                    // The keys take a round trip to agree, then the first ping goes out
                    // within an interval and takes a round trip to be answered
                    route.start_waiting_for_an_answer_in(rtt);
                    self.rtt_measurement.write().route_round_trip = Some(rtt);
                }
                *self.route.lock() = route;

                // Record connection start time
                if let Ok(mut start) = self.connection_start.lock() {
                    *start = Some(Instant::now());
                }
                self.start_silence_count();

                self.link_facts.link_up(selected_addr, true, started);
                self.link_facts.chosen_because(RouteReason::Answered);
                self.set_state(ConnectionState::Connected);
                self.discard_audio_queued_while_probing();
                self.start_receive_loop();
                self.start_keepalive_loop();
                self.start_liveness_monitor();
                self.start_secure_link_task();

                Ok(())
            }
            None => {
                // Timeout - fall back to the first candidate we could send to
                warn!("No candidate responded in time, falling back to first candidate");
                self.set_state(ConnectionState::Connecting);
                let fallback = fallback_candidate(candidates, |i| probing.sendable(i));
                let mut route = Route::new(fallback, other_candidates(fallback));
                // Nothing answered. A peer that offered the same public address with several
                // ports is behind a NAT that gives each destination a port of its own, and
                // the port it will send from is none of the ones offered: guess it (ADR-115).
                let prediction = PortPrediction::from_offered(
                    candidates,
                    rand::random::<[u8; 32]>(),
                    self.predict_on_loopback,
                );
                route.predicted.clone_from(&prediction);
                *self.route.lock() = route;

                // Record connection start time
                if let Ok(mut start) = self.connection_start.lock() {
                    *start = Some(Instant::now());
                }
                self.start_silence_count();

                self.link_facts.link_up(fallback, false, started);
                self.link_facts
                    .chosen_because(if LinkRoute::of(fallback) == LinkRoute::Public {
                        RouteReason::NoAnswerPublic
                    } else {
                        RouteReason::NoAnswerFirst
                    });
                self.set_state(ConnectionState::Connected);
                self.discard_audio_queued_while_probing();
                self.start_receive_loop();
                self.start_keepalive_loop();
                self.start_liveness_monitor();
                self.start_secure_link_task();
                if let Some(prediction) = prediction {
                    info!(
                        "No candidate answered and the peer's router changes its port per destination: guessing {} ports",
                        prediction.packets_per_try()
                    );
                    self.start_port_prediction(prediction);
                }
                self.start_repunch(candidates.to_vec());

                Ok(())
            }
        }
    }

    /// Disconnect from the remote peer
    pub fn disconnect(&mut self) {
        self.set_state(ConnectionState::Disconnected);
        self.transport.release();

        if let Some(handle) = self.receive_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.keepalive_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.liveness_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.secure_link_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.prediction_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.repunch_handle.take() {
            handle.abort();
        }

        info!("Disconnected from {}", self.remote_addr());
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

        self.encoder = Arc::new(std::sync::Mutex::new(codec));
        self.audio_channels = config.channels.max(1) as usize;

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

    /// Lets addresses on this machine (127/8) count as public ones when guessing the port of a
    /// peer behind a NAT (ADR-115), so that a test can stand in for the internet on one
    /// machine. The app never calls it: there, a peer's loopback address is not one to probe.
    #[doc(hidden)]
    pub fn predict_ports_on_loopback(&mut self) {
        self.predict_on_loopback = true;
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

    /// Start a full attempt again after automatic recovery gave up
    ///
    /// `connection.feature` has the user choose whether to retry after a long
    /// outage (REQ-CON-110); this is what that choice calls. The receive, keep-alive
    /// and liveness loops run from the moment the link is up until it is closed, so
    /// they are still there to be given the full give-up time again. Returns an error if
    /// the connection was deliberately closed rather than lost.
    pub fn reconnect(&mut self) -> Result<(), NetworkError> {
        match self.state() {
            ConnectionState::Disconnected => Err(NetworkError::NotConnected),
            ConnectionState::Reconnecting | ConnectionState::Failed => {
                // The give-up deadline runs from now, so the user gets a full
                // attempt. The link is not called up until a packet arrives.
                if let Ok(mut at) = self.retry_at.lock() {
                    *at = Instant::now();
                }
                self.reset_receive_state();
                self.set_state(ConnectionState::Reconnecting);
                Ok(())
            }
            // Up, or not up yet: there is no loss to recover from
            _ => Ok(()),
        }
    }

    /// Count the silence from now: until the link is up the only packets are
    /// probe answers, which the liveness monitor never sees, so the time since
    /// this connection was created says nothing about the link.
    fn start_silence_count(&self) {
        if let Ok(mut last) = self.last_received.lock() {
            *last = Instant::now();
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

        let remote_addr = self.remote_addr();
        let packet = Packet::latency_info(self.next_sequence(), info);
        let sent = send_sealed(&self.transport, &self.secure_link, packet, remote_addr).await?;
        if sent == 0 {
            // The keys are not agreed yet. The peer is told as soon as they are.
            *self.pending_latency_info.lock() = Some(info.clone());
            if self.secure_link.can_send_media() {
                // They were agreed while this was being put aside, and the task that
                // sends what waited may already have looked.
                let waiting = self.pending_latency_info.lock().take();
                if let Some(info) = waiting {
                    let packet = Packet::latency_info(self.next_sequence(), &info);
                    send_sealed(&self.transport, &self.secure_link, packet, remote_addr).await?;
                }
            }
            return Ok(());
        }

        debug!("Sent latency info to {}", remote_addr);
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
    ///
    /// PCM that would not fit one packet within [`crate::protocol::MAX_PAYLOAD_SIZE`] once sealed (256
    /// stereo frames) goes as several packets of equal length, each numbered and
    /// timestamped as the frame it is a part of. The receiver plays them as frames of
    /// their own; nothing on the wire says they were one.
    pub async fn send_audio(&self, data: &[f32], timestamp: u32) -> Result<(), NetworkError> {
        let splittable = self
            .encoder
            .lock()
            .map_err(|_| NetworkError::Configuration("Codec lock poisoned".into()))?
            .codec_type()
            == CodecType::Pcm;
        if !splittable || data.is_empty() {
            return self.send_audio_packet(data, timestamp).await;
        }
        let per_packet = samples_per_audio_packet(data.len(), self.audio_channels);
        let mut timestamp = timestamp;
        for part in data.chunks(per_packet) {
            self.send_audio_packet(part, timestamp).await?;
            timestamp = timestamp.wrapping_add((part.len() / self.audio_channels) as u32);
        }
        Ok(())
    }

    async fn send_audio_packet(&self, data: &[f32], timestamp: u32) -> Result<(), NetworkError> {
        if !self.state().can_transmit() {
            return Err(NetworkError::NotConnected);
        }
        // Until the keys are agreed there is nothing that may be sent, and audio that is
        // encoded now would take sequence numbers the peer then sees as lost.
        if !self.secure_link.can_send_media() {
            return Ok(());
        }

        // Encode with the configured codec. Without `set_audio_encoding` the codec
        // is PCM, so existing callers keep working.
        let bytes = self
            .encoder
            .lock()
            .map_err(|_| NetworkError::Configuration("Codec lock poisoned".into()))?
            .encode(data)
            .map_err(|e| NetworkError::Configuration(format!("Audio encode: {}", e)))?;

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

        let remote_addr = self.remote_addr();
        let len =
            send_sealed(&self.transport, &self.secure_link, packet, remote_addr).await? as u64;

        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(len, Ordering::Relaxed);

        // FEC covers the group this packet completes.
        if let Some(fec) = generated {
            let fec_packet = Packet::fec(sequence, timestamp, fec.to_bytes());
            let fec_len = send_sealed(&self.transport, &self.secure_link, fec_packet, remote_addr)
                .await? as u64;
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
        send_sealed(&self.transport, &self.secure_link, packet.clone(), addr)
            .await
            .map(|_| ())
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
            one_way: rtt.one_way,
            packet_loss_rate,
            jitter_ms: rtt.jitter_ms,
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            packets_received: self.packets_received.load(Ordering::Relaxed),
            last_received_at: (self.packets_received.load(Ordering::Relaxed) > 0)
                .then(|| *self.last_received.lock().unwrap_or_else(|e| e.into_inner())),
            audio_packets_received,
            audio_packets_lost,
            fec_recovered: self
                .fec_decoder
                .as_ref()
                .map(|_| self.fec_recovered.load(Ordering::Relaxed)),
            uptime_seconds: uptime,
            security: self.secure_link.security(),
            packets_refused: self.secure_link.refused(),
            samples_clipped: self
                .encoder
                .lock()
                .map(|codec| codec.clipped_samples())
                .unwrap_or(0),
        }
    }

    /// Whether what this connection carries is encrypted
    pub fn security(&self) -> LinkSecurity {
        self.secure_link.security()
    }

    fn next_sequence(&self) -> u32 {
        self.sequence.fetch_add(1, Ordering::Relaxed)
    }

    fn next_audio_sequence(&self) -> u32 {
        self.audio_sequence.fetch_add(1, Ordering::Relaxed)
    }

    /// Wait up to `wait` for one datagram while probing and take it in, answering a peer's
    /// request for a reply to its probe. A socket that cannot be read ends the check.
    async fn hear_probe(
        &self,
        probing: &mut Probing<'_>,
        wait: Duration,
    ) -> Result<(), NetworkError> {
        let heard = match tokio::time::timeout(wait, self.transport.recv_raw()).await {
            Err(_) => return Ok(()),
            Ok(Ok(heard)) => heard,
            Ok(Err(e)) => {
                warn!("Error receiving probe response: {}", e);
                self.set_state(ConnectionState::Failed);
                return Err(NetworkError::ConnectionFailed(
                    "No candidate responded".to_string(),
                ));
            }
        };
        let (bytes, from) = heard;
        if let Some(answer) = probing.hear(from, &bytes, Instant::now()) {
            let reply = Packet::route_probe_reply(self.next_sequence(), answer.reply_token);
            if let Err(e) = send_sealed(&self.transport, &self.secure_link, reply, from).await {
                debug!("Failed to answer the probe from {}: {}", from, e);
            }
            if let Some(token) = answer.probe_back {
                let probe = Packet::route_probe(self.next_sequence(), token);
                match send_sealed(&self.transport, &self.secure_link, probe, from).await {
                    Ok(_) => {
                        let (round, index) = ((token >> 16) as usize, (token & 0xFFFF) as usize);
                        probing.sent(index, round, Instant::now());
                        debug!("Sent a probe back to {} for the one it sent", from);
                    }
                    Err(e) => debug!("Failed to probe {} back: {}", from, e),
                }
            }
        }
        Ok(())
    }

    /// Drops the audio that reached the socket while connectivity was being
    /// probed.
    ///
    /// A peer that is already connected sends audio all that time. What arrives
    /// between the last read of the check and the receive loop starting would,
    /// read at once, overflow the play-out buffer (REQ-CON-116). Audio that old is
    /// not worth playing.
    fn discard_audio_queued_while_probing(&self) {
        while self.transport.try_recv_raw().is_some() {}
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
        let route = self.route.clone();
        let secure_link = self.secure_link.clone();
        let pending_latency_info = self.pending_latency_info.clone();
        let reconnect_config = self.reconnect_config;
        let heard_any = self.heard_any.clone();
        let sequence = Arc::new(AtomicU32::new(1_000_000)); // Separate sequence for pong responses

        let handle = tokio::spawn(async move {
            let (mut rx, _recv_handle) = transport.clone().start_receive_loop();

            while let Some((packet, addr)) = rx.recv().await {
                // Packets heard after the link gave up are what bring it back
                if ConnectionState::from_u8(state.load(Ordering::SeqCst))
                    == ConnectionState::Disconnected
                {
                    break;
                }

                // What the packet took on the wire, before it is opened
                let wire_len = (HEADER_SIZE + packet.payload.len()) as u64;

                // The keys are settled by what the peer sends, so only what comes from where the
                // peer is expected may settle them.
                let from_a_peer_address = route.lock().knows(addr);
                if !from_a_peer_address
                    && (packet.packet_type == PacketType::Control || !secure_link.has_keys())
                {
                    continue;
                }

                // Nothing below acts on a packet that has not been through the link: it is
                // decrypted there, and a forged or repeated one stops there.
                let opened = secure_link.open(packet);
                if let Opened::Dropped(reason) = &opened {
                    debug!("Dropped a packet from {}: {}", addr, reason);
                    continue;
                }
                heard_any.fetch_add(1, Ordering::Relaxed);
                link_facts.heard();

                // The peer is heard from an address other than the one we send
                // to, and has not answered us there: send to where it is heard.
                // Its key counts: until the keys are agreed nothing else it sends
                // is accepted, and no key can come back to it before the route
                // is there.
                let mut follow_after =
                    reconnect_config.answer_wait(rtt_measurement.read().slowest_recent());
                // A packet opened by an encrypted link was sealed by the peer, but not
                // necessarily sent from where it came from: the peer's packet can be sent again
                // from any address. So the address it came from is no proof of where the peer is.
                let encrypted = matches!(opened, Opened::Packet(_))
                    && secure_link.security() == LinkSecurity::Encrypted;
                // The peer is where an answer comes from: the answer to a probe sent to a
                // guessed port, from that port, shows the guess was right and that what we send
                // there reaches the peer, so there is no need to wait; the answer to a check sent
                // to an address the peer was heard from shows the peer is there. Nothing less
                // moves the route to an address that was not offered.
                let mut proven = false;
                if let Opened::Packet(packet) = &opened {
                    if let Some((true, token)) = packet.route_probe_token() {
                        if route.lock().is_answer_from_guessed(token, addr) {
                            follow_after = Duration::ZERO;
                            proven = true;
                        } else if encrypted && route.lock().check_answered(token, addr) {
                            proven = true;
                        }
                    }
                }
                // The network may have changed the address the peer is seen from since it
                // was offered. Ask there, sealed, before sending anything else there.
                if encrypted && !proven {
                    let check = route.lock().start_check(addr, follow_after);
                    if let Some(token) = check {
                        transport.also_expect_peer_at(addr);
                        let probe =
                            Packet::route_probe(sequence.fetch_add(1, Ordering::Relaxed), token);
                        debug!("Checking the address {} the peer was heard from", addr);
                        if let Err(e) = send_sealed(&transport, &secure_link, probe, addr).await {
                            debug!("Failed to check {}: {}", addr, e);
                        }
                    }
                }
                let followed = route.lock().follow(addr, follow_after, proven);
                if let Some(left) = followed {
                    info!(
                        "The peer answers nothing on {}; sending to {} instead",
                        left, addr
                    );
                    link_facts.route_moved(addr);
                }

                let packet = match opened {
                    Opened::Packet(packet) => packet,
                    Opened::KeyExchange { answer } => {
                        if answer {
                            if let Some(key) = secure_link.key_exchange_packet() {
                                if let Err(e) = transport.send_to(&key, addr).await {
                                    warn!("Failed to answer the peer's key exchange: {}", e);
                                }
                            }
                            // The keys are ours now: something encrypted shows the peer it has
                            // ours too, without waiting for the next keep-alive
                            let keep_alive =
                                Packet::keep_alive(sequence.fetch_add(1, Ordering::Relaxed));
                            if let Err(e) =
                                send_sealed(&transport, &secure_link, keep_alive, addr).await
                            {
                                warn!("Failed to send keep-alive: {}", e);
                            }
                        }
                        continue;
                    }
                    Opened::Dropped(_) => continue,
                };

                *last_received.lock().unwrap() = Instant::now();
                packets_received.fetch_add(1, Ordering::Relaxed);
                bytes_received.fetch_add(wire_len, Ordering::Relaxed);

                // What waited for the peer to show it has the keys goes as soon as it has
                if secure_link.can_send_media() {
                    let waiting = pending_latency_info.lock().take();
                    if let Some(info) = waiting {
                        let packet =
                            Packet::latency_info(sequence.fetch_add(1, Ordering::Relaxed), &info);
                        let remote_addr = route.lock().current;
                        if let Err(e) =
                            send_sealed(&transport, &secure_link, packet, remote_addr).await
                        {
                            warn!("Failed to send latency info: {}", e);
                        }
                    }
                }

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

                        let mut recovered = None;

                        // Register the packet with FEC before handing it on, so
                        // the group is complete when its FEC packet arrives.
                        if let (Some(ref decoder), Some(group_size)) =
                            (&fec_decoder, fec_group_size)
                        {
                            if let Ok(mut decoder) = decoder.lock() {
                                let group = packet.sequence / group_size as u32;
                                let index = (packet.sequence % group_size as u32) as usize;
                                // The FEC packet may already be here (reordered
                                // on the way), and this packet then completes
                                // the group.
                                recovered = decoder.add_packet(group, index, &packet.payload);
                            }
                        }

                        if let Some(ref callback) = audio_callback {
                            // Move the payload out: the receiver stores it in a
                            // jitter buffer, so a copy here would be wasted.
                            callback(packet.sequence, packet.payload, packet.timestamp);
                        }
                        if let Some(group_size) = fec_group_size {
                            deliver_recovered(
                                recovered,
                                group_size,
                                &fec_recovered,
                                &audio_callback,
                            );
                        }
                    }
                    PacketType::Fec => {
                        if let (Some(decoder), Some(group_size)) = (&fec_decoder, fec_group_size) {
                            let recovered =
                                FecPacket::from_bytes(&packet.payload).and_then(|fec| {
                                    decoder
                                        .lock()
                                        .ok()
                                        .and_then(|mut decoder| decoder.add_fec(fec))
                                });
                            deliver_recovered(
                                recovered,
                                group_size,
                                &fec_recovered,
                                &audio_callback,
                            );
                        }
                    }
                    PacketType::KeepAlive => {
                        debug!("Received keep-alive");
                        // A peer that is still probing times the route to us
                        if let (true, Some((false, token))) =
                            (from_a_peer_address, packet.route_probe_token())
                        {
                            let reply = Packet::route_probe_reply(
                                sequence.fetch_add(1, Ordering::Relaxed),
                                token,
                            );
                            if let Err(e) = send_sealed(&transport, &secure_link, reply, addr).await
                            {
                                warn!("Failed to answer a route probe: {}", e);
                            }
                        }
                    }
                    PacketType::LatencyPing => {
                        // Respond with pong
                        let ping_received_wall_us = now_unix_us();
                        if let Some(ping) = LatencyPing::from_bytes(&packet.payload) {
                            let pong = LatencyPong {
                                original_sent_time_us: ping.sent_time_us,
                                ping_sequence: ping.ping_sequence,
                                wall: ping.sent_wall_us.map(|original_sent_wall_us| PongWall {
                                    original_sent_wall_us,
                                    ping_received_wall_us,
                                    sent_wall_us: now_unix_us(),
                                }),
                            };
                            let pong_packet = Packet::latency_pong(
                                sequence.fetch_add(1, Ordering::Relaxed),
                                &pong,
                            );
                            let remote_addr = route.lock().current;
                            if let Err(e) =
                                send_sealed(&transport, &secure_link, pong_packet, remote_addr)
                                    .await
                            {
                                warn!("Failed to send latency pong: {}", e);
                            }
                            trace!("Responded to latency ping seq={}", ping.ping_sequence);
                        }
                    }
                    PacketType::LatencyPong => {
                        route.lock().answered();
                        let received_wall_us = now_unix_us();
                        // Update RTT measurement
                        if let Some(pong) = LatencyPong::from_bytes(&packet.payload) {
                            rtt_measurement
                                .write()
                                .process_pong(&pong, received_wall_us);
                        }
                    }
                    PacketType::LatencyInfo => {
                        note_peer_latency_info(
                            &packet.payload,
                            &latency_info_callback,
                            &peer_latency_info,
                        );
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
        let retry_at = self.retry_at.clone();
        let config = self.reconnect_config;
        let fec_decoder = self.fec_decoder.clone();
        let sequence_tracker_for_monitor = self.sequence_tracker.clone();
        let state_change_callback = self.state_change_callback.clone();
        let last_error = self.last_error.clone();
        let route = self.route.clone();

        let handle = tokio::spawn(async move {
            let mut ticker = interval(config.check_interval);

            loop {
                ticker.tick().await;

                let current = ConnectionState::from_u8(state.load(Ordering::SeqCst));
                if current == ConnectionState::Disconnected {
                    break;
                }

                let (silence, attempt) = match (last_received.lock(), retry_at.lock()) {
                    (Ok(last), Ok(retry)) => (last.elapsed(), last.elapsed().min(retry.elapsed())),
                    _ => continue,
                };
                let remote_addr = route.lock().current;

                let next = if attempt >= config.give_up_after {
                    // Automatic recovery has had long enough. Ask the user
                    // (REQ-CON-110); the keep-alives go on, so the link is
                    // taken up again if the peer is heard.
                    if current == ConnectionState::Failed {
                        None
                    } else {
                        if let Ok(mut err) = last_error.lock() {
                            *err = Some(format!(
                                "No packets from {} for {:.0}s",
                                remote_addr,
                                silence.as_secs_f32()
                            ));
                        }
                        Some(ConnectionState::Failed)
                    }
                } else if silence >= config.detect_after {
                    (current == ConnectionState::Connected).then_some(ConnectionState::Reconnecting)
                } else {
                    // Packets are flowing. If we had lost the link or given up
                    // on it, it is usable again.
                    matches!(
                        current,
                        ConnectionState::Reconnecting | ConnectionState::Failed
                    )
                    .then_some(ConnectionState::Connected)
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
                    if let Ok(mut err) = last_error.lock() {
                        *err = None;
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
            }
        });

        self.liveness_handle = Some(handle);
    }

    fn start_keepalive_loop(&mut self) {
        let transport = self.transport.clone();
        let state = self.state.clone();
        let route = self.route.clone();
        let sequence = AtomicU32::new(0);
        let rtt_measurement = self.rtt_measurement.clone();
        let secure_link = self.secure_link.clone();
        let keep_alive_interval = self.reconnect_config.keep_alive_interval;
        let reconnect_config = self.reconnect_config;
        let paused = self.sending_paused.clone();
        let link_facts = self.link_facts.clone();

        let handle = tokio::spawn(async move {
            let mut interval = interval(keep_alive_interval);

            loop {
                interval.tick().await;

                // Keeps probing after the link gave up, so the peer hears us as soon as it is back
                let current_state = ConnectionState::from_u8(state.load(Ordering::SeqCst));
                if current_state == ConnectionState::Disconnected {
                    break;
                }
                if paused.load(Ordering::Relaxed) {
                    continue;
                }

                let remote_addr = route.lock().current;

                // Send keep-alive
                let packet = Packet::keep_alive(sequence.fetch_add(1, Ordering::Relaxed));
                if let Err(e) = send_sealed(&transport, &secure_link, packet, remote_addr).await {
                    warn!("Failed to send keep-alive: {}", e);
                }

                // Send latency ping for RTT measurement, once the keys allow it
                if secure_link.can_send_media() {
                    let ping = rtt_measurement.write().create_ping();
                    let ping_packet =
                        Packet::latency_ping(sequence.fetch_add(1, Ordering::Relaxed), &ping);
                    if let Err(e) =
                        send_sealed(&transport, &secure_link, ping_packet, remote_addr).await
                    {
                        warn!("Failed to send latency ping: {}", e);
                    }
                    trace!("Sent latency ping seq={}", ping.ping_sequence);
                }

                // No answer to our pings: the peer may not be reached on this
                // address. Let it hear from us on its other ones too, so that
                // it is heard from there and the route can follow (REQ-CON-114).
                let probe_others_after =
                    reconnect_config.answer_wait(rtt_measurement.read().slowest_recent());
                let others = {
                    let route = route.lock();
                    if route.reprobing()
                        || probes_others(route.answered_at.elapsed(), probe_others_after)
                    {
                        route.others.iter().map(|&(addr, _)| addr).collect()
                    } else {
                        Vec::new()
                    }
                };
                for addr in others {
                    let packet = Packet::keep_alive(sequence.fetch_add(1, Ordering::Relaxed));
                    let sent = send_sealed(&transport, &secure_link, packet, addr)
                        .await
                        .is_ok();
                    if sent {
                        link_facts.sent_to_other();
                    }
                    route.lock().note_send(addr, sent);
                }
            }
        });

        self.keepalive_handle = Some(handle);
    }

    /// Starts the sending over, in turns, for as long as the peer is not heard at all
    /// ([`PunchSchedule`]): both ends send nothing for the quiet, then the end that is first this
    /// time sends to every address of the peer, and the other end does after it has waited. The
    /// first end of the pair is the one whose key sorts first, and the ends swap each time.
    fn start_repunch(&mut self, candidates: Vec<SocketAddr>) {
        if let Some(handle) = self.repunch_handle.take() {
            handle.abort();
        }
        let schedule = self.punch_schedule;
        let sorts_first = self.secure_link.sorts_before_peer().unwrap_or(true);
        let transport = self.transport.clone();
        let state = self.state.clone();
        let route = self.route.clone();
        let secure_link = self.secure_link.clone();
        let paused = self.sending_paused.clone();
        let heard_any = self.heard_any.clone();
        let link_facts = self.link_facts.clone();

        let handle = tokio::spawn(async move {
            let heard_before = heard_any.load(Ordering::Relaxed);
            // Waits for `time`; false when the peer was heard or the link ended meanwhile
            let wait = |time: Duration| {
                let state = state.clone();
                let heard_any = heard_any.clone();
                async move {
                    let until = Instant::now() + time;
                    while Instant::now() < until {
                        if heard_any.load(Ordering::Relaxed) > heard_before
                            || ConnectionState::from_u8(state.load(Ordering::SeqCst))
                                == ConnectionState::Disconnected
                        {
                            return false;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    true
                }
            };
            let sequence = AtomicU32::new(4_000_000);
            if !wait(schedule.settle).await {
                return;
            }
            let mut attempt: u32 = 0;
            loop {
                paused.store(true, Ordering::Relaxed);
                info!(
                    "The peer has not been heard: sending nothing for {:?}, then trying again (try {})",
                    schedule.quiet,
                    attempt + 1
                );
                let mut carry_on = wait(schedule.quiet).await;
                let first = attempt.is_multiple_of(2) == sorts_first;
                if carry_on && !first {
                    carry_on = wait(schedule.second_waits).await;
                }
                paused.store(false, Ordering::Relaxed);
                if !carry_on {
                    return;
                }
                link_facts.punched_again(first);
                // The end that did not wait sends on for as long as the wait, so that both
                // ends' tries last the same and the next one starts as far apart as this one
                let window = if first {
                    schedule.window + schedule.second_waits
                } else {
                    schedule.window
                };
                route.lock().reprobe_until = Some(Instant::now() + window);
                for round in 0..PROBE_ROUNDS {
                    for &addr in &candidates {
                        let packet = Packet::route_probe(
                            sequence.fetch_add(1, Ordering::Relaxed),
                            0xF000_0000 | (round as u32) << 8 | attempt.min(255),
                        );
                        if send_sealed(&transport, &secure_link, packet, addr)
                            .await
                            .is_ok()
                        {
                            link_facts.sent_to_other();
                        }
                    }
                    if !wait(ROUND_INTERVAL).await {
                        return;
                    }
                }
                if !wait(window).await {
                    return;
                }
                attempt += 1;
            }
        });

        self.repunch_handle = Some(handle);
    }

    /// Sends to the ports guessed for a peer behind a NAT that changes its port per
    /// destination, until the peer is heard (ADR-115)
    ///
    /// The peer's NAT lets in what comes from an address it has sent to, and has sent to us from
    /// a port that none of the addresses it offered names. Sending there, from the address it
    /// is to be sent to from, makes the packet that is to the right port one it accepts, and its
    /// answer comes from the port that was right. What is sent is bounded: a few ports at the
    /// pace of one every 10 ms, or a port set at 250 a second, and again after waits that
    /// double from 20 seconds to 5 minutes, a few times, then not at all until the peer has
    /// been heard and has gone silent again.
    fn start_port_prediction(&mut self, prediction: PortPrediction) {
        if let Some(handle) = self.prediction_handle.take() {
            handle.abort();
        }
        let transport = self.transport.clone();
        let state = self.state.clone();
        let secure_link = self.secure_link.clone();
        let packets_received = self.packets_received.clone();
        let last_received = self.last_received.clone();
        let silent_after = self.reconnect_config.detect_after;
        let paused = self.sending_paused.clone();

        let handle = tokio::spawn(async move {
            let sequence = AtomicU32::new(3_000_000);
            let disconnected = || {
                ConnectionState::from_u8(state.load(Ordering::SeqCst))
                    == ConnectionState::Disconnected
            };
            let silent = || {
                last_received
                    .lock()
                    .map(|last| last.elapsed() >= silent_after)
                    .unwrap_or(true)
            };
            let pause = prediction.pause();
            let mut retry = 0;
            let mut first = true;
            let mut heard = packets_received.load(Ordering::Relaxed);
            loop {
                if disconnected() {
                    return;
                }
                // A peer heard since the last try starts the retries over
                let now_heard = packets_received.load(Ordering::Relaxed);
                if now_heard > heard {
                    retry = 0;
                    heard = now_heard;
                }
                // A quiet of the punch schedule sends nothing, the guesses included
                if paused.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                // A peer that is heard has no port to guess. The first try is not held back:
                // the check that found nothing has just ended.
                if !first && !silent() {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                first = false;

                let heard_before = packets_received.load(Ordering::Relaxed);
                let mut answered = false;
                'try_: for pass in prediction.passes() {
                    for addr in pass.addrs {
                        if disconnected() {
                            return;
                        }
                        if packets_received.load(Ordering::Relaxed) > heard_before {
                            answered = true;
                            break 'try_;
                        }
                        for attempt in 0..=SEND_RETRIES {
                            let packet = Packet::route_probe(
                                sequence.fetch_add(1, Ordering::Relaxed),
                                prediction.token(addr.port()),
                            );
                            match send_sealed(&transport, &secure_link, packet, addr).await {
                                Ok(_) => break,
                                Err(e) if attempt == SEND_RETRIES => {
                                    debug!("Failed to probe the guessed port {}: {}", addr, e)
                                }
                                Err(_) => tokio::time::sleep(SEND_RETRY_INTERVAL).await,
                            }
                        }
                        tokio::time::sleep(pass.spacing).await;
                    }
                    tokio::time::sleep(pause).await;
                }
                if answered {
                    continue;
                }
                let Some(wait) = prediction.retry_after(retry) else {
                    // The tries are used up: nothing more is sent until the peer is heard
                    debug!("Gave up guessing the peer's port until it is heard");
                    while packets_received.load(Ordering::Relaxed) <= heard_before {
                        if disconnected() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    continue;
                };
                retry += 1;
                tokio::time::sleep(wait).await;
            }
        });

        self.prediction_handle = Some(handle);
    }

    /// Sends our key to the peer until it has shown it has it
    ///
    /// Every peer sends its key as soon as it is connected and again every
    /// [`KEY_EXCHANGE_INTERVAL`], so one that comes up later, or a packet lost on the way,
    /// does not leave the link without keys.
    fn start_secure_link_task(&mut self) {
        if let Some(handle) = self.secure_link_handle.take() {
            handle.abort();
        }

        let transport = self.transport.clone();
        let state = self.state.clone();
        let route = self.route.clone();
        let secure_link = self.secure_link.clone();
        let paused = self.sending_paused.clone();
        let sequence = AtomicU32::new(2_000_000);

        let handle = tokio::spawn(async move {
            let mut ticker = interval(KEY_EXCHANGE_INTERVAL);

            loop {
                ticker.tick().await;

                let current_state = ConnectionState::from_u8(state.load(Ordering::SeqCst));
                if current_state == ConnectionState::Disconnected {
                    break;
                }
                if paused.load(Ordering::Relaxed) {
                    continue;
                }
                let remote_addr = route.lock().current;

                if secure_link.wants_key_exchange() {
                    if let Some(key) = secure_link.key_exchange_packet() {
                        if let Err(e) = transport.send_to(&key, remote_addr).await {
                            warn!("Failed to send our key exchange: {}", e);
                        }
                    }
                    // With the keys ours, something encrypted shows the peer it has them too
                    // (the answer to its key can be lost on the way).
                    if secure_link.has_keys() {
                        let packet = Packet::keep_alive(sequence.fetch_add(1, Ordering::Relaxed));
                        if let Err(e) =
                            send_sealed(&transport, &secure_link, packet, remote_addr).await
                        {
                            warn!("Failed to send keep-alive: {}", e);
                        }
                    }
                }
            }
        });

        self.secure_link_handle = Some(handle);
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

    /// A connection that has been told whose key to check its peer by, as every connection is
    /// before it connects. Nobody holds that key, so it agrees keys with nobody unless
    /// [`trust_each_other`] is called on it.
    async fn conn() -> Connection {
        let mut conn = Connection::new("127.0.0.1:0").await.unwrap();
        conn.verify_peer(
            &LinkIdentity::generate(),
            &LinkIdentity::generate().public_key(),
        )
        .unwrap();
        conn
    }

    /// Has two connections check each other by the keys they would have told the server
    fn trust_each_other(a: &mut Connection, b: &mut Connection) {
        let (ours, theirs) = (LinkIdentity::generate(), LinkIdentity::generate());
        a.verify_peer(&ours, &theirs.public_key()).unwrap();
        b.verify_peer(&theirs, &ours.public_key()).unwrap();
    }

    #[tokio::test]
    async fn test_connection_creation() {
        let conn = conn().await;
        assert!(!conn.is_connected());
        assert!(conn.local_addr().port() > 0);
    }

    #[tokio::test]
    async fn test_connection_stats_initial() {
        let conn = conn().await;
        let stats = conn.stats();
        assert_eq!(stats.packets_sent, 0);
        assert_eq!(stats.packets_received, 0);
    }

    #[tokio::test]
    async fn test_connect_with_candidates_empty() {
        let mut conn = conn().await;
        let result = conn.connect_with_candidates(&[]).await;

        assert!(result.is_err());
        match result {
            Err(NetworkError::NoCandidates) => {}
            _ => panic!("Expected NoCandidates error"),
        }
    }

    #[tokio::test]
    async fn test_connect_with_candidates_already_connected() {
        let mut conn1 = conn().await;
        let conn2 = conn().await;

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
        let mut conn1 = conn().await;
        let conn2 = conn().await;

        let candidates = vec![conn2.local_addr()];
        conn1.connect_with_candidates(&candidates).await.unwrap();

        assert!(conn1.is_connected());
    }

    #[tokio::test]
    async fn test_connect_with_multiple_candidates_loopback() {
        // Test with multiple loopback candidates
        let mut conn1 = conn().await;
        let conn2 = conn().await;

        // Create fake candidates, only the real one will respond
        let (_silent, nowhere) = silent();
        let candidates = vec![
            nowhere,            // Fake - won't respond
            conn2.local_addr(), // Real - will respond
        ];

        // This should succeed by finding the working candidate
        conn1.connect_with_candidates(&candidates).await.unwrap();

        assert!(conn1.is_connected());
    }

    /// Verifies: REQ-TEL-015
    #[tokio::test]
    async fn when_a_candidate_answers_the_link_facts_say_the_route_was_confirmed() {
        let mut conn1 = conn().await;
        let mut conn2 = conn().await;
        // conn2 opens with a keep-alive, which conn1's probe wait takes as the answer.
        conn2.connect(conn1.local_addr()).await.unwrap();
        let (_silent, nowhere) = silent();
        let candidates = vec![nowhere, conn2.local_addr()];

        conn1.connect_with_candidates(&candidates).await.unwrap();

        let snapshot = conn1.link_facts().snapshot();
        assert_eq!(conn1.remote_addr(), conn2.local_addr());
        assert_eq!(snapshot.route, Some(super::super::LinkRoute::Loopback));
        assert_eq!(snapshot.route_confirmed, Some(true));
        assert!(snapshot.connect_ms.is_some());
        assert_eq!(snapshot.first_audio_ms, None);
    }

    /// A peer that already sends audio while we probe is one that does not encrypt: a peer
    /// that does holds its audio back until we have shown it we have the keys. Nothing but a
    /// keep-alive is taken from it, before or after the connection is up.
    ///
    /// Verifies: REQ-CON-116
    #[tokio::test]
    async fn when_a_peer_sends_plain_audio_and_latency_info_they_are_neither_played_nor_kept() {
        let mut conn1 = conn().await;
        let old_app = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let heard = Arc::new(AtomicU32::new(0));
        let counted = heard.clone();
        conn1.set_audio_callback(move |_, _, _| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        // The peer is up first: it opens with a keep-alive, which is the answer to conn1's
        // probe, and sends audio and its latency info while conn1 has not started reading.
        let to = conn1.local_addr();
        old_app
            .send_to(&Packet::keep_alive(0).to_bytes(), to)
            .await
            .unwrap();
        for sequence in 0..20 {
            old_app
                .send_to(&Packet::audio(sequence, 0, vec![0u8; 256]).to_bytes(), to)
                .await
                .unwrap();
        }
        let info = LatencyInfoMessage {
            capture_buffer_ms: 1.0,
            playback_buffer_ms: 2.0,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 3.0,
            frame_size: 64,
            sample_rate: 48_000,
            codec: "pcm".to_string(),
            channel_count: 1,
        };
        old_app
            .send_to(&Packet::latency_info(1, &info).to_bytes(), to)
            .await
            .unwrap();
        let (_silent, nowhere) = silent();
        let candidates = vec![nowhere, old_app.local_addr().unwrap()];

        conn1.connect_with_candidates(&candidates).await.unwrap();
        old_app
            .send_to(&Packet::audio(20, 0, vec![0u8; 256]).to_bytes(), to)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(heard.load(Ordering::SeqCst), 0, "plain audio is not played");
        assert!(conn1.peer_latency_info().is_none());
    }

    /// Verifies: REQ-CON-115
    #[tokio::test]
    async fn when_every_probe_is_answered_the_connection_is_up_as_soon_as_the_probes_are_out() {
        let mut conn1 = conn().await;
        let mut conn2 = conn().await;
        conn2.connect(conn1.local_addr()).await.unwrap();
        let (_silent, nowhere) = silent();
        let candidates = vec![nowhere, conn2.local_addr()];

        conn1.connect_with_candidates(&candidates).await.unwrap();

        let snapshot = conn1.link_facts().snapshot();
        assert!(
            snapshot.connect_ms.is_some_and(|ms| ms < 800),
            "the rounds of probes take under a second and then nothing is waited for: {snapshot:?}"
        );
    }

    /// Verifies: REQ-TEL-015
    #[tokio::test]
    async fn when_no_candidate_answers_the_link_facts_say_the_route_was_not_confirmed() {
        let mut conn1 = conn().await;
        let (_silent_a, nowhere_a) = silent();
        let (_silent_b, nowhere_b) = silent();
        let candidates = vec![nowhere_a, nowhere_b];

        conn1.connect_with_candidates(&candidates).await.unwrap();

        let snapshot = conn1.link_facts().snapshot();
        assert_eq!(snapshot.route, Some(super::super::LinkRoute::Loopback));
        assert_eq!(snapshot.route_confirmed, Some(false));
        assert!(
            snapshot.connect_ms.is_some_and(|ms| ms >= 900),
            "the fallback waits out the probe timeout: {snapshot:?}"
        );
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// A loopback address nobody answers from. The socket is held (and never read) for as
    /// long as the test runs, so no other test or process can be handed the port: the
    /// ephemeral range includes any fixed port a test might pick, and whoever got it would
    /// answer the probes.
    fn silent() -> (std::net::UdpSocket, SocketAddr) {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = socket.local_addr().unwrap();
        (socket, at)
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_a_candidate_we_could_not_send_to_answers_the_answer_is_not_taken() {
        let candidates = [addr("192.168.1.10:5000"), addr("100.64.0.2:5000")];
        let keep_alive = Packet::keep_alive(0).to_bytes();

        assert!(!is_probe_answer(
            &candidates,
            &[false, true],
            candidates[0],
            &keep_alive
        ));
        assert!(is_probe_answer(
            &candidates,
            &[false, true],
            candidates[1],
            &keep_alive
        ));
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_answer_comes_from_no_candidate_it_is_not_taken() {
        let candidates = [addr("192.168.1.10:5000")];
        let keep_alive = Packet::keep_alive(0).to_bytes();

        assert!(!is_probe_answer(
            &candidates,
            &[true],
            addr("192.168.1.10:5001"),
            &keep_alive
        ));
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_packet_from_a_candidate_is_not_a_keep_alive_it_is_not_an_answer() {
        let candidates = [addr("192.168.1.10:5000")];
        let audio = Packet::audio(0, 0, vec![0; 4]).to_bytes();

        assert!(!is_probe_answer(
            &candidates,
            &[true],
            candidates[0],
            &audio
        ));
        assert!(!is_probe_answer(
            &candidates,
            &[true],
            candidates[0],
            &[0xff; 3]
        ));
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_peer_has_not_answered_a_packet_from_another_address_moves_the_route_there() {
        let lan = addr("192.168.1.10:5000");
        let vpn = addr("100.64.0.2:5000");
        let mut route = Route::new(lan, vec![(vpn, true)]);

        assert_eq!(route.follow(vpn, Duration::ZERO, true), Some(lan));

        assert_eq!(route.current, vpn);
        assert_eq!(route.others, vec![(lan, true)]);
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_peer_answered_recently_the_route_stays_where_it_is() {
        let lan = addr("192.168.1.10:5000");
        let vpn = addr("100.64.0.2:5000");
        let mut route = Route::new(lan, vec![(vpn, true)]);
        route.answered();

        assert_eq!(route.follow(vpn, Duration::from_secs(60), true), None);

        assert_eq!(route.current, lan);
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_we_could_not_send_to_the_address_the_peer_is_heard_from_the_route_stays_where_it_is() {
        let lan = addr("192.168.1.10:5000");
        let vpn = addr("100.64.0.2:5000");
        let mut route = Route::new(lan, vec![(vpn, false)]);

        assert_eq!(route.follow(vpn, Duration::ZERO, true), None);

        assert_eq!(route.current, lan);
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_a_packet_comes_from_an_address_that_is_not_the_peers_the_route_stays_where_it_is() {
        let lan = addr("192.168.1.10:5000");
        let mut route = Route::new(lan, vec![(addr("100.64.0.2:5000"), true)]);

        assert_eq!(
            route.follow(addr("203.0.113.9:5000"), Duration::ZERO, true),
            None
        );

        assert_eq!(route.current, lan);
    }

    /// Verifies: REQ-CON-170
    #[test]
    fn when_a_packet_comes_from_a_guessed_port_without_proof_the_route_stays_where_it_is() {
        let lan = addr("192.168.1.10:5000");
        let mut route = Route::new(lan, vec![]);
        route.predicted = PortPrediction::from_offered(
            &[addr("203.0.113.7:40000"), addr("203.0.113.7:40001")],
            [7; 32],
            false,
        );
        let guessed = addr("203.0.113.7:40002");
        assert!(route.guessed(guessed));

        assert_eq!(route.follow(guessed, Duration::ZERO, false), None);
        assert_eq!(route.current, lan);

        assert_eq!(route.follow(guessed, Duration::ZERO, true), Some(lan));
        assert_eq!(route.current, guessed);
    }

    /// Verifies: REQ-CON-170
    #[test]
    fn when_a_probe_answer_carries_another_ports_token_it_is_not_the_answer_from_the_guessed_port()
    {
        let mut route = Route::new(addr("192.168.1.10:5000"), vec![]);
        let prediction = PortPrediction::from_offered(
            &[addr("203.0.113.7:40000"), addr("203.0.113.7:40001")],
            [7; 32],
            false,
        )
        .unwrap();
        let token = prediction.token(40002);
        route.predicted = Some(prediction);

        assert!(route.is_answer_from_guessed(token, addr("203.0.113.7:40002")));
        assert!(!route.is_answer_from_guessed(token, addr("203.0.113.7:40003")));
        assert!(!route.is_answer_from_guessed(token ^ 1, addr("203.0.113.7:40002")));
    }

    /// Verifies: REQ-CON-136
    #[test]
    fn when_the_answer_to_the_check_of_an_unoffered_address_comes_from_there_the_route_moves_there()
    {
        let lan = addr("192.168.1.10:5000");
        let moved = addr("192.168.1.10:6000");
        let mut route = Route::new(lan, Vec::new());

        let token = route.start_check(moved, Duration::ZERO).unwrap();
        assert_eq!(
            route.follow(moved, Duration::ZERO, false),
            None,
            "a packet from the address is not enough"
        );
        assert!(route.check_answered(token, moved));

        assert_eq!(route.follow(moved, Duration::ZERO, true), Some(lan));
        assert_eq!(route.current, moved);
    }

    /// Verifies: REQ-CON-175
    #[test]
    fn when_the_answer_to_a_check_comes_from_another_address_or_with_another_token_it_is_not_taken()
    {
        let lan = addr("192.168.1.10:5000");
        let moved = addr("192.168.1.10:6000");
        let other = addr("203.0.113.9:6000");
        let mut route = Route::new(lan, Vec::new());
        let token = route.start_check(moved, Duration::ZERO).unwrap();

        assert!(!route.check_answered(token, other));
        assert!(!route.check_answered(token ^ 1, moved));
        assert!(!route.knows(moved));
        assert_eq!(route.follow(moved, Duration::ZERO, false), None);
        assert_eq!(route.current, lan);
    }

    /// Verifies: REQ-CON-175
    #[test]
    fn when_a_check_was_answered_the_same_answer_is_not_taken_again() {
        let lan = addr("192.168.1.10:5000");
        let moved = addr("192.168.1.10:6000");
        let mut route = Route::new(lan, Vec::new());
        let token = route.start_check(moved, Duration::ZERO).unwrap();

        assert!(route.check_answered(token, moved));
        assert!(!route.check_answered(token, moved));
    }

    /// Verifies: REQ-CON-175
    #[test]
    fn when_a_check_went_out_a_moment_ago_the_address_is_not_checked_again_until_the_retry() {
        let lan = addr("192.168.1.10:5000");
        let moved = addr("192.168.1.10:6000");
        let mut route = Route::new(lan, Vec::new());
        let first = route.start_check(moved, Duration::ZERO).unwrap();

        assert_eq!(route.start_check(moved, Duration::ZERO), None);
        route.checks[0].sent_at -= PATH_CHECK_RETRY;
        let second = route.start_check(moved, Duration::ZERO).unwrap();

        assert!(!route.check_answered(first, moved), "the old token is gone");
        assert!(route.check_answered(second, moved));
    }

    /// Verifies: REQ-CON-175
    #[test]
    fn when_many_addresses_are_heard_from_only_a_few_checks_are_out_at_once() {
        let lan = addr("192.168.1.10:5000");
        let mut route = Route::new(lan, Vec::new());

        let sent = (0..20u16)
            .filter(|i| {
                route
                    .start_check(addr(&format!("198.51.100.1:{}", 7000 + i)), Duration::ZERO)
                    .is_some()
            })
            .count();

        assert_eq!(sent, MAX_PATH_CHECKS);
        route.checks[0].sent_at -= PATH_CHECK_LIFETIME;
        assert!(route
            .start_check(addr("198.51.100.1:9000"), Duration::ZERO)
            .is_some());
    }

    /// Verifies: REQ-CON-175
    #[test]
    fn when_the_answer_to_a_check_comes_after_it_expired_it_is_not_taken() {
        let lan = addr("192.168.1.10:5000");
        let moved = addr("192.168.1.10:6000");
        let mut route = Route::new(lan, Vec::new());
        let token = route.start_check(moved, Duration::ZERO).unwrap();
        route.checks[0].sent_at -= PATH_CHECK_LIFETIME;

        assert!(!route.check_answered(token, moved));
        assert!(!route.knows(moved));
    }

    /// Verifies: REQ-CON-136
    #[test]
    fn when_the_peer_answered_recently_an_unoffered_address_is_not_checked() {
        let lan = addr("192.168.1.10:5000");
        let moved = addr("192.168.1.10:6000");
        let mut route = Route::new(lan, Vec::new());
        route.answered();

        assert_eq!(route.start_check(moved, Duration::from_secs(60)), None);

        assert!(!route.knows(moved));
        assert_eq!(route.follow(moved, Duration::from_secs(60), true), None);
        assert_eq!(route.current, lan);
    }

    /// Verifies: REQ-CON-136
    #[test]
    fn when_the_address_is_already_known_it_is_not_checked() {
        let lan = addr("192.168.1.10:5000");
        let vpn = addr("100.64.0.2:5000");
        let mut route = Route::new(lan, vec![(vpn, false)]);

        assert_eq!(route.start_check(vpn, Duration::ZERO), None);
        assert_eq!(route.start_check(lan, Duration::ZERO), None);

        assert_eq!(route.others, vec![(vpn, false)]);
    }

    /// Verifies: REQ-CON-172
    #[test]
    fn when_no_candidate_answered_the_link_goes_to_the_public_address_before_a_private_one() {
        let private: SocketAddr = "192.168.1.20:5000".parse().unwrap();
        let public: SocketAddr = "203.0.113.7:5000".parse().unwrap();
        let other_public: SocketAddr = "198.51.100.9:6000".parse().unwrap();

        assert_eq!(
            fallback_candidate(&[private, public, other_public], |_| true),
            public
        );
        assert_eq!(
            fallback_candidate(&[private, public, other_public], |i| i != 1),
            other_public,
            "an address that cannot be sent to is not chosen"
        );
        assert_eq!(
            fallback_candidate(&[private, public], |i| i == 0),
            private,
            "with no public address to send to, the private one is what is left"
        );
        assert_eq!(fallback_candidate(&[private, public], |_| false), private);
    }

    /// Verifies: REQ-CON-144
    #[test]
    fn when_the_peer_stays_silent_the_other_addresses_are_left_alone_after_a_while() {
        let after = Duration::from_secs(2);

        assert!(!probes_others(Duration::from_secs(1), after));
        assert!(probes_others(after, after));
        assert!(probes_others(
            after + PROBE_OTHERS_FOR - Duration::from_millis(1),
            after
        ));
        assert!(!probes_others(after + PROBE_OTHERS_FOR, after));
        assert!(!probes_others(Duration::from_secs(3600), after));
    }

    /// Verifies: REQ-CON-114
    #[tokio::test]
    async fn when_no_candidate_answers_the_first_one_we_could_send_to_is_used() {
        let mut conn = conn().await;
        // An IPv6 address cannot be sent to from the IPv4 audio socket.
        let unreachable = addr("[::1]:59997");
        let (_held, quiet) = silent();

        conn.connect_with_candidates(&[unreachable, quiet])
            .await
            .unwrap();

        assert_eq!(conn.remote_addr(), quiet);
    }

    /// The reply to the probe of `round` that went to candidate `index`, heard from the
    /// address `from`
    fn reply_from(
        probing: &mut Probing<'_>,
        from: usize,
        index: usize,
        round: usize,
        now: Instant,
    ) {
        let bytes = Packet::route_probe_reply(0, probe_token(round, index)).to_bytes();
        probing.hear(probing.candidates[from], &bytes, now);
    }

    /// When the probe of `round` goes out to candidate `index`, after the check starts
    fn probe_time(round: usize, index: usize) -> Duration {
        ROUND_INTERVAL * round as u32 + CANDIDATE_DELAY * index as u32
    }

    fn probed(candidates: &[SocketAddr], at: Instant) -> Probing<'_> {
        let mut probing = Probing::new(candidates);
        for round in 0..PROBE_ROUNDS {
            for i in 0..candidates.len() {
                probing.sent(i, round, at + probe_time(round, i));
            }
        }
        probing
    }

    /// Every probe to candidate `index` answered, the one of round `r` after `rtts[r]`
    fn answer_probes(probing: &mut Probing<'_>, t0: Instant, index: usize, rtts: [u64; 5]) {
        for (round, rtt) in rtts.into_iter().enumerate() {
            let at = t0 + probe_time(round, index) + Duration::from_micros(rtt);
            reply_from(probing, index, index, round, at);
        }
    }

    fn settled(probing: &Probing<'_>, t0: Instant) -> Verdict {
        probing.assess(t0 + Duration::from_secs(2))
    }

    fn chosen(verdict: Verdict, candidates: &[SocketAddr]) -> SocketAddr {
        match verdict {
            Verdict::Fastest { index, .. } => candidates[index],
            Verdict::WaitUntil(_) => panic!("still waiting"),
            Verdict::NoAnswer => panic!("nothing answered"),
        }
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_relayed_tailscale_address_and_a_public_one_both_reply_the_shorter_round_trip_is_used()
    {
        let candidates = [addr("100.68.50.7:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        // Tailscale is listed first and answers with a relay's round trips; the public
        // address answers in 6 ms.
        answer_probes(
            &mut probing,
            t0,
            0,
            [120_000, 90_000, 150_000, 43_000, 187_000],
        );
        answer_probes(&mut probing, t0, 1, [6_000; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[1]);
    }

    /// A Tailscale path that looks a little faster in a 1 s check can still be a relay that
    /// stalls afterwards; the public address that answers as well is used.
    ///
    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_tailscale_address_is_only_a_little_faster_than_a_public_one_the_public_one_is_used() {
        let candidates = [addr("100.68.50.7:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        answer_probes(&mut probing, t0, 0, [48_000; 5]);
        answer_probes(&mut probing, t0, 1, [52_000; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[1]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_tailscale_address_is_much_faster_than_a_public_one_the_tailscale_one_is_used() {
        let candidates = [addr("100.68.50.7:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        answer_probes(&mut probing, t0, 0, [20_000; 5]);
        answer_probes(&mut probing, t0, 1, [52_000; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[0]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_nearer_kind_of_route_is_much_slower_the_faster_route_is_used() {
        let candidates = [addr("192.168.1.20:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        // A LAN address that replies, but slowly.
        answer_probes(&mut probing, t0, 0, [40_000; 5]);
        answer_probes(&mut probing, t0, 1, [8_000; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[1]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_round_trips_differ_by_less_than_the_noise_the_nearer_kind_of_route_is_used() {
        let candidates = [addr("203.0.113.7:5000"), addr("192.168.1.20:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        answer_probes(&mut probing, t0, 0, [800; 5]);
        answer_probes(&mut probing, t0, 1, [1_700; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[1]);
    }

    /// One lucky probe on a path that swings must not decide the route: the median does.
    ///
    /// Verifies: REQ-CON-115
    #[test]
    fn when_one_probe_of_a_slow_route_is_lucky_the_route_that_is_steady_is_used() {
        let candidates = [addr("100.68.50.7:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        answer_probes(
            &mut probing,
            t0,
            0,
            [4_000, 120_000, 150_000, 90_000, 187_000],
        );
        answer_probes(&mut probing, t0, 1, [30_000; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[1]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_route_answers_fewer_than_half_its_probes_a_slower_one_that_answers_all_is_used() {
        let candidates = [addr("203.0.113.7:5000"), addr("203.0.113.8:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        // Only the first of five probes to the first candidate comes back.
        reply_from(
            &mut probing,
            0,
            0,
            0,
            t0 + probe_time(0, 0) + Duration::from_millis(5),
        );
        answer_probes(&mut probing, t0, 1, [40_000; 5]);

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[1]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_route_answers_less_than_half_its_probes_but_nothing_else_answers_it_is_used() {
        let candidates = [addr("203.0.113.7:5000"), addr("203.0.113.8:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        reply_from(
            &mut probing,
            0,
            0,
            2,
            t0 + probe_time(2, 0) + Duration::from_millis(5),
        );

        let verdict = settled(&probing, t0);

        assert_eq!(chosen(verdict, &candidates), candidates[0]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_candidate_probed_later_may_still_be_faster_the_check_waits_for_it() {
        let candidates = [addr("203.0.113.7:5000"), addr("192.168.1.20:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        // The first candidate answers in 30 ms every time; the second has not answered yet.
        answer_probes(&mut probing, t0, 0, [30_000; 5]);

        // The last probe to the second went out at 4 * 80 + 50 ms, so it has until 30 ms
        // (plus the noise) after that to beat 30 ms
        let last = t0 + probe_time(PROBE_ROUNDS - 1, 1);
        let early = probing.assess(last + Duration::from_millis(10));
        let late = probing.assess(last + Duration::from_millis(40));

        assert!(matches!(early, Verdict::WaitUntil(_)));
        assert_eq!(chosen(late, &candidates), candidates[0]);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_reply_names_another_candidates_token_it_is_not_counted() {
        let candidates = [addr("203.0.113.7:5000"), addr("203.0.113.8:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);

        reply_from(&mut probing, 0, 1, 0, t0 + Duration::from_millis(5));

        assert!(probing.rtt.iter().flatten().all(Option::is_none));
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_reply_names_another_round_it_is_timed_from_the_probe_of_that_round() {
        let candidates = [addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);

        reply_from(
            &mut probing,
            0,
            0,
            3,
            t0 + probe_time(3, 0) + Duration::from_millis(7),
        );

        assert_eq!(probing.rtt[0][3], Some(Duration::from_millis(7)));
        assert_eq!(probing.rtt[0][0], None);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_the_peer_asks_for_a_reply_the_token_is_handed_back_and_a_plain_keep_alive_asks_for_none(
    ) {
        let candidates = [addr("203.0.113.7:5000")];
        let mut probing = probed(&candidates, Instant::now());
        let now = Instant::now();

        let asked = probing.hear(candidates[0], &Packet::route_probe(0, 42).to_bytes(), now);
        let plain = probing.hear(candidates[0], &Packet::keep_alive(1).to_bytes(), now);

        assert_eq!(asked.map(|answer| answer.reply_token), Some(42));
        assert_eq!(plain, None);
    }

    /// Verifies: REQ-CON-132
    #[test]
    fn when_the_peer_probes_an_address_none_of_ours_was_answered_by_a_probe_goes_back_to_it() {
        let candidates = [addr("203.0.113.7:5000")];
        let mut probing = probed(&candidates, Instant::now());
        let asked = Packet::route_probe(0, 42).to_bytes();

        let answer = probing.hear(candidates[0], &asked, Instant::now()).unwrap();

        assert_eq!(answer.reply_token, 42);
        assert_eq!(answer.probe_back, Some(probe_token(PROBE_ROUNDS, 0)));
    }

    /// Verifies: REQ-CON-132
    #[test]
    fn when_the_peer_keeps_probing_an_address_no_more_than_the_limit_of_probes_go_back() {
        let candidates = [addr("203.0.113.7:5000")];
        let mut probing = probed(&candidates, Instant::now());
        let asked = Packet::route_probe(0, 42).to_bytes();

        let sent_back = (0..TRIGGERED_PROBES + 4)
            .filter_map(|_| probing.hear(candidates[0], &asked, Instant::now()))
            .filter(|answer| answer.probe_back.is_some())
            .count();

        assert_eq!(sent_back, TRIGGERED_PROBES);
    }

    /// Verifies: REQ-CON-132
    #[test]
    fn when_one_of_our_probes_to_the_address_was_answered_none_goes_back_for_the_peers() {
        let candidates = [addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        reply_from(&mut probing, 0, 0, 0, t0 + Duration::from_millis(30));
        let asked = Packet::route_probe(0, 42).to_bytes();

        let answer = probing.hear(candidates[0], &asked, Instant::now()).unwrap();

        assert_eq!(answer.probe_back, None);
    }

    /// Verifies: REQ-CON-132
    #[test]
    fn when_the_probe_sent_back_is_answered_the_address_is_a_route() {
        let candidates = [addr("100.68.50.7:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        let asked = Packet::route_probe(0, 42).to_bytes();
        let back = probing
            .hear(candidates[1], &asked, t0 + Duration::from_millis(400))
            .and_then(|answer| answer.probe_back)
            .unwrap();
        probing.sent(1, (back >> 16) as usize, t0 + Duration::from_millis(400));

        reply_from(
            &mut probing,
            1,
            1,
            PROBE_ROUNDS,
            t0 + Duration::from_millis(430),
        );

        assert_eq!(chosen(settled(&probing, t0), &candidates), candidates[1]);
    }

    /// A peer that answers no probe is not chosen from its own keep-alives: only a measured
    /// round trip makes a route.
    ///
    /// Verifies: REQ-CON-115
    #[test]
    fn when_the_peer_answers_no_probe_its_keep_alives_do_not_make_a_route() {
        let candidates = [addr("100.68.50.7:5000"), addr("203.0.113.7:5000")];
        let t0 = Instant::now();
        let mut probing = probed(&candidates, t0);
        let keep_alive = Packet::keep_alive(0).to_bytes();
        probing.hear(candidates[0], &keep_alive, t0 + Duration::from_millis(10));
        probing.hear(candidates[1], &keep_alive, t0 + Duration::from_millis(200));

        assert!(matches!(settled(&probing, t0), Verdict::NoAnswer));
    }

    /// A stand-in for a peer's address: answers the route probes after `delays`, the first
    /// probe after the first delay, the next after the next, and round again.
    async fn peer_answering_probes_after(delays: &[u64]) -> SocketAddr {
        let delays: Vec<Duration> = delays.iter().map(|&ms| Duration::from_millis(ms)).collect();
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let at = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 256];
            let mut asked = 0;
            while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                let token = Packet::from_bytes(&buf[..len])
                    .and_then(|packet| packet.route_probe_token())
                    .filter(|(reply, _)| !reply)
                    .map(|(_, token)| token);
                if let Some(token) = token {
                    tokio::time::sleep(delays[asked % delays.len()]).await;
                    asked += 1;
                    let _ = socket
                        .send_to(&Packet::route_probe_reply(0, token).to_bytes(), from)
                        .await;
                }
            }
        });
        at
    }

    /// Verifies: REQ-CON-115
    #[tokio::test]
    async fn when_two_addresses_of_the_same_kind_reply_at_different_speeds_the_faster_is_used() {
        // The slow one is probed first and its reply arrives first (at 60 ms against
        // 50 + 20 ms), so only the round trips tell the two apart.
        let slow = peer_answering_probes_after(&[60]).await;
        let fast = peer_answering_probes_after(&[20]).await;
        let mut conn = conn().await;

        conn.connect_with_candidates(&[slow, fast]).await.unwrap();

        assert_eq!(conn.remote_addr(), fast);
        assert_eq!(conn.link_facts().snapshot().route_confirmed, Some(true));
    }

    /// The first probe of the nearest-looking candidate is answered at once and every other
    /// is slow; the other candidate is steady. Judged on one probe the first would win.
    ///
    /// Verifies: REQ-CON-115
    #[tokio::test]
    async fn when_one_probe_of_a_route_is_answered_at_once_and_the_rest_slowly_the_steady_route_is_used(
    ) {
        let lucky = peer_answering_probes_after(&[0, 70, 70, 70, 70]).await;
        let steady = peer_answering_probes_after(&[25]).await;
        let mut conn = conn().await;

        conn.connect_with_candidates(&[lucky, steady])
            .await
            .unwrap();

        assert_eq!(conn.remote_addr(), steady);
    }

    /// Verifies: REQ-CON-115
    #[tokio::test]
    async fn when_both_peers_probe_each_other_each_answers_the_others_probes_and_both_connect() {
        let mut a = conn().await;
        let mut b = conn().await;
        let (a_addr, b_addr) = (a.local_addr(), b.local_addr());
        let (_silent, nowhere) = silent();

        let (for_a, for_b) = ([nowhere, b_addr], [nowhere, a_addr]);

        let (from_a, from_b) = tokio::join!(
            a.connect_with_candidates(&for_a),
            b.connect_with_candidates(&for_b)
        );

        from_a.unwrap();
        from_b.unwrap();
        assert_eq!(a.remote_addr(), b_addr);
        assert_eq!(b.remote_addr(), a_addr);
        for conn in [&a, &b] {
            let snapshot = conn.link_facts().snapshot();
            assert_eq!(snapshot.route_confirmed, Some(true));
            assert!(
                snapshot.connect_ms.is_some_and(|ms| ms < 800),
                "the probes take their rounds and then no grace: {snapshot:?}"
            );
        }
    }

    /// A NAT in front of `inside` that lets `outside` in only once `inside` has sent to it.
    /// Returns the address `outside` is to use for `inside`, and the address `inside` is to
    /// use for `outside`.
    async fn address_restricted_nat(
        inside: SocketAddr,
        outside: SocketAddr,
    ) -> (SocketAddr, SocketAddr) {
        let public = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let private = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let (public_at, private_at) = (public.local_addr().unwrap(), private.local_addr().unwrap());
        let opened = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (public, private, opened) = (public.clone(), private.clone(), opened.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, from)) = private.recv_from(&mut buf).await {
                    if from == inside {
                        opened.store(true, Ordering::SeqCst);
                        let _ = public.send_to(&buf[..len], outside).await;
                    }
                }
            });
        }
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((len, from)) = public.recv_from(&mut buf).await {
                if from == outside && opened.load(Ordering::SeqCst) {
                    let _ = private.send_to(&buf[..len], inside).await;
                }
            }
        });
        (public_at, private_at)
    }

    /// A peer behind a NAT hears nothing from the other until it has sent to it, so whoever
    /// probes first is not heard at all. Each side's own probes were all dropped or all
    /// answered by the time the other's first one arrived, depending on who was late.
    ///
    /// Verifies: REQ-CON-132
    #[tokio::test]
    async fn when_a_nat_lets_the_peer_in_only_after_we_sent_to_it_both_connect_however_late_either_probes(
    ) {
        for (outside_late, inside_late) in [(0, 0), (0, 150), (0, 400), (0, 900), (400, 0)] {
            let mut outside = conn().await;
            let mut inside = conn().await;
            let (seen_from_outside, seen_from_inside) =
                address_restricted_nat(inside.local_addr(), outside.local_addr()).await;
            let (_silent_outside, nowhere_outside) = silent();
            let (_silent_inside, nowhere_inside) = silent();
            let outside_candidates = [nowhere_outside, seen_from_outside];
            let inside_candidates = [nowhere_inside, seen_from_inside];

            let (from_outside, from_inside) = tokio::join!(
                async {
                    tokio::time::sleep(Duration::from_millis(outside_late)).await;
                    outside.connect_with_candidates(&outside_candidates).await
                },
                async {
                    tokio::time::sleep(Duration::from_millis(inside_late)).await;
                    inside.connect_with_candidates(&inside_candidates).await
                }
            );

            from_outside.unwrap();
            from_inside.unwrap();
            let when = format!("outside probes {outside_late} ms late, inside {inside_late} ms");
            assert_eq!(outside.remote_addr(), seen_from_outside, "{when}");
            assert_eq!(inside.remote_addr(), seen_from_inside, "{when}");
            for conn in [&outside, &inside] {
                let snapshot = conn.link_facts().snapshot();
                assert_eq!(snapshot.route_confirmed, Some(true), "{when}: {snapshot:?}");
            }
        }
    }

    /// A NAT in front of `inside` that gives each address `inside` sends to a port of its own on
    /// `127.0.0.2`, and lets in from an address only what came from the address that port was
    /// made for (a symmetric NAT that hands out the next free port, or the free port of
    /// `port_after` when `inside` sends to more addresses than are named).
    ///
    /// `inside` sends to `fronts[i]` to send to `destinations[i]`; a destination that is `None`
    /// is one nobody answers at (a private address the peer offered), but it still takes a port.
    /// The `first_ports` ports are taken before, as by the servers STUN is asked. Returns the
    /// fronts and the ports taken by STUN.
    struct SymmetricNat {
        fronts: Vec<SocketAddr>,
        taken_by_stun: Vec<SocketAddr>,
        mapped: Arc<Mutex<Vec<Option<SocketAddr>>>>,
    }

    async fn symmetric_nat(
        inside: SocketAddr,
        destinations: Vec<Option<SocketAddr>>,
        first_ports: usize,
    ) -> SymmetricNat {
        // Ports in a row, so that "the next free one" is the next number
        let mut pool: Vec<Arc<tokio::net::UdpSocket>> = Vec::new();
        'search: loop {
            pool.clear();
            let base: u16 = 20000 + (rand::random::<u16>() % 30000);
            for i in 0..24u16 {
                match tokio::net::UdpSocket::bind(("127.0.0.2", base + i)).await {
                    Ok(socket) => pool.push(Arc::new(socket)),
                    Err(_) => continue 'search,
                }
            }
            break;
        }
        let ports: Vec<SocketAddr> = pool.iter().map(|s| s.local_addr().unwrap()).collect();
        let mut fronts = Vec::new();
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(first_ports));
        // Which front each port was made for
        let owner: Arc<Mutex<Vec<Option<usize>>>> = Arc::new(Mutex::new(vec![None; pool.len()]));
        let mapped: Arc<Mutex<Vec<Option<SocketAddr>>>> =
            Arc::new(Mutex::new(vec![None; destinations.len()]));
        let front_sockets: Vec<Arc<tokio::net::UdpSocket>> = {
            let mut sockets = Vec::new();
            for _ in &destinations {
                sockets.push(Arc::new(
                    tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap(),
                ));
            }
            sockets
        };
        for (i, front) in front_sockets.iter().enumerate() {
            fronts.push(front.local_addr().unwrap());
            let (front, pool, ports, next, owner, mapped) = (
                front.clone(),
                pool.clone(),
                ports.clone(),
                next.clone(),
                owner.clone(),
                mapped.clone(),
            );
            let destination = destinations[i];
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, from)) = front.recv_from(&mut buf).await {
                    if from != inside {
                        continue;
                    }
                    let index = {
                        let mut mapped = mapped.lock();
                        match mapped[i] {
                            Some(addr) => ports.iter().position(|p| *p == addr).unwrap(),
                            None => {
                                let index = next.fetch_add(1, Ordering::SeqCst);
                                mapped[i] = Some(ports[index]);
                                owner.lock()[index] = Some(i);
                                index
                            }
                        }
                    };
                    if let Some(destination) = destination {
                        let _ = pool[index].send_to(&buf[..len], destination).await;
                    }
                }
            });
        }
        for (index, socket) in pool.iter().enumerate() {
            let (socket, owner, fronts_to_inside) =
                (socket.clone(), owner.clone(), front_sockets.clone());
            let destinations = destinations.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                    let made_for = owner.lock()[index];
                    if let Some(i) = made_for {
                        if destinations[i] == Some(from) {
                            let _ = fronts_to_inside[i].send_to(&buf[..len], inside).await;
                        }
                    }
                }
            });
        }
        SymmetricNat {
            fronts,
            taken_by_stun: ports[..first_ports].to_vec(),
            mapped,
        }
    }

    /// A peer behind a NAT that gives each destination a port of its own offers the ports STUN
    /// saw, which are not the port it sends from. The other side guesses the port, and the
    /// guess that is right is answered.
    ///
    /// Verifies: REQ-CON-170
    #[tokio::test]
    async fn when_the_peer_is_behind_a_nat_that_gives_each_destination_its_own_port_the_route_goes_to_the_port_it_was_given(
    ) {
        let mut behind_nat = conn().await;
        let mut guessing = conn().await;
        guessing.predict_ports_on_loopback();
        trust_each_other(&mut behind_nat, &mut guessing);
        // Two private addresses it offered before the one that is `guessing`'s take ports too
        let nat = symmetric_nat(
            behind_nat.local_addr(),
            vec![None, None, Some(guessing.local_addr())],
            2,
        )
        .await;
        let (_silent, nowhere) = silent();
        let offered_by_nat = nat.taken_by_stun.clone();
        let mut behind_nat_candidates = nat.fronts.clone();
        behind_nat_candidates.insert(0, nowhere);
        let guessing_candidates = [nowhere, offered_by_nat[0], offered_by_nat[1]];
        let (from_nat, from_guessing) = tokio::join!(
            behind_nat.connect_with_candidates(&behind_nat_candidates),
            guessing.connect_with_candidates(&guessing_candidates)
        );
        from_nat.unwrap();
        from_guessing.unwrap();

        let given = nat.mapped.lock()[2].expect("the NAT gave the peer's address a port");
        let moved = async {
            while guessing.remote_addr() != given {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(6), moved)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "the route is {} and not {} (offered {:?})",
                    guessing.remote_addr(),
                    given,
                    offered_by_nat
                )
            });
        assert!(!offered_by_nat.contains(&given));
    }

    /// A peer that offers one port per public address has nothing to guess, and a peer that
    /// answers needs none.
    ///
    /// Verifies: REQ-CON-170
    #[tokio::test]
    async fn when_the_peer_is_not_behind_such_a_nat_no_port_is_guessed() {
        let mut a = conn().await;
        let mut b = conn().await;
        trust_each_other(&mut a, &mut b);
        let (_silent, nowhere) = silent();
        let b_at = b.local_addr();
        let a_at = a.local_addr();
        let (a_candidates, b_candidates) = ([nowhere, b_at], [nowhere, a_at]);
        let (from_a, from_b) = tokio::join!(
            a.connect_with_candidates(&a_candidates),
            b.connect_with_candidates(&b_candidates)
        );
        from_a.unwrap();
        from_b.unwrap();
        assert!(a.prediction_handle.is_none());
        assert!(b.prediction_handle.is_none());
        assert_eq!(a.route.lock().predicted, None);
    }

    /// Verifies: REQ-CON-115
    #[tokio::test]
    async fn when_a_peer_that_is_already_connected_gets_a_route_probe_it_replies_with_the_token() {
        let mut conn = conn().await;
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        conn.connect(peer.local_addr().unwrap()).await.unwrap();

        peer.send_to(&Packet::route_probe(0, 9).to_bytes(), conn.local_addr())
            .await
            .unwrap();

        let reply = tokio::time::timeout(Duration::from_secs(2), async {
            let mut buf = [0u8; 256];
            loop {
                let (len, _) = peer.recv_from(&mut buf).await.unwrap();
                let packet = Packet::from_bytes(&buf[..len]).unwrap();
                if let Some((true, token)) = packet.route_probe_token() {
                    return token;
                }
            }
        })
        .await
        .expect("the reply should come");
        assert_eq!(reply, 9);
    }

    /// The silence is counted from the moment the link is up, not from the moment the
    /// connection was created: setup before the probes can take seconds, and probe answers
    /// never reach the monitor.
    ///
    /// Verifies: REQ-CON-110
    #[tokio::test]
    async fn when_the_link_comes_up_long_after_the_connection_was_created_it_is_not_reported_silent(
    ) {
        let quick = ReconnectConfig {
            keep_alive_interval: Duration::from_millis(100),
            detect_after: Duration::from_millis(400),
            give_up_after: Duration::from_secs(30),
            check_interval: Duration::from_millis(25),
        };
        let mut conn = conn().await;
        conn.set_reconnect_config(quick);
        let states = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = states.clone();
        conn.set_state_change_callback(move |state| seen.lock().unwrap().push(state));
        let peer = peer_answering_probes_after(&[0]).await;
        tokio::time::sleep(Duration::from_millis(600)).await;

        let (_silent, nowhere) = silent();
        conn.connect_with_candidates(&[nowhere, peer])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(
            !states
                .lock()
                .unwrap()
                .contains(&ConnectionState::Reconnecting),
            "{:?}",
            states.lock().unwrap()
        );
    }

    /// Verifies: REQ-CON-114
    #[tokio::test]
    async fn when_the_peer_answers_nothing_on_the_chosen_address_but_is_heard_elsewhere_sends_go_there(
    ) {
        let quick = ReconnectConfig {
            keep_alive_interval: Duration::from_millis(100),
            detect_after: Duration::from_millis(300),
            give_up_after: Duration::from_secs(30),
            check_interval: Duration::from_millis(50),
        };
        let mut conn1 = conn().await;
        conn1.set_reconnect_config(quick);
        let mut conn2 = conn().await;
        conn2.set_reconnect_config(quick);
        trust_each_other(&mut conn1, &mut conn2);
        let (_held, quiet) = silent();
        // Nobody answers the probes yet, so the first candidate is taken.
        conn1
            .connect_with_candidates(&[quiet, conn2.local_addr()])
            .await
            .unwrap();
        assert_eq!(conn1.remote_addr(), quiet);

        // The peer comes up afterwards and is heard from its own address.
        conn2.connect(conn1.local_addr()).await.unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            while conn1.remote_addr() != conn2.local_addr() || conn1.rtt_ms().is_none() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("sends should move to the address the peer is heard from, and be answered");
        assert_eq!(conn1.state(), ConnectionState::Connected);
    }

    /// A relay between `a` and `b` at its own address that hands every datagram on after `delay`
    async fn delaying_relay(a: SocketAddr, b: SocketAddr, delay: Duration) -> SocketAddr {
        let socket = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let relay = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                let to = if from == a { b } else { a };
                let datagram = buf[..len].to_vec();
                let socket = socket.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = socket.send_to(&datagram, to).await;
                });
            }
        });
        relay
    }

    /// Two peers joined over two paths with the same round trip, longer than the keep-alive
    /// interval (the 10/7 tether's 1.3 s round trip on a 1 s interval, at a tenth). The keys
    /// take a round trip to agree, so the first ping is answered later than the configured wait
    /// after the route was taken up. That silence is not the peer failing to answer: neither
    /// peer may leave the route it chose.
    ///
    /// Verifies: REQ-CON-114
    #[tokio::test]
    async fn when_the_round_trip_is_longer_than_the_keep_alive_interval_the_route_chosen_is_kept() {
        let config = ReconnectConfig {
            keep_alive_interval: Duration::from_millis(100),
            detect_after: Duration::from_millis(200),
            give_up_after: Duration::from_secs(30),
            check_interval: Duration::from_millis(50),
        };
        let mut a = conn().await;
        let mut b = conn().await;
        trust_each_other(&mut a, &mut b);
        a.set_reconnect_config(config);
        b.set_reconnect_config(config);
        let one_way = Duration::from_millis(65);
        let first = delaying_relay(a.local_addr(), b.local_addr(), one_way).await;
        let second = delaying_relay(a.local_addr(), b.local_addr(), one_way).await;

        let candidates = [first, second];
        let (from_a, from_b) = tokio::join!(
            a.connect_with_candidates(&candidates),
            b.connect_with_candidates(&candidates)
        );
        from_a.unwrap();
        from_b.unwrap();
        let (chosen_by_a, chosen_by_b) = (a.remote_addr(), b.remote_addr());

        // Long enough for the first ping to be answered and for several more
        tokio::time::sleep(Duration::from_millis(1500)).await;

        assert_eq!(a.remote_addr(), chosen_by_a, "a left the route it chose");
        assert_eq!(b.remote_addr(), chosen_by_b, "b left the route it chose");
        assert!(a.rtt_ms().is_some() && b.rtt_ms().is_some());
    }

    /// Verifies: REQ-NET-029
    #[tokio::test]
    async fn when_a_frame_is_too_big_for_one_packet_it_is_sent_as_packets_that_fit() {
        use crate::network::SEAL_OVERHEAD;
        use crate::protocol::MAX_PAYLOAD_SIZE;

        let mut sender = conn().await;
        sender
            .set_audio_encoding(AudioEncodingConfig {
                codec_type: CodecType::Pcm,
                sample_rate: 48_000,
                channels: 2,
                frame_size: 256,
                bitrate: 0,
                fec_group_size: None,
            })
            .unwrap();
        let mut receiver = conn().await;
        trust_each_other(&mut sender, &mut receiver);
        let heard = Arc::new(std::sync::Mutex::new(Vec::<(u32, usize, u32, usize)>::new()));
        let record = heard.clone();
        receiver.set_audio_callback(move |sequence, payload, timestamp| {
            let mut samples = [0.0f32; 1024];
            let held = crate::audio::decode_packed(&payload, &mut samples).unwrap();
            record
                .lock()
                .unwrap()
                .push((sequence, payload.len(), timestamp, held));
        });
        sender.connect(receiver.local_addr()).await.unwrap();
        receiver.connect(sender.local_addr()).await.unwrap();

        // Audio waits for the keys to be agreed; a frame of 64 samples (256 bytes) shows
        // when it is let through.
        for _ in 0..50 {
            sender.send_audio(&[0.0f32; 64], 0).await.unwrap();
            if receiver.link_facts().snapshot().first_audio_ms.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        heard.lock().unwrap().clear();

        // 256 stereo frames: 1536 bytes, more than a packet carries.
        sender.send_audio(&[0.25f32; 512], 1000).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while heard.lock().unwrap().len() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the frame should arrive as packets");

        let heard = heard.lock().unwrap().clone();
        assert_eq!(heard.len(), 2, "256 frames go as two packets: {:?}", heard);
        let (first, second) = (heard[0], heard[1]);
        assert_eq!(second.0, first.0 + 1, "numbered one after the other");
        assert_eq!((first.3, second.3), (256, 256), "128 stereo frames each");
        assert_eq!(
            (first.2, second.2),
            (1000, 1128),
            "each timestamped where its audio starts"
        );
        assert!(first.1.max(second.1) + SEAL_OVERHEAD <= MAX_PAYLOAD_SIZE);
    }

    /// Verifies: REQ-TEL-015
    #[tokio::test]
    async fn when_audio_arrives_the_link_facts_have_a_first_audio_time() {
        let mut conn1 = conn().await;
        let mut conn2 = conn().await;
        trust_each_other(&mut conn1, &mut conn2);
        conn1.connect(conn2.local_addr()).await.unwrap();
        conn2.connect(conn1.local_addr()).await.unwrap();
        assert_eq!(conn1.link_facts().snapshot().first_audio_ms, None);

        // Audio waits for the keys to be agreed, so keep sending until one arrives.
        for _ in 0..50 {
            conn2.send_audio(&[0.0f32; 64], 0).await.unwrap();
            if conn1.link_facts().snapshot().first_audio_ms.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert!(conn1.link_facts().snapshot().first_audio_ms.is_some());
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_round_trip_is_short_the_peer_gets_the_configured_wait_to_answer() {
        let config = ReconnectConfig::default().validated();

        assert_eq!(config.answer_wait(None), config.detect_after);
        assert_eq!(
            config.answer_wait(Some(Duration::from_millis(60))),
            config.detect_after
        );
    }

    /// The tethered link of the 10/7 check: round trips of 1.1 to 1.6 s. The first ping
    /// after the route moves goes out up to one interval later and its answer takes a
    /// round trip, so the wait has to outlast both or the route moves back first.
    ///
    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_round_trip_is_long_the_peer_gets_time_for_the_answer_to_the_next_ping() {
        let config = ReconnectConfig::default().validated();
        let slowest = Duration::from_millis(1600);

        let wait = config.answer_wait(Some(slowest));

        assert!(
            wait > config.keep_alive_interval + slowest,
            "{:?} ends before the answer to the next ping can be back",
            wait
        );
        assert!(wait > config.detect_after);
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_the_round_trip_is_extreme_the_wait_stops_at_giving_up() {
        let config = ReconnectConfig::default().validated();

        assert_eq!(
            config.answer_wait(Some(Duration::from_secs(30))),
            config.give_up_after
        );
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn when_a_slow_link_has_just_moved_the_route_it_is_not_moved_back_before_the_answer_is_due() {
        let config = ReconnectConfig::default().validated();
        let lan = addr("192.168.1.10:5000");
        let vpn = addr("100.64.0.2:5000");
        let mut route = Route::new(lan, vec![(vpn, true)]);
        let mut rtt = RttMeasurement::default();
        for ms in [1100.0, 1600.0, 1300.0] {
            rtt.rtt_samples.push_back(ms);
        }
        let wait = config.answer_wait(rtt.slowest_recent());
        assert_eq!(route.follow(vpn, Duration::ZERO, true), Some(lan));

        // The answer is due one interval plus a round trip after the move: still inside
        // the wait, so a packet from the old address does not move the route back
        let due = config.keep_alive_interval + Duration::from_millis(1600);
        assert!(due < wait);
        assert_eq!(route.follow(lan, wait, true), None);
        assert_eq!(route.current, vpn);
    }

    /// A round trip that is growing is in no sample until it ends: the ping still out is
    /// the only sign of it.
    ///
    /// Verifies: REQ-CON-114
    #[test]
    fn when_a_ping_is_still_out_it_counts_for_as_long_as_it_has_been_out() {
        let mut rtt = RttMeasurement::default();
        rtt.rtt_samples.push_back(50.0);
        rtt.pending_pings
            .insert(0, Instant::now() - Duration::from_millis(1200));

        let slowest = rtt.slowest_recent().unwrap();

        assert!(slowest >= Duration::from_millis(1200), "{:?}", slowest);
        assert!(slowest < Duration::from_millis(1500), "{:?}", slowest);
    }

    /// A ping that never comes back must not count for ever: the peer is given up on
    ///
    /// Verifies: REQ-CON-114
    #[test]
    fn when_a_ping_has_been_out_very_long_it_counts_only_up_to_the_cap() {
        let mut rtt = RttMeasurement::default();
        rtt.pending_pings
            .insert(0, Instant::now() - Duration::from_secs(30));

        assert_eq!(rtt.slowest_recent(), Some(UNANSWERED_PING_CAP));
    }

    /// A ping sent before one that was answered was lost, and says nothing of how long an
    /// answer takes
    ///
    /// Verifies: REQ-CON-114
    #[test]
    fn when_a_later_ping_was_answered_an_earlier_one_still_out_does_not_count() {
        let mut rtt = RttMeasurement::default();
        let lost = rtt.create_ping();
        std::thread::sleep(Duration::from_millis(20));
        let answered = rtt.create_ping();
        rtt.pending_pings
            .insert(lost.ping_sequence, Instant::now() - Duration::from_secs(5));
        rtt.process_pong(
            &LatencyPong {
                original_sent_time_us: answered.sent_time_us,
                ping_sequence: answered.ping_sequence,
                wall: None,
            },
            0,
        );

        let slowest = rtt.slowest_recent().unwrap();

        assert!(slowest < Duration::from_millis(500), "{:?}", slowest);
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn the_slowest_recent_round_trip_is_unmeasured_before_the_first_pong() {
        let mut rtt = RttMeasurement::default();
        assert_eq!(rtt.slowest_recent(), None);

        rtt.rtt_samples.push_back(80.0);
        rtt.rtt_samples.push_back(250.0);
        rtt.rtt_samples.push_back(120.0);

        assert_eq!(rtt.slowest_recent(), Some(Duration::from_millis(250)));
    }

    /// Verifies: REQ-CON-114
    #[test]
    fn before_the_first_pong_the_round_trip_the_connectivity_check_timed_is_the_slowest() {
        let mut rtt = RttMeasurement {
            route_round_trip: Some(Duration::from_millis(1300)),
            ..RttMeasurement::default()
        };
        assert_eq!(rtt.slowest_recent(), Some(Duration::from_millis(1300)));

        rtt.rtt_samples.push_back(250.0);

        assert_eq!(rtt.slowest_recent(), Some(Duration::from_millis(250)));
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
        measurement.process_pong(
            &LatencyPong {
                original_sent_time_us: ping.sent_time_us,
                ping_sequence: ping.ping_sequence,
                wall: None,
            },
            0,
        );

        assert!(
            measurement.rtt_ms.is_some(),
            "the first pong must record a sample instead of leaving None"
        );
    }

    /// Each way of the round trip is timed by the clock of the end that sends and the clock of
    /// the end that receives: the ping's way is how long it took to arrive by the answerer's
    /// clock, the pong's way how long by the pinger's.
    ///
    /// Verifies: REQ-LAT-133
    #[test]
    fn when_a_pong_carries_clock_times_each_way_of_the_round_trip_is_timed() {
        let mut measurement = RttMeasurement::default();
        let ping = measurement.create_ping();

        measurement.process_pong(
            &LatencyPong {
                original_sent_time_us: ping.sent_time_us,
                ping_sequence: ping.ping_sequence,
                wall: Some(PongWall {
                    original_sent_wall_us: 1_000_000,
                    ping_received_wall_us: 1_031_000,
                    sent_wall_us: 1_031_500,
                }),
            },
            1_059_500,
        );

        let one_way = measurement
            .one_way
            .expect("the pong carried the clock times");
        assert!((one_way.up_ms - 31.0).abs() < 1e-3, "{one_way:?}");
        assert!((one_way.down_ms - 28.0).abs() < 1e-3, "{one_way:?}");
    }

    /// When the answerer's clock is ahead, the ways come out off by the gap in opposite
    /// directions and still add up to the round trip.
    ///
    /// Verifies: REQ-LAT-133
    #[test]
    fn when_the_peers_clock_is_ahead_the_two_ways_are_off_by_the_gap_and_still_add_up() {
        let mut measurement = RttMeasurement::default();
        let ping = measurement.create_ping();
        let gap_us = 10_000;

        measurement.process_pong(
            &LatencyPong {
                original_sent_time_us: ping.sent_time_us,
                ping_sequence: ping.ping_sequence,
                wall: Some(PongWall {
                    original_sent_wall_us: 1_000_000,
                    ping_received_wall_us: 1_020_000 + gap_us,
                    sent_wall_us: 1_020_000 + gap_us,
                }),
            },
            1_040_000,
        );

        let one_way = measurement.one_way.unwrap();
        assert!((one_way.up_ms - 30.0).abs() < 1e-3, "{one_way:?}");
        assert!((one_way.down_ms - 10.0).abs() < 1e-3, "{one_way:?}");
        assert!((one_way.up_ms + one_way.down_ms - 40.0).abs() < 1e-3);
    }

    /// A peer can put any clock time in its pong; times at the ends of the range are
    /// smoothed in like any other and never stop the receiver.
    ///
    /// Verifies: REQ-LAT-133
    #[test]
    fn when_a_pong_carries_times_at_the_ends_of_the_range_the_ways_are_still_computed() {
        let mut measurement = RttMeasurement::default();
        let ping = measurement.create_ping();

        measurement.process_pong(
            &LatencyPong {
                original_sent_time_us: ping.sent_time_us,
                ping_sequence: ping.ping_sequence,
                wall: Some(PongWall {
                    original_sent_wall_us: u64::MAX,
                    ping_received_wall_us: 0,
                    sent_wall_us: 0,
                }),
            },
            u64::MAX,
        );

        let one_way = measurement.one_way.unwrap();
        assert!(one_way.up_ms < 0.0 && one_way.down_ms > 0.0, "{one_way:?}");
    }

    /// A pong from a peer that sends no clock times leaves the round trip measured and the
    /// ways unknown.
    ///
    /// Verifies: REQ-LAT-133
    #[test]
    fn when_a_pong_carries_no_clock_times_the_ways_stay_unmeasured() {
        let mut measurement = RttMeasurement::default();
        let ping = measurement.create_ping();

        measurement.process_pong(
            &LatencyPong {
                original_sent_time_us: ping.sent_time_us,
                ping_sequence: ping.ping_sequence,
                wall: None,
            },
            5,
        );

        assert!(measurement.rtt_ms.is_some());
        assert_eq!(measurement.one_way, None);
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
