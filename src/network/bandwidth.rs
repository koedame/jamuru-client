//! Bandwidth requirement and measurement
//!
//! Uncompressed PCM has a fixed cost, so the interesting question is not "what
//! bitrate should we pick" but "does this link carry what the peer sends".
//! Bitrate adaptation is out of scope: narrow-band links are not a target and
//! PCM's rate cannot be lowered without adding latency (ADR-022). What this
//! module does is detect and report a link that cannot carry the stream
//! (REQ-LAT-124, REQ-LAT-125, REQ-LAT-126).
//!
//! # What the verdict is made of
//!
//! Not the received rate against the preset's requirement. A peer sends
//! exactly what its preset needs and no more, so on a healthy link the rate it
//! delivers can only equal the requirement, never exceed it: measured that
//! way, every healthy link sits on the boundary and is called insufficient or
//! marginal by the accounting alone (a 5% shortfall from leaving the UDP and IP
//! headers out of the count was enough), and a peer that has muted itself and
//! sends nothing reads as a dead link. The peer's own sequence numbers say how
//! many audio packets it sent, so the verdict is the share of them that did not
//! arrive: a link too narrow for the stream drops packets, and one that
//! carries it drops none.

use std::time::{Duration, Instant};

use crate::audio::AudioPreset;

/// Bits in a byte, spelled out so the bitrate arithmetic reads clearly
const BITS_PER_BYTE: f64 = 8.0;
/// Bytes per 32-bit float sample (ADR-003: PCM f32 is the default codec)
const BYTES_PER_SAMPLE: f64 = 4.0;
/// UDP + IP header bytes per packet on IPv4: 20 + 8. What the socket counts
/// is the jamjam packet; the link carries these on top.
pub const UDP_IP_OVERHEAD_BYTES: u64 = 20 + 8;

/// UDP + IP + jamjam header bytes per packet
///
/// [`UDP_IP_OVERHEAD_BYTES`] + [`crate::protocol::HEADER_SIZE`].
const PACKET_OVERHEAD_BYTES: f64 = UDP_IP_OVERHEAD_BYTES as f64 + 12.0;

/// Share of the peer's audio packets lost, from which a link is called marginal
///
/// The same line `quality` draws between a good link and a fair one: a link
/// that loses one packet in a hundred is already dropping audio.
const MARGINAL_LOSS: f64 = 0.01;
/// Share of the peer's audio packets lost, from which a link is called
/// insufficient. The same line `quality` draws between a fair link and a poor one.
const INSUFFICIENT_LOSS: f64 = 0.05;

/// Whether the link carried what the peer sent
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BandwidthStatus {
    /// Under 1% of the peer's audio packets were lost
    Sufficient,
    /// 1% or more, under 5%, were lost: the link is close to what it can carry
    Marginal,
    /// 5% or more were lost: the link does not carry the stream
    Insufficient,
}

impl BandwidthStatus {
    /// Classify the share of the peer's audio packets that did not arrive
    /// (0.0 - 1.0)
    pub fn classify(loss: f64) -> Self {
        if loss >= INSUFFICIENT_LOSS {
            BandwidthStatus::Insufficient
        } else if loss >= MARGINAL_LOSS {
            BandwidthStatus::Marginal
        } else {
            BandwidthStatus::Sufficient
        }
    }

    /// Identifier used on the IPC boundary and in the UI
    pub fn as_str(&self) -> &'static str {
        match self {
            BandwidthStatus::Sufficient => "sufficient",
            BandwidthStatus::Marginal => "marginal",
            BandwidthStatus::Insufficient => "insufficient",
        }
    }
}

/// Identifier for the IPC boundary and UI for a sampled interval
///
/// `None` means the interval was measured and carried zero bytes, which is a
/// different situation from an actual narrow link: nothing is arriving from the
/// peer at all, so classifying it against the bandwidth requirement would call
/// a dead connection "insufficient bandwidth" (REQ-LAT-127). Callers must not
/// reuse the previous classification for a zero-byte interval, or a stale
/// "insufficient"/"marginal" verdict survives after the peer stops sending.
pub fn status_label(status: Option<BandwidthStatus>) -> &'static str {
    match status {
        Some(status) => status.as_str(),
        None => "no_signal",
    }
}

