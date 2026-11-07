//! Network diagnostics for jamuru
//!
//! Provides network environment checks including:
//! - IPv4/IPv6 support detection (whether IPv6 gets out, not only whether an address exists)
//! - How the router maps ports, measured through STUN from one socket
//! - Whether the router answers UPnP (a read-only look; nothing is forwarded)
//! - Which peers the above make hard to reach (ADR-114)
//! - Connection stability (RTT, jitter, packet loss)
//! - Signaling server connectivity

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use super::{DiagnosticGrade, DiagnosticProblem, ProblemCode, ProblemSeverity};
use crate::network::port_mapping::{self, PortMappingError, RouterAnswer};
use crate::network::{
    bind_audio_std, IpFamilies, SignalingClient, StunClient, DEFAULT_STUN_SERVERS,
};

/// IPv4/IPv6 support status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpSupport {
    /// Whether IPv4 is available
    pub ipv4_available: bool,
    /// Whether IPv6 is available
    pub ipv6_available: bool,
    /// Local IPv4 addresses found
    pub ipv4_addresses: Vec<String>,
    /// Local IPv6 addresses found
    pub ipv6_addresses: Vec<String>,
    /// Public IPv4 address (via STUN)
    pub public_ipv4: Option<String>,
    /// Public IPv6 address (via STUN)
    pub public_ipv6: Option<String>,
}

/// How the router maps the audio socket's port, as STUN servers at different addresses see it
/// from one socket.
///
/// What is measured is the mapping only. Whether the router then lets a peer's packets in
/// (its filtering) cannot be told with these servers, so the old cone names are not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NatType {
    /// The host's own address is the public one: no NAT
    NoNat,
    /// Every server saw the same address and port, so a peer is sent where STUN saw the host
    Stable,
    /// Servers saw different ports (or addresses): the router maps per destination, and the
    /// address STUN saw is not the one a peer is reached at
    PerDestination,
    /// Fewer than two servers answered, or none
    Unknown,
}

/// Whether IPv6 gets out of this host
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ipv6Status {
    /// A global address, and a STUN server answered over IPv6
    Usable,
    /// A global address, but no STUN server answered over IPv6
    NoRoute,
    /// No global IPv6 address
    NoAddress,
}

/// What the router did when looked for with UPnP (IGD). Nothing is forwarded to find out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpnpStatus {
    /// The router answered and its outside address is a public one
    Answers,
    /// The setting "Open the router's port automatically" is off, so the router was not asked
    Off,
    /// Nothing that maps ports answered
    NoRouter,
    /// A router answered and refused (some turn UPnP off in their settings)
    Refused,
    /// The router's outside address is not public: another router (or the provider's NAT) sits
    /// between it and the internet
    DoubleNat,
}

/// Which peers this network makes hard to reach, from [`NatType`] and [`UpnpStatus`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerReach {
    /// A peer is reached at an address that works for any sender
    Wide,
    /// Peers whose own router also maps per destination cannot be reached over IPv4
    Narrow,
    /// STUN did not answer enough to tell
    Unknown,
}

impl PeerReach {
    /// A router that forwards the port (UPnP) gives every sender the same address, and so does a
    /// router that maps the same way for every destination or none at all
    pub fn of(nat: NatType, upnp: UpnpStatus) -> Self {
        if upnp == UpnpStatus::Answers {
            return PeerReach::Wide;
        }
        match nat {
            NatType::NoNat | NatType::Stable => PeerReach::Wide,
            NatType::PerDestination => PeerReach::Narrow,
            NatType::Unknown => PeerReach::Unknown,
        }
    }
}

/// Connection stability metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStability {
    /// Average RTT to STUN servers (ms)
    pub avg_rtt_ms: Option<f64>,
    /// Minimum RTT observed (ms)
    pub min_rtt_ms: Option<f64>,
    /// Maximum RTT observed (ms)
    pub max_rtt_ms: Option<f64>,
    /// Number of successful probes
    pub successful_probes: u32,
    /// Number of failed probes
    pub failed_probes: u32,
    /// Estimated packet loss rate (0.0 - 1.0)
    pub packet_loss_rate: f64,
}

