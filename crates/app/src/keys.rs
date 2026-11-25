// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where the apps keep their secret keys. The phone keeps an ed25519 seed
//! for SSH, an X25519 private key that devices seal pushes to and each
//! device's token for network access, in its Keychain, on this phone only,
//! readable after the first unlock so it can reconnect in the background and
//! its notification extension can open a push. The Mac keeps its app key,
//! the ed25519 seed it proves itself to each `maxd` with, in the login
//! keychain, where only the app that made it reads it without a prompt.
//! An Android phone keeps the same keys as files its Kotlin seals under an
//! Android Keystore key, out of backups. Tests keep them in memory.

use std::collections::BTreeMap;
use std::sync::Mutex;

use maxhq_protocol::ids::DeviceId;
use zeroize::Zeroizing;

/// One of the app's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    /// The ed25519 seed the phone logs in to devices with.
    Ssh,
    /// The X25519 private key devices seal the phone's pushes to.
    Push,
    /// A device's token for the phone's network access.
    Token(DeviceId),
    /// The ed25519 seed the Mac app proves itself to each `maxd` with,
    /// followed by the one before it while devices move to a new one.
    App,
}

impl Key {
    /// The name the key goes by in the system's store: its Keychain
    /// account, or its file name on Android, where the push service reads
    /// the push key by this name too.
    pub fn account(self) -> String {
        match self {
            Key::Ssh => "ssh-ed25519".to_owned(),
            Key::Push => "x25519".to_owned(),
            Key::Token(device) => format!("token-{device}"),
            Key::App => "ed25519".to_owned(),
        }
    }

    /// What the key is, for an error.
    pub fn what(self) -> &'static str {
        match self {
            Key::Ssh | Key::Push => "the phone's key",
            Key::Token(_) => "a device's token",
            Key::App => "this Mac's app key",
        }
    }
}

/// Where the app keeps its keys.
pub trait KeyStore: Send + Sync {
    /// The key, or None when there's none yet.
    fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String>;

    /// Keeps `bytes` as the key, in place of any before it.
    fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String>;

    /// Drops the key, if there is one.
    fn forget(&self, key: Key) -> Result<(), String>;
}

/// The key in `store`, 32 random bytes made and kept there the first time.
pub fn seed(store: &dyn KeyStore, key: Key) -> Result<Zeroizing<[u8; 32]>, String> {
    let mut seed = Zeroizing::new([0u8; 32]);
    match store.load(key)? {
        Some(kept) if kept.len() == seed.len() => seed.copy_from_slice(&kept),
        Some(_) => return Err("The phone's key in the Keychain isn't one MaxHQ made".into()),
        None => {
            getrandom::fill(seed.as_mut_slice())
                .map_err(|error| format!("Couldn't make the phone's keys: {error}"))?;
            store.save(key, seed.as_slice())?;
        }
    }
    Ok(seed)
}

/// A store in memory, for tests.
#[derive(Default)]
pub struct MemoryStore(Mutex<BTreeMap<Key, Zeroizing<Vec<u8>>>>);

impl KeyStore for MemoryStore {
    fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self.lock().get(&key).cloned())
    }

    fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
        self.lock().insert(key, Zeroizing::new(bytes.to_vec()));
        Ok(())
    }

    fn forget(&self, key: Key) -> Result<(), String> {
        self.lock().remove(&key);
        Ok(())
    }
}

impl MemoryStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<Key, Zeroizing<Vec<u8>>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The Keychain, each key a generic password under its service name.
///
/// The phone's keys set no access group, so each goes in the first one the
/// app's entitlements list, the group the notification extension shares,
/// and they're readable after the first unlock, on this phone only.
///
/// The Mac's app key sets no access control and no access group, so it goes
/// in the login keychain, whose default rule trusts only the app that made
/// the item: the signed app reads it without a prompt, and anything else
/// asks for the password.
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub struct Keychain;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod keychain {
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::passwords::{
        delete_generic_password_options, generic_password, set_generic_password_options,
        PasswordOptions,
    };
    use zeroize::Zeroizing;

    use super::{Key, KeyStore, Keychain};

    /// The phone's keys and tokens.
    const PHONE: &str = "maxhq.phone-key";

    /// The Mac app's key.
    const APP: &str = "maxhq.app-key";

    /// `errSecItemNotFound`: there's no key yet.
    const NOT_FOUND: i32 = -25300;

    fn options(key: Key) -> PasswordOptions {
        let service = match key {
            Key::App => APP,
            Key::Ssh | Key::Push | Key::Token(_) => PHONE,
        };
        PasswordOptions::new_generic_password(service, &key.account())
    }

    impl KeyStore for Keychain {
        fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            match generic_password(options(key)) {
                Ok(seed) => Ok(Some(Zeroizing::new(seed))),
                Err(error) if error.code() == NOT_FOUND => Ok(None),
                Err(error) => Err(format!("Couldn't read {}: {error}", key.what())),
            }
        }

        fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
            let kept = |error| format!("Couldn't keep {}: {error}", key.what());
            let mut options = options(key);
            if key != Key::App {
                let access = SecAccessControl::create_with_protection(
                    Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
                    0,
                )
                .map_err(kept)?;
                options.set_access_control(access);
                options.set_access_synchronized(Some(false));
            }
            set_generic_password_options(bytes, options).map_err(kept)
        }

        fn forget(&self, key: Key) -> Result<(), String> {
            match delete_generic_password_options(options(key)) {
                Err(error) if error.code() != NOT_FOUND => {
                    Err(format!("Couldn't forget {}: {error}", key.what()))
                }
                _ => Ok(()),
            }
        }
    }
}

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::AndroidStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_is_made_once_and_kept() {
        let store = MemoryStore::default();
        let ssh = seed(&store, Key::Ssh).unwrap();
        let push = seed(&store, Key::Push).unwrap();
        assert_ne!(*ssh, [0u8; 32]);
        assert_ne!(*ssh, *push);
        assert_eq!(*seed(&store, Key::Ssh).unwrap(), *ssh);
        assert_eq!(*seed(&store, Key::Push).unwrap(), *push);
    }

    #[test]
    fn a_key_of_the_wrong_size_is_refused() {
        let store = MemoryStore::default();
        store.save(Key::Push, &[1, 2, 3]).unwrap();
        assert!(seed(&store, Key::Push).is_err());
    }

    #[test]
    fn the_app_key_sits_apart_from_the_phone_keys() {
        let store = MemoryStore::default();
        let ssh = seed(&store, Key::Ssh).unwrap();
        store.save(Key::App, &[9; 32]).unwrap();
        assert_eq!(*seed(&store, Key::Ssh).unwrap(), *ssh);
        assert_eq!(store.load(Key::App).unwrap().unwrap().as_slice(), &[9; 32]);
        store.forget(Key::App).unwrap();
        assert_eq!(store.load(Key::App).unwrap(), None);
    }

    /// The Keychain items and Android files keep these names, and the
    /// Android push service reads the push key by its name, so they can't
    /// change.
    #[test]
    fn each_key_keeps_its_account_name() {
        let device = DeviceId::derive("devbox");
        assert_eq!(Key::Ssh.account(), "ssh-ed25519");
        assert_eq!(Key::Push.account(), "x25519");
        assert_eq!(Key::App.account(), "ed25519");
        assert_eq!(Key::Token(device).account(), format!("token-{device}"));
        let token = Key::Token(device).account();
        assert!(
            token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "{token} is a file name on Android"
        );
    }
}
