//! Metric liveness checks
//!
//! The traceability checks (ADR-018) catch a *test* that asserts nothing. They
//! cannot catch a *product* value that looks measured but is a constant, and two
//! of those shipped:
//!
//! | Field | Was | Effect |
//! |-------|-----|--------|
//! | `ConnectionStats::packet_loss_rate` | `0.0` behind a TODO | the quality classification built on it could never report loss |
//! | `LocalLatencyInfo::jitter_buffer_ms` | `0.0`, setter never called | the latency breakdown omitted up to 42.67ms of buffering |
//!
//! Both were invisible to a grep and to every existing test, because a constant
//! is a perfectly valid value. What distinguishes a measurement from a constant
//! is that it *moves* when the thing it measures changes.
//!
//! Each test below applies a stimulus and asserts the metric responds. Adding a
//! field to one of these structs without adding it here leaves the same hole
//! open, so `.claude/rules/traceability.md` requires it.

use std::time::Duration;

use jamjam::audio::{AudioPreset, BUDGET_SAMPLE_RATE};
use jamjam::network::{
    required_bps, BandwidthEstimator, Connection, ConnectionQuality, LatencyBreakdown,
    LocalLatencyInfo,
};
use jamjam::protocol::Packet;

/// Every field of `ConnectionStats` that describes traffic must respond to traffic.
///
/// Verifies: REQ-NET-026
#[tokio::test]
async fn connection_stats_respond_to_traffic() {
    let mut receiver = Connection::new("127.0.0.1:0")
        .await
        .expect("receiver socket");
    let receiver_addr = receiver.local_addr();
    receiver
        .connect(receiver_addr)
        .await
        .expect("receiver starts its loop");

    let mut sender = Connection::new("127.0.0.1:0").await.expect("sender socket");
    sender
        .connect(receiver_addr)
        .await
        .expect("sender connects");

    let before = sender.stats();

    // Send with a deliberate gap so loss is non-zero on the receiving side.
    for sequence in 0..10u32 {
        if sequence == 4 || sequence == 7 {
            continue;
        }
        let packet = Packet::audio(sequence, sequence * 64, vec![0u8; 64]);
        sender
            .send_raw_to(&packet, receiver_addr)
            .await
            .expect("send");
    }
    sender
        .send_audio(&[0.1f32; 64], 0)
        .await
        .expect("send audio");

    tokio::time::sleep(Duration::from_millis(250)).await;

    let after = sender.stats();
    let received = receiver.stats();

    assert!(
        after.packets_sent > before.packets_sent,
        "packets_sent did not move: {} -> {}",
        before.packets_sent,
        after.packets_sent
    );
    assert!(
        after.bytes_sent > before.bytes_sent,
        "bytes_sent did not move"
    );
    assert!(
        received.packets_received > 0,
        "packets_received stayed at 0 despite traffic"
    );
    assert!(
        received.bytes_received > 0,
        "bytes_received stayed at 0 despite traffic"
    );
    assert!(
        received.packet_loss_rate > 0.0,
        "packet_loss_rate stayed at 0.0 despite two missing sequence numbers - \
         it is a constant again"
    );

    // The derived classification must move with the figures it reads. No RTT
    // ping has fired within this test's short window (REQ-LAT-130), so go
    // through the classifier directly rather than `quality()`, which stays
    // `None` until the first sample arrives.
    assert_ne!(
        ConnectionQuality::classify(0.0, received.packet_loss_rate),
        ConnectionQuality::classify(0.0, 0.0),
        "quality() ignores packet_loss_rate"
    );
}

/// The latency breakdown must include every stage that contributes delay.
///
/// `LocalLatencyInfo::from_audio_config` cannot know the jitter buffer depth, so
/// the caller has to supply it. When nobody did, the breakdown under-reported
/// downstream latency by the whole buffer.
///
/// Verifies: REQ-NET-027
#[test]
fn latency_breakdown_includes_the_jitter_buffer() {
    let preset = AudioPreset::HighQuality;
    let jitter_ms = preset.jitter_buffer_delay_ms(BUDGET_SAMPLE_RATE);
    assert!(
        jitter_ms > 0.0,
        "this preset must have a buffer for the test to mean anything"
    );

    let mut info =
        LocalLatencyInfo::from_audio_config(preset.frame_size(), BUDGET_SAMPLE_RATE, "pcm");

    // Before the caller supplies it, the field is zero by construction.
    let without = LatencyBreakdown::calculate(&info, None, 20.0, 1.0);

    info.set_jitter_buffer_ms(jitter_ms);
    let with = LatencyBreakdown::calculate(&info, None, 20.0, 1.0);

    assert!(
        with.downstream.jitter_buffer_ms > 0.0,
        "the buffer is missing from the breakdown"
    );
    assert!(
        (with.downstream.jitter_buffer_ms - jitter_ms).abs() < 0.01,
        "breakdown reports {:.2}ms of buffering, preset specifies {:.2}ms",
        with.downstream.jitter_buffer_ms,
        jitter_ms
    );
    assert!(
        with.downstream.total() > without.downstream.total(),
        "downstream total ignores the jitter buffer: {:.2}ms either way",
        with.downstream.total()
    );
    assert!(
        (with.downstream.total() - without.downstream.total() - jitter_ms).abs() < 0.01,
        "the buffer must add its full delay to the downstream total"
    );
}

/// The bandwidth estimator must report the rate it observes, not a constant,
/// and its verdict must follow what the link drops.
///
/// Verifies: REQ-NET-028
#[test]
fn bandwidth_estimate_responds_to_throughput() {
    use std::time::Instant;

    let start = Instant::now();
    let preset = AudioPreset::Balanced;
    let mut estimator = BandwidthEstimator::new(Duration::from_millis(100));

    estimator.note_audio(0, 0);
    estimator.sample_at(start, 0);

    // A narrow link that drops half of what the peer sends, then a wide one
    // that drops nothing. Both the rate and the verdict must change.
    estimator.note_audio(50, 50);
    let narrow = estimator
        .sample_at(start + Duration::from_millis(100), 1_000)
        .expect("first interval");
    let narrow_status = estimator.status();

    estimator.note_audio(150, 50);
    let wide = estimator
        .sample_at(start + Duration::from_millis(200), 1_000_000)
        .expect("second interval");
    let wide_status = estimator.status();

    assert!(
        wide > narrow * 10.0,
        "measured rate did not follow throughput: {:.0} then {:.0} bps",
        narrow,
        wide
    );
    assert_ne!(
        narrow_status, wide_status,
        "the verdict is the same for {:.0} and {:.0} bps",
        narrow, wide
    );

    // The requirement must respond to the configuration it is derived from.
    let mono = required_bps(&preset, BUDGET_SAMPLE_RATE, 1);
    let stereo = required_bps(&preset, BUDGET_SAMPLE_RATE, 2);
    let high_rate = required_bps(&preset, 96_000, 2);
    assert!(
        stereo > mono,
        "channel count does not affect the requirement"
    );
    assert!(
        high_rate > stereo,
        "sample rate does not affect the requirement"
    );
}
