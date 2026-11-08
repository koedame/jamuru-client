//! Totals for one session, kept while it runs and written as `session_end`.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::network::{AddressCandidate, CandidateType, LinkSnapshot};

use super::event::{EndReason, SessionEnd};

/// How many of the user's own addresses one session line carries at most.
const MAX_LOCAL_IPS: usize = 16;

/// What one session has measured so far. Only totals are kept: nothing per
/// packet is stored or sent.
#[derive(Debug, Clone)]
pub struct SessionTally {
    started: Instant,
    /// Round-trip readings counted by value in tenths of a millisecond, so
    /// what is held grows with the number of distinct values, not with how
    /// long the session runs.
    rtt_tenths: BTreeMap<u32, u64>,
    rtt_samples: u64,
    loss_pct_sum: f64,
    loss_pct_samples: u64,
    loss_pct_max: Option<f64>,
    fec_samples: u64,
    fec_active_samples: u64,
    reconnect_count: u32,
    xrun_count: u64,
    last_reconnect_total: u64,
    last_xrun_total: u64,
    last_giveup_total: u64,
    last_fec_total: u64,
    participants: u32,
    peers_max: u32,
    link: LinkSnapshot,
    local_ips: Vec<IpAddr>,
    public_ip: Option<IpAddr>,
}

impl SessionTally {
    pub fn new() -> Self {
        Self::started_at(Instant::now())
    }

    pub(crate) fn started_at(started: Instant) -> Self {
        Self {
            started,
            rtt_tenths: BTreeMap::new(),
            rtt_samples: 0,
            loss_pct_sum: 0.0,
            loss_pct_samples: 0,
            loss_pct_max: None,
            fec_samples: 0,
            fec_active_samples: 0,
            reconnect_count: 0,
            xrun_count: 0,
            last_reconnect_total: 0,
            last_xrun_total: 0,
            last_giveup_total: 0,
            last_fec_total: 0,
            participants: 0,
            peers_max: 0,
            link: LinkSnapshot::default(),
            local_ips: Vec::new(),
            public_ip: None,
        }
    }

