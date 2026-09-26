//! Audio resampling module (ADR-013)
//!
//! Provides sample rate conversion for receive-side audio processing.
//! Per ADR-013: Resampling is performed on the receive side only.
//! The sender transmits at their native sample rate, and the receiver
//! converts to their local sample rate if necessary.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, PolynomialDegree, Resampler as RubatoResampler};

/// Error type for resampler operations
#[derive(Debug, thiserror::Error)]
pub enum ResamplerError {
    #[error("Resampler creation failed: {0}")]
    CreationFailed(String),
    #[error("Resampling failed: {0}")]
    ProcessFailed(String),
    #[error("Invalid sample rate: {0}")]
    InvalidSampleRate(u32),
}

/// Trait for audio resampling
pub trait AudioResampler: Send {
    /// Process input samples and return resampled output
    fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, ResamplerError>;

    /// Get the latency introduced by resampling in samples (at output rate)
    fn latency_samples(&self) -> usize;

    /// Get the latency introduced by resampling in milliseconds
    fn latency_ms(&self, output_rate: u32) -> f32 {
        (self.latency_samples() as f32 / output_rate as f32) * 1000.0
    }
}

/// Passthrough resampler for same sample rate (no conversion)
pub struct PassthroughResampler;

impl AudioResampler for PassthroughResampler {
    fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, ResamplerError> {
        Ok(input.to_vec())
    }

    fn latency_samples(&self) -> usize {
        0
    }
}

/// Fast resampler using rubato for real-time audio
pub struct FastResampler {
    resampler: Async<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    /// Frames (not samples) per resampler call
    chunk_size: usize,
    /// Interleaved input awaiting a full chunk
    input_buffer: Vec<f32>,
}

impl FastResampler {
    /// Create a new fast resampler
    ///
    /// # Arguments
    /// * `input_rate` - Input sample rate in Hz
    /// * `output_rate` - Output sample rate in Hz
    /// * `chunk_size` - Expected input chunk size (frame size)
    pub fn new(
        input_rate: u32,
        output_rate: u32,
        chunk_size: usize,
    ) -> Result<Self, ResamplerError> {
        Self::with_channels(input_rate, output_rate, chunk_size, 1)
    }

    /// Create a resampler for interleaved audio with `channels` channels
    ///
    /// # Arguments
    /// * `input_rate` - Input sample rate in Hz
    /// * `output_rate` - Output sample rate in Hz
    /// * `chunk_size` - Frames per resampler call (not samples)
    /// * `channels` - Interleaved channel count
    ///
    /// The receive path carries stereo, so a mono-only resampler would leave a
    /// peer at a different sample rate playing back at the wrong speed.
    pub fn with_channels(
        input_rate: u32,
        output_rate: u32,
        chunk_size: usize,
        channels: usize,
    ) -> Result<Self, ResamplerError> {
        if input_rate == 0 || output_rate == 0 {
            return Err(ResamplerError::InvalidSampleRate(0));
        }
        if channels == 0 {
            return Err(ResamplerError::CreationFailed(
                "channel count must be at least 1".to_string(),
            ));
        }

        let resample_ratio = output_rate as f64 / input_rate as f64;

        // Use polynomial interpolation with a fixed input size for low-latency
        // real-time processing
        // PolynomialDegree::Cubic is a good quality/latency trade-off
        let resampler = Async::<f32>::new_poly(
            resample_ratio,
            1.0, // max_resample_ratio_relative (no dynamic adjustment needed)
            PolynomialDegree::Cubic,
            chunk_size,
            channels,
            FixedAsync::Input,
        )
        .map_err(|e| ResamplerError::CreationFailed(e.to_string()))?;

        Ok(Self {
            resampler,
            input_rate,
            output_rate,
            channels,
            chunk_size,
            input_buffer: Vec::with_capacity(chunk_size * channels * 2),
        })
    }

    /// Interleaved channel count
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Get the input sample rate
    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// Get the output sample rate
    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }
}

impl AudioResampler for FastResampler {
    fn process(&mut self, input: &[f32]) -> Result<Vec<f32>, ResamplerError> {
        // Add interleaved input to the buffer
        self.input_buffer.extend_from_slice(input);

        // A chunk is `chunk_size` frames across every channel.
        let samples_per_chunk = self.chunk_size * self.channels;
        let mut output = Vec::new();

        while self.input_buffer.len() >= samples_per_chunk {
            let chunk = InterleavedSlice::new(
                &self.input_buffer[..samples_per_chunk],
                self.channels,
                self.chunk_size,
            )
            .map_err(|e| ResamplerError::ProcessFailed(e.to_string()))?;

            let resampled = self
                .resampler
                .process(&chunk, None)
                .map_err(|e| ResamplerError::ProcessFailed(e.to_string()))?;
            output.extend_from_slice(&resampled.take_data());

            self.input_buffer.drain(..samples_per_chunk);
        }

        Ok(output)
    }

