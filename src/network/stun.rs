//! STUN client for NAT traversal
//!
//! Implements RFC 5389 STUN (Session Traversal Utilities for NAT)
//! to discover public IP address and port mapping.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::error::NetworkError;
use super::transport::{destination_for, IpFamilies};

/// STUN message types
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;

/// STUN attribute types
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// STUN magic cookie (RFC 5389)
const MAGIC_COOKIE: u32 = 0x2112A442;

/// Default STUN servers (IPv4)
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

/// Default STUN servers (IPv6)
/// Note: Currently uses same servers as IPv4, as they support both protocols
#[allow(dead_code)]
pub const DEFAULT_STUN_SERVERS_V6: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

/// How long the other IP family is waited for once one has answered
const OTHER_FAMILY_GRACE: Duration = Duration::from_millis(1000);

/// How long the address another server sees the audio socket at is waited for. It only tells
/// whether the NAT keeps one port for the socket, so it must not hold up entering a room.
const OBSERVE_TIMEOUT_MS: u64 = 1000;

/// Result of a STUN binding request
#[derive(Debug, Clone)]
pub struct StunResult {
    /// Public address as seen by the STUN server
    pub mapped_address: SocketAddr,
    /// STUN server that was used
    pub server: String,
    /// The address of the server that answered
    pub server_addr: SocketAddr,
}

/// STUN client for discovering public address
///
/// The address a NAT reports is the mapping for the socket that sent the
/// request, so the socket has to be the one the audio will use. Pass the audio
/// socket (an `Arc<UdpSocket>` shared with the transport) rather than a fresh
/// one: a fresh socket gets its own mapping, and NATs that renumber ports give
/// it a different port than the audio socket has.
pub struct StunClient {
    socket: Arc<UdpSocket>,
    timeout_ms: u64,
}

impl StunClient {
    /// Create a new STUN client that sends from the given socket
    pub fn new(socket: impl Into<Arc<UdpSocket>>) -> Self {
        Self {
            socket: socket.into(),
            timeout_ms: 3000,
        }
    }

    /// Create a new STUN client with custom timeout
    pub fn with_timeout(socket: impl Into<Arc<UdpSocket>>, timeout_ms: u64) -> Self {
        Self {
            socket: socket.into(),
            timeout_ms,
        }
    }

    /// Discover public address using default STUN servers
    pub async fn discover_public_address(&self) -> Result<StunResult, NetworkError> {
        self.discover_public_address_from(DEFAULT_STUN_SERVERS)
            .await
    }

