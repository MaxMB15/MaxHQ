// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Skill proposals: the changed skill folders in a checkout's
//! `.maxhq/proposals/`, which an agent wrote or a launch kept as drift.
//! [`Delivery::found`] makes each folder a [`SkillProposal`], when an
//! agent's turn ends, when the project's Skills tab scans, after an apply
//! and once at start. Nothing watches the folders.
//!
//! A container's folders come through its guest and are copied under
//! `<data>/skills/proposals/`, so the rest reads them as it reads a folder
//! on this device.
//!
//! Approving one commits it to the skill's source in the library, with
//! the device's git identity as author; a third party's skill goes into
//! the personal source as a fork. Rejecting one removes it. Either way
//! the folder goes.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::{frontmatter, plan, tree, PROPOSALS_DIR, SKILL_FILE};
use maxhq_protocol::guest::GuestProposal;
use maxhq_protocol::ids::{AgentId, ProjectId, SkillProposalId, SkillSourceId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, AgentPlace, EntryKind, ManifestEntry};
use maxhq_protocol::model::WorkspaceLocation;
use maxhq_protocol::pulls::{PullFile, PullFileStatus};
use maxhq_protocol::skills::{
    ProposalAction, ProposalDiff, ProposalSeen, ProposalTarget, SkillFlags, SkillLock,
    SkillProposal, SkillSource, SourceTrust,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_transfer::beneath::Dir;
use tracing::{debug, warn};

use super::{Delivery, DELIVERY};
use crate::skills::held::Held;
use crate::skills::Library;

/// A checkout whose proposals are read: a folder on this device, or one
/// in a container workspace.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Checkout {
    /// The container workspace it's in, or none for this device.
    pub(super) guest: Option<WorkspaceId>,
    /// Its path, on this device or in the container.
    pub(super) path: String,
}

/// Where a proposal was found.
#[derive(Clone, Debug)]
pub(super) struct Kept {
    pub(super) project: ProjectId,
    pub(super) at: Checkout,
    /// The folder's name under `.maxhq/proposals`.
    pub(super) folder: String,
}

/// What an approve or reject asks for.
pub struct Answer {
    pub proposal: SkillProposalId,
    pub action: ProposalAction,
    pub push: bool,
    pub fork: Option<String>,
    pub elsewhere: Option<Box<SkillProposal>>,
    /// What the diff the owner read showed.
    pub seen: Option<ProposalSeen>,
}

/// Looks for proposals in the checkout an agent's turn just ended in, the
/// git checkout around `cwd` on this device or `cwd` in a container.
pub fn turn_ended(
    agent: AgentId,
    workspace: WorkspaceId,
    guest: Option<WorkspaceId>,
    cwd: PathBuf,
) {
    let Some(delivery) = DELIVERY.get().cloned() else {
        return;
    };
    let Some(project) = delivery.project_of(workspace) else {
        return;
    };
    tokio::spawn(async move {
        let path = match guest {
            Some(_) => cwd,
            None => tokio::task::spawn_blocking(move || {
                maxhq_core::git::find_checkout(&cwd).map_or(cwd, |checkout| checkout.root)
            })
            .await
            .unwrap_or_default(),
        };
        let at = Checkout {
            guest,
            path: path.display().to_string(),
        };
        delivery.look(project, at, Some(agent)).await;
    });
}

/// Looks for proposals in every checkout of the project at `place`, as
/// the project's Skills tab scans it.
pub async fn scanned(place: AgentPlace) {
    let Some(delivery) = DELIVERY.get().cloned() else {
        return;
    };
    let project = match &place {
        AgentPlace::Workspace { workspace } => delivery.project_of(*workspace),
        AgentPlace::Folder { path } => delivery.project_at(path),
    };
    if let Some(project) = project {
        delivery.look_all(project).await;
    }
}

/// Looks for proposals in the checkouts this device knows, so the ones
/// found before a restart come back.
pub async fn look_at_start() {
    let Some(delivery) = DELIVERY.get().cloned() else {
        return;
    };
    let device = delivery.hub.device().id;
    let mut projects: Vec<ProjectId> = delivery.hub.pick(|entity| match entity {
        Entity::Workspace(workspace) if workspace.device == device => workspace.project,
        _ => None,
    });
    projects.sort();
    projects.dedup();
    for project in projects {
        delivery.clone().look_all(project).await;
    }
}

