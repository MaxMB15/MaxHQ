// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Getting each project's locked skills to its checkouts on this device.
//! A folder checkout gets copies and links ([`super::apply`]). A container
//! workspace gets its set under `maxd`'s data folder, mounted read-only
//! through the `maxhq-skills` disk ([`super::sets`]), and its guest links
//! each checkout to the mount, with the always set in the agent's global
//! skills folders.
//!
//! Each launch calls [`ensure`] or [`ensure_guest`] first, so an agent
//! starts with the lock in place. After an apply, [`Running`] tells the
//! agents in the changed checkouts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use maxhq_core::skills::lock;
use maxhq_core::skills::store::TreeStore;
use maxhq_protocol::guest::{FolderScan, GuestProposal};
use maxhq_protocol::ids::{ProjectId, SkillProposalId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Vendor, Workspace, WorkspaceLocation};
use maxhq_protocol::skills::{
    CheckoutSkills, LockedSkill, PackState, ProjectSkills, SkillLock, TreeHash,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use tracing::warn;

pub use proposals::{answer, diff, look_at_start, scanned, turn_ended, Answer};
use proposals::{Checkout, Kept};

use super::apply::{self, Applied, Seen};
use super::{pack, sets};
use crate::db::Db;
use crate::drive::{Place, PlaceHost};
use crate::folder_trust;
use crate::hub::Hub;
use crate::incus::{Incus, InstanceName};
use crate::session::clock::BoxFuture;

/// The skills every agent gets, beside its project's. G decides them.
pub type Always = Arc<dyn Fn() -> Vec<LockedSkill> + Send + Sync>;

/// What an [`ensure`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ensured {
    /// Whether anything in the checkout changed.
    pub changed: bool,
    /// The skills whose copies had changed, now kept as proposals.
    pub drift: Vec<String>,
}

/// Why skills couldn't be put in.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct SkillsError(pub String);

/// A checkout whose skills changed while agents may run in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    /// The container workspace it's in, or none for this device.
    pub guest: Option<WorkspaceId>,
    /// Its path, on this device or in the container.
    pub path: PathBuf,
}

/// The agents running here, told when their skills change.
pub trait Running: Send + Sync {
    /// The skills in each of `changed` are new.
    fn skills_changed(&self, changed: Vec<Changed>);
}

/// What a guest links in one checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestLinks {
    /// The checkout's path in the container.
    pub checkout: String,
    /// The project's skills, by name.
    pub names: Vec<String>,
    /// The skills every agent gets, by name.
    pub always: Vec<String>,
    /// The global skills folders `always` goes in.
    pub homes: Vec<String>,
    /// The devflow pack's tree, which the mount holds.
    pub pack: Option<TreeHash>,
    /// Whether a failed install runs again.
    pub retry: bool,
}

/// A container workspace's guest, which makes the skill links.
pub trait GuestSkills: Send + Sync {
    /// Links a checkout to the mount as `links` says, and lands its pack,
    /// giving where the pack stands.
    fn link_skills(
        &self,
        workspace: WorkspaceId,
        links: GuestLinks,
    ) -> BoxFuture<Result<Option<PackState>, String>>;

    /// Reads the proposals in the checkout at `checkout`, after removing
    /// the one in the folder `remove`.
    fn read_proposals(
        &self,
        workspace: WorkspaceId,
        checkout: String,
        remove: Option<String>,
    ) -> BoxFuture<Result<Vec<GuestProposal>, String>> {
        let _ = (workspace, checkout, remove);
        Box::pin(async { Err("This container's guest can't read skill proposals.".to_owned()) })
    }

    /// Reads what in a folder can run when a Claude agent starts there.
    fn folder_scan(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        locked: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        let _ = (workspace, cwd, locked);
        Box::pin(async { Err("This container's guest can't read the folder.".to_owned()) })
    }
}

static DELIVERY: OnceLock<Arc<Delivery>> = OnceLock::new();

