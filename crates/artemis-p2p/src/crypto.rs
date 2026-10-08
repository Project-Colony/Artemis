use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use x25519_dalek::{PublicKey, StaticSecret};

/// Cryptographic identity for a peer — X25519 keypair for key exchange.
pub struct Identity {
    secret: StaticSecret,
    public: PublicKey,
}

impl Identity {
    /// Generate a new random identity.
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(rand::thread_rng());
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// Restore identity from a stored secret key (32 bytes, base64-encoded).
    pub fn from_secret_b64(secret_b64: &str) -> Result<Self, CryptoError> {
        let bytes = B64
            .decode(secret_b64)
            .map_err(|_| CryptoError::InvalidKey)?;
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKey);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        let secret = StaticSecret::from(arr);
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

    /// Derive a shared secret with another peer's public key,
    /// then encrypt a message using ChaCha20-Poly1305.
    pub fn encrypt_for(
        &self,
        recipient_pub: &PublicKey,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let shared = self.secret.diffie_hellman(recipient_pub);
        let key = chacha20poly1305::Key::from_slice(shared.as_bytes());
        let cipher = ChaCha20Poly1305::new(key);

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| CryptoError::EncryptionFailed)?;

        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// Decrypt a message from a peer using their public key.
    pub fn decrypt_from(
        &self,
        sender_pub: &PublicKey,
        data: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if data.len() < 12 {
            return Err(CryptoError::InvalidCiphertext);
        }
        let shared = self.secret.diffie_hellman(sender_pub);
        let key = chacha20poly1305::Key::from_slice(shared.as_bytes());
        let cipher = ChaCha20Poly1305::new(key);

        let nonce = Nonce::from_slice(&data[..12]);
        let ciphertext = &data[12..];

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| CryptoError::DecryptionFailed)
    }

    /// Encrypt a message for a recipient (base64 public key), returning base64 ciphertext.
    pub fn encrypt_for_b64(
        &self,
        recipient_pub_b64: &str,
        plaintext: &[u8],
    ) -> Result<String, CryptoError> {
        let pub_bytes = B64
            .decode(recipient_pub_b64)
            .map_err(|_| CryptoError::InvalidKey)?;
        if pub_bytes.len() != 32 {
            return Err(CryptoError::InvalidKey);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&pub_bytes);
        let recipient_pub = PublicKey::from(arr);
        let encrypted = self.encrypt_for(&recipient_pub, plaintext)?;
        Ok(B64.encode(encrypted))
    }

    /// Decrypt base64 ciphertext from a sender (base64 public key).
    pub fn decrypt_from_b64(
        &self,
        sender_pub_b64: &str,
        ciphertext_b64: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        let pub_bytes = B64
            .decode(sender_pub_b64)
            .map_err(|_| CryptoError::InvalidKey)?;
        if pub_bytes.len() != 32 {
            return Err(CryptoError::InvalidKey);
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&pub_bytes);
        let sender_pub = PublicKey::from(arr);
        let data = B64
            .decode(ciphertext_b64)
            .map_err(|_| CryptoError::InvalidCiphertext)?;
        self.decrypt_from(&sender_pub, &data)
    }
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
    fn short_ciphertext_rejected() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let result = bob.decrypt_from(alice.public_key(), &[0u8; 5]);
        assert!(result.is_err());
    }
}