/// Bits per second a preset needs in one direction
///
/// Counts the audio payload, the per-packet header overhead, and the FEC
/// redundancy the preset adds. Both sides send, so a session needs this much
/// upstream *and* downstream.
///
/// `channels` is what goes on the wire, which is not always what the device
/// captures - the sender transmits stereo so the receiver can pan.
pub fn required_bps(preset: &AudioPreset, sample_rate: u32, channels: u16) -> f64 {
    if sample_rate == 0 || channels == 0 {
        return 0.0;
    }

    let frame_size = preset.frame_size() as f64;
    let packets_per_second = sample_rate as f64 / frame_size;
    let payload_bytes_per_packet = frame_size * channels as f64 * BYTES_PER_SAMPLE;

    let bytes_per_second = packets_per_second * (payload_bytes_per_packet + PACKET_OVERHEAD_BYTES);

    // FEC sends one extra packet per group, so it costs the same fraction of the
    // stream as its redundancy ratio.
    let with_fec = bytes_per_second * (1.0 + preset.fec_redundancy() as f64);

    with_fec * BITS_PER_BYTE
}

/// Measures the rate at which bytes actually move, and how many of the peer's
/// audio packets did not arrive
///
/// Fed from the monotonically increasing counters `ConnectionStats` reports, so
/// it works without touching the send path. Rates are computed over the interval
/// between samples rather than since connect, so a rate that drops is visible
/// immediately instead of being averaged away.
#[derive(Debug)]
pub struct BandwidthEstimator {
    last_sample: Option<(Instant, u64)>,
    current_bps: f64,
    /// Shortest interval that produces a usable rate
    min_interval: Duration,
    /// Audio packets received and lost so far, as last reported
    audio_now: (u64, u64),
    /// The same, when the interval being measured began
    audio_at_start: (u64, u64),
    /// Share of the audio packets the peer sent in the last complete interval
    /// that did not arrive, or `None` when it sent none (muted, or silent)
    interval_loss: Option<f64>,
}

impl BandwidthEstimator {
    /// Create an estimator that reports a rate once `min_interval` has elapsed
    ///
    /// Too short an interval turns packet scheduling jitter into apparent
    /// bandwidth swings.
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_sample: None,
            current_bps: 0.0,
            min_interval,
            audio_now: (0, 0),
            audio_at_start: (0, 0),
            interval_loss: None,
        }
    }

    /// Say how many audio packets have arrived and how many were lost so far
    /// (cumulative, as `ConnectionStats` counts them). Call it before
    /// [`Self::sample`] with the same reading.
    pub fn note_audio(&mut self, received: u64, lost: u64) {
        self.audio_now = (received, lost);
    }

    /// Feed a cumulative byte counter, using an explicit clock reading
    ///
    /// Returns the rate over the interval, or `None` when too little time has
    /// passed. Taking `now` as an argument keeps the estimator testable without
    /// sleeping.
    pub fn sample_at(&mut self, now: Instant, cumulative_bytes: u64) -> Option<f64> {
        match self.last_sample {
            None => {
                self.last_sample = Some((now, cumulative_bytes));
                self.audio_at_start = self.audio_now;
                None
            }
            Some((last_time, last_bytes)) => {
                let elapsed = now.duration_since(last_time);
                if elapsed < self.min_interval {
                    return None;
                }

                // A counter that went backwards means the connection restarted;
                // treat it as a fresh baseline rather than reporting a negative
                // rate.
                let delta = cumulative_bytes.saturating_sub(last_bytes);
                let bps = delta as f64 * BITS_PER_BYTE / elapsed.as_secs_f64();

                // A late packet takes its loss back, so the lost count can go
                // down: an interval never has fewer than none.
                let received = self.audio_now.0.saturating_sub(self.audio_at_start.0);
                let lost = self.audio_now.1.saturating_sub(self.audio_at_start.1);
                self.interval_loss =
                    (received + lost > 0).then(|| lost as f64 / (received + lost) as f64);
                self.audio_at_start = self.audio_now;

                self.last_sample = Some((now, cumulative_bytes));
                self.current_bps = bps;
                Some(bps)
            }
        }
    }

    /// Feed a cumulative byte counter using the current clock
    pub fn sample(&mut self, cumulative_bytes: u64) -> Option<f64> {
        self.sample_at(Instant::now(), cumulative_bytes)
    }

    /// Most recently computed rate, or 0.0 before the first interval completes
    pub fn current_bps(&self) -> f64 {
        self.current_bps
    }

    /// Whether the link carried what the peer sent in the last complete
    /// interval
    ///
    /// Returns `None` until a rate has been measured, so a caller does not warn
    /// about a link it has not observed yet, and `None` for an interval that
    /// carried no bytes at all (REQ-LAT-127). An interval in which the peer sent
    /// no audio - it has muted itself - is sufficient: there was nothing to
    /// carry, and nothing was dropped.
    pub fn status(&self) -> Option<BandwidthStatus> {
        if self.last_sample.is_none() || self.current_bps == 0.0 {
            return None;
        }
        Some(match self.interval_loss {
            Some(loss) => BandwidthStatus::classify(loss),
            None => BandwidthStatus::Sufficient,
        })
    }
}

