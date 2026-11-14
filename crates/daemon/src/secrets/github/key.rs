// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The GitHub App's private key: reading the PEM GitHub gave, telling it
//! apart from other keys by a hash, and signing the App's JWTs.

use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use ring::digest::{digest, SHA256};
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde_json::json;
use thiserror::Error;
use zeroize::Zeroizing;

/// How far back a JWT's `iat` goes, for a clock a little ahead of
/// GitHub's.
const ISSUED_BEFORE: Duration = Duration::from_secs(60);
/// How long a JWT lasts. GitHub takes up to 10 minutes.
const LASTS: Duration = Duration::from_secs(9 * 60);

/// The DER of the rsaEncryption algorithm with its empty parameters.
const RSA_ALGORITHM: &[u8] = &[
    0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
];

/// A GitHub App's RSA key, ready to sign.
pub struct AppKey {
    pair: RsaKeyPair,
    sha256: String,
}

/// Why a key can't be used.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum KeyError {
    /// It isn't a PEM private key at all.
    #[error("That isn't a private key. Paste the whole .pem file GitHub gave you.")]
    NotPem,
    /// A passphrase protects it.
    #[error("That key has a passphrase, and GitHub's App keys don't. Paste the .pem file as GitHub gave it.")]
    Encrypted,
    /// It's a private key, but not RSA.
    #[error("That isn't an RSA key, and GitHub App keys are.")]
    NotRsa,
    /// The key wouldn't sign.
    #[error("The key couldn't sign a token.")]
    CantSign,
}

impl AppKey {
    /// Reads a PEM key, in PKCS #1 as GitHub gives it or in PKCS #8.
    pub fn parse(pem: &str) -> Result<Self, KeyError> {
        let (label, der) = decode(pem)?;
        let pair = match label {
            "RSA PRIVATE KEY" => RsaKeyPair::from_der(&der),
            "PRIVATE KEY" => RsaKeyPair::from_pkcs8(&der),
            "ENCRYPTED PRIVATE KEY" => return Err(KeyError::Encrypted),
            _ => return Err(KeyError::NotRsa),
        }
        .map_err(|_| KeyError::NotRsa)?;
        let sha256 = hex(digest(&SHA256, &public_key_info(pair.public().as_ref())).as_ref());
        Ok(Self { pair, sha256 })
    }

    /// The hex SHA-256 of the key's public half as SubjectPublicKeyInfo.
    /// GitHub's App settings show the same hash in base64.
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The public half as PKCS #1, for a test server to check JWTs with.
    #[cfg(test)]
    pub fn public_der(&self) -> Vec<u8> {
        self.pair.public().as_ref().to_vec()
    }

    /// A JWT that speaks for App `app_id` for the next few minutes.
    pub fn jwt(&self, app_id: u64, now: Timestamp) -> Result<Secret, KeyError> {
        let seconds = now.0 / 1_000;
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = json!({
            "iat": seconds.saturating_sub(ISSUED_BEFORE.as_secs()),
            "exp": seconds + LASTS.as_secs(),
            "iss": app_id,
        });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{claims}");
        let mut signature = vec![0; self.pair.public().modulus_len()];
        self.pair
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .map_err(|_| KeyError::CantSign)?;
        let signature = URL_SAFE_NO_PAD.encode(signature);
        Ok(Secret::new(format!("{signed}.{signature}")))
    }
}

/// The label and bytes of the first PEM block in `pem`.
pub(in crate::secrets) fn decode(pem: &str) -> Result<(&str, Zeroizing<Vec<u8>>), KeyError> {
    let start = pem.find("-----BEGIN ").ok_or(KeyError::NotPem)?;
    let rest = &pem[start + "-----BEGIN ".len()..];
    let end = rest.find("-----").ok_or(KeyError::NotPem)?;
    let label = &rest[..end];
    let rest = &rest[end + "-----".len()..];
    let footer = format!("-----END {label}-----");
    let body = &rest[..rest.find(&footer).ok_or(KeyError::NotPem)?];
    // Old OpenSSL marks an encrypted key with headers inside the block.
    if body.contains("Proc-Type:") || body.contains("DEK-Info:") {
        return Err(KeyError::Encrypted);
    }
    let text: Zeroizing<String> =
        Zeroizing::new(body.chars().filter(|c| !c.is_whitespace()).collect());
    let der = STANDARD
        .decode(text.as_bytes())
        .map_err(|_| KeyError::NotPem)?;
    if der.is_empty() {
        return Err(KeyError::NotPem);
    }
    Ok((label, Zeroizing::new(der)))
}

