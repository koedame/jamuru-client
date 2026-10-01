//! Encryption of the link to one peer
//!
//! Every packet that leaves a [`SecureLink`] is encrypted with AES-256-GCM, under a
//! key both ends derive from an X25519 exchange made over the same UDP path the audio
//! takes. A link is one pair of peers: one [`SecureLink`] for each peer an app talks to.
//!
//! The exchange is a single message each way (a [`PacketType::Control`] packet holding
//! the sender's ephemeral public key), sent again every few hundred milliseconds until
//! the peer has answered with something encrypted. Both sides derive two keys, one for
//! each direction, from the shared secret and both public keys, so a packet sent to the
//! peer cannot be played back to its sender.
//!
//! A sealed packet keeps its header and carries `counter (8 bytes) | ciphertext | tag
//! (16 bytes)` as its payload. The counter is the nonce and is taken from one count for
//! the whole link, whatever the packet type and whoever sends it, so a nonce is never
//! used twice under a key. The header is authenticated too, so a packet cannot be given
//! another type, sequence number or timestamp. The receiver refuses a counter it has
//! already accepted and one that is further behind than [`REPLAY_WINDOW`].
//!
//! What this does not do is say who the peer is: the exchange is not signed, so someone
//! who can alter packets on the path while the link is being set up could sit between
//! the two ends. Listening to the audio on its way is not possible without that.
//!
//! A peer that knows nothing of this (an older app) is recognised by what it sends -
//! plain audio and pings, never a key - and the link then carries plain packets, which
//! [`SecureLink::security`] reports so the user can be told.

use std::sync::atomic::{AtomicU64, Ordering};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use parking_lot::Mutex;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::protocol::{Packet, PacketType};

/// First byte of a control packet that carries a key exchange
const KEY_EXCHANGE: u8 = 0x01;

/// Second byte of a key exchange: X25519, HKDF-SHA256, AES-256-GCM
const SUITE: u8 = 0x01;

/// Size of a key exchange payload: message, suite, public key
const KEY_EXCHANGE_LEN: usize = 2 + 32;

/// Bytes of counter at the front of a sealed payload
const COUNTER_SIZE: usize = 8;

/// Bytes of authentication tag at the end of a sealed payload
const TAG_SIZE: usize = 16;

/// How many bytes sealing adds to a payload
pub const SEAL_OVERHEAD: usize = COUNTER_SIZE + TAG_SIZE;

/// How far behind the newest counter a packet may arrive and still be accepted. A packet
/// further behind than this is too late to play, and refusing it keeps the record of what
/// has been seen small.
const REPLAY_WINDOW: u64 = 128;

/// Mixed into every key, so the keys of another protocol or version are different keys
const KEY_SALT: &[u8] = b"jamjam-audio-link-v1";

/// What a link is doing about encryption
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LinkSecurity {
    /// The keys are not agreed yet, or are but the peer has not yet shown it has them. Only
    /// keep-alives go out.
    #[default]
    Negotiating,
    /// Both ends have the keys: everything is encrypted, and only encrypted packets are
    /// accepted
    Encrypted,
    /// The peer does not encrypt (an older app), so nothing is: plain packets in both
    /// directions
    Unencrypted,
}

impl LinkSecurity {
    /// The name the screen is given: `encrypted`, `negotiating` or `unencrypted`
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkSecurity::Negotiating => "negotiating",
            LinkSecurity::Encrypted => "encrypted",
            LinkSecurity::Unencrypted => "unencrypted",
        }
    }
}

