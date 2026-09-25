//! How the link to a peer came up: which kind of address carries it, how long
//! it took, and when the first audio arrived.
//!
//! The connection writes these once while it connects and receives; anyone
//! holding the handle from [`super::Connection::link_facts`] can read them at
//! any time. They exist for the usage log, which reports the kind of route and
//! two durations, never the peer's address.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

/// Stored in a millisecond slot that has not been measured.
const NOT_MEASURED: u64 = u64::MAX;

/// What kind of address the audio goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkRoute {
    /// A private, link-local or unique-local address: the peer is reached
    /// inside a local network.
    Lan,
    /// Any other address: the peer is reached through its public address.
    Public,
    /// This machine itself.
    Loopback,
}

impl LinkRoute {
    /// The kind of route that sending to `addr` is.
    pub fn of(addr: SocketAddr) -> Self {
        match addr.ip() {
            IpAddr::V4(ip) if ip.is_loopback() => Self::Loopback,
            IpAddr::V4(ip) if ip.is_private() || ip.is_link_local() => Self::Lan,
            IpAddr::V6(ip) if ip.is_loopback() => Self::Loopback,
            // fe80::/10 (link-local) and fc00::/7 (unique local)
            IpAddr::V6(ip) if (ip.segments()[0] & 0xffc0) == 0xfe80 => Self::Lan,
            IpAddr::V6(ip) if (ip.segments()[0] & 0xfe00) == 0xfc00 => Self::Lan,
            _ => Self::Public,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Self::Lan => 1,
            Self::Public => 2,
            Self::Loopback => 3,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Lan),
            2 => Some(Self::Public),
            3 => Some(Self::Loopback),
            _ => None,
        }
    }
}

/// What has been learned about the link so far. A value that has not been
/// measured yet is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkSnapshot {
    /// The kind of address the link went up on.
    pub route: Option<LinkRoute>,
    /// Whether that address was chosen because it answered a probe. `false`
    /// when the connection went up on an address that never answered (the
    /// only candidate, or the first one after no candidate answered in time).
    pub route_confirmed: Option<bool>,
    /// Milliseconds from starting to connect to the link going up.
    pub connect_ms: Option<u64>,
    /// Milliseconds from the link going up to the first audio packet.
    pub first_audio_ms: Option<u64>,
}

impl LinkSnapshot {
    /// Whether nothing has been measured, as before any connection.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The shared cells behind [`LinkSnapshot`].
#[derive(Debug)]
pub struct LinkFacts {
    route: AtomicU8,
    route_confirmed: AtomicBool,
    connect_ms: AtomicU64,
    first_audio_ms: AtomicU64,
    up_at: Mutex<Option<Instant>>,
}

impl LinkFacts {
    pub(crate) fn new() -> Self {
        Self {
            route: AtomicU8::new(0),
            route_confirmed: AtomicBool::new(false),
            connect_ms: AtomicU64::new(NOT_MEASURED),
            first_audio_ms: AtomicU64::new(NOT_MEASURED),
            up_at: Mutex::new(None),
        }
    }

    /// The link went up on `addr`, `started` being when connecting began.
    pub(crate) fn link_up(&self, addr: SocketAddr, confirmed: bool, started: Instant) {
        let now = Instant::now();
        self.route
            .store(LinkRoute::of(addr).to_u8(), Ordering::Relaxed);
        self.route_confirmed.store(confirmed, Ordering::Relaxed);
        self.connect_ms.store(
            u64::try_from(now.duration_since(started).as_millis()).unwrap_or(u64::MAX - 1),
            Ordering::Relaxed,
        );
        self.first_audio_ms.store(NOT_MEASURED, Ordering::Relaxed);
        *self.up_at.lock().unwrap() = Some(now);
    }

