// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Phones, as the Mac pairs and removes them. The Mac builds the phone's
//! book from the devices you tick, with their routes resolved the way
//! device links resolve them and the network access each publishes, and
//! has one device start the pairing. Once the phone pairs, the Mac lets
//! its key in on the other devices in its book, each when it's next
//! connected. A device with network access answers with a one-time code,
//! which the Mac puts in the phone's book until the phone has its token
//! there. A code lasts minutes, so the Mac can have those devices make
//! new ones for a phone that didn't trade them in time. Remove takes the
//! phone off every device that has it, again each when it's next
//! connected. The switch that lets a phone do everything reaches each
//! device that has the phone the same way.
//!
//! A device can report any phone it likes, with any key and any book. So
//! the Mac only acts on a phone it approved: one that answers a pairing
//! you started here, or one it already saw when it began keeping this.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_client::LinkSpec;
use maxhq_protocol::ids::{DeviceId, IdempotencyKey, LinkId, PairId, PhoneId};
use maxhq_protocol::link::LinkState;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::netaccess::PhoneNetwork;
use maxhq_protocol::phones::{Phone, PhoneBook, PhoneBookEntry, PhoneMac, PhoneRoute};
use maxhq_protocol::secret::Secret;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::ClientMessage;
use serde::{Deserialize, Serialize};
use specta::Type;
use zeroize::Zeroizing;

use super::peers::{find, Known, PeerFacts};
use super::{device_links, failed, lock, Connector, Core, Devices, LinkControl};
use crate::fleet::{check_can, Needs};
use crate::merge::Merge;
use crate::pair_code::{qr_path, PairCode, QrPath};
use crate::registry::local_link;
use crate::request::CommandError;

/// The setting the phone addresses are kept under, by device.
const ADDRESSES: &str = "phone_addresses";

/// The setting the removals still to reach a device are kept under.
const REMOVALS: &str = "phone_removals";

/// The setting the pairings you started are kept under, until each one's
/// phone shows up or it lapses.
const PAIRINGS: &str = "phone_pairings";

/// The setting the phones the Mac acts on are kept under.
const APPROVED: &str = "phone_approved";

/// The setting the switch you want for each phone is kept under.
const EVERYTHING: &str = "phone_everything";

/// How long after its code stops working the Mac waits for a pairing's
/// phone, since the phone may pair while the Mac is away.
const PAIRING_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// The SSH port when an address names none.
const SSH_PORT: u16 = 22;

/// A device a phone can reach, as Add phone lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PhoneDevice {
    /// The device.
    pub device: DeviceId,
    /// Its name.
    pub name: String,
    /// The address you gave for the phone, if any.
    pub address: Option<String>,
    /// Why the phone can't reach it, when it can't.
    pub note: Option<String>,
}

/// A pairing that waits for its phone, for the Mac to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PhonePairing {
    pub pair: PairId,
    /// The device the phone pairs through.
    pub device: DeviceId,
    /// The code to scan.
    pub qr: QrPath,
    /// When the code stops working.
    pub expires_at: Timestamp,
}

/// The phones on their way out.
type Removals = BTreeMap<PhoneId, Removal>;

/// Whether you want each phone to do everything, for each device that has
/// it. A phone you never switched isn't here, and devices keep what they
/// have for it.
type Wishes = BTreeMap<PhoneId, bool>;

/// The phones and devices that took a switch in one pass.
pub(super) type Switched = BTreeSet<(PhoneId, DeviceId)>;

/// The one-time codes devices gave for phones, by phone and device.
pub(super) type Codes = BTreeMap<(PhoneId, DeviceId), Secret>;

/// One phone on its way out. It stays until no device shows the phone, so
/// a device that drops it before the one that keeps its book doesn't get
/// it again, or until the phone pairs again.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Removal {
    /// When you removed it.
    at: Timestamp,
    /// The devices still to take it off.
    devices: BTreeSet<DeviceId>,
}

/// A pairing you started on this Mac, waiting for its phone.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Pairing {
    /// The device the phone pairs through, which keeps its book.
    keeper: DeviceId,
    /// The devices you ticked.
    devices: BTreeSet<DeviceId>,
    /// When the Mac stops waiting for the phone.
    until: Timestamp,
}

/// The phones the Mac acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Approved {
    /// Whether the phones from before the Mac kept this are in.
    settled: bool,
    phones: BTreeMap<PhoneId, Keeping>,
}

/// Which device keeps a phone's book, and the devices that book may reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Keeping {
    keeper: DeviceId,
    devices: BTreeSet<DeviceId>,
}

