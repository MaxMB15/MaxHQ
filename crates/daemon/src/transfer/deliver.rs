// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving staged attachments into an agent's attachments folder before a
//! prompt goes out, on this device or in its container, and copied files
//! into a checkout.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use maxhq_hold::RunAs;
use maxhq_os::fsmode;
use maxhq_protocol::guest::{HostToGuest, Placement};
use maxhq_protocol::ids::{AgentId, AttachmentId, TransferId, WorkspaceId};
use maxhq_protocol::message::PlaceFailure;
use maxhq_protocol::timeline::AttachmentRef;
use maxhq_transfer::beneath::{set_mode, Dir, FileType};
use maxhq_transfer::{name, Failure};
use tracing::debug;

use super::disk;
use super::push::{self, Stop};
use super::registry::Peer;
use super::{guest_attachment, Event, FromGuest, GuestLink, Remote, Task, Transfers, STALL};
use crate::drive::{Place, PlaceHost, Staged};
use crate::session::clock::BoxFuture;
use crate::session::holders::{AgentFiles, DeliverError, Looked};
use maxhq_core::tools::Page;

/// How many times a delivery tries to open a transfer an upload still
/// holds, and how long it waits between tries.
const OPEN_TRIES: u32 = 20;
const OPEN_WAIT: Duration = Duration::from_millis(50);

impl AgentFiles for Transfers {
    fn deliver(
        &self,
        place: &Place,
        staged: Vec<AttachmentId>,
    ) -> BoxFuture<Result<Vec<Staged>, DeliverError>> {
        let transfers = self.clone();
        let place = place.clone();
        Box::pin(async move {
            let mut delivered = Vec::with_capacity(staged.len());
            for id in staged {
                delivered.push(transfers.deliver_one(&place, id).await?);
            }
            Ok(delivered)
        })
    }

    fn collect(
        &self,
        place: &Place,
        call: &str,
        paths: Vec<String>,
    ) -> BoxFuture<Result<Vec<AttachmentRef>, String>> {
        let transfers = self.clone();
        let place = place.clone();
        let call = call.to_owned();
        Box::pin(async move { transfers.collect_files(&place, &call, &paths).await })
    }

    fn stage_found(
        &self,
        place: &Place,
        id: AttachmentId,
        path: &str,
    ) -> BoxFuture<Result<AttachmentRef, String>> {
        let transfers = self.clone();
        let place = place.clone();
        let path = path.to_owned();
        Box::pin(async move { transfers.stage_found(&place, id, &path).await })
    }

    fn look(&self, place: &Place, path: String) -> BoxFuture<Result<Looked, String>> {
        let transfers = self.clone();
        let place = place.clone();
        Box::pin(async move { transfers.look_at(&place, &path).await })
    }

    fn show(
        &self,
        place: &Place,
        call: &str,
        page: Page,
        name: String,
    ) -> BoxFuture<Result<(AttachmentRef, String), String>> {
        let transfers = self.clone();
        let place = place.clone();
        let call = call.to_owned();
        Box::pin(async move { transfers.show_page(&place, &call, page, &name).await })
    }
}

fn copy_failed(message: impl Into<String>) -> DeliverError {
    DeliverError::Copy(message.into())
}