/// Makes `delivery` the one the requests and launches reach. Only the
/// first call counts.
pub fn install(delivery: Delivery) {
    let _ = DELIVERY.set(Arc::new(delivery));
}

/// Tells `running` about changed skills from now on.
pub fn watch(running: Arc<dyn Running>) {
    if let Some(delivery) = DELIVERY.get() {
        let _ = delivery.running.set(running);
    }
}

/// Answers `SkillsSetProject`.
pub fn set_project(project: ProjectId, lock: SkillLock) -> ActionOutcome {
    match DELIVERY.get() {
        Some(delivery) => outcome(delivery.set_project(project, lock)),
        None => super::refused(),
    }
}

/// Answers `SkillsApply` once every checkout has the lock.
pub async fn apply(project: ProjectId) -> ActionOutcome {
    match DELIVERY.get() {
        Some(delivery) => outcome(delivery.clone().apply(project).await),
        None => super::refused(),
    }
}

/// Answers `SkillsTreesNeeded`.
pub fn trees_needed(skills: &SkillLock) -> ActionOutcome {
    match DELIVERY.get() {
        Some(delivery) => ActionOutcome::SkillTrees {
            trees: lock::missing(skills, |tree| delivery.store.has(tree)),
        },
        None => super::refused(),
    }
}

/// Puts the project's lock into a checkout on this device before an
/// agent starts there. A container's checkout goes through
/// [`ensure_guest`] instead, so this leaves it alone.
pub fn ensure(place: &Place) -> Result<Ensured, SkillsError> {
    match DELIVERY.get() {
        Some(delivery) if place.host == PlaceHost::Local => delivery.ensure(place),
        _ => Ok(Ensured::default()),
    }
}

/// Has a container's guest link a new checkout at `checkout` to the
/// workspace's skills, with the always set in each of `homes`.
pub async fn ensure_guest(
    workspace: WorkspaceId,
    checkout: String,
    homes: Vec<String>,
) -> Result<(), SkillsError> {
    match DELIVERY.get() {
        Some(delivery) => {
            delivery
                .clone()
                .ensure_guest(workspace, checkout, homes)
                .await
        }
        None => Ok(()),
    }
}

/// The names of the skills in the lock of the project a workspace belongs
/// to.
pub fn locked_names(workspace: WorkspaceId) -> Vec<String> {
    let Some(delivery) = DELIVERY.get() else {
        return Vec::new();
    };
    let Some(project) = delivery.project_of(workspace) else {
        return Vec::new();
    };
    let state = delivery.lock();
    state.locks.get(&project).map_or_else(Vec::new, |lock| {
        lock.skills.iter().map(|skill| skill.name.clone()).collect()
    })
}

/// Drops the skill set of a container workspace whose instance is gone.
pub fn forget_workspace(workspace: WorkspaceId) {
    if let Some(delivery) = DELIVERY.get() {
        delivery.forget_workspace(workspace);
    }
}

/// Notes whether agents in the checkout at `path` still wait for a
/// restart to see its skills.
pub fn restart_marked(path: &Path, needed: bool) {
    if let Some(delivery) = DELIVERY.get() {
        delivery.restart_marked(path, needed);
    }
}

/// The global skills folders in a container that an agent of `vendor`
/// whose config folder is `config_dir` reads.
pub fn guest_homes(vendor: Vendor, config_dir: &Path) -> Vec<String> {
    let mut homes = vec![format!("{}/.agents/skills", crate::recipe::AGENT_HOME)];
    if vendor == Vendor::Claude {
        homes.push(config_dir.join("skills").display().to_string());
    }
    homes
}