impl Approved {
    /// The devices `book` may reach, when it comes from the device that
    /// keeps an approved phone. A book from any other device is refused,
    /// since each device may report whatever book it likes for itself.
    fn reach(&self, book: &PhoneBook) -> Option<&BTreeSet<DeviceId>> {
        self.phones
            .get(&book.phone)
            .filter(|known| known.keeper == book.device)
            .map(|known| &known.devices)
    }
}

impl<C: Connector> Devices<C> {
    /// Every device whose `maxd` takes phones, by name, with whether a
    /// phone can reach it.
    pub async fn phone_devices(&self) -> Result<Vec<PhoneDevice>, CommandError> {
        let facts = self.core.facts()?;
        let devices = self.core.view.read(phone_hosts);
        let addresses = self.core.addresses();
        let mut found = Vec::with_capacity(devices.len());
        for device in devices {
            let (name, entry) = self.core.entry(facts, device, &addresses).await;
            found.push(PhoneDevice {
                device,
                name,
                address: addresses.get(&device).cloned(),
                note: entry.err(),
            });
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(found)
    }

    /// Starts pairing a phone through `through`, for it to reach each of
    /// `devices`. `through` is always one of them.
    pub async fn begin_phone_pair(
        &self,
        through: DeviceId,
        devices: &[DeviceId],
    ) -> Result<PhonePairing, CommandError> {
        self.core
            .view
            .read(|merge| check_can(merge, through, Needs::Phones))?;
        let facts = self.core.facts()?;
        let addresses = self.core.addresses();
        let mut ticked = vec![through];
        ticked.extend(devices.iter().copied().filter(|&device| device != through));
        let mut book = Vec::with_capacity(ticked.len());
        for device in ticked {
            self.core
                .view
                .read(|merge| check_can(merge, device, Needs::Phones))?;
            match self.core.entry(facts, device, &addresses).await {
                (_, Ok(entry)) => book.push(entry),
                (name, Err(note)) => return Err(failed(format!("{name}: {note}"))),
            }
        }
        let routes = book[0].routes.clone();
        let network = book[0].network.clone();
        let reach = book.iter().map(|entry| entry.device).collect();
        let action = ClientMessage::BeginPhonePair {
            key: IdempotencyKey::generate(),
            book,
            mac: self.core.view.read(this_mac),
        };
        let (pair, seed, host_key, expires_at) = match self.act_on(through, action).await? {
            ActionOutcome::PhonePair {
                pair,
                seed,
                host_key,
                expires_at,
            } => (pair, seed, host_key, expires_at),
            outcome => return Err(refusal(outcome)),
        };
        let seed = Zeroizing::new(STANDARD.decode(seed.expose()).unwrap_or_default());
        let seed: [u8; 32] = seed
            .as_slice()
            .try_into()
            .map_err(|_| failed("The device sent a pairing key MaxHQ can't read".to_owned()))?;
        let code = PairCode {
            pair,
            seed: Zeroizing::new(seed),
            device: through,
            host_key,
            routes,
            network,
        };
        let text = code.encode().map_err(failed)?;
        let qr = qr_path(&text).map_err(failed)?;
        // Kept before the code shows, so the phone can't arrive first.
        self.core.expect_phone(Pairing {
            keeper: through,
            devices: reach,
            until: expires_at.after(PAIRING_GRACE),
        })?;
        Ok(PhonePairing {
            pair,
            device: through,
            qr,
            expires_at,
        })
    }

    /// Takes a phone off every device that has it or may reach it: now
    /// where it's connected, and on the rest once they are.
    pub async fn remove_phone(&self, phone: PhoneId) -> Result<(), CommandError> {
        {
            let _turn = self.core.peer_turn.lock().await;
            let holders = self.core.view.read(|merge| holders(merge, phone));
            let mut removals = self.core.removals();
            let removal = removals.entry(phone).or_insert_with(|| Removal {
                at: Timestamp::now(),
                devices: BTreeSet::new(),
            });
            removal.at = Timestamp::now();
            removal.devices.extend(holders);
            self.core.keep_removals(&removals);
            let mut approved: Approved = self.core.read_setting(APPROVED);
            if approved.phones.remove(&phone).is_some()
                && let Err(error) = lock(&self.core.registry).set_setting(APPROVED, &approved)
            {
                tracing::error!(%error, "couldn't keep the approved phones");
            }
            if let Err(error) = self.core.keep_wish(phone, None) {
                tracing::error!(%error, "couldn't forget the phone's switch");
            }
        }
        self.core.phone_pass().await;
        Ok(())
    }

    /// Lets a phone do everything but let other keys in, or stops it, on
    /// each device that has it: now where it's connected, and on the rest
    /// once they are. Gives the names of the devices that have it or may
    /// reach it but don't have the switch yet.
    pub async fn set_phone_everything(
        &self,
        phone: PhoneId,
        everything: bool,
    ) -> Result<Vec<String>, CommandError> {
        self.core.facts()?;
        self.core.keep_wish(phone, Some(everything))?;
        let switched = self.core.phone_pass().await;
        Ok(self.core.view.read(|merge| {
            holders(merge, phone)
                .into_iter()
                .filter(|&device| {
                    // A device that doesn't have the phone has it off.
                    let shown = everything_on(merge, phone, device).unwrap_or(false);
                    shown != everything && !switched.contains(&(phone, device))
                })
                .map(|device| crate::fleet::device_name(merge, device))
                .collect()
        }))
    }

    /// Has each device that has `phone` with its network access on, but no
    /// sign-in from it, make a new one-time code, and puts the codes in the
    /// phone's book. Gives the names of the devices that didn't make one.
    pub async fn renew_phone_codes(&self, phone: PhoneId) -> Result<Vec<String>, CommandError> {
        self.core.facts()?;
        let missed = self.core.renew_codes(phone).await?;
        self.core.phone_pass().await;
        Ok(missed)
    }

    /// Sets the address a phone dials `device` on as well as its own
    /// routes, such as a port your router forwards, or clears it. It goes
    /// to the phones that pair from now on.
    pub fn set_phone_address(
        &self,
        device: DeviceId,
        address: Option<&str>,
    ) -> Result<(), CommandError> {
        let address = address.map(str::trim).filter(|text| !text.is_empty());
        let mut addresses = self.core.addresses();
        match address {
            Some(text) => {
                if parse_address(text, "dev").is_none() {
                    return Err(failed(
                        "Write the address as host, host:port or user@host:port".to_owned(),
                    ));
                }
                addresses.insert(device, text.to_owned());
            }
            None => {
                addresses.remove(&device);
            }
        }
        lock(&self.core.registry)
            .set_setting(ADDRESSES, &addresses)
            .map_err(|error| failed(format!("Couldn't keep the address: {error}")))
    }
}

impl<L: LinkControl> Core<L> {
    /// Brings the phones in line, apart, unless a pass is on its way
    /// already. Only the Mac does this.
    pub(super) fn sync_phones(self: &Arc<Self>) {
        if self.peers.get().is_none() || self.phones_due.swap(true, Ordering::AcqRel) {
            return;
        }
        let core = Arc::clone(self);
        self.runtime.spawn(async move { core.phone_pass().await });
    }

