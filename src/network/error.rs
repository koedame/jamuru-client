//! Network error types

use thiserror::Error;

/// Errors that can occur in the network subsystem
#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("Connection timeout")]
    ConnectionTimeout,

    #[error("Connection refused")]
    ConnectionRefused,

    #[error("Already connected")]
    AlreadyConnected,

    #[error("Not connected")]
    NotConnected,

    #[error("Send buffer full")]
    SendBufferFull,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid packet")]
    InvalidPacket,

    /// Audio encoding could not be set up: unavailable codec, or a poisoned lock
    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("Address parse error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),

    #[error("STUN failed: {0}")]
    StunFailed(String),

    #[error("Signaling error: {0}")]
    SignalingError(String),

    /// The signaling server could not be reached, and `failure` says how.
    /// `message` is the text shown to the user.
    #[error("{message}")]
    SignalingUnreachable {
        failure: SignalingFailure,
        message: String,
    },

    /// The server refused the device's identity, and its own time shows this
    /// computer's clock to be `offset_secs` ahead of it (negative: behind).
    #[error("{}", super::clock::clock_skew_message(*offset_secs))]
    ClockSkew { offset_secs: i64 },

    /// The signaling server closed the WebSocket (or it dropped)
    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Session full")]
    SessionFull,

    #[error("Peer not found: {0}")]
    PeerNotFound(String),

    #[error("Room not found: {0}")]
    RoomNotFound(String),

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Key exchange failed: {0}")]
    KeyExchangeFailed(String),

    #[error("No connection candidates available")]
    NoCandidates,

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}

/// How reaching the signaling server failed, as far as it can be told. The
/// usage log records this word instead of the message, which can carry an
/// address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalingFailure {
    /// The server answered with a 4xx status.
    Http4xx,
    /// The server answered with a 5xx status.
    Http5xx,
    Timeout,
    Tls,
    Dns,
    Other,
}

impl SignalingFailure {
    /// The failure a server's answer with `status` is.
    pub fn of_status(status: u16) -> Self {
        match status {
            400..=499 => Self::Http4xx,
            500..=599 => Self::Http5xx,
            _ => Self::Other,
        }
    }

    /// The failure `error` is, judged from it and the errors it wraps.
    ///
    /// The HTTP and WebSocket libraries do not give a name lookup or a
    /// certificate failure a type of its own to match on, so those two are
    /// told from the wording of the error chain, here and nowhere else.
    pub fn of_error(error: &(dyn std::error::Error + 'static)) -> Self {
        let mut chain = String::new();
        let mut timed_out = false;
        let mut next: Option<&(dyn std::error::Error + 'static)> = Some(error);
        while let Some(error) = next {
            if let Some(io) = error.downcast_ref::<std::io::Error>() {
                timed_out |= io.kind() == std::io::ErrorKind::TimedOut;
            }
            chain.push_str(&error.to_string().to_ascii_lowercase());
            chain.push('\n');
            next = error.source();
        }
        if timed_out || chain.contains("timed out") {
            Self::Timeout
        } else if chain.contains("dns error") || chain.contains("failed to lookup") {
            Self::Dns
        } else if chain.contains("certificate") || chain.contains("tls") {
            Self::Tls
        } else {
            Self::Other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Wrapped(&'static str, Option<Box<dyn std::error::Error + 'static>>);

    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref()
        }
    }

    fn chain(outer: &'static str, inner: &'static str) -> Wrapped {
        Wrapped(outer, Some(Box::new(Wrapped(inner, None))))
    }

    /// Verifies: REQ-TEL-017
    #[test]
    fn when_the_server_answers_with_a_4xx_or_5xx_status_the_failure_is_named_by_its_class() {
        assert_eq!(SignalingFailure::of_status(404), SignalingFailure::Http4xx);
        assert_eq!(SignalingFailure::of_status(429), SignalingFailure::Http4xx);
        assert_eq!(SignalingFailure::of_status(500), SignalingFailure::Http5xx);
        assert_eq!(SignalingFailure::of_status(530), SignalingFailure::Http5xx);
        assert_eq!(SignalingFailure::of_status(302), SignalingFailure::Other);
    }

    /// Verifies: REQ-TEL-017
    #[test]
    fn when_the_error_chain_says_the_name_did_not_resolve_the_failure_is_dns() {
        let error = chain(
            "error sending request",
            "dns error: failed to lookup address information",
        );
        assert_eq!(SignalingFailure::of_error(&error), SignalingFailure::Dns);
    }

    /// Verifies: REQ-TEL-017
    #[test]
    fn when_the_error_chain_says_the_certificate_was_refused_the_failure_is_tls() {
        let error = chain(
            "error sending request",
            "invalid peer certificate: UnknownIssuer",
        );
        assert_eq!(SignalingFailure::of_error(&error), SignalingFailure::Tls);
    }

    /// Verifies: REQ-TEL-017
    #[test]
    fn when_an_io_error_in_the_chain_timed_out_the_failure_is_a_timeout() {
        let error = std::io::Error::new(std::io::ErrorKind::TimedOut, "no answer");
        assert_eq!(
            SignalingFailure::of_error(&error),
            SignalingFailure::Timeout
        );
    }

    /// Verifies: REQ-TEL-017
    #[test]
    fn when_nothing_in_the_chain_is_recognised_the_failure_is_other() {
        let error = chain("error sending request", "connection refused");
        assert_eq!(SignalingFailure::of_error(&error), SignalingFailure::Other);
    }
}
