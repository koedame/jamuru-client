//! Reaching the server's relay for remote operation (ADR-044)
//!
//! An app that is to be operated from elsewhere holds an outbound WebSocket to
//! the relay, which pairs it with whoever operates it. This module asks whether
//! this installation is enrolled for remote debugging, names the relay's
//! WebSocket for a settings help, and opens the WebSocket. What is said over it
//! is the RPC protocol of the app, not this crate's concern.

use std::time::Duration;

use data_encoding::HEXLOWER;
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

/// Which end of a settings help's relay connection: the app being helped
/// opens it first, then the app helping opens the same number as the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpEnd {
    Host,
    Guest,
}

/// A new settings help number: 128 random bits as 32 lowercase hex digits. The
/// relay pairs the two ends by it and checks nothing else about who may join,
/// so it is not guessable.
pub fn new_help_session() -> String {
    HEXLOWER.encode(&rand::random::<[u8; 16]>())
}

/// Whether `session` is a help number as [`new_help_session`] makes them, which
/// is the only form the relay accepts.
pub fn is_help_session(session: &str) -> bool {
    session.len() == 32
        && session
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The relay's WebSocket for the settings help `session`, on the server that
/// `signaling_url` (from `discover_signaling_url`) names. The relay is served by
/// the same host as the signaling, under `/v1/remote/help/`.
pub fn help_relay_url(
    signaling_url: &str,
    session: &str,
    end: HelpEnd,
) -> Result<String, NetworkError> {
    if !is_help_session(session) {
        return Err(NetworkError::SignalingError(
            "Invalid settings help number".to_string(),
        ));
    }
    let scheme_end = signaling_url
        .strip_prefix("wss://")
        .map(|_| "wss://".len())
        .or_else(|| signaling_url.strip_prefix("ws://").map(|_| "ws://".len()))
        .ok_or_else(|| {
            NetworkError::SignalingError(
                "The signaling address is not a ws:// or wss:// URL".to_string(),
            )
        })?;
    let origin_end = signaling_url[scheme_end..]
        .find('/')
        .map_or(signaling_url.len(), |slash| scheme_end + slash);
    let role = match end {
        HelpEnd::Host => "host",
        HelpEnd::Guest => "guest",
    };
    Ok(format!(
        "{}/v1/remote/help/{}?role={}",
        &signaling_url[..origin_end],
        session,
        role
    ))
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
    fn a_help_session_number_is_128_random_bits_as_hex_and_no_two_are_alike() {
        let first = new_help_session();
        assert!(is_help_session(&first), "{}", first);
        assert_ne!(first, new_help_session());
    }

    #[test]
    fn only_32_lowercase_hex_digits_are_a_help_session_number() {
        for bad in [
            "",
            "abc",
            &"0".repeat(31),
            &"0".repeat(33),
            &"A".repeat(32),
            &"g".repeat(32),
        ] {
            assert!(!is_help_session(bad), "{:?}", bad);
        }
    }

    #[test]
    fn the_help_relay_is_on_the_signaling_host_and_names_the_end() {
        let session = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            help_relay_url(
                "wss://signaling.example.com/v1/signaling",
                session,
                HelpEnd::Host
            )
            .unwrap(),
            format!("wss://signaling.example.com/v1/remote/help/{session}?role=host")
        );
        assert_eq!(
            help_relay_url("ws://127.0.0.1:17890/v1/signaling", session, HelpEnd::Guest).unwrap(),
            format!("ws://127.0.0.1:17890/v1/remote/help/{session}?role=guest")
        );
        assert_eq!(
            help_relay_url("ws://localhost:1", session, HelpEnd::Host).unwrap(),
            format!("ws://localhost:1/v1/remote/help/{session}?role=host")
        );
    }

    #[test]
    fn a_help_relay_is_not_named_for_a_bad_number_or_a_bad_signaling_address() {
        let session = "0123456789abcdef0123456789abcdef";
        assert!(help_relay_url(
            "wss://signaling.example.com/v1/signaling",
            "../x",
            HelpEnd::Host
        )
        .is_err());
        assert!(help_relay_url(
            "https://signaling.example.com/v1/signaling",
            session,
            HelpEnd::Host
        )
        .is_err());
    }

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
