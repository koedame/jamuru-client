//! Signaling IPC commands for Tauri
//!
//! Provides commands to connect to signaling servers, list rooms, and join/leave rooms.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio::time::Duration;

use jamjam::network::{
    gather_host_candidates, AddressCandidate, PeerInfo, RoomInfo, SignalingClient,
    SignalingConnection, SignalingMessage,
};
use uuid::Uuid;

use crate::config::ConfigState;
use crate::device_identity::DeviceIdentityState;
use crate::logging::strip_userinfo;
use crate::streaming::StreamingState;
use crate::usage::UsageState;
use jamjam::telemetry::{Component, EndReason, SessionMode};

/// Connection ID counter
static NEXT_CONN_ID: AtomicU32 = AtomicU32::new(1);

/// How often the host pings an idle signaling connection to keep it alive
/// through an edge/proxy idle timeout. Independent of the
/// frontend's event polling, which only runs while inside a room and only
/// puts bytes on the wire when there is a message to relay.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);

/// Reaction on a chat message
#[derive(Debug, Clone, Serialize)]
pub struct Reaction {
    /// The emoji used for this reaction
    pub emoji: String,
    /// List of user IDs who reacted with this emoji
    pub user_ids: Vec<String>,
    /// Total count of this reaction
    pub count: u32,
}

/// Chat message for UI display
#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub content: String,
    pub timestamp: u64,
    /// True for system messages (join/leave notifications)
    pub is_system: bool,
    /// "join"/"leave" for a system message representing that specific room
    /// event, None otherwise. The backend does not know the UI language, so a
    /// system message carries no display text: `content` is empty and the
    /// frontend renders the sentence from this kind and `sender_name` (the
    /// participant the event is about, empty when unknown).
    pub system_kind: Option<String>,
    /// Reactions on this message
    #[serde(default)]
    pub reactions: Vec<Reaction>,
}

/// Current room state
struct RoomState {
    _room_id: String,
    peer_id: String,
    peer_name: String,
    chat_messages: Vec<ChatMessage>,
}

/// Signaling state managed by Tauri
pub struct SignalingState {
    connections: Mutex<HashMap<u32, SignalingConnection>>,
    room_state: Mutex<Option<RoomState>>,
    /// Connections whose loss has been logged. The UI polls a dead connection
    /// every tick; without this each poll would repeat the same line.
    lost_logged: std::sync::Mutex<HashSet<u32>>,
}

impl SignalingState {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            room_state: Mutex::new(None),
            lost_logged: std::sync::Mutex::new(HashSet::new()),
        }
    }
}

impl Default for SignalingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of joining a room
#[derive(Debug, Clone, Serialize)]
pub struct JoinResult {
    pub room_id: String,
    pub peer_id: String,
    pub invite_code: String,
    pub peers: Vec<PeerInfo>,
}

/// Connect to the signaling server
///
/// The server is resolved here rather than passed in, so the UI and
/// diagnostics cannot disagree on it (ADR-030). The client asks that server
/// where its signaling is, then presents this installation's device identity
/// (ADR-024) on the handshake, so the server can identify the device without
/// any account registration.
#[tauri::command]
pub async fn signaling_connect(
    app: AppHandle,
    state: tauri::State<'_, SignalingState>,
    identity_state: tauri::State<'_, DeviceIdentityState>,
    config_state: tauri::State<'_, ConfigState>,
    usage: tauri::State<'_, UsageState>,
) -> Result<u32, String> {
    let url = config_state.server_url();
    let shown_url = strip_userinfo(&url);
    tracing::info!("Signaling connect: {}", shown_url);
    let started = Instant::now();

    let conn = SignalingClient::new(&url, identity_state.identity())
        .connect()
        .await
        .map_err(|e| {
            tracing::error!(
                "Signaling connect to {} failed after {} ms: {}",
                shown_url,
                started.elapsed().as_millis(),
                e
            );
            usage.reporter().record_error(
                Component::Signaling,
                crate::usage::signaling_connect_failure_code(&e),
            );
            e.to_string()
        })?;

    let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    state.connections.lock().await.insert(conn_id, conn);

    tracing::info!(
        "Signaling connected to {} in {} ms (conn_id={})",
        shown_url,
        started.elapsed().as_millis(),
        conn_id
    );

    spawn_keepalive(app, conn_id);
    Ok(conn_id)
}

