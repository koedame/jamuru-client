//! The audio links of a CLI session: one to each participant who has published an address,
//! all through the session's one audio socket, and what plays is the mix of them
//! (ADR-077).
//!
//! Who gets a link, and what happens to it when its peer leaves or goes silent, is decided by
//! the core's `Roster`, the same as in the app. This carries those decisions out.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};
use tracing::{info, warn};
use uuid::Uuid;

use jamjam::audio::{
    PeerGain, PeerMeter, PeerMix, PeerRateChange, PlayoutStats, ReceivePath, VoiceHandle,
};
use jamjam::network::{
    AudioEncodingConfig, Connection, ConnectionState, ConnectionStats, LinkHealth, LinkIdentity,
    PeerLatencyInfo, UdpMux,
};
use jamjam::protocol::LatencyInfoMessage;

/// The connections the send task sends each captured frame to: those of the links that are up.
/// Replaced whole when a link comes up or goes, so the send task takes it without waiting for
/// whoever changes it.
pub type Senders = Arc<RwLock<Arc<[Arc<tokio::sync::Mutex<Connection>>]>>>;

/// How every link of a session carries audio
pub struct LinkSettings {
    pub encoding: AudioEncodingConfig,
    pub sample_rate: u32,
    pub frame_size: u32,
    /// The play-out delay each link starts with, in frames
    pub jitter_buffer_frames: u32,
    /// What a link tells its peer once it is up, so the peer can convert our audio and show the
    /// latency we add (ADR-013)
    pub announce: LatencyInfoMessage,
}

/// What the connectivity check of a link came to
pub struct Checked {
    peer: Uuid,
    serial: u64,
    result: Result<Connection, String>,
}

enum Stage {
    /// The connectivity check is running. It hands the connection over when it ends.
    Checking(tokio::task::JoinHandle<()>),
    Up(Arc<tokio::sync::Mutex<Connection>>),
    /// The link could not be set up
    Down,
}

struct Link {
    peer: Uuid,
    name: String,
    /// Tells this link from an earlier one to the same peer, whose check may end late
    serial: u64,
    /// Their audio, from payload to the mix
    receive: ReceivePath,
    /// Their place in the mix: they are heard for as long as the link is held
    _voice: VoiceHandle,
    stage: Stage,
}

/// What a link looked like when it was asked about or ended
#[derive(Clone)]
pub struct LinkReport {
    pub name: String,
    pub addr: Option<SocketAddr>,
    pub stats: ConnectionStats,
    pub peer_info: Option<PeerLatencyInfo>,
    pub playout: PlayoutStats,
    /// The play-out delay in force, after any adaptation
    pub delay_frames: u32,
}

/// The links of a session
pub struct Links {
    mux: UdpMux,
    mix: PeerMix,
    settings: LinkSettings,
    /// Our key in the room, which each peer's key exchange is checked against
    identity: LinkIdentity,
    links: Vec<Link>,
    /// Links that have ended, for what the session reports at its end
    ended: Vec<LinkReport>,
    next_serial: u64,
    senders: Senders,
    checks: tokio::sync::mpsc::UnboundedSender<Checked>,
}

impl Links {
    /// The links of a session whose audio socket is `mux`, with the receiver the finished
    /// connectivity checks arrive on (see [`Links::checked`]).
    pub fn new(
        mux: UdpMux,
        mix: PeerMix,
        settings: LinkSettings,
        identity: LinkIdentity,
        senders: Senders,
    ) -> (Self, tokio::sync::mpsc::UnboundedReceiver<Checked>) {
        let (checks, finished) = tokio::sync::mpsc::unbounded_channel();
        (
            Self {
                mux,
                mix,
                settings,
                identity,
                links: Vec::new(),
                ended: Vec::new(),
                next_serial: 0,
                senders,
                checks,
            },
            finished,
        )
    }

    /// Opens a link to `peer`: their place in the mix, and the connectivity check, which runs
    /// on its own so that the other links and the keyboard do not wait for it. `candidates` are
    /// raced, so a peer on the same network is reached directly rather than only through its
    /// public address (REQ-CON-113).
    pub fn start(
        &mut self,
        peer: Uuid,
        name: &str,
        candidates: Vec<SocketAddr>,
        link_key: Option<String>,
    ) -> Result<()> {
        let mut connection = Connection::on(&self.mux)?;
        connection.verify_peer(&self.identity, link_key.as_deref())?;
        connection.set_audio_encoding(self.settings.encoding.clone())?;

        let receive = ReceivePath::new(
            self.settings.encoding.codec_type,
            self.settings.sample_rate,
            self.settings.frame_size,
            self.settings.jitter_buffer_frames,
        )?;
        let voice = self
            .mix
            .add(
                receive.clone(),
                Arc::new(PeerGain::default()),
                Arc::new(PeerMeter::default()),
            )
            .ok_or_else(|| anyhow!("the session has no room for another link"))?;

        let for_audio = receive.clone();
        connection.set_audio_callback(move |sequence, payload, _timestamp| {
            if !for_audio.receive(sequence, &payload) {
                tracing::trace!("Play-out buffer refused frame {}", sequence);
            }
        });

        let for_rate = receive.clone();
        connection.set_latency_info_callback(move |peer| {
            for_rate.follow_peer_channels(peer.channel_count);
            match for_rate.follow_peer_rate(peer.sample_rate) {
                PeerRateChange::Unchanged => {}
                PeerRateChange::Resampling {
                    from,
                    to,
                    latency_ms,
                } => info!(
                    "Converting the peer's audio: {} Hz -> {} Hz (+{:.2} ms)",
                    from, to, latency_ms
                ),
                PeerRateChange::Passthrough => info!("Peer runs at our sample rate"),
                PeerRateChange::Failed(e) => {
                    warn!(
                        "Cannot convert the peer's sample rate, playing as is: {}",
                        e
                    )
                }
            }
        });

        // Sequence numbers carry on across an outage, so what was waiting before it must not
        // play after it (ADR-022).
        let for_state = receive.clone();
        connection.set_state_change_callback(move |state| {
            if state == ConnectionState::Connected {
                for_state.reset();
            }
        });

        self.next_serial += 1;
        let serial = self.next_serial;
        let finished = self.checks.clone();
        let checking = tokio::spawn(async move {
            let result = connection
                .connect_with_candidates(&candidates)
                .await
                .map(|()| connection)
                .map_err(|e| e.to_string());
            let _ = finished.send(Checked {
                peer,
                serial,
                result,
            });
        });

        self.links.push(Link {
            peer,
            name: name.to_string(),
            serial,
            receive,
            _voice: voice,
            stage: Stage::Checking(checking),
        });
        Ok(())
    }