impl Default for BandwidthEstimator {
    fn default() -> Self {
        // One second: long enough that per-packet scheduling averages out, short
        // enough that a user notices the warning while it is still true.
        Self::new(Duration::from_secs(1))
    }
}

/// How many consecutive non-sufficient intervals `BandwidthVerdict` requires
/// before reporting a warning
const CONFIRM_INTERVALS: u32 = 3;

/// How long after the first sample `BandwidthVerdict` withholds any verdict
const STARTUP_GRACE: Duration = Duration::from_secs(4);

/// Confirms a raw per-interval classification before it is safe to show a user
///
/// [`BandwidthEstimator::status`] reports exactly what one interval
/// measured, which is what the classifier and the tests need. A user-facing
/// warning needs more than that: a second in which a Wi-Fi link dropped two
/// packets in a hundred is not a link that cannot carry the stream. A peer
/// that has only just connected is worse - a test peer that replies after a
/// deliberate delay, or plain connection setup, means the first intervals
/// carry only keepalives (REQ-LAT-128). Both call for withholding judgment:
/// a single bad reading proves nothing, so `BandwidthVerdict` requires
/// [`CONFIRM_INTERVALS`] in a row before it reports one, while a sufficient
/// reading clears immediately (REQ-LAT-129).
#[derive(Debug, Default)]
pub struct BandwidthVerdict {
    connected_at: Option<Instant>,
    insufficient_streak: u32,
}

impl BandwidthVerdict {
    /// Confirm (or withhold) a raw classification observed at `now`
    ///
    /// `raw` is `None` for a zero-byte interval (REQ-LAT-127's no-signal case)
    /// and passes straight through unchanged: a dead link is never withheld or
    /// debounced. `Some(status)` is withheld during the startup grace period
    /// and must repeat for [`CONFIRM_INTERVALS`] consecutive calls before it
    /// is reported.
    pub fn confirm_at(
        &mut self,
        raw: Option<BandwidthStatus>,
        now: Instant,
    ) -> Option<BandwidthStatus> {
        let connected_at = *self.connected_at.get_or_insert(now);

        let Some(raw) = raw else {
            self.insufficient_streak = 0;
            return None;
        };

        if now.duration_since(connected_at) < STARTUP_GRACE {
            self.insufficient_streak = 0;
            return Some(BandwidthStatus::Sufficient);
        }

        if raw == BandwidthStatus::Sufficient {
            self.insufficient_streak = 0;
            return Some(raw);
        }

        self.insufficient_streak += 1;
        if self.insufficient_streak >= CONFIRM_INTERVALS {
            Some(raw)
        } else {
            Some(BandwidthStatus::Sufficient)
        }
    }

