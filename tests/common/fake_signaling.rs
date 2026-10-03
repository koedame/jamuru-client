//! A jamjam server on this machine, for tests that run the real CLI in a room.
//!
//! It answers `GET /api/v1/signaling` with its own WebSocket, takes the device identity headers
//! without checking them, and keeps rooms the way the real server does for what the CLI uses:
//! create, join, publish an address, chat, leave. Whoever publishes an address is announced at
//! `127.0.0.1` on the port of its audio socket, so two CLIs reach each other on any machine,
//! whatever interfaces it has.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use jamjam::network::{AddressCandidate, InviteCode, PeerInfo, SignalingMessage};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

struct Member {
    info: PeerInfo,
    tx: UnboundedSender<SignalingMessage>,
}

struct Room {
    id: String,
    invite_code: InviteCode,
    members: Vec<Member>,
}

#[derive(Default)]
struct State {
    rooms: Vec<Room>,
    /// In every room already: a peer that is not a connection to this server but a socket a test
    /// runs, such as one that echoes audio
    residents: Vec<PeerInfo>,
}

/// The server. Runs until dropped.
pub struct FakeSignaling {
    url: String,
    runtime: Option<tokio::runtime::Runtime>,
}

impl FakeSignaling {
    pub fn start() -> Self {
        Self::start_with_residents(Vec::new())
    }

    /// A server in whose every room the peer at `address` already is, with no key to check its
    /// key exchange against (as a peer of an older version has). A room that does not exist is
    /// made when someone joins it, as the always-open room of an echo is
    pub fn start_with_resident_at(name: &str, address: SocketAddr) -> Self {
        Self::start_with_residents(vec![PeerInfo {
            id: Uuid::new_v4(),
            name: name.to_string(),
            candidates: vec![AddressCandidate::host(address)],
            public_addr: Some(address),
            local_addr: Some(address),
            joined_at: 0,
            features: Vec::new(),
            link_key: None,
        }])
    }

