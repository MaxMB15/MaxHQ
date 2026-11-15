// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The state every observer writes to and every client connection reads.
//!
//! Observers report what they see, and the hub keeps the rules that span
//! them: it creates the project and workspace for each folder an agent runs
//! in, it turns a finished turn you haven't looked at into
//! [`Activity::DoneUnread`], and it shows each agent's inbox, which the
//! session manager keeps. Every change goes out to subscribers as one
//! numbered delta.
//!
//! The session manager writes the agents it holds through
//! [`Hub::apply_held`]. From then on the hub drops what observers report
//! about those agents, since an observer that finds a held CLI's session
//! files would otherwise report it again as one you started yourself. It
//! drops what observers report about agents you deleted for good, for the
//! same reason.
//!
//! A session you started in a terminal stays once its CLI ends there, as an
//! ended agent MaxHQ can continue, until it runs again, MaxHQ holds it, or
//! you settle or delete it. See [`Hub::replace_sessions`].

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use maxhq_core::git::find_checkout;
use maxhq_core::names;
use maxhq_core::store::Store;
use maxhq_protocol::ids::{AccountId, AgentId, InstanceId, ProjectId, WorkspaceId};
use maxhq_protocol::model::{
    Account, Activity, Agent, AgentLink, CloneProgress, Control, Device, DeviceSettings, Inbox,
    ModelChoice, PermissionMode, Project, Usage, Workspace, WorkspaceLocation, DEFAULT_HISTORY_MIB,
    DEFAULT_IDLE_RELEASE_MINUTES, DEFAULT_MEMORY_ITEMS,
};
use maxhq_protocol::pulls::{PullKey, PullLink};
use maxhq_protocol::state::{Change, Delta, Entity, EntityKey, Seq, Snapshot};
use maxhq_protocol::time::Timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::db::Db;

/// How many deltas the hub keeps for clients that reconnect.
pub const KEEP_DELTAS: usize = 4096;

/// What a client asked `maxd` to do, for the task that does it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Check usage now, for one account or all of them.
    RefreshUsage(Option<AccountId>),
}

/// A lead agent's activity changed, as its observer or the session manager
/// reported it.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    /// The agent.
    pub agent: AgentId,
    /// What it was doing, or none the first time this run sees it.
    pub from: Option<Activity>,
    /// What it does now.
    pub to: Activity,
    /// Its last activity, as reported with the change.
    pub at: Timestamp,
}

/// An observer's promise to say once it has reported everything it
/// watches. Until every observer has, an agent missing from the hub may
/// only not be seen yet. One dropped before it's kept never counts.
pub struct FirstPass(Option<Hub>);

impl FirstPass {
    /// Says the observer has reported everything it watches. Only the
    /// first call counts.
    pub fn done(&mut self) {
        if let Some(hub) = self.0.take()
            && let Some(left) = &mut hub.lock().first_passes
        {
            *left = left.saturating_sub(1);
        }
    }
}

/// A cheap handle to the shared state. Clone it into every task.
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    device: Device,
    instance: InstanceId,
    state: Mutex<State>,
    deltas: broadcast::Sender<Delta>,
    commands: broadcast::Sender<Command>,
}

struct State {
    store: Store,
    /// When you last looked at each agent.
    seen: HashMap<AgentId, Timestamp>,
    /// Each agent's activity as its observer reported it, before the
    /// done-unread rule.
    reported: HashMap<AgentId, Activity>,
    /// The workspace for each folder already located.
    folders: HashMap<String, WorkspaceId>,
    /// Agents the session manager holds.
    held: HashSet<AgentId>,
    /// Agents you deleted, which observers never bring back.
    deleted: HashSet<AgentId>,
    /// Sessions you started in a terminal that ended there, kept outside
    /// every observer's scope.
    ended: HashSet<AgentId>,
    /// Where those are kept across restarts, once `maxd` says. None in
    /// tests that don't need it.
    ended_db: Option<Db>,
    /// Agents whose CLI stops once its turn in your terminal ends, so
    /// MaxHQ takes them over, as the session manager keeps them.
    taking_over: HashSet<AgentId>,
    /// What CLIs reported about each account, and the models stored from
    /// before a restart. The usage checks report accounts without it, so
    /// the hub puts it back.
    from_clis: HashMap<AccountId, FromCli>,
    /// Where each account's models are kept across restarts, once `maxd`
    /// says. None in tests that don't need it.
    models_db: Option<Db>,
    /// Each agent's inbox, as the session manager keeps it. An agent
    /// without one has the default.
    inboxes: HashMap<AgentId, Inbox>,
    /// Each agent's PR links, as the session manager keeps them.
    pulls: HashMap<AgentId, Vec<PullLink>>,
    /// The names you gave agents, as the session manager keeps them.
    names: HashMap<AgentId, Named>,
    /// Where each lead agent's change of activity goes, once the session
    /// manager watches.
    moves: Option<mpsc::UnboundedSender<Moved>>,
    /// How many observers have yet to report everything they watch. None
    /// when no observer runs, and then no agent is known to be gone.
    first_passes: Option<usize>,
}

/// A terminal session that ended, as kept across restarts.
#[derive(Serialize, Deserialize)]
struct Ended {
    agent: Agent,
    /// The folder it ran in, whose workspace shows again with it.
    folder: Option<String>,
}

/// A name you gave an agent, which stands over the title its CLI or
/// observer reports.
struct Named {
    name: String,
    /// The title last reported under the name, which shows again once you
    /// clear it. None until the agent is reported.
    reported: Option<String>,
}

/// What a CLI reported about its account.
#[derive(Default)]
struct FromCli {
    models: Vec<ModelChoice>,
    usage: Option<Usage>,
}

/// A client's starting point: the state it lacks, then every later delta.
pub struct Subscription {
    /// Either the deltas after the client's `since`, or a snapshot.
    pub start: Start,
    /// Every delta after `start`.
    pub deltas: broadcast::Receiver<Delta>,
}

/// What a subscriber gets first.
pub enum Start {
    /// The deltas the client is missing, oldest first. May be empty.
    Deltas(Vec<Delta>),
    /// The whole state.
    Snapshot(Snapshot),
}

impl Hub {
    /// A hub holding only this device. `seen` restores when you last looked
    /// at each agent, from the previous run.
    pub fn new(device: Device, seen: HashMap<AgentId, Timestamp>) -> Self {
        let (deltas, _) = broadcast::channel(1024);
        let (commands, _) = broadcast::channel(64);
        let mut store = Store::new(KEEP_DELTAS);
        // The defaults, until `maxd` applies the settings it stored.
        let settings = default_settings(&device);
        store.apply([
            Change::Upsert {
                value: Entity::Device(device.clone()),
            },
            Change::Upsert {
                value: Entity::DeviceSettings(settings),
            },
        ]);
        Self {
            inner: Arc::new(Inner {
                device,
                instance: InstanceId::generate(),
                state: Mutex::new(State {
                    store,
                    seen,
                    reported: HashMap::new(),
                    folders: HashMap::new(),
                    held: HashSet::new(),
                    deleted: HashSet::new(),
                    ended: HashSet::new(),
                    ended_db: None,
                    taking_over: HashSet::new(),
                    from_clis: HashMap::new(),
                    models_db: None,
                    inboxes: HashMap::new(),
                    pulls: HashMap::new(),
                    names: HashMap::new(),
                    moves: None,
                    first_passes: None,
                }),
                deltas,
                commands,
            }),
        }
    }

