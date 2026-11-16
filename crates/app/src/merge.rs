// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Merges what every link reports into the one store the windows read.
//!
//! Each link keeps its own copy of what its device last reported. An entity
//! shows while any link holds it, so a project two devices both report
//! stays until neither does. A link speaks only for the device that said
//! hello on it: [`claim`] says who each kind of entity belongs to, and the
//! view leaves out what a link reports for anyone else. Links keep what they
//! report either way, so the check runs each time a key resolves and doesn't
//! depend on the order reports arrive in.
//!
//! When more than one link holds a key, a report that fits its own ID wins,
//! then this machine's own `maxd`, then the device that alone reported an
//! agent running since another reported it too, then the device that reported
//! the key first. Among one device's links, a connected link wins over one
//! that's down, and then the latest report wins. While a link is down, its
//! agents show as [`Activity::NotAccessible`] with everything else as last
//! reported, and the next report brings their real activity back.
//!
//! The client works out links and networks for itself, so those come from
//! it and never from a device.
//!
//! An inbox change still waiting in the outbox shows on its agent at once,
//! applied on top of what the device last reported, until the device has
//! taken it and reports the agent itself. Its link lists the agent until
//! then, so the sidebar can say what waits.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use maxhq_client::{LinkEvent, LinkSpec};
use maxhq_core::git::normalize_remote;
use maxhq_core::store::Store;
use maxhq_core::{inbox, names};
use maxhq_protocol::ids::{AgentId, DeviceId, LinkId};
use maxhq_protocol::link::{DeviceLink, LinkState, Network, Route};
use maxhq_protocol::model::{Activity, InboxChange, Project};
use maxhq_protocol::state::{Change, Delta, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;

/// How many deltas the view keeps. A window never asks for old ones, so
/// this only has to cover the deltas in flight.
const KEEP_DELTAS: usize = 256;

/// How far a claim follows its chain, such as a forward to its port, the
/// port's agent, that agent's parent and its workspace. A device can report a
/// chain that loops, so the walk has to end.
const CHAIN_DEPTH: u8 = 6;

/// Inbox changes still on their way to each agent's device, oldest first,
/// with the link each waits for and when each was made.
pub type Waiting = BTreeMap<AgentId, Vec<(LinkId, InboxChange, Timestamp)>>;

/// The merged state and what each link holds.
pub struct Merge {
    store: Store,
    links: BTreeMap<LinkId, Slot>,
    /// Counts reports, so the latest one can win.
    clock: u64,
    /// The outbox, as the agents show it.
    waiting: Waiting,
    /// The device each agent last ran on while another device held a copy, so
    /// a session copied to another device stays there once it goes idle.
    ran_on: BTreeMap<AgentId, DeviceId>,
}

/// One link's view of its device.
struct Slot {
    /// The link as the client last reported it, under the registry's name
    /// and with the agents whose changes wait for it.
    link: DeviceLink,
    /// The device that last said hello on this link. A link opened again on
    /// new routes reports no device until its next hello, and what it holds
    /// stays shown until then.
    device: Option<DeviceId>,
    /// Whether the link is connected, so its entities are current.
    live: bool,
    /// The entities its device last reported.
    held: BTreeMap<EntityKey, Held>,
    /// The kinds this link has been refused, so the log says each one once.
    warned: BTreeSet<String>,
}

/// One entity as a link last reported it.
struct Held {
    entity: Entity,
    /// When the link first reported this key, on the merge's clock.
    since: u64,
    /// When the link reported it, on the merge's clock.
    at: u64,
}

/// Who may report an entity.
enum Claim {
    /// The device with this ID, and no other.
    Device(DeviceId),
    /// The device that speaks for these keys, or any device while nobody
    /// holds them. Nobody holds them while a report is still on its way, and
    /// once a workspace has gone and its agents stay as history.
    Through(EntityKey, Option<EntityKey>),
    /// Every device, as with a project several devices have checked out.
    Anyone,
    /// No device. The client works these out for itself.
    Nobody,
}

impl Default for Merge {
    fn default() -> Self {
        Self::new()
    }
}

impl Merge {
    /// An empty view with no links.
    pub fn new() -> Self {
        Self {
            store: Store::new(KEEP_DELTAS),
            links: BTreeMap::new(),
            clock: 0,
            waiting: Waiting::new(),
            ran_on: BTreeMap::new(),
        }
    }

    /// The merged state.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The link with this ID, as the view shows it.
    #[cfg(test)]
    pub fn link(&self, id: LinkId) -> Option<&DeviceLink> {
        self.links.get(&id).map(|slot| &slot.link)
    }

    /// The links to the device the view shows `key` from, connected ones
    /// first.
    pub fn holders(&self, key: &EntityKey) -> Vec<&DeviceLink> {
        let Some((winner, _)) = self.winner(key, CHAIN_DEPTH) else {
            return Vec::new();
        };
        let mut holders: Vec<&Slot> = self
            .links
            .values()
            .filter(|slot| slot.device == winner.device)
            .filter(|slot| {
                let held = slot.held.get(key);
                held.is_some_and(|held| self.allowed(slot, &held.entity, CHAIN_DEPTH))
            })
            .collect();
        holders.sort_by_key(|slot| !slot.live);
        holders.into_iter().map(|slot| &slot.link).collect()
    }

    /// What one link holds for `key`, whichever link the view shows it from.
    pub fn held_by(&self, link: LinkId, key: &EntityKey) -> Option<&Entity> {
        let slot = self.links.get(&link)?;
        slot.held.get(key).map(|held| &held.entity)
    }

    /// Starts tracking a device you added, before its link reports
    /// anything. Adding one that's already there updates its name and
    /// route.
    pub fn add_link(&mut self, spec: &LinkSpec) -> Option<Delta> {
        let slot = self.links.entry(spec.id).or_insert_with(|| Slot {
            link: DeviceLink {
                routes: Vec::new(),
                id: spec.id,
                name: spec.name.clone(),
                device: None,
                route: spec.route.clone(),
                state: LinkState::Unknown,
                network: None,
                peer: None,
                via: Vec::new(),
                capabilities: None,
                protocol: None,
                waiting: waiting_for(&self.waiting, spec.id),
                claim: None,
            },
            device: None,
            live: false,
            held: BTreeMap::new(),
            warned: BTreeSet::new(),
        });
        slot.link.name.clone_from(&spec.name);
        slot.link.route = spec.route.clone();
        let link = slot.link.clone();
        self.store.apply([Change::Upsert {
            value: Entity::DeviceLink(link),
        }])
    }

    /// Gives a link the name you chose. The client keeps the name it was
    /// opened with, so this one wins over what the client reports.
    pub fn rename_link(&mut self, id: LinkId, name: &str) -> Option<Delta> {
        let slot = self.links.get_mut(&id)?;
        name.clone_into(&mut slot.link.name);
        let link = slot.link.clone();
        self.store.apply([Change::Upsert {
            value: Entity::DeviceLink(link),
        }])
    }

    /// Forgets a link and everything only it held.
    pub fn remove_link(&mut self, id: LinkId) -> Option<Delta> {
        let slot = self.links.remove(&id)?;
        // Without this machine's link, others may speak for its device again.
        let keys = if slot.link.route == Route::Local {
            self.all_keys().chain(slot.held.into_keys()).collect()
        } else {
            self.with_children(slot.held.into_keys().collect())
        };
        let mut changes = vec![Change::Remove {
            key: EntityKey::DeviceLink(id),
        }];
        changes.extend(self.resolve_all(&keys));
        self.store.apply(changes)
    }

    /// Applies what a link reported. Reports from a link that isn't
    /// tracked, such as one just removed, change nothing.
    pub fn link_event(&mut self, id: LinkId, event: LinkEvent) -> Option<Delta> {
        self.clock += 1;
        let at = self.clock;
        let slot = self.links.get_mut(&id)?;
        match event {
            LinkEvent::Link(link) => {
                let name = std::mem::take(&mut slot.link.name);
                let waiting = std::mem::take(&mut slot.link.waiting);
                let who = (slot.device, slot.link.route == Route::Local);
                slot.link = *link;
                slot.link.name = name;
                slot.link.waiting = waiting;
                slot.device = slot.link.device.or(slot.device);
                let was_live = slot.live;
                slot.live = matches!(slot.link.state, LinkState::Connected { .. });
                let mut changes = vec![Change::Upsert {
                    value: Entity::DeviceLink(slot.link.clone()),
                }];
                let own: BTreeSet<EntityKey> = slot.held.keys().copied().collect();
                if who != (slot.device, slot.link.route == Route::Local) {
                    // Who this link speaks for decides what every link may say.
                    let keys = self.all_keys().collect();
                    self.warn_refused(&keys);
                    changes.extend(self.resolve_all(&keys));
                } else if slot.live != was_live {
                    changes.extend(self.resolve_all(&own));
                }
                self.store.apply(changes)
            }
            LinkEvent::Synced(entities) => {
                let mut held = BTreeMap::new();
                for entity in entities.into_iter().filter(from_device) {
                    let key = entity.key();
                    let old = slot.held.get(&key);
                    let since = old.map_or(at, |old| old.since);
                    // An entity the sync didn't change keeps its time, so a
                    // sync alone doesn't take a key from another link.
                    let at = match old {
                        Some(old) if old.entity == entity => old.at,
                        _ => at,
                    };
                    held.insert(key, Held { entity, since, at });
                }
                let old = std::mem::replace(&mut slot.held, held);
                let keys: BTreeSet<EntityKey> =
                    old.into_keys().chain(slot.held.keys().copied()).collect();
                let changes = self.reported(keys);
                self.store.apply(changes)
            }
            LinkEvent::Changed(changes) => {
                let mut keys = BTreeSet::new();
                for change in changes {
                    match change {
                        Change::Upsert { value } if from_device(&value) => {
                            let key = value.key();
                            let since = slot.held.get(&key).map_or(at, |old| old.since);
                            let entity = value;
                            slot.held.insert(key, Held { entity, since, at });
                            keys.insert(key);
                        }
                        Change::Remove { key } if slot.held.remove(&key).is_some() => {
                            keys.insert(key);
                        }
                        _ => {}
                    }
                }
                let changes = self.reported(keys);
                self.store.apply(changes)
            }
            // Timelines and workflow runs go to the windows that opened
            // them, not the view, and a route that worked goes to the
            // registry. Only `maxd`'s peer links get requests.
            LinkEvent::Timeline { .. }
            | LinkEvent::Workflow { .. }
            | LinkEvent::RouteWorked { .. }
            | LinkEvent::PeerRequest { .. } => None,
        }
    }

    /// Shows each agent with the changes waiting for its device applied, in
    /// the order they were made and as of when each was made, and each link
    /// with the agents whose changes wait for it.
    pub fn set_waiting(&mut self, waiting: Waiting) -> Option<Delta> {
        let old = std::mem::replace(&mut self.waiting, waiting);
        let mut changes = Vec::new();
        for (id, slot) in &mut self.links {
            slot.link.waiting = waiting_for(&self.waiting, *id);
            changes.push(Change::Upsert {
                value: Entity::DeviceLink(slot.link.clone()),
            });
        }
        let keys: BTreeSet<EntityKey> = old
            .keys()
            .chain(self.waiting.keys())
            .map(|agent| EntityKey::Agent(*agent))
            .collect();
        changes.extend(self.resolve_all(&keys));
        self.store.apply(changes)
    }

    /// Makes the networks in the view exactly `networks`.
    pub fn set_networks(&mut self, networks: Vec<Network>) -> Option<Delta> {
        self.store.replace(
            |entity| matches!(entity, Entity::Network(_)),
            networks.into_iter().map(Entity::Network),
        )
    }

    /// The change that brings each key in line with what the links hold.
    fn resolve_all(&self, keys: &BTreeSet<EntityKey>) -> Vec<Change> {
        keys.iter()
            .map(|key| match self.resolve(key) {
                Some(value) => Change::Upsert { value },
                None => Change::Remove { key: *key },
            })
            .collect()
    }

    /// What the view shows for `key`: the winning report, with agents on a
    /// link that's down marked not accessible. An agent also gets the changes
    /// waiting for it, and a project only what fits its ID.
    fn resolve(&self, key: &EntityKey) -> Option<Entity> {
        let (slot, held) = self.winner(key, CHAIN_DEPTH)?;
        let mut entity = held.entity.clone();
        match &mut entity {
            Entity::Agent(agent) => {
                if !slot.live {
                    agent.activity = Activity::NotAccessible;
                }
                for (_, change, at) in self.waiting.get(&agent.id).into_iter().flatten() {
                    agent.inbox = inbox::apply(&agent.inbox, change, *at);
                }
            }
            Entity::Project(project) => trust_project(project),
            _ => {}
        }
        Some(entity)
    }

    /// What a link's new report changes in the view.
    fn reported(&mut self, keys: BTreeSet<EntityKey>) -> Vec<Change> {
        self.note_running(&keys);
        let keys = self.with_children(keys);
        self.warn_refused(&keys);
        self.resolve_all(&keys)
    }

    /// The report the view shows for `key` and the link it came from.
    fn winner(&self, key: &EntityKey, depth: u8) -> Option<(&Slot, &Held)> {
        self.links
            .values()
            .filter_map(|slot| slot.held.get(key).map(|held| (slot, held)))
            .filter(|(slot, held)| self.allowed(slot, &held.entity, depth))
            .max_by_key(|(slot, held)| self.rank(key, slot, held))
    }

    /// How one link's report of `key` stands against the others. A device can
    /// put its own ID in a value and any key beside it, so the latest report
    /// can't simply win between devices.
    fn rank(
        &self,
        key: &EntityKey,
        slot: &Slot,
        held: &Held,
    ) -> (bool, bool, bool, Reverse<u64>, bool, u64) {
        let first = match key {
            // A phone paired again through another device moves to a new
            // keeper, so the newest book stands. The Mac's own record of
            // what you approved decides whether that keeper counts.
            EntityKey::PhoneBook(_) => 0,
            _ => self
                .links
                .values()
                .filter(|other| other.device == slot.device)
                .filter_map(|other| other.held.get(key))
                .map(|other| other.since)
                .min()
                .unwrap_or(held.since),
        };
        let ran_here = match key {
            EntityKey::Agent(agent) => self.ran_on.get(agent) == slot.device.as_ref(),
            _ => false,
        };
        (
            fits_its_id(&held.entity),
            slot.link.route == Route::Local,
            ran_here,
            Reverse(first),
            slot.live,
            held.at,
        )
    }

    /// Whether this link may report this entity. A link that hasn't said
    /// hello speaks for nobody yet, and what it holds waits for the hello.
    fn allowed(&self, slot: &Slot, entity: &Entity, depth: u8) -> bool {
        let Some(device) = slot.device else {
            return false;
        };
        if self.has_own_link(slot, device) {
            return false;
        }
        match claim(entity) {
            Claim::Device(owner) => owner == device,
            Claim::Through(first, second) => {
                let fits =
                    |key: &EntityKey| self.owner(key, depth).is_none_or(|owner| owner == device);
                fits(&first) && second.as_ref().is_none_or(fits)
            }
            Claim::Anyone => true,
            Claim::Nobody => false,
        }
    }

    /// Whether `device` is this machine, reached over its own link while this
    /// other link names it too. A remote link can name any device in its
    /// hello, and this machine knows it reaches its own `maxd`.
    fn has_own_link(&self, slot: &Slot, device: DeviceId) -> bool {
        slot.link.route != Route::Local
            && self
                .links
                .values()
                .any(|other| other.link.route == Route::Local && other.device == Some(device))
    }

    /// The device that speaks for `key`. A report that doesn't fit its own ID
    /// speaks for nothing, so claiming a workspace that's gone doesn't take
    /// the agents left on it.
    fn owner(&self, key: &EntityKey, depth: u8) -> Option<DeviceId> {
        let (slot, held) = self.winner(key, depth.checked_sub(1)?)?;
        fits_its_id(&held.entity).then_some(slot.device)?
    }

    /// Notes the device an agent runs on, when another device holds a copy
    /// that doesn't run. A session can be copied to another device by hand,
    /// and the old one keeps reporting its copy.
    fn note_running(&mut self, keys: &BTreeSet<EntityKey>) {
        for key in keys {
            let EntityKey::Agent(agent) = key else {
                continue;
            };
            let (mut holding, mut running) = (BTreeSet::new(), BTreeSet::new());
            for slot in self.links.values() {
                let (Some(device), Some(held)) = (slot.device, slot.held.get(key)) else {
                    continue;
                };
                let Entity::Agent(value) = &held.entity else {
                    continue;
                };
                if self.allowed(slot, &held.entity, CHAIN_DEPTH) {
                    holding.insert(device);
                    if !inbox::resting(value) {
                        running.insert(device);
                    }
                }
            }
            if holding.len() < 2 || running.len() != 1 {
                continue;
            }
            let Some(device) = running.pop_first() else {
                continue;
            };
            if self.ran_on.insert(*agent, device) != Some(device) {
                tracing::warn!(%agent, %device, "two devices report this agent, and this one runs it");
            }
        }
    }

    /// The keys, with every key that hangs from them, since a report can
    /// change who speaks for a key's children.
    fn with_children(&self, keys: BTreeSet<EntityKey>) -> BTreeSet<EntityKey> {
        // One map and one walk, since a device chooses how long a chain it
        // reports and this runs on every report.
        let mut children: BTreeMap<EntityKey, Vec<EntityKey>> = BTreeMap::new();
        for (key, held) in self.links.values().flat_map(|slot| &slot.held) {
            if let Claim::Through(first, second) = claim(&held.entity) {
                children.entry(first).or_default().push(*key);
                if let Some(second) = second {
                    children.entry(second).or_default().push(*key);
                }
            }
        }
        let mut walk: Vec<EntityKey> = keys.iter().copied().collect();
        let mut keys = keys;
        while let Some(key) = walk.pop() {
            for child in children.get(&key).into_iter().flatten() {
                if keys.insert(*child) {
                    walk.push(*child);
                }
            }
        }
        keys
    }

    /// Every key any link holds.
    fn all_keys(&self) -> impl Iterator<Item = EntityKey> + '_ {
        self.links
            .values()
            .flat_map(|slot| slot.held.keys().copied())
    }

    /// Logs what a link reports for someone else, once per link and kind, so a
    /// kind these rules get wrong shows up. A report can get another link's
    /// older report refused, so this looks at every link.
    fn warn_refused(&mut self, keys: &BTreeSet<EntityKey>) {
        let mut refused = Vec::new();
        for (id, slot) in &self.links {
            if slot.device.is_none() {
                continue;
            }
            for held in keys.iter().filter_map(|key| slot.held.get(key)) {
                if !self.allowed(slot, &held.entity, CHAIN_DEPTH) {
                    refused.push((*id, kind(&held.entity.key())));
                }
            }
        }
        for (id, kind) in refused {
            let Some(slot) = self.links.get_mut(&id) else {
                continue;
            };
            if slot.warned.insert(kind.clone()) {
                tracing::warn!(link = %id, device = ?slot.device, %kind, "refused what this link reported for someone else");
            }
        }
    }
}

