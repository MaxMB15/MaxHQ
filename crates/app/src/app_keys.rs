// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This Mac's app key: the Ed25519 key the app proves itself to each
//! `maxd` with, right after the device's hello.
//!
//! The seed sits in one item in the system's store, read once per launch
//! and kept zeroized: the Keychain on a Mac, the keyring on Linux and
//! Windows, or a key file you chose when no keyring answered ([`store`]). While the app moves to a new key, the item holds the new seed
//! followed by the old one, so a device that's offline moves when the app
//! next reaches it. Beside the item, the app's database keeps a small
//! [`Record`]: the key's public half, the one it replaces and which devices
//! have moved, and which devices have trusted this Mac's key, so a refusal
//! can say the device no longer does.
//!
//! A missing item means a new key. One that can't be read, or isn't one or
//! two seeds, counts as tampered with: the app makes a new key and retires
//! the one the record names, which it removes from each device once the new
//! one is in.
//!
//! On each connection [`AppKeys`] proves the key. A device that doesn't
//! know it is asked to trust it, under "MaxHQ on <this Mac's name>". While
//! a new key waits on a device, the old one proves the connection first,
//! then the new one goes in over it and the old one comes out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures::future::BoxFuture;
use maxhq_client::proof::{Kind, Prover, Proving, Verdict};
use maxhq_client::LinkError;
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::message::ActionOutcome;
use serde::{Deserialize, Serialize};
use specta::Type;
use ssh_key::private::Ed25519Keypair;
use zeroize::Zeroizing;

use crate::keys::{Key, KeyStore};
use crate::registry::Registry;
pub use store::{remember, KeyHome, KeyHomeState, HOME_SETTING};
use store::{KeyFile, Place};

mod name;
pub mod store;

/// The setting the app keeps its key's [`Record`] under.
pub const SETTING: &str = "app_key";

/// How `maxd` starts turning down a good proof with a key it doesn't
/// trust. Only that refusal asks the device to trust the key.
pub(crate) const UNKNOWN_KEY: &str = "Unknown key:";
/// How `maxd` starts turning down a signature that doesn't check out.
const BAD_PROOF: &str = "Bad proof:";

/// A key's 32-byte seed.
type Seed = Zeroizing<[u8; 32]>;

/// This Mac's app key and how each device stands with it. Clones share it.
#[derive(Clone)]
pub struct AppKeys {
    inner: Arc<Inner>,
}

/// What every clone shares.
struct Inner {
    /// What a device calls the key it trusts.
    label: String,
    /// Where the key file goes, when you choose one.
    file: Option<PathBuf>,
    state: Mutex<State>,
    /// Saves the record and names the devices, once the app has them.
    keeper: OnceLock<Box<dyn Keeper>>,
}

/// The keys and what's known of each device.
struct State {
    /// Where the seeds live.
    store: Box<dyn KeyStore>,
    home: KeyHome,
    /// Whether to ask where to keep the key.
    ask: bool,
    current: Seed,
    /// The seed of the key before, while devices move off it, when the
    /// item still had it.
    previous: Option<Seed>,
    record: Record,
    /// The devices that turned the key down since the app opened.
    refusals: BTreeMap<DeviceId, Refusal>,
    /// Why the app made a new key as it opened, when it had to.
    remade: Option<String>,
    /// Whether the store holds the current key.
    kept: bool,
}

/// A device's refusal, in memory.
#[derive(Clone)]
struct Refusal {
    reason: String,
    kept_previous: bool,
}

/// What the app keeps about its key beside the Keychain item, in its
/// database under [`SETTING`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The current key's public half.
    #[serde(default)]
    public_key: Option<String>,
    /// The public half of the key before it, while devices move off it.
    #[serde(default)]
    previous: Option<String>,
    /// The devices that have moved to the current key.
    #[serde(default)]
    moved: BTreeSet<DeviceId>,
    /// The devices that have trusted this Mac's key.
    #[serde(default)]
    trusted: BTreeSet<DeviceId>,
}

/// Where the record goes and which devices the app knows.
pub trait Keeper: Send + Sync {
    /// Saves the record.
    fn save(&self, record: &Record);

    /// Every device the app has reached, which each move to a new key.
    fn devices(&self) -> Vec<DeviceId>;
}

