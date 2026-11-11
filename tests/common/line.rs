//! A bad line to put between two ends of a connection (ADR-124).
//!
//! [`BadLine`] is a UDP relay that carries the datagrams of both ends the way a poor network
//! would: it delays them, spreads the delays, drops them singly or in runs, reorders and repeats
//! some, holds them to a rate with a queue of limited depth and goes dark for a while. Each
//! direction has a line of its own. Every random choice comes from a seed, so the same seed
//! drops and delays the same datagrams every time.
//!
//! The decisions are made by [`Direction`], which is plain arithmetic on the time a datagram
//! arrives and its size and so can be checked without a socket. The relay gives every datagram
//! the time it is due and sends it then, whatever else is waiting: a datagram with a short delay
//! is not held up behind one with a long delay sent before it, as it would not be on a network.
//!
//! The figures follow `tc netem` where it has the same thing, so a condition run here can be
//! put beside the same condition applied to a real interface.

#![allow(dead_code)]

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the delay of one datagram varies around the delay of the line
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Jitter {
    /// Evenly between delay − jitter and delay + jitter
    Uniform,
    /// A normal distribution with the jitter as its standard deviation (netem's `distribution
    /// normal`)
    Normal,
    /// The delay is the shortest the line gives, and a Pareto-distributed extra (shape 3) is
    /// added that is jitter × (X − 1) for X ≥ 1: most datagrams are barely late and a few are
    /// late by many times the jitter, as on a busy Wi-Fi link. The mean is delay + jitter / 2.
    Pareto,
}

/// A Gilbert-Elliott loss: two states, each with its own chance of losing a datagram, and a
/// chance per datagram of moving from one to the other (netem's `loss gemodel p r 1-h 1-k`)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GilbertElliott {
    /// Chance per datagram of moving from the good state to the bad one
    pub to_bad: f64,
    /// Chance per datagram of moving from the bad state to the good one; 1 / this is the mean
    /// length of a run in the bad state
    pub to_good: f64,
    /// Chance of losing a datagram in the bad state
    pub loss_in_bad: f64,
    /// Chance of losing a datagram in the good state
    pub loss_in_good: f64,
}

impl GilbertElliott {
    /// The share of datagrams that arrive while the line is in the bad state
    pub fn share_in_bad(&self) -> f64 {
        self.to_bad / (self.to_bad + self.to_good)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Loss {
    None,
    /// Each datagram is lost with this chance (0.0 to 1.0)
    Random(f64),
    Burst(GilbertElliott),
}

/// What one direction of a line does to the datagrams that pass
#[derive(Clone, Debug)]
pub struct Impairment {
    pub delay_ms: f64,
    pub jitter_ms: f64,
    pub jitter: Jitter,
    pub loss: Loss,
    /// Chance that a datagram skips the delay and goes at once, which puts it ahead of those
    /// before it (netem's `reorder`)
    pub reorder: f64,
    /// Chance that a datagram is sent twice
    pub duplicate: f64,
    /// Bits per second the line carries; `None` for no limit
    pub rate_bps: Option<f64>,
    /// How long a datagram may wait for the rate before it is dropped instead: the depth of the
    /// queue in front of the line. Only with a rate.
    pub queue_ms: f64,
    /// Stretches of time, counted from the start of the line, in which nothing gets through
    pub outages: Vec<(Duration, Duration)>,
}

impl Default for Impairment {
    fn default() -> Self {
        Self {
            delay_ms: 0.0,
            jitter_ms: 0.0,
            jitter: Jitter::Normal,
            loss: Loss::None,
            reorder: 0.0,
            duplicate: 0.0,
            rate_bps: None,
            queue_ms: 500.0,
            outages: Vec::new(),
        }
    }
}

impl Impairment {
    pub fn delay(mut self, delay_ms: f64, jitter_ms: f64) -> Self {
        self.delay_ms = delay_ms;
        self.jitter_ms = jitter_ms;
        self
    }

    pub fn jitter(mut self, jitter: Jitter) -> Self {
        self.jitter = jitter;
        self
    }

    pub fn loss(mut self, percent: f64) -> Self {
        self.loss = Loss::Random(percent / 100.0);
        self
    }

    /// `loss gemodel` with the chances given in percent, as netem takes them
    pub fn burst_loss(
        mut self,
        to_bad_percent: f64,
        to_good_percent: f64,
        loss_in_bad_percent: f64,
        loss_in_good_percent: f64,
    ) -> Self {
        self.loss = Loss::Burst(GilbertElliott {
            to_bad: to_bad_percent / 100.0,
            to_good: to_good_percent / 100.0,
            loss_in_bad: loss_in_bad_percent / 100.0,
            loss_in_good: loss_in_good_percent / 100.0,
        });
        self
    }

    pub fn reorder(mut self, percent: f64) -> Self {
        self.reorder = percent / 100.0;
        self
    }

    pub fn duplicate(mut self, percent: f64) -> Self {
        self.duplicate = percent / 100.0;
        self
    }

    pub fn rate_mbit(mut self, mbit_per_second: f64) -> Self {
        self.rate_bps = Some(mbit_per_second * 1_000_000.0);
        self
    }

    pub fn queue_ms(mut self, queue_ms: f64) -> Self {
        self.queue_ms = queue_ms;
        self
    }

    pub fn outage(mut self, from: Duration, length: Duration) -> Self {
        self.outages.push((from, length));
        self
    }
}

/// A small generator with a fixed algorithm (splitmix64), so a seed means the same sequence
/// whatever version of any library is built in
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In [0, 1)
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, probability: f64) -> bool {
        probability > 0.0 && self.unit() < probability
    }

