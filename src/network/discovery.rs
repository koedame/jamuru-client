//! Finding the signaling server (`GET /api/v1/signaling`)
//!
//! The app holds no signaling address. It holds the address of the jamjam
//! server ([`crate::config::DEFAULT_SERVER_URL`], or `server_url` in
//! `config.toml`) and asks it where to connect each time it connects, so the
//! signaling server can move without a new release of the app (ADR-030).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::clock::{clock_offset_secs, now_unix_secs};
use super::error::{NetworkError, SignalingFailure};

/// Where the server answers with its signaling address.
pub const SIGNALING_ENDPOINT_PATH: &str = "/api/v1/signaling";

/// How long the question may take before connecting counts as failed.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// The server's answer: the signaling WebSocket to connect to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalingEndpoint {
    /// A `ws://` or `wss://` URL.
    pub url: String,
}

/// Asks the jamjam server at `server_url` where its signaling server is.
///
/// Redirects are not followed: the answer must come from the server that was
/// asked, over the scheme it was asked with (a redirect to `http://` would let
/// anyone on that hop name the signaling server the device identity is shown
/// to). Errors name no URL, as the server URL may carry credentials and the
/// caller logs the error.
pub async fn discover_signaling_url(server_url: &str) -> Result<String, NetworkError> {
    let endpoint = signaling_endpoint_url(server_url)?;
    let fail = |failure: SignalingFailure, what: String| NetworkError::SignalingUnreachable {
        failure,
        message: format!(
            "Asking the server for its signaling server failed: {}",
            what
        ),
    };

    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| fail(SignalingFailure::Other, e.without_url().to_string()))?;
    let response = client.get(&endpoint).send().await.map_err(|e| {
        let e = e.without_url();
        fail(SignalingFailure::of_error(&e), e.to_string())
    })?;
    if !response.status().is_success() {
        return Err(fail(
            SignalingFailure::of_status(response.status().as_u16()),
            format!("answered {}", response.status()),
        ));
    }
    let answer: SignalingEndpoint = response.json().await.map_err(|e| {
        fail(
            SignalingFailure::Other,
            format!("unexpected answer: {}", e.without_url()),
        )
    })?;
    check_signaling_url(server_url, &answer.url)?;
    Ok(answer.url)
}

/// How far this computer's clock is ahead of the server's, in seconds
/// (negative when it is behind), read from the `Date` of the server's answer
/// to the question of [`discover_signaling_url`]. `None` when the server does
/// not answer or its answer carries no date.
pub async fn server_clock_offset_secs(server_url: &str) -> Option<i64> {
    let endpoint = signaling_endpoint_url(server_url).ok()?;
    let client = reqwest::Client::builder()
        .timeout(DISCOVERY_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let response = client.get(&endpoint).send().await.ok()?;
    let date = response
        .headers()
        .get(reqwest::header::DATE)?
        .to_str()
        .ok()?;
    clock_offset_secs(date, now_unix_secs())
}

/// The URL to ask: `server_url` (`http://` or `https://`, optionally ending in
/// `/`) followed by [`SIGNALING_ENDPOINT_PATH`].
pub fn signaling_endpoint_url(server_url: &str) -> Result<String, NetworkError> {
    if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
        return Err(NetworkError::SignalingError(format!(
            "Invalid server URL {:?}: must start with http:// or https://",
            server_url
        )));
    }
    Ok(format!(
        "{}{}",
        server_url.trim_end_matches('/'),
        SIGNALING_ENDPOINT_PATH
    ))
}

