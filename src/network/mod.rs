//! Network module for P2P communication
//!
//! Handles UDP transport, NAT traversal, signaling, FEC, encryption, and connection management.

mod bandwidth;
mod connection;
mod device_identity;
mod discovery;
mod encryption;
mod error;
mod fec;
mod latency;
mod link_facts;
mod quality;
#[cfg(feature = "remote-link")]
mod remote;
mod sequence_tracker;
mod session;
mod signaling;
mod stun;
mod transport;

pub use bandwidth::{
    required_bps, status_label, BandwidthEstimator, BandwidthStatus, BandwidthVerdict,
};
pub use connection::{
    AudioEncodingConfig, Connection, ConnectionState, ConnectionStats, PeerLatencyInfo,
    ReconnectConfig,
};
pub use device_identity::{
    device_id_from_public_key, signed_payload, DeviceIdentity, DEVICE_ID_LEN,
};
pub use discovery::{
    check_signaling_url, discover_signaling_url, signaling_endpoint_url, SignalingEndpoint,
    SIGNALING_ENDPOINT_PATH,
};
pub use encryption::{EncryptedTransport, EncryptionContext, KeyExchangeMessage, KeyPair};
pub use error::{NetworkError, SignalingFailure};
pub use fec::{FecDecoder, FecEncoder, FecPacket, RecoveredPacket, FEC_GROUP_SIZE};
pub use latency::{
    DownstreamLatency, LatencyBreakdown, LocalLatencyInfo, NetworkLatencyInfo, UpstreamLatency,
};
pub use link_facts::{LinkFacts, LinkRoute, LinkSnapshot};
pub use quality::{ConnectionQuality, QualityChange, QualityMonitor};
#[cfg(feature = "remote-link")]
pub use remote::{
    connect_remote, discover_remote_enrollment, RemoteEnrollment, RemoteReader, RemoteWriter,
    REMOTE_ENROLLMENT_PATH,
};
pub use sequence_tracker::SequenceTracker;
pub use session::{Session, SessionConfig};
pub use signaling::{
    candidates_to_addrs, ensure_crypto_provider_installed, gather_candidates,
    gather_candidates_using, gather_host_candidates, generate_invite_code, invite_url,
    is_invite_code_format, parse_invite_url, AddressCandidate, CandidateType, PeerInfo, RoomInfo,
    SignalingClient, SignalingConnection, SignalingMessage, DEVICE_ID_HEADER, DEVICE_PUBKEY_HEADER,
    DEVICE_SIGNATURE_HEADER, DEVICE_TIMESTAMP_HEADER, INVITE_URL_SCHEME, MAX_PEERS_PER_ROOM,
    PEER_MESSAGE_FEATURE,
};
pub use stun::{StunClient, StunResult, DEFAULT_STUN_SERVERS};
pub use transport::{bind_std, UdpTransport};
