//! Network module for P2P communication
//!
//! Handles UDP transport, NAT traversal, signaling, FEC, encryption, and connection management.

mod bandwidth;
mod clock;
mod connection;
mod device_identity;
mod discovery;
mod encryption;
mod error;
mod fec;
mod latency;
mod link_facts;
mod quality;
mod remote;
pub mod room_driver;
mod room_entry;
pub mod room_session;
pub mod roster;
mod sequence_tracker;
pub mod settings_help;
mod signaling;
mod stun;
mod transport;
mod udp_mux;
mod wifi;

pub use bandwidth::{
    audio_packet_duration_ms, required_bps, status_label, BandwidthEstimator, BandwidthStatus,
    BandwidthVerdict,
};
pub use clock::{clock_offset_secs, CLOCK_SKEW_NOTICE_SECS};
pub use connection::{
    AudioEncodingConfig, Connection, ConnectionState, ConnectionStats, PeerLatencyInfo,
    ReconnectConfig,
};
pub use device_identity::{
    device_id_from_public_key, signed_payload, DeviceIdentity, DEVICE_ID_LEN,
};
pub use discovery::{
    check_signaling_url, discover_signaling_url, server_clock_offset_secs, signaling_endpoint_url,
    SignalingEndpoint, SIGNALING_ENDPOINT_PATH,
};
pub use encryption::{LinkIdentity, LinkSecurity, Opened, SecureLink, SEAL_OVERHEAD};
pub use error::{NetworkError, SignalingFailure};
pub use fec::{FecDecoder, FecEncoder, FecPacket, RecoveredPacket, FEC_GROUP_SIZE};
pub use latency::{
    DownstreamLatency, LatencyBreakdown, LocalLatencyInfo, NetworkLatencyInfo, UpstreamLatency,
};
pub use link_facts::{LinkFacts, LinkRoute, LinkSnapshot};
pub use quality::{ConnectionQuality, QualityChange, QualityMonitor};
pub use remote::{
    connect_remote, discover_remote_enrollment, help_relay_url, is_help_session, new_help_session,
    HelpEnd, RemoteEnrollment, RemoteReader, RemoteWriter, REMOTE_ENROLLMENT_PATH,
};
pub use room_entry::{EnterError, EnterRequest, Entered, RoomEntry};
pub use roster::{LinkHealth, Roster};
pub use sequence_tracker::SequenceTracker;
pub use signaling::{
    candidates_to_addrs, ensure_crypto_provider_installed, gather_candidates, gather_candidates_on,
    gather_candidates_using, gather_host_candidates, invite_url, parse_invite_url,
    AddressCandidate, CandidateType, ClientInfo, InvalidInviteCode, InviteCode, PeerInfo, RoomInfo,
    SignalingClient, SignalingConnection, SignalingMessage, DEVICE_ID_HEADER, DEVICE_PUBKEY_HEADER,
    DEVICE_SIGNATURE_HEADER, DEVICE_TIMESTAMP_HEADER, INVITE_URL_SCHEME, MAX_PEERS_PER_ROOM,
    PEER_MESSAGE_FEATURE,
};
pub use stun::{StunClient, StunResult, DEFAULT_STUN_SERVERS};
pub use transport::{bind_std, UdpTransport};
pub use udp_mux::UdpMux;
pub use wifi::default_route_is_wifi;
