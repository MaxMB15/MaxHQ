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
use tracing::{debug, warn};

use super::{Delivery, DELIVERY};
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

    /// `<data>/skills/proposals`, beside the container sets.
    fn mirror(&self) -> PathBuf {
        self.sets.with_file_name("proposals")
    }

    /// Makes each folder in the proposals of `at` a proposal of
    /// `project`, and takes away the ones whose folder went. A new one
    /// names `agent`; one seen before keeps what it had.
    pub(super) fn found(&self, project: ProjectId, at: &Checkout, agent: Option<AgentId>) {
        // No folder to read lists nothing, so what it showed goes away.
        let dir = self.proposals_dir(at).unwrap_or_default();
        let device = self.hub.device().id;
        let lock = self.lock().locks.get(&project).cloned().unwrap_or_default();
        let mut changes = Vec::new();
        let mut now = HashMap::new();
        for folder in folders(&dir) {
            let id = id_of(at, &folder);
            let Some(made) = build(&dir.join(&folder), &lock) else {
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
        let folder = self.proposals_dir(&kept.at)?.join(&kept.folder);
        let delivery = self.clone();
        let held = proposal.clone();
        let shown = tokio::task::spawn_blocking(move || {
            // Kept in the store, the tree can be copied to the holder, and
            // the diff shows that copy, which can't change under it.
            delivery.keep(&held.tree, &folder)?;
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
        // A folder that can't be named safely any more counts as changed.
        let Ok(dir) = self.proposals_dir(&kept.at) else {
            return true;
        };
        tree::of(&DiskFs, &dir.join(&kept.folder)).map_or(true, |now| now != proposal.tree)
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
            let folder = self.proposals_dir(&kept.at)?.join(&kept.folder);
            // The check and the commit both use this copy, so a change to the
            // folder after the check can't be what lands.
            let copy = self.private_copy(&folder)?;
            let now = tree::of(&DiskFs, &copy)
                .map_err(|err| format!("The proposal can't be read: {err}."));
            if !now.as_ref().is_ok_and(|now| *now == proposal.tree) {
                let _ = maxhq_transfer::remove_tree(&copy);
                now?;
                self.clone().look(kept.project, kept.at, None).await;
                return Err(CHANGED.into());
            }
            // The copy in the store is what was checked, and can't change.
            self.keep(&proposal.tree, &folder)?;
            let copy = self
                .store
                .path(&proposal.tree)
                .ok_or("The proposal's files haven't reached this device yet.")?;
            let seen = answer.seen.as_ref();
            let committed = self
                .commit(library, &proposal, &copy, answer.fork, answer.push, seen)
                .await;
            let _ = maxhq_transfer::remove_tree(&copy);
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

    /// Puts a copy of the proposal at `folder` in the store as `tree`,
    /// leaving the folder where it is.
    fn keep(&self, tree: &maxhq_protocol::skills::TreeHash, folder: &Path) -> Result<(), String> {
        if self.store.has(tree) {
            return Ok(());
        }
        let files = tree::read(&DiskFs, folder)
            .map_err(|err| format!("The proposal can't be read: {err}."))?;
        let to = self.scratch();
        let landed = copy_files(folder, &to, &files)
            .map_err(|err| format!("couldn't copy the proposal: {err}"))
            .and_then(|()| self.store.land(tree, &to));
        if landed.is_err() {
            let _ = maxhq_transfer::remove_tree(&to);
        }
        landed.map(drop)
    }

    /// A copy of the proposal at `folder` beside the mirror, out of the
    /// checkout's reach.
    fn private_copy(&self, folder: &Path) -> Result<PathBuf, String> {
        let files = tree::read(&DiskFs, folder)
            .map_err(|err| format!("The proposal can't be read: {err}."))?;
        let to = self.scratch();
        if let Err(err) = copy_files(folder, &to, &files) {
            let _ = maxhq_transfer::remove_tree(&to);
            return Err(format!("couldn't copy the proposal: {err}"));
        }
        Ok(to)
    }

    /// A new folder name beside the mirror, on the store's disk.
    fn scratch(&self) -> PathBuf {
        self.mirror()
            .join(format!(".copy-{}", uuid::Uuid::now_v7().simple()))
    }

    /// A copy of `folder` whose `SKILL.md` is named `name`, for a fork.
    fn renamed(&self, folder: &Path, name: &str) -> Result<PathBuf, String> {
        let failed = |err: io::Error| format!("couldn't copy the skill for its fork: {err}");
        let files = tree::read(&DiskFs, folder)
            .map_err(|err| format!("The proposal can't be read: {err}."))?;
        let to = self.scratch();
        copy_files(folder, &to, &files).map_err(failed)?;
        let skill = to.join(SKILL_FILE);
        let text = fs::read_to_string(&skill).map_err(failed)?;
        fs::write(&skill, with_name(&text, name)).map_err(failed)?;
        Ok(to)
    }

    /// Removes a proposal's folder, then the proposal.
    async fn remove(self: Arc<Self>, id: SkillProposalId, kept: Kept) -> Result<(), String> {
        let folder = self.proposals_dir(&kept.at)?.join(&kept.folder);
        match kept.at.guest {
            Some(_) => {
                self.clone()
                    .read_guest(&kept.at, Some(kept.folder.clone()))
                    .await?
            }
            None => match maxhq_transfer::remove_tree(&folder) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(format!("Couldn't remove the proposal's folder: {err}")),
            },
        }
        self.lock().proposals.remove(&id);
        self.hub.apply([Change::Remove {
            key: EntityKey::SkillProposal(id),
        }]);
        Ok(())
    }
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

/// The proposal folders in `dir`, by name. Dot folders and links are
/// passed over.
fn folders(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
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

/// The proposal the folder at `folder` makes, named by its `SKILL.md` or
/// else by the folder, with its base and source from `lock`. The fields
/// that say where it is are left for the caller.
pub(super) fn build(folder: &Path, lock: &SkillLock) -> Option<SkillProposal> {
    let files = match tree::read(&DiskFs, folder) {
        Ok(files) => files,
        Err(err) => {
            warn!(%err, folder = %folder.display(), "passed over a skill proposal that can't be hashed");
            return None;
        }
    };
    let tree = tree::hash(&files).ok()?;
    let doc = maxhq_core::fs::read_text(&folder.join(SKILL_FILE), tree::MAX_BYTES)
        .ok()
        .and_then(|text| frontmatter::parse(&text).ok());
    let name = doc
        .as_ref()
        .and_then(|doc| doc.name())
        .filter(|name| frontmatter::check_name(name).is_ok())
        .map(str::to_owned)
        .or_else(|| Some(folder.file_name()?.to_str()?.to_owned()))?;
    let locked = lock.skills.iter().find(|skill| skill.name == name);
    Some(SkillProposal {
        id: SkillProposalId::derive(""),
        device: maxhq_protocol::ids::DeviceId::derive(""),
        project: ProjectId::derive(""),
        agent: None,
        flags: doc.map_or_else(SkillFlags::default, |doc| frontmatter::flags(&doc, &files)),
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

/// Copies the files of a manifest read from `from` into a new `to`.
fn copy_files(from: &Path, to: &Path, files: &[ManifestEntry]) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in files {
        let target = to.join(&entry.path);
        match entry.kind {
            EntryKind::Dir => fs::create_dir_all(&target)?,
            _ => {
                fs::copy(from.join(&entry.path), &target)?;
                // A store copy is read-only, and the rename writes it.
                let mode = if entry.executable { 0o755 } else { 0o644 };
                fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
            }
        }
    }
    Ok(())
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