    /// Sends each connected device the removals waiting for it, then
    /// lets each phone in on the devices its book has that don't have it,
    /// and brings each phone's switch on each device in line with the one
    /// you want. Gives the phones and devices that took a switch.
    pub(super) async fn phone_pass(&self) -> Switched {
        let mut switched = Switched::new();
        if self.peers.get().is_none() {
            return switched;
        }
        let _turn = self.peer_turn.lock().await;
        // A change from here on brings another pass.
        self.phones_due.store(false, Ordering::Release);
        let mut removals = self.removals();
        let waiting: Vec<(PhoneId, DeviceId)> = removals
            .iter()
            .flat_map(|(&phone, removal)| {
                removal.devices.iter().map(move |&device| (phone, device))
            })
            .collect();
        for (phone, device) in waiting {
            let (known, takes) = self.view.read(|merge| {
                (
                    device_links(merge, device).next().is_some(),
                    takes_phones(merge, device),
                )
            });
            if known && !takes {
                continue;
            }
            let action = ClientMessage::RemovePhone {
                key: IdempotencyKey::generate(),
                phone,
            };
            // A device no link reaches any more has nothing to take.
            if known && !self.took(device, action).await {
                continue;
            }
            if let Some(removal) = removals.get_mut(&phone) {
                removal.devices.remove(&device);
            }
            self.keep_removals(&removals);
        }
        let before = removals.len();
        removals.retain(|&phone, removal| {
            !removal.devices.is_empty()
                || self
                    .view
                    .read(|merge| shown_since(merge, phone, removal.at))
        });
        if removals.len() != before {
            self.keep_removals(&removals);
        }
        let approved = self.approve(&removals);
        let wishes: Wishes = self.read_setting(EVERYTHING);
        let spreads = self.view.read(|merge| spreads(merge, &removals, &approved));
        for (device, phone) in spreads {
            let id = phone.id;
            let action = ClientMessage::AddPhone {
                key: IdempotencyKey::generate(),
                phone: id,
                name: phone.name,
                ssh_public: phone.ssh_public,
                push_public: phone.push_public,
            };
            let Some(outcome) = self.answer(device, action).await else {
                continue;
            };
            if let ActionOutcome::PhoneAdded { code } = outcome {
                lock(&self.phone_codes).insert((id, device), code);
            }
            // A device lets a phone in with its switch off, so it gets the
            // switch you want now, before it shows the phone.
            if wishes.get(&id) == Some(&true) && self.switch(device, id, true).await {
                switched.insert((id, device));
            }
        }
        let books = {
            let mut codes = lock(&self.phone_codes);
            let named = lock(&self.phone_named);
            self.view
                .read(|merge| books_due(merge, &removals, &approved, &mut codes, &named))
        };
        for due in books {
            let action = ClientMessage::SetPhoneBook {
                key: IdempotencyKey::generate(),
                phone: due.phone,
                book: due.devices,
                mac: due.mac,
            };
            // A keeper too old to keep the name gets it once a run.
            if due.naming {
                lock(&self.phone_named).insert((due.phone, due.keeper));
            }
            self.took(due.keeper, action).await;
        }
        let due = self
            .view
            .read(|merge| switches_due(merge, &removals, &wishes));
        let sends = due
            .into_iter()
            .filter(|&(device, phone, _)| !switched.contains(&(phone, device)))
            .map(|(device, phone, everything)| async move {
                let took = self.switch(device, phone, everything).await;
                took.then_some((phone, device))
            });
        switched.extend(futures::future::join_all(sends).await.into_iter().flatten());
        switched
    }

