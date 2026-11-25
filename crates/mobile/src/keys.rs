// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The phone's own keys: an ed25519 seed for SSH and an X25519 private key
//! that devices seal pushes to, each made once and kept in the Keychain,
//! or on Android under an Android Keystore key, on this phone only,
//! readable after the first unlock so the app can reconnect in the
//! background and the notification extension or push service can open a
//! push. Next to them sits each device's token for network access, which
//! only the app reads. Tests keep them in memory. The store itself lives
//! in [`maxhq_app::keys`], which the Mac app's key shares.

use std::fmt;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use curve25519_dalek::montgomery::MontgomeryPoint;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use maxhq_app::keys::Keychain;
pub use maxhq_app::keys::{seed, Key, KeyStore, MemoryStore};

/// A device's token for network access, and when it ends. Its `Debug`
/// never shows the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The bearer token the device gave.
    pub token: Zeroizing<String>,
    /// When the device stops taking it.
    pub expires: Timestamp,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("expires", &self.expires)
            .finish_non_exhaustive()
    }
}

/// Keeps `token` as `device`'s, in place of any before it.
pub fn save_token(store: &dyn KeyStore, device: DeviceId, token: &Token) -> Result<(), String> {
    let bytes = Zeroizing::new(
        serde_json::to_vec(token).map_err(|_| "Couldn't keep a device's token".to_owned())?,
    );
    store.save(Key::Token(device), &bytes)
}

/// `device`'s token, or None when the phone has none for it.
pub fn token(store: &dyn KeyStore, device: DeviceId) -> Result<Option<Token>, String> {
    let Some(bytes) = store.load(Key::Token(device))? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "A device's token in the Keychain isn't one MaxHQ made".to_owned())
}

/// Drops `device`'s token, as removing the device or pairing it again does.
pub fn forget_token(store: &dyn KeyStore, device: DeviceId) -> Result<(), String> {
    store.forget(Key::Token(device))
}

/// The public half of the X25519 key `private`, as base64, for a device
/// to seal pushes to.
pub fn push_public(private: &[u8; 32]) -> String {
    STANDARD.encode(MontgomeryPoint::mul_base_clamped(*private).to_bytes())
}

/// The system's store: the Keychain, or the Android Keystore through the
/// app's Kotlin, where the build has one.
pub(crate) fn system() -> Result<Box<dyn KeyStore>, maxhq_app::request::CommandError> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    return Ok(Box::new(Keychain));
    #[cfg(target_os = "android")]
    return Ok(Box::new(maxhq_app::keys::AndroidStore));
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    Err(maxhq_app::request::CommandError::Failed {
        message: "This build has no Keychain for the phone's key".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> DeviceId {
        DeviceId::derive(name)
    }

    fn kept(token: &str, expires: u64) -> Token {
        Token {
            token: Zeroizing::new(token.to_owned()),
            expires: Timestamp(expires),
        }
    }

    #[test]
    fn each_device_keeps_its_own_token_until_forgotten() {
        let store = MemoryStore::default();
        let (devbox, laptop) = (device("devbox"), device("laptop"));
        assert_eq!(token(&store, devbox).unwrap(), None);
        save_token(&store, devbox, &kept("secret-1", 10)).unwrap();
        save_token(&store, laptop, &kept("secret-2", 20)).unwrap();
        assert_eq!(token(&store, devbox).unwrap(), Some(kept("secret-1", 10)));
        save_token(&store, devbox, &kept("secret-3", 30)).unwrap();
        assert_eq!(token(&store, devbox).unwrap(), Some(kept("secret-3", 30)));
        forget_token(&store, devbox).unwrap();
        forget_token(&store, devbox).unwrap();
        assert_eq!(token(&store, devbox).unwrap(), None);
        assert_eq!(token(&store, laptop).unwrap(), Some(kept("secret-2", 20)));
        assert!(seed(&store, Key::Ssh).is_ok(), "the keys sit beside them");
    }

    #[test]
    fn a_token_never_shows_in_debug_or_errors() {
        let shown = format!("{:?}", kept("secret-1", 10));
        assert_eq!(shown, "Token { expires: Timestamp(10), .. }");
        let store = MemoryStore::default();
        let devbox = device("devbox");
        store.save(Key::Token(devbox), b"secret-1 cut sho").unwrap();
        let error = token(&store, devbox).unwrap_err();
        assert!(!error.contains("secret"), "{error}");
    }

    /// The public key matches the one maxd's push vectors seal to.
    #[test]
    fn the_push_key_is_the_one_devices_seal_to() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../daemon/fixtures/push-vectors.json")).unwrap();
        for vector in vectors["vectors"].as_array().unwrap() {
            let hex = |field: &str| -> Vec<u8> {
                let text = vector[field].as_str().unwrap();
                (0..text.len())
                    .step_by(2)
                    .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
                    .collect()
            };
            let private: [u8; 32] = hex("phone_private").try_into().unwrap();
            assert_eq!(push_public(&private), STANDARD.encode(hex("phone_public")));
        }
    }
}
