// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the proof's
// header and claims, the order of the checks and their failure codes, the
// RFC 7638 thumbprint, `ath`, the URL without its query and fragment, and
// the 300 second window. This signs with Ed25519, so a phone proves with
// its own SSH key.

//! DPoP proofs (RFC 9449): a compact JWT, signed with EdDSA over Ed25519,
//! that binds a request to its method, its URL and, with a token, the
//! token. Nothing here reads a clock: the caller passes the time.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::digest::{digest, SHA256};
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

/// How far a proof's `iat` may sit from the verifier's clock, either way,
/// in seconds. It bounds the clock skew a client may have too.
pub const MAX_CLOCK_SKEW: i64 = 300;

const TYP: &str = "dpop+jwt";
const ALG: &str = "EdDSA";
const KTY: &str = "OKP";
const CRV: &str = "Ed25519";

/// Why a proof failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum Failure {
    /// There's no proof.
    #[error("missing DPoP proof")]
    MissingProof,
    /// The proof isn't a DPoP JWT with an Ed25519 key.
    #[error("invalid DPoP JWT")]
    MalformedProof,
    /// It's made with another key.
    #[error("DPoP key thumbprint mismatch")]
    KeyMismatch,
    /// It's for another method.
    #[error("DPoP method mismatch")]
    MethodMismatch,
    /// It's for another URL.
    #[error("DPoP URL mismatch")]
    UrlMismatch,
    /// It's for another token, or none.
    #[error("DPoP access token hash mismatch")]
    AccessTokenHashMismatch,
    /// Its `iat` is too far from now.
    #[error("DPoP proof is outside the allowed time window")]
    TimeWindow,
    /// Its signature doesn't verify.
    #[error("invalid DPoP signature")]
    InvalidSignature,
    /// It names the certificate of another listener.
    #[error("DPoP pin mismatch")]
    PinMismatch,
}

/// What a proof that verified says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// Its key's RFC 7638 thumbprint.
    pub thumbprint: String,
    /// Its key.
    pub key: [u8; 32],
    /// Its ID, which a verifier keeps a while to refuse it again.
    pub jti: String,
    /// When it was made, in seconds since the epoch.
    pub iat: i64,
}

/// What a proof must match.
#[derive(Clone, Copy, Debug)]
pub struct Expected<'a> {
    /// The request's method.
    pub method: &'a str,
    /// The request's URL. Its query and fragment don't count.
    pub url: &'a str,
    /// Now, in seconds since the epoch.
    pub now: i64,
    /// The thumbprint the key must have, if one is known.
    pub thumbprint: Option<&'a str>,
    /// The token the request carries, which `ath` must name.
    pub access_token: Option<&'a str>,
    /// The verifier's certificate pin, which `pin` must name when the proof
    /// has one. A proof without one still passes, for phones from before
    /// the claim.
    pub pin: Option<&'a str>,
}

#[derive(Deserialize)]
struct Header {
    typ: String,
    alg: String,
    jwk: Jwk,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    crv: String,
    x: String,
    /// A private part, which a proof never carries.
    #[serde(default)]
    d: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
struct Claims {
    htm: String,
    htu: String,
    jti: String,
    iat: i64,
    #[serde(default)]
    ath: Option<String>,
    /// The certificate pin the phone checked before it sent this.
    #[serde(default)]
    pin: Option<String>,
}

/// The URL a proof names: `url` without its query and fragment, or none
/// when it isn't a URL.
pub fn normalize_htu(url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    parsed.set_fragment(None);
    parsed.set_query(None);
    Some(parsed.into())
}

/// The RFC 7638 thumbprint of an Ed25519 public key.
pub fn thumbprint(public: &[u8; 32]) -> String {
    // The required members, in order, with no spaces.
    let input = format!(
        r#"{{"crv":"{CRV}","kty":"{KTY}","x":"{}"}}"#,
        URL_SAFE_NO_PAD.encode(public)
    );
    URL_SAFE_NO_PAD.encode(digest(&SHA256, input.as_bytes()))
}

/// The `ath` for `access_token`.
pub fn access_token_hash(access_token: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest(&SHA256, access_token.as_bytes()))
}