    /// Sends `device` the switch for `phone`, and says whether it took it.
    async fn switch(&self, device: DeviceId, phone: PhoneId, everything: bool) -> bool {
        let action = ClientMessage::SetPhoneEverything {
            key: IdempotencyKey::generate(),
            phone,
            everything,
        };
        self.took(device, action).await
    }

    /// Sends `AddPhone` again to each device [`renewals`] gives for
    /// `phone`, and keeps the codes they answer for the pass that sends the
    /// book. Gives the names of the devices that made none.
    async fn renew_codes(&self, phone: PhoneId) -> Result<Vec<String>, CommandError> {
        let _turn = self.peer_turn.lock().await;
        let removals = self.removals();
        let approved = self.approve(&removals);
        let Renewals { due, mut missed } = self
            .view
            .read(|merge| renewals(merge, &removals, &approved, phone))
            .ok_or_else(|| failed("This Mac didn't pair that phone".to_owned()))?;
        for (device, phone) in due {
            let id = phone.id;
            let action = ClientMessage::AddPhone {
                key: IdempotencyKey::generate(),
                phone: id,
                name: phone.name,
                ssh_public: phone.ssh_public,
                push_public: phone.push_public,
            };
            match self.answer(device, action).await {
                Some(ActionOutcome::PhoneAdded { code }) => {
                    lock(&self.phone_codes).insert((id, device), code);
                }
                _ => {
                    missed.insert(device);
                }
            }
        }
        let mut names: Vec<String> = self.view.read(|merge| {
            missed
                .into_iter()
                .map(|device| crate::fleet::device_name(merge, device))
                .collect()
        });
        names.sort();
        Ok(names)
    }

    /// What links the devices, which only the Mac has.
    fn facts(&self) -> Result<&dyn PeerFacts, CommandError> {
        self.peers
            .get()
            .map(AsRef::as_ref)
            .ok_or_else(|| failed("Pair phones from MaxHQ on the Mac".to_owned()))
    }