    fn standard_normal(&mut self) -> f64 {
        let (u1, u2) = (1.0 - self.unit(), self.unit());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Why a datagram did not get through, counted
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    /// Datagrams that came to this direction
    pub offered: u64,
    pub bytes_offered: u64,
    /// Copies sent on, which is more than `offered` less the dropped when some are repeated
    pub delivered: u64,
    pub bytes_delivered: u64,
    /// Lost to the random or burst loss
    pub lost: u64,
    /// Dropped because the queue in front of the rate was full
    pub queue_full: u64,
    /// Dropped during an outage
    pub outage: u64,
    pub duplicated: u64,
    /// Sent ahead of those before them
    pub reordered: u64,
    /// The longest a datagram waited for the rate, in milliseconds
    pub longest_queue_ms: f64,
}

impl Counts {
    pub fn dropped(&self) -> u64 {
        self.lost + self.queue_full + self.outage
    }
}

/// One direction of a line: decides, for each datagram, whether it gets through and when
pub struct Direction {
    spec: Impairment,
    rng: Rng,
    in_bad_state: bool,
    /// When the line is free to start on the next datagram, in seconds from the start
    free_at: f64,
    counts: Counts,
}

impl Direction {
    pub fn new(spec: Impairment, seed: u64) -> Self {
        Self {
            spec,
            rng: Rng(seed),
            in_bad_state: false,
            free_at: 0.0,
            counts: Counts::default(),
        }
    }

    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    pub fn add_outage(&mut self, from: Duration, length: Duration) {
        self.spec.outages.push((from, length));
    }

    /// A datagram of `len` bytes comes at `at` (counted from the start of the line). Returns
    /// when each copy that gets through is due, counted the same way: none if it is lost, two if
    /// it is repeated.
    pub fn offer(&mut self, at: Duration, len: usize) -> Vec<Duration> {
        self.counts.offered += 1;
        self.counts.bytes_offered += len as u64;

        if self
            .spec
            .outages
            .iter()
            .any(|(from, length)| at >= *from && at < *from + *length)
        {
            self.counts.outage += 1;
            return Vec::new();
        }
        if self.is_lost() {
            self.counts.lost += 1;
            return Vec::new();
        }

        let arrived = at.as_secs_f64();
        let mut leaves = arrived;
        if let Some(rate) = self.spec.rate_bps {
            let waits = (self.free_at - arrived).max(0.0);
            if waits * 1000.0 > self.spec.queue_ms {
                self.counts.queue_full += 1;
                return Vec::new();
            }
            self.counts.longest_queue_ms = self.counts.longest_queue_ms.max(waits * 1000.0);
            self.free_at = arrived + waits + len as f64 * 8.0 / rate;
            leaves = self.free_at;
        }

        let mut due = Vec::with_capacity(2);
        let first = if self.rng.chance(self.spec.reorder) {
            self.counts.reordered += 1;
            leaves
        } else {
            leaves + self.delay_seconds()
        };
        due.push(first);
        if self.rng.chance(self.spec.duplicate) {
            self.counts.duplicated += 1;
            due.push(leaves + self.delay_seconds());
        }
        self.counts.delivered += due.len() as u64;
        self.counts.bytes_delivered += (due.len() * len) as u64;
        due.into_iter().map(Duration::from_secs_f64).collect()
    }

