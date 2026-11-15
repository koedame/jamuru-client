//! One side of a connection attempt through the NAT lab in `tests/nat/`.
//!
//! Gathers candidates through the given STUN servers, trades them with the other side through
//! files in a shared directory (what the signaling server does in a room), and connects the way a
//! room link does. Prints what happened, one fact per line.
//!
//! ```text
//! nat_lab_peer --name a --peer b --dir /lab --stun ip:port,ip:port,ip:port
//!              [--wait-secs N] [--settle-secs N] [--audio-secs N] [--connect-at-ms EPOCH_MS]
//!              [--punch-settle-ms N] [--punch-quiet-ms N] [--punch-second-ms N] [--punch-window-ms N]
//!              [--no-restun]
//!
//! Like a room link, it asks STUN again through the audio socket while it has heard nothing, tells
//! the other side an address it was not told (a new offer in the shared directory), and starts
//! over with the other side's offer when that changes. `--no-restun` is the app as it was.
//! ```

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jamjam::network::{
    bind_audio_std, gather_candidates_using, untold, with_mappings_seen, AddressCandidate,
    CandidateType, Connection, LinkIdentity, MappingWatch, PunchSchedule, UdpMux,
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

fn publish(dir: &Path, name: &str, key: &str, candidates: &[AddressCandidate]) {
    let mut offer = format!("KEY {key}\n");
    for candidate in candidates {
        offer.push_str(&format!("CAND {}\n", candidate.address));
    }
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, offer).expect("write the offer");
    std::fs::rename(&tmp, dir.join(format!("{name}.txt"))).expect("publish the offer");
}

fn candidates_of(offer: &str) -> Vec<SocketAddr> {
    offer
        .lines()
        .filter_map(|l| l.strip_prefix("CAND "))
        .filter_map(|a| a.parse().ok())
        .collect()
}

/// Returns the other side's candidates once they are not `current` any more
async fn changed_from(path: &Path, current: &[SocketAddr]) -> Vec<SocketAddr> {
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(text) = std::fs::read_to_string(path) {
            let now = candidates_of(&text);
            if now != current {
                return now;
            }
        }
    }
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

    let mut schedule = PunchSchedule::default();
    let millis = |name: &str| {
        arg(name)
            .and_then(|v| v.parse().ok())
            .map(Duration::from_millis)
    };
    schedule.settle = millis("--punch-settle-ms").unwrap_or(schedule.settle);
    schedule.quiet = millis("--punch-quiet-ms").unwrap_or(schedule.quiet);
    schedule.second_waits = millis("--punch-second-ms").unwrap_or(schedule.second_waits);
    schedule.window = millis("--punch-window-ms").unwrap_or(schedule.window);

    let identity = LinkIdentity::generate();
    let dir = Path::new(&dir);
    let own_key = identity.public_key().to_string();
    publish(dir, &name, &own_key, &gathered);

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
    let mut candidates = candidates_of(&theirs);

    let mux = Arc::new(UdpMux::from_std(socket).expect("mux on the socket"));
    // The audio's timestamp is the time it was sent in tenths of a millisecond, and the lab's
    // containers share a clock: a packet's delay is the time it arrives minus what it says
    let delays = Arc::new(Mutex::new(Vec::<f32>::new()));
    let new_connection = || {
        let mut conn = Connection::on(&mux).expect("connection on the mux");
        let heard = delays.clone();
        conn.set_audio_callback(move |_, _, timestamp| {
            let now = tenths_of_ms();
            heard
                .lock()
                .unwrap()
                .push(now.wrapping_sub(timestamp) as f32 / 10.0);
        });
        conn.verify_peer(&identity, &key).expect("peer key");
        conn.set_punch_schedule(schedule);
        conn
    };
    let mut conn = new_connection();
    // Waits for a wall-clock moment (the lab's containers share a clock), so the two sides'
    // first packets can be set a known time apart
    if let Some(at) = arg("--connect-at-ms").and_then(|v| v.parse::<u128>().ok()) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        if at > now {
            tokio::time::sleep(Duration::from_millis((at - now) as u64)).await;
        }
    }
    println!(
        "START_MS {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let started = Instant::now();
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let restun = !std::env::args().any(|a| a == "--no-restun");
    if restun {
        // While nothing is heard: asks STUN through the audio socket now and then, and tells
        // the other side an address it was not told
        let seen = Arc::new(std::sync::RwLock::new(Vec::new()));
        let mut watch = MappingWatch::new(&mux, seen.clone())
            .with_servers(servers.iter().map(|s| s.to_string()).collect());
        let connected = connected.clone();
        let mux = mux.clone();
        let dir = dir.to_path_buf();
        let name = name.clone();
        let own_key = own_key.clone();
        let mut told = gathered.clone();
        tokio::spawn(async move {
            let _keep = mux;
            while !connected.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(500)).await;
                watch.tick(Some(started.elapsed()));
                let seen_now = seen.read().unwrap().clone();
                let told_addrs: Vec<SocketAddr> = told.iter().map(|c| c.address).collect();
                if !untold(&told_addrs, &seen_now).is_empty() {
                    told = with_mappings_seen(&told, &seen_now);
                    publish(&dir, &name, &own_key, &told);
                    for c in &told {
                        println!("TOLD {} at_ms={}", c.address, started.elapsed().as_millis());
                    }
                }
            }
        });
    }
    // The connection says it is made once it has sent: it is heard that counts here
    let hear_for = Duration::from_secs(
        arg("--hear-secs")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
    );
    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    let peer_path = dir.join(format!("{peer}.txt"));
    loop {
        let changed = changed_from(&peer_path, &candidates);
        let attempt = async {
            let result = conn.connect_with_candidates(&candidates).await;
            if result.is_ok() {
                println!("CONNECTED ms={}", started.elapsed().as_millis());
                let heard = async {
                    while conn.stats().packets_received == 0 {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                };
                match tokio::time::timeout(hear_for, heard).await {
                    Ok(()) => println!("HEARD ms={}", started.elapsed().as_millis()),
                    Err(_) => println!("NOT_HEARD"),
                }
            }
            result
        };
        tokio::select! {
            result = attempt => {
                if let Err(e) = result {
                    println!("NOT_CONNECTED {e}");
                }
                break;
            }
            now = changed => {
                println!("PEER_MOVED {:?} at_ms={}", now, started.elapsed().as_millis());
                candidates = now;
                conn = new_connection();
            }
            _ = &mut deadline => {
                println!("NOT_CONNECTED timeout");
                break;
            }
        }
    }
    connected.store(true, std::sync::atomic::Ordering::SeqCst);
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
    println!("FACTS {:?}", conn.link_facts().snapshot());
    println!("DONE");
}