/// Answers `SkillProposalDiff`.
pub async fn diff(
    proposal: SkillProposalId,
    elsewhere: Option<Box<SkillProposal>>,
) -> ActionOutcome {
    match DELIVERY.get().cloned() {
        Some(delivery) => {
            let library = crate::skills::installed().filter(Library::holds);
            let sources = library.as_ref().map(Library::sources);
            let diff = match elsewhere {
                Some(found) => delivery.diff_elsewhere(*found, sources).await,
                None => delivery.diff(proposal, sources).await,
            };
            match diff {
                Ok(diff) => ActionOutcome::SkillProposalDiff { diff },
                Err(reason) => ActionOutcome::Refused { reason },
            }
        }
        None => crate::skills::refused(),
    }
}

/// Answers `SkillProposalAction`.
pub async fn answer(answer: Answer) -> ActionOutcome {
    match DELIVERY.get().cloned() {
        Some(delivery) => {
            let library = crate::skills::installed();
            super::outcome(delivery.answer(library.as_ref(), answer).await)
        }
        None => crate::skills::refused(),
    }
}

impl Delivery {
    /// Where the proposals of `at` are read on this device. A checkout's
    /// folder that's a link could lead anywhere, so it's refused, and what
    /// it holds is never listed, changed or removed.
    pub(super) fn proposals_dir(&self, at: &Checkout) -> Result<PathBuf, String> {
        let Some(workspace) = at.guest else {
            let root = Path::new(&at.path);
            return match plan::store_link(&DiskFs, root) {
                Ok(None) => Ok(root.join(PROPOSALS_DIR)),
                Ok(Some(_)) => Err("The checkout's proposals folder is a link.".to_owned()),
                Err(err) => Err(format!("The proposals folder can't be read: {err}.")),
            };
        };
        Ok(self
            .mirror()
            .join(workspace.to_string())
            .join(SkillProposalId::derive(&at.path).to_string()))
    }