    fn is_lost(&mut self) -> bool {
        match self.spec.loss {
            Loss::None => false,
            Loss::Random(probability) => self.rng.chance(probability),
            Loss::Burst(model) => {
                self.in_bad_state = if self.in_bad_state {
                    !self.rng.chance(model.to_good)
                } else {
                    self.rng.chance(model.to_bad)
                };
                let probability = if self.in_bad_state {
                    model.loss_in_bad
                } else {
                    model.loss_in_good
                };
                self.rng.chance(probability)
            }
        }
    }

    fn delay_seconds(&mut self) -> f64 {
        let (delay, jitter) = (self.spec.delay_ms, self.spec.jitter_ms);
        let ms = match self.spec.jitter {
            Jitter::Uniform => delay + jitter * (2.0 * self.rng.unit() - 1.0),
            Jitter::Normal => delay + jitter * self.rng.standard_normal(),
            Jitter::Pareto => {
                // Inverse of the Pareto distribution with scale 1 and shape 3
                let x = (1.0 - self.rng.unit()).powf(-1.0 / 3.0);
                delay + jitter * (x - 1.0)
            }
        };
        ms.max(0.0) / 1000.0
    }
}

/// A datagram waiting for its time
struct Waiting {
    due: Duration,
    order: u64,
    /// `true` if it goes to the far end, `false` back to the near end
    to_far: bool,
    bytes: Vec<u8>,
}

impl PartialEq for Waiting {
    fn eq(&self, other: &Self) -> bool {
        (self.due, self.order) == (other.due, other.order)
    }
}
impl Eq for Waiting {}
impl PartialOrd for Waiting {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Waiting {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.due, self.order).cmp(&(other.due, other.order))
    }
}

/// A relay with a bad line in each direction between its near end and its far end.
///
/// Both ends send to [`BadLine::addr`]. What comes from the far end goes to the near end through
/// the *down* line; what comes from anyone else goes to the far end through the *up* line. If
/// the near end is not given, the first sender other than the far end is the near end. Runs
/// until dropped.
pub struct BadLine {
    addr: SocketAddr,
    up: Arc<Mutex<Direction>>,
    down: Arc<Mutex<Direction>>,
    started: Instant,
    task: tokio::task::JoinHandle<()>,
}

impl BadLine {
    /// `near` is the end that sends first and `far` the one in front of which the line is, or
    /// `None` to take whoever speaks first. `up` is the line from near to far, `down` the way
    /// back. The seed fixes what each does; the two lines draw from sequences of their own.
    pub async fn start(
        near: Option<SocketAddr>,
        far: SocketAddr,
        up: Impairment,
        down: Impairment,
        seed: u64,
    ) -> Self {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("line socket");
        let addr = socket.local_addr().expect("line address");
        let up = Arc::new(Mutex::new(Direction::new(up, seed)));
        let down = Arc::new(Mutex::new(Direction::new(
            down,
            seed ^ 0xD6E8_FEB8_6659_FD93,
        )));
        let started = Instant::now();

        let task = {
            let (up, down) = (up.clone(), down.clone());
            tokio::spawn(async move {
                let mut near = near;
                let mut waiting: BinaryHeap<Reverse<Waiting>> = BinaryHeap::new();
                let mut order = 0u64;
                let mut buf = [0u8; 4096];
                loop {
                    let next = waiting.peek().map(|Reverse(w)| started + w.due);
                    tokio::select! {
                        received = socket.recv_from(&mut buf) => {
                            let Ok((len, from)) = received else { break };
                            let to_far = from != far;
                            if to_far && near.is_none() {
                                near = Some(from);
                            }
                            let direction = if to_far { &up } else { &down };
                            let dues = direction.lock().unwrap().offer(started.elapsed(), len);
                            for due in dues {
                                order += 1;
                                waiting.push(Reverse(Waiting {
                                    due,
                                    order,
                                    to_far,
                                    bytes: buf[..len].to_vec(),
                                }));
                            }
                        }
                        _ = async {
                            match next {
                                Some(at) => tokio::time::sleep_until(at.into()).await,
                                None => std::future::pending().await,
                            }
                        } => {
                            let now = started.elapsed();
                            while waiting.peek().is_some_and(|Reverse(w)| w.due <= now) {
                                let Reverse(w) = waiting.pop().expect("peeked");
                                let to = if w.to_far { Some(far) } else { near };
                                if let Some(to) = to {
                                    let _ = socket.send_to(&w.bytes, to).await;
                                }
                            }
                        }
                    }
                }
            })
        };

        Self {
            addr,
            up,
            down,
            started,
            task,
        }
    }