/// This Mac's app key, as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct AppKeyState {
    /// The key, as base64url of its 32 bytes, unpadded.
    pub public_key: String,
    /// The key before it, while devices move off it.
    pub previous: Option<String>,
    /// The devices still on the key before, while one waits.
    pub waiting: Vec<DeviceId>,
    /// The devices that turned the key down since the app opened.
    pub refusals: Vec<KeyRefusal>,
    /// Why the app made a new key as it opened, when it had to.
    pub remade: Option<String>,
    /// Whether the Keychain holds the key. When it doesn't, the key lasts
    /// until you quit.
    pub kept: bool,
}

/// A device that turned this Mac's key down.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct KeyRefusal {
    pub device: DeviceId,
    /// What the device said.
    pub reason: String,
    /// Whether the device trusted this Mac's key before, so it no longer
    /// does.
    pub trusted_before: bool,
    /// Whether it turned down only a new key and still trusts the one
    /// before.
    pub kept_previous: bool,
}

/// What one connection's proof goes by, taken from the state as it starts.
struct Plan {
    current: Seed,
    /// The key to move the device off, with its seed when the app has it.
    previous: Option<(Option<Seed>, String)>,
    /// Whether the device has trusted this Mac's key.
    trusted: bool,
}

/// What a proof sends the device. [`Proving`] sends it over the
/// connection, and tests answer it themselves.
trait Steps {
    async fn prove(&self, seed: &[u8; 32], then_sync: bool) -> Result<ActionOutcome, LinkError>;
    async fn enroll(&self, label: &str) -> Result<ActionOutcome, LinkError>;
    async fn remove(&self, public_key: &str) -> Result<ActionOutcome, LinkError>;
    fn device(&self) -> DeviceId;
    fn name(&self) -> &str;
}

impl Steps for Proving {
    async fn prove(&self, seed: &[u8; 32], then_sync: bool) -> Result<ActionOutcome, LinkError> {
        Proving::prove(self, seed, then_sync).await
    }

    async fn enroll(&self, label: &str) -> Result<ActionOutcome, LinkError> {
        Proving::enroll(self, label).await
    }

    async fn remove(&self, public_key: &str) -> Result<ActionOutcome, LinkError> {
        Proving::remove(self, public_key).await
    }

    fn device(&self) -> DeviceId {
        Proving::device(self)
    }

    fn name(&self) -> &str {
        Proving::name(self)
    }
}

impl AppKeys {
    /// Reads the key from `store`, the Keychain, once per launch, with the
    /// record the app saved. Makes a new key when there's none, or when the
    /// one there looks tampered with. A key it can't keep in `store` lasts
    /// until the app quits. Fails only when the system gives no random
    /// bytes.
    pub fn open(
        store: Box<dyn KeyStore>,
        record: Option<Record>,
        label: String,
    ) -> Result<Self, String> {
        Self::open_in(Place::only(store, KeyHome::Keychain), record, label)
    }

    /// Reads the key from `place`, as [`AppKeys::open`] does.
    pub(crate) fn open_in(
        place: Place,
        record: Option<Record>,
        label: String,
    ) -> Result<Self, String> {
        let Place {
            store,
            home,
            ask,
            file,
        } = place;
        let place = home.name();
        let mut record = record.unwrap_or_default();
        let (current, previous, remade) = match store.load(Key::App) {
            Ok(Some(bytes)) if bytes.len() == 32 => (Some(seed_of(&bytes)), None, None),
            Ok(Some(bytes)) if bytes.len() == 64 => (
                Some(seed_of(&bytes[..32])),
                Some(seed_of(&bytes[32..])),
                None,
            ),
            Ok(None) => (None, None, record.public_key.as_ref().map(|_| home.lost())),
            Ok(Some(_)) => {
                tracing::warn!("this computer's app key in {place} isn't one MaxHQ made");
                let odd = format!(
                    "MaxHQ made a new app key, since the one in {place} wasn't one it made"
                );
                (None, None, Some(odd))
            }
            Err(error) => {
                tracing::warn!(%error, "couldn't read this computer's app key");
                let unread =
                    format!("MaxHQ made a new app key, since it couldn't read the one in {place}");
                (None, None, Some(unread))
            }
        };
        let mut kept = true;
        let current = match current {
            Some(current) => current,
            None => {
                let current = fresh()?;
                // The new key retires the one the record names, which a
                // device may still trust.
                if let Some(old) = record.public_key.take() {
                    record.previous = Some(old);
                    record.moved.clear();
                }
                if let Err(error) = store.save(Key::App, current.as_slice()) {
                    tracing::error!(%error, "couldn't keep this Mac's app key, so it lasts until the app quits");
                    kept = false;
                }
                current
            }
        };
        if let Some(previous) = &previous {
            record.previous = Some(public_key(previous));
        }
        record.public_key = Some(public_key(&current));
        Ok(Self {
            inner: Arc::new(Inner {
                label,
                file,
                state: Mutex::new(State {
                    store,
                    home,
                    ask,
                    current,
                    previous,
                    record,
                    refusals: BTreeMap::new(),
                    remade,
                    kept,
                }),
                keeper: OnceLock::new(),
            }),
        })
    }

