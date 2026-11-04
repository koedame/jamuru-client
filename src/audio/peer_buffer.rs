//! The receive buffer of each participant: the one its owner sets by hand, and
//! the one another participant asks to be longer (ADR-097).
//!
//! Every link has a play-out buffer of its own (`ReceivePath`), so what holds
//! for one participant holds for that participant only. Two things move it:
//!
//! - **The owner's choice.** [`Own::Preset`] is the buffer the preset asks for,
//!   adapting to the link (ADR-031). [`Own::Fixed`] is a delay the owner typed:
//!   it is held, and adaptation leaves it alone.
//! - **A participant's ask**, once the owner accepted it: [`Terms`]. It is a
//!   floor under the delay, and the longer of the two is held. A participant
//!   whose own uplink is the one that is shaky knows better than the listener
//!   that the listener's buffer for them should be longer.
//!
//! [`PeerBufferControl`] is what the screen and the room's messages write and
//! what the audio thread reads; [`Driver`] is the audio thread's side of it. The
//! delay is counted in the packets of the participant, so every length here is
//! milliseconds in tenths on the outside and converted with the length of one
//! of their packets on the inside.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::playout::Grant;
use super::stream::ReceivePath;

/// The longest buffer that can be set or asked for, in tenths of a millisecond.
pub const MAX_BUFFER_MS10: u16 = 850;

/// The most missing frames a second that an ask can allow, in tenths.
pub const MAX_TOLERANCE_TENTHS: u8 = 100;

/// How far an automatic ask raises the buffer at a time, in tenths of a
/// millisecond.
pub const AUTO_STEP_MS10: u32 = 50;

/// The shortest an automatic ask comes down to, in samples.
pub const AUTO_LOWEST_SAMPLES: u32 = 48;

/// The shortest packet a participant sends, in samples. The ring is made for
/// [`MAX_BUFFER_MS10`] of them.
const SHORTEST_PACKET_SAMPLES: u32 = 32;

/// How many packets the longest buffer is, for packets of the shortest length at
/// `sample_rate`: what the ring has to hold.
pub fn ring_frames(sample_rate: u32) -> u32 {
    let samples = sample_rate as u64 * u64::from(MAX_BUFFER_MS10) / 10_000;
    samples.div_ceil(u64::from(SHORTEST_PACKET_SAMPLES)) as u32
}

/// What the owner chose for a participant's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Own {
    /// The buffer the preset asks for, adapting to the link
    #[default]
    Preset,
    /// This many tenths of a millisecond, held
    Fixed { ms10: u16 },
}

impl Own {
    /// `ms10` as the choice it is, or why it is not one.
    pub fn fixed(ms10: u32) -> Result<Self, String> {
        match u16::try_from(ms10) {
            Ok(ms10) if ms10 <= MAX_BUFFER_MS10 => Ok(Own::Fixed { ms10 }),
            _ => Err(format!(
                "The buffer is at most {} ms",
                MAX_BUFFER_MS10 as f32 / 10.0
            )),
        }
    }
}

/// What a participant asks of the buffer that plays their audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terms {
    /// The longest the buffer may be made, in tenths of a millisecond
    pub max_ms10: u16,
    /// How many frames a second may go missing before an automatic ask raises the
    /// buffer, in tenths
    pub tolerance_tenths: u8,
    /// Whether the buffer follows the link up to `max_ms10`; otherwise it is
    /// `max_ms10`
    pub auto: bool,
}

impl Terms {
    /// Whether the terms are within what an ask may say.
    pub fn is_valid(&self) -> bool {
        self.max_ms10 > 0
            && self.max_ms10 <= MAX_BUFFER_MS10
            && self.tolerance_tenths <= MAX_TOLERANCE_TENTHS
    }
}

/// How an ask stands on the audio, for the reply to the participant who made it
/// and for the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrantView {
    /// The buffer the ask holds, in tenths of a millisecond
    pub applied_ms10: u16,
    /// The buffer is not the one asked for, because it counts in packets
    pub rounded: bool,
    /// An automatic ask has come down to its lowest
    pub at_lowest: bool,
    /// An automatic ask is following the link
    pub auto_running: bool,
}

/// What the audio thread publishes about a buffer.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct View {
    /// The buffer in force, in tenths of a millisecond
    pub delay_ms10: u32,
    /// How an accepted ask stands
    pub granted: Option<GrantView>,
}

struct Inner {
    own: Own,
    grant: Option<Terms>,
    /// Length of one packet of the participant as the audio thread last knew it, in ms; 0 before
    packet_ms: f32,
    /// What the audio thread last published
    delay_frames: u32,
    floor_frames: u32,
    cap_frames: u32,
    lowest_frames: u32,
}