/// A proof for `method` on `url` made at `now`, with the Ed25519 key whose
/// seed is `seed`, and `ath` when the request carries `access_token`. The
/// value goes in the `DPoP` header. `jti` must be new for each proof, such
/// as a random UUID. None when `url` isn't a URL.
pub fn sign(
    seed: &[u8; 32],
    method: &str,
    url: &str,
    access_token: Option<&str>,
    jti: &str,
    now: i64,
) -> Option<String> {
    sign_for_pin(seed, method, url, access_token, None, jti, now)
}

/// [`sign`], and naming `pin`, the certificate pin the phone checked on
/// this connection, so a listener with another certificate can't relay the
/// proof to the real one.
pub fn sign_for_pin(
    seed: &[u8; 32],
    method: &str,
    url: &str,
    access_token: Option<&str>,
    pin: Option<&str>,
    jti: &str,
    now: i64,
) -> Option<String> {
    let htu = normalize_htu(url)?;
    let pair = Ed25519KeyPair::from_seed_unchecked(seed).ok()?;
    let header = json!({
        "typ": TYP,
        "alg": ALG,
        "jwk": { "kty": KTY, "crv": CRV, "x": URL_SAFE_NO_PAD.encode(pair.public_key()) },
    });
    let mut claims = json!({ "htm": method, "htu": htu, "jti": jti, "iat": now });
    if let Some(token) = access_token {
        claims["ath"] = access_token_hash(token).into();
    }
    if let Some(pin) = pin {
        claims["pin"] = pin.into();
    }
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature = URL_SAFE_NO_PAD.encode(pair.sign(input.as_bytes()));
    Some(format!("{input}.{signature}"))
}

/// Checks `proof` against `expected`, in this order: presence, form, key, method, URL, token hash, then time.
pub fn verify(proof: &str, expected: &Expected<'_>) -> Result<Verified, Failure> {
    let proof = proof.trim();
    if proof.is_empty() {
        return Err(Failure::MissingProof);
    }
    let mut parts = proof.split('.');
    let (Some(header), Some(claims), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Failure::MalformedProof);
    };
    if header.is_empty() || claims.is_empty() || signature.is_empty() {
        return Err(Failure::MalformedProof);
    }
    let parsed: Header = decode(header).ok_or(Failure::MalformedProof)?;
    let claims_parsed: Claims = decode(claims).ok_or(Failure::MalformedProof)?;
    if parsed.typ != TYP
        || parsed.alg != ALG
        || parsed.jwk.kty != KTY
        || parsed.jwk.crv != CRV
        || parsed.jwk.d.is_some()
        || claims_parsed.htm.is_empty()
        || claims_parsed.htu.is_empty()
        || claims_parsed.jti.is_empty()
    {
        return Err(Failure::MalformedProof);
    }
    let key: [u8; 32] = URL_SAFE_NO_PAD
        .decode(&parsed.jwk.x)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(Failure::MalformedProof)?;

    let thumbprint = thumbprint(&key);
    if expected.thumbprint.is_some_and(|want| want != thumbprint) {
        return Err(Failure::KeyMismatch);
    }
    if !claims_parsed.htm.eq_ignore_ascii_case(expected.method) {
        return Err(Failure::MethodMismatch);
    }
    if normalize_htu(expected.url).as_deref() != Some(claims_parsed.htu.as_str()) {
        return Err(Failure::UrlMismatch);
    }
    if let Some(token) = expected.access_token
        && claims_parsed.ath.as_deref() != Some(access_token_hash(token).as_str())
    {
        return Err(Failure::AccessTokenHashMismatch);
    }
    if let Some(pin) = expected.pin
        && claims_parsed
            .pin
            .as_deref()
            .is_some_and(|named| named != pin)
    {
        return Err(Failure::PinMismatch);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| Failure::InvalidSignature)?;
    let input = &proof[..header.len() + 1 + claims.len()];
    UnparsedPublicKey::new(&ED25519, key)
        .verify(input.as_bytes(), &signature)
        .map_err(|_| Failure::InvalidSignature)?;
    if expected.now.abs_diff(claims_parsed.iat) > MAX_CLOCK_SKEW as u64 {
        return Err(Failure::TimeWindow);
    }
    Ok(Verified {
        thumbprint,
        key,
        jti: claims_parsed.jti,
        iat: claims_parsed.iat,
    })
}