/// What became of a packet that arrived
#[derive(Debug)]
pub enum Opened {
    /// A packet to act on. If it was encrypted it has been decrypted.
    Packet(Packet),
    /// The peer's key exchange. When `answer` is set the peer may not have our key yet:
    /// send [`SecureLink::key_exchange_packet`].
    KeyExchange { answer: bool },
    /// Not acted on, and why. Anything that fails to decrypt, repeats an earlier packet or
    /// is not what the link is ready for ends up here.
    Dropped(&'static str),
}

/// The keys of an agreed link, and what has been seen of the peer
struct Keys {
    send: Aes256Gcm,
    receive: Aes256Gcm,
    /// The counter of the next packet sent
    next_counter: u64,
    peer_public: [u8; 32],
    window: ReplayWindow,
    /// Whether an encrypted packet from the peer has been accepted, which shows it has our
    /// key
    confirmed: bool,
}

enum State {
    Negotiating,
    Encrypted(Box<Keys>),
    Unencrypted,
}

/// The encryption of one link: the keys, the exchange that makes them and the packets
/// sealed and opened with them
///
/// Cheap to share (`Arc<SecureLink>`): every method takes `&self`.
pub struct SecureLink {
    secret: StaticSecret,
    public: [u8; 32],
    state: Mutex<State>,
    /// Packets turned away as forged, repeated or from the wrong state
    refused: AtomicU64,
}

impl Default for SecureLink {
    fn default() -> Self {
        Self::new()
    }
}

impl SecureLink {
    /// A link with a new key pair, not yet agreed with anyone
    pub fn new() -> Self {
        Self::with_secret(StaticSecret::random_from_rng(&mut UnwrapErr(SysRng)))
    }

    fn with_secret(secret: StaticSecret) -> Self {
        let public = PublicKey::from(&secret).to_bytes();
        Self {
            secret,
            public,
            state: Mutex::new(State::Negotiating),
            refused: AtomicU64::new(0),
        }
    }

    /// What the link is doing about encryption. It is `Encrypted` once the peer has shown it
    /// has the keys too, which is when audio may go.
    pub fn security(&self) -> LinkSecurity {
        match &*self.state.lock() {
            State::Negotiating => LinkSecurity::Negotiating,
            State::Encrypted(keys) if keys.confirmed => LinkSecurity::Encrypted,
            State::Encrypted(_) => LinkSecurity::Negotiating,
            State::Unencrypted => LinkSecurity::Unencrypted,
        }
    }

    /// How many packets were turned away as forged, repeated or out of place
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Our half of the key exchange
    pub fn key_exchange_packet(&self) -> Packet {
        let mut payload = Vec::with_capacity(KEY_EXCHANGE_LEN);
        payload.push(KEY_EXCHANGE);
        payload.push(SUITE);
        payload.extend_from_slice(&self.public);
        Packet::control(0, payload)
    }

    /// Whether our key should be sent (again): the link has no keys, or the peer has not
    /// yet shown it has ours
    pub fn wants_key_exchange(&self) -> bool {
        match &*self.state.lock() {
            State::Negotiating => true,
            State::Encrypted(keys) => !keys.confirmed,
            State::Unencrypted => false,
        }
    }

    /// Whether the keys are made on our side, whether or not the peer has shown it has them
    pub fn has_keys(&self) -> bool {
        matches!(&*self.state.lock(), State::Encrypted(_))
    }

    /// Whether it is settled that the link is encrypted (keys made) or is not (the peer cannot).
    /// Until then what the peer sends decides which it is.
    pub fn is_decided(&self) -> bool {
        !matches!(&*self.state.lock(), State::Negotiating)
    }

    /// Whether audio and everything else but keep-alives may be sent: the link has settled
    /// how it carries them, and the peer has shown it can take them (it has sent something
    /// encrypted, so it has the keys; or it cannot encrypt, and takes plain)
    pub fn can_send_media(&self) -> bool {
        self.security() != LinkSecurity::Negotiating
    }

