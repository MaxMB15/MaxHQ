// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where the app key lives on each system. A Mac keeps it in the Keychain.
//! Linux and Windows keep it in the system keyring: the Secret Service, or
//! Windows Credential Manager. When no keyring answers, the app asks before
//! it keeps the key in a file only you can read, since anything running as
//! you can read that file too. Until you choose, and after Not now, the key
//! lasts until the app quits.
//!
//! The key file's being there is the choice, so a choice that didn't reach
//! the database still holds. The database remembers Not now, so the app
//! asks once.

use std::io;
use std::path::{Path, PathBuf};

use maxhq_core::fs::{read_regular, write_atomic, Atomic};
use serde::{Deserialize, Serialize};
use specta::Type;
use zeroize::Zeroizing;

use crate::keys::{Key, KeyStore};
use crate::registry::Registry;

/// The setting the app keeps your choice of [`KeyHome`] under.
pub const HOME_SETTING: &str = "app_key_home";

/// The key file's name, in the app's data folder.
pub const FILE: &str = "app-key";

/// Where the app key lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum KeyHome {
    /// The Mac's Keychain.
    Keychain,
    /// The system keyring, on Linux and Windows.
    Keyring,
    /// A file only you can read, in the app's data folder.
    File,
    /// Nowhere: the key lasts until the app quits.
    Memory,
}

impl KeyHome {
    /// The store, as a reason names it.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Keychain => "the Keychain",
            Self::Keyring => "the keyring",
            Self::File => "the key file",
            Self::Memory => "memory",
        }
    }

    /// Why the app made a new key when the store had none, though the
    /// record names one.
    pub(super) fn lost(self) -> String {
        match self {
            Self::Memory => {
                "MaxHQ made a new app key, since no keyring answered to keep the one before".into()
            }
            home => format!(
                "MaxHQ made a new app key, since {} lost the one before",
                home.name()
            ),
        }
    }
}

/// Where the app key lives, as the key dialog and Settings show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct KeyHomeState {
    pub home: KeyHome,
    /// Whether to ask where to keep the key: no keyring answered as the app
    /// opened, and you haven't chosen yet.
    pub ask: bool,
    /// The file the key goes in when you choose one. None when the app has
    /// no data folder.
    pub file: Option<String>,
}

/// What reading the key from a store gave: the key, none yet, or why it
/// couldn't be read.
pub(crate) type Read = Result<Option<Zeroizing<Vec<u8>>>, String>;

/// The store the app key opens from, and what the app knows of it.
pub(crate) struct Place {
    pub(crate) store: Box<dyn KeyStore>,
    pub(crate) home: KeyHome,
    /// Whether to ask where to keep the key.
    pub(crate) ask: bool,
    /// Where the key file goes, when you choose one.
    pub(crate) file: Option<PathBuf>,
    /// What the store gave when the app asked whether it answers, so the
    /// key is read once. None when it wasn't read yet.
    pub(crate) read: Option<Read>,
}

impl Place {
    /// The Mac's Keychain, or any one store with no other to move to.
    pub(crate) fn only(store: Box<dyn KeyStore>, home: KeyHome) -> Self {
        Self {
            store,
            home,
            ask: false,
            file: None,
            read: None,
        }
    }
}

/// Where the key opens on a system with a keyring and a key file to choose
/// from: the file once you chose it, or while it's there; else the keyring,
/// when it answers; else memory, asking you unless you said Not now. The
/// keyring is asked only when the answer matters, since asking it may show
/// an unlock prompt.
#[cfg_attr(
    not(any(target_os = "linux", windows)),
    allow(dead_code, reason = "only Linux and Windows choose where the key goes")
)]
pub(crate) fn start(
    chosen: Option<KeyHome>,
    file: bool,
    keyring: impl FnOnce() -> bool,
) -> (KeyHome, bool) {
    if file || chosen == Some(KeyHome::File) {
        return (KeyHome::File, false);
    }
    if keyring() {
        return (KeyHome::Keyring, false);
    }
    (KeyHome::Memory, chosen != Some(KeyHome::Memory))
}

/// The store this system keeps the app key in, with the record's
/// `registry` for your choice and `data`, the app's data folder, for the
/// key file. None on a system with no store, so the app proves nothing.
#[cfg(target_os = "macos")]
pub(crate) fn place(_registry: &Registry, _data: Option<&Path>) -> Option<Place> {
    Some(Place::only(
        Box::new(crate::keys::Keychain),
        KeyHome::Keychain,
    ))
}

/// The store this system keeps the app key in, with the record's
/// `registry` for your choice and `data`, the app's data folder, for the
/// key file. None on a system with no store, so the app proves nothing.
#[cfg(any(target_os = "linux", windows))]
pub(crate) fn place(registry: &Registry, data: Option<&Path>) -> Option<Place> {
    let file = data.map(|data| data.join(FILE));
    let chosen = registry
        .setting::<KeyHome>(HOME_SETTING)
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "couldn't read where you chose to keep the app key");
            None
        })
        // A file chosen on a run that had a data folder needs one now.
        .filter(|home| *home != KeyHome::File || file.is_some());
    let there = file.as_deref().is_some_and(Path::exists);
    let mut read = None;
    let (home, ask) = start(chosen, there, || {
        read = os_keyring::Keyring.read();
        read.is_some()
    });
    let store: Box<dyn KeyStore> = match (home, &file) {
        (KeyHome::File, Some(file)) => Box::new(KeyFile::new(file.clone())),
        (KeyHome::Keyring, _) => Box::new(os_keyring::Keyring),
        _ => Box::new(Nowhere),
    };
    if home == KeyHome::Memory {
        tracing::warn!("no keyring answered, so this computer's app key lasts until the app quits");
    }
    Some(Place {
        store,
        home,
        ask: ask && file.is_some(),
        file,
        read,
    })
}