    /// `device`'s name, and its entry in a phone's book or why it has none:
    /// its routes as `ssh -G` resolves them, without one through a jump
    /// host, then the address you gave, with the host key setup pinned,
    /// and its network access while that's on. Network access alone will
    /// do, for a device with no SSH such as the Mac.
    async fn entry(
        &self,
        facts: &dyn PeerFacts,
        device: DeviceId,
        addresses: &BTreeMap<DeviceId, String>,
    ) -> (String, Result<PhoneBookEntry, String>) {
        let specs = self.specs();
        let links: BTreeSet<LinkId> = self.view.read(|merge| {
            device_links(merge, device)
                .map(|link| link.id)
                .filter(|link| specs.contains_key(link))
                .collect()
        });
        let known = Known::reached_by(links);
        let found = find(facts, device, &known, &specs, Vec::new()).await;
        let mut routes: Vec<PhoneRoute> = found
            .routes
            .into_iter()
            .map(|route| PhoneRoute {
                user: route.user,
                host: route.host,
                port: route.port,
            })
            .collect();
        let user = routes.first().map_or("", |route| route.user.as_str());
        if let Some(route) = addresses
            .get(&device)
            .and_then(|text| parse_address(text, user))
            && !routes.contains(&route)
        {
            routes.push(route);
        }
        let (network, off) = self
            .view
            .read(|merge| (network_of(merge, device), network_off(merge, device)));
        let entry = |routes, host_key, network| PhoneBookEntry {
            device,
            name: found.name.clone(),
            routes,
            host_key,
            network,
        };
        let entry = match (found.host_key.clone(), network) {
            (Some(host_key), network) if !routes.is_empty() => Ok(entry(routes, host_key, network)),
            (_, Some(network)) => Ok(entry(Vec::new(), String::new(), Some(network))),
            (_, None) if routes.is_empty() && off => {
                Err("Turn on Network access on its page to pair a phone with it.".to_owned())
            }
            (_, None) if routes.is_empty() => Err(
                "The phone has no way to reach it. Give it an address for the phone.".to_owned(),
            ),
            (_, None) => Err(
                "MaxHQ has no host key for it. Set it up again to pair a phone with it.".to_owned(),
            ),
        };
        (found.name, entry)
    }

    /// The addresses for the phone, by device.
    fn addresses(&self) -> BTreeMap<DeviceId, String> {
        self.read_setting(ADDRESSES)
    }

    /// The removals still to reach a device.
    fn removals(&self) -> Removals {
        self.read_setting(REMOVALS)
    }

    fn keep_removals(&self, removals: &Removals) {
        if let Err(error) = lock(&self.registry).set_setting(REMOVALS, removals) {
            tracing::error!(%error, "couldn't keep the phones to remove");
        }
    }

    /// Keeps the switch you want for `phone`, or forgets it. The registry
    /// stays locked throughout so another phone's switch isn't lost.
    fn keep_wish(&self, phone: PhoneId, everything: Option<bool>) -> Result<(), CommandError> {
        let keep = |error| failed(format!("Couldn't keep the switch: {error}"));
        let registry = lock(&self.registry);
        let mut wishes: Wishes = registry
            .setting(EVERYTHING)
            .map_err(keep)?
            .unwrap_or_default();
        let was = match everything {
            Some(on) => wishes.insert(phone, on),
            None => wishes.remove(&phone),
        };
        if was == everything {
            return Ok(());
        }
        registry.set_setting(EVERYTHING, &wishes).map_err(keep)
    }

    /// Keeps a pairing you started, for the pass that sees its phone. The
    /// registry stays locked throughout so a pass can't lose it.
    fn expect_phone(&self, pairing: Pairing) -> Result<(), CommandError> {
        let keep = |error| failed(format!("Couldn't keep the pairing: {error}"));
        let registry = lock(&self.registry);
        let mut pairings: Vec<Pairing> = registry
            .setting(PAIRINGS)
            .map_err(keep)?
            .unwrap_or_default();
        pairings.push(pairing);
        registry.set_setting(PAIRINGS, &pairings).map_err(keep)
    }

    /// The phones the Mac acts on, with what the view shows now taken in.
    /// Acts on none when it can't read them.
    fn approve(&self, removals: &Removals) -> Approved {
        let links = self.specs();
        let synced = lock(&self.phone_synced).clone();
        let (reports, settled) = self.view.read(|merge| {
            (
                keeper_reports(merge, removals),
                links_settled(merge, &links, &synced),
            )
        });
        let registry = lock(&self.registry);
        let kept = registry.setting::<Approved>(APPROVED).and_then(|approved| {
            let pairings = registry.setting::<Vec<Pairing>>(PAIRINGS)?;
            Ok((approved.unwrap_or_default(), pairings.unwrap_or_default()))
        });
        let (mut approved, mut pairings) = match kept {
            Ok(kept) => kept,
            Err(error) => {
                tracing::error!(%error, "couldn't read the approved phones");
                return Approved::default();
            }
        };
        let (was, waiting) = (approved.clone(), pairings.len());
        let now = Timestamp::now();
        pairings.retain(|pairing| pairing.until > now);
        take_in(&mut approved, &mut pairings, reports, settled);
        if approved != was
            && let Err(error) = registry.set_setting(APPROVED, &approved)
        {
            tracing::error!(%error, "couldn't keep the approved phones");
        }
        if pairings.len() != waiting
            && let Err(error) = registry.set_setting(PAIRINGS, &pairings)
        {
            tracing::error!(%error, "couldn't keep the pairings");
        }
        approved
    }

