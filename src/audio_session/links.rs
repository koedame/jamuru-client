//! The audio link to one participant, as the audio thread holds it.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use uuid::Uuid;

use super::shared::{BandwidthSnapshot, LinkReport, Shared};
use super::LinkTarget;
use crate::audio::{AudioPreset, PeerMix, ReceivePath, VoiceHandle, WIRE_CHANNELS};
use crate::link::{LinkSpec, Opened};
use crate::network::{
    audio_packet_duration_ms, AudioEncodingConfig, BandwidthEstimator, BandwidthStatus,
    BandwidthVerdict, Connection, ConnectionState, PeerLatencyInfo, QualityMonitor, UdpMux,
};

/// Where setting up the link to a participant has got
pub(super) enum LinkStage {
    /// The connectivity check is running. It hands the connection over when
    /// it ends.
    Checking(tokio::task::JoinHandle<Result<Connection, String>>),
    Up(Arc<tokio::sync::Mutex<Connection>>),
    /// The link could not be set up
    Down,
}

/// The audio link to one participant, as the audio thread holds it
pub(super) struct Link {
    pub(super) peer: Uuid,
    pub(super) report: Arc<LinkReport>,
    /// Their audio, from payload to the mix
    pub(super) receive: ReceivePath,
    /// Their place in the mix: they are heard for as long as the link is held
    _voice: VoiceHandle,
    pub(super) stage: LinkStage,
    /// Reports quality changes on the edge rather than on every stats poll
    quality_monitor: QualityMonitor,
    /// Measures what the link actually carries, so one too narrow for PCM can
    /// be reported instead of silently dropping audio (REQ-LAT-120's manual mode)
    bandwidth_estimator: BandwidthEstimator,
    bandwidth_verdict: BandwidthVerdict,
    last_bandwidth_status: Option<BandwidthStatus>,
    /// The play-out delay the display and the peer have been told about. The
    /// loop compares it with the buffer's own and reports the difference, so
    /// a preset switch, an adaptation and a reconnect all reach the display
    /// the same way (ADR-031).
    pub(super) announced_delay_frames: u32,
    /// The channel count the peer has been told we send
    pub(super) announced_channel_count: u8,
    /// The periods of the input and the output the peer has been told about
    pub(super) announced_os_periods: (Option<u32>, Option<u32>),
}

/// What every link of a session is set up with
pub(super) struct LinkSetup<'a> {
    pub(super) mux: &'a UdpMux,
    pub(super) mix: &'a PeerMix,
    pub(super) encoding: &'a AudioEncodingConfig,
    pub(super) sample_rate: u32,
    pub(super) buffer_size: u32,
    pub(super) packet_duration_ms: f32,
    pub(super) shared: &'a Shared,
}

