//! UDP transport layer

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use socket2::{Domain, Protocol, SockRef, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace};

use crate::protocol::Packet;

use super::error::NetworkError;
use super::udp_mux::Lane;

/// Bytes a datagram is read into. A packet is the 12-byte header and its
/// payload: a 256-frame stereo frame of 32-bit samples alone is 2048 bytes.
/// A datagram that fills the buffer may have been cut short, so it is refused
/// rather than played with its end missing.
pub(super) const RECV_BUFFER_SIZE: usize = 4096;

/// UDP transport for sending and receiving packets
pub struct UdpTransport {
    socket: Arc<UdpSocket>,
    local_addr: SocketAddr,
    /// Present when the socket is shared with the transports to other peers
    /// ([`super::UdpMux`]): what this transport's peer sends arrives here
    /// instead of being read off the socket.
    lane: Option<Lane>,
}

/// The IP families a socket can send to and receive from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpFamilies {
    pub v4: bool,
    pub v6: bool,
}

impl IpFamilies {
    /// An IPv4 socket
    pub const V4_ONLY: Self = Self {
        v4: true,
        v6: false,
    };

    /// What a socket bound to `local` can reach. An IPv6 socket reaches IPv4 hosts too (as
    /// IPv4-mapped addresses) unless it is IPv6-only.
    pub fn of_local(local: SocketAddr, only_v6: bool) -> Self {
        Self {
            v4: local.is_ipv4() || !only_v6,
            v6: local.is_ipv6(),
        }
    }

    /// What `socket` can reach
    pub fn of_std(socket: &std::net::UdpSocket) -> Self {
        match socket.local_addr() {
            Ok(local) => Self::of_local(local, SockRef::from(socket).only_v6().unwrap_or(true)),
            Err(_) => Self::V4_ONLY,
        }
    }

    /// What `socket` can reach
    pub fn of_tokio(socket: &UdpSocket) -> Self {
        match socket.local_addr() {
            Ok(local) => Self::of_local(local, SockRef::from(socket).only_v6().unwrap_or(true)),
            Err(_) => Self::V4_ONLY,
        }
    }

    /// Whether a socket of these families can send to `addr`
    pub fn reach(&self, addr: IpAddr) -> bool {
        match addr.to_canonical() {
            IpAddr::V4(_) => self.v4,
            IpAddr::V6(_) => self.v6,
        }
    }
}