    fn read_setting<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> T {
        lock(&self.registry)
            .setting(name)
            .unwrap_or_else(|error| {
                tracing::error!(%error, name, "couldn't read a phone setting");
                None
            })
            .unwrap_or_default()
    }
}

/// What a device answered instead of what was asked, as an error.
fn refusal(outcome: ActionOutcome) -> CommandError {
    match outcome {
        ActionOutcome::Refused { reason } => failed(reason),
        _ => failed("The device didn't start the pairing".to_owned()),
    }
}

/// Reads `host`, `host:port`, `user@host` or `user@host:port`, with an
/// IPv6 address in brackets, taking `user` when the address names none.
fn parse_address(text: &str, user: &str) -> Option<PhoneRoute> {
    let (user, rest) = match text.split_once('@') {
        Some((user, rest)) => (user, rest),
        None => (user, text),
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        let port = match after {
            "" => SSH_PORT,
            after => after.strip_prefix(':')?.parse().ok()?,
        };
        (host, port)
    } else {
        match rest.split_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (rest, SSH_PORT),
        }
    };
    let plain = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    };
    (plain(user) && plain(host) && port > 0).then(|| PhoneRoute {
        user: user.to_owned(),
        host: host.to_owned(),
        port,
    })
}

/// The devices whose `maxd` takes phones, once each.
fn phone_hosts(merge: &Merge) -> Vec<DeviceId> {
    let devices: BTreeSet<DeviceId> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::DeviceLink(link) if link.capabilities.is_some_and(|does| does.phones) => {
                link.device
            }
            _ => None,
        })
        .collect();
    devices.into_iter().collect()
}

/// Whether the view shows `device` connected through a link that says it
/// takes phones.
fn takes_phones(merge: &Merge, device: DeviceId) -> bool {
    super::device_connected(merge, device)
        && super::capabilities(merge, device).is_some_and(|does| does.phones)
}

/// Whether the view still shows `phone` as it was before `at`: a device
/// that hasn't dropped it yet, and no pairing since.
fn shown_since(merge: &Merge, phone: PhoneId, at: Timestamp) -> bool {
    let mut shown = false;
    for entity in merge.store().entities() {
        match entity {
            Entity::Phone(found) if found.id == phone => {
                if found.paired_at > at {
                    return false;
                }
                shown = true;
            }
            Entity::PhoneBook(book) if book.phone == phone => shown = true,
            _ => {}
        }
    }
    shown
}

/// Whether `device` lets `phone` do everything, or none when it doesn't
/// have the phone.
fn everything_on(merge: &Merge, phone: PhoneId, device: DeviceId) -> Option<bool> {
    match merge.store().get(&EntityKey::Phone { phone, device }) {
        Some(Entity::Phone(found)) => Some(found.everything),
        _ => None,
    }
}

/// Every device that has `phone` or may reach it.
fn holders(merge: &Merge, phone: PhoneId) -> BTreeSet<DeviceId> {
    let mut devices = BTreeSet::new();
    for entity in merge.store().entities() {
        match entity {
            Entity::Phone(found) if found.id == phone => {
                devices.insert(found.device);
            }
            Entity::PhoneBook(book) if book.phone == phone => {
                devices.insert(book.device);
                devices.extend(book.devices.iter().map(|entry| entry.device));
            }
            _ => {}
        }
    }
    devices
}

/// How a phone reaches `device` over the network, from the network access
/// it publishes: while it's on, with its pin and at least one host. A
/// device that publishes none is from before network access, so it's off.
fn network_of(merge: &Merge, device: DeviceId) -> Option<PhoneNetwork> {
    match merge.store().get(&EntityKey::NetworkAccess(device)) {
        Some(Entity::NetworkAccess(access)) if access.on && !access.hosts.is_empty() => {
            Some(PhoneNetwork {
                hosts: access.hosts.clone(),
                port: access.port,
                pin: access.pin.clone()?,
                code: None,
            })
        }
        _ => None,
    }
}

/// Whether `device` has network access, with its switch off.
fn network_off(merge: &Merge, device: DeviceId) -> bool {
    matches!(
        merge.store().get(&EntityKey::NetworkAccess(device)),
        Some(Entity::NetworkAccess(access)) if !access.on
    )
}

