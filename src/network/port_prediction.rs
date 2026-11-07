//! Guessing the port a peer behind a NAT that changes its port per destination is reached at
//!
//! Some NATs give the audio socket a different public port for each address it sends to. The
//! port STUN saw is the port the STUN server was told, not the one a peer will be told. Such a
//! peer shows itself by offering several public ports on one public IPv4 address: it asked
//! STUN more than one server (ADR-115). What those ports are like says how to guess the next:
//!
//! - ports a little apart (a NAT that hands out the next free port): the port toward the peer
//!   is the next one, or a few after it, so a few ports around the next one are tried. Ports
//!   that are no further apart than this are also ports a NAT that picks at random inside a
//!   small block might give, so when they share their middle bits the set is tried after;
//! - ports that share their middle bits but differ above and below (a NAT whose ports are
//!   restricted to a set, as with MAP-E: each user is given every 16 ports out of each of 63
//!   blocks of 1024): the port is one of that set, so the set is tried in turn;
//! - anything else (a NAT that picks any port at random): nothing is guessed.
//!
//! This module only says where to send and how fast. The sending is the connection's.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Two public ports of one peer further apart than this are not "the next free port"
const SEQUENTIAL_STEP_MAX: u16 = 16;

/// How many ports below and above the expected one a pass tries
const WINDOW_RADIUS: u16 = 3;

/// How far the window moves for each step of the guess, in ports: the width of the window
const WINDOW_SHIFT: u16 = 2 * WINDOW_RADIUS + 1;

/// How many windows are tried, the second and third beyond the first
const WINDOWS: u16 = 3;

/// How many times each window is sent: a second pass covers a first one that arrived before the
/// peer's NAT had made the mapping
const PASSES_PER_WINDOW: usize = 2;

/// How long one packet of a window waits for the one before it
const SEQUENTIAL_SPACING: Duration = Duration::from_millis(10);

/// How long a pass waits for the one before it
const PASS_PAUSE: Duration = Duration::from_millis(100);

/// The blocks of ports a NAT with a restricted port set takes from, and how many ports one user
/// has in each block. The first block, below port 1024, is not given to users.
const PORT_SET_BLOCKS: u16 = 63;
const PORT_SET_PER_BLOCK: u16 = 16;

/// How many ports a sweep of a port set sends in a second
const SWEEP_PER_SECOND: u64 = 250;

/// How long the first retry waits after a try that was not answered, and how long no retry waits
/// more than: the wait doubles
const RETRY_FIRST: Duration = Duration::from_secs(20);
const RETRY_LAST: Duration = Duration::from_secs(300);

/// How many times a guess is retried
const MAX_RETRIES: usize = 5;

/// The first port the guess may name: below it are the ports a NAT keeps for the system's own
const FIRST_PORT: u32 = 1024;

/// Addresses to send to one after another, each `spacing` after the one before
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pass {
    pub(super) addrs: Vec<SocketAddr>,
    pub(super) spacing: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pattern {
    /// The next free port after `expected` is guessed, and a few around it; and, when the ports
    /// offered also fit a set with `set` for its middle bits, the set after those
    Sequential { expected: u16, set: Option<u16> },
    /// Any port of the set that has `middle` between the block number and the offset
    PortSet { middle: u16 },
}

/// Where a peer that is behind a NAT that changes its port per destination may be reached
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PortPrediction {
    ip: Ipv4Addr,
    pattern: Pattern,
    /// The ports the peer offered: they are sent to anyway, so not again
    offered: Vec<u16>,
    /// Told to the peer in every probe, so that an answer can be told from a forged one
    nonce: u16,
}

/// Whether `ip` is an address a peer is reached at over the internet, so that the same address
/// offered with several ports is a NAT's doing and not several interfaces' (a host candidate
/// has the one port of the socket)
fn is_public_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    !ip.is_private()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        // Shared address space of a carrier's NAT, and of Tailscale
        && !(octets[0] == 100 && (64..128).contains(&octets[1]))
}

impl PortPrediction {
    /// What the addresses a peer offered say about its NAT: `None` when it offers no public IPv4
    /// address twice with different ports, or when its ports are not of a kind that can be guessed
    pub(super) fn from_offered(offered: &[SocketAddr], nonce: u16) -> Option<Self> {
        let ports_of = |ip: Ipv4Addr| -> Vec<u16> {
            let mut ports: Vec<u16> = offered
                .iter()
                .filter_map(|addr| match addr {
                    SocketAddr::V4(v4) if *v4.ip() == ip => Some(v4.port()),
                    _ => None,
                })
                .collect();
            ports.sort_unstable();
            ports.dedup();
            ports
        };
        let (ip, ports) = offered.iter().find_map(|addr| match addr {
            SocketAddr::V4(v4) if is_public_v4(*v4.ip()) => {
                Some((*v4.ip(), ports_of(*v4.ip()))).filter(|(_, ports)| ports.len() > 1)
            }
            _ => None,
        })?;

        let steps: Vec<u16> = ports.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let pattern = if steps.iter().all(|&step| step <= SEQUENTIAL_STEP_MAX) {
            let newest = *ports.last()?;
            let step = *steps.last()?;
            Pattern::Sequential {
                expected: newest.saturating_add(step),
                set: shared_middle(&ports),
            }
        } else if let Some(middle) = shared_middle(&ports) {
            Pattern::PortSet { middle }
        } else {
            return None;
        };
        Some(Self {
            ip,
            pattern,
            offered: ports,
            nonce,
        })
    }

