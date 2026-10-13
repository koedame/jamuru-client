//! Everyone else in the room, mixed for the output callback.
//!
//! Each participant there is an audio link to has a voice here: their
//! play-out buffer, and where their fader and pan stand. The output callback
//! asks for one frame and gets the sum of them all.
//!
//! Runs on the audio callback, so mixing allocates nothing and never waits. A
//! voice that is being put in or taken out at that instant is left out of one
//! frame; nobody else's audio is touched.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{pan_received, rms_level, GainRamp, PlayoutResult, ReceivePath, WIRE_CHANNELS};

/// Where one participant's fader and pan stand. Shared with whoever moves
/// them, so a change is heard on the next frame.
#[derive(Debug)]
pub struct PeerGain {
    /// Percent of unity gain (0-200); 0 when they are muted
    volume: AtomicU32,
    /// -100 (full left) to 100 (full right)
    pan: AtomicI32,
}

impl Default for PeerGain {
    fn default() -> Self {
        Self {
            volume: AtomicU32::new(100),
            pan: AtomicI32::new(0),
        }
    }
}

impl PeerGain {
    pub fn set_volume(&self, percent: u32) {
        self.volume.store(percent.min(200), Ordering::Relaxed);
    }

    pub fn set_pan(&self, pan: i32) {
        self.pan.store(pan.clamp(-100, 100), Ordering::Relaxed);
    }

    pub fn volume(&self) -> u32 {
        self.volume.load(Ordering::Relaxed)
    }

    pub fn pan(&self) -> i32 {
        self.pan.load(Ordering::Relaxed)
    }
}

/// What the mix measured of one participant, for their strip
#[derive(Debug, Default)]
pub struct PeerMeter {
    /// Level of what is played of them, after their fader (0-100)
    level: AtomicU32,
}

impl PeerMeter {
    pub fn level(&self) -> u32 {
        self.level.load(Ordering::Relaxed)
    }
}

/// One participant in the mix
struct Voice {
    receive: ReceivePath,
    gain: Arc<PeerGain>,
    meter: Arc<PeerMeter>,
    ramp: GainRamp,
    /// Their audio taken from the play-out buffer and not yet played. Frames
    /// come in whatever length their app and the rate conversion make them
    /// (ADR-013), so what one frame of ours does not use waits for the next.
    held: Vec<f32>,
    held_len: usize,
}

struct Inner {
    voices: Box<[Mutex<Option<Voice>>]>,
    frame_samples: usize,
    sample_rate: u32,
    /// Reads that found nothing to play, over every voice
    underruns: Arc<AtomicU64>,
}

/// The mix of everyone there is an audio link to.
///
/// Cheap to clone: every clone is the same mix, so one goes to the output
/// callback and another to whoever adds and removes participants.
#[derive(Clone)]
pub struct PeerMix {
    inner: Arc<Inner>,
}

/// A participant's place in the mix. They are heard until this is dropped.
pub struct VoiceHandle {
    mix: PeerMix,
    place: usize,
}

impl Drop for VoiceHandle {
    fn drop(&mut self) {
        let voice = self.mix.inner.voices[self.place]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        // Freed here, on the thread that took it out, not on the audio callback
        drop(voice);
    }
}

