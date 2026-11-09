//! Packet definitions for the jamuru protocol
//!
//! Packet format (12-byte header):
//! - version: 1 byte
//! - type: 1 byte
//! - sequence: 4 bytes (big-endian)
//! - timestamp: 4 bytes (big-endian, in samples)
//! - flags: 2 bytes

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Protocol version
///
/// 2: PCM audio is 24-bit integers, not 32-bit floats (ADR-096). The two have no
/// marker on the wire, and a payload of one read as the other is loud noise, so a
/// peer of the other version has to be refused rather than decoded.
///
/// 3: the 24-bit integers of a packet are packed without loss (ADR-111). A payload
/// of version 2 has no format byte in front, so it too has to be refused.
pub const PROTOCOL_VERSION: u8 = 3;

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
    /// The id the room knows the sender by, sealed with the link's keys
    PeerId = 0x08,
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
            0x08 => Ok(PacketType::PeerId),
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

    /// A packet saying the room knows the sender by `id`. The server gives whoever enters a new
    /// id, and a link outlives the sender's entry: what the other end is told over the link
    /// itself is what shows the link's far end is that participant.
    pub fn peer_id(sequence: u32, id: Uuid) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::PeerId,
            sequence,
            timestamp: 0,
            flags: PacketFlags::default(),
            payload: id.as_bytes().to_vec(),
        }
    }

    /// The id a [`Self::peer_id`] packet says, `None` for any other packet or a malformed one
    pub fn peer_id_said(&self) -> Option<Uuid> {
        if self.packet_type != PacketType::PeerId {
            return None;
        }
        Uuid::from_slice(&self.payload).ok()
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
/// Binary format (12 bytes, 20 with the wall-clock time):
/// - sent_time_us: 8 bytes (big-endian, microseconds since arbitrary epoch)
/// - ping_sequence: 4 bytes (big-endian)
/// - sent_wall_us: 8 bytes (big-endian, microseconds since the Unix epoch on the
///   sender's clock), appended after the fixed fields so a peer that does not know
///   it still reads the rest and ignores the trailing bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyPing {
    /// Monotonic timestamp when ping was sent (microseconds)
    pub sent_time_us: u64,
    /// Sequence number for this ping
    pub ping_sequence: u32,
    /// The sender's clock when the ping was sent, `None` from a peer that does not send it
    /// (REQ-LAT-132)
    pub sent_wall_us: Option<u64>,
}

impl LatencyPing {
    /// Size of the fixed part of a serialized LatencyPing in bytes
    pub const SIZE: usize = 12;

    /// Size with the wall-clock time
    pub const SIZE_WITH_WALL: usize = Self::SIZE + 8;

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE_WITH_WALL);
        buf.extend_from_slice(&self.sent_time_us.to_be_bytes());
        buf.extend_from_slice(&self.ping_sequence.to_be_bytes());
        if let Some(wall) = self.sent_wall_us {
            buf.extend_from_slice(&wall.to_be_bytes());
        }
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
            sent_wall_us: be_u64_at(data, Self::SIZE),
        })
    }
}

/// The 8 bytes at `offset` as a big-endian number, `None` when `data` ends before them
fn be_u64_at(data: &[u8], offset: usize) -> Option<u64> {
    let bytes: [u8; 8] = data.get(offset..offset + 8)?.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

/// Latency pong payload for RTT measurement
///
/// Binary format (12 bytes, 36 with the wall-clock times):
/// - original_sent_time_us: 8 bytes (big-endian)
/// - ping_sequence: 4 bytes (big-endian)
/// - original_sent_wall_us: 8 bytes, the ping's `sent_wall_us` echoed back
/// - ping_received_wall_us: 8 bytes, the answerer's clock when the ping arrived
/// - sent_wall_us: 8 bytes, the answerer's clock when the pong was sent
///
/// The three clock times are all or nothing, and sit after the fixed fields for the
/// same reason as the ping's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyPong {
    /// Original sent timestamp from ping
    pub original_sent_time_us: u64,
    /// Ping sequence number (echoed back)
    pub ping_sequence: u32,
    /// The clock times that give the two ways of the round trip, `None` when the ping
    /// did not carry its time (REQ-LAT-132)
    pub wall: Option<PongWall>,
}

/// Wall-clock times a pong carries so that the pinger can time each way of the round trip
/// by itself
///
/// Each is microseconds since the Unix epoch. The ping's way is
/// `ping_received_wall_us - original_sent_wall_us` and the pong's way is the pinger's own
/// clock on arrival minus `sent_wall_us`; each is right only as far as the two clocks agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongWall {
    /// The ping's `sent_wall_us`, on the pinger's clock
    pub original_sent_wall_us: u64,
    /// When the ping arrived, on the answerer's clock
    pub ping_received_wall_us: u64,
    /// When the pong was sent, on the answerer's clock
    pub sent_wall_us: u64,
}

impl LatencyPong {
    /// Size of the fixed part of a serialized LatencyPong in bytes
    pub const SIZE: usize = 12;

