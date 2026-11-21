// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Device links: each device whose `maxd` links to others gets the rest in
//! its peer book, and each one that takes SSH logins lets their keys in.
//!
//! The app asks a device for its key once and keeps it, with what each
//! device took last, so a reconnect with nothing changed sends nothing. A
//! device you remove comes off the others, each when it's next connected.
//! A pair you turn off stays off until you allow it again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use futures::future::BoxFuture;
#[cfg(feature = "desktop")]
use maxhq_client::setup;
use maxhq_client::LinkSpec;
use maxhq_protocol::agents::{PeerBook, PeerEntry, PeerRoute};
use maxhq_protocol::ids::{DeviceId, IdempotencyKey, LinkId};
use maxhq_protocol::leases::{TestHostState, TestKind};
use maxhq_protocol::link::{LinkState, Route};
use maxhq_protocol::local_models::{LinkedModels, ModelServerKind, ServerModel};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::ClientMessage;
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{
    device_connected, device_targets, failed, lock, Connector, Core, Devices, LinkControl,
};
use crate::fleet::{check_can, Needs};
use crate::request::CommandError;

/// The setting device links are kept under.
const DEVICE_LINKS: &str = "device_links";

/// What this machine knows about reaching the devices it links.
pub trait PeerFacts: Send + Sync + 'static {
    /// Where another device reaches `host`, one of a device's SSH routes,
    /// with no host in between, or none.
    fn route(&self, host: String) -> BoxFuture<'static, Option<PeerRoute>>;
    /// The ed25519 host key setup pinned for `device`, if any.
    fn host_key(&self, device: DeviceId) -> Option<String>;
    /// Pins `device`'s host key, when MaxHQ has none, from the one your
    /// own `known_hosts` has for `host`, and says whether it did. Facts
    /// with no `known_hosts` to read never pin.
    fn pin(&self, _device: DeviceId, _host: String) -> BoxFuture<'static, bool> {
        Box::pin(async { false })
    }
}

/// The facts `ssh -G` and MaxHQ's own `known_hosts` give.
#[cfg(feature = "desktop")]
pub struct SshFacts;

#[cfg(feature = "desktop")]
impl PeerFacts for SshFacts {
    fn route(&self, host: String) -> BoxFuture<'static, Option<PeerRoute>> {
        Box::pin(async move { setup::direct_route(&host).await })
    }

    fn host_key(&self, device: DeviceId) -> Option<String> {
        setup::pinned_host_key(&setup::paths()?.known_hosts, &device.to_string())
    }

    fn pin(&self, device: DeviceId, host: String) -> BoxFuture<'static, bool> {
        Box::pin(async move {
            let Some(paths) = setup::paths() else {
                return false;
            };
            let pinning = setup::pin_known_host(
                &setup::System,
                &paths.known_hosts,
                &device.to_string(),
                &host,
            )
            .await;
            match pinning {
                Ok(setup::Pinning::Pinned) => true,
                Ok(setup::Pinning::Differs) => {
                    tracing::warn!(
                        %device,
                        %host,
                        "your known_hosts has another key for a device, so MaxHQ keeps the one it pinned"
                    );
                    false
                }
                Ok(_) => false,
                Err(error) => {
                    tracing::warn!(%device, %error, "couldn't pin a device's host key");
                    false
                }
            }
        })
    }
}

/// One other device, as a device's settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PeerLink {
    pub peer: DeviceId,
    pub name: String,
    pub state: PeerLinkState,
    /// Why it can't link yet, when the app knows.
    pub note: Option<String>,
}

/// Where a link between two devices stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PeerLinkState {
    /// Each has the other.
    Linked,
    /// One of them hasn't taken it yet, most often because it's away.
    Waiting,
    /// You turned it off.
    Off,
}

/// What the app keeps about device links.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    /// Each device whose `maxd` links to others, by its id.
    devices: BTreeMap<DeviceId, Known>,
    /// The pairs you turned off, the lower id first.
    off: BTreeSet<(DeviceId, DeviceId)>,
    /// The revokes waiting for a device: the device, then the one it lets
    /// go of.
    revokes: BTreeSet<(DeviceId, DeviceId)>,
}

