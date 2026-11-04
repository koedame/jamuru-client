//! Marking the audio packets as time-critical (ADR-098)
//!
//! An IP header has a six-bit priority field (DSCP; the upper bits of IPv4's ToS byte and
//! IPv6's Traffic Class). A home Wi-Fi access point, and the Wi-Fi adapter of the sending PC,
//! read it to pick one of the four WMM queues (RFC 8325): with audio marked Expedited
//! Forwarding (46) it is sent ahead of a video stream or a download on the same Wi-Fi. Most
//! of the internet clears the field, so what this buys is on the first hop, inside the home.
//!
//! How the mark is set differs by system, and is set for the whole session, on its one socket:
//!
//! - Linux: `IP_TOS` / `IPV6_TCLASS` to EF, and `SO_PRIORITY` 6 for the local queue.
//! - macOS: `SO_NET_SERVICE_TYPE` to voice, which sets both the DSCP and the Wi-Fi queue. If
//!   the system refuses it, `IP_TOS` / `IPV6_TCLASS`.
//! - Windows: `setsockopt(IP_TOS)` is ignored unless a policy allows it, so the packets go
//!   through qWAVE, the system's QoS service, as a voice flow. A flow is made for one
//!   destination, so one is added for each address the first time a packet goes to it. No
//!   administrator is needed (setting the DSCP value by hand would).
//!
//! The mark is a request: a network may clear it or, rarely, drop what is marked. That is
//! why it is a setting (`voice_priority`).

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tracing::{info, warn};

/// EF (46) in the upper six bits of the byte
#[cfg(unix)]
const EF_TRAFFIC_CLASS: u32 = 46 << 2;

/// How `SO_NET_SERVICE_TYPE` is spelled in `<sys/socket.h>`, and the value of
/// `NET_SERVICE_TYPE_VO` (libc has neither)
#[cfg(target_os = "macos")]
const SO_NET_SERVICE_TYPE: libc::c_int = 0x1116;
#[cfg(target_os = "macos")]
const NET_SERVICE_TYPE_VO: libc::c_int = 4;

/// The audio socket's packets are marked as voice, for as long as this lives
pub struct VoicePriority {
    how: &'static str,
    #[cfg(windows)]
    flows: windows_qos::Flows,
}

impl VoicePriority {
    /// Marks what `socket` sends. `None` where the system does not take it (the reason is
    /// logged): audio goes unmarked then.
    pub fn on(socket: &UdpSocket) -> Option<Self> {
        match mark(socket) {
            Ok(priority) => {
                info!("Audio packets are marked as voice ({})", priority.how);
                Some(priority)
            }
            Err(reason) => {
                warn!("Audio packets are not marked as voice: {reason}");
                None
            }
        }
    }

    /// What was used to mark the packets, for the log and the tests
    pub fn how(&self) -> &'static str {
        self.how
    }

    /// Called as a packet goes to `addr`. On Windows the first one to an address adds the flow
    /// that marks the packets to it; elsewhere the mark is on the socket and there is
    /// nothing to do.
    #[inline]
    pub(super) fn sending_to(&self, socket: &std::sync::Arc<UdpSocket>, addr: SocketAddr) {
        #[cfg(windows)]
        self.flows.add_for(socket, addr);
        #[cfg(not(windows))]
        let _ = (socket, addr);
    }
}

#[cfg(target_os = "linux")]
fn mark(socket: &UdpSocket) -> Result<VoicePriority, String> {
    let sock = socket2::SockRef::from(socket);
    let local = socket.local_addr().map_err(|e| e.to_string())?;
    // An IPv6 socket sends an IPv4 peer's packets with the IPv4 header, so it needs both
    let v4 = sock.set_tos_v4(EF_TRAFFIC_CLASS);
    let v6 = if local.is_ipv6() {
        sock.set_tclass_v6(EF_TRAFFIC_CLASS)
    } else {
        Ok(())
    };
    // The queue of this machine's own network stack and Wi-Fi driver (0-6 need no privilege)
    let queue = sock.set_priority(6);
    let marked = if local.is_ipv6() {
        v4.is_ok() || v6.is_ok()
    } else {
        v4.is_ok()
    };
    if !marked {
        let reason = v4.err().or(v6.err()).map(|e| e.to_string());
        return Err(reason.unwrap_or_default());
    }
    Ok(VoicePriority {
        how: if queue.is_ok() {
            "IP_TOS, IPV6_TCLASS and SO_PRIORITY"
        } else {
            "IP_TOS and IPV6_TCLASS"
        },
    })
}

