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
use tokio::time::{interval_at, timeout, timeout_at, Instant, Interval, MissedTickBehavior};
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

/// How long a relay connection may go without a single frame from the relay
/// before it counts as lost. The relay answers a ping at once, so a connection
/// that stays silent this long is not listening any more: a NAT that forgot the
/// mapping, a path that went away. Without this the read waits for ever, as TCP
/// has nothing in flight to time out.
pub const RELAY_SILENCE_LIMIT: Duration = Duration::from_secs(60);

/// How long a send may take before the connection counts as lost. A peer that
/// stopped acknowledging lets the kernel's buffer fill, and the send then waits
/// without end.
const RELAY_SEND_LIMIT: Duration = Duration::from_secs(30);

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

/// Asks the jamuru server at `server_url` whether this installation is
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
    Ok(halves(socket, RELAY_SILENCE_LIMIT))
}

fn halves(socket: Socket, silence_limit: Duration) -> (RemoteWriter, RemoteReader) {
    let (sink, stream) = socket.split();
    (
        RemoteWriter {
            sink,
            send_limit: RELAY_SEND_LIMIT,
        },
        RemoteReader {
            stream,
            silence_limit,
            last_heard: Instant::now(),
        },
    )
}

/// The sending half of a relay connection.
pub struct RemoteWriter {
    sink: SplitSink<Socket, Message>,
    send_limit: Duration,
}

impl RemoteWriter {
    /// Sends one text frame.
    pub async fn send_text(&mut self, text: String) -> Result<(), NetworkError> {
        self.send(Message::Text(text.into())).await
    }

    /// Sends a WebSocket ping, which the relay answers with a pong. The pong is
    /// what [`RemoteReader::recv_text`] counts as the relay still being there.
    pub async fn send_ping(&mut self) -> Result<(), NetworkError> {
        self.send(Message::Ping(Vec::new().into())).await
    }

    async fn send(&mut self, message: Message) -> Result<(), NetworkError> {
        match timeout(self.send_limit, self.sink.send(message)).await {
            Ok(sent) => {
                sent.map_err(|e| NetworkError::SignalingError(format!("Send failed: {}", e)))
            }
            Err(_) => Err(NetworkError::SignalingError(format!(
                "Send failed: no progress in {} s",
                self.send_limit.as_secs()
            ))),
        }
    }

    /// Ends the connection.
    pub async fn close(&mut self) {
        let _ = self.sink.close().await;
    }
}

/// The receiving half of a relay connection.
pub struct RemoteReader {
    stream: SplitStream<Socket>,
    silence_limit: Duration,
    /// When the relay last sent anything, a pong included. Kept here rather than
    /// in the caller's loop so that dropping a pending `recv_text` (a `select!`
    /// taking another branch) does not forget it.
    last_heard: Instant,
}

impl RemoteReader {
    /// Replaces how long the relay may stay silent; the ping interval follows.
    pub fn with_silence_limit(mut self, limit: Duration) -> Self {
        self.silence_limit = limit;
        self
    }

    /// When to ping the relay: three times within the silence limit, so one
    /// lost ping does not end the connection. Pair it with
    /// [`RemoteWriter::send_ping`] in the loop that reads.
    pub fn ping_timer(&self) -> Interval {
        let every = self.silence_limit / 3;
        let mut timer = interval_at(Instant::now() + every, every);
        timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
        timer
    }

    /// The next text frame, or `None` once the relay ended the connection.
    /// Anything that is not text is skipped: the protocol has nothing else.
    /// Fails when the relay has sent nothing, not even a pong, for the silence
    /// limit.
    pub async fn recv_text(&mut self) -> Result<Option<String>, NetworkError> {
        loop {
            let next = timeout_at(self.last_heard + self.silence_limit, self.stream.next())
                .await
                .map_err(|_| {
                    NetworkError::SignalingError(format!(
                        "Receive failed: nothing heard from the relay for {} s",
                        self.silence_limit.as_secs()
                    ))
                })?;
            if next.is_some() {
                self.last_heard = Instant::now();
            }
            match next {
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
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    const LIMIT: Duration = Duration::from_millis(300);

    /// A relay on a local port that accepts one connection. `answers` decides
    /// whether it reads from it, and so answers pings, or only holds it open.
    async fn local_relay(answers: bool) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/remote/test", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(tcp).await.unwrap();
            if answers {
                socket.send(Message::Text("hello".into())).await.unwrap();
                while socket.next().await.is_some() {}
            } else {
                std::future::pending::<()>().await;
            }
        });
        (url, task)
    }

    async fn connect_to(url: &str) -> (RemoteWriter, RemoteReader) {
        let (writer, reader) = connect_remote(url, &DeviceIdentity::generate())
            .await
            .unwrap();
        (writer, reader.with_silence_limit(LIMIT))
    }

    /// Reads the way the app's loops do: pinging on the timer, until a frame
    /// or a failure.
    async fn read_with_pings(
        writer: &mut RemoteWriter,
        reader: &mut RemoteReader,
    ) -> Result<Option<String>, NetworkError> {
        let mut ping = reader.ping_timer();
        loop {
            tokio::select! {
                _ = ping.tick() => writer.send_ping().await?,
                frame = reader.recv_text() => return frame,
            }
        }
    }

    /// Verifies: REQ-RMT-021
    #[tokio::test]
    async fn when_the_relay_goes_silent_the_read_fails_instead_of_waiting_for_ever() {
        let (url, _relay) = local_relay(false).await;
        let (mut writer, mut reader) = connect_to(&url).await;

        let started = std::time::Instant::now();
        let outcome = timeout(
            Duration::from_secs(5),
            read_with_pings(&mut writer, &mut reader),
        )
        .await
        .expect("the read should have given up");

        assert!(outcome.is_err(), "{:?}", outcome);
        assert!(started.elapsed() >= LIMIT, "{:?}", started.elapsed());
    }

    /// Verifies: REQ-RMT-021
    #[tokio::test]
    async fn while_the_relay_answers_pings_a_quiet_connection_stays_up_past_the_limit() {
        let (url, _relay) = local_relay(true).await;
        let (mut writer, mut reader) = connect_to(&url).await;
        assert_eq!(reader.recv_text().await.unwrap().as_deref(), Some("hello"));

        let outcome = timeout(LIMIT * 4, read_with_pings(&mut writer, &mut reader)).await;

        assert!(outcome.is_err(), "the connection ended: {:?}", outcome);
    }

    /// Verifies: REQ-RMT-021
    #[tokio::test]
    async fn the_silence_is_counted_across_reads_that_were_given_up() {
        let (url, _relay) = local_relay(false).await;
        let (_writer, mut reader) = connect_to(&url).await;

        // A loop that keeps taking another branch drops the read each time.
        for _ in 0..3 {
            let _ = timeout(LIMIT / 4, reader.recv_text()).await;
        }
        let outcome = timeout(LIMIT, reader.recv_text()).await;

        assert!(matches!(outcome, Ok(Err(_))), "{:?}", outcome);
    }

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
