//! What a peer sends is what the receiver plays, at every frame size a preset uses.

use jamjam::audio::{
    AudioCodec, AudioPreset, CodecConfig, CodecType, PcmCodec, ReceivePath, WIRE_CHANNELS,
};
use jamjam::network::UdpTransport;
use jamjam::protocol::Packet;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES_SENT: usize = 20;

/// Stereo samples that differ from each other and from one frame to the next,
/// so a sample lost or moved anywhere shows as a mismatch.
fn signal(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|i| (i as f32 * 0.001).sin() * 0.5 + (i % 7) as f32 * 0.01)
        .collect()
}

/// Sends `FRAMES_SENT` uncompressed frames of `frame_size` over UDP, hands each
/// received packet to a receive path, and returns what it plays with what was sent.
async fn send_and_play(frame_size: u32) -> (Vec<f32>, Vec<f32>) {
    let frame_samples = frame_size as usize * WIRE_CHANNELS;
    let sent = signal(frame_samples * FRAMES_SENT);

    let sender = UdpTransport::bind("127.0.0.1:0")
        .await
        .expect("sender socket");
    let receiver = UdpTransport::bind("127.0.0.1:0")
        .await
        .expect("receiver socket");
    let mut codec = PcmCodec::new(&CodecConfig {
        codec_type: CodecType::Pcm,
        sample_rate: SAMPLE_RATE,
        channels: WIRE_CHANNELS as u16,
        frame_size,
        bitrate: 0,
    });
    let path = ReceivePath::new(CodecType::Pcm, SAMPLE_RATE, frame_size, 0).expect("receive path");

    for (sequence, frame) in sent.chunks(frame_samples).enumerate() {
        let payload = codec.encode(frame).expect("encode");
        let packet = Packet::audio(sequence as u32, sequence as u32 * frame_size, payload);
        sender
            .send_to(&packet, receiver.local_addr())
            .await
            .expect("send");
        let (packet, _) = receiver.recv_from().await.expect("receive");
        assert!(path.receive(packet.sequence, &packet.payload));
    }

    let mut played = Vec::new();
    let mut out = vec![0.0f32; frame_samples * 3];
    for _ in 0..FRAMES_SENT {
        let read = path.read_into(&mut out);
        played.extend_from_slice(&out[..read.samples]);
    }
    (sent, played)
}

async fn assert_played_as_sent(frame_size: u32) {
    let (sent, played) = send_and_play(frame_size).await;
    assert_eq!(
        played.len(),
        sent.len(),
        "samples played at frame size {frame_size}"
    );
    assert!(
        played == sent,
        "samples played differ from those sent at frame size {frame_size}"
    );
}

/// Test: A 32-frame stream is played as it was sent
#[tokio::test]
async fn test_frame_size_32_is_played_as_sent() {
    assert_played_as_sent(32).await;
}

/// Test: A 64-frame stream is played as it was sent
#[tokio::test]
async fn test_frame_size_64_is_played_as_sent() {
    assert_played_as_sent(64).await;
}

/// Test: A 128-frame stream is played as it was sent
#[tokio::test]
async fn test_frame_size_128_is_played_as_sent() {
    assert_played_as_sent(128).await;
}

/// Test: A 256-frame stream is played as it was sent
///
/// The packet is 2060 bytes (12-byte header, 2048 bytes of stereo f32), more
/// than the 2048 the socket was read into, so its last 3 samples were cut off.
#[tokio::test]
async fn test_frame_size_256_is_played_as_sent() {
    assert_played_as_sent(256).await;
}

/// Test: Every preset's frame size is covered above
#[test]
fn test_every_preset_frame_size_has_a_test() {
    for preset in AudioPreset::all() {
        assert!(
            [32, 64, 128, 256].contains(&preset.frame_size()),
            "{preset:?} plays frames of {}, which no test above covers",
            preset.frame_size()
        );
    }
}
