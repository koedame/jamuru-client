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
//! Who the peer is comes from the signaling server, not from the path. Each participant
//! of a room makes a [`LinkIdentity`] (an Ed25519 key pair) when it enters, tells the
//! server the public half, and the server hands it to the others with the rest of the
//! participant's information. A link made [`SecureLink::for_peer`] with that key signs its
//! half of the exchange (the ephemeral key, the peer's identity and its own) and takes only
//! a half signed the same way, so someone who can alter packets on the path cannot sit
//! between the two ends: they cannot sign a key of their own as the peer. The signaling
//! connection is `wss`, so the identities reach the ends unaltered; the server that hands
//! them out is trusted.
//!
//! A link with no peer key to check (a peer whose app predates this, or a direct
//! connection that has no server) exchanges unsigned and does not say who the peer is:
//! [`SecureLink::checks_peer`] tells the two apart. A link that does check the peer never
//! falls back to plain or to an unsigned exchange, whatever arrives.
//!
//! A server that talks to many apps from one socket cannot
//! tell which participant an address is, so they cannot name the app in what they sign. A
//! link made [`SecureLink::answering`] signs its half for the ephemeral key of the half it
//! answers instead, which an app that checks takes as it takes one signed for its identity:
//! a signature from the key the server gave for the peer, over the app's own fresh key and
//! the peer's, is something no one else can make. That link does not check the app (it does
//! not know who it is), and answers a half that is not signed with a half that is not.
//!
//! A peer that knows nothing of encryption (an older app) is recognised by what it sends -
//! plain audio and pings, never a key - and the link then carries plain packets, which
//! [`SecureLink::security`] reports so the user can be told.

use std::sync::atomic::{AtomicU64, Ordering};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use data_encoding::BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use parking_lot::Mutex;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::TryRng;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::protocol::{Packet, PacketType};

/// First byte of a control packet that carries a key exchange
const KEY_EXCHANGE: u8 = 0x01;

/// Second byte of a key exchange: X25519, HKDF-SHA256, AES-256-GCM
const SUITE: u8 = 0x01;

/// Second byte of a signed key exchange: as [`SUITE`], with the ephemeral key signed by the
/// sender's [`LinkIdentity`]
const SIGNED_SUITE: u8 = 0x02;

/// Size of a key exchange payload: message, suite, public key
const KEY_EXCHANGE_LEN: usize = 2 + 32;

/// Bytes of Ed25519 signature at the end of a signed key exchange
const SIGNATURE_LEN: usize = 64;

/// Mixed into every signed text, so a signature made here is a signature of nothing else
const SIGNATURE_DOMAIN: &[u8] = b"jamjam-link-key-exchange-v1:";

/// As [`SIGNATURE_DOMAIN`], for a half signed for the ephemeral key it answers
const ANSWER_SIGNATURE_DOMAIN: &[u8] = b"jamjam-link-key-exchange-answer-v1:";

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

/// The key pair a participant signs its key exchanges with for as long as it stays in a
/// room. It is made when the participant enters and told to the server (see
/// [`LinkIdentity::public_key`]), which hands the public half to the others in the room: it
/// is how they know a key exchange is from that participant. It is not the device identity,
/// which no other participant ever learns.
#[derive(Clone)]
pub struct LinkIdentity {
    signing_key: SigningKey,
}

impl LinkIdentity {
    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        SysRng
            .try_fill_bytes(&mut secret)
            .expect("operating-system randomness is unavailable");
        Self {
            signing_key: SigningKey::from_bytes(&secret),
        }
    }

    /// The public key as the signaling messages carry it: base64 of the 32 bytes
    pub fn public_key(&self) -> String {
        BASE64.encode(self.signing_key.verifying_key().as_bytes())
    }
}

/// Deliberately omits the secret key so an accidental `{:?}` can't leak it.
impl std::fmt::Debug for LinkIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkIdentity")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

/// What a link needs to sign its half of the exchange and to check the peer's
struct PeerCheck {
    ours: SigningKey,
    theirs: VerifyingKey,
    /// Our ephemeral key, which a peer that does not know who we are signs for
    ephemeral: [u8; 32],
    /// Our key exchange payload, signed once: it is sent again every few hundred
    /// milliseconds until the peer has answered
    payload: Vec<u8>,
}

/// What a signature of a key exchange covers: who sent it, who it is for and the ephemeral
/// key. Binding the recipient stops a half meant for one participant from being passed to
/// another.
fn signed_text(sender: &[u8; 32], recipient: &[u8; 32], ephemeral: &[u8; 32]) -> Vec<u8> {
    text_of(SIGNATURE_DOMAIN, sender, recipient, ephemeral)
}

