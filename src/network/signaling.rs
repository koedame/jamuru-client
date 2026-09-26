//! Signaling client and the messages it exchanges with the signaling server
//!
//! Handles room creation, peer discovery, and connection coordination.

use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::clock::{clock_skew_of_refusal, now_unix_secs};
use super::device_identity::DeviceIdentity;
use super::discovery::discover_signaling_url;
use super::error::{NetworkError, SignalingFailure};

/// The four `X-Device-*` handshake headers proving `identity` to a server
/// (ADR-024), signed for the current time.
pub(super) fn signed_device_headers(identity: &DeviceIdentity) -> [(&'static str, String); 4] {
    let timestamp = now_unix_secs();
    [
        (DEVICE_ID_HEADER, identity.device_id().to_string()),
        (DEVICE_PUBKEY_HEADER, identity.public_key_b64()),
        (DEVICE_SIGNATURE_HEADER, identity.sign_timestamp(timestamp)),
        (DEVICE_TIMESTAMP_HEADER, timestamp.to_string()),
    ]
}

/// Maximum peers per room
pub const MAX_PEERS_PER_ROOM: usize = 10;

/// The feature an app announces when it creates or joins a room to say it
/// takes [`SignalingMessage::PeerMessage`]s (ADR-043). The server relays peer
/// messages only to apps that announced it, so an older app never receives
/// one, and lists each participant's features in its [`PeerInfo`].
pub const PEER_MESSAGE_FEATURE: &str = "peer_message";

/// WebSocket handshake headers carrying a client's device identity
/// (ADR-024). [`SignalingClient::connect`] always sends all four: the server
/// refuses a connection without them.
pub const DEVICE_ID_HEADER: &str = "x-device-id";
pub const DEVICE_PUBKEY_HEADER: &str = "x-device-pubkey";
pub const DEVICE_SIGNATURE_HEADER: &str = "x-device-signature";
pub const DEVICE_TIMESTAMP_HEADER: &str = "x-device-timestamp";

/// Address candidate type for ICE-like connection establishment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateType {
    /// Local address (highest priority for same network)
    Host,
    /// Server reflexive address (public IP via STUN)
    ServerReflexive,
}

/// A single address candidate for connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressCandidate {
    /// The socket address
    pub address: SocketAddr,
    /// Type of candidate
    pub candidate_type: CandidateType,
    /// Priority (higher = better, RFC 5245 style)
    pub priority: u32,
}

impl AddressCandidate {
    /// Create a new host candidate
    pub fn host(address: SocketAddr) -> Self {
        // Host candidates have high priority
        // IPv6 gets slightly higher priority than IPv4 (Happy Eyeballs)
        let type_pref: u32 = 126; // Host type preference
        let local_pref: u32 = if address.is_ipv6() { 65535 } else { 65534 };
        let priority = (type_pref << 24) | (local_pref << 8) | 255;

        Self {
            address,
            candidate_type: CandidateType::Host,
            priority,
        }
    }

    /// Create a new server reflexive candidate (from STUN)
    pub fn server_reflexive(address: SocketAddr) -> Self {
        // Server reflexive has lower priority than host
        let type_pref: u32 = 100;
        let local_pref: u32 = if address.is_ipv6() { 65535 } else { 65534 };
        let priority = (type_pref << 24) | (local_pref << 8) | 255;

        Self {
            address,
            candidate_type: CandidateType::ServerReflexive,
            priority,
        }
    }
}

/// Peer information with multiple address candidates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: Uuid,
    pub name: String,
    /// All address candidates for this peer (sorted by priority)
    #[serde(default)]
    pub candidates: Vec<AddressCandidate>,
    /// Legacy: single public address (for backward compatibility)
    #[serde(default)]
    pub public_addr: Option<SocketAddr>,
    /// Legacy: single local address (for backward compatibility)
    #[serde(default)]
    pub local_addr: Option<SocketAddr>,
    /// Unix timestamp (seconds) when this peer joined the room.
    /// Defaults to 0 for older clients/fixtures that predate this field.
    #[serde(default)]
    pub joined_at: u64,
    /// What the peer's app announced it can do beyond the base protocol
    /// (such as [`PEER_MESSAGE_FEATURE`]). Empty from a server or an app that
    /// predates features.
    #[serde(default)]
    pub features: Vec<String>,
}

impl PeerInfo {
    /// Whether the peer's app takes peer messages.
    pub fn takes_peer_messages(&self) -> bool {
        self.features.iter().any(|f| f == PEER_MESSAGE_FEATURE)
    }

