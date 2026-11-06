//! One UDP socket shared by the links to every peer in a room
//!
//! A room has one audio address per participant: the port is bound once, told to the room,
//! and every other participant sends to it. A [`super::Connection`] reads its socket
//! itself, so two of them on one socket would take each other's packets. The mux is the
//! only reader, and hands each datagram to the lane of the peer it came from.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tracing::{debug, error, trace};

use super::error::NetworkError;
use super::transport::{bind_std, canonical_addr, UdpTransport, RECV_BUFFER_SIZE};

/// How many datagrams a lane holds for a reader that has fallen behind. Past that they are
/// dropped, as a socket's own buffer does.
const LANE_CAPACITY: usize = 1024;

/// How many addresses a lane takes from besides the ones the peer was offered at: the ones it was
/// heard from since, once a packet from there got through its link
const LEARNED_ADDRS: usize = 4;

/// Datagrams from an address no link expects that are handed on at once, and how many more per
/// second. A link follows a peer to a new address by the first packet it opens from there, so a few
/// are enough; the rest is what anyone on the internet can send to an open port (ADR-113).
const UNKNOWN_BURST: f64 = 64.0;
const UNKNOWN_PER_SEC: f64 = 32.0;

/// A datagram and where it came from
pub(super) type Datagram = (Vec<u8>, SocketAddr);

/// Lets a limited number of datagrams from unexpected addresses through
struct UnknownSources {
    tokens: f64,
    at: Instant,
}

impl UnknownSources {
    fn new(now: Instant) -> Self {
        Self {
            tokens: UNKNOWN_BURST,
            at: now,
        }
    }

    fn allow(&mut self, now: Instant) -> bool {
        let passed = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + passed * UNKNOWN_PER_SEC).min(UNKNOWN_BURST);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

struct LaneState {
    waiting: VecDeque<Datagram>,
    /// The addresses the peer sends from. `None` while the lane takes nothing: before its
    /// connection starts, and after it ended.
    peer_addrs: Option<Vec<SocketAddr>>,
    /// How many of `peer_addrs` came from `claim`; the rest were learned
    offered: usize,
}

struct LaneShared {
    state: Mutex<LaneState>,
    arrived: Notify,
}

/// The receiving half of a transport that shares its socket: what one peer sends
pub(super) struct Lane {
    shared: Arc<LaneShared>,
    lanes: Arc<Mutex<Vec<Arc<LaneShared>>>>,
}

impl Lane {
    /// Takes what `peer_addrs` send from now on
    pub(super) fn claim(&self, peer_addrs: &[SocketAddr]) {
        let mut state = self.shared.state.lock();
        let learned: Vec<SocketAddr> = match &state.peer_addrs {
            Some(addrs) => addrs[state.offered.min(addrs.len())..]
                .iter()
                .filter(|addr| !peer_addrs.contains(addr))
                .copied()
                .collect(),
            None => Vec::new(),
        };
        state.offered = peer_addrs.len();
        let mut all = peer_addrs.to_vec();
        all.extend(learned);
        state.peer_addrs = Some(all);
    }

    /// Takes what `addr` sends too, because a packet that got through the link came from there.
    /// Keeps the latest few.
    pub(super) fn learn(&self, addr: SocketAddr) {
        let mut state = self.shared.state.lock();
        let offered = state.offered;
        if let Some(addrs) = state.peer_addrs.as_mut() {
            if addrs.contains(&addr) {
                return;
            }
            if addrs.len() >= offered + LEARNED_ADDRS {
                addrs.remove(offered);
            }
            addrs.push(addr);
        }
    }

    /// Takes nothing from now on, and lets go of what is waiting
    pub(super) fn release(&self) {
        {
            let mut state = self.shared.state.lock();
            state.peer_addrs = None;
            state.waiting.clear();
        }
        self.shared.arrived.notify_one();
    }

    /// The next datagram for this lane, or `None` once the lane takes nothing
    ///
    /// Counts against the budget of the task as a read of a socket does, so that a link
    /// with a backlog does not keep the other links on its thread from theirs.
    pub(super) async fn recv(&self) -> Option<Datagram> {
        tokio::task::coop::consume_budget().await;
        loop {
            {
                let mut state = self.shared.state.lock();
                if let Some(datagram) = state.waiting.pop_front() {
                    return Some(datagram);
                }
                state.peer_addrs.as_ref()?;
            }
            self.shared.arrived.notified().await;
        }
    }