/// Wraps a PKCS #1 RSA public key in a SubjectPublicKeyInfo.
fn public_key_info(rsa_public_key: &[u8]) -> Vec<u8> {
    let mut bits = vec![0x03];
    bits.extend(length(rsa_public_key.len() + 1));
    bits.push(0);
    bits.extend_from_slice(rsa_public_key);
    let mut info = vec![0x30];
    info.extend(length(RSA_ALGORITHM.len() + bits.len()));
    info.extend_from_slice(RSA_ALGORITHM);
    info.extend(bits);
    info
}

/// A DER length.
fn length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        return vec![len as u8];
    }
    let bytes: Vec<u8> = len
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .collect();
    let mut out = vec![0x80 | bytes.len() as u8];
    out.extend(bytes);
    out
}

/// Lowercase hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use ring::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
    use serde_json::Value;

    use super::*;
    use crate::secrets::testing::{pem, EC_PKCS8, RSA_PKCS1, RSA_PKCS8, RSA_SHA256};

    #[test]
    fn reads_both_rsa_forms_as_the_same_key() {
        let pkcs1 = AppKey::parse(&pem("RSA PRIVATE KEY", RSA_PKCS1)).unwrap();
        let pkcs8 = AppKey::parse(&pem("PRIVATE KEY", RSA_PKCS8)).unwrap();
        assert_eq!(pkcs1.sha256(), RSA_SHA256);
        assert_eq!(pkcs8.sha256(), RSA_SHA256);
    }

    #[test]
    fn takes_a_key_with_text_around_it() {
        let pasted = format!("  \n{}\n\n", pem("RSA PRIVATE KEY", RSA_PKCS1));
        assert!(AppKey::parse(&pasted).is_ok());
    }

    #[test]
    fn refuses_what_isnt_a_github_app_key() {
        let refused = |text: &str| AppKey::parse(text).err();
        assert_eq!(refused(""), Some(KeyError::NotPem));
        assert_eq!(refused("test-not-a-real-token"), Some(KeyError::NotPem));
        assert_eq!(
            refused(&pem("RSA PRIVATE KEY", "not base64!")),
            Some(KeyError::NotPem)
        );
        assert_eq!(
            refused(&pem("PRIVATE KEY", EC_PKCS8)),
            Some(KeyError::NotRsa)
        );
        assert_eq!(
            refused(&pem("OPENSSH PRIVATE KEY", RSA_PKCS8)),
            Some(KeyError::NotRsa)
        );
        assert_eq!(
            refused(&pem("ENCRYPTED PRIVATE KEY", RSA_PKCS8)),
            Some(KeyError::Encrypted)
        );
        let old = pem(
            "RSA PRIVATE KEY",
            &format!("Proc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\n{RSA_PKCS1}"),
        );
        assert_eq!(refused(&old), Some(KeyError::Encrypted));
        // Half a key.
        let half = &RSA_PKCS1[..RSA_PKCS1.len() / 2 / 4 * 4];
        assert_eq!(
            refused(&pem("RSA PRIVATE KEY", half)),
            Some(KeyError::NotRsa)
        );
    }

    #[test]
    fn signs_a_jwt_github_takes() {
        let key = AppKey::parse(&pem("RSA PRIVATE KEY", RSA_PKCS1)).unwrap();
        let jwt = key.jwt(42, Timestamp(1_800_000_000_500)).unwrap();
        let parts: Vec<&str> = jwt.expose().split('.').collect();
        assert_eq!(parts.len(), 3);

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "RS256");
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], 42);
        assert_eq!(claims["iat"], 1_800_000_000 - 60);
        assert_eq!(claims["exp"], 1_800_000_000 + 540);

        let signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        let public = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.public_der());
        let signed = format!("{}.{}", parts[0], parts[1]);
        public.verify(signed.as_bytes(), &signature).unwrap();
    }

    #[test]
    fn writes_der_lengths() {
        assert_eq!(length(5), [5]);
        assert_eq!(length(0x7f), [0x7f]);
        assert_eq!(length(0x80), [0x81, 0x80]);
        assert_eq!(length(270), [0x82, 0x01, 0x0e]);
    }
}