    /// Get all candidate addresses sorted by priority (highest first)
    pub fn get_sorted_candidates(&self) -> Vec<SocketAddr> {
        let mut candidates = self.candidates.clone();
        candidates.sort_by_key(|c| std::cmp::Reverse(c.priority));

        let mut addrs: Vec<SocketAddr> = candidates.into_iter().map(|c| c.address).collect();

        // Include legacy addresses if not already present
        if let Some(addr) = self.public_addr {
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
        if let Some(addr) = self.local_addr {
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }

        // The app publishes its bind address (`0.0.0.0:port`) as `local_addr`.
        // That names no host: macOS refuses the send (EHOSTUNREACH) and Linux
        // delivers it to ourselves.
        addrs.retain(|addr| !addr.ip().is_unspecified());

        addrs
    }
}

/// Room information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInfo {
    pub id: String,
    pub name: String,
    pub peer_count: usize,
    pub max_peers: usize,
    pub has_password: bool,
    /// 6-character invite code for easy room sharing
    pub invite_code: String,
    /// True for the room the server offers for trying a connection. The app
    /// shows a shortcut into it only when the server lists one, so which room
    /// that is - and whether there is one at all - is the server's to decide.
    ///
    /// Defaulted so a list from a server that predates this field parses, as
    /// a list with no such room.
    #[serde(default)]
    pub test_room: bool,
}

/// Signaling message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SignalingMessage {
    // Client -> Server
    CreateRoom {
        room_name: String,
        password: Option<String>,
        peer_name: String,
        /// What this app can do beyond the base protocol, told to the others
        /// in its [`PeerInfo`]. Left out when empty, as an older app does.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    },
    JoinRoom {
        room_id: String,
        password: Option<String>,
        peer_name: String,
        /// As [`SignalingMessage::CreateRoom`]'s.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    },
    LeaveRoom,
    /// Update peer connection information with multiple candidates
    UpdatePeerInfo {
        /// All address candidates (preferred)
        #[serde(default)]
        candidates: Vec<AddressCandidate>,
        /// Legacy: single public address
        #[serde(default)]
        public_addr: Option<SocketAddr>,
        /// Legacy: single local address
        #[serde(default)]
        local_addr: Option<SocketAddr>,
    },
    ListRooms,

    // Server -> Client
    RoomCreated {
        room_id: String,
        peer_id: Uuid,
        /// 6-character invite code for easy room sharing
        invite_code: String,
    },
    RoomJoined {
        room_id: String,
        peer_id: Uuid,
        /// The room's invite code, so every participant can share the room -
        /// not just whoever created it. Without it a client has nothing to
        /// show but the room's UUID, which is not a code anyone can join with.
        ///
        /// Defaulted so a client still parses the reply from a server that
        /// predates this field; it then has no code to show, rather than
        /// failing the join outright.
        #[serde(default)]
        invite_code: String,
        peers: Vec<PeerInfo>,
    },
    PeerJoined {
        peer: PeerInfo,
    },
    PeerLeft {
        peer_id: Uuid,
    },
    PeerUpdated {
        peer: PeerInfo,
    },
    RoomList {
        rooms: Vec<RoomInfo>,
    },
    Error {
        message: String,
    },
    /// Sent to every peer in a room the server has closed. Each client should
    /// treat this as an immediate disconnect.
    RoomClosed {
        reason: String,
    },
    /// Broadcast room-wide when the server removes one peer from the room;
    /// only the client whose `peer_id` matches should disconnect, other
    /// clients in the room should ignore it.
    Kicked {
        peer_id: Uuid,
        reason: String,
    },

    // Chat messages
    /// Send a chat message to the room
    ChatMessage {
        sender_id: String,
        sender_name: String,
        content: String,
        timestamp: u64,
    },

    /// A message from one app to another in the same room, relayed by the
    /// server (ADR-043). Sent with `to` naming one participant, or without it
    /// for everyone in the room who takes peer messages (the sender too, when
    /// it announced that it takes them). It arrives with `from` and
    /// `from_name` set by the server to the sending participant, whatever the
    /// sender wrote there; a received one without `from` did not come through
    /// a server and is to be dropped. The server does not read `body` - what
    /// it means is between the apps - but it has to be a JSON object.
    ///
    /// Nothing comes back when the server does not deliver it (the addressee
    /// left, is in another room, or does not take peer messages): an exchange
    /// built on these messages cannot wait for an answer forever.
    PeerMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_name: Option<String>,
        body: serde_json::Map<String, serde_json::Value>,
    },
}

/// Rustls 0.23 requires exactly one process-wide default `CryptoProvider`
/// and refuses to auto-select one when more than one crypto backend crate
/// is linked in. Rather than depend on which backends happen to be linked
/// into a given binary, every TLS entry point (such as
/// [`SignalingClient::connect`]) installs the provider explicitly before
/// first use. `Once`-guarded so calling it from multiple entry points in the
/// same process is harmless.
pub fn ensure_crypto_provider_installed() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Characters used for invite code generation.
/// Excludes visually confusing characters: 0, O, I, 1, L
const INVITE_CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// Length of invite codes
const INVITE_CODE_LENGTH: usize = 6;