    /// A datagram that is already waiting, if there is one
    pub(super) fn try_recv(&self) -> Option<Datagram> {
        self.shared.state.lock().waiting.pop_front()
    }
}

impl Drop for Lane {
    fn drop(&mut self) {
        self.release();
        self.lanes
            .lock()
            .retain(|lane| !Arc::ptr_eq(lane, &self.shared));
    }
}

/// Hands `bytes` to the lanes it is for: those whose peer sends from `from`, or every lane
/// when no peer is known there. A peer whose address changed is still its link's to tell by
/// its keys, as it is when a connection has the socket to itself. Only a limited number of
/// datagrams from addresses no peer is known at get that far (`unknown`): every lane opens
/// them, so they are what a sender who is nobody's peer could make the room's links work on.
fn deliver(
    lanes: &[Arc<LaneShared>],
    bytes: Vec<u8>,
    from: SocketAddr,
    unknown: &mut UnknownSources,
) {
    let taking = |lane: &Arc<LaneShared>, known_only: bool| {
        let state = lane.state.lock();
        match &state.peer_addrs {
            Some(addrs) => !known_only || addrs.contains(&from),
            None => false,
        }
    };
    let mut targets: Vec<&Arc<LaneShared>> =
        lanes.iter().filter(|lane| taking(lane, true)).collect();
    if targets.is_empty() {
        targets = lanes.iter().filter(|lane| taking(lane, false)).collect();
        if !targets.is_empty() && !unknown.allow(Instant::now()) {
            trace!(
                "Dropping a datagram from {}: too many from unknown addresses",
                from
            );
            return;
        }
    }

    let Some((last, others)) = targets.split_last() else {
        trace!("No link takes the datagram from {}", from);
        return;
    };
    for lane in others {
        put(lane, bytes.clone(), from);
    }
    put(last, bytes, from);
}

fn put(lane: &LaneShared, bytes: Vec<u8>, from: SocketAddr) {
    {
        let mut state = lane.state.lock();
        if state.peer_addrs.is_none() {
            return;
        }
        if state.waiting.len() >= LANE_CAPACITY {
            debug!("Dropping a datagram from {}: its link is not reading", from);
            return;
        }
        state.waiting.push_back((bytes, from));
    }
    lane.arrived.notify_one();
}

/// A UDP socket that several connections send and receive through, each to its own peer
///
/// Make one connection per peer with [`super::Connection::on`]. Must be created, and its
/// connections used, on the Tokio runtime that will drive it (ADR-026).
pub struct UdpMux {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    lanes: Arc<Mutex<Vec<Arc<LaneShared>>>>,
    reader: tokio::task::JoinHandle<()>,
}

impl UdpMux {
    /// Binds a socket to share
    pub async fn bind(addr: &str) -> Result<Self, NetworkError> {
        Self::from_std(bind_std(addr)?)
    }

    /// Shares an already-bound socket, such as the one whose port the room was told
    pub fn from_std(std_socket: std::net::UdpSocket) -> Result<Self, NetworkError> {
        let socket = Arc::new(UdpSocket::from_std(std_socket)?);
        let local_addr = socket.local_addr()?;
        let lanes: Arc<Mutex<Vec<Arc<LaneShared>>>> = Arc::new(Mutex::new(Vec::new()));

        let reading = socket.clone();
        let routing = lanes.clone();
        let reader = tokio::spawn(async move {
            let mut buf = vec![0u8; RECV_BUFFER_SIZE];
            let mut unknown = UnknownSources::new(Instant::now());
            loop {
                match reading.recv_from(&mut buf).await {
                    // A datagram that fills the buffer may have been cut short
                    Ok((len, from)) if len == RECV_BUFFER_SIZE => {
                        debug!("Dropping a datagram from {from} that fills the receive buffer");
                    }
                    Ok((len, from)) => {
                        let lanes = routing.lock().clone();
                        deliver(
                            &lanes,
                            buf[..len].to_vec(),
                            canonical_addr(from),
                            &mut unknown,
                        );
                    }
                    Err(e) => error!("Receive error: {}", e),
                }
            }
        });

        Ok(Self {
            socket,
            local_addr,
            lanes,
            reader,
        })
    }

    /// The address every peer sends to
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// A transport to one peer: it sends through the shared socket, and receives what that
    /// peer sends once the connection that owns it says where the peer is
    pub(super) fn transport(&self) -> UdpTransport {
        let shared = Arc::new(LaneShared {
            state: Mutex::new(LaneState {
                waiting: VecDeque::new(),
                peer_addrs: None,
                offered: 0,
            }),
            arrived: Notify::new(),
        });
        self.lanes.lock().push(shared.clone());
        UdpTransport::sharing(
            self.socket.clone(),
            self.local_addr,
            Lane {
                shared,
                lanes: self.lanes.clone(),
            },
        )
    }
}

impl Drop for UdpMux {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn lane(peer_addrs: Option<&[&str]>) -> Arc<LaneShared> {
        Arc::new(LaneShared {
            state: Mutex::new(LaneState {
                waiting: VecDeque::new(),
                peer_addrs: peer_addrs.map(|addrs| addrs.iter().map(|a| addr(a)).collect()),
                offered: peer_addrs.map_or(0, |addrs| addrs.len()),
            }),
            arrived: Notify::new(),
        })
    }