impl Transfers {
    pub(super) async fn deliver_one(
        &self,
        place: &Place,
        id: AttachmentId,
    ) -> Result<Staged, DeliverError> {
        match place.host {
            PlaceHost::Local => self.take_local(place, id).await,
            PlaceHost::Guest { workspace } => {
                let agent = place
                    .attachments
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<AgentId>().ok())
                    .ok_or_else(|| {
                        copy_failed("the agent's attachments folder isn't named for it")
                    })?;
                let remote = self.shared.remote().get(&id).cloned();
                match remote {
                    Some(remote) if remote.workspace != workspace => Err(DeliverError::Missing(id)),
                    Some(remote) if remote.agent == Some(agent) => Ok(Staged {
                        path: PathBuf::from(&remote.attachment.path),
                        attachment: remote.attachment,
                        text: remote.text.map(PathBuf::from),
                    }),
                    // Another agent's folder has it, and it stays theirs.
                    Some(remote) if remote.agent.is_some() => Err(DeliverError::Missing(id)),
                    Some(remote) => self.relocate(workspace, agent, remote).await,
                    None => self.push_staged(workspace, agent, id).await,
                }
            }
        }
    }

    /// Moves an attachment staged on this device into a local agent's
    /// folder, as M2 does. One a message moved there already stays.
    async fn take_local(&self, place: &Place, id: AttachmentId) -> Result<Staged, DeliverError> {
        let shared = self.shared.clone();
        let into = place.attachments.clone();
        tokio::task::spawn_blocking(move || {
            let found = shared
                .store
                .find(id)
                .or_else(|| shared.store.sent(id, &into))
                .ok_or(DeliverError::Missing(id))?;
            shared
                .store
                .take(&found, &into)
                .map_err(|err| copy_failed(err.to_string()))
        })
        .await
        .map_err(|_| copy_failed("the copy stopped"))?
    }

    /// Opens a transfer only a workspace's guest may name, for work no
    /// client asked for over a connection.
    async fn open_for(
        &self,
        transfer: TransferId,
        workspace: WorkspaceId,
    ) -> Result<Task, DeliverError> {
        for _ in 0..OPEN_TRIES {
            if let Some(inbox) = self
                .shared
                .open
                .open(transfer, vec![Peer::Guest(workspace)])
            {
                return Ok(Task {
                    shared: self.shared.clone(),
                    inbox,
                    route: None,
                    key: None,
                    transfer,
                    stash: None,
                });
            }
            tokio::time::sleep(OPEN_WAIT).await;
        }
        Err(copy_failed("the attachment is still being uploaded"))
    }

    fn link_for(&self, workspace: WorkspaceId) -> Result<GuestLink, DeliverError> {
        self.shared
            .link(workspace)
            .ok_or_else(|| copy_failed("the workspace's container isn't connected"))
    }

    /// Moves an upload from the container's staging folder into this
    /// agent's. Sending the begin again with the agent set makes the guest
    /// move it.
    async fn relocate(
        &self,
        workspace: WorkspaceId,
        agent: AgentId,
        remote: Remote,
    ) -> Result<Staged, DeliverError> {
        let link = self.link_for(workspace)?;
        let mut task = self.open_for(remote.transfer, workspace).await?;
        let begin = HostToGuest::UploadBegin {
            transfer: remote.transfer,
            agent: Some(agent),
            entries: remote.entries.clone(),
        };
        let gone = || copy_failed("the workspace's container went away");
        link.message(&begin).await.map_err(|_| gone())?;
        loop {
            match task.next_within(STALL).await {
                Some(Event::Guest(FromGuest::Stored(landed))) => {
                    let (mut attachment, text) =
                        guest_attachment(remote.transfer, &remote.entries, &landed)
                            .map_err(|failure| copy_failed(failure.message))?;
                    attachment.id = remote.attachment.id;
                    attachment.thumbnail = attachment.thumbnail.or(remote.attachment.thumbnail);
                    self.shared.note_remote(Remote {
                        agent: Some(agent),
                        attachment: attachment.clone(),
                        text: text.clone(),
                        at: Instant::now(),
                        ..remote
                    });
                    return Ok(Staged {
                        path: PathBuf::from(&attachment.path),
                        attachment,
                        text: text.map(PathBuf::from),
                    });
                }
                // The guest no longer has it, and opened a new landing.
                Some(Event::Guest(FromGuest::Credit { .. })) => {
                    link.cancel(remote.transfer).await;
                    self.shared.remote().remove(&remote.attachment.id);
                    return Err(DeliverError::Missing(remote.attachment.id));
                }
                Some(Event::Guest(FromGuest::Failed(failure))) => {
                    return Err(copy_failed(failure.message))
                }
                Some(Event::GuestDown) | None => return Err(gone()),
                Some(_) => debug!("dropped an event a delivery doesn't take"),
            }
        }
    }

    /// Pushes an attachment staged on this device into the container, then
    /// removes the staged copy. It's noted as the agent's, so a message to
    /// the agent can name it again, such as a queued one taken back.
    async fn push_staged(
        &self,
        workspace: WorkspaceId,
        agent: AgentId,
        id: AttachmentId,
    ) -> Result<Staged, DeliverError> {
        let shared = self.shared.clone();
        let found = tokio::task::spawn_blocking(move || shared.store.find(id))
            .await
            .map_err(|_| copy_failed("the copy stopped"))?
            .ok_or(DeliverError::Missing(id))?;
        let link = self.link_for(workspace)?;
        let walk = disk::walk(PathBuf::from(&found.path))
            .await
            .map_err(|failure| copy_failed(failure.message))?;
        let transfer = TransferId::generate();
        let mut task = self.open_for(transfer, workspace).await?;
        let landed = match push::push(&mut task, &link, Some(agent), &walk).await {
            Ok(landed) => landed,
            Err(Stop::Failed(failure)) => return Err(copy_failed(failure.message)),
            Err(Stop::Cancelled) => return Err(copy_failed("the copy was cancelled")),
        };
        drop(task);
        let (mut attachment, text) = guest_attachment(transfer, &walk.entries, &landed)
            .map_err(|failure: Failure| copy_failed(failure.message))?;
        attachment.id = id;
        attachment.name = found.name;
        attachment.mime = found.mime;
        attachment.kind = found.kind;
        attachment.thumbnail = found.thumbnail.or(attachment.thumbnail);
        let staged = self.shared.root.join("staged").join(id.to_string());
        disk::remove(&staged).await;
        self.shared.note_remote(Remote {
            workspace,
            agent: Some(agent),
            transfer,
            entries: walk.entries,
            attachment: attachment.clone(),
            text: text.clone(),
            at: Instant::now(),
        });
        Ok(Staged {
            path: PathBuf::from(&attachment.path),
            attachment,
            text: text.map(PathBuf::from),
        })
    }
}