/// The agents with changes waiting for `link`, in ID order.
fn waiting_for(waiting: &Waiting, link: LinkId) -> Vec<AgentId> {
    waiting
        .iter()
        .filter(|(_, changes)| changes.iter().any(|(id, ..)| *id == link))
        .map(|(agent, _)| *agent)
        .collect()
}

/// Whether a device may report this entity. Links and networks are the
/// client's own business.
fn from_device(entity: &Entity) -> bool {
    !matches!(claim(entity), Claim::Nobody)
}

/// Who may report this entity. A new kind fails to compile here until someone
/// decides its rule.
fn claim(entity: &Entity) -> Claim {
    match entity {
        Entity::Device(device) => Claim::Device(device.id),
        Entity::DeviceSettings(settings) => Claim::Device(settings.device),
        Entity::Workspace(workspace) => Claim::Device(workspace.device),
        Entity::Account(account) => Claim::Device(account.device),
        Entity::ProjectSettings(settings) => Claim::Device(settings.device),
        Entity::Credential(credential) => Claim::Device(credential.device),
        Entity::DeviceTools(tools) => Claim::Device(tools.device),
        Entity::DeviceTasks(tasks) => Claim::Device(tasks.device),
        Entity::AcpAgent(agent) => Claim::Device(agent.device),
        Entity::TestHost(host) => Claim::Device(host.device),
        Entity::TestLease(lease) => Claim::Device(lease.device),
        Entity::WorkflowRun(run) => Claim::Device(run.key.device),
        Entity::HostReport(report) => Claim::Device(report.device),
        Entity::Move(moving) => Claim::Device(moving.device),
        Entity::AcpRegistry(registry) => Claim::Device(registry.device),
        Entity::SkillLibrary(library) => Claim::Device(library.device),
        Entity::ProjectSkills(skills) => Claim::Device(skills.device),
        Entity::SkillProposal(proposal) => Claim::Device(proposal.device),
        Entity::Phone(phone) => Claim::Device(phone.device),
        Entity::PhoneBook(book) => Claim::Device(book.device),
        Entity::NetworkAccess(access) => Claim::Device(access.device),
        Entity::PhoneSession(session) => Claim::Device(session.device),
        Entity::HiddenProjects(hidden) => Claim::Device(hidden.device),
        Entity::Dispatcher(dispatcher) => Claim::Device(dispatcher.device),
        Entity::CommandRules(rules) => Claim::Device(rules.device),
        Entity::ProjectNotes(notes) => Claim::Device(notes.device),
        Entity::AppKey(key) => Claim::Device(key.device),
        Entity::Admission(admission) => Claim::Device(admission.device),
        // A guest's agents and ports come through its host, under the host's
        // ID, so these hang from a workspace that host reports.
        Entity::Agent(agent) => Claim::Through(
            EntityKey::Workspace(agent.workspace),
            agent.parent.map(EntityKey::Agent),
        ),
        Entity::ListeningPort(port) => Claim::Through(
            EntityKey::Workspace(port.workspace),
            port.agent.map(EntityKey::Agent),
        ),
        Entity::Attention(event) => Claim::Through(EntityKey::Agent(event.agent), None),
        // A terminal runs in its agent's folder, on the device that runs it.
        Entity::Terminal(terminal) => Claim::Through(EntityKey::Agent(terminal.agent), None),
        Entity::Forward(forward) => Claim::Through(EntityKey::ListeningPort(forward.port), None),
        Entity::Project(_) => Claim::Anyone,
        Entity::DeviceLink(_) | Entity::Network(_) => Claim::Nobody,
    }
}

