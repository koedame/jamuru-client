//! Asking a participant to buffer your audio for longer, and answering such an ask (ADR-097)
//!
//! `BufferRequests` decides; this sends what it decides over the room's signaling connection as
//! peer messages, sets the receive buffer it says to set, and hands its events to the UI with the
//! rest of the room's events.

use std::collections::VecDeque;
use std::sync::MutexGuard;

use jamjam::audio::{BufferTerms, GrantView};
use jamjam::network::buffer_request::{
    BufferMessage, BufferRequestError, BufferRequests, Out, Policy, Standing,
};

use super::*;

/// What the UI hears of asks, as a window event
pub const BUFFER_REQUEST_EVENT: &str = "buffer-request";

/// The most peer messages these asks put on the connection in [`BUDGET_WINDOW`]. The server
/// closes a connection that sends more than thirty in ten seconds, and the chat, a help and the
/// recording notice share it.
const BUDGET: usize = 15;
const BUDGET_WINDOW: Duration = Duration::from_secs(10);

fn lock(state: &SignalingState) -> MutexGuard<'_, BufferRequests> {
    state.buffer_asks.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether one more message may go out now.
fn within_budget(state: &SignalingState, now: Instant) -> bool {
    let mut sent = state.buffer_sent.lock().unwrap_or_else(|e| e.into_inner());
    while sent
        .front()
        .is_some_and(|at| now.duration_since(*at) >= BUDGET_WINDOW)
    {
        sent.pop_front();
    }
    if sent.len() >= BUDGET {
        return false;
    }
    sent.push_back(now);
    true
}

/// Sends `message` to `to`. A failed send is logged only: a lost connection surfaces on the next
/// poll, which ends the asks.
async fn send(
    conn: &mut SignalingConnection,
    state: &SignalingState,
    to: Uuid,
    message: BufferMessage,
) {
    if !within_budget(state, Instant::now()) {
        tracing::warn!("Held back a buffer message: too many went out just now");
        return;
    }
    let body = match serde_json::to_value(PeerBody::BufferRequest(message)) {
        Ok(serde_json::Value::Object(body)) => body,
        other => {
            tracing::error!("A buffer message did not encode as an object: {:?}", other);
            return;
        }
    };
    let sent = conn
        .send(SignalingMessage::PeerMessage {
            to: Some(to),
            from: None,
            from_name: None,
            body,
        })
        .await;
    if let Err(e) = sent {
        tracing::warn!("Could not send a buffer message: {}", e);
    }
}

/// Carries out what `BufferRequests` decided: messages go out, buffers are set (and what they
/// came to is told back), and the events are for the UI.
pub(super) async fn carry_out(
    conn: &mut SignalingConnection,
    state: &SignalingState,
    streaming: &StreamingState,
    outs: Vec<Out>,
    now: Instant,
) -> Vec<SignalingEvent> {
    let mut events = Vec::new();
    let mut queue: VecDeque<Out> = outs.into();
    while let Some(out) = queue.pop_front() {
        match out {
            Out::Send { to, message } => send(conn, state, to, message).await,
            Out::Event(event) => events.push(SignalingEvent::BufferRequest { event }),
            Out::Apply { peer, terms } => {
                tracing::info!("The receive buffer of {} is set to {:?}", peer, terms);
                let view = streaming.set_peer_buffer_grant(peer, terms);
                if let Some(view) = view {
                    let more = lock(state).applied(peer, view, now);
                    queue.extend(more);
                }
            }
        }
    }
    events
}

/// A buffer message arrived from `from`.
pub(super) async fn received(
    conn: &mut SignalingConnection,
    state: &SignalingState,
    streaming: &StreamingState,
    from: Uuid,
    from_name: &str,
    message: BufferMessage,
) -> Vec<SignalingEvent> {
    let now = Instant::now();
    let outs = lock(state).receive(from, from_name, message, now);
    carry_out(conn, state, streaming, outs, now).await
}

/// Time passes: asks go out again, how they stand is asked, and the buffers' state is kept for
/// the next answer.
pub(super) async fn tick(
    conn: &mut SignalingConnection,
    state: &SignalingState,
    streaming: &StreamingState,
) -> Vec<SignalingEvent> {
    let now = Instant::now();
    let held = lock(state).held();
    for (peer, _) in held {
        if let Some(view) = streaming.peer_buffer(peer).1.granted {
            lock(state).refresh(peer, view);
        }
    }
    let outs = lock(state).tick(now);
    carry_out(conn, state, streaming, outs, now).await
}

/// A participant left the room, or this app did: what was asked is over.
pub(super) fn peer_left_outs(state: &SignalingState, peer: Uuid) -> Vec<Out> {
    lock(state).peer_left(peer)
}

/// Only the events among `outs`, and the buffers they put back, when there is no one to send to
/// (this app left or lost the connection).
pub(super) fn end_all(state: &SignalingState, streaming: &StreamingState) -> Vec<SignalingEvent> {
    let outs = lock(state).reset();
    let mut events = Vec::new();
    for out in outs {
        match out {
            Out::Event(event) => events.push(SignalingEvent::BufferRequest { event }),
            Out::Apply { peer, terms } => {
                streaming.set_peer_buffer_grant(peer, terms);
            }
            Out::Send { .. } => {}
        }
    }
    events
}

