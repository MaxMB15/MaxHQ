// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The ACP registry on this device: its index, fetched at most every five
//! minutes and cached under maxd's data folder, with the cache standing in
//! when a fetch fails. It shows as one `AcpRegistry` for the device.
//!
//! An entry installs through the installer, as an archive for the place's
//! target or else as its npm or PyPI package, in this device or in one of
//! its container workspaces. Once installed it becomes a local ACP agent
//! definition, which runs like any other. An archive with no checksum
//! installs only after you confirm that version, and the confirm is kept.
//! Sign-in stays with each agent: its methods show as it lists them, and
//! nothing for them is kept here.
//!
//! Requests arrive through the free functions here, which reach the
//! registry [`install`] put in place.

mod index;

pub(crate) use index::bounded_version;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use maxhq_protocol::acp::{AcpAgent, AcpEnvValue, AcpSkills};
use maxhq_protocol::hosts::{AcpRegistry, RegistryAgent, RegistryAuth, RegistryInstall};
use maxhq_protocol::ids::AcpAgentId;
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::{Change, Entity, EntityKey, ToolKind, ToolPlace};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::acp_agents::{self, AcpAgents};
use crate::db::Db;
use crate::hub::Hub;
use crate::tools::install::{self, fetch, ArchiveFile, InstallDef, Installed, Installer, Source};
use index::{Choice, Entry, Index, Kind};

/// How long a fetch stands before the next one.
const MAX_AGE: Duration = Duration::from_secs(5 * 60);
/// The cache's file name in maxd's data folder.
const CACHE: &str = "acp-registry.json";

/// The registry the free functions reach, once `maxd` has opened it.
static STORE: OnceLock<Registry> = OnceLock::new();

/// An entry's installs, as the database keeps them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Record {
    /// The versions you confirmed with no checksum.
    #[serde(default)]
    confirmed: Vec<String>,
    /// Where it's installed.
    #[serde(default)]
    places: Vec<Placed>,
}

/// One install of an entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Placed {
    place: ToolPlace,
    /// The registry's version.
    version: String,
    /// The command, through its link when it has one.
    path: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// The device's ACP registry. Clones share it.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

struct Inner {
    hub: Hub,
    db: Db,
    agents: AcpAgents,
    installer: Arc<dyn Installer>,
    url: String,
    cache: PathBuf,
    max_age: Duration,
    state: Mutex<State>,
    /// Held while a fetch runs, so only one does.
    fetching: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    index: Index,
    fetched_at: Option<Timestamp>,
    fresh_until: Option<Instant>,
    problem: Option<String>,
    records: BTreeMap<String, Record>,
}

impl Registry {
    /// The registry, showing what the cache in `data` holds until the
    /// first fetch.
    pub fn open(
        hub: Hub,
        db: Db,
        agents: AcpAgents,
        installer: Arc<dyn Installer>,
        data: &std::path::Path,
    ) -> Self {
        Self::with(hub, db, agents, installer, index::URL, data, MAX_AGE)
    }

