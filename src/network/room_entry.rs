//! Entering a room: what a participant asks the server, and how the reply is read.
//!
//! The app and the CLI each built the request and sorted the reply on their own, and could
//! drift apart in what they asked for or how they took a refusal. Both go through
//! [`SignalingConnection::enter_room`] now, and add only what is theirs to the room entered:
//! the app its state and records, the CLI what it prints.

use uuid::Uuid;

use super::error::NetworkError;
use super::signaling::{ClientInfo, InviteCode, PeerInfo, SignalingConnection, SignalingMessage};

/// How a participant means to enter a room
#[derive(Debug, Clone)]
pub enum RoomEntry {
    /// Make a room with this name
    Create { room_name: String },
    /// Join the room with this id or invite code
    Join { room_id: String },
}

/// What a participant tells the server when it enters a room
#[derive(Debug, Clone)]
pub struct EnterRequest {
    pub entry: RoomEntry,
    pub peer_name: String,
    /// The public half of the key this participant signs its key exchanges with; the others in
    /// the room check its key exchange against it
    pub link_key: String,
    /// The peer messages this participant takes (empty: none)
    pub features: Vec<String>,
    /// What the people who run the service are told about this participant's app, never passed
    /// on to the other participants
    pub client_info: Option<ClientInfo>,
}

/// The room a participant entered
#[derive(Debug, Clone)]
pub struct Entered {
    pub room_id: String,
    /// This participant in the room
    pub peer_id: Uuid,
    /// The code others join with. A room joined by its id may come back without one
    pub invite_code: Option<InviteCode>,
    /// Everyone already in the room (nobody, for a room just made)
    pub peers: Vec<PeerInfo>,
    /// The room was made by this request, not joined
    pub created: bool,
}

