// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The APNs key this device signs its pushes to phones with: the `.p8`
//! file Apple gave, with its Key ID and team. The Mac sends it once, the
//! device keeps it in the store, and it never leaves again. Each push
//! carries an ES256 provider token made from it, which is reused for 50
//! minutes, inside Apple's 20 to 60 minute window.

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
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::github::key::decode;
use super::{refused, Secrets};

/// The key's file, as Apple gave it.
const APNS_KEY_FILE: &str = "apns.p8";
/// The key's ID and team.
const APNS_FILE: &str = "apns.json";

/// How long a provider token is reused. Apple refuses one older than an
/// hour, and one made more often than every 20 minutes.
pub const REUSE: Duration = Duration::from_secs(50 * 60);

/// Why a key was refused, for a person to read.
const NOT_A_KEY: &str =
    "That isn't an APNs key. Choose the .p8 file Apple gave you when you made the key.";

/// An APNs key, ready to sign provider tokens.
pub struct ApnsKey {
    key_id: String,
    team_id: String,
    pair: EcdsaKeyPair,
    /// The token made last, and when.
    made: Mutex<Option<(Secret, Timestamp)>>,
}

impl ApnsKey {
    /// Reads a `.p8` file: an unencrypted P-256 key in PKCS #8.
    pub fn parse(key_id: &str, team_id: &str, pem: &str) -> Result<Self, &'static str> {
        let (label, der) = decode(pem).map_err(|_| NOT_A_KEY)?;
        if label != "PRIVATE KEY" {
            return Err(NOT_A_KEY);
        }
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng())
            .map_err(|_| NOT_A_KEY)?;
        Ok(Self {
            key_id: key_id.to_owned(),
            team_id: team_id.to_owned(),
            pair,
            made: Mutex::new(None),
        })
    }

    /// Apple's ID for the key.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// A provider token for now: the one made last while it's younger
    /// than [`REUSE`], or else a new one.
    pub fn token(&self, now: Timestamp) -> Result<Secret, &'static str> {
        let mut made = self.lock();
        if let Some((token, at)) = made.as_ref()
            && now.0 >= at.0
            && now.0 - at.0 < duration_ms(REUSE)
        {
            return Ok(token.clone());
        }
        let token = self.sign(now)?;
        *made = Some((token.clone(), now));
        Ok(token)
    }

    /// Drops the token made last, for when APNs says it's expired, so the
    /// next push makes a new one.
    pub fn expire(&self) {
        *self.lock() = None;
    }

    /// A new provider token, issued at `now`.
    fn sign(&self, now: Timestamp) -> Result<Secret, &'static str> {
        let header = json!({ "alg": "ES256", "kid": self.key_id });
        let claims = json!({ "iss": self.team_id, "iat": now.0 / 1_000 });
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = self
            .pair
            .sign(&rng(), signed.as_bytes())
            .map_err(|_| "The APNs key couldn't sign a token.")?;
        let signature = URL_SAFE_NO_PAD.encode(signature.as_ref());
        Ok(Secret::new(format!("{signed}.{signature}")))
    }

    fn lock(&self) -> MutexGuard<'_, Option<(Secret, Timestamp)>> {
        self.made.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn rng() -> SystemRandom {
    SystemRandom::new()
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The key the store holds, if any.
#[derive(Default)]
pub struct Held(Mutex<Option<Stored>>);

impl Held {
    fn lock(&self) -> MutexGuard<'_, Option<Stored>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Stored {
    key: Arc<ApnsKey>,
    stored_at: Timestamp,
}

/// `apns.json`.
#[derive(Serialize, Deserialize)]
struct KeyFile {
    key_id: String,
    team_id: String,
    stored_at: Timestamp,
}

/// The APNs key's credential on `device`.
fn credential_id(device: DeviceId) -> CredentialId {
    CredentialId::derive(&format!("apns:{device}"))
}

/// A Key ID or team ID as Apple writes them: 10 letters and digits.
fn apple_id(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() == 10 && text.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| text.to_ascii_uppercase())
}

