//! Adversarial input tests: a hostile peer or relay sends malformed, random,
//! tampered, or replayed bytes on the wire. None of it may crash, hang, read
//! out of bounds, or coerce us into allocating attacker-chosen memory.
//!
//! In a peer-to-peer session the bytes on the socket come straight from another
//! participant, so every parser that runs on received data is directly reachable
//! by an attacker. These tests exercise each wire parser through the public API
//! the way an attacker would: with garbage. The property under test is
//! "decoding untrusted bytes is total" — it returns `None`/`Err` or a bounded
//! value, and never panics.

use jamjam::network::{
    FecDecoder, FecPacket, LinkSecurity, Opened, SecureLink, SequenceTracker, SEAL_OVERHEAD,
};
use jamjam::protocol::{
    LatencyInfoMessage, LatencyPing, LatencyPong, Packet, PacketType, HEADER_SIZE,
};

/// Small deterministic PRNG so the fuzz corpus is reproducible across runs and
/// machines (a security regression must fail the same way everywhere, and we do
/// not want a dev-dependency just to make noise).
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        // Avoid the zero fixed point.
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = (self.next_u64() as usize) % (max_len + 1);
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

/// Every wire parser must survive an arbitrary byte string. A panic here means a
/// peer can crash the app by sending a single packet.
#[test]
fn wire_parsers_never_panic_on_random_bytes() {
    let mut rng = XorShift::new(0xC0FFEE);

    for _ in 0..50_000 {
        // Payloads up to a couple of MTUs: covers short, header-boundary, and
        // over-long inputs. UDP would cap real datagrams, but the parser must
        // not rely on that.
        let data = rng.bytes(3000);

        // Top-level packet frame.
        let _ = Packet::from_bytes(&data);

        // The payload parsers are dispatched on packet type after the frame is
        // decoded, so feed them the same untrusted bytes directly.
        let _ = FecPacket::from_bytes(&data);
        let _ = LatencyPing::from_bytes(&data);
        let _ = LatencyPong::from_bytes(&data);
        let _ = LatencyInfoMessage::from_bytes(&data);
    }
}

/// Length-boundary inputs are the classic place for off-by-one out-of-bounds
/// reads. Walk every length from empty to just past each header.
#[test]
fn wire_parsers_survive_every_length_boundary() {
    for len in 0..64usize {
        let zeros = vec![0u8; len];
        let ones = vec![0xFFu8; len];
        for data in [&zeros, &ones] {
            let _ = Packet::from_bytes(data);
            let _ = FecPacket::from_bytes(data);
            let _ = LatencyPing::from_bytes(data);
            let _ = LatencyPong::from_bytes(data);
            let _ = LatencyInfoMessage::from_bytes(data);
        }
    }
}

/// A FEC packet claims to carry up to 255 sub-packets. A hostile peer sets the
/// count high but sends a short buffer, hoping the parser trusts the count and
/// reads past the end. It must reject the frame instead.
#[test]
fn fec_packet_rejects_count_larger_than_buffer() {
    // group_sequence (4) + packet_count=255 (1) + reserved (1), then nothing.
    let mut data = vec![0u8; 6];
    data[4] = 255;
    assert!(
        FecPacket::from_bytes(&data).is_none(),
        "a packet_count that exceeds the buffer must be rejected, not trusted"
    );

    // Enough bytes for the header and lengths table, but the lengths point past
    // what the fec_data can satisfy. Parsing must still succeed without reading
    // out of bounds; recovery (below) must stay bounded.
    let mut data = vec![0u8; 6 + 2 * 4];
    data[4] = 4; // four sub-packets
    let parsed = FecPacket::from_bytes(&data).expect("well-formed header parses");
    assert_eq!(parsed.packet_count, 4);
}

/// `LatencyInfo` has a variable-length codec string prefixed by a byte length.
/// A hostile peer sets the length to 255 but truncates the buffer.
#[test]
fn latency_info_rejects_codec_len_past_buffer() {
    let mut data = vec![0u8; LatencyInfoMessage::MIN_SIZE];
    // Byte 28 is codec_len; claim 255 while sending none of it.
    data[28] = 255;
    assert!(
        LatencyInfoMessage::from_bytes(&data).is_none(),
        "a codec length past the buffer must be rejected"
    );
}

