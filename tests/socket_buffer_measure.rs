//! Measures what the size of the audio socket's receive buffer is worth: how many packets a
//! receive thread that is kept away for a while loses, with the system's default buffer and with
//! the one `audio_session::socket_buffers` asks for. The stream is that of 16 participants at 64-sample
//! frames (750 packets per second each, about 420 bytes), sent to the socket over the loopback.
//! Run by hand, on each system the app runs on:
//!
//! ```text
//! cargo test --release --test socket_buffer_measure -- --ignored --nocapture
//! ```

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use jamjam::audio_session::socket_buffers::enlarge_socket_buffers;

const PARTICIPANTS: u32 = 16;
const PACKETS_PER_SECOND_EACH: u32 = 750;
const PACKET_BYTES: usize = 420;
/// How long the stream runs for each row
const RUN: Duration = Duration::from_secs(3);
/// How long the receive thread is away each time, in ms
const AWAY_MS: [u64; 6] = [5, 10, 20, 40, 80, 160];

/// Packets lost out of those sent, when the receiver is away for `away` every 500 ms
fn lost(enlarged: bool, away: Duration) -> (u64, u64) {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    if enlarged {
        enlarge_socket_buffers(&receiver);
    }
    receiver.set_nonblocking(true).unwrap();
    let target = receiver.local_addr().unwrap();

    let sender = std::thread::spawn(move || {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let rate = u64::from(PARTICIPANTS * PACKETS_PER_SECOND_EACH);
        let started = Instant::now();
        let mut sent = 0u64;
        let mut packet = [0u8; PACKET_BYTES];
        while started.elapsed() < RUN {
            // Everything that is due by now goes out in one go, as a loop that runs late sends
            let due = (started.elapsed().as_secs_f64() * rate as f64) as u64;
            while sent < due {
                packet[..8].copy_from_slice(&sent.to_le_bytes());
                socket.send_to(&packet, target).unwrap();
                sent += 1;
            }
            std::hint::spin_loop();
        }
        sent
    });

    let mut received = 0u64;
    let mut buffer = [0u8; 2048];
    let started = Instant::now();
    let mut next_away = started + Duration::from_millis(500);
    while started.elapsed() < RUN + Duration::from_millis(50) {
        if Instant::now() >= next_away {
            std::thread::sleep(away);
            next_away = Instant::now() + Duration::from_millis(500);
        }
        while receiver.recv_from(&mut buffer).is_ok() {
            received += 1;
        }
    }
    let sent = sender.join().unwrap();
    while receiver.recv_from(&mut buffer).is_ok() {
        received += 1;
    }
    (sent, received)
}

#[test]
#[ignore = "a measurement to run by hand"]
fn measure_packets_lost_while_the_receive_thread_is_away() {
    println!(
        "{} participants, {} packets/s in all, {} bytes each, away every 500 ms",
        PARTICIPANTS,
        PARTICIPANTS * PACKETS_PER_SECOND_EACH,
        PACKET_BYTES
    );
    let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
    let size = || socket2::SockRef::from(&probe).recv_buffer_size().unwrap();
    let default_size = size();
    enlarge_socket_buffers(&probe);
    println!(
        "receive buffer: default {default_size} bytes, enlarged {} bytes",
        size()
    );
    println!("away_ms  default_lost  enlarged_lost  (of sent)");
    for away_ms in AWAY_MS {
        let away = Duration::from_millis(away_ms);
        let (sent_default, got_default) = lost(false, away);
        let (sent_enlarged, got_enlarged) = lost(true, away);
        println!(
            "{away_ms:>7}  {:>12}  {:>13}  ({sent_default} / {sent_enlarged})",
            sent_default - got_default,
            sent_enlarged - got_enlarged
        );
    }
}