/// Each phone the view shows with a book, as the device that keeps the
/// book reports it, unless the phone is on its way out.
fn keeper_reports(merge: &Merge, removals: &Removals) -> Vec<(PhoneId, Keeping)> {
    merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::PhoneBook(book)
                if !removals.contains_key(&book.phone)
                    && merge
                        .store()
                        .get(&EntityKey::Phone {
                            phone: book.phone,
                            device: book.device,
                        })
                        .is_some() =>
            {
                Some((
                    book.phone,
                    Keeping {
                        keeper: book.device,
                        devices: book.devices.iter().map(|entry| entry.device).collect(),
                    },
                ))
            }
            _ => None,
        })
        .collect()
}

/// Whether every link has said what it can for now: its snapshot once
/// it's connected, or that its device can't be reached.
fn links_settled(
    merge: &Merge,
    links: &BTreeMap<LinkId, LinkSpec>,
    synced: &BTreeSet<LinkId>,
) -> bool {
    links
        .keys()
        .all(|id| match merge.store().get(&EntityKey::DeviceLink(*id)) {
            Some(Entity::DeviceLink(link)) => match link.state {
                LinkState::Connected { .. } => synced.contains(id),
                LinkState::Unreachable { .. } => true,
                _ => false,
            },
            _ => false,
        })
}

/// Approves what the keepers report. A phone answers a pairing when its
/// keeper is the device you paired through and its book names just the
/// devices you ticked, which uses that pairing up. That also covers an
/// approved phone paired again, through another device or to reach more.
/// The first time the links settle, every phone they show is approved
/// too, since those were paired before the Mac kept this.
fn take_in(
    approved: &mut Approved,
    pairings: &mut Vec<Pairing>,
    reports: Vec<(PhoneId, Keeping)>,
    settled: bool,
) {
    for (phone, report) in &reports {
        let known = approved.phones.get(phone).is_some_and(|known| {
            known.keeper == report.keeper && report.devices.is_subset(&known.devices)
        });
        if known {
            continue;
        }
        let answered = pairings.iter().position(|pairing| {
            pairing.keeper == report.keeper && pairing.devices == report.devices
        });
        if let Some(at) = answered {
            pairings.remove(at);
            approved.phones.insert(*phone, report.clone());
        }
    }
    if settled && !approved.settled {
        approved.settled = true;
        for (phone, report) in reports {
            approved.phones.entry(phone).or_insert(report);
        }
    }
}

/// This Mac as a phone's book names it, once its own link knows its device.
fn this_mac(merge: &Merge) -> Option<PhoneMac> {
    let device = match merge.store().get(&EntityKey::DeviceLink(local_link())) {
        Some(Entity::DeviceLink(link)) => link.device?,
        _ => return None,
    };
    Some(PhoneMac {
        device,
        name: crate::fleet::device_name(merge, device),
    })
}

/// A book for [`books_due`] to send.
struct BookDue {
    keeper: DeviceId,
    phone: PhoneId,
    devices: Vec<PhoneBookEntry>,
    /// The Mac the book names.
    mac: Option<PhoneMac>,
    /// Whether it goes to give the book this Mac's name.
    naming: bool,
}

/// The books to send, each to the connected device that keeps it, for an
/// approved phone that isn't on its way out: each entry's network part as its
/// device publishes it now, with the code `codes` or the book has for it
/// until that device lists a session for the phone. An entry with no other
/// way in keeps the network part it had, and so does a device the phone
/// wasn't approved for. Takes the codes that are done with out of `codes`.
/// A book that names no Mac, or this Mac by an old name, gets this Mac's
/// name, unless it went with it already this run as `named` says.
fn books_due(
    merge: &Merge,
    removals: &Removals,
    approved: &Approved,
    codes: &mut Codes,
    named: &BTreeSet<(PhoneId, DeviceId)>,
) -> Vec<BookDue> {
    let me = this_mac(merge);
    let sessions: BTreeSet<(PhoneId, DeviceId)> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::PhoneSession(session) => Some((session.phone, session.device)),
            _ => None,
        })
        .collect();
    codes.retain(|key, _| !sessions.contains(key) && !removals.contains_key(&key.0));
    let mut due = Vec::new();
    for entity in merge.store().entities() {
        let Entity::PhoneBook(book) = entity else {
            continue;
        };
        if removals.contains_key(&book.phone) || !takes_phones(merge, book.device) {
            continue;
        }
        let Some(reach) = approved.reach(book) else {
            continue;
        };
        let devices: Vec<PhoneBookEntry> = book
            .devices
            .iter()
            .map(|entry| {
                if !reach.contains(&entry.device) {
                    return entry.clone();
                }
                let key = (book.phone, entry.device);
                let kept = entry.network.as_ref().and_then(|net| net.code.clone());
                let mut entry = entry.clone();
                if super::device_connected(merge, entry.device) {
                    let now = network_of(merge, entry.device);
                    if now.is_some() || !entry.routes.is_empty() {
                        entry.network = now;
                    }
                }
                if let Some(network) = &mut entry.network {
                    network.code = codes
                        .get(&key)
                        .cloned()
                        .or(kept)
                        .filter(|_| !sessions.contains(&key));
                }
                entry
            })
            .collect();
        let naming = me.as_ref().is_some_and(|me| {
            let stale = book
                .mac
                .as_ref()
                .is_none_or(|mac| mac.device == me.device && mac.name != me.name);
            stale && !named.contains(&(book.phone, book.device))
        });
        if devices != book.devices || naming {
            let mac = if naming { me.clone() } else { book.mac.clone() };
            due.push(BookDue {
                keeper: book.device,
                phone: book.phone,
                devices,
                mac,
                naming,
            });
        }
    }
    due
}

