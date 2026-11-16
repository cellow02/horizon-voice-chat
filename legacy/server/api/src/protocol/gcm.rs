//! AES-128-GCM (NIST SP 800-38D) on top of the `aes` block cipher, for 96-bit IVs and no
//! additional data, which is all the voice protocol uses.
//!
//! Horizon's older mod API lets a mod use only the crates in Horizon's own lock file. That has
//! `aes` but not `aes-gcm`, so the counter mode and the GHASH tag are written out here. The
//! interop tests check this file against the `aes-gcm` crate and the NIST vectors.

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit};

const TAG_SIZE: usize = 16;

/// Multiplies in GF(2^128) as GCM defines it (bit 0 is the most significant bit). No branch or
/// table depends on the values, so timing does not leak the hash key.
fn multiply(x: u128, y: u128) -> u128 {
    const R: u128 = 0xe1 << 120;
    let mut z = 0u128;
    let mut v = x;
    for bit in (0..128).rev() {
        z ^= v & 0u128.wrapping_sub((y >> bit) & 1);
        v = (v >> 1) ^ (R & 0u128.wrapping_sub(v & 1));
    }
    z
}

struct Gcm {
    cipher: Aes128,
    /// The hash key: the encryption of the zero block.
    h: u128,
    /// The counter block's first 12 bytes.
    iv: [u8; 12],
}

impl Gcm {
    fn new(key: &[u8; 16], iv: &[u8; 12]) -> Self {
        let cipher = Aes128::new(key.into());
        let mut zero = [0u8; 16];
        cipher.encrypt_block((&mut zero).into());
        Self {
            cipher,
            h: u128::from_be_bytes(zero),
            iv: *iv,
        }
    }

    /// The encryption of counter block `counter` (1 masks the tag, data starts at 2).
    fn key_stream(&self, counter: u32) -> [u8; 16] {
        let mut block = [0u8; 16];
        block[..12].copy_from_slice(&self.iv);
        block[12..].copy_from_slice(&counter.to_be_bytes());
        self.cipher.encrypt_block((&mut block).into());
        block
    }

    /// Encrypts or decrypts `data` in place: counter mode is its own inverse.
    fn apply(&self, data: &mut [u8]) {
        for (index, chunk) in data.chunks_mut(16).enumerate() {
            let stream = self.key_stream(index as u32 + 2);
            for (byte, key) in chunk.iter_mut().zip(stream) {
                *byte ^= key;
            }
        }
    }

    fn tag(&self, ciphertext: &[u8]) -> [u8; TAG_SIZE] {
        let mut y = 0u128;
        for chunk in ciphertext.chunks(16) {
            let mut block = [0u8; 16];
            block[..chunk.len()].copy_from_slice(chunk);
            y = multiply(y ^ u128::from_be_bytes(block), self.h);
        }
        // The lengths in bits: no additional data, then the ciphertext.
        y = multiply(y ^ (ciphertext.len() as u128 * 8), self.h);
        (y ^ u128::from_be_bytes(self.key_stream(1))).to_be_bytes()
    }
}

/// Ciphertext, then the 16-byte tag.
pub fn seal(key: &[u8; 16], iv: &[u8; 12], plain: &[u8]) -> Vec<u8> {
    let gcm = Gcm::new(key, iv);
    let mut out = plain.to_vec();
    gcm.apply(&mut out);
    let tag = gcm.tag(&out);
    out.extend_from_slice(&tag);
    out
}

/// None when the tag does not match.
pub fn open(key: &[u8; 16], iv: &[u8; 12], sealed: &[u8]) -> Option<Vec<u8>> {
    let (ciphertext, tag) = sealed.split_at_checked(sealed.len().checked_sub(TAG_SIZE)?)?;
    let gcm = Gcm::new(key, iv);
    // Compared without stopping at the first difference.
    let difference = gcm
        .tag(ciphertext)
        .iter()
        .zip(tag)
        .fold(0u8, |sum, (a, b)| sum | (a ^ b));
    if difference != 0 {
        return None;
    }
    let mut out = ciphertext.to_vec();
    gcm.apply(&mut out);
    Some(out)
}