/// One device that links to others.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(super) struct Known {
    /// The links in the registry that reach it.
    links: BTreeSet<LinkId>,
    /// Its own SSH key, once it gave it.
    key: Option<String>,
    /// The book it took last.
    book: Option<PeerBook>,
    /// The keys it let in, by device.
    let_in: BTreeMap<DeviceId, String>,
}

#[cfg(feature = "desktop")]
impl Known {
    /// A device the registry reaches through `links`, that the app knows
    /// nothing else about yet.
    pub(super) fn reached_by(links: BTreeSet<LinkId>) -> Self {
        Self {
            links,
            ..Self::default()
        }
    }
}

/// What the registry and this machine say about one device.
pub(super) struct Found {
    pub(super) name: String,
    /// Whether it takes SSH logins: it has an SSH route.
    ssh: bool,
    /// The routes another device dials it on.
    pub(super) routes: Vec<PeerRoute>,
    /// The host key setup pinned for it.
    pub(super) host_key: Option<String>,
    /// The kinds of test machine it hosts.
    test_kinds: Vec<TestKind>,
    /// Its managed model server, for the others' local agents.
    pub(super) model_server: Option<LinkedModels>,
    /// The devices whose managed models its local agents use.
    pub(super) models_from: Vec<DeviceId>,
    /// Whether it's this machine, which runs the app and so may move
    /// agents on the others.
    pub(super) home: bool,
}

/// What one device should have.
struct Want {
    let_in: BTreeMap<DeviceId, String>,
    book: PeerBook,
}

impl<C: Connector> Devices<C> {
    /// Starts linking devices to each other with what `facts` give, and
    /// links them now.
    pub fn link_peers(&self, facts: impl PeerFacts) {
        if self.core.peers.set(Box::new(facts)).is_ok() {
            self.core.sync_peers();
        }
    }

    /// Whether a link other than `link` says it reaches `device` now, or is
    /// on record as having done so.
    pub fn another_link_reaches(&self, device: DeviceId, link: LinkId) -> bool {
        let seen = self.core.view.read(|merge| {
            merge.store().entities().any(|entity| {
                matches!(entity, Entity::DeviceLink(other)
                    if other.id != link && other.device == Some(device))
            })
        });
        seen || !self.core.kept_links(device, link).is_empty()
    }

    /// Sends the devices their books again, after a setup kept a device's
    /// host keys.
    pub fn host_keys_changed(&self) {
        self.core.sync_peers();
    }

    /// The other devices `device` links to, each with where it stands.
    pub fn device_links(&self, device: DeviceId) -> Result<Vec<PeerLink>, CommandError> {
        self.core
            .view
            .read(|merge| check_can(merge, device, Needs::Peers))?;
        let kept = self.core.kept();
        let specs = self.core.specs();
        let Some(me) = kept.devices.get(&device) else {
            return Ok(Vec::new());
        };
        let facts = self.core.peers.get();
        Ok(kept
            .devices
            .iter()
            .filter(|(peer, _)| **peer != device)
            .map(|(&peer, other)| {
                let state = if kept.off.contains(&pair(device, peer)) {
                    PeerLinkState::Off
                } else if has(me, other, peer, &specs) && has(other, me, device, &specs) {
                    PeerLinkState::Linked
                } else {
                    PeerLinkState::Waiting
                };
                let unpinned = takes_logins(other, &specs)
                    && facts.is_some_and(|facts| facts.host_key(peer).is_none());
                PeerLink {
                    peer,
                    name: name_of(other, &specs),
                    state,
                    note: (unpinned && state != PeerLinkState::Off).then(|| {
                        "MaxHQ has no host key for it. Set it up again to link it.".to_owned()
                    }),
                }
            })
            .collect())
    }

    /// Turns the link between `device` and `peer` off, which takes each
    /// out of the other, or allows it again. Either way the devices hear
    /// now, or when they're next connected.
    pub async fn set_device_link(
        &self,
        device: DeviceId,
        peer: DeviceId,
        on: bool,
    ) -> Result<(), CommandError> {
        self.core
            .view
            .read(|merge| check_can(merge, device, Needs::Peers))?;
        if device == peer {
            return Err(failed("A device can't link to itself".to_owned()));
        }
        {
            let _turn = self.core.peer_turn.lock().await;
            let mut kept = self.core.kept();
            if on {
                kept.off.remove(&pair(device, peer));
            } else {
                kept.off.insert(pair(device, peer));
                kept.revokes.insert((device, peer));
                kept.revokes.insert((peer, device));
            }
            self.core.keep(&kept);
        }
        self.core.peer_pass().await;
        Ok(())
    }
}

