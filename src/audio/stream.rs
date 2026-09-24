//! Audio as it travels between peers, and the one path received audio takes.
//!
//! The CLI and the GUI both run sessions. Everything between the socket and
//! the output callback - decoding, sample-rate conversion, the play-out buffer
//! and its concealment - lives here, so the two cannot drift apart: a problem
//! with jitter or loss reproduces in the CLI exactly as the app would play it
//! (ADR-027, ADR-028).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use super::codec::{create_codec, AudioCodec, CodecConfig, CodecError, CodecType};
use super::playout::{
    PlayoutBuffer, PlayoutConfig, PlayoutRead, PlayoutResult, PlayoutStats, WriteOutcome,
};
use super::preset::AudioPreset;
use super::resampler::{create_resampler_with_channels, AudioResampler, ResamplerError};

/// Channels carried on the wire.
///
/// The sender transmits stereo with its own pan already applied, so the receive
/// path is stereo throughout - including the resampler.
pub const WIRE_CHANNELS: usize = 2;

/// Turns a captured mono frame into the stereo frame that is sent.
///
/// `volume` is a gain (1.0 = unchanged) and `pan` runs from -100 (left) to 100
/// (right) with a constant-power law, so a centred source is 3 dB down on each
/// side rather than louder in the middle. `out` must hold two samples per
/// input sample.
pub fn mono_to_wire(mono: &[f32], volume: f32, pan: i32, out: &mut [f32]) {
    let angle = ((pan.clamp(-100, 100) + 100) as f32 / 200.0) * std::f32::consts::FRAC_PI_2;
    let (left_gain, right_gain) = (angle.cos(), angle.sin());

    for (frame, &sample) in out.as_chunks_mut::<WIRE_CHANNELS>().0.iter_mut().zip(mono) {
        let scaled = sample * volume;
        frame[0] = scaled * left_gain;
        frame[1] = scaled * right_gain;
    }
}

/// Turns a captured frame of `channels` channels into the stereo frame that is
/// sent, applying `volume` and `pan` (both as in [`mono_to_wire`]).
///
/// A mono capture is placed in the stereo field by `pan`. A stereo capture
/// keeps its own image: `pan` acts as a balance, leaving the side it points to
/// untouched and turning the other side down, so a centred balance passes the
/// capture through unchanged. `out` must hold two samples per captured frame.
pub fn capture_to_wire(captured: &[f32], channels: usize, volume: f32, pan: i32, out: &mut [f32]) {
    if channels != WIRE_CHANNELS {
        mono_to_wire(captured, volume, pan, out);
        return;
    }

    let angle = (pan.clamp(-100, 100).unsigned_abs() as f32 / 100.0) * std::f32::consts::FRAC_PI_2;
    let quieter = angle.cos();
    let (left_gain, right_gain) = if pan > 0 {
        (quieter, 1.0)
    } else {
        (1.0, quieter)
    };

    for (frame, pair) in out
        .as_chunks_mut::<WIRE_CHANNELS>()
        .0
        .iter_mut()
        .zip(captured.as_chunks::<WIRE_CHANNELS>().0)
    {
        frame[0] = pair[0] * volume * left_gain;
        frame[1] = pair[1] * volume * right_gain;
    }
}

/// What following the peer's sample rate changed.
#[derive(Debug)]
pub enum PeerRateChange {
    /// The rate is the one already being followed.
    Unchanged,
    /// The peer runs at another rate; its audio is converted from now on.
    Resampling {
        from: u32,
        to: u32,
        /// Delay the conversion adds.
        latency_ms: f32,
    },
    /// The peer runs at our rate; nothing is converted.
    Passthrough,
    /// A converter could not be built. Audio plays unconverted.
    Failed(ResamplerError),
}

/// Decoder with the scratch frame it decodes into, so decoding allocates
/// nothing per packet.
struct Decoder {
    codec: Box<dyn AudioCodec>,
    frame: Vec<f32>,
}

