//! Asks the router to forward the audio port, then reports what reaches it.
//!
//! The lab in `tests/upnp/` runs this behind a miniupnpd router: it prints the candidates the
//! app would offer a room, waits for one datagram on the audio socket, prints it, and closes
//! the forward.
//!
//! ```text
//! port_mapping_probe [--no-router] [--wait-secs N]
//! ```

use std::time::Duration;

use jamjam::network::{bind_audio_std, gather_candidates_on, port_mapping, CandidateType};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ask_router = !args.iter().any(|a| a == "--no-router");
    let wait_secs: u64 = args
        .iter()
        .position(|a| a == "--wait-secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let socket = bind_audio_std(0).expect("bind the audio socket");
    let port = socket.local_addr().unwrap().port();
    let started = std::time::Instant::now();
    let gathered = gather_candidates_on(&socket, ask_router)
        .await
        .expect("gather candidates");
    println!("PORT {port}");
    println!("GATHER_MS {}", started.elapsed().as_millis());
    for candidate in &gathered.candidates {
        let kind = match candidate.candidate_type {
            CandidateType::Host => "host",
            CandidateType::ServerReflexive => "reflexive",
        };
        println!("CANDIDATE {kind} {}", candidate.address);
    }
    match &gathered.port_mapping {
        Some(mapping) => println!("MAPPED {}", mapping.external_addr()),
        None => {
            println!("NOT_MAPPED");
            if ask_router {
                if let Err(e) =
                    port_mapping::open_with(port_mapping::Options::default(), port).await
                {
                    println!("NOT_MAPPED_BECAUSE {e}");
                }
            }
        }
    }
    println!("READY");

    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(wait_secs)))
        .unwrap();
    let mut buf = [0u8; 1500];
    match tokio::task::spawn_blocking(move || {
        socket
            .recv_from(&mut buf)
            .map(|(n, from)| (buf[..n].to_vec(), from))
    })
    .await
    .unwrap()
    {
        Ok((data, from)) => println!(
            "RECEIVED {} from {from}",
            String::from_utf8_lossy(&data).trim()
        ),
        Err(e) => println!("NOTHING_RECEIVED {e}"),
    }

    if let Some(mapping) = gathered.port_mapping {
        mapping.close().await;
        println!("CLOSED");
    }
}
