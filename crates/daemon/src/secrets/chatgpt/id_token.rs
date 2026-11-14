// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Checks the identity token a sign-in gets: signed with one of the
//! issuer's keys in RS256 or ES256, from the issuer, for this device's
//! client, for this round's nonce, and not yet expired.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use ring::signature::{
    RsaPublicKeyComponents, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256,
};
use serde::Deserialize;
use serde_json::Value;

/// How far the token's expiry may lag the device's clock, in milliseconds.
const CLOCK_SKEW_MS: u64 = 5_000;

/// The issuer's keys, as its JWKS lists them.
#[derive(Deserialize)]
pub struct Jwks {
    keys: Vec<Jwk>,
}

/// One public key.
#[derive(Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

/// Who the token says signed in.
#[derive(Debug, PartialEq, Eq)]
pub struct Identity {
    /// The account's ID, which stays the same for this client.
    pub subject: String,
    /// Its email, when the token gives one.
    pub email: Option<String>,
}

/// What the token must match.
pub struct Expected<'a> {
    pub issuer: &'a str,
    /// The client the token is for.
    pub audience: &'a str,
    pub nonce: &'a Secret,
    pub now: Timestamp,
}

/// The identity in `token`, or which check it failed. The reason never
/// quotes the token.
pub fn verify(token: &Secret, keys: &Jwks, expected: &Expected) -> Result<Identity, &'static str> {
    let parts: Vec<&str> = token.expose().split('.').collect();
    let [header, claims, signature] = parts.as_slice() else {
        return Err("it isn't a JWT");
    };
    let header: Header = decode(header).ok_or("its header doesn't read")?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "its signature doesn't read")?;
    let signed = &token.expose()[..header_len(token.expose())];
    let kty = match header.alg.as_str() {
        "RS256" => "RSA",
        "ES256" => "EC",
        _ => return Err("it's signed with an algorithm that isn't taken"),
    };
    let verified = keys
        .keys
        .iter()
        .filter(|key| key.kty == kty)
        .filter(|key| header.kid.is_none() || key.kid == header.kid)
        .filter(|key| key.alg.as_deref().is_none_or(|alg| alg == header.alg))
        .any(|key| key.verifies(signed.as_bytes(), &signature));
    if !verified {
        return Err("no key of the issuer's signed it");
    }

    let claims: Value = decode(claims).ok_or("its claims don't read")?;
    if claims["iss"].as_str() != Some(expected.issuer) {
        return Err("it's from another issuer");
    }
    let for_client = match &claims["aud"] {
        Value::String(aud) => aud == expected.audience,
        Value::Array(auds) => auds.iter().any(|aud| aud == expected.audience),
        _ => false,
    };
    if !for_client {
        return Err("it's for another client");
    }
    let expires = claims["exp"]
        .as_u64()
        .ok_or("it has no expiry")?
        .saturating_mul(1000);
    if expires.saturating_add(CLOCK_SKEW_MS) <= expected.now.0 {
        return Err("it expired");
    }
    if claims["nonce"].as_str() != Some(expected.nonce.expose()) {
        return Err("it's for another sign-in");
    }
    let subject = match claims["sub"].as_str() {
        Some(sub) if !sub.is_empty() => sub.to_owned(),
        _ => return Err("it names no account"),
    };
    Ok(Identity {
        subject,
        email: claims["email"].as_str().map(str::to_owned),
    })
}

/// How long the signed part, the header and claims, is.
fn header_len(token: &str) -> usize {
    token.rfind('.').unwrap_or(0)
}

/// A base64url JSON part.
fn decode<T: serde::de::DeserializeOwned>(part: &str) -> Option<T> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()
}

impl Jwk {
    fn verifies(&self, message: &[u8], signature: &[u8]) -> bool {
        let field = |value: &Option<String>| URL_SAFE_NO_PAD.decode(value.as_deref()?).ok();
        match self.kty.as_str() {
            "RSA" => {
                let (Some(n), Some(e)) = (field(&self.n), field(&self.e)) else {
                    return false;
                };
                RsaPublicKeyComponents { n, e }
                    .verify(&RSA_PKCS1_2048_8192_SHA256, message, signature)
                    .is_ok()
            }
            "EC" if self.crv.as_deref() == Some("P-256") => {
                let (Some(x), Some(y)) = (field(&self.x), field(&self.y)) else {
                    return false;
                };
                let mut point = vec![4];
                point.extend(x);
                point.extend(y);
                UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
                    .verify(message, signature)
                    .is_ok()
            }
            _ => false,
        }
    }
}