/// Generate a 6-character invite code using readable characters.
/// Uses characters A-H, J-N, P-Z, 2-9 (excludes 0, O, I, 1, L for readability).
pub fn generate_invite_code() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..INVITE_CODE_LENGTH)
        .map(|_| {
            let idx = rng.random_range(0..INVITE_CODE_CHARS.len());
            INVITE_CODE_CHARS[idx] as char
        })
        .collect()
}

/// Check if a string matches the invite code format (6 uppercase alphanumeric characters).
pub fn is_invite_code_format(s: &str) -> bool {
    s.len() == INVITE_CODE_LENGTH && s.chars().all(|c| INVITE_CODE_CHARS.contains(&(c as u8)))
}

/// URL scheme used by invite links (`jamjam://join/ABC123`)
pub const INVITE_URL_SCHEME: &str = "jamjam";

/// Path segment that identifies a join link
const INVITE_URL_PATH: &str = "join";

/// Build the invite URL for a room code (REQ-CON-101)
///
/// ```
/// use jamjam::network::invite_url;
/// assert_eq!(invite_url("ABC234"), "jamjam://join/ABC234");
/// ```
pub fn invite_url(invite_code: &str) -> String {
    format!(
        "{}://{}/{}",
        INVITE_URL_SCHEME, INVITE_URL_PATH, invite_code
    )
}

/// Extract the invite code from an invite URL (REQ-CON-103)
///
/// Returns `None` unless the URL uses the `jamjam` scheme, names the `join`
/// path, and carries a code that passes [`is_invite_code_format`]. Rejecting a
/// malformed code here means the join attempt fails locally with a clear cause
/// rather than as a "room not found" from the server.
///
/// The code is upper-cased first, so a link that has been lower-cased in transit
/// still works. Nothing else about the input is normalised.
///
/// ```
/// use jamjam::network::parse_invite_url;
/// assert_eq!(parse_invite_url("jamjam://join/ABC234"), Some("ABC234".to_string()));
/// assert_eq!(parse_invite_url("https://example.com/join/ABC234"), None);
/// ```
pub fn parse_invite_url(url: &str) -> Option<String> {
    let prefix = format!("{}://{}/", INVITE_URL_SCHEME, INVITE_URL_PATH);
    let rest = url.trim().strip_prefix(&prefix)?;

    // Tolerate a trailing slash or query string, but nothing further down a path:
    // `jamjam://join/ABC234/extra` is not a code this function should guess at.
    let code = rest
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if code.contains('/') {
        return None;
    }

    let code = code.to_ascii_uppercase();
    if is_invite_code_format(&code) {
        Some(code)
    } else {
        None
    }
}

/// Signaling client for connecting to a signaling server
///
/// Knows the jamjam server, not the signaling server: [`Self::connect`] asks
/// the server where its signaling is (`GET /api/v1/signaling`) and connects
/// there, proving the device identity (ADR-024). There is no anonymous
/// connection.
pub struct SignalingClient {
    server_url: String,
    device_identity: Arc<DeviceIdentity>,
}

impl SignalingClient {
    /// A client for the jamjam server at `server_url` (`http://` or
    /// `https://`), connecting as `device_identity`.
    pub fn new(server_url: &str, device_identity: Arc<DeviceIdentity>) -> Self {
        ensure_crypto_provider_installed();
        Self {
            server_url: server_url.to_string(),
            device_identity,
        }
    }

    /// The jamjam server this client asks for the signaling server.
    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// Asks the server where its signaling is, then connects there with the
    /// four `X-Device-*` headers on the handshake.
    pub async fn connect(&self) -> Result<SignalingConnection, NetworkError> {
        let signaling_url = discover_signaling_url(&self.server_url).await?;
        let mut request = signaling_url
            .as_str()
            .into_client_request()
            .map_err(|e| NetworkError::SignalingError(format!("Invalid signaling URL: {}", e)))?;
        for (name, value) in signed_device_headers(&self.device_identity) {
            let header_value = value.parse().map_err(|e| {
                NetworkError::SignalingError(format!("Invalid {} header: {}", name, e))
            })?;
            request.headers_mut().insert(name, header_value);
        }
        let (ws_stream, _) = connect_async(request).await.map_err(|e| {
            use tokio_tungstenite::tungstenite::Error as WsError;
            if let WsError::Http(response) = &e {
                let date = response.headers().get("date").and_then(|v| v.to_str().ok());
                if let Some(skew) = clock_skew_of_refusal(response.status().as_u16(), date) {
                    return skew;
                }
            }
            let failure = match &e {
                WsError::Http(response) => SignalingFailure::of_status(response.status().as_u16()),
                WsError::Tls(_) => SignalingFailure::Tls,
                WsError::Io(io) => SignalingFailure::of_error(io),
                _ => SignalingFailure::Other,
            };
            NetworkError::SignalingUnreachable {
                failure,
                message: format!("Connect failed: {}", e),
            }
        })?;

        debug!("Connected to signaling server: {}", signaling_url);

        Ok(SignalingConnection { ws_stream })
    }
}

