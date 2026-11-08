//! The route moves to a new address of the peer only once the peer has answered there.
//!
//! A packet the peer sealed can be sent again from any address, so the address a packet came
//! from does not show where the peer is.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use jamjam::network::{Connection, ConnectionState, LinkSecurity};
use jamjam::protocol::{Packet, PacketType};
use parking_lot::Mutex;
use tokio::net::UdpSocket;

/// What a listener on the way keeps while the path carries nothing from b to a
struct Path {
    outage: Arc<AtomicBool>,
    kept: Arc<Mutex<Vec<Vec<u8>>>>,
    toward_a: std::net::SocketAddr,
}

/// a and b joined by two relays; from `outage` on, what b sends is kept instead of passed on
async fn pair_behind_a_relay() -> (Connection, Connection, Path) {
    let mut a = Connection::new("127.0.0.1:0").await.unwrap();
    let mut b = Connection::new("127.0.0.1:0").await.unwrap();
    common::trust_each_other(&mut a, &mut b);
    let (a_addr, b_addr) = (a.local_addr(), b.local_addr());
    let facing_a = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let facing_b = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let outage = Arc::new(AtomicBool::new(false));
    let kept = Arc::new(Mutex::new(Vec::new()));
    {
        let (facing_a, facing_b) = (facing_a.clone(), facing_b.clone());
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((len, _)) = facing_a.recv_from(&mut buf).await {
                let _ = facing_b.send_to(&buf[..len], b_addr).await;
            }
        });
    }
    {
        let (facing_a, facing_b) = (facing_a.clone(), facing_b.clone());
        let (outage, kept) = (outage.clone(), kept.clone());
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((len, _)) = facing_b.recv_from(&mut buf).await {
                if outage.load(Ordering::SeqCst) {
                    kept.lock().push(buf[..len].to_vec());
                } else {
                    let _ = facing_a.send_to(&buf[..len], a_addr).await;
                }
            }
        });
    }
    let (toward_a, toward_b) = (
        facing_a.local_addr().unwrap(),
        facing_b.local_addr().unwrap(),
    );
    let (for_a, for_b) = ([toward_a], [toward_b]);
    let (ra, rb) = tokio::join!(
        a.connect_with_candidates(&for_a),
        b.connect_with_candidates(&for_b)
    );
    ra.unwrap();
    rb.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(a.state(), ConnectionState::Connected);
    assert_eq!(a.security(), LinkSecurity::Encrypted);
    (
        a,
        b,
        Path {
            outage,
            kept,
            toward_a,
        },
    )
}

/// Verifies: REQ-CON-175
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_the_peers_sealed_packets_are_sent_again_from_another_address_the_route_stays_where_it_is(
) {
    let (a, _b, path) = pair_behind_a_relay().await;
    let before = a.remote_addr();
    assert_eq!(before, path.toward_a);

    // The path stops carrying b's packets; a listener keeps them
    path.outage.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let other = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // ... and sends them to a, one at a time, from an address of its own, for longer than a
    // waits for the peer to answer
    let mut sent_again = 0;
    for _ in 0..25 {
        let packet = {
            let mut kept = path.kept.lock();
            let mut found = None;
            while let Some(bytes) = kept.pop() {
                if Packet::from_bytes(&bytes)
                    .is_some_and(|p| p.packet_type != PacketType::LatencyPong)
                {
                    found = Some(bytes);
                    break;
                }
            }
            found
        };
        if let Some(bytes) = packet {
            other.send_to(&bytes, a.local_addr()).await.unwrap();
            sent_again += 1;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            a.remote_addr(),
            before,
            "a sends to {} after {sent_again} packets",
            other.local_addr().unwrap()
        );
    }
    assert!(
        sent_again >= 10,
        "only {sent_again} packets were sent again"
    );

    // What a sent there is the check, which only the peer can answer: a few small packets
    let mut buf = [0u8; 2048];
    let mut got = Vec::new();
    while let Ok(Ok((len, _))) =
        tokio::time::timeout(Duration::from_millis(200), other.recv_from(&mut buf)).await
    {
        got.push(len);
    }
    assert!(!got.is_empty(), "a never asked the address it heard from");
    assert!(
        got.len() <= 8,
        "a sent {} packets to the address it heard from",
        got.len()
    );
    assert!(
        got.iter().all(|&len| len <= 128),
        "a sent more than a check: {got:?}"
    );
}
