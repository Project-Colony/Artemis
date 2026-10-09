use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// ChaCha20-Poly1305 nonce length. Every ciphertext starts with its nonce.
const NONCE_LEN: usize = 12;

/// What a ciphertext protects. Each purpose derives its own key, so a
/// ciphertext made for one is never accepted as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A direct message sent through the relay.
    DirectMessage,
    /// Connection details posted to a peer's signaling Gist.
    Signaling,
}

impl Purpose {
    /// HKDF `info` label for this purpose.
    ///
    /// Public protocol constants for domain separation, not secrets: every
    /// client must use the same bytes, and changing them changes every key.
    fn label(self) -> &'static [u8] {
        match self {
            Self::DirectMessage => b"artemis-p2p v1 direct message",
            Self::Signaling => b"artemis-p2p v1 signaling",
        }
    }
}

/// Cryptographic identity for a peer — X25519 keypair for key exchange.
pub struct Identity {
    secret: StaticSecret,
    public: PublicKey,
}

impl Identity {
    /// Generate a new random identity from the OS CSPRNG.
    pub fn generate() -> Self {
        let secret = StaticSecret::random();
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Restore identity from a stored secret key (32 bytes, base64-encoded).
    pub fn from_secret_b64(secret_b64: &str) -> Result<Self, CryptoError> {
        let secret = StaticSecret::from(*decode_key(secret_b64)?);
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

    /// Derive the ChaCha20-Poly1305 cipher for messages from `sender` to
    /// `recipient`, one of which is this identity and the other `peer`.
    ///
    /// The raw X25519 output is never used as the key: HKDF-SHA256 derives
    /// one from it, bound to the purpose and to both public keys in sending
    /// order. Alice-to-Bob and Bob-to-Alice therefore use different keys, so a
    /// relay cannot reflect Alice's own message back to her as one from Bob.
    /// A low-order peer key is refused, because it makes the shared secret all
    /// zeros and the key a constant anyone can compute.
    fn cipher(
        &self,
        purpose: Purpose,
        peer: &PublicKey,
        sender: &PublicKey,
        recipient: &PublicKey,
    ) -> Result<ChaCha20Poly1305, CryptoError> {
        let shared = self.secret.diffie_hellman(peer);
        if !shared.was_contributory() {
            return Err(CryptoError::InvalidKey);
        }
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(None, shared.as_bytes())
            .expand_multi_info(
                &[purpose.label(), sender.as_bytes(), recipient.as_bytes()],
                key.as_mut_slice(),
            )
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Ok(ChaCha20Poly1305::new((&*key).into()))
    }

    /// Encrypt a message for a peer. The output is a fresh random nonce from
    /// the OS CSPRNG followed by the ciphertext.
    pub fn encrypt_for(
        &self,
        purpose: Purpose,
        recipient_pub: &PublicKey,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let nonce = Nonce::try_generate().map_err(|_| CryptoError::EncryptionFailed)?;
        let ciphertext = self
            .cipher(purpose, recipient_pub, &self.public, recipient_pub)?
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::EncryptionFailed)?;
        Ok([nonce.as_slice(), &ciphertext].concat())
    }

    /// Decrypt a message a peer sent to us, using their public key.
    pub fn decrypt_from(
        &self,
        purpose: Purpose,
        sender_pub: &PublicKey,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        open(
            self.cipher(purpose, sender_pub, sender_pub, &self.public)?,
            data,
        )
    }

    /// Decrypt a message we sent to a peer earlier (our own side of a
    /// conversation history), using the recipient's public key.
    pub fn decrypt_sent(
        &self,
        purpose: Purpose,
        recipient_pub: &PublicKey,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        open(
            self.cipher(purpose, recipient_pub, &self.public, recipient_pub)?,
            data,
        )
    }

    /// Encrypt a message for a recipient (base64 public key), returning base64 ciphertext.
    pub fn encrypt_for_b64(
        &self,
        purpose: Purpose,
        recipient_pub_b64: &str,
        plaintext: &[u8],
    ) -> Result<String, CryptoError> {
        let recipient_pub = PublicKey::from(*decode_key(recipient_pub_b64)?);
        let encrypted = self.encrypt_for(purpose, &recipient_pub, plaintext)?;
        Ok(B64.encode(encrypted))
    }

    /// Decrypt base64 ciphertext from a sender (base64 public key).
    pub fn decrypt_from_b64(
        &self,
        purpose: Purpose,
        sender_pub_b64: &str,
        ciphertext_b64: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        let sender_pub = PublicKey::from(*decode_key(sender_pub_b64)?);
        self.decrypt_from(purpose, &sender_pub, &decode_ciphertext(ciphertext_b64)?)
    }

    /// Decrypt base64 ciphertext we sent to a recipient (base64 public key).
    pub fn decrypt_sent_b64(
        &self,
        purpose: Purpose,
        recipient_pub_b64: &str,
        ciphertext_b64: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        let recipient_pub = PublicKey::from(*decode_key(recipient_pub_b64)?);
        self.decrypt_sent(purpose, &recipient_pub, &decode_ciphertext(ciphertext_b64)?)
    }
}

/// Split off the nonce and authenticate and decrypt the rest.
fn open(cipher: ChaCha20Poly1305, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if data.len() < NONCE_LEN {
        return Err(CryptoError::InvalidCiphertext);
    }
    let (nonce, ciphertext) = data.split_at(NONCE_LEN);
    let nonce = Nonce::try_from(nonce).map_err(|_| CryptoError::InvalidCiphertext)?;
    cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| CryptoError::DecryptionFailed)
}