/// Why a participant did not enter the room
#[derive(Debug, thiserror::Error)]
pub enum EnterError {
    /// The request or the reply did not get through
    #[error(transparent)]
    Connection(#[from] NetworkError),
    /// The server refused, and says why
    #[error("{0}")]
    Refused(String),
    /// The server answered with something that is no answer to entering a room
    #[error("Unexpected response")]
    Unexpected(Box<SignalingMessage>),
}

impl EnterRequest {
    fn into_message(self) -> SignalingMessage {
        let Self {
            entry,
            peer_name,
            link_key,
            features,
            client_info,
        } = self;
        match entry {
            RoomEntry::Create { room_name } => SignalingMessage::CreateRoom {
                room_name,
                password: None,
                peer_name,
                features,
                client_info,
                link_key: Some(link_key),
            },
            RoomEntry::Join { room_id } => SignalingMessage::JoinRoom {
                room_id,
                password: None,
                peer_name,
                features,
                client_info,
                link_key: Some(link_key),
            },
        }
    }
}

/// What the server's one reply to a request to enter a room says
fn read_reply(reply: SignalingMessage) -> Result<Entered, EnterError> {
    match reply {
        SignalingMessage::RoomCreated {
            room_id,
            peer_id,
            invite_code,
        } => Ok(Entered {
            room_id,
            peer_id,
            invite_code: Some(invite_code),
            peers: Vec::new(),
            created: true,
        }),
        SignalingMessage::RoomJoined {
            room_id,
            peer_id,
            invite_code,
            peers,
        } => Ok(Entered {
            room_id,
            peer_id,
            invite_code,
            peers,
            created: false,
        }),
        SignalingMessage::Error { message } => Err(EnterError::Refused(message)),
        other => Err(EnterError::Unexpected(Box::new(other))),
    }
}

impl SignalingConnection {
    /// Asks the server to make a room or let this participant into one, and reads the one reply
    pub async fn enter_room(&mut self, request: EnterRequest) -> Result<Entered, EnterError> {
        self.send(request.into_message()).await?;
        read_reply(self.recv().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(entry: RoomEntry) -> EnterRequest {
        EnterRequest {
            entry,
            peer_name: "Aki".to_string(),
            link_key: "key".to_string(),
            features: vec!["peer_message".to_string()],
            client_info: None,
        }
    }

    /// Verifies: REQ-CON-135
    #[test]
    fn a_request_to_create_asks_for_a_room_with_the_name_the_features_and_the_key() {
        let message = request(RoomEntry::Create {
            room_name: "Studio".to_string(),
        })
        .into_message();

        match message {
            SignalingMessage::CreateRoom {
                room_name,
                password,
                peer_name,
                features,
                link_key,
                ..
            } => {
                assert_eq!(room_name, "Studio");
                assert_eq!(password, None);
                assert_eq!(peer_name, "Aki");
                assert_eq!(features, vec!["peer_message".to_string()]);
                assert_eq!(link_key.as_deref(), Some("key"));
            }
            other => panic!("expected CreateRoom, got {other:?}"),
        }
    }

    /// Verifies: REQ-CON-135
    #[test]
    fn a_request_to_join_asks_for_the_room_by_what_was_given_with_the_features_and_the_key() {
        let message = request(RoomEntry::Join {
            room_id: "ABC234XYZ".to_string(),
        })
        .into_message();

        match message {
            SignalingMessage::JoinRoom {
                room_id,
                peer_name,
                features,
                link_key,
                ..
            } => {
                assert_eq!(room_id, "ABC234XYZ");
                assert_eq!(peer_name, "Aki");
                assert_eq!(features, vec!["peer_message".to_string()]);
                assert_eq!(link_key.as_deref(), Some("key"));
            }
            other => panic!("expected JoinRoom, got {other:?}"),
        }
    }

    /// Verifies: REQ-CON-135
    #[test]
    fn a_room_just_made_has_its_code_and_nobody_else_in_it() {
        let peer_id = Uuid::new_v4();
        let code = InviteCode::generate();

        let entered = read_reply(SignalingMessage::RoomCreated {
            room_id: "room".to_string(),
            peer_id,
            invite_code: code.clone(),
        })
        .unwrap();

        assert!(entered.created);
        assert_eq!(entered.room_id, "room");
        assert_eq!(entered.peer_id, peer_id);
        assert_eq!(entered.invite_code, Some(code));
        assert!(entered.peers.is_empty());
    }

    /// Verifies: REQ-CON-135, REQ-CON-026
    #[test]
    fn a_room_joined_has_those_already_in_it_and_the_code_when_the_server_gives_one() {
        let peer_id = Uuid::new_v4();
        let other = PeerInfo {
            id: Uuid::new_v4(),
            name: "Bo".to_string(),
            candidates: Vec::new(),
            public_addr: None,
            local_addr: None,
            joined_at: 0,
            features: Vec::new(),
            link_key: None,
        };

        let entered = read_reply(SignalingMessage::RoomJoined {
            room_id: "room".to_string(),
            peer_id,
            invite_code: None,
            peers: vec![other.clone()],
        })
        .unwrap();

        assert!(!entered.created);
        assert_eq!(entered.invite_code, None);
        assert_eq!(entered.peers.len(), 1);
        assert_eq!(entered.peers[0].id, other.id);
    }

    /// Verifies: REQ-CON-135
    #[test]
    fn a_refusal_carries_what_the_server_said_and_any_other_reply_is_unexpected() {
        let refused = read_reply(SignalingMessage::Error {
            message: "no such room".to_string(),
        })
        .unwrap_err();
        assert!(matches!(&refused, EnterError::Refused(why) if why == "no such room"));
        assert_eq!(refused.to_string(), "no such room");

        let unexpected = read_reply(SignalingMessage::PeerLeft {
            peer_id: Uuid::new_v4(),
        })
        .unwrap_err();
        assert!(matches!(&unexpected, EnterError::Unexpected(_)));
        assert_eq!(unexpected.to_string(), "Unexpected response");
    }
}