/// An active connection to the signaling server
pub struct SignalingConnection {
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl SignalingConnection {
    /// Send a message to the server
    pub async fn send(&mut self, msg: SignalingMessage) -> Result<(), NetworkError> {
        let json = serde_json::to_string(&msg)
            .map_err(|e| NetworkError::SignalingError(format!("Serialize failed: {}", e)))?;

        self.ws_stream
            .send(Message::Text(json.into()))
            .await
            .map_err(|e| NetworkError::SignalingError(format!("Send failed: {}", e)))?;

        Ok(())
    }

    /// Send a WebSocket ping frame. `recv()` only puts bytes on the wire when
    /// there is a message to relay, so a client that is merely listening
    /// (connected to the server but not yet in a room, or in a quiet room)
    /// can go silent long enough to hit an edge/proxy idle timeout. The host
    /// calls this on a timer to keep the connection alive regardless of
    /// application traffic.
    pub async fn send_ping(&mut self) -> Result<(), NetworkError> {
        self.ws_stream
            .send(Message::Ping(Vec::new().into()))
            .await
            .map_err(|e| NetworkError::SignalingError(format!("Ping failed: {}", e)))?;
        Ok(())
    }

    /// Receive a message from the server
    ///
    /// A message whose `type` this client does not know is skipped: the
    /// server may have gained a message type after this client was built, and
    /// failing here would make every such message look like a lost connection
    /// to an app that is already installed. A known type that does not parse
    /// is still an error - that is a real incompatibility, not a newer message.
    pub async fn recv(&mut self) -> Result<SignalingMessage, NetworkError> {
        loop {
            match self.ws_stream.next().await {
                Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                    Ok(msg) => return Ok(msg),
                    Err(e) if is_of_unknown_type(&text) => {
                        debug!(
                            "Skipping a signaling message this client does not know: {}",
                            e
                        );
                        continue;
                    }
                    Err(e) => {
                        return Err(NetworkError::SignalingError(format!(
                            "Deserialize failed: {}",
                            e
                        )));
                    }
                },
                Some(Ok(Message::Close(_))) | None => {
                    return Err(NetworkError::ConnectionClosed);
                }
                Some(Err(e)) => {
                    return Err(NetworkError::SignalingError(format!(
                        "Receive failed: {}",
                        e
                    )));
                }
                _ => continue,
            }
        }
    }

    /// Close the connection
    pub async fn close(mut self) -> Result<(), NetworkError> {
        self.ws_stream
            .close(None)
            .await
            .map_err(|e| NetworkError::SignalingError(format!("Close failed: {}", e)))?;
        Ok(())
    }
}

/// Whether `text` is a message whose `type` is not one of [`SignalingMessage`]'s.
///
/// Decided from the tag alone: the tag is parsed on its own, with no data
/// beside it, so serde's "unknown variant" can only be about the tag. Parsing
/// the whole message would give the same wording for an unknown value deep
/// inside a known message (a candidate type the protocol gained later), which
/// is an incompatibility to report, not a newer message to skip.
fn is_of_unknown_type(text: &str) -> bool {
    let Some(tag) = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
    else {
        return false;
    };
    match serde_json::from_value::<SignalingMessage>(serde_json::json!({ "type": tag })) {
        Ok(_) => false,
        Err(e) => e.to_string().starts_with("unknown variant"),
    }
}

/// Gather all address candidates for the local peer
///
/// This function collects:
/// 1. Local host addresses of the interfaces that `audio_socket`'s address
///    family can use, each with the socket's port
/// 2. The server reflexive address via STUN (public IP/port mapping)
///
/// STUN is asked through `audio_socket` itself. The mapping a NAT creates is
/// per source socket, and NATs that renumber ports give a different socket a
/// different port, so a query from any other socket would advertise a port
/// nothing is mapped to. The reflexive candidate is exactly what the STUN
/// server saw, port included.
///
/// The socket must not be reading for anything else while this runs: the
/// reply arrives on it. Call it before the connection starts receiving.
///
/// Candidates are returned sorted by priority (highest first).
pub async fn gather_candidates(audio_socket: &Arc<tokio::net::UdpSocket>) -> Vec<AddressCandidate> {
    gather_candidates_using(audio_socket, super::stun::DEFAULT_STUN_SERVERS).await
}