    /// The device this `maxd` runs on.
    pub fn device(&self) -> &Device {
        &self.inner.device
    }

    /// This run of `maxd`.
    pub fn instance(&self) -> InstanceId {
        self.inner.instance
    }

    /// The current version.
    #[cfg(test)]
    pub fn seq(&self) -> Seq {
        self.lock().store.seq()
    }

    /// The whole state now.
    pub fn snapshot(&self) -> Snapshot {
        self.lock().store.snapshot()
    }

    /// The entity with this key, as it is now.
    pub fn get(&self, key: &EntityKey) -> Option<Entity> {
        self.lock().store.get(key).cloned()
    }

    /// Starts a subscriber from `since`. The start and the receiver are
    /// taken together under the lock, so no delta falls between them.
    pub fn subscribe(&self, since: Option<Seq>) -> Subscription {
        let state = self.lock();
        let deltas = self.inner.deltas.subscribe();
        let start = match since.and_then(|seq| state.store.since(seq)) {
            Some(missing) => Start::Deltas(missing),
            None => Start::Snapshot(state.store.snapshot()),
        };
        Subscription { start, deltas }
    }

    /// Applies an observer's changes in order. Changes to agents the
    /// session manager holds or you deleted, and to their attention events,
    /// are dropped.
    pub fn apply(&self, changes: impl IntoIterator<Item = Change>) {
        let mut state = self.lock();
        let mut kept = Vec::new();
        for change in changes {
            if !state.touches_held(&change) {
                kept.push(state.rewrite(change));
            }
        }
        let changes = kept;
        let delta = state.store.apply(changes);
        self.publish(delta);
    }

    /// Applies the session manager's changes in order. An agent upserted
    /// here is held from then on, until it's removed here.
    pub fn apply_held(&self, changes: impl IntoIterator<Item = Change>) {
        let mut state = self.lock();
        let changes: Vec<Change> = changes
            .into_iter()
            .map(|change| {
                match &change {
                    Change::Upsert {
                        value: Entity::Agent(agent),
                    } => {
                        state.held.insert(agent.id);
                        state.forget_ended(agent.id);
                    }
                    Change::Remove {
                        key: EntityKey::Agent(agent),
                    } => {
                        state.held.remove(agent);
                    }
                    _ => {}
                }
                state.rewrite(change)
            })
            .collect();
        let delta = state.store.apply(changes);
        self.publish(delta);
    }

    /// Makes the entities `scope` selects exactly `entities`, removing the
    /// rest. See [`Store::replace`]. Held agents and their attention events
    /// are outside every scope, and reports of them, and of agents you
    /// deleted, are dropped.
    pub fn replace(
        &self,
        scope: impl Fn(&Entity) -> bool,
        entities: impl IntoIterator<Item = Entity>,
    ) {
        self.replace_keeping(scope, entities, false);
    }

    /// [`Hub::replace`] for an observer of the sessions you start in a
    /// terminal. A lead session in scope that this pass doesn't report
    /// ended there, so it stays as an ended agent, without its sub-agents
    /// and attention events, until it runs again, MaxHQ holds it, or you
    /// settle or delete it. A session whose process now runs another one
    /// was only replaced, as by `/clear`, and goes.
    pub fn replace_sessions(
        &self,
        scope: impl Fn(&Entity) -> bool,
        entities: impl IntoIterator<Item = Entity>,
    ) {
        self.replace_keeping(scope, entities, true);
    }

    fn replace_keeping(
        &self,
        scope: impl Fn(&Entity) -> bool,
        entities: impl IntoIterator<Item = Entity>,
        keep_ended: bool,
    ) {
        let mut state = self.lock();
        let mut kept = Vec::new();
        for entity in entities {
            if state.drops(&entity) {
                continue;
            }
            if let Entity::Agent(agent) = &entity {
                state.forget_ended(agent.id);
            }
            kept.push(state.rewrite_entity(entity));
        }
        if keep_ended {
            for agent in state.newly_ended(&scope, &kept) {
                state.keep_ended(&agent);
                kept.push(state.rewrite_entity(Entity::Agent(agent)));
            }
        }
        let entities = kept;
        let State {
            store, held, ended, ..
        } = &mut *state;
        let delta = store.replace(
            |entity| scope(entity) && !is_held(held, entity) && !is_ended(ended, entity),
            entities,
        );
        self.publish(delta);
    }

    /// Shows the terminal sessions that ended in earlier runs, and keeps
    /// every later one in `db`. Call it after [`Hub::remember_deleted`] and
    /// before any observer reports.
    pub fn remember_ended(&self, db: Db) {
        let stored: Vec<Ended> = db.ended_outside().unwrap_or_else(|err| {
            tracing::warn!(%err, "couldn't read the ended terminal sessions");
            Vec::new()
        });
        // Its folder shows again first, as an observer would show it.
        for ended in &stored {
            if let Some(folder) = &ended.folder {
                self.locate_folder(Path::new(folder));
            }
        }
        let mut state = self.lock();
        state.ended_db = Some(db);
        let mut changes = Vec::new();
        for Ended { agent, .. } in stored {
            let settled = state
                .inboxes
                .get(&agent.id)
                .is_some_and(|inbox| inbox.settled.is_some());
            let entity = Entity::Agent(agent);
            if settled || state.drops(&entity) {
                state.forget_ended_row(entity.key());
                continue;
            }
            if let Entity::Agent(agent) = &entity {
                state.ended.insert(agent.id);
            }
            changes.push(Change::Upsert {
                value: state.rewrite_entity(entity),
            });
        }
        let delta = state.store.apply(changes);
        self.publish(delta);
    }

    /// Marks an agent as waiting for its turn in your terminal to end, so
    /// MaxHQ takes it over, or clears the mark.
    pub fn set_taking_over(&self, agent: AgentId, on: bool) {
        let mut state = self.lock();
        let changed = if on {
            state.taking_over.insert(agent)
        } else {
            state.taking_over.remove(&agent)
        };
        let current = state.store.get(&EntityKey::Agent(agent)).cloned();
        if changed && let Some(Entity::Agent(mut current)) = current {
            current.taking_over = on;
            let delta = state.store.apply([Change::Upsert {
                value: Entity::Agent(current),
            }]);
            self.publish(delta);
        }
    }

