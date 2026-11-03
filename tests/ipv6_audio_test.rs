//! Audio over IPv6, and over IPv4 and IPv6 at once, through the one dual-stack socket an app
//! has for the whole room (ADR-093).
//!
//! The peers are on this machine's loopback: `::1` for IPv6, `127.0.0.1` for IPv4.

mod common;

use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::audio::{PeerGain, PeerMeter, PeerMix};
use jamjam::link::{LinkSpec, Opened};
use jamjam::network::{
    bind_audio_std, AudioEncodingConfig, Connection, ConnectionState, IpFamilies, LinkIdentity,
    UdpMux,
};

use common::wait_until_encrypted;

const FRAME: u32 = 64;
const RATE: u32 = 48_000;

/// Whether this machine has IPv6 at all. A machine without it has no IPv6 route to test, and
/// the app's socket is IPv4 then (covered by the transport's own tests).
fn has_ipv6() -> bool {
    std::net::UdpSocket::bind("[::1]:0").is_ok()
}

fn encoding() -> AudioEncodingConfig {
    AudioEncodingConfig {
        channels: 2,
        frame_size: FRAME,
        fec_group_size: None,
        ..Default::default()
    }
}

/// One participant: its socket, its mix, and its key
struct Side {
    mux: UdpMux,
    mix: PeerMix,
    identity: LinkIdentity,
}

impl Side {
    fn with(mux: UdpMux) -> Self {
        Self {
            mux,
            mix: PeerMix::new(2, RATE, FRAME, Arc::new(AtomicU64::new(0))),
            identity: LinkIdentity::generate(),
        }
    }

    /// An app's socket: dual-stack
    fn app() -> Self {
        let socket = bind_audio_std(0).expect("audio socket");
        assert_eq!(
            IpFamilies::of_std(&socket),
            IpFamilies { v4: true, v6: true }
        );
        Self::with(UdpMux::from_std(socket).expect("mux"))
    }

    async fn at(addr: &str) -> Self {
        Self::with(UdpMux::bind(addr).await.expect("socket"))
    }

    /// Where a peer on this machine sends to reach this side
    fn reached_at(&self, loopback: &str) -> SocketAddr {
        let port = self.mux.local_addr().port();
        format!("{}:{}", loopback, port).parse().unwrap()
    }

    fn open(&self, their_key: &str) -> Opened {
        let encoding = encoding();
        let spec = LinkSpec {
            mux: &self.mux,
            mix: &self.mix,
            encoding: &encoding,
            sample_rate: RATE,
            frame_size: FRAME,
            jitter_buffer_frames: 2,
        };
        let states = Arc::new(Mutex::new(Vec::<ConnectionState>::new()));
        spec.open(
            Connection::on(&self.mux).expect("connection"),
            (&self.identity, Some(their_key)),
            Arc::new(PeerGain::default()),
            Arc::new(PeerMeter::default()),
            move |state| states.lock().unwrap().push(state),
        )
        .expect("a place in the mix")
    }
}

/// Sends from `from` until `to`'s mix plays something
async fn audio_reaches(from: &Opened, to: &Side) {
    let signal = vec![0.5f32; FRAME as usize * 2];
    let mut out = vec![0.0f32; FRAME as usize * 2];
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        from.connection.send_audio(&signal, 0).await.expect("send");
        tokio::time::sleep(Duration::from_millis(2)).await;
        to.mix.mix_into(&mut out, 100, None);
        if out.iter().any(|sample| *sample != 0.0) {
            return;
        }
        assert!(Instant::now() < deadline, "the audio never arrived");
    }
}

/// Verifies: REQ-CON-145
#[tokio::test]
async fn when_the_peer_is_reached_over_ipv6_audio_flows_both_ways_and_the_route_is_ipv6() {
    if !has_ipv6() {
        return;
    }
    let (app, peer) = (Side::app(), Side::at("[::1]:0").await);
    let mut from_app = app.open(&peer.identity.public_key());
    let mut from_peer = peer.open(&app.identity.public_key());

    let (up_app, up_peer) = tokio::join!(
        from_app.connection.connect(peer.reached_at("[::1]")),
        from_peer.connection.connect(app.reached_at("[::1]"))
    );
    up_app.expect("the app connects");
    up_peer.expect("the peer connects");
    wait_until_encrypted(&[&from_app.connection, &from_peer.connection]).await;

    audio_reaches(&from_app, &peer).await;
    audio_reaches(&from_peer, &app).await;
    assert!(from_app.connection.remote_addr().is_ipv6());
}

