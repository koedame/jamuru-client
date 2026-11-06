//! Asking the router to open the audio port (UPnP IGD, ADR-113)
//!
//! STUN tells a peer which public address and port the router gave the audio socket, but a
//! router that keeps a separate mapping per destination (a symmetric NAT) gives the peer a
//! different port than the one STUN saw, and there is no relay to fall back to. A port mapping
//! the router was asked for is the same for every sender, so the address it gives is one any
//! peer can reach.
//!
//! This asks the router through UPnP Internet Gateway Device (IGD), the protocol home routers
//! speak for this, and offers the resulting address to the room as one more candidate. It never
//! stands in the way of STUN: when there is no router that answers, the router refuses, or its
//! outside address is not a public one, [`open`] fails and the candidates are the ones STUN gave.
//!
//! A mapping is a request for the router to forward one UDP port to this machine, so it is kept
//! only as long as the audio socket is in use: it is given a lease that the router drops on its
//! own, renewed while the [`PortMapping`] lives, and removed when it is closed or dropped.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use igd_next::aio::tokio::{search_gateway, Tokio};
use igd_next::aio::Gateway;
use igd_next::{AddPortError, GatewayIpVersion, PortMappingProtocol, SearchOptions};
use tracing::{debug, info, warn};

/// What the router shows for the mapping in its list of port forwards
pub const MAPPING_DESCRIPTION: &str = "jamuru";

/// How long the router keeps a mapping that is not renewed. A crashed app leaves its mapping
/// open for at most this long.
pub const LEASE_SECS: u32 = 3600;

/// How long to wait for a router to answer the search, and for the whole [`open`]
const SEARCH_TIMEOUT: Duration = Duration::from_millis(1500);
const OPEN_TIMEOUT: Duration = Duration::from_secs(4);

/// How long to wait for the router to remove a mapping
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long after a search found no router before the next one is made. A person who enters
/// rooms one after another on a network with no such router waits for the search once.
const NO_ROUTER_REMEMBERED: Duration = Duration::from_secs(300);

/// The outside ports tried when the router already forwards the audio port to someone else
const OTHER_PORTS: std::ops::Range<u16> = 49152..49202;
const OTHER_PORT_TRIES: usize = 3;

/// Why no port mapping was made
#[derive(Debug, thiserror::Error)]
pub enum PortMappingError {
    /// Nothing answered the search, or what answered is not a router that maps ports
    #[error("no router that maps ports answered: {0}")]
    NoRouter(String),
    /// The router's outside address is not one a peer on the internet can send to (the router
    /// is behind another NAT), so a mapping on it would help no one
    #[error("the router's outside address {0} is not a public one")]
    NotPublic(IpAddr),
    /// The router was asked and did not map the port
    #[error("the router did not map the port: {0}")]
    Refused(String),
    /// The router did not answer in time
    #[error("the router did not answer in time")]
    TimedOut,
}

/// A port the router forwards to the audio socket, kept alive until closed or dropped
pub struct PortMapping {
    external: SocketAddr,
    mapped: Arc<Mapped>,
    renewal: Option<tokio::task::JoinHandle<()>>,
}

struct Mapped {
    gateway: Gateway<Tokio>,
    external_port: u16,
    local: SocketAddr,
    /// 0 for a router that keeps only mappings with no end
    lease_secs: u32,
    removed: AtomicBool,
}

impl std::fmt::Debug for PortMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortMapping")
            .field("external", &self.external)
            .field("local", &self.mapped.local)
            .field("lease_secs", &self.mapped.lease_secs)
            .finish()
    }
}

impl PortMapping {
    /// The public address and port peers can send to
    pub fn external_addr(&self) -> SocketAddr {
        self.external
    }

    /// Removes the mapping from the router, waiting for its answer
    pub async fn close(mut self) {
        if let Some(renewal) = self.renewal.take() {
            renewal.abort();
        }
        self.mapped.remove().await;
    }
}

impl Drop for PortMapping {
    /// A mapping dropped without [`PortMapping::close`] is removed in the background; with no
    /// runtime to do that, the router drops it when the lease ends.
    fn drop(&mut self) {
        if let Some(renewal) = self.renewal.take() {
            renewal.abort();
        }
        if self.mapped.removed.load(Ordering::SeqCst) {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let mapped = self.mapped.clone();
            runtime.spawn(async move { mapped.remove().await });
        }
    }
}

