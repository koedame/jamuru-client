//! Network module for P2P communication
//!
//! Handles UDP transport, NAT traversal, signaling, FEC, encryption, and connection management.

mod bandwidth;
pub mod buffer_request;
mod clock;
mod connection;
pub mod device_delay_notice;
mod device_identity;
mod discovery;
pub mod dropout;
mod encryption;
mod error;
mod fec;
mod latency;
mod link_facts;
pub mod port_mapping;
mod port_prediction;
mod priority;
mod quality;
pub mod recording_notice;
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
    AudioEncodingConfig, Connection, ConnectionState, ConnectionStats, OneWayDelays,
    PeerLatencyInfo, PunchSchedule, ReconnectConfig,
};
pub use device_identity::{
    device_id_from_public_key, signed_payload, DeviceIdentity, DEVICE_ID_LEN,
};
pub use discovery::{
    check_signaling_url, discover_signaling_url, server_clock_offset_secs, signaling_endpoint_url,
    SignalingEndpoint, SIGNALING_ENDPOINT_PATH,
};
pub use encryption::{LinkClaim, LinkIdentity, LinkSecurity, Opened, SecureLink, SEAL_OVERHEAD};
pub use error::{NetworkError, SignalingFailure};
pub use fec::{FecDecoder, FecEncoder, FecPacket, RecoveredPacket, FEC_GROUP_SIZE};
pub use latency::{
    longer_of_buffer_and_period, DownstreamLatency, LatencyBreakdown, LocalLatencyInfo,
    NetworkLatencyInfo, UpstreamLatency,
};
pub use link_facts::{LinkFacts, LinkRoute, LinkSnapshot, RouteReason};
pub use port_mapping::PortMapping;
pub use priority::mark_voice;
pub use quality::{ConnectionQuality, QualityChange, QualityMonitor};
pub use remote::{
    connect_remote, discover_remote_enrollment, help_relay_url, is_help_session, new_help_session,
    HelpEnd, RemoteEnrollment, RemoteReader, RemoteWriter, REMOTE_ENROLLMENT_PATH,
};
pub use room_entry::{EnterError, EnterRequest, Entered, RoomEntry};
pub use roster::{LinkHealth, Roster};
pub use sequence_tracker::SequenceTracker;
pub use signaling::{
    candidates_to_addrs, ensure_crypto_provider_installed, gather_candidates,
    gather_candidates_mapped, gather_candidates_on, gather_candidates_using,
    gather_host_candidates, gather_host_candidates_for, invite_url, parse_invite_url,
    AddressCandidate, CandidateType, ClientInfo, Gathered, InvalidInviteCode, InviteCode, PeerInfo,
    PeerPresence, RoomInfo, SignalingClient, SignalingConnection, SignalingMessage,
    DEVICE_ID_HEADER, DEVICE_PUBKEY_HEADER, DEVICE_SIGNATURE_HEADER, DEVICE_TIMESTAMP_HEADER,
    INVITE_URL_SCHEME, MAX_PEERS_PER_ROOM, MAX_PEER_CANDIDATES, PEER_MESSAGE_FEATURE, PING_TEXT,
    PONG_TEXT,
};
pub use stun::{StunClient, StunResult, DEFAULT_STUN_SERVERS};
pub use transport::{bind_audio_std, bind_std, canonical_addr, IpFamilies, UdpTransport};
pub use udp_mux::UdpMux;
pub use wifi::{default_route_is_wifi, has_default_route};