/// Received audio, from payload to output callback.
///
/// Cheap to clone: every clone shares the same buffer, so one goes to the
/// network callback and another to the output callback.
#[derive(Clone)]
pub struct ReceivePath {
    playout: Arc<Mutex<PlayoutBuffer>>,
    decoder: Arc<Mutex<Decoder>>,
    resampler: Arc<Mutex<Option<Box<dyn AudioResampler>>>>,
    /// Peer rate being followed; 0 until the peer has said.
    peer_rate: Arc<AtomicU32>,
    sample_rate: u32,
    frame_size: u32,
}

impl ReceivePath {
    /// A receive path for stereo frames of `frame_size` samples per channel,
    /// held back by `target_delay_frames`.
    pub fn new(
        codec_type: CodecType,
        sample_rate: u32,
        frame_size: u32,
        target_delay_frames: u32,
    ) -> Result<Self, CodecError> {
        let codec = create_codec(&CodecConfig {
            codec_type,
            sample_rate,
            channels: WIRE_CHANNELS as u16,
            frame_size,
            bitrate: 0,
        })?;
        let frame_samples = frame_size as usize * WIRE_CHANNELS;

        // The ring is sized for the largest preset, so switching preset in a
        // session never needs a bigger one (ADR-031).
        let largest_preset_delay = AudioPreset::all()
            .iter()
            .map(AudioPreset::jitter_buffer_frames)
            .max()
            .unwrap_or(0);
        let ring_delay_frames =
            PlayoutConfig::for_delay(frame_samples, largest_preset_delay).max_delay_frames;

        Ok(Self {
            playout: Arc::new(Mutex::new(PlayoutBuffer::with_ring_for(
                PlayoutConfig::for_delay(frame_samples, target_delay_frames),
                ring_delay_frames,
            ))),
            decoder: Arc::new(Mutex::new(Decoder {
                codec,
                frame: vec![0.0; frame_samples],
            })),
            resampler: Arc::new(Mutex::new(None)),
            peer_rate: Arc::new(AtomicU32::new(0)),
            sample_rate,
            frame_size,
        })
    }

    /// Interleaved samples in one nominal frame. What the output device should
    /// be opened with.
    pub fn frame_samples(&self) -> usize {
        self.frame_size as usize * WIRE_CHANNELS
    }