    /// Hands over where the record goes and which devices the app knows,
    /// and saves the record as it stands.
    pub fn keep(&self, keeper: Box<dyn Keeper>) {
        if self.inner.keeper.set(keeper).is_ok() {
            let record = self.state().record.clone();
            self.save(&record);
        }
    }

    /// The key's public half, as base64url of its 32 bytes, unpadded.
    pub fn public_key(&self) -> String {
        public_key(&self.state().current)
    }

    /// What a device calls the key it trusts.
    pub fn label(&self) -> &str {
        &self.inner.label
    }

    /// Whether `public_key` is one of this Mac's: the key, or the one
    /// before while devices move off it.
    pub fn is_own(&self, public_key: &str) -> bool {
        let state = self.state();
        state.record.public_key.as_deref() == Some(public_key)
            || state.record.previous.as_deref() == Some(public_key)
    }

    /// The key and how the devices stand with it, as Settings shows it.
    pub fn view(&self) -> AppKeyState {
        self.settle();
        let devices = self.devices();
        let state = self.state();
        let waiting = match &state.record.previous {
            Some(_) => devices
                .into_iter()
                .filter(|device| !state.record.moved.contains(device))
                .collect(),
            None => Vec::new(),
        };
        AppKeyState {
            public_key: public_key(&state.current),
            previous: state.record.previous.clone(),
            waiting,
            refusals: state
                .refusals
                .iter()
                .map(|(device, refusal)| KeyRefusal {
                    device: *device,
                    reason: refusal.reason.clone(),
                    trusted_before: state.record.trusted.contains(device),
                    kept_previous: refusal.kept_previous,
                })
                .collect(),
            remade: state.remade.clone(),
            kept: state.kept,
        }
    }

    /// Makes a new key, which each device moves to as the app next proves
    /// itself there, keeping the old one until every device has. Refused
    /// while a new key still waits on a device, and when the Keychain
    /// can't keep the new one.
    pub fn rotate(&self) -> Result<(), String> {
        self.settle();
        let record = {
            let mut state = self.state();
            if state.record.previous.is_some() {
                return Err("This Mac's new app key hasn't reached every device yet".into());
            }
            let next = fresh()?;
            state
                .store
                .save(Key::App, &item(&next, Some(&state.current)))
                .map_err(|error| format!("Couldn't keep the new key: {error}"))?;
            let old = std::mem::replace(&mut state.current, next);
            state.record.previous = state.record.public_key.take();
            state.record.public_key = Some(public_key(&state.current));
            state.record.moved.clear();
            state.previous = Some(old);
            state.kept = true;
            state.remade = None;
            state.record.clone()
        };
        self.save(&record);
        Ok(())
    }

    /// Takes the key out of its store for good, as the app deletes its
    /// data. Until the app quits it proves with the key it holds.
    pub fn forget(&self) -> Result<(), String> {
        // Held through the forget, so a rotation can't put a key back
        // halfway.
        let mut state = self.state();
        state.store.forget(Key::App)?;
        state.kept = false;
        Ok(())
    }

    /// Where the key lives, for the key dialog and Settings. None on a Mac,
    /// whose Keychain is the only place it goes.
    pub fn home(&self) -> Option<KeyHomeState> {
        let state = self.state();
        (state.home != KeyHome::Keychain).then(|| KeyHomeState {
            home: state.home,
            ask: state.ask,
            file: self
                .inner
                .file
                .as_ref()
                .map(|file| file.display().to_string()),
        })
    }