    /// The token a probe to `port` carries, and its answer carries back. The top bit keeps it
    /// apart from the tokens of the connectivity check, which count rounds from zero.
    pub(super) fn token(&self, port: u16) -> u32 {
        (u32::from(0x8000 | (self.nonce & 0x7FFF)) << 16) | u32::from(port)
    }

    /// Whether `token` is what a probe to `addr` carries
    pub(super) fn is_token_for(&self, token: u32, addr: SocketAddr) -> bool {
        addr.is_ipv4() && token == self.token(addr.port())
    }

    /// Whether `addr` is an address that is guessed at: the peer's, and one of the ports
    pub(super) fn covers(&self, addr: SocketAddr) -> bool {
        let SocketAddr::V4(v4) = addr else {
            return false;
        };
        *v4.ip() == self.ip
            && !self.offered.contains(&v4.port())
            && match self.pattern {
                Pattern::Sequential { expected, set } => {
                    let first = i32::from(expected) - i32::from(WINDOW_RADIUS);
                    let last = i32::from(expected)
                        + i32::from(WINDOW_RADIUS)
                        + i32::from(WINDOW_SHIFT) * i32::from(WINDOWS - 1);
                    (first..=last).contains(&i32::from(v4.port()))
                        || set.is_some_and(|middle| in_port_set(v4.port(), middle))
                }
                Pattern::PortSet { middle } => in_port_set(v4.port(), middle),
            }
    }

    /// The addresses to send to, one pass after another. A window of ports is sent twice before
    /// the next window, which is further from the expected port.
    pub(super) fn passes(&self) -> Vec<Pass> {
        let at = |port: u32| {
            (port >= FIRST_PORT && port <= u32::from(u16::MAX))
                .then(|| u16::try_from(port).ok())
                .flatten()
                .filter(|port| !self.offered.contains(port))
                .map(|port| SocketAddr::new(IpAddr::V4(self.ip), port))
        };
        let sweep = |middle: u16| Pass {
            addrs: (1..=PORT_SET_BLOCKS)
                .flat_map(|block| {
                    (0..PORT_SET_PER_BLOCK).map(move |offset| (block << 10) | (middle << 4) | offset)
                })
                .filter_map(|port| at(u32::from(port)))
                .collect(),
            spacing: Duration::from_millis(1000 / SWEEP_PER_SECOND),
        };
        match self.pattern {
            Pattern::Sequential { expected, set } => (0..WINDOWS)
                .flat_map(|window| {
                    let from = u32::from(expected) + u32::from(WINDOW_SHIFT) * u32::from(window);
                    let addrs: Vec<SocketAddr> = (0..WINDOW_SHIFT)
                        .filter_map(|i| {
                            at((from + u32::from(i)).checked_sub(u32::from(WINDOW_RADIUS))?)
                        })
                        .collect();
                    let pass = Pass {
                        addrs,
                        spacing: SEQUENTIAL_SPACING,
                    };
                    std::iter::repeat_n(pass, PASSES_PER_WINDOW)
                })
                .filter(|pass| !pass.addrs.is_empty())
                .chain(set.map(sweep))
                .collect(),
            Pattern::PortSet { middle } => vec![sweep(middle)],
        }
    }

    /// How long a pass waits for the one before it
    pub(super) fn pause(&self) -> Duration {
        PASS_PAUSE
    }

    /// How long to wait before the `retry`th try again (the first is `0`) after a try that was
    /// not answered, or `None` once the tries are used up
    pub(super) fn retry_after(&self, retry: usize) -> Option<Duration> {
        (retry < MAX_RETRIES).then(|| {
            RETRY_FIRST
                .saturating_mul(1u32 << retry.min(16))
                .min(RETRY_LAST)
        })
    }

    /// The most packets a try sends, and with every retry
    pub(super) fn packets_per_try(&self) -> usize {
        self.passes().iter().map(|pass| pass.addrs.len()).sum()
    }
}

