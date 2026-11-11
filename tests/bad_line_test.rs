//! The bad line that tests put between two ends does what it is set to (ADR-124)
//!
//! A measurement taken through a simulated line is only worth what the simulation is. These tests
//! check the line itself: that the share of datagrams it loses, the delays it gives, the rate it
//! holds to and the runs of loss it makes are those it was set to, first on the arithmetic that
//! decides them and then on datagrams that cross a socket, and that one seed always gives the
//! same line.

mod common;

use std::time::{Duration, Instant};

use common::line::{condition, BadLine, Counts, Direction, Impairment, Jitter, Loss, CONDITIONS};
use tokio::net::UdpSocket;

const PACKET: usize = 1000;

fn ms(milliseconds: f64) -> Duration {
    Duration::from_secs_f64(milliseconds / 1000.0)
}

/// Offers `count` datagrams one millisecond apart and returns for each when it is due after
/// leaving (empty if it was lost)
fn run(spec: Impairment, seed: u64, count: usize) -> (Vec<Vec<Duration>>, Counts) {
    let mut direction = Direction::new(spec, seed);
    let outcomes = (0..count)
        .map(|i| direction.offer(ms(i as f64), PACKET))
        .collect();
    (outcomes, direction.counts().clone())
}

fn delays_ms(outcomes: &[Vec<Duration>]) -> Vec<f64> {
    outcomes
        .iter()
        .enumerate()
        .flat_map(|(i, dues)| {
            dues.iter()
                .map(move |due| (due.as_secs_f64() - i as f64 / 1000.0) * 1000.0)
        })
        .collect()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

fn standard_deviation(values: &[f64]) -> f64 {
    let m = mean(values);
    (values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

/// With a loss of 5 %, about 5 % of the datagrams are lost, and only those
#[test]
fn a_random_loss_of_5_percent_loses_5_percent_of_the_datagrams() {
    let (outcomes, counts) = run(Impairment::default().loss(5.0), 1, 100_000);

    let lost = outcomes.iter().filter(|dues| dues.is_empty()).count();
    let share = lost as f64 / outcomes.len() as f64;
    assert!((0.0465..0.0535).contains(&share), "lost {share}");
    assert_eq!(counts.lost as usize, lost);
    assert_eq!(counts.offered, 100_000);
    assert_eq!(counts.delivered as usize, outcomes.len() - lost);
}

/// The losses of the bursty model come in runs about as long as the model says, and the time
/// spent in them is the share it gives
#[test]
fn a_burst_loss_comes_in_runs_of_the_length_the_model_gives() {
    // netem gemodel 0.1% 5% 100% 0%: runs of 1 / 5 % = 20 on average, in the bad state
    // 0.1 / (0.1 + 5) = 1.96 % of the time
    let spec = Impairment::default().burst_loss(0.1, 5.0, 100.0, 0.0);
    let Loss::Burst(model) = spec.loss else {
        panic!("a burst loss");
    };
    let (outcomes, _) = run(spec, 7, 3_000_000);

    let mut runs = Vec::new();
    let mut current = 0usize;
    for dues in &outcomes {
        if dues.is_empty() {
            current += 1;
        } else if current > 0 {
            runs.push(current);
            current = 0;
        }
    }
    let lost: usize = runs.iter().sum();
    let share = lost as f64 / outcomes.len() as f64;
    let mean_run = lost as f64 / runs.len() as f64;

    assert!(
        (model.share_in_bad() * 0.85..model.share_in_bad() * 1.15).contains(&share),
        "share lost {share}, the model has {}",
        model.share_in_bad()
    );
    assert!(
        (17.0..23.0).contains(&mean_run),
        "mean run {mean_run} over {} runs",
        runs.len()
    );
    assert!(
        runs.iter().any(|run| *run >= 60),
        "a run of 3 times the mean is to be expected among {} runs",
        runs.len()
    );
}

/// With a normal spread of 8 ms around 35 ms, the delays have that mean and that deviation
#[test]
fn a_normal_jitter_gives_delays_with_the_mean_and_deviation_set() {
    let (outcomes, _) = run(Impairment::default().delay(35.0, 8.0), 3, 50_000);
    let delays = delays_ms(&outcomes);

    assert!(
        (34.8..35.2).contains(&mean(&delays)),
        "mean {}",
        mean(&delays)
    );
    assert!(
        (7.8..8.2).contains(&standard_deviation(&delays)),
        "deviation {}",
        standard_deviation(&delays)
    );
}

/// With a uniform spread, no delay is outside delay ± jitter and they fill the range
#[test]
fn a_uniform_jitter_keeps_the_delays_within_the_range_and_fills_it() {
    let (outcomes, _) = run(
        Impairment::default()
            .delay(50.0, 10.0)
            .jitter(Jitter::Uniform),
        3,
        20_000,
    );
    let delays = delays_ms(&outcomes);
    let (min, max) = delays
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(*d), hi.max(*d)));

    assert!(min >= 40.0 - 1e-6 && max <= 60.0 + 1e-6, "{min}..{max}");
    assert!(
        min < 40.1 && max > 59.9,
        "the range is filled: {min}..{max}"
    );
    assert!((49.8..50.2).contains(&mean(&delays)));
}

/// With a Pareto spread no delay is shorter than the delay set, the mean is half the jitter
/// above it, and a few are many jitters late
#[test]
fn a_pareto_jitter_is_mostly_small_and_sometimes_many_times_the_jitter() {
    let (outcomes, _) = run(
        Impairment::default().delay(0.1, 3.0).jitter(Jitter::Pareto),
        5,
        200_000,
    );
    let delays = delays_ms(&outcomes);
    let mut sorted = delays.clone();
    sorted.sort_by(f64::total_cmp);

    assert!(sorted[0] >= 0.1 - 1e-6, "shortest {}", sorted[0]);
    assert!(
        (1.4..1.9).contains(&(mean(&delays) - 0.1)),
        "mean {}",
        mean(&delays)
    );
    // Half of them are within the median of X - 1 = 2^(1/3) - 1 = 0.26 jitters
    assert!(
        sorted[sorted.len() / 2] < 0.1 + 0.3 * 3.0,
        "median {}",
        sorted[sorted.len() / 2]
    );
    assert!(
        *sorted.last().unwrap() > 0.1 + 10.0 * 3.0,
        "the tail reaches {}",
        sorted.last().unwrap()
    );
}

/// A line held to 1 Mbit/s carries 1 Mbit/s of what is sent into it and drops the rest once
/// its queue is full, and no datagram waits longer than the queue is deep
#[test]
fn a_rate_limit_carries_that_rate_and_drops_what_does_not_fit_in_the_queue() {
    // 1000-byte datagrams every 4 ms are 2 Mbit/s into a line of 1 Mbit/s with 100 ms of queue
    let mut direction = Direction::new(Impairment::default().rate_mbit(1.0).queue_ms(100.0), 1);
    let mut last_due = Duration::ZERO;
    for i in 0..1000 {
        let dues = direction.offer(ms(i as f64 * 4.0), PACKET);
        if let Some(due) = dues.first() {
            assert!(*due >= last_due, "a line carries datagrams in order");
            last_due = *due;
        }
    }
    let counts = direction.counts();

    // Over the 4 s the datagrams were offered in, the line sends at most 1 Mbit/s, plus the
    // 100 ms it had queued when they stopped
    let carried_bits = counts.bytes_delivered as f64 * 8.0;
    assert!(
        (carried_bits / 1e6 - 4.1).abs() < 0.15,
        "carried {} Mbit in 4 s",
        carried_bits / 1e6
    );
    assert!(counts.queue_full > 400, "dropped {}", counts.queue_full);
    assert!(
        counts.longest_queue_ms <= 100.0 && counts.longest_queue_ms > 95.0,
        "longest wait {} ms",
        counts.longest_queue_ms
    );
    // One datagram takes 8 ms to go out, so datagrams 1 ms apart leave 8 ms apart
    let (outcomes, _) = run(Impairment::default().rate_mbit(1.0), 1, 3);
    let spacing = outcomes[1][0] - outcomes[0][0];
    assert!(
        (spacing.as_secs_f64() * 1000.0 - 8.0).abs() < 0.01,
        "spacing {spacing:?}"
    );
}

/// A line that is sent less than its rate does not queue or drop anything
#[test]
fn a_line_sent_less_than_its_rate_neither_queues_nor_drops() {
    // 1000 bytes every 10 ms is 0.8 Mbit/s into 1 Mbit/s
    let mut direction = Direction::new(Impairment::default().rate_mbit(1.0), 1);
    for i in 0..500 {
        assert_eq!(direction.offer(ms(i as f64 * 10.0), PACKET).len(), 1);
    }
    assert_eq!(direction.counts().queue_full, 0);
    assert!(direction.counts().longest_queue_ms < 0.01);
}

/// Datagrams given a chance of skipping the delay arrive ahead of those before them, and repeated
/// ones arrive twice
#[test]
fn reordered_datagrams_go_ahead_and_repeated_ones_go_twice() {
    let (outcomes, counts) = run(
        Impairment::default()
            .delay(20.0, 0.0)
            .reorder(25.0)
            .duplicate(10.0),
        2,
        20_000,
    );

    assert!((0.23..0.27).contains(&(counts.reordered as f64 / 20_000.0)));
    assert!((0.09..0.11).contains(&(counts.duplicated as f64 / 20_000.0)));
    assert_eq!(counts.delivered, 20_000 + counts.duplicated);
    let firsts: Vec<Duration> = outcomes.iter().map(|dues| dues[0]).collect();
    assert!(
        firsts.windows(2).filter(|pair| pair[1] < pair[0]).count() > 1000,
        "a datagram that skips the delay overtakes the one before it"
    );
}

/// Nothing gets through during an outage and everything does outside it
#[test]
fn nothing_gets_through_during_an_outage() {
    let spec = Impairment::default()
        .delay(10.0, 0.0)
        .outage(ms(1000.0), ms(500.0));
    let mut direction = Direction::new(spec, 1);

    assert_eq!(direction.offer(ms(999.0), PACKET).len(), 1);
    assert!(direction.offer(ms(1000.0), PACKET).is_empty());
    assert!(direction.offer(ms(1499.0), PACKET).is_empty());
    assert_eq!(direction.offer(ms(1500.0), PACKET).len(), 1);
    assert_eq!(direction.counts().outage, 2);
}

/// One seed makes the same line every time, another seed a different one
#[test]
fn the_same_seed_gives_the_same_line_and_another_seed_a_different_one() {
    let spec = || {
        Impairment::default()
            .delay(40.0, 10.0)
            .burst_loss(1.0, 20.0, 100.0, 1.0)
            .reorder(5.0)
            .duplicate(2.0)
            .rate_mbit(5.0)
    };
    let (a, _) = run(spec(), 42, 10_000);
    let (b, _) = run(spec(), 42, 10_000);
    let (c, _) = run(spec(), 43, 10_000);

    assert_eq!(a, b);
    assert_ne!(a, c);
}

/// The named conditions have the figures they are named for
#[test]
fn the_named_conditions_have_the_figures_they_are_named_for() {
    let each_way = |name: &str| (condition(name).expect("a condition of that name").each_way)();

    let fiber = each_way("fiber");
    assert_eq!(
        (fiber.delay_ms, fiber.jitter_ms, fiber.rate_bps),
        (3.0, 1.0, None)
    );
    assert_eq!(fiber.loss, Loss::None);

    let good = each_way("4g-good");
    assert_eq!(
        (good.delay_ms, good.jitter_ms, good.rate_bps),
        (35.0, 8.0, Some(20e6))
    );
    assert_eq!(good.loss, Loss::Random(0.002));

    let busy = each_way("4g-busy");
    assert_eq!(
        (busy.delay_ms, busy.jitter_ms, busy.rate_bps),
        (90.0, 25.0, Some(5e6))
    );
    assert_eq!(busy.loss, Loss::Random(0.01));

    let burst = each_way("burst-loss");
    assert_eq!(
        (burst.delay_ms, burst.jitter_ms, burst.rate_bps),
        (40.0, 10.0, None)
    );
    let Loss::Burst(model) = burst.loss else {
        panic!("a burst loss");
    };
    assert_eq!(
        (
            model.to_bad,
            model.to_good,
            model.loss_in_bad,
            model.loss_in_good
        ),
        (0.001, 0.05, 1.0, 0.0)
    );
    assert!((model.share_in_bad() - 0.0196).abs() < 0.0001);

    let narrow = each_way("narrow");
    assert_eq!(
        (narrow.delay_ms, narrow.jitter_ms, narrow.rate_bps),
        (40.0, 10.0, Some(1e6))
    );

    let bloat = each_way("queue-bloat");
    assert_eq!(
        (bloat.delay_ms, bloat.jitter_ms, bloat.rate_bps),
        (30.0, 10.0, Some(4e6))
    );
    assert_eq!(bloat.queue_ms, 600.0);

    assert_eq!(each_way("loss-5").loss, Loss::Random(0.05));
    assert_eq!(each_way("loss-20").loss, Loss::Random(0.20));
    let wifi = each_way("wifi");
    assert_eq!(
        (wifi.delay_ms, wifi.jitter_ms, wifi.jitter),
        (0.1, 3.0, Jitter::Pareto)
    );
    let wired = each_way("wired");
    assert_eq!((wired.delay_ms, wired.jitter_ms), (0.2, 0.3));

    let mut names: Vec<_> = CONDITIONS.iter().map(|c| c.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CONDITIONS.len(), "a name is used once");
}

// ---- through a socket ----

/// Sends `count` datagrams numbered from 0, `gap` apart, and returns when each was sent
async fn send_numbered(
    from: &UdpSocket,
    to: std::net::SocketAddr,
    count: u32,
    gap: Duration,
) -> Vec<Instant> {
    let mut sent = Vec::new();
    for n in 0..count {
        let mut datagram = vec![0u8; PACKET];
        datagram[..4].copy_from_slice(&n.to_be_bytes());
        sent.push(Instant::now());
        from.send_to(&datagram, to).await.expect("send");
        tokio::time::sleep(gap).await;
    }
    sent
}

/// Reads until nothing has come for `quiet`: the number of each datagram and when it came
async fn collect(on: &UdpSocket, quiet: Duration) -> Vec<(u32, Instant)> {
    let mut got = Vec::new();
    let mut buf = [0u8; 2048];
    while let Ok(Ok((len, _))) = tokio::time::timeout(quiet, on.recv_from(&mut buf)).await {
        assert_eq!(len, PACKET, "the line carries a datagram as it is");
        got.push((
            u32::from_be_bytes(buf[..4].try_into().unwrap()),
            Instant::now(),
        ));
    }
    got
}

async fn ends() -> (UdpSocket, UdpSocket) {
    (
        UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        UdpSocket::bind("127.0.0.1:0").await.unwrap(),
    )
}

/// Datagrams that cross the relay arrive after the delay set, not before and not much after
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn datagrams_through_the_relay_arrive_after_the_delay_set() {
    let (near, far) = ends().await;
    let line = BadLine::start(
        None,
        far.local_addr().unwrap(),
        Impairment::default().delay(30.0, 0.0),
        Impairment::default(),
        1,
    )
    .await;

    let (sent, got) = tokio::join!(
        send_numbered(&near, line.addr(), 100, Duration::from_millis(2)),
        collect(&far, Duration::from_millis(300))
    );

    assert_eq!(got.len(), 100);
    for (n, at) in &got {
        let took = at.duration_since(sent[*n as usize]).as_secs_f64() * 1000.0;
        assert!((29.0..90.0).contains(&took), "datagram {n} took {took} ms");
    }
}

/// A datagram with a short delay is not held up behind one sent before it with a long delay
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_short_delay_is_not_held_up_behind_a_long_one_sent_before_it() {
    let (near, far) = ends().await;
    let line = BadLine::start(
        None,
        far.local_addr().unwrap(),
        Impairment::default()
            .delay(50.0, 49.0)
            .jitter(Jitter::Uniform),
        Impairment::default(),
        9,
    )
    .await;

    let (sent, got) = tokio::join!(
        send_numbered(&near, line.addr(), 300, Duration::from_millis(1)),
        collect(&far, Duration::from_millis(300))
    );

    assert_eq!(got.len(), 300);
    let overtaken = got.windows(2).filter(|pair| pair[1].0 < pair[0].0).count();
    assert!(overtaken > 20, "datagrams overtake each other: {overtaken}");
    for (n, at) in &got {
        let took = at.duration_since(sent[*n as usize]).as_secs_f64() * 1000.0;
        assert!(
            took < 99.0 + 60.0,
            "datagram {n} took {took} ms: waiting for the others adds to its delay"
        );
    }
}

/// The two directions are lines of their own: a loss set one way is not seen the other
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loss_set_one_way_is_not_seen_the_other_way() {
    let (near, far) = ends().await;
    let line = BadLine::start(
        None,
        far.local_addr().unwrap(),
        Impairment::default().loss(10.0),
        Impairment::default(),
        4,
    )
    .await;

    let (_, up) = tokio::join!(
        send_numbered(&near, line.addr(), 2000, Duration::from_micros(200)),
        collect(&far, Duration::from_millis(300))
    );
    // The relay knows the near end from what it sent, so the far end can answer to the line
    let (_, down) = tokio::join!(
        send_numbered(&far, line.addr(), 500, Duration::from_micros(200)),
        collect(&near, Duration::from_millis(300))
    );

    assert_eq!(down.len(), 500, "nothing is lost the way back");
    let share = 1.0 - up.len() as f64 / 2000.0;
    assert!((0.07..0.13).contains(&share), "lost {share}");
    assert_eq!(line.up().lost as usize, 2000 - up.len());
    assert_eq!(line.up().delivered as usize, up.len());
    assert_eq!(line.down().dropped(), 0);
}

/// Nothing crosses while the line is out, and traffic goes on after it
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_crosses_a_relay_during_a_blackout_and_traffic_goes_on_after() {
    let (near, far) = ends().await;
    let line = BadLine::start(
        None,
        far.local_addr().unwrap(),
        Impairment::default(),
        Impairment::default(),
        1,
    )
    .await;

    send_numbered(&near, line.addr(), 5, Duration::from_millis(2)).await;
    assert_eq!(collect(&far, Duration::from_millis(100)).await.len(), 5);

    line.black_out(Duration::from_millis(400));
    send_numbered(&near, line.addr(), 5, Duration::from_millis(2)).await;
    assert_eq!(collect(&far, Duration::from_millis(100)).await.len(), 0);

    tokio::time::sleep(Duration::from_millis(400)).await;
    send_numbered(&near, line.addr(), 5, Duration::from_millis(2)).await;
    assert_eq!(collect(&far, Duration::from_millis(100)).await.len(), 5);
    assert_eq!(line.up().outage, 5);
}

/// Two runs with the same seed lose the same datagrams
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_relays_with_the_same_seed_lose_the_same_datagrams() {
    async fn survivors(seed: u64) -> Vec<u32> {
        let (near, far) = ends().await;
        let line = BadLine::start(
            None,
            far.local_addr().unwrap(),
            Impairment::default().loss(20.0),
            Impairment::default(),
            seed,
        )
        .await;
        let (_, got) = tokio::join!(
            send_numbered(&near, line.addr(), 1000, Duration::from_micros(300)),
            collect(&far, Duration::from_millis(200))
        );
        let mut got: Vec<u32> = got.into_iter().map(|(n, _)| n).collect();
        got.sort_unstable();
        got
    }

    let (a, b, c) = (
        survivors(11).await,
        survivors(11).await,
        survivors(12).await,
    );
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!((700..900).contains(&a.len()), "kept {}", a.len());
}
