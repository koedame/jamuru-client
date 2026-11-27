//! Packet Loss Concealment for PCM audio
//!
//! Implements "front frame repeat + fadeout" strategy for concealing
//! lost packets in uncompressed PCM audio streams.

/// PCM Packet Loss Concealment
///
/// Strategy:
/// - 1st loss: Repeat the last good frame
/// - 2nd+ loss: Apply exponential fadeout (0.9^n)
/// - 5+ consecutive losses: Output silence
pub struct PcmPlc {
    /// Last successfully received frame
    last_frame: Vec<f32>,
    /// Number of consecutive lost frames
    consecutive_losses: u32,
    /// Frame size in samples per channel
    frame_size: u32,
    /// Number of channels
    channels: u16,
    /// Maximum consecutive losses before silence
    max_losses_before_silence: u32,
    /// Fadeout factor per consecutive loss
    fadeout_factor: f32,
}

impl PcmPlc {
    /// Create a new PCM PLC instance
    ///
    /// # Arguments
    /// * `frame_size` - Number of samples per channel per frame
    /// * `channels` - Number of audio channels
    pub fn new(frame_size: u32, channels: u16) -> Self {
        let total_samples = frame_size as usize * channels as usize;
        Self {
            last_frame: vec![0.0; total_samples],
            consecutive_losses: 0,
            frame_size,
            channels,
            max_losses_before_silence: 5,
            fadeout_factor: 0.85,
        }
    }

    /// Create with custom fadeout parameters
    ///
    /// # Arguments
    /// * `frame_size` - Number of samples per channel per frame
    /// * `channels` - Number of audio channels
    /// * `max_losses` - Maximum consecutive losses before outputting silence
    /// * `fadeout_factor` - Multiplier applied per consecutive loss (0.0-1.0)
    pub fn with_config(
        frame_size: u32,
        channels: u16,
        max_losses: u32,
        fadeout_factor: f32,
    ) -> Self {
        let total_samples = frame_size as usize * channels as usize;
        Self {
            last_frame: vec![0.0; total_samples],
            consecutive_losses: 0,
            frame_size,
            channels,
            max_losses_before_silence: max_losses,
            fadeout_factor: fadeout_factor.clamp(0.0, 1.0),
        }
    }

    /// Store a successfully received frame
    ///
    /// Call this for every successfully decoded frame to update the PLC state.
    pub fn store_frame(&mut self, samples: &[f32]) {
        self.last_frame.clear();
        self.last_frame.extend_from_slice(samples);
        self.consecutive_losses = 0;
    }

    /// Write concealment audio for a lost frame into `out`.
    ///
    /// The allocation-free form of [`Self::generate_concealment`], for callers
    /// on the audio callback, where allocating is not allowed.
    /// `out` is filled to its own length, so a caller with a different frame
    /// size still gets a complete buffer.
    pub fn conceal_into(&mut self, out: &mut [f32]) {
        self.consecutive_losses += 1;

        if self.consecutive_losses > self.max_losses_before_silence {
            out.fill(0.0);
            return;
        }

        let gain = self.fadeout_factor.powi(self.consecutive_losses as i32);
        for (slot, &last) in out.iter_mut().zip(self.last_frame.iter()) {
            *slot = last * gain;
        }
        // A shorter stored frame than the caller asked for leaves a tail; it
        // must be silence rather than whatever the buffer held before.
        if out.len() > self.last_frame.len() {
            out[self.last_frame.len()..].fill(0.0);
        }
    }

    /// Generate concealment audio for a lost frame
    ///
    /// Returns interpolated audio based on the last good frame.
    /// Applies fadeout for consecutive losses.
    pub fn generate_concealment(&mut self) -> Vec<f32> {
        self.consecutive_losses += 1;

        // After too many consecutive losses, output silence
        if self.consecutive_losses > self.max_losses_before_silence {
            let total_samples = self.frame_size as usize * self.channels as usize;
            return vec![0.0; total_samples];
        }

        // Calculate fadeout gain: factor^consecutive_losses
        let gain = self.fadeout_factor.powi(self.consecutive_losses as i32);

        // Apply fadeout to last frame
        self.last_frame.iter().map(|&s| s * gain).collect()
    }