fn decode<T: for<'de> Deserialize<'de>>(part: &str) -> Option<T> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7; 32];
    const OTHER: [u8; 32] = [9; 32];
    const URL: &str = "https://devbox.example.ts.net:7466/token";
    const NOW: i64 = 1_800_000_000;

    fn public(seed: &[u8; 32]) -> [u8; 32] {
        let pair = Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
        pair.public_key().as_ref().try_into().unwrap()
    }

    fn expected<'a>(thumb: &'a str, token: Option<&'a str>) -> Expected<'a> {
        Expected {
            method: "POST",
            url: URL,
            now: NOW,
            thumbprint: Some(thumb),
            access_token: token,
            pin: None,
        }
    }

    /// A proof with its header or claims changed, signed again with SEED.
    fn forged(header: serde_json::Value, claims: serde_json::Value) -> String {
        let pair = Ed25519KeyPair::from_seed_unchecked(&SEED).unwrap();
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = URL_SAFE_NO_PAD.encode(pair.sign(input.as_bytes()));
        format!("{input}.{signature}")
    }

    fn jwk() -> serde_json::Value {
        json!({ "kty": "OKP", "crv": "Ed25519", "x": URL_SAFE_NO_PAD.encode(public(&SEED)) })
    }

    fn claims() -> serde_json::Value {
        json!({ "htm": "POST", "htu": URL, "jti": "one", "iat": NOW })
    }

    #[test]
    fn a_signed_proof_verifies_and_names_its_key() {
        let thumb = thumbprint(&public(&SEED));
        let proof = sign(&SEED, "POST", URL, Some("tok"), "one", NOW).unwrap();
        let verified = verify(&proof, &expected(&thumb, Some("tok"))).unwrap();
        assert_eq!(verified.thumbprint, thumb);
        assert_eq!(verified.key, public(&SEED));
        assert_eq!(verified.jti, "one");
        assert_eq!(verified.iat, NOW);

        // The method's case, the query and the fragment don't count.
        let proof = sign(
            &SEED,
            "post",
            "https://devbox.example.ts.net:7466/token?a=1#b",
            None,
            "two",
            NOW,
        )
        .unwrap();
        assert!(verify(&proof, &expected(&thumb, None)).is_ok());
        // A known key isn't needed to verify.
        let anyone = Expected {
            thumbprint: None,
            ..expected(&thumb, None)
        };
        assert!(verify(&proof, &anyone).is_ok());
    }

    #[test]
    fn a_proof_naming_a_pin_works_only_on_that_pin() {
        let thumb = thumbprint(&public(&SEED));
        let on = |pin| Expected {
            pin,
            ..expected(&thumb, None)
        };
        let named = |pin| sign_for_pin(&SEED, "POST", URL, None, pin, "one", NOW).unwrap();
        assert!(verify(&named(Some("ours")), &on(Some("ours"))).is_ok());
        assert_eq!(
            verify(&named(Some("theirs")), &on(Some("ours"))),
            Err(Failure::PinMismatch)
        );
        // An older phone's proof names none, and a verifier with no pin of
        // its own doesn't look.
        assert!(verify(&named(None), &on(Some("ours"))).is_ok());
        assert!(verify(&named(Some("theirs")), &on(None)).is_ok());
    }

    #[test]
    fn the_thumbprint_is_rfc_7638s() {
        // RFC 8037's appendix A.3: the thumbprint of its example key.
        let x = URL_SAFE_NO_PAD
            .decode("11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo")
            .unwrap();
        assert_eq!(
            thumbprint(&x.try_into().unwrap()),
            "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k"
        );
    }

    #[test]
    fn each_mismatch_fails_with_its_code() {
        let thumb = thumbprint(&public(&SEED));
        let proof = |method: &str, url: &str, token: Option<&str>, at: i64| {
            sign(&SEED, method, url, token, "one", at).unwrap()
        };
        let check = |proof: String, want: Option<&str>| verify(&proof, &expected(&thumb, want));

        assert_eq!(
            check(proof("GET", URL, None, NOW), None),
            Err(Failure::MethodMismatch)
        );
        assert_eq!(
            check(
                proof("POST", "https://devbox.example.ts.net:7466/pair", None, NOW),
                None
            ),
            Err(Failure::UrlMismatch)
        );
        assert_eq!(
            check(proof("POST", URL, None, NOW - MAX_CLOCK_SKEW - 1), None),
            Err(Failure::TimeWindow)
        );
        assert_eq!(
            check(proof("POST", URL, None, NOW + MAX_CLOCK_SKEW + 1), None),
            Err(Failure::TimeWindow)
        );
        assert!(check(proof("POST", URL, None, NOW - MAX_CLOCK_SKEW), None).is_ok());
        // A time so far off that the difference doesn't fit, and wraps.
        assert_eq!(
            check(proof("POST", URL, None, NOW.wrapping_add(i64::MIN)), None),
            Err(Failure::TimeWindow)
        );
        assert_eq!(
            check(proof("POST", URL, Some("other"), NOW), Some("tok")),
            Err(Failure::AccessTokenHashMismatch)
        );
        assert_eq!(
            check(proof("POST", URL, None, NOW), Some("tok")),
            Err(Failure::AccessTokenHashMismatch)
        );
        let other = sign(&OTHER, "POST", URL, None, "one", NOW).unwrap();
        assert_eq!(check(other, None), Err(Failure::KeyMismatch));
        assert_eq!(
            verify(" ", &expected(&thumb, None)),
            Err(Failure::MissingProof)
        );
        assert_eq!(
            verify("a.b", &expected(&thumb, None)),
            Err(Failure::MalformedProof)
        );
    }

    #[test]
    fn a_bad_signature_fails() {
        let thumb = thumbprint(&public(&SEED));
        let proof = sign(&SEED, "POST", URL, None, "one", NOW).unwrap();
        let (input, _) = proof.rsplit_once('.').unwrap();
        let other = Ed25519KeyPair::from_seed_unchecked(&OTHER).unwrap();
        let wrong = format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(other.sign(input.as_bytes()))
        );
        assert_eq!(
            verify(&wrong, &expected(&thumb, None)),
            Err(Failure::InvalidSignature)
        );
        // Claims changed after signing.
        let (header, rest) = proof.split_once('.').unwrap();
        let (_, signature) = rest.split_once('.').unwrap();
        let claims = URL_SAFE_NO_PAD
            .encode(json!({ "htm": "POST", "htu": URL, "jti": "two", "iat": NOW }).to_string());
        let changed = format!("{header}.{claims}.{signature}");
        assert_eq!(
            verify(&changed, &expected(&thumb, None)),
            Err(Failure::InvalidSignature)
        );
    }

    #[test]
    fn only_a_dpop_jwt_with_an_ed25519_key_is_read() {
        let thumb = thumbprint(&public(&SEED));
        let check = |header| verify(&forged(header, claims()), &expected(&thumb, None));
        assert!(check(json!({ "typ": "dpop+jwt", "alg": "EdDSA", "jwk": jwk() })).is_ok());
        assert_eq!(
            check(json!({ "typ": "dpop+jwt", "alg": "ES256", "jwk": jwk() })),
            Err(Failure::MalformedProof)
        );
        assert_eq!(
            check(json!({ "typ": "JWT", "alg": "EdDSA", "jwk": jwk() })),
            Err(Failure::MalformedProof)
        );
        let mut private = jwk();
        private["d"] = json!("secret");
        assert_eq!(
            check(json!({ "typ": "dpop+jwt", "alg": "EdDSA", "jwk": private })),
            Err(Failure::MalformedProof)
        );
        let mut ec = jwk();
        ec["kty"] = json!("EC");
        assert_eq!(
            check(json!({ "typ": "dpop+jwt", "alg": "EdDSA", "jwk": ec })),
            Err(Failure::MalformedProof)
        );
    }
}
