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

use jamjam::network::{EncryptionContext, FecDecoder, FecPacket, KeyPair, SequenceTracker};
use jamjam::protocol::{LatencyInfoMessage, LatencyPing, LatencyPong, Packet, HEADER_SIZE};

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

/// Decrypting attacker-supplied bytes must fail cleanly (authentication) and
/// never panic, for any sequence number and any length — including inputs
/// shorter than the GCM tag.
#[test]
fn decrypt_rejects_arbitrary_ciphertext_without_panicking() {
    let ctx = EncryptionContext::from_shared_secret(&[0x42u8; 32], true);
    let mut rng = XorShift::new(0xBADC0DE);

    for _ in 0..20_000 {
        let seq = rng.next_u64() as u32;
        let ct = rng.bytes(64);
        assert!(
            ctx.decrypt(seq, &ct).is_err(),
            "unauthenticated ciphertext must never decrypt (seq={seq}, len={})",
            ct.len()
        );
    }
}

/// A tampered or replayed-to-wrong-slot audio packet must be rejected: GCM
/// authentication binds the ciphertext, and the nonce is derived from the
/// sequence number, so flipping a bit or reusing the ciphertext under a
/// different sequence both fail. (Reinforces the encryption unit tests at the
/// level a peer actually attacks: same key context, hostile inputs.)
#[test]
fn tampered_or_resequenced_audio_is_rejected() {
    let ctx = EncryptionContext::from_shared_secret(&[0x07u8; 32], true);
    let plaintext = b"live audio frame";
    let seq = 4242u32;
    let ciphertext = ctx.encrypt(seq, plaintext).expect("encrypt");

    // Correct decrypt as a control.
    assert_eq!(ctx.decrypt(seq, &ciphertext).unwrap(), plaintext);

    // Flip each byte in turn: every single-bit... actually single-byte tamper
    // must break authentication.
    for i in 0..ciphertext.len() {
        let mut t = ciphertext.clone();
        t[i] ^= 0xFF;
        assert!(
            ctx.decrypt(seq, &t).is_err(),
            "tampering byte {i} must fail authentication"
        );
    }

    // Replay the exact ciphertext under a different sequence number: the nonce
    // no longer matches, so it must fail.
    assert!(
        ctx.decrypt(seq.wrapping_add(1), &ciphertext).is_err(),
        "a captured frame replayed under a different sequence must fail"
    );

    // Truncated ciphertext (shorter than the tag) must fail, not panic.
    assert!(ctx
        .decrypt(seq, &ciphertext[..ciphertext.len() / 2])
        .is_err());
}

/// A key-exchange message from a peer carries a raw 32-byte public key. Any
/// 32-byte value is a syntactically valid X25519 public key, but deriving a
/// shared secret from a hostile or degenerate key (all-zero, low-order) must not
/// panic — at worst it yields a shared secret we then fail to use.
#[test]
fn key_agreement_survives_hostile_public_keys() {
    let hostile_keys: [[u8; 32]; 3] = [
        [0u8; 32],    // all zero
        [0xFFu8; 32], // all ones
        {
            // Known low-order point for Curve25519.
            let mut k = [0u8; 32];
            k[0] = 1;
            k
        },
    ];

    for peer_public in hostile_keys {
        let ours = KeyPair::generate();
        let shared = ours.derive_shared_secret(&peer_public);
        // Just touching the bytes must not panic; contributory behaviour is a
        // separate concern, this test only asserts totality.
        let _ = shared.as_bytes();
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