    /// Records the models an account's CLI reported, and shows them on the
    /// account once the account is known. A list that differs from the
    /// kept one is stored too.
    pub fn set_models(&self, account: AccountId, models: Vec<ModelChoice>) {
        let mut state = self.lock();
        let kept = &mut state.from_clis.entry(account).or_default().models;
        if *kept == models {
            return;
        }
        if let Some(db) = &state.models_db
            && let Err(err) = db.put_models(account, &models)
        {
            tracing::warn!(%account, %err, "couldn't store the account's models");
        }
        state.from_clis.entry(account).or_default().models = models;
        self.refresh_account(&mut state, account);
    }

    /// Puts back the models each account's CLI listed before `maxd`
    /// restarted, and stores every later list in `db`. Call it before any
    /// account shows.
    pub fn remember_models(&self, db: Db) {
        let stored = db.models().unwrap_or_else(|err| {
            tracing::warn!(%err, "couldn't read the stored models");
            HashMap::new()
        });
        let mut state = self.lock();
        for (account, models) in stored {
            state.from_clis.entry(account).or_default().models = models;
            self.refresh_account(&mut state, account);
        }
        state.models_db = Some(db);
    }

    /// Hides the agents you deleted in earlier runs. Call it before any
    /// observer reports.
    pub fn remember_deleted(&self, db: &Db) {
        let deleted = db.deleted().unwrap_or_else(|err| {
            tracing::warn!(%err, "couldn't read the deleted agents");
            HashSet::new()
        });
        self.hide_deleted(deleted);
    }

    /// Removes agents you deleted, with their sub-agents and attention
    /// events, and drops what observers report of them from now on. An
    /// agent whose CLI still runs or whose session files stay would
    /// otherwise come back as one you started yourself.
    pub fn hide_deleted(&self, agents: impl IntoIterator<Item = AgentId>) {
        let mut state = self.lock();
        for agent in agents {
            state.forget_ended(agent);
            state.seen.remove(&agent);
            state.reported.remove(&agent);
            state.inboxes.remove(&agent);
            state.pulls.remove(&agent);
            state.names.remove(&agent);
            state.deleted.insert(agent);
        }
        let shown: Vec<Change> = state
            .store
            .entities()
            .filter(|entity| is_deleted(&state.deleted, entity))
            .map(|entity| Change::Remove { key: entity.key() })
            .collect();
        let delta = state.store.apply(shown);
        self.publish(delta);
    }

    /// Whether you deleted the agent.
    pub fn is_deleted(&self, agent: AgentId) -> bool {
        self.lock().deleted.contains(&agent)
    }

    /// Forgets an account's models, once the account itself is gone.
    pub fn forget_models(&self, account: AccountId) {
        let mut state = self.lock();
        if let Some(reported) = state.from_clis.get_mut(&account) {
            reported.models.clear();
        }
        if let Some(db) = &state.models_db
            && let Err(err) = db.remove_models(account)
        {
            tracing::warn!(%account, %err, "couldn't forget the account's models");
        }
    }

    /// Records the rate limits an account's CLI reported, and shows them on
    /// the account once the account is known.
    pub fn set_usage(&self, account: AccountId, usage: Usage) {
        let mut state = self.lock();
        state.from_clis.entry(account).or_default().usage = Some(usage);
        self.refresh_account(&mut state, account);
    }

    /// When an account's CLI last reported its rate limits.
    pub fn usage_reported_at(&self, account: AccountId) -> Option<Timestamp> {
        let state = self.lock();
        let usage = state.from_clis.get(&account)?.usage.as_ref()?;
        Some(usage.checked_at)
    }

    /// The workspace for a folder on this device, creating it and its
    /// project the first time. The project comes from the checkout that
    /// holds the folder, or is the folder itself outside any checkout. A
    /// folder MaxHQ is still cloning keeps showing its clone.
    pub fn locate_folder(&self, folder: &Path) -> WorkspaceId {
        self.locate(folder, false)
    }

    /// Shows a folder whose clone finished as the checkout it now is.
    pub fn cloned(&self, folder: &Path) -> WorkspaceId {
        self.locate(folder, true)
    }

    fn locate(&self, folder: &Path, cloned: bool) -> WorkspaceId {
        let key = folder.to_string_lossy().into_owned();
        let device = self.inner.device.id;
        let location = WorkspaceLocation::Folder { path: key.clone() };
        let workspace = names::workspace(device, &location);
        {
            let state = self.lock();
            if let Some(id) = state.folders.get(&key) {
                return *id;
            }
            if !cloned && is_cloning(state.store.get(&EntityKey::Workspace(workspace))) {
                return workspace;
            }
        }
        // Read the checkout outside the lock. Two observers racing here
        // derive the same IDs, so the second upsert changes nothing.
        let checkout = find_checkout(folder);
        let root = checkout
            .as_ref()
            .map_or(key.clone(), |c| c.root.to_string_lossy().into_owned());
        let remote = checkout.as_ref().and_then(|c| c.remote.as_deref());
        let project = names::project(device, &root, remote);
        self.apply([
            Change::Upsert {
                value: Entity::Project(Project {
                    id: project.id,
                    name: project.name,
                    repo: checkout.and_then(|c| c.remote),
                    default_vendor: None,
                    default_account: None,
                    default_workspace: None,
                }),
            },
            Change::Upsert {
                value: Entity::Workspace(Workspace {
                    id: workspace,
                    project: Some(project.id),
                    device,
                    location,
                    managed: None,
                    clone: None,
                }),
            },
        ]);
        self.lock().folders.insert(key, workspace);
        workspace
    }

    /// The workspace for a folder of MaxHQ's own, such as the
    /// dispatcher's. It's in no project, and a folder lookup finds it
    /// without making one.
    pub fn own_folder(&self, folder: &Path) -> WorkspaceId {
        let key = folder.to_string_lossy().into_owned();
        let device = self.inner.device.id;
        let location = WorkspaceLocation::Folder { path: key.clone() };
        let workspace = names::workspace(device, &location);
        self.apply([Change::Upsert {
            value: Entity::Workspace(Workspace {
                id: workspace,
                project: None,
                device,
                location,
                managed: None,
                clone: None,
            }),
        }]);
        self.lock().folders.insert(key, workspace);
        workspace
    }

    /// Shows a folder MaxHQ is cloning a project into, and how the clone
    /// goes.
    pub fn show_clone(&self, folder: &Path, project: ProjectId, clone: CloneProgress) {
        let device = self.inner.device.id;
        let key = folder.to_string_lossy().into_owned();
        // A folder cloned again shows as a checkout again once it's done.
        self.lock().folders.remove(&key);
        let location = WorkspaceLocation::Folder { path: key };
        self.apply([Change::Upsert {
            value: Entity::Workspace(Workspace {
                id: names::workspace(device, &location),
                project: Some(project),
                device,
                location,
                managed: None,
                clone: Some(clone),
            }),
        }]);
    }

