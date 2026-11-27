//! Audio presets and their application-induced latency budgets
//!
//! This module is the single source of truth for preset parameters and for the
//! one-way, application-induced latency each preset is allowed to cost.
//!
//! Before this module existed the same numbers lived in three places (the
//! zero-latency mode's summary table, the Tauri config layer, and the E2E quality thresholds) and had
//! drifted apart. `src-tauri` now re-exports [`AudioPreset`] from here and the
//! latency tests derive their expectations from the same constants, so the GUI,
//! the audio path and the tests cannot disagree.
//!
//! ## Latency model
//!
//! One-way application-induced latency is the sum of three buffering stages.
//! Encode/decode is 0 ms because the default codec is uncompressed
//! PCM f32:
//!
//! ```text
//! app_latency = capture_buffer + jitter_buffer + playback_buffer
//!             = frame_duration * (1 + jitter_buffer_frames + 1)
//! ```
//!
//! Network RTT is deliberately excluded: it is not attributable to the
//! application, and the top-priority requirement is that total latency
//! approaches "network RTT only".
//!
//! Budgets are defined in [`AudioPreset::max_app_latency_ms`].

use crate::audio::codec::CodecType;
use serde::{Deserialize, Serialize};

/// Sample rate the latency budgets are defined at (48 kHz is the recommended rate).
///
/// Budgets are stated at this rate; [`AudioPreset::designed_app_latency_ms`]
/// accepts any rate so that 44.1 kHz and 96 kHz can be checked as well.
pub const BUDGET_SAMPLE_RATE: u32 = 48_000;

/// Number of buffering stages that cost one frame each: capture and playback.
///
/// The jitter buffer is counted separately because its depth is preset-specific.
const FIXED_FRAME_STAGES: u32 = 2;

/// Available audio presets
///
/// Parameters come from the zero-latency mode and the preset latency budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AudioPreset {
    /// Zero latency - for domestic fiber sessions (0 frame jitter buffer, 32 samples)
    ZeroLatency,
    /// Ultra low latency - for LAN jams (1 frame jitter buffer, 64 samples)
    UltraLowLatency,
    /// Balanced - for typical internet connections (4 frame jitter buffer, 128 samples)
    #[default]
    Balanced,
    /// High quality - for recording (8 frame jitter buffer, 256 samples)
    HighQuality,
}

impl AudioPreset {
    /// Audio frame size in samples per channel
    ///
    /// This is also the value written to `AppConfig::buffer_size` when the
    /// preset is applied.
    pub fn frame_size(&self) -> u32 {
        match self {
            AudioPreset::ZeroLatency => 32,
            AudioPreset::UltraLowLatency => 64,
            AudioPreset::Balanced => 128,
            AudioPreset::HighQuality => 256,
        }
    }

    /// Steady-state jitter buffer depth in frames
    ///
    /// `0` means passthrough: the play-out buffer holds nothing back (see
    /// [`crate::audio::PlayoutConfig`]).
    pub fn jitter_buffer_frames(&self) -> u32 {
        match self {
            AudioPreset::ZeroLatency => 0,
            AudioPreset::UltraLowLatency => 1,
            AudioPreset::Balanced => 4,
            AudioPreset::HighQuality => 8,
        }
    }

    /// Maximum one-way, application-induced latency this preset may cost, in ms
    ///
    /// Verified by `tests/latency_test.rs` against
    /// [`Self::designed_app_latency_ms`]. Raising a budget requires a new design
    /// decision.
    ///
    /// - `zero-latency` and `ultra-low-latency` carry the external targets
    ///   verbatim, because they are what the top-priority requirement promises.
    /// - `balanced` and `high-quality` are the designed value plus 2 ms of
    ///   headroom, because the first estimates for them predate the preset
    ///   parameters and understate the real cost.
    pub fn max_app_latency_ms(&self) -> f32 {
        match self {
            AudioPreset::ZeroLatency => 2.0,
            AudioPreset::UltraLowLatency => 5.0,
            AudioPreset::Balanced => 18.0,
            AudioPreset::HighQuality => 56.0,
        }
    }

