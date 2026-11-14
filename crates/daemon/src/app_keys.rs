// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The app keys this device trusts. A connection that proves it holds one
//! may do everything the app does.
//!
//! They sit in a file beside the phones', which holds public keys only, so
//! nothing in it is worth stealing. `maxd run` and `maxd authorize-key`
//! both change it, so each change takes a lock file first, then reads the
//! file, changes it and writes it whole. `maxd run` shows the keys on its
//! hub, as each read or change finds them.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use maxhq_protocol::admission::{AppKey, AppKeyOrigin};
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::state::Entity;
use maxhq_protocol::time::Timestamp;
use tokio::sync::broadcast;
use tracing::warn;

use crate::admission::Admissions;
use crate::hub::Hub;
use crate::secrets::files::Folder;

/// The file, in the phones' folder.
pub const FILE: &str = "app-keys.json";
/// The file each change locks first.
const LOCK: &str = "app-keys.lock";
/// The longest label kept, in characters.
const LABEL_MAX: usize = 100;
/// The label of a key that came without one.
const LABEL_DEFAULT: &str = "MaxHQ";

/// What the Mac's code signature check said about the program on the
/// other end of a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    /// This `maxd` has no team to check against: it's unsigned, or it
    /// isn't on a Mac.
    Off,
    /// The program is the app, signed by `maxd`'s own team.
    Passed,
    /// It isn't, or it couldn't be told.
    Failed,
}

/// Who asks the device to trust a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asker {
    /// A connection already proven with a key the device trusts. That's how
    /// the app changes its key.
    App,
    /// A connection that proved it holds the key and nothing more, with
    /// what the signature check said about it.
    Connection(Signature),
    /// `maxd authorize-key`, which setup runs over your own login.
    Setup,
    /// You, from the card that asked whether to trust the key.
    Approved,
}

/// What the device does with a key it's asked to trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// It trusts the key already.
    Trusted,
    /// It trusts the key from now on, which came about as `origin` says.
    Add(AppKeyOrigin),
    /// It needs a yes from you first.
    Ask,
}

/// Decides whether to trust a key: `trusted` when the device trusts it
/// already, and `trusts_any` when it trusts some key.
///
/// A Mac with a signed `maxd` takes a new key only from the signed app, so
/// no program there can be first. A device without the check, a remote
/// device or one with an unsigned `maxd`, takes the first key offered while
/// it trusts none. Setup's key counts only then too. Past that, a new key
/// needs a key the device trusts, or your yes.
pub fn decide(trusted: bool, trusts_any: bool, asker: Asker) -> Decision {
    match asker {
        _ if trusted => Decision::Trusted,
        Asker::App => Decision::Add(AppKeyOrigin::App),
        Asker::Connection(Signature::Passed) => Decision::Add(AppKeyOrigin::Signature),
        Asker::Connection(Signature::Off) if !trusts_any => Decision::Add(AppKeyOrigin::First),
        Asker::Setup if !trusts_any => Decision::Add(AppKeyOrigin::Setup),
        Asker::Approved => Decision::Add(AppKeyOrigin::Approved),
        Asker::Connection(_) | Asker::Setup => Decision::Ask,
    }
}

/// What removing a key came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// The device no longer trusts it.
    Removed,
    /// The device didn't trust it.
    Absent,
    /// It's the only key the device trusts, so it stays: without one, the
    /// device would take the next key anyone offers.
    Last,
}

/// The app keys file, shared by every connection.
#[derive(Clone)]
pub struct AppKeys(Arc<Inner>);

struct Inner {
    folder: PathBuf,
    device: DeviceId,
    /// Each key removed, so connections proven with it close.
    removed: broadcast::Sender<String>,
    /// Where the keys show, once `maxd run` says.
    hub: OnceLock<Hub>,
    /// The cards that ask whether to let a program in or trust its key.
    admissions: Admissions,
}