    /// Stops showing a folder whose clone was removed. A folder that isn't
    /// being cloned stays.
    pub fn forget_clone(&self, folder: &Path) {
        let location = WorkspaceLocation::Folder {
            path: folder.to_string_lossy().into_owned(),
        };
        let key = EntityKey::Workspace(names::workspace(self.inner.device.id, &location));
        let mut state = self.lock();
        if is_cloning(state.store.get(&key)) {
            state.folders.remove(folder.to_string_lossy().as_ref());
            let delta = state.store.apply([Change::Remove { key }]);
            self.publish(delta);
        }
    }

    /// Whether `folder`, or a folder it's in, is still being cloned.
    pub fn in_clone(&self, folder: &Path) -> bool {
        let device = self.inner.device.id;
        let state = self.lock();
        folder.ancestors().any(|dir| {
            let location = WorkspaceLocation::Folder {
                path: dir.to_string_lossy().into_owned(),
            };
            let key = EntityKey::Workspace(names::workspace(device, &location));
            is_cloning(state.store.get(&key))
        })
    }

    /// Records that you looked at an agent now, which clears its
    /// done-unread state. Returns the time recorded, for storing.
    pub fn mark_seen(&self, agent: AgentId) -> Timestamp {
        let now = Timestamp::now();
        let mut state = self.lock();
        state.seen.insert(agent, now);
        let current = state.store.get(&EntityKey::Agent(agent)).cloned();
        if let Some(Entity::Agent(mut current)) = current {
            if let Some(reported) = state.reported.get(&agent) {
                current.activity = reported.clone();
            }
            let delta = state.store.apply([Change::Upsert {
                value: Entity::Agent(current),
            }]);
            self.publish(delta);
        }
        now
    }

    /// The agent's inbox. An agent without one has the default.
    pub fn inbox(&self, agent: AgentId) -> Inbox {
        self.lock().inboxes.get(&agent).cloned().unwrap_or_default()
    }

    /// Puts back the inboxes the previous run stored, and shows them on the
    /// agents already known.
    pub fn restore_inboxes(&self, inboxes: HashMap<AgentId, Inbox>) {
        for (agent, inbox) in inboxes {
            self.set_inbox(agent, inbox);
        }
    }

    /// Changes an agent's inbox, and upserts the agent so it shows. A
    /// terminal session that ended goes once you settle it.
    pub fn set_inbox(&self, agent: AgentId, inbox: Inbox) {
        let mut state = self.lock();
        if inbox == Inbox::default() {
            state.inboxes.remove(&agent);
        } else {
            state.inboxes.insert(agent, inbox.clone());
        }
        if inbox.settled.is_some() && state.forget_ended(agent) {
            let gone = HashSet::from([agent]);
            let shown: Vec<Change> = state
                .store
                .entities()
                .filter(|entity| is_deleted(&gone, entity))
                .map(|entity| Change::Remove { key: entity.key() })
                .collect();
            let delta = state.store.apply(shown);
            self.publish(delta);
            return;
        }
        let current = state.store.get(&EntityKey::Agent(agent)).cloned();
        if let Some(Entity::Agent(mut current)) = current
            && current.inbox != inbox
        {
            current.inbox = inbox;
            let delta = state.store.apply([Change::Upsert {
                value: Entity::Agent(current),
            }]);
            self.publish(delta);
        }
    }

    /// Forgets an agent's inbox, once the agent itself is gone.
    pub fn forget_inbox(&self, agent: AgentId) {
        self.lock().inboxes.remove(&agent);
    }

    /// The agent's PR links.
    pub fn pulls(&self, agent: AgentId) -> Vec<PullLink> {
        self.lock().pulls.get(&agent).cloned().unwrap_or_default()
    }

    /// The agents that link PR `key`, including those that unlinked it.
    pub fn linking(&self, key: &PullKey) -> Vec<AgentId> {
        let state = self.lock();
        let links = state.pulls.iter();
        links
            .filter(|(_, links)| links.iter().any(|link| link.key == *key))
            .map(|(agent, _)| *agent)
            .collect()
    }

    /// Changes an agent's PR links, and upserts the agent so they show.
    pub fn set_pulls(&self, agent: AgentId, pulls: Vec<PullLink>) {
        let mut state = self.lock();
        if pulls.is_empty() {
            state.pulls.remove(&agent);
        } else {
            state.pulls.insert(agent, pulls.clone());
        }
        let current = state.store.get(&EntityKey::Agent(agent)).cloned();
        if let Some(Entity::Agent(mut current)) = current
            && current.pulls != pulls
        {
            current.pulls = pulls;
            let delta = state.store.apply([Change::Upsert {
                value: Entity::Agent(current),
            }]);
            self.publish(delta);
        }
    }

    /// Shows `name` over the agent's reported title from now on, or with
    /// none, the reported title again.
    pub fn set_name(&self, agent: AgentId, name: Option<String>) {
        let mut state = self.lock();
        let State { store, names, .. } = &mut *state;
        let current = match store.get(&EntityKey::Agent(agent)) {
            Some(Entity::Agent(current)) => Some(current.clone()),
            _ => None,
        };
        let reported = names
            .remove(&agent)
            .and_then(|named| named.reported)
            .or_else(|| current.as_ref().map(|current| current.title.clone()));
        let title = name.clone().or_else(|| reported.clone());
        if let Some(name) = name {
            names.insert(agent, Named { name, reported });
        }
        if let (Some(mut current), Some(title)) = (current, title)
            && current.title != title
        {
            current.title = title;
            let delta = store.apply([Change::Upsert {
                value: Entity::Agent(current),
            }]);
            self.publish(delta);
        }
    }

    /// The agents with an inbox other than the default.
    pub fn filed(&self) -> Vec<AgentId> {
        self.lock().inboxes.keys().copied().collect()
    }

    /// Counts an observer that will say when it has first reported
    /// everything it watches. Take every observer's before starting any.
    pub fn first_pass(&self) -> FirstPass {
        *self.lock().first_passes.get_or_insert(0) += 1;
        FirstPass(Some(self.clone()))
    }

    /// Whether every observer has reported everything it watches at least
    /// once, so an agent the hub doesn't know isn't there. Never true when
    /// no observer runs.
    pub fn observers_reported(&self) -> bool {
        self.lock().first_passes == Some(0)
    }