/// [`gather_candidates`] with an explicit STUN server list.
pub async fn gather_candidates_using(
    audio_socket: &Arc<tokio::net::UdpSocket>,
    stun_servers: &[&str],
) -> Vec<AddressCandidate> {
    let mut candidates = Vec::new();

    let local_addr = match audio_socket.local_addr() {
        Ok(addr) => addr,
        Err(e) => {
            warn!("Audio socket has no local address: {}", e);
            return candidates;
        }
    };

    candidates.extend(gather_host_candidates(local_addr.port()));
    // Only the socket's own family can be used to reach it.
    candidates.retain(|c| c.address.is_ipv4() == local_addr.is_ipv4());

    let stun = super::stun::StunClient::new(audio_socket.clone());
    match stun.discover_public_address_from(stun_servers).await {
        Ok(result) => {
            let addr = result.mapped_address;
            candidates.push(AddressCandidate::server_reflexive(addr));
            info!("Added server reflexive candidate: {}", addr);
        }
        Err(e) => warn!("No server reflexive candidate: {}", e),
    }

    // Sort by priority (highest first)
    candidates.sort_by_key(|c| std::cmp::Reverse(c.priority));

    // Remove duplicates (same address)
    candidates.dedup_by(|a, b| a.address == b.address);

    info!("Gathered {} address candidates", candidates.len());
    candidates
}

/// Host candidates: every non-loopback interface address, at `local_port`.
///
/// Needs no STUN, so it is all that can be offered when the audio socket is
/// not available to ask through.
pub fn gather_host_candidates(local_port: u16) -> Vec<AddressCandidate> {
    let mut candidates = Vec::new();
    if let Ok(addrs) = local_ip_address::list_afinet_netifas() {
        for (_, ip) in addrs {
            // Skip loopback addresses
            if ip.is_loopback() {
                continue;
            }

            let addr = SocketAddr::new(ip, local_port);
            candidates.push(AddressCandidate::host(addr));
            debug!("Added host candidate: {}", addr);
        }
    }
    candidates
}

