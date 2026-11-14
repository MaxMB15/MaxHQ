// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notes relay: while the app has links to two or more devices, it
//! keeps their project notes in step itself, so they don't need each
//! other's keys.
//!
//! This machine's own `maxd` is the hub. When a link comes up, a snapshot
//! comes in, or a device reports a change to a project's notes, the relay
//! waits for the changes to settle, then compares the note heads this
//! machine's `maxd` reports for the project with those of every other
//! connected device that reports notes for it or has a workspace in it.
//! Each device that differs swaps versions with the hub. So the hub ends up
//! with every project's notes, and the other devices only with those of
//! their own projects.
//!
//! A project has one pass at a time. Changes that come in during a pass
//! bring one more after it. A failed exchange waits for the next change or
//! link: `maxd` syncs with its peers itself while the app is closed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_client::LinkEvent;
use maxhq_core::notes::sync::{exchange, Answer, NotesEnd};
use maxhq_protocol::ids::{DeviceId, IdempotencyKey, NoteId, ProjectId};
use maxhq_protocol::link::{LinkState, Route};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::notes::{NoteVersion, VersionHash};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::ClientMessage;

use super::{device_targets, lock, Core, LinkControl};
use crate::merge::Merge;

/// How long a project's notes stay unchanged before the relay compares
/// them.
const SETTLE: Duration = Duration::from_millis(300);

/// The protocol version whose `maxd` first syncs notes.
const NOTES_SINCE: u32 = 27;

/// The projects waiting for a pass, or in one.
#[derive(Default)]
pub(super) struct Relay {
    due: Mutex<BTreeMap<ProjectId, Due>>,
    /// What the relay did, for tests to check its timing.
    #[cfg(test)]
    pub(super) log: Mutex<Vec<Logged>>,
}

/// One project's place in the relay.
#[derive(Default)]
struct Due {
    /// Counts the changes asked for, so a wait that sees it move waits
    /// again, and a pass that sees it move runs once more.
    asked: u64,
}

/// What the relay did, and when.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(super) enum Logged {
    /// A pass over a project began.
    Pass(ProjectId),
    /// An exchange with a device began.
    Start(ProjectId, DeviceId, std::time::Instant),
    /// An exchange with a device ended.
    End(ProjectId, DeviceId),
}

/// Which projects a report asks the relay to look at.
pub(super) enum Touched {
    /// None.
    Nothing,
    /// Every project with notes on some device.
    All,
    /// These.
    Some(BTreeSet<ProjectId>),
}

/// The projects whose notes a report may have put out of step: all of
/// them for a snapshot, else those whose notes changed or that gained a
/// workspace.
pub(super) fn touched(event: &LinkEvent) -> Touched {
    let changes = match event {
        LinkEvent::Synced(_) => return Touched::All,
        LinkEvent::Changed(changes) => changes,
        _ => return Touched::Nothing,
    };
    let projects: BTreeSet<ProjectId> = changes
        .iter()
        .filter_map(|change| match change {
            Change::Upsert {
                value: Entity::ProjectNotes(notes),
            } => Some(notes.project),
            Change::Upsert {
                value: Entity::Workspace(workspace),
            } => workspace.project,
            Change::Remove {
                key: EntityKey::ProjectNotes { project, .. },
            } => Some(*project),
            _ => None,
        })
        .collect();
    if projects.is_empty() {
        Touched::Nothing
    } else {
        Touched::Some(projects)
    }
}

impl<L: LinkControl> Core<L> {
    /// Has the relay look at the projects a report touched, each once
    /// its notes have settled.
    pub(super) fn relay_notes(self: &Arc<Self>, touched: Touched) {
        let projects = match touched {
            Touched::Nothing => return,
            Touched::All => self.view.read(noted_projects),
            Touched::Some(projects) => projects,
        };
        for project in projects {
            let first = {
                let mut due = lock(&self.notes.due);
                let first = !due.contains_key(&project);
                due.entry(project).or_default().asked += 1;
                first
            };
            if first {
                let core = Arc::clone(self);
                self.runtime
                    .spawn(async move { core.relay_project(project).await });
            }
        }
    }

    /// Runs passes over one project until no change came in during the
    /// last, each once the project's notes have settled.
    async fn relay_project(self: Arc<Self>, project: ProjectId) {
        loop {
            let mut seen = self.asked(project);
            loop {
                tokio::time::sleep(SETTLE).await;
                let now = self.asked(project);
                if now == seen {
                    break;
                }
                seen = now;
            }
            self.notes_pass(project).await;
            let mut due = lock(&self.notes.due);
            if due.get(&project).is_none_or(|due| due.asked == seen) {
                due.remove(&project);
                return;
            }
        }
    }

    /// How many changes the project has had asked for.
    fn asked(&self, project: ProjectId) -> u64 {
        lock(&self.notes.due)
            .get(&project)
            .map_or(0, |due| due.asked)
    }

    /// Swaps the project's notes between this machine's `maxd` and each
    /// device whose heads differ from it, one device at a time, so a later
    /// device gets an earlier one's changes in the same pass.
    async fn notes_pass(&self, project: ProjectId) {
        #[cfg(test)]
        lock(&self.notes.log).push(Logged::Pass(project));
        let Some((hub, others)) = self.view.read(|merge| out_of_step(merge, project)) else {
            return;
        };
        let hub_end = LinkNotes {
            core: self,
            device: hub,
        };
        for device in others {
            #[cfg(test)]
            lock(&self.notes.log).push(Logged::Start(project, device, std::time::Instant::now()));
            let end = LinkNotes { core: self, device };
            match exchange(&hub_end, &end, project).await {
                Ok(moved) => tracing::debug!(
                    %project, %device, to_hub = moved.to_a, to_device = moved.to_b,
                    "swapped notes"
                ),
                Err(error) => {
                    tracing::warn!(%project, %device, %error, "couldn't swap notes with a device");
                }
            }
            #[cfg(test)]
            lock(&self.notes.log).push(Logged::End(project, device));
        }
    }