/// The buffer of one participant as the screen, the room's messages and the
/// audio thread share it.
pub struct PeerBufferControl {
    revision: AtomicU32,
    inner: Mutex<Inner>,
}

impl Default for PeerBufferControl {
    fn default() -> Self {
        Self {
            revision: AtomicU32::new(0),
            inner: Mutex::new(Inner {
                own: Own::Preset,
                grant: None,
                packet_ms: 0.0,
                delay_frames: 0,
                floor_frames: 0,
                cap_frames: 0,
                lowest_frames: 0,
            }),
        }
    }
}

fn frames_of(ms10: u32, packet_ms: f32) -> u32 {
    if packet_ms <= 0.0 {
        return 0;
    }
    (ms10 as f32 / 10.0 / packet_ms).round() as u32
}

/// The most whole packets within `ms10`, at least one: an ask is never given
/// more than it said.
fn cap_frames_of(ms10: u32, packet_ms: f32) -> u32 {
    if packet_ms <= 0.0 {
        return 0;
    }
    ((ms10 as f32 / 10.0 / packet_ms).floor() as u32).max(1)
}

fn ms10_of(frames: u32, packet_ms: f32) -> u32 {
    (frames as f32 * packet_ms * 10.0).round() as u32
}

impl Inner {
    fn locked(&self) -> bool {
        false
    }
}

impl PeerBufferControl {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        debug_assert!(!inner.locked());
        inner
    }

    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    /// The owner's choice.
    pub fn own(&self) -> Own {
        self.lock().own
    }

    /// Sets the owner's choice.
    pub fn set_own(&self, own: Own) {
        let mut inner = self.lock();
        if inner.own != own {
            inner.own = own;
            drop(inner);
            self.changed();
        }
    }

    /// The ask that was accepted, if one is.
    pub fn grant(&self) -> Option<Terms> {
        self.lock().grant
    }

    /// Holds the buffer to what a participant asked, or lets go of it. Returns
    /// how the ask stands, as far as it can be told before the audio thread
    /// has followed.
    pub fn set_grant(&self, terms: Option<Terms>) -> Option<GrantView> {
        let mut inner = self.lock();
        if inner.grant != terms {
            if let Some(terms) = terms {
                // What the audio thread will start from, so that the answer
                // to the ask does not wait for it
                let cap = cap_frames_of(u32::from(terms.max_ms10), inner.packet_ms);
                let lowest = lowest_frames(inner.packet_ms, 0);
                inner.cap_frames = cap;
                inner.lowest_frames = lowest;
                inner.floor_frames = if terms.auto { lowest.min(cap) } else { cap };
            } else {
                inner.floor_frames = 0;
            }
            inner.grant = terms;
            let view = grant_view(&inner);
            drop(inner);
            self.changed();
            return view;
        }
        grant_view(&inner)
    }

    /// How the buffer stands.
    pub fn view(&self) -> View {
        let inner = self.lock();
        View {
            delay_ms10: ms10_of(inner.delay_frames, inner.packet_ms),
            granted: grant_view(&inner),
        }
    }

    fn revision(&self) -> u32 {
        self.revision.load(Ordering::SeqCst)
    }
}

/// The shortest an automatic ask comes down to, in packets of `packet_ms` at
/// `sample_rate` (0: at 48 kHz).
fn lowest_frames(packet_ms: f32, sample_rate: u32) -> u32 {
    let rate = if sample_rate == 0 {
        48_000
    } else {
        sample_rate
    };
    let lowest_ms = AUTO_LOWEST_SAMPLES as f32 * 1000.0 / rate as f32;
    if packet_ms <= 0.0 {
        return 1;
    }
    ((lowest_ms / packet_ms).ceil() as u32).max(1)
}

fn grant_view(inner: &Inner) -> Option<GrantView> {
    let terms = inner.grant?;
    if inner.packet_ms <= 0.0 {
        return Some(GrantView {
            applied_ms10: if terms.auto { 0 } else { terms.max_ms10 },
            rounded: false,
            at_lowest: terms.auto,
            auto_running: terms.auto,
        });
    }
    let applied = ms10_of(inner.floor_frames, inner.packet_ms).min(u32::from(u16::MAX)) as u16;
    Some(GrantView {
        applied_ms10: applied,
        rounded: !terms.auto && applied != terms.max_ms10,
        at_lowest: terms.auto && inner.floor_frames <= inner.lowest_frames,
        auto_running: terms.auto && inner.floor_frames < inner.cap_frames,
    })
}

