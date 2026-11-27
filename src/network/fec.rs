//! Forward Error Correction (FEC) for packet loss recovery
//!
//! Implements XOR-based FEC to recover lost packets without retransmission.
//! For every N data packets, generates 1 FEC packet that can recover any single lost packet.

use std::collections::HashMap;

/// FEC group size (number of data packets per FEC packet)
pub const FEC_GROUP_SIZE: usize = 4;

/// FEC packet generator
pub struct FecEncoder {
    group_size: usize,
    current_group: Vec<Vec<u8>>,
    group_sequence: u32,
}

impl FecEncoder {
    /// Create a new FEC encoder
    pub fn new() -> Self {
        Self::with_group_size(FEC_GROUP_SIZE)
    }

    /// Create a new FEC encoder with custom group size
    pub fn with_group_size(group_size: usize) -> Self {
        Self {
            group_size,
            current_group: Vec::with_capacity(group_size),
            group_sequence: 0,
        }
    }

    /// Add a data packet and optionally return an FEC packet
    /// Returns (should_send_fec, fec_data) when the group is complete
    pub fn add_packet(&mut self, data: &[u8]) -> Option<FecPacket> {
        self.current_group.push(data.to_vec());

        if self.current_group.len() >= self.group_size {
            let fec = self.generate_fec();
            self.current_group.clear();
            self.group_sequence += 1;
            Some(fec)
        } else {
            None
        }
    }

    /// Generate FEC packet for the current group
    fn generate_fec(&self) -> FecPacket {
        // Find the maximum packet length
        let max_len = self
            .current_group
            .iter()
            .map(|p| p.len())
            .max()
            .unwrap_or(0);

        // XOR all packets together
        let mut fec_data = vec![0u8; max_len];
        let mut lengths = Vec::with_capacity(self.group_size);

        for packet in &self.current_group {
            lengths.push(packet.len() as u16);
            for (i, &byte) in packet.iter().enumerate() {
                fec_data[i] ^= byte;
            }
        }

        FecPacket {
            group_sequence: self.group_sequence,
            packet_count: self.current_group.len() as u8,
            packet_lengths: lengths,
            fec_data,
        }
    }

    /// Get current group sequence
    pub fn group_sequence(&self) -> u32 {
        self.group_sequence
    }
}

impl Default for FecEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// FEC packet data
#[derive(Debug, Clone)]
pub struct FecPacket {
    /// Sequence number of this FEC group
    pub group_sequence: u32,
    /// Number of data packets in this group
    pub packet_count: u8,
    /// Original lengths of each packet
    pub packet_lengths: Vec<u16>,
    /// XOR'd FEC data
    pub fec_data: Vec<u8>,
}

impl FecPacket {
    /// Serialize FEC packet to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(8 + self.packet_lengths.len() * 2 + self.fec_data.len());

        bytes.extend_from_slice(&self.group_sequence.to_be_bytes());
        bytes.push(self.packet_count);

        // Reserved byte for alignment
        bytes.push(0);

        // Packet lengths
        for &len in &self.packet_lengths {
            bytes.extend_from_slice(&len.to_be_bytes());
        }

        // FEC data
        bytes.extend_from_slice(&self.fec_data);

        bytes
    }

    /// Deserialize FEC packet from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 6 {
            return None;
        }

        let group_sequence = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let packet_count = data[4];
        // data[5] is reserved

        let lengths_size = packet_count as usize * 2;
        if data.len() < 6 + lengths_size {
            return None;
        }

        let mut packet_lengths = Vec::with_capacity(packet_count as usize);
        for i in 0..packet_count as usize {
            let offset = 6 + i * 2;
            let len = u16::from_be_bytes([data[offset], data[offset + 1]]);
            packet_lengths.push(len);
        }

        let fec_data = data[6 + lengths_size..].to_vec();

        Some(Self {
            group_sequence,
            packet_count,
            packet_lengths,
            fec_data,
        })
    }
}

/// FEC decoder for recovering lost packets
pub struct FecDecoder {
    #[allow(dead_code)]
    group_size: usize,
    /// Received packets by group: group_sequence -> (packet_index, data)
    groups: HashMap<u32, GroupState>,
    /// Maximum groups to keep in memory
    max_groups: usize,
}

struct GroupState {
    packets: HashMap<usize, Vec<u8>>,
    fec: Option<FecPacket>,
    recovered: bool,
}

impl FecDecoder {
    /// Create a new FEC decoder
    pub fn new() -> Self {
        Self::with_group_size(FEC_GROUP_SIZE)
    }

    /// Create a new FEC decoder with custom group size
    pub fn with_group_size(group_size: usize) -> Self {
        Self {
            group_size,
            groups: HashMap::new(),
            max_groups: 16,
        }
    }