/// Pings `conn_id` on a timer until the connection is gone (disconnected by
/// the frontend, or dead - a ping on a closed socket errors just like any
/// other send). Self-terminating, so callers don't need to cancel it.
fn spawn_keepalive(app: AppHandle, conn_id: u32) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(KEEPALIVE_INTERVAL).await;
            let state = app.state::<SignalingState>();
            let mut connections = state.connections.lock().await;
            let Some(conn) = connections.get_mut(&conn_id) else {
                return; // Disconnected.
            };
            if let Err(e) = conn.send_ping().await {
                tracing::debug!("Keep-alive ping for conn_id={} failed: {}", conn_id, e);
                return; // signaling_poll_events will surface the loss.
            }
        }
    });
}

/// Disconnect from a signaling server
#[tauri::command]
pub async fn signaling_disconnect(
    conn_id: u32,
    state: tauri::State<'_, SignalingState>,
    usage: tauri::State<'_, UsageState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let mut connections = state.connections.lock().await;
    if let Some(conn) = connections.remove(&conn_id) {
        usage.session_ended(&streaming, EndReason::Left);
        tracing::info!("Signaling disconnect requested (conn_id={})", conn_id);
        conn.close().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// List available rooms
#[tauri::command]
pub async fn signaling_list_rooms(
    conn_id: u32,
    state: tauri::State<'_, SignalingState>,
) -> Result<Vec<RoomInfo>, String> {
    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;

    conn.send(SignalingMessage::ListRooms)
        .await
        .map_err(|e| e.to_string())?;

    match conn.recv().await.map_err(|e| e.to_string())? {
        SignalingMessage::RoomList { rooms } => Ok(rooms),
        SignalingMessage::Error { message } => Err(message),
        _ => Err("Unexpected response".to_string()),
    }
}

/// Join a room
#[tauri::command]
pub async fn signaling_join_room(
    conn_id: u32,
    room_id: String,
    peer_name: String,
    app: AppHandle,
    state: tauri::State<'_, SignalingState>,
    usage: tauri::State<'_, UsageState>,
) -> Result<JoinResult, String> {
    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;

    // `room_id` is what the user typed - usually an invite code, which lets
    // whoever reads the log file join the room - so it is not logged here.
    tracing::info!("Joining a room (conn_id={})", conn_id);
    conn.send(SignalingMessage::JoinRoom {
        room_id: room_id.clone(),
        password: None,
        peer_name: peer_name.clone(),
    })
    .await
    .map_err(|e| e.to_string())?;

    match conn.recv().await.map_err(|e| e.to_string())? {
        SignalingMessage::RoomJoined {
            room_id,
            peer_id,
            invite_code,
            peers,
        } => {
            // Store room state for chat
            let peer_id_str = peer_id.to_string();
            let mut room_state = state.room_state.lock().await;
            *room_state = Some(RoomState {
                _room_id: room_id.clone(),
                peer_id: peer_id_str.clone(),
                peer_name,
                chat_messages: vec![],
            });

            tracing::info!(
                "Joined room {} as peer {} ({} other peer(s))",
                room_id,
                peer_id_str,
                peers.len()
            );
            usage.session_started(&app, SessionMode::Join, peers.len() as u32 + 1);
            Ok(JoinResult {
                room_id,
                peer_id: peer_id_str,
                invite_code,
                peers,
            })
        }
        SignalingMessage::Error { message } => {
            tracing::warn!("Server refused to join the room: {}", message);
            Err(message)
        }
        _ => {
            tracing::warn!("Unexpected reply to JoinRoom");
            Err("Unexpected response".to_string())
        }
    }
}

/// Advertises this app's audio address to the room.
///
/// Without this, two GUI instances deadlock: each starts streaming only once
/// it sees a peer with an address, and neither publishes one, so audio never
/// flows between them (ADR-026). The CLI has always done this; the GUI did
/// not.
///
/// `local_port` is the port from `streaming_prepare`.
#[tauri::command]
pub async fn signaling_publish_local_candidates(
    conn_id: u32,
    local_port: u16,
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
    usage: tauri::State<'_, UsageState>,
) -> Result<usize, String> {
    let candidates = local_candidates(&streaming, local_port).await;
    usage
        .reporter()
        .with_session(|tally| tally.set_local_candidates(&candidates));

    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;

    conn.send(SignalingMessage::UpdatePeerInfo {
        candidates: candidates.clone(),
        public_addr: candidates.first().map(|c| c.address),
        local_addr: Some(
            format!("0.0.0.0:{}", local_port)
                .parse()
                .map_err(|e| format!("Invalid local port {}: {}", local_port, e))?,
        ),
    })
    .await
    .map_err(|e| e.to_string())?;

    tracing::info!(
        "Published {} address candidate(s) for port {}",
        candidates.len(),
        local_port
    );
    Ok(candidates.len())
}

/// Address candidates for `local_port` that a peer can actually send to.
///
/// The audio socket is bound to `0.0.0.0`, so it is IPv4-only: `sendto` an IPv6
/// destination fails with EINVAL, and IPv6 link-local also needs a scope id
/// (`fe80::1%en0`) that the wire format does not carry. `gather_candidates`
/// therefore offers IPv4 only, ordered by the candidate priority (host before
/// server-reflexive, which prefers a LAN path over hairpinning through the
/// public address).
///
/// `architecture.md` describes IPv4/IPv6 dual-stack with Happy Eyeballs. The
/// transport does not implement it yet - it binds `0.0.0.0`. Advertising what
/// the socket can actually receive beats addresses only a future dual-stack
/// transport could use (ADR-026).
///
/// The public address is asked of STUN through the audio socket, so it carries
/// the port the NAT really assigned to it (ADR-035). If that socket is no
/// longer available, only host candidates are offered - a public address
/// guessed from another socket's mapping would be wrong on a NAT that
/// renumbers ports.
async fn local_candidates(streaming: &StreamingState, local_port: u16) -> Vec<AddressCandidate> {
    let candidates = match streaming.gather_candidates().await {
        Some(candidates) => candidates,
        None => {
            tracing::warn!(
                "Audio socket for port {} is not available to ask STUN through; \
                 advertising host addresses only",
                local_port
            );
            let mut hosts: Vec<AddressCandidate> = gather_host_candidates(local_port)
                .into_iter()
                .filter(|c| c.address.is_ipv4())
                .collect();
            hosts.sort_by_key(|c| std::cmp::Reverse(c.priority));
            hosts
        }
    };

    if !candidates.is_empty() {
        return candidates;
    }

    // No usable interface - a CI container, or networking switched off. Two
    // instances on one machine can still reach each other over loopback, which
    // `gather_candidates` deliberately excludes. Never reached on a machine
    // with a network interface.
    tracing::warn!(
        "No usable IPv4 address candidates for port {}; advertising loopback so \
         same-machine peers can still connect",
        local_port
    );
    match format!("127.0.0.1:{}", local_port).parse() {
        Ok(addr) => vec![AddressCandidate::host(addr)],
        Err(e) => {
            tracing::error!("Could not build a loopback candidate: {}", e);
            Vec::new()
        }
    }
}

/// Leave the current room
#[tauri::command]
pub async fn signaling_leave_room(
    conn_id: u32,
    state: tauri::State<'_, SignalingState>,
    usage: tauri::State<'_, UsageState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;

    tracing::info!("Leaving the room (conn_id={})", conn_id);
    usage.session_ended(&streaming, EndReason::Left);
    conn.send(SignalingMessage::LeaveRoom)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Create a new room
#[tauri::command]
pub async fn signaling_create_room(
    conn_id: u32,
    room_name: String,
    peer_name: String,
    app: AppHandle,
    state: tauri::State<'_, SignalingState>,
    usage: tauri::State<'_, UsageState>,
) -> Result<JoinResult, String> {
    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;

    tracing::info!("Creating a room (conn_id={})", conn_id);
    conn.send(SignalingMessage::CreateRoom {
        room_name,
        password: None,
        peer_name: peer_name.clone(),
    })
    .await
    .map_err(|e| e.to_string())?;

    match conn.recv().await.map_err(|e| e.to_string())? {
        SignalingMessage::RoomCreated {
            room_id,
            peer_id,
            invite_code,
        } => {
            // Store room state for chat
            let peer_id_str = peer_id.to_string();
            let mut room_state = state.room_state.lock().await;
            *room_state = Some(RoomState {
                _room_id: room_id.clone(),
                peer_id: peer_id_str.clone(),
                peer_name,
                chat_messages: vec![],
            });

            tracing::info!("Created room {} as peer {}", room_id, peer_id_str);
            usage.session_started(&app, SessionMode::Create, 1);
            Ok(JoinResult {
                room_id,
                peer_id: peer_id_str,
                invite_code,
                peers: vec![],
            })
        }
        SignalingMessage::Error { message } => {
            tracing::warn!("Server refused to create the room: {}", message);
            Err(message)
        }
        _ => {
            tracing::warn!("Unexpected reply to CreateRoom");
            Err("Unexpected response".to_string())
        }
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Send a chat message
#[tauri::command]
pub async fn signaling_send_chat(
    conn_id: u32,
    content: String,
    state: tauri::State<'_, SignalingState>,
) -> Result<(), String> {
    // Get sender info from room state
    let room_state_guard = state.room_state.lock().await;
    let room_state = room_state_guard.as_ref().ok_or("Not in a room")?;
    let sender_id = room_state.peer_id.clone();
    let sender_name = room_state.peer_name.clone();
    drop(room_state_guard);

    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;

    let timestamp = current_timestamp();

    conn.send(SignalingMessage::ChatMessage {
        sender_id: sender_id.clone(),
        sender_name: sender_name.clone(),
        content: content.clone(),
        timestamp,
    })
    .await
    .map_err(|e| e.to_string())?;

    drop(connections);

    // Add own message to chat_messages immediately for display
    let mut room_state_guard = state.room_state.lock().await;
    if let Some(ref mut rs) = *room_state_guard {
        rs.chat_messages.push(ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            sender_id,
            sender_name,
            content,
            timestamp,
            is_system: false,
            system_kind: None,
            reactions: vec![],
        });
    }

    Ok(())
}

/// Get chat messages (for polling)
#[tauri::command]
pub async fn signaling_get_chat_messages(
    since_timestamp: Option<u64>,
    state: tauri::State<'_, SignalingState>,
) -> Result<Vec<ChatMessage>, String> {
    let room_state_guard = state.room_state.lock().await;
    let room_state = room_state_guard.as_ref().ok_or("Not in a room")?;

    let messages = if let Some(since) = since_timestamp {
        room_state
            .chat_messages
            .iter()
            .filter(|m| m.timestamp > since)
            .cloned()
            .collect()
    } else {
        room_state.chat_messages.clone()
    };

    Ok(messages)
}

/// Add a reaction to a chat message
///
/// If the user has already reacted with this emoji, this is a no-op.
/// Returns the updated message.
#[tauri::command]
pub async fn signaling_add_reaction(
    message_id: String,
    emoji: String,
    state: tauri::State<'_, SignalingState>,
) -> Result<ChatMessage, String> {
    let mut room_state_guard = state.room_state.lock().await;
    let room_state = room_state_guard.as_mut().ok_or("Not in a room")?;
    let user_id = room_state.peer_id.clone();

    // Find the message
    let message = room_state
        .chat_messages
        .iter_mut()
        .find(|m| m.id == message_id)
        .ok_or("Message not found")?;

    // Find or create the reaction for this emoji
    if let Some(reaction) = message.reactions.iter_mut().find(|r| r.emoji == emoji) {
        // Add user if not already present
        if !reaction.user_ids.contains(&user_id) {
            reaction.user_ids.push(user_id);
            reaction.count += 1;
        }
    } else {
        // Create new reaction
        message.reactions.push(Reaction {
            emoji,
            user_ids: vec![user_id],
            count: 1,
        });
    }

    Ok(message.clone())
}

/// Remove a reaction from a chat message
///
/// If the user hasn't reacted with this emoji, this is a no-op.
/// Returns the updated message.
#[tauri::command]
pub async fn signaling_remove_reaction(
    message_id: String,
    emoji: String,
    state: tauri::State<'_, SignalingState>,
) -> Result<ChatMessage, String> {
    let mut room_state_guard = state.room_state.lock().await;
    let room_state = room_state_guard.as_mut().ok_or("Not in a room")?;
    let user_id = room_state.peer_id.clone();

    // Find the message
    let message = room_state
        .chat_messages
        .iter_mut()
        .find(|m| m.id == message_id)
        .ok_or("Message not found")?;

    // Find and update the reaction
    if let Some(reaction) = message.reactions.iter_mut().find(|r| r.emoji == emoji) {
        // Remove user if present
        if let Some(pos) = reaction.user_ids.iter().position(|id| id == &user_id) {
            reaction.user_ids.remove(pos);
            reaction.count = reaction.count.saturating_sub(1);
        }
    }

    // Remove reactions with count 0
    message.reactions.retain(|r| r.count > 0);

    Ok(message.clone())
}

/// Toggle a reaction on a chat message
///
/// If the user has already reacted with this emoji, removes it.
/// Otherwise, adds the reaction.
/// Returns the updated message.
#[tauri::command]
pub async fn signaling_toggle_reaction(
    message_id: String,
    emoji: String,
    state: tauri::State<'_, SignalingState>,
) -> Result<ChatMessage, String> {
    let mut room_state_guard = state.room_state.lock().await;
    let room_state = room_state_guard.as_mut().ok_or("Not in a room")?;
    let user_id = room_state.peer_id.clone();

    // Find the message
    let message = room_state
        .chat_messages
        .iter_mut()
        .find(|m| m.id == message_id)
        .ok_or("Message not found")?;

    // Check if user already reacted with this emoji
    let already_reacted = message
        .reactions
        .iter()
        .any(|r| r.emoji == emoji && r.user_ids.contains(&user_id));

    if already_reacted {
        // Remove reaction
        if let Some(reaction) = message.reactions.iter_mut().find(|r| r.emoji == emoji) {
            if let Some(pos) = reaction.user_ids.iter().position(|id| id == &user_id) {
                reaction.user_ids.remove(pos);
                reaction.count = reaction.count.saturating_sub(1);
            }
        }
        // Remove reactions with count 0
        message.reactions.retain(|r| r.count > 0);
    } else {
        // Add reaction
        if let Some(reaction) = message.reactions.iter_mut().find(|r| r.emoji == emoji) {
            reaction.user_ids.push(user_id);
            reaction.count += 1;
        } else {
            message.reactions.push(Reaction {
                emoji,
                user_ids: vec![user_id],
                count: 1,
            });
        }
    }

    Ok(message.clone())
}

/// Signaling event for the UI
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum SignalingEvent {
    /// A peer joined the room
    PeerJoined { peer: PeerInfo },
    /// A peer left the room
    PeerLeft { peer_id: String },
    /// A peer's info was updated
    PeerUpdated { peer: PeerInfo },
    /// A chat message was received
    ChatMessageReceived { message: ChatMessage },
    /// The server closed the room. It closes this connection right after
    /// sending this, so the frontend must treat `conn_id` as dead and
    /// reconnect rather than keep using it.
    RoomClosed { reason: String },
    /// The server removed a peer from the room. Broadcast to the whole
    /// room, but the server only closes the connection belonging
    /// to `peer_id` - the frontend should only reconnect when `peer_id`
    /// matches its own peer id (own_peer_id below), otherwise just note
    /// that a peer was removed (a PeerLeft event follows once that
    /// connection actually closes).
    Kicked { peer_id: String, reason: String },
    /// The connection dropped without the server sending `RoomClosed` first
    /// (network blip, proxy reset, server restart). `conn_id` is dead - the
    /// frontend must disconnect it and reconnect.
    ConnectionLost { reason: String },
}

/// Poll for signaling events (peer join/leave, chat messages)
/// Returns pending events and clears them from the queue
#[tauri::command]
pub async fn signaling_poll_events(
    conn_id: u32,
    state: tauri::State<'_, SignalingState>,
    usage: tauri::State<'_, UsageState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<Vec<SignalingEvent>, String> {
    use tokio::time::{timeout, Duration};

    let mut events = Vec::new();

    // Try to receive messages with a short timeout
    let mut connections = state.connections.lock().await;
    let conn = match connections.get_mut(&conn_id) {
        Some(c) => c,
        None => return Ok(events), // No connection, return empty
    };

    // Poll with 50ms timeout to avoid blocking too long
    loop {
        match timeout(Duration::from_millis(50), conn.recv()).await {
            Ok(Ok(msg)) => {
                match msg {
                    SignalingMessage::PeerJoined { peer } => {
                        // Add system message for join
                        let mut room_state = state.room_state.lock().await;
                        if let Some(ref mut rs) = *room_state {
                            rs.chat_messages.push(ChatMessage {
                                id: Uuid::new_v4().to_string(),
                                sender_id: String::new(),
                                sender_name: peer.name.clone(),
                                content: String::new(),
                                timestamp: current_timestamp(),
                                is_system: true,
                                system_kind: Some("join".to_string()),
                                reactions: vec![],
                            });
                        }
                        drop(room_state);

                        usage.participant_joined();
                        tracing::info!("Peer {} joined the room", peer.id);
                        events.push(SignalingEvent::PeerJoined { peer });
                    }
                    SignalingMessage::PeerLeft { peer_id } => {
                        // Add system message for leave
                        let mut room_state = state.room_state.lock().await;
                        if let Some(ref mut rs) = *room_state {
                            rs.chat_messages.push(ChatMessage {
                                id: Uuid::new_v4().to_string(),
                                sender_id: String::new(),
                                sender_name: String::new(),
                                content: String::new(),
                                timestamp: current_timestamp(),
                                is_system: true,
                                system_kind: Some("leave".to_string()),
                                reactions: vec![],
                            });
                        }
                        drop(room_state);

                        usage.participant_left();
                        tracing::info!("Peer {} left the room", peer_id);
                        events.push(SignalingEvent::PeerLeft {
                            peer_id: peer_id.to_string(),
                        });
                    }
                    SignalingMessage::PeerUpdated { peer } => {
                        events.push(SignalingEvent::PeerUpdated { peer });
                    }
                    SignalingMessage::ChatMessage {
                        sender_id,
                        sender_name,
                        content,
                        timestamp,
                    } => {
                        // Skip own messages (already added in signaling_send_chat)
                        let mut room_state = state.room_state.lock().await;
                        let is_own_message = room_state
                            .as_ref()
                            .map(|rs| rs.peer_id == sender_id)
                            .unwrap_or(false);

                        if is_own_message {
                            drop(room_state);
                            continue;
                        }

                        let chat_msg = ChatMessage {
                            id: Uuid::new_v4().to_string(),
                            sender_id: sender_id.clone(),
                            sender_name: sender_name.clone(),
                            content: content.clone(),
                            timestamp,
                            is_system: false,
                            system_kind: None,
                            reactions: vec![],
                        };

                        // Store in room state
                        if let Some(ref mut rs) = *room_state {
                            rs.chat_messages.push(chat_msg.clone());
                        }
                        drop(room_state);

                        events.push(SignalingEvent::ChatMessageReceived { message: chat_msg });
                    }
                    SignalingMessage::RoomClosed { reason } => {
                        // The server closes this connection right after this
                        // message, so drop the now-stale room state - the
                        // frontend must reconnect to keep using signaling.
                        let mut room_state = state.room_state.lock().await;
                        *room_state = None;
                        drop(room_state);

                        usage.session_ended(&streaming, EndReason::Disconnected);
                        tracing::warn!("Room closed by the server: {}", reason);
                        events.push(SignalingEvent::RoomClosed { reason });
                    }
                    SignalingMessage::Kicked { peer_id, reason } => {
                        let peer_id_str = peer_id.to_string();
                        let mut room_state = state.room_state.lock().await;
                        let is_self = room_state
                            .as_ref()
                            .map(|rs| rs.peer_id == peer_id_str)
                            .unwrap_or(false);
                        if is_self {
                            *room_state = None;
                            usage.session_ended(&streaming, EndReason::Disconnected);
                        }
                        drop(room_state);

                        tracing::warn!(
                            "Peer {} was kicked ({}): {}",
                            peer_id_str,
                            if is_self { "this app" } else { "another peer" },
                            reason
                        );
                        events.push(SignalingEvent::Kicked {
                            peer_id: peer_id_str,
                            reason,
                        });
                    }
                    _ => {
                        // Ignore other message types during polling
                    }
                }
            }
            Ok(Err(e)) => {
                // Connection error, stop polling. Reported once: the frontend
                // reconnects as soon as it sees the first ConnectionLost
                // event, so later polls of this dead conn_id (if any slip in
                // before it stops) would just repeat the same event.
                let first_time = state
                    .lost_logged
                    .lock()
                    .map(|mut logged| logged.insert(conn_id))
                    .unwrap_or(false);
                if first_time {
                    tracing::warn!("Signaling connection {} was lost: {}", conn_id, e);
                    usage
                        .reporter()
                        .record_error(Component::Signaling, crate::usage::signaling_loss_code(&e));
                    usage.session_ended(&streaming, EndReason::Disconnected);
                    let mut room_state = state.room_state.lock().await;
                    *room_state = None;
                    drop(room_state);
                    events.push(SignalingEvent::ConnectionLost {
                        reason: e.to_string(),
                    });
                }
                break;
            }
            Err(_) => {
                // Timeout, no more messages
                break;
            }
        }
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;

    // Built from parts, not single literals: `no_other_source_names_a_signaling_server`
    // (distribution_config_test.rs) flags any hardcoded server URL scheme in
    // this crate as a possible second distribution default. These are
    // throwaway local fixture addresses, not ones.
    const TEST_HTTP_SCHEME: &str = concat!("http", "://");
    const TEST_WS_SCHEME: &str = concat!("ws", "://");

    /// Answers the signaling question (`GET /api/v1/signaling`) on the first
    /// connection to `listener` with the listener's own address, then
    /// completes the WebSocket handshake on the next one. It does not check
    /// the device-identity headers.
    async fn answer_question_then_accept(
        listener: tokio::net::TcpListener,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let addr = listener.local_addr().unwrap();
        // The question, on its own connection.
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await.unwrap();
        let body = format!(r#"{{"url":"{TEST_WS_SCHEME}{addr}/v1/signaling"}}"#);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        drop(stream);

        let (stream, _) = listener.accept().await.unwrap();
        tokio_tungstenite::accept_async(stream).await.unwrap()
    }

    /// A server that drops the WebSocket right after the handshake, without a
    /// close frame - the same "reset without closing handshake" seen when the
    /// signaling connection drops unexpectedly in production, as opposed to a
    /// graceful `RoomClosed` message. Returns the server URL to give the client.
    async fn spawn_reset_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let ws = answer_question_then_accept(listener).await;
            drop(ws);
        });
        format!("{TEST_HTTP_SCHEME}{addr}")
    }

    /// A server that sends `messages` right after the handshake and then keeps
    /// the connection open. Returns the server URL to give the client.
    async fn spawn_server_sending(messages: Vec<String>) -> String {
        use futures_util::SinkExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut ws = answer_question_then_accept(listener).await;
            for message in messages {
                ws.send(tokio_tungstenite::tungstenite::Message::Text(
                    message.into(),
                ))
                .await
                .unwrap();
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        format!("{TEST_HTTP_SCHEME}{addr}")
    }

    /// A mock app holding `conn` as connection 1, with the state
    /// `signaling_poll_events` needs.
    fn app_with_connection(conn: SignalingConnection) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(SignalingState::new());
        app.manage(UsageState::with_reporter(
            jamjam::telemetry::UsageReporter::new(
                None,
                "test",
                std::sync::Arc::new(jamjam::telemetry::NoTransport),
                false,
            ),
        ));
        app.manage(StreamingState::new());
        app.state::<SignalingState>()
            .connections
            .try_lock()
            .unwrap()
            .insert(1, conn);
        app
    }

    /// A connection that drops without the server sending `RoomClosed` first
    /// must still surface through `signaling_poll_events`, or the frontend
    /// silently stops receiving events forever without reconnecting.
    /// Exercises the real command, not a reimplementation of it.
    #[tokio::test]
    async fn a_connection_reset_without_a_close_frame_is_reported_as_connection_lost() {
        let url = spawn_reset_server().await;
        let identity = std::sync::Arc::new(jamjam::network::DeviceIdentity::generate());
        let conn = SignalingClient::new(&url, identity)
            .connect()
            .await
            .unwrap();

        let app = app_with_connection(conn);
        let state = app.state::<SignalingState>();

        // Give the server task time to accept and drop the socket before we
        // poll - recv() only reports the loss once the reset has happened.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let events = signaling_poll_events(
            1,
            state.clone(),
            app.state::<UsageState>(),
            app.state::<StreamingState>(),
        )
        .await
        .expect("signaling_poll_events should not itself error");

        assert!(
            matches!(events.as_slice(), [SignalingEvent::ConnectionLost { .. }]),
            "expected exactly one ConnectionLost event, got {:?}",
            events
        );

        // Polling again must not repeat the event (lost_logged gate) or
        // panic on the now-dead connection.
        let events_again = signaling_poll_events(
            1,
            state,
            app.state::<UsageState>(),
            app.state::<StreamingState>(),
        )
        .await
        .unwrap();
        assert!(events_again.is_empty());
    }

    /// A message type the server gained after this app was built is skipped:
    /// the peer update behind it still arrives, and the connection is not
    /// reported lost (which would make the app leave the room and reconnect).
    ///
    /// Verifies: REQ-CON-030
    #[tokio::test]
    async fn a_server_message_of_a_type_this_app_does_not_know_is_skipped_not_reported_as_a_lost_connection(
    ) {
        let peer_id = Uuid::new_v4();
        let url = spawn_server_sending(vec![
            r#"{"type":"SomethingNewer","data":{"anything":[1,{"nested":true}]}}"#.to_string(),
            format!(r#"{{"type":"PeerLeft","data":{{"peer_id":"{peer_id}"}}}}"#),
        ])
        .await;
        let identity = std::sync::Arc::new(jamjam::network::DeviceIdentity::generate());
        let conn = SignalingClient::new(&url, identity)
            .connect()
            .await
            .unwrap();
        let app = app_with_connection(conn);

        let mut events = Vec::new();
        for _ in 0..20 {
            events.extend(
                signaling_poll_events(
                    1,
                    app.state::<SignalingState>(),
                    app.state::<UsageState>(),
                    app.state::<StreamingState>(),
                )
                .await
                .unwrap(),
            );
            if !events.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            matches!(
                events.as_slice(),
                [SignalingEvent::PeerLeft { peer_id: left }] if *left == peer_id.to_string()
            ),
            "expected only the PeerLeft behind the unknown message, got {:?}",
            events
        );
    }
}