    fn waiting(lane: &LaneShared) -> Vec<Vec<u8>> {
        lane.state
            .lock()
            .waiting
            .iter()
            .map(|(bytes, _)| bytes.clone())
            .collect()
    }

    /// Verifies: REQ-CON-033
    #[test]
    fn a_datagram_from_a_peers_address_goes_to_that_peers_lane_only() {
        let lanes = vec![
            lane(Some(&["192.0.2.2:5000"])),
            lane(Some(&["192.0.2.3:5000"])),
        ];

        deliver(
            &lanes,
            vec![7],
            addr("192.0.2.3:5000"),
            &mut UnknownSources::new(Instant::now()),
        );

        assert!(waiting(&lanes[0]).is_empty());
        assert_eq!(waiting(&lanes[1]), vec![vec![7]]);
    }

    /// Verifies: REQ-CON-033
    #[test]
    fn a_datagram_from_any_of_a_peers_addresses_goes_to_its_lane() {
        let lanes = vec![lane(Some(&["192.168.1.20:5000", "203.0.113.7:6000"]))];

        deliver(
            &lanes,
            vec![1],
            addr("192.168.1.20:5000"),
            &mut UnknownSources::new(Instant::now()),
        );
        deliver(
            &lanes,
            vec![2],
            addr("203.0.113.7:6000"),
            &mut UnknownSources::new(Instant::now()),
        );

        assert_eq!(waiting(&lanes[0]), vec![vec![1], vec![2]]);
    }

    /// Two participants at home behind different routers can publish the same private
    /// address. Which of them a packet from it is, only the keys of the links can tell.
    ///
    /// Verifies: REQ-CON-033
    #[test]
    fn a_datagram_from_an_address_two_peers_both_published_goes_to_both_lanes() {
        let lanes = vec![
            lane(Some(&["192.168.1.20:5000"])),
            lane(Some(&["192.168.1.20:5000"])),
        ];

        deliver(
            &lanes,
            vec![7],
            addr("192.168.1.20:5000"),
            &mut UnknownSources::new(Instant::now()),
        );

        assert_eq!(waiting(&lanes[0]), vec![vec![7]]);
        assert_eq!(waiting(&lanes[1]), vec![vec![7]]);
    }

    /// Verifies: REQ-CON-033
    #[test]
    fn a_datagram_from_an_address_no_peer_published_is_offered_to_every_lane() {
        let lanes = vec![
            lane(Some(&["192.0.2.2:5000"])),
            lane(Some(&["192.0.2.3:5000"])),
        ];

        deliver(
            &lanes,
            vec![7],
            addr("198.51.100.9:4000"),
            &mut UnknownSources::new(Instant::now()),
        );

        assert_eq!(waiting(&lanes[0]), vec![vec![7]]);
        assert_eq!(waiting(&lanes[1]), vec![vec![7]]);
    }

    /// Verifies: REQ-CON-033
    #[test]
    fn a_lane_whose_connection_has_not_started_or_has_ended_takes_nothing() {
        let lanes = vec![lane(None), lane(Some(&["192.0.2.3:5000"]))];

        deliver(
            &lanes,
            vec![7],
            addr("198.51.100.9:4000"),
            &mut UnknownSources::new(Instant::now()),
        );

        assert!(waiting(&lanes[0]).is_empty());
        assert_eq!(waiting(&lanes[1]), vec![vec![7]]);
    }

    #[test]
    fn a_lane_that_is_not_read_drops_what_does_not_fit_and_keeps_what_it_held() {
        let lanes = vec![lane(Some(&["192.0.2.2:5000"]))];

        for i in 0..LANE_CAPACITY + 5 {
            deliver(
                &lanes,
                vec![(i % 251) as u8],
                addr("192.0.2.2:5000"),
                &mut UnknownSources::new(Instant::now()),
            );
        }

        let held = waiting(&lanes[0]);
        assert_eq!(held.len(), LANE_CAPACITY);
        assert_eq!(held[0], vec![0]);
    }

