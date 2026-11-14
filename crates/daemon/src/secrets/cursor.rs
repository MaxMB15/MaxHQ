// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's Cursor API key. It gives this device a key account, which
//! the holder launches with `CURSOR_API_KEY`. A device keeps one key, so a
//! second one replaces the first. The Mac uses the CLI's own login instead.

use std::sync::{Mutex, MutexGuard, PoisonError};

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, CredentialId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Credential, CredentialKind, LoginKind, Vendor};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::{login_account, named, pasted, refused, Secrets, CURSOR_FILE, TOO_LONG};

/// The name a key gets when the user gives none.
const DEFAULT_LABEL: &str = "API key";
/// The longest key taken. Real ones are well under this.
const MAX_KEY: usize = 4096;
/// Where Cursor keeps its config inside containers.
const CONTAINER_CONFIG: &str = crate::recipe::CURSOR_CONFIG;

/// The key the store holds, if any.
#[derive(Default)]
pub struct Key(Mutex<Option<Stored>>);

impl Key {
    fn lock(&self) -> MutexGuard<'_, Option<Stored>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The key, as its file holds it.
#[derive(Serialize, Deserialize)]
struct Stored {
    label: String,
    stored_at: Timestamp,
    key: Secret,
}

/// The key account a Cursor key gives `device`.
pub(super) fn key_account(device: DeviceId) -> AccountId {
    names::account(device, Vendor::Cursor, "key")
}

/// The credential for the Cursor key on `device`.
fn credential_id(device: DeviceId) -> CredentialId {
    CredentialId::derive(&format!("cursor-key:{device}"))
}

impl Secrets {
    /// Answers `StoreCursorKey`.
    pub(super) fn store_cursor_key(&self, label: String, key: Secret) -> ActionOutcome {
        let Some(key) = pasted(key, MAX_KEY) else {
            return refused(
                "That isn't a Cursor API key. Paste the key from your Cursor dashboard, and nothing else.",
            );
        };
        let Some(label) = named(&label, DEFAULT_LABEL) else {
            return refused(TOO_LONG);
        };
        let stored = Stored {
            label,
            stored_at: self.now(),
            key,
        };
        let Ok(text) = serde_json::to_vec(&stored).map(Zeroizing::new) else {
            return refused("This device couldn't store the key.");
        };

        let mut held = self.inner.cursor.lock();
        if let Err(err) = self.inner.folder.write(CURSOR_FILE, &text) {
            warn!("couldn't store a Cursor API key: {err}");
            return refused(format!("This device couldn't store the key: {err}"));
        }
        let account = self.publish_key(&stored);
        *held = Some(stored);
        info!(%account, "stored a Cursor API key");
        ActionOutcome::Done
    }

    /// The key behind the key account.
    pub(super) fn api_key(&self, account: AccountId) -> Option<Secret> {
        if account != key_account(self.device()) {
            return None;
        }
        let held = self.inner.cursor.lock();
        held.as_ref().map(|stored| stored.key.clone())
    }

    /// Reads the key the folder holds, and publishes its account.
    pub(super) fn load_cursor(&self) {
        let text = match self.inner.folder.read(CURSOR_FILE) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(err) => {
                warn!("couldn't read the stored Cursor API key: {err}");
                return;
            }
        };
        // The reason never quotes the file.
        let Ok(stored) = serde_json::from_str::<Stored>(text.expose()) else {
            warn!("skipped {CURSOR_FILE}: it doesn't hold a stored Cursor API key");
            return;
        };
        self.publish_key(&stored);
        *self.inner.cursor.lock() = Some(stored);
    }

    /// Publishes the key's account and credential, and returns the account.
    fn publish_key(&self, stored: &Stored) -> AccountId {
        let device = self.device();
        let account = key_account(device);
        let credential = Credential {
            id: credential_id(device),
            device,
            kind: CredentialKind::CursorKey {
                account,
                label: stored.label.clone(),
            },
            stored_at: stored.stored_at,
            problem: None,
        };
        let shown = login_account(
            account,
            device,
            Vendor::Cursor,
            stored.label.clone(),
            LoginKind::CursorApiKey,
            CONTAINER_CONFIG.to_owned(),
        );
        self.publish(credential, shown);
        account
    }