    /// Add a received data packet
    /// Returns recovered packet if possible
    pub fn add_packet(
        &mut self,
        group_sequence: u32,
        packet_index: usize,
        data: &[u8],
    ) -> Option<RecoveredPacket> {
        let group = self
            .groups
            .entry(group_sequence)
            .or_insert_with(|| GroupState {
                packets: HashMap::new(),
                fec: None,
                recovered: false,
            });

        group.packets.insert(packet_index, data.to_vec());

        // Try to recover if we have FEC and are missing exactly one packet
        let recovered = self.try_recover(group_sequence);
        self.cleanup();
        recovered
    }

    /// Discard all partially received groups
    ///
    /// Call this after a reconnect: sequence numbers continue from where they
    /// left off, but the groups in flight during the outage will never complete,
    /// and a stale group could pair a pre-outage packet with a post-outage FEC
    /// packet and "recover" nonsense.
    pub fn reset(&mut self) {
        self.groups.clear();
    }

    /// Add a received FEC packet
    /// Returns recovered packet if possible
    pub fn add_fec(&mut self, fec: FecPacket) -> Option<RecoveredPacket> {
        let group_sequence = fec.group_sequence;

        let group = self
            .groups
            .entry(group_sequence)
            .or_insert_with(|| GroupState {
                packets: HashMap::new(),
                fec: None,
                recovered: false,
            });

        group.fec = Some(fec);

        // Try to recover
        let recovered = self.try_recover(group_sequence);
        self.cleanup();
        recovered
    }

    /// Try to recover a lost packet in a group
    fn try_recover(&mut self, group_sequence: u32) -> Option<RecoveredPacket> {
        let group = self.groups.get_mut(&group_sequence)?;

        // Already recovered or no FEC
        if group.recovered || group.fec.is_none() {
            return None;
        }

        let fec = group.fec.as_ref().unwrap();
        let expected_count = fec.packet_count as usize;

        // Check if we're missing exactly one packet
        if group.packets.len() + 1 != expected_count {
            return None;
        }

        // Find the missing packet index
        let mut missing_index = None;
        for i in 0..expected_count {
            if !group.packets.contains_key(&i) {
                missing_index = Some(i);
                break;
            }
        }

        let missing_index = missing_index?;

        // Recover the missing packet by XORing all received packets with FEC data
        let recovered_len = fec.packet_lengths.get(missing_index).copied()? as usize;
        let mut recovered = fec.fec_data.clone();

        for (&idx, packet) in &group.packets {
            if idx != missing_index {
                for (i, &byte) in packet.iter().enumerate() {
                    if i < recovered.len() {
                        recovered[i] ^= byte;
                    }
                }
            }
        }

        // Truncate to original length
        recovered.truncate(recovered_len);

        group.recovered = true;

        Some(RecoveredPacket {
            group_sequence,
            packet_index: missing_index,
            data: recovered,
        })
    }

    /// Remove old groups to prevent memory growth
    ///
    /// Runs after every packet, not only after a recovery: without loss no
    /// group ever recovers, and every group would be kept for the whole call.
    fn cleanup(&mut self) {
        while self.groups.len() > self.max_groups {
            // Find oldest group
            if let Some(&oldest) = self.groups.keys().min() {
                self.groups.remove(&oldest);
            }
        }
    }
}

impl Default for FecDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// A recovered packet
#[derive(Debug, Clone)]
pub struct RecoveredPacket {
    /// Group sequence this packet belongs to
    pub group_sequence: u32,
    /// Index within the group
    pub packet_index: usize,
    /// Recovered data
    pub data: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fec_encoder_basic() {
        let mut encoder = FecEncoder::with_group_size(4);

        // Add 3 packets, no FEC yet
        assert!(encoder.add_packet(&[1, 2, 3]).is_none());
        assert!(encoder.add_packet(&[4, 5, 6]).is_none());
        assert!(encoder.add_packet(&[7, 8, 9]).is_none());

        // Add 4th packet, get FEC
        let fec = encoder.add_packet(&[10, 11, 12]).unwrap();
        assert_eq!(fec.packet_count, 4);
        assert_eq!(fec.packet_lengths.len(), 4);
    }

