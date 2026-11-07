//! A STUN server for the lab in `tests/nat/`: answers a binding request from each address given
//! with the address and port it came from.
//!
//! ```text
//! nat_lab_stun <ip:port> [<ip:port> ...]
//! ```

use std::net::SocketAddr;

use tokio::net::UdpSocket;

const MAGIC_COOKIE: u32 = 0x2112A442;

#[tokio::main]
async fn main() {
    let binds: Vec<String> = std::env::args().skip(1).collect();
    assert!(!binds.is_empty(), "give at least one address to listen on");
    let mut servers = Vec::new();
    for bind in binds {
        let socket = UdpSocket::bind(&bind).await.expect("bind STUN");
        println!("STUN listening on {bind}");
        servers.push(tokio::spawn(async move {
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
        }));
    }
    for server in servers {
        let _ = server.await;
    }
}
