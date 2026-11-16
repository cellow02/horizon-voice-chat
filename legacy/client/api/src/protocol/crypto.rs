//! Packet encryption: AES-128-GCM with a random 12-byte IV in front of the ciphertext.

use super::gcm;

/// Bytes of the per-connection key.
pub const SECRET_SIZE: usize = 16;
/// Bytes of the IV in front of every encrypted payload.
pub const IV_SIZE: usize = 12;
/// Bytes of the authentication tag behind the ciphertext.
pub const TAG_SIZE: usize = 16;

/// Fills `out` from the operating system's random source.
pub fn random_bytes(out: &mut [u8]) {
    getrandom::getrandom(out).expect("the operating system has a random source");
}

/// A random UUID (version 4).
pub fn random_uuid() -> u128 {
    let mut bytes = [0u8; 16];
    random_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    u128::from_be_bytes(bytes)
}

/// The key one player's voice connection is encrypted with.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub [u8; SECRET_SIZE]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    pub fn random() -> Self {
        let mut key = [0u8; SECRET_SIZE];
        random_bytes(&mut key);
        Self(key)
    }

    /// IV, then ciphertext and tag.
    pub fn encrypt(&self, plain: &[u8]) -> Vec<u8> {
        let mut iv = [0u8; IV_SIZE];
        random_bytes(&mut iv);
        let sealed = gcm::seal(&self.0, &iv, plain);
        let mut out = Vec::with_capacity(IV_SIZE + sealed.len());
        out.extend_from_slice(&iv);
        out.extend_from_slice(&sealed);
        out
    }

    /// None for a payload that is too short, has the wrong key or was changed.
    pub fn decrypt(&self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < IV_SIZE + TAG_SIZE {
            return None;
        }
        let (iv, sealed) = payload.split_at(IV_SIZE);
        gcm::open(&self.0, iv.try_into().ok()?, sealed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_round_trips_and_a_changed_one_is_refused() {
        let secret = Secret::random();
        let sealed = secret.encrypt(b"hello voice");
        assert_eq!(sealed.len(), IV_SIZE + 11 + TAG_SIZE);
        assert_eq!(secret.decrypt(&sealed).as_deref(), Some(&b"hello voice"[..]));
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(secret.decrypt(&changed), None);
        assert_eq!(Secret::random().decrypt(&sealed), None);
        assert_eq!(secret.decrypt(&sealed[..IV_SIZE + 3]), None);
    }

    /// NIST SP 800-38D test case 3 (AES-128-GCM, 96-bit IV, no AAD), so this
    /// interoperates with any standard implementation such as Java's `AES/GCM/NoPadding`.
    #[test]
    fn decrypts_the_nist_reference_vector() {
        fn hex(text: &str) -> Vec<u8> {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect()
        }
        let secret = Secret(hex("feffe9928665731c6d6a8f9467308308").try_into().unwrap());
        let plain = hex(
            "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
             1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255",
        );
        let mut payload = hex("cafebabefacedbaddecaf888");
        payload.extend(hex(
            "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e\
             21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985",
        ));
        payload.extend(hex("4d5c2af327cd64a62cf35abd2ba6fab4"));
        assert_eq!(secret.decrypt(&payload), Some(plain));
    }
}
