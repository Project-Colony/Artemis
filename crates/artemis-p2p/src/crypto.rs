use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

/// HKDF `info` label that scopes derived keys to Artemis direct messages.
///
/// A public protocol constant for domain separation, not a secret: every
/// client must use the same bytes, and changing them changes every key.
const DM_KEY_LABEL: &[u8] = b"artemis-p2p v1 dm key";

/// ChaCha20-Poly1305 nonce length. Every ciphertext starts with its nonce.
const NONCE_LEN: usize = 12;

/// Cryptographic identity for a peer — X25519 keypair for key exchange.
pub struct Identity {
    secret: StaticSecret,
    public: PublicKey,
}

impl Identity {
    /// Generate a new random identity from the OS CSPRNG.
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Restore identity from a stored secret key (32 bytes, base64-encoded).
    pub fn from_secret_b64(secret_b64: &str) -> Result<Self, CryptoError> {
        let secret = StaticSecret::from(decode_key(secret_b64)?);
        let public = PublicKey::from(&secret);
        Ok(Self { secret, public })
    }

    /// Get the public key as base64.
    pub fn public_key_b64(&self) -> String {
        B64.encode(self.public.as_bytes())
    }

    /// Get the secret key as base64 (for storage).
    pub fn secret_key_b64(&self) -> String {
        B64.encode(self.secret.to_bytes())
    }

    /// Get the raw public key.
    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    /// Derive the ChaCha20-Poly1305 cipher shared with `peer`.
    ///
    /// The raw X25519 output is never used as the key: HKDF-SHA256 derives
    /// one from it, bound to both public keys (sorted, so both sides derive the
    /// same key). A low-order peer key is refused, because it makes the shared
    /// secret all zeros and the key a constant anyone can compute.
    fn cipher_for(&self, peer: &PublicKey) -> Result<ChaCha20Poly1305, CryptoError> {
        let shared = self.secret.diffie_hellman(peer);
        if !shared.was_contributory() {
            return Err(CryptoError::InvalidKey);
        }
        let (a, b) = (self.public.as_bytes(), peer.as_bytes());
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut key = Key::default();
        Hkdf::<Sha256>::new(None, shared.as_bytes())
            .expand_multi_info(&[DM_KEY_LABEL, lo, hi], &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Ok(ChaCha20Poly1305::new(&key))
    }

    /// Encrypt a message for a peer. The output is a fresh random nonce from
    /// the OS CSPRNG followed by the ciphertext.
    pub fn encrypt_for(
        &self,
        recipient_pub: &PublicKey,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher_for(recipient_pub)?
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::EncryptionFailed)?;
        Ok([nonce.as_slice(), &ciphertext].concat())
    }

    /// Decrypt a message from a peer using their public key.
    pub fn decrypt_from(
        &self,
        sender_pub: &PublicKey,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if data.len() < NONCE_LEN {
            return Err(CryptoError::InvalidCiphertext);
        }
        let (nonce, ciphertext) = data.split_at(NONCE_LEN);
        self.cipher_for(sender_pub)?
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)
    }

    /// Encrypt a message for a recipient (base64 public key), returning base64 ciphertext.
    pub fn encrypt_for_b64(
        &self,
        recipient_pub_b64: &str,
        plaintext: &[u8],
    ) -> Result<String, CryptoError> {
        let recipient_pub = PublicKey::from(decode_key(recipient_pub_b64)?);
        let encrypted = self.encrypt_for(&recipient_pub, plaintext)?;
        Ok(B64.encode(encrypted))
    }

    /// Decrypt base64 ciphertext from a sender (base64 public key).
    pub fn decrypt_from_b64(
        &self,
        sender_pub_b64: &str,
        ciphertext_b64: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        let sender_pub = PublicKey::from(decode_key(sender_pub_b64)?);
        let data = B64
            .decode(ciphertext_b64)
            .map_err(|_| CryptoError::InvalidCiphertext)?;
        self.decrypt_from(&sender_pub, &data)
    }
}

/// Decode a base64 X25519 key, which must be exactly 32 bytes.
fn decode_key(b64: &str) -> Result<[u8; 32], CryptoError> {
    B64.decode(b64)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(CryptoError::InvalidKey)
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid key")]
    InvalidKey,
    #[error("encryption failed")]
    EncryptionFailed,
    #[error("decryption failed")]
    DecryptionFailed,
    #[error("invalid ciphertext")]
    InvalidCiphertext,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_generation_produces_valid_keys() {
        let id = Identity::generate();
        let pub_b64 = id.public_key_b64();
        let sec_b64 = id.secret_key_b64();
        // X25519 keys are 32 bytes, base64 encoded = 44 chars
        assert_eq!(pub_b64.len(), 44);
        assert_eq!(sec_b64.len(), 44);
    }

    #[test]
    fn identity_restore_from_secret() {
        let id = Identity::generate();
        let sec_b64 = id.secret_key_b64();
        let pub_b64 = id.public_key_b64();

        let restored = Identity::from_secret_b64(&sec_b64).unwrap();
        assert_eq!(restored.public_key_b64(), pub_b64);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let alice = Identity::generate();
        let bob = Identity::generate();

        let plaintext = b"Hello, Bob!";
        let ciphertext = alice.encrypt_for(bob.public_key(), plaintext).unwrap();

        let decrypted = bob.decrypt_from(alice.public_key(), &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_b64_roundtrip() {
        let alice = Identity::generate();
        let bob = Identity::generate();

        let plaintext = b"Secret message";
        let encrypted = alice
            .encrypt_for_b64(&bob.public_key_b64(), plaintext)
            .unwrap();

        let decrypted = bob
            .decrypt_from_b64(&alice.public_key_b64(), &encrypted)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let eve = Identity::generate();

        let ciphertext = alice.encrypt_for(bob.public_key(), b"secret").unwrap();

        // Eve should not be able to decrypt
        let result = eve.decrypt_from(alice.public_key(), &ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_secret_key_rejected() {
        assert!(Identity::from_secret_b64("not-valid-base64!!!").is_err());
        // Valid base64 but wrong length
        assert!(Identity::from_secret_b64("AQID").is_err());
    }

    #[test]
    fn low_order_peer_key_rejected() {
        let alice = Identity::generate();
        // u = 0 is a low-order point: the shared secret would be all zeros.
        let low_order = PublicKey::from(<[u8; 32]>::default());
        assert!(matches!(
            alice.encrypt_for(&low_order, b"secret"),
            Err(CryptoError::InvalidKey)
        ));
    }

    #[test]
    fn each_message_gets_a_fresh_nonce() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let first = alice.encrypt_for(bob.public_key(), b"same").unwrap();
        let second = alice.encrypt_for(bob.public_key(), b"same").unwrap();
        assert_ne!(first[..NONCE_LEN], second[..NONCE_LEN]);
        assert_ne!(first, second);
    }

    #[test]
    fn short_ciphertext_rejected() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let result = bob.decrypt_from(alice.public_key(), &[0u8; 5]);
        assert!(result.is_err());
    }
}