/// FEC recovery XORs received sub-packets against the redundancy block and
/// truncates to the missing packet's declared length. A hostile peer inflates
/// that declared length hoping to grow the output or read past the redundancy
/// block. The recovered data must never exceed the redundancy block it came
/// from, regardless of the declared length.
#[test]
fn fec_recovery_output_is_bounded_by_redundancy_block() {
    // Two sub-packets in the group; we will "receive" index 0 and let recovery
    // reconstruct index 1. Declare index 1's length as u16::MAX.
    let fec_data = vec![0xABu8; 8];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&7u32.to_be_bytes()); // group_sequence
    bytes.push(2); // packet_count
    bytes.push(0); // reserved
    bytes.extend_from_slice(&4u16.to_be_bytes()); // length[0]
    bytes.extend_from_slice(&u16::MAX.to_be_bytes()); // length[1] (hostile)
    bytes.extend_from_slice(&fec_data);

    let fec = FecPacket::from_bytes(&bytes).expect("header is well-formed");

    let mut decoder = FecDecoder::new();
    assert!(decoder
        .add_packet(7, 0, &[0x11, 0x22, 0x33, 0x44])
        .is_none());
    let recovered = decoder
        .add_fec(fec)
        .expect("one missing packet plus FEC recovers");

    assert!(
        recovered.data.len() <= fec_data.len(),
        "recovered length {} must not exceed the {}-byte redundancy block",
        recovered.data.len(),
        fec_data.len()
    );
}

/// Feeding the FEC decoder a stream of hostile groups must not grow memory
/// without bound: the decoder caps the number of live groups. We can only
/// observe this indirectly (no panic, no runaway), but the loop would OOM if the
/// cap were missing.
#[test]
fn fec_decoder_does_not_grow_without_bound() {
    let mut decoder = FecDecoder::new();
    for group in 0..100_000u32 {
        // Never complete a group, so nothing is ever recovered or cleared by
        // success — only the internal cap can bound memory here.
        let _ = decoder.add_packet(group, 0, &[0u8; 16]);
    }
}

/// Two links that have agreed keys, as two peers have once they are connected
fn agreed_links() -> (SecureLink, SecureLink) {
    let (a, b) = (SecureLink::new(), SecureLink::new());
    let _ = a.open(b.key_exchange_packet());
    let _ = b.open(a.key_exchange_packet());
    // Each shows the other it has the keys with the first thing it sends
    let from_a = a.seal(Packet::keep_alive(0)).expect("a keep-alive goes");
    let from_b = b.seal(Packet::keep_alive(0)).expect("a keep-alive goes");
    assert!(matches!(b.open(from_a), Opened::Packet(_)));
    assert!(matches!(a.open(from_b), Opened::Packet(_)));
    assert_eq!(a.security(), LinkSecurity::Encrypted);
    assert_eq!(b.security(), LinkSecurity::Encrypted);
    (a, b)
}

/// Opening attacker-supplied bytes marked as encrypted must fail cleanly
/// (authentication) and never panic, for any counter and any length - including
/// inputs shorter than the counter and the GCM tag.
#[test]
fn opening_arbitrary_ciphertext_never_succeeds_and_never_panics() {
    let (_, link) = agreed_links();
    let mut rng = XorShift::new(0xBADC0DE);

    for _ in 0..20_000 {
        let mut packet = Packet::audio(rng.next_u64() as u32, rng.next_u64() as u32, rng.bytes(64));
        packet.flags.encrypted = true;
        assert!(
            matches!(link.open(packet), Opened::Dropped(_)),
            "unauthenticated ciphertext must never open"
        );
    }
}

/// A tampered or replayed audio packet must be rejected: GCM authentication
/// binds the ciphertext and the header, so flipping a bit, giving the packet
/// another sequence number or timestamp, or sending it twice all fail.
#[test]
fn tampered_resequenced_or_repeated_audio_is_rejected() {
    let (a, b) = agreed_links();
    let sealed = a
        .seal(Packet::audio(4242, 99, b"live audio frame".to_vec()))
        .expect("seal");
    assert_eq!(
        sealed.payload.len(),
        b"live audio frame".len() + SEAL_OVERHEAD
    );

    // Every single-byte tamper of the payload must break authentication.
    for i in 0..sealed.payload.len() {
        let mut t = sealed.clone();
        t.payload[i] ^= 0xFF;
        assert!(
            matches!(b.open(t), Opened::Dropped(_)),
            "tampering byte {i} must fail authentication"
        );
    }

    // The same bytes under another sequence number, timestamp or type.
    let mut resequenced = sealed.clone();
    resequenced.sequence += 1;
    assert!(matches!(b.open(resequenced), Opened::Dropped(_)));
    let mut retimed = sealed.clone();
    retimed.timestamp += 1;
    assert!(matches!(b.open(retimed), Opened::Dropped(_)));
    let mut retyped = sealed.clone();
    retyped.packet_type = PacketType::Fec;
    assert!(matches!(b.open(retyped), Opened::Dropped(_)));

    // Truncated ciphertext (shorter than the tag) must fail, not panic.
    let mut truncated = sealed.clone();
    truncated.payload.truncate(sealed.payload.len() / 2);
    assert!(matches!(b.open(truncated), Opened::Dropped(_)));

    // The genuine packet opens once; a replay of it does not.
    assert!(matches!(b.open(sealed.clone()), Opened::Packet(_)));
    assert!(matches!(b.open(sealed), Opened::Dropped(_)));
}