impl<L: LinkControl> Core<L> {
    /// Pins the host key of the device that came up over SSH on link `id`,
    /// apart, when MaxHQ has none, and links the devices again once it
    /// does.
    pub(super) fn pin_host_key(self: &Arc<Self>, id: LinkId) {
        let Some(facts) = self.peers.get() else {
            return;
        };
        let Some((device, host)) = self.view.read(|merge| ssh_device(merge, id)) else {
            return;
        };
        let pinning = facts.pin(device, host);
        let core = Arc::clone(self);
        self.runtime.spawn(async move {
            if pinning.await {
                core.sync_peers();
            }
        });
    }

    /// Links the devices, apart, unless a pass is on its way already.
    pub(super) fn sync_peers(self: &Arc<Self>) {
        if self.peers.get().is_none() || self.peers_due.swap(true, Ordering::AcqRel) {
            return;
        }
        let core = Arc::clone(self);
        self.runtime.spawn(async move { core.peer_pass().await });
    }

    /// Brings every connected device in line: its key first, then the
    /// revokes waiting for it, the keys it lets in and its book.
    async fn peer_pass(&self) {
        let Some(facts) = self.peers.get() else {
            return;
        };
        let _turn = self.peer_turn.lock().await;
        // A change from here on brings another pass.
        self.peers_due.store(false, Ordering::Release);
        let specs = self.specs();
        let mut kept = self.kept();
        self.learn(&mut kept, &specs);
        self.keep(&kept);
        let connected: Vec<DeviceId> = kept
            .devices
            .keys()
            .copied()
            .filter(|&device| self.view.read(|merge| connected_peer(merge, device)))
            .collect();
        for &device in &connected {
            self.ask_key(device, &mut kept).await;
        }
        let mut hosted = self.view.read(test_kinds);
        let mut shared = self.view.read(shared_models);
        let mut using = self.view.read(models_from);
        let mut found = BTreeMap::new();
        for (&device, known) in &kept.devices {
            let kinds = hosted.remove(&device).unwrap_or_default();
            let mut one = find(facts.as_ref(), device, known, &specs, kinds).await;
            one.home = self.view.read(|merge| super::is_local(merge, device));
            one.model_server = shared.remove(&device);
            one.models_from = using.remove(&device).unwrap_or_default();
            found.insert(device, one);
        }
        for device in connected {
            self.bring(device, &mut kept, &found).await;
        }
    }

    /// Picks up the devices the view shows linking to others, and drops
    /// those no link in the registry reaches any more. A dropped device
    /// comes off every other.
    fn learn(&self, kept: &mut Kept, specs: &BTreeMap<LinkId, LinkSpec>) {
        let (seen, held): (Vec<(LinkId, DeviceId)>, Vec<LinkId>) = self.view.read(|merge| {
            let links = || {
                merge.store().entities().filter_map(|entity| match entity {
                    Entity::DeviceLink(link) => Some(link),
                    _ => None,
                })
            };
            let seen = links()
                .filter(|link| link.capabilities.is_some_and(|does| does.peers))
                .filter_map(|link| Some((link.id, link.device?)))
                .collect();
            let held = links()
                .filter(|link| link.claim.is_some())
                .map(|link| link.id)
                .collect();
            (seen, held)
        });
        // A link waiting for your answer about its device reaches none.
        for known in kept.devices.values_mut() {
            for link in &held {
                known.links.remove(link);
            }
        }
        for (link, device) in seen {
            if !specs.contains_key(&link) {
                continue;
            }
            for (other, known) in kept.devices.iter_mut() {
                if *other != device {
                    known.links.remove(&link);
                }
            }
            kept.devices.entry(device).or_default().links.insert(link);
        }
        let mut gone = Vec::new();
        kept.devices.retain(|&device, known| {
            known.links.retain(|link| specs.contains_key(link));
            let stays = !known.links.is_empty();
            if !stays {
                gone.push(device);
            }
            stays
        });
        for device in gone {
            kept.off.retain(|&(a, b)| a != device && b != device);
            kept.revokes.retain(|&(to, _)| to != device);
            for (&other, known) in kept.devices.iter_mut() {
                known.let_in.remove(&device);
                kept.revokes.insert((other, device));
            }
        }
    }

