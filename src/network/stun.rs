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

/// Result of a STUN binding request
#[derive(Debug, Clone)]
pub struct StunResult {
    /// Public address as seen by the STUN server
    pub mapped_address: SocketAddr,
    /// STUN server that was used
    pub server: String,
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

    /// Send a binding request to a specific STUN server
    pub async fn binding_request(&self, server: &str) -> Result<StunResult, NetworkError> {
        // A socket can only send to its own address family, and the first
        // address DNS returns is often IPv6 even for an IPv4 socket.
        let local_is_ipv4 = self
            .socket
            .local_addr()
            .map_err(|e| NetworkError::StunFailed(format!("Local address unavailable: {}", e)))?
            .is_ipv4();
        let server_addr: SocketAddr = tokio::net::lookup_host(server)
            .await
            .map_err(|e| NetworkError::StunFailed(format!("DNS lookup failed: {}", e)))?
            .find(|addr| addr.is_ipv4() == local_is_ipv4)
            .ok_or_else(|| NetworkError::StunFailed("No address found".to_string()))?;

        debug!("Sending STUN binding request to {}", server_addr);

        // Build STUN binding request
        let transaction_id: [u8; 12] = rand::random();
        let request = build_binding_request(&transaction_id);

        // Send request
        self.socket
            .send_to(&request, server_addr)
            .await
            .map_err(|e| NetworkError::StunFailed(format!("Send failed: {}", e)))?;

        // Wait for the response. The socket is the audio socket, so anything
        // else that reaches it in the meantime (a peer's keep-alive) is skipped
        // rather than ending the query.
        let deadline = Instant::now() + Duration::from_millis(self.timeout_ms);
        let mut buf = [0u8; 576]; // Minimum STUN message size
        let mapped_address = loop {
            let (len, from) = tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf))
                .await
                .map_err(|_| NetworkError::StunFailed("Timeout".to_string()))?
                .map_err(|e| NetworkError::StunFailed(format!("Receive failed: {}", e)))?;

            match parse_binding_response(&buf[..len], &transaction_id) {
                Ok(addr) => break addr,
                Err(e) => debug!(
                    "Ignoring datagram from {} while waiting for STUN: {}",
                    from, e
                ),
            }
        };

        info!("STUN discovered public address: {}", mapped_address);

        Ok(StunResult {
            mapped_address,
            server: server.to_string(),
        })
    }
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
