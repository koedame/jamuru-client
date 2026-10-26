//! Setting up the audio link to one participant, for the app and the CLI alike (ADR-077).
//!
//! A link is a [`Connection`] on the session's one audio socket, the peer's place in the mix,
//! and the path their audio takes from payload to that mix. What has to be done to a connection
//! before it is checked - the order of it, and that the play-out buffer is emptied when the
//! link comes back from an outage (ADR-022) - is done here once; what the app and the CLI each
//! want to know about the link's state, they hook in.

use std::sync::{Arc, RwLock};

use crate::audio::{PeerGain, PeerMeter, PeerMix, ReceivePath, VoiceHandle};
use crate::network::{
    AudioEncodingConfig, Connection, ConnectionState, LinkIdentity, NetworkError, UdpMux,
};

/// The connections a send loop sends each captured frame to: those of the links that are up.
/// Replaced whole when a link comes up or goes, so the loop takes it without waiting for
/// whoever changes it.
pub type Senders = Arc<RwLock<Arc<[Arc<tokio::sync::Mutex<Connection>>]>>>;

/// What every link of a session is set up with
pub struct LinkSpec<'a> {
    pub mux: &'a UdpMux,
    pub mix: &'a PeerMix,
    pub encoding: &'a AudioEncodingConfig,
    pub sample_rate: u32,
    pub frame_size: u32,
    /// The play-out delay the link starts with, in frames
    pub jitter_buffer_frames: u32,
}

/// A link that is set up and not yet checked
pub struct Opened {
    /// Not connected: [`Connection::connect_with_candidates`] does that
    pub connection: Connection,
    /// Their audio, from payload to the mix
    pub receive: ReceivePath,
    /// Their place in the mix: they are heard for as long as it is held
    pub voice: VoiceHandle,
}

/// Why a link could not be set up
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("Failed to set up the link to the peer: {0}")]
    Identity(NetworkError),
    #[error("Failed to configure audio encoding: {0}")]
    Encoding(NetworkError),
    #[error("Failed to create audio decoder: {0}")]
    Decoder(String),
    #[error("The session has no room for another link")]
    NoRoom,
}

impl LinkSpec<'_> {
    /// Sets up `connection`, made on the session's socket for one participant.
    ///
    /// `identity` is our key in the room and the key the peer told the server it signs with,
    /// which the key exchange is checked against. A peer that told none (its app is too old) is
    /// not linked to. `on_state` is told
    /// each change of the connection's state, after the play-out buffer has been emptied for a
    /// connection that came up.
    pub fn open(
        &self,
        mut connection: Connection,
        identity: (&LinkIdentity, Option<&str>),
        gain: Arc<PeerGain>,
        meter: Arc<PeerMeter>,
        on_state: impl Fn(ConnectionState) + Send + Sync + 'static,
    ) -> Result<Opened, OpenError> {
        let (ours, peer_key) = identity;
        peer_key
            .ok_or(NetworkError::PeerKeyMissing)
            .and_then(|peer_key| connection.verify_peer(ours, peer_key))
            .map_err(OpenError::Identity)?;
        // The codec and FEC come from the preset (ADR-021)
        connection
            .set_audio_encoding(self.encoding.clone())
            .map_err(OpenError::Encoding)?;

        // Jitter buffer keyed by sequence number (ADR-020). Received audio waits in the
        // play-out buffer, and only there, until the output callback takes it (ADR-028).
        let receive = ReceivePath::new(
            self.encoding.codec_type,
            self.sample_rate,
            self.frame_size,
            self.jitter_buffer_frames,
        )
        .map_err(|e| OpenError::Decoder(e.to_string()))?;
        let voice = self
            .mix
            .add(receive.clone(), gain, meter)
            .ok_or(OpenError::NoRoom)?;

        // Sequence numbers carry on across an outage, so what was waiting before it must not
        // play after it (ADR-022).
        let for_state = receive.clone();
        connection.set_state_change_callback(move |state| {
            if state == ConnectionState::Connected {
                for_state.reset();
            }
            on_state(state);
        });

        // Runs on the network task, not the realtime audio callback, so locking and taking
        // ownership of the payload are both acceptable here.
        let for_audio = receive.clone();
        connection.set_audio_callback(move |sequence, payload, _timestamp| {
            if !for_audio.receive(sequence, &payload) {
                tracing::trace!("Play-out buffer refused frame {}", sequence);
            }
        });

        Ok(Opened {
            connection,
            receive,
            voice,
        })
    }
}