    /// The proposals folder of `at`, open, or none when it isn't there.
    /// Everything read or removed in it goes through this, so a link put
    /// in place of `.maxhq` or `.maxhq/proposals` after the look above
    /// fails the open instead of leading somewhere else.
    pub(super) fn open_proposals(&self, at: &Checkout) -> Result<Option<Dir>, String> {
        let dir = self.proposals_dir(at)?;
        let opened = match at.guest {
            Some(_) => Dir::open(&dir),
            None => open_checkout(Path::new(&at.path)),
        };
        match opened {
            Ok(dir) => Ok(Some(dir)),
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(None)
            }
            Err(err) => Err(format!("The proposals folder can't be read: {err}.")),
        }
    }

    /// The files of a kept proposal, read once through the open folder.
    fn read_kept(&self, kept: &Kept) -> Result<Held, String> {
        let unread = |err: &dyn std::fmt::Display| format!("The proposal can't be read: {err}.");
        let dir = self
            .open_proposals(&kept.at)?
            .ok_or_else(|| unread(&"its folder isn't there any more"))?;
        let folder = dir
            .dir(std::slice::from_ref(&kept.folder))
            .map_err(|err| unread(&err))?;
        Held::read(&folder).map_err(|err| unread(&err))
    }

    /// `<data>/skills/proposals`, beside the container sets.
    fn mirror(&self) -> PathBuf {
        self.sets.with_file_name("proposals")
    }

    /// Makes each folder in the proposals of `at` a proposal of
    /// `project`, and takes away the ones whose folder went. A new one
    /// names `agent`; one seen before keeps what it had.
    pub(super) fn found(&self, project: ProjectId, at: &Checkout, agent: Option<AgentId>) {
        // No folder to read lists nothing, so what it showed goes away.
        let dir = self.open_proposals(at).ok().flatten();
        let device = self.hub.device().id;
        let lock = self.lock().locks.get(&project).cloned().unwrap_or_default();
        let mut changes = Vec::new();
        let mut now = HashMap::new();
        for (folder, open) in dir.as_ref().map(folders).unwrap_or_default() {
            let id = id_of(at, &folder);
            let Some(made) = build(&open, &folder, &lock) else {
                continue;
            };
            let was = match self.hub.get(&EntityKey::SkillProposal(id)) {
                Some(Entity::SkillProposal(was)) => Some(was),
                _ => None,
            };
            let proposal = SkillProposal {
                id,
                device,
                project,
                agent: was.as_ref().map_or(agent, |was| was.agent),
                checkout: at.path.clone(),
                created_at: was
                    .as_ref()
                    .map_or_else(Timestamp::now, |was| was.created_at),
                ..made
            };
            if was.as_ref() != Some(&proposal) {
                changes.push(Change::Upsert {
                    value: Entity::SkillProposal(proposal),
                });
            }
            now.insert(
                id,
                Kept {
                    project,
                    at: at.clone(),
                    folder,
                },
            );
        }
        let mut state = self.lock();
        state.proposals.retain(|id, kept| {
            let gone = kept.at == *at && !now.contains_key(id);
            if gone {
                changes.push(Change::Remove {
                    key: EntityKey::SkillProposal(*id),
                });
            }
            !gone
        });
        state.proposals.extend(now);
        drop(state);
        if !changes.is_empty() {
            self.hub.apply(changes);
        }
    }

    /// Reads the proposals of `at`, through its guest for a container.
    async fn look(self: Arc<Self>, project: ProjectId, at: Checkout, agent: Option<AgentId>) {
        if let Err(err) = self.clone().read_guest(&at, None).await {
            debug!(%err, checkout = at.path, "couldn't read a container's skill proposals");
            return;
        }
        let delivery = self.clone();
        let looked = tokio::task::spawn_blocking(move || delivery.found(project, &at, agent)).await;
        if let Err(err) = looked {
            warn!(%err, "looking for skill proposals stopped");
        }
    }

    async fn look_all(self: Arc<Self>, project: ProjectId) {
        for at in self.checkouts_of(project) {
            self.clone().look(project, at, None).await;
        }
    }

    /// Copies a container checkout's proposals into the mirror, after the
    /// guest removes `remove`. A checkout on this device needs nothing.
    async fn read_guest(
        self: Arc<Self>,
        at: &Checkout,
        remove: Option<String>,
    ) -> Result<(), String> {
        let Some(workspace) = at.guest else {
            return Ok(());
        };
        let guests = self
            .guests
            .clone()
            .ok_or("This device can't reach its containers.")?;
        let read = guests
            .read_proposals(workspace, at.path.clone(), remove)
            .await?;
        let dir = self.proposals_dir(at)?;
        tokio::task::spawn_blocking(move || mirror(&dir, &read))
            .await
            .map_err(|err| format!("copying the proposals stopped: {err}"))?
            .map_err(|err| format!("couldn't copy the proposals: {err}"))
    }

    /// Every checkout of `project` this device knows: its workspaces',
    /// the ones that got skills, and the ones proposals were found in.
    fn checkouts_of(&self, project: ProjectId) -> Vec<Checkout> {
        let device = self.hub.device().id;
        let mut found: Vec<Checkout> = self.hub.pick(|entity| match entity {
            Entity::Workspace(workspace)
                if workspace.project == Some(project) && workspace.device == device =>
            {
                Some(match &workspace.location {
                    WorkspaceLocation::Folder { path } => Checkout {
                        guest: None,
                        path: path.clone(),
                    },
                    WorkspaceLocation::Container { path, .. } => Checkout {
                        guest: Some(workspace.id),
                        path: path.clone(),
                    },
                })
            }
            _ => None,
        });
        let guests: Vec<WorkspaceId> = found.iter().filter_map(|at| at.guest).collect();
        let state = self.lock();
        let folders = state.folders.get(&project).into_iter().flatten();
        found.extend(folders.map(|path| Checkout {
            guest: None,
            path: path.display().to_string(),
        }));
        for workspace in guests {
            let homes = state.guest_homes.get(&workspace).into_iter().flatten();
            found.extend(homes.map(|(path, _)| Checkout {
                guest: Some(workspace),
                path: path.clone(),
            }));
        }
        let kept = state
            .proposals
            .values()
            .filter(|kept| kept.project == project);
        found.extend(kept.map(|kept| kept.at.clone()));
        drop(state);
        let mut seen = std::collections::HashSet::new();
        found.retain(|at| seen.insert(at.clone()));
        found
    }

    /// The project whose checkout is the folder at `path`.
    fn project_at(&self, path: &str) -> Option<ProjectId> {
        let device = self.hub.device().id;
        let workspace = self.hub.pick(|entity| match entity {
            Entity::Workspace(workspace) if workspace.device == device => {
                match &workspace.location {
                    WorkspaceLocation::Folder { path: at } if at == path => workspace.project,
                    _ => None,
                }
            }
            _ => None,
        });
        let state = self.lock();
        workspace.into_iter().next().or_else(|| {
            let folder = Path::new(path);
            let with = |(project, folders): (&ProjectId, &std::collections::BTreeSet<PathBuf>)| {
                folders.contains(folder).then_some(*project)
            };
            state.folders.iter().find_map(with)
        })
    }

    /// The proposal and where it is, looking in its project's checkouts
    /// again when this run hasn't seen it.
    async fn kept(self: Arc<Self>, id: SkillProposalId) -> Result<(SkillProposal, Kept), String> {
        let gone = || "This proposal isn't on this device any more.".to_owned();
        let Some(Entity::SkillProposal(proposal)) = self.hub.get(&EntityKey::SkillProposal(id))
        else {
            return Err(gone());
        };
        if !self.lock().proposals.contains_key(&id) {
            self.clone().look_all(proposal.project).await;
        }
        let kept = self.lock().proposals.get(&id).cloned().ok_or_else(gone)?;
        Ok((proposal, kept))
    }

    async fn diff(
        self: Arc<Self>,
        id: SkillProposalId,
        sources: Option<Vec<SkillSource>>,
    ) -> Result<ProposalDiff, String> {
        let (proposal, kept) = self.clone().kept(id).await?;
        self.proposals_dir(&kept.at)?;
        let delivery = self.clone();
        let (held, at) = (proposal.clone(), kept.clone());
        let shown = tokio::task::spawn_blocking(move || {
            // Kept in the store, the tree can be copied to the holder, and
            // the diff shows that copy, which can't change under it.
            if !delivery.store.has(&held.tree) {
                delivery.keep(&held.tree, &delivery.read_kept(&at)?)?;
            }
            delivery.shown(&held, sources.as_deref())
        })
        .await
        .map_err(|err| format!("the diff stopped: {err}"))?;
        if shown.is_err() && self.has_changed(&proposal, &kept) {
            // A new look gives the viewer the proposal as it is now.
            self.clone().look(kept.project, kept.at, None).await;
        }
        shown
    }

    /// What a proposal found on another device changes against this
    /// device's library. Its tree must be in the store already.
    async fn diff_elsewhere(
        self: Arc<Self>,
        proposal: SkillProposal,
        sources: Option<Vec<SkillSource>>,
    ) -> Result<ProposalDiff, String> {
        let sources = sources.ok_or(crate::skills::NOT_YET)?;
        tokio::task::spawn_blocking(move || self.shown(&proposal, Some(&sources)))
            .await
            .map_err(|err| format!("the diff stopped: {err}"))?
    }

    /// The diff of a proposal whose tree is in the store, against the
    /// skill in `sources` when this device keeps the library, and else
    /// against the base the proposal names. Its flags are the tree's own.
    fn shown(
        &self,
        proposal: &SkillProposal,
        sources: Option<&[SkillSource]>,
    ) -> Result<ProposalDiff, String> {
        let folder = self
            .store
            .path(&proposal.tree)
            .ok_or("The proposal's files haven't reached this device yet.")?;
        let target = sources
            .map(|sources| target(sources, proposal, &folder))
            .transpose()?;
        let base = target
            .as_ref()
            .map_or(proposal.base.as_ref(), |target| target.base.as_ref());
        let base = match base {
            Some(base) if self.store.has(base) => self.store.path(base),
            Some(_) => {
                return Err(
                    "This device's store doesn't have the skill as it was, so the change can't be shown."
                        .to_owned(),
                )
            }
            None => None,
        };
        let files = tree::read(&DiskFs, &folder)
            .map_err(|err| format!("The proposal can't be read: {err}."))?;
        let flags = fs::read_to_string(folder.join(SKILL_FILE))
            .ok()
            .and_then(|text| frontmatter::parse(&text).ok())
            .map_or_else(SkillFlags::default, |doc| frontmatter::flags(&doc, &files));
        Ok(ProposalDiff {
            files: changed_files(base.as_deref(), &folder)?,
            flags,
            tree: Some(proposal.tree.clone()),
            target,
        })
    }

    /// Whether the folder of a kept proposal no longer holds its tree.
    fn has_changed(&self, proposal: &SkillProposal, kept: &Kept) -> bool {
        // A folder that can't be read safely any more counts as changed.
        let now = self.read_kept(kept).ok().and_then(|held| held.tree().ok());
        now.is_none_or(|now| now != proposal.tree)
    }

    async fn answer(
        self: Arc<Self>,
        library: Option<&Library>,
        answer: Answer,
    ) -> Result<(), String> {
        if let Some(proposal) = answer.elsewhere {
            if answer.action != ProposalAction::Approve {
                return Err("A proposal on another device is rejected there.".into());
            }
            let library = library.ok_or(crate::skills::NOT_YET)?;
            let folder = Some(&proposal.tree)
                .filter(|tree| self.store.has(tree))
                .and_then(|tree| self.store.path(tree))
                .ok_or("The proposal's files haven't reached this device yet.")?;
            let seen = answer.seen.as_ref();
            return self
                .commit(library, &proposal, &folder, answer.fork, answer.push, seen)
                .await;
        }
        let (proposal, kept) = self.clone().kept(answer.proposal).await?;
        if answer.action == ProposalAction::Approve {
            let library = library.ok_or(crate::skills::NOT_YET)?;
            // The folder is read once. What's checked is those bytes, and
            // the copy they make in the store is what's committed, so a
            // change to the folder after the check can't be what lands.
            let delivery = self.clone();
            let (at, tree) = (kept.clone(), proposal.tree.clone());
            let checked = tokio::task::spawn_blocking(move || {
                let held = delivery.read_kept(&at)?;
                let now = held
                    .tree()
                    .map_err(|err| format!("The proposal can't be read: {err}."))?;
                if now != tree {
                    return Ok(false);
                }
                delivery.keep(&tree, &held).map(|()| true)
            })
            .await
            .map_err(|err| format!("the approve stopped: {err}"))??;
            if !checked {
                self.clone().look(kept.project, kept.at, None).await;
                return Err(CHANGED.into());
            }
            // The store's copy stays: it's the store's, and other
            // proposals or skills can share its tree.
            let copy = self
                .store
                .path(&proposal.tree)
                .ok_or("The proposal's files haven't reached this device yet.")?;
            let seen = answer.seen.as_ref();
            let committed = self
                .commit(library, &proposal, &copy, answer.fork, answer.push, seen)
                .await;
            // A push that didn't go through still leaves the change in.
            if committed
                .as_ref()
                .is_err_and(|why| !why.starts_with(PUSH_FAILED))
            {
                return committed;
            }
            self.remove(answer.proposal, kept).await?;
            return committed;
        }
        self.remove(answer.proposal, kept).await
    }

    /// Commits a proposal's folder to its skill's source, or as a fork
    /// into the personal source, and pushes when asked.
    async fn commit(
        &self,
        library: &Library,
        proposal: &SkillProposal,
        folder: &Path,
        fork: Option<String>,
        push: bool,
        seen: Option<&ProposalSeen>,
    ) -> Result<(), String> {
        let sources = library.sources();
        let aim = target(&sources, proposal, folder)?;
        let fork = fork.map(|name| name.trim().to_owned());
        match seen {
            Some(seen) if seen.tree != proposal.tree || seen.base != aim.base => {
                return Err(CHANGED.into())
            }
            // An app from before `seen` took the proposal's word for it and
            // showed a new skill, so it can't be the one to replace one.
            None if fork.is_none() && proposal.base.is_none() && aim.base.is_some() => {
                return Err(format!(
                    "The library already has a skill called {}. Update MaxHQ to see what this changes.",
                    aim.name
                ))
            }
            _ => {}
        }
        let source = aim
            .source
            .and_then(|id| sources.iter().find(|source| source.id == id));
        let third_party = source.is_some_and(|source| source.trust == SourceTrust::ThirdParty);
        let (target, name, renamed): (SkillSourceId, String, Option<PathBuf>) = match (fork, source)
        {
            (Some(name), _) => {
                frontmatter::check_name(&name)
                    .map_err(|why| format!("The name {name} {why}."))?;
                let personal = library.personal().await?;
                let taken = library.sources().into_iter().any(|source| {
                    source.id == personal && source.skills.iter().any(|skill| skill.name == name)
                });
                if taken {
                    return Err(format!(
                        "Your personal source already has a skill called {name}. Pick another name."
                    ));
                }
                let copy = self.renamed(folder, &name)?;
                (personal, name, Some(copy))
            }
            (None, _) if third_party => {
                return Err(
                    "This skill is a third party's. Name a fork to approve it into your personal source."
                        .into(),
                )
            }
            (None, Some(source)) => (source.id, aim.name.clone(), None),
            (None, None) => (library.personal().await?, aim.name.clone(), None),
        };
        let message = match &renamed {
            Some(_) => format!("Fork {} as {name}", proposal.name),
            None => format!("Take the proposed change to {name}"),
        };
        let from = renamed.as_deref().unwrap_or(folder);
        let committed = library.commit_folder(target, &name, from, &message).await;
        if let Some(copy) = &renamed {
            let _ = maxhq_transfer::remove_tree(copy);
        }
        committed?;
        if push {
            library
                .push(target)
                .await
                .map_err(|why| format!("{PUSH_FAILED} {why}"))?;
        }
        Ok(())
    }

    /// Puts the proposal files `held` in the store as `tree`. The store
    /// hashes what lands, so files that aren't `tree` don't.
    fn keep(&self, tree: &maxhq_protocol::skills::TreeHash, held: &Held) -> Result<(), String> {
        if self.store.has(tree) {
            return Ok(());
        }
        let to = self.scratch();
        let landed = held
            .write(&to)
            .map_err(|err| format!("couldn't copy the proposal: {err}"))
            .and_then(|()| self.store.land(tree, &to));
        if landed.is_err() {
            let _ = maxhq_transfer::remove_tree(&to);
        }
        landed.map(drop)
    }

    /// A new folder name beside the mirror, on the store's disk.
    fn scratch(&self) -> PathBuf {
        self.mirror()
            .join(format!(".copy-{}", uuid::Uuid::now_v7().simple()))
    }

    /// A copy of `folder` whose `SKILL.md` is named `name`, for a fork.
    fn renamed(&self, folder: &Path, name: &str) -> Result<PathBuf, String> {
        let failed = |err: io::Error| format!("couldn't copy the skill for its fork: {err}");
        let held = Held::at(folder).map_err(|err| format!("The proposal can't be read: {err}."))?;
        let to = self.scratch();
        let skill = to.join(SKILL_FILE);
        let made = held.write(&to).and_then(|()| {
            let text = fs::read_to_string(&skill)?;
            fs::write(&skill, with_name(&text, name))
        });
        if let Err(err) = made {
            let _ = maxhq_transfer::remove_tree(&to);
            return Err(failed(err));
        }
        Ok(to)
    }

    /// Removes a proposal's folder, then the proposal.
    async fn remove(self: Arc<Self>, id: SkillProposalId, kept: Kept) -> Result<(), String> {
        match kept.at.guest {
            Some(_) => {
                self.clone()
                    .read_guest(&kept.at, Some(kept.folder.clone()))
                    .await?
            }
            None => {
                let delivery = self.clone();
                let at = kept.clone();
                tokio::task::spawn_blocking(move || delivery.remove_folder(&at))
                    .await
                    .map_err(|err| format!("removing the proposal stopped: {err}"))??
            }
        }
        self.lock().proposals.remove(&id);
        self.hub.apply([Change::Remove {
            key: EntityKey::SkillProposal(id),
        }]);
        Ok(())
    }

    /// Removes a kept proposal's folder on this device through the open
    /// proposals folder, so nothing outside it can be what goes.
    fn remove_folder(&self, kept: &Kept) -> Result<(), String> {
        let Some(dir) = self.open_proposals(&kept.at)? else {
            return Ok(());
        };
        dir.remove_tree(&kept.folder)
            .map_err(|err| format!("Couldn't remove the proposal's folder: {err}"))
    }
}