/// The audio thread's side of a [`PeerBufferControl`]: carries what was set to
/// the buffer of the link, and says what the buffer came to.
#[derive(Default)]
pub struct Driver {
    revision: Option<u32>,
    packet_us: u32,
}

impl Driver {
    /// Follows `control` with `receive`, whose packets last `packet_ms`.
    /// Call it often; it does work only when something was set or the packets
    /// changed, and publishes when `publish` says the time has come.
    pub fn drive(
        &mut self,
        control: &PeerBufferControl,
        receive: &ReceivePath,
        packet_ms: f32,
        publish: bool,
    ) {
        let revision = control.revision();
        let packet_us = (packet_ms * 1000.0) as u32;
        let changed = self.revision != Some(revision) || self.packet_us != packet_us;
        if !changed && !publish {
            return;
        }
        let sample_rate = receive.sample_rate();
        let mut inner = control.lock();
        inner.packet_ms = packet_ms;
        if changed {
            self.revision = Some(revision);
            self.packet_us = packet_us;
            match inner.own {
                Own::Preset => receive.unpin_delay(),
                Own::Fixed { ms10 } => {
                    receive.pin_delay_frames(frames_of(u32::from(ms10), packet_ms));
                }
            }
            match inner.grant {
                None => {
                    receive.set_grant(None);
                    inner.floor_frames = 0;
                }
                Some(terms) => {
                    let cap = cap_frames_of(u32::from(terms.max_ms10), packet_ms);
                    let lowest = lowest_frames(packet_ms, sample_rate);
                    inner.cap_frames = cap;
                    inner.lowest_frames = lowest;
                    inner.floor_frames = receive.set_grant(Some(Grant {
                        cap_frames: cap,
                        step_frames: frames_of(AUTO_STEP_MS10, packet_ms).max(1),
                        min_frames: lowest,
                        tolerance_tenths: u32::from(terms.tolerance_tenths),
                        auto: terms.auto,
                    }));
                }
            }
        }
        if inner.grant.is_some() {
            inner.floor_frames = receive.floor_frames();
        }
        inner.delay_frames = receive.delay_frames();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioPreset, ReceivePath};

    const RATE: u32 = 48_000;
    const FRAME: u32 = 64;
    /// One packet of 64 samples at 48 kHz, in ms
    const PACKET_MS: f32 = 64.0 / 48.0;

    fn receive(delay_frames: u32) -> ReceivePath {
        ReceivePath::new(
            AudioPreset::Balanced.codec_type(),
            RATE,
            FRAME,
            delay_frames,
        )
        .unwrap()
    }

    /// Verifies: REQ-LAT-031
    #[test]
    fn the_ring_holds_the_longest_buffer_in_the_shortest_packets() {
        // 85 ms at 48 kHz is 4080 samples, 127.5 packets of 32
        assert_eq!(ring_frames(48_000), 128);
        assert_eq!(ring_frames(96_000), 255);
        assert_eq!(ring_frames(44_100), 118);
    }