impl Secrets {
    /// Answers `SetApnsKey`: checks the key signs, stores it, and
    /// publishes its credential. A new key replaces the old one.
    pub(super) fn set_apns_key(
        &self,
        key_id: &str,
        team_id: &str,
        private_key: Secret,
    ) -> ActionOutcome {
        let Some(key_id) = apple_id(key_id) else {
            return refused("The Key ID is the 10 letters and digits Apple shows for the key.");
        };
        let Some(team_id) = apple_id(team_id) else {
            return refused("The team ID is the 10 letters and digits Apple shows for your team.");
        };
        let pem = Zeroizing::new(format!("{}\n", private_key.expose().trim()));
        drop(private_key);
        let key = match ApnsKey::parse(&key_id, &team_id, &pem) {
            Ok(key) => key,
            Err(reason) => return refused(reason),
        };
        if let Err(reason) = key.sign(self.now()) {
            return refused(reason);
        }
        let stored_at = self.now();
        let file = KeyFile {
            key_id: key_id.clone(),
            team_id: team_id.clone(),
            stored_at,
        };
        let Ok(json) = serde_json::to_vec_pretty(&file) else {
            return refused("This device couldn't store the key.");
        };

        let folder = &self.inner.folder;
        let mut held = self.inner.apns.lock();
        if let Err(err) = folder.write(APNS_KEY_FILE, pem.as_bytes()) {
            warn!("couldn't store the APNs key: {err}");
            return refused(format!("This device couldn't store the key: {err}"));
        }
        if let Err(err) = folder.write(APNS_FILE, &json) {
            // The new key sits next to the old key's file now, so neither
            // stays.
            warn!("couldn't store the APNs key's ID: {err}");
            let _ = folder.remove(APNS_KEY_FILE);
            let _ = folder.remove(APNS_FILE);
            if held.take().is_some() {
                self.inner.hub.apply([Change::Remove {
                    key: EntityKey::Credential(credential_id(self.device())),
                }]);
            }
            return refused(format!("This device couldn't store the key: {err}"));
        }
        info!(key_id, "stored the APNs key");
        let stored = Stored {
            key: Arc::new(key),
            stored_at,
        };
        self.publish_apns(&stored);
        *held = Some(stored);
        ActionOutcome::Done
    }

    /// Reads the key the folder holds, if any, and publishes it.
    pub(super) fn load_apns(&self) {
        let folder = &self.inner.folder;
        let (file, pem) = match (folder.read(APNS_FILE), folder.read(APNS_KEY_FILE)) {
            (Ok(file), Ok(pem)) => (file, pem),
            (Err(err), _) | (_, Err(err)) => {
                warn!("couldn't read the APNs key: {err}");
                return;
            }
        };
        let (file, pem) = match (file, pem) {
            (Some(file), Some(pem)) => (file, pem),
            (None, None) => return,
            _ => {
                // A crash between the two writes leaves one alone.
                warn!("removed half an APNs key, so set it again");
                let _ = folder.remove(APNS_KEY_FILE);
                let _ = folder.remove(APNS_FILE);
                return;
            }
        };
        let Ok(file) = serde_json::from_str::<KeyFile>(file.expose()) else {
            warn!("skipped the APNs key: {APNS_FILE} doesn't describe one");
            return;
        };
        let key = match ApnsKey::parse(&file.key_id, &file.team_id, pem.expose()) {
            Ok(key) => key,
            Err(_) => {
                warn!(
                    key_id = file.key_id,
                    "skipped the APNs key: it doesn't read"
                );
                return;
            }
        };
        info!(key_id = file.key_id, "loaded the APNs key");
        let stored = Stored {
            key: Arc::new(key),
            stored_at: file.stored_at,
        };
        self.publish_apns(&stored);
        *self.inner.apns.lock() = Some(stored);
    }

    fn publish_apns(&self, stored: &Stored) {
        let device = self.device();
        self.inner.hub.apply([Change::Upsert {
            value: Entity::Credential(Credential {
                id: credential_id(device),
                device,
                kind: CredentialKind::Apns {
                    key_id: stored.key.key_id.clone(),
                    team_id: stored.key.team_id.clone(),
                },
                stored_at: stored.stored_at,
                problem: None,
            }),
        }]);
    }

    /// The key pushes sign with, while the device holds one.
    pub(super) fn apns(&self) -> Option<Arc<ApnsKey>> {
        self.inner.apns.lock().as_ref().map(|s| s.key.clone())
    }

    /// Forgets the key behind `credential`, deleting its files. Returns
    /// none when `credential` isn't the key's.
    pub(super) fn forget_apns(&self, credential: CredentialId) -> Option<ActionOutcome> {
        if credential != credential_id(self.device()) {
            return None;
        }
        let mut held = self.inner.apns.lock();
        let key_id = held.as_ref()?.key.key_id.clone();
        if let Err(err) = self.inner.folder.remove(APNS_KEY_FILE) {
            warn!(key_id, "couldn't delete the APNs key: {err}");
            return Some(refused(format!(
                "This device couldn't delete the key: {err}"
            )));
        }
        if let Err(err) = self.inner.folder.remove(APNS_FILE) {
            // The key is gone, which is what counts.
            warn!(key_id, "couldn't delete {APNS_FILE}: {err}");
        }
        *held = None;
        self.inner.hub.apply([Change::Remove {
            key: EntityKey::Credential(credential),
        }]);
        info!(key_id, "forgot the APNs key");
        Some(ActionOutcome::Done)
    }
}

