//! Network diagnostics for jamuru
//!
//! Provides network environment checks including:
//! - IPv4/IPv6 support detection
//! - NAT type detection via STUN
//! - Connection stability (RTT, jitter, packet loss)
//! - Signaling server connectivity

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use super::{DiagnosticGrade, DiagnosticProblem, ProblemCode, ProblemSeverity};
use crate::network::{SignalingClient, StunClient, DEFAULT_STUN_SERVERS};

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

/// NAT type detected via STUN
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NatType {
    /// No NAT (public IP)
    NoNat,
    /// Full cone NAT (easiest for P2P)
    FullCone,
    /// Restricted cone NAT
    RestrictedCone,
    /// Port restricted cone NAT
    PortRestrictedCone,
    /// Symmetric NAT (hardest for P2P)
    Symmetric,
    /// Could not determine
    Unknown,
}

impl NatType {
    /// Get P2P connection difficulty description
    pub fn p2p_difficulty(&self) -> &'static str {
        match self {
            NatType::NoNat => "Easy (no NAT)",
            NatType::FullCone => "Easy",
            NatType::RestrictedCone => "Moderate",
            NatType::PortRestrictedCone => "Moderate",
            NatType::Symmetric => "Difficult (may need relay)",
            NatType::Unknown => "Unknown",
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
    /// Detected NAT type
    pub nat_type: NatType,
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
    pub async fn run(signaling: &SignalingClient) -> NetworkDiagnosticsResult {
        let mut problems = Vec::new();

        // Check IP support
        let ip_support = Self::check_ip_support().await;

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

        // Check NAT type
        let nat_type = Self::detect_nat_type(&ip_support).await;

        // Add problem for symmetric NAT
        if nat_type == NatType::Symmetric {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Warning,
                category: "network".to_string(),
                code: ProblemCode::SymmetricNat,
            });
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
            connection_stability,
            jitter_ms,
            stability_metrics,
            signaling: signaling_result,
            problems,
        }
    }

    /// Check IPv4/IPv6 support
    async fn check_ip_support() -> IpSupport {
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
            if !addr.is_loopback() && !is_link_local_ipv6(&addr) && !is_unique_local_ipv6(&addr) {
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
                        if !addr.is_loopback()
                            && !is_link_local_ipv6(&addr)
                            && !is_unique_local_ipv6(&addr)
                            && !ipv6_addresses.contains(&addr.to_string()) =>
                    {
                        ipv6_addresses.push(addr.to_string());
                    }
                    _ => {}
                }
            }
        }

        // Try to get public IPv4 via STUN
        if !ipv4_addresses.is_empty() {
            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await {
                let client = StunClient::with_timeout(socket, 2000);
                if let Ok(result) = client.discover_public_address().await {
                    public_ipv4 = Some(result.mapped_address.ip().to_string());
                }
            }
        }

        // Try to get public IPv6 via STUN
        if !ipv6_addresses.is_empty() {
            if let Ok(socket) = UdpSocket::bind("[::]:0").await {
                let client = StunClient::with_timeout(socket, 2000);
                if let Ok(result) = client.discover_public_address().await {
                    if result.mapped_address.ip().is_ipv6() {
                        public_ipv6 = Some(result.mapped_address.ip().to_string());
                    }
                }
            }
        }

        IpSupport {
            ipv4_available: !ipv4_addresses.is_empty(),
            ipv6_available: !ipv6_addresses.is_empty(),
            ipv4_addresses,
            ipv6_addresses,
            public_ipv4,
            public_ipv6,
        }
    }

    /// Detect NAT type using STUN
    async fn detect_nat_type(ip_support: &IpSupport) -> NatType {
        // If we have a public IP that matches local IP, no NAT
        if let Some(ref public) = ip_support.public_ipv4 {
            if ip_support.ipv4_addresses.contains(public) {
                return NatType::NoNat;
            }
        }

        // Try multiple STUN servers to detect NAT type
        let mut mapped_addresses: Vec<SocketAddr> = Vec::new();

        for server in DEFAULT_STUN_SERVERS.iter().take(3) {
            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await {
                let client = StunClient::with_timeout(socket, 2000);
                if let Ok(result) = client.binding_request(server).await {
                    mapped_addresses.push(result.mapped_address);
                }
            }
        }

        if mapped_addresses.is_empty() {
            return NatType::Unknown;
        }

        // Check if all mapped addresses are the same
        let first_addr: &SocketAddr = &mapped_addresses[0];
        let all_same_ip = mapped_addresses
            .iter()
            .all(|a: &SocketAddr| a.ip() == first_addr.ip());
        let all_same_port = mapped_addresses
            .iter()
            .all(|a: &SocketAddr| a.port() == first_addr.port());

        if all_same_ip && all_same_port {
            // Same IP and port from different servers - likely Full Cone or Restricted
            NatType::FullCone
        } else if all_same_ip && !all_same_port {
            // Same IP but different ports - Port Restricted or Symmetric
            // Need more sophisticated testing to distinguish
            NatType::PortRestrictedCone
        } else {
            // Different IPs - Symmetric NAT
            NatType::Symmetric
        }
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

/// Check if an IPv6 address is link-local (fe80::/10)
fn is_link_local_ipv6(addr: &Ipv6Addr) -> bool {
    let segments = addr.segments();
    (segments[0] & 0xffc0) == 0xfe80
}

/// Check if an IPv6 address is unique local (fc00::/7)
fn is_unique_local_ipv6(addr: &Ipv6Addr) -> bool {
    let segments = addr.segments();
    (segments[0] & 0xfe00) == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_type_difficulty() {
        assert_eq!(NatType::NoNat.p2p_difficulty(), "Easy (no NAT)");
        assert_eq!(NatType::FullCone.p2p_difficulty(), "Easy");
        assert_eq!(
            NatType::Symmetric.p2p_difficulty(),
            "Difficult (may need relay)"
        );
    }

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

    #[test]
    fn test_ipv6_checks() {
        let link_local: Ipv6Addr = "fe80::1".parse().unwrap();
        assert!(is_link_local_ipv6(&link_local));

        let unique_local: Ipv6Addr = "fd00::1".parse().unwrap();
        assert!(is_unique_local_ipv6(&unique_local));

        let global: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert!(!is_link_local_ipv6(&global));
        assert!(!is_unique_local_ipv6(&global));
    }
}