    /// Duration of one audio frame at `sample_rate`, in ms
    pub fn frame_duration_ms(&self, sample_rate: u32) -> f32 {
        frame_duration_ms(self.frame_size(), sample_rate)
    }

    /// Delay contributed by the jitter buffer at `sample_rate`, in ms
    pub fn jitter_buffer_delay_ms(&self, sample_rate: u32) -> f32 {
        self.frame_duration_ms(sample_rate) * self.jitter_buffer_frames() as f32
    }

    /// One-way, application-induced latency implied by this preset's parameters
    ///
    /// Computed from the model documented at the module level. Compare against
    /// [`Self::max_app_latency_ms`] to detect a preset whose parameters no
    /// longer fit its budget.
    pub fn designed_app_latency_ms(&self, sample_rate: u32) -> f32 {
        let stages = FIXED_FRAME_STAGES + self.jitter_buffer_frames();
        // Accumulate in f64 so a preset sitting exactly on its budget is not
        // pushed over it by f32 rounding.
        let frame_ms = frame_duration_ms_f64(self.frame_size(), sample_rate);
        (frame_ms * stages as f64) as f32
    }

    /// Codec this preset uses
    ///
    /// Every preset uses uncompressed PCM. Opus accepts only 120/240/480/960/
    /// 1920/2880-sample frames at 48kHz, and no preset's frame size (32, 64,
    /// 128, 256) is among them - encoding at 128 fails outright. Adopting Opus
    /// would mean either abandoning power-of-two device buffer sizes or
    /// repacketising frames, both of which cost latency.
    pub fn codec_type(&self) -> CodecType {
        CodecType::Pcm
    }

    /// FEC group size, or `None` when FEC is disabled for this preset
    ///
    /// Equal to the jitter buffer depth. A lost packet is recovered at most
    /// `group_size - 1` frames late, and the buffer plays `jitter_buffer_frames`
    /// behind the newest packet, so a larger group would recover packets after
    /// they were due.
    ///
    /// A depth of 1 would make the FEC packet a duplicate of the single data
    /// packet - 100% redundancy for no reordering benefit - so FEC is off below
    /// a depth of 2.
    pub fn fec_group_size(&self) -> Option<usize> {
        match self.jitter_buffer_frames() {
            0 | 1 => None,
            frames => Some(frames as usize),
        }
    }

    /// Fraction of extra bandwidth FEC costs, as a ratio (0.25 = 25%)
    pub fn fec_redundancy(&self) -> f32 {
        match self.fec_group_size() {
            None => 0.0,
            Some(group) => 1.0 / group as f32,
        }
    }

    /// Preset identifier used in config files and on the IPC boundary
    pub fn name(&self) -> &'static str {
        match self {
            AudioPreset::ZeroLatency => "zero-latency",
            AudioPreset::UltraLowLatency => "ultra-low-latency",
            AudioPreset::Balanced => "balanced",
            AudioPreset::HighQuality => "high-quality",
        }
    }

    /// Parse a preset from its identifier, or `None` if unknown
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "zero-latency" => Some(AudioPreset::ZeroLatency),
            "ultra-low-latency" => Some(AudioPreset::UltraLowLatency),
            "balanced" => Some(AudioPreset::Balanced),
            "high-quality" => Some(AudioPreset::HighQuality),
            _ => None,
        }
    }

    /// All presets, ordered from lowest to highest latency
    pub fn all() -> Vec<Self> {
        vec![
            AudioPreset::ZeroLatency,
            AudioPreset::UltraLowLatency,
            AudioPreset::Balanced,
            AudioPreset::HighQuality,
        ]
    }
}

/// Duration of `frame_size` samples at `sample_rate`, in ms
///
/// Not exported: callers go through [`AudioPreset::frame_duration_ms`], which
/// ties the frame size to a preset instead of letting the two drift apart.
fn frame_duration_ms(frame_size: u32, sample_rate: u32) -> f32 {
    frame_duration_ms_f64(frame_size, sample_rate) as f32
}

