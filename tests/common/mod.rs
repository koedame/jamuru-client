//! What a test needs to be the other end of a connection.
//!
//! An app does not send audio until it has agreed keys with its peer, and takes only what it can
//! open. A test peer that is a bare socket is therefore silent to it. [`Peer`] is a socket with
//! the same encryption an app has, so a test can send and receive through it as a peer would.

#![allow(dead_code)]

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use jamjam::network::{Connection, LinkSecurity, Opened, SecureLink};
use jamjam::protocol::Packet;

/// A UDP socket that speaks the protocol like an app: it answers the key exchange, seals what it
/// sends and opens what it receives
pub struct Peer {
    pub socket: UdpSocket,
    pub link: SecureLink,
}

impl Peer {
    pub fn bind() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("peer socket");
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        Self {
            socket,
            link: SecureLink::new(),
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.socket.local_addr().expect("peer address")
    }

    /// Reads one datagram. `Ok(Some(..))` is a packet to act on; `Ok(None)` is one the link took
    /// (a key exchange, answered here) or refused; `Err` is the read timing out.
    pub fn recv(&self) -> io::Result<Option<(Packet, SocketAddr)>> {
        let mut buf = [0u8; 4096];
        let (len, from) = self.socket.recv_from(&mut buf)?;
        let Some(packet) = Packet::from_bytes(&buf[..len]) else {
            return Ok(None);
        };
        match self.link.open(packet) {
            Opened::Packet(packet) => Ok(Some((packet, from))),
            Opened::KeyExchange { answer } => {
                if answer {
                    self.send_key_exchange(from);
                    // Something encrypted shows the other end that we have the keys
                    self.send(Packet::keep_alive(0), from);
                }
                Ok(None)
            }
            Opened::Dropped(_) => Ok(None),
        }
    }

    /// The next packet the link accepts, whoever sent it
    pub fn recv_packet(&self) -> (Packet, SocketAddr) {
        loop {
            if let Some(received) = self.recv().expect("the sender's packet arrives") {
                return received;
            }
        }
    }

    /// Sends `packet` the way an app would: sealed once the keys are agreed
    pub fn send(&self, packet: Packet, to: SocketAddr) {
        if let Some(sealed) = self.link.seal(packet) {
            self.socket
                .send_to(&sealed.to_bytes(), to)
                .expect("peer send");
        }
    }

    pub fn send_key_exchange(&self, to: SocketAddr) {
        self.socket
            .send_to(&self.link.key_exchange_packet().to_bytes(), to)
            .expect("peer send");
    }

    /// Sends our key to `to` and reads until the keys are agreed, so what is sent next is
    /// encrypted
    pub fn agree_keys_with(&self, to: SocketAddr) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.link.security() != LinkSecurity::Encrypted {
            assert!(Instant::now() < deadline, "the keys were not agreed");
            self.send_key_exchange(to);
            let _ = self.recv();
        }
    }
}

/// Waits until every connection has agreed keys with its peer
pub async fn wait_until_encrypted(connections: &[&Connection]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while connections
        .iter()
        .any(|connection| connection.security() != LinkSecurity::Encrypted)
    {
        assert!(Instant::now() < deadline, "the keys were not agreed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// What a [`Tap`] is told about each datagram that crosses it, and what it sends on: usually the
/// datagram itself, but a test may change it, drop it (an empty list), repeat it or add to it
pub type Rewrite = Box<dyn Fn(SocketAddr, Vec<u8>) -> Vec<Vec<u8>> + Send + 'static>;

/// A relay between two connections that carries every datagram both ways and keeps a copy of each
/// as it was on the wire. Connect both ends to [`Tap::addr`].
pub struct Tap {
    addr: SocketAddr,
    log: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    socket: std::sync::Arc<tokio::net::UdpSocket>,
    to_a: SocketAddr,
    to_b: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Tap {
    pub async fn start(a: SocketAddr, b: SocketAddr) -> Self {
        Self::start_rewriting(a, b, Box::new(|_, datagram| vec![datagram])).await
    }

    pub async fn start_rewriting(a: SocketAddr, b: SocketAddr, rewrite: Rewrite) -> Self {
        let socket = std::sync::Arc::new(
            tokio::net::UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("tap socket"),
        );
        let addr = socket.local_addr().expect("tap address");
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let task = {
            let (socket, log) = (socket.clone(), log.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok((len, from)) = socket.recv_from(&mut buf).await {
                    let to = if from == a { b } else { a };
                    let datagram = buf[..len].to_vec();
                    log.lock().unwrap().push(datagram.clone());
                    for datagram in rewrite(from, datagram) {
                        let _ = socket.send_to(&datagram, to).await;
                    }
                }
            })
        };

        Self {
            addr,
            log,
            socket,
            to_a: a,
            to_b: b,
            task,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Everything that crossed the tap since the last [`Tap::forget`], as it was on the wire
    pub fn datagrams(&self) -> Vec<Vec<u8>> {
        self.log.lock().unwrap().clone()
    }

    pub fn forget(&self) {
        self.log.lock().unwrap().clear();
    }

    /// Sends `datagram` to the first end as though the other end had
    pub async fn inject_to_a(&self, datagram: &[u8]) {
        self.socket
            .send_to(datagram, self.to_a)
            .await
            .expect("inject");
    }

    /// Sends `datagram` to the second end as though the first end had
    pub async fn inject_to_b(&self, datagram: &[u8]) {
        self.socket
            .send_to(datagram, self.to_b)
            .await
            .expect("inject");
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.task.abort();
    }
}
