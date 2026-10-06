//! Packet definitions for the jamuru protocol
//!
//! Packet format (12-byte header):
//! - version: 1 byte
//! - type: 1 byte
//! - sequence: 4 bytes (big-endian)
//! - timestamp: 4 bytes (big-endian, in samples)
//! - flags: 2 bytes

use serde::{Deserialize, Serialize};

/// Protocol version
pub const PROTOCOL_VERSION: u8 = 1;

/// Header size in bytes
pub const HEADER_SIZE: usize = 12;

/// Maximum payload size (MTU - IP header - UDP header - our header)
/// 1500 - 20 - 8 - 12 = 1460 bytes
pub const MAX_PAYLOAD_SIZE: usize = 1460;

const ROUTE_PROBE_REQUEST: u8 = 1;
const ROUTE_PROBE_REPLY: u8 = 2;
const ROUTE_PROBE_SIZE: usize = 5;

/// Packet types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PacketType {
    /// Audio data
    Audio = 0x01,
    /// FEC redundancy data
    Fec = 0x02,
    /// Control message
    Control = 0x03,
    /// Keep-alive ping
    KeepAlive = 0x04,
    /// Latency measurement ping request
    LatencyPing = 0x05,
    /// Latency measurement pong response
    LatencyPong = 0x06,
    /// Latency configuration info exchange
    LatencyInfo = 0x07,
}

impl TryFrom<u8> for PacketType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(PacketType::Audio),
            0x02 => Ok(PacketType::Fec),
            0x03 => Ok(PacketType::Control),
            0x04 => Ok(PacketType::KeepAlive),
            0x05 => Ok(PacketType::LatencyPing),
            0x06 => Ok(PacketType::LatencyPong),
            0x07 => Ok(PacketType::LatencyInfo),
            _ => Err(()),
        }
    }
}

/// Packet flags
#[derive(Debug, Clone, Copy, Default)]
pub struct PacketFlags {
    /// Packet is encrypted
    pub encrypted: bool,
    /// Packet contains FEC info
    pub has_fec: bool,
}

impl PacketFlags {
    pub fn to_u16(self) -> u16 {
        let mut flags = 0u16;
        if self.encrypted {
            flags |= 0x0001;
        }
        if self.has_fec {
            flags |= 0x0002;
        }
        flags
    }

    pub fn from_u16(value: u16) -> Self {
        Self {
            encrypted: (value & 0x0001) != 0,
            has_fec: (value & 0x0002) != 0,
        }
    }
}

/// A network packet
#[derive(Debug, Clone)]
pub struct Packet {
    pub version: u8,
    pub packet_type: PacketType,
    pub sequence: u32,
    pub timestamp: u32,
    pub flags: PacketFlags,
    pub payload: Vec<u8>,
}