fn outcome(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// This device's skill delivery.
pub struct Delivery {
    hub: Hub,
    db: Db,
    store: Arc<dyn TreeStore>,
    incus: Option<Incus>,
    /// Each container workspace's folder, as `<sets>/<workspace>`.
    sets: PathBuf,
    always: Always,
    guests: Option<Arc<dyn GuestSkills>>,
    running: OnceLock<Arc<dyn Running>>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    locks: HashMap<ProjectId, SkillLock>,
    /// What's known of each folder checkout's copies.
    seen: HashMap<PathBuf, Seen>,
    /// Each project's checkouts, by path.
    checkouts: HashMap<ProjectId, BTreeMap<String, CheckoutSkills>>,
    /// Each project's folder checkouts on this device that got skills.
    folders: HashMap<ProjectId, BTreeSet<PathBuf>>,
    /// Each container checkout's homes, by workspace and path.
    guest_homes: HashMap<WorkspaceId, BTreeMap<String, Vec<String>>>,
    /// Where each proposal this run found is.
    proposals: HashMap<SkillProposalId, Kept>,
    /// The folder checkouts whose pack is installing.
    installing: BTreeSet<PathBuf>,
}

impl Delivery {
    /// The delivery, with the locks the database kept published again.
    pub fn open(
        hub: Hub,
        db: Db,
        store: Arc<dyn TreeStore>,
        incus: Option<Incus>,
        sets: PathBuf,
        always: Always,
        guests: Option<Arc<dyn GuestSkills>>,
    ) -> Self {
        let mut state = State::default();
        match db.project_skills() {
            Ok(kept) => {
                for skills in kept {
                    state.locks.insert(skills.project, skills.lock);
                }
            }
            Err(err) => warn!(%err, "couldn't read the projects' skills"),
        }
        let delivery = Self {
            hub,
            db,
            store,
            incus,
            sets,
            always,
            guests,
            running: OnceLock::new(),
            state: Mutex::new(state),
        };
        let projects: Vec<ProjectId> = delivery.lock().locks.keys().copied().collect();
        for project in projects {
            delivery.publish(project);
        }
        delivery
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn always_names(&self) -> Vec<String> {
        (self.always)()
            .into_iter()
            .map(|skill| skill.name)
            .collect()
    }

    fn set_project(&self, project: ProjectId, skills: SkillLock) -> Result<(), String> {
        lock::check(&skills, &self.always_names()).map_err(|err| err.to_string())?;
        let kept = ProjectSkills {
            project,
            device: self.hub.device().id,
            lock: skills.clone(),
            checkouts: Vec::new(),
        };
        self.db
            .put_project_skills(&kept)
            .map_err(|err| format!("couldn't keep the project's skills: {err}"))?;
        self.lock().locks.insert(project, skills);
        self.publish(project);
        Ok(())
    }

    async fn apply(self: Arc<Self>, project: ProjectId) -> Result<(), String> {
        let skills = self
            .lock()
            .locks
            .get(&project)
            .cloned()
            .ok_or("This project has no skills on this device.")?;
        if !lock::missing(&skills, |tree| self.store.has(tree)).is_empty() {
            return Err("This device doesn't have every skill in the lock yet.".into());
        }
        let device = self.hub.device().id;
        let workspaces = self.hub.pick(|entity| match entity {
            Entity::Workspace(workspace)
                if workspace.project == Some(project) && workspace.device == device =>
            {
                Some(workspace.clone())
            }
            _ => None,
        });
        let mut folders = self
            .lock()
            .folders
            .get(&project)
            .cloned()
            .unwrap_or_default();
        let mut failed = Vec::new();
        let mut changed = Vec::new();
        for workspace in &workspaces {
            match &workspace.location {
                WorkspaceLocation::Folder { path } => {
                    folders.insert(PathBuf::from(path));
                }
                WorkspaceLocation::Container { instance, path } => {
                    match self
                        .clone()
                        .container(workspace, instance, path, true)
                        .await
                    {
                        Ok(paths) => changed.extend(paths.into_iter().map(|path| Changed {
                            guest: Some(workspace.id),
                            path: PathBuf::from(path),
                        })),
                        Err(err) => failed.push(err),
                    }
                }
            }
        }
        for path in folders {
            let delivery = self.clone();
            let skills = skills.clone();
            let root = path.clone();
            let landed =
                tokio::task::spawn_blocking(move || delivery.land(project, &skills, &root, true))
                    .await;
            match landed {
                Ok(Ok(applied)) if applied.changed => changed.push(Changed { guest: None, path }),
                Ok(Ok(_)) => {}
                Ok(Err(err)) => failed.push(err),
                Err(err) => failed.push(format!("the apply stopped: {err}")),
            }
        }
        if !changed.is_empty()
            && let Some(running) = self.running.get()
        {
            running.skills_changed(changed);
        }
        match failed.is_empty() {
            true => Ok(()),
            false => Err(failed.join(" ")),
        }
    }

    /// Puts the set in a container workspace's folder, makes sure its disk
    /// is there, and has its guest link each checkout it knows, a failed
    /// pack install again on `retry`. Gives the paths it linked.
    async fn container(
        self: Arc<Self>,
        workspace: &Workspace,
        instance: &str,
        main: &str,
        retry: bool,
    ) -> Result<Vec<String>, String> {
        let root = self.sets.join(workspace.id.to_string());
        let set = self.set_for(workspace.project);
        let (delivery, folder) = (self.clone(), root.clone());
        let published =
            tokio::task::spawn_blocking(move || sets::publish(&*delivery.store, &folder, &set))
                .await
                .map_err(|err| format!("the skill set stopped: {err}"))??;
        if let Some(incus) = &self.incus {
            let name = InstanceName::new(instance).map_err(|err| err.to_string())?;
            incus
                .add_device(&name, sets::DEVICE, sets::device(&root))
                .await
                .map_err(|err| format!("couldn't add the skills disk: {err}"))?;
        }
        let mut checkouts = self
            .lock()
            .guest_homes
            .get(&workspace.id)
            .cloned()
            .unwrap_or_default();
        checkouts.entry(main.to_owned()).or_default();
        let mut linked = Vec::new();
        for (checkout, homes) in checkouts {
            self.link(workspace, checkout.clone(), homes, retry).await?;
            linked.push(checkout);
        }
        // A set already in use changed nothing an agent sees.
        Ok(if published { linked } else { Vec::new() })
    }

    /// The set a container of `project` mounts: its lock and the always
    /// set.
    fn set_for(&self, project: Option<ProjectId>) -> SkillLock {
        let mut set = project
            .and_then(|project| self.lock().locks.get(&project).cloned())
            .unwrap_or_default();
        set.skills.extend((self.always)());
        set
    }

    /// Has the guest link one checkout, and notes it.
    async fn link(
        &self,
        workspace: &Workspace,
        checkout: String,
        homes: Vec<String>,
        retry: bool,
    ) -> Result<(), String> {
        let Some(guests) = &self.guests else {
            return Ok(());
        };
        let skills = workspace
            .project
            .and_then(|project| self.lock().locks.get(&project).cloned())
            .unwrap_or_default();
        let names: Vec<String> = skills.skills.into_iter().map(|skill| skill.name).collect();
        let installs = skills.pack.is_some();
        let links = GuestLinks {
            checkout: checkout.clone(),
            names: names.clone(),
            always: self.always_names(),
            homes: homes.clone(),
            pack: skills.pack.map(|pack| pack.tree),
            retry,
        };
        let scan = || {
            let read = guests.folder_scan(workspace.id, checkout.clone(), names.clone());
            folder_trust::from_guest(read, workspace.id, &checkout)
        };
        let before = if installs { Some(scan().await) } else { None };
        let pack = guests
            .link_skills(workspace.id, links)
            .await
            .map_err(|err| format!("couldn't link the skills in {checkout}: {err}"))?;
        if let Some(before) = before {
            folder_trust::keep_trust(&self.db, &before, &scan().await);
        }
        let mut state = self.lock();
        state
            .guest_homes
            .entry(workspace.id)
            .or_default()
            .insert(checkout.clone(), homes);
        if let Some(project) = workspace.project
            && state.locks.contains_key(&project)
        {
            state.note(project, checkout.clone(), Vec::new(), Vec::new());
            state.set_pack(project, &checkout, pack);
            drop(state);
            self.publish(project);
        }
        Ok(())
    }

    fn ensure(&self, place: &Place) -> Result<Ensured, SkillsError> {
        let Some(project) = self.project_of(place.workspace) else {
            return Ok(Ensured::default());
        };
        let Some(skills) = self.lock().locks.get(&project).cloned() else {
            return Ok(Ensured::default());
        };
        let applied = self
            .land(project, &skills, &place.cwd, false)
            .map_err(SkillsError)?;
        Ok(Ensured {
            changed: applied.changed,
            drift: applied.drift.into_iter().map(|drift| drift.name).collect(),
        })
    }

    async fn ensure_guest(
        self: Arc<Self>,
        workspace: WorkspaceId,
        checkout: String,
        homes: Vec<String>,
    ) -> Result<(), SkillsError> {
        let Some(Entity::Workspace(found)) = self.hub.get(&EntityKey::Workspace(workspace)) else {
            return Ok(());
        };
        let WorkspaceLocation::Container { instance, path } = &found.location else {
            return Ok(());
        };
        let set = self.set_for(found.project);
        if set.skills.is_empty() && set.pack.is_none() {
            return Ok(());
        }
        self.lock()
            .guest_homes
            .entry(workspace)
            .or_default()
            .insert(checkout, homes);
        self.clone()
            .container(&found, instance, path, false)
            .await
            .map(drop)
            .map_err(SkillsError)
    }

    /// Removes a gone workspace's folder, which nothing mounts any more,
    /// and what's known of its checkouts.
    fn forget_workspace(&self, workspace: WorkspaceId) {
        self.lock().guest_homes.remove(&workspace);
        let root = self.sets.join(workspace.to_string());
        if let Err(err) = apply::clear(&root) {
            warn!(%workspace, %err, "couldn't remove a gone workspace's skills");
        }
    }

    fn project_of(&self, workspace: WorkspaceId) -> Option<ProjectId> {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(workspace)) => workspace.project,
            _ => None,
        }
    }

    /// Puts `skills` into the folder checkout at `root`, keeps what drifted
    /// as proposals, lands the pack, a failed install again on `retry`,
    /// and publishes where the checkout stands.
    fn land(
        &self,
        project: ProjectId,
        skills: &SkillLock,
        root: &Path,
        retry: bool,
    ) -> Result<Applied, String> {
        let mut state = self.lock();
        let seen = state.seen.entry(root.to_owned()).or_default();
        let mut applied = apply::checkout(&*self.store, root, skills, seen)?;
        let path = root.display().to_string();
        let drift = applied
            .drift
            .iter()
            .map(|drift| drift.name.clone())
            .collect();
        state.note(project, path.clone(), drift, applied.outside(root));
        state
            .folders
            .entry(project)
            .or_default()
            .insert(root.to_owned());
        drop(state);
        let at = Checkout { guest: None, path };
        self.found(project, &at, None);
        self.publish(project);
        applied.changed |= self.land_pack(project, skills, root, retry);
        Ok(applied)
    }

    /// Lands the lock's pack in the folder checkout at `root`, or takes
    /// MaxHQ's copy out when it has none, and publishes where it stands.
    /// Says whether the installer ran. One install per checkout runs at a
    /// time, and a second asks for nothing.
    fn land_pack(&self, project: ProjectId, skills: &SkillLock, root: &Path, retry: bool) -> bool {
        let path = root.display().to_string();
        let logs = self.sets.with_file_name(pack::LOGS);
        let Some(locked) = &skills.pack else {
            let state = pack::remove(root, &logs, None).unwrap_or_else(|err| {
                warn!(%err, path, "couldn't take the pack out");
                pack::unlocked(root)
            });
            self.mark_pack(project, &path, state);
            return false;
        };
        let Some(from) = self.store.path(&locked.tree).filter(|from| from.is_dir()) else {
            let lines = vec!["This device doesn't have the pack yet.".to_owned()];
            let tree = locked.tree.clone();
            self.mark_pack(project, &path, Some(PackState::Failed { tree, lines }));
            return false;
        };
        if !self.lock().installing.insert(root.to_owned()) {
            return false;
        }
        let mut ran = false;
        let landing = pack::Landing {
            root,
            from: &from,
            tree: &locked.tree,
            logs: &logs,
            user: None,
            retry,
            timeout: pack::TIMEOUT,
        };
        let names: Vec<String> = skills
            .skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect();
        let mut before = None;
        let state = pack::land(&landing, || {
            ran = true;
            before = Some(folder_trust::scan_here(root, &names));
            let tree = locked.tree.clone();
            self.mark_pack(project, &path, Some(PackState::Installing { tree }));
        });
        if let Some(before) = before {
            folder_trust::keep_trust(&self.db, &before, &folder_trust::scan_here(root, &names));
        }
        self.lock().installing.remove(root);
        self.mark_pack(project, &path, Some(state));
        ran
    }

    /// Notes where a checkout's pack stands, publishing when it moved.
    fn mark_pack(&self, project: ProjectId, path: &str, pack: Option<PackState>) {
        if self.lock().set_pack(project, path, pack) {
            self.publish(project);
        }
    }

    fn restart_marked(&self, path: &Path, needed: bool) {
        let path = path.display().to_string();
        let mut state = self.lock();
        let Some(project) = state.checkouts.iter_mut().find_map(|(project, checkouts)| {
            let checkout = checkouts.get_mut(&path)?;
            let moved = checkout.needs_restart != needed;
            checkout.needs_restart = needed;
            moved.then_some(*project)
        }) else {
            return;
        };
        drop(state);
        self.publish(project);
    }

    /// Publishes where a project's skills stand on this device.
    fn publish(&self, project: ProjectId) {
        let state = self.lock();
        let Some(skills) = state.locks.get(&project) else {
            return;
        };
        let value = Entity::ProjectSkills(ProjectSkills {
            project,
            device: self.hub.device().id,
            lock: skills.clone(),
            checkouts: state
                .checkouts
                .get(&project)
                .map(|checkouts| checkouts.values().cloned().collect())
                .unwrap_or_default(),
        });
        drop(state);
        self.hub.apply([Change::Upsert { value }]);
    }
}

