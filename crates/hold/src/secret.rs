// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The pipe that carries the holder's key and secret values to it, and the
//! key itself.
//!
//! The spawner writes the key and then each secret variable on the
//! holder's stdin and closes it. The holder reads the lot once at start,
//! keeps the key in memory, checks the names against `launch.json` and sets
//! the values in the CLI's environment only. Each entry is its name's
//! length as 4 bytes, big-endian, then the name, then the same for the
//! value. The key's name is empty, which no variable's can be. No error
//! here repeats a value.
//!
//! The key is 32 random bytes as hex. The spawner also writes it to
//! `holder.key`, so a client that starts later, such as `maxd` after a
//! restart, can show it in its hello.

use std::borrow::Cow;
use std::ffi::OsString;
use std::io;

use subtle::ConstantTimeEq;

pub use maxhq_protocol::Secret;

/// The most the secret pipe carries, names included.
pub(crate) const PIPE_CAP: usize = 1024 * 1024;

/// How many random bytes make a key.
const KEY_BYTES: usize = 32;

/// What the pipe carries: the holder's key, when it has one, and the
/// secret variables, each with its name.
pub(crate) type Piped = (Option<Secret>, Vec<(OsString, Secret)>);

/// Bytes that are zeroed when they're dropped, for the pipe's contents.
pub(crate) struct Zeroed(pub Vec<u8>);

impl Drop for Zeroed {
    fn drop(&mut self) {
        zero(std::mem::take(&mut self.0));
    }
}

/// Overwrites `bytes` before they're freed. `black_box` keeps the compiler
/// from dropping the writes as dead.
pub(crate) fn zero(mut bytes: Vec<u8>) {
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

/// A new key for a holder.
pub(crate) fn new_key() -> io::Result<Secret> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut raw = Zeroed(vec![0; KEY_BYTES]);
    getrandom::fill(&mut raw.0).map_err(io::Error::other)?;
    let mut text = String::with_capacity(KEY_BYTES * 2);
    for byte in &raw.0 {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(Secret::new(text))
}

/// A new nonce for a shell's marks: 32 hex digits.
pub(crate) fn new_nonce() -> io::Result<String> {
    new_key().map(|key| key.expose()[..32].to_owned())
}

/// Whether `shown` is `key`. It takes as long however much of the two
/// match, so a client can't find the key a byte at a time.
pub(crate) fn key_matches(key: &Secret, shown: &Secret) -> bool {
    bool::from(key.expose().as_bytes().ct_eq(shown.expose().as_bytes()))
}

/// The pipe's contents for `key`, when there is one, and `secrets`.
pub(crate) fn encode(
    key: Option<&Secret>,
    secrets: &[(OsString, Secret)],
) -> Result<Zeroed, &'static str> {
    let key = key.map(|key| (Cow::Borrowed(&[][..]), key));
    let secrets = secrets
        .iter()
        .map(|(name, value)| (crate::sys::os_bytes(name), value));
    let mut out = Zeroed(Vec::new());
    for (name, value) in key.into_iter().chain(secrets) {
        for part in [&name[..], value.expose().as_bytes()] {
            let len = u32::try_from(part.len()).map_err(|_| "a secret variable is too long")?;
            out.0.extend_from_slice(&len.to_be_bytes());
            out.0.extend_from_slice(part);
        }
    }
    if out.0.len() > PIPE_CAP {
        return Err("the secret variables come to more than 1 MiB");
    }
    Ok(out)
}

/// The key, when `keyed`, and the secret variables in the pipe's contents.
pub(crate) fn decode(mut bytes: &[u8], keyed: bool) -> Result<Piped, &'static str> {
    let take = |bytes: &mut &[u8]| -> Result<Zeroed, &'static str> {
        let cut_short = "the secret variables on stdin were cut short";
        let (len, rest) = bytes.split_first_chunk::<4>().ok_or(cut_short)?;
        let len = usize::try_from(u32::from_be_bytes(*len)).map_err(|_| cut_short)?;
        let part = rest.get(..len).ok_or(cut_short)?;
        *bytes = &rest[len..];
        Ok(Zeroed(part.to_vec()))
    };
    let mut entries = Vec::new();
    while !bytes.is_empty() {
        let name = take(&mut bytes)?;
        let value = take(&mut bytes)?;
        let value = std::str::from_utf8(&value.0).map_err(|_| "a secret value isn't UTF-8")?;
        let name = crate::sys::os_from_bytes(name.0.clone())
            .ok_or("a secret variable's name on stdin isn't one this OS takes")?;
        entries.push((name, Secret::new(value.to_owned())));
    }
    let key = if keyed {
        if entries.first().is_none_or(|(name, _)| !name.is_empty()) {
            return Err("the holder's key didn't come first on stdin");
        }
        Some(entries.remove(0).1)
    } else {
        None
    };
    if entries.iter().any(|(name, _)| name.is_empty()) {
        return Err("a secret variable on stdin has no name");
    }
    Ok((key, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Vec<(OsString, Secret)> {
        vec![
            (
                "CLAUDE_CODE_OAUTH_TOKEN".into(),
                Secret::new("test-not-a-real-token".into()),
            ),
            ("EMPTY".into(), Secret::new(String::new())),
        ]
    }

    #[test]
    fn secrets_survive_the_pipe() {
        let bytes = encode(None, &secrets()).unwrap();
        assert_eq!(decode(&bytes.0, false).unwrap(), (None, secrets()));
        assert_eq!(decode(&[], false).unwrap(), (None, Vec::new()));
    }

    #[test]
    fn the_key_goes_first_and_comes_back_apart() {
        let key = new_key().unwrap();
        for secrets in [Vec::new(), secrets()] {
            let bytes = encode(Some(&key), &secrets).unwrap();
            let (back, rest) = decode(&bytes.0, true).unwrap();
            assert_eq!(back.as_ref(), Some(&key));
            assert_eq!(rest, secrets);
            // A holder that wants a key won't take a pipe without one,
            // and one that doesn't won't take a nameless variable.
            let bare = encode(None, &secrets).unwrap();
            assert!(decode(&bare.0, true).is_err());
            assert!(decode(&bytes.0, false).is_err());
        }
    }

    #[test]
    fn keys_are_random_hex_and_match_only_themselves() {
        let one = new_key().unwrap();
        let two = new_key().unwrap();
        assert_eq!(one.expose().len(), 64);
        assert!(one.expose().bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(one, two);
        assert!(key_matches(&one, &one.clone()));
        assert!(!key_matches(&one, &two));
        assert!(!key_matches(&one, &Secret::new(String::new())));
        let short = Secret::new(one.expose()[..63].to_owned());
        assert!(!key_matches(&one, &short));
        assert_eq!(format!("{one:?}"), "<secret>");
    }

    #[test]
    fn a_cut_pipe_fails_without_showing_the_value() {
        let secrets = vec![("TOKEN".into(), Secret::new("test-not-a-real-token".into()))];
        let bytes = encode(None, &secrets).unwrap();
        for end in 1..bytes.0.len() {
            let err = decode(&bytes.0[..end], false).unwrap_err();
            assert!(!err.contains("test-not"), "{err}");
        }
        let mut bad = bytes.0.clone();
        let last = bad.len() - 1;
        bad[last] = 0xff;
        assert_eq!(
            decode(&bad, false).unwrap_err(),
            "a secret value isn't UTF-8"
        );
    }
}