/// The proposals folder of the checkout at `root` on this device, opened
/// one part at a time without following a link. The checkout itself may
/// be reached through one.
fn open_checkout(root: &Path) -> io::Result<Dir> {
    let parts: Vec<String> = PROPOSALS_DIR.split('/').map(str::to_owned).collect();
    Dir::open(&fs::canonicalize(root)?)?.dir(&parts)
}

/// How a commit whose push failed starts its answer.
const PUSH_FAILED: &str = "The change is committed, but the push didn't go through.";

/// How an approve of something other than what was read is refused.
const CHANGED: &str = "The proposal changed since you opened it. Open it again to see the change.";

/// A proposal's ID, the same each time its folder is found.
fn id_of(at: &Checkout, folder: &str) -> SkillProposalId {
    let guest = at.guest.map(|id| id.to_string()).unwrap_or_default();
    SkillProposalId::derive(&format!("{guest}\n{}\n{folder}", at.path))
}

/// The proposal folders in `dir`, by name, each open. Dot folders, links
/// and files are passed over.
fn folders(dir: &Dir) -> Vec<(String, Dir)> {
    let mut names = dir.names().unwrap_or_default();
    names.retain(|name| !name.starts_with('.'));
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let open = dir.dir(std::slice::from_ref(&name)).ok()?;
            Some((name, open))
        })
        .collect()
}