/// Runs an action on `conn_id`: `decide` changes the asks, and what it decided is carried out at
/// once; its events reach the UI on the next poll.
async fn act<T>(
    state: &SignalingState,
    streaming: &StreamingState,
    conn_id: u32,
    decide: impl FnOnce(&mut BufferRequests, Instant) -> Result<(T, Vec<Out>), BufferRequestError>,
) -> Result<T, String> {
    let now = Instant::now();
    let (value, outs) = decide(&mut lock(state), now).map_err(|e| e.to_string())?;
    let mut connections = state.connections.lock().await;
    let conn = connections
        .get_mut(&conn_id)
        .ok_or("Connection not found")?;
    let events = carry_out(conn, state, streaming, outs, now).await;
    drop(connections);
    queue_events(state, events);
    Ok(value)
}

fn parse_peer(peer_id: &str) -> Result<Uuid, String> {
    Uuid::parse_str(peer_id).map_err(|e| format!("Invalid peer id: {}", e))
}

/// Asks `peer_id` to hold the buffer their side plays this app's audio from to `terms`.
#[tauri::command]
pub async fn buffer_request_ask(
    conn_id: u32,
    peer_id: String,
    terms: BufferTerms,
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let peer = parse_peer(&peer_id)?;
    act(&state, &streaming, conn_id, |asks, now| {
        Ok(((), asks.ask(peer, terms, now)?))
    })
    .await
}

/// Takes the ask of `peer_id` back, or clears what is shown of one that ended.
#[tauri::command]
pub async fn buffer_request_withdraw(
    conn_id: u32,
    peer_id: String,
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let peer = parse_peer(&peer_id)?;
    act(&state, &streaming, conn_id, |asks, _| {
        Ok(((), asks.withdraw(peer)?))
    })
    .await
}

/// Answers the ask of `peer_id` the user was shown: take it up, or turn it down.
#[tauri::command]
pub async fn buffer_request_answer(
    conn_id: u32,
    peer_id: String,
    accept: bool,
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let peer = parse_peer(&peer_id)?;
    act(&state, &streaming, conn_id, |asks, now| {
        let outs = if accept {
            asks.accept(peer, now)?
        } else {
            asks.decline(peer)?
        };
        Ok(((), outs))
    })
    .await
}

/// Puts the buffer for `peer_id` back to what it was before they asked.
#[tauri::command]
pub async fn buffer_request_restore(
    conn_id: u32,
    peer_id: String,
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Result<(), String> {
    let peer = parse_peer(&peer_id)?;
    act(&state, &streaming, conn_id, |asks, now| {
        Ok(((), asks.restore(peer, now)?))
    })
    .await
}

/// An ask this app made, as the UI draws it
#[derive(Debug, Clone, Serialize)]
pub struct AskedOf {
    pub peer_id: String,
    pub terms: BufferTerms,
    pub standing: Standing,
}

/// An ask waiting for this app's user
#[derive(Debug, Clone, Serialize)]
pub struct Prompt {
    pub peer_id: String,
    pub peer_name: String,
    pub terms: BufferTerms,
}

/// A buffer this app holds for a participant who asked
#[derive(Debug, Clone, Serialize)]
pub struct HeldFor {
    pub peer_id: String,
    pub terms: BufferTerms,
    pub applied: Option<GrantView>,
}

/// Where the asks stand, for a window that opens after they began
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub policy: Policy,
    pub asked: Vec<AskedOf>,
    pub prompts: Vec<Prompt>,
    pub held: Vec<HeldFor>,
}

/// Where the asks stand now.
#[tauri::command]
pub fn buffer_request_snapshot(
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
) -> Snapshot {
    let asks = lock(&state);
    Snapshot {
        policy: asks.policy(),
        asked: asks
            .asked()
            .into_iter()
            .map(|(peer, terms, standing)| AskedOf {
                peer_id: peer.to_string(),
                terms,
                standing,
            })
            .collect(),
        prompts: asks
            .prompts()
            .into_iter()
            .map(|(peer, peer_name, terms)| Prompt {
                peer_id: peer.to_string(),
                peer_name,
                terms,
            })
            .collect(),
        held: asks
            .held()
            .into_iter()
            .map(|(peer, terms)| HeldFor {
                peer_id: peer.to_string(),
                terms,
                applied: streaming.peer_buffer(peer).1.granted,
            })
            .collect(),
    }
}

/// Chooses whether this app takes asks, and whether it asks its user about each. Kept in the
/// settings; an ask waiting for an answer is turned down when asks are no longer taken.
#[tauri::command]
pub async fn buffer_request_set_policy(
    policy: Policy,
    state: tauri::State<'_, SignalingState>,
    streaming: tauri::State<'_, StreamingState>,
    config: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    config.modify(|config| config.buffer_asks = policy)?;
    let outs = lock(&state).set_policy(policy);
    let conn_id = *state
        .buffer_asks_conn
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let Some(conn_id) = conn_id else {
        queue_events(&state, end_events(outs));
        return Ok(());
    };
    let mut connections = state.connections.lock().await;
    if let Some(conn) = connections.get_mut(&conn_id) {
        let events = carry_out(conn, &state, &streaming, outs, now).await;
        drop(connections);
        queue_events(&state, events);
    } else {
        drop(connections);
        queue_events(&state, end_events(outs));
    }
    Ok(())
}

/// The events among `outs`.
fn end_events(outs: Vec<Out>) -> Vec<SignalingEvent> {
    outs.into_iter()
        .filter_map(|out| match out {
            Out::Event(event) => Some(SignalingEvent::BufferRequest { event }),
            _ => None,
        })
        .collect()
}

impl SignalingState {
    /// Sets whether asks are taken at the start, from the settings file.
    pub fn init_buffer_policy(&self, policy: Policy) {
        lock(self).set_policy(policy);
    }
}
