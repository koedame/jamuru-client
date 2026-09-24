//! Totals for one session, kept while it runs and written as `session_end`.

use std::time::{Duration, Instant};

use super::event::{EndReason, SessionEnd};

/// What one session has measured so far. Only totals are kept: nothing per
/// packet is stored or sent.
#[derive(Debug, Clone)]
pub struct SessionTally {
    started: Instant,
    rtt_ms: Vec<f64>,
    loss_pct_sum: f64,
    loss_pct_samples: u64,
    loss_pct_max: Option<f64>,
    fec_samples: u64,
    fec_active_samples: u64,
    reconnect_count: u32,
    xrun_count: u64,
    last_reconnect_total: u64,
    last_xrun_total: u64,
    last_fec_total: u64,
    participants: u32,
    peers_max: u32,
}

impl SessionTally {
    pub fn new() -> Self {
        Self::started_at(Instant::now())
    }

    pub(crate) fn started_at(started: Instant) -> Self {
        Self {
            started,
            rtt_ms: Vec::new(),
            loss_pct_sum: 0.0,
            loss_pct_samples: 0,
            loss_pct_max: None,
            fec_samples: 0,
            fec_active_samples: 0,
            reconnect_count: 0,
            xrun_count: 0,
            last_reconnect_total: 0,
            last_xrun_total: 0,
            last_fec_total: 0,
            participants: 0,
            peers_max: 0,
        }
    }

    /// One reading of the link: the round-trip time (`None` until the first
    /// one arrives) and the share of packets lost (0.0 to 1.0).
    pub fn sample(&mut self, rtt_ms: Option<f32>, loss_rate: f32) {
        if let Some(rtt) = rtt_ms.filter(|rtt| rtt.is_finite() && *rtt >= 0.0) {
            self.rtt_ms.push(f64::from(rtt));
        }
        if loss_rate.is_finite() {
            let pct = f64::from(loss_rate).clamp(0.0, 1.0) * 100.0;
            self.loss_pct_sum += pct;
            self.loss_pct_samples += 1;
            self.loss_pct_max = Some(self.loss_pct_max.map_or(pct, |max| max.max(pct)));
        }
    }

    /// One reading of a link that sends FEC: the total of packets FEC has
    /// rebuilt so far, from a counter that restarts from zero with the
    /// connection. The reading counts as active when that total grew since the
    /// last reading.
    pub fn sample_fec_total(&mut self, recovered_total: u64) {
        let recovered = counter_growth(&mut self.last_fec_total, recovered_total);
        self.fec_samples += 1;
        if recovered > 0 {
            self.fec_active_samples += 1;
        }
    }

    /// How many people are in the room now, the user included.
    pub fn set_participants(&mut self, participants: u32) {
        self.participants = participants;
        self.peers_max = self.peers_max.max(participants);
    }

    pub fn participant_joined(&mut self) {
        self.set_participants(self.participants.saturating_add(1));
    }

    pub fn participant_left(&mut self) {
        self.participants = self.participants.saturating_sub(1);
    }

    /// The connection dropped and began to reconnect `count` more times.
    pub fn add_reconnects(&mut self, count: u32) {
        self.reconnect_count = self.reconnect_count.saturating_add(count);
    }

    /// The audio path underran `count` more times.
    pub fn add_xruns(&mut self, count: u64) {
        self.xrun_count = self.xrun_count.saturating_add(count);
    }

    /// The reconnect total a counter that restarts from zero shows now. What
    /// was added since the last call is what counts: a total that went down
    /// means the counter restarted, so the whole of it is new.
    pub fn add_reconnects_total(&mut self, total: u64) {
        let new = counter_growth(&mut self.last_reconnect_total, total);
        self.add_reconnects(u32::try_from(new).unwrap_or(u32::MAX));
    }

    /// Like [`Self::add_reconnects_total`], for underruns.
    pub fn add_xruns_total(&mut self, total: u64) {
        let new = counter_growth(&mut self.last_xrun_total, total);
        self.add_xruns(new);
    }

    /// The `session_end` for a session that ends now.
    pub fn finish(&self, end_reason: EndReason) -> SessionEnd {
        self.finish_after(self.started.elapsed(), end_reason)
    }

    pub(crate) fn finish_after(&self, elapsed: Duration, end_reason: EndReason) -> SessionEnd {
        let mut sorted = self.rtt_ms.clone();
        sorted.sort_by(f64::total_cmp);
        SessionEnd {
            duration_s: elapsed.as_secs(),
            end_reason,
            reconnect_count: self.reconnect_count,
            peers_max: self.peers_max,
            rtt_ms_p50: percentile(&sorted, 50),
            rtt_ms_p95: percentile(&sorted, 95),
            loss_pct_mean: (self.loss_pct_samples > 0)
                .then(|| round_tenth(self.loss_pct_sum / self.loss_pct_samples as f64)),
            loss_pct_max: self.loss_pct_max.map(round_tenth),
            fec_active_pct: (self.fec_samples > 0).then(|| {
                round_tenth(self.fec_active_samples as f64 / self.fec_samples as f64 * 100.0)
            }),
            xrun_count: self.xrun_count,
        }
    }
}

impl Default for SessionTally {
    fn default() -> Self {
        Self::new()
    }
}

/// Nearest-rank percentile of an ascending list, rounded to 0.1.
fn percentile(sorted: &[f64], p: usize) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p * sorted.len()).div_ceil(100).max(1);
    Some(round_tenth(sorted[rank - 1]))
}

fn round_tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// How much a counter that may restart from zero grew since `last`.
fn counter_growth(last: &mut u64, total: u64) -> u64 {
    let growth = if total >= *last { total - *last } else { total };
    *last = total;
    growth
}