impl LinkSetup<'_> {
    /// Starts an audio link to `target`: their place in the mix, and the
    /// connectivity check, which runs on its own so that the others' audio
    /// and the session's commands do not wait for it.
    pub(super) fn open(
        &self,
        target: LinkTarget,
        jitter_buffer_frames: u32,
    ) -> Result<Link, String> {
        let peer = target.peer;
        let connection = Connection::on(self.mux)
            .map_err(|e| format!("Failed to create the connection: {}", e))?;

        let report = Arc::new(LinkReport::default());
        report.jitter_buffer_delay_us.store(
            (jitter_buffer_frames as f32 * self.packet_duration_ms * 1000.0) as u32,
            Ordering::SeqCst,
        );
        report
            .delay_frames
            .store(jitter_buffer_frames, Ordering::SeqCst);
        report.set_state("connecting", None);

        let gain = self.shared.gain_of(peer);

        let report_for_state = report.clone();
        let reconnect_count = self.shared.reconnect_count.clone();
        let silence_giveups = self.shared.silence_giveups.clone();
        let link_facts = connection.link_facts();
        if let Ok(mut shared) = self.shared.link_facts.write() {
            shared.get_or_insert_with(|| link_facts.clone());
        }
        let spec = LinkSpec {
            mux: self.mux,
            mix: self.mix,
            encoding: self.encoding,
            sample_rate: self.sample_rate,
            frame_size: self.buffer_size,
            jitter_buffer_frames,
        };
        let identity = target
            .link
            .as_ref()
            .map(|(ours, peer_key)| (ours, peer_key.as_deref()));
        let Opened {
            mut connection,
            receive,
            voice,
        } = spec
            .open(
                connection,
                identity,
                gain,
                report.meter.clone(),
                move |state| {
                    tracing::info!("Audio connection to {} state: {:?}", peer, state);
                    if state == ConnectionState::Reconnecting {
                        reconnect_count.fetch_add(1, Ordering::Relaxed);
                    }
                    // Failing to connect at all is reported by the connect call itself;
                    // a failure after the link came up is the peer going silent.
                    if state == ConnectionState::Failed && link_facts.snapshot().route.is_some() {
                        silence_giveups.fetch_add(1, Ordering::Relaxed);
                    }

                    // Publish the transition so the UI can show reconnection progress
                    // and prompt once recovery gives up (REQ-CON-110).
                    report_for_state.set_state(&format!("{:?}", state).to_lowercase(), None);
                },
            )
            .map_err(|e| e.to_string())?;
        if let Ok(mut flights) = self.shared.flights.write() {
            flights.push((peer, receive.flight().clone()));
        }

        // Race all their candidates, so a peer on the same network is reached
        // directly rather than only through its public address (REQ-CON-113).
        let candidates = target.candidates;
        let checking = tokio::spawn(async move {
            connection
                .connect_with_candidates(&candidates)
                .await
                .map_err(|e| e.to_string())?;
            Ok(connection)
        });

        Ok(Link {
            peer,
            report,
            receive,
            _voice: voice,
            stage: LinkStage::Checking(checking),
            quality_monitor: QualityMonitor::new(),
            bandwidth_estimator: BandwidthEstimator::default(),
            bandwidth_verdict: BandwidthVerdict::default(),
            last_bandwidth_status: None,
            announced_delay_frames: jitter_buffer_frames,
            // Told when the link comes up
            announced_channel_count: 0,
            announced_os_periods: (None, None),
        })
    }
}

/// How long one audio packet of a peer lasts, in ms: what the peer `told` of
/// its frame size and sample rate says, and until it has, as long as ours.
/// The play-out buffer counts its delay in the peer's packets.
fn peer_packet_ms(told: Option<&PeerLatencyInfo>, ours: f32) -> f32 {
    match told {
        Some(info) if info.frame_size > 0 && info.sample_rate > 0 => {
            audio_packet_duration_ms(info.frame_size, WIRE_CHANNELS, info.sample_rate)
        }
        _ => ours,
    }
}

impl Link {
    pub(super) fn connection(&self) -> Option<&Arc<tokio::sync::Mutex<Connection>>> {
        match &self.stage {
            LinkStage::Up(connection) => Some(connection),
            _ => None,
        }
    }

    /// The delay their play-out buffer holds, in microseconds, where a packet
    /// of ours lasts `our_packet_ms`.
    pub(super) fn delay_us(&self, our_packet_ms: f32) -> u32 {
        let told = self.report.latency_info.read().ok();
        let packet_ms = peer_packet_ms(told.as_ref().and_then(|info| info.as_ref()), our_packet_ms);
        (self.receive.delay_frames() as f32 * packet_ms * 1000.0) as u32
    }

    /// Publishes what the play-out buffer did and the delay it holds
    pub(super) fn publish_playout(&self) {
        if let Ok(mut playout) = self.report.playout.write() {
            *playout = self.receive.stats();
        }
        self.report
            .delay_frames
            .store(self.receive.delay_frames(), Ordering::SeqCst);
    }

