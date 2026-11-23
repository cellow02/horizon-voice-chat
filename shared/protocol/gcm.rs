//! AES-128-GCM through the `aes-gcm` crate.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};

/// Ciphertext, then the 16-byte tag.
pub fn seal(key: &[u8; 16], iv: &[u8; 12], plain: &[u8]) -> Vec<u8> {
    Aes128Gcm::new_from_slice(key)
        .expect("the key is 16 bytes")
        .encrypt(Nonce::from_slice(iv), plain)
        .expect("a packet is far below the GCM length limit")
}

/// None when the tag does not match.
pub fn open(key: &[u8; 16], iv: &[u8; 12], sealed: &[u8]) -> Option<Vec<u8>> {
    Aes128Gcm::new_from_slice(key)
        .ok()?
        .decrypt(Nonce::from_slice(iv), sealed)
        .ok()
}
