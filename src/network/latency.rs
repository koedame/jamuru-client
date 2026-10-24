//! Latency measurement and breakdown for P2P audio connections
//!
//! This module provides structures and utilities for tracking and displaying
//! end-to-end audio latency broken down by component.

use serde::{Deserialize, Serialize};

use super::connection::PeerLatencyInfo;

/// Local audio configuration latency info (calculated from config)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalLatencyInfo {
    /// Capture buffer latency in ms (frame_size / sample_rate * 1000)
    pub capture_buffer_ms: f32,
    /// Playback buffer latency in ms (frame_size / sample_rate * 1000)
    pub playback_buffer_ms: f32,
    /// Codec encode latency in ms (0 for PCM)
    pub encode_ms: f32,
    /// Codec decode latency in ms (0 for PCM)
    pub decode_ms: f32,
    /// Current jitter buffer delay in ms
    pub jitter_buffer_ms: f32,
    /// Frame size in samples
    pub frame_size: u32,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Codec name
    pub codec: String,
}

impl LocalLatencyInfo {
    /// Create local latency info from audio configuration
    pub fn from_audio_config(frame_size: u32, sample_rate: u32, codec: &str) -> Self {
        let buffer_ms = (frame_size as f32 / sample_rate as f32) * 1000.0;
        let codec_latency = match codec.to_lowercase().as_str() {
            "pcm" | "pcm32" | "pcm16" => 0.0,
            "opus" => 2.5, // Opus algorithmic delay is ~2.5ms at 48kHz
            _ => 0.0,
        };

        Self {
            capture_buffer_ms: buffer_ms,
            playback_buffer_ms: buffer_ms,
            encode_ms: codec_latency,
            decode_ms: codec_latency,
            jitter_buffer_ms: 0.0, // Updated separately
            frame_size,
            sample_rate,
            codec: codec.to_string(),
        }
    }

    /// Count the period the system hands the devices their samples at (the
    /// Windows mixer's 10 ms, or what the way the device was opened gives).
    /// Each side takes the longer of its buffer and its period; a side whose
    /// period is not known keeps its buffer.
    pub fn set_os_periods_us(&mut self, capture: Option<u32>, playback: Option<u32>) {
        self.capture_buffer_ms = longer_of_buffer_and_period(self.capture_buffer_ms, capture);
        self.playback_buffer_ms = longer_of_buffer_and_period(self.playback_buffer_ms, playback);
    }

    /// Update jitter buffer delay
    pub fn set_jitter_buffer_ms(&mut self, jitter_buffer_ms: f32) {
        self.jitter_buffer_ms = jitter_buffer_ms;
    }
}

/// The latency of one side: its buffer or the system's period, whichever is
/// longer. `None`: the period is not known, and the buffer is all there is.
pub fn longer_of_buffer_and_period(buffer_ms: f32, period_us: Option<u32>) -> f32 {
    buffer_ms.max(period_us.map_or(0.0, |us| us as f32 / 1000.0))
}

/// Network latency info (measured)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkLatencyInfo {
    /// Round-trip time in ms
    pub rtt_ms: f32,
    /// One-way latency estimate (RTT/2) in ms
    pub one_way_ms: f32,
    /// Jitter (variation in packet arrival) in ms
    pub jitter_ms: f32,
    /// Packet loss rate (0.0-1.0)
    pub packet_loss_rate: f32,
}

impl NetworkLatencyInfo {
    /// Create from RTT and jitter measurements
    pub fn from_measurements(rtt_ms: f32, jitter_ms: f32, packet_loss_rate: f32) -> Self {
        Self {
            rtt_ms,
            one_way_ms: rtt_ms / 2.0,
            jitter_ms,
            packet_loss_rate,
        }
    }
}

