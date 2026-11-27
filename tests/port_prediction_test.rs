//! What the app offers a peer when its router gives the audio socket a different public port for
//! each address it sends to
//!
//! On loopback: a NAT in front of the audio socket sends what the socket sends to each of
//! several STUN servers out from a port of its own (the next free one, or the same one for
//! every server), and the servers report the address they were reached from.

use std::net::SocketAddr;
use std::sync::Arc;

use jamjam::network::{gather_candidates_using, CandidateType};
use parking_lot::Mutex;
use tokio::net::UdpSocket;

const MAGIC_COOKIE: u32 = 0x2112A442;

/// What an external port of the NAT sends back, and where to: each server's answer to the front
/// made for that server
type Replies = Vec<(SocketAddr, Arc<UdpSocket>)>;

/// A STUN server on 127.0.0.1 that reports the address a request came from
async fn stun_server() -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind STUN");
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 576];
        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            if len < 20 || buf[0..2] != [0x00, 0x01] {
                continue;
            }
            let SocketAddr::V4(from) = from else { continue };
            let mut reply = Vec::with_capacity(32);
            reply.extend_from_slice(&0x0101u16.to_be_bytes());
            reply.extend_from_slice(&12u16.to_be_bytes());
            reply.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
            reply.extend_from_slice(&buf[8..20]);
            reply.extend_from_slice(&0x0020u16.to_be_bytes());
            reply.extend_from_slice(&8u16.to_be_bytes());
            reply.extend_from_slice(&[0x00, 0x01]);
            reply.extend_from_slice(&(from.port() ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
            reply.extend_from_slice(&(u32::from(*from.ip()) ^ MAGIC_COOKIE).to_be_bytes());
            let _ = socket.send_to(&reply, from).await;
        }
    });
    addr
}

/// Puts a NAT in front of `client` toward `servers`: one socket (a front) to send to for each of
/// them. With `per_destination`, every server is reached from a port of its own, one after another
/// from a start the NAT picks; without, from one port for all. Returns the addresses to give the
/// app as the servers, and the ports the NAT sent from.
///
/// Everything is on 127.0.0.1, the one IPv4 loopback address every OS has (macOS has no
/// 127.0.0.2): the servers and the fronts are told apart by port, and the NAT's ports are the
/// addresses it returns.
async fn nat_toward(
    client: SocketAddr,
    servers: Vec<SocketAddr>,
    per_destination: bool,
) -> (Vec<String>, Arc<Mutex<Vec<SocketAddr>>>) {
    let used = Arc::new(Mutex::new(Vec::<SocketAddr>::new()));
    // Ports in a row on one address, taken as the NAT hands them out
    // A port another socket holds (or, on Windows, one in a range the system keeps) makes the
    // search start again elsewhere; a bind that keeps failing stops the test instead of retrying
    // forever.
    let mut pool: Vec<Arc<UdpSocket>> = Vec::new();
    let mut last_error = None;
    'search: for _ in 0..200 {
        pool.clear();
        let base: u16 = 20000 + (rand::random::<u16>() % 30000);
        for i in 0..8u16 {
            match UdpSocket::bind(("127.0.0.1", base + i)).await {
                Ok(socket) => pool.push(Arc::new(socket)),
                Err(e) => {
                    last_error = Some(e);
                    continue 'search;
                }
            }
        }
        break;
    }
    assert_eq!(
        pool.len(),
        8,
        "no 8 ports in a row on 127.0.0.1: {last_error:?}"
    );
    // What each port gets back from a server goes out of the front made for that server
    let mut listeners: Vec<(Arc<UdpSocket>, Replies)> = Vec::new();
    let mut fronts = Vec::new();
    for (i, server) in servers.into_iter().enumerate() {
        let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        fronts.push(front.local_addr().unwrap().to_string());
        let external = pool[if per_destination { i } else { 0 }].clone();
        used.lock().push(external.local_addr().unwrap());
        match listeners
            .iter_mut()
            .find(|(socket, _)| Arc::ptr_eq(socket, &external))
        {
            Some((_, replies)) => replies.push((server, front.clone())),
            None => listeners.push((external.clone(), vec![(server, front.clone())])),
        }
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((len, from)) = front.recv_from(&mut buf).await {
                if from == client {
                    let _ = external.send_to(&buf[..len], server).await;
                }
            }
        });
    }
    for (external, replies) in listeners {
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while let Ok((len, from)) = external.recv_from(&mut buf).await {
                if let Some((_, front)) = replies.iter().find(|(server, _)| *server == from) {
                    let _ = front.send_to(&buf[..len], client).await;
                }
            }
        });
    }
    (fronts, used)
}

/// The server-reflexive addresses the app offers from behind the NAT, and the ports the NAT sent
/// from (one per server, in the order of the servers)
async fn gather_through_nat(per_destination: bool) -> (Vec<SocketAddr>, Vec<SocketAddr>) {
    let audio = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let mut servers = Vec::new();
    for _ in 0..3 {
        servers.push(stun_server().await);
    }
    let (fronts, used) = nat_toward(audio.local_addr().unwrap(), servers, per_destination).await;
    let fronts: Vec<&str> = fronts.iter().map(String::as_str).collect();
    let offered = gather_candidates_using(&audio, &fronts)
        .await
        .iter()
        .filter(|c| c.candidate_type == CandidateType::ServerReflexive)
        .map(|c| c.address)
        .collect();
    let used = used.lock().clone();
    (offered, used)
}

#[tokio::test]
async fn when_the_nat_gives_each_server_a_port_of_its_own_the_port_each_saw_is_offered() {
    let (mut offered, mut used) = gather_through_nat(true).await;
    offered.sort();
    used.sort();
    assert_eq!(offered.len(), 3, "{offered:?}");
    let ports: Vec<u16> = offered.iter().map(SocketAddr::port).collect();
    assert_eq!(ports[1], ports[0] + 1, "{offered:?}");
    assert_eq!(ports[2], ports[0] + 2, "{offered:?}");
    assert_eq!(offered, used, "the ports the NAT sent from");
}

#[tokio::test]
async fn when_the_nat_keeps_one_port_for_the_socket_only_that_port_is_offered() {
    let (offered, used) = gather_through_nat(false).await;
    assert_eq!(offered, vec![used[0]], "{used:?}");
}
