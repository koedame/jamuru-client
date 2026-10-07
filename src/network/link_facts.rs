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
use std::time::{Duration, Instant};

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

/// [`route_preference`] of the best kind of route: nothing can beat it.
pub(super) const NEAREST_ROUTE_PREFERENCE: u32 = 3;

/// How good a route to `addr` is expected to be, the higher the better: inside
/// a local network (3), any other address (2), through an overlay network such
/// as Tailscale (1). This ranks routes that cannot be timed (a peer that does
/// not answer the connectivity check's probes) and breaks ties between routes
/// whose round trips are alike. A peer on the same LAN is a hop away. A public
/// address that answers is a direct path across the NAT. An overlay that
/// answers may be a direct path too, but it may as well be a relay (Tailscale's
/// DERP), so it is never better than a public address that answers.
pub(super) fn route_preference(addr: SocketAddr) -> u32 {
    if is_overlay(addr.ip()) {
        return 1;
    }
    match LinkRoute::of(addr) {
        LinkRoute::Lan | LinkRoute::Loopback => NEAREST_ROUTE_PREFERENCE,
        LinkRoute::Public => 2,
    }
}

/// How much slower than a public address a route through an overlay is held to be when
/// the two are compared by round trip. An overlay that answers may be relayed (Tailscale's
/// DERP), and a relay can stall for seconds after a check that looked fine, so it has to be
/// clearly faster than the public address to be used, not merely level with it.
pub(super) const OVERLAY_HANDICAP: Duration = Duration::from_millis(10);

/// What is added to a round trip to `addr` before routes are compared:
/// [`OVERLAY_HANDICAP`] for an overlay address, nothing for any other.
pub(super) fn route_handicap(addr: SocketAddr) -> Duration {
    if is_overlay(addr.ip()) {
        OVERLAY_HANDICAP
    } else {
        Duration::ZERO
    }
}

/// Which of the peer's addresses a connection may use, when a developer pins it
/// with the `JAMJAM_ROUTE` environment variable. Without it the connection
/// measures every address and takes the nearest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteOverride {
    /// `lan`: addresses inside a local network
    Lan,
    /// `public`: addresses that are neither a LAN's nor an overlay's
    Public,
    /// `tailscale`: addresses in Tailscale's range
    Overlay,
    /// An IP address: that host, on any port
    Host(IpAddr),
}