/// Where approving `proposal` goes, from the library's `sources` and the
/// tree at `folder`. A proposal found on another device says what it likes
/// of itself, so its name must be the one its tree carries and its source
/// is only a first place to look: the skill of that name decides, in your
/// own sources before a third party's. With no such skill it adds one.
fn target(
    sources: &[SkillSource],
    proposal: &SkillProposal,
    folder: &Path,
) -> Result<ProposalTarget, String> {
    let name = &proposal.name;
    let text = fs::read_to_string(folder.join(SKILL_FILE)).ok();
    // A NUL makes git call the file binary, so its text wouldn't be shown.
    if text.as_deref().is_some_and(|text| text.contains('\0')) {
        return Err("This proposal's SKILL.md has a NUL byte, so its text can't be shown.".into());
    }
    let doc = text.and_then(|text| frontmatter::parse(&text).ok());
    let named = doc
        .as_ref()
        .and_then(|doc| doc.name())
        .filter(|named| frontmatter::check_name(named).is_ok());
    if named.is_some_and(|named| named != name) {
        return Err(format!(
            "This proposal says it changes {name}, but its files are for another skill."
        ));
    }
    frontmatter::check_name(name).map_err(|why| format!("The name {name} {why}."))?;
    let hinted = match proposal.source {
        Some(id) => Some(
            sources
                .iter()
                .find(|source| source.id == id)
                .ok_or("The skill's source isn't in the library any more.")?,
        ),
        None => None,
    };
    let skill_in = |source: &SkillSource| {
        let found = source.skills.iter().find(|skill| &skill.name == name);
        found.map(|skill| skill.tree.clone())
    };
    let holding = hinted
        .filter(|source| skill_in(source).is_some())
        .or_else(|| {
            let holding = || sources.iter().filter(|source| skill_in(source).is_some());
            holding()
                .find(|source| source.trust == SourceTrust::Own)
                .or_else(|| holding().next())
        });
    Ok(ProposalTarget {
        name: name.clone(),
        source: holding.or(hinted).map(|source| source.id),
        base: holding.and_then(skill_in),
    })
}

