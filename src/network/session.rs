//! Session manager for group P2P audio sessions
//!
//! Manages multiple peer connections and audio mixing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::encryption::{LinkIdentity, LinkSecurity, Opened, SecureLink};
use super::error::NetworkError;
use super::signaling::PeerInfo;
use super::transport::UdpTransport;
use crate::protocol::{Packet, PacketType};

/// How often our key is sent to a peer that has not shown it has it
const KEY_EXCHANGE_INTERVAL: Duration = Duration::from_millis(250);

/// Session configuration
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Local UDP port (0 for auto-assign)
    pub local_port: u16,
    /// Maximum number of peers
    pub max_peers: usize,
    /// Enable audio mixing (combine all peer audio)
    pub enable_mixing: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            local_port: 0,
            max_peers: 10,
            enable_mixing: true,
        }
    }
}

/// Peer state in the session
struct Peer {
    info: PeerInfo,
    addr: SocketAddr,
    connected: AtomicBool,
    packets_received: AtomicU32,
    last_audio: Option<Vec<f32>>,
    /// The encryption of the link to this peer: nothing is sent to it, or taken from it,
    /// without going through this
    link: Arc<SecureLink>,
}

/// Audio callback for received audio from a peer
pub type PeerAudioCallback = Box<dyn Fn(Uuid, &[f32], u32) + Send + Sync + 'static>;

/// Mixed audio callback
pub type MixedAudioCallback = Box<dyn Fn(&[f32], u32) + Send + Sync + 'static>;

/// A multi-peer P2P audio session
pub struct Session {
    transport: Arc<UdpTransport>,
    peers: Arc<RwLock<HashMap<Uuid, Peer>>>,
    config: SessionConfig,
    running: Arc<AtomicBool>,
    sequence: AtomicU32,
    local_peer_id: Uuid,
    peer_audio_callback: Option<Arc<PeerAudioCallback>>,
    mixed_audio_callback: Option<Arc<MixedAudioCallback>>,
    receive_handle: Option<tokio::task::JoinHandle<()>>,
    /// Inner receive loop handle from UdpTransport (must be aborted to release socket)
    inner_recv_handle: Option<tokio::task::JoinHandle<()>>,
    /// Sends our key to the peers that have not shown they have it
    key_exchange_handle: Option<tokio::task::JoinHandle<()>>,
    /// The key our key exchanges are signed with, when the links to the peers added are to
    /// check who they are
    link_identity: Option<LinkIdentity>,
}

impl Session {
    /// Create a new session
    pub async fn new(config: SessionConfig) -> Result<Self, NetworkError> {
        let local_addr = format!("0.0.0.0:{}", config.local_port);
        let transport = UdpTransport::bind(&local_addr).await?;

        Ok(Self {
            transport: Arc::new(transport),
            peers: Arc::new(RwLock::new(HashMap::new())),
            config,
            running: Arc::new(AtomicBool::new(false)),
            sequence: AtomicU32::new(0),
            local_peer_id: Uuid::new_v4(),
            peer_audio_callback: None,
            mixed_audio_callback: None,
            receive_handle: None,
            inner_recv_handle: None,
            key_exchange_handle: None,
            link_identity: None,
        })
    }

    /// Has the links to the peers added from now on check who each peer is, by the key in
    /// its [`PeerInfo::link_key`] and signing our key exchange with `identity`. A peer that
    /// told no key gets a link that is encrypted but not checked.
    pub fn set_link_identity(&mut self, identity: LinkIdentity) {
        self.link_identity = Some(identity);
    }

    /// Get local peer ID
    pub fn local_peer_id(&self) -> Uuid {
        self.local_peer_id
    }

    /// Get local address
    pub fn local_addr(&self) -> SocketAddr {
        self.transport.local_addr()
    }

    /// Add a peer to the session
    pub async fn add_peer(&self, info: PeerInfo, addr: SocketAddr) -> Result<(), NetworkError> {
        let mut peers = self.peers.write().await;

        if peers.len() >= self.config.max_peers {
            return Err(NetworkError::SessionFull);
        }

        if peers.contains_key(&info.id) {
            return Ok(()); // Already added
        }

        info!("Adding peer {} ({}) at {}", info.name, info.id, addr);

        let link = Arc::new(SecureLink::for_peer(
            self.link_identity.as_ref(),
            info.link_key.as_deref(),
        ));
        peers.insert(
            info.id,
            Peer {
                info,
                addr,
                connected: AtomicBool::new(true),
                packets_received: AtomicU32::new(0),
                last_audio: None,
                link,
            },
        );

        Ok(())
    }

    /// Remove a peer from the session
    pub async fn remove_peer(&self, peer_id: Uuid) {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.remove(&peer_id) {
            info!("Removed peer {} ({})", peer.info.name, peer_id);
        }
    }

