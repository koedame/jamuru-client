//! Sending a batch of lines.
//!
//! One anonymous `POST` per batch: no device signature header, no cookie, no
//! identifier other than what is in the lines themselves. A batch that does
//! not arrive is dropped, so nothing here retries.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::config::DEFAULT_SERVER_URL;

/// Where the server takes usage logs.
pub const USAGE_LOGS_PATH: &str = "/api/v1/usage-logs";

/// The body's media type: one JSON object per line.
pub const NDJSON_CONTENT_TYPE: &str = "application/x-ndjson";

/// How long one send may take before it counts as failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a batch arrived.
pub type Delivery<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Where a batch of lines goes. The reporter takes one so a test can watch
/// what would have been sent.
pub trait Transport: Send + Sync {
    /// Sends `body` (NDJSON). `true` only when the server took it.
    fn send(&self, body: Vec<u8>) -> Delivery<'_>;
}

/// Sends to the jamjam server the build was made for.
pub struct HttpTransport {
    endpoint: String,
}

impl HttpTransport {
    /// For the server at `server_url` (`http://` or `https://`); `None` for
    /// anything else, which leaves the reporter with nowhere to send.
    pub fn new(server_url: &str) -> Option<Self> {
        if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
            return None;
        }
        Some(Self {
            endpoint: format!("{}{}", server_url.trim_end_matches('/'), USAGE_LOGS_PATH),
        })
    }

    /// For the server this build carries as its default (`DEFAULT_SERVER_URL`,
    /// the same place `GET /api/v1/signaling` is asked). Not the `server_url`
    /// setting: that is a server of the user's own, and usage is not sent
    /// there.
    pub fn for_build() -> Option<Self> {
        Self::new(DEFAULT_SERVER_URL)
    }
}

impl Transport for HttpTransport {
    fn send(&self, body: Vec<u8>) -> Delivery<'_> {
        Box::pin(async move {
            // Redirects are not followed: a redirect would send the lines
            // somewhere the build did not name.
            let Ok(client) = reqwest::Client::builder()
                .timeout(SEND_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
            else {
                return false;
            };
            let sent = client
                .post(&self.endpoint)
                .header(reqwest::header::CONTENT_TYPE, NDJSON_CONTENT_TYPE)
                .body(body)
                .send()
                .await;
            matches!(sent, Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT)
        })
    }
}

/// A transport with nowhere to go: a build that names no server.
pub struct NoTransport;

impl Transport for NoTransport {
    fn send(&self, _body: Vec<u8>) -> Delivery<'_> {
        Box::pin(async { false })
    }
}