    /// Size with the wall-clock times
    pub const SIZE_WITH_WALL: usize = Self::SIZE + 24;

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SIZE_WITH_WALL);
        buf.extend_from_slice(&self.original_sent_time_us.to_be_bytes());
        buf.extend_from_slice(&self.ping_sequence.to_be_bytes());
        if let Some(wall) = self.wall {
            buf.extend_from_slice(&wall.original_sent_wall_us.to_be_bytes());
            buf.extend_from_slice(&wall.ping_received_wall_us.to_be_bytes());
            buf.extend_from_slice(&wall.sent_wall_us.to_be_bytes());
        }
        buf
    }

    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        let wall = (data.len() >= Self::SIZE_WITH_WALL).then(|| PongWall {
            original_sent_wall_us: be_u64_at(data, Self::SIZE).unwrap_or(0),
            ping_received_wall_us: be_u64_at(data, Self::SIZE + 8).unwrap_or(0),
            sent_wall_us: be_u64_at(data, Self::SIZE + 16).unwrap_or(0),
        });
        Some(Self {
            original_sent_time_us: u64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]),
            ping_sequence: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
            wall,
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
    fn when_a_peer_id_packet_goes_over_the_wire_the_same_id_is_read_back() {
        let id = Uuid::from_u128(0x1234_5678_9abc_def0);

        let wire = Packet::peer_id(7, id).to_bytes();
        let read = Packet::from_bytes(&wire).unwrap();

        assert_eq!(read.packet_type, PacketType::PeerId);
        assert_eq!(read.peer_id_said(), Some(id));
    }

    #[test]
    fn when_a_peer_id_packet_does_not_hold_exactly_an_id_no_id_is_read() {
        for payload in [vec![], vec![1u8; 15], vec![1u8; 17]] {
            let mut packet = Packet::peer_id(1, Uuid::nil());
            packet.payload = payload;

            assert_eq!(packet.peer_id_said(), None);
        }
        assert_eq!(Packet::keep_alive(1).peer_id_said(), None);
    }

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
            sent_wall_us: Some(1_790_444_697_123_456),
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
            wall: Some(PongWall {
                original_sent_wall_us: 1_790_444_697_123_456,
                ping_received_wall_us: 1_790_444_697_131_000,
                sent_wall_us: 1_790_444_697_131_050,
            }),
        };
        let bytes = pong.to_bytes();
        let decoded = LatencyPong::from_bytes(&bytes).expect("Failed to decode");
        assert_eq!(decoded, pong);
    }

    /// A peer that predates the clock times sends the 12 fixed bytes; they read as no time
    /// rather than as a refused packet, and a newer peer's trailing bytes are ignored by an
    /// older one (it reads only the fixed part).
    ///
    /// Verifies: REQ-LAT-132
    #[test]
    fn when_a_ping_or_pong_has_no_clock_times_they_are_none_and_the_rest_is_read() {
        let ping = LatencyPing {
            sent_time_us: 7,
            ping_sequence: 3,
            sent_wall_us: None,
        };
        let bytes = ping.to_bytes();
        assert_eq!(bytes.len(), LatencyPing::SIZE);
        assert_eq!(LatencyPing::from_bytes(&bytes), Some(ping));

        let pong = LatencyPong {
            original_sent_time_us: 7,
            ping_sequence: 3,
            wall: None,
        };
        let bytes = pong.to_bytes();
        assert_eq!(bytes.len(), LatencyPong::SIZE);
        assert_eq!(LatencyPong::from_bytes(&bytes), Some(pong));
    }

    /// Verifies: REQ-LAT-132
    #[test]
    fn when_a_ping_carries_its_clock_time_an_older_reader_still_reads_the_fixed_part() {
        let ping = LatencyPing {
            sent_time_us: 99,
            ping_sequence: 5,
            sent_wall_us: Some(1_790_444_697_000_000),
        };
        let bytes = ping.to_bytes();
        assert_eq!(bytes.len(), LatencyPing::SIZE_WITH_WALL);
        // What a reader that knows only the 12 fixed bytes sees
        let older = LatencyPing::from_bytes(&bytes[..LatencyPing::SIZE]).unwrap();
        assert_eq!((older.sent_time_us, older.ping_sequence), (99, 5));
        assert_eq!(older.sent_wall_us, None);
    }

    /// A pong cut short in the middle of the clock times has none of them, not half
    ///
    /// Verifies: REQ-LAT-132
    #[test]
    fn when_a_pong_is_cut_short_inside_its_clock_times_it_has_none_of_them() {
        let pong = LatencyPong {
            original_sent_time_us: 1,
            ping_sequence: 2,
            wall: Some(PongWall {
                original_sent_wall_us: 10,
                ping_received_wall_us: 20,
                sent_wall_us: 30,
            }),
        };
        let bytes = pong.to_bytes();
        let cut = LatencyPong::from_bytes(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(cut.wall, None);
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
            sent_wall_us: None,
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

    /// A peer that sends 32-bit float audio has to be refused, not decoded as 24-bit.
    ///
    /// Verifies: REQ-AUD-044
    #[test]
    fn a_packet_of_the_version_that_sent_32_bit_float_is_refused() {
        let mut data = Packet::audio(1, 0, vec![0u8; 12]).to_bytes();
        assert!(Packet::from_bytes(&data).is_some());
        data[0] = 1;
        assert!(Packet::from_bytes(&data).is_none());
    }

    #[test]
    fn test_invalid_protocol_version() {
        let mut data = vec![0u8; HEADER_SIZE];
        data[0] = 99; // Invalid version
        assert!(Packet::from_bytes(&data).is_none());
    }
}