    /// Verifies: REQ-LAT-031
    #[test]
    fn a_buffer_longer_than_eighty_five_milliseconds_is_not_a_choice() {
        assert_eq!(Own::fixed(850), Ok(Own::Fixed { ms10: 850 }));
        assert!(Own::fixed(851).is_err());
        assert!(Own::fixed(70_000).is_err());
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn an_ask_is_within_what_it_may_say_or_it_is_not_one() {
        let ask = |max_ms10, tolerance_tenths| Terms {
            max_ms10,
            tolerance_tenths,
            auto: false,
        };

        assert!(ask(850, 100).is_valid());
        assert!(ask(1, 0).is_valid());
        assert!(!ask(0, 10).is_valid(), "no buffer at all is not an ask");
        assert!(!ask(851, 10).is_valid());
        assert!(!ask(100, 101).is_valid());
    }

    /// Verifies: REQ-LAT-031
    #[test]
    fn a_fixed_buffer_is_held_in_whole_packets_and_the_choice_goes_back_to_the_preset() {
        let (control, receive, mut driver) =
            (PeerBufferControl::default(), receive(4), Driver::default());

        control.set_own(Own::Fixed { ms10: 200 });
        driver.drive(&control, &receive, PACKET_MS, true);
        // 20 ms is 15 packets of 1.33 ms
        assert_eq!(receive.delay_frames(), 15);
        assert_eq!(control.view().delay_ms10, 200);

        control.set_own(Own::Preset);
        driver.drive(&control, &receive, PACKET_MS, true);
        assert_eq!(receive.delay_frames(), 4, "the preset's delay is back");
    }

    /// Verifies: REQ-LAT-032
    #[test]
    fn an_ask_that_is_accepted_holds_the_buffer_and_letting_go_gives_it_back() {
        let (control, receive, mut driver) =
            (PeerBufferControl::default(), receive(4), Driver::default());
        driver.drive(&control, &receive, PACKET_MS, true);

        let view = control
            .set_grant(Some(Terms {
                max_ms10: 300,
                tolerance_tenths: 10,
                auto: false,
            }))
            .unwrap();
        assert_eq!(
            view.applied_ms10, 293,
            "the answer needs no wait for the audio thread"
        );
        driver.drive(&control, &receive, PACKET_MS, true);
        assert_eq!(
            receive.delay_frames(),
            22,
            "30 ms is 22.5 packets, taken as 22"
        );
        let granted = control.view().granted.unwrap();
        assert_eq!(granted.applied_ms10, 293, "22 packets");
        assert!(granted.rounded, "the packets do not divide the 30 ms");

        assert_eq!(control.set_grant(None), None);
        driver.drive(&control, &receive, PACKET_MS, true);
        assert_eq!(receive.delay_frames(), 4);
        assert_eq!(control.view().granted, None);
    }

    /// Verifies: REQ-LAT-032
    #[test]
    fn an_automatic_ask_starts_at_its_lowest_and_says_it_follows_the_link() {
        let (control, receive, mut driver) =
            (PeerBufferControl::default(), receive(4), Driver::default());
        driver.drive(&control, &receive, PACKET_MS, true);

        control.set_grant(Some(Terms {
            max_ms10: 400,
            tolerance_tenths: 10,
            auto: true,
        }));
        driver.drive(&control, &receive, PACKET_MS, true);

        let granted = control.view().granted.unwrap();
        // 48 samples at 48 kHz are 1 ms: one packet of 1.33 ms
        assert_eq!(granted.applied_ms10, 13);
        assert!(granted.at_lowest && granted.auto_running);
        assert_eq!(
            receive.delay_frames(),
            4,
            "the preset's own delay is longer"
        );
    }

    /// Verifies: REQ-LAT-030
    #[test]
    fn what_is_set_for_one_participant_leaves_the_buffer_of_every_other_as_it_was() {
        let controls: Vec<_> = (0..3).map(|_| PeerBufferControl::default()).collect();
        let receives: Vec<_> = (0..3).map(|_| receive(4)).collect();
        let mut drivers: Vec<_> = (0..3).map(|_| Driver::default()).collect();
        let delays = |receives: &[ReceivePath]| -> Vec<u32> {
            receives.iter().map(ReceivePath::delay_frames).collect()
        };
        for ((control, receive), driver) in controls.iter().zip(&receives).zip(&mut drivers) {
            driver.drive(control, receive, PACKET_MS, true);
        }
        assert_eq!(delays(&receives), [4, 4, 4]);

        controls[1].set_grant(Some(Terms {
            max_ms10: 400,
            tolerance_tenths: 10,
            auto: false,
        }));
        for ((control, receive), driver) in controls.iter().zip(&receives).zip(&mut drivers) {
            driver.drive(control, receive, PACKET_MS, true);
        }
        assert_eq!(delays(&receives), [4, 30, 4], "only the one asked about");

        controls[1].set_grant(None);
        controls[2].set_own(Own::Fixed { ms10: 100 });
        for ((control, receive), driver) in controls.iter().zip(&receives).zip(&mut drivers) {
            driver.drive(control, receive, PACKET_MS, true);
        }
        assert_eq!(
            delays(&receives),
            [4, 4, 8],
            "back, and only the other one set"
        );
    }

    /// Verifies: REQ-LAT-032
    #[test]
    fn an_ask_stays_when_the_packets_of_the_participant_turn_out_longer_than_assumed() {
        let (control, receive, mut driver) =
            (PeerBufferControl::default(), receive(4), Driver::default());
        control.set_grant(Some(Terms {
            max_ms10: 200,
            tolerance_tenths: 10,
            auto: false,
        }));
        driver.drive(&control, &receive, PACKET_MS, true);
        assert_eq!(receive.delay_frames(), 15);

        // Their packets are 128 samples: the same 20 ms is half the packets,
        // and not more than 20 ms of them
        driver.drive(&control, &receive, 128.0 / 48.0, true);

        assert_eq!(receive.delay_frames(), 7);
    }
}