/// A key-exchange message from a peer carries a raw 32-byte public key. Any
/// 32-byte value is a syntactically valid X25519 public key, but a hostile or
/// degenerate one (all-zero, low-order) must not panic, and must not give a
/// secret an eavesdropper could compute: the link keeps negotiating.
#[test]
fn key_agreement_survives_hostile_public_keys() {
    let hostile_keys: [[u8; 32]; 3] = [
        [0u8; 32], // all zero
        {
            // Known low-order point for Curve25519.
            let mut k = [0u8; 32];
            k[0] = 1;
            k
        },
        {
            // The other low-order point of order 8.
            let mut k = [0u8; 32];
            k[0] = 0xe0;
            k[1] = 0xeb;
            k[2] = 0x7a;
            k[3] = 0x7c;
            k[4] = 0x3b;
            k[5] = 0x41;
            k[6] = 0xb8;
            k[7] = 0xae;
            k[8] = 0x16;
            k[9] = 0x56;
            k[10] = 0xe3;
            k[11] = 0xfa;
            k[12] = 0xf1;
            k[13] = 0x9f;
            k[14] = 0xc4;
            k[15] = 0x6a;
            k[16] = 0xda;
            k[17] = 0x09;
            k[18] = 0x8d;
            k[19] = 0xeb;
            k[20] = 0x9c;
            k[21] = 0x32;
            k[22] = 0xb1;
            k[23] = 0xfd;
            k[24] = 0x86;
            k[25] = 0x62;
            k[26] = 0x05;
            k[27] = 0x16;
            k[28] = 0x5f;
            k[29] = 0x49;
            k[30] = 0xb8;
            k
        },
    ];

    for peer_public in hostile_keys {
        let link = SecureLink::new();
        let mut payload = vec![0x01, 0x01];
        payload.extend_from_slice(&peer_public);
        let _ = link.open(Packet::control(0, payload));
        assert_eq!(link.security(), LinkSecurity::Negotiating);
    }

    // Garbage of every length in the key-exchange packet type must not panic either.
    let mut rng = XorShift::new(0xFEED);
    for _ in 0..5_000 {
        let link = SecureLink::new();
        let _ = link.open(Packet::control(0, rng.bytes(80)));
    }
}

/// An attacker who captured earlier packets replays them. The tracker exposes a
/// replay-detection primitive (`was_received`) that lets the receive path
/// recognise a sequence it has already seen, and replaying a seen sequence must
/// not be misreported as recovering a lost packet.
#[test]
fn sequence_tracker_exposes_replay_detection() {
    let mut tracker = SequenceTracker::new();

    tracker.record(1000);
    assert!(
        tracker.was_received(1000),
        "a freshly recorded sequence reports as received"
    );

    // Replay the same sequence: it is a duplicate, so no new loss recovery may
    // be reported, and it still reads as already-received.
    let losses = tracker.record(1000);
    assert!(
        losses.is_empty(),
        "replaying a seen sequence must not be reported as recovering losses"
    );
    assert!(
        tracker.was_received(1000),
        "a replayed sequence is still recognised as already received"
    );
}

/// A valid frame header followed by an arbitrary payload must decode into a
/// packet whose payload is exactly the trailing bytes — never more. This guards
/// against a length field being trusted over the actual buffer size.
#[test]
fn valid_header_with_arbitrary_payload_is_bounded() {
    let mut rng = XorShift::new(0x5EED);
    for _ in 0..5_000 {
        let payload = rng.bytes(2000);
        let packet = Packet::audio(
            rng.next_u64() as u32,
            rng.next_u64() as u32,
            payload.clone(),
        );
        let bytes = packet.to_bytes();
        let decoded = Packet::from_bytes(&bytes).expect("our own frame round-trips");
        assert_eq!(decoded.payload, payload);
        assert_eq!(bytes.len(), HEADER_SIZE + payload.len());
    }
}