    #[test]
    fn test_fec_roundtrip() {
        let mut encoder = FecEncoder::with_group_size(4);

        let packets = vec![
            vec![1, 2, 3, 4],
            vec![5, 6, 7, 8],
            vec![9, 10, 11, 12],
            vec![13, 14, 15, 16],
        ];

        // Generate FEC
        let mut fec = None;
        for packet in &packets {
            fec = encoder.add_packet(packet);
        }
        let fec = fec.unwrap();

        // Serialize and deserialize
        let bytes = fec.to_bytes();
        let parsed = FecPacket::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.group_sequence, fec.group_sequence);
        assert_eq!(parsed.packet_count, fec.packet_count);
    }

    #[test]
    fn test_fec_recovery() {
        let mut encoder = FecEncoder::with_group_size(4);

        let packets = vec![
            vec![1, 2, 3, 4],
            vec![5, 6, 7, 8],
            vec![9, 10, 11, 12],
            vec![13, 14, 15, 16],
        ];

        // Generate FEC
        let mut fec = None;
        for packet in &packets {
            fec = encoder.add_packet(packet);
        }
        let fec = fec.unwrap();

        // Simulate losing packet 2
        let mut decoder = FecDecoder::with_group_size(4);
        decoder.add_packet(0, 0, &packets[0]);
        decoder.add_packet(0, 1, &packets[1]);
        // Skip packet 2
        decoder.add_packet(0, 3, &packets[3]);

        // Add FEC and recover
        let recovered = decoder.add_fec(fec).unwrap();
        assert_eq!(recovered.packet_index, 2);
        assert_eq!(recovered.data, packets[2]);
    }

    #[test]
    fn test_fec_no_recovery_without_fec() {
        let mut decoder = FecDecoder::with_group_size(4);

        // Add packets without FEC
        assert!(decoder.add_packet(0, 0, &[1, 2, 3]).is_none());
        assert!(decoder.add_packet(0, 1, &[4, 5, 6]).is_none());
        assert!(decoder.add_packet(0, 3, &[10, 11, 12]).is_none());

        // Still missing packet 2 and no FEC - cannot recover
    }

    #[test]
    fn test_fec_variable_length_packets() {
        let mut encoder = FecEncoder::with_group_size(3);

        let packets = vec![vec![1, 2], vec![3, 4, 5, 6], vec![7]];

        let mut fec = None;
        for packet in &packets {
            fec = encoder.add_packet(packet);
        }
        let fec = fec.unwrap();

        // Lose middle packet
        let mut decoder = FecDecoder::with_group_size(3);
        decoder.add_packet(0, 0, &packets[0]);
        decoder.add_packet(0, 2, &packets[2]);

        let recovered = decoder.add_fec(fec).unwrap();
        assert_eq!(recovered.packet_index, 1);
        assert_eq!(recovered.data, packets[1]);
    }

    #[test]
    fn test_decoder_keeps_bounded_groups_without_loss() {
        let group_size = 8;
        let mut encoder = FecEncoder::with_group_size(group_size);
        let mut decoder = FecDecoder::with_group_size(group_size);
        let payload = [7u8; 64];

        // Far more groups than the decoder may keep, none of them lossy
        for seq in 0..(group_size * 1000) as u32 {
            decoder.add_packet(
                seq / group_size as u32,
                (seq % group_size as u32) as usize,
                &payload,
            );
            if let Some(fec) = encoder.add_packet(&payload) {
                decoder.add_fec(fec);
            }
            assert!(decoder.groups.len() <= decoder.max_groups);
        }
    }

    #[test]
    fn test_decoder_recovers_after_many_lossless_groups() {
        let group_size = 8;
        let mut encoder = FecEncoder::with_group_size(group_size);
        let mut decoder = FecDecoder::with_group_size(group_size);
        let packet = |seq: u32| vec![(seq % 251) as u8; 40 + (seq % 5) as usize];
        let lost = group_size * 500 + 3;

        let mut recovered = None;
        for seq in 0..(group_size * 501) as u32 {
            let data = packet(seq);
            if seq as usize != lost {
                decoder.add_packet(
                    seq / group_size as u32,
                    (seq % group_size as u32) as usize,
                    &data,
                );
            }
            if let Some(fec) = encoder.add_packet(&data) {
                if let Some(r) = decoder.add_fec(fec) {
                    recovered = Some(r);
                }
            }
        }

        let recovered = recovered.expect("the lost packet is recovered");
        assert_eq!(recovered.group_sequence, 500);
        assert_eq!(recovered.packet_index, 3);
        assert_eq!(recovered.data, packet(lost as u32));
    }

    #[test]
    fn test_decoder_recovers_with_fec_older_than_the_newest_group() {
        let group_size = 4;
        let mut encoder = FecEncoder::with_group_size(group_size);
        let mut decoder = FecDecoder::with_group_size(group_size);
        let packets: Vec<Vec<u8>> = (0..group_size as u8).map(|i| vec![i + 1; 16]).collect();
        let mut fec = None;
        for p in &packets {
            fec = encoder.add_packet(p);
        }

        // Group 0 loses packet 1; its FEC arrives after the next group started
        for (i, p) in packets.iter().enumerate() {
            if i != 1 {
                decoder.add_packet(0, i, p);
            }
        }
        decoder.add_packet(1, 0, &[9; 16]);

        let recovered = decoder.add_fec(fec.unwrap()).unwrap();
        assert_eq!(recovered.packet_index, 1);
        assert_eq!(recovered.data, packets[1]);
    }
}