impl ConnectionStability {
    /// Convert to diagnostic grade
    pub fn to_grade(&self) -> DiagnosticGrade {
        // If we have no successful probes, grade is Unknown
        if self.successful_probes == 0 {
            return DiagnosticGrade::Unknown;
        }

        // Calculate based on packet loss and RTT consistency
        let loss_score = if self.packet_loss_rate < 0.01 {
            100
        } else if self.packet_loss_rate < 0.05 {
            75
        } else if self.packet_loss_rate < 0.1 {
            50
        } else {
            25
        };

        // Check RTT consistency (max - min should be small)
        let consistency_score = match (self.min_rtt_ms, self.max_rtt_ms) {
            (Some(min), Some(max)) => {
                let range = max - min;
                if range < 5.0 {
                    100
                } else if range < 20.0 {
                    75
                } else if range < 50.0 {
                    50
                } else {
                    25
                }
            }
            _ => 50,
        };

        let total_score = (loss_score + consistency_score) / 2;

        if total_score >= 90 {
            DiagnosticGrade::A
        } else if total_score >= 70 {
            DiagnosticGrade::B
        } else {
            DiagnosticGrade::C
        }
    }
}

/// Signaling server diagnostics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingDiagnostics {
    /// Whether connection was successful
    pub connected: bool,
    /// Connection time (ms)
    pub connection_time_ms: Option<u64>,
    /// Error message if failed
    pub error: Option<String>,
}

/// Complete network diagnostics result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkDiagnosticsResult {
    /// IP support status
    pub ip_support: IpSupport,
    /// How the router maps the audio socket's port
    pub nat_type: NatType,
    /// Whether IPv6 gets out
    pub ipv6: Ipv6Status,
    /// What the router did when looked for with UPnP
    pub upnp: UpnpStatus,
    /// Which peers the above make hard to reach
    pub peer_reach: PeerReach,
    /// Connection stability grade
    pub connection_stability: DiagnosticGrade,
    /// Jitter (RTT variation) in ms
    pub jitter_ms: Option<f64>,
    /// Stability metrics
    pub stability_metrics: ConnectionStability,
    /// Signaling server diagnostics
    pub signaling: SignalingDiagnostics,
    /// Detected problems
    pub problems: Vec<DiagnosticProblem>,
}

/// Network diagnostics runner
pub struct NetworkDiagnostics;

