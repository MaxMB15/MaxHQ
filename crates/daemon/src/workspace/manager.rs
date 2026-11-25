// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The workspace manager: what it keeps for each container workspace, how
//! it saves and shows each change, and A-10's cache rebuilt from the
//! instance keys when `maxd` starts.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use maxhq_protocol::guest::GuestUnsaved;
use maxhq_protocol::ids::{AgentId, CredentialId, DeviceId, ProjectId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    Activity, AllowedWorkflowEnv, Credential, CredentialKind, DeviceSettings, GuestState, Project,
    ProjectSettings, WorkspaceHost, WorkspaceLocation, WorkspaceState,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tokio::sync::Notify;
use tracing::{info, warn};

use super::egress::HostFacts;
use super::exec::IncusExec;
use super::keys::{key, Keys, Record};
use super::manifest::{self, Manifest};
use super::repo::Repo;
use super::{Github, GuestLinks};
use crate::db::Db;
use crate::guestlink;
use crate::hub::Hub;
use crate::incus::{Incus, Instance, InstanceName, PREFIX};
use crate::recipe::{Step, RECIPE_VERSION};
use crate::session::clock::BoxFuture;
use crate::session::holders::{AgentSummary, Container, Workspaces};

/// What a manager works with.
pub struct Config {
    /// The Incus daemon on this device.
    pub incus: Incus,
    /// Where workspaces show.
    pub hub: Hub,
    /// Where their records live.
    pub db: Db,
    /// `maxd`'s runtime folder, which holds each proxy socket.
    pub runtime: PathBuf,
    /// `maxd`'s data folder, which holds the folder manifests.
    pub data: PathBuf,
    /// Mints GitHub tokens for the clone and the push preflight.
    pub github: Arc<dyn Github>,
    /// The links to each guest.
    pub links: Arc<dyn GuestLinks>,
    /// The guest binary. None looks for it where
    /// [`guest_binary`](crate::guestlink::guest_binary) does.
    pub guest_binary: Option<PathBuf>,
    /// The recipe version a finished container has.
    pub recipe: u32,
    /// The recipe steps a container that finished a version still needs.
    pub steps: fn(u32) -> Vec<Step>,
    /// How long things wait.
    pub timing: Timing,
    /// Reads what the host knows about its own addresses and ports.
    pub host_facts: fn() -> HostFacts,
    /// The host spec's caps on each workspace.
    pub caps: super::caps::Caps,
}

impl Config {
    /// The usual config on a device, with no guest links yet.
    pub fn new(incus: Incus, hub: Hub, db: Db, runtime: PathBuf, data: PathBuf) -> Self {
        Self {
            incus,
            hub,
            db,
            runtime,
            data,
            github: Arc::new(super::Broker),
            links: Arc::new(super::NoLinks),
            guest_binary: None,
            recipe: RECIPE_VERSION,
            steps: crate::recipe::steps_after,
            timing: Timing::default(),
            host_facts: HostFacts::read,
            caps: super::caps::Caps::default(),
        }
    }
}

/// How long the manager waits for things.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// A-11's pause between tries at boot.
    pub retry: Duration,
    /// How often it rereads the instances, for events it missed.
    pub resync: Duration,
    /// How many times it checks that a container is up.
    pub ready_tries: u32,
    /// The pause between those checks.
    pub ready_pause: Duration,
    /// How long it waits for a guest to link.
    pub guest_wait: Duration,
    /// How long a start or a stop may take.
    pub state_timeout: Duration,
    /// How long a stop's answer waits for the container to stop, under the
    /// client's wait for a reply. A longer stop answers then and goes on
    /// showing Stopping.
    pub stop_answer: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            retry: Duration::from_secs(10),
            resync: Duration::from_secs(30),
            ready_tries: 60,
            ready_pause: Duration::from_secs(2),
            guest_wait: Duration::from_secs(60),
            state_timeout: Duration::from_secs(60),
            stop_answer: Duration::from_secs(10),
        }
    }
}