    /// Confirm (or withhold) a raw classification using the current clock
    pub fn confirm(&mut self, raw: Option<BandwidthStatus>) -> Option<BandwidthStatus> {
        self.confirm_at(raw, Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PCM's requirement is fixed by the sample rate and width, and ADR-003
    /// records it as roughly 1.5 Mbps per channel.
    ///
    /// Verifies: REQ-NET-020
    #[test]
    fn pcm_requirement_matches_the_documented_figure() {
        // Mono, no FEC: 48000 * 4 * 8 = 1.536 Mbps plus header overhead.
        let mono = required_bps(&AudioPreset::ZeroLatency, 48_000, 1);
        assert!(
            mono > 1_536_000.0,
            "{} must exceed the payload-only figure once headers count",
            mono
        );
        assert!(
            mono < 2_200_000.0,
            "{} is implausibly far above 1.5 Mbps for one channel",
            mono
        );

        // Stereo costs about twice the payload, but shares the headers.
        let stereo = required_bps(&AudioPreset::ZeroLatency, 48_000, 2);
        assert!(stereo > mono);
        assert!(stereo < mono * 2.0);

        // Doubling the sample rate doubles the requirement.
        let high_rate = required_bps(&AudioPreset::ZeroLatency, 96_000, 1);
        assert!((high_rate / mono - 2.0).abs() < 0.01);
    }

    /// FEC is extra packets, so it must show up as extra bandwidth in exactly
    /// the proportion the preset's redundancy states (ADR-021).
    ///
    /// Verifies: REQ-NET-021
    #[test]
    fn fec_costs_its_redundancy_in_bandwidth() {
        // Balanced adds a 25% FEC group; ultra-low-latency adds none.
        let no_fec = AudioPreset::UltraLowLatency;
        let with_fec = AudioPreset::Balanced;
        assert_eq!(no_fec.fec_redundancy(), 0.0);
        assert!((with_fec.fec_redundancy() - 0.25).abs() < 1e-6);

        // Compare against the same preset with its FEC cost removed, so frame
        // size does not confound the comparison.
        let balanced = required_bps(&with_fec, 48_000, 2);
        let balanced_without_fec = balanced / (1.0 + with_fec.fec_redundancy() as f64);
        assert!((balanced / balanced_without_fec - 1.25).abs() < 1e-6);
    }

    /// A smaller frame carries the same audio in more packets, so header
    /// overhead makes it cost more.
    ///
    /// Verifies: REQ-NET-022
    #[test]
    fn smaller_frames_cost_more_in_overhead() {
        // Compare presets that both have no FEC, so only the frame size differs.
        let tiny = required_bps(&AudioPreset::ZeroLatency, 48_000, 2);
        let small = required_bps(&AudioPreset::UltraLowLatency, 48_000, 2);
        assert!(
            tiny > small,
            "32-sample frames ({:.0} bps) should cost more than 64 ({:.0} bps)",
            tiny,
            small
        );
    }

    /// Verifies: REQ-NET-023
    /// Verifies: REQ-LAT-124
    /// Verifies: REQ-LAT-125
    #[test]
    fn status_classifies_the_share_of_the_peers_packets_that_were_lost() {
        assert_eq!(BandwidthStatus::classify(0.0), BandwidthStatus::Sufficient);
        assert_eq!(
            BandwidthStatus::classify(0.0099),
            BandwidthStatus::Sufficient
        );
        assert_eq!(BandwidthStatus::classify(0.01), BandwidthStatus::Marginal);
        assert_eq!(BandwidthStatus::classify(0.0499), BandwidthStatus::Marginal);
        assert_eq!(
            BandwidthStatus::classify(0.05),
            BandwidthStatus::Insufficient
        );
        assert_eq!(
            BandwidthStatus::classify(1.0),
            BandwidthStatus::Insufficient
        );
    }

    /// One interval of a stream at exactly `preset`'s requirement, on the wire,
    /// with `audio` packets received and `lost` lost in it. The estimator
    /// starts at `start`, so the interval ends a second later.
    fn one_interval(
        estimator: &mut BandwidthEstimator,
        start: Instant,
        second: u64,
        bytes_per_second: u64,
        audio: (u64, u64),
    ) {
        let (received, lost) = audio;
        estimator.note_audio(received * (second + 1), lost * (second + 1));
        estimator.sample_at(
            start + Duration::from_secs(second + 1),
            bytes_per_second * (second + 1),
        );
    }

    /// A healthy link delivers exactly what the peer sends, which is exactly
    /// the preset's requirement and never more. Measured against the
    /// requirement that is the boundary, so the old verdict read every healthy
    /// link as insufficient or marginal; measured by what was lost it reads as
    /// sufficient.
    ///
    /// Verifies: REQ-LAT-124
    /// Verifies: REQ-LAT-125
    #[test]
    fn a_link_that_delivers_exactly_what_the_peer_sends_is_sufficient() {
        let preset = AudioPreset::UltraLowLatency;
        let required_bytes = (required_bps(&preset, 48_000, 2) / 8.0) as u64;
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        estimator.sample_at(start, 0);

        // 750 packets a second, none lost, at exactly the requirement.
        one_interval(&mut estimator, start, 0, required_bytes, (750, 0));

        assert_eq!(estimator.status(), Some(BandwidthStatus::Sufficient));
        let measured = estimator.current_bps();
        assert!(
            (measured / required_bps(&preset, 48_000, 2) - 1.0).abs() < 0.01,
            "the rate is the requirement, {} bps",
            measured
        );
    }

    /// A peer that has muted itself sends no audio, only keepalives. That is
    /// not a link too narrow for the stream.
    ///
    /// Verifies: REQ-LAT-131
    #[test]
    fn a_peer_that_sends_no_audio_is_not_a_narrow_link() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        estimator.sample_at(start, 0);

        // Two keepalives and a ping a second.
        one_interval(&mut estimator, start, 0, 3 * 52, (0, 0));

        assert_eq!(estimator.status(), Some(BandwidthStatus::Sufficient));
    }

    /// Packets the peer's sequence numbers show as missing decide the verdict,
    /// however many bytes arrived.
    ///
    /// Verifies: REQ-LAT-124
    /// Verifies: REQ-LAT-125
    #[test]
    fn a_link_that_drops_the_peers_packets_is_marginal_and_then_insufficient() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        estimator.sample_at(start, 0);

        one_interval(&mut estimator, start, 0, 400_000, (740, 10));
        assert_eq!(estimator.status(), Some(BandwidthStatus::Marginal));

        one_interval(&mut estimator, start, 1, 400_000, (740, 40));
        assert_eq!(estimator.status(), Some(BandwidthStatus::Insufficient));
    }

