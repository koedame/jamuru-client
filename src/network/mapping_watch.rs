//! Finds out, while a link hears nothing, whether the router now gives the audio socket another
//! address than the room was told (ADR-129)
//!
//! The room is told the address STUN saw when the app entered it. A router can give the socket
//! another one by the time a link to someone who came later is started: it forgot the mapping
//! after a quiet, and gave a new one to what was sent next. The peer then sends to the address it
//! was told, which leads nowhere, and what the app sends leaves from the new one, which the peer's
//! router does not let in. Neither end can see it: only asking a STUN server again, through the
//! socket the links send through, says what the router gives it now.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tracing::{debug, info};

use super::stun::{StunResult, DEFAULT_STUN_SERVERS};
use super::udp_mux::{MuxStun, UdpMux};

/// When STUN is asked again
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskPlan {
    /// How long a link must have heard nothing before the first question: longer than the
    /// connectivity check takes, so that a link that is merely slow to start is left alone
    pub after: Duration,
    /// How long before the next question, for as long as a link hears nothing
    pub every: Duration,
    /// How many questions at most, until every link has been heard
    pub at_most: u32,
}

impl Default for AskPlan {
    fn default() -> Self {
        Self {
            after: Duration::from_secs(6),
            every: Duration::from_secs(30),
            at_most: 4,
        }
    }
}

/// Asks STUN through the shared socket when a link has heard nothing for a while, and keeps what
/// it saw. Whether that is something the room has not been told is for whoever tells the room
/// (see [`untold`]).
pub struct MappingWatch {
    stun: MuxStun,
    servers: Vec<String>,
    plan: AskPlan,
    seen: Arc<RwLock<Vec<SocketAddr>>>,
    asking: Option<JoinHandle<Vec<StunResult>>>,
    last_asked: Option<Instant>,
    asked: u32,
}

impl MappingWatch {
    /// A watch on the socket of `mux`. What it sees is put in `seen`.
    pub fn new(mux: &UdpMux, seen: Arc<RwLock<Vec<SocketAddr>>>) -> Self {
        Self::with_plan(mux, seen, AskPlan::default())
    }

    pub fn with_plan(mux: &UdpMux, seen: Arc<RwLock<Vec<SocketAddr>>>, plan: AskPlan) -> Self {
        Self {
            stun: mux.stun(),
            servers: DEFAULT_STUN_SERVERS.iter().map(|s| s.to_string()).collect(),
            plan,
            seen,
            asking: None,
            last_asked: None,
            asked: 0,
        }
    }

    /// Uses these STUN servers instead of the default ones
    pub fn with_servers(mut self, servers: Vec<String>) -> Self {
        self.servers = servers;
        self
    }

    /// Takes in an answer that has come, and asks again if the time has come. Does not wait for
    /// the answer: call it every so often, with how long the link that has gone longest without
    /// hearing its peer has been waiting (`None` when no link waits).
    ///
    /// Must be called on the runtime that drives the mux.
    pub fn tick(&mut self, waiting_for: Option<Duration>) {
        if self
            .asking
            .as_ref()
            .is_some_and(|asking| asking.is_finished())
        {
            self.take_answer();
        }
        let Some(waited) = waiting_for else {
            // Everyone is heard: a later silence starts the questions over
            self.asked = 0;
            return;
        };
        let due = waited >= self.plan.after
            && self.asked < self.plan.at_most
            && self.asking.is_none()
            && self
                .last_asked
                .is_none_or(|at| at.elapsed() >= self.plan.every);
        if due {
            self.ask();
        }
    }

    fn ask(&mut self) {
        self.asked += 1;
        self.last_asked = Some(Instant::now());
        let stun = self.stun.clone();
        let servers = self.servers.clone();
        debug!("A link hears nothing: asking STUN what the router gives the audio socket");
        self.asking = Some(tokio::spawn(async move {
            let names: Vec<&str> = servers.iter().map(String::as_str).collect();
            stun.mappings_seen_by(&names).await
        }));
    }

    fn take_answer(&mut self) {
        let Some(asking) = self.asking.take() else {
            return;
        };
        // The task is finished, so this does not wait
        let Some(results) = futures_util::FutureExt::now_or_never(asking).and_then(Result::ok)
        else {
            return;
        };
        let mut addrs: Vec<SocketAddr> = Vec::new();
        for result in results {
            if !addrs.contains(&result.mapped_address) {
                addrs.push(result.mapped_address);
            }
        }
        if addrs.is_empty() {
            return;
        }
        info!("STUN sees the audio socket at {:?}", addrs);
        if let Ok(mut seen) = self.seen.write() {
            *seen = addrs;
        }
    }
}

/// How many addresses the room is told the router gives the socket at most: the ones it was
/// told when entered and the ones seen since, newest first
pub const MAX_TOLD_MAPPINGS: usize = 3;