/// The bits between the block number and the offset in a port set, when every port has the
/// same ones and is in a block a user can have
fn shared_middle(ports: &[u16]) -> Option<u16> {
    // One more port than a few is what tells a set from chance: two ports share their middle
    // bits once in 64
    if ports.len() < 3 {
        return None;
    }
    let middle = (ports[0] >> 4) & 0x3F;
    ports
        .iter()
        .all(|&port| in_port_set(port, middle))
        .then_some(middle)
}

/// Whether `port` is in the set whose middle bits are `middle`
fn in_port_set(port: u16, middle: u16) -> bool {
    (port >> 10) >= 1 && ((port >> 4) & 0x3F) == middle
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn predicted(offered: &[&str]) -> Option<PortPrediction> {
        let offered: Vec<SocketAddr> = offered.iter().map(|a| addr(a)).collect();
        PortPrediction::from_offered(&offered, 7)
    }

    fn ports(pass: &Pass) -> Vec<u16> {
        pass.addrs.iter().map(SocketAddr::port).collect()
    }

    fn sent(prediction: &PortPrediction) -> Vec<u16> {
        prediction.passes().iter().flat_map(ports).collect()
    }

    #[test]
    fn when_a_peer_offers_one_public_port_nothing_is_guessed() {
        assert_eq!(predicted(&["203.0.113.7:40000", "192.168.1.5:5000"]), None);
    }

    #[test]
    fn when_a_peer_offers_ports_on_private_addresses_only_nothing_is_guessed() {
        assert_eq!(predicted(&["192.168.1.5:5000", "192.168.1.5:5001"]), None);
        assert_eq!(predicted(&["100.101.102.103:5000", "100.101.102.103:5001"]), None);
    }

    #[test]
    fn when_a_peer_offers_ports_on_two_public_addresses_nothing_is_guessed() {
        assert_eq!(predicted(&["203.0.113.7:40000", "198.51.100.9:40001"]), None);
    }

    #[test]
    fn when_a_peer_offers_ports_that_are_far_apart_and_in_no_set_nothing_is_guessed() {
        assert_eq!(
            predicted(&["203.0.113.7:40000", "203.0.113.7:12345", "203.0.113.7:55555"]),
            None
        );
        // Two ports share their middle bits once in 64, which is not enough to take for a set
        assert_eq!(predicted(&["203.0.113.7:2064", "203.0.113.7:4112"]), None);
    }

    #[test]
    fn when_a_peer_offers_the_next_port_the_ports_around_the_one_after_are_tried_twice_each() {
        // Two ports share their middle bits too rarely to be a set: only the windows are tried
        let prediction = predicted(&["203.0.113.7:40000", "203.0.113.7:40001"]).unwrap();
        let passes = prediction.passes();
        assert_eq!(passes.len(), 6);
        // The expected port is 40002; the offered ones are not sent again
        assert_eq!(ports(&passes[0]), vec![39999, 40002, 40003, 40004, 40005]);
        assert_eq!(passes[0], passes[1]);
        assert_eq!(ports(&passes[2]), (40006..=40012).collect::<Vec<_>>());
        assert_eq!(passes[2], passes[3]);
        assert_eq!(ports(&passes[4]), (40013..=40019).collect::<Vec<_>>());
        assert_eq!(passes[4], passes[5]);
        assert!(passes
            .iter()
            .flat_map(|pass| &pass.addrs)
            .all(|a| a.ip() == addr("203.0.113.7:1").ip()));
    }

    #[test]
    fn when_the_peer_offers_ports_a_few_apart_the_step_is_carried_on() {
        let prediction = predicted(&["203.0.113.7:40000", "203.0.113.7:40004"]).unwrap();
        // The next is 40008, three either side of it
        let first = ports(&prediction.passes()[0]);
        assert!(first.contains(&40005));
        assert!(first.contains(&40011));
        assert!(!first.contains(&40012));
    }

    #[test]
    fn when_the_ports_are_near_the_top_of_the_range_none_beyond_it_is_sent() {
        let prediction = predicted(&["203.0.113.7:65530", "203.0.113.7:65531"]).unwrap();
        let sent = sent(&prediction);
        assert!(!sent.is_empty());
        assert!(sent.iter().all(|&p| p >= 65529));
    }

    #[test]
    fn when_the_ports_are_near_the_bottom_none_below_1024_is_sent() {
        let prediction = predicted(&["203.0.113.7:1025", "203.0.113.7:1026"]).unwrap();
        assert!(sent(&prediction).iter().all(|&p| p >= 1024));
    }

    #[test]
    fn when_a_peer_offers_three_ports_of_one_port_set_the_whole_set_is_swept_in_one_pass() {
        // Middle bits 0b000101: ports 1024 * block + 16 * 5 + offset
        let prediction = predicted(&[
            "203.0.113.7:1104",
            "203.0.113.7:5200",
            "203.0.113.7:33875",
        ])
        .unwrap();
        let passes = prediction.passes();
        assert_eq!(passes.len(), 1);
        // 63 blocks of 16, less the three the peer offered
        assert_eq!(passes[0].addrs.len(), 63 * 16 - 3);
        assert!(passes[0].addrs.iter().all(|a| in_port_set(a.port(), 5)));
        assert_eq!(passes[0].spacing, Duration::from_millis(4));
    }

    /// A NAT that picks any port inside a block of 16 offers ports that are only a few apart,
    /// as one that counts up does.
    #[test]
    fn when_three_ports_are_a_few_apart_and_in_one_set_the_windows_are_tried_and_then_the_set() {
        let prediction = predicted(&[
            "203.0.113.7:20083",
            "203.0.113.7:20092",
            "203.0.113.7:20093",
        ])
        .unwrap();
        let passes = prediction.passes();
        assert_eq!(passes.len(), 7);
        assert_eq!(passes[0].spacing, SEQUENTIAL_SPACING);
        let sweep = passes.last().unwrap();
        assert_eq!(sweep.spacing, Duration::from_millis(4));
        // Every port of the block the ports are in is in the sweep, but for those offered
        assert!(sweep.addrs.contains(&addr("203.0.113.7:20080")));
        assert!(sweep.addrs.contains(&addr("203.0.113.7:20095")));
        assert!(!sweep.addrs.contains(&addr("203.0.113.7:20092")));
        assert!(prediction.covers(addr("203.0.113.7:20080")));
        assert!(prediction.covers(addr("203.0.113.7:41584")));
    }

    #[test]
    fn when_a_pass_is_sent_at_the_pace_the_packets_of_one_try_take_a_few_seconds() {
        let time = |prediction: &PortPrediction| {
            let passes = prediction.passes();
            passes
                .iter()
                .map(|pass| pass.spacing * pass.addrs.len() as u32 + prediction.pause())
                .sum::<Duration>()
        };
        let sweep = predicted(&["203.0.113.7:1104", "203.0.113.7:5200", "203.0.113.7:33875"])
            .unwrap();
        assert!(time(&sweep) <= Duration::from_secs(5), "{:?}", time(&sweep));
        let window = predicted(&["203.0.113.7:40000", "203.0.113.7:40001"]).unwrap();
        assert!(time(&window) <= Duration::from_secs(2), "{:?}", time(&window));
        let both = predicted(&["203.0.113.7:20083", "203.0.113.7:20092", "203.0.113.7:20093"])
            .unwrap();
        assert!(time(&both) <= Duration::from_secs(7), "{:?}", time(&both));
    }

    #[test]
    fn when_a_try_was_not_answered_the_wait_to_the_next_doubles_up_to_five_minutes() {
        let prediction = predicted(&["203.0.113.7:40000", "203.0.113.7:40001"]).unwrap();
        let waits: Vec<u64> = (0..6)
            .map(|retry| prediction.retry_after(retry).map_or(0, |d| d.as_secs()))
            .collect();
        assert_eq!(waits, vec![20, 40, 80, 160, 300, 0]);
    }

    #[test]
    fn an_address_is_covered_when_it_is_the_peers_and_one_the_guess_may_name() {
        let prediction = predicted(&["203.0.113.7:40000", "203.0.113.7:40001"]).unwrap();
        assert!(prediction.covers(addr("203.0.113.7:40003")));
        assert!(prediction.covers(addr("203.0.113.7:40019")));
        assert!(!prediction.covers(addr("203.0.113.7:40020")));
        assert!(!prediction.covers(addr("203.0.113.7:39998")));
        // An offered address is the connectivity check's, and another host is not the peer
        assert!(!prediction.covers(addr("203.0.113.7:40001")));
        assert!(!prediction.covers(addr("203.0.113.8:40003")));
        assert!(!prediction.covers(addr("[2001:db8::1]:40003")));
    }

    #[test]
    fn a_probe_answer_is_the_peers_only_when_it_carries_the_token_of_the_port_it_came_from() {
        let prediction = predicted(&["203.0.113.7:40000", "203.0.113.7:40001"]).unwrap();
        let from = addr("203.0.113.7:40003");
        let token = prediction.token(40003);
        assert!(prediction.is_token_for(token, from));
        assert!(!prediction.is_token_for(token, addr("203.0.113.7:40004")));
        assert!(!prediction.is_token_for(prediction.token(40004), from));
        // The top bit keeps it apart from the check's tokens, which count rounds from zero
        assert!(token >> 16 >= 0x8000);
        let other = PortPrediction::from_offered(
            &[addr("203.0.113.7:40000"), addr("203.0.113.7:40001")],
            8,
        )
        .unwrap();
        assert!(!other.is_token_for(token, from));
    }
}