/// Runs this device's container workspaces. Clones share one manager.
#[derive(Clone)]
pub struct Manager {
    pub(super) inner: Arc<Inner>,
}

pub(super) struct Inner {
    pub(super) config: Config,
    pub(super) device: DeviceId,
    /// This host boot, for A-11.
    pub(super) host_boot: String,
    state: Mutex<Shared>,
    /// Wakes what waits for a change to a workspace to finish.
    freed: Notify,
    capable: AtomicBool,
}

#[derive(Default)]
pub(super) struct Shared {
    pub(super) records: BTreeMap<WorkspaceId, Record>,
    /// Workspaces a create, start, stop or delete is changing now.
    pub(super) ops: BTreeSet<WorkspaceId>,
    pub(super) settings: BTreeMap<ProjectId, ProjectSettings>,
}

/// Marks a workspace as being changed until it drops.
pub(super) struct Op {
    manager: Manager,
    workspace: WorkspaceId,
}

impl Drop for Op {
    fn drop(&mut self) {
        self.manager.shared().ops.remove(&self.workspace);
        self.manager.inner.freed.notify_waiters();
    }
}

impl Manager {
    /// A manager with the records and project settings this device saved,
    /// shown at once. [`Manager::run`] checks them against Incus.
    pub fn new(config: Config) -> Self {
        let device = config.hub.device().id;
        let host_boot = config.hub.device().boot_id.0.clone();
        let records: Vec<Record> = config.db.workspaces().unwrap_or_else(|error| {
            warn!(%error, "couldn't read the saved workspaces");
            Vec::new()
        });
        let settings = config.db.project_settings().unwrap_or_else(|error| {
            warn!(%error, "couldn't read the saved project settings");
            Vec::new()
        });
        let mut shared = Shared::default();
        let mut shown = Vec::new();
        for record in records {
            if record.keys.device != device {
                continue;
            }
            shown.push(Change::Upsert {
                value: Entity::Workspace(record.entity()),
            });
            shared.records.insert(record.keys.workspace, record);
        }
        for settings in settings {
            if settings.device != device {
                continue;
            }
            shown.push(Change::Upsert {
                value: Entity::ProjectSettings(settings.clone()),
            });
            shared.settings.insert(settings.project, settings);
        }
        config.hub.apply(shown);
        Self {
            inner: Arc::new(Inner {
                config,
                device,
                host_boot,
                state: Mutex::new(shared),
                freed: Notify::new(),
                capable: AtomicBool::new(false),
            }),
        }
    }

    /// Whether Incus answered, so clients may offer container workspaces.
    pub fn capable(&self) -> bool {
        self.inner.capable.load(Ordering::Relaxed)
    }

    /// The Incus client, which [`GuestInstall`](super::GuestInstall)
    /// installs guest binaries through.
    pub fn incus(&self) -> &Incus {
        &self.inner.config.incus
    }

    /// Commands in a workspace's container, for worktrees and the like.
    /// None for a workspace this device doesn't manage.
    pub fn exec(&self, workspace: WorkspaceId) -> Option<IncusExec> {
        let record = self.record(workspace)?;
        let name = InstanceName::new(&record.instance).ok()?;
        Some(IncusExec::new(self.incus().clone(), name))
    }

    /// Records what the guest link says about a workspace's guest.
    pub fn set_guest(&self, workspace: WorkspaceId, guest: GuestState) {
        self.update(workspace, |record| record.guest = guest);
    }

    /// Keeps the SSH host key a workspace's guest reported, so editors can
    /// check it.
    pub fn set_ssh_host_key(&self, workspace: WorkspaceId, key: String) {
        self.update(workspace, |record| record.ssh_host_key = Some(key));
    }

    /// The memory limit a workspace in `project` gets: the project's
    /// setting on this device, else the host spec's.
    pub(super) fn memory_soft(&self, project: ProjectId) -> Option<u64> {
        let project = self.project_settings(project).memory_soft;
        self.config().caps.memory(project)
    }