impl Mapped {
    async fn remove(&self) {
        if self.removed.swap(true, Ordering::SeqCst) {
            return;
        }
        let removal = self
            .gateway
            .remove_port(PortMappingProtocol::UDP, self.external_port);
        match tokio::time::timeout(CLOSE_TIMEOUT, removal).await {
            Ok(Ok(())) => info!("Closed the router's port {}", self.external_port),
            Ok(Err(e)) => warn!(
                "The router did not remove port {} ({}); it ends with the lease",
                self.external_port, e
            ),
            Err(_) => warn!(
                "The router did not answer removing port {}; it ends with the lease",
                self.external_port
            ),
        }
    }

    async fn add(&self) -> Result<(), AddPortError> {
        self.gateway
            .add_port(
                PortMappingProtocol::UDP,
                self.external_port,
                self.local,
                self.lease_secs,
                MAPPING_DESCRIPTION,
            )
            .await
    }
}

/// How the router is searched for and how long a mapping lasts
pub struct Options {
    pub search: SearchOptions,
    pub lease_secs: u32,
}

impl Default for Options {
    /// The way the app asks: on the multicast address UPnP routers listen on, with
    /// [`LEASE_SECS`]
    fn default() -> Self {
        let mut search = SearchOptions::default();
        search.timeout = Some(SEARCH_TIMEOUT);
        search.single_search_timeout = Some(SEARCH_TIMEOUT);
        search.gateway_ip_version = GatewayIpVersion::V4;
        Self {
            search,
            lease_secs: LEASE_SECS,
        }
    }
}

/// [`open_with`] for the router on the network, remembering for a while that there was none
pub async fn open(local_port: u16) -> Result<PortMapping, PortMappingError> {
    if let Some(since) = *NO_ROUTER_SINCE.lock().unwrap_or_else(|e| e.into_inner()) {
        if since.elapsed() < NO_ROUTER_REMEMBERED {
            debug!("Not searching for a router again: none answered a moment ago");
            return Err(PortMappingError::NoRouter("none answered earlier".into()));
        }
    }
    let result = open_with(Options::default(), local_port).await;
    *NO_ROUTER_SINCE.lock().unwrap_or_else(|e| e.into_inner()) = match &result {
        Err(PortMappingError::NoRouter(_)) | Err(PortMappingError::TimedOut) => {
            Some(Instant::now())
        }
        _ => None,
    };
    result
}

static NO_ROUTER_SINCE: Mutex<Option<Instant>> = Mutex::new(None);

/// Asks the router found by `options.search` to forward its public UDP port `local_port` (or, when that
/// is taken, another) to this machine's `local_port`.
///
/// Gives up after a few seconds: entering a room waits for this.
pub async fn open_with(options: Options, local_port: u16) -> Result<PortMapping, PortMappingError> {
    tokio::time::timeout(OPEN_TIMEOUT, open_unbounded(options, local_port))
        .await
        .unwrap_or(Err(PortMappingError::TimedOut))
}

async fn open_unbounded(
    options: Options,
    local_port: u16,
) -> Result<PortMapping, PortMappingError> {
    let allowed = options.search.allowed_gateway_ips.clone();
    let gateway = search_gateway(options.search)
        .await
        .map_err(|e| PortMappingError::NoRouter(e.to_string()))?;
    debug!("Found a router that maps ports at {}", gateway.addr);
    if !on_local_network(gateway.addr.ip(), &allowed) {
        return Err(PortMappingError::NoRouter(format!(
            "{} answered from outside the local network",
            gateway.addr.ip()
        )));
    }

    let external_ip = gateway
        .get_external_ip()
        .await
        .map_err(|e| PortMappingError::Refused(e.to_string()))?;
    if !is_public(external_ip) {
        return Err(PortMappingError::NotPublic(external_ip));
    }

    let local_ip = local_ip_toward(gateway.addr.ip()).map_err(|e| {
        PortMappingError::Refused(format!("no local address toward the router: {}", e))
    })?;
    let local = SocketAddr::new(local_ip.into(), local_port);

    let mut mapped = Mapped {
        gateway,
        external_port: local_port,
        local,
        lease_secs: options.lease_secs,
        removed: AtomicBool::new(false),
    };
    add_first_free_port(&mut mapped).await?;
    info!(
        "The router forwards {}:{} to {}",
        external_ip, mapped.external_port, mapped.local
    );

    let external = SocketAddr::new(external_ip, mapped.external_port);
    let mapped = Arc::new(mapped);
    let renewal = (mapped.lease_secs > 0).then(|| tokio::spawn(renew(mapped.clone())));
    Ok(PortMapping {
        external,
        mapped,
        renewal,
    })
}