    /// Moves the key to `home`: the keyring, or the key file you chose
    /// over the risk the dialog names. Memory only answers the dialog with
    /// Not now, and moves nothing. The caller saves the choice with
    /// [`remember`]. This may wait on the keyring's unlock prompt, so it
    /// runs off the async threads.
    pub fn keep_in(&self, home: KeyHome) -> Result<(), String> {
        let store: Box<dyn KeyStore> = match home {
            KeyHome::Keyring => store::keyring()?,
            KeyHome::File => match &self.inner.file {
                Some(file) => Box::new(KeyFile::new(file.clone())),
                None => return Err("MaxHQ has no data folder to keep a key file in".into()),
            },
            KeyHome::Memory => {
                let mut state = self.state();
                if state.home != KeyHome::Memory {
                    return Err(format!(
                        "This computer's app key is in {} already",
                        state.home.name()
                    ));
                }
                state.ask = false;
                return Ok(());
            }
            KeyHome::Keychain => return Err("Only a Mac keeps its app key in the Keychain".into()),
        };
        self.move_to(store, home)
    }

    /// Saves the key, and the one before while devices move off it, in
    /// `store`, then takes them out of the store they were in.
    fn move_to(&self, store: Box<dyn KeyStore>, home: KeyHome) -> Result<(), String> {
        let mut state = self.state();
        if state.home == home {
            state.ask = false;
            return Ok(());
        }
        store.save(Key::App, &item(&state.current, state.previous.as_ref()))?;
        if let Err(error) = state.store.forget(Key::App) {
            tracing::warn!(%error, from = state.home.name(), "couldn't take the app key out of where it was");
        }
        tracing::info!(to = home.name(), "moved this computer's app key");
        state.store = store;
        state.home = home;
        state.kept = true;
        state.ask = false;
        Ok(())
    }

    /// Proves one connection, and records how it went.
    async fn run(&self, steps: &impl Steps) -> Result<Verdict, LinkError> {
        let device = steps.device();
        let plan = self.plan(device);
        // While the device may still trust only the key before, it proves
        // the connection first, so the new one goes in with no card.
        let mut proven = false;
        if let Some((Some(seed), _)) = &plan.previous {
            proven = steps.prove(seed, plan.trusted).await? == ActionOutcome::Done;
        }
        let outcome = match steps.prove(&plan.current, plan.trusted).await? {
            // Only a key the device doesn't know asks to go in. A proof
            // that doesn't check out, or a device that can't read its
            // keys, would only fail again.
            ActionOutcome::Refused { reason } if reason.starts_with(UNKNOWN_KEY) => {
                let enrolled = steps.enroll(&self.inner.label).await?;
                // The connection goes on as the new key, not the one about
                // to come out.
                if enrolled == ActionOutcome::Done && proven {
                    let again = steps.prove(&plan.current, false).await?;
                    if again != ActionOutcome::Done {
                        tracing::warn!(%device, ?again, "a device took the new app key but not its proof");
                    }
                }
                enrolled
            }
            outcome => outcome,
        };
        if outcome != ActionOutcome::Done {
            let reason = reason_of(outcome);
            self.refused(device, &reason, proven);
            if proven {
                return Ok(Verdict::Proven);
            }
            let name = steps.name();
            return Ok(Verdict::Refused(if plan.trusted {
                format!("{name} no longer trusts this Mac's app key: {reason}")
            } else {
                format!("{name} doesn't trust this Mac's app key: {reason}")
            }));
        }
        if let Some((_, old)) = &plan.previous {
            // A device that never had it may turn the removal down, and
            // one that did no longer has it either way.
            let outcome = steps.remove(old).await?;
            if outcome != ActionOutcome::Done {
                tracing::info!(%device, ?outcome, "a device kept no old app key to remove");
            }
            self.moved(device);
        }
        self.trusted(device);
        Ok(Verdict::Proven)
    }

    /// What `device`'s proof goes by now.
    fn plan(&self, device: DeviceId) -> Plan {
        let state = self.state();
        let previous = state
            .record
            .previous
            .clone()
            .filter(|_| !state.record.moved.contains(&device))
            .map(|old| {
                let seed = state.previous.as_ref().map(|seed| seed_of(seed.as_slice()));
                (seed, old)
            });
        Plan {
            current: seed_of(state.current.as_slice()),
            previous,
            trusted: state.record.trusted.contains(&device),
        }
    }

    /// Notes that `device` turned the key down.
    fn refused(&self, device: DeviceId, reason: &str, kept_previous: bool) {
        self.state().refusals.insert(
            device,
            Refusal {
                reason: reason.to_owned(),
                kept_previous,
            },
        );
    }

    /// Notes that `device` trusts the key.
    fn trusted(&self, device: DeviceId) {
        let record = {
            let mut state = self.state();
            state.refusals.remove(&device);
            if !state.record.trusted.insert(device) {
                return;
            }
            state.record.clone()
        };
        self.save(&record);
    }