impl Packet {
    /// Create a new audio packet
    pub fn audio(sequence: u32, timestamp: u32, payload: Vec<u8>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::Audio,
            sequence,
            timestamp,
            flags: PacketFlags::default(),
            payload,
        }
    }

    /// Create a new FEC packet
    ///
    /// `payload` is a serialised `FecPacket`. The group number it belongs to is
    /// derivable from the audio sequence numbers on both sides (ADR-021), so it
    /// is not repeated in the header.
    pub fn fec(sequence: u32, timestamp: u32, payload: Vec<u8>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::Fec,
            sequence,
            timestamp,
            flags: PacketFlags::default(),
            payload,
        }
    }

    /// Create a new control packet
    ///
    /// `payload` starts with a byte saying what the message is. A peer that
    /// does not know the message ignores the packet, so a message can be added
    /// without breaking an older app.
    pub fn control(sequence: u32, payload: Vec<u8>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::Control,
            sequence,
            timestamp: 0,
            flags: PacketFlags::default(),
            payload,
        }
    }

    /// Create a new keep-alive packet
    pub fn keep_alive(sequence: u32) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::KeepAlive,
            sequence,
            timestamp: 0,
            flags: PacketFlags::default(),
            payload: Vec::new(),
        }
    }

    /// A keep-alive that asks the peer to send one back carrying the same `token`, so the
    /// sender can time the round trip on the address it went to
    ///
    /// A peer that does not know the request takes it for an ordinary keep-alive and sends
    /// nothing back. Unlike a latency ping it may go before the link has agreed its keys:
    /// it holds only a number the sender chose.
    pub fn route_probe(sequence: u32, token: u32) -> Self {
        Self::keep_alive_with(sequence, ROUTE_PROBE_REQUEST, token)
    }

    /// The keep-alive that answers a [`Self::route_probe`]
    pub fn route_probe_reply(sequence: u32, token: u32) -> Self {
        Self::keep_alive_with(sequence, ROUTE_PROBE_REPLY, token)
    }

    /// Whether this is a [`Self::route_probe`] (`false`) or a [`Self::route_probe_reply`]
    /// (`true`), and its token. `None` for any other packet, including a plain keep-alive.
    pub fn route_probe_token(&self) -> Option<(bool, u32)> {
        if self.packet_type != PacketType::KeepAlive || self.payload.len() != ROUTE_PROBE_SIZE {
            return None;
        }
        let reply = match self.payload[0] {
            ROUTE_PROBE_REQUEST => false,
            ROUTE_PROBE_REPLY => true,
            _ => return None,
        };
        let token = u32::from_be_bytes([
            self.payload[1],
            self.payload[2],
            self.payload[3],
            self.payload[4],
        ]);
        Some((reply, token))
    }

    fn keep_alive_with(sequence: u32, kind: u8, token: u32) -> Self {
        let mut packet = Self::keep_alive(sequence);
        packet.payload.push(kind);
        packet.payload.extend_from_slice(&token.to_be_bytes());
        packet
    }

    /// Create a new latency ping packet
    pub fn latency_ping(sequence: u32, ping: &LatencyPing) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::LatencyPing,
            sequence,
            timestamp: 0,
            flags: PacketFlags::default(),
            payload: ping.to_bytes(),
        }
    }

    /// Create a new latency pong packet
    pub fn latency_pong(sequence: u32, pong: &LatencyPong) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::LatencyPong,
            sequence,
            timestamp: 0,
            flags: PacketFlags::default(),
            payload: pong.to_bytes(),
        }
    }

    /// Create a new latency info packet
    pub fn latency_info(sequence: u32, info: &LatencyInfoMessage) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::LatencyInfo,
            sequence,
            timestamp: 0,
            flags: PacketFlags::default(),
            payload: info.to_bytes(),
        }
    }

    /// The header as it goes on the wire
    pub fn header_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut header = [0u8; HEADER_SIZE];
        header[0] = self.version;
        header[1] = self.packet_type as u8;
        header[2..6].copy_from_slice(&self.sequence.to_be_bytes());
        header[6..10].copy_from_slice(&self.timestamp.to_be_bytes());
        header[10..12].copy_from_slice(&self.flags.to_u16().to_be_bytes());
        header
    }

    /// Serialize the packet to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_SIZE + self.payload.len());

        buf.extend_from_slice(&self.header_bytes());
        buf.extend_from_slice(&self.payload);

        buf
    }

    /// Deserialize a packet from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_SIZE {
            return None;
        }

        let version = data[0];
        if version != PROTOCOL_VERSION {
            return None;
        }

        let packet_type = PacketType::try_from(data[1]).ok()?;
        let sequence = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
        let timestamp = u32::from_be_bytes([data[6], data[7], data[8], data[9]]);
        let flags = PacketFlags::from_u16(u16::from_be_bytes([data[10], data[11]]));
        let payload = data[HEADER_SIZE..].to_vec();

        Some(Self {
            version,
            packet_type,
            sequence,
            timestamp,
            flags,
            payload,
        })
    }
}

// ============================================================================
// Latency measurement message types
// ============================================================================

/// Latency ping payload for RTT measurement
///
/// Binary format (12 bytes):
/// - sent_time_us: 8 bytes (big-endian, microseconds since arbitrary epoch)
/// - ping_sequence: 4 bytes (big-endian)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyPing {
    /// Monotonic timestamp when ping was sent (microseconds)
    pub sent_time_us: u64,
    /// Sequence number for this ping
    pub ping_sequence: u32,
}