/// Whether a report's ID comes from what the report says. Old records and a
/// few internal IDs don't, so this only ranks a report and never refuses one.
fn fits_its_id(entity: &Entity) -> bool {
    match entity {
        Entity::Workspace(workspace) => {
            workspace.id == names::workspace(workspace.device, &workspace.location)
        }
        Entity::Account(account) => {
            account.id == names::account(account.device, account.vendor, &account.config_dir)
        }
        Entity::WorkflowRun(run) => run.id == run.key.id(),
        Entity::Project(project) => repo_fits(project) == Some(true),
        _ => true,
    }
}

/// Whether a project's repository gives its ID and name, when it has one
/// that can. A project with no usable remote is named by a device and folder
/// it doesn't carry, so nothing here can check it.
fn repo_fits(project: &Project) -> Option<bool> {
    let repo = project.repo.as_deref()?;
    normalize_remote(repo)?;
    // The device only names a project with no usable remote.
    let named = names::project(DeviceId::derive(""), "", Some(repo));
    Some(named.id == project.id && named.name == project.name)
}

/// Takes out what a device can't speak to in a project every device may
/// report. No `maxd` sets the defaults, and a forged default workspace would
/// steer new agents to the device that reported it.
fn trust_project(project: &mut Project) {
    project.default_vendor = None;
    project.default_account = None;
    project.default_workspace = None;
    if repo_fits(project) == Some(false) {
        project.repo = None;
    }
}