    /// Ends the link: the check if it is still running, the connection if it
    /// is up. What the link looked like at the end is published first, and
    /// returned.
    pub(super) async fn close(self) -> Arc<LinkReport> {
        if let LinkStage::Up(connection) = &self.stage {
            let conn = connection.lock().await;
            if let Ok(mut stats) = self.report.stats.write() {
                *stats = Some(conn.stats());
            }
            if let Some(info) = conn.peer_latency_info() {
                if let Ok(mut told) = self.report.latency_info.write() {
                    *told = Some(info);
                }
            }
            if let Ok(mut addr) = self.report.remote_addr.write() {
                *addr = Some(conn.remote_addr());
            }
        }
        self.publish_playout();
        match self.stage {
            LinkStage::Checking(checking) => checking.abort(),
            LinkStage::Up(connection) => connection.lock().await.disconnect(),
            LinkStage::Down => {}
        }
        self.report
    }

    /// Publishes the link's statistics and what the peer told about its
    /// audio, and follows the peer's sample rate and channels. Does nothing
    /// while the link is not up, or while its connection is busy sending.
    pub(super) fn refresh(&mut self, preset: &AudioPreset, required_bps: f64, sample_rate: u32) {
        let Some(connection) = self.connection().cloned() else {
            return;
        };
        let Ok(conn) = connection.try_lock() else {
            return;
        };
        // The log keeps the start of an id, which tells the links apart
        let peer = self.peer;
        let conn_stats = conn.stats();

        // Log quality transitions once, on the edge (REQ-LAT-107). Held
        // back until the first RTT sample arrives, so a link nobody has
        // measured yet is never logged as "good" (REQ-LAT-130).
        if let Some(rtt_ms) = conn_stats.rtt_ms {
            if let Some(change) = self
                .quality_monitor
                .update(rtt_ms, conn_stats.packet_loss_rate)
            {
                if change.is_degradation() {
                    tracing::warn!(
                        "Connection quality to {} degraded to {} (RTT {:.1}ms, loss {:.2}%)",
                        peer,
                        change.to.as_str(),
                        rtt_ms,
                        conn_stats.packet_loss_rate * 100.0
                    );
                } else {
                    tracing::info!(
                        "Connection quality to {} is now {} (RTT {:.1}ms, loss {:.2}%)",
                        peer,
                        change.to.as_str(),
                        rtt_ms,
                        conn_stats.packet_loss_rate * 100.0
                    );
                }
            }
        }

        // Warn once when the link stops carrying what the preset needs, and once
        // when the peer stops sending anything at all (REQ-LAT-127). The snapshot
        // is published on every completed interval, not only when a status is
        // classified, so a zero-byte interval overwrites (rather than leaves in
        // place) whatever verdict was measured before the peer went quiet.
        //
        // The raw per-interval classification goes through `bandwidth_verdict`
        // before it reaches the snapshot or the log: PCM's requirement has no
        // headroom built in, so a single interval can read insufficient on a
        // perfectly healthy link, and a peer that has only just connected (a
        // test peer that replies after a deliberate delay, or plain connection
        // setup) reads as insufficient rather than idle for its first few
        // seconds (REQ-LAT-128, REQ-LAT-129).
        self.bandwidth_estimator.note_audio(
            conn_stats.audio_packets_received,
            conn_stats.audio_packets_lost,
        );
        if self
            .bandwidth_estimator
            .sample(conn_stats.wire_bytes_received())
            .is_some()
        {
            let raw_status = self.bandwidth_estimator.status();
            let status = self.bandwidth_verdict.confirm(raw_status);
            let measured_bps = self.bandwidth_estimator.current_bps();
            if let Ok(mut snapshot) = self.report.bandwidth.write() {
                *snapshot = Some(BandwidthSnapshot {
                    measured_bps,
                    required_bps,
                    status,
                });
            }

            if status != self.last_bandwidth_status {
                match status {
                    Some(BandwidthStatus::Insufficient) => tracing::warn!(
                        "Link drops the audio of {}: {:.0} kbps arrive where {} needs {:.0} kbps",
                        peer,
                        measured_bps / 1000.0,
                        preset.name(),
                        required_bps / 1000.0
                    ),
                    Some(BandwidthStatus::Marginal) => tracing::warn!(
                        "Link loses some of the audio packets of {} ({:.0} kbps arrive)",
                        peer,
                        measured_bps / 1000.0
                    ),
                    Some(BandwidthStatus::Sufficient) => tracing::info!(
                        "Link bandwidth to {} is sufficient for {}",
                        peer,
                        preset.name()
                    ),
                    None => tracing::warn!("No bytes received from {} in the last interval", peer),
                }
                self.last_bandwidth_status = status;
            }
        }

        if let Ok(mut stats) = self.report.stats.write() {
            *stats = Some(conn_stats);
        }
        if let Ok(mut addr) = self.report.remote_addr.write() {
            *addr = Some(conn.remote_addr());
        }
        // Update peer latency info (ADR-013: sample rate from remote peer)
        if let Some(peer_info) = conn.peer_latency_info() {
            self.receive.follow_peer(&peer_info).log(&peer, sample_rate);
            if let Ok(mut info) = self.report.latency_info.write() {
                *info = Some(peer_info);
            }
        }
    }
}