    /// Discover public address using the first of `servers` that answers
    pub async fn discover_public_address_from(
        &self,
        servers: &[&str],
    ) -> Result<StunResult, NetworkError> {
        for server in servers {
            match self.binding_request(server).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    warn!("STUN request to {} failed: {}", server, e);
                    continue;
                }
            }
        }
        Err(NetworkError::StunFailed(
            "All STUN servers failed".to_string(),
        ))
    }

    /// Send a binding request to a specific STUN server, over IPv4 where the socket can
    /// (a socket that is IPv6-only asks over IPv6)
    pub async fn binding_request(&self, server: &str) -> Result<StunResult, NetworkError> {
        let families = IpFamilies::of_tokio(&self.socket);
        let server_addr = resolve(server, families.v4).await?;
        let mut answers = self
            .ask(&[(server_addr, rand::random())], None, self.timeout_ms)
            .await?;
        let (server_addr, mapped_address) = answers
            .pop()
            .ok_or_else(|| NetworkError::StunFailed("Timeout".to_string()))?;
        info!("STUN discovered public address: {}", mapped_address);
        Ok(StunResult {
            mapped_address,
            server: server.to_string(),
            server_addr,
        })
    }

    /// The public address of each IP family in `ask`, as the first of `servers` that answers
    /// for that family reports it: at most one IPv4 and one IPv6 address.
    ///
    /// A dual-stack audio socket has a NAT mapping for IPv4 and none for IPv6 (an IPv6 host
    /// is reached at its own address, though a firewall may only let through what it has
    /// sent to first), so the two are asked together, from the one socket, in the same
    /// round. An address that answers shortens the wait for the other to
    /// [`OTHER_FAMILY_GRACE`]: a host with an IPv6 address but no IPv6 route must not hold up
    /// entering a room. The families are never asked of a socket that cannot reach them.
    pub async fn discover_public_addresses_from(
        &self,
        servers: &[&str],
        ask: IpFamilies,
    ) -> Vec<StunResult> {
        let reach = IpFamilies::of_tokio(&self.socket);
        let mut want_v4 = ask.v4 && reach.v4;
        let mut want_v6 = ask.v6 && reach.v6;
        let mut found: Vec<StunResult> = Vec::new();

        for server in servers {
            if !want_v4 && !want_v6 {
                break;
            }
            let mut targets: Vec<(SocketAddr, [u8; 12])> = Vec::new();
            let resolved = match tokio::net::lookup_host(server).await {
                Ok(addrs) => addrs.collect::<Vec<_>>(),
                Err(e) => {
                    warn!(
                        "STUN request to {} failed: DNS lookup failed: {}",
                        server, e
                    );
                    continue;
                }
            };
            if want_v4 {
                targets.extend(
                    resolved
                        .iter()
                        .find(|a| a.is_ipv4())
                        .map(|a| (*a, rand::random())),
                );
            }
            if want_v6 {
                targets.extend(
                    resolved
                        .iter()
                        .find(|a| a.is_ipv6())
                        .map(|a| (*a, rand::random())),
                );
            }
            let asked_v4 = targets.iter().any(|(a, _)| a.is_ipv4());
            let asked_v6 = targets.iter().any(|(a, _)| a.is_ipv6());
            if targets.is_empty() {
                continue;
            }
            let answers = match self
                .ask(&targets, Some(OTHER_FAMILY_GRACE), self.timeout_ms)
                .await
            {
                Ok(answers) => answers,
                Err(e) => {
                    warn!("STUN request to {} failed: {}", server, e);
                    continue;
                }
            };
            for (target, mapped) in answers {
                if target.is_ipv4() {
                    want_v4 = false;
                } else {
                    want_v6 = false;
                }
                info!("STUN discovered public address: {}", mapped);
                found.push(StunResult {
                    mapped_address: mapped,
                    server: server.to_string(),
                    server_addr: target,
                });
            }
            // A round that found something has had its grace: a family that was asked and
            // did not answer is not waited for through the other servers. One this server
            // has no address for was not asked, and goes on to the next.
            let unasked = (want_v4 && !asked_v4) || (want_v6 && !asked_v6);
            if !found.is_empty() && !unasked {
                break;
            }
        }
        found
    }

    /// The address the audio socket is seen at by up to `wanted` servers other than the one
    /// that gave `first`, each at an address of its own (IPv4): the ports they report
    ///
    /// A NAT that keeps one port for the socket reports the same port to every server. One
    /// that gives each destination a port of its own reports a different one to each, and those
    /// are what a peer needs to guess the port it is to send to (ADR-115). Asked together, from
    /// the audio socket. Servers that do not answer in time are left out.
    pub async fn observe_from_other_servers(
        &self,
        servers: &[&str],
        first: &StunResult,
        wanted: usize,
    ) -> Vec<SocketAddr> {
        let mut seen_at = vec![first.server_addr.ip()];
        let mut targets: Vec<(SocketAddr, [u8; 12])> = Vec::new();
        for server in servers {
            if targets.len() == wanted {
                break;
            }
            match resolve(server, true).await {
                Ok(addr) if !seen_at.contains(&addr.ip()) => {
                    seen_at.push(addr.ip());
                    targets.push((addr, rand::random()));
                }
                Ok(_) => {}
                Err(e) => debug!("No second look through {}: {}", server, e),
            }
        }
        if targets.is_empty() {
            return Vec::new();
        }
        match self.ask(&targets, None, OBSERVE_TIMEOUT_MS).await {
            Ok(answers) => answers.into_iter().map(|(_, mapped)| mapped).collect(),
            Err(e) => {
                debug!("No second look at the public address: {}", e);
                Vec::new()
            }
        }
    }

    /// Sends one binding request to each of `targets` and collects the answers: the
    /// address each target reports, until all have answered or the wait is over (`timeout_ms`).
    /// With a `grace`, the wait ends that long after the first answer.
    ///
    /// The socket is the audio socket, so anything else that reaches it in the meantime
    /// (a peer's keep-alive) is skipped rather than ending the query.
    async fn ask(
        &self,
        targets: &[(SocketAddr, [u8; 12])],
        grace: Option<Duration>,
        timeout_ms: u64,
    ) -> Result<Vec<(SocketAddr, SocketAddr)>, NetworkError> {
        let local = self
            .socket
            .local_addr()
            .map_err(|e| NetworkError::StunFailed(format!("Local address unavailable: {}", e)))?;
        // A target the socket cannot send to (no route for its family) is left out; the others
        // are still asked.
        let mut sent: Vec<(SocketAddr, [u8; 12])> = Vec::new();
        let mut last_error = None;
        for &(target, transaction_id) in targets {
            debug!("Sending STUN binding request to {}", target);
            match self
                .socket
                .send_to(
                    &build_binding_request(&transaction_id),
                    destination_for(local, target),
                )
                .await
            {
                Ok(_) => sent.push((target, transaction_id)),
                Err(e) => last_error = Some(e),
            }
        }
        if sent.is_empty() {
            return Err(NetworkError::StunFailed(match last_error {
                Some(e) => format!("Send failed: {}", e),
                None => "No address found".to_string(),
            }));
        }
        let targets = &sent[..];

        let mut deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut buf = [0u8; 576]; // Minimum STUN message size
        let mut answers: Vec<(SocketAddr, SocketAddr)> = Vec::new();
        while answers.len() < targets.len() {
            let received =
                match tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf)).await {
                    Ok(received) => received
                        .map_err(|e| NetworkError::StunFailed(format!("Receive failed: {}", e)))?,
                    Err(_) if answers.is_empty() => {
                        return Err(NetworkError::StunFailed("Timeout".to_string()))
                    }
                    Err(_) => break,
                };
            let (len, from) = received;
            let reply = targets.iter().find_map(|(target, transaction_id)| {
                parse_binding_response(&buf[..len], transaction_id)
                    .ok()
                    .map(|mapped| (*target, mapped))
            });
            match reply {
                Some((target, mapped)) if !answers.iter().any(|(t, _)| *t == target) => {
                    answers.push((target, mapped));
                    if let Some(grace) = grace {
                        deadline = deadline.min(Instant::now() + grace);
                    }
                }
                _ => debug!("Ignoring datagram from {} while waiting for STUN", from),
            }
        }
        Ok(answers)
    }
}