    /// Keeps the latest unsaved work a workspace's guest reported, summed
    /// over its checkouts. A stopped workspace keeps its last one, which
    /// the read time shows is old.
    pub fn set_unsaved(&self, workspace: WorkspaceId, worktrees: &[GuestUnsaved]) {
        let summary = super::unsaved::summary(worktrees, Timestamp::now());
        self.update(workspace, |record| record.unsaved = Some(summary));
    }

    /// Why an editor can't open SSH into a workspace now. None once it's
    /// a container that runs.
    pub fn ssh_blocked(&self, workspace: WorkspaceId) -> Option<String> {
        let Some(record) = self.record(workspace) else {
            return Some("that workspace isn't a container on this device".to_owned());
        };
        match record.state {
            WorkspaceState::Running => None,
            _ => Some(format!("{} isn't running", record.instance)),
        }
    }

    /// One project's settings on this device, or the defaults: resume on
    /// restart, no extra ignored ports and no git author.
    pub fn project_settings(&self, project: ProjectId) -> ProjectSettings {
        self.shared()
            .settings
            .get(&project)
            .cloned()
            .unwrap_or_else(|| self.default_settings(project))
    }

    fn default_settings(&self, project: ProjectId) -> ProjectSettings {
        ProjectSettings {
            project,
            device: self.inner.device,
            resume_on_restart: true,
            ignored_ports: Vec::new(),
            git_author: None,
            auto_settle_days: None,
            settle_on_merge: true,
            workflow_env: true,
            memory_soft: None,
            workflow_env_allowed: None,
        }
    }

    /// Answers `SetProjectSettings`: saves and shows them, and gives the
    /// project's running containers the git author. The workflow variables
    /// you allowed stay as `maxd` kept them, and turning the variables off
    /// forgets them.
    pub async fn set_project_settings(&self, mut settings: ProjectSettings) -> ActionOutcome {
        if settings.device != self.inner.device {
            return refused("These settings belong to another device.");
        }
        {
            // Held so an Allow kept at the same time isn't lost.
            let mut shared = self.shared();
            settings.workflow_env_allowed = shared
                .settings
                .get(&settings.project)
                .filter(|_| settings.workflow_env)
                .and_then(|stored| stored.workflow_env_allowed.clone());
            if let Err(error) = self.inner.config.db.put_project_settings(&settings) {
                return refused(format!("maxd couldn't save the settings: {error}"));
            }
            shared.settings.insert(settings.project, settings.clone());
            self.inner.config.hub.apply([Change::Upsert {
                value: Entity::ProjectSettings(settings.clone()),
            }]);
        }
        let running: Vec<Record> = self
            .shared()
            .records
            .values()
            .filter(|r| r.keys.project == settings.project && r.state == WorkspaceState::Running)
            .cloned()
            .collect();
        if !running.is_empty() {
            let manager = self.clone();
            tokio::spawn(async move {
                for record in running {
                    if let Err(error) = manager.apply_author(&record).await {
                        warn!(instance = %record.instance, %error, "couldn't set the git author");
                    }
                }
            });
        }
        ActionOutcome::Done
    }

    /// Checks the saved records against Incus, watches its events, starts
    /// what A-11 says to, and rereads the instances now and then. Returns
    /// at once on a device with no Incus and no workspaces.
    pub async fn run(self) {
        let socket = self.incus().socket().to_owned();
        if !socket.exists() && self.shared().records.is_empty() {
            info!(socket = %socket.display(), "no Incus here, so no container workspaces");
            return;
        }
        let timing = self.inner.config.timing;
        let mut said = false;
        while let Err(error) = self.incus().ping().await {
            if !said {
                info!(%error, "waiting for Incus to answer");
                said = true;
            }
            tokio::time::sleep(timing.retry).await;
        }
        let running = self.rebuild().await;
        self.publish_host().await;
        self.inner.capable.store(true, Ordering::Relaxed);
        tokio::spawn(self.clone().watch_events());
        tokio::spawn(self.clone().boot(running));
        loop {
            tokio::time::sleep(timing.resync).await;
            self.resync().await;
        }
    }