/// The proposal the folder `named`, open at `folder`, makes: named by its
/// `SKILL.md` or else by the folder, with its base and source from `lock`.
/// The fields that say where it is are left for the caller.
pub(super) fn build(folder: &Dir, named: &str, lock: &SkillLock) -> Option<SkillProposal> {
    let held = match Held::read(folder) {
        Ok(held) => held,
        Err(err) => {
            warn!(%err, folder = named, "passed over a skill proposal that can't be hashed");
            return None;
        }
    };
    let tree = held.tree().ok()?;
    let doc = held
        .file(SKILL_FILE)
        .and_then(|text| std::str::from_utf8(text).ok())
        .and_then(|text| frontmatter::parse(text).ok());
    let files = &held.entries;
    let name = doc
        .as_ref()
        .and_then(|doc| doc.name())
        .filter(|name| frontmatter::check_name(name).is_ok())
        .map_or_else(|| named.to_owned(), str::to_owned);
    let locked = lock.skills.iter().find(|skill| skill.name == name);
    Some(SkillProposal {
        id: SkillProposalId::derive(""),
        device: maxhq_protocol::ids::DeviceId::derive(""),
        project: ProjectId::derive(""),
        agent: None,
        flags: doc.map_or_else(SkillFlags::default, |doc| frontmatter::flags(&doc, files)),
        base: locked.map(|skill| skill.tree.clone()),
        source: locked.map(|skill| skill.source),
        name,
        tree,
        checkout: String::new(),
        created_at: Timestamp(0),
    })
}