    /// Each lead agent's change of activity from now on. A second watcher
    /// replaces the first.
    pub fn watch_moves(&self) -> mpsc::UnboundedReceiver<Moved> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.lock().moves = Some(tx);
        rx
    }

    /// The agent with its activity as last reported, before the
    /// done-unread rule.
    pub fn reported(&self, agent: AgentId) -> Option<Agent> {
        let state = self.lock();
        match state.store.get(&EntityKey::Agent(agent)) {
            Some(Entity::Agent(agent)) => Some(state.as_reported(agent)),
            _ => None,
        }
    }

    /// What `pick` takes from each entity.
    pub fn pick<T>(&self, pick: impl FnMut(&Entity) -> Option<T>) -> Vec<T> {
        self.lock().store.entities().filter_map(pick).collect()
    }

    /// Every lead agent, as [`Hub::reported`] gives it.
    pub fn leads(&self) -> Vec<Agent> {
        let state = self.lock();
        state
            .store
            .entities()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) if agent.parent.is_none() => Some(state.as_reported(agent)),
                _ => None,
            })
            .collect()
    }

    /// The sub-agents of a lead.
    pub fn subagents(&self, lead: AgentId) -> Vec<Agent> {
        self.lock()
            .store
            .entities()
            .filter_map(|entity| match entity {
                Entity::Agent(agent) if agent.parent == Some(lead) => Some(agent.clone()),
                _ => None,
            })
            .collect()
    }

    /// Asks the tasks listening on [`Hub::commands`] to do something.
    pub fn send(&self, command: Command) {
        // No receivers just means no task handles it yet.
        let _ = self.inner.commands.send(command);
    }

    /// Commands from clients.
    pub fn commands(&self) -> broadcast::Receiver<Command> {
        self.inner.commands.subscribe()
    }

    /// Upserts an account again, so what its CLIs reported shows.
    fn refresh_account(&self, state: &mut State, account: AccountId) {
        let current = state.store.get(&EntityKey::Account(account)).cloned();
        if let Some(Entity::Account(current)) = current {
            let change = state.rewrite(Change::Upsert {
                value: Entity::Account(current),
            });
            let delta = state.store.apply([change]);
            self.publish(delta);
        }
    }

    fn publish(&self, delta: Option<Delta>) {
        if let Some(delta) = delta {
            // No receivers just means no client is connected.
            let _ = self.inner.deltas.send(delta);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves the store consistent, since
        // every change is applied whole, so carry on with it.
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The settings a device starts with: Auto on a Mac and Full access
/// everywhere else.
pub fn default_settings(device: &Device) -> DeviceSettings {
    DeviceSettings {
        device: device.id,
        default_permission: if device.os == "macos" {
            PermissionMode::Auto
        } else {
            PermissionMode::FullAccess
        },
        workspace_network: None,
        workspace_pool: None,
        auto_settle_days: 5,
        snooze_at_limit: false,
        continue_claude_at_limit: false,
        continue_codex_at_limit: false,
        claude_effort: None,
        codex_effort: None,
        placement: Default::default(),
        tool_updates: Vec::new(),
        cursor_usage: false,
        keep_awake: Default::default(),
        test_hosts: Default::default(),
        skill_library: None,
        network: Default::default(),
        idle_release_minutes: DEFAULT_IDLE_RELEASE_MINUTES,
        memory_items: DEFAULT_MEMORY_ITEMS,
        history_mib: DEFAULT_HISTORY_MIB,
        claude_setup_token: false,
    }
}

/// Whether the entity is a workspace still being cloned.
fn is_cloning(entity: Option<&Entity>) -> bool {
    matches!(entity, Some(Entity::Workspace(workspace)) if workspace.clone.is_some())
}

/// Whether the entity is a held agent or one of its attention events.
fn is_held(held: &HashSet<AgentId>, entity: &Entity) -> bool {
    match entity {
        Entity::Agent(agent) => held.contains(&agent.id),
        Entity::Attention(event) => held.contains(&event.agent),
        _ => false,
    }
}

/// Whether the entity is a terminal session kept since it ended.
fn is_ended(ended: &HashSet<AgentId>, entity: &Entity) -> bool {
    matches!(entity, Entity::Agent(agent) if ended.contains(&agent.id))
}

/// The agent a terminal session shows once its CLI ended there: down, with
/// no process, and idle unless it stopped with an error.
fn as_ended(mut agent: Agent, now: Timestamp) -> Agent {
    agent.pid = None;
    agent.link = AgentLink::Down {
        reason: "exited".into(),
        since: Some(now),
    };
    if !matches!(agent.activity, Activity::Error { .. }) {
        agent.activity = Activity::Idle;
    }
    agent.taking_over = false;
    agent
}

/// Whether the entity is an agent you deleted, a sub-agent of one, or an
/// attention event of one.
fn is_deleted(deleted: &HashSet<AgentId>, entity: &Entity) -> bool {
    match entity {
        Entity::Agent(agent) => {
            deleted.contains(&agent.id) || agent.parent.is_some_and(|lead| deleted.contains(&lead))
        }
        Entity::Attention(event) => deleted.contains(&event.agent),
        _ => false,
    }
}

impl State {
    /// The lead terminal sessions in `scope` that ended since the last
    /// pass, as reported: shown and running, but missing from `reported`
    /// while nothing reported runs in their process.
    fn newly_ended(&self, scope: &impl Fn(&Entity) -> bool, reported: &[Entity]) -> Vec<Agent> {
        let mut ids = HashSet::new();
        let mut pids = HashSet::new();
        for entity in reported {
            if let Entity::Agent(agent) = entity {
                ids.insert(agent.id);
                pids.extend(agent.pid);
            }
        }
        let now = Timestamp::now();
        self.store
            .entities()
            .filter(|entity| scope(entity) && !self.drops(entity))
            .filter_map(|entity| match entity {
                Entity::Agent(agent)
                    if agent.parent.is_none()
                        && agent.control == Control::Observed
                        && agent.link == AgentLink::Live
                        && !ids.contains(&agent.id)
                        && !agent.pid.is_some_and(|pid| pids.contains(&pid)) =>
                {
                    let mut agent = self.as_reported(agent);
                    if let Some(Named {
                        reported: Some(title),
                        ..
                    }) = self.names.get(&agent.id)
                    {
                        agent.title.clone_from(title);
                    }
                    Some(as_ended(agent, now))
                }
                _ => None,
            })
            .collect()
    }

    /// Keeps a terminal session that ended, here and across restarts.
    fn keep_ended(&mut self, agent: &Agent) {
        self.ended.insert(agent.id);
        let folder = match self.store.get(&EntityKey::Workspace(agent.workspace)) {
            Some(Entity::Workspace(Workspace {
                location: WorkspaceLocation::Folder { path },
                ..
            })) => Some(path.clone()),
            _ => None,
        };
        let record = Ended {
            agent: agent.clone(),
            folder,
        };
        if let Some(db) = &self.ended_db
            && let Err(err) = db.put_ended_outside(agent.id, &record)
        {
            tracing::warn!(agent = %agent.id, %err, "couldn't keep the ended terminal session");
        }
    }

    /// Stops keeping an agent as a terminal session that ended. Returns
    /// whether it was one.
    fn forget_ended(&mut self, agent: AgentId) -> bool {
        let was = self.ended.remove(&agent);
        if was {
            self.forget_ended_row(EntityKey::Agent(agent));
        }
        was
    }

    fn forget_ended_row(&self, key: EntityKey) {
        if let EntityKey::Agent(agent) = key
            && let Some(db) = &self.ended_db
            && let Err(err) = db.remove_ended_outside(agent)
        {
            tracing::warn!(%agent, %err, "couldn't forget the ended terminal session");
        }
    }

    /// An observer's entity with the rules of [`State::rewrite`] applied.
    fn rewrite_entity(&mut self, entity: Entity) -> Entity {
        match self.rewrite(Change::Upsert { value: entity }) {
            Change::Upsert { value } => value,
            Change::Remove { .. } => unreachable!("rewrite keeps the kind of change"),
        }
    }

    /// Whether observers' reports of the entity are dropped: it's a held
    /// agent or one you deleted, or an attention event of one.
    fn drops(&self, entity: &Entity) -> bool {
        is_held(&self.held, entity) || is_deleted(&self.deleted, entity)
    }

    /// Whether an observer's change touches a held or deleted agent or one
    /// of its attention events.
    fn touches_held(&self, change: &Change) -> bool {
        match change {
            Change::Upsert { value } => self.drops(value),
            Change::Remove { key } => self.store.get(key).is_some_and(|entity| self.drops(entity)),
        }
    }

    /// Applies the rules that span observers to an upsert: the done-unread
    /// rule, the inbox, the PR links and the name you gave for agents, and
    /// what held CLIs reported for accounts.
    fn rewrite(&mut self, change: Change) -> Change {
        match change {
            Change::Upsert {
                value: Entity::Agent(agent),
            } => {
                self.tell_move(&agent);
                let mut agent = self.done_unread(agent);
                agent.inbox = self.inboxes.get(&agent.id).cloned().unwrap_or_default();
                agent.taking_over = self.taking_over.contains(&agent.id);
                agent.pulls = self.pulls.get(&agent.id).cloned().unwrap_or_default();
                if let Some(named) = self.names.get_mut(&agent.id) {
                    named.reported = Some(std::mem::replace(&mut agent.title, named.name.clone()));
                }
                Change::Upsert {
                    value: Entity::Agent(agent),
                }
            }
            Change::Upsert {
                value: Entity::Account(account),
            } => Change::Upsert {
                value: Entity::Account(self.with_cli_reports(account)),
            },
            change => change,
        }
    }

    /// The agent with its activity as last reported.
    fn as_reported(&self, agent: &Agent) -> Agent {
        let mut agent = agent.clone();
        if let Some(reported) = self.reported.get(&agent.id) {
            agent.activity = reported.clone();
        }
        agent
    }

    /// Tells the session manager when a lead agent's reported activity
    /// changes, before [`State::done_unread`] records the new one.
    fn tell_move(&mut self, agent: &Agent) {
        let Some(moves) = &self.moves else {
            return;
        };
        let from = self.reported.get(&agent.id);
        if agent.parent.is_some() || from == Some(&agent.activity) {
            return;
        }
        let moved = Moved {
            agent: agent.id,
            from: from.cloned(),
            to: agent.activity.clone(),
            at: agent.last_activity_at,
        };
        if moves.send(moved).is_err() {
            self.moves = None;
        }
    }

    /// A lead agent that reports idle after a turn that ended since you last
    /// looked shows as done and unread. The first time `maxd` sees an agent
    /// counts as looking, so agents that were idle before `maxd` started
    /// aren't all marked unread.
    fn done_unread(&mut self, mut agent: Agent) -> Agent {
        self.reported.insert(agent.id, agent.activity.clone());
        let seen = *self.seen.entry(agent.id).or_insert(agent.last_activity_at);
        let idle = matches!(
            agent.activity,
            Activity::Idle | Activity::IdleCacheWarm { .. }
        );
        if agent.parent.is_none() && idle && agent.last_activity_at > seen {
            agent.activity = Activity::DoneUnread;
        }
        agent
    }

    /// An account reported without models keeps the ones its CLI reported,
    /// and keeps its CLI's usage while that's the newer reading.
    fn with_cli_reports(&self, mut account: Account) -> Account {
        let Some(reported) = self.from_clis.get(&account.id) else {
            return account;
        };
        if account.models.is_empty() {
            account.models.clone_from(&reported.models);
        }
        if let Some(usage) = &reported.usage {
            let older = account
                .usage
                .as_ref()
                .is_none_or(|current| current.checked_at < usage.checked_at);
            if older {
                account.usage = Some(usage.clone());
                account.usage_failure = None;
            }
        }
        account
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::attention::{AttentionEvent, AttentionKind};
    use maxhq_protocol::ids::{AttentionId, BootId, DeviceId};
    use maxhq_protocol::model::{AgentLink, Control, Vendor};

    fn hub() -> Hub {
        Hub::new(
            Device {
                id: DeviceId::derive("test"),
                name: "mac".into(),
                os: "macos".into(),
                boot_id: BootId("b".into()),
                maxd_version: "0.1.0".into(),
                winding_down_until: None,
                workspace_host: None,
                build: None,
                target: None,
            },
            HashMap::new(),
        )
    }

    fn agent(activity: Activity, at: u64) -> Entity {
        Entity::Agent(Agent {
            id: AgentId::derive("a"),
            workspace: WorkspaceId::derive("w"),
            parent: None,
            vendor: Vendor::Claude,
            account: None,
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some("s".into()),
            control: Control::Observed,
            pid: Some(42),
            link: AgentLink::Live,
            title: "maxhq".into(),
            activity,
            context: None,
            started_at: Timestamp(1),
            last_activity_at: Timestamp(at),
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

    fn activity(hub: &Hub) -> Activity {
        let key = EntityKey::Agent(AgentId::derive("a"));
        match hub.get(&key) {
            Some(Entity::Agent(agent)) => agent.activity.clone(),
            other => panic!("no agent: {other:?}"),
        }
    }

    #[test]
    fn a_turn_that_ends_after_you_looked_is_unread_until_you_look_again() {
        let hub = hub();
        let only_agents = |e: &Entity| matches!(e, Entity::Agent(_));
        hub.replace(only_agents, [agent(Activity::Idle, 10)]);
        assert_eq!(activity(&hub), Activity::Idle, "first sight counts as seen");

        hub.replace(only_agents, [agent(Activity::Busy, 20)]);
        hub.replace(only_agents, [agent(Activity::Idle, 30)]);
        assert_eq!(activity(&hub), Activity::DoneUnread);

        hub.mark_seen(AgentId::derive("a"));
        assert_eq!(activity(&hub), Activity::Idle);
    }

    #[test]
    fn subscribers_start_from_what_they_have() {
        let hub = hub();
        let start = hub.seq();
        hub.apply([Change::Upsert {
            value: agent(Activity::Busy, 5),
        }]);
        let Subscription { start: first, .. } = hub.subscribe(Some(start));
        assert!(matches!(first, Start::Deltas(d) if d.len() == 1));
        let Subscription { start: fresh, .. } = hub.subscribe(None);
        assert!(matches!(fresh, Start::Snapshot(s) if s.entities.len() == 3));
    }

    #[test]
    fn new_agents_default_to_auto_on_a_mac() {
        let key = EntityKey::DeviceSettings(DeviceId::derive("test"));
        match hub().get(&key) {
            Some(Entity::DeviceSettings(settings)) => {
                assert_eq!(settings.default_permission, PermissionMode::Auto);
            }
            other => panic!("no settings: {other:?}"),
        }
    }

    #[test]
    fn folders_become_workspaces_with_a_project() {
        let hub = hub();
        let dir = std::env::temp_dir();
        let first = hub.locate_folder(&dir);
        assert_eq!(hub.locate_folder(&dir), first);
        let snapshot = hub.snapshot();
        assert!(snapshot
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Project(_))));
        assert!(snapshot
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Workspace(w) if w.id == first)));
    }

    #[test]
    fn an_own_folder_is_in_no_project() {
        let hub = hub();
        let dir = std::env::temp_dir().join("dispatcher");
        let own = hub.own_folder(&dir);
        assert_eq!(hub.locate_folder(&dir), own);
        let snapshot = hub.snapshot();
        assert!(!snapshot
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Project(_))));
        assert!(snapshot
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Workspace(w) if w.id == own && w.project.is_none())));
    }

    #[test]
    fn observers_cant_overwrite_or_remove_held_agents() {
        let hub = hub();
        let observed = |e: &Entity| matches!(e, Entity::Agent(_) | Entity::Attention(_));
        let Entity::Agent(mut held) = agent(Activity::Busy, 5) else {
            unreachable!()
        };
        held.control = Control::Held;
        hub.apply_held([Change::Upsert {
            value: Entity::Agent(held),
        }]);
        let question = AttentionEvent {
            id: AttentionId::derive("q"),
            agent: AgentId::derive("a"),
            kind: AttentionKind::Question,
            summary: "which?".into(),
            options: vec![],
            accepts_text: true,
            created_at: Timestamp(5),
            expires_at: None,
        };
        hub.replace(
            observed,
            [agent(Activity::Idle, 9), Entity::Attention(question)],
        );
        assert_eq!(activity(&hub), Activity::Busy);
        hub.replace(observed, []);
        assert_eq!(activity(&hub), Activity::Busy);
        assert!(hub
            .get(&EntityKey::Attention(AttentionId::derive("q")))
            .is_none());

        hub.apply_held([Change::Remove {
            key: EntityKey::Agent(AgentId::derive("a")),
        }]);
        hub.replace(observed, [agent(Activity::Idle, 9)]);
        let observed_again = hub.get(&EntityKey::Agent(AgentId::derive("a")));
        assert!(
            matches!(observed_again, Some(Entity::Agent(a)) if a.control == Control::Observed),
            "released agents are observed again"
        );
    }

    #[test]
    fn observers_never_bring_back_a_deleted_agent() {
        let db = Db::open_in_memory().unwrap();
        let shown = hub();
        let observed = |e: &Entity| matches!(e, Entity::Agent(_) | Entity::Attention(_));
        let lead = AgentId::derive("a");
        let Entity::Agent(mut sub) = agent(Activity::Busy, 5) else {
            unreachable!()
        };
        sub.id = AgentId::derive("sub");
        sub.parent = Some(lead);
        let question = AttentionEvent {
            id: AttentionId::derive("q"),
            agent: lead,
            kind: AttentionKind::Question,
            summary: "which?".into(),
            options: vec![],
            accepts_text: true,
            created_at: Timestamp(5),
            expires_at: None,
        };
        let reports = || {
            [
                agent(Activity::Idle, 9),
                Entity::Agent(sub.clone()),
                Entity::Attention(question.clone()),
            ]
        };
        shown.replace(observed, reports());
        assert!(shown.get(&EntityKey::Agent(lead)).is_some());

        db.mark_deleted(&[lead], Timestamp(10)).unwrap();
        shown.hide_deleted([lead]);
        assert!(shown.is_deleted(lead));
        let gone = |hub: &Hub| hub.snapshot().entities.iter().all(|e| !observed(e));
        assert!(gone(&shown), "what was shown goes");
        shown.replace(observed, reports());
        shown.apply(reports().map(|value| Change::Upsert { value }));
        assert!(gone(&shown), "new reports are dropped");

        let restarted = hub();
        restarted.remember_deleted(&db);
        restarted.replace(observed, reports());
        assert!(gone(&restarted), "and stay dropped after a restart");
    }

    fn models(hub: &Hub, account: AccountId) -> Vec<ModelChoice> {
        match hub.get(&EntityKey::Account(account)) {
            Some(Entity::Account(account)) => account.models,
            other => panic!("no account: {other:?}"),
        }
    }

    #[test]
    fn an_accounts_models_come_back_after_a_restart_until_its_forgotten() {
        let dir = crate::testing::TempDir::new();
        let path = dir.path().join(crate::db::FILE);
        let id = AccountId::derive("codex");
        let account = Entity::Account(Account {
            id,
            device: DeviceId::derive("test"),
            vendor: Vendor::Codex,
            label: "Codex".into(),
            config_dir: "/home/dev/.codex".into(),
            email: None,
            organization: None,
            plan: None,
            models: Vec::new(),
            usage: None,
            usage_failure: None,
            acp: None,
            login: Default::default(),
            origin: None,
        });
        let listed = vec![ModelChoice {
            id: "gpt-6.1-sol".into(),
            label: "GPT-6.1 Sol".into(),
            default: true,
            efforts: Vec::new(),
            default_effort: None,
        }];

        let first = hub();
        first.remember_models(Db::open(&path).unwrap());
        first.set_models(id, listed.clone());
        drop(first);

        let second = hub();
        second.remember_models(Db::open(&path).unwrap());
        second.apply([Change::Upsert {
            value: account.clone(),
        }]);
        assert_eq!(models(&second, id), listed, "a usage check shows them");

        let seq = second.seq();
        second.set_models(id, listed.clone());
        assert_eq!(second.seq(), seq, "the same list changes nothing");

        second.forget_models(id);
        let third = hub();
        third.remember_models(Db::open(&path).unwrap());
        third.apply([Change::Upsert { value: account }]);
        assert!(models(&third, id).is_empty());
    }

    fn inbox(hub: &Hub) -> Inbox {
        match hub.get(&EntityKey::Agent(AgentId::derive("a"))) {
            Some(Entity::Agent(agent)) => agent.inbox,
            other => panic!("no agent: {other:?}"),
        }
    }

    #[test]
    fn every_upsert_shows_the_kept_inbox() {
        let hub = hub();
        let settled = Inbox {
            settled: Some(maxhq_protocol::model::Settled {
                at: Timestamp(3),
                by: maxhq_protocol::model::Settler::You,
            }),
            ..Inbox::default()
        };
        hub.set_inbox(AgentId::derive("a"), settled.clone());
        hub.apply([Change::Upsert {
            value: agent(Activity::Idle, 5),
        }]);
        assert_eq!(inbox(&hub), settled, "an agent seen later shows it");
        assert_eq!(hub.filed(), [AgentId::derive("a")]);

        let seq = hub.seq();
        hub.set_inbox(AgentId::derive("a"), Inbox::default());
        assert_eq!(inbox(&hub), Inbox::default());
        assert!(hub.seq() > seq, "the change goes out as a delta");
        assert!(hub.filed().is_empty(), "the default isn't kept");

        hub.restore_inboxes(HashMap::from([(AgentId::derive("a"), settled.clone())]));
        hub.apply([Change::Upsert {
            value: agent(Activity::Busy, 6),
        }]);
        assert_eq!(
            inbox(&hub),
            settled,
            "an observer's report doesn't clear it"
        );
    }

    #[test]
    fn every_upsert_shows_the_kept_pr_links() {
        let hub = hub();
        let link = PullLink {
            key: maxhq_core::pulls::key("owner/repo", 12),
            source: maxhq_protocol::pulls::LinkSource::Created,
            at: Timestamp(3),
            snapshot: None,
            watch: None,
        };
        let pulls = |hub: &Hub| match hub.get(&EntityKey::Agent(AgentId::derive("a"))) {
            Some(Entity::Agent(agent)) => agent.pulls,
            other => panic!("no agent: {other:?}"),
        };
        hub.set_pulls(AgentId::derive("a"), vec![link.clone()]);
        hub.apply([Change::Upsert {
            value: agent(Activity::Idle, 5),
        }]);
        assert_eq!(
            pulls(&hub),
            [link],
            "an observer's report doesn't clear them"
        );

        let seq = hub.seq();
        hub.set_pulls(AgentId::derive("a"), Vec::new());
        assert!(pulls(&hub).is_empty());
        assert!(hub.seq() > seq, "the change goes out as a delta");
    }

    #[test]
    fn a_name_stands_over_every_reported_title_until_you_clear_it() {
        let hub = hub();
        let id = AgentId::derive("a");
        let title = |hub: &Hub| match hub.get(&EntityKey::Agent(id)) {
            Some(Entity::Agent(agent)) => agent.title,
            other => panic!("no agent: {other:?}"),
        };
        let reported = |title: &str| match agent(Activity::Idle, 5) {
            Entity::Agent(agent) => Change::Upsert {
                value: Entity::Agent(Agent {
                    title: title.into(),
                    ..agent
                }),
            },
            _ => unreachable!(),
        };
        hub.set_name(id, Some("Kept".into()));
        hub.apply([reported("first")]);
        assert_eq!(title(&hub), "Kept", "an agent seen later shows it");
        hub.set_name(id, None);
        assert_eq!(title(&hub), "first");

        let seq = hub.seq();
        hub.set_name(id, Some("Mine".into()));
        assert_eq!(title(&hub), "Mine");
        assert!(hub.seq() > seq, "the change goes out as a delta");
        hub.apply_held([reported("second")]);
        assert_eq!(title(&hub), "Mine", "a later title doesn't replace it");
        hub.set_name(id, Some("Mine again".into()));
        hub.set_name(id, None);
        assert_eq!(title(&hub), "second", "the last reported title comes back");

        hub.set_name(id, Some("Mine".into()));
        hub.hide_deleted([id]);
        assert!(hub.lock().names.is_empty(), "deleting forgets the name");
    }

    #[test]
    fn the_observers_have_reported_once_each_says_so() {
        let both = hub();
        assert!(!both.observers_reported(), "not while none runs");
        let mut claude = both.first_pass();
        let mut codex = both.first_pass();
        claude.done();
        claude.done();
        assert!(!both.observers_reported(), "a second call doesn't count");
        codex.done();
        assert!(both.observers_reported());

        let one = hub();
        let mut claude = one.first_pass();
        let crashed = one.first_pass();
        drop(crashed);
        claude.done();
        assert!(
            !one.observers_reported(),
            "an observer that stopped short never counts"
        );
    }

    #[test]
    fn the_watcher_hears_each_change_of_a_lead_agents_activity() {
        let hub = hub();
        let mut moves = hub.watch_moves();
        hub.apply([Change::Upsert {
            value: agent(Activity::Busy, 5),
        }]);
        hub.apply([Change::Upsert {
            value: agent(Activity::Busy, 6),
        }]);
        hub.apply([Change::Upsert {
            value: agent(Activity::Idle, 7),
        }]);
        let Entity::Agent(mut sub) = agent(Activity::Busy, 8) else {
            unreachable!()
        };
        sub.id = AgentId::derive("sub");
        sub.parent = Some(AgentId::derive("a"));
        hub.apply([Change::Upsert {
            value: Entity::Agent(sub),
        }]);

        let first = moves.try_recv().unwrap();
        assert_eq!((first.from, first.to), (None, Activity::Busy));
        let second = moves.try_recv().unwrap();
        assert_eq!(
            (second.from, second.to, second.at),
            (Some(Activity::Busy), Activity::Idle, Timestamp(7))
        );
        assert!(
            moves.try_recv().is_err(),
            "sub-agents and repeats aren't told"
        );
        let leads = hub.leads();
        assert_eq!(leads.len(), 1);
        assert_eq!(
            hub.subagents(AgentId::derive("a"))[0].id,
            AgentId::derive("sub")
        );
        assert_eq!(
            hub.reported(AgentId::derive("a"))
                .map(|agent| agent.activity),
            Some(Activity::Idle)
        );
    }

    fn shown(hub: &Hub) -> Vec<Agent> {
        hub.pick(|entity| match entity {
            Entity::Agent(agent) => Some(agent.clone()),
            _ => None,
        })
    }

    fn sessions(hub: &Hub, entities: Vec<Entity>) {
        hub.replace_sessions(|entity| matches!(entity, Entity::Agent(_)), entities);
    }

    #[test]
    fn an_ended_session_stays_across_restarts_until_maxhq_holds_it() {
        let db = Db::open_in_memory().unwrap();
        let first = hub();
        first.remember_ended(db.clone());
        sessions(&first, vec![agent(Activity::Busy, 2)]);
        sessions(&first, Vec::new());
        let ended = shown(&first);
        assert_eq!(ended.len(), 1);
        assert!(matches!(ended[0].link, AgentLink::Down { .. }));
        assert_eq!((ended[0].pid, &ended[0].activity), (None, &Activity::Idle));

        // After a restart it shows again before any observer reports.
        let second = hub();
        second.remember_ended(db.clone());
        assert_eq!(shown(&second), ended);

        // Held by MaxHQ, it shows once, and no longer as ended.
        let Entity::Agent(mut held) = agent(Activity::Idle, 3) else {
            unreachable!()
        };
        held.control = Control::Held;
        second.apply_held([Change::Upsert {
            value: Entity::Agent(held),
        }]);
        sessions(&second, Vec::new());
        let now = shown(&second);
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].control, Control::Held);
        let third = hub();
        third.remember_ended(db);
        assert!(shown(&third).is_empty());
    }

    #[test]
    fn a_session_replaced_in_its_process_goes() {
        let hub = hub();
        sessions(&hub, vec![agent(Activity::Idle, 2)]);
        let Entity::Agent(mut cleared) = agent(Activity::Idle, 3) else {
            unreachable!()
        };
        cleared.id = AgentId::derive("b");
        sessions(&hub, vec![Entity::Agent(cleared)]);
        let now = shown(&hub);
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].id, AgentId::derive("b"));
    }
}
