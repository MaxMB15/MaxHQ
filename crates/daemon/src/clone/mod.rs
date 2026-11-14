// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Folder workspaces that MaxHQ clones: `CloneFolder` and the requests that
//! cancel, retry and remove a clone. The [`tracker`] runs git, and this
//! shows each clone as its folder's workspace in the hub and saves the
//! folder once it's cloned, so it's shown again after a restart.

mod tracker;

pub(crate) use tracker::redact;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use maxhq_protocol::ids::{DeviceId, ProjectId, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::CloneProgress;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tracing::warn;

use self::tracker::{Sink, Spec, Tracker};
use crate::db::Db;
use crate::hub::Hub;
use crate::server::not_yet;

/// The clones the request functions answer through.
static CLONES: OnceLock<Clones> = OnceLock::new();

struct Clones {
    tracker: Arc<Tracker>,
    hub: Hub,
}

/// Makes the clones on this device answer requests, with the `git` on
/// PATH. Only the first call counts.
pub fn install(hub: Hub, db: Db) {
    let device = hub.device().id;
    let sink = Arc::new(Shown {
        hub: hub.clone(),
        db,
    });
    let tracker = Tracker::new(device, "git", sink);
    let _ = CLONES.set(Clones { tracker, hub });
}

/// Shows the folders MaxHQ cloned before, those still there.
pub async fn restore(hub: Hub, db: Db) {
    let shown = tokio::task::spawn_blocking(move || {
        let folders = match db.folders() {
            Ok(folders) => folders,
            Err(err) => {
                warn!("couldn't read the cloned folders: {err}");
                return;
            }
        };
        for folder in folders.iter().filter(|folder| folder.is_dir()) {
            hub.locate_folder(folder);
        }
    });
    let _ = shown.await;
}

/// Answers `CloneFolder`: starts cloning `project`'s repo into `name` in
/// `parent`. The answer is the new folder's workspace, which shows the
/// clone from the start.
pub async fn start(
    project: ProjectId,
    device: DeviceId,
    parent: String,
    name: String,
) -> ActionOutcome {
    let Some(clones) = CLONES.get() else {
        return not_yet();
    };
    if device != clones.hub.device().id {
        return refused("That folder is for another device.".to_owned());
    }
    let Some(url) = repo_url(&clones.hub, project) else {
        return refused("maxd doesn't know that project's repository on this device.".to_owned());
    };
    match clones.tracker.start(project, url, parent, name).await {
        Ok(workspace) => ActionOutcome::Created { workspace },
        Err(reason) => refused(reason),
    }
}

/// Answers `CloneCancel`.
pub async fn cancel(workspace: WorkspaceId) -> ActionOutcome {
    match CLONES.get() {
        Some(clones) => done(clones.tracker.cancel(workspace).await),
        None => not_yet(),
    }
}

/// Answers `CloneRetry`.
pub async fn retry(workspace: WorkspaceId) -> ActionOutcome {
    match CLONES.get() {
        Some(clones) => done(clones.tracker.retry(workspace).await),
        None => not_yet(),
    }
}

/// Answers `CloneRemove`.
pub async fn remove(workspace: WorkspaceId) -> ActionOutcome {
    match CLONES.get() {
        Some(clones) => done(clones.tracker.remove(workspace).await),
        None => not_yet(),
    }
}

/// What to clone for `project`: the remote of its checkouts here, or the
/// repo this device's GitHub App reaches.
fn repo_url(hub: &Hub, project: ProjectId) -> Option<String> {
    match hub.get(&EntityKey::Project(project)) {
        Some(Entity::Project(found)) if found.repo.is_some() => found.repo,
        _ => crate::workspace::app_repo_url(project),
    }
}

fn done(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => refused(reason),
    }
}

fn refused(reason: String) -> ActionOutcome {
    ActionOutcome::Refused { reason }
}

/// Shows clones in the hub, and saves the folders that finish.
struct Shown {
    hub: Hub,
    db: Db,
}

impl Sink for Shown {
    fn show(&self, clone: &Spec, progress: CloneProgress) {
        self.hub.show_clone(&clone.folder, clone.project, progress);
    }