/// Verifies: REQ-CON-145
#[tokio::test]
async fn when_the_peer_is_reached_over_ipv4_audio_flows_and_its_address_is_the_ipv4_one() {
    if !has_ipv6() {
        return;
    }
    let (app, peer) = (Side::app(), Side::at("127.0.0.1:0").await);
    let mut from_app = app.open(&peer.identity.public_key());
    let mut from_peer = peer.open(&app.identity.public_key());

    let (up_app, up_peer) = tokio::join!(
        from_app.connection.connect(peer.reached_at("127.0.0.1")),
        from_peer.connection.connect(app.reached_at("127.0.0.1"))
    );
    up_app.expect("the app connects");
    up_peer.expect("the peer connects");
    wait_until_encrypted(&[&from_app.connection, &from_peer.connection]).await;

    audio_reaches(&from_app, &peer).await;
    audio_reaches(&from_peer, &app).await;
    // The peer's packets arrive on an IPv6 socket as IPv4-mapped addresses; the link knows the
    // peer by its own IPv4 address, not by the mapped spelling
    assert_eq!(
        from_app.connection.remote_addr(),
        peer.reached_at("127.0.0.1")
    );
}

/// Verifies: REQ-CON-145
#[tokio::test]
async fn when_one_peer_is_ipv4_and_another_ipv6_one_socket_carries_both_links() {
    if !has_ipv6() {
        return;
    }
    let app = Side::app();
    let (over_v6, over_v4) = (Side::at("[::1]:0").await, Side::at("127.0.0.1:0").await);
    let mut app_to_v6 = app.open(&over_v6.identity.public_key());
    let mut app_to_v4 = app.open(&over_v4.identity.public_key());
    let mut v6_to_app = over_v6.open(&app.identity.public_key());
    let mut v4_to_app = over_v4.open(&app.identity.public_key());

    let (a, b, c, d) = tokio::join!(
        app_to_v6.connection.connect(over_v6.reached_at("[::1]")),
        app_to_v4
            .connection
            .connect(over_v4.reached_at("127.0.0.1")),
        v6_to_app.connection.connect(app.reached_at("[::1]")),
        v4_to_app.connection.connect(app.reached_at("127.0.0.1"))
    );
    a.expect("link to the IPv6 peer");
    b.expect("link to the IPv4 peer");
    c.expect("the IPv6 peer connects");
    d.expect("the IPv4 peer connects");
    wait_until_encrypted(&[
        &app_to_v6.connection,
        &app_to_v4.connection,
        &v6_to_app.connection,
        &v4_to_app.connection,
    ])
    .await;

    audio_reaches(&v6_to_app, &app).await;
    audio_reaches(&app_to_v6, &over_v6).await;
    audio_reaches(&app_to_v4, &over_v4).await;
    assert!(app_to_v6.connection.remote_addr().is_ipv6());
    assert!(app_to_v4.connection.remote_addr().is_ipv4());
}

/// Verifies: REQ-CON-145
#[tokio::test]
async fn when_a_peer_has_both_an_ipv4_and_an_ipv6_address_the_one_that_answers_is_chosen() {
    if !has_ipv6() {
        return;
    }
    for (answers, dead) in [("[::1]", "127.0.0.1"), ("127.0.0.1", "[::1]")] {
        let (app, peer) = (Side::app(), Side::at(&format!("{answers}:0")).await);
        let mut from_app = app.open(&peer.identity.public_key());
        let mut from_peer = peer.open(&app.identity.public_key());
        let candidates: Vec<SocketAddr> = vec![
            format!("{dead}:9").parse().unwrap(),
            peer.reached_at(answers),
        ];

        let (up_app, up_peer) = tokio::join!(
            from_app.connection.connect_with_candidates(&candidates),
            from_peer.connection.connect(app.reached_at(answers))
        );
        up_app.expect("the app connects");
        up_peer.expect("the peer connects");

        assert_eq!(
            from_app.connection.remote_addr(),
            peer.reached_at(answers),
            "answering side {answers}"
        );
    }
}
