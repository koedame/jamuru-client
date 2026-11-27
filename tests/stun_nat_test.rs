//! Public address discovery behind a NAT that renumbers ports
//!
//! A NAT maps each source socket to its own public port, and many home routers
//! pick a port unrelated to the socket's own. Whatever is advertised to a peer
//! has to be the port the NAT assigned to the *audio* socket - only traffic to
//! that port is forwarded back to it. These tests put a NAT that always
//! renumbers between the audio socket and a STUN server, all on loopback (a real
//! network namespace with `iptables --random` needs root, which the test
//! environments do not have), and check that a peer using the advertised
//! address reaches the audio socket.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use jamjam::network::{gather_candidates_using, AddressCandidate, CandidateType};
use tokio::net::UdpSocket;
use tokio::time::timeout;

const MAGIC_COOKIE: u32 = 0x2112A442;

/// Answers STUN binding requests with the address they came from, as a real
/// server does.
async fn spawn_stun_server() -> SocketAddr {
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

/// A NAT in front of one client that gives it a public port different from its
/// own, on a public address (`127.0.0.2`) different from the client's where the
/// loopback interface has one.
///
/// The address apart is what lets a peer that sends to the socket's own port on
/// the public address (what was published before) hit nothing. macOS has
/// 127.0.0.1 alone, so there the public side is on 127.0.0.1 too: the published
/// address is still checked to be the NAT's port, but sending to the socket's
/// own port there would reach it.
///
/// Inbound traffic is forwarded to the client only after the client has sent
/// something out, as with a real mapping.
struct RenumberingNat {
    inside: SocketAddr,
    external: SocketAddr,
}

impl RenumberingNat {
    /// Sends everything `client` emits to `forward_to` (the STUN server) from
    /// the public side, and everything arriving on the public side back to
    /// `client`.
    async fn start(client: SocketAddr, forward_to: SocketAddr) -> Self {
        let inside = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let mut external = public_side().await;
        while external.local_addr().unwrap().port() == client.port() {
            external = public_side().await;
        }
        let external = Arc::new(external);
        let mapped = Arc::new(AtomicBool::new(false));

        {
            let (inside, external, mapped) = (inside.clone(), external.clone(), mapped.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, from)) = inside.recv_from(&mut buf).await {
                    if from == client {
                        mapped.store(true, Ordering::SeqCst);
                        let _ = external.send_to(&buf[..len], forward_to).await;
                    }
                }
            });
        }
        {
            let (inside, external) = (inside.clone(), external.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, _)) = external.recv_from(&mut buf).await {
                    if mapped.load(Ordering::SeqCst) {
                        let _ = inside.send_to(&buf[..len], client).await;
                    }
                }
            });
        }

        Self {
            inside: inside.local_addr().unwrap(),
            external: external.local_addr().unwrap(),
        }
    }
}

/// A socket on 127.0.0.2, or on 127.0.0.1 where there is no such address (macOS)
async fn public_side() -> UdpSocket {
    match UdpSocket::bind("127.0.0.2:0").await {
        Ok(socket) => socket,
        Err(_) => UdpSocket::bind("127.0.0.1:0").await.unwrap(),
    }
}

struct Scene {
    audio: Arc<UdpSocket>,
    nat: RenumberingNat,
}

async fn scene() -> Scene {
    let stun = spawn_stun_server().await;
    let audio = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let nat = RenumberingNat::start(audio.local_addr().unwrap(), stun).await;
    Scene { audio, nat }
}

fn server_reflexive(candidates: &[AddressCandidate]) -> Vec<SocketAddr> {
    candidates
        .iter()
        .filter(|c| c.candidate_type == CandidateType::ServerReflexive)
        .map(|c| c.address)
        .collect()
}

/// Verifies: REQ-CON-027
///
/// Given a NAT that gives the audio socket a public port other than its own,
/// the address published is the public address and port STUN saw.
#[tokio::test]
async fn nat_renumbers_port_then_published_address_is_the_one_stun_saw() {
    let Scene { audio, nat } = scene().await;
    assert_ne!(
        nat.external.port(),
        audio.local_addr().unwrap().port(),
        "the NAT under test must renumber ports"
    );

    let candidates = gather_candidates_using(&audio, &[&nat.inside.to_string()]).await;

    assert_eq!(server_reflexive(&candidates), vec![nat.external]);
}

/// Verifies: REQ-CON-027
///
/// A peer that sends to the published address reaches the audio socket. This is
/// what the port number is for: sending to the socket's own port on the public
/// address (what was published before) hits nothing.
#[tokio::test]
async fn peer_sends_to_published_address_then_audio_socket_receives_it() {
    let Scene { audio, nat } = scene().await;
    let candidates = gather_candidates_using(&audio, &[&nat.inside.to_string()]).await;
    let published = *server_reflexive(&candidates)
        .first()
        .expect("a server reflexive candidate");

    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer.send_to(b"hello", published).await.unwrap();

    let mut buf = [0u8; 64];
    let (len, _) = timeout(Duration::from_secs(2), audio.recv_from(&mut buf))
        .await
        .expect("the audio socket received nothing from the published address")
        .unwrap();
    assert_eq!(&buf[..len], b"hello");
}

/// Verifies: REQ-CON-027
///
/// The audio socket may already hold datagrams that are not the STUN reply
/// (a peer's keep-alive that arrived first); they must not end the query.
#[tokio::test]
async fn datagram_arrives_before_stun_reply_then_reply_is_still_found() {
    let Scene { audio, nat } = scene().await;
    let stray = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stray
        .send_to(b"not a stun message at all", audio.local_addr().unwrap())
        .await
        .unwrap();

    let candidates = gather_candidates_using(&audio, &[&nat.inside.to_string()]).await;

    assert_eq!(server_reflexive(&candidates), vec![nat.external]);
}

/// Verifies: REQ-CON-027
///
/// With no STUN server answering, the host candidates are still offered and no
/// public address is invented.
#[tokio::test]
async fn stun_unreachable_then_no_server_reflexive_candidate_is_published() {
    let audio = Arc::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap());
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    let candidates =
        gather_candidates_using(&audio, &[&silent.local_addr().unwrap().to_string()]).await;

    assert!(server_reflexive(&candidates).is_empty());
}