    /// Notes that `device` moved off the key before.
    fn moved(&self, device: DeviceId) {
        let record = {
            let mut state = self.state();
            state.record.moved.insert(device);
            state.record.clone()
        };
        self.save(&record);
        self.settle();
    }

    /// Lets go of the key before once every device the app knows has
    /// moved off it.
    fn settle(&self) {
        let Some(keeper) = self.inner.keeper.get() else {
            return;
        };
        let devices = keeper.devices();
        let record = {
            let mut state = self.state();
            if state.record.previous.is_none()
                || devices
                    .iter()
                    .any(|device| !state.record.moved.contains(device))
            {
                return;
            }
            if state.previous.is_some() || !state.kept {
                if let Err(error) = state.store.save(Key::App, state.current.as_slice()) {
                    tracing::warn!(%error, "couldn't let go of this Mac's old app key");
                    return;
                }
                state.kept = true;
            }
            state.previous = None;
            state.record.previous = None;
            state.record.moved.clear();
            state.record.clone()
        };
        tracing::info!("every device moved to this Mac's new app key");
        self.save(&record);
    }

    fn devices(&self) -> Vec<DeviceId> {
        self.inner
            .keeper
            .get()
            .map(|keeper| keeper.devices())
            .unwrap_or_default()
    }

    fn save(&self, record: &Record) {
        if let Some(keeper) = self.inner.keeper.get() {
            keeper.save(record);
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Prover for AppKeys {
    fn kind(&self) -> Kind {
        Kind::App
    }

    fn prove(&self, proving: Proving) -> BoxFuture<'static, Verdict> {
        let keys = self.clone();
        Box::pin(async move {
            match keys.run(&proving).await {
                Ok(verdict) => verdict,
                Err(error) => {
                    tracing::debug!(%error, "a proof ended with its connection");
                    Verdict::Lost
                }
            }
        })
    }
}

/// This computer's app key from where the system keeps it ([`store`]),
/// with the record the app saved in `registry`. `data` is the app's data
/// folder, where a key file goes if you choose one. None when the system
/// gives no random bytes for a new key, or has no store, so the app proves
/// nothing.
pub fn from_system(registry: &Registry, data: Option<&Path>) -> Option<AppKeys> {
    let place = store::place(registry, data)?;
    let record = registry.setting::<Record>(SETTING).unwrap_or_else(|error| {
        tracing::warn!(%error, "couldn't read what the app knows of its key");
        None
    });
    match AppKeys::open_in(place, record, label()) {
        Ok(keys) => Some(keys),
        Err(error) => {
            tracing::error!(%error, "this computer has no app key, so it proves nothing");
            None
        }
    }
}

/// What a device's answer says, for a refusal, which follows words such
/// as "doesn't trust this Mac's app key:". It drops the prefix `maxd` sorts
/// its refusals by, which would only repeat them.
pub(crate) fn reason_of(outcome: ActionOutcome) -> String {
    match outcome {
        ActionOutcome::Refused { reason } => [UNKNOWN_KEY, BAD_PROOF]
            .iter()
            .find_map(|prefix| reason.strip_prefix(prefix))
            .map_or(reason.as_str(), str::trim_start)
            .to_owned(),
        other => format!("it answered {other:?}"),
    }
}

/// A new random seed.
fn fresh() -> Result<Seed, String> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut_slice())
        .map_err(|error| format!("Couldn't make an app key: {error}"))?;
    Ok(seed)
}

/// The seed in the first 32 bytes of `bytes`.
fn seed_of(bytes: &[u8]) -> Seed {
    let mut seed = Zeroizing::new([0u8; 32]);
    seed.copy_from_slice(&bytes[..32]);
    seed
}

/// The public half of the key `seed` makes, as base64url, unpadded.
fn public_key(seed: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(Ed25519Keypair::from_seed(seed).public.0)
}

/// The seed `first`, followed by `second` while devices move off it: one
/// item, as the store keeps it.
fn item(first: &Seed, second: Option<&Seed>) -> Zeroizing<Vec<u8>> {
    let mut item = Zeroizing::new(Vec::with_capacity(64));
    item.extend_from_slice(first.as_slice());
    if let Some(second) = second {
        item.extend_from_slice(second.as_slice());
    }
    item
}

/// What a device calls this computer's key: "MaxHQ on <its name>".
pub fn label() -> String {
    format!("MaxHQ on {}", name::computer_name())
}

#[cfg(test)]
mod tests;