    /// One reading of the link: the round-trip time (`None` until the first
    /// one arrives) and the share of packets lost (0.0 to 1.0).
    pub fn sample(&mut self, rtt_ms: Option<f32>, loss_rate: f32) {
        if let Some(rtt) = rtt_ms.filter(|rtt| rtt.is_finite() && *rtt >= 0.0) {
            // Saturates at `u32::MAX` tenths (about five days), far beyond any
            // round trip a link reports.
            let tenths = (f64::from(rtt) * 10.0).round() as u32;
            *self.rtt_tenths.entry(tenths).or_insert(0) += 1;
            self.rtt_samples += 1;
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

    /// What is known about how the link to the peer came up. A reading with
    /// nothing measured (no link yet) leaves the last real one in place, so a
    /// link that was up and then torn down is still described at the end.
    pub fn set_link(&mut self, link: LinkSnapshot) {
        if !link.is_empty() {
            self.link = link;
        }
    }

    /// The addresses the user offered the peer: their own addresses on their
    /// networks (host candidates) and the public one a STUN server saw
    /// (server-reflexive). Only the addresses are kept, not the ports.
    pub fn set_local_candidates(&mut self, candidates: &[AddressCandidate]) {
        self.local_ips.clear();
        self.public_ip = None;
        for candidate in candidates {
            let ip = candidate.address.ip();
            match candidate.candidate_type {
                CandidateType::Host => {
                    if !self.local_ips.contains(&ip) && self.local_ips.len() < MAX_LOCAL_IPS {
                        self.local_ips.push(ip);
                    }
                }
                CandidateType::ServerReflexive => {
                    self.public_ip.get_or_insert(ip);
                }
            }
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

    /// Like [`Self::add_reconnects_total`], for the times an established link
    /// was given up for silence. Returns how many are new since the last call.
    pub fn new_silence_giveups(&mut self, total: u64) -> u64 {
        counter_growth(&mut self.last_giveup_total, total)
    }

    /// Like [`Self::add_reconnects_total`], for underruns.
    pub fn add_xruns_total(&mut self, total: u64) {
        let new = counter_growth(&mut self.last_xrun_total, total);
        self.add_xruns(new);
    }

    /// Nearest-rank percentile of the round-trip readings, in milliseconds to
    /// 0.1.
    fn rtt_percentile(&self, p: u64) -> Option<f64> {
        if self.rtt_samples == 0 {
            return None;
        }
        let rank = (p * self.rtt_samples).div_ceil(100).max(1);
        let mut seen = 0;
        for (&tenths, &count) in &self.rtt_tenths {
            seen += count;
            if seen >= rank {
                return Some(f64::from(tenths) / 10.0);
            }
        }
        None
    }

    /// The `session_end` for a session that ends now.
    pub fn finish(&self, end_reason: EndReason) -> SessionEnd {
        self.finish_after(self.started.elapsed(), end_reason)
    }

    pub(crate) fn finish_after(&self, elapsed: Duration, end_reason: EndReason) -> SessionEnd {
        SessionEnd {
            duration_s: elapsed.as_secs(),
            end_reason,
            reconnect_count: self.reconnect_count,
            peers_max: self.peers_max,
            rtt_ms_p50: self.rtt_percentile(50),
            rtt_ms_p95: self.rtt_percentile(95),
            loss_pct_mean: (self.loss_pct_samples > 0)
                .then(|| round_tenth(self.loss_pct_sum / self.loss_pct_samples as f64)),
            loss_pct_max: self.loss_pct_max.map(round_tenth),
            fec_active_pct: (self.fec_samples > 0).then(|| {
                round_tenth(self.fec_active_samples as f64 / self.fec_samples as f64 * 100.0)
            }),
            xrun_count: self.xrun_count,
            route: self.link.route,
            route_confirmed: self.link.route_confirmed,
            connect_ms: self.link.connect_ms,
            first_audio_ms: self.link.first_audio_ms,
            local_ips: self.local_ips.clone(),
            public_ip: self.public_ip,
        }
    }
}

impl Default for SessionTally {
    fn default() -> Self {
        Self::new()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The sorted-list way the percentiles were computed before the readings
    /// were counted by value.
    fn sorted_list_percentile(readings: &[f32], p: usize) -> Option<f64> {
        let mut sorted: Vec<f64> = readings
            .iter()
            .filter(|rtt| rtt.is_finite() && **rtt >= 0.0)
            .map(|rtt| f64::from(*rtt))
            .collect();
        sorted.sort_by(f64::total_cmp);
        if sorted.is_empty() {
            return None;
        }
        let rank = (p * sorted.len()).div_ceil(100).max(1);
        Some(round_tenth(sorted[rank - 1]))
    }

    fn end_of(readings: &[f32]) -> SessionEnd {
        let mut tally = SessionTally::new();
        for rtt in readings {
            tally.sample(Some(*rtt), 0.0);
        }
        tally.finish_after(Duration::from_secs(1), EndReason::Left)
    }

    fn pseudo_random_readings(count: usize, seed: u64) -> Vec<f32> {
        let mut x = seed;
        (0..count)
            .map(|_| {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                10.0 + ((x >> 33) % 8000) as f32 / 100.0
            })
            .collect()
    }

    /// Verifies: REQ-TEL-010
    #[test]
    fn when_readings_are_counted_by_value_the_percentiles_match_the_sorted_list() {
        let cases: Vec<Vec<f32>> = vec![
            vec![42.0],
            vec![0.0],
            vec![0.0, 0.04, 0.05, 0.06, 0.1],
            vec![20.0; 1000],
            [vec![15.0; 940], vec![80.0; 60]].concat(),
            [vec![15.0; 949], vec![80.0; 51]].concat(),
            (1..=100).map(|n| n as f32).collect(),
            (1..=101).map(|n| n as f32 * 0.37).collect(),
            vec![0.1, 0.1, 0.1, 999.9, 5000.0],
            pseudo_random_readings(5000, 1),
            pseudo_random_readings(777, 2),
        ];
        for readings in cases {
            let end = end_of(&readings);
            assert_eq!(
                end.rtt_ms_p50,
                sorted_list_percentile(&readings, 50),
                "p50 of {} readings",
                readings.len()
            );
            assert_eq!(
                end.rtt_ms_p95,
                sorted_list_percentile(&readings, 95),
                "p95 of {} readings",
                readings.len()
            );
        }
    }

    /// Verifies: REQ-TEL-010
    #[test]
    fn when_readings_are_invalid_or_missing_they_are_not_counted() {
        let mut tally = SessionTally::new();
        for rtt in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0] {
            tally.sample(Some(rtt), 0.0);
        }
        tally.sample(None, 0.0);

        let end = tally.finish_after(Duration::from_secs(1), EndReason::Left);

        assert_eq!(end.rtt_ms_p50, None);
        assert_eq!(end.rtt_ms_p95, None);
    }

    /// Verifies: REQ-TEL-010
    #[test]
    fn when_a_hundred_thousand_readings_arrive_only_the_distinct_values_are_held() {
        let readings = pseudo_random_readings(100_000, 3);
        let mut tally = SessionTally::new();
        for rtt in &readings {
            tally.sample(Some(*rtt), 0.0);
        }

        let mut distinct: Vec<u32> = readings
            .iter()
            .map(|rtt| (f64::from(*rtt) * 10.0).round() as u32)
            .collect();
        distinct.sort_unstable();
        distinct.dedup();

        assert_eq!(tally.rtt_samples, 100_000);
        assert_eq!(tally.rtt_tenths.len(), distinct.len());
        assert!(tally.rtt_tenths.len() <= 1000);
    }
}