    /// The verdict is about the last interval. A link that lost packets and
    /// recovered is sufficient again.
    ///
    /// Verifies: REQ-LAT-124
    #[test]
    fn the_verdict_follows_the_last_interval_and_not_the_whole_session() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        estimator.sample_at(start, 0);

        estimator.note_audio(700, 50);
        estimator.sample_at(start + Duration::from_secs(1), 400_000);
        assert_eq!(estimator.status(), Some(BandwidthStatus::Insufficient));

        // The next second: 750 more arrived, none more were lost.
        estimator.note_audio(1_450, 50);
        estimator.sample_at(start + Duration::from_secs(2), 800_000);
        assert_eq!(estimator.status(), Some(BandwidthStatus::Sufficient));
    }

    /// A late packet takes its loss back, so the cumulative count of lost
    /// packets can go down. That must not read as anything but a clean
    /// interval.
    ///
    /// Verifies: REQ-LAT-124
    #[test]
    fn a_packet_that_takes_its_loss_back_does_not_break_the_interval() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        estimator.note_audio(0, 5);
        estimator.sample_at(start, 0);

        estimator.note_audio(750, 4);
        estimator.sample_at(start + Duration::from_secs(1), 400_000);

        assert_eq!(estimator.status(), Some(BandwidthStatus::Sufficient));
    }

    /// The estimator must report the rate over the interval, ignore samples that
    /// arrive too soon, and survive a counter reset.
    ///
    /// Verifies: REQ-NET-024
    #[test]
    fn estimator_measures_the_interval_rate() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));

        // First sample only establishes a baseline.
        assert_eq!(estimator.sample_at(start, 0), None);
        assert_eq!(estimator.current_bps(), 0.0);

        // Too soon: no rate, and the baseline is untouched.
        assert_eq!(
            estimator.sample_at(start + Duration::from_millis(500), 100_000),
            None
        );

        // 125_000 bytes in one second is 1 Mbps.
        let bps = estimator
            .sample_at(start + Duration::from_secs(1), 125_000)
            .expect("a full interval produces a rate");
        assert!((bps - 1_000_000.0).abs() < 1.0, "got {} bps", bps);

        // Rates come from the interval, not from the start: a quiet second
        // reports a low rate even though the total is large.
        let idle = estimator
            .sample_at(start + Duration::from_secs(2), 125_000)
            .expect("rate for the second interval");
        assert_eq!(idle, 0.0);

        // A counter that went backwards is a reconnect, not a negative rate.
        let after_reset = estimator
            .sample_at(start + Duration::from_secs(3), 10)
            .expect("rate after reset");
        assert!(after_reset >= 0.0, "got {} bps", after_reset);
    }

    /// Verifies: REQ-LAT-127
    #[test]
    fn status_label_when_the_interval_carried_zero_bytes_reports_no_signal_not_the_last_verdict() {
        assert_eq!(status_label(None), "no_signal");
    }

    /// Verifies: REQ-LAT-127
    #[test]
    fn status_label_when_a_rate_was_measured_reports_the_classification() {
        assert_eq!(
            status_label(Some(BandwidthStatus::Insufficient)),
            "insufficient"
        );
        assert_eq!(status_label(Some(BandwidthStatus::Marginal)), "marginal");
        assert_eq!(
            status_label(Some(BandwidthStatus::Sufficient)),
            "sufficient"
        );
    }

    /// Verifies: REQ-NET-025
    /// Verifies: REQ-LAT-126
    #[test]
    fn status_is_absent_until_measured() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));

        assert_eq!(estimator.status(), None);

        // The baseline alone measures nothing.
        estimator.sample_at(start, 0);
        assert_eq!(estimator.status(), None);

        // An interval that carried bytes has a verdict.
        estimator.sample_at(start + Duration::from_secs(1), 10_000_000);
        assert_eq!(estimator.status(), Some(BandwidthStatus::Sufficient));
    }

    /// An interval with no bytes at all is a dead link, not a verdict.
    ///
    /// Verifies: REQ-LAT-127
    #[test]
    fn an_interval_that_carried_nothing_has_no_verdict() {
        let start = Instant::now();
        let mut estimator = BandwidthEstimator::new(Duration::from_secs(1));
        estimator.sample_at(start, 100_000);

        estimator.sample_at(start + Duration::from_secs(1), 100_000);

        assert_eq!(estimator.status(), None);
    }

    /// Verifies: REQ-LAT-128
    #[test]
    fn verdict_withholds_judgment_during_the_startup_grace_period() {
        let start = Instant::now();
        let mut verdict = BandwidthVerdict::default();

        // Even a link that reads as insufficient on every interval must not be
        // reported while still inside the grace period - a peer that has only
        // just connected (a test peer that replies after a deliberate delay,
        // or plain connection setup) has not really shown what it can carry.
        for elapsed_secs in 0..3 {
            assert_eq!(
                verdict.confirm_at(
                    Some(BandwidthStatus::Insufficient),
                    start + Duration::from_secs(elapsed_secs)
                ),
                Some(BandwidthStatus::Sufficient),
                "reported a verdict {elapsed_secs}s after connecting, inside the grace period"
            );
        }
    }

    /// Verifies: REQ-LAT-129
    #[test]
    fn verdict_requires_consecutive_confirmation_before_reporting_insufficient() {
        let start = Instant::now();
        let mut verdict = BandwidthVerdict::default();

        // Clear the grace period with one confirmed-sufficient reading.
        verdict.confirm_at(Some(BandwidthStatus::Sufficient), start);

        let after_grace = start + STARTUP_GRACE;
        // The first two insufficient readings in a row are not yet reported.
        for i in 0..CONFIRM_INTERVALS - 1 {
            assert_eq!(
                verdict.confirm_at(
                    Some(BandwidthStatus::Insufficient),
                    after_grace + Duration::from_secs(i as u64)
                ),
                Some(BandwidthStatus::Sufficient),
                "reported before {} consecutive readings confirmed it",
                CONFIRM_INTERVALS
            );
        }

        // The CONFIRM_INTERVALS-th consecutive insufficient reading is reported.
        assert_eq!(
            verdict.confirm_at(
                Some(BandwidthStatus::Insufficient),
                after_grace + Duration::from_secs((CONFIRM_INTERVALS - 1) as u64)
            ),
            Some(BandwidthStatus::Insufficient)
        );

        // A single sufficient reading clears it immediately, no debounce needed.
        assert_eq!(
            verdict.confirm_at(
                Some(BandwidthStatus::Sufficient),
                after_grace + Duration::from_secs(CONFIRM_INTERVALS as u64)
            ),
            Some(BandwidthStatus::Sufficient)
        );
    }

    /// Verifies: REQ-LAT-127
    /// Verifies: REQ-LAT-129
    #[test]
    fn verdict_passes_no_signal_through_unconfirmed_and_resets_the_streak() {
        let start = Instant::now();
        let mut verdict = BandwidthVerdict::default();
        verdict.confirm_at(Some(BandwidthStatus::Sufficient), start);
        let after_grace = start + STARTUP_GRACE;

        // Two insufficient readings build up a streak, short of CONFIRM_INTERVALS.
        verdict.confirm_at(Some(BandwidthStatus::Insufficient), after_grace);
        verdict.confirm_at(
            Some(BandwidthStatus::Insufficient),
            after_grace + Duration::from_secs(1),
        );

        // A dead link must be reported as no_signal even mid-streak, not folded
        // into the insufficient-bandwidth debounce.
        assert_eq!(
            verdict.confirm_at(None, after_grace + Duration::from_secs(2)),
            None
        );

        // The streak reset by the no_signal interval means the next
        // insufficient readings must reconfirm from scratch.
        for i in 0..CONFIRM_INTERVALS - 1 {
            assert_eq!(
                verdict.confirm_at(
                    Some(BandwidthStatus::Insufficient),
                    after_grace + Duration::from_secs(3 + i as u64)
                ),
                Some(BandwidthStatus::Sufficient),
                "streak was not reset by the intervening no_signal interval"
            );
        }
    }
}
