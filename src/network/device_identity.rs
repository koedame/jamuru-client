//! Per-installation device identity (ADR-024)
//!
//! jamuru has no account registration. Instead, each installation generates
//! an Ed25519 key pair on first launch and derives a globally unique device
//! identifier from its public key. The identifier is presented on the
//! signaling WebSocket handshake together with a signature, so a client can
//! only claim an identifier it holds the private key for.
//!
//! The claim can be checked against the public key alone (no database, no
//! HTTP call): see [`device_id_from_public_key`] and [`signed_payload`].

use data_encoding::{BASE32_NOPAD, BASE64};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand::rngs::SysRng;
use rand::TryRng;
use sha2::{Digest, Sha256};

/// Prefix mixed into every signed payload so a signature produced here can
/// never be replayed as a signature for some other jamuru protocol. The
/// version suffix lets a future scheme change be distinguished on the wire.
const SIGNATURE_DOMAIN: &str = "jamjam-device-v1:";

/// Leading bytes of `SHA-256(public_key)` that make up a device identifier.
/// 16 bytes = 128 bits, which encodes to 26 base32 characters and makes a
/// global collision between two independently generated key pairs
/// negligible.
const DEVICE_ID_HASH_BYTES: usize = 16;

/// Length of a device identifier in characters: `ceil(128 / 5)`.
pub const DEVICE_ID_LEN: usize = 26;

/// An installation's Ed25519 key pair and the device identifier derived from
/// it.
///
/// Generate one with [`DeviceIdentity::generate`] on first launch and
/// persist [`DeviceIdentity::secret_bytes`]; restore it on later launches
/// with [`DeviceIdentity::from_secret_bytes`].
pub struct DeviceIdentity {
    signing_key: SigningKey,
    /// Derived once at construction - callers ask for it on every connect.
    device_id: String,
}

impl DeviceIdentity {
    /// Generates a new identity from operating-system randomness.
    pub fn generate() -> Self {
        let mut secret = [0u8; 32];
        SysRng
            .try_fill_bytes(&mut secret)
            .expect("operating-system randomness is unavailable");
        Self::from_secret_bytes(&secret)
    }

    /// Restores an identity from a previously persisted secret key.
    pub fn from_secret_bytes(secret: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(secret);
        let device_id = device_id_from_public_key(&signing_key.verifying_key());
        Self {
            signing_key,
            device_id,
        }
    }

    /// The secret key bytes, for persisting this identity. Treat as a
    /// credential: whoever holds these bytes can present this device
    /// identifier.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// This installation's device identifier ([`DEVICE_ID_LEN`] base32
    /// characters).
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The public key, base64-encoded for the `X-Device-PubKey` handshake
    /// header.
    pub fn public_key_b64(&self) -> String {
        BASE64.encode(self.signing_key.verifying_key().as_bytes())
    }

    /// Signs `unix_secs` for the `X-Device-Signature` handshake header. The
    /// same value must be sent as `X-Device-Timestamp` so the verifier can
    /// reconstruct the payload and bound the replay window.
    pub fn sign_timestamp(&self, unix_secs: i64) -> String {
        let signature = self.signing_key.sign(signed_payload(unix_secs).as_bytes());
        BASE64.encode(&signature.to_bytes())
    }
}

/// Deliberately omits the secret key so an accidental `{:?}` can't leak it.
impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// Derives a device identifier from a public key. This is the only
/// definition of the mapping - both the client (naming itself) and the
/// server (checking the claim) go through here.
pub fn device_id_from_public_key(public_key: &VerifyingKey) -> String {
    let digest = Sha256::digest(public_key.as_bytes());
    BASE32_NOPAD.encode(&digest[..DEVICE_ID_HASH_BYTES])
}

/// The exact text a device signs for a handshake at `unix_secs`. Whoever
/// checks the signature has to rebuild the same text.
pub fn signed_payload(unix_secs: i64) -> String {
    format!("{}{}", SIGNATURE_DOMAIN, unix_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_encoding::HEXLOWER;

    /// Verifies: REQ-IDT-001
    #[test]
    fn test_identity_restored_from_secret_bytes_keeps_the_same_device_id() {
        let original = DeviceIdentity::generate();
        let restored = DeviceIdentity::from_secret_bytes(&original.secret_bytes());
        assert_eq!(original.device_id(), restored.device_id());
        assert_eq!(original.public_key_b64(), restored.public_key_b64());
    }

    /// Verifies: REQ-IDT-001
    #[test]
    fn test_identity_restored_from_rfc8032_secret_has_the_rfc_public_key() {
        // RFC 8032 section 7.1, TEST 1. Secret keys persisted by earlier
        // builds must keep their device identifier after a dependency upgrade.
        let secret: [u8; 32] = HEXLOWER
            .decode(b"9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .unwrap()
            .try_into()
            .unwrap();
        let public = HEXLOWER
            .decode(b"d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
            .unwrap();

        let identity = DeviceIdentity::from_secret_bytes(&secret);

        assert_eq!(identity.public_key_b64(), BASE64.encode(&public));
    }

    /// Verifies: REQ-IDT-002
    #[test]
    fn test_device_id_is_26_base32_characters() {
        let device_id = DeviceIdentity::generate().device_id().to_string();
        assert_eq!(device_id.len(), DEVICE_ID_LEN);
        assert!(
            device_id
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "unexpected characters in {}",
            device_id
        );
    }

    /// Verifies: REQ-IDT-002
    #[test]
    fn test_distinct_key_pairs_produce_distinct_device_ids() {
        let ids: std::collections::HashSet<String> = (0..64)
            .map(|_| DeviceIdentity::generate().device_id().to_string())
            .collect();
        assert_eq!(ids.len(), 64);
    }

    /// Verifies: REQ-IDT-002
    #[test]
    fn test_device_id_is_derived_from_the_public_key_alone() {
        let identity = DeviceIdentity::generate();
        let public_key_bytes: [u8; 32] = BASE64
            .decode(identity.public_key_b64().as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key_bytes).unwrap();
        assert_eq!(
            device_id_from_public_key(&verifying_key),
            identity.device_id()
        );
    }

    #[test]
    fn test_the_signature_verifies_against_the_public_key_over_the_signed_payload() {
        use ed25519_dalek::{Signature, Verifier};

        let identity = DeviceIdentity::generate();
        let timestamp = 1_800_000_000;
        let public_key_bytes: [u8; 32] = BASE64
            .decode(identity.public_key_b64().as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        let signature_bytes: [u8; 64] = BASE64
            .decode(identity.sign_timestamp(timestamp).as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        let verifying_key = VerifyingKey::from_bytes(&public_key_bytes).unwrap();
        let signature = Signature::from_bytes(&signature_bytes);

        assert!(verifying_key
            .verify(signed_payload(timestamp).as_bytes(), &signature)
            .is_ok());
        assert!(verifying_key
            .verify(signed_payload(timestamp + 1).as_bytes(), &signature)
            .is_err());
    }

    #[test]
    fn test_debug_does_not_expose_the_secret_key() {
        let identity = DeviceIdentity::generate();
        let secret_b64 = BASE64.encode(&identity.secret_bytes());
        let rendered = format!("{:?}", identity);
        assert!(rendered.contains(identity.device_id()));
        assert!(!rendered.contains(&secret_b64));
    }
}