    /// A connectivity check ended. Returns the name of the peer when the link came up.
    pub async fn checked(&mut self, checked: Checked) -> Option<String> {
        let link = self
            .links
            .iter_mut()
            .find(|link| link.peer == checked.peer && link.serial == checked.serial)?;
        match checked.result {
            Ok(connection) => {
                let connection = Arc::new(tokio::sync::Mutex::new(connection));
                link.stage = Stage::Up(connection.clone());
                let name = link.name.clone();
                self.refresh_senders();
                if let Err(e) = connection
                    .lock()
                    .await
                    .send_latency_info(&self.settings.announce)
                    .await
                {
                    warn!("Failed to send latency info: {}", e);
                }
                Some(name)
            }
            Err(e) => {
                warn!("Could not connect to {}: {}", link.name, e);
                link.stage = Stage::Down;
                None
            }
        }
    }

    /// Closes the link to `peer`. The audio with the others goes on.
    pub async fn end(&mut self, peer: Uuid) {
        let Some(at) = self.links.iter().position(|link| link.peer == peer) else {
            return;
        };
        let link = self.links.remove(at);
        self.refresh_senders();
        let report = close(link).await;
        self.ended.push(report);
    }

    /// Closes every link
    pub async fn end_all(&mut self) {
        for link in std::mem::take(&mut self.links) {
            let report = close(link).await;
            self.ended.push(report);
        }
        self.refresh_senders();
    }

    /// How each link is doing, for the roster. A link whose connection is busy sending is left
    /// out: the next time round says.
    pub fn healths(&self) -> Vec<(Uuid, LinkHealth)> {
        self.links
            .iter()
            .filter_map(|link| {
                let health = match &link.stage {
                    Stage::Checking(_) => LinkHealth::Starting,
                    Stage::Down => LinkHealth::Failed,
                    Stage::Up(connection) => {
                        let connection = connection.try_lock().ok()?;
                        LinkHealth::of(connection.state(), connection.stats().packets_received)
                    }
                };
                Some((link.peer, health))
            })
            .collect()
    }

    /// Lets each link's play-out delay follow the link (ADR-031). Call it every
    /// `ADAPT_INTERVAL`.
    pub fn adapt(&self) {
        for link in &self.links {
            if let Some(frames) = link.receive.adapt() {
                info!(
                    "Play-out delay for {} adjusted to {} frame(s)",
                    link.name, frames
                );
            }
        }
    }

    /// What every link looks like now, those that ended first and then those that run, each
    /// in the order they were opened
    pub async fn reports(&self) -> Vec<LinkReport> {
        let mut reports: Vec<LinkReport> = self.ended.clone();
        for link in &self.links {
            reports.push(report_of(link).await);
        }
        reports
    }

    /// Ends every link and returns what they looked like, as [`Links::reports`] does
    pub async fn finish(mut self) -> Vec<LinkReport> {
        self.end_all().await;
        self.ended
    }

    fn refresh_senders(&self) {
        let up: Arc<[Arc<tokio::sync::Mutex<Connection>>]> = self
            .links
            .iter()
            .filter_map(|link| match &link.stage {
                Stage::Up(connection) => Some(connection.clone()),
                _ => None,
            })
            .collect();
        *self.senders.write().unwrap_or_else(|e| e.into_inner()) = up;
    }
}

async fn report_of(link: &Link) -> LinkReport {
    let (addr, stats, peer_info) = match &link.stage {
        Stage::Up(connection) => {
            let connection = connection.lock().await;
            (
                Some(connection.remote_addr()),
                connection.stats(),
                connection.peer_latency_info(),
            )
        }
        _ => (None, ConnectionStats::default(), None),
    };
    LinkReport {
        name: link.name.clone(),
        addr,
        stats,
        peer_info,
        playout: link.receive.stats(),
        delay_frames: link.receive.delay_frames(),
    }
}

/// Ends the link: the check if it is still running, the connection if it is up
async fn close(link: Link) -> LinkReport {
    let report = report_of(&link).await;
    match link.stage {
        Stage::Checking(checking) => checking.abort(),
        Stage::Up(connection) => connection.lock().await.disconnect(),
        Stage::Down => {}
    }
    report
}
