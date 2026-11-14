// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Encrypts a push to the phone's push key, so Apple and anyone on the path
//! see only the placeholder text. The phone's extension opens it with
//! CryptoKit:
//!
//! 1. X25519 between a fresh key made for this push and the phone's key.
//! 2. HKDF-SHA256 of the shared secret, salted with the fresh public key
//!    then the phone's, with [`INFO`], to a 32-byte key.
//! 3. ChaCha20-Poly1305 with a random nonce and no associated data.
//!
//! The sealed bytes are the fresh public key (32 bytes), then CryptoKit's
//! combined box: the nonce (12), the ciphertext and the tag (16).
//! `fixtures/push-vectors.json` holds a worked example the phone's tests
//! open too.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::error::Unspecified;
use ring::hkdf::{Salt, HKDF_SHA256};
use ring::rand::{SecureRandom, SystemRandom};

/// HKDF's info, which names this layout.
pub const INFO: &[u8] = b"maxhq push 1";
/// An X25519 public key's length.
pub const KEY_LEN: usize = 32;

/// Seals `plaintext` to `phone`, the phone's X25519 public key.
pub fn seal(phone: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, Unspecified> {
    let rng = SystemRandom::new();
    let fresh = EphemeralPrivateKey::generate(&X25519, &rng)?;
    let mut nonce = [0; NONCE_LEN];
    rng.fill(&mut nonce)?;
    seal_with(fresh, nonce, phone, plaintext)
}

/// Seals with a given fresh key and nonce, which only the test vectors
/// choose.
fn seal_with(
    fresh: EphemeralPrivateKey,
    nonce: [u8; NONCE_LEN],
    phone: &[u8; KEY_LEN],
    plaintext: &[u8],
) -> Result<Vec<u8>, Unspecified> {
    let fresh_public = fresh.compute_public_key()?;
    let key = agree_ephemeral(fresh, &UnparsedPublicKey::new(&X25519, phone), |shared| {
        key(shared, fresh_public.as_ref(), phone)
    })??;
    let mut sealed = Vec::with_capacity(KEY_LEN + NONCE_LEN + plaintext.len() + 16);
    sealed.extend_from_slice(fresh_public.as_ref());
    sealed.extend_from_slice(&nonce);
    let mut body = plaintext.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut body)?;
    sealed.extend_from_slice(&body);
    Ok(sealed)
}

/// The ChaCha20-Poly1305 key from the shared secret.
fn key(shared: &[u8], fresh: &[u8], phone: &[u8]) -> Result<LessSafeKey, Unspecified> {
    let salt = [fresh, phone].concat();
    let prk = Salt::new(HKDF_SHA256, &salt).extract(shared);
    let okm = prk.expand(&[INFO], &CHACHA20_POLY1305)?;
    Ok(LessSafeKey::new(UnboundKey::from(okm)))
}

/// A key and nonce from fixed bytes, for the test vectors.
#[cfg(test)]
#[allow(deprecated)]
fn fixed(private: &[u8; KEY_LEN]) -> EphemeralPrivateKey {
    let rng = ring::test::rand::FixedSliceRandom { bytes: private };
    EphemeralPrivateKey::generate(&X25519, &rng).unwrap()
}

/// The public key for an X25519 private key, as the phone makes it.
#[cfg(test)]
pub fn public(private: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    let public = fixed(private).compute_public_key().unwrap();
    public.as_ref().try_into().unwrap()
}

/// Opens what [`seal`] sealed with the phone's private key, as the phone's
/// extension does.
#[cfg(test)]
pub fn open(private: &[u8; KEY_LEN], sealed: &[u8]) -> Result<Vec<u8>, Unspecified> {
    if sealed.len() < KEY_LEN + NONCE_LEN + 16 {
        return Err(Unspecified);
    }
    let (fresh, rest) = sealed.split_at(KEY_LEN);
    let (nonce, body) = rest.split_at(NONCE_LEN);
    let phone = public(private);
    let key = agree_ephemeral(
        fixed(private),
        &UnparsedPublicKey::new(&X25519, fresh),
        |shared| key(shared, fresh, &phone),
    )??;
    let mut body = body.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce)?;
    let opened = key.open_in_place(nonce, Aad::empty(), &mut body)?;
    Ok(opened.to_vec())
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use serde::Deserialize;

    use super::*;

    /// `fixtures/push-vectors.json`.
    #[derive(Deserialize)]
    struct Vectors {
        info: String,
        vectors: Vec<Vector>,
    }

    #[derive(Deserialize)]
    struct Vector {
        phone_private: String,
        phone_public: String,
        fresh_private: String,
        fresh_public: String,
        nonce: String,
        plaintext: String,
        sealed: String,
    }

    fn hex<const N: usize>(text: &str) -> [u8; N] {
        let bytes: Vec<u8> = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect();
        bytes.try_into().unwrap()
    }

    fn vectors() -> Vectors {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/push-vectors.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn push_sealing_matches_the_shared_vectors() {
        let vectors = vectors();
        assert_eq!(vectors.info.as_bytes(), INFO);
        assert!(!vectors.vectors.is_empty());
        for vector in vectors.vectors {
            let phone_private = hex(&vector.phone_private);
            let phone = hex(&vector.phone_public);
            assert_eq!(public(&phone_private), phone);
            let fresh_private = hex(&vector.fresh_private);
            assert_eq!(public(&fresh_private), hex(&vector.fresh_public));
            let sealed = seal_with(
                fixed(&fresh_private),
                hex(&vector.nonce),
                &phone,
                vector.plaintext.as_bytes(),
            )
            .unwrap();
            assert_eq!(STANDARD.encode(&sealed), vector.sealed);

            let opened = open(&phone_private, &sealed).unwrap();
            assert_eq!(opened, vector.plaintext.as_bytes());
            for at in [0, KEY_LEN, KEY_LEN + NONCE_LEN, sealed.len() - 1] {
                let mut changed = sealed.clone();
                changed[at] ^= 1;
                assert!(open(&phone_private, &changed).is_err(), "byte {at}");
            }
        }
    }

    #[test]
    fn each_push_seals_with_a_fresh_key() {
        let private = [7; KEY_LEN];
        let phone = public(&private);
        let a = seal(&phone, b"one").unwrap();
        let b = seal(&phone, b"one").unwrap();
        assert_ne!(a[..KEY_LEN], b[..KEY_LEN]);
        assert_eq!(open(&private, &a).unwrap(), b"one");
        assert_eq!(open(&private, &b).unwrap(), b"one");
        assert!(open(&[8; KEY_LEN], &a).is_err(), "another phone");
    }
}