fn frame_duration_ms_f64(frame_size: u32, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    frame_size as f64 / sample_rate as f64 * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_fits_its_latency_budget() {
        for preset in AudioPreset::all() {
            let designed = preset.designed_app_latency_ms(BUDGET_SAMPLE_RATE);
            let budget = preset.max_app_latency_ms();
            assert!(
                designed <= budget,
                "preset {} costs {:.2}ms but its budget is {:.2}ms",
                preset.name(),
                designed,
                budget
            );
        }
    }

    #[test]
    fn presets_are_ordered_by_latency() {
        let latencies: Vec<f32> = AudioPreset::all()
            .iter()
            .map(|p| p.designed_app_latency_ms(BUDGET_SAMPLE_RATE))
            .collect();

        for pair in latencies.windows(2) {
            assert!(
                pair[0] < pair[1],
                "presets must be ordered from lowest to highest latency, got {:?}",
                latencies
            );
        }
    }

    #[test]
    fn preset_names_round_trip() {
        for preset in AudioPreset::all() {
            let parsed = AudioPreset::from_name(preset.name());
            assert_eq!(parsed, Some(preset.clone()), "round trip for {:?}", preset);
        }
        assert_eq!(AudioPreset::from_name("nonexistent"), None);
    }

    #[test]
    fn frame_duration_matches_sample_rate() {
        // 64 samples @ 48kHz = 1.33ms
        assert!((frame_duration_ms(64, 48_000) - 1.333).abs() < 0.001);
        // 256 samples @ 48kHz = 5.33ms
        assert!((frame_duration_ms(256, 48_000) - 5.333).abs() < 0.001);
        // 96kHz halves the duration of the same frame size
        assert!((frame_duration_ms(64, 96_000) - 0.667).abs() < 0.001);
        // A zero sample rate must not divide by zero
        assert_eq!(frame_duration_ms(64, 0), 0.0);
    }

    /// The FEC group must never exceed the jitter buffer depth, or recovered
    /// packets arrive after they were due.
    #[test]
    fn fec_group_never_outlives_the_jitter_buffer() {
        for preset in AudioPreset::all() {
            let depth = preset.jitter_buffer_frames() as usize;
            match preset.fec_group_size() {
                None => assert!(
                    depth <= 1,
                    "{} disables FEC but has {} frames of buffering to work with",
                    preset.name(),
                    depth
                ),
                Some(group) => {
                    assert!(
                        group <= depth,
                        "{} recovers up to {} frames late but only buffers {}",
                        preset.name(),
                        group,
                        depth
                    );
                    assert!(group >= 2, "{} has a pointless FEC group", preset.name());
                    assert!((preset.fec_redundancy() - 1.0 / group as f32).abs() < 1e-6);
                }
            }
        }
    }

    /// Every preset must use a codec that is available and whose frame size the
    /// codec actually accepts. PCM satisfies both unconditionally.
    #[test]
    fn every_preset_uses_an_available_codec() {
        for preset in AudioPreset::all() {
            assert_eq!(
                preset.codec_type(),
                CodecType::Pcm,
                "{} must use PCM: Opus cannot encode a {}-sample frame",
                preset.name(),
                preset.frame_size()
            );
            assert!(
                preset.codec_type().is_available(),
                "{} selected a codec this build cannot use",
                preset.name()
            );
        }

        // Opus frame sizes at 48kHz. Recorded here so that a future change to a
        // preset frame size has to confront the constraint rather than
        // rediscover it as a runtime error.
        const OPUS_FRAME_SIZES: [u32; 6] = [120, 240, 480, 960, 1920, 2880];
        for preset in AudioPreset::all() {
            assert!(
                !OPUS_FRAME_SIZES.contains(&preset.frame_size()),
                "{} now has an Opus-compatible frame size; revisit the presets' codec settings",
                preset.name()
            );
        }
    }

    #[test]
    fn zero_latency_uses_passthrough_jitter_buffer() {
        assert_eq!(AudioPreset::ZeroLatency.jitter_buffer_frames(), 0);
        assert_eq!(
            AudioPreset::ZeroLatency.jitter_buffer_delay_ms(BUDGET_SAMPLE_RATE),
            0.0
        );
    }
}
