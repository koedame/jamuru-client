//! Which links went quiet and whose line it is (ADR-112, REQ-CON-162..164)
//!
//! `Dropouts` decides; this feeds it what the audio links hear and how the signaling connection
//! stands, sends what it decides over the room's signaling connection, and hands the result to
//! the status the screen reads.

use jamjam::network::dropout::{
    Dropouts, Heard, Line, LinkView, Network, DROPOUT_AFTER, SERVER_PING_EVERY, SERVER_PING_SLACK,
};
use jamjam::network::has_default_route;
use jamjam::network::settings_help::PeerBody;

use super::*;

/// The most messages the dropout logic puts on the connection in [`BUDGET_WINDOW`]: what it tells
/// the room and its questions to the server. The server closes a connection that sends more than
/// thirty in ten seconds, and the chat, a help, the buffer asks and the recording notice share it.
const BUDGET: usize = 6;
const BUDGET_WINDOW: Duration = Duration::from_secs(10);

/// How long what the system said of the network is taken to hold. Asking it can mean running a
/// program, so it is not asked at every tick.
const NETWORK_FRESH_FOR: Duration = Duration::from_secs(2);

/// Whether the signaling connection has been quiet for longer than a ping may be late: nothing
/// came in, though the server answers a ping at once.
pub(super) fn connection_is_dead(conn: &SignalingConnection) -> bool {
    conn.quiet_for() > SERVER_PING_EVERY + SERVER_PING_SLACK
}

fn lock(state: &SignalingState) -> std::sync::MutexGuard<'_, Dropouts> {
    state.dropouts.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether one more message may go out now.
fn within_budget(state: &SignalingState, now: Instant) -> bool {
    let mut sent = state.dropout_sent.lock().unwrap_or_else(|e| e.into_inner());
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

/// What the system says of the machine's network, asked no more often than
/// [`NETWORK_FRESH_FOR`].
async fn network_now(state: &SignalingState, now: Instant) -> Network {
    let known = *state.network.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, network)) = known {
        if now.duration_since(at) < NETWORK_FRESH_FOR {
            return network;
        }
    }
    let network = tokio::task::spawn_blocking(has_default_route)
        .await
        .ok()
        .flatten()
        .map_or(Network::Unknown, |route| {
            if route {
                Network::Online
            } else {
                Network::Offline
            }
        });
    *state.network.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, network));
    network
}

/// Takes what the links hear now, decides which are quiet and on whose side, tells the room and
/// the server what that calls for, and publishes the result for the status. Called on every poll
/// of the connection of the room.
pub(super) async fn tick(
    conn: &mut SignalingConnection,
    state: &SignalingState,
    streaming: &StreamingState,
    in_room: bool,
) {
    let now = Instant::now();
    let heard: Vec<Heard> = if in_room {
        streaming.link_last_heard()
    } else {
        Vec::new()
    };
    if heard.is_empty() {
        *lock(state) = Dropouts::new();
        streaming.set_standing(HashMap::new());
        return;
    }

    let someone_quiet = heard.iter().any(|link| {
        link.at
            .is_some_and(|at| now.duration_since(at) >= DROPOUT_AFTER)
    });
    let network = if someone_quiet {
        network_now(state, now).await
    } else {
        Network::Unknown
    };
    let line = if connection_is_dead(conn) {
        Line::Down
    } else {
        Line::Up
    };
    let tick = lock(state).tick(now, &heard, line, network);
    streaming.set_standing(tick.standing);

    if let Some(view) = tick.share {
        send_view(conn, state, view, now).await;
    }
    if tick.ask_presence && within_budget(state, now) {
        if let Err(e) = conn.send(SignalingMessage::GetPresence).await {
            tracing::warn!("Could not ask the server who is in the room: {}", e);
        }
    }
}

/// Tells everyone in the room how long ago this app last heard each of them.
async fn send_view(
    conn: &mut SignalingConnection,
    state: &SignalingState,
    view: LinkView,
    now: Instant,
) {
    if !within_budget(state, now) {
        tracing::warn!("Held back what this app hears of the room: too many messages went out");
        return;
    }
    let body = match serde_json::to_value(PeerBody::LinkView(view)) {
        Ok(serde_json::Value::Object(body)) => body,
        other => {
            tracing::error!("A link view did not encode as an object: {:?}", other);
            return;
        }
    };
    let sent = conn
        .send(SignalingMessage::PeerMessage {
            to: None,
            from: None,
            from_name: None,
            body,
        })
        .await;
    if let Err(e) = sent {
        tracing::warn!("Could not tell the room what this app hears: {}", e);
    }
}

/// A participant said how long ago they last heard each of the others.
pub(super) fn view_received(state: &SignalingState, from: Uuid, view: LinkView) {
    lock(state).view_received(Instant::now(), from, view);
}

/// The server said who is in the room and how long ago it last heard from each.
pub(super) fn presence_received(state: &SignalingState, peers: Vec<jamjam::network::PeerPresence>) {
    lock(state).presence_received(
        Instant::now(),
        peers
            .into_iter()
            .map(|peer| (peer.peer_id, peer.idle_ms.map(Duration::from_millis)))
            .collect(),
    );
}
