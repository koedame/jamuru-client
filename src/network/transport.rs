//! UDP transport layer

use std::io;
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

/// Held for reading by every send through a socket, and for writing while a socket sends with a
/// hop limit other than its own ([`send_with_hop_limit`]). The hop limit belongs to the socket, and
/// one socket carries the audio of every link in a room: a packet of another link sent meanwhile
/// would go out with the small hop limit and die on the way.
static HOP_LIMIT: parking_lot::RwLock<()> = parking_lot::RwLock::new(());

/// Sends `data` to `to` through `socket` with the socket's own hop limit
///
/// Every send through a socket the app shares goes through here, so that none of them goes out
/// while [`send_with_hop_limit`] has changed the hop limit.
pub(super) async fn send_datagram(
    socket: &UdpSocket,
    data: &[u8],
    to: SocketAddr,
) -> io::Result<usize> {
    loop {
        {
            let _gate = HOP_LIMIT.read();
            match socket.try_send_to(data, to) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                sent => return sent,
            }
        }
        socket.writable().await?;
    }
}

/// Sends `data` to each of `to` through `socket` with `hops` as the hop limit (the TTL of IPv4),
/// then gives the socket its own hop limit back. Returns how many of `to` it was sent to.
///
/// An IPv6 socket that takes IPv4 too sends to an IPv4 peer with its IPv4 TTL on some systems and
/// with its IPv6 hop limit on others, so both are set where the socket has them. Fails without
/// sending when neither can be set.
pub(super) fn send_with_hop_limit(
    socket: &UdpSocket,
    data: &[u8],
    to: &[SocketAddr],
    hops: u32,
) -> io::Result<usize> {
    let sock = SockRef::from(socket);
    let ipv6 = socket.local_addr()?.is_ipv6();
    let _gate = HOP_LIMIT.write();
    let own_v4 = sock.ttl_v4().ok();
    let own_v6 = if ipv6 {
        sock.unicast_hops_v6().ok()
    } else {
        None
    };
    if own_v4.is_none() && own_v6.is_none() {
        return Err(io::Error::other("the socket has no hop limit to set"));
    }
    let set = own_v4
        .map_or(Ok(()), |_| sock.set_ttl_v4(hops))
        .and_then(|()| own_v6.map_or(Ok(()), |_| sock.set_unicast_hops_v6(hops)));
    let sent = set.map(|()| {
        // Straight to the system: Tokio's `try_send_to` refuses until the runtime has seen the
        // socket writable, which a socket that has not sent yet it has not
        to.iter()
            .filter(|&&addr| sock.send_to(data, &addr.into()).is_ok())
            .count()
    });
    let restored = own_v4
        .map_or(Ok(()), |own| sock.set_ttl_v4(own))
        .and_then(|()| own_v6.map_or(Ok(()), |own| sock.set_unicast_hops_v6(own)));
    if let Err(e) = restored {
        error!("The socket's own hop limit could not be set back: {e}");
        return Err(e);
    }
    sent
}

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

    /// Says that the link now sends to `addr`, so that a shared socket's datagrams from there
    /// come to this transport for as long as it does
    pub(super) fn sending_to(&self, addr: SocketAddr) {
        if let Some(lane) = &self.lane {
            lane.follow(addr);
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
        send_datagram(&self.socket, &data, destination_for(self.local_addr, addr)).await?;
        trace!("Sent {} bytes to {}", data.len(), addr);
        Ok(())
    }

    /// Sends `packet` to each of `to` with `hops` as its hop limit instead of the socket's own
    /// ([`send_with_hop_limit`]). Returns how many of `to` it was sent to.
    pub(super) fn send_with_hop_limit(
        &self,
        packet: &Packet,
        to: &[SocketAddr],
        hops: u32,
    ) -> io::Result<usize> {
        let to: Vec<SocketAddr> = to
            .iter()
            .map(|&addr| destination_for(self.local_addr, addr))
            .collect();
        send_with_hop_limit(&self.socket, &packet.to_bytes(), &to, hops)
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

/// A socket that tells the hop limit (the TTL of IPv4) each datagram arrived with, for the tests
/// of [`send_with_hop_limit`]. Loopback does not lower it, so it is the one the sender set.
#[cfg(all(test, unix))]
pub(super) struct HopLimitReceiver {
    socket: std::net::UdpSocket,
}

#[cfg(all(test, unix))]
impl HopLimitReceiver {
    pub(super) fn bind(addr: &str) -> Self {
        use std::os::fd::AsRawFd;
        let socket = std::net::UdpSocket::bind(addr).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let (level, name) = if socket.local_addr().unwrap().is_ipv4() {
            (libc::IPPROTO_IP, libc::IP_RECVTTL)
        } else {
            (libc::IPPROTO_IPV6, libc::IPV6_RECVHOPLIMIT)
        };
        let on: libc::c_int = 1;
        // SAFETY: `on` outlives the call and its size is the one passed
        let set = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                level,
                name,
                (&on as *const libc::c_int).cast(),
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        assert_eq!(set, 0, "{}", io::Error::last_os_error());
        Self { socket }
    }

    pub(super) fn local_addr(&self) -> SocketAddr {
        self.socket.local_addr().unwrap()
    }

    /// The next datagram and the hop limit it arrived with; `None` when none comes for 2 s
    pub(super) fn recv(&self) -> Option<(Vec<u8>, u32)> {
        use std::os::fd::AsRawFd;
        let mut buf = vec![0u8; RECV_BUFFER_SIZE];
        let mut control = [0u64; 16];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        // SAFETY: an all-zero msghdr is a valid empty one
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&control) as _;
        // SAFETY: `msg` points at `iov`, `buf` and `control`, which outlive the call
        let len = unsafe { libc::recvmsg(self.socket.as_raw_fd(), &mut msg, 0) };
        if len < 0 {
            return None;
        }
        buf.truncate(len as usize);
        // SAFETY: the headers are walked as the system filled them in, within `control`
        let mut header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        while !header.is_null() {
            let (level, kind) = unsafe { ((*header).cmsg_level, (*header).cmsg_type) };
            let data = unsafe { libc::CMSG_DATA(header) };
            // Linux gives the TTL as an int under IP_TTL, macOS as a byte under IP_RECVTTL
            let hops = match (level, kind) {
                (libc::IPPROTO_IP, libc::IP_TTL) | (libc::IPPROTO_IPV6, libc::IPV6_HOPLIMIT) => {
                    Some(unsafe { data.cast::<libc::c_int>().read_unaligned() } as u32)
                }
                (libc::IPPROTO_IP, libc::IP_RECVTTL) => Some(u32::from(unsafe { *data })),
                _ => None,
            };
            if let Some(hops) = hops {
                return Some((buf, hops));
            }
            header = unsafe { libc::CMSG_NXTHDR(&msg, header) };
        }
        panic!("a datagram came without its hop limit");
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

    /// Sends one packet through `sender` with the hop limit `hops` and one with the socket's own,
    /// and returns the hop limits they arrived at `receiver` with
    #[cfg(unix)]
    async fn hop_limits_through(
        sender: &UdpTransport,
        receiver: &HopLimitReceiver,
        hops: u32,
    ) -> (u32, u32) {
        let to = receiver.local_addr();
        let sent = sender
            .send_with_hop_limit(&Packet::keep_alive(1), &[to], hops)
            .unwrap();
        assert_eq!(sent, 1);
        let (_, limited) = receiver
            .recv()
            .expect("the packet with the hop limit arrives");
        sender.send_to(&Packet::keep_alive(2), to).await.unwrap();
        let (_, own) = receiver.recv().expect("the next packet arrives");
        (limited, own)
    }

    /// Verifies: REQ-CON-181
    #[cfg(unix)]
    #[tokio::test]
    async fn when_a_dual_stack_socket_sends_to_an_ipv4_peer_with_a_hop_limit_the_packet_has_it_and_the_next_one_does_not(
    ) {
        let sender = UdpTransport::from_std(bind_audio_std(0).unwrap()).unwrap();
        let receiver = HopLimitReceiver::bind("127.0.0.1:0");

        let (limited, own) = hop_limits_through(&sender, &receiver, 3).await;

        assert_eq!(limited, 3);
        assert!(own > 3, "the socket's own hop limit came back: {own}");
    }

    /// Verifies: REQ-CON-181
    #[cfg(unix)]
    #[tokio::test]
    async fn when_a_dual_stack_socket_sends_to_an_ipv6_peer_with_a_hop_limit_the_packet_has_it_and_the_next_one_does_not(
    ) {
        if !ip6_loopback_is_usable() {
            return;
        }
        let sender = UdpTransport::from_std(bind_audio_std(0).unwrap()).unwrap();
        let receiver = HopLimitReceiver::bind("[::1]:0");

        let (limited, own) = hop_limits_through(&sender, &receiver, 3).await;

        assert_eq!(limited, 3);
        assert!(own > 3, "the socket's own hop limit came back: {own}");
    }

    /// Verifies: REQ-CON-181
    #[cfg(unix)]
    #[tokio::test]
    async fn when_an_ipv4_socket_sends_with_a_hop_limit_the_packet_has_it_and_the_next_one_does_not(
    ) {
        let sender = UdpTransport::bind("127.0.0.1:0").await.unwrap();
        let receiver = HopLimitReceiver::bind("127.0.0.1:0");

        let (limited, own) = hop_limits_through(&sender, &receiver, 3).await;

        assert_eq!(limited, 3);
        assert!(own > 3, "the socket's own hop limit came back: {own}");
    }

    /// The other links of a room send their audio through the same socket while one link sends
    /// with a small hop limit; none of their packets may go out with it.
    ///
    /// Verifies: REQ-CON-181
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn when_one_link_sends_with_a_hop_limit_while_another_sends_audio_through_the_socket_the_audio_keeps_the_sockets_own(
    ) {
        let sender = Arc::new(UdpTransport::from_std(bind_audio_std(0).unwrap()).unwrap());
        let audio = Arc::new(HopLimitReceiver::bind("127.0.0.1:0"));
        let opened = HopLimitReceiver::bind("127.0.0.1:0");
        let audio_at = audio.local_addr();
        let packets = 2000;

        let other_link = {
            let sender = sender.clone();
            tokio::spawn(async move {
                for sequence in 0..packets {
                    sender
                        .send_to(&Packet::audio(sequence, 0, vec![0u8; 256]), audio_at)
                        .await
                        .unwrap();
                    tokio::task::yield_now().await;
                }
            })
        };
        let heard = {
            let audio = audio.clone();
            tokio::task::spawn_blocking(move || {
                let mut hops = Vec::new();
                while hops.len() < packets as usize {
                    match audio.recv() {
                        Some((_, h)) => hops.push(h),
                        None => break,
                    }
                }
                hops
            })
        };
        while !other_link.is_finished() {
            sender
                .send_with_hop_limit(&Packet::keep_alive(0), &[opened.local_addr()], 1)
                .unwrap();
            tokio::task::yield_now().await;
        }
        other_link.await.unwrap();
        let hops = heard.await.unwrap();

        assert_eq!(hops.len(), packets as usize, "every audio packet arrives");
        assert!(
            hops.iter().all(|&h| h > 1),
            "{} audio packets went out with the hop limit 1",
            hops.iter().filter(|&&h| h == 1).count()
        );
    }
}