    fn with(
        hub: Hub,
        db: Db,
        agents: AcpAgents,
        installer: Arc<dyn Installer>,
        url: &str,
        data: &std::path::Path,
        max_age: Duration,
    ) -> Self {
        let cache = data.join(CACHE);
        let mut state = State::default();
        if let Ok(text) = std::fs::read_to_string(&cache) {
            match index::parse(&text) {
                Ok(index) => {
                    state.index = index;
                    state.fetched_at = std::fs::metadata(&cache)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .map(Timestamp::from);
                }
                Err(err) => warn!(%err, "skipping the ACP registry's cache"),
            }
        }
        let records = db.acp_installs().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the ACP registry's installs");
            Vec::new()
        });
        for (id, text) in records {
            match serde_json::from_str::<Record>(&text) {
                Ok(record) => {
                    state.records.insert(id, record);
                }
                Err(err) => warn!(%err, "skipping an ACP registry install that doesn't parse"),
            }
        }
        let registry = Self {
            inner: Arc::new(Inner {
                hub,
                db,
                agents,
                installer,
                url: url.to_owned(),
                cache,
                max_age,
                state: Mutex::new(state),
                fetching: tokio::sync::Mutex::new(()),
            }),
        };
        registry.publish();
        registry
    }

    /// Fetches the index now and then, for as long as maxd runs.
    pub async fn watch(self) {
        loop {
            self.refresh().await;
            tokio::time::sleep(self.inner.max_age).await;
        }
    }

    /// Fetches the index unless the last fetch is recent, keeping the one
    /// it had and why when the fetch fails.
    async fn refresh(&self) {
        let _one = self.inner.fetching.lock().await;
        let fresh = self
            .state()
            .fresh_until
            .is_some_and(|until| Instant::now() < until);
        if fresh {
            return;
        }
        let fetched = match fetch::text(&self.inner.url).await {
            Ok(text) => index::parse(&text).map(|index| (text, index)),
            Err(err) => Err(err),
        };
        let fetched = match fetched {
            Ok((text, index)) => {
                if let Err(err) = self.write_cache(text).await {
                    warn!(%err, "couldn't cache the ACP registry");
                }
                Ok(index)
            }
            Err(err) => Err(err),
        };
        {
            let mut state = self.state();
            match fetched {
                Ok(index) => {
                    state.index = index;
                    state.fetched_at = Some(Timestamp::now());
                    state.problem = None;
                }
                Err(err) => {
                    warn!(%err, "couldn't fetch the ACP registry");
                    state.problem = Some(crate::fleet::sentence(&err));
                }
            }
            state.fresh_until = Some(Instant::now() + self.inner.max_age);
        }
        self.publish();
    }

    /// Writes the cache through a file of its own, so a reader never sees
    /// half of one.
    async fn write_cache(&self, text: String) -> std::io::Result<()> {
        let cache = self.inner.cache.clone();
        let staged = cache.with_extension("json.new");
        tokio::fs::write(&staged, text).await?;
        tokio::fs::rename(&staged, &cache).await
    }

    /// Answers `InstallAcpAgent`.
    async fn install(
        &self,
        id: String,
        version: String,
        place: ToolPlace,
        confirm: bool,
    ) -> ActionOutcome {
        match self.start_install(id, Some(version), place, confirm).await {
            Ok(_) => ActionOutcome::Done,
            Err(reason) => refused(reason),
        }
    }

    /// Answers `InstallTool` for an entry: the registry's version, or the
    /// one named, once you've confirmed it when it needs that.
    async fn update(&self, id: String, version: Option<String>, place: ToolPlace) -> ActionOutcome {
        match self.start_install(id, version, place, false).await {
            Ok(_) => ActionOutcome::Done,
            Err(reason) => refused(reason),
        }
    }

    /// Checks an install and starts it on its own task.
    async fn start_install(
        &self,
        id: String,
        version: Option<String>,
        place: ToolPlace,
        confirm: bool,
    ) -> Result<JoinHandle<()>, String> {
        if index::NATIVE.contains(&id.as_str()) {
            return Err(format!("MaxHQ runs {id} as one of its own agents."));
        }
        self.refresh().await;
        let entry = self
            .state()
            .index
            .agents
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or("The ACP registry doesn't list that agent.")?;
        if version
            .as_ref()
            .is_some_and(|version| *version != entry.version)
        {
            return Err(format!(
                "The ACP registry now has {} {}. Install that one.",
                entry.name, entry.version
            ));
        }
        let target = target(place);
        let choice = index::choose(&entry, &target)
            .ok_or_else(|| format!("{} has no build for {target}.", entry.name))?;
        if matches!(choice.kind, Kind::Binary { sha256: None, .. }) {
            let mut state = self.state();
            let record = state.records.entry(id.clone()).or_default();
            if !record.confirmed.contains(&entry.version) {
                if !confirm {
                    return Err(format!(
                        "{} {} has no checksum to check its download against. \
                         Confirm to install it anyway.",
                        entry.name, entry.version
                    ));
                }
                record.confirmed.push(entry.version.clone());
                let record = record.clone();
                drop(state);
                self.store(&id, &record)?;
            }
        }
        let (def, version) = install_def(&entry, &choice, &target);
        let registry = self.clone();
        Ok(tokio::spawn(async move {
            let installed = registry
                .inner
                .installer
                .ensure(&def, Some(&version), &place)
                .await;
            match installed {
                Ok(installed) => registry.installed(&entry, &choice, place, installed).await,
                Err(err) => warn!(agent = %entry.id, %err, "an ACP registry install failed"),
            }
        }))
    }

    /// Keeps a finished install and defines the agent from it.
    async fn installed(&self, entry: &Entry, choice: &Choice, place: ToolPlace, done: Installed) {
        info!(agent = %entry.id, version = %entry.version, "installed an ACP registry agent");
        let placed = Placed {
            place,
            version: entry.version.clone(),
            path: done.path,
            args: choice.args.clone(),
            env: choice.env.clone(),
        };
        let record = {
            let mut state = self.state();
            let record = state.records.entry(entry.id.clone()).or_default();
            record.places.retain(|have| have.place != place);
            record.places.push(placed);
            record.clone()
        };
        if let Err(reason) = self.store(&entry.id, &record) {
            warn!(agent = %entry.id, %reason, "couldn't keep an ACP registry install");
        }
        self.define(&entry.id, &entry.name, &record).await;
        self.publish();
    }

    /// Answers `RemoveTool` for an entry. Removing its last install
    /// deletes its definition, which waits for its agents to stop.
    async fn remove(&self, id: String, place: ToolPlace) -> ActionOutcome {
        match self.start_remove(id, place) {
            Ok(_) => ActionOutcome::Done,
            Err(reason) => refused(reason),
        }
    }

    fn start_remove(&self, id: String, place: ToolPlace) -> Result<JoinHandle<()>, String> {
        let record = self.state().records.get(&id).cloned().unwrap_or_default();
        if !record.places.iter().any(|have| have.place == place) {
            return Err("MaxHQ didn't install that agent there.".to_owned());
        }
        let last = record.places.len() == 1;
        let def_id = self.def_id(&id);
        let defined = matches!(
            self.inner.hub.get(&EntityKey::AcpAgent(def_id)),
            Some(Entity::AcpAgent(_))
        );
        let name = self
            .defined(def_id)
            .map_or_else(|| id.clone(), |agent| agent.name);
        if last
            && defined
            && let ActionOutcome::Refused { reason } = self.inner.agents.delete(def_id)
        {
            return Err(reason);
        }
        let registry = self.clone();
        Ok(tokio::spawn(async move {
            let kind = ToolKind::Acp { id: id.clone() };
            let removed = registry.inner.installer.remove(&kind, &place).await;
            let record = {
                let mut state = registry.state();
                let record = state.records.entry(id.clone()).or_default();
                if removed.is_ok() {
                    record.places.retain(|have| have.place != place);
                }
                record.clone()
            };
            match removed {
                Ok(()) => {
                    info!(agent = %id, "removed an ACP registry agent");
                    if let Err(reason) = registry.store(&id, &record) {
                        warn!(agent = %id, %reason, "couldn't keep an ACP registry removal");
                    }
                }
                Err(err) => warn!(agent = %id, %err, "an ACP registry removal failed"),
            }
            if !record.places.is_empty() {
                registry.define(&id, &name, &record).await;
            }
            registry.publish();
        }))
    }

    /// Defines the local ACP agent an entry's installs run as, keeping
    /// what you changed on it. One installed in a container workspace
    /// runs there through its link, by name.
    async fn define(&self, id: &str, name: &str, record: &Record) {
        let Some(first) = record.places.first() else {
            return;
        };
        let host = record
            .places
            .iter()
            .find(|placed| placed.place == ToolPlace::Host);
        let main = host.unwrap_or(first);
        let in_containers = record
            .places
            .iter()
            .any(|placed| placed.place != ToolPlace::Host);
        // An install with a link has its path there, not in its folder.
        let linked = record
            .places
            .iter()
            .all(|placed| !placed.path.contains("/lib/maxhq/tools/"));
        let containers = in_containers && linked;
        let program = if containers {
            main.path
                .rsplit('/')
                .next()
                .unwrap_or(&main.path)
                .to_owned()
        } else {
            main.path.clone()
        };
        let env: Vec<AcpEnvValue> = main
            .env
            .iter()
            .filter(|(name, _)| acp_agents::variable_name(name))
            .map(|(name, value)| AcpEnvValue {
                name: name.clone(),
                value: Secret::new(value.clone()),
            })
            .collect();
        let def_id = self.def_id(id);
        let mut agent = self.defined(def_id).unwrap_or_else(|| AcpAgent {
            id: def_id,
            device: self.inner.hub.device().id,
            name: name.to_owned(),
            program: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            containers: false,
            problem: None,
            skills: AcpSkills::default(),
        });
        agent.program = program;
        agent.args = main.args.clone();
        agent.containers = containers;
        for value in &env {
            if !agent.env.contains(&value.name) {
                agent.env.push(value.name.clone());
            }
        }
        // An agent with no install on this device runs only in its
        // containers, so it isn't probed here.
        let probe = host.is_some();
        if let ActionOutcome::Refused { reason } = self.inner.agents.store(agent, env, probe).await
        {
            warn!(agent = %id, %reason, "couldn't define an ACP registry agent");
        }
    }

    /// The definition an entry's installs run as.
    fn def_id(&self, id: &str) -> AcpAgentId {
        AcpAgentId::derive(&format!("acp-registry:{}:{id}", self.inner.hub.device().id))
    }

    fn defined(&self, id: AcpAgentId) -> Option<AcpAgent> {
        match self.inner.hub.get(&EntityKey::AcpAgent(id)) {
            Some(Entity::AcpAgent(agent)) => Some(agent),
            _ => None,
        }
    }

    fn store(&self, id: &str, record: &Record) -> Result<(), String> {
        let text = serde_json::to_string(record).map_err(|err| err.to_string())?;
        self.inner
            .db
            .put_acp_install(id, &text)
            .map_err(|err| format!("This device couldn't keep the install: {err}"))
    }

    /// Shows the device's registry, when it changed.
    fn publish(&self) {
        let host = target(ToolPlace::Host);
        let entity = {
            let state = self.state();
            let agents = state
                .index
                .agents
                .iter()
                .map(|entry| {
                    let record = state.records.get(&entry.id);
                    let installed = record.and_then(|record| {
                        let host = record
                            .places
                            .iter()
                            .find(|placed| placed.place == ToolPlace::Host);
                        host.or(record.places.first())
                            .map(|placed| placed.version.clone())
                    });
                    RegistryAgent {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                        version: entry.version.clone(),
                        description: Some(entry.description.clone())
                            .filter(|text| !text.is_empty()),
                        install: index::choose(entry, &host).map(|choice| shown(&choice)),
                        auth: self.auth(&entry.id),
                        installed,
                    }
                })
                .collect();
            Entity::AcpRegistry(AcpRegistry {
                device: self.inner.hub.device().id,
                fetched_at: state.fetched_at,
                problem: state.problem.clone(),
                agents,
            })
        };
        let key = EntityKey::AcpRegistry(self.inner.hub.device().id);
        if self.inner.hub.get(&key).as_ref() != Some(&entity) {
            self.inner.hub.apply([Change::Upsert { value: entity }]);
        }
    }

    /// The sign-in methods an installed entry's agent listed.
    fn auth(&self, id: &str) -> Vec<RegistryAuth> {
        let Some(agent) = self.defined(self.def_id(id)) else {
            return Vec::new();
        };
        let terminal = self.inner.agents.terminal(agent.id);
        let account = acp_agents::account_of(&agent);
        let Some(Entity::Account(account)) = self.inner.hub.get(&EntityKey::Account(account))
        else {
            return Vec::new();
        };
        account
            .acp
            .map(|features| features.auth)
            .unwrap_or_default()
            .into_iter()
            .map(|method| RegistryAuth {
                terminal: terminal.contains(&method.id),
                id: method.id,
                name: method.name,
            })
            .collect()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The target a place installs for. A container workspace shares this
/// device's kernel, so it's Linux on this device's architecture.
fn target(place: ToolPlace) -> String {
    let own = crate::identity::TARGET;
    match place {
        ToolPlace::Host => install::target_for(own),
        ToolPlace::Workspace { .. } => install::target_for(own.split('-').next().unwrap_or(own)),
    }
}

/// What the installer puts in place for `choice`, and at which version.
fn install_def(entry: &Entry, choice: &Choice, target: &str) -> (InstallDef, String) {
    let kind = ToolKind::Acp {
        id: entry.id.clone(),
    };
    match &choice.kind {
        Kind::Binary {
            archive,
            command,
            sha256,
        } => (
            InstallDef {
                kind,
                binary: Some(command.clone()),
                source: Source::Archive {
                    files: vec![ArchiveFile {
                        target: target.to_owned(),
                        url: archive.clone(),
                        sha256: sha256.clone(),
                        size: None,
                    }],
                    confirmed_unchecked: sha256.is_none(),
                },
            },
            entry.version.clone(),
        ),
        Kind::Npx { name, version } => (
            InstallDef {
                kind,
                binary: None,
                source: Source::Npm {
                    package: name.clone(),
                },
            },
            version.clone(),
        ),
        Kind::Uvx { name, version } => (
            InstallDef {
                kind,
                binary: None,
                source: Source::Uv {
                    package: name.clone(),
                },
            },
            version.clone(),
        ),
    }
}

/// How a choice shows in the app.
fn shown(choice: &Choice) -> RegistryInstall {
    match &choice.kind {
        Kind::Binary { sha256, .. } => RegistryInstall::Binary {
            checked: sha256.is_some(),
        },
        Kind::Npx { name, version } => RegistryInstall::Npx {
            package: format!("{name}@{version}"),
        },
        Kind::Uvx { name, version } => RegistryInstall::Uvx {
            package: format!("{name}=={version}"),
        },
    }
}

/// Makes `registry` the one the free functions reach. Only the first call
/// counts.
pub fn install(registry: Registry) {
    let _ = STORE.set(registry);
}

/// Answers `InstallAcpAgent`.
pub async fn install_request(
    agent: String,
    version: String,
    place: ToolPlace,
    confirm_unchecked: bool,
) -> ActionOutcome {
    match STORE.get() {
        Some(registry) => {
            registry
                .install(agent, version, place, confirm_unchecked)
                .await
        }
        None => crate::server::not_yet(),
    }
}

/// Answers `InstallTool` for a registry agent.
pub async fn update(id: String, version: Option<String>, place: ToolPlace) -> ActionOutcome {
    match STORE.get() {
        Some(registry) => registry.update(id, version, place).await,
        None => crate::server::not_yet(),
    }
}

/// Answers `RemoveTool` for a registry agent.
pub async fn remove(id: String, place: ToolPlace) -> ActionOutcome {
    match STORE.get() {
        Some(registry) => registry.remove(id, place).await,
        None => crate::server::not_yet(),
    }
}

/// A refusal for a person to read.
fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests;