/// Accepts the server's answer only if it is a WebSocket URL that is no less
/// protected than the question was: a server asked over `https://` may not send
/// the app to an unencrypted `ws://`.
pub fn check_signaling_url(server_url: &str, signaling_url: &str) -> Result<(), NetworkError> {
    let secure = signaling_url.starts_with("wss://");
    if !secure && !signaling_url.starts_with("ws://") {
        return Err(NetworkError::SignalingError(format!(
            "The server named {:?} as its signaling server, which is not a ws:// or wss:// URL",
            signaling_url
        )));
    }
    if server_url.starts_with("https://") && !secure {
        return Err(NetworkError::SignalingError(format!(
            "The server was asked over https:// but named the unencrypted {:?}",
            signaling_url
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_goes_to_the_v1_path_under_the_server_url() {
        assert_eq!(
            signaling_endpoint_url("https://server.example.com").unwrap(),
            "https://server.example.com/api/v1/signaling"
        );
        assert_eq!(
            signaling_endpoint_url("http://localhost:17890/").unwrap(),
            "http://localhost:17890/api/v1/signaling"
        );
    }

    /// A server on this machine that answers every request with `status` and
    /// `body`, and counts the requests. Returns its URL.
    async fn answering_server(
        status: &'static str,
        body: String,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nLocation: /elsewhere\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        (format!("{}://{}", "http", addr), requests)
    }

    #[tokio::test]
    async fn when_the_server_names_a_websocket_the_answer_is_its_url() {
        let (server, _) = answering_server(
            "200 OK",
            r#"{"url":"ws://127.0.0.1:1/v1/signaling"}"#.to_string(),
        )
        .await;
        assert_eq!(
            discover_signaling_url(&server).await.unwrap(),
            "ws://127.0.0.1:1/v1/signaling"
        );
    }

    /// Verifies: REQ-CON-029
    #[tokio::test]
    async fn when_the_server_names_something_other_than_a_websocket_the_app_does_not_connect() {
        let (server, _) = answering_server(
            "200 OK",
            r#"{"url":"https://signal.example.com/v1/signaling"}"#.to_string(),
        )
        .await;
        assert!(discover_signaling_url(&server).await.is_err());
    }

    /// Verifies: REQ-TEL-017
    #[tokio::test]
    async fn when_the_server_answers_530_the_failure_is_a_5xx() {
        let (server, _) = answering_server("530 Origin Down", String::new()).await;

        let error = discover_signaling_url(&server).await.unwrap_err();

        assert!(
            matches!(
                error,
                NetworkError::SignalingUnreachable {
                    failure: SignalingFailure::Http5xx,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// Verifies: REQ-TEL-017
    #[tokio::test]
    async fn when_the_server_answers_404_the_failure_is_a_4xx() {
        let (server, _) = answering_server("404 Not Found", String::new()).await;

        let error = discover_signaling_url(&server).await.unwrap_err();

        assert!(
            matches!(
                error,
                NetworkError::SignalingUnreachable {
                    failure: SignalingFailure::Http4xx,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// Verifies: REQ-TEL-017
    #[tokio::test]
    async fn when_the_server_name_does_not_resolve_the_failure_is_dns() {
        // `.invalid` never resolves (RFC 6761).
        let error = discover_signaling_url(&format!("{}://no-such-host.invalid", "https"))
            .await
            .unwrap_err();

        assert!(
            matches!(
                error,
                NetworkError::SignalingUnreachable {
                    failure: SignalingFailure::Dns,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    /// The text the user sees when the server cannot be reached is the same
    /// as before the failure had a kind.
    #[tokio::test]
    async fn when_the_server_answers_530_the_message_still_names_the_status() {
        let (server, _) = answering_server("530 Origin Down", String::new()).await;

        let error = discover_signaling_url(&server).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "Asking the server for its signaling server failed: answered 530 <unknown status code>"
        );
    }

    #[tokio::test]
    async fn when_the_server_redirects_the_app_does_not_follow() {
        let (server, requests) = answering_server("302 Found", String::new()).await;
        assert!(discover_signaling_url(&server).await.is_err());
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn when_asking_fails_the_error_does_not_repeat_the_credentials_in_the_server_url() {
        let error = discover_signaling_url("http://user:hunter2@127.0.0.1:1")
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("hunter2"), "{}", error);
    }

    #[test]
    fn a_server_url_that_is_not_http_is_refused_before_asking() {
        for url in ["wss://server.example.com", "server.example.com", ""] {
            assert!(signaling_endpoint_url(url).is_err(), "{:?}", url);
        }
    }

    /// Verifies: REQ-CON-029
    #[test]
    fn an_answer_that_is_not_a_websocket_url_is_refused() {
        for answer in [
            "https://signal.example.com/v1/signaling",
            "signal.example.com",
            "",
        ] {
            assert!(
                check_signaling_url("http://localhost:17890", answer).is_err(),
                "{:?}",
                answer
            );
        }
    }

    /// Verifies: REQ-CON-029
    #[test]
    fn a_server_asked_over_https_cannot_send_the_app_to_an_unencrypted_websocket() {
        assert!(check_signaling_url(
            "https://server.example.com",
            "ws://signal.example.com/v1/signaling"
        )
        .is_err());
        assert!(check_signaling_url(
            "https://server.example.com",
            "wss://signal.example.com/v1/signaling"
        )
        .is_ok());
        assert!(check_signaling_url(
            "http://localhost:17890",
            "ws://localhost:17890/v1/signaling"
        )
        .is_ok());
    }
}