    /// Asks a device for its key, unless the app has it.
    async fn ask_key(&self, device: DeviceId, kept: &mut Kept) {
        if kept
            .devices
            .get(&device)
            .is_none_or(|known| known.key.is_some())
        {
            return;
        }
        let action = ClientMessage::PeerKey {
            key: IdempotencyKey::generate(),
        };
        match self.act_on_peer(device, action).await {
            Some(ActionOutcome::PeerKey { public_key }) => {
                if let Some(known) = kept.devices.get_mut(&device) {
                    known.key = Some(public_key);
                }
                self.keep(kept);
            }
            Some(outcome) => tracing::warn!(%device, ?outcome, "a device didn't give its key"),
            None => {}
        }
    }

    /// Sends a device what changed since it last took its links. What it
    /// doesn't take waits for the next pass.
    async fn bring(&self, device: DeviceId, kept: &mut Kept, found: &BTreeMap<DeviceId, Found>) {
        let want = want(device, kept, found);
        let Some(known) = kept.devices.get(&device) else {
            return;
        };
        let stale = known
            .let_in
            .keys()
            .filter(|peer| !want.let_in.contains_key(peer))
            .map(|&peer| (device, peer));
        let revokes: BTreeSet<_> = kept
            .revokes
            .iter()
            .copied()
            .filter(|&(to, _)| to == device)
            .chain(stale)
            .collect();
        for (_, peer) in revokes {
            let action = ClientMessage::RevokePeer {
                key: IdempotencyKey::generate(),
                device: peer,
            };
            if !self.took(device, action).await {
                return;
            }
            kept.revokes.remove(&(device, peer));
            if let Some(known) = kept.devices.get_mut(&device) {
                known.let_in.remove(&peer);
            }
            self.keep(kept);
        }
        for (peer, public_key) in want.let_in {
            if kept
                .devices
                .get(&device)
                .is_some_and(|known| known.let_in.get(&peer) == Some(&public_key))
            {
                continue;
            }
            let action = ClientMessage::AuthorizePeer {
                key: IdempotencyKey::generate(),
                device: peer,
                public_key: public_key.clone(),
            };
            if !self.took(device, action).await {
                return;
            }
            if let Some(known) = kept.devices.get_mut(&device) {
                known.let_in.insert(peer, public_key);
            }
            self.keep(kept);
        }
        if kept
            .devices
            .get(&device)
            .is_some_and(|known| known.book.as_ref() == Some(&want.book))
        {
            return;
        }
        let action = ClientMessage::SetPeerBook {
            key: IdempotencyKey::generate(),
            book: want.book.clone(),
        };
        if self.took(device, action).await {
            if let Some(known) = kept.devices.get_mut(&device) {
                known.book = Some(want.book);
            }
            self.keep(kept);
        }
    }

    /// Sends a device one change, and says whether it took it.
    pub(super) async fn took(&self, device: DeviceId, action: ClientMessage) -> bool {
        self.answer(device, action).await.is_some()
    }

    /// Sends a device one change, and gives its answer when it took it:
    /// done, or the code a device with network access made for a phone.
    pub(super) async fn answer(
        &self,
        device: DeviceId,
        action: ClientMessage,
    ) -> Option<ActionOutcome> {
        match self.act_on_peer(device, action).await? {
            outcome @ (ActionOutcome::Done | ActionOutcome::PhoneAdded { .. }) => Some(outcome),
            outcome => {
                tracing::warn!(%device, ?outcome, "a device didn't take a change");
                None
            }
        }
    }