#[cfg(target_os = "macos")]
fn mark(socket: &UdpSocket) -> Result<VoicePriority, String> {
    use std::os::fd::AsRawFd;

    let value = NET_SERVICE_TYPE_VO;
    // SAFETY: a valid descriptor, and a pointer to a c_int of the size given
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_NET_SERVICE_TYPE,
            std::ptr::addr_of!(value).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result == 0 {
        return Ok(VoicePriority {
            how: "SO_NET_SERVICE_TYPE",
        });
    }
    let refused = std::io::Error::last_os_error();
    info!("SO_NET_SERVICE_TYPE was refused ({refused}); using IP_TOS");

    let sock = socket2::SockRef::from(socket);
    let local = socket.local_addr().map_err(|e| e.to_string())?;
    let v4 = sock.set_tos_v4(EF_TRAFFIC_CLASS);
    let v6 = if local.is_ipv6() {
        sock.set_tclass_v6(EF_TRAFFIC_CLASS)
    } else {
        Ok(())
    };
    v4.and(v6).map_err(|e| format!("IP_TOS: {e}"))?;
    Ok(VoicePriority {
        how: "IP_TOS and IPV6_TCLASS",
    })
}

#[cfg(windows)]
fn mark(_socket: &UdpSocket) -> Result<VoicePriority, String> {
    Ok(VoicePriority {
        how: "qWAVE voice flows",
        flows: windows_qos::Flows::open()?,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn mark(_socket: &UdpSocket) -> Result<VoicePriority, String> {
    Err("this system has no way to mark them".to_string())
}

#[cfg(windows)]
mod windows_qos {
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::os::windows::io::AsRawSocket;
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tokio::net::UdpSocket;
    use tracing::{debug, info, warn};
    use windows::Win32::Foundation::{GetLastError, HANDLE};
    use windows::Win32::NetworkManagement::QoS::{
        QOSAddSocketToFlow, QOSCloseHandle, QOSCreateHandle, QOSTrafficTypeVoice,
        QOS_NON_ADAPTIVE_FLOW, QOS_VERSION,
    };
    use windows::Win32::Networking::WinSock::{SOCKADDR, SOCKET};

    /// qWAVE's handle, and the addresses a flow has been asked for
    pub(super) struct Flows {
        handle: Arc<Handle>,
        asked: Mutex<HashSet<SocketAddr>>,
    }

    struct Handle(HANDLE);

    // SAFETY: a qWAVE handle may be used from any thread
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle QOSCreateHandle gave, closed once. Closing ends its flows.
            unsafe {
                let _ = QOSCloseHandle(self.0);
            }
        }
    }

    impl Flows {
        pub(super) fn open() -> Result<Self, String> {
            let version = QOS_VERSION {
                MajorVersion: 1,
                MinorVersion: 0,
            };
            let mut handle = HANDLE::default();
            // SAFETY: valid pointers to a version and a handle
            let created = unsafe { QOSCreateHandle(&version, &mut handle) };
            if !created.as_bool() {
                // SAFETY: reads the thread's last error
                let code = unsafe { GetLastError() };
                return Err(format!("QOSCreateHandle failed ({code:?})"));
            }
            Ok(Self {
                handle: Arc::new(Handle(handle)),
                asked: Mutex::new(HashSet::new()),
            })
        }

        /// Asks qWAVE to mark what `socket` sends to `addr`, if it has not been asked
        ///
        /// Done on a thread of its own: qWAVE answers over RPC to a service, which can take
        /// a few milliseconds, and the audio of the other peers is received on this thread.
        /// The packets sent before it answers go unmarked.
        pub(super) fn add_for(&self, socket: &Arc<UdpSocket>, addr: SocketAddr) {
            let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port());
            if !self.asked.lock().insert(addr) {
                return;
            }
            let handle = self.handle.clone();
            let socket = socket.clone();
            let spawned = std::thread::Builder::new()
                .name("voice-flow".to_string())
                .spawn(move || {
                    let dest = socket2::SockAddr::from(addr);
                    let mut flow_id = 0u32;
                    // SAFETY: the handle and the socket are alive (this thread holds both),
                    // `dest` is a sockaddr of the length it says, `flow_id` starts at 0
                    let added = unsafe {
                        QOSAddSocketToFlow(
                            handle.0,
                            SOCKET(socket.as_raw_socket() as usize),
                            Some(dest.as_ptr().cast::<SOCKADDR>()),
                            QOSTrafficTypeVoice,
                            Some(QOS_NON_ADAPTIVE_FLOW),
                            &mut flow_id,
                        )
                    };
                    if added.as_bool() {
                        info!("Packets to {addr} are marked as voice (qWAVE flow {flow_id})");
                    } else {
                        // SAFETY: reads the thread's last error
                        let code = unsafe { GetLastError() };
                        warn!("Packets to {addr} are not marked as voice: QOSAddSocketToFlow failed ({code:?})");
                    }
                });
            if let Err(e) = spawned {
                debug!("No thread for the voice flow to {addr}: {e}");
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    /// What a receiver reads as the traffic class of a datagram, from the IP header
    fn received_traffic_class(receiver: &std::net::UdpSocket, v6: bool) -> u8 {
        let on: libc::c_int = 1;
        let (level, option) = if v6 {
            (libc::IPPROTO_IPV6, libc::IPV6_RECVTCLASS)
        } else {
            (libc::IPPROTO_IP, libc::IP_RECVTOS)
        };
        // SAFETY: a valid descriptor and a c_int
        unsafe {
            assert_eq!(
                libc::setsockopt(
                    receiver.as_raw_fd(),
                    level,
                    option,
                    std::ptr::addr_of!(on).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                ),
                0
            );
        }
        let mut data = [0u8; 16];
        let mut iov = libc::iovec {
            iov_base: data.as_mut_ptr().cast(),
            iov_len: data.len(),
        };
        let mut control = [0u8; 128];
        // SAFETY: zeroed is a valid empty msghdr, filled in below
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len() as _;
        // SAFETY: msg points at buffers that live through the call
        let got = unsafe { libc::recvmsg(receiver.as_raw_fd(), &mut msg, 0) };
        assert!(got > 0, "no datagram: {}", std::io::Error::last_os_error());
        // SAFETY: walks the control messages the kernel wrote into `control`
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                let is_class = if v6 {
                    (*cmsg).cmsg_level == libc::IPPROTO_IPV6
                        && (*cmsg).cmsg_type == libc::IPV6_TCLASS
                } else {
                    (*cmsg).cmsg_level == libc::IPPROTO_IP && (*cmsg).cmsg_type == libc::IP_TOS
                };
                if is_class {
                    // IPv4 gives the byte; IPv6 gives an int
                    return if v6 {
                        *(libc::CMSG_DATA(cmsg).cast::<libc::c_int>()) as u8
                    } else {
                        *libc::CMSG_DATA(cmsg)
                    };
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        panic!("the datagram carried no traffic class");
    }

    fn receiver(bind: &str) -> std::net::UdpSocket {
        let socket = std::net::UdpSocket::bind(bind).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        socket
    }

    async fn marked_audio_socket() -> (UdpSocket, VoicePriority) {
        let socket = UdpSocket::from_std(crate::network::bind_audio_std(0).unwrap()).unwrap();
        let priority = VoicePriority::on(&socket).expect("the socket takes the mark");
        (socket, priority)
    }

    /// Verifies: REQ-CON-160
    #[tokio::test]
    async fn when_the_audio_socket_is_marked_an_ipv4_peer_reads_expedited_forwarding() {
        let (socket, _priority) = marked_audio_socket().await;
        let peer = receiver("127.0.0.1:0");

        socket
            .send_to(b"audio", peer.local_addr().unwrap())
            .await
            .unwrap();

        assert_eq!(received_traffic_class(&peer, false), EF_TRAFFIC_CLASS as u8);
    }

    /// Verifies: REQ-CON-160
    #[tokio::test]
    async fn when_the_audio_socket_is_marked_an_ipv6_peer_reads_expedited_forwarding() {
        if std::net::UdpSocket::bind("[::1]:0").is_err() {
            return;
        }
        let (socket, _priority) = marked_audio_socket().await;
        let peer = receiver("[::1]:0");

        socket
            .send_to(b"audio", peer.local_addr().unwrap())
            .await
            .unwrap();

        assert_eq!(received_traffic_class(&peer, true), EF_TRAFFIC_CLASS as u8);
    }

    /// Verifies: REQ-CON-160
    #[tokio::test]
    async fn when_the_audio_socket_is_not_marked_a_peer_reads_no_priority() {
        let socket = UdpSocket::from_std(crate::network::bind_audio_std(0).unwrap()).unwrap();
        let peer = receiver("127.0.0.1:0");

        socket
            .send_to(b"audio", peer.local_addr().unwrap())
            .await
            .unwrap();

        assert_eq!(received_traffic_class(&peer, false), 0);
    }
}
