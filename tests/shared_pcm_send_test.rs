//! One frame going to several links is encoded once, and each link sends what it would have
//! sent had it encoded the frame itself (REQ-AUD-135)

mod common;

use jamjam::audio::CodecType;
use jamjam::network::{AudioEncodingConfig, Connection, SharedPcm};
use jamjam::protocol::{Packet, PacketType};

/// 300 stereo frames do not fit one packet (256 do), so a frame goes as two
const FRAME: usize = 300;

/// What a frame with a loud peak and a quiet tail sounds like: every sample different, three of
/// them past 0 dBFS
fn signal() -> Vec<f32> {
    let mut samples: Vec<f32> = (0..FRAME * 2)
        .map(|i| ((i as f32) * 0.037).sin() * 0.4 + (i as f32 + 0.5) / 4001.0 - 0.1)
        .collect();
    samples[10] = 1.5;
    samples[11] = -2.0;
    samples[500] = f32::NAN;
    samples
}

fn encoding(codec_type: CodecType, channels: u16) -> AudioEncodingConfig {
    AudioEncodingConfig {
        codec_type,
        channels,
        frame_size: FRAME as u32,
        fec_group_size: None,
        ..Default::default()
    }
}

/// A link to a peer that has agreed keys with it
async fn link(encoding: AudioEncodingConfig) -> (Connection, common::Peer) {
    let peer = common::Peer::bind();
    let mut conn = Connection::new("127.0.0.1:0").await.expect("socket");
    common::trust_peer(&mut conn, &peer);
    conn.set_audio_encoding(encoding).expect("encoding");
    conn.connect(peer.local_addr()).await.expect("connect");
    peer.agree_keys_with(conn.local_addr());
    common::wait_until_encrypted(&[&conn]).await;
    (conn, peer)
}

/// The audio packets the peer is sent: (sequence, timestamp, payload)
fn heard(peer: &common::Peer, count: usize) -> Vec<(u32, u32, Vec<u8>)> {
    let mut audio = Vec::new();
    while audio.len() < count {
        let (packet, _): (Packet, _) = peer.recv_packet();
        if packet.packet_type == PacketType::Audio {
            audio.push((packet.sequence, packet.timestamp, packet.payload));
        }
    }
    audio
}

/// Verifies: REQ-AUD-135
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_a_frame_is_shared_the_peer_gets_the_packets_its_own_encoding_would_send() {
    let (own, own_peer) = link(encoding(CodecType::Pcm, 2)).await;
    let (shared_link, shared_peer) = link(encoding(CodecType::Pcm, 2)).await;
    let mut shared = SharedPcm::new(2);

    for frame in 0..3u32 {
        let timestamp = frame * FRAME as u32;
        own.send_audio(&signal(), timestamp).await.expect("own");
        shared.next_frame();
        shared_link
            .send_audio_shared(&signal(), timestamp, &mut shared)
            .await
            .expect("shared");
    }

    let own_packets = heard(&own_peer, 6);
    let shared_packets = heard(&shared_peer, 6);
    assert_eq!(
        shared_packets, own_packets,
        "sequence, timestamp and bytes of every packet are the same"
    );
    assert_eq!(
        own_packets.iter().map(|p| p.0).collect::<Vec<_>>(),
        (0..6).collect::<Vec<_>>(),
        "a frame that does not fit one packet went as two, numbered without gaps"
    );
}

/// Verifies: REQ-AUD-135
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_a_frame_goes_to_many_links_each_gets_the_same_bytes_under_its_own_numbers() {
    let mut links = Vec::new();
    for _ in 0..3 {
        links.push(link(encoding(CodecType::Pcm, 2)).await);
    }
    let mut shared = SharedPcm::new(2);

    for frame in 0..2u32 {
        shared.next_frame();
        for (conn, _) in &links {
            conn.send_audio_shared(&signal(), frame * FRAME as u32, &mut shared)
                .await
                .expect("shared");
        }
    }

    let first = heard(&links[0].1, 4);
    for (_, peer) in &links[1..] {
        assert_eq!(heard(peer, 4), first, "every link sent the same packets");
    }
    assert_eq!(
        first.iter().map(|p| p.0).collect::<Vec<_>>(),
        vec![0, 1, 2, 3],
        "and numbered them itself"
    );
}

/// Verifies: REQ-AUD-135
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_a_frame_is_shared_each_link_counts_the_samples_that_went_past_0_dbfs() {
    let (a, _a_peer) = link(encoding(CodecType::Pcm, 2)).await;
    let (b, _b_peer) = link(encoding(CodecType::Pcm, 2)).await;
    let mut shared = SharedPcm::new(2);

    shared.next_frame();
    for conn in [&a, &b] {
        conn.send_audio_shared(&signal(), 0, &mut shared)
            .await
            .expect("shared");
    }

    assert_eq!(a.stats().samples_clipped, 3, "1.5, -2.0 and NaN");
    assert_eq!(
        b.stats().samples_clipped,
        3,
        "the same, on the link that reused the bytes"
    );
}

/// Verifies: REQ-AUD-135
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_a_link_cannot_send_yet_it_takes_no_sequence_number() {
    let peer = common::Peer::bind();
    let mut conn = Connection::new("127.0.0.1:0").await.expect("socket");
    common::trust_peer(&mut conn, &peer);
    conn.set_audio_encoding(encoding(CodecType::Pcm, 2))
        .expect("encoding");
    conn.connect(peer.local_addr()).await.expect("connect");
    let mut shared = SharedPcm::new(2);

    // The keys are not agreed: nothing goes, and nothing is numbered
    for _ in 0..3 {
        shared.next_frame();
        conn.send_audio_shared(&signal(), 0, &mut shared)
            .await
            .expect("sending before the keys are agreed is not an error");
    }
    assert_eq!(conn.stats().packets_sent, 0);
    assert_eq!(conn.stats().samples_clipped, 0, "nothing was sent to count");

    peer.agree_keys_with(conn.local_addr());
    common::wait_until_encrypted(&[&conn]).await;
    shared.next_frame();
    conn.send_audio_shared(&signal(), 0, &mut shared)
        .await
        .expect("shared");

    assert_eq!(
        heard(&peer, 2).iter().map(|p| p.0).collect::<Vec<_>>(),
        vec![0, 1]
    );
}

/// Verifies: REQ-AUD-135
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_a_link_does_not_carry_pcm_of_the_shared_channels_it_encodes_its_own() {
    let (mono, mono_peer) = link(encoding(CodecType::Pcm, 1)).await;
    let (stereo, stereo_peer) = link(encoding(CodecType::Pcm, 2)).await;
    let (reference, reference_peer) = link(encoding(CodecType::Pcm, 1)).await;
    let mut shared = SharedPcm::new(2);
    let mono_signal: Vec<f32> = signal()[..FRAME].to_vec();

    shared.next_frame();
    stereo
        .send_audio_shared(&signal(), 0, &mut shared)
        .await
        .expect("stereo");
    mono.send_audio_shared(&mono_signal, 0, &mut shared)
        .await
        .expect("mono");
    reference
        .send_audio(&mono_signal, 0)
        .await
        .expect("reference");

    let _ = heard(&stereo_peer, 2);
    assert_eq!(
        heard(&mono_peer, 1),
        heard(&reference_peer, 1),
        "the mono link sent what it would have without sharing"
    );
}