    pub(super) fn shared(&self) -> MutexGuard<'_, Shared> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn config(&self) -> &Config {
        &self.inner.config
    }

    pub(super) fn record(&self, workspace: WorkspaceId) -> Option<Record> {
        self.shared().records.get(&workspace).cloned()
    }

    /// The workspace an instance belongs to.
    pub(super) fn find_instance(&self, instance: &str) -> Option<WorkspaceId> {
        self.shared()
            .records
            .values()
            .find(|record| record.instance == instance)
            .map(|record| record.keys.workspace)
    }

    /// Marks a workspace as being changed, or None when it already is.
    pub(super) fn begin(&self, workspace: WorkspaceId) -> Option<Op> {
        self.shared().ops.insert(workspace).then(|| Op {
            manager: self.clone(),
            workspace,
        })
    }

    /// Waits for the change to a workspace that runs now to finish, then
    /// marks it as being changed.
    pub(super) async fn begin_after(&self, workspace: WorkspaceId) -> Op {
        loop {
            // Made before the try, so a change that ends in between still
            // wakes it.
            let freed = self.inner.freed.notified();
            if let Some(op) = self.begin(workspace) {
                return op;
            }
            freed.await;
        }
    }

    /// Whether something is changing a workspace now.
    pub(super) fn changing(&self, workspace: WorkspaceId) -> bool {
        self.shared().ops.contains(&workspace)
    }

    /// Keeps, saves and shows a record.
    pub(super) fn put(&self, record: Record) {
        let mut shared = self.shared();
        self.save(&record);
        shared.records.insert(record.keys.workspace, record);
    }

    /// Changes a record, then saves and shows it. None when there's none.
    pub(super) fn update(
        &self,
        workspace: WorkspaceId,
        change: impl FnOnce(&mut Record),
    ) -> Option<Record> {
        let mut shared = self.shared();
        let record = shared.records.get_mut(&workspace)?;
        let before = record.clone();
        change(record);
        if *record != before {
            self.save(record);
        }
        Some(record.clone())
    }

    /// Sets where a workspace stands.
    pub(super) fn set_state(&self, workspace: WorkspaceId, state: WorkspaceState) {
        self.update(workspace, |record| record.state = state);
    }

    /// Forgets a workspace whose instance is gone.
    pub(super) fn forget(&self, workspace: WorkspaceId) {
        let mut shared = self.shared();
        if shared.records.remove(&workspace).is_none() {
            return;
        }
        if let Err(error) = self.inner.config.db.remove_workspace(workspace) {
            warn!(%workspace, %error, "couldn't forget a workspace");
        }
        self.inner.config.hub.apply([Change::Remove {
            key: EntityKey::Workspace(workspace),
        }]);
        drop(shared);
        self.inner.config.links.forget(workspace);
        crate::skills::deliver::forget_workspace(workspace);
    }

    /// Writes a record to the database and the hub. Callers hold the lock,
    /// so changes show in the order they happen.
    fn save(&self, record: &Record) {
        let id = record.keys.workspace;
        if let Err(error) = self
            .inner
            .config
            .db
            .put_workspace(id, &record.instance, record)
        {
            warn!(workspace = %id, %error, "couldn't save a workspace");
        }
        self.inner.config.hub.apply([Change::Upsert {
            value: Entity::Workspace(record.entity()),
        }]);
    }

    /// The agents in a workspace that are at work, so an update or a
    /// delete waits.
    pub(super) fn busy_agents(&self, record: &Record) -> Vec<AgentId> {
        let hub = &self.inner.config.hub;
        record
            .keys
            .agents
            .iter()
            .filter(|summary| {
                matches!(
                    hub.get(&EntityKey::Agent(summary.agent)),
                    Some(Entity::Agent(agent)) if matches!(
                        agent.activity,
                        Activity::Starting
                            | Activity::Busy
                            | Activity::BusyInSubagents
                            | Activity::NeedsInput { .. }
                    )
                )
            })
            .map(|summary| summary.agent)
            .collect()
    }

    /// Where an instance's proxy device listens on the host.
    pub(super) fn socket(&self, name: &InstanceName) -> PathBuf {
        guestlink::socket_path(&self.inner.config.runtime, name)
    }

    /// Makes the folder for the proxy sockets, 0700, which the proxy
    /// device needs before it can listen.
    pub(super) fn ensure_guests_dir(&self) -> io::Result<PathBuf> {
        let dir = self.inner.config.runtime.join(guestlink::GUESTS_DIR);
        maxhq_os::fsmode::private_dir_all(&dir)?;
        Ok(dir)
    }

    /// Removes a proxy socket Incus left behind.
    pub(super) fn remove_socket(&self, name: &InstanceName) {
        let socket = self.socket(name);
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => warn!(socket = %socket.display(), %error, "couldn't remove a socket"),
        }
    }

    /// This device's settings as they are now.
    pub(super) fn device_settings(&self) -> DeviceSettings {
        let device = self.inner.device;
        if let Some(Entity::DeviceSettings(settings)) = self
            .inner
            .config
            .hub
            .get(&EntityKey::DeviceSettings(device))
        {
            return settings;
        }
        match self.inner.config.db.device_settings(device) {
            Ok(Some(settings)) => settings,
            _ => crate::hub::default_settings(self.inner.config.hub.device()),
        }
    }

    /// The repos this device's GitHub App reaches, as "owner/name".
    pub(super) fn app_repos(&self) -> Vec<String> {
        // The same ID the secret store gives the App's credential.
        let id = CredentialId::derive(&format!("github-app:{}", self.inner.device));
        match self.inner.config.hub.get(&EntityKey::Credential(id)) {
            Some(Entity::Credential(Credential {
                kind: CredentialKind::GithubApp { repos, .. },
                ..
            })) => repos,
            _ => Vec::new(),
        }
    }

    /// The repo the GitHub App reaches whose project is `project`.
    pub(super) fn app_repo(&self, project: ProjectId) -> Option<Repo> {
        let device = self.inner.device;
        self.app_repos()
            .iter()
            .filter_map(|repo| Repo::parse(repo))
            .find(|repo| repo.project(device) == project)
    }

    /// Shows a workspace's project when nothing else does, so a container
    /// whose repo has no folder on this device still has one.
    pub(super) fn ensure_project(&self, keys: &Keys) {
        let hub = &self.inner.config.hub;
        if hub.get(&EntityKey::Project(keys.project)).is_some() {
            return;
        }
        hub.apply([Change::Upsert {
            value: Entity::Project(Project {
                id: keys.project,
                name: keys.title.clone(),
                repo: Some(keys.repo.clone()),
                default_vendor: None,
                default_account: None,
                default_workspace: None,
            }),
        }]);
    }

    /// Shows what Incus offers for workspaces on this device.
    pub(super) async fn publish_host(&self) {
        let incus = self.incus();
        let (networks, pools) = match (incus.networks().await, incus.pools().await) {
            (Ok(networks), Ok(pools)) => (networks, pools),
            (Err(error), _) | (_, Err(error)) => {
                warn!(%error, "couldn't read what Incus offers");
                return;
            }
        };
        let host = WorkspaceHost {
            networks: networks
                .iter()
                .filter(|network| network.is_managed_bridge())
                .map(|network| network.name.clone())
                .collect(),
            pools: pools.into_iter().map(|pool| pool.name).collect(),
        };
        let hub = &self.inner.config.hub;
        let mut device = match hub.get(&EntityKey::Device(self.inner.device)) {
            Some(Entity::Device(device)) => device,
            _ => hub.device().clone(),
        };
        if device.workspace_host.as_ref() == Some(&host) {
            return;
        }
        device.workspace_host = Some(host);
        hub.apply([Change::Upsert {
            value: Entity::Device(device),
        }]);
    }

    /// Sets or clears the project's git author in a container's repo.
    pub(super) async fn apply_author(&self, record: &Record) -> Result<(), String> {
        let exec = self
            .exec(record.keys.workspace)
            .ok_or_else(|| format!("{} isn't a MaxHQ instance", record.instance))?;
        let author = self.project_settings(record.keys.project).git_author;
        let dir = record.repo_dir();
        let (name, email) = author
            .as_ref()
            .map_or(("", ""), |a| (a.name.as_str(), a.email.as_str()));
        let cmd = ["sh", "-c", AUTHOR_SCRIPT, "sh", &dir, name, email];
        super::exec::agent_ok(&exec, &cmd)
            .await
            .map(drop)
            .map_err(|error| error.to_string())
    }

    /// A-10: makes the records match the instances Incus has, and returns
    /// the running ones, which still need their probe and guest link.
    pub(super) async fn rebuild(&self) -> Vec<WorkspaceId> {
        let instances = match self.incus().instances().await {
            Ok(instances) => instances,
            Err(error) => {
                warn!(%error, "couldn't list the instances, so the saved workspaces stand");
                return Vec::new();
            }
        };
        let mut seen = BTreeSet::new();
        let mut running = Vec::new();
        for instance in &instances {
            let Some(keys) = self.ours(instance) else {
                continue;
            };
            let workspace = keys.workspace;
            seen.insert(workspace);
            if self.changing(workspace) {
                continue;
            }
            let old = self.record(workspace);
            let state = if keys.recipe == 0 {
                WorkspaceState::Failed {
                    reason: "Its create didn't finish. Delete it and make it again.".to_owned(),
                }
            } else if instance.is_running() {
                running.push(workspace);
                match old.as_ref().map(|r| &r.state) {
                    Some(WorkspaceState::Running) => WorkspaceState::Running,
                    _ => WorkspaceState::Starting,
                }
            } else {
                let pool = instance
                    .pool()
                    .or(old.as_ref().and_then(|r| r.pool.as_deref()));
                match pool {
                    Some(pool) if !self.incus().pool_available(pool).await.unwrap_or(true) => {
                        WorkspaceState::StorageLocked
                    }
                    _ => WorkspaceState::Stopped,
                }
            };
            let now = Timestamp::now();
            let record = Record {
                instance: instance.name.clone(),
                state,
                guest: old.as_ref().map_or_else(
                    || GuestState::Down {
                        reason: "not linked yet".to_owned(),
                        since: now,
                    },
                    |r| r.guest.clone(),
                ),
                egress: old
                    .as_ref()
                    .map_or(maxhq_protocol::model::EgressCheck::NotRun, |r| {
                        r.egress.clone()
                    }),
                egress_note: old.as_ref().and_then(|r| r.egress_note.clone()),
                pool: instance
                    .pool()
                    .map(str::to_owned)
                    .or_else(|| old.as_ref().and_then(|r| r.pool.clone())),
                network: instance
                    .network()
                    .map(str::to_owned)
                    .or_else(|| old.as_ref().and_then(|r| r.network.clone())),
                host_boot: old.as_ref().and_then(|r| r.host_boot.clone()),
                ssh_host_key: old.as_ref().and_then(|r| r.ssh_host_key.clone()),
                memory_soft: super::caps::memory_of(instance),
                unsaved: old.as_ref().and_then(|r| r.unsaved),
                keys,
            };
            self.ensure_project(&record.keys);
            self.put(record);
        }
        let gone: Vec<WorkspaceId> = {
            let shared = self.shared();
            shared
                .records
                .iter()
                .filter(|(id, record)| {
                    !seen.contains(id)
                        && !matches!(record.state, WorkspaceState::Failed { .. })
                        && !shared.ops.contains(id)
                })
                .map(|(id, _)| *id)
                .collect()
        };
        for workspace in gone {
            self.forget(workspace);
        }
        running
    }

    /// The keys of a workspace this device's `maxd` made, or None for any
    /// other instance, such as a test host's.
    pub(super) fn ours(&self, instance: &Instance) -> Option<Keys> {
        if !instance.name.starts_with(PREFIX)
            || !instance.is_managed()
            || super::test_host::is_test_host(instance)
        {
            return None;
        }
        InstanceName::new(&instance.name).ok()?;
        match Keys::decode(&instance.config) {
            Ok(keys) if keys.device == self.inner.device => Some(keys),
            Ok(_) => {
                warn!(instance = %instance.name, "skipping an instance another device's maxd made");
                None
            }
            Err(error) => {
                warn!(instance = %instance.name, %error, "skipping an instance whose keys don't read");
                None
            }
        }
    }

    /// A-10 for a workspace: the agents go in `user.maxhq.agents` for a
    /// container, or in the folder's manifest.
    async fn write_agents(
        &self,
        workspace: WorkspaceId,
        agents: Vec<AgentSummary>,
    ) -> Result<(), String> {
        if let Some(record) = self.record(workspace) {
            let name = InstanceName::new(&record.instance).map_err(|error| error.to_string())?;
            let json = serde_json::to_string(&agents).map_err(|error| error.to_string())?;
            let keys = BTreeMap::from([(key::AGENTS.to_owned(), json)]);
            self.incus()
                .set_config(&name, keys)
                .await
                .map_err(|error| {
                    format!("couldn't record {}'s agents: {error}", record.instance)
                })?;
            self.update(workspace, |record| record.keys.agents = agents);
            return Ok(());
        }
        let hub = &self.inner.config.hub;
        let found = match hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(found)) if found.device == self.inner.device => found,
            _ => return Err("That workspace isn't on this device.".to_owned()),
        };
        let WorkspaceLocation::Folder { path } = &found.location else {
            return Err("That container isn't one this maxd manages.".to_owned());
        };
        let mut manifest = Manifest::new(self.inner.device, workspace, path.clone(), agents);
        if let Some(project) = found.project {
            manifest.project = Some(project);
            if let Some(Entity::Project(project)) = hub.get(&EntityKey::Project(project)) {
                manifest.title = Some(project.name);
                manifest.repo = project.repo;
            }
        }
        manifest::write(&self.inner.config.data, &manifest)
            .map_err(|error| format!("couldn't write the workspace's manifest: {error}"))
    }
}