/// The connections of the links that are up, for the send thread
pub(super) fn senders_of(links: &[Link]) -> Arc<[Arc<tokio::sync::Mutex<Connection>>]> {
    links
        .iter()
        .filter_map(|link| link.connection().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::LinkIdentity;
    use std::sync::atomic::AtomicU64;

    /// The delay shown for a peer is its play-out buffer's depth in packets
    /// times how long a packet of *theirs* lasts. Counted in ours, a peer at
    /// twice our buffer size showed half the delay it had.
    ///
    /// Verifies: REQ-AUD-038
    #[test]
    fn a_peers_packet_lasts_as_long_as_its_own_buffer_size_says_and_as_ours_until_it_has_told() {
        let ours = audio_packet_duration_ms(64, WIRE_CHANNELS, 48_000);
        let told = |frame_size, sample_rate| PeerLatencyInfo {
            capture_buffer_ms: 0.0,
            playback_buffer_ms: 0.0,
            encode_ms: 0.0,
            decode_ms: 0.0,
            jitter_buffer_ms: 0.0,
            frame_size,
            sample_rate,
            codec: "pcm".to_string(),
            channel_count: 2,
        };

        assert_eq!(peer_packet_ms(None, ours), ours);
        assert_eq!(peer_packet_ms(Some(&told(64, 48_000)), ours), ours);
        assert_eq!(peer_packet_ms(Some(&told(128, 48_000)), ours), ours * 2.0);
        // 256 frames go as two packets of 128 (ADR-070)
        assert_eq!(peer_packet_ms(Some(&told(256, 48_000)), ours), ours * 2.0);
        assert_eq!(peer_packet_ms(Some(&told(0, 0)), ours), ours);
    }

    fn setup<'a>(
        mux: &'a UdpMux,
        mix: &'a PeerMix,
        encoding: &'a AudioEncodingConfig,
        shared: &'a Shared,
        buffer_size: u32,
    ) -> LinkSetup<'a> {
        LinkSetup {
            mux,
            mix,
            encoding,
            sample_rate: 48_000,
            buffer_size,
            packet_duration_ms: audio_packet_duration_ms(buffer_size, WIRE_CHANNELS, 48_000),
            shared,
        }
    }

    fn encoding(buffer_size: u32) -> AudioEncodingConfig {
        let preset = AudioPreset::HighQuality;
        AudioEncodingConfig {
            codec_type: preset.codec_type(),
            sample_rate: 48_000,
            channels: WIRE_CHANNELS as u16,
            frame_size: buffer_size,
            bitrate: 0,
            fec_group_size: preset.fec_group_size(),
        }
    }

    /// A link that is still being checked is not one the captured audio goes to, so what is
    /// captured meanwhile is not held for it and cannot reach the peer as a burst of old audio
    /// when the check ends. It is sent to from the moment its check has ended.
    ///
    /// Verifies: REQ-CLI-009
    #[tokio::test]
    async fn audio_captured_while_a_link_is_checked_is_not_held_for_it_and_goes_to_it_once_it_is_up(
    ) {
        let (ours, theirs) = (LinkIdentity::generate(), LinkIdentity::generate());
        let (our_key, their_key) = (ours.public_key(), theirs.public_key());
        let (our_mux, their_mux) = (
            UdpMux::bind("127.0.0.1:0").await.expect("our socket"),
            UdpMux::bind("127.0.0.1:0").await.expect("their socket"),
        );
        let (our_addr, their_addr) = (our_mux.local_addr(), their_mux.local_addr());
        let mix = PeerMix::new(2, 48_000, 128, Arc::new(AtomicU64::new(0)));
        let encoding = encoding(128);
        let shared = Shared::new();
        let mut link = setup(&our_mux, &mix, &encoding, &shared, 128)
            .open(
                LinkTarget {
                    peer: Uuid::new_v4(),
                    candidates: vec![their_addr],
                    link: Some((ours, Some(their_key))),
                },
                AudioPreset::HighQuality.jitter_buffer_frames(),
            )
            .expect("the link starts");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            senders_of(std::slice::from_ref(&link)).is_empty(),
            "nobody has answered, so the link is not up"
        );

        let mut ben = Connection::on(&their_mux).expect("their link");
        ben.verify_peer(&theirs, Some(&our_key)).expect("our key");
        ben.connect(our_addr).await.expect("they answer");
        let LinkStage::Checking(checking) = &mut link.stage else {
            panic!("the link is being checked");
        };
        let connection = tokio::time::timeout(std::time::Duration::from_secs(10), checking)
            .await
            .expect("the check ends")
            .expect("the check runs")
            .expect("the check succeeds");
        link.stage = LinkStage::Up(Arc::new(tokio::sync::Mutex::new(connection)));

        assert_eq!(
            senders_of(std::slice::from_ref(&link)).len(),
            1,
            "the link is sent to once its check has ended"
        );
    }

    /// The delay told to the peer counts packets, not frames: a frame too big for one packet
    /// goes as several, so the buffer's depth lasts packets long (ADR-070).
    ///
    /// Verifies: REQ-CLI-010
    #[tokio::test]
    async fn jitter_buffer_told_to_the_peer_counts_packets_not_frames() {
        let mux = UdpMux::bind("127.0.0.1:0").await.expect("a socket");
        let preset = AudioPreset::HighQuality;
        let depth = preset.jitter_buffer_frames() as f32;
        let delay_ms = |buffer_size: u32| {
            let mix = PeerMix::new(2, 48_000, buffer_size, Arc::new(AtomicU64::new(0)));
            let encoding = encoding(buffer_size);
            let shared = Shared::new();
            let setup = setup(&mux, &mix, &encoding, &shared, buffer_size);
            let link = setup
                .open(
                    LinkTarget {
                        peer: Uuid::new_v4(),
                        candidates: vec!["127.0.0.1:9".parse().unwrap()],
                        link: None,
                    },
                    preset.jitter_buffer_frames(),
                )
                .expect("the link starts");
            link.delay_us(setup.packet_duration_ms) as f32 / 1000.0
        };

        // 256 frames go as two packets of 128: a packet lasts 2.667 ms, not the frame's 5.333 ms
        let big = delay_ms(256);
        assert!(
            (big - depth * 128.0 / 48.0).abs() < 0.01,
            "256 frames: got {big} ms"
        );
        let small = delay_ms(64);
        assert!(
            (small - depth * 64.0 / 48.0).abs() < 0.01,
            "64 frames: got {small} ms"
        );
    }
}
