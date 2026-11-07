//! One side of a connection attempt through the NAT lab in `tests/nat/`.
//!
//! Gathers candidates through the given STUN servers, trades them with the other side through
//! files in a shared directory (what the signaling server does in a room), and connects the way a
//! room link does. Prints what happened, one fact per line.
//!
//! ```text
//! nat_lab_peer --name a --peer b --dir /lab --stun ip:port,ip:port,ip:port
//!              [--wait-secs N] [--settle-secs N] [--audio-secs N]
//! ```

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::network::{
    bind_audio_std, gather_candidates_using, CandidateType, Connection, LinkIdentity,
};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn tenths_of_ms() -> u32 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    (since.as_micros() / 100) as u32
}

async fn read_when_there(path: &Path, wait: Duration) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < wait {
        if let Ok(text) = std::fs::read_to_string(path) {
            return Some(text);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

#[tokio::main]
async fn main() {
    let name = arg("--name").expect("--name");
    let peer = arg("--peer").expect("--peer");
    let dir = arg("--dir").expect("--dir");
    let stun = arg("--stun").expect("--stun");
    let wait = Duration::from_secs(
        arg("--wait-secs")
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
    );
    let settle = Duration::from_secs(
        arg("--settle-secs")
            .and_then(|v| v.parse().ok())
            .unwrap_or(15),
    );
    let audio_secs: u64 = arg("--audio-secs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let servers: Vec<&str> = stun.split(',').collect();

    let gather_started = Instant::now();
    let socket = bind_audio_std(0).expect("bind the audio socket");
    let asking = Arc::new(
        tokio::net::UdpSocket::from_std(socket.try_clone().expect("clone the socket"))
            .expect("socket for STUN"),
    );
    let gathered = gather_candidates_using(&asking, &servers).await;
    drop(asking);
    println!("GATHER_MS {}", gather_started.elapsed().as_millis());
    for candidate in &gathered {
        let kind = match candidate.candidate_type {
            CandidateType::Host => "host",
            CandidateType::ServerReflexive => "reflexive",
        };
        println!("CANDIDATE {kind} {}", candidate.address);
    }

    let identity = LinkIdentity::generate();
    let mut offer = format!("KEY {}\n", identity.public_key());
    for candidate in &gathered {
        offer.push_str(&format!("CAND {}\n", candidate.address));
    }
    let dir = Path::new(&dir);
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, offer).expect("write the offer");
    std::fs::rename(&tmp, dir.join(format!("{name}.txt"))).expect("publish the offer");

    let Some(theirs) = read_when_there(&dir.join(format!("{peer}.txt")), wait).await else {
        println!("NO_OFFER_FROM_PEER");
        println!("DONE");
        return;
    };
    let key = theirs
        .lines()
        .find_map(|l| l.strip_prefix("KEY "))
        .expect("the peer's key")
        .to_string();
    let candidates: Vec<SocketAddr> = theirs
        .lines()
        .filter_map(|l| l.strip_prefix("CAND "))
        .filter_map(|a| a.parse().ok())
        .collect();

    let mut conn = Connection::from_std(socket).expect("connection on the socket");
    // The audio's timestamp is the time it was sent in tenths of a millisecond, and the lab's
    // containers share a clock: a packet's delay is the time it arrives minus what it says
    let delays = Arc::new(Mutex::new(Vec::<f32>::new()));
    let heard = delays.clone();
    conn.set_audio_callback(move |_, _, timestamp| {
        let now = tenths_of_ms();
        heard
            .lock()
            .unwrap()
            .push(now.wrapping_sub(timestamp) as f32 / 10.0);
    });
    conn.verify_peer(&identity, &key).expect("peer key");
    let started = Instant::now();
    match tokio::time::timeout(wait, conn.connect_with_candidates(&candidates)).await {
        Ok(Ok(())) => println!("CONNECTED ms={}", started.elapsed().as_millis()),
        Ok(Err(e)) => println!("NOT_CONNECTED {e}"),
        Err(_) => println!("NOT_CONNECTED timeout"),
    }
    // The route can still move after the first packet (a guessed port found later)
    let mut last = conn.remote_addr();
    println!("ROUTE {last}");
    let settle_until = Instant::now() + settle;
    while Instant::now() < settle_until {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = conn.remote_addr();
        if now != last {
            println!("ROUTE {now} at_ms={}", started.elapsed().as_millis());
            last = now;
        }
    }
    if audio_secs > 0 {
        // 2.5 ms of 48 kHz audio every 2.5 ms, for `audio_secs`
        let frame = vec![0.25f32; 120];
        let mut sent = 0u32;
        let mut tick = tokio::time::interval(Duration::from_micros(2500));
        let until = Instant::now() + Duration::from_secs(audio_secs);
        while Instant::now() < until {
            tick.tick().await;
            if conn.send_audio(&frame, tenths_of_ms()).await.is_ok() {
                sent += 1;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut delays = delays.lock().unwrap().clone();
        delays.sort_by(f32::total_cmp);
        let at = |p: f32| {
            delays
                .get(((delays.len() as f32 * p) as usize).min(delays.len().saturating_sub(1)))
                .copied()
                .unwrap_or(f32::NAN)
        };
        let stats = conn.stats();
        println!(
            "AUDIO sent={sent} received={} delay_ms p50={:.2} p95={:.2} max={:.2} rtt_ms={:?} jitter_ms={:.2} loss={:.4}",
            delays.len(),
            at(0.5),
            at(0.95),
            at(1.0),
            stats.rtt_ms,
            stats.jitter_ms,
            stats.packet_loss_rate
        );
    }
    let stats = conn.stats();
    println!(
        "STATS sent={} received={}",
        stats.packets_sent, stats.packets_received
    );
    println!("DONE");
}