    fn latency_samples(&self) -> usize {
        // Polynomial interpolation has minimal latency (typically < 10 samples)
        self.resampler.output_delay()
    }
}

/// Converts what a capture device delivers, at the rate the device opens at,
/// to the session's rate.
///
/// Runs in the device's audio callback, so nothing is allocated after
/// construction. Input is gathered into chunks of `CHUNK_FRAMES` (the
/// resampler takes a fixed size); the part of a chunk not yet complete waits
/// for the next call. That, with the interpolation's own `delay_frames`, is all
/// the delay the conversion adds: a few frames, well under a millisecond.
///
/// Septic interpolation, the widest polynomial rubato offers: its pass band is
/// flat within 0.1 dB to 10 kHz and 1 dB to 15 kHz (44100 to 48000 Hz), where
/// cubic is 0.5 dB and 1.9 dB down. A windowed-sinc filter is flatter still but
/// delays by 0.35 ms to 0.7 ms and cuts off above about 15 kHz to 17 kHz, so it
/// was not taken.
pub struct CaptureResampler {
    resampler: Async<f32>,
    channels: usize,
    /// Interleaved input gathered towards the next chunk
    pending: Vec<f32>,
    /// Interleaved output of one chunk
    converted: Vec<f32>,
}

impl CaptureResampler {
    /// Frames per resampler call. Small, so that little waits for the next
    /// device callback (which can be 10 ms away) to complete a chunk.
    const CHUNK_FRAMES: usize = 8;

    pub fn new(
        device_rate: u32,
        session_rate: u32,
        channels: usize,
    ) -> Result<Self, ResamplerError> {
        if device_rate == 0 || session_rate == 0 {
            return Err(ResamplerError::InvalidSampleRate(0));
        }
        if channels == 0 {
            return Err(ResamplerError::CreationFailed(
                "channel count must be at least 1".to_string(),
            ));
        }
        let resampler = Async::<f32>::new_poly(
            session_rate as f64 / device_rate as f64,
            1.0,
            PolynomialDegree::Septic,
            Self::CHUNK_FRAMES,
            channels,
            FixedAsync::Input,
        )
        .map_err(|e| ResamplerError::CreationFailed(e.to_string()))?;
        Ok(Self {
            converted: vec![0.0; resampler.output_frames_max() * channels],
            pending: Vec::with_capacity(Self::CHUNK_FRAMES * channels),
            resampler,
            channels,
        })
    }

    /// Frames, at the session's rate, that a sound is delayed by the interpolation
    pub fn delay_frames(&self) -> usize {
        self.resampler.output_delay()
    }

    /// Converts `input` (interleaved) and hands each converted chunk to
    /// `emit`. A chunk the resampler refuses is dropped.
    pub fn process(&mut self, mut input: &[f32], mut emit: impl FnMut(&[f32])) {
        let chunk_samples = Self::CHUNK_FRAMES * self.channels;
        while !input.is_empty() {
            let taken = input.len().min(chunk_samples - self.pending.len());
            self.pending.extend_from_slice(&input[..taken]);
            input = &input[taken..];
            if self.pending.len() < chunk_samples {
                break;
            }
            let out_frames = self.converted.len() / self.channels;
            let converted = InterleavedSlice::new(&self.pending, self.channels, Self::CHUNK_FRAMES)
                .ok()
                .zip(InterleavedSlice::new_mut(&mut self.converted, self.channels, out_frames).ok())
                .and_then(|(chunk, mut out)| {
                    self.resampler
                        .process_into_buffer(&chunk, &mut out, None)
                        .ok()
                });
            self.pending.clear();
            if let Some((_, frames)) = converted {
                emit(&self.converted[..frames * self.channels]);
            }
        }
    }
}

/// Create a resampler for the given sample rates
///
/// Returns a PassthroughResampler if input and output rates are the same,
/// otherwise returns a FastResampler.
pub fn create_resampler(
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
) -> Result<Box<dyn AudioResampler>, ResamplerError> {
    create_resampler_with_channels(input_rate, output_rate, chunk_size, 1)
}