    /// Reset PLC state
    ///
    /// Call this when reconnecting or starting a new stream.
    pub fn reset(&mut self) {
        let total_samples = self.frame_size as usize * self.channels as usize;
        self.last_frame = vec![0.0; total_samples];
        self.consecutive_losses = 0;
    }

    /// Get number of consecutive losses
    pub fn consecutive_losses(&self) -> u32 {
        self.consecutive_losses
    }

    /// Check if currently in concealment mode
    pub fn is_concealing(&self) -> bool {
        self.consecutive_losses > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plc_first_loss() {
        let mut plc = PcmPlc::new(120, 1);

        // Store a frame
        let frame: Vec<f32> = (0..120).map(|i| i as f32 / 120.0).collect();
        plc.store_frame(&frame);

        // First loss - should repeat with fadeout
        let concealed = plc.generate_concealment();
        assert_eq!(concealed.len(), 120);

        // Should be attenuated by fadeout_factor (0.85)
        for (i, &sample) in concealed.iter().enumerate() {
            let expected = frame[i] * 0.85;
            assert!((sample - expected).abs() < 1e-6, "Sample {} mismatch", i);
        }
    }

    #[test]
    fn test_plc_consecutive_losses() {
        let mut plc = PcmPlc::new(120, 1);

        // Store a frame with constant value for easy testing
        let frame = vec![1.0; 120];
        plc.store_frame(&frame);

        // Multiple consecutive losses
        let c1 = plc.generate_concealment();
        assert!((c1[0] - 0.85).abs() < 1e-6); // 0.85^1

        let c2 = plc.generate_concealment();
        assert!((c2[0] - 0.7225).abs() < 1e-4); // 0.85^2

        let c3 = plc.generate_concealment();
        assert!((c3[0] - 0.614125).abs() < 1e-4); // 0.85^3
    }

    #[test]
    fn test_plc_silence_after_many_losses() {
        let mut plc = PcmPlc::new(120, 1);

        let frame = vec![1.0; 120];
        plc.store_frame(&frame);

        // Generate 6 concealment frames (max is 5)
        for _ in 0..6 {
            plc.generate_concealment();
        }

        // 6th+ should be silence
        let concealed = plc.generate_concealment();
        assert!(concealed.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_plc_reset_on_good_frame() {
        let mut plc = PcmPlc::new(120, 1);

        let frame = vec![1.0; 120];
        plc.store_frame(&frame);

        // Some losses
        plc.generate_concealment();
        plc.generate_concealment();
        assert_eq!(plc.consecutive_losses(), 2);

        // Good frame resets counter
        plc.store_frame(&frame);
        assert_eq!(plc.consecutive_losses(), 0);

        // Next loss starts from 1 again
        let c = plc.generate_concealment();
        assert!((c[0] - 0.85).abs() < 1e-6);
    }

    #[test]
    fn test_plc_stereo() {
        let mut plc = PcmPlc::new(120, 2);

        // Stereo frame: 120 samples * 2 channels = 240 total
        let frame: Vec<f32> = (0..240).map(|i| (i % 2) as f32).collect();
        plc.store_frame(&frame);

        let concealed = plc.generate_concealment();
        assert_eq!(concealed.len(), 240);
    }

    #[test]
    fn test_plc_reset() {
        let mut plc = PcmPlc::new(120, 1);

        let frame = vec![1.0; 120];
        plc.store_frame(&frame);
        plc.generate_concealment();
        plc.generate_concealment();

        plc.reset();
        assert_eq!(plc.consecutive_losses(), 0);
        assert!(plc.last_frame.iter().all(|&s| s == 0.0));
    }
}