    /// Prepares `packet` for the wire: encrypted when the link is, as it is when the peer
    /// does not encrypt, and `None` when it must not go yet. Until the peer has shown it has
    /// the keys only keep-alives go (sealed once there are keys): they hold nothing, and they
    /// are what shows the peer.
    pub fn seal(&self, mut packet: Packet) -> Option<Packet> {
        match &mut *self.state.lock() {
            State::Unencrypted => Some(packet),
            State::Negotiating => (packet.packet_type == PacketType::KeepAlive).then_some(packet),
            State::Encrypted(keys) => {
                if !keys.confirmed && packet.packet_type != PacketType::KeepAlive {
                    return None;
                }
                let counter = keys.next_counter;
                keys.next_counter = counter.checked_add(1)?;

                packet.flags.encrypted = true;
                let header = packet.header_bytes();
                let sealed = keys
                    .send
                    .encrypt(
                        &Nonce::from(nonce(counter)),
                        Payload {
                            msg: &packet.payload,
                            aad: &header,
                        },
                    )
                    .ok()?;

                let mut payload = Vec::with_capacity(COUNTER_SIZE + sealed.len());
                payload.extend_from_slice(&counter.to_be_bytes());
                payload.extend_from_slice(&sealed);
                packet.payload = payload;
                Some(packet)
            }
        }
    }

    /// Takes a packet that arrived and says what to do with it
    pub fn open(&self, packet: Packet) -> Opened {
        let opened = if packet.packet_type == PacketType::Control {
            self.receive_control(&packet)
        } else {
            self.receive(packet)
        };
        if matches!(opened, Opened::Dropped(_)) {
            self.refused.fetch_add(1, Ordering::Relaxed);
        }
        opened
    }

    fn receive(&self, packet: Packet) -> Opened {
        let mut state = self.state.lock();
        match &mut *state {
            State::Unencrypted if packet.flags.encrypted => {
                Opened::Dropped("encrypted packet on an unencrypted link")
            }
            State::Unencrypted => Opened::Packet(packet),
            State::Negotiating if packet.flags.encrypted => {
                Opened::Dropped("encrypted packet before the keys are agreed")
            }
            State::Negotiating if packet.packet_type == PacketType::KeepAlive => {
                Opened::Packet(packet)
            }
            State::Negotiating => {
                // A peer with the keys sends nothing but keep-alives plain before it has
                // agreed them with us. One that sends more plain is one that cannot
                // encrypt.
                *state = State::Unencrypted;
                Opened::Packet(packet)
            }
            State::Encrypted(keys) => match open_encrypted(keys, packet) {
                Ok(packet) => Opened::Packet(packet),
                Err(reason) => Opened::Dropped(reason),
            },
        }
    }

    fn receive_control(&self, packet: &Packet) -> Opened {
        let Some(peer_public) = parse_key_exchange(&packet.payload) else {
            // Not a message this app knows: from a newer app
            return Opened::Dropped("control message of another kind");
        };
        if peer_public == self.public {
            return Opened::Dropped("our own key exchange played back");
        }

        let mut state = self.state.lock();
        let answer = match &*state {
            State::Unencrypted => {
                return Opened::Dropped("key exchange on an unencrypted link");
            }
            State::Encrypted(keys) if keys.peer_public == peer_public => false,
            // Until the peer has shown it has our key, a key different from the one first
            // heard is as likely to be the peer's own (a stale copy of the app before it)
            // as a forgery, and ignoring it would leave the link without a peer. Once it
            // has been shown, the keys stand.
            State::Encrypted(keys) if keys.confirmed => {
                return Opened::Dropped("another key after the link was confirmed");
            }
            State::Negotiating | State::Encrypted(_) => true,
        };

        if answer {
            let shared = self.secret.diffie_hellman(&PublicKey::from(peer_public));
            if !shared.was_contributory() {
                return Opened::Dropped("key exchange with a weak key");
            }
            *state = State::Encrypted(Box::new(derive_keys(
                shared.as_bytes(),
                &self.public,
                &peer_public,
            )));
        }
        Opened::KeyExchange { answer }
    }
}

/// Decrypts `packet` under `keys`, refusing one that repeats or is too far behind
fn open_encrypted(keys: &mut Keys, mut packet: Packet) -> Result<Packet, &'static str> {
    if !packet.flags.encrypted {
        return Err("plain packet on an encrypted link");
    }
    if packet.payload.len() < SEAL_OVERHEAD {
        return Err("encrypted packet too short");
    }
    let (counter, sealed) = packet.payload.split_at(COUNTER_SIZE);
    let counter = u64::from_be_bytes(counter.try_into().expect("split at the counter's size"));
    if !keys.window.is_new(counter) {
        return Err("repeated or too old");
    }