/// The address of `server` in the IPv4 family if `v4`, in the IPv6 one if not
async fn resolve(server: &str, v4: bool) -> Result<SocketAddr, NetworkError> {
    tokio::net::lookup_host(server)
        .await
        .map_err(|e| NetworkError::StunFailed(format!("DNS lookup failed: {}", e)))?
        .find(|addr| addr.is_ipv4() == v4)
        .ok_or_else(|| NetworkError::StunFailed("No address found".to_string()))
}

/// Build a STUN binding request message
fn build_binding_request(transaction_id: &[u8; 12]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(20);

    // Message type: Binding Request
    msg.extend_from_slice(&BINDING_REQUEST.to_be_bytes());

    // Message length (0 for binding request with no attributes)
    msg.extend_from_slice(&0u16.to_be_bytes());

    // Magic cookie
    msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());

    // Transaction ID (12 bytes)
    msg.extend_from_slice(transaction_id);

    msg
}

/// Parse a STUN binding response
fn parse_binding_response(
    data: &[u8],
    expected_txn_id: &[u8; 12],
) -> Result<SocketAddr, NetworkError> {
    if data.len() < 20 {
        return Err(NetworkError::StunFailed("Response too short".to_string()));
    }

    // Check message type
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != BINDING_RESPONSE {
        return Err(NetworkError::StunFailed(format!(
            "Unexpected message type: 0x{:04x}",
            msg_type
        )));
    }

    // Check magic cookie
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if cookie != MAGIC_COOKIE {
        return Err(NetworkError::StunFailed("Invalid magic cookie".to_string()));
    }

    // Check transaction ID
    if &data[8..20] != expected_txn_id {
        return Err(NetworkError::StunFailed(
            "Transaction ID mismatch".to_string(),
        ));
    }

    // Parse message length
    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if data.len() < 20 + msg_len {
        return Err(NetworkError::StunFailed("Message truncated".to_string()));
    }

    // Parse attributes
    let mut offset = 20;
    while offset + 4 <= 20 + msg_len {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;

        if offset + 4 + attr_len > data.len() {
            break;
        }

        let attr_data = &data[offset + 4..offset + 4 + attr_len];

        match attr_type {
            XOR_MAPPED_ADDRESS => {
                return parse_xor_mapped_address(attr_data, expected_txn_id);
            }
            MAPPED_ADDRESS => {
                return parse_mapped_address(attr_data);
            }
            _ => {}
        }

        // Align to 4-byte boundary
        offset += 4 + ((attr_len + 3) & !3);
    }

    Err(NetworkError::StunFailed(
        "No mapped address in response".to_string(),
    ))
}

