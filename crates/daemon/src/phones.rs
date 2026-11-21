// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Phones. A phone pairs through one device:
//!
//! 1. The Mac asks for a pairing with the phone's book. `maxd` makes a
//!    one-time Ed25519 key, lets it in to run only `maxd proxy --pair`,
//!    and answers with its seed for the QR code.
//! 2. The phone logs in with that key. Its first frame names the pairing,
//!    which takes the key's line out at once, so it works once. A pairing
//!    nobody uses is gone after [`PAIR_WITHIN`].
//! 3. The phone sends its own keys. `maxd` lets the phone's key in to run
//!    only `maxd proxy --phone`, keeps the phone and its book, and answers
//!    with both.
//!
//! A phone may pair over network access instead, whose `POST /pair`
//! carries the seed itself and takes the same pairing.
//!
//! The other devices in the book let the phone in when the Mac sends them
//! `AddPhone`. Each device publishes a [`Phone`] for every phone it lets
//! in, and the device a phone paired through publishes its [`PhoneBook`].

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_core::authorized_keys;
use maxhq_protocol::ids::{DeviceId, PairId, PhoneId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::phones::{Phone, PhoneBook, PhoneBookEntry, PhoneMac, PushToken};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::Secret;
use ring::rand::{SecureRandom, SystemRandom};
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::PublicKey;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::agents::same_hash;
use crate::db::Db;
use crate::hub::Hub;
use crate::secrets::files::Folder;
use crate::session::clock::Clock;
use crate::sshkey::ed25519_line;

/// The refusal for every phone request on a device that doesn't pair.
pub const NOT_YET: &str = "Phones aren't set up yet.";
/// The refusal for what only the Mac does, to a phone whose switch is off.
pub const PHONE_CANT: &str = "A phone can't do that. To let this phone, turn on Let this phone do \
                              everything for it in the Mac's Settings, under Phones.";
/// How long a one-time pairing key works.
pub const PAIR_WITHIN: Duration = Duration::from_secs(10 * 60);
/// The refusal for a pairing that no longer waits.
const PAIR_OVER: &str = "That pairing is over. Start again on the Mac.";
/// The phones folder under `maxd`'s data folder.
pub const DIR: &str = "phones";

/// The phones' file in the folder.
const FILE: &str = "phones.json";
/// The most pairings that wait at once.
const PAIRS_MAX: usize = 4;
/// The most devices a book holds, and routes a device has.
const BOOK_MAX: usize = 64;
const ROUTES_MAX: usize = 8;
/// The longest name and host.
const NAME_MAX: usize = 64;
const HOST_MAX: usize = 255;
/// A certificate pin's length: base64url, without padding, of 32 bytes.
const PIN_LEN: usize = 43;

/// The phones this device lets in. Clones share them.
#[derive(Clone)]
pub struct Phones(Arc<Inner>);

struct Inner {
    hub: Hub,
    folder: Folder,
    /// Which phones may do everything here, which the database keeps.
    db: Db,
    /// The `authorized_keys` file the lines go in.
    keys: PathBuf,
    clock: Arc<dyn Clock>,
    /// The most pairings that wait at once, [`PAIRS_MAX`] but in tests.
    pairs_max: usize,
    state: Mutex<State>,
    /// Each phone removed, so its connections close.
    removed: broadcast::Sender<PhoneId>,
}

#[derive(Default)]
struct State {
    phones: BTreeMap<PhoneId, Kept>,
    pairs: HashMap<PairId, Waiting>,
}

/// A phone in the file: what's published, and its book when it paired
/// here.
#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    phone: Phone,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    book: Option<Vec<PhoneBookEntry>>,
    /// The Mac that made the book, when it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mac: Option<PhoneMac>,
}

/// A pairing that waits for its phone.
struct Waiting {
    book: Vec<PhoneBookEntry>,
    mac: Option<PhoneMac>,
    until: Instant,
    /// The one-time key's public half, which a seed must derive.
    public: [u8; 32],
}

/// A pairing a connection took, which it may finish once.
#[derive(Debug)]
pub struct Pairing {
    book: Vec<PhoneBookEntry>,
    mac: Option<PhoneMac>,
}