/// The addresses in `seen` that are not among `told`, the room's server-reflexive ones. Only the
/// first [`MAX_TOLD_MAPPINGS`] of `seen` count: a router that gives each server another port
/// would otherwise leave addresses that are never told, and the room would be told again and
/// again.
pub fn untold(told: &[SocketAddr], seen: &[SocketAddr]) -> Vec<SocketAddr> {
    seen.iter()
        .filter(|addr| addr.is_ipv4())
        .take(MAX_TOLD_MAPPINGS)
        .filter(|addr| !told.contains(addr))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// A STUN server that answers with the address a request came from
    async fn stun_server() -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 576];
            while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                let SocketAddr::V4(from) = from else { continue };
                if len < 20 {
                    continue;
                }
                let cookie = 0x2112A442u32;
                let mut reply = Vec::new();
                reply.extend_from_slice(&0x0101u16.to_be_bytes());
                reply.extend_from_slice(&12u16.to_be_bytes());
                reply.extend_from_slice(&cookie.to_be_bytes());
                reply.extend_from_slice(&buf[8..20]);
                reply.extend_from_slice(&0x0020u16.to_be_bytes());
                reply.extend_from_slice(&8u16.to_be_bytes());
                reply.extend_from_slice(&[0x00, 0x01]);
                reply.extend_from_slice(&(from.port() ^ (cookie >> 16) as u16).to_be_bytes());
                reply.extend_from_slice(&(u32::from(*from.ip()) ^ cookie).to_be_bytes());
                let _ = socket.send_to(&reply, from).await;
            }
        });
        server
    }

    fn fast_plan() -> AskPlan {
        AskPlan {
            after: Duration::from_secs(6),
            every: Duration::from_millis(100),
            at_most: 2,
        }
    }

    async fn settle(watch: &mut MappingWatch, waiting_for: Option<Duration>) {
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            watch.tick(waiting_for);
        }
    }

    /// Verifies: REQ-CON-177
    #[tokio::test]
    async fn when_a_link_has_waited_long_enough_then_the_address_stun_sees_is_kept() {
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();
        let server = stun_server().await;
        let seen = Arc::new(RwLock::new(Vec::new()));
        let mut watch = MappingWatch::with_plan(&mux, seen.clone(), fast_plan())
            .with_servers(vec![server.to_string()]);

        watch.tick(Some(Duration::from_secs(7)));
        settle(&mut watch, Some(Duration::from_secs(7))).await;

        assert_eq!(*seen.read().unwrap(), vec![mux.local_addr()]);
    }

    /// Verifies: REQ-CON-177
    #[tokio::test]
    async fn when_no_link_has_waited_long_enough_then_stun_is_not_asked() {
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();
        let server = stun_server().await;
        let seen = Arc::new(RwLock::new(Vec::new()));
        let mut watch = MappingWatch::with_plan(&mux, seen.clone(), fast_plan())
            .with_servers(vec![server.to_string()]);

        settle(&mut watch, Some(Duration::from_secs(5))).await;
        settle(&mut watch, None).await;

        assert!(seen.read().unwrap().is_empty());
        assert_eq!(watch.asked, 0);
    }

    /// Verifies: REQ-CON-177
    #[tokio::test]
    async fn when_every_link_stays_silent_then_stun_is_asked_a_few_times_and_no_more() {
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();
        let server = stun_server().await;
        let seen = Arc::new(RwLock::new(Vec::new()));
        let mut watch = MappingWatch::with_plan(&mux, seen.clone(), fast_plan())
            .with_servers(vec![server.to_string()]);

        for _ in 0..5 {
            settle(&mut watch, Some(Duration::from_secs(60))).await;
        }

        assert_eq!(watch.asked, 2);
    }

    /// Verifies: REQ-CON-177
    #[tokio::test]
    async fn when_the_links_are_heard_and_one_goes_quiet_later_then_the_questions_start_over() {
        let mux = UdpMux::bind("127.0.0.1:0").await.unwrap();
        let server = stun_server().await;
        let seen = Arc::new(RwLock::new(Vec::new()));
        let mut watch = MappingWatch::with_plan(&mux, seen.clone(), fast_plan())
            .with_servers(vec![server.to_string()]);
        for _ in 0..3 {
            settle(&mut watch, Some(Duration::from_secs(60))).await;
        }
        assert_eq!(watch.asked, 2);
        let before = watch.last_asked.unwrap();

        watch.tick(None);
        assert_eq!(watch.asked, 0);
        settle(&mut watch, Some(Duration::from_secs(60))).await;

        assert!(watch.last_asked.unwrap() > before);
        assert_eq!(watch.asked, 2);
    }

    /// Verifies: REQ-CON-177
    #[test]
    fn when_stun_sees_an_address_the_room_was_not_told_then_it_is_untold() {
        let told = [addr("203.0.113.5:4000")];
        let seen = [addr("203.0.113.5:4000"), addr("203.0.113.5:5000")];

        assert_eq!(untold(&told, &seen), vec![addr("203.0.113.5:5000")]);
        assert!(untold(&told, &seen[..1]).is_empty());
    }
}