/// Decode base64 ciphertext (nonce followed by the sealed message).
fn decode_ciphertext(b64: &str) -> Result<Vec<u8>, CryptoError> {
    B64.decode(b64).map_err(|_| CryptoError::InvalidCiphertext)
}

/// Decode a base64 X25519 key, which must be exactly 32 bytes. The buffers
/// are wiped on drop, since the same path decodes secret keys.
fn decode_key(b64: &str) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let bytes = Zeroizing::new(B64.decode(b64).map_err(|_| CryptoError::InvalidKey)?);
    let mut key = Zeroizing::new([0u8; 32]);
    if bytes.len() != key.len() {
        return Err(CryptoError::InvalidKey);
    }
    key.copy_from_slice(&bytes);
    Ok(key)
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

    const DM: Purpose = Purpose::DirectMessage;

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
        let ciphertext = alice.encrypt_for(DM, bob.public_key(), plaintext).unwrap();

        let decrypted = bob
            .decrypt_from(DM, alice.public_key(), &ciphertext)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_b64_roundtrip() {
        let alice = Identity::generate();
        let bob = Identity::generate();

        let plaintext = b"Secret message";
        let encrypted = alice
            .encrypt_for_b64(DM, &bob.public_key_b64(), plaintext)
            .unwrap();

        let decrypted = bob
            .decrypt_from_b64(DM, &alice.public_key_b64(), &encrypted)
            .unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let eve = Identity::generate();

        let ciphertext = alice.encrypt_for(DM, bob.public_key(), b"secret").unwrap();

        // Eve should not be able to decrypt
        let result = eve.decrypt_from(DM, alice.public_key(), &ciphertext);
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
            alice.encrypt_for(DM, &low_order, b"secret"),
            Err(CryptoError::InvalidKey)
        ));
    }

    #[test]
    fn each_message_gets_a_fresh_nonce() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let first = alice.encrypt_for(DM, bob.public_key(), b"same").unwrap();
        let second = alice.encrypt_for(DM, bob.public_key(), b"same").unwrap();
        assert_ne!(first[..NONCE_LEN], second[..NONCE_LEN]);
        assert_ne!(first, second);
    }

    #[test]
    fn short_ciphertext_rejected() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let result = bob.decrypt_from(DM, alice.public_key(), &[0u8; 5]);
        assert!(result.is_err());
    }

    #[test]
    fn sender_can_read_own_sent_message() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let ciphertext = alice.encrypt_for(DM, bob.public_key(), b"hi").unwrap();
        let plaintext = alice
            .decrypt_sent(DM, bob.public_key(), &ciphertext)
            .unwrap();
        assert_eq!(plaintext, b"hi");
    }

    #[test]
    fn reflected_message_rejected() {
        // A relay bouncing Alice's message to Bob back to her, as if Bob sent it.
        let alice = Identity::generate();
        let bob = Identity::generate();
        let ciphertext = alice.encrypt_for(DM, bob.public_key(), b"hi").unwrap();
        assert!(alice
            .decrypt_from(DM, bob.public_key(), &ciphertext)
            .is_err());
    }

    #[test]
    fn purposes_do_not_mix() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let dm = alice.encrypt_for(DM, bob.public_key(), b"hi").unwrap();
        let signaling = Purpose::Signaling;
        assert!(bob
            .decrypt_from(signaling, alice.public_key(), &dm)
            .is_err());
    }
}