    let header = packet.header_bytes();
    let plain = keys
        .receive
        .decrypt(
            &Nonce::from(nonce(counter)),
            Payload {
                msg: sealed,
                aad: &header,
            },
        )
        .map_err(|_| "does not authenticate")?;

    keys.window.accept(counter);
    keys.confirmed = true;
    packet.flags.encrypted = false;
    packet.payload = plain;
    Ok(packet)
}

fn parse_key_exchange(payload: &[u8]) -> Option<[u8; 32]> {
    if payload.len() != KEY_EXCHANGE_LEN || payload[0] != KEY_EXCHANGE || payload[1] != SUITE {
        return None;
    }
    payload[2..].try_into().ok()
}

/// The two keys of a link. The one end whose public key is the smaller sends under the
/// first and receives under the second; the other end does the reverse. The public keys
/// are mixed in, so a key is bound to the pair it was made for.
fn derive_keys(shared: &[u8; 32], ours: &[u8; 32], theirs: &[u8; 32]) -> Keys {
    let we_are_low = ours < theirs;
    let (low, high) = if we_are_low {
        (ours, theirs)
    } else {
        (theirs, ours)
    };
    let mut salt = Vec::with_capacity(KEY_SALT.len() + 64);
    salt.extend_from_slice(KEY_SALT);
    salt.extend_from_slice(low);
    salt.extend_from_slice(high);

    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared);
    let mut low_to_high = [0u8; 32];
    let mut high_to_low = [0u8; 32];
    hkdf.expand(b"low to high", &mut low_to_high)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    hkdf.expand(b"high to low", &mut high_to_low)
        .expect("32 bytes is a valid HKDF-SHA256 output length");

    let cipher = |key: [u8; 32]| Aes256Gcm::new(&Key::<Aes256Gcm>::from(key));
    let (send, receive) = if we_are_low {
        (cipher(low_to_high), cipher(high_to_low))
    } else {
        (cipher(high_to_low), cipher(low_to_high))
    };

    Keys {
        send,
        receive,
        next_counter: 0,
        peer_public: *theirs,
        window: ReplayWindow::default(),
        confirmed: false,
    }
}