/// The store this system keeps the app key in. None here: this system has
/// no store yet, so the app proves nothing.
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(crate) fn place(_registry: &Registry, _data: Option<&Path>) -> Option<Place> {
    None
}

/// The system keyring, as a store to move the key to.
pub(crate) fn keyring() -> Result<Box<dyn KeyStore>, String> {
    #[cfg(any(target_os = "linux", windows))]
    {
        Ok(Box::new(os_keyring::Keyring))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err("This system has no keyring MaxHQ can keep its app key in".into())
    }
}

/// Saves where you chose to keep the key, so the app opens it from there
/// and doesn't ask again.
pub fn remember(registry: &Registry, home: KeyHome) -> Result<(), String> {
    registry
        .set_setting(HOME_SETTING, &home)
        .map_err(|error| format!("Couldn't save where the app key is kept: {error}"))
}

/// Keeps nothing, so the key lasts until the app quits.
#[cfg_attr(
    not(any(target_os = "linux", windows)),
    allow(
        dead_code,
        reason = "only Linux and Windows open with nowhere to keep the key"
    )
)]
pub(crate) struct Nowhere;

impl KeyStore for Nowhere {
    fn load(&self, _key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(None)
    }

    fn save(&self, _key: Key, _bytes: &[u8]) -> Result<(), String> {
        Err("No keyring answered, and you haven't chosen a key file".into())
    }

    fn forget(&self, _key: Key) -> Result<(), String> {
        Ok(())
    }
}

/// The app key in a file only you can read, written whole each time.
pub(crate) struct KeyFile(PathBuf);

impl KeyFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

/// The largest key file read, two seeds, and one byte more so a longer
/// one reads as not one MaxHQ made.
const FILE_LIMIT: u64 = 64;

impl KeyStore for KeyFile {
    fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        only_app(key)?;
        match read_regular(&self.0, FILE_LIMIT) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Couldn't read {}: {error}", self.0.display())),
        }
    }

    fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
        only_app(key)?;
        let kept = |error: io::Error| format!("Couldn't write {}: {error}", self.0.display());
        if let Some(folder) = self.0.parent() {
            std::fs::create_dir_all(folder).map_err(kept)?;
        }
        write_atomic(&self.0, bytes, Atomic::PRIVATE).map_err(kept)
    }

    fn forget(&self, key: Key) -> Result<(), String> {
        only_app(key)?;
        match std::fs::remove_file(&self.0) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Err(format!("Couldn't delete {}: {error}", self.0.display()))
            }
            _ => Ok(()),
        }
    }
}

/// Refuses every key but the app key, the only one the desktop app keeps
/// in these stores.
fn only_app(key: Key) -> Result<(), String> {
    match key {
        Key::App => Ok(()),
        _ => Err("Only the app key is kept here".into()),
    }
}

#[cfg(any(target_os = "linux", windows))]
mod os_keyring {
    use keyring::{Entry, Error};
    use zeroize::Zeroizing;

    use super::{only_app, Read};
    use crate::keys::{Key, KeyStore};

    /// The system keyring: the Secret Service on Linux, Credential Manager
    /// on Windows.
    pub(crate) struct Keyring;

    impl Keyring {
        /// The app key's entry, under the same service and account as on
        /// the Mac.
        fn entry() -> Result<Entry, String> {
            Entry::new(Key::App.service(), &Key::App.account())
                .map_err(|error| format!("Couldn't open the keyring: {error}"))
        }

        /// The app key, read as [`KeyStore::load`] reads it, when the
        /// keyring answers, with the key or without one. None when it
        /// doesn't: a keyring that's missing, or locked with its unlock
        /// turned down.
        pub(crate) fn read(&self) -> Option<Read> {
            match Self::entry().ok()?.get_secret() {
                Err(Error::PlatformFailure(error) | Error::NoStorageAccess(error)) => {
                    tracing::info!(%error, "the keyring didn't answer");
                    None
                }
                secret => Some(loaded(secret)),
            }
        }
    }

    /// The key from what the keyring gave, held so it's zeroed when
    /// dropped.
    fn loaded(secret: Result<Vec<u8>, Error>) -> Read {
        match secret {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("Couldn't read the app key in the keyring: {error}")),
        }
    }

    impl KeyStore for Keyring {
        fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
            only_app(key)?;
            loaded(Self::entry()?.get_secret())
        }

        fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
            only_app(key)?;
            Self::entry()?
                .set_secret(bytes)
                .map_err(|error| format!("Couldn't keep the app key in the keyring: {error}"))
        }

        fn forget(&self, key: Key) -> Result<(), String> {
            only_app(key)?;
            match Self::entry()?.delete_credential() {
                Ok(()) | Err(Error::NoEntry) => Ok(()),
                Err(error) => Err(format!(
                    "Couldn't delete the app key in the keyring: {error}"
                )),
            }
        }
    }
}
