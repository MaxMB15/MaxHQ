// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Follows the behaviour of MIT-licensed code listed in THIRD_PARTY_NOTICES:
// where the signature sits and how it's written. ring checks the digest in
// constant time.

//! Checks a webhook request's HMAC-SHA256 signature against the task's
//! signing secret.

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_protocol::tasks::{DigestEncoding, SignatureCheck};
use ring::hmac;

/// Whether `headers` carry a signature of `body` made with `secret`, in
/// the header, encoding and prefix `check` names. The prefix, such as
/// GitHub's `sha256=`, matches in any case. An empty secret signs
/// nothing, since anyone can sign with it.
pub fn verify(
    check: &SignatureCheck,
    secret: &[u8],
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some(value) = headers.get(&check.header.to_ascii_lowercase()) else {
        return false;
    };
    let value = value.trim();
    let n = check.prefix.len();
    if !value
        .get(..n)
        .is_some_and(|head| head.eq_ignore_ascii_case(&check.prefix))
    {
        return false;
    }
    let candidate = &value[n..];
    let tag = match check.encoding {
        DigestEncoding::Hex => from_hex(candidate),
        DigestEncoding::Base64 => STANDARD.decode(candidate).ok(),
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    tag.is_some_and(|tag| hmac::verify(&key, body, &tag).is_ok())
}

/// The bytes `text` writes in hex, in either case.
fn from_hex(text: &str) -> Option<Vec<u8>> {
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 4231, test case 2.
    const SECRET: &[u8] = b"Jefe";
    const BODY: &[u8] = b"what do ya want for nothing?";
    const HEX: &str = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
    const BASE64: &str = "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM=";

    fn github() -> SignatureCheck {
        SignatureCheck {
            header: "X-Hub-Signature-256".into(),
            encoding: DigestEncoding::Hex,
            prefix: "sha256=".into(),
            has_secret: true,
        }
    }

    fn headers(name: &str, value: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(name.to_owned(), value.to_owned())])
    }

    #[test]
    fn accepts_a_hex_signature_after_its_prefix_in_any_case() {
        let sent = headers(
            "x-hub-signature-256",
            &format!(" SHA256={} ", HEX.to_uppercase()),
        );
        assert!(verify(&github(), SECRET, &sent, BODY));
    }

    #[test]
    fn rejects_a_changed_body_another_secret_no_header_and_no_prefix() {
        let sent = headers("x-hub-signature-256", &format!("sha256={HEX}"));
        assert!(!verify(
            &github(),
            SECRET,
            &sent,
            b"what do ya want for something?"
        ));
        assert!(!verify(&github(), b"other", &sent, BODY));
        assert!(!verify(&github(), SECRET, &BTreeMap::new(), BODY));
        assert!(!verify(
            &github(),
            SECRET,
            &headers("x-hub-signature-256", HEX),
            BODY
        ));
        assert!(!verify(
            &github(),
            SECRET,
            &headers("x-hub-signature-256", "sha256=zz"),
            BODY
        ));
    }

    #[test]
    fn an_empty_secret_signs_nothing() {
        let key = hmac::Key::new(hmac::HMAC_SHA256, b"");
        let tag = hmac::sign(&key, BODY);
        let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let sent = headers("x-hub-signature-256", &format!("sha256={hex}"));
        assert!(!verify(&github(), b"", &sent, BODY));
    }

    #[test]
    fn accepts_a_base64_signature_without_a_prefix() {
        let check = SignatureCheck {
            header: "X-Signature".into(),
            encoding: DigestEncoding::Base64,
            prefix: String::new(),
            has_secret: true,
        };
        assert!(verify(
            &check,
            SECRET,
            &headers("x-signature", BASE64),
            BODY
        ));
        assert!(!verify(&check, SECRET, &headers("x-signature", HEX), BODY));
    }
}