impl RouteOverride {
    /// The route named by `JAMJAM_ROUTE`; `None` when it is unset or empty.
    /// A value that names nothing is an error, so a typo does not quietly
    /// leave the route unpinned.
    pub(super) fn from_env() -> Result<Option<Self>, String> {
        match std::env::var("JAMJAM_ROUTE") {
            Ok(value) if !value.trim().is_empty() => Self::parse(&value).map(Some),
            _ => Ok(None),
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        match value.to_ascii_lowercase().as_str() {
            "lan" => Ok(Self::Lan),
            "public" => Ok(Self::Public),
            "tailscale" | "overlay" => Ok(Self::Overlay),
            _ => value.parse::<IpAddr>().map(Self::Host).map_err(|_| {
                format!("JAMJAM_ROUTE={value:?} is none of lan, public, tailscale or an IP address")
            }),
        }
    }

    pub(super) fn allows(self, addr: SocketAddr) -> bool {
        match self {
            Self::Lan => route_preference(addr) == NEAREST_ROUTE_PREFERENCE,
            Self::Public => route_preference(addr) == 2,
            Self::Overlay => is_overlay(addr.ip()),
            Self::Host(ip) => addr.ip() == ip,
        }
    }
}

/// Whether `ip` is in the range Tailscale hands out: 100.64.0.0/10 (the
/// carrier-grade NAT space) or fd7a:115c:a1e0::/48.
fn is_overlay(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 0x40,
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
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

    /// The route moved to `addr` after the link went up. Nothing has answered
    /// there yet, and the connect time and first audio stay as they were.
    pub(crate) fn route_moved(&self, addr: SocketAddr) {
        self.route
            .store(LinkRoute::of(addr).to_u8(), Ordering::Relaxed);
        self.route_confirmed.store(false, Ordering::Relaxed);
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

    /// Verifies: REQ-CON-115
    #[test]
    fn when_the_address_is_on_the_lan_it_is_preferred_to_an_overlay_and_to_a_public_one() {
        let lan = route_preference(addr("192.168.1.20:5000"));
        let overlay = route_preference(addr("100.68.50.7:5000"));
        let public = route_preference(addr("203.0.113.7:5000"));

        assert!(lan > public, "{lan} > {public}");
        assert!(lan > overlay, "{lan} > {overlay}");
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_a_public_address_and_a_tailscale_address_both_answer_the_public_one_is_preferred() {
        let overlay = route_preference(addr("100.68.50.7:5000"));
        let public = route_preference(addr("203.0.113.7:5000"));
        let public_v6 = route_preference(addr("[2001:db8::7]:5000"));

        assert!(public > overlay, "{public} > {overlay}");
        assert!(public_v6 > overlay, "{public_v6} > {overlay}");
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_the_address_is_a_tailscale_one_its_round_trip_is_held_to_be_longer_than_any_others() {
        assert_eq!(route_handicap(addr("100.68.50.7:5000")), OVERLAY_HANDICAP);
        assert_eq!(route_handicap(addr("203.0.113.7:5000")), Duration::ZERO);
        assert_eq!(route_handicap(addr("192.168.1.20:5000")), Duration::ZERO);
    }

    /// Verifies: REQ-CON-115
    #[test]
    fn when_the_address_is_in_the_tailscale_range_it_is_an_overlay_in_both_families() {
        for text in [
            "100.64.0.1:5000",
            "100.127.255.254:5000",
            "[fd7a:115c:a1e0::1]:5000",
        ] {
            assert_eq!(route_preference(addr(text)), 1, "{text}");
        }
        for text in ["100.63.255.255:5000", "100.128.0.1:5000", "[fd00::1]:5000"] {
            assert_ne!(route_preference(addr(text)), 1, "{text}");
        }
    }

    #[test]
    fn when_a_route_is_pinned_only_the_addresses_of_that_kind_are_allowed() {
        let lan = addr("192.168.1.20:5000");
        let overlay = addr("100.68.50.7:5000");
        let public = addr("203.0.113.7:5000");

        assert!(RouteOverride::Lan.allows(lan));
        assert!(!RouteOverride::Lan.allows(overlay) && !RouteOverride::Lan.allows(public));
        assert!(RouteOverride::Overlay.allows(overlay));
        assert!(!RouteOverride::Overlay.allows(lan) && !RouteOverride::Overlay.allows(public));
        assert!(RouteOverride::Public.allows(public));
        assert!(!RouteOverride::Public.allows(lan) && !RouteOverride::Public.allows(overlay));
        assert!(RouteOverride::Host(public.ip()).allows(addr("203.0.113.7:6000")));
        assert!(!RouteOverride::Host(public.ip()).allows(lan));
    }

    #[test]
    fn when_the_pinned_route_is_written_in_any_case_or_as_an_address_it_is_read() {
        assert_eq!(RouteOverride::parse(" LAN "), Ok(RouteOverride::Lan));
        assert_eq!(
            RouteOverride::parse("Tailscale"),
            Ok(RouteOverride::Overlay)
        );
        assert_eq!(RouteOverride::parse("public"), Ok(RouteOverride::Public));
        assert_eq!(
            RouteOverride::parse("203.0.113.7"),
            Ok(RouteOverride::Host("203.0.113.7".parse().unwrap()))
        );
    }

    #[test]
    fn when_the_pinned_route_names_nothing_it_is_an_error() {
        assert!(RouteOverride::parse("wifi").is_err());
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
    fn when_the_route_moves_the_route_changes_and_connect_time_stays() {
        let facts = LinkFacts::new();
        facts.link_up(addr("192.168.1.20:5000"), true, Instant::now());
        let connect_ms = facts.snapshot().connect_ms;

        facts.route_moved(addr("203.0.113.7:5000"));

        let snapshot = facts.snapshot();
        assert_eq!(snapshot.route, Some(LinkRoute::Public));
        assert_eq!(snapshot.route_confirmed, Some(false));
        assert_eq!(snapshot.connect_ms, connect_ms);
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