impl PeerMix {
    /// A mix of up to `capacity` participants, in stereo frames of
    /// `frame_size` samples per channel. `underruns` counts the reads that
    /// found nothing to play.
    pub fn new(
        capacity: usize,
        sample_rate: u32,
        frame_size: u32,
        underruns: Arc<AtomicU64>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                voices: (0..capacity).map(|_| Mutex::new(None)).collect(),
                frame_samples: frame_size as usize * WIRE_CHANNELS,
                sample_rate,
                underruns,
            }),
        }
    }

    /// Interleaved samples in the frame [`PeerMix::mix_into`] makes. What the
    /// output device should be opened with.
    pub fn frame_samples(&self) -> usize {
        self.inner.frame_samples
    }

    /// Length of one frame, in microseconds
    pub fn frame_us(&self) -> u64 {
        (self.inner.frame_samples / WIRE_CHANNELS) as u64 * 1_000_000
            / self.inner.sample_rate.max(1) as u64
    }

    /// Puts a participant in the mix: what `receive` holds is played at
    /// `gain`, and `meter` says how. `None` when every place is taken.
    pub fn add(
        &self,
        receive: ReceivePath,
        gain: Arc<PeerGain>,
        meter: Arc<PeerMeter>,
    ) -> Option<VoiceHandle> {
        let frame_samples = self.inner.frame_samples;
        // Made before a place is held, so the callback is kept out of it only
        // for as long as putting the voice in takes
        let mut voice = Some(Voice {
            ramp: GainRamp::new(self.inner.sample_rate),
            // The widest frame their play-out buffer hands over, after what
            // is left of the one before
            held: vec![0.0; frame_samples + receive.widest_frame_samples()],
            held_len: 0,
            receive,
            gain,
            meter,
        });
        for (place, slot) in self.inner.voices.iter().enumerate() {
            let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_none() {
                *slot = voice.take();
                return Some(VoiceHandle {
                    mix: self.clone(),
                    place,
                });
            }
        }
        None
    }

    /// Fills `out` with one frame of everyone mixed, at `master_percent` of
    /// their own gains, and returns how many samples it wrote: always one
    /// frame, silent when nobody has audio, so the device keeps its cadence.
    ///
    /// `dry`, when given, gets the same frame before anyone's fader and pan
    /// are applied (recording/streaming output `B`).
    pub fn mix_into(
        &self,
        out: &mut [f32],
        master_percent: u32,
        mut dry: Option<&mut [f32]>,
    ) -> usize {
        let samples = self.inner.frame_samples.min(out.len());
        let out = &mut out[..samples];
        out.fill(0.0);
        if let Some(dry) = dry.as_deref_mut() {
            dry[..samples].fill(0.0);
        }
        let master = master_percent as f32 / 100.0;

        for slot in self.inner.voices.iter() {
            let Ok(mut slot) = slot.try_lock() else {
                continue;
            };
            let Some(voice) = slot.as_mut() else {
                continue;
            };

            while voice.held_len < samples {
                let read = voice.receive.read_into(&mut voice.held[voice.held_len..]);
                if matches!(read.result, PlayoutResult::Priming | PlayoutResult::Starved) {
                    self.inner.underruns.fetch_add(1, Ordering::Relaxed);
                }
                if read.samples == 0 {
                    voice.held[voice.held_len..samples].fill(0.0);
                    voice.held_len = samples;
                    break;
                }
                voice.held_len += read.samples;
            }

            let theirs = &mut voice.held[..samples];
            if let Some(dry) = dry.as_deref_mut() {
                for (sum, sample) in dry[..samples].iter_mut().zip(theirs.iter()) {
                    *sum += sample;
                }
            }
            pan_received(theirs, voice.receive.peer_channels(), 1.0, voice.gain.pan());
            // The fader and mute move to their new gain over a few
            // milliseconds, so muting a participant does not click.
            voice.ramp.apply(
                theirs,
                WIRE_CHANNELS,
                voice.gain.volume() as f32 / 100.0 * master,
            );
            voice
                .meter
                .level
                .store(rms_level(theirs), Ordering::Relaxed);
            for (sum, sample) in out.iter_mut().zip(theirs.iter()) {
                *sum += sample;
            }

            voice.held.copy_within(samples..voice.held_len, 0);
            voice.held_len -= samples;
        }
        samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioPreset;

    const RATE: u32 = 48_000;

    /// A PCM payload as `ReceivePath::receive` expects it
    fn pcm(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn mix(capacity: usize, frame: u32) -> PeerMix {
        PeerMix::new(capacity, RATE, frame, Arc::new(AtomicU64::new(0)))
    }

    /// A participant who sends stereo frames of `frame` samples per channel,
    /// played as soon as they arrive
    fn participant(frame: u32) -> ReceivePath {
        let receive = ReceivePath::new(AudioPreset::ZeroLatency.codec_type(), RATE, frame, 0)
            .expect("a PCM receive path");
        receive.follow_peer_channels(2);
        receive
    }

    /// A stereo frame with `left` and `right` in every sample pair
    fn frame_of(frame: u32, left: f32, right: f32) -> Vec<f32> {
        (0..frame).flat_map(|_| [left, right]).collect()
    }

    struct Heard {
        _voice: VoiceHandle,
        gain: Arc<PeerGain>,
        meter: Arc<PeerMeter>,
    }

    fn add(mix: &PeerMix, receive: &ReceivePath) -> Heard {
        let gain = Arc::new(PeerGain::default());
        let meter = Arc::new(PeerMeter::default());
        let voice = mix
            .add(receive.clone(), gain.clone(), meter.clone())
            .expect("a free place");
        Heard {
            _voice: voice,
            gain,
            meter,
        }
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    /// The bug this guards: with three in a room only one of the others was
    /// played, because the output callback read one play-out buffer.
    ///
    /// Verifies: REQ-CON-133
    #[test]
    fn two_participants_send_at_once_plays_the_sum_of_both() {
        const FRAME: u32 = 4;
        let mix = mix(9, FRAME);
        let (ben, cho) = (participant(FRAME), participant(FRAME));
        let _ben = add(&mix, &ben);
        let _cho = add(&mix, &cho);
        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.25, 0.0))));
        assert!(cho.receive(0, &pcm(&frame_of(FRAME, 0.0, 0.5))));

        let mut out = vec![9.0; mix.frame_samples() * 3];
        let samples = mix.mix_into(&mut out, 100, None);

        assert_eq!(samples, mix.frame_samples());
        assert!(
            close(&out[..samples], &frame_of(FRAME, 0.25, 0.5)),
            "{:?}",
            &out[..samples]
        );
    }

    /// Verifies: REQ-GUI-009
    #[test]
    fn each_participant_is_played_at_their_own_fader_and_the_master_scales_them_all() {
        const FRAME: u32 = 4;
        let mix = mix(9, FRAME);
        let (ben, cho) = (participant(FRAME), participant(FRAME));
        let ben_heard = add(&mix, &ben);
        let cho_heard = add(&mix, &cho);
        ben_heard.gain.set_volume(50);
        cho_heard.gain.set_volume(200);
        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.4, 0.0))));
        assert!(cho.receive(0, &pcm(&frame_of(FRAME, 0.0, 0.1))));

        let mut out = vec![0.0; mix.frame_samples()];
        let samples = mix.mix_into(&mut out, 50, None);

        assert!(
            close(&out[..samples], &frame_of(FRAME, 0.1, 0.1)),
            "{:?}",
            &out[..samples]
        );
    }

    /// `B` (recording/streaming output) must carry what the others sent, not
    /// what the listener's faders and pans turned it into.
    #[test]
    fn the_dry_frame_is_the_sum_of_what_everyone_sent_before_their_faders_and_pans() {
        const FRAME: u32 = 4;
        let mix = mix(9, FRAME);
        let (ben, cho) = (participant(FRAME), participant(FRAME));
        let ben_heard = add(&mix, &ben);
        let _cho = add(&mix, &cho);
        ben_heard.gain.set_volume(50);
        ben_heard.gain.set_pan(100);
        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.8, 0.2))));
        assert!(cho.receive(0, &pcm(&frame_of(FRAME, 0.1, 0.1))));

        let mut out = vec![0.0; mix.frame_samples()];
        let mut dry = vec![9.0; mix.frame_samples()];
        let samples = mix.mix_into(&mut out, 100, Some(&mut dry));

        assert!(
            close(&dry[..samples], &frame_of(FRAME, 0.9, 0.3)),
            "{dry:?}"
        );
        assert!(!close(&out[..samples], &dry[..samples]));
    }

    /// Muting a participant must not cut their audio off in one sample: that
    /// step is the click.
    ///
    /// Verifies: REQ-AUD-034
    #[test]
    fn a_participant_is_muted_fades_them_out_instead_of_cutting_them_and_leaves_the_others() {
        const FRAME: u32 = 480;
        let mix = mix(9, FRAME);
        let (ben, cho) = (participant(FRAME), participant(FRAME));
        let ben_heard = add(&mix, &ben);
        let _cho = add(&mix, &cho);
        let mut out = vec![0.0; mix.frame_samples()];
        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.5, 0.0))));
        assert!(cho.receive(0, &pcm(&frame_of(FRAME, 0.0, 0.5))));
        mix.mix_into(&mut out, 100, None);

        ben_heard.gain.set_volume(0);
        assert!(ben.receive(1, &pcm(&frame_of(FRAME, 0.5, 0.0))));
        assert!(cho.receive(1, &pcm(&frame_of(FRAME, 0.0, 0.5))));
        let samples = mix.mix_into(&mut out, 100, None);

        let left: Vec<f32> = out[..samples].iter().step_by(2).copied().collect();
        let right: Vec<f32> = out[..samples].iter().skip(1).step_by(2).copied().collect();
        assert!(
            left[0] > 0.45,
            "the first sample is still nearly full level"
        );
        assert!(
            left.windows(2).all(|pair| pair[1] <= pair[0]),
            "the level only goes down"
        );
        assert_eq!(left[FRAME as usize - 1], 0.0, "and it ends silent");
        assert!(
            right.iter().all(|s| (*s - 0.5).abs() < 1e-6),
            "the other one is untouched"
        );
    }

    /// A participant whose app runs a smaller buffer sends shorter frames. The
    /// mix has to take two of theirs for one of ours, not play half a frame of
    /// silence.
    ///
    /// Verifies: REQ-CON-133
    #[test]
    fn a_participant_sends_shorter_frames_than_ours_is_mixed_without_gaps() {
        const OURS: u32 = 4;
        const THEIRS: u32 = 2;
        let mix = mix(9, OURS);
        let (ben, cho) = (participant(OURS), participant(OURS));
        let _ben = add(&mix, &ben);
        let _cho = add(&mix, &cho);
        assert!(ben.receive(0, &pcm(&frame_of(OURS, 0.25, 0.25))));
        assert!(cho.receive(0, &pcm(&frame_of(THEIRS, 0.5, 0.5))));
        assert!(cho.receive(1, &pcm(&frame_of(THEIRS, 0.5, 0.5))));

        let mut out = vec![0.0; mix.frame_samples()];
        let samples = mix.mix_into(&mut out, 100, None);

        assert!(
            close(&out[..samples], &frame_of(OURS, 0.75, 0.75)),
            "{:?}",
            &out[..samples]
        );
    }

    /// A frame longer than ours, as a rate conversion makes them: what one
    /// frame of ours does not use is played with the next, not dropped.
    ///
    /// Verifies: REQ-CON-133
    #[test]
    fn a_participant_sends_longer_frames_than_ours_keeps_the_rest_for_the_next_frame() {
        const OURS: u32 = 4;
        let mix = mix(9, OURS);
        let ben = participant(OURS);
        let _ben = add(&mix, &ben);
        let long: Vec<f32> = (1..=6).flat_map(|i| [i as f32 / 10.0; 2]).collect();
        assert!(ben.receive(0, &pcm(&long)));
        assert!(ben.receive(1, &pcm(&long)));

        let mut out = vec![0.0; mix.frame_samples()];
        mix.mix_into(&mut out, 100, None);
        let first = out.clone();
        mix.mix_into(&mut out, 100, None);

        let played: Vec<f32> = first.iter().chain(&out).step_by(2).copied().collect();
        assert!(
            close(&played, &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.1, 0.2]),
            "{played:?}"
        );
    }

    /// A participant whose app runs a larger buffer sends frames several
    /// times the length of ours.
    ///
    /// Verifies: REQ-AUD-038
    #[test]
    fn a_participant_sends_frames_four_times_ours_is_played_whole_over_four_of_our_frames() {
        const OURS: u32 = 32;
        const THEIRS: u32 = 128;
        let mix = mix(9, OURS);
        let ben = participant(OURS);
        let _ben = add(&mix, &ben);
        let sent: Vec<f32> = (0..THEIRS).flat_map(|i| [i as f32 / 1000.0; 2]).collect();
        assert!(ben.receive(0, &pcm(&sent)));

        let mut out = vec![0.0; mix.frame_samples()];
        let mut played = Vec::new();
        for _ in 0..THEIRS / OURS {
            let samples = mix.mix_into(&mut out, 100, None);
            played.extend_from_slice(&out[..samples]);
        }

        assert!(close(&played, &sent), "{played:?}");
    }

    /// Verifies: REQ-CON-133
    #[test]
    fn a_participant_leaves_the_mix_plays_the_others_and_frees_their_place() {
        const FRAME: u32 = 4;
        let mix = mix(2, FRAME);
        let (ben, cho, dan) = (participant(FRAME), participant(FRAME), participant(FRAME));
        let ben_heard = add(&mix, &ben);
        let _cho = add(&mix, &cho);
        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.25, 0.25))));
        assert!(cho.receive(0, &pcm(&frame_of(FRAME, 0.5, 0.5))));

        drop(ben_heard);
        let mut out = vec![0.0; mix.frame_samples()];
        let samples = mix.mix_into(&mut out, 100, None);

        assert!(close(&out[..samples], &frame_of(FRAME, 0.5, 0.5)));
        assert!(
            mix.add(
                dan,
                Arc::new(PeerGain::default()),
                Arc::new(PeerMeter::default())
            )
            .is_some(),
            "the place of the one who left is free"
        );
    }

    /// Verifies: REQ-CON-020
    #[test]
    fn a_full_room_has_a_place_for_each_of_the_nine_others_and_no_more() {
        const FRAME: u32 = 4;
        let others = crate::network::MAX_PEERS_PER_ROOM - 1;
        let mix = mix(others, FRAME);
        let everyone: Vec<ReceivePath> = (0..others).map(|_| participant(FRAME)).collect();
        let _heard: Vec<Heard> = everyone.iter().map(|p| add(&mix, p)).collect();
        for receive in &everyone {
            assert!(receive.receive(0, &pcm(&frame_of(FRAME, 0.1, 0.1))));
        }

        let mut out = vec![0.0; mix.frame_samples()];
        let samples = mix.mix_into(&mut out, 100, None);

        assert_eq!(others, 9);
        assert!(
            close(&out[..samples], &frame_of(FRAME, 0.9, 0.9)),
            "{:?}",
            &out[..samples]
        );
        assert!(mix
            .add(
                participant(FRAME),
                Arc::new(PeerGain::default()),
                Arc::new(PeerMeter::default())
            )
            .is_none());
    }

    /// Verifies: REQ-AUD-112
    #[test]
    fn nobody_is_in_the_mix_makes_one_frame_of_silence() {
        const FRAME: u32 = 32;
        let mix = mix(9, FRAME);

        let mut out = vec![9.0; mix.frame_samples() * 3];
        let samples = mix.mix_into(&mut out, 100, None);

        assert_eq!(samples, mix.frame_samples());
        assert!(out[..samples].iter().all(|s| *s == 0.0));
    }

    /// Each strip's meter has to follow that participant, not the mix: the
    /// screen once showed every participant the same level.
    ///
    /// Verifies: REQ-GUI-009
    #[test]
    fn each_participants_meter_follows_their_own_audio() {
        const FRAME: u32 = 64;
        let mix = mix(9, FRAME);
        let (ben, cho) = (participant(FRAME), participant(FRAME));
        let ben_heard = add(&mix, &ben);
        let cho_heard = add(&mix, &cho);
        let mut out = vec![0.0; mix.frame_samples()];

        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.5, 0.5))));
        assert!(cho.receive(0, &pcm(&frame_of(FRAME, 0.1, 0.1))));
        mix.mix_into(&mut out, 100, None);
        assert_eq!((ben_heard.meter.level(), cho_heard.meter.level()), (50, 10));

        assert!(ben.receive(1, &pcm(&frame_of(FRAME, 0.0, 0.0))));
        assert!(cho.receive(1, &pcm(&frame_of(FRAME, 1.0, 1.0))));
        mix.mix_into(&mut out, 100, None);
        assert_eq!((ben_heard.meter.level(), cho_heard.meter.level()), (0, 100));
    }

    #[test]
    fn a_participant_with_nothing_to_play_counts_one_underrun_and_one_with_audio_none() {
        const FRAME: u32 = 4;
        let total = Arc::new(AtomicU64::new(0));
        let mix = PeerMix::new(9, RATE, FRAME, total.clone());
        let (ben, cho) = (participant(FRAME), participant(FRAME));
        let _ben = add(&mix, &ben);
        let _cho = add(&mix, &cho);
        assert!(ben.receive(0, &pcm(&frame_of(FRAME, 0.25, 0.25))));

        let mut out = vec![0.0; mix.frame_samples()];
        mix.mix_into(&mut out, 100, None);

        assert_eq!(total.load(Ordering::Relaxed), 1);
    }
}