/// What a signature covers when the sender does not know who the recipient is: the ephemeral
/// key of the half it answers stands for it. That key is made for the one link, so the
/// signature cannot be used for any other.
fn answer_signed_text(sender: &[u8; 32], answered: &[u8; 32], ephemeral: &[u8; 32]) -> Vec<u8> {
    text_of(ANSWER_SIGNATURE_DOMAIN, sender, answered, ephemeral)
}

fn text_of(domain: &[u8], sender: &[u8; 32], to: &[u8; 32], ephemeral: &[u8; 32]) -> Vec<u8> {
    let mut text = Vec::with_capacity(domain.len() + 96);
    text.extend_from_slice(domain);
    text.extend_from_slice(sender);
    text.extend_from_slice(to);
    text.extend_from_slice(ephemeral);
    text
}

fn signed_payload(ours: &SigningKey, text: &[u8], ephemeral: &[u8; 32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(KEY_EXCHANGE_LEN + SIGNATURE_LEN);
    payload.push(KEY_EXCHANGE);
    payload.push(SIGNED_SUITE);
    payload.extend_from_slice(ephemeral);
    payload.extend_from_slice(&ours.sign(text).to_bytes());
    payload
}

impl PeerCheck {
    fn new(ours: &LinkIdentity, theirs: VerifyingKey, ephemeral: &[u8; 32]) -> Self {
        let ours = ours.signing_key.clone();
        let text = signed_text(
            ours.verifying_key().as_bytes(),
            theirs.as_bytes(),
            ephemeral,
        );
        let payload = signed_payload(&ours, &text, ephemeral);
        Self {
            ours,
            theirs,
            ephemeral: *ephemeral,
            payload,
        }
    }

    /// The peer's ephemeral key, if `payload` is a key exchange the peer signed for us: for
    /// our identity, or for our ephemeral key when the peer could not tell who we are
    fn open(&self, payload: &[u8]) -> Option<[u8; 32]> {
        if payload.len() != KEY_EXCHANGE_LEN + SIGNATURE_LEN
            || payload[0] != KEY_EXCHANGE
            || payload[1] != SIGNED_SUITE
        {
            return None;
        }
        let ephemeral: [u8; 32] = payload[2..KEY_EXCHANGE_LEN].try_into().ok()?;
        let signature = Signature::from_slice(&payload[KEY_EXCHANGE_LEN..]).ok()?;
        let for_our_identity = signed_text(
            self.theirs.as_bytes(),
            self.ours.verifying_key().as_bytes(),
            &ephemeral,
        );
        let for_our_key = answer_signed_text(self.theirs.as_bytes(), &self.ephemeral, &ephemeral);
        (self.theirs.verify(&for_our_identity, &signature).is_ok()
            || self.theirs.verify(&for_our_key, &signature).is_ok())
        .then_some(ephemeral)
    }
}

/// What a link that answers any app needs: who we are, and the half to send once an app's
/// key is known (it is made from that key)
struct Answering {
    ours: SigningKey,
    answer: Mutex<Option<Vec<u8>>>,
}

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
    /// Set when the peer's identity is known, which is when its half of the exchange has to
    /// be signed by it
    peer: Option<PeerCheck>,
    /// Set for a link that signs for whichever app it is talking to
    answering: Option<Answering>,
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
    /// A link with a new key pair, not yet agreed with anyone, that does not check who the
    /// peer is
    pub fn new() -> Self {
        Self::with_secret(StaticSecret::random_from_rng(&mut UnwrapErr(SysRng)), None)
    }

    /// A link to the participant whose [`LinkIdentity::public_key`] the server gave as
    /// `peer_key`, that signs our half of the exchange with `ours` and takes only a half the
    /// peer signed. When either is missing - the app has no identity (a direct connection),
    /// or the peer told none because its app predates this - or `peer_key` is not a key, it
    /// is a link as [`SecureLink::new`] makes and [`SecureLink::checks_peer`] is `false`.
    pub fn for_peer(ours: Option<&LinkIdentity>, peer_key: Option<&str>) -> Self {
        let secret = StaticSecret::random_from_rng(&mut UnwrapErr(SysRng));
        let check = ours.zip(peer_key).and_then(|(ours, peer_key)| {
            let theirs = parse_link_key(peer_key)?;
            Some(PeerCheck::new(
                ours,
                theirs,
                PublicKey::from(&secret).as_bytes(),
            ))
        });
        Self::with_secret(secret, check)
    }

    /// A link for a server that talks to many apps from one socket and cannot tell which
    /// participant an address is. `ours` is the key it
    /// tells the room as its [`LinkIdentity::public_key`]. It does not check who the app is.
    /// An app whose half is signed gets a half signed by `ours` for the app's ephemeral key, so
    /// an app that checks it by the key the server gave can tell it from someone on the path;
    /// an app whose half is not signed gets one that is not.
    pub fn answering(ours: &LinkIdentity) -> Self {
        let mut link =
            Self::with_secret(StaticSecret::random_from_rng(&mut UnwrapErr(SysRng)), None);
        link.answering = Some(Answering {
            ours: ours.signing_key.clone(),
            answer: Mutex::new(None),
        });
        link
    }

    fn with_secret(secret: StaticSecret, peer: Option<PeerCheck>) -> Self {
        let public = PublicKey::from(&secret).to_bytes();
        Self {
            secret,
            public,
            peer,
            answering: None,
            state: Mutex::new(State::Negotiating),
            refused: AtomicU64::new(0),
        }
    }

    /// Whether the keys of this link are bound to the peer's identity from the server. When
    /// they are not, the link is encrypted but anyone who can alter packets on the path while
    /// it is being set up could be the peer.
    pub fn checks_peer(&self) -> bool {
        self.peer.is_some()
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
        if let Some(check) = &self.peer {
            return Packet::control(0, check.payload.clone());
        }
        if let Some(payload) = self
            .answering
            .as_ref()
            .and_then(|a| a.answer.lock().clone())
        {
            return Packet::control(0, payload);
        }
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
            // A peer whose identity the server gave us is an app that encrypts, so plain
            // audio is not that peer
            State::Negotiating if self.peer.is_some() => {
                Opened::Dropped("plain packet from a peer that encrypts")
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
        let (peer_public, signed) = match (&self.peer, &self.answering) {
            (Some(check), _) => match check.open(&packet.payload) {
                Some(peer_public) => (peer_public, true),
                None => return Opened::Dropped("key exchange not signed by the peer"),
            },
            // Who the app is is not known, so a signature is not checked, but how the half
            // came tells how to answer it
            (None, Some(_)) => match parse_any_key_exchange(&packet.payload) {
                Some(half) => half,
                None => return Opened::Dropped("control message of another kind"),
            },
            (None, None) => match parse_key_exchange(&packet.payload) {
                Some(peer_public) => (peer_public, false),
                // Not a message this app knows: from a newer app
                None => return Opened::Dropped("control message of another kind"),
            },
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
            if let Some(answering) = &self.answering {
                *answering.answer.lock() = signed.then(|| {
                    let text = answer_signed_text(
                        answering.ours.verifying_key().as_bytes(),
                        &peer_public,
                        &self.public,
                    );
                    signed_payload(&answering.ours, &text, &self.public)
                });
            }
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

fn parse_link_key(text: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = BASE64.decode(text.as_bytes()).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// The ephemeral key in a key exchange, signed or not, and whether it is signed. The
/// signature is not looked at.
fn parse_any_key_exchange(payload: &[u8]) -> Option<([u8; 32], bool)> {
    let signed = match payload {
        [KEY_EXCHANGE, SUITE, ..] if payload.len() == KEY_EXCHANGE_LEN => false,
        [KEY_EXCHANGE, SIGNED_SUITE, ..] if payload.len() == KEY_EXCHANGE_LEN + SIGNATURE_LEN => {
            true
        }
        _ => return None,
    };
    Some((payload[2..KEY_EXCHANGE_LEN].try_into().ok()?, signed))
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
        SecureLink::with_secret(StaticSecret::from([secret_byte; 32]), None)
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

    /// The identities of two participants, and the links each would make to the other
    fn verified_links() -> (LinkIdentity, LinkIdentity, SecureLink, SecureLink) {
        let (ia, ib) = (LinkIdentity::generate(), LinkIdentity::generate());
        let a = SecureLink::for_peer(Some(&ia), Some(&ib.public_key()));
        let b = SecureLink::for_peer(Some(&ib), Some(&ia.public_key()));
        (ia, ib, a, b)
    }

    /// Verifies: REQ-SEC-007
    #[test]
    fn when_both_ends_know_the_other_by_its_key_they_agree_keys_and_audio_goes() {
        let (_, _, a, b) = verified_links();
        assert!(a.checks_peer() && b.checks_peer());

        assert!(matches!(
            a.open(b.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            b.open(a.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        let from_a = a.seal(Packet::keep_alive(0)).unwrap();
        let from_b = b.seal(Packet::keep_alive(0)).unwrap();
        assert!(opened(&b, from_a).is_some());
        assert!(opened(&a, from_b).is_some());

        let sealed = a.seal(audio(5, b"some audio")).unwrap();
        assert_eq!(opened(&b, sealed).unwrap().payload, b"some audio");
        assert_eq!(a.security(), LinkSecurity::Encrypted);
    }

    /// Verifies: REQ-SEC-007
    #[test]
    fn when_a_key_exchange_is_signed_by_someone_else_it_is_not_taken() {
        let (ia, _, a, _) = verified_links();
        // Someone on the path makes a key of their own and signs it with an identity of theirs,
        // for a
        let stranger = LinkIdentity::generate();
        let forged = SecureLink::for_peer(Some(&stranger), Some(&ia.public_key()));

        assert!(matches!(
            a.open(forged.key_exchange_packet()),
            Opened::Dropped("key exchange not signed by the peer")
        ));
        assert_eq!(a.security(), LinkSecurity::Negotiating);
        assert!(!a.has_keys());
    }

    /// Verifies: REQ-SEC-007
    #[test]
    fn when_the_key_in_a_signed_key_exchange_is_replaced_it_is_not_taken() {
        let (_, _, a, b) = verified_links();
        let mut packet = b.key_exchange_packet();
        // The signature stays, the ephemeral key it covers is the on-path attacker's
        packet.payload[2..34].copy_from_slice(&SecureLink::new().public);

        assert!(matches!(a.open(packet), Opened::Dropped(_)));
        assert!(!a.has_keys());
    }

    /// Verifies: REQ-SEC-007
    #[test]
    fn when_a_peer_that_must_sign_sends_an_unsigned_key_exchange_it_is_not_taken() {
        let (_, _, a, _) = verified_links();

        assert!(matches!(
            a.open(SecureLink::new().key_exchange_packet()),
            Opened::Dropped("key exchange not signed by the peer")
        ));
        assert!(!a.has_keys());
    }

    /// A half signed for one participant is of no use against another
    ///
    /// Verifies: REQ-SEC-007
    #[test]
    fn when_a_key_exchange_meant_for_another_participant_is_passed_on_it_is_not_taken() {
        let (ia, ib) = (LinkIdentity::generate(), LinkIdentity::generate());
        let ic = LinkIdentity::generate();
        // b signs for a. c, who also takes b to be b, is handed it
        let b_for_a = SecureLink::for_peer(Some(&ib), Some(&ia.public_key()));
        let c = SecureLink::for_peer(Some(&ic), Some(&ib.public_key()));

        assert!(matches!(
            c.open(b_for_a.key_exchange_packet()),
            Opened::Dropped("key exchange not signed by the peer")
        ));
    }

    /// Verifies: REQ-SEC-007
    #[test]
    fn when_a_peer_that_must_sign_is_sent_plain_audio_the_link_does_not_go_plain() {
        let (_, _, a, _) = verified_links();

        assert!(matches!(
            a.open(audio(1, b"plain")),
            Opened::Dropped("plain packet from a peer that encrypts")
        ));
        assert_eq!(a.security(), LinkSecurity::Negotiating);
        assert!(!a.is_decided());
    }

    /// Verifies: REQ-SEC-008
    #[test]
    fn when_the_peer_told_no_key_the_link_agrees_keys_unsigned_and_says_it_does_not_check() {
        let ours = LinkIdentity::generate();
        let a = SecureLink::for_peer(Some(&ours), None);
        let b = SecureLink::new();
        assert!(!a.checks_peer() && !b.checks_peer());

        assert!(matches!(
            a.open(b.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(a.has_keys());
    }

    /// Verifies: REQ-SEC-008
    #[test]
    fn when_the_peer_key_is_not_a_key_the_link_does_not_check_the_peer() {
        let ours = LinkIdentity::generate();
        for not_a_key in ["", "not base64!", "AAAA", &BASE64.encode(&[7u8; 31])] {
            let link = SecureLink::for_peer(Some(&ours), Some(not_a_key));
            assert!(!link.checks_peer(), "{not_a_key:?}");
        }
        // And with no identity of our own there is nothing to sign with
        let link = SecureLink::for_peer(None, Some(&ours.public_key()));
        assert!(!link.checks_peer());
    }

    /// An app that checks the server by the key the server gave, and a server that answers any
    /// app, with the key that server told the room
    fn app_and_answering_server() -> (LinkIdentity, LinkIdentity, SecureLink, SecureLink) {
        let (app, server) = (LinkIdentity::generate(), LinkIdentity::generate());
        let a = SecureLink::for_peer(Some(&app), Some(&server.public_key()));
        let s = SecureLink::answering(&server);
        (app, server, a, s)
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn when_an_app_that_checks_talks_to_a_server_that_answers_any_app_they_agree_keys_and_audio_goes(
    ) {
        let (_, _, a, s) = app_and_answering_server();
        assert!(a.checks_peer());
        assert!(!s.checks_peer());

        assert!(matches!(
            s.open(a.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            a.open(s.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        let from_a = a.seal(Packet::keep_alive(0)).unwrap();
        let from_s = s.seal(Packet::keep_alive(0)).unwrap();
        assert!(opened(&s, from_a).is_some());
        assert!(opened(&a, from_s).is_some());

        let sealed = a.seal(audio(5, b"some audio")).unwrap();
        assert_eq!(opened(&s, sealed).unwrap().payload, b"some audio");
        assert_eq!(a.security(), LinkSecurity::Encrypted);
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn when_a_server_that_answers_any_app_has_heard_no_key_it_sends_an_unsigned_half_that_an_app_that_checks_does_not_take(
    ) {
        let (_, _, a, s) = app_and_answering_server();

        assert!(matches!(
            a.open(s.key_exchange_packet()),
            Opened::Dropped("key exchange not signed by the peer")
        ));
        assert!(!a.has_keys());
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn when_someone_on_the_path_answers_an_app_that_checks_with_a_key_of_their_own_it_is_not_taken()
    {
        let (_, _, a, _) = app_and_answering_server();
        let stranger = LinkIdentity::generate();
        let stranger_server = SecureLink::answering(&stranger);
        // The stranger takes the app's half as the server would and answers it, signed with
        // the identity it has, which is not the one the server told the room
        stranger_server.open(a.key_exchange_packet());
        let signed_by_stranger = stranger_server.key_exchange_packet();
        // ...and answers it unsigned
        let unsigned = SecureLink::new().key_exchange_packet();

        for forged in [signed_by_stranger, unsigned] {
            assert!(matches!(
                a.open(forged),
                Opened::Dropped("key exchange not signed by the peer")
            ));
        }
        assert!(!a.has_keys());
    }

    /// The server signs whatever half it is sent, so what it signed for one app must not be of
    /// use against another
    ///
    /// Verifies: REQ-SEC-009
    #[test]
    fn when_the_answer_the_server_gave_one_app_is_passed_to_another_it_is_not_taken() {
        let (_, server, a, s) = app_and_answering_server();
        let other =
            SecureLink::for_peer(Some(&LinkIdentity::generate()), Some(&server.public_key()));
        s.open(a.key_exchange_packet());

        assert!(matches!(
            other.open(s.key_exchange_packet()),
            Opened::Dropped("key exchange not signed by the peer")
        ));
        assert!(!other.has_keys());
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn when_the_ephemeral_key_in_the_servers_answer_is_replaced_it_is_not_taken() {
        let (_, _, a, s) = app_and_answering_server();
        s.open(a.key_exchange_packet());
        let mut packet = s.key_exchange_packet();
        packet.payload[2..34].copy_from_slice(&SecureLink::new().public);

        assert!(matches!(a.open(packet), Opened::Dropped(_)));
        assert!(!a.has_keys());
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn when_the_app_that_the_server_answers_sends_a_half_that_is_not_signed_the_answer_is_not_signed(
    ) {
        let (_, _, _, s) = app_and_answering_server();
        let unchecked = SecureLink::new();

        assert!(matches!(
            s.open(unchecked.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            unchecked.open(s.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn when_the_app_changes_its_key_before_the_link_is_confirmed_the_server_answers_the_new_one() {
        let (app, server, first, s) = app_and_answering_server();
        s.open(first.key_exchange_packet());
        let restarted = SecureLink::for_peer(Some(&app), Some(&server.public_key()));

        assert!(matches!(
            s.open(restarted.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            restarted.open(s.key_exchange_packet()),
            Opened::KeyExchange { answer: true }
        ));
        assert!(matches!(
            first.open(s.key_exchange_packet()),
            Opened::Dropped("key exchange not signed by the peer")
        ));
    }

    /// Verifies: REQ-SEC-009
    #[test]
    fn a_server_that_answers_any_app_still_carries_an_app_that_sends_plain_audio() {
        let (_, _, _, s) = app_and_answering_server();

        assert!(opened(&s, audio(1, b"plain")).is_some());
        assert_eq!(s.security(), LinkSecurity::Unencrypted);
    }

    #[test]
    fn the_debug_form_of_an_identity_does_not_show_the_secret_key() {
        let identity = LinkIdentity::generate();
        let rendered = format!("{identity:?}");
        assert!(rendered.contains(&identity.public_key()));
        assert!(!rendered.contains(&BASE64.encode(&identity.signing_key.to_bytes())));
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
