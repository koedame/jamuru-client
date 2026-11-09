//! Concealment never plays louder than full scale, however hostile the sound and the loss
//! (ADR-105). Hostile here means well-formed audio that is as hard on the predictors as audio
//! gets: full-scale square waves, DC, alternating samples and noise, with random gaps,
//! reordering and length changes.

use jamjam::audio::{encode_packed, CodecType, ReceivePath};

struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Hostile sound with random loss, reordering and length changes, at three rates: whatever is
/// concealed or played stays finite and within full scale.
///
/// Verifies: REQ-AUD-070
/// Verifies: REQ-AUD-071
#[test]
fn hostile_audio_with_random_loss_at_any_rate_plays_within_full_scale() {
    let mut rng = XorShift(0x5EED_1234);
    for rate in [48_000u32, 44_100, 96_000] {
        let path = ReceivePath::new(CodecType::Pcm, rate, 64, 4).unwrap();
        let mut out = vec![0.0f32; path.widest_frame_samples()];
        let mut seq = 0u32;
        let mut peak = 0.0f32;
        for round in 0..20_000u32 {
            let frames = [32usize, 64, 128, 243][(rng.next() % 4) as usize];
            let kind = rng.next() % 5;
            let samples: Vec<f32> = (0..frames * 2)
                .map(|i| match kind {
                    0 => {
                        if (i / 2) % 2 == 0 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    1 => 1.0,
                    2 => {
                        if (i / 7) % 2 == 0 {
                            0.9999
                        } else {
                            -1.0
                        }
                    }
                    3 => ((rng.next() >> 40) as f32 / (1u64 << 23) as f32) - 1.0,
                    _ => 0.0,
                })
                .collect();
            let mut payload = Vec::new();
            encode_packed(&samples, 2, &mut payload);
            match rng.next() % 10 {
                0 => {}                                                // lost
                1 => seq = seq.wrapping_add((rng.next() % 50) as u32), // jump
                2 => {
                    path.receive(seq.wrapping_sub(3), &payload);
                } // late
                _ => {
                    path.receive(seq, &payload);
                }
            }
            seq = seq.wrapping_add(1);
            if round % 2 == 0 {
                let read = path.read_into(&mut out);
                for s in &out[..read.samples] {
                    assert!(s.is_finite(), "non-finite sample at rate {rate}");
                    peak = peak.max(s.abs());
                }
            }
        }
        println!("rate {rate}: peak {peak}");
        assert!(peak <= 1.0, "rate {rate}: concealment reached {peak}");
    }
}