    fn start_with_residents(residents: Vec<PeerInfo>) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime of the fake signaling server");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the server");
        listener.set_nonblocking(true).expect("non-blocking");
        let addr = listener.local_addr().expect("server address");
        let state = Arc::new(Mutex::new(State {
            rooms: Vec::new(),
            residents,
        }));
        runtime.spawn(async move {
            let listener = TcpListener::from_std(listener).expect("listener");
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, addr, state.clone()));
            }
        });
        Self {
            url: format!("http://{}", addr),
            runtime: Some(runtime),
        }
    }

    /// What `--server` takes
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for FakeSignaling {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// One connection: the question of where the signaling is, or the WebSocket itself
async fn serve(mut stream: TcpStream, addr: SocketAddr, state: Arc<Mutex<State>>) {
    let mut head = [0u8; 256];
    let Ok(read) = stream.peek(&mut head).await else {
        return;
    };
    if String::from_utf8_lossy(&head[..read]).starts_with("GET /api/v1/signaling") {
        let body = format!(r#"{{"url":"ws://{}/v1/signaling"}}"#, addr);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        return;
    }

    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut incoming) = ws.split();
    let (tx, mut outgoing) = unbounded_channel::<SignalingMessage>();
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let json = serde_json::to_string(&message).expect("serialize");
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    let mut me: Option<(String, Uuid)> = None;
    while let Some(Ok(frame)) = incoming.next().await {
        let Message::Text(text) = frame else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<SignalingMessage>(&text) else {
            continue;
        };
        let mut state = state.lock().unwrap();
        match message {
            SignalingMessage::CreateRoom {
                peer_name,
                link_key,
                features,
                ..
            } => {
                let room_id = Uuid::new_v4().to_string();
                let invite_code = InviteCode::generate();
                let peer_id = Uuid::new_v4();
                state.rooms.push(Room {
                    id: room_id.clone(),
                    invite_code: invite_code.clone(),
                    members: vec![Member {
                        info: new_peer(peer_id, peer_name, link_key, features),
                        tx: tx.clone(),
                    }],
                });
                me = Some((room_id.clone(), peer_id));
                let _ = tx.send(SignalingMessage::RoomCreated {
                    room_id,
                    peer_id,
                    invite_code,
                });
            }
            SignalingMessage::JoinRoom {
                room_id,
                peer_name,
                link_key,
                features,
                ..
            } => {
                let residents = state.residents.clone();
                if !residents.is_empty() && !state.rooms.iter().any(|room| room.id == room_id) {
                    state.rooms.push(Room {
                        id: room_id.clone(),
                        invite_code: InviteCode::generate(),
                        members: Vec::new(),
                    });
                }
                let Some(room) = state
                    .rooms
                    .iter_mut()
                    .find(|room| room.id == room_id || room.invite_code.as_str() == room_id)
                else {
                    let _ = tx.send(SignalingMessage::Error {
                        message: "no such room".to_string(),
                    });
                    continue;
                };
                let peer_id = Uuid::new_v4();
                let info = new_peer(peer_id, peer_name, link_key, features);
                let peers: Vec<PeerInfo> = room
                    .members
                    .iter()
                    .map(|member| member.info.clone())
                    .chain(residents)
                    .collect();
                for member in &room.members {
                    let _ = member
                        .tx
                        .send(SignalingMessage::PeerJoined { peer: info.clone() });
                }
                room.members.push(Member {
                    info,
                    tx: tx.clone(),
                });
                me = Some((room.id.clone(), peer_id));
                let _ = tx.send(SignalingMessage::RoomJoined {
                    room_id: room.id.clone(),
                    peer_id,
                    invite_code: Some(room.invite_code.clone()),
                    peers,
                });
            }
            SignalingMessage::UpdatePeerInfo { local_addr, .. } => {
                let Some((room_id, peer_id)) = &me else {
                    continue;
                };
                let Some(port) = local_addr.map(|addr| addr.port()) else {
                    continue;
                };
                let address = SocketAddr::from(([127, 0, 0, 1], port));
                let Some(room) = state.rooms.iter_mut().find(|room| &room.id == room_id) else {
                    continue;
                };
                let Some(updated) = room
                    .members
                    .iter_mut()
                    .find(|member| &member.info.id == peer_id)
                    .map(|member| {
                        member.info.candidates = vec![AddressCandidate::host(address)];
                        member.info.public_addr = Some(address);
                        member.info.local_addr = Some(address);
                        member.info.clone()
                    })
                else {
                    continue;
                };
                tell_the_others(
                    room,
                    peer_id,
                    SignalingMessage::PeerUpdated { peer: updated },
                );
            }
            chat @ SignalingMessage::ChatMessage { .. } => {
                let Some((room_id, peer_id)) = &me else {
                    continue;
                };
                if let Some(room) = state.rooms.iter().find(|room| &room.id == room_id) {
                    tell_the_others(room, peer_id, chat);
                }
            }
            SignalingMessage::LeaveRoom => break,
            _ => {}
        }
    }

    if let Some((room_id, peer_id)) = me {
        let mut state = state.lock().unwrap();
        if let Some(room) = state.rooms.iter_mut().find(|room| room.id == room_id) {
            room.members.retain(|member| member.info.id != peer_id);
            tell_the_others(room, &peer_id, SignalingMessage::PeerLeft { peer_id });
        }
    }
    writer.abort();
}

fn new_peer(id: Uuid, name: String, link_key: Option<String>, features: Vec<String>) -> PeerInfo {
    PeerInfo {
        id,
        name,
        candidates: Vec::new(),
        public_addr: None,
        local_addr: None,
        joined_at: 0,
        features,
        link_key,
    }
}

fn tell_the_others(room: &Room, except: &Uuid, message: SignalingMessage) {
    for member in room
        .members
        .iter()
        .filter(|member| &member.info.id != except)
    {
        let _ = member.tx.send(message.clone());
    }
}
