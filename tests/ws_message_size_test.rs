//! How large a message from the relay or from the signaling server the app takes in.
//!
//! The servers are local WebSocket servers that send one message of the size under test.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use jamjam::network::{
    connect_remote, DeviceIdentity, SignalingClient, SignalingMessage, RELAY_MAX_MESSAGE_BYTES,
};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

/// What the signaling server may send in one message, and the library's own limit above it
const SIGNALING_LIMIT: usize = 512 * 1024;
const LIBRARY_LIMIT: usize = 64 * 1024 * 1024;

/// Sends `messages` over the WebSocket, then waits for the other end to go
async fn serve_ws(stream: TcpStream, messages: Vec<String>) {
    let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut incoming) = ws.split();
    for message in messages {
        if sink.send(Message::text(message)).await.is_err() {
            return;
        }
    }
    while incoming.next().await.is_some() {}
}

/// A relay at the returned URL that sends `message` to the first to connect
async fn relay_sending(message: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/v1/remote/x", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        serve_ws(stream, vec![message]).await;
    });
    url
}

/// A server whose answer to the question of where its signaling is points at itself, and which
/// sends `messages` to the first WebSocket that connects. Returns the server's URL.
async fn signaling_sending(messages: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut head = [0u8; 256];
            let Ok(read) = stream.peek(&mut head).await else {
                continue;
            };
            if String::from_utf8_lossy(&head[..read]).starts_with("GET /api/v1/signaling") {
                let body = format!(r#"{{"url":"ws://{addr}/v1/signaling"}}"#);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                continue;
            }
            tokio::spawn(serve_ws(stream, messages.clone()));
        }
    });
    format!("http://{addr}")
}

/// A message the signaling client skips as one of a type it does not know, `size` bytes long
fn unknown_message_of(size: usize) -> String {
    format!(
        r#"{{"type":"from_a_newer_server","data":"{}"}}"#,
        "x".repeat(size)
    )
}

#[tokio::test]
async fn a_message_from_the_relay_is_larger_than_the_rpc_takes_ends_the_connection_and_one_that_fits_arrives(
) {
    let identity = DeviceIdentity::generate();

    let fits = "x".repeat(RELAY_MAX_MESSAGE_BYTES - 1024);
    let url = relay_sending(fits.clone()).await;
    let (_writer, mut reader) = connect_remote(&url, &identity).await.unwrap();
    let received = reader.recv_text().await.unwrap().unwrap();
    assert_eq!(received.len(), fits.len());

    let too_large = "x".repeat(RELAY_MAX_MESSAGE_BYTES + 1024);
    let url = relay_sending(too_large).await;
    let (_writer, mut reader) = connect_remote(&url, &identity).await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), reader.recv_text())
        .await
        .expect("the connection ends");
    assert!(received.is_err(), "a message of the size arrived");
}

#[tokio::test]
async fn a_message_from_the_relay_is_far_larger_than_the_rpc_takes_ends_the_connection() {
    let identity = DeviceIdentity::generate();
    let url = relay_sending("x".repeat(LIBRARY_LIMIT / 4)).await;
    let (_writer, mut reader) = connect_remote(&url, &identity).await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(5), reader.recv_text())
        .await
        .expect("the connection ends");

    assert!(received.is_err(), "a message of the size arrived");
}

#[tokio::test]
async fn a_message_from_the_signaling_server_is_larger_than_a_room_needs_ends_the_connection_and_one_that_fits_is_read(
) {
    let leave = serde_json::to_string(&SignalingMessage::LeaveRoom).unwrap();
    let server = signaling_sending(vec![unknown_message_of(SIGNALING_LIMIT - 1024), leave]).await;
    let client = SignalingClient::new(&server, Arc::new(DeviceIdentity::generate()));
    let mut connection = client.connect().await.unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), connection.recv())
        .await
        .expect("a message arrives")
        .unwrap();
    assert!(
        matches!(message, SignalingMessage::LeaveRoom),
        "{message:?}"
    );

    let server = signaling_sending(vec![unknown_message_of(SIGNALING_LIMIT + 1024)]).await;
    let client = SignalingClient::new(&server, Arc::new(DeviceIdentity::generate()));
    let mut connection = client.connect().await.unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), connection.recv())
        .await
        .expect("the connection ends");
    assert!(message.is_err(), "a message of the size arrived");
}