/// Parse XOR-MAPPED-ADDRESS attribute (RFC 5389)
fn parse_xor_mapped_address(
    data: &[u8],
    transaction_id: &[u8; 12],
) -> Result<SocketAddr, NetworkError> {
    if data.len() < 8 {
        return Err(NetworkError::StunFailed(
            "XOR-MAPPED-ADDRESS too short".to_string(),
        ));
    }

    let family = data[1];
    let xor_port = u16::from_be_bytes([data[2], data[3]]);
    let port = xor_port ^ ((MAGIC_COOKIE >> 16) as u16);

    match family {
        0x01 => {
            // IPv4
            let xor_addr = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
            let addr = xor_addr ^ MAGIC_COOKIE;
            let ip = std::net::Ipv4Addr::from(addr);
            Ok(SocketAddr::new(ip.into(), port))
        }
        0x02 => {
            // IPv6: XOR with magic cookie (4 bytes) + transaction ID (12 bytes)
            if data.len() < 20 {
                return Err(NetworkError::StunFailed(
                    "XOR-MAPPED-ADDRESS IPv6 too short".to_string(),
                ));
            }

            // Build XOR mask: magic cookie + transaction ID
            let mut xor_mask = [0u8; 16];
            xor_mask[0..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            xor_mask[4..16].copy_from_slice(transaction_id);

            // XOR the address
            let mut ip_bytes = [0u8; 16];
            for i in 0..16 {
                ip_bytes[i] = data[4 + i] ^ xor_mask[i];
            }

            let ip = std::net::Ipv6Addr::from(ip_bytes);
            Ok(SocketAddr::new(ip.into(), port))
        }
        _ => Err(NetworkError::StunFailed(format!(
            "Unknown address family: {}",
            family
        ))),
    }
}

/// Parse MAPPED-ADDRESS attribute (legacy, non-XOR)
fn parse_mapped_address(data: &[u8]) -> Result<SocketAddr, NetworkError> {
    if data.len() < 8 {
        return Err(NetworkError::StunFailed(
            "MAPPED-ADDRESS too short".to_string(),
        ));
    }

    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 => {
            // IPv4
            let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Ok(SocketAddr::new(ip.into(), port))
        }
        0x02 => {
            // IPv6
            if data.len() < 20 {
                return Err(NetworkError::StunFailed(
                    "MAPPED-ADDRESS IPv6 too short".to_string(),
                ));
            }
            let mut ip_bytes = [0u8; 16];
            ip_bytes.copy_from_slice(&data[4..20]);
            let ip = std::net::Ipv6Addr::from(ip_bytes);
            Ok(SocketAddr::new(ip.into(), port))
        }
        _ => Err(NetworkError::StunFailed(format!(
            "Unknown address family: {}",
            family
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_binding_request() {
        let txn_id = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let request = build_binding_request(&txn_id);

        assert_eq!(request.len(), 20);
        // Check message type
        assert_eq!(request[0], 0x00);
        assert_eq!(request[1], 0x01);
        // Check message length
        assert_eq!(request[2], 0x00);
        assert_eq!(request[3], 0x00);
        // Check magic cookie
        assert_eq!(request[4], 0x21);
        assert_eq!(request[5], 0x12);
        assert_eq!(request[6], 0xA4);
        assert_eq!(request[7], 0x42);
        // Check transaction ID
        assert_eq!(&request[8..20], &txn_id);
    }

    #[test]
    fn test_parse_xor_mapped_address_ipv4() {
        // XOR-MAPPED-ADDRESS for 192.168.1.100:5000
        // XOR port: 5000 (0x1388) ^ 0x2112 = 0x329A
        // XOR addr: 192.168.1.100 (0xC0A80164) ^ 0x2112A442 = 0xE1BAA526
        let txn_id = [0u8; 12]; // Not used for IPv4
        let data = [
            0x00, 0x01, // Reserved + Family (IPv4)
            0x32, 0x9A, // XOR'd port
            0xE1, 0xBA, 0xA5, 0x26, // XOR'd address
        ];

        let result = parse_xor_mapped_address(&data, &txn_id).unwrap();
        assert_eq!(result.port(), 5000);
        assert_eq!(result.ip().to_string(), "192.168.1.100");
    }

    #[test]
    fn test_parse_xor_mapped_address_ipv6() {
        // XOR-MAPPED-ADDRESS for 2001:db8::1:5000
        // IPv6 address: 2001:0db8:0000:0000:0000:0000:0000:0001
        // XOR mask: magic cookie (0x2112A442) + transaction ID
        let txn_id: [u8; 12] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        ];

        // Original IPv6: 2001:0db8:0000:0000:0000:0000:0000:0001
        let original_ip: [u8; 16] = [
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ];

        // XOR mask: magic cookie + transaction ID
        let mut xor_mask = [0u8; 16];
        xor_mask[0..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        xor_mask[4..16].copy_from_slice(&txn_id);

        // XOR the address
        let mut xored_ip = [0u8; 16];
        for i in 0..16 {
            xored_ip[i] = original_ip[i] ^ xor_mask[i];
        }

        // XOR port: 5000 ^ 0x2112 = 0x329A
        let mut data = vec![
            0x00, 0x02, // Reserved + Family (IPv6)
            0x32, 0x9A, // XOR'd port
        ];
        data.extend_from_slice(&xored_ip);

        let result = parse_xor_mapped_address(&data, &txn_id).unwrap();
        assert_eq!(result.port(), 5000);
        assert_eq!(result.ip().to_string(), "2001:db8::1");
    }

    #[test]
    fn test_parse_mapped_address_ipv6() {
        // MAPPED-ADDRESS for 2001:db8::1:8080
        let data = [
            0x00, 0x02, // Reserved + Family (IPv6)
            0x1F, 0x90, // Port (8080)
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ];

        let result = parse_mapped_address(&data).unwrap();
        assert_eq!(result.port(), 8080);
        assert_eq!(result.ip().to_string(), "2001:db8::1");
    }

    /// A STUN server on this machine that answers one binding request by
    /// reporting `reported` as the sender's address. Returns its `host:port`.
    async fn answering_server(reported: SocketAddr) -> String {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 576];
            let (len, from) = socket.recv_from(&mut buf).await.unwrap();
            assert!(len >= 20);

            let SocketAddr::V4(reported) = reported else {
                unreachable!("the test reports an IPv4 address");
            };
            let mut attr = vec![0x00, 0x01];
            attr.extend_from_slice(&(reported.port() ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
            attr.extend_from_slice(&(u32::from(*reported.ip()) ^ MAGIC_COOKIE).to_be_bytes());

            let mut response = Vec::new();
            response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
            response.extend_from_slice(&((4 + attr.len()) as u16).to_be_bytes());
            response.extend_from_slice(&buf[4..20]); // magic cookie + transaction ID
            response.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
            response.extend_from_slice(&(attr.len() as u16).to_be_bytes());
            response.extend_from_slice(&attr);
            socket.send_to(&response, from).await.unwrap();
        });
        addr.to_string()
    }

    #[tokio::test]
    async fn test_discovery_falls_through_unresolvable_and_silent_servers_to_one_that_answers() {
        let reported: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        let answering = answering_server(reported).await;
        // Accepts the datagram and never replies, like a server that dropped us.
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let silent_addr = silent.local_addr().unwrap().to_string();

        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client = StunClient::with_timeout(socket, 300);
        let result = client
            .discover_public_address_from(&[
                "stun-down.invalid:3478",
                silent_addr.as_str(),
                answering.as_str(),
            ])
            .await
            .unwrap();

        assert_eq!(result.mapped_address, reported);
        assert_eq!(result.server, answering);
    }

    /// A STUN server at `bind` that answers one binding request, reporting `reported` (of
    /// either family) as the sender's address. Returns its `host:port`.
    async fn server_reporting(bind: &str, reported: SocketAddr) -> String {
        let socket = UdpSocket::bind(bind).await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 576];
            let (len, from) = socket.recv_from(&mut buf).await.unwrap();
            assert!(len >= 20);

            let mut mask = [0u8; 16];
            mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            mask[4..].copy_from_slice(&buf[8..20]); // transaction ID
            let mut attr = vec![0x00, if reported.is_ipv4() { 0x01 } else { 0x02 }];
            attr.extend_from_slice(&(reported.port() ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
            match reported.ip() {
                std::net::IpAddr::V4(ip) => {
                    attr.extend(ip.octets().iter().zip(mask).map(|(byte, mask)| byte ^ mask))
                }
                std::net::IpAddr::V6(ip) => {
                    attr.extend(ip.octets().iter().zip(mask).map(|(byte, mask)| byte ^ mask))
                }
            }

            let mut response = Vec::new();
            response.extend_from_slice(&BINDING_RESPONSE.to_be_bytes());
            response.extend_from_slice(&((4 + attr.len()) as u16).to_be_bytes());
            response.extend_from_slice(&buf[4..20]); // magic cookie + transaction ID
            response.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
            response.extend_from_slice(&(attr.len() as u16).to_be_bytes());
            response.extend_from_slice(&attr);
            socket.send_to(&response, from).await.unwrap();
        });
        addr.to_string()
    }

    /// The audio socket of an app: dual-stack where the system has IPv6
    fn audio_socket() -> UdpSocket {
        UdpSocket::from_std(super::super::bind_audio_std(0).unwrap()).unwrap()
    }

    /// Verifies: REQ-CON-145
    #[tokio::test]
    async fn when_the_socket_is_dual_stack_both_families_are_asked_and_each_answer_is_kept() {
        let socket = audio_socket();
        if !IpFamilies::of_tokio(&socket).v6 {
            return; // no IPv6 here: the next test covers the IPv4-only socket
        }
        let reported_v4: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        let reported_v6: SocketAddr = "[2001:db8::7]:41000".parse().unwrap();
        let v4_server = server_reporting("127.0.0.1:0", reported_v4).await;
        let v6_server = server_reporting("[::1]:0", reported_v6).await;

        let client = StunClient::with_timeout(socket, 1000);
        let found = client
            .discover_public_addresses_from(
                &[v4_server.as_str(), v6_server.as_str()],
                IpFamilies { v4: true, v6: true },
            )
            .await;

        let mut addresses: Vec<SocketAddr> = found.iter().map(|r| r.mapped_address).collect();
        addresses.sort();
        assert_eq!(addresses, vec![reported_v4, reported_v6]);
    }

    /// Verifies: REQ-CON-145
    #[tokio::test]
    async fn when_one_family_answers_and_the_other_is_silent_the_answer_waits_only_the_grace() {
        let socket = audio_socket();
        if !IpFamilies::of_tokio(&socket).v6 {
            return; // no IPv6 here
        }
        let reported: SocketAddr = "203.0.113.7:40000".parse().unwrap();
        let v4_server: SocketAddr = server_reporting("127.0.0.1:0", reported)
            .await
            .parse()
            .unwrap();
        // Takes the request and never answers, like a host with an IPv6 address and no route
        let silent = UdpSocket::bind("[::1]:0").await.unwrap();
        let silent_addr = silent.local_addr().unwrap();

        let client = StunClient::with_timeout(socket, 5000);
        let started = Instant::now();
        let answers = client
            .ask(
                &[(v4_server, [1; 12]), (silent_addr, [2; 12])],
                Some(OTHER_FAMILY_GRACE),
                5000,
            )
            .await
            .unwrap();

        assert_eq!(answers, vec![(v4_server, reported)]);
        assert!(
            started.elapsed() < OTHER_FAMILY_GRACE + Duration::from_millis(1500),
            "the IPv4 answer waited {:?} for the silent IPv6 one",
            started.elapsed()
        );
    }

    /// Verifies: REQ-CON-145
    #[tokio::test]
    async fn when_the_socket_is_ipv4_only_the_ipv6_family_is_not_asked() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let reported_v6: SocketAddr = "[2001:db8::7]:41000".parse().unwrap();
        let v6_server = server_reporting("[::1]:0", reported_v6).await;

        let client = StunClient::with_timeout(socket, 300);
        let found = client
            .discover_public_addresses_from(
                &[v6_server.as_str()],
                IpFamilies { v4: true, v6: true },
            )
            .await;

        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn test_discovery_fails_when_no_server_answers() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client = StunClient::with_timeout(socket, 300);
        let result = client
            .discover_public_address_from(&["stun-down.invalid:3478"])
            .await;

        assert!(matches!(result, Err(NetworkError::StunFailed(_))));
    }
}
