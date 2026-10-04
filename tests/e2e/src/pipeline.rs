//! Software audio pipeline
//!
//! Runs audio through the real jamuru send and receive path - codec, packet
//! serialisation and the play-out buffer with its PLC - without a network
//! socket or an audio device. This is what the loopback layer measures.
//!
//! What it does and does not cover:
//!
//! | Stage | Covered |
//! |-------|---------|
//! | Codec encode/decode | yes, the real `PcmCodec` |
//! | Packet serialisation | yes, the real `Packet::to_bytes` / `from_bytes` |
//! | Jitter buffer delay and reordering | yes, the real `PlayoutBuffer`, configured as the receive path configures it |
//! | Packet loss concealment | yes, the `PcmPlc` inside `PlayoutBuffer` |
//! | Capture and playback device buffers | no - see [`SoftwarePipeline::device_buffer_latency_ms`] |
//! | UDP transport, NAT traversal, encryption | no - covered by the network layers |
//!
//! Because the device buffers cannot exist without hardware, the pipeline
//! reports the delay it actually introduces, and callers add the two device
//! frame stages analytically to get the full application latency (ADR-019).

use jamjam::audio::{
    AudioCodec, AudioPreset, CodecConfig, CodecType, PcmCodec, PlayoutBuffer, PlayoutConfig,
    PlayoutResult,
};
use jamjam::protocol::Packet;

/// Number of buffering stages that exist only with real audio hardware:
/// the capture buffer and the playback buffer.
const DEVICE_FRAME_STAGES: u32 = 2;

/// A single-peer send/receive path built from the production components
pub struct SoftwarePipeline {
    preset: AudioPreset,
    sample_rate: u32,
    frame_size: usize,
    channels: u16,
    encoder: PcmCodec,
    decoder: PcmCodec,
    playout: PlayoutBuffer,
    sequence: u32,
    /// Frames the receiver had to conceal because the packet was lost
    concealed_frames: u32,
    /// Frames the receiver had to fill with silence while the buffer primed
    underrun_frames: u32,
}

impl SoftwarePipeline {
    /// Build a pipeline configured like the given preset
    pub fn new(preset: AudioPreset, sample_rate: u32, channels: u16) -> Self {
        let frame_size = preset.frame_size();
        let codec_config = CodecConfig {
            codec_type: CodecType::Pcm,
            sample_rate,
            channels,
            frame_size,
            ..Default::default()
        };

        // The settings the receive path builds for this preset (ADR-028).
        // Nothing here calls `adapt`, so the delay stays at what the preset
        // specifies - which is what the loopback layer measures.
        let playout_config = PlayoutConfig::for_delay(
            frame_size as usize * channels as usize,
            preset.jitter_buffer_frames(),
        );

        Self {
            preset,
            sample_rate,
            frame_size: frame_size as usize,
            channels,
            encoder: PcmCodec::new(&codec_config),
            decoder: PcmCodec::new(&codec_config),
            playout: PlayoutBuffer::new(playout_config),
            sequence: 0,
            concealed_frames: 0,
            underrun_frames: 0,
        }
    }

    /// Push `input` through the pipeline and collect what the receiver plays
    ///
    /// `lost_frames` lists frame indices that are dropped on the wire, so a
    /// caller can exercise concealment. Pass an empty slice for a clean link.
    pub fn process(&mut self, input: &[f32], lost_frames: &[usize]) -> Vec<f32> {
        let samples_per_frame = self.frame_size * self.channels as usize;
        let mut output = Vec::with_capacity(input.len());

        for (index, frame) in input.chunks(samples_per_frame).enumerate() {
            // A trailing partial frame would desynchronise the codec.
            if frame.len() < samples_per_frame {
                break;
            }

            if !lost_frames.contains(&index) {
                let encoded = self.encoder.encode(frame).expect("PCM encode cannot fail");
                let packet = Packet::audio(
                    self.sequence,
                    self.sequence.wrapping_mul(self.frame_size as u32),
                    encoded,
                );

                // Round-trip through the wire format the same way the receive
                // path does, so a serialisation bug shows up here.
                let wire = packet.to_bytes();
                let received =
                    Packet::from_bytes(&wire).expect("a packet we just built must parse");
                // The receive path decodes on arrival and stores the samples
                // (ADR-028), so decoding happens here rather than on playback.
                let decoded = self
                    .decoder
                    .decode(&received.payload)
                    .expect("PCM decode cannot fail");
                self.playout.write(received.sequence, &decoded);
            }
            self.sequence = self.sequence.wrapping_add(1);

            output.extend(self.pop_one_frame(samples_per_frame));
        }

        output
    }