impl NetworkDiagnostics {
    /// Run all network diagnostics. The signaling check connects the way a
    /// session does: through `signaling`'s server, proving its device identity.
    ///
    /// `ask_router` is the setting "Open the router's port automatically": when it is off the
    /// router is not looked for, as in a session. When it is on, the router is only looked for
    /// and asked its outside address; no port is forwarded.
    pub async fn run(signaling: &SignalingClient, ask_router: bool) -> NetworkDiagnosticsResult {
        let mut problems = Vec::new();

        // The router is looked for while STUN is asked, as when entering a room
        let ((ip_support, mapped_ipv4), upnp) =
            tokio::join!(Self::check_ip_support(), Self::check_upnp(ask_router),);

        // Add problems for IP support issues
        if !ip_support.ipv4_available && !ip_support.ipv6_available {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Error,
                category: "network".to_string(),
                code: ProblemCode::NoConnectivity,
            });
        } else if !ip_support.ipv6_available {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Info,
                category: "network".to_string(),
                code: ProblemCode::NoIpv6,
            });
        }

        if crate::network::default_route_is_wifi() == Some(true) {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Warning,
                category: "network".to_string(),
                code: ProblemCode::WifiConnection,
            });
        }

        let ipv6 = ipv6_status(&ip_support);
        let nat_type = nat_type_of(&ip_support, &mapped_ipv4);
        let peer_reach = PeerReach::of(nat_type, upnp);
        if ip_support.ipv4_available || ip_support.ipv6_available {
            problems.extend(reach_problems(ipv6, upnp, peer_reach));
        }

        // Check connection stability
        let stability_metrics = Self::measure_stability().await;
        let connection_stability = stability_metrics.to_grade();

        // Calculate jitter from RTT measurements
        let jitter_ms = match (stability_metrics.min_rtt_ms, stability_metrics.max_rtt_ms) {
            (Some(min), Some(max)) => Some((max - min) / 2.0),
            _ => None,
        };

        // Add problems for stability issues
        if connection_stability == DiagnosticGrade::C {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Warning,
                category: "network".to_string(),
                code: ProblemCode::UnstableConnection,
            });
        }

        if let Some(jitter) = jitter_ms {
            if jitter > 10.0 {
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Warning,
                    category: "network".to_string(),
                    code: ProblemCode::HighJitter { jitter_ms: jitter },
                });
            }
        }

        // Check signaling server
        let signaling_result = Self::check_signaling(signaling).await;

        if !signaling_result.connected {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Error,
                category: "network".to_string(),
                code: ProblemCode::SignalingUnreachable {
                    url: signaling.server_url().to_string(),
                    error: signaling_result.error.clone(),
                },
            });
        }

        NetworkDiagnosticsResult {
            ip_support,
            nat_type,
            ipv6,
            upnp,
            peer_reach,
            connection_stability,
            jitter_ms,
            stability_metrics,
            signaling: signaling_result,
            problems,
        }
    }

    /// Check IPv4/IPv6 support, and which outside port each of several STUN servers sees the
    /// host at, both from one dual-stack socket like the one a session sends its audio from
    /// (ADR-093). A fresh socket for each server would get a fresh mapping each time, which says
    /// nothing about how the router maps one socket.
    async fn check_ip_support() -> (IpSupport, Vec<SocketAddr>) {
        let mut ipv4_addresses = Vec::new();
        let mut ipv6_addresses = Vec::new();
        let mut public_ipv4 = None;
        let mut public_ipv6 = None;

        // Get local IPv4 address
        if let Ok(IpAddr::V4(addr)) = local_ip_address::local_ip() {
            if !addr.is_loopback() && !addr.is_link_local() {
                ipv4_addresses.push(addr.to_string());
            }
        }

        // Get local IPv6 address
        if let Ok(IpAddr::V6(addr)) = local_ip_address::local_ipv6() {
            if is_global_ipv6(&addr) {
                ipv6_addresses.push(addr.to_string());
            }
        }

        // Also try to list all network interfaces for more complete detection
        if let Ok(list) = local_ip_address::list_afinet_netifas() {
            for (_, ip) in list {
                match ip {
                    IpAddr::V4(addr)
                        if !addr.is_loopback()
                            && !addr.is_link_local()
                            && !ipv4_addresses.contains(&addr.to_string()) =>
                    {
                        ipv4_addresses.push(addr.to_string());
                    }
                    IpAddr::V6(addr)
                        if is_global_ipv6(&addr) && !ipv6_addresses.contains(&addr.to_string()) =>
                    {
                        ipv6_addresses.push(addr.to_string());
                    }
                    _ => {}
                }
            }
        }

        let mut mapped_ipv4 = Vec::new();
        let socket = bind_audio_std(0)
            .and_then(|socket| UdpSocket::from_std(socket).map_err(Into::into))
            .map(Arc::new);
        match socket {
            Ok(socket) => {
                let client = StunClient::with_timeout(socket.clone(), 2000);
                let ask = IpFamilies {
                    v4: !ipv4_addresses.is_empty(),
                    v6: !ipv6_addresses.is_empty(),
                };
                for found in client
                    .discover_public_addresses_from(DEFAULT_STUN_SERVERS, ask)
                    .await
                {
                    match found.mapped_address.ip() {
                        IpAddr::V4(ip) => public_ipv4 = Some(ip.to_string()),
                        IpAddr::V6(ip) => public_ipv6 = Some(ip.to_string()),
                    }
                }
                if !ipv4_addresses.is_empty() {
                    mapped_ipv4 = client
                        .mappings_seen_by(DEFAULT_STUN_SERVERS)
                        .await
                        .into_iter()
                        .map(|found| found.mapped_address)
                        .collect();
                }
            }
            Err(e) => warn!("No socket to ask STUN through: {}", e),
        }

        let support = IpSupport {
            ipv4_available: !ipv4_addresses.is_empty(),
            ipv6_available: !ipv6_addresses.is_empty(),
            ipv4_addresses,
            ipv6_addresses,
            public_ipv4,
            public_ipv6,
        };
        (support, mapped_ipv4)
    }

    /// Look for the router with UPnP, without forwarding anything. `ask` is the setting that lets
    /// the app talk to the router at all.
    async fn check_upnp(ask: bool) -> UpnpStatus {
        if !ask {
            return UpnpStatus::Off;
        }
        upnp_status(port_mapping::probe().await)
    }

    /// Measure connection stability
    async fn measure_stability() -> ConnectionStability {
        let mut rtt_samples = Vec::new();
        let mut failed_probes = 0u32;
        let probe_count = 10;

        // Send multiple probes to STUN servers
        for i in 0..probe_count {
            let server = DEFAULT_STUN_SERVERS[i % DEFAULT_STUN_SERVERS.len()];

            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await {
                let start = Instant::now();
                let client = StunClient::with_timeout(socket, 1000);

                match client.binding_request(server).await {
                    Ok(_) => {
                        let rtt = start.elapsed().as_secs_f64() * 1000.0;
                        rtt_samples.push(rtt);
                        debug!("STUN probe {} to {}: {:.2}ms", i, server, rtt);
                    }
                    Err(e) => {
                        warn!("STUN probe {} to {} failed: {}", i, server, e);
                        failed_probes += 1;
                    }
                }
            } else {
                failed_probes += 1;
            }

            // Small delay between probes
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let successful_probes = rtt_samples.len() as u32;

        if rtt_samples.is_empty() {
            return ConnectionStability {
                avg_rtt_ms: None,
                min_rtt_ms: None,
                max_rtt_ms: None,
                successful_probes: 0,
                failed_probes,
                packet_loss_rate: 1.0,
            };
        }

        let avg_rtt = rtt_samples.iter().sum::<f64>() / rtt_samples.len() as f64;
        let min_rtt = rtt_samples.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_rtt = rtt_samples
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let packet_loss_rate = failed_probes as f64 / probe_count as f64;

        ConnectionStability {
            avg_rtt_ms: Some(avg_rtt),
            min_rtt_ms: Some(min_rtt),
            max_rtt_ms: Some(max_rtt),
            successful_probes,
            failed_probes,
            packet_loss_rate,
        }
    }

    /// Check signaling server connectivity: ask the server where its signaling
    /// is and connect there, as joining a room does
    async fn check_signaling(client: &SignalingClient) -> SignalingDiagnostics {
        let start = Instant::now();

        let connect_result = tokio::time::timeout(Duration::from_secs(5), client.connect()).await;

        match connect_result {
            Ok(Ok(connection)) => {
                let connection_time = start.elapsed().as_millis() as u64;
                info!(
                    "Signaling server connection successful in {}ms",
                    connection_time
                );

                // Close the connection
                drop(connection);

                SignalingDiagnostics {
                    connected: true,
                    connection_time_ms: Some(connection_time),
                    error: None,
                }
            }
            Ok(Err(e)) => {
                warn!("Signaling server connection failed: {}", e);
                SignalingDiagnostics {
                    connected: false,
                    connection_time_ms: None,
                    error: Some(e.to_string()),
                }
            }
            Err(_) => {
                warn!("Signaling server connection timed out");
                SignalingDiagnostics {
                    connected: false,
                    connection_time_ms: None,
                    error: Some("Connection timed out".to_string()),
                }
            }
        }
    }
}

