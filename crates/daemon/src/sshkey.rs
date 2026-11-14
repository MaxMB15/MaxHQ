// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! OpenSSH public key lines, as clients hand theirs over and guests report
//! their host keys. Only `ssh-ed25519` keys pass, and each comes back in
//! one form with no comment, so the same key always reads the same.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// The key type, as it starts a line and its blob.
const ED25519: &str = "ssh-ed25519";

/// The longest line read. An ed25519 line is about 80 bytes, so the room
/// left is for a comment.
pub const MAX_LINE: usize = 1024;

/// The line `text` holds as `ssh-ed25519 <base64>`, or none when it
/// isn't one ed25519 key. A comment after the key is dropped.
pub fn ed25519_line(text: &str) -> Option<String> {
    let key = ed25519_key(text)?;
    Some(format!("{ED25519} {}", STANDARD.encode(blob_of(&key))))
}

/// The 32 bytes of the one ed25519 key `text` holds, as
/// [`ed25519_line`] reads it.
pub fn ed25519_key(text: &str) -> Option<[u8; 32]> {
    if text.len() > MAX_LINE {
        return None;
    }
    let mut words = text.trim().split_ascii_whitespace();
    if words.next()? != ED25519 {
        return None;
    }
    let encoded = words.next()?;
    let blob = STANDARD.decode(encoded).ok()?;
    blob_key(&blob)
}

/// The 32 key bytes of an ed25519 public key blob.
fn blob_key(blob: &[u8]) -> Option<[u8; 32]> {
    let (name, rest) = string(blob)?;
    if name != ED25519.as_bytes() {
        return None;
    }
    let (key, rest) = string(rest)?;
    if !rest.is_empty() {
        return None;
    }
    key.try_into().ok()
}

/// The blob that holds `key`.
fn blob_of(key: &[u8; 32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(51);
    for part in [ED25519.as_bytes(), key.as_slice()] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    blob
}

/// One length-prefixed string off the front of `bytes`, and what follows.
fn string(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?);
    let len = usize::try_from(len).ok()?;
    let rest = &bytes[4..];
    (rest.len() >= len).then(|| rest.split_at(len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line for a made-up key, with every key byte `byte`.
    fn line(byte: u8) -> String {
        format!("{ED25519} {}", STANDARD.encode(blob_of(&[byte; 32])))
    }

    #[test]
    fn an_ed25519_line_comes_back_without_its_comment() {
        let key = line(7);
        assert_eq!(ed25519_line(&key).as_deref(), Some(key.as_str()));
        assert_eq!(ed25519_key(&key), Some([7; 32]));
        let commented = format!("  {key} dev@devbox\n");
        assert_eq!(ed25519_line(&commented).as_deref(), Some(key.as_str()));
    }

    #[test]
    fn anything_else_is_refused() {
        let key = line(7);
        let encoded = key.split_once(' ').unwrap().1;
        let mut long = blob_of(&[7; 32]);
        long.push(0);
        let mut renamed = blob_of(&[7; 32]);
        renamed[4] = b'x';
        for text in [
            String::new(),
            ED25519.to_owned(),
            format!("ssh-rsa {encoded}"),
            format!("{ED25519} not-base64!"),
            format!("{ED25519} {}", STANDARD.encode(&long)),
            format!("{ED25519} {}", STANDARD.encode(&renamed)),
            format!("{ED25519} {}", STANDARD.encode([0u8; 3])),
            format!("{key} {}", "x".repeat(MAX_LINE)),
        ] {
            assert_eq!(ed25519_line(&text), None, "{text:?}");
        }
    }
}