/// The name of a key's kind, as the log shows it.
fn kind(key: &EntityKey) -> String {
    let name = format!("{key:?}");
    name.split(['(', ' ']).next().unwrap_or_default().to_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
    use maxhq_protocol::hosts::{HostReport, HostSource, Move, MoveState, MoveStep};
    use maxhq_protocol::ids::{
        AccountId, AgentId, AttentionId, BootId, ClientId, DeviceId, ForwardId, MoveId, NetworkId,
        PortId, ProjectId, WorkspaceId,
    };
    use maxhq_protocol::link::{NetworkKind, NetworkState};
    use maxhq_protocol::message::Capabilities;
    use maxhq_protocol::model::{
        Agent, AgentLink, BindAddress, Control, Device, Forward, ListeningPort, Vendor, Workspace,
        WorkspaceLocation,
    };
    use maxhq_protocol::netaccess::NetworkAccess;
    use maxhq_protocol::skills::SkillLibrary;
    use maxhq_protocol::time::Timestamp;

    pub(crate) fn spec(name: &str) -> LinkSpec {
        LinkSpec {
            fallbacks: Vec::new(),
            last_good: Default::default(),
            id: LinkId::derive(name),
            name: name.into(),
            route: Route::Ssh { host: name.into() },
        }
    }

    pub(crate) fn link_state(spec: &LinkSpec, state: LinkState) -> LinkEvent {
        LinkEvent::Link(Box::new(DeviceLink {
            routes: Vec::new(),
            id: spec.id,
            name: spec.name.clone(),
            device: Some(DeviceId::derive(&spec.name)),
            route: spec.route.clone(),
            state,
            network: None,
            peer: None,
            via: Vec::new(),
            capabilities: None,
            protocol: None,
            waiting: Vec::new(),
            claim: None,
        }))
    }

    pub(crate) fn connected() -> LinkState {
        LinkState::Connected {
            since: Timestamp(1),
        }
    }

    pub(crate) fn unreachable() -> LinkState {
        LinkState::Unreachable {
            reason: "Tailscale is off".into(),
            last_seen: None,
        }
    }

    pub(crate) fn project(name: &str) -> Entity {
        Entity::Project(Project {
            id: ProjectId::derive(name),
            name: name.into(),
            repo: None,
            default_vendor: None,
            default_account: None,
            default_workspace: None,
        })
    }

    pub(crate) fn workspace(name: &str, project: &str) -> Entity {
        Entity::Workspace(Workspace {
            id: WorkspaceId::derive(name),
            project: Some(ProjectId::derive(project)),
            device: DeviceId::derive("device"),
            location: WorkspaceLocation::Folder {
                path: format!("/src/{name}"),
            },
            managed: None,
            clone: None,
        })
    }

    pub(crate) fn agent(name: &str, workspace: &str, activity: Activity) -> Entity {
        Entity::Agent(Agent {
            id: AgentId::derive(name),
            workspace: WorkspaceId::derive(workspace),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: None,
            control: Control::Observed,
            pid: None,
            link: AgentLink::Live,
            title: format!("{name} title"),
            activity,
            context: None,
            started_at: Timestamp(1),
            last_activity_at: Timestamp(2),
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
        })
    }

    /// A workspace on `device`, under the ID that device gives it.
    fn workspace_on(device: &str, path: &str, project: &str) -> Entity {
        let device = DeviceId::derive(device);
        let location = WorkspaceLocation::Folder { path: path.into() };
        Entity::Workspace(Workspace {
            id: names::workspace(device, &location),
            project: Some(ProjectId::derive(project)),
            device,
            location,
            managed: None,
            clone: None,
        })
    }

    /// An agent in a workspace.
    fn agent_in(name: &str, workspace: &Entity, activity: Activity) -> Entity {
        let Entity::Workspace(workspace) = workspace else {
            unreachable!("agents live in workspaces");
        };
        match agent(name, "any", activity) {
            Entity::Agent(mut agent) => {
                agent.workspace = workspace.id;
                Entity::Agent(agent)
            }
            _ => unreachable!(),
        }
    }

    /// A project under the ID and name its remote gives it.
    fn repo_project(remote: &str) -> Entity {
        let named = names::project(DeviceId::derive("any"), "", Some(remote));
        Entity::Project(Project {
            id: named.id,
            name: named.name,
            repo: Some(remote.into()),
            default_vendor: None,
            default_account: None,
            default_workspace: None,
        })
    }

    /// A link saying hello as `device`.
    fn hello(spec: &LinkSpec, device: &str, state: LinkState) -> LinkEvent {
        let LinkEvent::Link(mut link) = link_state(spec, state) else {
            unreachable!("link_state reports a link");
        };
        link.device = Some(DeviceId::derive(device));
        LinkEvent::Link(link)
    }

    /// This machine's own link.
    fn local_spec() -> LinkSpec {
        LinkSpec {
            route: Route::Local,
            ..spec("local")
        }
    }

    /// A merge with this machine as `mac` and connected links to `remotes`,
    /// each saying hello under its own name.
    fn fleet(remotes: &[&str]) -> (Merge, LinkSpec) {
        let mut merge = connected_merge(remotes);
        let local = local_spec();
        merge.add_link(&local);
        merge.link_event(local.id, hello(&local, "mac", connected()));
        (merge, local)
    }

    fn upsert(values: impl IntoIterator<Item = Entity>) -> LinkEvent {
        LinkEvent::Changed(
            values
                .into_iter()
                .map(|value| Change::Upsert { value })
                .collect(),
        )
    }

    /// Fails on the first entity the view doesn't show as reported.
    #[track_caller]
    fn assert_shows(merge: &Merge, entities: &[Entity]) {
        for entity in entities {
            assert_eq!(shown(merge, entity.key()).as_ref(), Some(entity));
        }
    }

    fn connected_merge(names: &[&str]) -> Merge {
        let mut merge = Merge::new();
        for name in names {
            let spec = spec(name);
            merge.add_link(&spec);
            merge.link_event(spec.id, link_state(&spec, connected()));
        }
        merge
    }

    fn shown(merge: &Merge, key: EntityKey) -> Option<Entity> {
        merge.store().get(&key).cloned()
    }

    fn activity(merge: &Merge, name: &str) -> Option<Activity> {
        match shown(merge, EntityKey::Agent(AgentId::derive(name))) {
            Some(Entity::Agent(agent)) => Some(agent.activity),
            _ => None,
        }
    }

    #[test]
    fn a_project_two_devices_report_stays_until_neither_does() {
        let mut merge = connected_merge(&["mac", "nl"]);
        let (mac, nl) = (spec("mac").id, spec("nl").id);
        let key = EntityKey::Project(ProjectId::derive("maxhq"));
        merge.link_event(mac, LinkEvent::Synced(vec![project("maxhq")]));
        merge.link_event(nl, LinkEvent::Synced(vec![project("maxhq")]));
        let named = |merge: &Merge| merge.holders(&key)[0].id;
        assert_eq!(
            named(&merge),
            mac,
            "it shows from the device that said first"
        );

        merge.link_event(mac, LinkEvent::Changed(vec![Change::Remove { key }]));
        assert!(shown(&merge, key).is_some(), "nl still reports it");
        assert_eq!(named(&merge), nl);
        merge.link_event(nl, LinkEvent::Synced(vec![]));
        assert!(shown(&merge, key).is_none());
        assert!(merge.holders(&key).is_empty());
    }

    #[test]
    fn removing_a_link_drops_only_what_no_other_link_holds() {
        let mut merge = connected_merge(&["mac", "nl"]);
        let (mac, nl) = (spec("mac").id, spec("nl").id);
        merge.link_event(
            mac,
            LinkEvent::Synced(vec![project("maxhq"), project("comet")]),
        );
        merge.link_event(nl, LinkEvent::Synced(vec![project("maxhq")]));

        let delta = merge.remove_link(mac).unwrap();
        assert_eq!(
            delta.changes,
            [
                Change::Remove {
                    key: EntityKey::DeviceLink(mac)
                },
                Change::Remove {
                    key: EntityKey::Project(ProjectId::derive("comet"))
                },
            ]
        );
        assert!(shown(&merge, EntityKey::Project(ProjectId::derive("maxhq"))).is_some());
        assert!(merge.link_event(mac, LinkEvent::Synced(vec![])).is_none());
        assert!(merge.remove_link(mac).is_none());
    }

    #[test]
    fn on_one_device_a_connected_link_wins_and_then_the_latest_report() {
        let (lan, tailnet) = (spec("lan"), spec("tailnet"));
        let mut merge = Merge::new();
        for link in [&lan, &tailnet] {
            merge.add_link(link);
            merge.link_event(link.id, hello(link, "device", connected()));
        }
        let (mac, nl) = (lan.id, tailnet.id);
        let key = EntityKey::Project(ProjectId::derive("maxhq"));
        let renamed = |name: &str| match project("maxhq") {
            Entity::Project(mut p) => {
                p.name = name.into();
                Entity::Project(p)
            }
            _ => unreachable!(),
        };
        merge.link_event(mac, LinkEvent::Synced(vec![renamed("from mac")]));
        merge.link_event(nl, LinkEvent::Synced(vec![renamed("from nl")]));
        assert_eq!(shown(&merge, key), Some(renamed("from nl")));

        // A sync that changes nothing doesn't take the key back.
        merge.link_event(mac, LinkEvent::Synced(vec![renamed("from mac")]));
        assert_eq!(shown(&merge, key), Some(renamed("from nl")));

        // While nl is down, mac's connected copy wins although it's older.
        merge.link_event(nl, hello(&tailnet, "device", unreachable()));
        assert_eq!(shown(&merge, key), Some(renamed("from mac")));
        merge.link_event(nl, hello(&tailnet, "device", connected()));
        assert_eq!(shown(&merge, key), Some(renamed("from nl")));
    }

    #[test]
    fn agents_on_a_link_that_is_down_are_not_accessible_until_it_reports() {
        let mut merge = connected_merge(&["nl"]);
        let nl = spec("nl");
        merge.link_event(
            nl.id,
            LinkEvent::Synced(vec![
                project("maxhq"),
                workspace("w", "maxhq"),
                agent("a", "w", Activity::Busy),
            ]),
        );
        assert_eq!(activity(&merge, "a"), Some(Activity::Busy));

        let down = merge
            .link_event(
                nl.id,
                link_state(
                    &nl,
                    LinkState::Connecting {
                        step: maxhq_protocol::link::ConnectStep::Dialing,
                        attempt: 1,
                    },
                ),
            )
            .unwrap();
        assert_eq!(
            down.changes.len(),
            2,
            "the link and the agent, in one delta"
        );
        let Some(Entity::Agent(agent)) = shown(&merge, EntityKey::Agent(AgentId::derive("a")))
        else {
            panic!("the agent stays while its link is down");
        };
        assert_eq!(agent.activity, Activity::NotAccessible);
        assert_eq!(agent.title, "a title", "the last known title stays");
        assert_eq!(agent.workspace, WorkspaceId::derive("w"));
        assert!(shown(&merge, EntityKey::Project(ProjectId::derive("maxhq"))).is_some());

        // A report while it's down is kept, but still not accessible.
        merge.link_event(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: agent_entity("a", Activity::Idle),
            }]),
        );
        assert_eq!(activity(&merge, "a"), Some(Activity::NotAccessible));

        merge.link_event(nl.id, link_state(&nl, connected()));
        assert_eq!(activity(&merge, "a"), Some(Activity::Idle));
        merge.link_event(
            nl.id,
            LinkEvent::Changed(vec![Change::Upsert {
                value: agent_entity("a", Activity::DoneUnread),
            }]),
        );
        assert_eq!(activity(&merge, "a"), Some(Activity::DoneUnread));
    }

    fn inbox(merge: &Merge, name: &str) -> Option<maxhq_protocol::model::Inbox> {
        match shown(merge, EntityKey::Agent(AgentId::derive(name))) {
            Some(Entity::Agent(agent)) => Some(agent.inbox),
            _ => None,
        }
    }

    #[test]
    fn waiting_changes_show_on_the_agent_in_order_until_they_go() {
        let mut merge = connected_merge(&["nl"]);
        let nl = spec("nl");
        merge.link_event(
            nl.id,
            LinkEvent::Synced(vec![
                agent("a", "w", Activity::Idle),
                agent("b", "w", Activity::Idle),
            ]),
        );
        merge.link_event(nl.id, link_state(&nl, unreachable()));

        let pin = InboxChange::Pin { key: "m".into() };
        let delta = merge
            .set_waiting(Waiting::from([(
                AgentId::derive("a"),
                vec![
                    (nl.id, InboxChange::Settle, Timestamp(10)),
                    (nl.id, pin, Timestamp(20)),
                ],
            )]))
            .unwrap();
        assert_eq!(
            delta.changes.len(),
            2,
            "the link and the agent with changes"
        );
        let shown = inbox(&merge, "a").unwrap();
        assert_eq!(shown.settled, None, "the pin came after the settle");
        assert!(shown.kept_active);
        assert_eq!(
            shown.pin.map(|pin| (pin.at, pin.key)),
            Some((Timestamp(20), "m".into())),
            "applied as of when it was made"
        );
        assert_eq!(activity(&merge, "a"), Some(Activity::NotAccessible));
        assert_eq!(inbox(&merge, "b"), Some(Default::default()));

        // A report from the device keeps the overlay on top.
        merge.link_event(nl.id, link_state(&nl, connected()));
        assert!(inbox(&merge, "a").unwrap().pin.is_some());
        assert_eq!(activity(&merge, "a"), Some(Activity::Idle));

        assert!(merge.set_waiting(Waiting::new()).is_some());
        assert_eq!(inbox(&merge, "a"), Some(Default::default()));
        assert!(merge.set_waiting(Waiting::new()).is_none(), "nothing left");
    }

    fn waiting_on(merge: &Merge, name: &str) -> Vec<AgentId> {
        match shown(merge, EntityKey::DeviceLink(spec(name).id)) {
            Some(Entity::DeviceLink(link)) => link.waiting,
            other => panic!("expected the link, got {other:?}"),
        }
    }

    #[test]
    fn each_link_shows_the_agents_whose_changes_wait_for_it() {
        let mut merge = connected_merge(&["nl", "uk"]);
        let (nl, uk) = (spec("nl"), spec("uk"));
        let (a, b) = (AgentId::derive("a"), AgentId::derive("b"));
        merge.set_waiting(Waiting::from([
            (a, vec![(nl.id, InboxChange::Settle, Timestamp(10))]),
            (
                b,
                vec![
                    (uk.id, InboxChange::Settle, Timestamp(10)),
                    (uk.id, InboxChange::Unsettle, Timestamp(20)),
                ],
            ),
        ]));
        assert_eq!(waiting_on(&merge, "nl"), vec![a]);
        assert_eq!(waiting_on(&merge, "uk"), vec![b]);

        // The client reports a link without them, and the view keeps them.
        merge.link_event(nl.id, link_state(&nl, unreachable()));
        assert_eq!(waiting_on(&merge, "nl"), vec![a]);

        // A link added after the outbox shows, as at startup, picks them up.
        merge.remove_link(uk.id);
        merge.add_link(&uk);
        assert_eq!(waiting_on(&merge, "uk"), vec![b]);

        merge.set_waiting(Waiting::from([(
            a,
            vec![(nl.id, InboxChange::Settle, Timestamp(10))],
        )]));
        assert_eq!(waiting_on(&merge, "uk"), Vec::<AgentId>::new());
        assert_eq!(waiting_on(&merge, "nl"), vec![a]);
    }

    fn agent_entity(name: &str, activity: Activity) -> Entity {
        agent(name, "w", activity)
    }

    #[test]
    fn links_and_networks_come_from_the_client() {
        let mut merge = Merge::new();
        let nl = spec("nl");
        let added = merge.add_link(&nl).unwrap();
        assert!(matches!(
            &added.changes[..],
            [Change::Upsert { value: Entity::DeviceLink(link) }] if link.state == LinkState::Unknown
        ));

        // A device can't report links or networks.
        let network = Network {
            id: NetworkId::derive("tailnet"),
            kind: NetworkKind::Tailscale {
                magic_dns_suffix: None,
                client_version: None,
            },
            name: "tailnet".into(),
            state: NetworkState::Up,
        };
        let spoofed = DeviceLink {
            name: "spoofed".into(),
            ..merge.link(nl.id).unwrap().clone()
        };
        assert!(merge
            .link_event(
                nl.id,
                LinkEvent::Synced(vec![
                    Entity::DeviceLink(spoofed),
                    Entity::Network(network.clone()),
                ]),
            )
            .is_none());

        // The registry's name wins over the one the client reports.
        merge.rename_link(nl.id, "NL");
        let mut reported = link_state(&nl, connected());
        if let LinkEvent::Link(link) = &mut reported {
            link.name = "nl".into();
        }
        merge.link_event(nl.id, reported);
        assert_eq!(merge.link(nl.id).unwrap().name, "NL");
        assert!(matches!(
            shown(&merge, EntityKey::DeviceLink(nl.id)),
            Some(Entity::DeviceLink(link)) if link.name == "NL" && link.state == connected()
        ));

        merge.set_networks(vec![network.clone()]);
        assert_eq!(
            shown(&merge, EntityKey::Network(network.id)),
            Some(Entity::Network(network.clone()))
        );
        assert!(merge.set_networks(vec![network.clone()]).is_none());
        merge.set_networks(vec![]);
        assert!(shown(&merge, EntityKey::Network(network.id)).is_none());
    }

    #[test]
    fn what_a_device_does_reaches_the_view_and_stays_through_a_drop() {
        let mut merge = Merge::new();
        let nl = spec("nl");
        merge.add_link(&nl);
        let does = Capabilities {
            transfers: true,
            tunnels: false,
            workspaces: true,
            github_checks: true,
            inbox: false,
            ..Capabilities::default()
        };
        let report = |state| match link_state(&nl, state) {
            LinkEvent::Link(mut link) => {
                link.capabilities = Some(does);
                LinkEvent::Link(link)
            }
            _ => unreachable!(),
        };
        let capabilities = |merge: &Merge| match shown(merge, EntityKey::DeviceLink(nl.id)) {
            Some(Entity::DeviceLink(link)) => link.capabilities,
            _ => None,
        };

        assert_eq!(capabilities(&merge), None);
        merge.link_event(nl.id, report(connected()));
        assert_eq!(capabilities(&merge), Some(does));
        merge.link_event(nl.id, report(unreachable()));
        assert_eq!(capabilities(&merge), Some(does));
    }

    #[test]
    fn every_report_is_one_delta_and_repeats_make_none() {
        let mut merge = connected_merge(&["nl"]);
        let nl = spec("nl").id;
        let seq = merge.store().seq();
        let entities = vec![project("maxhq"), workspace_on("nl", "/w", "maxhq")];
        let delta = merge
            .link_event(nl, LinkEvent::Synced(entities.clone()))
            .unwrap();
        assert_eq!(delta.seq, seq.next());
        assert_eq!(delta.changes.len(), 2);
        assert!(merge.link_event(nl, LinkEvent::Synced(entities)).is_none());
        assert!(merge
            .link_event(nl, link_state(&spec("nl"), connected()))
            .is_none());
    }

    fn device_entity(device: &str, build: &str) -> Entity {
        Entity::Device(Device {
            id: DeviceId::derive(device),
            name: device.into(),
            os: "linux".into(),
            boot_id: BootId(device.into()),
            maxd_version: "1".into(),
            winding_down_until: None,
            workspace_host: None,
            build: Some(build.into()),
            target: None,
        })
    }

    fn host_report(device: &str, source: &str) -> Entity {
        Entity::HostReport(HostReport {
            device: DeviceId::derive(device),
            source: HostSource::File {
                path: source.into(),
            },
            checked_at: Timestamp(1),
            items: Vec::new(),
        })
    }

    fn network_access(device: &str, host: &str) -> Entity {
        Entity::NetworkAccess(NetworkAccess {
            device: DeviceId::derive(device),
            on: true,
            port: 6666,
            hosts: vec![host.into()],
            pin: Some("pin".into()),
            problem: None,
        })
    }

    fn skill_library(device: &str, skill: &str) -> Entity {
        Entity::SkillLibrary(SkillLibrary {
            device: DeviceId::derive(device),
            sources: Vec::new(),
            always: vec![skill.into()],
            always_skills: Vec::new(),
        })
    }

    fn a_move(device: &str, source: &str, workspace: &Entity) -> Entity {
        let Entity::Workspace(workspace) = workspace else {
            unreachable!("a move names a workspace");
        };
        Entity::Move(Move {
            id: MoveId::derive(&format!("move:{device}:{source}")),
            device: DeviceId::derive(device),
            source: source.into(),
            target: format!("mhq-{source}"),
            workspace: Some(workspace.id),
            step: MoveStep::Copy,
            state: MoveState::Running {
                doing: "Copying".into(),
            },
            plan: None,
            source_changed: false,
            kept_until: None,
            unsaved: Vec::new(),
            checklist: Vec::new(),
        })
    }

    fn attention_on(name: &str, agent: AgentId) -> Entity {
        Entity::Attention(AttentionEvent {
            id: AttentionId::derive(name),
            agent,
            kind: AttentionKind::Question,
            summary: name.into(),
            options: Vec::new(),
            accepts_text: true,
            created_at: Timestamp(1),
            expires_at: None,
        })
    }

    fn port_on(name: &str, workspace: &Entity, agent: Option<AgentId>) -> Entity {
        let Entity::Workspace(workspace) = workspace else {
            unreachable!("ports live in workspaces");
        };
        Entity::ListeningPort(ListeningPort {
            id: PortId::derive(name),
            workspace: workspace.id,
            agent,
            port: 3000,
            bind: BindAddress::Loopback,
            process: None,
            url_hint: None,
            first_seen: Timestamp(1),
            gone_since: None,
        })
    }

    fn forward_of(name: &str, port: PortId) -> Entity {
        Entity::Forward(Forward {
            id: ForwardId::derive(name),
            port,
            client: ClientId::derive("client"),
            local_port: 4000,
            opened_at: Timestamp(1),
        })
    }

    fn agent_id(name: &str) -> AgentId {
        AgentId::derive(name)
    }

    #[test]
    fn a_fleet_keeps_all_it_reports_through_moves_drops_and_returns() {
        let (mut merge, local) = fleet(&["node"]);
        let node = spec("node");

        // This Mac: a checkout of a project, an agent with a sub-agent, a port
        // that agent opened with a forward, and a question from it.
        let shared = repo_project("https://example.com/team/app.git");
        let EntityKey::Project(_) = shared.key() else {
            unreachable!()
        };
        let mac_folder = workspace_on("mac", "/src/app", "app");
        let mac_agent = agent_in("mac agent", &mac_folder, Activity::Busy);
        let mut mac_sub = agent_in("mac sub", &mac_folder, Activity::Busy);
        if let Entity::Agent(sub) = &mut mac_sub {
            sub.parent = Some(agent_id("mac agent"));
        }
        let mac_port = port_on("mac port", &mac_folder, Some(agent_id("mac agent")));
        let mac_report = vec![
            device_entity("mac", "b1"),
            network_access("mac", "192.0.2.1"),
            skill_library("mac", "style"),
            shared.clone(),
            mac_folder.clone(),
            mac_agent.clone(),
            mac_sub,
            mac_port,
            forward_of("mac forward", PortId::derive("mac port")),
            attention_on("mac question", agent_id("mac agent")),
        ];

        // The server: the same project in a container, whose guest reports
        // through it, an agent in there, and a folder agent whose workspace
        // has gone and which still shows as history.
        let location = WorkspaceLocation::Container {
            instance: "ws-app".into(),
            path: "/work/app".into(),
        };
        let device = DeviceId::derive("node");
        let container = Entity::Workspace(Workspace {
            id: names::workspace(device, &location),
            project: Some(match &shared {
                Entity::Project(project) => project.id,
                _ => unreachable!(),
            }),
            device,
            location,
            managed: None,
            clone: None,
        });
        let guest_agent = agent_in("guest agent", &container, Activity::Busy);
        let gone = workspace_on("node", "/src/gone", "app");
        let history = agent_in("history", &gone, Activity::Idle);
        let node_report = vec![
            device_entity("node", "b1"),
            network_access("node", "192.0.2.2"),
            host_report("node", "spec"),
            shared.clone(),
            container.clone(),
            guest_agent.clone(),
            history.clone(),
            port_on("node port", &container, Some(agent_id("guest agent"))),
            attention_on("node question", agent_id("guest agent")),
        ];
        merge.link_event(local.id, LinkEvent::Synced(mac_report.clone()));
        merge.link_event(node.id, LinkEvent::Synced(node_report.clone()));
        assert_shows(&merge, &mac_report);
        assert_shows(&merge, &node_report);

        // A batch brings an agent before its workspace, and a port and its
        // question before the agent.
        let new_folder = workspace_on("node", "/src/new", "app");
        let new_agent = agent_in("new agent", &new_folder, Activity::Busy);
        let arrivals = vec![
            attention_on("new question", agent_id("new agent")),
            port_on("new port", &new_folder, Some(agent_id("new agent"))),
            new_agent.clone(),
            new_folder.clone(),
        ];
        merge.link_event(node.id, upsert(arrivals.clone()));
        assert_shows(&merge, &arrivals);

        // A move on the server: the old container stays while a new one takes
        // over, and the agent resumes in it under its own ID.
        let target = WorkspaceLocation::Container {
            instance: "mhq-app".into(),
            path: "/work/app".into(),
        };
        let moved_to = Entity::Workspace(Workspace {
            id: names::workspace(device, &target),
            project: Some(match &shared {
                Entity::Project(project) => project.id,
                _ => unreachable!(),
            }),
            device,
            location: target,
            managed: None,
            clone: None,
        });
        let moved_agent = agent_in("guest agent", &moved_to, Activity::Busy);
        let moving = a_move("node", "ws-app", &moved_to);
        let during = vec![moved_to.clone(), moved_agent.clone(), moving.clone()];
        merge.link_event(node.id, upsert(during.clone()));
        assert_shows(&merge, &during);
        assert_shows(&merge, std::slice::from_ref(&container));
        merge.link_event(
            node.id,
            LinkEvent::Changed(vec![Change::Remove {
                key: container.key(),
            }]),
        );
        assert_shows(&merge, &during);
        assert_shows(&merge, &[history.clone(), new_agent.clone()]);

        // The server drops. All of it stays, with its agents not accessible,
        // and the Mac's rows don't move. Sending still has its way back.
        merge.link_event(node.id, hello(&node, "node", unreachable()));
        assert_shows(&merge, &mac_report);
        assert_shows(&merge, &during[..1]);
        assert_eq!(
            activity(&merge, "guest agent"),
            Some(Activity::NotAccessible),
            "its agents aren't accessible while it's down"
        );
        assert_eq!(activity(&merge, "history"), Some(Activity::NotAccessible));
        assert!(shown(
            &merge,
            EntityKey::ListeningPort(PortId::derive("node port"))
        )
        .is_some());
        assert_eq!(
            merge.holders(&EntityKey::Agent(agent_id("guest agent")))[0].id,
            node.id
        );

        // It comes back on a new route, which reports no device until its
        // hello. Nothing goes missing in between.
        let mut reopened = link_state(&node, LinkState::Unknown);
        if let LinkEvent::Link(link) = &mut reopened {
            link.device = None;
        }
        merge.link_event(node.id, reopened);
        assert_shows(&merge, &during[..1]);
        assert!(shown(&merge, EntityKey::Agent(agent_id("guest agent"))).is_some());

        merge.link_event(node.id, hello(&node, "node", connected()));
        let after = vec![
            device_entity("node", "b2"),
            network_access("node", "192.0.2.2"),
            host_report("node", "spec"),
            shared.clone(),
            moved_to,
            moved_agent,
            moving,
            history,
            new_folder,
            new_agent,
        ];
        merge.link_event(node.id, LinkEvent::Synced(after.clone()));
        assert_shows(&merge, &after);
        assert_shows(&merge, &mac_report);

        assert!(
            merge.links.values().all(|slot| slot.warned.is_empty()),
            "nothing honest is refused"
        );

        // The link goes for good. The Mac still reports the project, and its
        // checkout on the server goes with the server.
        merge.remove_link(node.id);
        assert_shows(&merge, &mac_report);
        assert!(shown(&merge, EntityKey::Device(device)).is_none());
    }

    #[test]
    fn a_link_speaks_only_for_the_device_that_said_hello_on_it() {
        let forged = |victim: &str| {
            vec![
                device_entity(victim, "forged"),
                network_access(victim, "203.0.113.66"),
                host_report(victim, "forged"),
                skill_library(victim, "forged"),
                workspace_on(victim, "/forged", "app"),
            ]
        };
        for reports_first in [false, true] {
            let mut merge = connected_merge(&["victim", "hostile"]);
            let (victim, hostile) = (spec("victim").id, spec("hostile").id);
            let honest = vec![
                device_entity("victim", "real"),
                network_access("victim", "192.0.2.3"),
                host_report("victim", "real"),
                skill_library("victim", "real"),
            ];
            if reports_first {
                merge.link_event(hostile, LinkEvent::Synced(forged("victim")));
            }
            merge.link_event(victim, LinkEvent::Synced(honest.clone()));
            merge.link_event(hostile, upsert(forged("victim")));
            assert_shows(&merge, &honest);
            let forged_folder = workspace_on("victim", "/forged", "app");
            assert!(shown(&merge, forged_folder.key()).is_none());
            assert!(merge.holders(&forged_folder.key()).is_empty());

            // The victim going doesn't let the forgery through either.
            merge.remove_link(victim);
            for entity in forged("victim") {
                assert!(shown(&merge, entity.key()).is_none(), "{entity:?}");
            }
        }

        // The log says each kind once, for each link.
        let mut merge = connected_merge(&["hostile"]);
        let hostile = spec("hostile").id;
        merge.link_event(hostile, upsert(forged("one")));
        merge.link_event(hostile, upsert(forged("two")));
        let warned = &merge.links[&hostile].warned;
        let kinds: Vec<&str> = warned.iter().map(String::as_str).collect();
        assert_eq!(
            kinds,
            [
                "Device",
                "HostReport",
                "NetworkAccess",
                "SkillLibrary",
                "Workspace"
            ]
        );
    }

    #[test]
    fn what_a_link_reports_before_its_hello_waits_for_it() {
        let mut merge = Merge::new();
        let node = spec("node");
        merge.add_link(&node);
        let folder = workspace_on("node", "/src/app", "app");
        let report = vec![device_entity("node", "b1"), folder.clone(), project("app")];
        merge.link_event(node.id, LinkEvent::Synced(report.clone()));
        assert!(shown(&merge, folder.key()).is_none());
        assert!(
            merge.links[&node.id].warned.is_empty(),
            "nothing is refused yet"
        );

        merge.link_event(node.id, hello(&node, "node", connected()));
        assert_shows(&merge, &report);
    }

    #[test]
    fn a_device_can_t_hang_its_reports_on_another_device_s_keys() {
        let folder = workspace_on("victim", "/src/app", "app");
        let real = agent_in("real", &folder, Activity::Busy);
        let port = port_on("real port", &folder, Some(agent_id("real")));
        let honest = vec![
            folder.clone(),
            real.clone(),
            port.clone(),
            attention_on("real question", agent_id("real")),
        ];
        let own = workspace_on("hostile", "/own", "app");
        let mut sub = agent_in("forged sub", &own, Activity::Busy);
        if let Entity::Agent(sub) = &mut sub {
            sub.parent = Some(agent_id("real"));
        }
        let forged = vec![
            agent_in("forged", &folder, Activity::Busy),
            sub,
            port_on("forged port", &folder, None),
            port_on("forged own port", &own, Some(agent_id("real"))),
            attention_on("forged question", agent_id("real")),
            forward_of("forged forward", PortId::derive("real port")),
        ];
        let refused = |merge: &Merge| {
            for entity in &forged {
                assert!(shown(merge, entity.key()).is_none(), "{entity:?}");
            }
        };

        // Whichever arrives first.
        let mut merge = connected_merge(&["victim", "hostile"]);
        let (victim, hostile) = (spec("victim").id, spec("hostile").id);
        merge.link_event(hostile, upsert(forged.clone()));
        merge.link_event(victim, upsert(honest.clone()));
        refused(&merge);
        assert_shows(&merge, &honest);
        merge.link_event(hostile, upsert([own.clone()]));
        assert_shows(&merge, &[own]);
        refused(&merge);

        // Its own port and question on its own agent stay its own.
        let mine = workspace_on("hostile", "/mine", "app");
        let its_agent = agent_in("its agent", &mine, Activity::Busy);
        let its_own = vec![
            mine.clone(),
            its_agent,
            port_on("its port", &mine, Some(agent_id("its agent"))),
            attention_on("its question", agent_id("its agent")),
            forward_of("its forward", PortId::derive("its port")),
        ];
        merge.link_event(hostile, upsert(its_own.clone()));
        assert_shows(&merge, &its_own);

        // A chain that loops ends, and shows nothing wrong.
        let looped = |name: &str, parent: &str| match agent_in(name, &mine, Activity::Busy) {
            Entity::Agent(mut agent) => {
                agent.parent = Some(agent_id(parent));
                Entity::Agent(agent)
            }
            _ => unreachable!(),
        };
        let loops = vec![
            looped("one", "two"),
            looped("two", "one"),
            looped("self", "self"),
        ];
        merge.link_event(hostile, upsert(loops.clone()));
        assert_shows(&merge, &loops);
    }

    #[test]
    fn a_device_can_t_take_a_workspace_by_putting_its_own_id_in_it() {
        let folder = workspace_on("victim", "/src/app", "app");
        let real = agent_in("real", &folder, Activity::Idle);
        let forged = match &folder {
            Entity::Workspace(workspace) => Entity::Workspace(Workspace {
                device: DeviceId::derive("hostile"),
                location: WorkspaceLocation::Folder {
                    path: "/forged".into(),
                },
                ..workspace.clone()
            }),
            _ => unreachable!(),
        };
        let forged_agent = agent_in("forged", &folder, Activity::Busy);

        for hostile_first in [true, false] {
            let mut merge = connected_merge(&["victim", "hostile"]);
            let (victim, hostile) = (spec("victim").id, spec("hostile").id);
            let claim = upsert([forged.clone(), forged_agent.clone()]);
            let honest = upsert([folder.clone(), real.clone()]);
            let order = if hostile_first {
                [(hostile, claim), (victim, honest)]
            } else {
                [(victim, honest), (hostile, claim)]
            };
            for (link, event) in order {
                merge.link_event(link, event);
            }
            assert_shows(&merge, &[folder.clone(), real.clone()]);
            assert!(shown(&merge, forged_agent.key()).is_none());
        }

        // Claiming one that's gone doesn't take the agents left on it, which
        // stay as history.
        let mut merge = connected_merge(&["victim", "hostile"]);
        let (victim, hostile) = (spec("victim").id, spec("hostile").id);
        merge.link_event(victim, upsert([real.clone()]));
        merge.link_event(hostile, upsert([forged.clone()]));
        assert_shows(&merge, &[real]);
    }

    #[test]
    fn a_session_copied_to_another_device_follows_the_one_that_runs_it() {
        let mut merge = connected_merge(&["old", "new"]);
        let (old, new) = (spec("old"), spec("new"));
        let there = workspace_on("old", "/src/app", "app");
        let here = workspace_on("new", "/work/app", "app");
        let copy = |workspace: &Entity, activity: Activity| agent_in("copied", workspace, activity);
        let key = EntityKey::Agent(agent_id("copied"));
        merge.link_event(
            old.id,
            upsert([there.clone(), copy(&there, Activity::Idle)]),
        );

        // Another device reporting a copy that isn't running takes nothing.
        merge.link_event(new.id, upsert([here.clone(), copy(&here, Activity::Idle)]));
        assert_shows(&merge, &[copy(&there, Activity::Idle)]);

        // It runs there now, while the old device goes on reporting its copy.
        merge.link_event(new.id, upsert([copy(&here, Activity::Busy)]));
        assert_shows(&merge, &[copy(&here, Activity::Busy)]);
        merge.link_event(old.id, upsert([copy(&there, Activity::Idle)]));
        assert_shows(&merge, &[copy(&here, Activity::Busy)]);
        assert_eq!(merge.holders(&key)[0].id, new.id);

        // It stays once it rests, and through either link dropping.
        merge.link_event(new.id, upsert([copy(&here, Activity::Idle)]));
        assert_shows(&merge, &[copy(&here, Activity::Idle)]);
        merge.link_event(old.id, hello(&old, "old", unreachable()));
        merge.link_event(old.id, hello(&old, "old", connected()));
        merge.link_event(
            old.id,
            LinkEvent::Synced(vec![there.clone(), copy(&there, Activity::Idle)]),
        );
        assert_shows(&merge, &[copy(&here, Activity::Idle)]);
        assert_eq!(merge.holders(&key)[0].id, new.id);

        // Resumed back on the old device, it goes back.
        merge.link_event(old.id, upsert([copy(&there, Activity::Busy)]));
        assert_shows(&merge, &[copy(&there, Activity::Busy)]);

        // The copy that's left shows when the other device forgets it.
        merge.link_event(old.id, LinkEvent::Changed(vec![Change::Remove { key }]));
        assert_shows(&merge, &[copy(&here, Activity::Idle)]);
    }

    #[test]
    fn a_forged_copy_of_an_agent_takes_neither_a_running_one_nor_this_machine_s() {
        let (mut merge, local) = fleet(&["victim", "hostile"]);
        let (victim, hostile) = (spec("victim").id, spec("hostile").id);
        let folder = workspace_on("victim", "/src/app", "app");
        let own = workspace_on("hostile", "/own", "app");
        let mine = workspace_on("mac", "/src/app", "app");
        let running = agent_in("running", &folder, Activity::Busy);
        let here = agent_in("here", &mine, Activity::Idle);
        merge.link_event(victim, upsert([folder, running.clone()]));
        merge.link_event(local.id, upsert([mine, here.clone()]));

        let forged = vec![
            own,
            agent_in(
                "running",
                &workspace_on("hostile", "/own", "app"),
                Activity::Busy,
            ),
            agent_in(
                "here",
                &workspace_on("hostile", "/own", "app"),
                Activity::Busy,
            ),
        ];
        merge.link_event(hostile, upsert(forged));
        assert_shows(&merge, &[running, here]);
        assert_eq!(
            merge.holders(&EntityKey::Agent(agent_id("running")))[0].id,
            victim
        );
        assert_eq!(
            merge.holders(&EntityKey::Agent(agent_id("here")))[0].id,
            local.id
        );
    }

    #[test]
    fn a_remote_link_that_names_this_machine_speaks_for_nothing() {
        let local = local_spec();
        let hostile = spec("hostile");
        let mut merge = Merge::new();
        merge.add_link(&local);
        merge.add_link(&hostile);

        // It says hello before this machine's link does, and shows only until
        // this machine's link knows its own ID.
        merge.link_event(hostile.id, hello(&hostile, "mac", connected()));
        let forged = vec![
            device_entity("mac", "forged"),
            workspace_on("mac", "/forged", "app"),
            project("forged"),
        ];
        merge.link_event(hostile.id, LinkEvent::Synced(forged.clone()));
        assert!(shown(&merge, forged[0].key()).is_some());

        merge.link_event(local.id, hello(&local, "mac", connected()));
        for entity in &forged {
            assert!(shown(&merge, entity.key()).is_none(), "{entity:?}");
        }
        assert!(merge.links[&hostile.id].warned.contains("Device"));
        let real = device_entity("mac", "real");
        merge.link_event(local.id, upsert([real.clone()]));
        assert_shows(&merge, &[real]);
    }

    #[test]
    fn a_project_shows_only_what_fits_its_id() {
        let real = repo_project("https://example.com/team/app.git");
        let Entity::Project(project_real) = &real else {
            unreachable!()
        };
        let lying = |change: &dyn Fn(&mut Project)| {
            let mut project = project_real.clone();
            change(&mut project);
            Entity::Project(project)
        };
        let forged = [
            lying(&|p| p.repo = Some("https://evil.example/team/app.git".into())),
            lying(&|p| p.name = "forged".into()),
            lying(&|p| p.repo = None),
        ];
        for forged in forged {
            let mut merge = connected_merge(&["victim", "hostile"]);
            let (victim, hostile) = (spec("victim").id, spec("hostile").id);
            merge.link_event(hostile, upsert([forged]));
            merge.link_event(victim, upsert([real.clone()]));
            assert_shows(&merge, std::slice::from_ref(&real));
        }

        // Another form of the same remote fits, as a device with an SSH remote
        // reports it.
        let ssh = repo_project("git@example.com:team/app.git");
        assert_eq!(ssh.key(), real.key());
        let mut merge = connected_merge(&["one", "two"]);
        merge.link_event(spec("one").id, upsert([ssh.clone()]));
        merge.link_event(spec("two").id, upsert([real.clone()]));
        assert_shows(&merge, &[ssh]);

        // With nobody to outrank it, it shows without what it can't speak to.
        let mut merge = connected_merge(&["hostile"]);
        let steering = lying(&|p| {
            p.repo = Some("https://evil.example/other.git".into());
            p.default_vendor = Some(Vendor::Claude);
            p.default_account = Some(AccountId::derive("hostile"));
            p.default_workspace = Some(WorkspaceId::derive("hostile"));
        });
        merge.link_event(spec("hostile").id, upsert([steering]));
        let Some(Entity::Project(shows)) = shown(&merge, real.key()) else {
            panic!("the project still shows");
        };
        assert_eq!(shows.repo, None);
        assert_eq!(shows.default_vendor, None);
        assert_eq!(shows.default_account, None);
        assert_eq!(shows.default_workspace, None);

        // A checkout with no usable remote has nothing to check, and keeps its
        // own name.
        let folder = project("scratch");
        let mut merge = connected_merge(&["one"]);
        merge.link_event(spec("one").id, upsert([folder.clone()]));
        assert_shows(&merge, &[folder]);
    }
}
