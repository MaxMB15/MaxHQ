// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Project notes on this device. Every version is kept in `maxd.db`, each
//! note merges to one head there, and the device publishes each project's
//! notes as a `ProjectNotes`, tombstones included. Clients read and save
//! notes through the requests here, agents through the notes tools, and
//! other devices swap the versions each lacks over the peer links, which
//! [`sync`] starts.
//!
//! A peer keeps notes for a project only when it has a workspace in it or
//! holds notes for it already. Otherwise it lists none and refuses a put,
//! so notes don't spread to devices that have nothing to do with them.

pub mod store;
pub mod sync;
#[cfg(test)]
pub(crate) mod tests;

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use maxhq_protocol::agents::PeerRequest;
use maxhq_protocol::ids::{DeviceId, NoteId, ProjectId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::notes::{
    NoteAuthor, NoteSummary, NoteVersion, NoteVersionInfo, ProjectNotes, VersionHash,
};
use maxhq_protocol::state::{Change, Entity};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::ClientMessage;
use tokio::sync::mpsc;
use tracing::warn;

use crate::db::Db;
use crate::hub::Hub;

pub use store::Save;

/// The refusal a peer gives for a project it keeps no notes for. The
/// device that sent the put takes it as settled and doesn't try again.
pub const NOT_KEPT: &str = "This device doesn't keep notes for that project.";

/// This device's notes. Clones share them.
#[derive(Clone)]
pub struct Notes(Arc<Inner>);

struct Inner {
    db: Db,
    hub: Hub,
    me: DeviceId,
    /// Held while a project's notes are read and published, so a list
    /// read before another save can't land after it.
    publish: Mutex<()>,
    /// Tells [`sync`] which projects changed, once it runs.
    changed: OnceLock<mpsc::UnboundedSender<sync::Changed>>,
}

impl Notes {
    /// The notes in `db`, published to `hub`.
    pub fn new(db: Db, hub: Hub) -> Self {
        let me = hub.device().id;
        Self(Arc::new(Inner {
            db,
            hub,
            me,
            publish: Mutex::new(()),
            changed: OnceLock::new(),
        }))
    }

    /// This device.
    pub fn me(&self) -> DeviceId {
        self.0.me
    }

    /// Publishes every project's notes, at start.
    pub async fn show_all(&self) {
        let shown = self
            .blocking(|notes| {
                let projects = store::projects(&notes.0.db.lock())?;
                for project in projects {
                    notes.show(project);
                }
                Ok(())
            })
            .await;
        if let Err(err) = shown {
            warn!(%err, "couldn't read the notes");
        }
    }

    /// Every project this device holds notes for.
    pub async fn projects(&self) -> Result<Vec<ProjectId>, String> {
        self.blocking(|notes| store::projects(&notes.0.db.lock()))
            .await
    }

    /// Saves a note and gives its head and whether the save merged.
    pub async fn save(&self, save: Save) -> Result<(NoteVersion, bool), String> {
        let project = save.project;
        let saved = self
            .blocking(move |notes| {
                let saved = store::save(&notes.0.db, save)?;
                notes.show(project);
                Ok(saved)
            })
            .await?;
        self.changed(project, None);
        Ok(saved)
    }

    /// A version of a note, or its head when `version` is none.
    pub async fn read(
        &self,
        project: ProjectId,
        note: NoteId,
        version: Option<VersionHash>,
    ) -> Result<NoteVersion, String> {
        self.blocking(move |notes| {
            let conn = notes.0.db.lock();
            match version {
                Some(hash) => match store::version(&conn, project, &hash)? {
                    Some(version) if version.note == note => Ok(version),
                    _ => Err(store::NO_VERSION.into()),
                },
                None => store::head(&conn, project, note)?.ok_or_else(|| store::NO_NOTE.into()),
            }
        })
        .await
    }

    /// A note's versions, newest first, without their bodies.
    pub async fn history(
        &self,
        project: ProjectId,
        note: NoteId,
    ) -> Result<Vec<NoteVersionInfo>, String> {
        self.blocking(move |notes| store::history(&notes.0.db.lock(), project, note))
            .await
    }

    /// Each of a project's notes at its head, tombstones included.
    pub async fn summaries(&self, project: ProjectId) -> Result<Vec<NoteSummary>, String> {
        self.blocking(move |notes| store::summaries(&notes.0.db.lock(), project, notes.0.me))
            .await
    }

    /// The hash of every version of a project's notes, sorted.
    pub async fn hashes(&self, project: ProjectId) -> Result<Vec<VersionHash>, String> {
        self.blocking(move |notes| store::hashes(&notes.0.db.lock(), project))
            .await
    }

    /// The versions held of `hashes`, in the order asked, maybe only the
    /// first few.
    pub async fn versions(
        &self,
        project: ProjectId,
        hashes: Vec<VersionHash>,
    ) -> Result<Vec<NoteVersion>, String> {
        self.blocking(move |notes| store::versions(&notes.0.db.lock(), project, &hashes))
            .await
    }

    /// Stores versions this device lacks and merges the heads they leave.
    /// `from` is the device that sent them, which doesn't need them back.
    pub async fn put(
        &self,
        project: ProjectId,
        versions: Vec<NoteVersion>,
        from: Option<DeviceId>,
    ) -> Result<u32, String> {
        let stored = self
            .blocking(move |notes| {
                let stored = store::put(&notes.0.db, project, &versions)?;
                if stored > 0 {
                    notes.show(project);
                }
                Ok(stored)
            })
            .await?;
        if stored > 0 {
            self.changed(project, from);
        }
        Ok(stored)
    }

    /// Whether this device keeps notes for `project`: it has a workspace
    /// in it, or holds notes for it already.
    pub async fn keeps(&self, project: ProjectId) -> Result<bool, String> {
        let me = self.0.me;
        let in_project = !self
            .0
            .hub
            .pick(|entity| match entity {
                Entity::Workspace(workspace)
                    if workspace.device == me && workspace.project == Some(project) =>
                {
                    Some(())
                }
                _ => None,
            })
            .is_empty();
        if in_project {
            return Ok(true);
        }
        self.blocking(move |notes| store::holds_project(&notes.0.db.lock(), project))
            .await
    }

    /// Answers one of a client's note requests. Anything else is refused.
    pub async fn answer(&self, message: ClientMessage) -> ActionOutcome {
        let outcome = match message {
            ClientMessage::ReadNote {
                project,
                note,
                version,
                ..
            } => self
                .read(project, note, version)
                .await
                .map(|version| ActionOutcome::NoteRead { version }),
            ClientMessage::SaveNote {
                project,
                note,
                base,
                title,
                body,
                deleted,
                ..
            } => self
                .save(Save {
                    project,
                    note,
                    base,
                    title,
                    body,
                    deleted,
                    at: Timestamp::now(),
                    by: Some(NoteAuthor {
                        device: self.0.me,
                        agent: None,
                    }),
                })
                .await
                .map(|(version, merged)| ActionOutcome::NoteSaved { version, merged }),
            ClientMessage::NoteHistory { project, note, .. } => self
                .history(project, note)
                .await
                .map(|versions| ActionOutcome::NoteHistory { versions }),
            ClientMessage::NoteHashes { project, .. } => self
                .hashes(project)
                .await
                .map(|hashes| ActionOutcome::NoteHashes { hashes }),
            ClientMessage::NoteVersions {
                project, hashes, ..
            } => self
                .versions(project, hashes)
                .await
                .map(|versions| ActionOutcome::NoteVersions { versions }),
            ClientMessage::PutNoteVersions {
                project, versions, ..
            } => self
                .put(project, versions, None)
                .await
                .map(|stored| ActionOutcome::NotesPut { stored }),
            _ => Err("That isn't a note request.".into()),
        };
        outcome.unwrap_or_else(|reason| ActionOutcome::Refused { reason })
    }

    /// Answers a note request another device sent, for a project this
    /// device keeps notes for. For any other it lists nothing and refuses
    /// a put.
    pub async fn serve(&self, from: DeviceId, request: PeerRequest) -> ActionOutcome {
        let project = match &request {
            PeerRequest::NoteHashes { project }
            | PeerRequest::NoteVersions { project, .. }
            | PeerRequest::PutNoteVersions { project, .. } => *project,
            _ => {
                return ActionOutcome::Refused {
                    reason: "That isn't a note request.".into(),
                }
            }
        };
        let kept = match self.keeps(project).await {
            Ok(kept) => kept,
            Err(reason) => return ActionOutcome::Refused { reason },
        };
        let outcome = match request {
            PeerRequest::NoteHashes { .. } if !kept => {
                Ok(ActionOutcome::NoteHashes { hashes: Vec::new() })
            }
            PeerRequest::NoteVersions { .. } if !kept => Ok(ActionOutcome::NoteVersions {
                versions: Vec::new(),
            }),
            PeerRequest::PutNoteVersions { .. } if !kept => Err(NOT_KEPT.to_owned()),
            PeerRequest::NoteHashes { .. } => self
                .hashes(project)
                .await
                .map(|hashes| ActionOutcome::NoteHashes { hashes }),
            PeerRequest::NoteVersions { hashes, .. } => self
                .versions(project, hashes)
                .await
                .map(|versions| ActionOutcome::NoteVersions { versions }),
            PeerRequest::PutNoteVersions { versions, .. } => self
                .put(project, versions, Some(from))
                .await
                .map(|stored| ActionOutcome::NotesPut { stored }),
            _ => unreachable!("matched above"),
        };
        outcome.unwrap_or_else(|reason| ActionOutcome::Refused { reason })
    }

    /// Has the other devices brought up to date on a project, all but
    /// `from`.
    fn changed(&self, project: ProjectId, from: Option<DeviceId>) {
        if let Some(changed) = self.0.changed.get() {
            let _ = changed.send(sync::Changed { project, from });
        }
    }

    /// Reads a project's notes and publishes them. Run it off the loop. A
    /// read fails only when SQLite does, and the next change shows them
    /// again.
    fn show(&self, project: ProjectId) {
        let _held = self
            .0
            .publish
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let notes = match store::summaries(&self.0.db.lock(), project, self.0.me) {
            Ok(notes) => notes,
            Err(err) => {
                warn!(%err, %project, "couldn't read a project's notes");
                return;
            }
        };
        if notes.is_empty() {
            return;
        }
        self.0.hub.apply([Change::Upsert {
            value: Entity::ProjectNotes(ProjectNotes {
                device: self.0.me,
                project,
                notes,
            }),
        }]);
    }

    /// Runs `work` on a blocking thread, since it reads or writes SQLite.
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Notes) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let notes = self.clone();
        tokio::task::spawn_blocking(move || work(&notes))
            .await
            .unwrap_or_else(|err| Err(format!("The notes failed: {err}")))
    }
}

/// The notes `maxd` runs, once it has made them.
static NOTES: OnceLock<Notes> = OnceLock::new();

/// Makes `notes` the ones the server answers clients from. Only the first
/// call counts.
pub fn install(notes: Notes) {
    let _ = NOTES.set(notes);
}

/// Answers one of a client's note requests from the installed notes.
pub async fn answer(message: ClientMessage) -> ActionOutcome {
    match NOTES.get() {
        Some(notes) => notes.answer(message).await,
        None => ActionOutcome::Refused {
            reason: "maxd is still starting.".into(),
        },
    }
}