/// Get sorted addresses from candidates for connection attempts
pub fn candidates_to_addrs(candidates: &[AddressCandidate]) -> Vec<SocketAddr> {
    candidates.iter().map(|c| c.address).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server on this machine whose clock reads `now - behind_secs`: it
    /// answers the question of where its signaling is, and refuses the
    /// WebSocket handshake with a 401 dated by that clock. Returns its URL.
    async fn refusing_server(behind_secs: i64) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_lowercase();
                let date = chrono::DateTime::from_timestamp(now_unix_secs() - behind_secs, 0)
                    .unwrap()
                    .format("%a, %d %b %Y %H:%M:%S GMT");
                let response = if request.contains("upgrade: websocket") {
                    format!(
                        "HTTP/1.1 401 Unauthorized\r\nDate: {date}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    let body = format!(r#"{{"url":"ws://{addr}/v1/signaling"}}"#);
                    format!(
                        "HTTP/1.1 200 OK\r\nDate: {date}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        format!("{}://{}", "http", addr)
    }

    async fn connect_error(server: &str) -> NetworkError {
        SignalingClient::new(server, Arc::new(DeviceIdentity::generate()))
            .connect()
            .await
            .err()
            .expect("the server refuses the connection")
    }

    /// Verifies: REQ-IDT-009
    #[tokio::test]
    async fn when_the_clock_is_minutes_off_and_the_server_refuses_the_identity_the_error_is_the_clock_with_its_size(
    ) {
        let server = refusing_server(375).await;

        let error = connect_error(&server).await;

        assert!(
            matches!(error, NetworkError::ClockSkew { offset_secs } if (374..=376).contains(&offset_secs)),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("ahead of the server's by 37"),
            "{error}"
        );
    }

    /// Verifies: REQ-IDT-009
    #[tokio::test]
    async fn when_the_clock_is_behind_and_the_server_refuses_the_identity_the_error_says_behind() {
        let server = refusing_server(-900).await;

        let error = connect_error(&server).await;

        assert!(
            matches!(error, NetworkError::ClockSkew { offset_secs } if (-901..=-899).contains(&offset_secs)),
            "{error:?}"
        );
    }

    /// Verifies: REQ-IDT-009
    #[tokio::test]
    async fn when_the_clock_is_right_and_the_server_refuses_the_identity_it_is_still_a_4xx() {
        let server = refusing_server(0).await;

        let error = connect_error(&server).await;

        assert!(
            matches!(
                error,
                NetworkError::SignalingUnreachable {
                    failure: SignalingFailure::Http4xx,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// Verifies: REQ-IDT-009
    #[tokio::test]
    async fn when_asked_the_server_says_how_far_the_clock_is_from_its_own() {
        let server = refusing_server(375).await;

        let offset = super::super::discovery::server_clock_offset_secs(&server)
            .await
            .expect("the server dates its answer");

        assert!((374..=376).contains(&offset), "{offset}");
    }

    #[test]
    fn a_room_list_from_a_server_that_marks_no_test_room_parses_with_none_marked() {
        let json = r#"{"type":"RoomList","data":{"rooms":[{"id":"r1","name":"Jam","peer_count":1,"max_peers":10,"has_password":false,"invite_code":"ABC234"}]}}"#;

        let SignalingMessage::RoomList { rooms } = serde_json::from_str(json).unwrap() else {
            panic!("not a RoomList");
        };
        assert!(!rooms[0].test_room);
    }

    #[test]
    fn a_room_the_server_marks_as_its_test_room_parses_as_one() {
        let json = r#"{"type":"RoomList","data":{"rooms":[{"id":"r1","name":"Test Room","peer_count":0,"max_peers":10,"has_password":false,"invite_code":"ABC234","test_room":true}]}}"#;

        let SignalingMessage::RoomList { rooms } = serde_json::from_str(json).unwrap() else {
            panic!("not a RoomList");
        };
        assert!(rooms[0].test_room);
        assert_eq!(rooms[0].invite_code, "ABC234");
    }

    /// Verifies: REQ-CON-030
    #[test]
    fn a_message_of_a_type_the_protocol_gained_later_is_classified_as_unknown() {
        assert!(is_of_unknown_type(
            r#"{"type":"SomethingNewer","data":{"anything":[1,2,{"nested":true}]}}"#
        ));
        assert!(is_of_unknown_type(r#"{"data":{},"type":"SomethingNewer"}"#));
    }

    /// A known type that does not parse is an incompatibility to report, not
    /// a newer message to skip.
    ///
    /// Verifies: REQ-CON-030
    #[test]
    fn a_known_message_type_with_a_malformed_field_is_not_classified_as_unknown() {
        let malformed = r#"{"type":"PeerLeft","data":{"peer_id":"not-a-uuid"}}"#;
        assert!(serde_json::from_str::<SignalingMessage>(malformed).is_err());
        assert!(!is_of_unknown_type(malformed));
    }

    /// An unknown value inside a known message reads, to serde, exactly like an
    /// unknown message type. It must still be reported: skipping it would drop
    /// a peer's arrival without a trace.
    ///
    /// Verifies: REQ-CON-030
    #[test]
    fn a_known_message_carrying_an_unknown_value_deep_inside_is_not_classified_as_unknown() {
        let peer_id = Uuid::new_v4();
        let text = format!(
            r#"{{"type":"PeerJoined","data":{{"peer":{{"id":"{peer_id}","name":"A","candidates":[{{"address":"192.0.2.1:5000","candidate_type":"Relay","priority":1}}]}}}}}}"#
        );
        let e = serde_json::from_str::<SignalingMessage>(&text).unwrap_err();
        assert!(e.to_string().starts_with("unknown variant"), "{}", e);

        assert!(!is_of_unknown_type(&text));
    }

    #[test]
    fn text_that_is_not_a_tagged_message_is_not_classified_as_unknown() {
        for text in ["not json", "[]", r#"{"data":{}}"#, r#"{"type":7}"#] {
            assert!(!is_of_unknown_type(text), "{}", text);
        }
    }

    /// An app with nothing to announce sends the message as an app from
    /// before features did, so an older server reads it unchanged.
    ///
    /// Verifies: REQ-CON-031
    #[test]
    fn an_app_that_announces_no_features_sends_no_features_field() {
        let json = serde_json::to_value(SignalingMessage::JoinRoom {
            room_id: "ABC234".to_string(),
            password: None,
            peer_name: "Bob".to_string(),
            features: vec![],
        })
        .unwrap();
        assert!(json["data"].get("features").is_none(), "{}", json);

        let json = serde_json::to_value(SignalingMessage::CreateRoom {
            room_name: "Jam".to_string(),
            password: None,
            peer_name: "Alice".to_string(),
            features: vec![PEER_MESSAGE_FEATURE.to_string()],
        })
        .unwrap();
        assert_eq!(
            json["data"]["features"],
            serde_json::json!(["peer_message"])
        );
    }

    /// A participant listed by a server, or joined from an app, that predates
    /// features takes no peer messages - so none is offered to it.
    ///
    /// Verifies: REQ-CON-031
    #[test]
    fn a_participant_listed_without_features_takes_no_peer_messages() {
        let id = Uuid::new_v4();
        let older: PeerInfo =
            serde_json::from_str(&format!(r#"{{"id":"{id}","name":"Old"}}"#)).unwrap();
        assert!(!older.takes_peer_messages());

        let newer: PeerInfo = serde_json::from_str(&format!(
            r#"{{"id":"{id}","name":"New","features":["something_else","peer_message"]}}"#
        ))
        .unwrap();
        assert!(newer.takes_peer_messages());
    }

    fn object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("an object").clone()
    }

    /// Sent, a peer message carries only the addressee and the body; relayed,
    /// it arrives with the sender the server stamped.
    ///
    /// Verifies: REQ-CON-031
    #[test]
    fn a_peer_message_is_sent_without_a_sender_and_arrives_with_the_one_the_server_stamped() {
        let to = Uuid::new_v4();
        let sent = serde_json::to_value(SignalingMessage::PeerMessage {
            to: Some(to),
            from: None,
            from_name: None,
            body: object(serde_json::json!({"settings_help": {"kind": "request"}})),
        })
        .unwrap();
        assert_eq!(
            sent,
            serde_json::json!({"type": "PeerMessage", "data": {"to": to, "body": {"settings_help": {"kind": "request"}}}})
        );

        let from = Uuid::new_v4();
        let relayed: SignalingMessage = serde_json::from_value(serde_json::json!({
            "type": "PeerMessage",
            "data": {"from": from, "from_name": "Aki", "to": null, "body": {"any": 1}}
        }))
        .unwrap();
        let SignalingMessage::PeerMessage {
            to,
            from: stamped,
            from_name,
            body,
        } = relayed
        else {
            panic!("not a PeerMessage");
        };
        assert_eq!(to, None, "to everyone in the room");
        assert_eq!(stamped, Some(from));
        assert_eq!(from_name.as_deref(), Some("Aki"));
        assert_eq!(body, object(serde_json::json!({"any": 1})));

        let not_an_object: Result<SignalingMessage, _> =
            serde_json::from_value(serde_json::json!({
                "type": "PeerMessage",
                "data": {"from": from, "from_name": "Aki", "body": "text"}
            }));
        assert!(
            not_an_object.is_err(),
            "a body that is not an object is refused"
        );
    }

    #[test]
    fn test_signaling_message_serialize() {
        let msg = SignalingMessage::CreateRoom {
            room_name: "Test Room".to_string(),
            password: None,
            peer_name: "Alice".to_string(),
            features: vec![],
        };

        let json = serde_json::to_string(&msg).unwrap();
        let parsed: SignalingMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            SignalingMessage::CreateRoom {
                room_name,
                password,
                peer_name,
                ..
            } => {
                assert_eq!(room_name, "Test Room");
                assert!(password.is_none());
                assert_eq!(peer_name, "Alice");
            }
            _ => panic!("Wrong message type"),
        }
    }

    /// Verifies: REQ-CON-021
    #[test]
    fn test_generate_invite_code_length() {
        let code = generate_invite_code();
        assert_eq!(code.len(), INVITE_CODE_LENGTH);
        assert!(
            is_invite_code_format(&code),
            "{:?} fails the format check",
            code
        );
    }

    #[test]
    fn test_generate_invite_code_valid_chars() {
        // Generate multiple codes to test character validity
        for _ in 0..100 {
            let code = generate_invite_code();
            for c in code.chars() {
                assert!(
                    INVITE_CODE_CHARS.contains(&(c as u8)),
                    "Invalid character '{}' in invite code",
                    c
                );
            }
        }
    }

    #[test]
    fn test_generate_invite_code_excludes_confusing_chars() {
        // Generate many codes and verify excluded characters never appear
        let excluded_chars = ['0', 'O', 'I', '1', 'L'];
        for _ in 0..1000 {
            let code = generate_invite_code();
            for c in excluded_chars {
                assert!(
                    !code.contains(c),
                    "Invite code '{}' contains excluded character '{}'",
                    code,
                    c
                );
            }
        }
    }

    #[test]
    fn test_generate_invite_code_uniqueness() {
        // Generate codes and check for uniqueness (probabilistic test)
        // Using 100 samples to minimize collision probability (~1/100,000)
        // while still validating the randomness of the generator
        use std::collections::HashSet;
        let mut codes = HashSet::new();
        for _ in 0..100 {
            let code = generate_invite_code();
            codes.insert(code);
        }
        // With 29^6 possible codes (~594M), 100 codes should all be unique
        assert_eq!(codes.len(), 100);
    }

    #[test]
    fn test_is_invite_code_format_valid() {
        assert!(is_invite_code_format("ABC234"));
        assert!(is_invite_code_format("HJKMNP"));
        assert!(is_invite_code_format("QRSTUV"));
        assert!(is_invite_code_format("WXY789"));
    }

    #[test]
    fn test_is_invite_code_format_invalid_length() {
        assert!(!is_invite_code_format("ABC23")); // Too short
        assert!(!is_invite_code_format("ABC2345")); // Too long
        assert!(!is_invite_code_format("")); // Empty
    }

    #[test]
    fn test_is_invite_code_format_invalid_chars() {
        assert!(!is_invite_code_format("ABC230")); // Contains '0'
        assert!(!is_invite_code_format("ABCDE1")); // Contains '1'
        assert!(!is_invite_code_format("ABCDEO")); // Contains 'O'
        assert!(!is_invite_code_format("ABCDEI")); // Contains 'I'
        assert!(!is_invite_code_format("ABCDEL")); // Contains 'L'
        assert!(!is_invite_code_format("abc234")); // Lowercase
    }

    #[test]
    fn test_is_invite_code_format_uuid_like_strings() {
        // UUID-like strings should not match invite code format
        assert!(!is_invite_code_format("a1b2c3d4")); // 8-char UUID prefix
        assert!(!is_invite_code_format("a1b2c3d4-e5f6"));
    }

    #[test]
    fn test_address_candidate_host_ipv4() {
        let addr: std::net::SocketAddr = "192.168.1.100:5000".parse().unwrap();
        let candidate = AddressCandidate::host(addr);

        assert_eq!(candidate.address, addr);
        assert_eq!(candidate.candidate_type, CandidateType::Host);
        // Host type preference = 126, local_pref for IPv4 = 65534
        assert!(candidate.priority > 0);
    }

    #[test]
    fn test_address_candidate_host_ipv6() {
        let addr: std::net::SocketAddr = "[::1]:5000".parse().unwrap();
        let candidate = AddressCandidate::host(addr);

        assert_eq!(candidate.address, addr);
        assert_eq!(candidate.candidate_type, CandidateType::Host);
        assert!(candidate.priority > 0);
    }

    #[test]
    fn test_address_candidate_server_reflexive() {
        let addr: std::net::SocketAddr = "203.0.113.50:5000".parse().unwrap();
        let candidate = AddressCandidate::server_reflexive(addr);

        assert_eq!(candidate.address, addr);
        assert_eq!(candidate.candidate_type, CandidateType::ServerReflexive);
        assert!(candidate.priority > 0);
    }

    #[test]
    fn test_address_candidate_priority_ordering() {
        // Host candidates should have higher priority than server reflexive
        let host_v4 = AddressCandidate::host("192.168.1.100:5000".parse().unwrap());
        let host_v6 = AddressCandidate::host("[2001:db8::1]:5000".parse().unwrap());
        let srflx_v4 = AddressCandidate::server_reflexive("203.0.113.50:5000".parse().unwrap());
        let srflx_v6 = AddressCandidate::server_reflexive("[2001:db8::2]:5000".parse().unwrap());

        // Host > ServerReflexive
        assert!(host_v4.priority > srflx_v4.priority);
        assert!(host_v6.priority > srflx_v6.priority);

        // IPv6 slightly higher than IPv4 within same type
        assert!(host_v6.priority > host_v4.priority);
        assert!(srflx_v6.priority > srflx_v4.priority);
    }

    #[test]
    fn test_candidates_to_addrs() {
        let candidates = vec![
            AddressCandidate::host("192.168.1.100:5000".parse().unwrap()),
            AddressCandidate::server_reflexive("203.0.113.50:5000".parse().unwrap()),
        ];

        let addrs = candidates_to_addrs(&candidates);

        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0], "192.168.1.100:5000".parse().unwrap());
        assert_eq!(addrs[1], "203.0.113.50:5000".parse().unwrap());
    }

    #[test]
    fn test_candidates_to_addrs_empty() {
        let candidates: Vec<AddressCandidate> = vec![];
        let addrs = candidates_to_addrs(&candidates);
        assert!(addrs.is_empty());
    }

    #[test]
    fn test_peer_info_with_candidates() {
        let peer = PeerInfo {
            id: uuid::Uuid::new_v4(),
            name: "TestPeer".to_string(),
            candidates: vec![AddressCandidate::host(
                "192.168.1.100:5000".parse().unwrap(),
            )],
            public_addr: None,
            local_addr: None,
            joined_at: 0,
            features: vec![],
        };

        assert_eq!(peer.candidates.len(), 1);
        assert_eq!(peer.candidates[0].candidate_type, CandidateType::Host);
    }

    #[test]
    fn test_peer_info_backward_compatible_serialization() {
        // Test that PeerInfo can be deserialized without candidates field (backward compat)
        let json = r#"{"id":"550e8400-e29b-41d4-a716-446655440000","name":"OldPeer","public_addr":"192.168.1.1:5000","local_addr":"192.168.1.1:5000"}"#;
        let peer: PeerInfo = serde_json::from_str(json).unwrap();

        assert_eq!(peer.name, "OldPeer");
        assert!(peer.candidates.is_empty()); // Default empty
        assert!(peer.public_addr.is_some());
    }

    #[test]
    fn test_peer_info_joined_at_defaults_to_zero_when_absent() {
        let json = r#"{"id":"550e8400-e29b-41d4-a716-446655440000","name":"OldPeer"}"#;
        let peer: PeerInfo = serde_json::from_str(json).unwrap();
        assert_eq!(peer.joined_at, 0);
    }
}