impl LatencyPing {
    /// Size of serialized LatencyPing in bytes
    pub const SIZE: usize = 12;

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE);
        buf.extend_from_slice(&self.sent_time_us.to_be_bytes());
        buf.extend_from_slice(&self.ping_sequence.to_be_bytes());
        buf
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            sent_time_us: u64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]),
            ping_sequence: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        })
    }
}

/// Latency pong payload for RTT measurement
///
/// Binary format (12 bytes):
/// - original_sent_time_us: 8 bytes (big-endian)
/// - ping_sequence: 4 bytes (big-endian)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyPong {
    /// Original sent timestamp from ping
    pub original_sent_time_us: u64,
    /// Ping sequence number (echoed back)
    pub ping_sequence: u32,
}

impl LatencyPong {
    /// Size of serialized LatencyPong in bytes
    pub const SIZE: usize = 12;

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE);
        buf.extend_from_slice(&self.original_sent_time_us.to_be_bytes());
        buf.extend_from_slice(&self.ping_sequence.to_be_bytes());
        buf
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            original_sent_time_us: u64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]),
            ping_sequence: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        })
    }
}

/// Latency configuration info exchanged between peers
///
/// Binary format (28 bytes + codec string + optional trailing byte):
/// - capture_buffer_ms: 4 bytes (f32 big-endian)
/// - playback_buffer_ms: 4 bytes (f32 big-endian)
/// - encode_ms: 4 bytes (f32 big-endian)
/// - decode_ms: 4 bytes (f32 big-endian)
/// - jitter_buffer_ms: 4 bytes (f32 big-endian)
/// - frame_size: 4 bytes (u32 big-endian)
/// - sample_rate: 4 bytes (u32 big-endian)
/// - codec_len: 1 byte
/// - codec: variable length UTF-8 string
/// - channel_count: 1 byte, appended after the codec string (added after
///   `PROTOCOL_VERSION` 1 shipped without it)
///
/// `channel_count` sits after the variable-length `codec` string rather than
/// among the fixed-size fields so a peer running the version without it can
/// still decode everything up to `codec` (this parser only checks
/// `data.len()` is large enough, so it silently ignores the extra trailing
/// byte from a newer peer). A peer that hasn't been updated to send it yet
/// decodes here as `DEFAULT_CHANNEL_COUNT` (2, matching the mixer's previous
/// hardcoded assumption).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencyInfoMessage {
    /// Capture buffer latency (ms)
    pub capture_buffer_ms: f32,
    /// Playback buffer latency (ms)
    pub playback_buffer_ms: f32,
    /// Encode latency (ms)
    pub encode_ms: f32,
    /// Decode latency (ms)
    pub decode_ms: f32,
    /// Current jitter buffer delay (ms)
    pub jitter_buffer_ms: f32,
    /// Frame size in samples
    pub frame_size: u32,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Codec name
    pub codec: String,
    /// Transmit channel count (1 = mono, 2 = stereo)
    pub channel_count: u8,
}

impl LatencyInfoMessage {
    /// Minimum size without codec string or channel_count
    pub const MIN_SIZE: usize = 29; // 7 * 4 + 1

    /// `channel_count` a peer decodes when the sender predates that field
    pub const DEFAULT_CHANNEL_COUNT: u8 = 2;

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let codec_bytes = self.codec.as_bytes();
        let codec_len = codec_bytes.len().min(255) as u8;
        let mut buf = Vec::with_capacity(Self::MIN_SIZE + codec_len as usize + 1);

        buf.extend_from_slice(&self.capture_buffer_ms.to_be_bytes());
        buf.extend_from_slice(&self.playback_buffer_ms.to_be_bytes());
        buf.extend_from_slice(&self.encode_ms.to_be_bytes());
        buf.extend_from_slice(&self.decode_ms.to_be_bytes());
        buf.extend_from_slice(&self.jitter_buffer_ms.to_be_bytes());
        buf.extend_from_slice(&self.frame_size.to_be_bytes());
        buf.extend_from_slice(&self.sample_rate.to_be_bytes());
        buf.push(codec_len);
        buf.extend_from_slice(&codec_bytes[..codec_len as usize]);
        buf.push(self.channel_count);