impl State {
    /// Notes that the checkout at `path` has its project's lock, with the
    /// skills in `drift` kept as proposals and `outside` the links that
    /// stopped some landing. A link at `.maxhq` stopped them all, so that
    /// checkout hasn't got the lock. A restart it waits for still counts.
    fn note(&mut self, project: ProjectId, path: String, drift: Vec<String>, outside: Vec<String>) {
        let checkouts = self.checkouts.entry(project).or_default();
        let was = checkouts.get(&path);
        let needs_restart = was.is_some_and(|was| was.needs_restart);
        let pack = was.and_then(|was| was.pack.clone());
        checkouts.insert(
            path.clone(),
            CheckoutSkills {
                path,
                applied: !outside.iter().any(|link| link.starts_with(".maxhq")),
                drift,
                needs_restart,
                pack,
                outside,
            },
        );
    }

    /// Notes where the noted checkout at `path` has its pack, saying
    /// whether that moved.
    fn set_pack(&mut self, project: ProjectId, path: &str, pack: Option<PackState>) -> bool {
        let Some(checkout) = self
            .checkouts
            .get_mut(&project)
            .and_then(|checkouts| checkouts.get_mut(path))
        else {
            return false;
        };
        let moved = checkout.pack != pack;
        checkout.pack = pack;
        moved
    }
}

mod proposals;

#[cfg(test)]
mod tests;
