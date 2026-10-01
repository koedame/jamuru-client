//! Measures how long audio takes from `Connection::send_audio` to the receiving connection's
//! audio callback over loopback, with the packet sizes and pace of a real session. Run by hand to
//! compare a change that touches the packet path against the version before it:
//!
//! ```text
//! cargo test --release --test wire_latency_measure -- --ignored --nocapture
//! ```
//!
//! It uses only what every version of the connection offers, so the same file runs on both sides
//! of a comparison. The figures are the time spent in the app (encoding, sealing, the socket,
//! opening); the network between two machines comes on top of them.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::network::{AudioEncodingConfig, Connection};

/// 128 frames of stereo 32-bit float: the Balanced preset's packet
const FRAME: usize = 128;
const PACKETS: usize = 3000;
/// A packet every 2.67 ms, the pace of 128 frames at 48 kHz
const PACE: Duration = Duration::from_micros(2667);

fn percentile(sorted: &[u128], p: f64) -> u128 {
    sorted[((sorted.len() - 1) as f64 * p) as usize]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "a measurement, run by hand"]
async fn how_long_audio_takes_from_send_to_callback_over_loopback() {
    let encoding = || AudioEncodingConfig {
        channels: 2,
        frame_size: FRAME as u32,
        ..Default::default()
    };
    let mut sender = Connection::new("127.0.0.1:0").await.unwrap();
    let mut receiver = Connection::new("127.0.0.1:0").await.unwrap();
    sender.set_audio_encoding(encoding()).unwrap();
    receiver.set_audio_encoding(encoding()).unwrap();

    // sequence -> when it arrived
    let arrivals: Arc<Mutex<Vec<(u32, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = arrivals.clone();
    receiver.set_audio_callback(move |sequence, _, _| {
        sink.lock().unwrap().push((sequence, Instant::now()));
    });
    sender.connect(receiver.local_addr()).await.unwrap();
    receiver.connect(sender.local_addr()).await.unwrap();

    let signal: Vec<f32> = (0..FRAME * 2).map(|i| (i as f32 * 0.001).sin()).collect();

    // Until the first frame arrives (a connection that encrypts holds audio back until its keys
    // are agreed), then count from there
    let started = Instant::now();
    while arrivals.lock().unwrap().is_empty() {
        sender.send_audio(&signal, 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "no audio arrived"
        );
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    arrivals.lock().unwrap().clear();

    let mut sent: Vec<(u32, Instant)> = Vec::with_capacity(PACKETS);
    let mut next = Instant::now();
    for _ in 0..PACKETS {
        tokio::time::sleep_until(next.into()).await;
        next += PACE;
        // Sequence numbers are the connection's own; they are read back from the arrivals
        let at = Instant::now();
        sender.send_audio(&signal, 0).await.unwrap();
        sent.push((0, at));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    let arrivals = arrivals.lock().unwrap().clone();
    let first_sequence = arrivals.first().expect("audio arrived").0;
    let mut micros: Vec<u128> = arrivals
        .iter()
        .filter_map(|(sequence, at)| {
            let index = (sequence - first_sequence) as usize;
            sent.get(index)
                .map(|(_, sent_at)| at.duration_since(*sent_at).as_micros())
        })
        .collect();
    micros.sort_unstable();
    let lost = PACKETS - micros.len().min(PACKETS);
    println!(
        "send -> callback over loopback, {} packets of {} bytes, {} lost: \
         p50 {} us, p95 {} us, p99 {} us, max {} us",
        micros.len(),
        FRAME * 2 * 4,
        lost,
        percentile(&micros, 0.50),
        percentile(&micros, 0.95),
        percentile(&micros, 0.99),
        micros.last().unwrap(),
    );
}

/// What encrypting and decrypting one packet costs by itself, with no socket or scheduler in the
/// way. Only exists where the packet path encrypts.
#[test]
#[ignore = "a measurement, run by hand"]
fn how_long_sealing_and_opening_one_packet_takes() {
    use jamjam::network::{Opened, SecureLink};
    use jamjam::protocol::Packet;

    let (a, b) = (SecureLink::new(), SecureLink::new());
    let _ = a.open(b.key_exchange_packet());
    let _ = b.open(a.key_exchange_packet());

    for payload_bytes in [FRAME * 2 * 4, 2048] {
        let packet = Packet::audio(1, 0, vec![0x5a; payload_bytes]);
        const ROUNDS: u32 = 200_000;

        let started = Instant::now();
        for _ in 0..ROUNDS {
            let sealed = a.seal(packet.clone()).expect("sealed");
            match b.open(sealed) {
                Opened::Packet(opened) => assert_eq!(opened.payload.len(), payload_bytes),
                _ => panic!("the packet did not open"),
            }
        }
        let per_packet = started.elapsed() / ROUNDS;
        println!(
            "seal + open of a {payload_bytes}-byte payload: {} ns per packet",
            per_packet.as_nanos()
        );
    }
}
