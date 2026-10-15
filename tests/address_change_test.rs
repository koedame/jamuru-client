//! The address a peer is seen from changes after an outage (a network switch, a NAT that
//! renumbers its ports): the connection sends to where the peer is heard from now.
//!
//! The two ends are joined by a relay standing in for the NAT.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use jamjam::network::{Connection, ConnectionState, LinkSecurity, ReconnectConfig};
use tokio::net::UdpSocket;

/// 0 = open, 1 = blackout (nothing passes), 2 = remapped (b is seen from a new port; the old
/// port no longer reaches b)
struct Nat {
    mode: Arc<AtomicU8>,
    toward_a: SocketAddr,
    toward_b: SocketAddr,
}

async fn nat(a: SocketAddr, b: SocketAddr) -> Nat {
    let mode = Arc::new(AtomicU8::new(0));
    let out1 = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let out2 = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let inner = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let toward_a = out1.local_addr().unwrap();
    let toward_b = inner.local_addr().unwrap();
    // a -> b, through the old mapping only
    {
        let (mode, out1, inner) = (mode.clone(), out1.clone(), inner.clone());
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((len, from)) = out1.recv_from(&mut buf).await {
                assert_eq!(from, a);
                if mode.load(Ordering::SeqCst) == 0 {
                    let _ = inner.send_to(&buf[..len], b).await;
                }
            }
        });
    }
    // a -> b through the new mapping, which is what reaches b once it is remapped
    {
        let (mode, out2, inner) = (mode.clone(), out2.clone(), inner.clone());
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((len, _)) = out2.recv_from(&mut buf).await {
                if mode.load(Ordering::SeqCst) == 2 {
                    let _ = inner.send_to(&buf[..len], b).await;
                }
            }
        });
    }
    // b -> a, from the old port until remapped, then from the new one
    {
        let mode = mode.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok((len, _)) = inner.recv_from(&mut buf).await {
                match mode.load(Ordering::SeqCst) {
                    0 => {
                        let _ = out1.send_to(&buf[..len], a).await;
                    }
                    2 => {
                        let _ = out2.send_to(&buf[..len], a).await;
                    }
                    _ => {}
                }
            }
        });
    }
    Nat {
        mode,
        toward_a,
        toward_b,
    }
}

const CONFIG: ReconnectConfig = ReconnectConfig {
    keep_alive_interval: Duration::from_millis(100),
    detect_after: Duration::from_millis(300),
    give_up_after: Duration::from_secs(1),
    check_interval: Duration::from_millis(50),
};

async fn connected_pair() -> (Connection, Connection, Nat) {
    let mut a = Connection::new("127.0.0.1:0").await.unwrap();
    let mut b = Connection::new("127.0.0.1:0").await.unwrap();
    a.set_reconnect_config(CONFIG);
    b.set_reconnect_config(CONFIG);
    let n = nat(a.local_addr(), b.local_addr()).await;
    let (for_a, for_b) = ([n.toward_a], [n.toward_b]);
    let (ra, rb) = tokio::join!(
        a.connect_with_candidates(&for_a),
        b.connect_with_candidates(&for_b)
    );
    ra.unwrap();
    rb.unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(a.state(), ConnectionState::Connected);
    assert_eq!(b.state(), ConnectionState::Connected);
    assert_eq!(a.security(), LinkSecurity::Encrypted);
    assert_eq!(b.security(), LinkSecurity::Encrypted);
    (a, b, n)
}

/// Verifies: REQ-CON-136
#[tokio::test]
async fn when_the_peer_is_seen_from_a_new_port_after_an_outage_both_ends_hear_each_other_again() {
    let (a, b, n) = connected_pair().await;

    // Nothing passes for longer than the link waits before it gives up
    n.mode.store(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (heard_by_a, heard_by_b) = (a.stats().packets_received, b.stats().packets_received);

    // b is seen from a new port; the port a knew no longer reaches it
    n.mode.store(2, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(5)).await;

    assert!(a.stats().packets_received > heard_by_a + 10);
    assert!(
        b.stats().packets_received > heard_by_b + 10,
        "b hears nothing: a still sends to {}",
        a.remote_addr()
    );
    assert_ne!(a.remote_addr(), n.toward_a);
    assert_eq!(a.state(), ConnectionState::Connected);
    assert_eq!(b.state(), ConnectionState::Connected);
}

/// Verifies: REQ-CON-136
#[tokio::test]
async fn when_a_stranger_sends_from_another_address_the_connection_keeps_sending_to_the_peer() {
    let (a, _b, n) = connected_pair().await;
    let before = a.remote_addr();
    let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // Not a packet of the link: a stranger who learned the port, even while the peer is silent
    n.mode.store(1, Ordering::SeqCst);
    for _ in 0..30 {
        stranger.send_to(&[0x55; 64], a.local_addr()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(a.remote_addr(), before);
}