/// Whether `addr` is an IPv6 global unicast address (`2000::/3`): one a host elsewhere on the
/// internet can send to. The same test the candidates of a session pass.
fn is_global_ipv6(addr: &Ipv6Addr) -> bool {
    (addr.segments()[0] & 0xe000) == 0x2000
}

/// Whether IPv6 gets out: an address that is global, and a STUN server that answered over it
fn ipv6_status(ip_support: &IpSupport) -> Ipv6Status {
    match (ip_support.ipv6_available, &ip_support.public_ipv6) {
        (false, _) => Ipv6Status::NoAddress,
        (true, Some(_)) => Ipv6Status::Usable,
        (true, None) => Ipv6Status::NoRoute,
    }
}

/// How the router maps the audio socket, from the addresses the STUN servers saw it at
fn nat_type_of(ip_support: &IpSupport, seen: &[SocketAddr]) -> NatType {
    if let Some(public) = &ip_support.public_ipv4 {
        if ip_support.ipv4_addresses.contains(public) {
            return NatType::NoNat;
        }
    }
    match seen {
        [] | [_] => NatType::Unknown,
        [first, rest @ ..] if rest.iter().all(|addr| addr == first) => NatType::Stable,
        _ => NatType::PerDestination,
    }
}

/// What the router's answer to the look says
fn upnp_status(answer: Result<RouterAnswer, PortMappingError>) -> UpnpStatus {
    match answer {
        Ok(_) => UpnpStatus::Answers,
        Err(PortMappingError::NoRouter(_) | PortMappingError::TimedOut) => UpnpStatus::NoRouter,
        Err(PortMappingError::Refused(_)) => UpnpStatus::Refused,
        Err(PortMappingError::NotPublic(_)) => UpnpStatus::DoubleNat,
    }
}

