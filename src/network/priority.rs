//! Marking the audio packets as time-critical (ADR-110)
//!
//! An IP header has a six-bit priority field (DSCP; the upper bits of IPv4's ToS byte and
//! IPv6's Traffic Class). A home Wi-Fi access point, and the Wi-Fi adapter of the sending PC,
//! read it to pick one of the four WMM queues (RFC 8325): with audio marked Expedited
//! Forwarding (46) it is sent ahead of a video stream or a download on the same Wi-Fi. Most
//! of the internet clears the field, so what this buys is on the first hop, inside the home.
//!
//! The mark is set once, on the session's audio socket, and every packet then carries it:
//!
//! - Linux: `IP_TOS` / `IPV6_TCLASS` to EF, and `SO_PRIORITY` 6 for the local queue.
//! - macOS: not marked. `IP_TOS` / `IPV6_TCLASS` leave the header unmarked on a dual-stack
//!   socket, and `SO_NET_SERVICE_TYPE` (voice), the system's own way, made the sent audio drop
//!   out in a measured session (ADR-110).
//! - Windows: `IP_TOS` / `IPV6_TCLASS`. qWAVE, the system's QoS service, cannot do this job: it
//!   marks a flow per destination, refuses an IPv6 socket that sends to an IPv4 address (the
//!   audio socket is dual-stack, ADR-093), and gives voice 56 (CS7), not 46.
//!
//! The mark is a request: a network may clear it or, rarely, drop what is marked. That is
//! why it is a setting (`voice_priority`).

#[cfg(any(target_os = "linux", windows))]
use socket2::SockRef;
use tracing::{info, warn};

/// EF (46) in the upper six bits of the byte
#[cfg(any(target_os = "linux", windows))]
const EF_TRAFFIC_CLASS: u32 = 46 << 2;

/// Marks what `socket` sends as voice, and logs how. Returns what was used, or `None` where the
/// system does not take it (the reason is logged): audio goes unmarked then.
pub fn mark_voice(socket: &std::net::UdpSocket) -> Option<&'static str> {
    match mark(socket) {
        Ok(how) => {
            info!("Audio packets are marked as voice ({how})");
            Some(how)
        }
        Err(reason) => {
            warn!("Audio packets are not marked as voice: {reason}");
            None
        }
    }
}

/// `IP_TOS` for what goes out with an IPv4 header (an IPv6 socket sends to an IPv4 peer with
/// one, too) and `IPV6_TCLASS` for IPv6. Marked if either took.
#[cfg(any(target_os = "linux", windows))]
fn mark_traffic_class(socket: &std::net::UdpSocket) -> Result<(), String> {
    let local = socket.local_addr().map_err(|e| e.to_string())?;
    let v4 = SockRef::from(socket).set_tos_v4(EF_TRAFFIC_CLASS);
    let v6 = if local.is_ipv6() {
        set_tclass_v6(socket)
    } else {
        Ok(())
    };
    let marked = if local.is_ipv6() {
        v4.is_ok() || v6.is_ok()
    } else {
        v4.is_ok()
    };
    if marked {
        return Ok(());
    }
    let reason = v4.err().or(v6.err()).map(|e| e.to_string());
    Err(reason.unwrap_or_default())
}

#[cfg(target_os = "linux")]
fn set_tclass_v6(socket: &std::net::UdpSocket) -> std::io::Result<()> {
    SockRef::from(socket).set_tclass_v6(EF_TRAFFIC_CLASS)
}

#[cfg(windows)]
fn set_tclass_v6(socket: &std::net::UdpSocket) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows::Win32::Networking::WinSock::{
        setsockopt, IPPROTO_IPV6, IPV6_TCLASS, SOCKET, SOCKET_ERROR,
    };

    let value = EF_TRAFFIC_CLASS as i32;
    // SAFETY: a valid socket, and the bytes of an int
    let result = unsafe {
        setsockopt(
            SOCKET(socket.as_raw_socket() as usize),
            IPPROTO_IPV6.0,
            IPV6_TCLASS,
            Some(&value.to_ne_bytes()),
        )
    };
    if result == SOCKET_ERROR {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn mark(socket: &std::net::UdpSocket) -> Result<&'static str, String> {
    mark_traffic_class(socket)?;
    // The queue of this machine's own network stack and Wi-Fi driver (0-6 need no privilege)
    Ok(if SockRef::from(socket).set_priority(6).is_ok() {
        "IP_TOS, IPV6_TCLASS and SO_PRIORITY"
    } else {
        "IP_TOS and IPV6_TCLASS"
    })
}

#[cfg(windows)]
fn mark(socket: &std::net::UdpSocket) -> Result<&'static str, String> {
    mark_traffic_class(socket).map(|()| "IP_TOS and IPV6_TCLASS")
}

#[cfg(target_os = "macos")]
fn mark(_socket: &std::net::UdpSocket) -> Result<&'static str, String> {
    Err("macOS does not carry the mark in the packets, and its voice service type made the sent audio drop out".to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn mark(_socket: &std::net::UdpSocket) -> Result<&'static str, String> {
    Err("this system has no way to mark them".to_string())
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

    /// Verifies: REQ-CON-160
    #[test]
    fn when_the_audio_socket_is_marked_an_ipv4_peer_reads_expedited_forwarding() {
        let socket = crate::network::bind_audio_std(0).unwrap();
        assert!(mark_voice(&socket).is_some());
        let peer = receiver("127.0.0.1:0");

        socket
            .send_to(b"audio", peer.local_addr().unwrap())
            .unwrap();

        assert_eq!(received_traffic_class(&peer, false), EF_TRAFFIC_CLASS as u8);
    }

    /// Verifies: REQ-CON-160
    #[test]
    fn when_the_audio_socket_is_marked_an_ipv6_peer_reads_expedited_forwarding() {
        if std::net::UdpSocket::bind("[::1]:0").is_err() {
            return;
        }
        let socket = crate::network::bind_audio_std(0).unwrap();
        assert!(mark_voice(&socket).is_some());
        let peer = receiver("[::1]:0");

        socket
            .send_to(b"audio", peer.local_addr().unwrap())
            .unwrap();

        assert_eq!(received_traffic_class(&peer, true), EF_TRAFFIC_CLASS as u8);
    }

    /// Verifies: REQ-CON-160
    #[test]
    fn when_the_audio_socket_is_not_marked_a_peer_reads_no_priority() {
        let socket = crate::network::bind_audio_std(0).unwrap();
        let peer = receiver("127.0.0.1:0");

        socket
            .send_to(b"audio", peer.local_addr().unwrap())
            .unwrap();

        assert_eq!(received_traffic_class(&peer, false), 0);
    }
}
