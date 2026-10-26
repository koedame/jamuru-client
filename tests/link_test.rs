//! Setting up the audio link to a participant ([`jamjam::link`]), between two on this machine.

mod common;

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::audio::{PeerGain, PeerMeter, PeerMix};
use jamjam::link::{LinkSpec, OpenError, Opened};
use jamjam::network::{AudioEncodingConfig, Connection, ConnectionState, LinkIdentity, UdpMux};

use common::wait_until_encrypted;

const FRAME: u32 = 64;
const RATE: u32 = 48_000;

fn encoding() -> AudioEncodingConfig {
    AudioEncodingConfig {
        channels: 2,
        frame_size: FRAME,
        fec_group_size: None,
        ..Default::default()
    }
}

fn mix_for(capacity: usize) -> PeerMix {
    PeerMix::new(capacity, RATE, FRAME, Arc::new(AtomicU64::new(0)))
}

/// One participant's side of a link to one other: its socket, its mix, and its key
struct Side {
    mux: UdpMux,
    mix: PeerMix,
    identity: LinkIdentity,
    states: Arc<Mutex<Vec<ConnectionState>>>,
}

impl Side {
    async fn new(capacity: usize) -> Self {
        Self {
            mux: UdpMux::bind("127.0.0.1:0").await.expect("audio socket"),
            mix: mix_for(capacity),
            identity: LinkIdentity::generate(),
            states: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn open(&self, their_key: &str) -> Result<Opened, OpenError> {
        let encoding = encoding();
        let spec = LinkSpec {
            mux: &self.mux,
            mix: &self.mix,
            encoding: &encoding,
            sample_rate: RATE,
            frame_size: FRAME,
            jitter_buffer_frames: 2,
        };
        let states = self.states.clone();
        spec.open(
            Connection::on(&self.mux).expect("connection"),
            (&self.identity, Some(their_key)),
            Arc::new(PeerGain::default()),
            Arc::new(PeerMeter::default()),
            move |state| states.lock().unwrap().push(state),
        )
    }
}

/// Verifies: REQ-CON-133
#[tokio::test]
async fn when_two_links_are_set_up_and_connected_the_hook_hears_it_and_the_peers_audio_is_mixed() {
    let (a, b) = (Side::new(1).await, Side::new(1).await);
    let mut from_a = a.open(&b.identity.public_key()).expect("a's link");
    let mut from_b = b.open(&a.identity.public_key()).expect("b's link");

    let (a_addr, b_addr) = (a.mux.local_addr(), b.mux.local_addr());
    let (up_a, up_b) = tokio::join!(
        from_a.connection.connect(b_addr),
        from_b.connection.connect(a_addr)
    );
    up_a.expect("a connects");
    up_b.expect("b connects");
    wait_until_encrypted(&[&from_a.connection, &from_b.connection]).await;

    assert!(a
        .states
        .lock()
        .unwrap()
        .contains(&ConnectionState::Connected));
    assert!(b
        .states
        .lock()
        .unwrap()
        .contains(&ConnectionState::Connected));

    // What a sends is what b's mix plays: the link was set up so that the peer's payloads
    // reach the play-out buffer the mix reads
    let signal = vec![0.5f32; FRAME as usize * 2];
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut out = vec![0.0f32; FRAME as usize * 2];
    loop {
        from_a
            .connection
            .send_audio(&signal, 0)
            .await
            .expect("send");
        tokio::time::sleep(Duration::from_millis(2)).await;
        b.mix.mix_into(&mut out, 100, None);
        if out.iter().any(|sample| *sample != 0.0) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "b's mix never played what a sent"
        );
    }
}

/// Verifies: REQ-CON-020
#[tokio::test]
async fn when_the_mix_has_no_place_left_the_link_is_refused() {
    let a = Side::new(1).await;
    let other = LinkIdentity::generate().public_key();

    let first = a.open(&other).expect("the first link has a place");
    let second = a.open(&other);

    assert!(matches!(second, Err(OpenError::NoRoom)));
    drop(first);
}