    /// An audio packet arrived. Only the first one since the link went up counts.
    pub(crate) fn audio_received(&self) {
        if self.first_audio_ms.load(Ordering::Relaxed) != NOT_MEASURED {
            return;
        }
        let Some(up_at) = *self.up_at.lock().unwrap() else {
            return;
        };
        let ms = u64::try_from(up_at.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        let _ = self.first_audio_ms.compare_exchange(
            NOT_MEASURED,
            ms,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    /// Reads what is known now.
    pub fn snapshot(&self) -> LinkSnapshot {
        let route = LinkRoute::from_u8(self.route.load(Ordering::Relaxed));
        let measured = |cell: &AtomicU64| {
            let ms = cell.load(Ordering::Relaxed);
            (ms != NOT_MEASURED).then_some(ms)
        };
        LinkSnapshot {
            route,
            route_confirmed: route.map(|_| self.route_confirmed.load(Ordering::Relaxed)),
            connect_ms: measured(&self.connect_ms),
            first_audio_ms: measured(&self.first_audio_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn when_the_address_is_private_the_route_is_lan() {
        for text in [
            "192.168.1.20:5000",
            "10.0.0.5:5000",
            "172.16.4.9:5000",
            "169.254.3.4:5000",
            "[fe80::1]:5000",
            "[fd00::1]:5000",
        ] {
            assert_eq!(LinkRoute::of(addr(text)), LinkRoute::Lan, "{text}");
        }
    }

    #[test]
    fn when_the_address_is_public_the_route_is_public() {
        for text in ["203.0.113.7:5000", "8.8.8.8:5000", "[2001:db8::1]:5000"] {
            assert_eq!(LinkRoute::of(addr(text)), LinkRoute::Public, "{text}");
        }
    }

    #[test]
    fn when_the_address_is_this_machine_the_route_is_loopback() {
        assert_eq!(LinkRoute::of(addr("127.0.0.1:5000")), LinkRoute::Loopback);
        assert_eq!(LinkRoute::of(addr("[::1]:5000")), LinkRoute::Loopback);
    }

    #[test]
    fn when_nothing_has_connected_the_snapshot_is_empty() {
        assert!(LinkFacts::new().snapshot().is_empty());
    }

    #[test]
    fn when_the_link_goes_up_the_route_and_connect_time_are_known_but_not_the_first_audio() {
        let facts = LinkFacts::new();
        facts.link_up(addr("192.168.1.20:5000"), true, Instant::now());

        let snapshot = facts.snapshot();
        assert_eq!(snapshot.route, Some(LinkRoute::Lan));
        assert_eq!(snapshot.route_confirmed, Some(true));
        assert!(snapshot.connect_ms.is_some());
        assert_eq!(snapshot.first_audio_ms, None);
    }

    #[test]
    fn when_audio_arrives_before_the_link_is_up_no_first_audio_time_is_kept() {
        let facts = LinkFacts::new();
        facts.audio_received();
        assert_eq!(facts.snapshot().first_audio_ms, None);
    }

    #[test]
    fn when_several_audio_packets_arrive_only_the_first_sets_the_time() {
        let facts = LinkFacts::new();
        facts.link_up(addr("203.0.113.7:5000"), false, Instant::now());
        facts.audio_received();
        let first = facts.snapshot().first_audio_ms;
        assert!(first.is_some());

        std::thread::sleep(std::time::Duration::from_millis(15));
        facts.audio_received();

        assert_eq!(facts.snapshot().first_audio_ms, first);
    }

    #[test]
    fn when_the_link_goes_up_again_the_first_audio_time_starts_over() {
        let facts = LinkFacts::new();
        facts.link_up(addr("203.0.113.7:5000"), false, Instant::now());
        facts.audio_received();

        facts.link_up(addr("192.168.1.20:5000"), true, Instant::now());

        let snapshot = facts.snapshot();
        assert_eq!(snapshot.route, Some(LinkRoute::Lan));
        assert_eq!(snapshot.first_audio_ms, None);
    }
}