    fn done(&self, clone: &Spec) {
        let (hub, db, folder) = (self.hub.clone(), self.db.clone(), clone.folder.clone());
        // Reading the new checkout and saving it touch the disk.
        tokio::task::spawn_blocking(move || {
            save(&db, &folder);
            hub.cloned(&folder);
        });
    }

    fn removed(&self, clone: &Spec) {
        self.hub.forget_clone(&clone.folder);
    }
}

fn save(db: &Db, folder: &Path) {
    let Some(path) = folder.to_str() else {
        return;
    };
    if let Err(err) = db.add_folder(path, Timestamp::now()) {
        warn!(folder = %folder.display(), "couldn't save a cloned folder: {err}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    use maxhq_core::names;
    use maxhq_protocol::ids::BootId;
    use maxhq_protocol::model::{CloneStage, CloneState, Device, WorkspaceLocation};

    use super::*;
    use crate::testing::TempDir;

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

    fn shown(hub: &Hub, folder: &Path) -> Option<maxhq_protocol::model::Workspace> {
        let location = WorkspaceLocation::Folder {
            path: folder.to_str().unwrap().to_owned(),
        };
        let id = names::workspace(hub.device().id, &location);
        match hub.get(&EntityKey::Workspace(id)) {
            Some(Entity::Workspace(found)) => Some(found),
            _ => None,
        }
    }

    /// Waits until `check` holds.
    async fn until(what: &str, check: impl Fn() -> bool) {
        let start = Instant::now();
        while !check() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "never saw {what}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_clone_shows_in_the_hub_and_its_folder_survives_a_restart() {
        let dir = TempDir::new();
        let parent = dir.path().canonicalize().unwrap();
        let db = Db::open(&dir.path().join(crate::db::FILE)).unwrap();
        let hub = hub();
        let git = parent.join("git");
        fs::write(&git, "#!/bin/sh\nmkdir -p \"$5/.git\"\n").unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        let sink = Arc::new(Shown {
            hub: hub.clone(),
            db: db.clone(),
        });
        let tracker = Tracker::new(hub.device().id, git.into_os_string(), sink);

        let project = ProjectId::derive("project");
        let parent_text = parent.to_str().unwrap().to_owned();
        let url = "https://example.com/owner/name.git".to_owned();
        let workspace = tracker
            .start(project, url, parent_text, "name".to_owned())
            .await
            .unwrap();
        let folder = parent.join("name");
        until("the clone finish", || {
            shown(&hub, &folder).is_some_and(|found| found.clone.is_none())
        })
        .await;
        let found = shown(&hub, &folder).unwrap();
        assert_eq!(found.id, workspace);
        assert!(!hub.in_clone(&folder.join("src")));
        assert_eq!(db.folders().unwrap(), std::slice::from_ref(&folder));

        // A new run of maxd shows it again, but not a folder that's gone.
        db.add_folder("/nowhere/gone", Timestamp::now()).unwrap();
        let again = self::hub();
        restore(again.clone(), db.clone()).await;
        assert_eq!(shown(&again, &folder).unwrap().id, workspace);
        assert!(shown(&again, Path::new("/nowhere/gone")).is_none());
    }

    #[test]
    fn a_clone_in_progress_holds_its_folder_until_it_is_removed() {
        let dir = TempDir::new();
        let hub = hub();
        let shown_by = Shown {
            hub: hub.clone(),
            db: Db::open(&dir.path().join(crate::db::FILE)).unwrap(),
        };
        let folder = dir.path().join("name");
        let location = WorkspaceLocation::Folder {
            path: folder.to_str().unwrap().to_owned(),
        };
        let workspace = names::workspace(hub.device().id, &location);
        let spec = Spec::testing(workspace, ProjectId::derive("project"), folder.clone());
        let cloning = CloneProgress {
            state: CloneState::Cloning {
                stage: CloneStage::Receiving,
                percent: Some(42),
                stalled: false,
            },
        };
        shown_by.show(&spec, cloning.clone());
        assert_eq!(shown(&hub, &folder).unwrap().clone, Some(cloning));
        assert!(hub.in_clone(&folder.join("src")));
        // An agent seen in the folder meanwhile doesn't make it a checkout.
        hub.locate_folder(&folder);
        assert!(hub.in_clone(&folder));
        shown_by.removed(&spec);
        assert!(shown(&hub, &folder).is_none());
    }
}