impl Phones {
    /// Starts with the phones `folder` holds, each with its switch as `db`
    /// keeps it, and publishes them. Lines of pairings from before a
    /// restart are taken out, since none of them can finish.
    pub fn open(hub: Hub, folder: Folder, db: Db, keys: PathBuf, clock: Arc<dyn Clock>) -> Self {
        folder.clear_leftovers();
        let mut phones: BTreeMap<PhoneId, Kept> = match folder.read(FILE) {
            Ok(Some(text)) => match serde_json::from_str::<Vec<Kept>>(text.expose()) {
                Ok(kept) => kept.into_iter().map(|k| (k.phone.id, k)).collect(),
                Err(_) => {
                    warn!("skipped the phones: {FILE} doesn't describe any");
                    BTreeMap::new()
                }
            },
            Ok(None) => BTreeMap::new(),
            Err(err) => {
                warn!("couldn't read the phones: {err}");
                BTreeMap::new()
            }
        };
        // A switch the database doesn't have is off.
        let everything = db.phones_everything().unwrap_or_else(|err| {
            warn!("couldn't read which phones may do everything: {err}");
            Default::default()
        });
        for kept in phones.values_mut() {
            kept.phone.everything = everything.contains(&kept.phone.id);
        }
        if let Err(err) =
            authorized_keys::remove_where(&keys, |tag| tag.starts_with(authorized_keys::PAIR_TAG))
        {
            warn!("couldn't take out old pairing keys: {err}");
        }
        let me = hub.device().id;
        hub.apply(phones.values().flat_map(|kept| entities(kept, me)));
        Self(Arc::new(Inner {
            hub,
            folder,
            db,
            keys,
            clock,
            pairs_max: PAIRS_MAX,
            state: Mutex::new(State {
                phones,
                pairs: HashMap::new(),
            }),
            removed: broadcast::channel(16).0,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn me(&self) -> DeviceId {
        self.0.hub.device().id
    }

    /// Answers `BeginPhonePair`: a one-time key for a phone with `book`,
    /// which `mac` made.
    pub fn begin(&self, book: Vec<PhoneBookEntry>, mac: Option<PhoneMac>) -> ActionOutcome {
        let book = match check_book(book) {
            Ok(book) => book,
            Err(reason) => return refused(reason),
        };
        let mac = check_mac(mac);
        let me = self.me();
        let Some(host_key) = book
            .iter()
            .find(|e| e.device == me)
            .map(|e| e.host_key.clone())
        else {
            return refused("The phone's devices have to include the one it pairs through.");
        };
        let now = self.0.clock.now();
        let pair = PairId::generate();
        {
            let mut state = self.lock();
            state.pairs.retain(|_, waiting| waiting.until > now);
            if state.pairs.len() >= self.0.pairs_max {
                return refused("Too many phones are pairing here. Try again in a few minutes.");
            }
        }
        let mut seed = Zeroizing::new([0u8; 32]);
        if SystemRandom::new().fill(seed.as_mut()).is_err() {
            return refused("This device gave no random bytes for the key.");
        }
        let key = Ed25519Keypair::from_seed(&seed).public;
        let public = match PublicKey::from(key).to_openssh() {
            Ok(public) => public,
            Err(err) => return refused(format!("This device couldn't make the key: {err}")),
        };
        let line = match authorized_keys::pair_line(&public, &pair.to_string()) {
            Ok(line) => line,
            Err(reason) => return refused(reason),
        };
        if let Err(err) = authorized_keys::add(&self.0.keys, &line) {
            return refused(format!("This device couldn't let the key in: {err}"));
        }
        let until = now + PAIR_WITHIN;
        self.lock().pairs.insert(
            pair,
            Waiting {
                book,
                mac,
                until,
                public: key.0,
            },
        );
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let phones = self.clone();
            runtime.spawn(async move {
                phones.0.clock.sleep_until(until).await;
                phones.expire(pair);
            });
        }
        info!(%pair, "a phone may pair for the next ten minutes");
        ActionOutcome::PhonePair {
            pair,
            seed: Secret::new(STANDARD.encode(seed.as_ref())),
            host_key,
            expires_at: self.0.clock.wall().after(PAIR_WITHIN),
        }
    }

    /// Takes a pairing nobody used out, with its key.
    fn expire(&self, pair: PairId) {
        if self.lock().pairs.remove(&pair).is_some() {
            self.drop_key(pair);
            info!(%pair, "a pairing went unused");
        }
    }

    fn drop_key(&self, pair: PairId) {
        if let Err(err) =
            authorized_keys::remove(&self.0.keys, &authorized_keys::pair_tag(&pair.to_string()))
        {
            warn!(%pair, "couldn't take out a pairing key: {err}");
        }
    }

    /// The one-time key's public half of `pair`, while it still waits. A
    /// connection whose first frame names the pairing proves it holds that
    /// key before it [`Phones::take`]s the pairing.
    pub fn waiting(&self, pair: PairId) -> Result<[u8; 32], &'static str> {
        let now = self.0.clock.now();
        self.lock()
            .pairs
            .get(&pair)
            .filter(|waiting| waiting.until > now)
            .map(|waiting| waiting.public)
            .ok_or(PAIR_OVER)
    }

    /// Takes the pairing `pair`, and its key, so the key works once. A
    /// connection takes it once it proves it holds the one-time key.
    pub fn take(&self, pair: PairId) -> Result<Pairing, &'static str> {
        let waiting = self.lock().pairs.remove(&pair);
        self.drop_key(pair);
        match waiting {
            Some(waiting) if waiting.until > self.0.clock.now() => Ok(Pairing {
                book: waiting.book,
                mac: waiting.mac,
            }),
            _ => Err(PAIR_OVER),
        }
    }

    /// Whether `seed`, as base64 of its 32 bytes, is the one-time key of
    /// `pair`, which still waits. Network access takes a pairing only
    /// then, since its request carries the seed instead of an SSH login.
    pub fn seed_fits(&self, pair: PairId, seed: &Secret) -> bool {
        let Ok(seed) = STANDARD.decode(seed.expose()).map(Zeroizing::new) else {
            return false;
        };
        let Ok(seed) = <[u8; 32]>::try_from(seed.as_slice()).map(Zeroizing::new) else {
            return false;
        };
        let public = Ed25519Keypair::from_seed(&seed).public.0;
        let now = self.0.clock.now();
        self.lock()
            .pairs
            .get(&pair)
            .is_some_and(|waiting| waiting.until > now && same_hash(&waiting.public, &public))
    }

    /// Answers `PairPhone` on the connection that took `pairing`.
    pub fn pair(
        &self,
        pairing: Pairing,
        name: &str,
        ssh_public: &str,
        push_public: &str,
    ) -> ActionOutcome {
        let Some(ssh_public) = ed25519_line(ssh_public) else {
            return refused("The phone's key isn't an ed25519 key.");
        };
        let phone = PhoneId::derive(&ssh_public);
        let Pairing { book, mac } = pairing;
        let kept = Some(book.clone());
        match self.let_in(phone, name, ssh_public, push_public, kept, mac.clone()) {
            ActionOutcome::Done => {
                info!(%phone, "a phone paired");
                ActionOutcome::PhonePaired { phone, book, mac }
            }
            other => other,
        }
    }

    /// Answers `AddPhone`, for a phone paired through another device.
    pub fn add(
        &self,
        phone: PhoneId,
        name: &str,
        ssh_public: &str,
        push_public: &str,
    ) -> ActionOutcome {
        let Some(ssh_public) = ed25519_line(ssh_public) else {
            return refused("The phone's key isn't an ed25519 key.");
        };
        // A phone's ID comes from its key, as pairing gives it.
        if phone != PhoneId::derive(&ssh_public) {
            return refused("The phone's ID doesn't match its key.");
        }
        let (book, mac) = self
            .lock()
            .phones
            .get(&phone)
            .map(|kept| (kept.book.clone(), kept.mac.clone()))
            .unwrap_or_default();
        match self.let_in(phone, name, ssh_public, push_public, book, mac) {
            // With network access on, the phone gets a code for it too.
            ActionOutcome::Done => crate::netaccess::added(phone),
            other => other,
        }
    }

    /// Writes the phone's line and keeps it, with its book and the Mac that
    /// made it when it has one here.
    fn let_in(
        &self,
        phone: PhoneId,
        name: &str,
        ssh_public: String,
        push_public: &str,
        book: Option<Vec<PhoneBookEntry>>,
        mac: Option<PhoneMac>,
    ) -> ActionOutcome {
        let name = name.trim();
        if name.is_empty() || name.len() > NAME_MAX || name.chars().any(char::is_control) {
            return refused("Give the phone a short name.");
        }
        if STANDARD
            .decode(push_public)
            .map_or(true, |key| key.len() != 32)
        {
            return refused("The phone's push key isn't an X25519 key.");
        }
        let id = phone.to_string();
        let line = match authorized_keys::phone_line(&ssh_public, &id) {
            Ok(line) => line,
            Err(reason) => return refused(reason),
        };
        let tag = authorized_keys::phone_tag(&id);
        let written = authorized_keys::remove(&self.0.keys, &tag)
            .and_then(|_| authorized_keys::add(&self.0.keys, &line));
        if let Err(err) = written {
            return refused(format!("This device couldn't let the phone in: {err}"));
        }
        let mut state = self.lock();
        let before = state.phones.get(&phone).cloned();
        let kept = Kept {
            phone: Phone {
                id: phone,
                device: self.me(),
                name: name.to_owned(),
                ssh_public,
                push_public: push_public.to_owned(),
                paired_at: before
                    .as_ref()
                    .map_or_else(|| self.0.clock.wall(), |b| b.phone.paired_at),
                push: before
                    .as_ref()
                    .map(|b| b.phone.push.clone())
                    .unwrap_or_default(),
                everything: before.as_ref().is_some_and(|b| b.phone.everything),
            },
            book,
            mac,
        };
        state.phones.insert(phone, kept.clone());
        if let Err(err) = save(&self.0.folder, &state.phones) {
            match before {
                Some(before) => state.phones.insert(phone, before),
                None => {
                    let _ = authorized_keys::remove(&self.0.keys, &tag);
                    state.phones.remove(&phone)
                }
            };
            return refused(format!("This device couldn't keep the phone: {err}"));
        }
        drop(state);
        self.0.hub.apply(entities(&kept, self.me()));
        ActionOutcome::Done
    }

    /// Answers `SetPhoneBook`, for a phone that paired here. A book that
    /// doesn't name its Mac keeps the one before.
    pub fn set_book(
        &self,
        phone: PhoneId,
        book: Vec<PhoneBookEntry>,
        mac: Option<PhoneMac>,
    ) -> ActionOutcome {
        let book = match check_book(book) {
            Ok(book) => book,
            Err(reason) => return refused(reason),
        };
        let me = self.me();
        let mut state = self.lock();
        let Some(kept) = state.phones.get_mut(&phone).filter(|k| k.book.is_some()) else {
            return refused("That phone didn't pair through this device.");
        };
        let before = (kept.book.replace(book), kept.mac.clone());
        if let Some(mac) = check_mac(mac) {
            kept.mac = Some(mac);
        }
        let changed = kept.clone();
        if let Err(err) = save(&self.0.folder, &state.phones) {
            if let Some(kept) = state.phones.get_mut(&phone) {
                (kept.book, kept.mac) = before;
            }
            return refused(format!("This device couldn't keep the book: {err}"));
        }
        drop(state);
        self.0.hub.apply(entities(&changed, me));
        ActionOutcome::Done
    }

    /// Answers `SetPhoneEverything`, for a phone this device lets in.
    pub fn set_everything(&self, phone: PhoneId, everything: bool) -> ActionOutcome {
        let mut state = self.lock();
        let Some(kept) = state.phones.get_mut(&phone) else {
            return refused("This device doesn't let that phone in.");
        };
        if kept.phone.everything == everything {
            return ActionOutcome::Done;
        }
        if let Err(err) = self.0.db.set_phone_everything(phone, everything) {
            return refused(format!("This device couldn't keep the switch: {err}"));
        }
        kept.phone.everything = everything;
        let changed = kept.phone.clone();
        drop(state);
        info!(%phone, everything, "a phone's switch changed");
        self.0.hub.apply([Change::Upsert {
            value: Entity::Phone(changed),
        }]);
        ActionOutcome::Done
    }

    /// Whether `phone` may do everything here but let other keys in.
    pub fn everything(&self, phone: PhoneId) -> bool {
        self.lock()
            .phones
            .get(&phone)
            .is_some_and(|kept| kept.phone.everything)
    }

    /// Answers `RemovePhone`: the phone's network sessions and codes, its
    /// line, its push token, its book and its connections go. A phone
    /// that isn't here is done already.
    pub fn remove(&self, phone: PhoneId) -> ActionOutcome {
        if let Err(reason) = crate::netaccess::forget(phone) {
            return refused(reason);
        }
        if let Err(err) = authorized_keys::remove(
            &self.0.keys,
            &authorized_keys::phone_tag(&phone.to_string()),
        ) {
            return refused(format!(
                "This device couldn't take the phone's key out: {err}"
            ));
        }
        // A phone let in again starts with the switch off.
        if let Err(err) = self.0.db.set_phone_everything(phone, false) {
            return refused(format!("This device couldn't forget the phone: {err}"));
        }
        let mut state = self.lock();
        if let Some(kept) = state.phones.remove(&phone) {
            if let Err(err) = save(&self.0.folder, &state.phones) {
                state.phones.insert(phone, kept);
                return refused(format!("This device couldn't forget the phone: {err}"));
            }
            info!(%phone, "a phone was removed");
        }
        drop(state);
        self.0.hub.apply([
            Change::Remove {
                key: EntityKey::Phone {
                    phone,
                    device: self.me(),
                },
            },
            Change::Remove {
                key: EntityKey::PhoneBook(phone),
            },
        ]);
        crate::push::forget(phone);
        let _ = self.0.removed.send(phone);
        ActionOutcome::Done
    }

    /// The SSH key of `phone`, when this device lets it in.
    pub fn ssh_public(&self, phone: PhoneId) -> Option<String> {
        self.lock()
            .phones
            .get(&phone)
            .map(|kept| kept.phone.ssh_public.clone())
    }

    /// Whether this device lets `phone` in.
    pub fn knows(&self, phone: PhoneId) -> bool {
        self.lock().phones.contains_key(&phone)
    }

    /// Each phone removed from now on.
    pub fn removed(&self) -> broadcast::Receiver<PhoneId> {
        self.0.removed.subscribe()
    }

    /// Answers `SetPushToken` from `phone`'s own connection, and says on
    /// the phone what this device now pushes to it.
    pub fn set_token(&self, phone: PhoneId, push: PushToken) -> ActionOutcome {
        let mut kinds = push.kinds.clone();
        let outcome = crate::push::set_token(phone, push);
        if outcome != ActionOutcome::Done {
            return outcome;
        }
        kinds.sort_unstable();
        kinds.dedup();
        let mut state = self.lock();
        let Some(kept) = state.phones.get_mut(&phone) else {
            return outcome;
        };
        if kept.phone.push == kinds {
            return outcome;
        }
        kept.phone.push = kinds;
        let phone = kept.phone.clone();
        if let Err(err) = save(&self.0.folder, &state.phones) {
            warn!(id = %phone.id, "couldn't keep what's pushed to a phone: {err}");
        }
        drop(state);
        self.0.hub.apply([Change::Upsert {
            value: Entity::Phone(phone),
        }]);
        outcome
    }
}

/// What `kept` publishes.
fn entities(kept: &Kept, me: DeviceId) -> Vec<Change> {
    let mut changes = vec![Change::Upsert {
        value: Entity::Phone(kept.phone.clone()),
    }];
    if let Some(devices) = &kept.book {
        changes.push(Change::Upsert {
            value: Entity::PhoneBook(PhoneBook {
                phone: kept.phone.id,
                device: me,
                devices: devices.clone(),
                mac: kept.mac.clone(),
            }),
        });
    }
    changes
}

fn save(folder: &Folder, phones: &BTreeMap<PhoneId, Kept>) -> io::Result<()> {
    if phones.is_empty() {
        return folder.remove(FILE);
    }
    let kept: Vec<&Kept> = phones.values().collect();
    let json = serde_json::to_vec(&kept).map_err(io::Error::other)?;
    folder.write(FILE, &json)
}

/// The book, checked: each device once, with a name, plain routes and an
/// ed25519 host key, which comes back in the form ssh writes.
fn check_book(mut book: Vec<PhoneBookEntry>) -> Result<Vec<PhoneBookEntry>, String> {
    if book.is_empty() || book.len() > BOOK_MAX {
        return Err("The phone needs at least one device to reach.".into());
    }
    let plain = |text: &str, max: usize| {
        !text.is_empty()
            && text.len() <= max
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.:@".contains(c))
    };
    let mut seen = Vec::with_capacity(book.len());
    for entry in &mut book {
        if seen.contains(&entry.device) {
            return Err(format!("{} is in the phone's devices twice.", entry.name));
        }
        seen.push(entry.device);
        let name = entry.name.trim();
        if name.is_empty() || name.len() > NAME_MAX || name.chars().any(char::is_control) {
            return Err("Each of the phone's devices needs a short name.".into());
        }
        entry.name = name.to_owned();
        // Network access alone will do, for a device with no SSH.
        if (entry.routes.is_empty() && entry.network.is_none()) || entry.routes.len() > ROUTES_MAX {
            return Err(format!("The phone has no address for {}.", entry.name));
        }
        if entry.routes.iter().any(|route| {
            !plain(&route.user, NAME_MAX) || !plain(&route.host, HOST_MAX) || route.port == 0
        }) {
            return Err(format!(
                "An address for {} isn't a plain SSH address.",
                entry.name
            ));
        }
        if let Some(network) = &entry.network
            && (network.hosts.is_empty()
                || network.hosts.len() > ROUTES_MAX
                || network.hosts.iter().any(|host| !plain(host, HOST_MAX))
                || network.port == 0
                || network.pin.len() != PIN_LEN
                || !plain(&network.pin, PIN_LEN))
        {
            return Err(format!(
                "The network address for {} isn't a plain one.",
                entry.name
            ));
        }
        if !entry.routes.is_empty() {
            entry.host_key = ed25519_line(&entry.host_key)
                .ok_or_else(|| format!("The host key for {} isn't an ed25519 key.", entry.name))?;
        }
    }
    Ok(book)
}

/// The Mac's name, trimmed, or none when it isn't a short one, since the
/// phone then names the book by its keeper.
fn check_mac(mac: Option<PhoneMac>) -> Option<PhoneMac> {
    let mut mac = mac?;
    let name = mac.name.trim();
    if name.is_empty() || name.len() > NAME_MAX || name.chars().any(char::is_control) {
        return None;
    }
    mac.name = name.to_owned();
    Some(mac)
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// The service the free functions reach, once `maxd` has started it.
static PHONES: OnceLock<Phones> = OnceLock::new();

/// Makes `phones` the service the free functions reach.
pub fn install(phones: Phones) {
    let _ = PHONES.set(phones);
}

/// The service, once `maxd` has started it.
pub fn installed() -> Option<&'static Phones> {
    PHONES.get()
}

/// Whether the Mac turned `phone`'s switch on here, so it may do what
/// only the Mac does, short of letting other keys in.
pub fn lets(phone: PhoneId) -> bool {
    installed().is_some_and(|phones| phones.everything(phone))
}

/// Runs `f` on the service, or refuses on a device that doesn't pair.
pub fn with(f: impl FnOnce(&Phones) -> ActionOutcome) -> ActionOutcome {
    match PHONES.get() {
        Some(phones) => f(phones),
        None => refused(NOT_YET),
    }
}

/// The service for tests of other modules, installed once for the whole
/// test run, with its files in a folder of its own.
#[cfg(test)]
pub fn for_tests() -> &'static Phones {
    PHONES.get_or_init(|| {
        let temp = std::mem::ManuallyDrop::new(crate::testing::TempDir::new());
        let mut phones = Phones::open(
            crate::secrets::testing::hub_for("devbox"),
            Folder::open(temp.path().join(DIR)).expect("phones folder"),
            Db::open_in_memory().expect("database"),
            temp.path().join("authorized_keys"),
            Arc::new(crate::session::clock::SystemClock),
        );
        // Every test that pairs shares this store, at the same time, and
        // some leave a pairing waiting on purpose, so the cap fits them all.
        Arc::get_mut(&mut phones.0).expect("a new store").pairs_max = 64;
        phones
    })
}

/// The public half of the ed25519 key made from `seed` as an OpenSSH line,
/// as a phone or a host shows it, for tests of other modules.
#[cfg(test)]
pub fn openssh_for_tests(seed: &[u8; 32]) -> String {
    PublicKey::from(Ed25519Keypair::from_seed(seed).public)
        .to_openssh()
        .expect("an OpenSSH line")
}

/// A phone book of just the device [`for_tests`] runs as, reached over
/// SSH, for tests of other modules.
#[cfg(test)]
pub fn book_for_tests() -> Vec<PhoneBookEntry> {
    vec![PhoneBookEntry {
        device: DeviceId::derive("devbox"),
        name: "devbox".into(),
        routes: vec![maxhq_protocol::phones::PhoneRoute {
            user: "dev".into(),
            host: "devbox.local".into(),
            port: 22,
        }],
        host_key: openssh_for_tests(&[212; 32]),
        network: None,
    }]
}

mod store;
#[cfg(test)]
mod tests;
