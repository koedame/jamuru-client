//! Reaching the server's relay for remote operation (ADR-044)
//!
//! An app that is to be operated from elsewhere holds an outbound WebSocket to
//! the relay, which pairs it with whoever operates it. This module asks whether
//! this installation is enrolled for that, and opens the WebSocket. What is
//! said over it is the RPC protocol of the app, not this crate's concern.
//!
//! Only builds with the `remote-link` feature contain it: a release build has
//! no way to open the connection.

use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use super::device_identity::DeviceIdentity;
use super::discovery::check_signaling_url;
use super::error::{NetworkError, SignalingFailure};
use super::signaling::{ensure_crypto_provider_installed, signed_device_headers};

/// Where the server says whether this installation is enrolled.
pub const REMOTE_ENROLLMENT_PATH: &str = "/api/v1/remote/enrollment";

/// How long the question may take before it counts as failed.
const ENROLLMENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The server's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteEnrollment {
    /// Whether this installation may be operated remotely.
    pub enrolled: bool,
    /// The relay's WebSocket (`ws://` or `wss://`); empty when the server has
    /// none.
    #[serde(default)]
    pub url: String,
}

/// Asks the jamjam server at `server_url` whether this installation is
/// enrolled, proving the device identity (ADR-024). Nothing else is sent.
///
/// Redirects are not followed, for the reason given for the signaling question
/// (`discover_signaling_url`). Errors name no URL: the server URL may carry
/// credentials and the caller logs the error.
pub async fn discover_remote_enrollment(
    server_url: &str,
    identity: &DeviceIdentity,
) -> Result<RemoteEnrollment, NetworkError> {
    ensure_crypto_provider_installed();
    if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
        return Err(NetworkError::SignalingError(format!(
            "Invalid server URL {:?}: must start with http:// or https://",
            server_url
        )));
    }
    let endpoint = format!(
        "{}{}",
        server_url.trim_end_matches('/'),
        REMOTE_ENROLLMENT_PATH
    );
    let fail = |what: String| NetworkError::SignalingUnreachable {
        failure: SignalingFailure::Other,
        message: format!("Asking the server about remote operation failed: {}", what),
    };

    let client = reqwest::Client::builder()
        .timeout(ENROLLMENT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| fail(e.without_url().to_string()))?;
    let mut request = client.get(&endpoint);
    for (name, value) in signed_device_headers(identity) {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|e| fail(e.without_url().to_string()))?;
    if !response.status().is_success() {
        return Err(fail(format!("answered {}", response.status())));
    }
    let answer: RemoteEnrollment = response
        .json()
        .await
        .map_err(|e| fail(format!("unexpected answer: {}", e.without_url())))?;
    if answer.enrolled {
        check_signaling_url(server_url, &answer.url)?;
    }
    Ok(answer)
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens the relay's WebSocket at `url` with the device identity on the
/// handshake, and returns its two halves.
pub async fn connect_remote(
    url: &str,
    identity: &DeviceIdentity,
) -> Result<(RemoteWriter, RemoteReader), NetworkError> {
    ensure_crypto_provider_installed();
    let mut request = url
        .into_client_request()
        .map_err(|e| NetworkError::SignalingError(format!("Invalid relay URL: {}", e)))?;
    for (name, value) in signed_device_headers(identity) {
        let value = value
            .parse()
            .map_err(|e| NetworkError::SignalingError(format!("Invalid {} header: {}", name, e)))?;
        request.headers_mut().insert(name, value);
    }
    let (socket, _) = connect_async(request).await.map_err(|e| {
        NetworkError::SignalingError(format!("Connecting to the relay failed: {}", e))
    })?;
    let (sink, stream) = socket.split();
    Ok((RemoteWriter { sink }, RemoteReader { stream }))
}

/// The sending half of a relay connection.
pub struct RemoteWriter {
    sink: SplitSink<Socket, Message>,
}

impl RemoteWriter {
    /// Sends one text frame.
    pub async fn send_text(&mut self, text: String) -> Result<(), NetworkError> {
        self.sink
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| NetworkError::SignalingError(format!("Send failed: {}", e)))
    }

    /// Ends the connection.
    pub async fn close(&mut self) {
        let _ = self.sink.close().await;
    }
}

/// The receiving half of a relay connection.
pub struct RemoteReader {
    stream: SplitStream<Socket>,
}

impl RemoteReader {
    /// The next text frame, or `None` once the relay ended the connection.
    /// Anything that is not text is skipped: the protocol has nothing else.
    pub async fn recv_text(&mut self) -> Result<Option<String>, NetworkError> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Text(text))) => return Ok(Some(text.to_string())),
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(_)) => continue,
                Some(Err(e)) => {
                    return Err(NetworkError::SignalingError(format!(
                        "Receive failed: {}",
                        e
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_without_a_relay_url_reads_as_not_enrolled_with_no_url() {
        let answer: RemoteEnrollment = serde_json::from_str(r#"{"enrolled": false}"#).unwrap();
        assert_eq!(
            answer,
            RemoteEnrollment {
                enrolled: false,
                url: String::new()
            }
        );
    }
}
