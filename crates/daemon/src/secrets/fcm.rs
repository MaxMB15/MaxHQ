// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Firebase service account this device pushes to Android phones
//! with: its project, its email and its RSA key, from the JSON file Google
//! gave. The Mac sends it once, the device keeps it in the store as
//! `fcm.json`, and it never leaves again. Each push carries an OAuth access
//! token, which Google trades for an RS256 JWT the key signs, reused until
//! [`MARGIN`] before Google says it expires.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::ids::{CredentialId, DeviceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Credential, CredentialKind};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::github::key::decode;
use super::{refused, Secrets};

/// The account: its project, email and key.
const FCM_FILE: &str = "fcm.json";

/// Google's OAuth token endpoint, which is also the JWT's audience.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// What the access token may do: send FCM messages, nothing else.
pub const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// How long a JWT is good for. Google takes at most an hour.
const ASSERTION_LIFE: Duration = Duration::from_secs(60 * 60);
/// How long before Google's expiry an access token stops being used, so
/// one never runs out on the way. An hour's token lasts 55 minutes.
pub const MARGIN: Duration = Duration::from_secs(5 * 60);

/// Why a key was refused, for a person to read.
const NOT_A_KEY: &str = "That isn't a Firebase service account key. Choose the JSON file Firebase \
     gave you when you made the key.";

/// A service account's key, ready to sign JWTs.
pub struct FcmKey {
    project_id: String,
    client_email: String,
    pair: RsaKeyPair,
    /// The access token Google gave last, and when it stops being used.
    access: Mutex<Option<(Secret, Timestamp)>>,
}

impl FcmKey {
    /// Reads the account's `private_key`: an unencrypted RSA key in
    /// PKCS #8, as Google writes it.
    pub fn parse(project_id: &str, client_email: &str, pem: &str) -> Result<Self, &'static str> {
        let (label, der) = decode(pem).map_err(|_| NOT_A_KEY)?;
        if label != "PRIVATE KEY" {
            return Err(NOT_A_KEY);
        }
        let pair = RsaKeyPair::from_pkcs8(&der).map_err(|_| NOT_A_KEY)?;
        Ok(Self {
            project_id: project_id.to_owned(),
            client_email: client_email.to_owned(),
            pair,
            access: Mutex::new(None),
        })
    }

    /// The Firebase project, which names the endpoint.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The access token Google gave last, while it's still good at `now`.
    pub fn access(&self, now: Timestamp) -> Option<Secret> {
        match self.lock().as_ref() {
            Some((token, until)) if now.0 < until.0 => Some(token.clone()),
            _ => None,
        }
    }

    /// Keeps `token`, which Google said at `now` lasts `expires_in`
    /// seconds. One that lasts no longer than [`MARGIN`] isn't kept.
    pub fn keep(&self, token: Secret, now: Timestamp, expires_in: u64) {
        let life = Duration::from_secs(expires_in).saturating_sub(MARGIN);
        *self.lock() = (!life.is_zero()).then(|| (token, Timestamp(now.0 + duration_ms(life))));
    }

    /// Drops the access token, for when FCM refuses it, so the next push
    /// gets a new one.
    pub fn expire(&self) {
        *self.lock() = None;
    }

    /// A JWT issued at `now`, which Google trades for an access token.
    pub fn assertion(&self, now: Timestamp) -> Result<Secret, &'static str> {
        let header = json!({ "alg": "RS256", "typ": "JWT" });
        let iat = now.0 / 1_000;
        let claims = json!({
            "iss": self.client_email,
            "scope": SCOPE,
            "aud": TOKEN_URL,
            "iat": iat,
            "exp": iat + ASSERTION_LIFE.as_secs(),
        });
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut signature = vec![0; self.pair.public().modulus_len()];
        self.pair
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .map_err(|_| "The Firebase key couldn't sign a token.")?;
        let signature = URL_SAFE_NO_PAD.encode(signature);
        Ok(Secret::new(format!("{signed}.{signature}")))
    }

    fn lock(&self) -> MutexGuard<'_, Option<(Secret, Timestamp)>> {
        self.access.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The account the store holds, if any.
#[derive(Default)]
pub struct Held(Mutex<Option<Stored>>);

impl Held {
    fn lock(&self) -> MutexGuard<'_, Option<Stored>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Stored {
    key: Arc<FcmKey>,
    stored_at: Timestamp,
}

/// `fcm.json`. One file, so a crash never leaves half an account.
#[derive(Serialize, Deserialize)]
struct KeyFile {
    project_id: String,
    client_email: String,
    private_key: Secret,
    stored_at: Timestamp,
}

/// The Firebase key's credential on `device`.
fn credential_id(device: DeviceId) -> CredentialId {
    CredentialId::derive(&format!("fcm:{device}"))
}

/// A Firebase project ID: lowercase letters, digits and dashes, starting
/// with a letter, or an older one under a domain, such as
/// `example.com:maxhq`.
fn project_id(text: &str) -> Option<String> {
    let text = text.trim();
    let fits = (1..=100).contains(&text.len())
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b':')
        });
    fits.then(|| text.to_owned())
}