    /// Drain the play-out buffer after the input has been fully pushed
    ///
    /// The buffer holds frames back, so without draining the tail of the signal
    /// never reaches the output. Stops when the buffer is empty rather than
    /// popping a fixed count, which would manufacture spurious losses.
    pub fn drain(&mut self) -> Vec<f32> {
        let samples_per_frame = self.frame_size * self.channels as usize;
        let mut output = Vec::new();

        while self.playout.ready_frames() > 0 {
            output.extend(self.pop_one_frame(samples_per_frame));
        }

        output
    }

    /// Pull one frame the way the output callback does
    fn pop_one_frame(&mut self, samples_per_frame: usize) -> Vec<f32> {
        let mut frame = vec![0.0; samples_per_frame];
        let read = self.playout.read_into(&mut frame);
        match read.result {
            PlayoutResult::Played { .. } => {}
            PlayoutResult::Concealed { .. } => self.concealed_frames += 1,
            PlayoutResult::Priming | PlayoutResult::Padded | PlayoutResult::Starved => {
                // Nothing to play yet; the output device would be fed silence
                // for this period. While priming, this is the delay the preset
                // buys.
                self.underrun_frames += 1;
            }
        }
        frame.truncate(read.samples);
        frame
    }

    /// Jitter buffer delay this preset is specified to cost (ADR-019)
    ///
    /// Everything else in the software path (codec, packetisation) is
    /// sample-synchronous for PCM, so this is the whole specified pipeline delay.
    pub fn specified_jitter_delay_ms(&self) -> f32 {
        self.preset.jitter_buffer_delay_ms(self.sample_rate)
    }

    /// Jitter buffer delay this pipeline actually produced, in frames
    ///
    /// This is *observed*, not predicted: it counts the frames the receiver had
    /// to fill with silence while the buffer primed. Deriving it instead of
    /// restating `PlayoutBuffer`'s start policy keeps that policy defined in one
    /// place - if the buffer changes when it begins playback, this follows.
    ///
    /// Only meaningful after [`Self::process`] has run.
    pub fn effective_jitter_delay_frames(&self) -> u32 {
        self.underrun_frames
    }

    /// Jitter buffer delay this pipeline actually produced, in milliseconds
    pub fn effective_jitter_delay_ms(&self) -> f32 {
        self.effective_jitter_delay_frames() as f32
            * self.preset.frame_duration_ms(self.sample_rate)
    }

    /// Samples of silence the receiver plays before the signal appears
    ///
    /// Quality scoring must skip these, otherwise it compares the reference
    /// against the priming period and reports a low score for a pipeline that
    /// did nothing wrong.
    pub fn output_delay_samples(&self) -> usize {
        self.effective_jitter_delay_frames() as usize * self.frame_size * self.channels as usize
    }

    /// Latency contributed by the capture and playback device buffers
    ///
    /// Not measurable without hardware, so it is derived from the frame length
    /// exactly as ADR-019 specifies.
    pub fn device_buffer_latency_ms(&self) -> f32 {
        self.preset.frame_duration_ms(self.sample_rate) * DEVICE_FRAME_STAGES as f32
    }

