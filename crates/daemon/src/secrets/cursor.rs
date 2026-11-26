// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's Cursor API key. It gives this device a key account, which
//! the holder launches with `CURSOR_API_KEY`. A device keeps one key, so a
//! second one replaces the first. The Mac uses the CLI's own login instead.
//! Once Cursor says whose the key is, the account carries that email, so
//! the app shows it with the same person's CLI login on a Mac.

mod owner;

use std::pin::pin;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_core::names;
use maxhq_protocol::ids::{AccountId, CredentialId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Credential, CredentialKind, LoginKind, Vendor};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{info, warn};
use zeroize::Zeroizing;

use self::owner::{Ask, Owner};
use super::{login_account, named, pasted, refused, Secrets, CURSOR_FILE, TOO_LONG};

/// The name a key gets when the user gives none.
const DEFAULT_LABEL: &str = "API key";
/// The longest key taken. Real ones are well under this.
const MAX_KEY: usize = 4096;
/// Where Cursor keeps its config inside containers.
const CONTAINER_CONFIG: &str = crate::recipe::CURSOR_CONFIG;
/// How long the first wait after a failed ask is. Each one after doubles.
const FIRST_WAIT: Duration = Duration::from_secs(60);
/// The longest wait between asks.
const MOST_WAIT: Duration = Duration::from_secs(60 * 60);

/// The key the store holds, if any.
#[derive(Default)]
pub struct Key {
    held: Mutex<Option<Held>>,
    /// Tells the owner check the key changed.
    changed: Notify,
}

impl Key {
    fn lock(&self) -> MutexGuard<'_, Option<Held>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The key, with whose it is once Cursor said. Each start asks again.
struct Held {
    stored: Stored,
    owner: Option<Owner>,
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
        // The same key under a new name still belongs to the same person.
        let owner = held
            .take()
            .filter(|held| held.stored.key == stored.key)
            .and_then(|held| held.owner);
        let account = self.publish_key(&stored, owner.as_ref());
        *held = Some(Held { stored, owner });
        drop(held);
        self.inner.cursor.changed.notify_waiters();
        info!(%account, "stored a Cursor API key");
        ActionOutcome::Done
    }

    /// The key behind the key account.
    pub(super) fn api_key(&self, account: AccountId) -> Option<Secret> {
        if account != key_account(self.device()) {
            return None;
        }
        let held = self.inner.cursor.lock();
        held.as_ref().map(|held| held.stored.key.clone())
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
        self.publish_key(&stored, None);
        // The owner check starts after this and asks first thing.
        *self.inner.cursor.lock() = Some(Held {
            stored,
            owner: None,
        });
    }

    /// Publishes the key's account, with its owner's email once known, and
    /// its credential. Returns the account.
    fn publish_key(&self, stored: &Stored, owner: Option<&Owner>) -> AccountId {
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
        let mut shown = login_account(
            account,
            device,
            Vendor::Cursor,
            stored.label.clone(),
            LoginKind::CursorApiKey,
            CONTAINER_CONFIG.to_owned(),
        );
        shown.email = owner.and_then(|owner| owner.email.clone());
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
        drop(held);
        self.inner.cursor.changed.notify_waiters();
        info!(%account, "forgot the Cursor API key");
        Some(ActionOutcome::Done)
    }

    /// Learns whose the key is from Cursor, in the background: once it's
    /// stored, once `maxd` starts with one, and after a failure again,
    /// waiting twice as long each time up to an hour.
    pub async fn check_cursor_key(self) {
        self.ask_cursor(owner::Live::default(), FIRST_WAIT).await;
    }

    async fn ask_cursor(&self, mut cursor: impl Ask, first_wait: Duration) {
        let mut failures = 0;
        loop {
            // Listening before the key is read hears every later change.
            let mut changed = pin!(self.inner.cursor.changed.notified());
            changed.as_mut().enable();
            let Some(key) = self.unasked_key() else {
                failures = 0;
                changed.await;
                continue;
            };
            let answer = tokio::select! {
                answer = cursor.owner(&key) => Some(answer),
                // The key changed or went, so its answer no longer counts.
                () = changed.as_mut() => None,
            };
            match answer {
                None => failures = 0,
                Some(Ok(owner)) => {
                    failures = 0;
                    self.learned(&key, owner);
                }
                Some(Err(reason)) => {
                    failures += 1;
                    let wait = backoff(first_wait, failures);
                    info!(%reason, ?wait, "couldn't learn whose the Cursor API key is");
                    tokio::select! {
                        () = changed => failures = 0,
                        () = tokio::time::sleep(wait) => {}
                    }
                }
            }
        }
    }