    /// Verifies: REQ-CON-033
    #[tokio::test]
    async fn a_lane_with_a_backlog_lets_the_other_links_on_its_thread_run() {
        let backlog = 1000;
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();
        let transport = mux.transport();
        transport.expect_peer_at(&[addr("192.0.2.2:5000")]);
        for _ in 0..backlog {
            deliver(
                &mux.lanes.lock(),
                vec![7],
                addr("192.0.2.2:5000"),
                &mut UnknownSources::new(Instant::now()),
            );
        }

        let taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counting = taken.clone();
        let reading = tokio::spawn(async move {
            for _ in 0..backlog {
                transport.recv_raw().await.unwrap();
                counting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let seeing = taken.clone();
        let other = tokio::spawn(async move { seeing.load(std::sync::atomic::Ordering::SeqCst) });

        let taken_when_the_other_ran = other.await.unwrap();
        reading.await.unwrap();
        assert!(
            taken_when_the_other_ran < backlog,
            "the whole backlog was read before any other task ran"
        );
    }

    #[tokio::test]
    async fn a_released_lane_ends_a_read_that_was_waiting_on_it() {
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();
        let transport = Arc::new(mux.transport());
        transport.expect_peer_at(&[addr("127.0.0.1:9")]);

        let reading = transport.clone();
        let read = tokio::spawn(async move { reading.recv_raw().await });
        tokio::task::yield_now().await;
        transport.release();

        assert!(matches!(
            read.await.unwrap(),
            Err(NetworkError::NotConnected)
        ));
    }

    #[tokio::test]
    async fn dropping_a_transport_takes_its_lane_off_the_socket() {
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();

        let transport = mux.transport();
        assert_eq!(mux.lanes.lock().len(), 1);
        drop(transport);

        assert!(mux.lanes.lock().is_empty());
    }

    /// Verifies: REQ-CON-165
    #[test]
    fn when_datagrams_from_unknown_addresses_keep_coming_then_only_a_few_are_handed_on() {
        let lanes = vec![lane(Some(&["192.0.2.2:5000"]))];
        let mut unknown = UnknownSources::new(Instant::now());

        for i in 0..500u16 {
            deliver(
                &lanes,
                vec![1],
                addr(&format!("203.0.113.9:{}", 10000 + i)),
                &mut unknown,
            );
        }

        let handed_on = waiting(&lanes[0]).len();
        assert!((64..=66).contains(&handed_on), "{handed_on}");
    }

    /// Verifies: REQ-CON-165
    #[test]
    fn when_unknown_addresses_have_used_up_the_allowance_then_a_peers_datagrams_still_come() {
        let lanes = vec![lane(Some(&["192.0.2.2:5000"]))];
        let mut unknown = UnknownSources::new(Instant::now());
        for i in 0..500u16 {
            deliver(
                &lanes,
                vec![1],
                addr(&format!("203.0.113.9:{}", 10000 + i)),
                &mut unknown,
            );
        }
        lanes[0].state.lock().waiting.clear();

        deliver(&lanes, vec![9], addr("192.0.2.2:5000"), &mut unknown);

        assert_eq!(waiting(&lanes[0]), vec![vec![9]]);
    }

    #[test]
    fn when_time_passes_then_unknown_addresses_are_allowed_again() {
        let start = Instant::now();
        let mut unknown = UnknownSources::new(start);
        let spent = (0..70).filter(|_| unknown.allow(start)).count();
        assert_eq!(spent, 64);

        assert!(!unknown.allow(start));
        assert!(unknown.allow(start + std::time::Duration::from_secs(1)));
    }

    /// Verifies: REQ-CON-165
    #[test]
    fn when_a_packet_got_through_from_a_new_address_then_its_datagrams_are_no_longer_limited() {
        let shared = lane(Some(&["192.0.2.2:5000"]));
        let owner = Lane {
            shared: shared.clone(),
            lanes: Arc::new(Mutex::new(Vec::new())),
        };
        let lanes = vec![shared.clone()];
        let mut unknown = UnknownSources::new(Instant::now());
        for i in 0..500u16 {
            deliver(
                &lanes,
                vec![1],
                addr(&format!("203.0.113.9:{}", 10000 + i)),
                &mut unknown,
            );
        }
        shared.state.lock().waiting.clear();

        owner.learn(addr("198.51.100.4:7000"));
        deliver(&lanes, vec![5], addr("198.51.100.4:7000"), &mut unknown);

        assert_eq!(waiting(&shared), vec![vec![5]]);
    }

    #[test]
    fn when_more_addresses_are_learned_than_fit_then_the_oldest_goes_and_the_offered_one_stays() {
        let shared = lane(Some(&["192.0.2.2:5000"]));
        let owner = Lane {
            shared: shared.clone(),
            lanes: Arc::new(Mutex::new(Vec::new())),
        };

        for port in 7000..7000 + LEARNED_ADDRS as u16 + 2 {
            owner.learn(addr(&format!("198.51.100.4:{port}")));
        }

        let addrs = shared.state.lock().peer_addrs.clone().unwrap();
        assert_eq!(addrs.len(), 1 + LEARNED_ADDRS);
        assert_eq!(addrs[0], addr("192.0.2.2:5000"));
        assert!(!addrs.contains(&addr("198.51.100.4:7000")));
        assert!(addrs.contains(&addr("198.51.100.4:7005")));
    }
}