    /// Whether what is sent to and taken from `peer_id` is encrypted
    pub async fn peer_security(&self, peer_id: Uuid) -> Option<LinkSecurity> {
        let peers = self.peers.read().await;
        peers.get(&peer_id).map(|peer| peer.link.security())
    }

    /// Get list of connected peers
    pub async fn peers(&self) -> Vec<PeerInfo> {
        let peers = self.peers.read().await;
        peers.values().map(|p| p.info.clone()).collect()
    }

    /// Set callback for individual peer audio
    pub fn set_peer_audio_callback<F>(&mut self, callback: F)
    where
        F: Fn(Uuid, &[f32], u32) + Send + Sync + 'static,
    {
        self.peer_audio_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Set callback for mixed audio from all peers
    pub fn set_mixed_audio_callback<F>(&mut self, callback: F)
    where
        F: Fn(&[f32], u32) + Send + Sync + 'static,
    {
        self.mixed_audio_callback = Some(Arc::new(Box::new(callback)));
    }

    /// Start the session
    ///
    /// # Thread Safety
    /// This method takes `&mut self`, ensuring exclusive access.
    /// The `running` flag is set atomically before spawning the receive loop.
    pub fn start(&mut self) {
        if self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(true, Ordering::SeqCst);
        self.start_receive_loop();
        self.start_key_exchange_loop();
        info!("Session started on {}", self.transport.local_addr());
    }

    /// Stop the session
    ///
    /// # Thread Safety
    /// This method takes `&mut self`, ensuring exclusive access.
    /// The `running` flag is set to false atomically, which signals the
    /// receive loop to terminate. The abort is a fallback.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        // Abort inner receive loop first (holds socket reference)
        if let Some(handle) = self.inner_recv_handle.take() {
            handle.abort();
        }

        // Then abort outer receive loop
        if let Some(handle) = self.receive_handle.take() {
            handle.abort();
        }

        if let Some(handle) = self.key_exchange_handle.take() {
            handle.abort();
        }

        info!("Session stopped");
    }