/// A service account's email, such as
/// `maxhq-push@<project>.iam.gserviceaccount.com`.
fn client_email(text: &str) -> Option<String> {
    let text = text.trim();
    let fits = (3..=320).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_graphic())
        && text.split_once('@').is_some_and(|(name, domain)| {
            !name.is_empty() && !domain.is_empty() && !domain.contains('@')
        });
    fits.then(|| text.to_owned())
}

impl Secrets {
    /// Answers `SetFcmKey`: checks the key signs, stores it, and
    /// publishes its credential. A new key replaces the old one.
    pub(super) fn set_fcm_key(
        &self,
        project: &str,
        email: &str,
        private_key: Secret,
    ) -> ActionOutcome {
        let Some(project) = project_id(project) else {
            return refused(
                "The project ID is the Firebase project's ID, from the JSON file's project_id.",
            );
        };
        let Some(email) = client_email(email) else {
            return refused("The client email is the service account's address, from the JSON file's client_email.");
        };
        let pem = Secret::new(format!("{}\n", private_key.expose().trim()));
        drop(private_key);
        let key = match FcmKey::parse(&project, &email, pem.expose()) {
            Ok(key) => key,
            Err(reason) => return refused(reason),
        };
        if let Err(reason) = key.assertion(self.now()) {
            return refused(reason);
        }
        let stored_at = self.now();
        let file = KeyFile {
            project_id: project.clone(),
            client_email: email,
            private_key: pem,
            stored_at,
        };
        let Ok(json) = serde_json::to_vec_pretty(&file).map(Zeroizing::new) else {
            return refused("This device couldn't store the key.");
        };
        drop(file);

        let mut held = self.inner.fcm.lock();
        if let Err(err) = self.inner.folder.write(FCM_FILE, &json) {
            warn!("couldn't store the Firebase key: {err}");
            return refused(format!("This device couldn't store the key: {err}"));
        }
        info!(project, "stored the Firebase key");
        let stored = Stored {
            key: Arc::new(key),
            stored_at,
        };
        self.publish_fcm(&stored);
        *held = Some(stored);
        ActionOutcome::Done
    }

    /// Reads the account the folder holds, if any, and publishes it.
    pub(super) fn load_fcm(&self) {
        let text = match self.inner.folder.read(FCM_FILE) {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(err) => {
                warn!("couldn't read the Firebase key: {err}");
                return;
            }
        };
        let Ok(file) = serde_json::from_str::<KeyFile>(text.expose()) else {
            warn!("skipped the Firebase key: {FCM_FILE} doesn't describe one");
            return;
        };
        let key = match FcmKey::parse(
            &file.project_id,
            &file.client_email,
            file.private_key.expose(),
        ) {
            Ok(key) => key,
            Err(_) => {
                warn!(
                    project = file.project_id,
                    "skipped the Firebase key: it doesn't read"
                );
                return;
            }
        };
        info!(project = file.project_id, "loaded the Firebase key");
        let stored = Stored {
            key: Arc::new(key),
            stored_at: file.stored_at,
        };
        self.publish_fcm(&stored);
        *self.inner.fcm.lock() = Some(stored);
    }

    fn publish_fcm(&self, stored: &Stored) {
        let device = self.device();
        self.inner.hub.apply([Change::Upsert {
            value: Entity::Credential(Credential {
                id: credential_id(device),
                device,
                kind: CredentialKind::Fcm {
                    project_id: stored.key.project_id.clone(),
                    client_email: stored.key.client_email.clone(),
                },
                stored_at: stored.stored_at,
                problem: None,
            }),
        }]);
    }

    /// The key pushes to Android phones go with, while the device holds
    /// one.
    pub(super) fn fcm(&self) -> Option<Arc<FcmKey>> {
        self.inner.fcm.lock().as_ref().map(|s| s.key.clone())
    }

    /// Forgets the key behind `credential`, deleting its file. Returns
    /// none when `credential` isn't the key's.
    pub(super) fn forget_fcm(&self, credential: CredentialId) -> Option<ActionOutcome> {
        if credential != credential_id(self.device()) {
            return None;
        }
        let mut held = self.inner.fcm.lock();
        let project = held.as_ref()?.key.project_id.clone();
        if let Err(err) = self.inner.folder.remove(FCM_FILE) {
            warn!(project, "couldn't delete the Firebase key: {err}");
            return Some(refused(format!(
                "This device couldn't delete the key: {err}"
            )));
        }
        *held = None;
        self.inner.hub.apply([Change::Remove {
            key: EntityKey::Credential(credential),
        }]);
        info!(project, "forgot the Firebase key");
        Some(ActionOutcome::Done)
    }
}