impl AppKeys {
    /// The keys `device` trusts, in `folder`. Nothing is read until a key
    /// is asked about.
    pub fn open(folder: PathBuf, device: DeviceId) -> Self {
        Self(Arc::new_cyclic(|me: &Weak<Inner>| {
            let me = me.clone();
            Inner {
                folder,
                device,
                removed: broadcast::channel(16).0,
                hub: OnceLock::new(),
                admissions: Admissions::new(Box::new(move |public_key, label| {
                    let inner = me
                        .upgrade()
                        .ok_or_else(|| io::Error::other("maxd is stopping"))?;
                    AppKeys(inner).enroll(public_key, label, Asker::Approved)
                })),
            }
        }))
    }

    /// The cards that ask you about programs on this device.
    pub fn admissions(&self) -> &Admissions {
        &self.0.admissions
    }

    /// Shows the keys on `hub` from now on, as each read or change finds
    /// them. Only connections past the gate sync, so only they see them.
    pub fn publish_to(&self, hub: Hub) {
        if self.0.hub.set(hub).is_err() {
            return;
        }
        if let Err(err) = self.change(|_| ((), false)) {
            warn!(%err, "couldn't read the app keys to show them");
        }
    }

    /// Each key removed from now on.
    pub fn removed(&self) -> broadcast::Receiver<String> {
        self.0.removed.subscribe()
    }

    /// Whether the device trusts `public_key`, which a connection just
    /// proved it holds. A trusted key's proof time moves to now.
    pub fn proved(&self, public_key: &str) -> io::Result<bool> {
        self.change(
            |keys| match keys.iter_mut().find(|k| k.public_key == public_key) {
                Some(key) => {
                    key.proved_at = Some(Timestamp::now());
                    (true, true)
                }
                None => (false, false),
            },
        )
    }

    /// Trusts `public_key` when `asker` may add it, as [`decide`] says. The
    /// key is in its canonical form, from [`canonical`].
    pub fn enroll(&self, public_key: &str, label: &str, asker: Asker) -> io::Result<Decision> {
        let device = self.0.device;
        self.change(|keys| {
            let trusted = keys.iter().any(|k| k.public_key == public_key);
            let decision = decide(trusted, !keys.is_empty(), asker);
            let Decision::Add(origin) = decision else {
                return (decision, false);
            };
            let now = Timestamp::now();
            keys.push(AppKey {
                id: AppKey::id_for(device, public_key),
                device,
                public_key: public_key.to_owned(),
                label: clean_label(label),
                added_at: now,
                origin,
                // Setup only hands the key over. Every other way, the
                // connection asking has just proved it holds the key.
                proved_at: (asker != Asker::Setup).then_some(now),
            });
            (decision, true)
        })
    }

    /// Stops trusting `public_key`, unless it's the last key.
    pub fn remove(&self, public_key: &str) -> io::Result<Removal> {
        let Some(public_key) = canonical(public_key) else {
            return Ok(Removal::Absent);
        };
        let removal = self.change(|keys| {
            if !keys.iter().any(|k| k.public_key == public_key) {
                return (Removal::Absent, false);
            }
            if keys.len() == 1 {
                return (Removal::Last, false);
            }
            keys.retain(|k| k.public_key != public_key);
            (Removal::Removed, true)
        })?;
        if removal == Removal::Removed {
            let _ = self.0.removed.send(public_key);
        }
        Ok(removal)
    }

    /// Every key the device trusts.
    #[cfg(test)]
    pub fn keys(&self) -> io::Result<Vec<AppKey>> {
        self.change(|keys| (keys.clone(), false))
    }

