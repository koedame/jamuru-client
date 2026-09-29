//! UDP transport layer

use std::net::SocketAddr;
use std::sync::Arc;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace};

use crate::protocol::Packet;

use super::error::NetworkError;

/// Bytes a datagram is read into. A packet is the 12-byte header and its
/// payload: a 256-frame stereo frame of 32-bit samples alone is 2048 bytes.
/// A datagram that fills the buffer may have been cut short, so it is refused
/// rather than played with its end missing.
const RECV_BUFFER_SIZE: usize = 4096;

/// UDP transport for sending and receiving packets
pub struct UdpTransport {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
}

/// Binds a UDP socket with the options this transport needs, without
/// touching Tokio.
///
/// Exposed so a caller can bind early - to learn its port and advertise it -
/// and hand the socket to whichever runtime will actually poll it
/// ([`UdpTransport::from_std`]). See ADR-026.
pub fn bind_std(addr: &str) -> Result<std::net::UdpSocket, NetworkError> {
    let parsed_addr: SocketAddr = addr.parse()?;

    let domain = if parsed_addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

    // SO_REUSEADDR so a quick rejoin after leaving a session can rebind.
    socket.set_reuse_address(true)?;
    // Non-blocking is required before Tokio adopts the socket.
    socket.set_nonblocking(true)?;
    socket.bind(&parsed_addr.into())?;

    Ok(socket.into())
}

impl UdpTransport {
    /// Bind to a local address with SO_REUSEADDR enabled
    pub async fn bind(addr: &str) -> Result<Self, NetworkError> {
        Self::from_std(bind_std(addr)?)
    }

    /// Adopts an already-bound standard socket.
    ///
    /// Split from [`Self::bind`] so a caller can learn its port before the
    /// audio thread exists. A `tokio::net::UdpSocket` is registered with the
    /// reactor of whichever runtime created it and stops receiving wakeups if
    /// moved to another, so the conversion has to happen on the runtime that
    /// will poll it - `std::net::UdpSocket` is just a descriptor and moves
    /// freely. See ADR-026.
    ///
    /// Must be called from within the Tokio runtime that will drive this
    /// transport.
    pub fn from_std(std_socket: std::net::UdpSocket) -> Result<Self, NetworkError> {
        let socket = UdpSocket::from_std(std_socket)?;
        let local_addr = socket.local_addr()?;

        info!("UDP transport bound to {}", local_addr);

        Ok(Self {
            socket: Arc::new(socket),
            local_addr,
        })
    }

    /// Get the local address
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The underlying socket, for asking STUN through the address peers will
    /// send audio to (see [`super::gather_candidates`]).
    pub fn socket(&self) -> &Arc<UdpSocket> {
        &self.socket
    }

    /// Send a packet to a remote address
    pub async fn send_to(&self, packet: &Packet, addr: SocketAddr) -> Result<(), NetworkError> {
        let data = packet.to_bytes();
        self.socket.send_to(&data, addr).await?;
        trace!("Sent {} bytes to {}", data.len(), addr);
        Ok(())
    }

    /// Receive a packet (returns packet and sender address)
    pub async fn recv_from(&self) -> Result<(Packet, SocketAddr), NetworkError> {
        let mut buf = vec![0u8; RECV_BUFFER_SIZE];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        if len == RECV_BUFFER_SIZE {
            debug!("Dropping a datagram from {addr} that fills the receive buffer");
            return Err(NetworkError::InvalidPacket);
        }
        buf.truncate(len);

        let packet = Packet::from_bytes(&buf).ok_or(NetworkError::InvalidPacket)?;
        trace!("Received {} bytes from {}", len, addr);

        Ok((packet, addr))
    }

    /// Receive raw bytes (for connectivity probing without packet parsing)
    pub async fn recv_raw(&self) -> Result<(Vec<u8>, SocketAddr), NetworkError> {
        let mut buf = vec![0u8; RECV_BUFFER_SIZE];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        trace!("Received {} raw bytes from {}", len, addr);
        Ok((buf, addr))
    }

    /// Start a receive loop that sends packets to a channel
    pub fn start_receive_loop(
        self: Arc<Self>,
    ) -> (
        mpsc::Receiver<(Packet, SocketAddr)>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, rx) = mpsc::channel(1024);
        let socket = self.clone();

        let handle = tokio::spawn(async move {
            loop {
                match socket.recv_from().await {
                    Ok((packet, addr)) => {
                        if tx.send((packet, addr)).await.is_err() {
                            debug!("Receive channel closed, stopping receive loop");
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Receive error: {}", e);
                    }
                }
            }
        });

        (rx, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transport_bind() {
        let transport = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        assert!(transport.local_addr().port() > 0);
    }

    #[tokio::test]
    async fn test_transport_send_receive() {
        let transport1 = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let transport2 = UdpTransport::bind("127.0.0.1:0").await.unwrap();

        let packet = Packet::audio(1, 100, vec![1, 2, 3, 4]);
        transport1
            .send_to(&packet, transport2.local_addr())
            .await
            .unwrap();

        let (received, from_addr) = transport2.recv_from().await.unwrap();
        assert_eq!(received.sequence, 1);
        assert_eq!(received.timestamp, 100);
        assert_eq!(received.payload, vec![1, 2, 3, 4]);
        assert_eq!(from_addr, transport1.local_addr());
    }

    #[tokio::test]
    async fn test_transport_receives_a_packet_carrying_a_256_frame_stereo_frame() {
        let transport1 = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let transport2 = UdpTransport::bind("127.0.0.1:0").await.unwrap();

        let payload: Vec<u8> = (0..2048).map(|i| i as u8).collect();
        let packet = Packet::audio(1, 0, payload.clone());
        transport1
            .send_to(&packet, transport2.local_addr())
            .await
            .unwrap();

        let (received, _) = transport2.recv_from().await.unwrap();
        assert_eq!(received.payload, payload);
    }

    #[tokio::test]
    async fn test_transport_refuses_a_datagram_that_may_have_been_cut_short() {
        let transport1 = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let transport2 = UdpTransport::bind("127.0.0.1:0").await.unwrap();

        let packet = Packet::audio(1, 0, vec![0; RECV_BUFFER_SIZE]);
        transport1
            .send_to(&packet, transport2.local_addr())
            .await
            .unwrap();

        assert!(matches!(
            transport2.recv_from().await,
            Err(NetworkError::InvalidPacket)
        ));
    }

    /// Test SO_REUSEADDR allows rebinding to same port after drop
    #[tokio::test]
    async fn test_transport_port_reuse() {
        // First bind to get an assigned port
        let transport1 = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let port = transport1.local_addr().port();
        let addr = format!("127.0.0.1:{}", port);

        // Drop the first transport
        drop(transport1);

        // Should be able to immediately rebind to the same port (SO_REUSEADDR)
        let transport2 = UdpTransport::bind(&addr).await;
        assert!(
            transport2.is_ok(),
            "Should be able to rebind to same port with SO_REUSEADDR"
        );
        assert_eq!(transport2.unwrap().local_addr().port(), port);
    }

    /// Test multiple consecutive rebinds to same port
    #[tokio::test]
    async fn test_transport_repeated_port_reuse() {
        // First bind to get an assigned port
        let transport = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let port = transport.local_addr().port();
        let addr = format!("127.0.0.1:{}", port);
        drop(transport);

        // Rebind multiple times in succession
        for i in 0..5 {
            let transport = UdpTransport::bind(&addr).await;
            assert!(transport.is_ok(), "Rebind attempt {} should succeed", i + 1);
            drop(transport);
        }
    }
}