    /// Forgets the key behind `credential`, with its account. Returns none
    /// when `credential` isn't the key's.
    pub(super) fn forget_cursor(&self, credential: CredentialId) -> Option<ActionOutcome> {
        let device = self.device();
        if credential != credential_id(device) {
            return None;
        }
        let mut held = self.inner.cursor.lock();
        held.as_ref()?;
        if let Err(err) = self.inner.folder.remove(CURSOR_FILE) {
            warn!("couldn't delete the Cursor API key: {err}");
            return Some(refused(format!(
                "This device couldn't delete the key: {err}"
            )));
        }
        *held = None;
        let account = key_account(device);
        self.unpublish(credential, account);
        info!(%account, "forgot the Cursor API key");
        Some(ActionOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::secrets::testing::{account, credentials, hub};
    use crate::secrets::{DIR, MAX_LABEL};
    use crate::session::holders::Credentials;
    use crate::testing::TempDir;

    /// A key's shape, with nothing real in it.
    const KEY: &str = "key_test-not-a-real-cursor-key-AAAAAAAAAAAAAAAA";

    fn open(temp: &TempDir) -> Secrets {
        Secrets::open(temp.path().join(DIR), hub()).unwrap()
    }

    #[test]
    fn stores_a_key_and_publishes_its_account() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let (head, tail) = KEY.split_at(20);
        let wrapped = Secret::new(format!(" {head}\n {tail}\n"));
        assert_eq!(
            secrets.store_cursor_key(" Personal ".into(), wrapped),
            ActionOutcome::Done
        );

        let id = key_account(secrets.device());
        let account = account(&secrets, id).unwrap();
        assert_eq!(account.vendor, Vendor::Cursor);
        assert_eq!(account.login, LoginKind::CursorApiKey);
        assert_eq!(account.label, "Personal");
        assert_eq!(account.origin, None);
        assert_eq!(account.config_dir, CONTAINER_CONFIG);

        let [credential] = &credentials(&secrets)[..] else {
            panic!("one credential");
        };
        assert_eq!(
            credential.kind,
            CredentialKind::CursorKey {
                account: id,
                label: "Personal".into(),
            }
        );
        assert_eq!(secrets.cursor_key(id).unwrap().expose(), KEY, "joined");
        assert!(secrets.cursor_key(AccountId::derive("other")).is_none());
        assert!(secrets.claude_token(id).is_none());

        let file = temp.path().join(DIR).join(CURSOR_FILE);
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn refuses_what_isnt_a_key() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        for bad in ["", " \n", "key_\0test", "key_tést"] {
            let outcome = secrets.store_cursor_key(String::new(), Secret::new(bad.to_owned()));
            assert!(matches!(outcome, ActionOutcome::Refused { .. }), "{bad:?}");
        }
        let long = "a".repeat(MAX_LABEL + 1);
        let outcome = secrets.store_cursor_key(long, Secret::new(KEY.to_owned()));
        assert!(matches!(outcome, ActionOutcome::Refused { .. }));
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.inner.folder.names().unwrap().is_empty());
    }

    #[test]
    fn a_second_key_replaces_the_first_and_comes_back_on_open() {
        let temp = TempDir::new();
        let id = {
            let secrets = open(&temp);
            secrets.store_cursor_key(String::new(), Secret::new("key_first".into()));
            secrets.store_cursor_key("Work".into(), Secret::new("key_second".into()));
            let id = key_account(secrets.device());
            assert_eq!(credentials(&secrets).len(), 1);
            id
        };
        let secrets = open(&temp);
        assert_eq!(secrets.cursor_key(id).unwrap().expose(), "key_second");
        assert_eq!(account(&secrets, id).unwrap().label, "Work");
        assert_eq!(credentials(&secrets).len(), 1);
    }

    #[test]
    fn a_key_with_no_name_gets_one() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        secrets.store_cursor_key("  ".into(), Secret::new(KEY.into()));
        let id = key_account(secrets.device());
        assert_eq!(account(&secrets, id).unwrap().label, DEFAULT_LABEL);
    }

    #[test]
    fn a_bad_file_is_skipped() {
        let temp = TempDir::new();
        drop(open(&temp));
        fs::write(temp.path().join(DIR).join(CURSOR_FILE), "not json").unwrap();
        let secrets = open(&temp);
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.cursor_key(key_account(secrets.device())).is_none());
    }

    #[test]
    fn forgetting_the_key_drops_its_account() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        secrets.store_cursor_key(String::new(), Secret::new(KEY.into()));
        let id = key_account(secrets.device());
        let credential = credential_id(secrets.device());

        assert_eq!(secrets.forget_file(credential), ActionOutcome::Done);
        assert!(secrets.cursor_key(id).is_none());
        assert!(account(&secrets, id).is_none());
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.inner.folder.names().unwrap().is_empty());
        assert!(matches!(
            secrets.forget_file(credential),
            ActionOutcome::Refused { .. }
        ));
        assert!(open(&temp).cursor_key(id).is_none());
    }
}