    /// Send audio to all peers
    pub async fn broadcast_audio(&self, data: &[f32], timestamp: u32) -> Result<(), NetworkError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(NetworkError::NotConnected);
        }

        // Convert f32 samples to bytes
        let bytes: Vec<u8> = data.iter().flat_map(|&s| s.to_le_bytes()).collect();
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let packet = Packet::audio(sequence, timestamp, bytes);

        // Send to all peers, each through its own link: a peer whose keys are not agreed
        // yet gets nothing.
        let peers = self.peers.read().await;
        for peer in peers.values() {
            if peer.connected.load(Ordering::SeqCst) {
                let Some(sealed) = peer.link.seal(packet.clone()) else {
                    continue;
                };
                if let Err(e) = self.transport.send_to(&sealed, peer.addr).await {
                    warn!("Failed to send to peer {}: {}", peer.info.id, e);
                }
            }
        }

        Ok(())
    }

    /// Send audio to a specific peer
    pub async fn send_audio_to(
        &self,
        peer_id: Uuid,
        data: &[f32],
        timestamp: u32,
    ) -> Result<(), NetworkError> {
        let peers = self.peers.read().await;
        let peer = peers
            .get(&peer_id)
            .ok_or_else(|| NetworkError::PeerNotFound(peer_id.to_string()))?;

        let bytes: Vec<u8> = data.iter().flat_map(|&s| s.to_le_bytes()).collect();
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let packet = Packet::audio(sequence, timestamp, bytes);

        if let Some(sealed) = peer.link.seal(packet) {
            self.transport.send_to(&sealed, peer.addr).await?;
        }
        Ok(())
    }

    /// Sends our key to each peer that has not shown it has it, until it has
    fn start_key_exchange_loop(&mut self) {
        let transport = self.transport.clone();
        let peers = self.peers.clone();
        let running = self.running.clone();

        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(KEY_EXCHANGE_INTERVAL);
            while running.load(Ordering::SeqCst) {
                ticker.tick().await;
                let waiting: Vec<(SocketAddr, Arc<SecureLink>)> = peers
                    .read()
                    .await
                    .values()
                    .filter(|peer| peer.link.wants_key_exchange())
                    .map(|peer| (peer.addr, peer.link.clone()))
                    .collect();
                for (addr, link) in waiting {
                    if let Err(e) = transport.send_to(&link.key_exchange_packet(), addr).await {
                        warn!("Failed to send our key to {}: {}", addr, e);
                    }
                    // With the keys ours, something encrypted shows the peer it has them too
                    if let Some(keep_alive) =
                        link.seal(Packet::keep_alive(0)).filter(|_| link.has_keys())
                    {
                        let _ = transport.send_to(&keep_alive, addr).await;
                    }
                }
            }
        });

        self.key_exchange_handle = Some(handle);
    }

    fn start_receive_loop(&mut self) {
        let transport = self.transport.clone();
        let peers = self.peers.clone();
        let running = self.running.clone();
        let peer_callback = self.peer_audio_callback.clone();
        let mixed_callback = self.mixed_audio_callback.clone();
        let enable_mixing = self.config.enable_mixing;

        // Start inner receive loop and store handle for cleanup
        let (mut rx, inner_handle) = transport.clone().start_receive_loop();
        self.inner_recv_handle = Some(inner_handle);

        let handle = tokio::spawn(async move {
            while let Some((packet, addr)) = rx.recv().await {
                if !running.load(Ordering::SeqCst) {
                    break;
                }

                // Find peer by address
                let mut peers_guard = peers.write().await;
                let peer_id = {
                    let peer = peers_guard.values().find(|p| p.addr == addr);
                    peer.map(|p| p.info.id)
                };

                // Only a peer's own packets are taken, and they go through its link: one
                // that fails to open, or that repeats an earlier packet, is dropped there.
                let packet = match peer_id.and_then(|id| peers_guard.get(&id)) {
                    Some(peer) => match peer.link.open(packet) {
                        Opened::Packet(packet) => packet,
                        Opened::KeyExchange { answer } => {
                            if answer {
                                let key = peer.link.key_exchange_packet();
                                if let Err(e) = transport.send_to(&key, addr).await {
                                    warn!("Failed to answer {}'s key: {}", addr, e);
                                }
                                if let Some(keep_alive) = peer.link.seal(Packet::keep_alive(0)) {
                                    let _ = transport.send_to(&keep_alive, addr).await;
                                }
                            }
                            continue;
                        }
                        Opened::Dropped(reason) => {
                            debug!("Dropped a packet from {}: {}", addr, reason);
                            continue;
                        }
                    },
                    None => {
                        debug!("Received a packet from unknown address: {}", addr);
                        continue;
                    }
                };
                if packet.packet_type != PacketType::Audio {
                    continue;
                }

                if let Some(peer_id) = peer_id {
                    // Convert bytes to f32 samples
                    let samples: Vec<f32> = packet
                        .payload
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|chunk| f32::from_le_bytes(*chunk))
                        .collect();

                    // Update peer's last audio
                    if let Some(peer) = peers_guard.get_mut(&peer_id) {
                        peer.packets_received.fetch_add(1, Ordering::Relaxed);
                        peer.last_audio = Some(samples.clone());
                    }

                    // Call per-peer callback
                    if let Some(ref callback) = peer_callback {
                        callback(peer_id, &samples, packet.timestamp);
                    }

                    // Mix audio from all peers if enabled
                    if enable_mixing {
                        if let Some(ref callback) = mixed_callback {
                            let mixed = mix_audio(&peers_guard);
                            if !mixed.is_empty() {
                                callback(&mixed, packet.timestamp);
                            }
                        }
                    }
                }
            }
        });

        self.receive_handle = Some(handle);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Mix audio from all peers
fn mix_audio(peers: &HashMap<Uuid, Peer>) -> Vec<f32> {
    let audio_buffers: Vec<&Vec<f32>> = peers
        .values()
        .filter_map(|p| p.last_audio.as_ref())
        .collect();

    if audio_buffers.is_empty() {
        return Vec::new();
    }

    // Find the maximum length
    let max_len = audio_buffers.iter().map(|b| b.len()).max().unwrap_or(0);

    if max_len == 0 {
        return Vec::new();
    }

    // Mix all buffers
    let mut mixed = vec![0.0f32; max_len];
    let num_sources = audio_buffers.len() as f32;

    for buffer in &audio_buffers {
        for (i, &sample) in buffer.iter().enumerate() {
            mixed[i] += sample / num_sources;
        }
    }

    // Soft clip to prevent clipping
    for sample in &mut mixed {
        *sample = soft_clip(*sample);
    }

    mixed
}

/// Soft clipping function to prevent harsh distortion
fn soft_clip(x: f32) -> f32 {
    if x.abs() < 0.5 {
        x
    } else {
        x.signum() * (1.0 - (-4.0 * (x.abs() - 0.5)).exp() * 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soft_clip() {
        // Values below threshold pass through
        assert!((soft_clip(0.3) - 0.3).abs() < 0.001);
        assert!((soft_clip(-0.3) - (-0.3)).abs() < 0.001);

        // Values above threshold are compressed
        let clipped = soft_clip(1.0);
        assert!(clipped < 1.0);
        assert!(clipped > 0.5);
    }

    #[test]
    fn test_mix_audio_empty() {
        let peers: HashMap<Uuid, Peer> = HashMap::new();
        let mixed = mix_audio(&peers);
        assert!(mixed.is_empty());
    }

    #[tokio::test]
    async fn test_session_creation() {
        let config = SessionConfig::default();
        let session = Session::new(config).await.unwrap();
        assert!(session.local_addr().port() > 0);
    }
}