/// `addr` as the system reports a datagram's sender: an IPv4-mapped address
/// (`::ffff:192.0.2.1`), which a dual-stack socket reports for an IPv4 peer, is the IPv4
/// address itself. One address has one spelling everywhere the app compares or shows it.
pub fn canonical_addr(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

/// `addr` as a socket bound to `local` takes it: an IPv6 socket, dual-stack or not, is given
/// an IPv4 destination as an IPv4-mapped address (Linux alone accepts a plain IPv4 one).
pub(super) fn destination_for(local: SocketAddr, addr: SocketAddr) -> SocketAddr {
    match (local, addr) {
        (SocketAddr::V6(_), SocketAddr::V4(v4)) => {
            SocketAddr::new(IpAddr::V6(v4.ip().to_ipv6_mapped()), v4.port())
        }
        _ => addr,
    }
}

/// Binds a UDP socket with the options this transport needs, without
/// touching Tokio.
///
/// Exposed so a caller can bind early - to learn its port and advertise it -
/// and hand the socket to whichever runtime will actually poll it
/// ([`UdpTransport::from_std`]). See ADR-026.
///
/// An unspecified IPv6 address (`[::]:port`) gives a dual-stack socket - one that takes IPv4
/// peers too - on every system (Windows makes it IPv6-only unless told otherwise).
pub fn bind_std(addr: &str) -> Result<std::net::UdpSocket, NetworkError> {
    let parsed_addr: SocketAddr = addr.parse()?;

    let domain = if parsed_addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

    if parsed_addr.is_ipv6() && parsed_addr.ip().is_unspecified() {
        socket.set_only_v6(false)?;
    }
    // SO_REUSEADDR so a quick rejoin after leaving a session can rebind its port. Not for a
    // port the system picks: it then gives the same port to every socket that asks with the
    // option, and the datagrams of all of them go to whichever bound last.
    if parsed_addr.port() != 0 {
        socket.set_reuse_address(true)?;
    }
    // Non-blocking is required before Tokio adopts the socket.
    socket.set_nonblocking(true)?;
    socket.bind(&parsed_addr.into())?;

    Ok(socket.into())
}

/// Binds the socket the audio of a whole session goes through: one dual-stack socket, so
/// that a peer is reached over IPv6 when both ends have it and over IPv4 otherwise (ADR-093).
/// Falls back to an IPv4 socket where the system has no IPv6 or will not make a socket take
/// both.
pub fn bind_audio_std(port: u16) -> Result<std::net::UdpSocket, NetworkError> {
    match bind_std(&format!("[::]:{port}")) {
        Ok(socket) => Ok(socket),
        Err(e) => {
            info!("No dual-stack audio socket ({e}); using IPv4 only");
            bind_std(&format!("0.0.0.0:{port}"))
        }
    }
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
            lane: None,
        })
    }

    /// A transport that sends through `socket` and receives through `lane`
    pub(super) fn sharing(socket: Arc<UdpSocket>, local_addr: SocketAddr, lane: Lane) -> Self {
        Self {
            socket,
            local_addr,
            lane: Some(lane),
        }
    }

    /// Says where the peer sends from, so that a shared socket's datagrams from
    /// there come to this transport. A transport that has its socket to itself
    /// receives everything anyway.
    pub(super) fn expect_peer_at(&self, peer_addrs: &[SocketAddr]) {
        if let Some(lane) = &self.lane {
            lane.claim(peer_addrs);
        }
    }

    /// Says that a packet that got through the link came from `addr`, so that a shared socket's
    /// datagrams from there come to this transport as well
    pub(super) fn also_expect_peer_at(&self, addr: SocketAddr) {
        if let Some(lane) = &self.lane {
            lane.learn(addr);
        }
    }

    /// Stops taking the peer's datagrams off a shared socket, which ends a
    /// receive that is waiting for one
    pub(super) fn release(&self) {
        if let Some(lane) = &self.lane {
            lane.release();
        }
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
        self.socket
            .send_to(&data, destination_for(self.local_addr, addr))
            .await?;
        trace!("Sent {} bytes to {}", data.len(), addr);
        Ok(())
    }

    /// Receive a packet (returns packet and sender address)
    ///
    /// `NetworkError::NotConnected` from a transport that shares its socket
    /// means it no longer takes its peer's datagrams: nothing more will come.
    pub async fn recv_from(&self) -> Result<(Packet, SocketAddr), NetworkError> {
        let (buf, addr) = match &self.lane {
            Some(lane) => lane.recv().await.ok_or(NetworkError::NotConnected)?,
            None => {
                let mut buf = vec![0u8; RECV_BUFFER_SIZE];
                let (len, addr) = self.socket.recv_from(&mut buf).await?;
                if len == RECV_BUFFER_SIZE {
                    debug!("Dropping a datagram from {addr} that fills the receive buffer");
                    return Err(NetworkError::InvalidPacket);
                }
                buf.truncate(len);
                (buf, canonical_addr(addr))
            }
        };

        let packet = Packet::from_bytes(&buf).ok_or(NetworkError::InvalidPacket)?;
        trace!("Received {} bytes from {}", buf.len(), addr);

        Ok((packet, addr))
    }

    /// Receive raw bytes (for connectivity probing without packet parsing)
    pub async fn recv_raw(&self) -> Result<(Vec<u8>, SocketAddr), NetworkError> {
        if let Some(lane) = &self.lane {
            return lane.recv().await.ok_or(NetworkError::NotConnected);
        }
        let mut buf = vec![0u8; RECV_BUFFER_SIZE];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        buf.truncate(len);
        trace!("Received {} raw bytes from {}", len, addr);
        Ok((buf, canonical_addr(addr)))
    }

    /// Receive raw bytes if a datagram is already waiting, without blocking
    pub fn try_recv_raw(&self) -> Option<(Vec<u8>, SocketAddr)> {
        if let Some(lane) = &self.lane {
            return lane.try_recv();
        }
        let mut buf = vec![0u8; RECV_BUFFER_SIZE];
        let (len, addr) = self.socket.try_recv_from(&mut buf).ok()?;
        buf.truncate(len);
        Some((buf, canonical_addr(addr)))
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
                    Err(NetworkError::NotConnected) => {
                        debug!("The transport takes no more datagrams, stopping receive loop");
                        break;
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

    /// Test a port the system picks stays with its socket: another one that asks for the
    /// port with SO_REUSEADDR would take the datagrams meant for it. (On Windows
    /// SO_REUSEADDR lets a socket take over a port whatever the other asked for.)
    #[cfg(unix)]
    #[tokio::test]
    async fn test_transport_port_the_system_picked_is_refused_to_a_second_socket() {
        let transport = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("127.0.0.1:{}", transport.local_addr().port());

        assert!(UdpTransport::bind(&addr).await.is_err());
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

    fn ip6_loopback_is_usable() -> bool {
        bind_std("[::1]:0").is_ok()
    }

    /// Verifies: REQ-CON-145
    #[test]
    fn when_the_system_has_ipv6_the_audio_socket_is_dual_stack() {
        let socket = bind_audio_std(0).unwrap();

        let families = IpFamilies::of_std(&socket);

        if ip6_loopback_is_usable() {
            assert!(socket.local_addr().unwrap().is_ipv6());
            assert_eq!(families, IpFamilies { v4: true, v6: true });
        } else {
            assert_eq!(families, IpFamilies::V4_ONLY);
        }
    }

    /// Verifies: REQ-CON-145
    #[test]
    fn when_the_address_is_ipv6_unspecified_the_socket_takes_ipv4_peers_too() {
        if !ip6_loopback_is_usable() {
            return;
        }

        let socket = bind_std("[::]:0").unwrap();

        assert!(IpFamilies::of_std(&socket).v4);
    }

    /// Verifies: REQ-CON-145
    #[test]
    fn when_a_socket_is_ipv6_only_it_does_not_reach_ipv4_hosts() {
        let families = IpFamilies::of_local("[::1]:5000".parse().unwrap(), true);

        assert!(families.v6);
        assert!(!families.reach("192.0.2.1".parse().unwrap()));
        assert!(families.reach("2001:db8::1".parse().unwrap()));
    }

    /// Verifies: REQ-CON-145
    #[test]
    fn when_an_address_is_ipv4_mapped_it_is_the_ipv4_address() {
        let mapped: SocketAddr = "[::ffff:192.0.2.1]:5000".parse().unwrap();

        assert_eq!(
            canonical_addr(mapped),
            "192.0.2.1:5000".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            canonical_addr("[2001:db8::1]:5000".parse().unwrap()),
            "[2001:db8::1]:5000".parse::<SocketAddr>().unwrap()
        );
    }

    /// Verifies: REQ-CON-145
    #[test]
    fn when_an_ipv6_socket_sends_to_ipv4_the_destination_is_ipv4_mapped() {
        let v6: SocketAddr = "[::]:5000".parse().unwrap();
        let v4: SocketAddr = "192.0.2.1:6000".parse().unwrap();

        assert_eq!(
            destination_for(v6, v4),
            "[::ffff:192.0.2.1]:6000".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(destination_for("0.0.0.0:5000".parse().unwrap(), v4), v4);
        let v6_peer: SocketAddr = "[2001:db8::1]:6000".parse().unwrap();
        assert_eq!(destination_for(v6, v6_peer), v6_peer);
    }

    /// Verifies: REQ-CON-145
    #[tokio::test]
    async fn when_a_dual_stack_socket_talks_to_an_ipv4_peer_both_see_ipv4_addresses() {
        if !ip6_loopback_is_usable() {
            return;
        }
        let audio = UdpTransport::from_std(bind_audio_std(0).unwrap()).unwrap();
        let peer = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let audio_at = SocketAddr::from(([127, 0, 0, 1], audio.local_addr().port()));

        audio
            .send_to(&Packet::keep_alive(1), peer.local_addr())
            .await
            .unwrap();
        let (_, heard_by_peer) = peer.recv_from().await.unwrap();
        peer.send_to(&Packet::keep_alive(2), audio_at)
            .await
            .unwrap();
        let (_, heard_by_audio) = audio.recv_from().await.unwrap();

        assert_eq!(heard_by_peer, audio_at);
        assert_eq!(heard_by_audio, peer.local_addr());
    }

    /// Verifies: REQ-CON-145
    #[tokio::test]
    async fn when_a_dual_stack_socket_talks_to_an_ipv6_peer_the_audio_arrives() {
        if !ip6_loopback_is_usable() {
            return;
        }
        let audio = UdpTransport::from_std(bind_audio_std(0).unwrap()).unwrap();
        let peer = UdpTransport::bind("[::1]:0").await.unwrap();
        let audio_at = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, audio.local_addr().port()));

        audio
            .send_to(&Packet::keep_alive(1), peer.local_addr())
            .await
            .unwrap();
        let (_, heard_by_peer) = peer.recv_from().await.unwrap();
        peer.send_to(&Packet::keep_alive(2), audio_at)
            .await
            .unwrap();
        let (_, heard_by_audio) = audio.recv_from().await.unwrap();

        assert_eq!(heard_by_peer, audio_at);
        assert_eq!(heard_by_audio, peer.local_addr());
    }
}
