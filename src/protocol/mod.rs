//! Network protocol definitions
//!
//! Defines the packet format for audio data transmission.

mod packet;

pub use packet::{
    route_probe_kind, LatencyInfoMessage, LatencyPing, LatencyPong, Packet, PacketType, PongWall,
    RouteProbeFields, HEADER_SIZE, MAX_PAYLOAD_SIZE, PROTOCOL_VERSION, ROUTE_PROBE_SIGNATURE_LEN,
};