/// Create a resampler for interleaved audio with `channels` channels
///
/// `chunk_size` is frames, not samples. The receive path is stereo, so passing
/// 1 there would resample only every other sample and shift the pitch.
pub fn create_resampler_with_channels(
    input_rate: u32,
    output_rate: u32,
    chunk_size: usize,
    channels: usize,
) -> Result<Box<dyn AudioResampler>, ResamplerError> {
    if input_rate == output_rate {
        Ok(Box::new(PassthroughResampler))
    } else {
        Ok(Box::new(FastResampler::with_channels(
            input_rate,
            output_rate,
            chunk_size,
            channels,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_resampler() {
        let mut resampler = PassthroughResampler;
        let input: Vec<f32> = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let output = resampler.process(&input).unwrap();
        assert_eq!(input, output);
        assert_eq!(resampler.latency_samples(), 0);
    }

    #[test]
    fn test_fast_resampler_44100_to_48000() {
        let chunk_size = 128;
        let mut resampler = FastResampler::new(44100, 48000, chunk_size).unwrap();

        // Create a test signal (sine wave)
        let input: Vec<f32> = (0..chunk_size).map(|i| (i as f32 * 0.1).sin()).collect();

        let output = resampler.process(&input).unwrap();

        // Output should be approximately 48000/44100 times the input length
        let expected_ratio = 48000.0 / 44100.0;
        let expected_len = (chunk_size as f32 * expected_ratio).round() as usize;
        // Allow some tolerance due to internal buffering
        assert!(
            output.len() >= expected_len - 10 && output.len() <= expected_len + 10,
            "Expected ~{} samples, got {}",
            expected_len,
            output.len()
        );
    }

    #[test]
    fn test_fast_resampler_48000_to_44100() {
        let chunk_size = 128;
        let mut resampler = FastResampler::new(48000, 44100, chunk_size).unwrap();

        let input: Vec<f32> = (0..chunk_size).map(|i| (i as f32 * 0.1).sin()).collect();

        let output = resampler.process(&input).unwrap();

        let expected_ratio = 44100.0 / 48000.0;
        let expected_len = (chunk_size as f32 * expected_ratio).round() as usize;
        assert!(
            output.len() >= expected_len - 10 && output.len() <= expected_len + 10,
            "Expected ~{} samples, got {}",
            expected_len,
            output.len()
        );
    }

    #[test]
    fn test_create_resampler_passthrough() {
        let resampler = create_resampler(48000, 48000, 128).unwrap();
        assert_eq!(resampler.latency_samples(), 0);
    }

    #[test]
    fn test_create_resampler_conversion() {
        let resampler = create_resampler(44100, 48000, 128).unwrap();
        // FastResampler should have some latency (just verify it works)
        let _ = resampler.latency_samples();
    }

    fn convert(resampler: &mut CaptureResampler, input: &[f32], piece: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for part in input.chunks(piece) {
            resampler.process(part, |converted| out.extend_from_slice(converted));
        }
        out
    }

    fn tone(hz: f32, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn level_db(samples: &[f32]) -> f32 {
        let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
        20.0 * (rms / (0.5 / 2f32.sqrt())).log10()
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn a_capture_converted_from_44100_to_48000_keeps_the_level_of_music_up_to_15khz() {
        for (hz, within_db) in [
            (1000.0, 0.01),
            (5000.0, 0.01),
            (10000.0, 0.1),
            (15000.0, 1.0),
        ] {
            let mut resampler = CaptureResampler::new(44100, 48000, 1).unwrap();
            let out = convert(&mut resampler, &tone(hz, 44100, 44100), 441);
            let settled = &out[out.len() / 2..];
            let db = level_db(settled);
            assert!(db.abs() <= within_db, "{hz} Hz came out {db:.2} dB");
        }
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn a_capture_converted_to_another_rate_has_the_length_of_the_rate_ratio() {
        let mut resampler = CaptureResampler::new(44100, 48000, 1).unwrap();
        let out = convert(&mut resampler, &tone(1000.0, 44100, 44100), 441);
        let shortfall = 48000_i64 - out.len() as i64;
        assert!((0..=16).contains(&shortfall), "{} frames out", out.len());
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn a_capture_gives_the_same_audio_however_the_device_cuts_it_into_callbacks() {
        let input = tone(1000.0, 44100, 4410);
        let whole = convert(
            &mut CaptureResampler::new(44100, 48000, 1).unwrap(),
            &input,
            4410,
        );
        for piece in [1, 7, 64, 441] {
            let cut = convert(
                &mut CaptureResampler::new(44100, 48000, 1).unwrap(),
                &input,
                piece,
            );
            assert_eq!(whole, cut, "callbacks of {piece} frames");
        }
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn the_conversion_delays_a_sound_by_under_a_third_of_a_millisecond() {
        let mut resampler = CaptureResampler::new(44100, 48000, 1).unwrap();
        let mut input = vec![0.0; 4410];
        input[2205] = 1.0;
        let out = convert(&mut resampler, &input, 441);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        let expected = 2205.0 * 48000.0 / 44100.0;
        let delay_frames = peak as f64 - expected;
        assert!(
            delay_frames < 48000.0 * 0.0003,
            "delayed by {delay_frames:.1} frames at 48000 Hz ({:.3} ms)",
            delay_frames / 48.0
        );
    }

    /// Verifies: REQ-AUD-124
    #[test]
    fn a_stereo_capture_keeps_each_channel_to_itself() {
        let mut resampler = CaptureResampler::new(44100, 48000, 2).unwrap();
        let left = tone(1000.0, 44100, 8820);
        let stereo: Vec<f32> = left.iter().flat_map(|&l| [l, 0.0]).collect();
        let out = convert(&mut resampler, &stereo, 882);
        let lefts: Vec<f32> = out.iter().step_by(2).copied().collect();
        let rights: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!(level_db(&lefts[lefts.len() / 2..]).abs() < 0.1);
        assert!(rights.iter().all(|r| *r == 0.0));
    }
}