    /// Runs `f` on the keys under the lock. It gives what to answer and
    /// whether it changed them, and changed keys are written back whole.
    fn change<T>(&self, f: impl FnOnce(&mut Vec<AppKey>) -> (T, bool)) -> io::Result<T> {
        let folder = Folder::open(self.0.folder.clone())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.0.folder.join(LOCK))?;
        lock.lock()?;
        let mut keys = match folder.read(FILE)? {
            Some(text) => serde_json::from_str::<Vec<AppKey>>(text.expose()).map_err(|err| {
                io::Error::other(format!("{FILE} doesn't describe app keys: {err}"))
            })?,
            None => Vec::new(),
        };
        let (answer, changed) = f(&mut keys);
        if changed {
            if keys.is_empty() {
                folder.remove(FILE)?;
            } else {
                let json = serde_json::to_vec_pretty(&keys).map_err(io::Error::other)?;
                folder.write(FILE, &json)?;
            }
        }
        // Every read shows what it found, so a key `maxd authorize-key`
        // added from its own process shows at the next proof or change.
        // The hub sends nothing when nothing differs.
        if let Some(hub) = self.0.hub.get() {
            hub.replace(
                |entity| matches!(entity, Entity::AppKey(_)),
                keys.into_iter().map(Entity::AppKey),
            );
        }
        Ok(answer)
    }
}

/// `public_key` as the file keeps it, or none when it isn't base64url of
/// 32 bytes.
pub fn canonical(public_key: &str) -> Option<String> {
    let bytes: [u8; 32] = URL_SAFE_NO_PAD.decode(public_key).ok()?.try_into().ok()?;
    Some(URL_SAFE_NO_PAD.encode(bytes))
}

/// A label fit to show: one line, not too long, never empty.
pub(crate) fn clean_label(label: &str) -> String {
    let label: String = label
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let label: String = label.trim().chars().take(LABEL_MAX).collect();
    match label.trim_end() {
        "" => LABEL_DEFAULT.to_owned(),
        label => label.to_owned(),
    }
}

/// A test app key, and what a test does to prove it holds it.
#[cfg(test)]
pub mod testing {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use maxhq_core::prove::{self, Challenge, Kind, Statement};
    use maxhq_protocol::ids::{ClientId, IdempotencyKey};
    use maxhq_protocol::message::ServerHello;
    use maxhq_protocol::ClientMessage;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::{AppKeys, Asker, Decision};

    /// The test app key's seed.
    pub const SEED: [u8; 32] = [7; 32];

    /// The test app key, as the app sends it.
    pub fn public_key() -> String {
        public_key_of(&SEED)
    }

    /// The public half of the key with `seed`.
    pub fn public_key_of(seed: &[u8; 32]) -> String {
        let pair = Ed25519KeyPair::from_seed_unchecked(seed).expect("a key");
        URL_SAFE_NO_PAD.encode(pair.public_key())
    }

    /// The `Prove` a client with `client` sends after `hello`, signed with
    /// the app key whose seed is `seed`.
    pub fn prove_with(seed: &[u8; 32], hello: &ServerHello, client: ClientId) -> ClientMessage {
        prove_as(Kind::App, seed, hello, client)
    }

    /// The `Prove` of a `kind` connection, such as a phone's, signed with
    /// the key whose seed is `seed`.
    pub fn prove_as(
        kind: Kind,
        seed: &[u8; 32],
        hello: &ServerHello,
        client: ClientId,
    ) -> ClientMessage {
        let challenge = hello
            .challenge
            .as_deref()
            .and_then(Challenge::parse)
            .expect("a challenge in the hello");
        let statement = Statement {
            kind,
            device: hello.device,
            instance: hello.instance,
            client,
            challenge,
        };
        let proof = prove::sign(seed, &statement).expect("a proof");
        ClientMessage::Prove {
            key: IdempotencyKey::generate(),
            public_key: proof.public_key,
            signature: proof.signature,
        }
    }

    /// The `Prove` with the test app key.
    pub fn prove(hello: &ServerHello, client: ClientId) -> ClientMessage {
        prove_with(&SEED, hello, client)
    }

    /// Makes `keys` trust the test app key, as setup would.
    pub fn trust(keys: &AppKeys) {
        let decision = keys
            .enroll(&public_key(), "MaxHQ tests", Asker::Setup)
            .expect("the app keys file");
        assert!(
            matches!(decision, Decision::Add(_) | Decision::Trusted),
            "expected the test key in, got {decision:?}"
        );
    }
}

#[cfg(test)]
mod tests;