/// Upstream latency breakdown (self -> peer)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpstreamLatency {
    /// Local capture buffer latency
    pub capture_buffer_ms: f32,
    /// Local encode latency
    pub encode_ms: f32,
    /// Network one-way (self -> peer)
    pub network_ms: f32,
    /// Peer's jitter buffer latency
    pub peer_jitter_buffer_ms: f32,
    /// Peer's decode latency
    pub peer_decode_ms: f32,
    /// Peer's playback buffer latency
    pub peer_playback_buffer_ms: f32,
}

impl UpstreamLatency {
    /// Calculate total upstream latency
    pub fn total(&self) -> f32 {
        self.capture_buffer_ms
            + self.encode_ms
            + self.network_ms
            + self.peer_jitter_buffer_ms
            + self.peer_decode_ms
            + self.peer_playback_buffer_ms
    }
}

/// Downstream latency breakdown (peer -> self)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownstreamLatency {
    /// Peer's capture buffer latency
    pub peer_capture_buffer_ms: f32,
    /// Peer's encode latency
    pub peer_encode_ms: f32,
    /// Network one-way (peer -> self)
    pub network_ms: f32,
    /// Local jitter buffer latency
    pub jitter_buffer_ms: f32,
    /// Local decode latency
    pub decode_ms: f32,
    /// Local playback buffer latency
    pub playback_buffer_ms: f32,
}

impl DownstreamLatency {
    /// Calculate total downstream latency
    pub fn total(&self) -> f32 {
        self.peer_capture_buffer_ms
            + self.peer_encode_ms
            + self.network_ms
            + self.jitter_buffer_ms
            + self.decode_ms
            + self.playback_buffer_ms
    }
}

/// Complete latency breakdown for a peer connection
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LatencyBreakdown {
    /// Total upstream latency (self -> peer) in ms
    pub upstream_total_ms: f32,
    /// Total downstream latency (peer -> self) in ms
    pub downstream_total_ms: f32,
    /// Total round-trip latency in ms
    pub roundtrip_total_ms: f32,
    /// Upstream breakdown
    pub upstream: UpstreamLatency,
    /// Downstream breakdown
    pub downstream: DownstreamLatency,
    /// Network stats
    pub network: NetworkLatencyInfo,
}

impl LatencyBreakdown {
    /// Calculate latency breakdown from local info, peer info, and network measurements
    pub fn calculate(
        local: &LocalLatencyInfo,
        peer: Option<&PeerLatencyInfo>,
        rtt_ms: f32,
        jitter_ms: f32,
    ) -> Self {
        let one_way_ms = rtt_ms / 2.0;

        // Use peer info if available, otherwise use zeros (unknown)
        let (peer_capture, peer_playback, peer_encode, peer_decode, peer_jitter) =
            if let Some(p) = peer {
                (
                    p.capture_buffer_ms,
                    p.playback_buffer_ms,
                    p.encode_ms,
                    p.decode_ms,
                    p.jitter_buffer_ms,
                )
            } else {
                (0.0, 0.0, 0.0, 0.0, 0.0)
            };

        let upstream = UpstreamLatency {
            capture_buffer_ms: local.capture_buffer_ms,
            encode_ms: local.encode_ms,
            network_ms: one_way_ms,
            peer_jitter_buffer_ms: peer_jitter,
            peer_decode_ms: peer_decode,
            peer_playback_buffer_ms: peer_playback,
        };

        let downstream = DownstreamLatency {
            peer_capture_buffer_ms: peer_capture,
            peer_encode_ms: peer_encode,
            network_ms: one_way_ms,
            jitter_buffer_ms: local.jitter_buffer_ms,
            decode_ms: local.decode_ms,
            playback_buffer_ms: local.playback_buffer_ms,
        };

        let upstream_total = upstream.total();
        let downstream_total = downstream.total();

        Self {
            upstream_total_ms: upstream_total,
            downstream_total_ms: downstream_total,
            roundtrip_total_ms: upstream_total + downstream_total,
            upstream,
            downstream,
            network: NetworkLatencyInfo::from_measurements(rtt_ms, jitter_ms, 0.0),
        }
    }

