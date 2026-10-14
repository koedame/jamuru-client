//! Entering a room through a signaling connection, against a server on this machine.

mod common;

use std::sync::Arc;

use common::fake_signaling::FakeSignaling;
use jamjam::network::{
    DeviceIdentity, EnterError, EnterRequest, RoomEntry, SignalingClient, SignalingConnection,
};

async fn connect(server: &FakeSignaling) -> SignalingConnection {
    SignalingClient::new(server.url(), Arc::new(DeviceIdentity::generate()))
        .connect()
        .await
        .expect("connect to the server on this machine")
}

fn request(entry: RoomEntry, name: &str) -> EnterRequest {
    EnterRequest {
        entry,
        peer_name: name.to_string(),
        link_key: "a-link-key".to_string(),
        features: Vec::new(),
        client_info: None,
    }
}

/// Verifies: REQ-CON-135
#[tokio::test]
async fn when_a_room_is_made_the_one_who_made_it_is_alone_in_it_and_others_join_by_its_code() {
    let server = FakeSignaling::start();

    let mut maker = connect(&server).await;
    let made = maker
        .enter_room(request(
            RoomEntry::Create {
                room_name: "Studio".to_string(),
            },
            "Aki",
        ))
        .await
        .expect("make a room");
    assert!(made.created && made.peers.is_empty());
    let code = made.invite_code.expect("a made room has a code");

    let mut joiner = connect(&server).await;
    let joined = joiner
        .enter_room(request(
            RoomEntry::Join {
                room_id: code.to_string(),
            },
            "Bo",
        ))
        .await
        .expect("join by the code");
    assert!(!joined.created);
    assert_eq!(joined.room_id, made.room_id);
    assert_eq!(joined.peers.len(), 1);
    assert_eq!(joined.peers[0].id, made.peer_id);
    assert_eq!(joined.peers[0].link_key.as_deref(), Some("a-link-key"));
}

/// Verifies: REQ-CON-135
#[tokio::test]
async fn when_the_room_does_not_exist_the_servers_refusal_comes_back_as_it_was_said() {
    let server = FakeSignaling::start();
    let mut conn = connect(&server).await;

    let refused = conn
        .enter_room(request(
            RoomEntry::Join {
                room_id: "NOROOM234".to_string(),
            },
            "Aki",
        ))
        .await
        .expect_err("there is no such room");

    assert!(matches!(&refused, EnterError::Refused(why) if why == "no such room"));
}