        buf
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::MIN_SIZE {
            return None;
        }

        let capture_buffer_ms = f32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let playback_buffer_ms = f32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let encode_ms = f32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let decode_ms = f32::from_be_bytes([data[12], data[13], data[14], data[15]]);
        let jitter_buffer_ms = f32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let frame_size = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        let sample_rate = u32::from_be_bytes([data[24], data[25], data[26], data[27]]);
        let codec_len = data[28] as usize;

        if data.len() < Self::MIN_SIZE + codec_len {
            return None;
        }

        let codec = String::from_utf8(data[29..29 + codec_len].to_vec()).ok()?;

        let channel_count_offset = 29 + codec_len;
        let channel_count = data
            .get(channel_count_offset)
            .copied()
            .unwrap_or(Self::DEFAULT_CHANNEL_COUNT);

        Some(Self {
            capture_buffer_ms,
            playback_buffer_ms,
            encode_ms,
            decode_ms,
            jitter_buffer_ms,
            frame_size,
            sample_rate,
            codec,
            channel_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_a_route_probe_is_sent_the_peer_reads_its_token_and_that_it_is_a_request() {
        let bytes = Packet::route_probe(7, 0xABCD).to_bytes();

        let packet = Packet::from_bytes(&bytes).expect("a keep-alive with a payload decodes");

        assert_eq!(packet.packet_type, PacketType::KeepAlive);
        assert_eq!(packet.route_probe_token(), Some((false, 0xABCD)));
    }

    #[test]
    fn when_a_route_probe_is_answered_the_reply_carries_the_token_and_is_not_a_request() {
        let bytes = Packet::route_probe_reply(8, 0xABCD).to_bytes();

        let packet = Packet::from_bytes(&bytes).unwrap();

        assert_eq!(packet.route_probe_token(), Some((true, 0xABCD)));
    }

    #[test]
    fn when_a_keep_alive_carries_nothing_it_is_not_a_route_probe() {
        assert_eq!(Packet::keep_alive(1).route_probe_token(), None);
        assert_eq!(
            Packet::audio(1, 0, vec![1, 0, 0, 0, 0]).route_probe_token(),
            None
        );
    }

    #[test]
    fn test_packet_roundtrip() {
        let original = Packet::audio(42, 12345, vec![1, 2, 3, 4, 5]);
        let bytes = original.to_bytes();
        let decoded = Packet::from_bytes(&bytes).expect("Failed to decode packet");

        assert_eq!(decoded.version, original.version);
        assert_eq!(decoded.packet_type, original.packet_type);
        assert_eq!(decoded.sequence, original.sequence);
        assert_eq!(decoded.timestamp, original.timestamp);
        assert_eq!(decoded.payload, original.payload);
    }

    #[test]
    fn test_packet_type_conversion() {
        assert_eq!(PacketType::try_from(0x01), Ok(PacketType::Audio));
        assert_eq!(PacketType::try_from(0x02), Ok(PacketType::Fec));
        assert_eq!(PacketType::try_from(0x03), Ok(PacketType::Control));
        assert_eq!(PacketType::try_from(0x04), Ok(PacketType::KeepAlive));
        assert_eq!(PacketType::try_from(0x05), Ok(PacketType::LatencyPing));
        assert_eq!(PacketType::try_from(0x06), Ok(PacketType::LatencyPong));
        assert_eq!(PacketType::try_from(0x07), Ok(PacketType::LatencyInfo));
        assert_eq!(PacketType::try_from(0xFF), Err(()));
    }

    #[test]
    fn test_latency_ping_roundtrip() {
        let ping = LatencyPing {
            sent_time_us: 1234567890123,
            ping_sequence: 42,
        };
        let bytes = ping.to_bytes();
        let decoded = LatencyPing::from_bytes(&bytes).expect("Failed to decode");
        assert_eq!(decoded, ping);
    }

    #[test]
    fn test_latency_pong_roundtrip() {
        let pong = LatencyPong {
            original_sent_time_us: 1234567890123,
            ping_sequence: 42,
        };
        let bytes = pong.to_bytes();
        let decoded = LatencyPong::from_bytes(&bytes).expect("Failed to decode");
        assert_eq!(decoded, pong);
    }

    #[test]
    fn test_latency_info_roundtrip() {
        let info = LatencyInfoMessage {
            capture_buffer_ms: 2.67,
            playback_buffer_ms: 2.67,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 5.34,
            frame_size: 32,
            sample_rate: 48000,
            codec: "pcm".to_string(),
            channel_count: 1,
        };
        let bytes = info.to_bytes();
        let decoded = LatencyInfoMessage::from_bytes(&bytes).expect("Failed to decode");
        assert_eq!(decoded, info);
    }

    #[test]
    fn test_latency_info_from_peer_without_channel_count_defaults_to_stereo() {
        // A peer running the version before channel_count existed sends a
        // payload that stops right after the codec string.
        let info = LatencyInfoMessage {
            capture_buffer_ms: 2.67,
            playback_buffer_ms: 2.67,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 5.34,
            frame_size: 32,
            sample_rate: 48000,
            codec: "pcm".to_string(),
            channel_count: 1, // irrelevant: stripped off below
        };
        let mut bytes = info.to_bytes();
        bytes.truncate(bytes.len() - 1); // drop the trailing channel_count byte

        let decoded = LatencyInfoMessage::from_bytes(&bytes).expect("Failed to decode");
        assert_eq!(
            decoded.channel_count,
            LatencyInfoMessage::DEFAULT_CHANNEL_COUNT
        );
        assert_eq!(decoded.sample_rate, info.sample_rate);
        assert_eq!(decoded.codec, info.codec);
    }

    #[test]
    fn test_latency_info_ignores_unknown_trailing_bytes() {
        // A future peer might append more fields after channel_count; today's
        // parser should still decode the fields it knows about.
        let info = LatencyInfoMessage {
            capture_buffer_ms: 2.67,
            playback_buffer_ms: 2.67,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 5.34,
            frame_size: 32,
            sample_rate: 48000,
            codec: "pcm".to_string(),
            channel_count: 1,
        };
        let mut bytes = info.to_bytes();
        bytes.extend_from_slice(&[0xAA, 0xBB, 0xCC]);

        let decoded = LatencyInfoMessage::from_bytes(&bytes).expect("Failed to decode");
        assert_eq!(decoded.channel_count, info.channel_count);
    }

    #[test]
    fn test_latency_ping_packet() {
        let ping = LatencyPing {
            sent_time_us: 1000000,
            ping_sequence: 1,
        };
        let packet = Packet::latency_ping(100, &ping);
        assert_eq!(packet.packet_type, PacketType::LatencyPing);

        let bytes = packet.to_bytes();
        let decoded = Packet::from_bytes(&bytes).expect("Failed to decode packet");
        assert_eq!(decoded.packet_type, PacketType::LatencyPing);

        let decoded_ping =
            LatencyPing::from_bytes(&decoded.payload).expect("Failed to decode ping");
        assert_eq!(decoded_ping, ping);
    }

    #[test]
    fn test_flags_roundtrip() {
        let flags = PacketFlags {
            encrypted: true,
            has_fec: true,
        };
        let encoded = flags.to_u16();
        let decoded = PacketFlags::from_u16(encoded);

        assert_eq!(decoded.encrypted, flags.encrypted);
        assert_eq!(decoded.has_fec, flags.has_fec);
    }

    #[test]
    fn test_header_size() {
        let packet = Packet::audio(0, 0, vec![]);
        let bytes = packet.to_bytes();
        assert_eq!(bytes.len(), HEADER_SIZE);
    }

    #[test]
    fn test_invalid_packet_too_short() {
        let data = vec![0u8; HEADER_SIZE - 1];
        assert!(Packet::from_bytes(&data).is_none());
    }

    #[test]
    fn test_invalid_protocol_version() {
        let mut data = vec![0u8; HEADER_SIZE];
        data[0] = 99; // Invalid version
        assert!(Packet::from_bytes(&data).is_none());
    }
}
