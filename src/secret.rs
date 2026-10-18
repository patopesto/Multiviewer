use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};

const PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

/// AES-256 key baked into the binary. This keeps secrets out of the project
/// file in plain text, but the key ships with the (public) source: it is
/// obfuscation against casual inspection, not a confidentiality guarantee.
const KEY: [u8; 32] = [
    0xf9, 0xb2, 0x46, 0xc0, 0x3e, 0xb8, 0xb6, 0x0b, 0xb2, 0x9f, 0xec, 0x87, 0x17, 0x67, 0x07, 0xb1,
    0x0c, 0x21, 0xc8, 0x42, 0x7c, 0x6f, 0x49, 0xd2, 0xbc, 0xa4, 0xc8, 0xc1, 0x75, 0x4c, 0x7b, 0x68,
];

fn cipher() -> Aes256Gcm {
    return Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&KEY));
}

/// Encrypt a secret into a `v1:`-tagged hex blob. An empty secret stays empty
/// so projects without a password keep a clean config.
pub fn encrypt(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let Ok(ciphertext) = cipher().encrypt(&nonce, secret.as_bytes()) else {
        tracing::error!("secret encryption failed");
        return String::new();
    };
    let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    return format!("{PREFIX}{}", to_hex(&blob));
}

/// Decrypt a blob produced by [`encrypt`]. Unreadable input yields an empty
/// secret rather than failing the load: a project must stay openable even when
/// its secret cannot be recovered.
pub fn decrypt(blob: &str) -> String {
    if blob.is_empty() {
        return String::new();
    }
    let Some(hex) = blob.strip_prefix(PREFIX) else {
        tracing::warn!("secret has an unrecognized format; ignoring it");
        return String::new();
    };
    let Some(bytes) = from_hex(hex) else {
        tracing::warn!("secret is not valid hex; ignoring it");
        return String::new();
    };
    if bytes.len() <= NONCE_LEN {
        tracing::warn!("secret is too short; ignoring it");
        return String::new();
    }
    let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
    let Ok(plaintext) = cipher().decrypt(Nonce::from_slice(nonce), ciphertext) else {
        tracing::warn!("secret could not be decrypted; ignoring it");
        return String::new();
    };
    let Ok(text) = String::from_utf8(plaintext) else {
        tracing::warn!("secret is not valid UTF-8; ignoring it");
        return String::new();
    };
    return text;
}

fn to_hex(bytes: &[u8]) -> String {
    return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    return digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            return Some(((hi << 4) | lo) as u8);
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_secret() {
        let blob = encrypt("hunter2");
        assert_ne!(blob, "hunter2");
        assert!(blob.starts_with(PREFIX));
        assert_eq!(decrypt(&blob), "hunter2");
    }

    #[test]
    fn empty_secret_stays_empty() {
        assert_eq!(encrypt(""), "");
        assert_eq!(decrypt(""), "");
    }

    /// A fresh nonce per encryption; reuse would leak plaintext XOR under GCM.
    #[test]
    fn encryption_is_not_deterministic() {
        assert_ne!(encrypt("same"), encrypt("same"));
    }

    /// Legacy/corrupt input degrades to empty instead of failing the load.
    #[test]
    fn unreadable_blobs_decrypt_to_empty() {
        assert_eq!(decrypt("plaintext"), "");
        assert_eq!(decrypt("v1:zz"), "");
        assert_eq!(decrypt("v1:00ff"), "");
        let mut tampered = encrypt("hunter2");
        tampered.pop();
        assert_eq!(decrypt(&tampered), "");
    }
}