    /// Hands a received payload to the play-out buffer, reporting whether it
    /// was stored.
    ///
    /// Called from the network side, never from the audio callback: decoding
    /// happens here so the frame is ready by the time its turn comes.
    pub fn receive(&self, sequence: u32, payload: &[u8]) -> bool {
        let mut decoder = match self.decoder.lock() {
            Ok(decoder) => decoder,
            Err(_) => return false,
        };
        let Decoder { codec, frame } = &mut *decoder;
        let decoded = match codec.decode_into(payload, frame) {
            Ok(len) => &frame[..len],
            Err(e) => {
                tracing::warn!("Discarding undecodable audio frame: {}", e);
                return false;
            }
        };

        // A peer at another sample rate is converted before it is stored, and
        // the converted frame keeps whatever length the conversion produced -
        // padding it to a fixed size would change pitch (ADR-013).
        match self.resampler.lock() {
            Ok(mut guard) => match guard.as_mut() {
                Some(resampler) => match resampler.process(decoded) {
                    Ok(converted) if !converted.is_empty() => self.store(sequence, &converted),
                    // The resampler buffers until it has a full chunk; nothing
                    // ready is not an error.
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!("Resampling failed, playing unconverted: {}", e);
                        self.store(sequence, decoded)
                    }
                },
                None => self.store(sequence, decoded),
            },
            Err(_) => self.store(sequence, decoded),
        }
    }

    /// Fills `out` with the next frame. Called from the audio callback.
    ///
    /// Never waits: if the network side holds the buffer at this instant, the
    /// result is one nominal frame of silence, reported as
    /// [`PlayoutResult::Priming`]. One frame lost that way cannot stall the
    /// device, and it lasts no longer than the frame it stands in for.
    pub fn read_into(&self, out: &mut [f32]) -> PlayoutRead {
        match self.playout.try_lock() {
            Ok(mut buffer) => buffer.read_into(out),
            Err(_) => {
                let samples = self.frame_samples().min(out.len());
                out[..samples].fill(0.0);
                PlayoutRead {
                    result: PlayoutResult::Priming,
                    samples,
                }
            }
        }
    }

    /// Drops everything waiting. Called when the link is (re-)established:
    /// sequence numbers carry on from before an outage, so stale frames would
    /// otherwise play out of order (ADR-022).
    pub fn reset(&self) {
        if let Ok(mut buffer) = self.playout.lock() {
            buffer.reset();
        }
    }

    /// Chooses a new play-out delay, as a preset switch does. What is held
    /// moves to it: silence goes in to raise the delay and frames are skipped
    /// to lower it (ADR-031).
    pub fn set_delay_frames(&self, frames: u32) {
        if let Ok(mut buffer) = self.playout.lock() {
            buffer.set_delay(frames);
        }
    }

    /// The play-out delay in force, in frames.
    ///
    /// Read from the buffer rather than remembered by the caller, because
    /// adaptation and a reconnect move it too.
    pub fn delay_frames(&self) -> u32 {
        self.playout
            .lock()
            .map(|buffer| buffer.target_delay_frames())
            .unwrap_or(0)
    }

    /// Lets the delay follow the link over the time since the last call.
    /// Returns the new delay when it moved. Call it about once a second.
    pub fn adapt(&self) -> Option<u32> {
        self.playout.lock().ok()?.adapt()
    }

    /// Converts the peer's audio when it runs at another sample rate
    /// (ADR-013). Call it whenever the peer reports its rate.
    pub fn follow_peer_rate(&self, peer_rate: u32) -> PeerRateChange {
        if self.peer_rate.swap(peer_rate, Ordering::SeqCst) == peer_rate {
            return PeerRateChange::Unchanged;
        }

        if peer_rate == self.sample_rate {
            self.set_resampler(None);
            return PeerRateChange::Passthrough;
        }

        match create_resampler_with_channels(
            peer_rate,
            self.sample_rate,
            self.frame_size as usize,
            WIRE_CHANNELS,
        ) {
            Ok(resampler) => {
                let latency_ms = resampler.latency_ms(self.sample_rate);
                self.set_resampler(Some(resampler));
                PeerRateChange::Resampling {
                    from: peer_rate,
                    to: self.sample_rate,
                    latency_ms,
                }
            }
            Err(e) => {
                self.set_resampler(None);
                PeerRateChange::Failed(e)
            }
        }
    }

    /// What has happened to received frames so far.
    pub fn stats(&self) -> PlayoutStats {
        self.playout
            .lock()
            .map(|buffer| buffer.stats())
            .unwrap_or_default()
    }

    fn store(&self, sequence: u32, samples: &[f32]) -> bool {
        match self.playout.lock() {
            Ok(mut buffer) => matches!(
                buffer.write(sequence, samples),
                WriteOutcome::Accepted | WriteOutcome::Resynced
            ),
            Err(_) => false,
        }
    }

    fn set_resampler(&self, resampler: Option<Box<dyn AudioResampler>>) {
        if let Ok(mut guard) = self.resampler.lock() {
            *guard = resampler;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn stereo_frame(frame_size: u32, value: f32) -> Vec<f32> {
        vec![value; frame_size as usize * WIRE_CHANNELS]
    }

    /// A payload handed to the receive path comes out of the output side
    /// decoded, in order, after the configured delay - the same route for
    /// every interface.
    ///
    /// Verifies: REQ-LAT-025
    #[test]
    fn a_received_payload_plays_after_the_configured_delay() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 4, 2).expect("PCM is always available");
        let mut out = stereo_frame(4, 9.0);

        for sequence in 0..2 {
            assert!(path.receive(sequence, &pcm(&stereo_frame(4, sequence as f32 + 1.0))));
            assert_eq!(path.read_into(&mut out).result, PlayoutResult::Priming);
            assert!(out.iter().all(|&s| s == 0.0), "priming must be silent");
        }

        assert!(path.receive(2, &pcm(&stereo_frame(4, 3.0))));
        let read = path.read_into(&mut out);
        assert_eq!(read.result, PlayoutResult::Played { sequence: 0 });
        assert_eq!(read.samples, 8);
        assert_eq!(out, stereo_frame(4, 1.0), "the oldest frame plays first");
        assert_eq!(path.stats().frames_played, 1);
    }

    /// A frame that never arrives is concealed rather than skipped, so the
    /// frames after it keep their timing.
    ///
    /// Verifies: REQ-LAT-025
    #[test]
    fn a_missing_frame_is_concealed_in_its_slot() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 4, 0).expect("PCM is always available");
        let mut out = stereo_frame(4, 0.0);

        path.receive(0, &pcm(&stereo_frame(4, 0.5)));
        assert_eq!(
            path.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 0 }
        );

        // 1 is lost; 2 arrives.
        path.receive(2, &pcm(&stereo_frame(4, 0.25)));
        assert_eq!(
            path.read_into(&mut out).result,
            PlayoutResult::Concealed { sequence: 1 }
        );
        assert_eq!(
            path.read_into(&mut out).result,
            PlayoutResult::Played { sequence: 2 }
        );
        assert_eq!(out, stereo_frame(4, 0.25));

        let stats = path.stats();
        assert_eq!((stats.frames_played, stats.frames_concealed), (2, 1));
    }

    /// A payload that does not decode is refused instead of reaching the
    /// buffer as garbage.
    #[test]
    fn an_undecodable_payload_is_not_stored() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 4, 0).expect("PCM is always available");

        assert!(!path.receive(0, &[1, 2, 3]));
        assert_eq!(path.stats().frames_written, 0);
    }

    /// Resetting drops what was waiting, so frames from before an outage
    /// cannot play after it.
    #[test]
    fn reset_discards_waiting_frames() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 4, 1).expect("PCM is always available");
        path.receive(0, &pcm(&stereo_frame(4, 0.5)));
        path.receive(1, &pcm(&stereo_frame(4, 0.5)));

        path.reset();

        let mut out = stereo_frame(4, 9.0);
        assert_eq!(path.read_into(&mut out).result, PlayoutResult::Priming);
        assert_eq!(path.stats(), PlayoutStats::default());
    }

    /// Every preset can be switched to from every other, in a running
    /// session, and lands on its own delay - the range the path started with
    /// does not cut it short (ADR-031).
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn every_preset_can_be_switched_to_from_every_other() {
        for from in AudioPreset::all() {
            let path = ReceivePath::new(CodecType::Pcm, 48000, 4, from.jitter_buffer_frames())
                .expect("PCM is always available");
            for to in AudioPreset::all() {
                path.set_delay_frames(to.jitter_buffer_frames());
                assert_eq!(
                    path.delay_frames(),
                    to.jitter_buffer_frames(),
                    "{} to {}",
                    from.name(),
                    to.name()
                );
            }
        }
    }

    /// The delay change reaches what plays, not just what is reported: after a
    /// switch to a deeper preset the output is silent for the extra frames and
    /// the buffer then holds the new depth.
    ///
    /// Verifies: REQ-LAT-106
    #[test]
    fn a_preset_switch_in_a_running_session_changes_what_is_held() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 4, 1).expect("PCM is always available");
        let mut out = stereo_frame(4, 0.0);
        let mut sequence = 0;
        let mut period = |path: &ReceivePath, out: &mut [f32]| {
            assert!(path.receive(sequence, &pcm(&stereo_frame(4, sequence as f32 + 1.0))));
            sequence += 1;
            path.read_into(out).result
        };

        for _ in 0..4 {
            period(&path, &mut out);
        }

        path.set_delay_frames(3);
        assert_eq!(period(&path, &mut out), PlayoutResult::Padded);
        assert_eq!(period(&path, &mut out), PlayoutResult::Padded);
        assert!(matches!(
            period(&path, &mut out),
            PlayoutResult::Played { .. }
        ));
        assert_eq!(path.stats().frames_concealed, 0);
    }

    /// Adaptation runs on the path the receive loop uses: a lossy stretch
    /// makes the delay grow and says so.
    ///
    /// Verifies: REQ-LAT-108
    #[test]
    fn adaptation_through_the_receive_path_grows_the_delay_on_loss() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 4, 2).expect("PCM is always available");
        let mut out = stereo_frame(4, 0.0);

        for sequence in 0..4u32 {
            path.receive(sequence, &pcm(&stereo_frame(4, 1.0)));
            path.read_into(&mut out);
        }
        assert_eq!(path.adapt(), None, "no stretch to judge yet");

        // Every other frame goes missing.
        for sequence in 4..60u32 {
            if sequence % 2 == 0 {
                path.receive(sequence, &pcm(&stereo_frame(4, 1.0)));
            }
            path.read_into(&mut out);
        }
        assert_eq!(path.adapt(), Some(3));
        assert_eq!(path.delay_frames(), 3);
    }

    /// A peer at another rate is converted, and one at ours is not. Asking
    /// again with the same rate changes nothing (ADR-013).
    #[test]
    fn the_peer_rate_decides_whether_audio_is_converted() {
        let path = ReceivePath::new(CodecType::Pcm, 48000, 64, 0).expect("PCM is always available");

        match path.follow_peer_rate(44100) {
            PeerRateChange::Resampling { from, to, .. } => assert_eq!((from, to), (44100, 48000)),
            other => panic!("a 44.1k peer must be converted, got {:?}", other),
        }
        assert!(matches!(
            path.follow_peer_rate(44100),
            PeerRateChange::Unchanged
        ));

        // A converted 44.1k frame is longer than the nominal 48k one, and it
        // is stored at that length rather than cut to size.
        let mut stored = 0;
        for sequence in 0..8 {
            if path.receive(sequence, &pcm(&vec![0.1; 64 * WIRE_CHANNELS])) {
                stored += 1;
            }
        }
        assert!(stored > 0, "converted audio must reach the buffer");
        let mut out = vec![0.0; path.frame_samples() * 3];
        let mut lengths = Vec::new();
        loop {
            let read = path.read_into(&mut out);
            match read.result {
                PlayoutResult::Played { .. } => lengths.push(read.samples),
                _ => break,
            }
        }
        assert!(
            lengths.iter().any(|&len| len != path.frame_samples()),
            "converted frames keep their own length, got {:?}",
            lengths
        );

        assert!(matches!(
            path.follow_peer_rate(48000),
            PeerRateChange::Passthrough
        ));
    }

    /// A centred source is equal on both sides, and a hard pan silences the
    /// other side - the frame the peer receives already carries the pan.
    #[test]
    fn mono_to_wire_applies_volume_and_pan() {
        let mut out = [0.0f32; 4];

        mono_to_wire(&[1.0, -1.0], 1.0, 0, &mut out);
        assert!((out[0] - out[1]).abs() < 1e-6);
        assert!((out[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((out[2] + std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);

        mono_to_wire(&[1.0, 1.0], 0.5, -100, &mut out);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!(out[1].abs() < 1e-6);
    }

    /// The two sides of a stereo capture stay apart: nothing is mixed across,
    /// and a centred balance leaves the level alone.
    #[test]
    fn capture_to_wire_keeps_the_sides_of_a_stereo_capture_apart() {
        let mut out = [0.0f32; 4];

        capture_to_wire(&[0.25, -0.5, 0.75, 0.125], 2, 1.0, 0, &mut out);

        assert_eq!(out, [0.25, -0.5, 0.75, 0.125]);
    }

    /// Balance turns down the side away from where it points, and volume
    /// scales both.
    #[test]
    fn capture_to_wire_balances_and_scales_a_stereo_capture() {
        let mut out = [0.0f32; 2];

        capture_to_wire(&[1.0, 1.0], 2, 0.5, 100, &mut out);
        assert!(out[0].abs() < 1e-6, "hard right silences the left: {out:?}");
        assert!((out[1] - 0.5).abs() < 1e-6);

        capture_to_wire(&[1.0, 1.0], 2, 1.0, -100, &mut out);
        assert!((out[0] - 1.0).abs() < 1e-6);
        assert!(out[1].abs() < 1e-6, "hard left silences the right: {out:?}");
    }

    /// A mono capture takes the same path as before: one sample becomes a
    /// panned pair.
    #[test]
    fn capture_to_wire_places_a_mono_capture_with_the_pan_law() {
        let mut from_capture = [0.0f32; 4];
        let mut from_mono = [0.0f32; 4];

        capture_to_wire(&[0.5, -0.25], 1, 0.8, 30, &mut from_capture);
        mono_to_wire(&[0.5, -0.25], 0.8, 30, &mut from_mono);

        assert_eq!(from_capture, from_mono);
    }
}