    /// Sends an action to a device, or none when it didn't go out.
    async fn act_on_peer(&self, device: DeviceId, action: ClientMessage) -> Option<ActionOutcome> {
        let targets = self.view.read(|merge| device_targets(merge, device)).ok()?;
        match self.act_through(targets, action).await {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                tracing::debug!(%device, %error, "a change to a device waits");
                None
            }
        }
    }

    /// The device links kept last.
    fn kept(&self) -> Kept {
        lock(&self.registry)
            .setting(DEVICE_LINKS)
            .unwrap_or_else(|error| {
                tracing::error!(%error, "couldn't read the device links");
                None
            })
            .unwrap_or_default()
    }

    /// The links other than `link` the device links last kept under
    /// `device`.
    pub(super) fn kept_links(&self, device: DeviceId, link: LinkId) -> Vec<LinkId> {
        self.kept()
            .devices
            .get(&device)
            .map_or_else(Vec::new, |known| {
                known
                    .links
                    .iter()
                    .copied()
                    .filter(|&other| other != link)
                    .collect()
            })
    }

    /// Keeps the device links.
    fn keep(&self, kept: &Kept) {
        if let Err(error) = lock(&self.registry).set_setting(DEVICE_LINKS, kept) {
            tracing::error!(%error, "couldn't keep the device links");
        }
    }

    /// The registry's links, by id.
    pub(super) fn specs(&self) -> BTreeMap<LinkId, LinkSpec> {
        let specs = lock(&self.registry).list().unwrap_or_else(|error| {
            tracing::error!(%error, "couldn't read the device list");
            Vec::new()
        });
        specs.into_iter().map(|spec| (spec.id, spec)).collect()
    }
}

/// What the registry and this machine say about a device.
pub(super) async fn find(
    facts: &dyn PeerFacts,
    device: DeviceId,
    known: &Known,
    specs: &BTreeMap<LinkId, LinkSpec>,
    test_kinds: Vec<TestKind>,
) -> Found {
    let hosts = ssh_hosts(known, specs);
    let mut routes = Vec::new();
    for host in &hosts {
        if let Some(route) = facts.route(host.clone()).await
            && !routes.contains(&route)
        {
            routes.push(route);
        }
    }
    Found {
        name: name_of(known, specs),
        ssh: !hosts.is_empty(),
        routes,
        host_key: if hosts.is_empty() {
            None
        } else {
            facts.host_key(device)
        },
        test_kinds,
        model_server: None,
        models_from: Vec::new(),
        home: false,
    }
}

/// What `device` should have: the keys of the others it lets in, when it
/// takes SSH logins, and a book with every other that can be in one.
fn want(device: DeviceId, kept: &Kept, found: &BTreeMap<DeviceId, Found>) -> Want {
    let ssh = found.get(&device).is_some_and(|found| found.ssh);
    let others: Vec<_> = kept
        .devices
        .iter()
        .filter(|&(&peer, _)| peer != device && !kept.off.contains(&pair(device, peer)))
        .collect();
    let let_in = others
        .iter()
        .filter(|_| ssh)
        .filter_map(|&(&peer, known)| Some((peer, known.key.clone()?)))
        .collect();
    let peers = others
        .iter()
        .filter_map(|&(&peer, _)| entry(peer, found.get(&peer)?))
        .collect();
    Want {
        let_in,
        book: PeerBook { ssh, peers },
    }
}

/// A device's entry in another's book. One reached over SSH needs the
/// host key setup pinned, so it's left out without one. One with no SSH
/// route is never dialed, so it has no host key.
fn entry(device: DeviceId, found: &Found) -> Option<PeerEntry> {
    let host_key = if found.ssh {
        found.host_key.clone()?
    } else {
        String::new()
    };
    Some(PeerEntry {
        device,
        name: found.name.clone(),
        routes: found.routes.clone(),
        host_key,
        test_kinds: found.test_kinds.clone(),
        home: found.home,
        model_server: found.model_server.clone(),
        models_from: found.models_from.clone(),
    })
}

/// Whether `known`'s last book has `peer`, and it let `peer` in when it
/// takes SSH logins.
fn has(known: &Known, other: &Known, peer: DeviceId, specs: &BTreeMap<LinkId, LinkSpec>) -> bool {
    let in_book = known
        .book
        .as_ref()
        .is_some_and(|book| book.peers.iter().any(|entry| entry.device == peer));
    let let_in = !takes_logins(known, specs)
        || other
            .key
            .as_ref()
            .is_some_and(|key| known.let_in.get(&peer) == Some(key));
    in_book && let_in
}

/// Whether a device has an SSH route.
fn takes_logins(known: &Known, specs: &BTreeMap<LinkId, LinkSpec>) -> bool {
    !ssh_hosts(known, specs).is_empty()
}