impl Workspaces for Manager {
    fn container(&self, workspace: WorkspaceId) -> Option<Container> {
        let record = self.record(workspace)?;
        Some(Container {
            instance: record.instance.clone(),
            repo: record.repo_dir(),
            blocked: record.blocked(),
        })
    }

    fn record_agents(
        &self,
        workspace: WorkspaceId,
        agents: Vec<AgentSummary>,
    ) -> BoxFuture<Result<(), String>> {
        let manager = self.clone();
        Box::pin(async move { manager.write_agents(workspace, agents).await })
    }

    fn allow_workflow_env(
        &self,
        project: ProjectId,
        allowed: AllowedWorkflowEnv,
    ) -> Result<(), String> {
        let mut shared = self.shared();
        let mut settings = shared
            .settings
            .get(&project)
            .cloned()
            .unwrap_or_else(|| self.default_settings(project));
        if !settings.workflow_env {
            return Err("the project gives agents no workflow variables".to_owned());
        }
        settings.workflow_env_allowed = Some(allowed);
        self.inner
            .config
            .db
            .put_project_settings(&settings)
            .map_err(|error| error.to_string())?;
        shared.settings.insert(project, settings.clone());
        self.inner.config.hub.apply([Change::Upsert {
            value: Entity::ProjectSettings(settings),
        }]);
        Ok(())
    }
}

/// Sets the git author in the repo at `$1` to `$2 <$3>`, or clears it when
/// `$2` is empty. A repo that isn't there yet is left alone.
const AUTHOR_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 0
if [ -n "$2" ]; then
  git config user.name "$2" && git config user.email "$3"
else
  git config --unset-all user.name
  git config --unset-all user.email
  exit 0
fi"#;

/// A refusal for a person to read.
pub(super) fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}