/// A made-up P-256 key in a `.p8` file's shape, for tests.
#[cfg(test)]
pub fn test_p8() -> String {
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng()).unwrap();
    use base64::engine::general_purpose::STANDARD;
    super::testing::pem("PRIVATE KEY", &STANDARD.encode(pkcs8.as_ref()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ring::signature::{KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};
    use serde_json::Value;

    use super::*;
    use crate::secrets::testing::{credentials, hub, pem, EC_PKCS8, RSA_PKCS8};
    use crate::secrets::DIR;
    use crate::testing::TempDir;

    const KEY_ID: &str = "ABC123DEFG";
    const TEAM_ID: &str = "TEAM123456";

    fn open(temp: &TempDir) -> Secrets {
        Secrets::open(temp.path().join(DIR), hub()).unwrap()
    }

    fn part(token: &str, n: usize) -> Value {
        let part = token.split('.').nth(n).unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
    }

    #[test]
    fn a_token_is_es256_and_reused_inside_fifty_minutes() {
        let pem = test_p8();
        let key = ApnsKey::parse(KEY_ID, TEAM_ID, &pem).unwrap();
        let start = Timestamp(1_800_000_000_000);
        let first = key.token(start).unwrap();
        let text = first.expose();
        assert_eq!(part(text, 0), json!({ "alg": "ES256", "kid": KEY_ID }));
        assert_eq!(
            part(text, 1),
            json!({ "iss": TEAM_ID, "iat": 1_800_000_000 })
        );
        let (signed, signature) = text.rsplit_once('.').unwrap();
        let public = key.pair.public_key().as_ref().to_vec();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(
                signed.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .unwrap();

        let later = Timestamp(start.0 + 49 * 60 * 1_000);
        assert_eq!(key.token(later).unwrap(), first, "reused");
        let fifty = Timestamp(start.0 + 50 * 60 * 1_000);
        let second = key.token(fifty).unwrap();
        assert_ne!(second, first, "made again at fifty minutes");
        assert_eq!(part(second.expose(), 1)["iat"], json!(1_800_003_000));
        key.expire();
        assert_ne!(key.token(fifty).unwrap(), second, "made again once expired");
    }

    #[test]
    fn stores_a_key_publishes_it_and_reads_it_back() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let outcome = secrets.set_apns_key(" abc123defg ", TEAM_ID, Secret::new(test_p8()));
        assert_eq!(outcome, ActionOutcome::Done);
        let [credential] = &credentials(&secrets)[..] else {
            panic!("one credential");
        };
        assert_eq!(
            credential.kind,
            CredentialKind::Apns {
                key_id: KEY_ID.into(),
                team_id: TEAM_ID.into(),
            }
        );
        assert_eq!(secrets.apns().unwrap().key_id(), KEY_ID);
        let again = open(&temp);
        assert_eq!(again.apns().unwrap().key_id(), KEY_ID);
        assert_eq!(credentials(&again).len(), 1);

        assert_eq!(
            again.forget_file(credential.id),
            ActionOutcome::Done,
            "forget takes it"
        );
        assert!(again.apns().is_none());
        assert!(credentials(&again).is_empty());
        assert!(fs::read_dir(temp.path().join(DIR))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn refuses_what_isnt_an_apns_key() {
        let temp = TempDir::new();
        let secrets = open(&temp);
        let set = |key_id: &str, team_id: &str, key: String| {
            secrets.set_apns_key(key_id, team_id, Secret::new(key))
        };
        for outcome in [
            set("short", TEAM_ID, test_p8()),
            set(KEY_ID, "not a team", test_p8()),
            set(KEY_ID, TEAM_ID, "not a key".into()),
            set(KEY_ID, TEAM_ID, pem("PRIVATE KEY", RSA_PKCS8)),
            set(KEY_ID, TEAM_ID, pem("PRIVATE KEY", EC_PKCS8)),
            set(KEY_ID, TEAM_ID, pem("ENCRYPTED PRIVATE KEY", RSA_PKCS8)),
        ] {
            let ActionOutcome::Refused { reason } = outcome else {
                panic!("refused");
            };
            assert!(!reason.contains("PRIVATE KEY"), "{reason}");
        }
        assert!(credentials(&secrets).is_empty());
        assert!(secrets.apns().is_none());
    }
}