/// What the person can do about a network that reaches few peers, or that did not say
fn reach_problems(ipv6: Ipv6Status, upnp: UpnpStatus, reach: PeerReach) -> Vec<DiagnosticProblem> {
    let problem = |severity, code| DiagnosticProblem {
        severity,
        category: "network".to_string(),
        code,
    };
    let mut problems = Vec::new();
    if ipv6 == Ipv6Status::NoRoute {
        problems.push(problem(ProblemSeverity::Info, ProblemCode::Ipv6Unreachable));
    }
    match reach {
        PeerReach::Narrow => {
            // IPv6 gets around the mapping for peers that have it too
            let severity = if ipv6 == Ipv6Status::Usable {
                ProblemSeverity::Info
            } else {
                ProblemSeverity::Warning
            };
            problems.push(problem(severity, ProblemCode::PerDestinationNat));
            problems.push(problem(
                ProblemSeverity::Info,
                match upnp {
                    UpnpStatus::Off => ProblemCode::UpnpOff,
                    UpnpStatus::Refused => ProblemCode::UpnpRefused,
                    UpnpStatus::DoubleNat => ProblemCode::UpnpDoubleNat,
                    UpnpStatus::NoRouter | UpnpStatus::Answers => ProblemCode::UpnpNoRouter,
                },
            ));
        }
        PeerReach::Unknown => {
            problems.push(problem(
                ProblemSeverity::Warning,
                ProblemCode::StunUnanswered,
            ));
        }
        PeerReach::Wide => {}
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_stability_grade() {
        // Perfect stability
        let stable = ConnectionStability {
            avg_rtt_ms: Some(10.0),
            min_rtt_ms: Some(8.0),
            max_rtt_ms: Some(12.0),
            successful_probes: 10,
            failed_probes: 0,
            packet_loss_rate: 0.0,
        };
        assert_eq!(stable.to_grade(), DiagnosticGrade::A);

        // Moderate stability
        let moderate = ConnectionStability {
            avg_rtt_ms: Some(30.0),
            min_rtt_ms: Some(15.0),
            max_rtt_ms: Some(45.0),
            successful_probes: 8,
            failed_probes: 2,
            packet_loss_rate: 0.2,
        };
        assert!(matches!(
            moderate.to_grade(),
            DiagnosticGrade::B | DiagnosticGrade::C
        ));

        // No successful probes
        let failed = ConnectionStability {
            avg_rtt_ms: None,
            min_rtt_ms: None,
            max_rtt_ms: None,
            successful_probes: 0,
            failed_probes: 10,
            packet_loss_rate: 1.0,
        };
        assert_eq!(failed.to_grade(), DiagnosticGrade::Unknown);
    }

    fn support(public_v4: Option<&str>, local_v4: &[&str]) -> IpSupport {
        IpSupport {
            ipv4_available: true,
            ipv6_available: false,
            ipv4_addresses: local_v4.iter().map(|a| a.to_string()).collect(),
            ipv6_addresses: Vec::new(),
            public_ipv4: public_v4.map(str::to_string),
            public_ipv6: None,
        }
    }

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// Given the same address and port seen by every STUN server
    /// When the router's mapping is judged
    /// Then it is Stable
    ///
    /// Verifies: REQ-CON-166
    #[test]
    fn when_every_server_sees_the_same_port_the_mapping_is_stable() {
        let seen = [addr("198.51.100.1:40000"), addr("198.51.100.1:40000")];
        assert_eq!(
            nat_type_of(&support(Some("198.51.100.1"), &["192.168.0.5"]), &seen),
            NatType::Stable
        );
    }

    /// Given servers that see the same address at different ports
    /// When the router's mapping is judged
    /// Then it maps per destination
    ///
    /// Verifies: REQ-CON-166
    #[test]
    fn when_the_servers_see_different_ports_the_mapping_is_per_destination() {
        let seen = [addr("198.51.100.1:40000"), addr("198.51.100.1:40001")];
        assert_eq!(
            nat_type_of(&support(Some("198.51.100.1"), &["192.168.0.5"]), &seen),
            NatType::PerDestination
        );
    }

    /// Given fewer than two answers
    /// When the router's mapping is judged
    /// Then it is Unknown, not guessed
    ///
    /// Verifies: REQ-CON-166
    #[test]
    fn when_fewer_than_two_servers_answer_the_mapping_is_unknown() {
        let s = support(Some("198.51.100.1"), &["192.168.0.5"]);
        assert_eq!(nat_type_of(&s, &[]), NatType::Unknown);
        assert_eq!(
            nat_type_of(&s, &[addr("198.51.100.1:40000")]),
            NatType::Unknown
        );
    }

    /// Given the host holds the public address itself
    /// When the router's mapping is judged
    /// Then there is no NAT, whatever the servers saw
    ///
    /// Verifies: REQ-CON-166
    #[test]
    fn when_the_host_holds_the_public_address_there_is_no_nat() {
        assert_eq!(
            nat_type_of(&support(Some("203.0.113.7"), &["203.0.113.7"]), &[]),
            NatType::NoNat
        );
    }

    /// Verifies: REQ-CON-167
    #[test]
    fn ipv6_is_usable_only_when_an_address_exists_and_a_server_answered_over_it() {
        let mut s = support(None, &[]);
        assert_eq!(ipv6_status(&s), Ipv6Status::NoAddress);
        s.ipv6_available = true;
        assert_eq!(ipv6_status(&s), Ipv6Status::NoRoute);
        s.public_ipv6 = Some("2001:db8::1".to_string());
        assert_eq!(ipv6_status(&s), Ipv6Status::Usable);
    }

    /// Verifies: REQ-CON-167
    #[test]
    fn only_global_unicast_addresses_count_as_ipv6_that_gets_out() {
        assert!(!is_global_ipv6(&"fe80::1".parse().unwrap()));
        assert!(!is_global_ipv6(&"fd00::1".parse().unwrap()));
        assert!(is_global_ipv6(&"2001:db8::1".parse().unwrap()));
    }

    /// Verifies: REQ-CON-167
    #[test]
    fn what_the_router_said_to_the_look_becomes_a_status() {
        let ip = "203.0.113.7".parse().unwrap();
        assert_eq!(
            upnp_status(Ok(RouterAnswer { external_ip: ip })),
            UpnpStatus::Answers
        );
        assert_eq!(
            upnp_status(Err(PortMappingError::NoRouter("x".into()))),
            UpnpStatus::NoRouter
        );
        assert_eq!(
            upnp_status(Err(PortMappingError::TimedOut)),
            UpnpStatus::NoRouter
        );
        assert_eq!(
            upnp_status(Err(PortMappingError::Refused("x".into()))),
            UpnpStatus::Refused
        );
        assert_eq!(
            upnp_status(Err(PortMappingError::NotPublic(
                "192.168.1.1".parse().unwrap()
            ))),
            UpnpStatus::DoubleNat
        );
    }

    /// Verifies: REQ-CON-166
    #[test]
    fn a_router_that_forwards_the_port_reaches_every_peer_even_when_it_maps_per_destination() {
        assert_eq!(
            PeerReach::of(NatType::PerDestination, UpnpStatus::Answers),
            PeerReach::Wide
        );
        assert_eq!(
            PeerReach::of(NatType::PerDestination, UpnpStatus::Off),
            PeerReach::Narrow
        );
        assert_eq!(
            PeerReach::of(NatType::Stable, UpnpStatus::NoRouter),
            PeerReach::Wide
        );
        assert_eq!(
            PeerReach::of(NatType::Unknown, UpnpStatus::NoRouter),
            PeerReach::Unknown
        );
    }

    fn codes(problems: &[DiagnosticProblem]) -> Vec<String> {
        problems.iter().map(|p| format!("{:?}", p.code)).collect()
    }

    /// Verifies: REQ-CON-168
    #[test]
    fn when_peers_are_hard_to_reach_the_problems_say_what_to_do() {
        let narrow = reach_problems(Ipv6Status::NoAddress, UpnpStatus::Off, PeerReach::Narrow);
        assert_eq!(codes(&narrow), ["PerDestinationNat", "UpnpOff"]);
        assert_eq!(narrow[0].severity, ProblemSeverity::Warning);

        let double = reach_problems(
            Ipv6Status::NoAddress,
            UpnpStatus::DoubleNat,
            PeerReach::Narrow,
        );
        assert!(codes(&double).contains(&"UpnpDoubleNat".to_string()));

        let refused = reach_problems(
            Ipv6Status::NoAddress,
            UpnpStatus::Refused,
            PeerReach::Narrow,
        );
        assert!(codes(&refused).contains(&"UpnpRefused".to_string()));
    }

    /// Verifies: REQ-CON-168
    #[test]
    fn when_ipv6_works_a_per_destination_mapping_is_only_information() {
        let problems = reach_problems(Ipv6Status::Usable, UpnpStatus::NoRouter, PeerReach::Narrow);
        assert_eq!(problems[0].severity, ProblemSeverity::Info);
    }

    /// Verifies: REQ-CON-168
    #[test]
    fn when_the_network_reaches_everyone_and_ipv6_is_absent_nothing_is_raised() {
        assert!(reach_problems(Ipv6Status::NoAddress, UpnpStatus::Off, PeerReach::Wide).is_empty());
    }

    /// Verifies: REQ-CON-168
    #[test]
    fn when_stun_did_not_answer_it_is_said_and_when_ipv6_has_no_route_it_is_noted() {
        let unknown = reach_problems(Ipv6Status::NoRoute, UpnpStatus::Off, PeerReach::Unknown);
        assert_eq!(codes(&unknown), ["Ipv6Unreachable", "StunUnanswered"]);
    }
}
