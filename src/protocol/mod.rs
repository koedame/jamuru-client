//! Network protocol definitions
//!
//! Defines the packet format for audio data transmission.

mod packet;

pub use packet::{
    LatencyInfoMessage, LatencyPing, LatencyPong, Packet, PacketType, HEADER_SIZE,
    MAX_PAYLOAD_SIZE, PROTOCOL_VERSION,
};
