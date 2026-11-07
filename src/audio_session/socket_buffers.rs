//! Larger buffers on the audio socket. Every participant sends to the one socket of the session,
//! so with 16 participants it takes in about 12 000 packets per second. The default receive
//! buffer of Windows (64 KiB) holds about 20 ms of that: a receive thread kept away for longer
//! than that, by a busy machine or a power-saving core, loses packets before the app reads them.

use std::net::UdpSocket;

use socket2::SockRef;
use tracing::{info, warn};

/// What the buffers of the audio socket are asked to hold, each direction. At 750 packets per
/// second of about 400 bytes from each of 16 participants that is a little over 200 ms of what
/// they send together.
const SOCKET_BUFFER_BYTES: usize = 1 << 20;

/// Asks for larger buffers on the audio socket and says in the log what it was given. A system
/// that keeps a smaller size (Linux caps it at `net.core.rmem_max`) is not an error: the socket
/// runs with what it got.
pub fn enlarge_socket_buffers(socket: &UdpSocket) {
    let sock = SockRef::from(socket);
    let (receive_before, send_before) = (
        sock.recv_buffer_size().unwrap_or(0),
        sock.send_buffer_size().unwrap_or(0),
    );
    let asked_receive = sock.set_recv_buffer_size(SOCKET_BUFFER_BYTES);
    let asked_send = sock.set_send_buffer_size(SOCKET_BUFFER_BYTES);
    let (receive_after, send_after) = (
        sock.recv_buffer_size().unwrap_or(0),
        sock.send_buffer_size().unwrap_or(0),
    );
    match (asked_receive, asked_send) {
        (Ok(()), Ok(())) => info!(
            "Audio socket buffers: receive {receive_before} -> {receive_after} bytes, send {send_before} -> {send_after} bytes"
        ),
        (receive, send) => warn!(
            "The system did not take the audio socket buffer sizes (receive: {}, send: {}); they stay {receive_after} and {send_after} bytes",
            receive.err().map_or("ok".to_string(), |e| e.to_string()),
            send.err().map_or("ok".to_string(), |e| e.to_string()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-AUD-134
    #[test]
    fn when_the_socket_buffers_are_enlarged_then_they_are_not_smaller_than_before() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sock = SockRef::from(&socket);
        let before = (
            sock.recv_buffer_size().unwrap(),
            sock.send_buffer_size().unwrap(),
        );
        enlarge_socket_buffers(&socket);
        assert!(sock.recv_buffer_size().unwrap() >= before.0);
        assert!(sock.send_buffer_size().unwrap() >= before.1);
    }
}