    /// The key, while Cursor hasn't said whose it is.
    fn unasked_key(&self) -> Option<Secret> {
        let held = self.inner.cursor.lock();
        held.as_ref()
            .filter(|held| held.owner.is_none())
            .map(|held| held.stored.key.clone())
    }

    /// Shows `owner` on the key's account, unless `key` was forgotten or
    /// replaced while Cursor answered.
    fn learned(&self, key: &Secret, owner: Owner) {
        let mut held = self.inner.cursor.lock();
        let Some(current) = held.as_mut().filter(|held| held.stored.key == *key) else {
            return;
        };
        let account = self.publish_key(&current.stored, Some(&owner));
        current.owner = Some(owner);
        info!(%account, "learned whose the Cursor API key is");
    }
}

/// How long to wait after `failures` failed asks in a row.
fn backoff(first: Duration, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(6);
    (first * 2u32.pow(doublings)).min(MOST_WAIT)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use tokio::sync::{mpsc, oneshot};
    use tokio::task::JoinHandle;
    use tokio::time::{sleep, timeout};

    use super::*;
    use crate::secrets::testing::{account, credentials, hub};
    use crate::secrets::{DIR, MAX_LABEL};
    use crate::session::holders::Credentials;
    use crate::testing::TempDir;

    /// A key's shape, with nothing real in it.
    const KEY: &str = "key_test-not-a-real-cursor-key-AAAAAAAAAAAAAAAA";
    /// Whose Cursor says the key is.
    const EMAIL: &str = "someone@example.com";
    /// How long a test waits for the check.
    const PATIENCE: Duration = Duration::from_secs(10);

    fn open(temp: &TempDir) -> Secrets {
        Secrets::open(temp.path().join(DIR), hub()).unwrap()
    }

    /// One ask: the key, and where its answer goes.
    type Asked = (String, oneshot::Sender<Result<Owner, String>>);

    /// Cursor, with the test answering each ask.
    struct Stub(mpsc::UnboundedSender<Asked>);

    impl Ask for Stub {
        async fn owner(&mut self, key: &Secret) -> Result<Owner, String> {
            let (reply, answer) = oneshot::channel();
            self.0
                .send((key.expose().to_owned(), reply))
                .expect("the test listens");
            answer
                .await
                .unwrap_or_else(|_| Err("the test didn't answer".into()))
        }
    }

    /// Runs the owner check against the stub, and hands back its asks.
    fn check(
        secrets: &Secrets,
        first_wait: Duration,
    ) -> (mpsc::UnboundedReceiver<Asked>, JoinHandle<()>) {
        let (asks, asked) = mpsc::unbounded_channel();
        let secrets = secrets.clone();
        let task = tokio::spawn(async move { secrets.ask_cursor(Stub(asks), first_wait).await });
        (asked, task)
    }

    async fn next(asked: &mut mpsc::UnboundedReceiver<Asked>) -> Asked {
        timeout(PATIENCE, asked.recv())
            .await
            .expect("an ask")
            .expect("the check runs")
    }

    fn someone() -> Result<Owner, String> {
        Ok(Owner {
            email: Some(EMAIL.into()),
        })
    }

    fn email(secrets: &Secrets) -> Option<String> {
        account(secrets, key_account(secrets.device()))?.email
    }

    async fn email_becomes(secrets: &Secrets, want: &str) {
        let deadline = Instant::now() + PATIENCE;
        while email(secrets).as_deref() != Some(want) {
            assert!(
                Instant::now() < deadline,
                "the email stayed {:?}",
                email(secrets)
            );
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// Whether the check asks again within a moment.
    async fn asks_again(asked: &mut mpsc::UnboundedReceiver<Asked>) -> bool {
        sleep(Duration::from_millis(50)).await;
        asked.try_recv().is_ok()
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

    #[tokio::test]
    async fn a_stored_key_gets_its_owners_email() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let (mut asked, _check) = check(&secrets, FIRST_WAIT);
        secrets.store_cursor_key(String::new(), Secret::new(KEY.into()));
        assert!(account(&secrets, key_account(secrets.device())).is_some());
        assert_eq!(email(&secrets), None, "until Cursor says");

        let (key, reply) = next(&mut asked).await;
        assert_eq!(key, KEY);
        reply.send(someone()).unwrap();
        email_becomes(&secrets, EMAIL).await;

        // The same key under a new name keeps it, without asking again.
        secrets.store_cursor_key("Work".into(), Secret::new(KEY.into()));
        let shown = account(&secrets, key_account(secrets.device())).unwrap();
        assert_eq!(shown.label, "Work");
        assert_eq!(shown.email.as_deref(), Some(EMAIL));
        assert_eq!(shown.plan, None, "Cursor doesn't say the key's plan");
        assert!(!asks_again(&mut asked).await);
    }

    #[tokio::test]
    async fn a_key_loaded_at_start_is_asked_about_again() {
        let temp = TempDir::new();
        open(&temp).store_cursor_key(String::new(), Secret::new(KEY.into()));
        let secrets = open(&temp);
        assert_eq!(email(&secrets), None);
        let (mut asked, _check) = check(&secrets, FIRST_WAIT);
        let (key, reply) = next(&mut asked).await;
        assert_eq!(key, KEY);
        reply.send(someone()).unwrap();
        email_becomes(&secrets, EMAIL).await;
        assert!(!asks_again(&mut asked).await);
    }

    #[tokio::test]
    async fn a_failed_ask_leaves_the_email_out_until_a_later_one_works() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let (mut asked, _check) = check(&secrets, Duration::from_millis(10));
        secrets.store_cursor_key(String::new(), Secret::new(KEY.into()));
        let (_, reply) = next(&mut asked).await;
        reply.send(Err("Cursor answered 500".into())).unwrap();

        let (key, reply) = next(&mut asked).await;
        assert_eq!(key, KEY);
        assert!(account(&secrets, key_account(secrets.device())).is_some());
        assert_eq!(email(&secrets), None);
        reply.send(someone()).unwrap();
        email_becomes(&secrets, EMAIL).await;
    }

    #[tokio::test]
    async fn forgetting_the_key_drops_its_pending_ask() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let (mut asked, _check) = check(&secrets, FIRST_WAIT);
        secrets.store_cursor_key(String::new(), Secret::new(KEY.into()));
        let (_, mut reply) = next(&mut asked).await;

        let credential = credential_id(secrets.device());
        assert_eq!(secrets.forget_file(credential), ActionOutcome::Done);
        timeout(PATIENCE, reply.closed())
            .await
            .expect("the ask was dropped");
        // An answer that comes anyway shows nowhere.
        secrets.learned(&Secret::new(KEY.into()), someone().unwrap());
        assert!(account(&secrets, key_account(secrets.device())).is_none());
        assert!(credentials(&secrets).is_empty());
        assert!(!asks_again(&mut asked).await);
    }

    #[tokio::test]
    async fn an_answer_about_a_replaced_key_counts_for_nothing() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let (mut asked, _check) = check(&secrets, FIRST_WAIT);
        secrets.store_cursor_key(String::new(), Secret::new("key_first".into()));
        let (key, mut first) = next(&mut asked).await;
        assert_eq!(key, "key_first");

        secrets.store_cursor_key(String::new(), Secret::new("key_second".into()));
        timeout(PATIENCE, first.closed())
            .await
            .expect("the first ask was dropped");
        let (key, second) = next(&mut asked).await;
        assert_eq!(key, "key_second");
        let earlier = Owner {
            email: Some("earlier@example.com".into()),
        };
        secrets.learned(&Secret::new("key_first".into()), earlier);
        assert_eq!(email(&secrets), None);

        second.send(someone()).unwrap();
        email_becomes(&secrets, EMAIL).await;
    }

    #[test]
    fn each_wait_doubles_up_to_an_hour() {
        let waits = [1, 2, 3, 6, 7, 50].map(|failures| backoff(FIRST_WAIT, failures).as_secs());
        assert_eq!(waits, [60, 120, 240, 1920, 3600, 3600]);
    }
}
