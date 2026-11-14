// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The two ends of copying a project's ignored files: listing them in the
//! source checkout, and moving an upload of them into the new one. A
//! folder workspace does both on this device, as the daemon's user, and a
//! container's guest does them inside it.

use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use maxhq_core::ignored_files::{self, IGNORED_FILES_LIMIT, LIST_TIMEOUT};
use maxhq_protocol::guest::{GuestToHost, HostToGuest, Placement};
use maxhq_protocol::ids::{AttachmentId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, IgnoredFile};

use super::deliver::place;
use super::{disk, guest_cwd, no_workspace, PlaceHost, Shared, Transfers, MAX_GUEST_PATH};
use crate::guest::worktree::git;

/// How long a guest may take to list, past git's own limit.
const LIST_WAIT: Duration = LIST_TIMEOUT.saturating_add(Duration::from_secs(5));

/// How long a guest may take to move a copy's files into place.
const PLACE_WAIT: Duration = Duration::from_secs(60);

impl Transfers {
    /// Answers `ListIgnoredFiles`: the files git ignores in a workspace's
    /// checkout.
    pub fn list_ignored(
        &self,
        workspace: WorkspaceId,
    ) -> impl Future<Output = ActionOutcome> + Send + 'static {
        let shared = self.shared.clone();
        async move {
            let Some((host, checkout)) = shared.places.workspace(workspace) else {
                return refused(no_workspace().message);
            };
            match host {
                PlaceHost::Local => {
                    let list = move || ignored_files::list(git(None), &checkout);
                    match tokio::task::spawn_blocking(list).await {
                        Ok(listed) => ActionOutcome::IgnoredFiles {
                            files: listed.files,
                            truncated: listed.truncated,
                        },
                        Err(_) => refused("the listing stopped".to_owned()),
                    }
                }
                PlaceHost::Guest { workspace } => {
                    from_guest_list(&shared, workspace, &checkout).await
                }
            }
        }
    }

    /// Answers `PlaceStaged`: moves an upload staged for a workspace into
    /// its checkout, keeping the paths under the upload's top folder. The
    /// staged upload goes once it's placed.
    pub fn place_staged(
        &self,
        workspace: WorkspaceId,
        attachment: AttachmentId,
        overwrite: bool,
    ) -> impl Future<Output = ActionOutcome> + Send + 'static {
        let shared = self.shared.clone();
        async move {
            let Some((host, checkout)) = shared.places.workspace(workspace) else {
                return refused(no_workspace().message);
            };
            let placed = match host {
                PlaceHost::Local => place_here(&shared, attachment, checkout, overwrite).await,
                PlaceHost::Guest { workspace } => {
                    place_in_guest(&shared, workspace, attachment, &checkout, overwrite).await
                }
            };
            match placed {
                Ok(placement) => ActionOutcome::Placed {
                    written: placement.written,
                    failed: placement.failed,
                },
                Err(reason) => refused(reason),
            }
        }
    }
}

fn refused(reason: String) -> ActionOutcome {
    ActionOutcome::Refused { reason }
}

async fn from_guest_list(
    shared: &Shared,
    workspace: WorkspaceId,
    checkout: &Path,
) -> ActionOutcome {
    let Some(link) = shared.link(workspace) else {
        return refused(UNREACHABLE.to_owned());
    };
    let cwd = match guest_cwd(checkout) {
        Ok(cwd) => cwd,
        Err(failure) => return refused(failure.message),
    };
    let make = Box::new(move |req| HostToGuest::ListIgnored { req, cwd });
    match link.out.request(LIST_WAIT, make).await {
        Ok(GuestToHost::Ignored {
            files, truncated, ..
        }) => match check_listing(&files) {
            Ok(()) => ActionOutcome::IgnoredFiles { files, truncated },
            Err(reason) => refused(reason.to_owned()),
        },
        Ok(_) => refused("the container answered something else".to_owned()),
        Err(reason) => refused(reason),
    }
}

/// What a request says while a container's guest isn't linked.
const UNREACHABLE: &str = "the workspace's container isn't connected";

/// Refuses a guest's listing that breaks the limits a listing keeps.
fn check_listing(files: &[IgnoredFile]) -> Result<(), &'static str> {
    if files.len() > IGNORED_FILES_LIMIT {
        return Err("the container listed more files than a listing holds");
    }
    let plain = |path: &str| {
        !path.is_empty()
            && path.len() <= MAX_GUEST_PATH
            && !path.contains('\0')
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
    };
    if !files.iter().all(|file| plain(&file.path)) {
        return Err("the container listed a path outside the checkout");
    }
    Ok(())
}

/// Places an upload staged on this device.
async fn place_here(
    shared: &Arc<Shared>,
    attachment: AttachmentId,
    checkout: PathBuf,
    overwrite: bool,
) -> Result<Placement, String> {
    let ours = shared.clone();
    let placed = tokio::task::spawn_blocking(move || {
        let found = ours.store.find(attachment).ok_or(MISSING)?;
        place(Path::new(&found.path), &checkout, overwrite, None)
    })
    .await
    .unwrap_or_else(|_| Err("placing the files stopped".to_owned()));
    if placed.is_ok() {
        let staged = shared.root.join("staged").join(attachment.to_string());
        disk::remove(&staged).await;
    }
    placed
}

/// What placing says when the upload isn't staged any more.
const MISSING: &str = "the copied files aren't staged on that device any more";

/// Places an upload staged in a container, through its guest.
async fn place_in_guest(
    shared: &Shared,
    workspace: WorkspaceId,
    attachment: AttachmentId,
    checkout: &Path,
    overwrite: bool,
) -> Result<Placement, String> {
    let remote = shared.remote().get(&attachment).cloned();
    let staged = match remote {
        Some(remote) if remote.workspace == workspace && remote.agent.is_none() => remote.transfer,
        _ => return Err(MISSING.to_owned()),
    };
    let link = shared.link(workspace).ok_or(UNREACHABLE)?;
    let cwd = guest_cwd(checkout).map_err(|failure| failure.message)?;
    let make = Box::new(move |req| HostToGuest::Place {
        req,
        staged,
        cwd,
        overwrite,
    });
    match link.out.request(PLACE_WAIT, make).await? {
        GuestToHost::Placed { result, .. } => {
            if result.is_ok() {
                shared.remote().remove(&attachment);
            }
            result
        }
        _ => Err("the container answered something else".to_owned()),
    }
}