/// Every SSH host the registry reaches a device at.
fn ssh_hosts(known: &Known, specs: &BTreeMap<LinkId, LinkSpec>) -> Vec<String> {
    known
        .links
        .iter()
        .filter_map(|link| specs.get(link))
        .flat_map(|spec| std::iter::once(&spec.route).chain(&spec.fallbacks))
        .filter_map(|route| match route {
            Route::Ssh { host } => Some(host.clone()),
            _ => None,
        })
        .collect()
}

/// What you called a device, from its first link.
fn name_of(known: &Known, specs: &BTreeMap<LinkId, LinkSpec>) -> String {
    known
        .links
        .iter()
        .find_map(|link| specs.get(link))
        .map_or_else(|| "The device".to_owned(), |spec| spec.name.clone())
}

/// A pair of devices, the lower id first.
fn pair(a: DeviceId, b: DeviceId) -> (DeviceId, DeviceId) {
    (a.min(b), a.max(b))
}

/// The device link `id` reaches and the SSH host it's connected through,
/// when the view shows it connected over SSH.
fn ssh_device(merge: &crate::merge::Merge, id: LinkId) -> Option<(DeviceId, String)> {
    match merge.store().get(&EntityKey::DeviceLink(id)) {
        Some(Entity::DeviceLink(link)) if matches!(link.state, LinkState::Connected { .. }) => {
            match &link.route {
                Route::Ssh { host } => Some((link.device?, host.clone())),
                Route::Local => None,
            }
        }
        _ => None,
    }
}

/// Whether the view shows `device` connected through a link that says it
/// links to others.
fn connected_peer(merge: &crate::merge::Merge, device: DeviceId) -> bool {
    device_connected(merge, device)
        && super::capabilities(merge, device).is_some_and(|does| does.peers)
}

/// The kinds of test machine each device hosts, from the hosts it
/// publishes. One that lacks what it needs hosts nothing yet.
pub(super) fn test_kinds(merge: &crate::merge::Merge) -> BTreeMap<DeviceId, Vec<TestKind>> {
    let mut kinds: BTreeMap<DeviceId, Vec<TestKind>> = BTreeMap::new();
    for entity in merge.store().entities() {
        if let Entity::TestHost(host) = entity
            && host.state != TestHostState::Unavailable
        {
            let hosted = kinds.entry(host.device).or_default();
            if !hosted.contains(&host.kind) {
                hosted.push(host.kind);
            }
        }
    }
    for hosted in kinds.values_mut() {
        hosted.sort_by_key(|kind| TestKind::ALL.iter().position(|each| each == kind));
    }
    kinds
}

/// Each device's managed model server, as the others' books carry it:
/// its models with no load or speed, so a model loading or a turn's speed
/// sends no book. A device whose switch is off still has one; it says no
/// when asked, and the others' pickers show why.
pub(super) fn shared_models(merge: &crate::merge::Merge) -> BTreeMap<DeviceId, LinkedModels> {
    merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::ModelServer(server) if server.kind == ModelServerKind::Managed => Some((
                server.device,
                LinkedModels {
                    server: server.id,
                    label: server.label.clone(),
                    models: server
                        .models
                        .iter()
                        .map(|model| ServerModel {
                            load: None,
                            speed: None,
                            ..model.clone()
                        })
                        .collect(),
                },
            )),
            _ => None,
        })
        .collect()
}

/// For each device, the others whose managed models its agents use, so
/// one that takes no SSH logins keeps a link up for them.
pub(super) fn models_from(merge: &crate::merge::Merge) -> BTreeMap<DeviceId, Vec<DeviceId>> {
    let store = merge.store();
    let mut using: BTreeMap<DeviceId, Vec<DeviceId>> = BTreeMap::new();
    for entity in store.entities() {
        let Entity::Agent(agent) = entity else {
            continue;
        };
        let Some(Entity::Account(account)) = agent
            .account
            .and_then(|account| store.get(&EntityKey::Account(account)))
        else {
            continue;
        };
        let Some(Entity::ModelServer(server)) = account
            .model_server
            .and_then(|server| store.get(&EntityKey::ModelServer(server)))
        else {
            continue;
        };
        if server.device != account.device {
            let from = using.entry(account.device).or_default();
            if !from.contains(&server.device) {
                from.push(server.device);
                from.sort();
            }
        }
    }
    using
}

#[cfg(test)]
mod tests;