/// Moves the files under `top`, an upload's top folder in a staging
/// folder, to the same paths under `checkout`, such as the ignored files a
/// project copy brings along. Folders on the way are made as needed.
///
/// A `.git` part, a name the disk resolves to the `.git` beside it, a link or
/// anything else that isn't a file or folder is left out, and so is a file
/// already in the checkout unless `overwrite` is set. Each one left out is in
/// `failed`. Nothing in the checkout is followed: every step opens beneath
/// it. With `owner` set, which takes root, the files moved and the folders
/// made become that user's.
pub(crate) fn place(
    top: &Path,
    checkout: &Path,
    overwrite: bool,
    owner: Option<&RunAs>,
) -> Result<Placement, String> {
    let into = Dir::open(checkout)
        .map_err(|err| format!("couldn't open {}: {err}", checkout.display()))?;
    place_into(top, &into, overwrite, owner)
}

/// [`place`] into a folder already open, for a caller that can't trust the
/// path to it to stay the same.
pub(crate) fn place_into(
    top: &Path,
    into: &Dir,
    overwrite: bool,
    owner: Option<&RunAs>,
) -> Result<Placement, String> {
    let staged = Dir::open(top).map_err(|err| format!("couldn't open the copied files: {err}"))?;
    let mut placement = Placement {
        written: 0,
        failed: Vec::new(),
    };
    let mut folders = vec![Vec::new()];
    while let Some(parts) = folders.pop() {
        let at = |name: &str| {
            let mut path = parts.join("/");
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(name);
            path
        };
        let from = staged
            .dir(&parts)
            .map_err(|err| format!("couldn't open the copied files: {err}"))?;
        let to = match into.dir(&parts) {
            Ok(to) => to,
            Err(err) => {
                placement.failed.push(PlaceFailure {
                    path: parts.join("/"),
                    reason: format!("couldn't open the folder: {err}"),
                });
                continue;
            }
        };
        let names = fs::read_dir(top.join(parts.join("/")))
            .map_err(|err| format!("couldn't read the copied files: {err}"))?;
        for entry in names {
            let entry = entry.map_err(|err| format!("couldn't read the copied files: {err}"))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                placement.failed.push(PlaceFailure {
                    path: at(&entry.file_name().to_string_lossy()),
                    reason: "the name isn't text".to_owned(),
                });
                continue;
            };
            let fail = |reason: String| PlaceFailure {
                path: at(&name),
                reason,
            };
            // The disk has the last word on a name, since its folding rules
            // may not match `skipped`. An error here counts as a match.
            if name::part(&name).is_err()
                || name::skipped(&name)
                || to.same_entry(&name, ".git").unwrap_or(true)
            {
                placement
                    .failed
                    .push(fail("MaxHQ doesn't copy a file by that name".to_owned()));
                continue;
            }
            let kind = entry.file_type().map_err(|err| err.to_string());
            let there = to.kind(&name).map_err(|err| err.to_string());
            let moved = match (kind, there) {
                (Err(err), _) | (_, Err(err)) => Err(err),
                (Ok(kind), Ok(there)) if kind.is_dir() => match there {
                    None => make_folder(&to, &name, owner),
                    Some(FileType::Directory) => Ok(()),
                    Some(_) => Err("something that isn't a folder is in the way".to_owned()),
                }
                .map(|()| {
                    let mut inner = parts.clone();
                    inner.push(name.clone());
                    folders.push(inner);
                })
                .map(|()| false),
                (Ok(kind), Ok(_)) if !kind.is_file() => {
                    Err("it's a link or a special file, not a file".to_owned())
                }
                (Ok(_), Ok(None)) => move_file(&from, &to, &name, owner).map(|()| true),
                (Ok(_), Ok(Some(FileType::RegularFile))) if overwrite => {
                    move_file(&from, &to, &name, owner).map(|()| true)
                }
                (Ok(_), Ok(Some(FileType::RegularFile))) => {
                    Err("a file is already there".to_owned())
                }
                (Ok(_), Ok(Some(_))) => Err("something that isn't a file is there".to_owned()),
            };
            match moved {
                Ok(true) => placement.written += 1,
                Ok(false) => {}
                Err(reason) => placement.failed.push(fail(reason)),
            }
        }
    }
    Ok(placement)
}