/// Puts a container's proposals in `dir`, in place of what was there.
fn mirror(dir: &Path, proposals: &[GuestProposal]) -> io::Result<()> {
    match maxhq_transfer::remove_tree(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    fs::create_dir_all(dir)?;
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    for proposal in proposals {
        if !plain(&proposal.folder) || proposal.folder.contains('/') {
            return Err(bad("a proposal folder's name isn't plain"));
        }
        let folder = dir.join(&proposal.folder);
        fs::create_dir_all(&folder)?;
        for file in &proposal.files {
            if !plain(&file.path) {
                return Err(bad("a proposal file's path isn't plain"));
            }
            let bytes = STANDARD
                .decode(file.data.as_bytes())
                .map_err(|_| bad("a proposal file isn't base64"))?;
            let to = folder.join(&file.path);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&to, bytes)?;
            let mode = if file.executable { 0o755 } else { 0o644 };
            fs::set_permissions(&to, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// Whether `path` is relative, with only plain parts.
fn plain(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// A `SKILL.md` whose frontmatter names `name`.
fn with_name(text: &str, name: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let fenced = lines.first().is_some_and(|line| line.trim_end() == "---");
    let end = lines
        .iter()
        .skip(1)
        .position(|line| line.trim_end() == "---")
        .map(|at| at + 1);
    match (fenced, end) {
        (true, Some(end)) => {
            let line = format!("name: {name}");
            match lines[1..end]
                .iter()
                .position(|line| line.trim_start().starts_with("name:"))
            {
                Some(at) => lines[at + 1] = line,
                None => lines.insert(1, line),
            }
        }
        _ => {
            let mut fronted = vec!["---".to_owned(), format!("name: {name}"), "---".to_owned()];
            fronted.append(&mut lines);
            lines = fronted;
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Each file that differs between the folders `base` and `now`, as a PR's
/// files are shown. With no base, every file is new.
fn changed_files(base: Option<&Path>, now: &Path) -> Result<Vec<PullFile>, String> {
    let read = |dir: &Path| -> Result<BTreeMap<String, ManifestEntry>, String> {
        let files =
            tree::read(&DiskFs, dir).map_err(|err| format!("The skill can't be read: {err}."))?;
        Ok(files
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .map(|entry| (entry.path.clone(), entry))
            .collect())
    };
    let before = match base {
        Some(base) => read(base)?,
        None => BTreeMap::new(),
    };
    let after = read(now)?;
    let mut paths: Vec<&String> = before.keys().chain(after.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    for path in paths {
        let (old, new) = (before.get(path), after.get(path));
        let status = match (old, new) {
            (None, _) => PullFileStatus::Added,
            (_, None) => PullFileStatus::Removed,
            (Some(old), Some(new)) if old.sha256 != new.sha256 => PullFileStatus::Modified,
            (Some(old), Some(new)) if old.executable != new.executable => PullFileStatus::Changed,
            _ => continue,
        };
        let side = |dir: Option<&Path>, entry: Option<&ManifestEntry>| match (dir, entry) {
            (Some(dir), Some(_)) => dir.join(path),
            _ => PathBuf::from("/dev/null"),
        };
        let patch = match status {
            PullFileStatus::Changed => None,
            _ => patch(&side(base, old), &side(Some(now), new))?,
        };
        let count = |mark: char| {
            patch.as_deref().map_or(0, |patch| {
                patch.lines().filter(|line| line.starts_with(mark)).count() as u32
            })
        };
        files.push(PullFile {
            path: path.clone(),
            status,
            additions: count('+'),
            deletions: count('-'),
            patch,
        });
    }
    Ok(files)
}

/// The unified diff from `old` to `new`, from its first hunk on, or none
/// for a binary file.
fn patch(old: &Path, new: &Path) -> Result<Option<String>, String> {
    let mut command = crate::guest::worktree::git(None);
    command
        .args(["diff", "--no-index", "--no-color", "--no-ext-diff", "--"])
        .arg(old)
        .arg(new)
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    let out = command
        .output()
        .map_err(|err| format!("Couldn't run git for the diff: {err}"))?;
    // It ends with 1 when the files differ.
    if !matches!(out.status.code(), Some(0 | 1)) {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git couldn't diff the skill: {}", said.trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.find("\n@@").map(|at| text[at + 1..].to_owned()))
}

#[cfg(test)]
mod tests;