/// The switches to send: to each connected device that has a phone you
/// switched, when it shows the other way, unless the phone is on its way
/// out.
fn switches_due(
    merge: &Merge,
    removals: &Removals,
    wishes: &Wishes,
) -> Vec<(DeviceId, PhoneId, bool)> {
    merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::Phone(phone)
                if !removals.contains_key(&phone.id) && takes_phones(merge, phone.device) =>
            {
                let &everything = wishes.get(&phone.id)?;
                (phone.everything != everything).then_some((phone.device, phone.id, everything))
            }
            _ => None,
        })
        .collect()
}

/// The phones to let in, each with the connected device that should have
/// it: every device an approved phone may reach and its book names that
/// doesn't have it, unless the phone is on its way out.
fn spreads(merge: &Merge, removals: &Removals, approved: &Approved) -> Vec<(DeviceId, Phone)> {
    let mut phones: BTreeMap<(PhoneId, DeviceId), &Phone> = BTreeMap::new();
    for entity in merge.store().entities() {
        if let Entity::Phone(phone) = entity {
            phones.insert((phone.id, phone.device), phone);
        }
    }
    let mut spreads = Vec::new();
    for entity in merge.store().entities() {
        let Entity::PhoneBook(book) = entity else {
            continue;
        };
        let Some(&paired) = phones.get(&(book.phone, book.device)) else {
            continue;
        };
        if removals.contains_key(&book.phone) {
            continue;
        }
        let Some(reach) = approved.reach(book) else {
            continue;
        };
        for entry in &book.devices {
            if reach.contains(&entry.device)
                && !phones.contains_key(&(book.phone, entry.device))
                && takes_phones(merge, entry.device)
            {
                spreads.push((entry.device, paired.clone()));
            }
        }
    }
    spreads
}

/// The devices to make a new code for a phone, and those that can't now.
#[derive(Debug, Default, PartialEq, Eq)]
struct Renewals {
    /// Each connected device to ask, with the phone as it shows it.
    due: Vec<(DeviceId, Phone)>,
    /// The devices that need one but aren't connected.
    missed: BTreeSet<DeviceId>,
}

/// What a new code for `phone` takes: each device its book may reach that
/// has it with network access on but no session for it. The book only
/// takes a code while the device that keeps it is connected, so while it
/// isn't, every one of them is missed, and so is it. None for a phone the
/// Mac doesn't act on, or one on its way out.
fn renewals(
    merge: &Merge,
    removals: &Removals,
    approved: &Approved,
    phone: PhoneId,
) -> Option<Renewals> {
    if removals.contains_key(&phone) {
        return None;
    }
    let Some(Entity::PhoneBook(book)) = merge.store().get(&EntityKey::PhoneBook(phone)) else {
        return None;
    };
    let reach = approved.reach(book)?;
    let signed_in: BTreeSet<DeviceId> = merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::PhoneSession(session) if session.phone == phone => Some(session.device),
            _ => None,
        })
        .collect();
    let keeper_in = takes_phones(merge, book.device);
    let mut renewals = Renewals::default();
    for entry in &book.devices {
        let device = entry.device;
        let Some(Entity::Phone(shown)) = merge.store().get(&EntityKey::Phone { phone, device })
        else {
            continue;
        };
        if !reach.contains(&device) || signed_in.contains(&device) {
            continue;
        }
        if network_of(merge, device).is_none() {
            continue;
        }
        if keeper_in && takes_phones(merge, device) {
            renewals.due.push((device, shown.clone()));
        } else {
            renewals.missed.insert(device);
        }
    }
    if !keeper_in && !renewals.missed.is_empty() {
        renewals.missed.insert(book.device);
    }
    Some(renewals)
}

#[cfg(test)]
mod tests;