    /// Where both ends send to
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// What the line from near to far did so far
    pub fn up(&self) -> Counts {
        self.up.lock().unwrap().counts().clone()
    }

    /// What the line from far to near did so far
    pub fn down(&self) -> Counts {
        self.down.lock().unwrap().counts().clone()
    }

    /// Nothing gets through either way from now on for `length`
    pub fn black_out(&self, length: Duration) {
        let from = self.started.elapsed();
        self.up.lock().unwrap().add_outage(from, length);
        self.down.lock().unwrap().add_outage(from, length);
    }
}

impl Drop for BadLine {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A named bad line: the same figures each way
pub struct Condition {
    /// What to call it on a command line
    pub name: &'static str,
    /// In a sentence
    pub about: &'static str,
    /// One way; the line back is the same
    pub each_way: fn() -> Impairment,
}

/// The conditions that have a name. The first six are the figures of the lines a real device is
/// tried on, so a run here can be compared with one there; the rest are single causes.
///
/// Delays are one way, so a round trip is twice the delay.
pub const CONDITIONS: &[Condition] = &[
    Condition {
        name: "fiber",
        about: "fibre: 3 ms ± 1 ms, no loss, no limit",
        each_way: || Impairment::default().delay(3.0, 1.0),
    },
    Condition {
        name: "4g-good",
        about: "4G, good: 35 ms ± 8 ms, 0.2 % lost, 20 Mbit/s",
        each_way: || {
            Impairment::default()
                .delay(35.0, 8.0)
                .loss(0.2)
                .rate_mbit(20.0)
        },
    },
    Condition {
        name: "4g-busy",
        about: "4G, crowded: 90 ms ± 25 ms, 1 % lost, 5 Mbit/s",
        each_way: || Impairment::default().delay(90.0, 25.0).loss(1.0).rate_mbit(5.0),
    },
    Condition {
        name: "burst-loss",
        about: "runs of lost packets: 40 ms ± 10 ms, runs of 20 on average, about 2 % of the time (Gilbert-Elliott 0.1 %, 5 %, 100 %, 0 %)",
        each_way: || {
            Impairment::default()
                .delay(40.0, 10.0)
                .burst_loss(0.1, 5.0, 100.0, 0.0)
        },
    },
    Condition {
        name: "narrow",
        about: "too little bandwidth: 40 ms ± 10 ms, 1 Mbit/s",
        each_way: || Impairment::default().delay(40.0, 10.0).rate_mbit(1.0),
    },
    Condition {
        name: "queue-bloat",
        about: "a long queue in front of a narrow line: 30 ms ± 10 ms, 4 Mbit/s, 600 ms of queue",
        each_way: || {
            Impairment::default()
                .delay(30.0, 10.0)
                .rate_mbit(4.0)
                .queue_ms(600.0)
        },
    },
    Condition {
        name: "loss-5",
        about: "5 % lost at random, 20 ms ± 2 ms",
        each_way: || Impairment::default().delay(20.0, 2.0).loss(5.0),
    },
    Condition {
        name: "loss-20",
        about: "20 % lost at random, 20 ms ± 2 ms",
        each_way: || Impairment::default().delay(20.0, 2.0).loss(20.0),
    },
    Condition {
        name: "wifi",
        about: "Wi-Fi: 0.1 ms with a Pareto-distributed extra of 3.0 ms",
        each_way: || {
            Impairment::default()
                .delay(0.1, 3.0)
                .jitter(Jitter::Pareto)
        },
    },
    Condition {
        name: "wired",
        about: "wired LAN: 0.2 ms ± 0.3 ms",
        each_way: || Impairment::default().delay(0.2, 0.3),
    },
];

/// The condition called `name`
pub fn condition(name: &str) -> Option<&'static Condition> {
    CONDITIONS.iter().find(|c| c.name == name)
}