/// Makes the folder `name` in `to`, giving it to `owner`.
fn make_folder(to: &Dir, name: &str, owner: Option<&RunAs>) -> Result<(), String> {
    to.make_dir(name, 0o755).map_err(|err| err.to_string())?;
    if let Some(user) = owner {
        to.chown(name, user.uid, user.gid)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Moves the file `name` from `from` to `to`, giving it to `owner` first.
/// Across disks it's copied to a `.part` file beside where it goes, which
/// is then renamed into place.
fn move_file(from: &Dir, to: &Dir, name: &str, owner: Option<&RunAs>) -> Result<(), String> {
    if let Some(user) = owner {
        from.chown(name, user.uid, user.gid)
            .map_err(|err| err.to_string())?;
    }
    match from.rename_into(name, to, name) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() != io::ErrorKind::CrossesDevices => {
            return Err(err.to_string());
        }
        Err(_) => {}
    }
    let part = format!(".{name}.part");
    let copied = copy_across(from, to, name, &part, owner).and_then(|()| to.rename(&part, name));
    if copied.is_err() {
        let _ = to.remove_file(&part);
    }
    copied.map_err(|err| err.to_string())
}

fn copy_across(
    from: &Dir,
    to: &Dir,
    name: &str,
    part: &str,
    owner: Option<&RunAs>,
) -> io::Result<()> {
    let mut source = from.read_file(name)?;
    let executable = fsmode::mode_of(&source.metadata()?) & 0o111 != 0;
    let mut file = to.open_file(part, true)?;
    file.set_len(0)?;
    io::copy(&mut source, &mut file)?;
    set_mode(&file, executable)?;
    file.sync_all()?;
    if let Some(user) = owner {
        to.chown(part, user.uid, user.gid)?;
    }
    Ok(())
}