/// The throwaway RSA key the tests share, as a service account's PEM.
#[cfg(test)]
pub fn test_pem() -> String {
    super::testing::pem("PRIVATE KEY", super::testing::RSA_PKCS8)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ring::signature::{KeyPair, UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
    use serde_json::Value;

    use super::*;
    use crate::secrets::testing::{credentials, hub, pem, EC_PKCS8, RSA_PKCS1};
    use crate::secrets::DIR;
    use crate::testing::TempDir;

    const PROJECT: &str = "maxhq-test";
    const EMAIL: &str = "pusher@maxhq-test.iam.example.invalid";

    fn open(temp: &TempDir) -> Secrets {
        Secrets::open(temp.path().join(DIR), hub()).unwrap()
    }

    fn part(token: &str, n: usize) -> Value {
        let part = token.split('.').nth(n).unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
    }

    #[test]
    fn an_assertion_is_rs256_for_firebase_messaging() {
        let key = FcmKey::parse(PROJECT, EMAIL, &test_pem()).unwrap();
        let assertion = key.assertion(Timestamp(1_800_000_000_123)).unwrap();
        let text = assertion.expose();
        assert_eq!(part(text, 0), json!({ "alg": "RS256", "typ": "JWT" }));
        assert_eq!(
            part(text, 1),
            json!({
                "iss": EMAIL,
                "scope": "https://www.googleapis.com/auth/firebase.messaging",
                "aud": "https://oauth2.googleapis.com/token",
                "iat": 1_800_000_000,
                "exp": 1_800_003_600,
            })
        );
        let (signed, signature) = text.rsplit_once('.').unwrap();
        let public = key.pair.public_key().as_ref().to_vec();
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public)
            .verify(
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .unwrap();
    }

    #[test]
    fn an_access_token_is_kept_until_five_minutes_before_it_expires() {
        let key = FcmKey::parse(PROJECT, EMAIL, &test_pem()).unwrap();
        let start = Timestamp(1_800_000_000_000);
        assert!(key.access(start).is_none());
        key.keep(Secret::new("test-not-a-real-access".into()), start, 3_600);
        let minutes = |n: u64| Timestamp(start.0 + n * 60 * 1_000);
        assert_eq!(
            key.access(minutes(54)).unwrap().expose(),
            "test-not-a-real-access"
        );
        assert!(key.access(minutes(55)).is_none(), "gone at 55 minutes");
        key.expire();
        assert!(key.access(start).is_none(), "gone once expired");
        key.keep(Secret::new("test-not-a-real-access".into()), start, 300);
        assert!(key.access(start).is_none(), "too short to keep");
    }

    #[test]
    fn stores_a_key_publishes_it_and_reads_it_back() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let outcome = secrets.set_fcm_key(&format!(" {PROJECT} "), EMAIL, Secret::new(test_pem()));
        assert_eq!(outcome, ActionOutcome::Done);
        let [credential] = &credentials(&secrets)[..] else {
            panic!("one credential");
        };
        assert_eq!(
            credential.kind,
            CredentialKind::Fcm {
                project_id: PROJECT.into(),
                client_email: EMAIL.into(),
            }
        );
        assert_eq!(secrets.fcm().unwrap().project_id(), PROJECT);
        let again = open(&temp);
        assert_eq!(again.fcm().unwrap().project_id(), PROJECT);
        assert_eq!(credentials(&again).len(), 1);

        assert_eq!(
            again.forget_file(credential.id),
            ActionOutcome::Done,
            "forget takes it"
        );
        assert!(again.fcm().is_none());
        assert!(credentials(&again).is_empty());
        assert!(fs::read_dir(temp.path().join(DIR))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn the_apns_and_firebase_keys_live_side_by_side() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let apns = super::super::apns::test_p8();
        assert_eq!(
            secrets.set_apns_key("ABC123DEFG", "TEAM123456", Secret::new(apns)),
            ActionOutcome::Done
        );
        assert_eq!(
            secrets.set_fcm_key(PROJECT, EMAIL, Secret::new(test_pem())),
            ActionOutcome::Done
        );
        assert_eq!(credentials(&secrets).len(), 2);
        assert!(secrets.apns().is_some() && secrets.fcm().is_some());
    }

    #[test]
    fn refuses_what_isnt_a_firebase_key() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let set = |project: &str, email: &str, key: String| {
            secrets.set_fcm_key(project, email, Secret::new(key))
        };
        for outcome in [
            set("", EMAIL, test_pem()),
            set("Maxhq", EMAIL, test_pem()),
            set("maxhq/../x", EMAIL, test_pem()),
            set(PROJECT, "not an email", test_pem()),
            set(PROJECT, "@example.invalid", test_pem()),
            set(PROJECT, EMAIL, "not a key".into()),
            set(PROJECT, EMAIL, pem("PRIVATE KEY", EC_PKCS8)),
            set(PROJECT, EMAIL, pem("RSA PRIVATE KEY", RSA_PKCS1)),
            set(PROJECT, EMAIL, pem("ENCRYPTED PRIVATE KEY", RSA_PKCS1)),
        ] {
            let ActionOutcome::Refused { reason } = outcome else {
                panic!("refused");
            };
            assert!(!reason.contains("PRIVATE KEY"), "{reason}");
        }
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.fcm().is_none());
    }

    #[test]
    fn takes_a_domain_scoped_project() {
        assert_eq!(
            project_id("example.com:maxhq").as_deref(),
            Some("example.com:maxhq")
        );
        assert_eq!(project_id("1maxhq"), None);
    }
}