    /// Sends a note request to a device and waits for its answer.
    async fn note_request(
        &self,
        device: DeviceId,
        message: ClientMessage,
    ) -> Result<ActionOutcome, String> {
        let targets = self
            .view
            .read(|merge| device_targets(merge, device))
            .map_err(|error| error.to_string())?;
        match self.act_through(targets, message).await {
            Ok(ActionOutcome::Refused { reason }) => Err(reason),
            Ok(outcome) => Ok(outcome),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// A device's notes, over its link.
struct LinkNotes<'a, L> {
    core: &'a Core<L>,
    device: DeviceId,
}

impl<L: LinkControl> LinkNotes<'_, L> {
    /// Asks the device, and reads the answer with `read`.
    async fn ask<T>(
        &self,
        message: ClientMessage,
        read: fn(ActionOutcome) -> Option<T>,
    ) -> Result<T, String> {
        let outcome = self.core.note_request(self.device, message).await?;
        read(outcome).ok_or_else(|| "The device answered a note request oddly.".to_owned())
    }
}

impl<L: LinkControl> NotesEnd for LinkNotes<'_, L> {
    fn hashes(&self, project: ProjectId) -> Answer<'_, Vec<VersionHash>> {
        let message = ClientMessage::NoteHashes {
            key: IdempotencyKey::generate(),
            project,
        };
        Box::pin(self.ask(message, |outcome| match outcome {
            ActionOutcome::NoteHashes { hashes } => Some(hashes),
            _ => None,
        }))
    }

    fn versions(
        &self,
        project: ProjectId,
        hashes: Vec<VersionHash>,
    ) -> Answer<'_, Vec<NoteVersion>> {
        let message = ClientMessage::NoteVersions {
            key: IdempotencyKey::generate(),
            project,
            hashes,
        };
        Box::pin(self.ask(message, |outcome| match outcome {
            ActionOutcome::NoteVersions { versions } => Some(versions),
            _ => None,
        }))
    }

    fn put(&self, project: ProjectId, versions: Vec<NoteVersion>) -> Answer<'_, u32> {
        let message = ClientMessage::PutNoteVersions {
            key: IdempotencyKey::generate(),
            project,
            versions,
        };
        Box::pin(self.ask(message, |outcome| match outcome {
            ActionOutcome::NotesPut { stored } => Some(stored),
            _ => None,
        }))
    }
}

/// Every project some device reports notes for.
fn noted_projects(merge: &Merge) -> BTreeSet<ProjectId> {
    merge
        .store()
        .entities()
        .filter_map(|entity| match entity {
            Entity::ProjectNotes(notes) => Some(notes.project),
            _ => None,
        })
        .collect()
}

/// This machine's device, and the other connected devices whose notes for
/// the project differ from its own, when its `maxd` is connected and syncs
/// notes.
fn out_of_step(merge: &Merge, project: ProjectId) -> Option<(DeviceId, Vec<DeviceId>)> {
    let up: BTreeMap<DeviceId, bool> = connected_devices(merge);
    let hub = merge
        .store()
        .entities()
        .find_map(|entity| match entity {
            Entity::DeviceLink(link) if link.route == Route::Local => link.device,
            _ => None,
        })
        .filter(|hub| up.get(hub) == Some(&true))?;
    let ours = heads(merge, project, hub).unwrap_or_default();
    let others = up
        .into_iter()
        .filter(|&(device, takes_notes)| device != hub && takes_notes)
        .map(|(device, _)| device)
        .filter(|&device| match heads(merge, project, device) {
            Some(theirs) => theirs != ours,
            None => !ours.is_empty() && has_workspace(merge, project, device),
        })
        .collect();
    Some((hub, others))
}

/// The devices with a connected link, each with whether its `maxd` is new
/// enough to sync notes. One whose version isn't known yet gets a try, as
/// the client sends it any request.
fn connected_devices(merge: &Merge) -> BTreeMap<DeviceId, bool> {
    let mut up = BTreeMap::new();
    for entity in merge.store().entities() {
        if let Entity::DeviceLink(link) = entity
            && let Some(device) = link.device
            && matches!(link.state, LinkState::Connected { .. })
        {
            let new_enough = link.protocol.is_none_or(|version| version >= NOTES_SINCE);
            *up.entry(device).or_insert(false) |= new_enough;
        }
    }
    up
}

/// The head of each note a device reports for the project, deleted ones
/// too, or none when it reports none.
fn heads(
    merge: &Merge,
    project: ProjectId,
    device: DeviceId,
) -> Option<BTreeMap<NoteId, VersionHash>> {
    match merge
        .store()
        .get(&EntityKey::ProjectNotes { project, device })
    {
        Some(Entity::ProjectNotes(notes)) => Some(
            notes
                .notes
                .iter()
                .map(|note| (note.note, note.head.clone()))
                .collect(),
        ),
        _ => None,
    }
}

/// Whether the device has a workspace in the project.
fn has_workspace(merge: &Merge, project: ProjectId, device: DeviceId) -> bool {
    merge.store().entities().any(|entity| {
        matches!(entity, Entity::Workspace(workspace)
            if workspace.device == device && workspace.project == Some(project))
    })
}

#[cfg(test)]
pub(super) mod tests;