    /// Total one-way application latency: measured pipeline delay plus the
    /// analytically derived device buffers
    pub fn total_app_latency_ms(&self, measured_pipeline_ms: f32) -> f32 {
        measured_pipeline_ms + self.device_buffer_latency_ms()
    }

    /// Frames the receiver had to conceal
    pub fn concealed_frames(&self) -> u32 {
        self.concealed_frames
    }

    /// Frames the receiver filled with silence while the buffer primed
    pub fn underrun_frames(&self) -> u32 {
        self.underrun_frames
    }

    /// The preset this pipeline was built from
    pub fn preset(&self) -> &AudioPreset {
        &self.preset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sample_rate: u32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (i as f32 / sample_rate as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect()
    }

    #[test]
    fn zero_latency_preset_is_sample_synchronous() {
        let sample_rate = 48_000;
        let mut pipeline = SoftwarePipeline::new(AudioPreset::ZeroLatency, sample_rate, 1);
        let input = sine(sample_rate, 32 * 16);

        let mut output = pipeline.process(&input, &[]);
        output.extend(pipeline.drain());

        assert_eq!(pipeline.specified_jitter_delay_ms(), 0.0);
        assert_eq!(pipeline.effective_jitter_delay_ms(), 0.0);
        assert_eq!(
            pipeline.underrun_frames(),
            0,
            "passthrough must not underrun"
        );
        assert_eq!(
            output, input,
            "with a passthrough buffer and PCM the output must be bit-identical"
        );
    }

    #[test]
    fn buffered_preset_delays_the_signal_by_its_jitter_depth() {
        let sample_rate = 48_000;
        let preset = AudioPreset::Balanced;
        let frame = preset.frame_size() as usize;
        let jitter_frames = preset.jitter_buffer_frames() as usize;

        let mut pipeline = SoftwarePipeline::new(preset, sample_rate, 1);
        let input = sine(sample_rate, frame * 24);

        let mut output = pipeline.process(&input, &[]);
        output.extend(pipeline.drain());

        // Since ADR-020 the observed delay equals the configured depth: the
        // buffer accumulates one frame more than the target before it starts,
        // so it still holds `jitter_frames` afterwards.
        let observed_delay_frames = pipeline.effective_jitter_delay_frames() as usize;
        let expected_delay_samples = observed_delay_frames * frame;

        assert_eq!(
            observed_delay_frames, jitter_frames,
            "the observed delay must equal the depth the preset specifies"
        );
        assert_eq!(
            pipeline.effective_jitter_delay_ms(),
            pipeline.specified_jitter_delay_ms(),
            "measured and specified jitter delay must now agree (ADR-020)"
        );
        assert!(
            output[..expected_delay_samples].iter().all(|&s| s == 0.0),
            "the priming period must be silence"
        );
        assert_eq!(
            &output[expected_delay_samples..expected_delay_samples + frame],
            &input[..frame],
            "after the priming delay the original signal must appear unaltered"
        );
    }

    #[test]
    fn a_lost_frame_is_concealed_rather_than_dropped() {
        let sample_rate = 48_000;
        let preset = AudioPreset::UltraLowLatency;
        let frame = preset.frame_size() as usize;

        let mut pipeline = SoftwarePipeline::new(preset, sample_rate, 1);
        let input = sine(sample_rate, frame * 12);

        let mut output = pipeline.process(&input, &[5]);
        output.extend(pipeline.drain());

        assert_eq!(
            pipeline.concealed_frames(),
            1,
            "the dropped frame must be concealed"
        );

        // Concealment replaces the lost frame rather than skipping it, so the
        // output holds every input frame plus the buffer's priming delay.
        let delay_samples = pipeline.output_delay_samples();
        assert_eq!(
            output.len(),
            input.len() + delay_samples,
            "concealment must not drop or duplicate frames"
        );
    }
}