/// Maps the audio port to the same outside port or, when the router already forwards that to
/// someone else, to a few others. A router that keeps only mappings with no end gets none: if the
/// app were killed, the forward would stay and later send the port's traffic to whatever binds it.
async fn add_first_free_port(mapped: &mut Mapped) -> Result<(), PortMappingError> {
    let mut ports = vec![mapped.local.port()];
    ports.extend((0..OTHER_PORT_TRIES).map(|_| {
        OTHER_PORTS.start + rand::random::<u16>() % (OTHER_PORTS.end - OTHER_PORTS.start)
    }));
    let mut last = String::new();
    for port in ports {
        mapped.external_port = port;
        match mapped.add().await {
            Ok(()) => return Ok(()),
            Err(AddPortError::PortInUse) => {
                last = "the port is forwarded to another machine".into()
            }
            Err(e) => return Err(PortMappingError::Refused(e.to_string())),
        }
    }
    Err(PortMappingError::Refused(last))
}

/// Adds the mapping again before its lease ends, for as long as the task lives
async fn renew(mapped: Arc<Mapped>) {
    let mut every = tokio::time::interval(Duration::from_secs(u64::from(
        (mapped.lease_secs / 2).max(1),
    )));
    every.tick().await;
    loop {
        every.tick().await;
        match mapped.add().await {
            Ok(()) => debug!("Renewed the router's port {}", mapped.external_port),
            Err(e) => warn!(
                "The router did not renew port {} ({}); it ends with the lease",
                mapped.external_port, e
            ),
        }
    }
}

/// The address of this machine on the network that reaches `gateway`: the one a router
/// forwards to. Connecting a UDP socket sends nothing; it only picks the route.
fn local_ip_toward(gateway: IpAddr) -> std::io::Result<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(SocketAddr::new(gateway, 9))?;
    match socket.local_addr()?.ip() {
        IpAddr::V4(ip) => Ok(ip),
        IpAddr::V6(ip) => Err(std::io::Error::other(format!(
            "the route to the router starts at {}, not an IPv4 address",
            ip
        ))),
    }
}

/// Whether `ip` is an address a home router has: private or link-local IPv4. A router answering
/// from anywhere else is not the one in this house. `allowed` are the addresses the caller named.
fn on_local_network(ip: IpAddr, allowed: &[IpAddr]) -> bool {
    allowed.contains(&ip) || matches!(ip, IpAddr::V4(v4) if v4.is_private() || v4.is_link_local())
}

/// Whether a peer on the internet can send to `ip`: not a private, shared (carrier-grade NAT),
/// link-local or otherwise reserved IPv4 address
fn is_public(ip: IpAddr) -> bool {
    let IpAddr::V4(ip) = ip else { return false };
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || a >= 240)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(text: &str) -> bool {
        is_public(text.parse().unwrap())
    }

    #[test]
    fn when_the_outside_address_is_a_home_address_then_it_is_not_public() {
        assert!(!public("192.168.1.1"));
        assert!(!public("10.0.0.7"));
        assert!(!public("172.16.5.5"));
        assert!(!public("169.254.1.1"));
        assert!(!public("127.0.0.1"));
        assert!(!public("0.0.0.0"));
    }

    #[test]
    fn when_a_router_answers_from_a_home_address_then_it_is_on_the_local_network() {
        assert!(on_local_network("192.168.1.1".parse().unwrap(), &[]));
        assert!(on_local_network("10.0.0.1".parse().unwrap(), &[]));
        assert!(on_local_network("169.254.1.1".parse().unwrap(), &[]));
    }

    #[test]
    fn when_a_router_answers_from_a_public_or_local_machine_address_then_it_is_not_on_the_local_network(
    ) {
        assert!(!on_local_network("203.0.113.9".parse().unwrap(), &[]));
        assert!(!on_local_network("127.0.0.1".parse().unwrap(), &[]));
        assert!(!on_local_network("2001:db8::1".parse().unwrap(), &[]));
        let named = ["127.0.0.1".parse().unwrap()];
        assert!(on_local_network("127.0.0.1".parse().unwrap(), &named));
    }

    #[test]
    fn when_the_outside_address_is_carrier_grade_nat_then_it_is_not_public() {
        assert!(!public("100.64.0.1"));
        assert!(!public("100.127.255.254"));
        assert!(public("100.63.255.255"));
        assert!(public("100.128.0.1"));
    }

    #[test]
    fn when_the_outside_address_is_on_the_internet_then_it_is_public() {
        assert!(public("203.0.113.9"));
        assert!(public("8.8.8.8"));
        assert!(!public("224.0.0.1"));
        assert!(!public("255.255.255.255"));
        assert!(!public("2001:db8::1"));
    }

    #[test]
    fn when_the_gateway_is_on_this_machine_then_the_local_address_is_loopback() {
        let ip = local_ip_toward("127.0.0.1".parse().unwrap()).unwrap();
        assert!(ip.is_loopback());
    }
}
