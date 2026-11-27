//! Connection quality classification
//!
//! Turns the raw RTT and packet loss figures a connection reports into the three
//! states the UI shows. The thresholds live here rather than in the UI or in a
//! test, so there is exactly one definition of what "good" means.

use serde::{Deserialize, Serialize};

/// RTT below which a link is good, in milliseconds
const GOOD_MAX_RTT_MS: f32 = 30.0;
/// Packet loss below which a link is good, as a ratio
const GOOD_MAX_LOSS: f32 = 0.01;
/// RTT below which a link is still fair, in milliseconds
const FAIR_MAX_RTT_MS: f32 = 100.0;
/// Packet loss below which a link is still fair, as a ratio
const FAIR_MAX_LOSS: f32 = 0.05;

/// How usable a connection currently is
///
/// Both RTT and loss must be within a band to reach it: a 10ms link losing 8% of
/// packets is poor, not good.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionQuality {
    /// RTT < 30ms and loss < 1%
    Good,
    /// RTT < 100ms and loss < 5%
    Fair,
    /// RTT >= 100ms or loss >= 5%
    Poor,
}

impl ConnectionQuality {
    /// Classify a link from its RTT and packet loss ratio
    ///
    /// `packet_loss_rate` is a ratio in `0.0..=1.0`, not a percentage.
    ///
    /// Values that are not finite (an RTT that has never been measured, for
    /// instance) classify as [`ConnectionQuality::Poor`]: reporting an
    /// unmeasured link as good would be worse than reporting it as bad.
    pub fn classify(rtt_ms: f32, packet_loss_rate: f32) -> Self {
        if !rtt_ms.is_finite() || !packet_loss_rate.is_finite() {
            return ConnectionQuality::Poor;
        }

        if rtt_ms < GOOD_MAX_RTT_MS && packet_loss_rate < GOOD_MAX_LOSS {
            ConnectionQuality::Good
        } else if rtt_ms < FAIR_MAX_RTT_MS && packet_loss_rate < FAIR_MAX_LOSS {
            ConnectionQuality::Fair
        } else {
            ConnectionQuality::Poor
        }
    }

    /// Identifier used on the IPC boundary and in the UI
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionQuality::Good => "good",
            ConnectionQuality::Fair => "fair",
            ConnectionQuality::Poor => "poor",
        }
    }

    /// Whether this quality is good enough to recommend the zero-latency preset
    ///
    /// Zero-latency has no jitter protection at all, so anything below
    /// [`ConnectionQuality::Good`] would turn network variation directly into
    /// audible disruption.
    pub fn suits_zero_latency(&self) -> bool {
        *self == ConnectionQuality::Good
    }
}

/// Tracks quality over time so a change can be reported once, not every sample
///
/// The UI warns the user when quality degrades. Without this the
/// caller would have to re-warn on every stats poll.
#[derive(Debug, Default)]
pub struct QualityMonitor {
    current: Option<ConnectionQuality>,
}

/// A quality transition worth telling the user about
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityChange {
    /// Quality before the change; `None` on the first sample
    pub from: Option<ConnectionQuality>,
    /// Quality after the change
    pub to: ConnectionQuality,
}

impl QualityChange {
    /// Whether the link got worse
    ///
    /// `ConnectionQuality` orders best to worst, so a larger value is worse.
    pub fn is_degradation(&self) -> bool {
        match self.from {
            None => false,
            Some(from) => self.to > from,
        }
    }
}

impl QualityMonitor {
    /// Create a monitor that has not seen a sample yet
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a new measurement
    ///
    /// Returns `Some` only when the classification differs from the previous
    /// sample, so a caller can notify on the edge rather than on every poll.
    pub fn update(&mut self, rtt_ms: f32, packet_loss_rate: f32) -> Option<QualityChange> {
        let quality = ConnectionQuality::classify(rtt_ms, packet_loss_rate);

        if self.current == Some(quality) {
            return None;
        }

        let change = QualityChange {
            from: self.current,
            to: quality,
        };
        self.current = Some(quality);
        Some(change)
    }

    /// Most recent classification, or `None` before the first sample
    pub fn current(&self) -> Option<ConnectionQuality> {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three bands from latency.feature, including the cases where RTT is
    /// fine but loss is not.
    #[test]
    fn classification_matches_the_specified_bands() {
        // (rtt_ms, loss, expected)
        let cases = [
            (20.0, 0.005, ConnectionQuality::Good),
            (50.0, 0.02, ConnectionQuality::Fair),
            (150.0, 0.01, ConnectionQuality::Poor),
            // Good RTT, unacceptable loss: still poor.
            (20.0, 0.10, ConnectionQuality::Poor),
            // Boundaries are exclusive, so exactly at a limit drops a band.
            (30.0, 0.0, ConnectionQuality::Fair),
            (0.0, 0.01, ConnectionQuality::Fair),
            (100.0, 0.0, ConnectionQuality::Poor),
            (0.0, 0.05, ConnectionQuality::Poor),
        ];

        for (rtt, loss, expected) in cases {
            assert_eq!(
                ConnectionQuality::classify(rtt, loss),
                expected,
                "RTT {}ms with {:.1}% loss",
                rtt,
                loss * 100.0
            );
        }
    }

    /// An unmeasured link must not be reported as good.
    #[test]
    fn unmeasured_values_classify_as_poor() {
        assert_eq!(
            ConnectionQuality::classify(f32::NAN, 0.0),
            ConnectionQuality::Poor
        );
        assert_eq!(
            ConnectionQuality::classify(10.0, f32::NAN),
            ConnectionQuality::Poor
        );
        assert_eq!(
            ConnectionQuality::classify(f32::INFINITY, 0.0),
            ConnectionQuality::Poor
        );
    }

    /// Only zero-latency's own band may recommend it.
    #[test]
    fn only_good_links_suit_zero_latency() {
        assert!(ConnectionQuality::Good.suits_zero_latency());
        assert!(!ConnectionQuality::Fair.suits_zero_latency());
        assert!(!ConnectionQuality::Poor.suits_zero_latency());
    }

    /// A change must be reported once, on the edge, and degradations must be
    /// distinguishable from recoveries.
    #[test]
    fn monitor_reports_each_transition_once() {
        let mut monitor = QualityMonitor::new();
        assert_eq!(monitor.current(), None);

        // First sample is a change from nothing, and is not a degradation.
        let first = monitor.update(10.0, 0.0).expect("first sample is a change");
        assert_eq!(first.from, None);
        assert_eq!(first.to, ConnectionQuality::Good);
        assert!(!first.is_degradation());

        // Steady state reports nothing, however often it is polled.
        assert!(monitor.update(12.0, 0.001).is_none());
        assert!(monitor.update(15.0, 0.002).is_none());

        // Degradation is reported once.
        let worse = monitor.update(120.0, 0.0).expect("degradation is a change");
        assert_eq!(worse.from, Some(ConnectionQuality::Good));
        assert_eq!(worse.to, ConnectionQuality::Poor);
        assert!(worse.is_degradation());
        assert!(monitor.update(130.0, 0.0).is_none());

        // Recovery is reported, and is not a degradation.
        let better = monitor.update(20.0, 0.0).expect("recovery is a change");
        assert_eq!(better.to, ConnectionQuality::Good);
        assert!(!better.is_degradation());
        assert_eq!(monitor.current(), Some(ConnectionQuality::Good));
    }
}