/// The nonce of the packet with `counter`. Each direction has its own key, so a counter
/// need only be unique within one.
fn nonce(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

/// Which counters have been accepted, within [`REPLAY_WINDOW`] of the newest
#[derive(Default)]
struct ReplayWindow {
    started: bool,
    newest: u64,
    /// Bit `n` is set when the counter `newest - n` has been accepted
    seen: u128,
}

impl ReplayWindow {
    /// Whether `counter` has not been accepted and is not too far behind
    fn is_new(&self, counter: u64) -> bool {
        if !self.started || counter > self.newest {
            return true;
        }
        let behind = self.newest - counter;
        behind < REPLAY_WINDOW && self.seen & (1u128 << behind) == 0
    }

    fn accept(&mut self, counter: u64) {
        if !self.started {
            *self = Self {
                started: true,
                newest: counter,
                seen: 1,
            };
        } else if counter > self.newest {
            let ahead = counter - self.newest;
            self.seen = if ahead >= REPLAY_WINDOW {
                1
            } else {
                (self.seen << ahead) | 1
            };
            self.newest = counter;
        } else {
            self.seen |= 1u128 << (self.newest - counter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_with(secret_byte: u8) -> SecureLink {
        SecureLink::with_secret(StaticSecret::from([secret_byte; 32]))
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Two links that have exchanged keys, each told the other's, and not yet heard anything
    /// of each other that is encrypted
    fn exchanged() -> (SecureLink, SecureLink) {
        let (a, b) = (SecureLink::new(), SecureLink::new());
        assert!(matches!(
            a.open(b.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            b.open(a.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        (a, b)
    }

    /// Two links that have exchanged keys and heard each other's keep-alive, so each knows the
    /// other has the keys and audio may go either way
    fn agreed() -> (SecureLink, SecureLink) {
        let (a, b) = exchanged();
        let from_a = a.seal(Packet::keep_alive(0)).unwrap();
        let from_b = b.seal(Packet::keep_alive(0)).unwrap();
        assert!(opened(&b, from_a).is_some());
        assert!(opened(&a, from_b).is_some());
        (a, b)
    }

    fn audio(sequence: u32, payload: &[u8]) -> Packet {
        Packet::audio(sequence, 7, payload.to_vec())
    }

    fn opened(link: &SecureLink, packet: Packet) -> Option<Packet> {
        match link.open(packet) {
            Opened::Packet(packet) => Some(packet),
            _ => None,
        }
    }

    /// Verifies: REQ-SEC-001
    #[test]
    fn when_the_keys_are_agreed_audio_sealed_by_one_end_opens_at_the_other() {
        let (a, b) = agreed();

        let sealed = a.seal(audio(5, b"some audio")).unwrap();
        let received = opened(&b, sealed).unwrap();

        assert_eq!(received.payload, b"some audio");
        assert_eq!(received.sequence, 5);
        assert_eq!(received.timestamp, 7);
        assert!(!received.flags.encrypted);
    }

    #[test]
    fn when_the_keys_are_agreed_each_direction_works() {
        let (a, b) = agreed();

        let from_a = opened(&b, a.seal(audio(1, b"to b")).unwrap()).unwrap();
        let from_b = opened(&a, b.seal(audio(1, b"to a")).unwrap()).unwrap();

        assert_eq!(from_a.payload, b"to b");
        assert_eq!(from_b.payload, b"to a");
    }

    #[test]
    fn a_sealed_packet_carries_no_plain_payload_and_says_it_is_encrypted() {
        let (a, _b) = agreed();
        let payload = b"a recognisable run of bytes, 0123456789abcdef".to_vec();

        let sealed = a.seal(audio(1, &payload)).unwrap();

        assert!(sealed.flags.encrypted);
        assert_eq!(sealed.payload.len(), payload.len() + SEAL_OVERHEAD);
        let wire = sealed.to_bytes();
        assert!(!wire.windows(payload.len()).any(|w| w == payload.as_slice()));
        assert!(!wire.windows(8).any(|w| w == &payload[..8]));
    }

    #[test]
    fn the_same_payload_sealed_twice_looks_different_on_the_wire() {
        let (a, _b) = agreed();

        let first = a.seal(audio(1, b"same")).unwrap();
        let second = a.seal(audio(1, b"same")).unwrap();

        assert_ne!(first.payload, second.payload);
    }

    /// Verifies: REQ-SEC-003
    #[test]
    fn packets_of_every_type_sharing_a_sequence_number_are_sealed_under_different_nonces() {
        let (a, _b) = agreed();
        let mut counters = std::collections::HashSet::new();

        for sequence in [0, 0, 1, 1] {
            for packet in [
                Packet::audio(sequence, 0, b"x".to_vec()),
                Packet::fec(sequence, 0, b"x".to_vec()),
                Packet::keep_alive(sequence),
            ] {
                let sealed = a.seal(packet).unwrap();
                assert!(counters.insert(sealed.payload[..COUNTER_SIZE].to_vec()));
            }
        }
    }

    /// Verifies: REQ-SEC-002
    #[test]
    fn a_packet_changed_on_the_way_is_refused() {
        let (a, b) = agreed();
        let sealed = a.seal(audio(1, b"some audio")).unwrap();

        for index in 0..sealed.payload.len() {
            let mut changed = sealed.clone();
            changed.payload[index] ^= 0x01;
            assert!(opened(&b, changed).is_none(), "byte {index}");
        }
        let mut shorter = sealed.clone();
        shorter.payload.pop();
        assert!(opened(&b, shorter).is_none());
        assert!(opened(&b, sealed).is_some(), "the unchanged packet opens");
    }

    /// Verifies: REQ-SEC-002
    #[test]
    fn a_packet_given_another_header_is_refused() {
        let (a, b) = agreed();
        let sealed = a.seal(audio(1, b"some audio")).unwrap();

        let mut other_sequence = sealed.clone();
        other_sequence.sequence = 2;
        let mut other_timestamp = sealed.clone();
        other_timestamp.timestamp += 1;
        let mut other_type = sealed.clone();
        other_type.packet_type = PacketType::Fec;

        assert!(opened(&b, other_sequence).is_none());
        assert!(opened(&b, other_timestamp).is_none());
        assert!(opened(&b, other_type).is_none());
        assert!(opened(&b, sealed).is_some());
    }

    /// Verifies: REQ-SEC-002
    #[test]
    fn a_packet_that_arrives_twice_is_accepted_once() {
        let (a, b) = agreed();
        let sealed = a.seal(audio(1, b"some audio")).unwrap();

        assert!(opened(&b, sealed.clone()).is_some());
        assert!(opened(&b, sealed).is_none());
    }

    #[test]
    fn packets_that_arrive_out_of_order_are_accepted() {
        let (a, b) = agreed();
        let sealed: Vec<Packet> = (0..10).map(|i| a.seal(audio(i, b"x")).unwrap()).collect();

        for index in [3, 1, 9, 0, 2, 8, 5, 4, 7, 6] {
            assert!(opened(&b, sealed[index].clone()).is_some(), "{index}");
        }
    }

    /// Verifies: REQ-SEC-002
    #[test]
    fn a_packet_further_behind_than_the_window_is_refused() {
        let (a, b) = agreed();
        let first = a.seal(audio(0, b"first")).unwrap();
        for sequence in 1..=REPLAY_WINDOW as u32 {
            let packet = a.seal(audio(sequence, b"x")).unwrap();
            assert!(opened(&b, packet).is_some());
        }

        assert!(opened(&b, first).is_none());
    }

    /// Verifies: REQ-SEC-003
    #[test]
    fn a_packet_sent_to_the_peer_cannot_be_played_back_to_its_sender() {
        let (a, _b) = agreed();
        let sealed = a.seal(audio(1, b"some audio")).unwrap();

        assert!(opened(&a, sealed).is_none());
    }

    /// Verifies: REQ-SEC-003
    #[test]
    fn a_packet_sealed_for_another_peer_is_refused() {
        let (a, _b) = agreed();
        let (_c, d) = agreed();
        let sealed = a.seal(audio(1, b"some audio")).unwrap();

        assert!(opened(&d, sealed).is_none());
    }

    /// Verifies: REQ-SEC-005
    #[test]
    fn before_the_keys_are_agreed_only_keep_alives_may_be_sent() {
        let link = SecureLink::new();

        assert!(link.seal(Packet::keep_alive(0)).is_some());
        assert!(link.seal(audio(0, b"x")).is_none());
        assert!(link.seal(Packet::fec(0, 0, b"x".to_vec())).is_none());
        assert_eq!(link.security(), LinkSecurity::Negotiating);
        assert!(!link.can_send_media());
    }

    /// Verifies: REQ-SEC-002
    /// Verifies: REQ-SEC-005
    #[test]
    fn until_the_peer_has_shown_it_has_the_keys_only_keep_alives_may_be_sent_and_they_are_sealed() {
        let (a, b) = exchanged();

        assert!(!a.can_send_media());
        assert_eq!(a.security(), LinkSecurity::Negotiating);
        assert!(a.seal(audio(0, b"x")).is_none());
        let keep_alive = a.seal(Packet::keep_alive(0)).expect("a keep-alive may go");
        assert!(keep_alive.flags.encrypted);

        assert!(opened(&b, keep_alive).is_some());
        assert!(b.can_send_media(), "b has now heard that a has the keys");
        assert!(!a.can_send_media(), "a has not heard anything of b yet");
    }

    #[test]
    fn when_the_keys_are_agreed_a_plain_packet_is_refused_whatever_it_is() {
        let (_a, b) = agreed();

        assert!(opened(&b, audio(1, b"plain")).is_none());
        assert!(opened(&b, Packet::keep_alive(1)).is_none());
        assert_eq!(b.security(), LinkSecurity::Encrypted);
    }

    #[test]
    fn before_the_keys_are_agreed_a_peers_keep_alive_is_taken_and_the_link_keeps_negotiating() {
        let link = SecureLink::new();

        assert!(opened(&link, Packet::keep_alive(1)).is_some());
        assert_eq!(link.security(), LinkSecurity::Negotiating);
    }

    /// Verifies: REQ-SEC-004
    #[test]
    fn a_peer_that_sends_plain_audio_before_any_key_cannot_encrypt_and_the_link_goes_plain() {
        let link = SecureLink::new();

        let received = opened(&link, audio(1, b"plain"));

        assert_eq!(received.unwrap().payload, b"plain");
        assert_eq!(link.security(), LinkSecurity::Unencrypted);
        assert!(link.seal(audio(2, b"out")).unwrap().payload == b"out");
        assert!(!link.wants_key_exchange());
    }

    #[test]
    fn on_a_plain_link_a_key_exchange_and_an_encrypted_packet_are_refused() {
        let link = SecureLink::new();
        opened(&link, audio(1, b"plain"));
        let (a, _b) = agreed();

        assert!(matches!(
            link.open(a.key_exchange_packet()),
            Opened::Dropped(_)
        ));
        let sealed = a.seal(audio(1, b"x")).unwrap();
        assert!(opened(&link, sealed).is_none());
        assert_eq!(link.security(), LinkSecurity::Unencrypted);
    }

    #[test]
    fn the_peers_key_is_answered_once_and_repeats_of_it_are_not() {
        let (a, b) = (SecureLink::new(), SecureLink::new());

        assert!(matches!(
            a.open(b.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            a.open(b.key_exchange_packet()),
            Opened::KeyExchange { answer: false }
        ));
    }

    #[test]
    fn a_link_asks_for_its_key_to_be_sent_until_the_peer_has_shown_it_has_it() {
        let (a, b) = (SecureLink::new(), SecureLink::new());
        assert!(a.wants_key_exchange());

        a.open(b.key_exchange_packet());
        assert!(a.wants_key_exchange(), "keys agreed, peer not heard yet");

        b.open(a.key_exchange_packet());
        opened(&a, b.seal(Packet::keep_alive(1)).unwrap()).unwrap();
        assert!(!a.wants_key_exchange());
    }

    /// Verifies: REQ-SEC-003
    #[test]
    fn our_own_key_played_back_to_us_is_refused() {
        let link = SecureLink::new();

        assert!(matches!(
            link.open(link.key_exchange_packet()),
            Opened::Dropped(_)
        ));
        assert_eq!(link.security(), LinkSecurity::Negotiating);
    }

    /// Verifies: REQ-SEC-002
    #[test]
    fn a_key_that_gives_no_secret_is_refused() {
        let link = SecureLink::new();
        let mut payload = vec![KEY_EXCHANGE, SUITE];
        payload.extend_from_slice(&[0u8; 32]);

        assert!(matches!(
            link.open(Packet::control(0, payload)),
            Opened::Dropped(_)
        ));
        assert_eq!(link.security(), LinkSecurity::Negotiating);
    }

    #[test]
    fn a_control_packet_that_is_not_a_key_exchange_is_ignored() {
        let link = SecureLink::new();

        for payload in [
            vec![],
            vec![KEY_EXCHANGE],
            vec![9, 9, 9],
            vec![KEY_EXCHANGE, 2, 0],
        ] {
            assert!(matches!(
                link.open(Packet::control(0, payload)),
                Opened::Dropped(_)
            ));
        }
        let mut with_another_suite = vec![KEY_EXCHANGE, SUITE + 1];
        with_another_suite.extend_from_slice(&[7u8; 32]);
        assert!(matches!(
            link.open(Packet::control(0, with_another_suite)),
            Opened::Dropped(_)
        ));
        assert_eq!(link.security(), LinkSecurity::Negotiating);
    }

    #[test]
    fn a_different_key_before_the_peer_is_confirmed_replaces_the_first() {
        let (a, first) = (SecureLink::new(), SecureLink::new());
        let second = SecureLink::new();
        a.open(first.key_exchange_packet());

        assert!(matches!(
            a.open(second.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        second.open(a.key_exchange_packet());

        assert!(opened(&a, second.seal(Packet::keep_alive(1)).unwrap()).is_some());
    }

    #[test]
    fn a_different_key_after_the_peer_is_confirmed_is_refused() {
        let (a, b) = agreed();
        opened(&a, b.seal(audio(1, b"x")).unwrap()).unwrap();
        let intruder = SecureLink::new();

        assert!(matches!(
            a.open(intruder.key_exchange_packet()),
            Opened::Dropped(_)
        ));
        assert!(opened(&a, b.seal(audio(2, b"x")).unwrap()).is_some());
    }

    #[test]
    fn a_link_counts_the_packets_it_turns_away() {
        let (a, b) = agreed();
        let sealed = a.seal(audio(1, b"x")).unwrap();
        opened(&b, sealed.clone());
        let before = b.refused();

        opened(&b, sealed);
        opened(&b, audio(2, b"plain"));

        assert_eq!(b.refused(), before + 2);
    }

    /// Sealed bytes for fixed keys, so a dependency upgrade cannot silently change the
    /// wire format. The expected bytes were made independently, with Python's `cryptography`
    /// (X25519, HKDF-SHA256 over the salt described in [`derive_keys`], AES-256-GCM).
    /// Verifies: REQ-SEC-003
    #[test]
    fn the_sealed_bytes_for_fixed_keys_match_the_known_answer() {
        let (a, b) = (link_with(0x11), link_with(0x22));
        a.open(b.key_exchange_packet());
        b.open(a.key_exchange_packet());
        // The first packet of each is the keep-alive that shows the other it has the keys
        let hello_a = a.seal(Packet::keep_alive(0)).unwrap();
        let hello_b = b.seal(Packet::keep_alive(0)).unwrap();
        assert!(opened(&b, hello_a).is_some());
        assert!(opened(&a, hello_b).is_some());

        let from_a = a.seal(audio(12345, b"Hello, encrypted world!")).unwrap();
        let from_b = b.seal(audio(12345, b"Hello, encrypted world!")).unwrap();

        assert_eq!(
            hex(&from_a.to_bytes()),
            "0101000030390000000700010000000000000001fe048d6a64edd154e96d6ea7e2f79484984b5ac3e92473ab82c512d4969f540f429c7afd0857e8"
        );
        assert_eq!(
            hex(&from_b.to_bytes()),
            "01010000303900000007000100000000000000019fd57190c6c29b9e0229087e1cde4b4ecab5d0180ef4d896b32ce91f8842ab7207e6cfa0491ae6"
        );
        assert!(opened(&b, from_a).is_some());
        assert!(opened(&a, from_b).is_some());
    }

    #[test]
    fn the_replay_window_forgets_what_is_further_behind_than_it_reaches() {
        let mut window = ReplayWindow::default();
        assert!(window.is_new(5));
        window.accept(5);
        assert!(!window.is_new(5));
        assert!(window.is_new(4));
        window.accept(4);
        assert!(!window.is_new(4));

        window.accept(5 + REPLAY_WINDOW);
        assert!(!window.is_new(5), "5 is now a full window behind");
        assert!(window.is_new(6));
        assert!(!window.is_new(5 + REPLAY_WINDOW));

        window.accept(u64::MAX / 2);
        assert!(!window.is_new(6));
        assert!(window.is_new(u64::MAX / 2 + 1));
    }
}