    /// Check if peer info is available (non-zero values)
    pub fn has_peer_info(&self) -> bool {
        self.upstream.peer_playback_buffer_ms > 0.0 || self.downstream.peer_capture_buffer_ms > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_latency_info_pcm() {
        let info = LocalLatencyInfo::from_audio_config(128, 48000, "pcm");
        assert!((info.capture_buffer_ms - 2.67).abs() < 0.1); // 128/48000 * 1000 ≈ 2.67ms
        assert_eq!(info.encode_ms, 0.0);
        assert_eq!(info.decode_ms, 0.0);
    }

    /// Verifies: REQ-AUD-043
    #[test]
    fn when_the_system_period_is_longer_than_the_buffer_the_period_is_the_latency() {
        let mut info = LocalLatencyInfo::from_audio_config(64, 48000, "pcm");
        info.set_os_periods_us(Some(10_000), Some(10_000));

        assert!((info.capture_buffer_ms - 10.0).abs() < 0.001);
        assert!((info.playback_buffer_ms - 10.0).abs() < 0.001);
    }

    /// Verifies: REQ-AUD-043
    #[test]
    fn when_the_buffer_is_longer_than_the_system_period_the_buffer_is_the_latency() {
        let mut info = LocalLatencyInfo::from_audio_config(960, 48000, "pcm");
        info.set_os_periods_us(Some(3_000), Some(10_000));

        assert!((info.capture_buffer_ms - 20.0).abs() < 0.001);
        assert!((info.playback_buffer_ms - 20.0).abs() < 0.001);
    }

    /// Verifies: REQ-AUD-043
    #[test]
    fn when_a_side_has_no_known_period_it_keeps_its_buffer_and_the_other_side_takes_its_period() {
        let mut info = LocalLatencyInfo::from_audio_config(64, 48000, "pcm");
        info.set_os_periods_us(None, Some(10_000));

        assert!((info.capture_buffer_ms - 1.333).abs() < 0.01);
        assert!((info.playback_buffer_ms - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_local_latency_info_opus() {
        let info = LocalLatencyInfo::from_audio_config(960, 48000, "opus");
        assert!((info.capture_buffer_ms - 20.0).abs() < 0.1); // 960/48000 * 1000 = 20ms
        assert!((info.encode_ms - 2.5).abs() < 0.1);
    }

    #[test]
    fn test_latency_breakdown_calculation() {
        let local = LocalLatencyInfo {
            capture_buffer_ms: 2.67,
            playback_buffer_ms: 2.67,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 5.0,
            frame_size: 32,
            sample_rate: 48000,
            codec: "pcm".to_string(),
        };

        let peer = PeerLatencyInfo {
            capture_buffer_ms: 0.67,
            playback_buffer_ms: 0.67,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 0.0,
            frame_size: 32,
            sample_rate: 48000,
            codec: "pcm".to_string(),
            channel_count: 2,
        };

        let breakdown = LatencyBreakdown::calculate(&local, Some(&peer), 15.0, 1.0);

        // Upstream: 2.67 + 0 + 7.5 + 0 + 0 + 0.67 = 10.84ms
        assert!((breakdown.upstream_total_ms - 10.84).abs() < 0.1);

        // Downstream: 0.67 + 0 + 7.5 + 5.0 + 0 + 2.67 = 15.84ms
        assert!((breakdown.downstream_total_ms - 15.84).abs() < 0.1);

        assert!(breakdown.has_peer_info());
    }

    #[test]
    fn test_latency_breakdown_without_peer_info() {
        let local = LocalLatencyInfo::from_audio_config(128, 48000, "pcm");
        let breakdown = LatencyBreakdown::calculate(&local, None, 10.0, 0.5);

        assert!(!breakdown.has_peer_info());
        // Network latency should still be calculated
        assert!((breakdown.network.one_way_ms - 5.0).abs() < 0.1);
    }
}
