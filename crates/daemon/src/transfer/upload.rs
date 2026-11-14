// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Uploads, staged on this device or relayed to a container's guest.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use maxhq_core::skills::disk::DiskFs;
use maxhq_core::skills::store::TreeStore;
use maxhq_core::skills::tree as skill_tree;
use maxhq_protocol::guest::HostToGuest;
use maxhq_protocol::ids::{AgentId, AttachmentId, TransferId};
use maxhq_protocol::message::{ErrorCode, ManifestEntry, UploadSource};
use maxhq_protocol::skills::TreeHash;
use maxhq_protocol::timeline::AttachmentRef;
use maxhq_transfer::manifest::Layout;
use maxhq_transfer::send::RESTARTS;
use maxhq_transfer::{Failure, Source};
use tracing::debug;

use super::disk::{self, one_top, remove, ticker, Land};
use super::push::{self, Stop};
use super::{
    guest_attachment, End, Event, FromGuest, GuestLink, GuestStored, Next, Remote, Shared, Task,
    STALE_AFTER,
};

/// Where an upload on this device lands.
enum Dest {
    /// The staging folder, as an attachment.
    Staging,
    /// The skill store, as the tree it must hash to.
    Tree(TreeHash, Arc<dyn TreeStore>),
}

/// Lands an upload in this device's staging folder.
pub(super) async fn stage(task: &mut Task, source: UploadSource) -> Option<End> {
    let id = AttachmentId(task.transfer.0);
    // A begin sent again after the upload landed and its task ended.
    if let Some(found) = find(&task.shared, id).await {
        return Some(End::Stored(found));
    }
    land_here(task, source, &Dest::Staging).await
}

/// Lands an upload in this device's skill store as `tree`.
pub(super) async fn store_tree(
    task: &mut Task,
    tree: TreeHash,
    store: Arc<dyn TreeStore>,
    source: UploadSource,
) -> Option<End> {
    // The tree came another way, or a begin was sent again after it
    // landed.
    if store.has(&tree) {
        return Some(End::Landed);
    }
    land_here(task, source, &Dest::Tree(tree, store)).await
}

async fn land_here(task: &mut Task, source: UploadSource, dest: &Dest) -> Option<End> {
    clean(&task.shared).await;
    let incoming = task.shared.incoming(task.transfer);
    let end = match source {
        UploadSource::Stream { entries, .. } => stream(task, &incoming, entries, dest).await,
        UploadSource::DevicePath { path } => {
            // A copy on this device starts over, since the files may have
            // changed since an earlier try.
            remove(&incoming).await;
            copy(task, &incoming, path, dest).await
        }
    };
    match end {
        Ok(None) => None,
        Ok(Some(end @ (End::Stored(_) | End::Landed))) => Some(end),
        Ok(Some(end)) => {
            remove(&incoming).await;
            Some(end)
        }
        Err(failure) => {
            remove(&incoming).await;
            Some(End::refused(failure))
        }
    }
}

/// Lands bytes the client sends. `None` means the client went quiet, and
/// what landed stays for a resend to pick up.
async fn stream(
    task: &mut Task,
    incoming: &Path,
    entries: Vec<ManifestEntry>,
    dest: &Dest,
) -> Result<Option<End>, Failure> {
    one_top(&entries)?;
    let shared = task.shared.clone();
    let total = Layout::of(&entries).total();
    disk::check_quota(
        shared.root.clone(),
        incoming.to_owned(),
        total,
        shared.quota,
    )
    .await?;
    let mut land = Land::open(incoming.to_owned(), entries, shared.room).await?;
    task.credit(land.resume().await?).await;
    let mut tick = ticker();
    loop {
        if land.is_done() {
            return finish(task, incoming, &land, dest).await.map(Some);
        }
        let ticking = !land.is_synced();
        match task.next(ticking.then_some(&mut tick)).await {
            Next::Idle => return Ok(None),
            Next::Tick => {
                if let Some(credit) = land.tick().await? {
                    task.credit(credit).await;
                }
            }
            Next::Event(Event::ClientChunk(first)) => {
                let chunks = task
                    .client_chunks(first)
                    .into_iter()
                    .map(|chunk| (chunk.offset, chunk.data))
                    .collect();
                match land.write(chunks).await {
                    Ok(Some(credit)) => task.credit(credit).await,
                    Ok(None) => {}
                    Err(failure) if failure.code == ErrorCode::HashMismatch => {
                        task.failed(&failure).await;
                    }
                    Err(failure) => return Err(failure),
                }
            }
            Next::Event(Event::Again { route, key }) => {
                task.again(route, key);
                task.credit(land.resume().await?).await;
            }
            Next::Event(Event::Cancel) => return Ok(Some(End::Cancelled)),
            Next::Event(_) => debug!("dropped an event an upload doesn't take"),
        }
    }
}

/// Copies a file or folder on this device into staging.
async fn copy(
    task: &mut Task,
    incoming: &Path,
    path: String,
    dest: &Dest,
) -> Result<Option<End>, Failure> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(Failure::new(
            ErrorCode::BadPath,
            None,
            "a path on the device must be absolute",
        ));
    }
    let walk = disk::walk(path).await?;
    one_top(&walk.entries)?;
    let shared = task.shared.clone();
    disk::check_quota(
        shared.root.clone(),
        incoming.to_owned(),
        walk.total(),
        shared.quota,
    )
    .await?;
    let mut land = Land::open(incoming.to_owned(), walk.entries.clone(), shared.room).await?;
    let mut source = Source::new(&walk);
    let mut restarts = 0;
    while !land.is_done() {
        source = match land.fill(source).await {
            Ok(source) => source,
            Err(failure) if failure.code == ErrorCode::HashMismatch && restarts < RESTARTS => {
                restarts += 1;
                Source::new(&walk)
            }
            Err(failure) => return Err(failure),
        };
        while task.has_event() {
            match task.stash.take() {
                Some(Event::Cancel) => return Ok(Some(End::Cancelled)),
                Some(Event::Again { route, key }) => task.again(route, key),
                _ => debug!("dropped an event a copy doesn't take"),
            }
        }
    }
    finish(task, incoming, &land, dest).await.map(Some)
}

/// Moves a landed upload into staging, makes its preview or text, and
/// describes it; or moves it into the skill store once it matches its
/// tree.
async fn finish(task: &Task, incoming: &Path, land: &Land, dest: &Dest) -> Result<End, Failure> {
    let tops = land.tops();
    let [top] = tops.as_slice() else {
        return Err(Failure::new(
            ErrorCode::Internal,
            None,
            "an upload landed with more than one thing at its top",
        ));
    };
    let from = incoming.join(top);
    if let Dest::Tree(tree, store) = dest {
        let (tree, store, incoming) = (tree.clone(), store.clone(), incoming.to_owned());
        return disk::blocking(move || {
            match skill_tree::of(&DiskFs, &from) {
                Ok(found) if found == tree => {}
                Ok(_) => {
                    // Not a hash mismatch, which a client answers by
                    // sending again: the same files would fail the same.
                    return Err(Failure::new(
                        ErrorCode::NotAllowed,
                        None,
                        format!("the skill's files don't match {}", tree.0),
                    ));
                }
                Err(err) => return Err(Failure::new(ErrorCode::NotAllowed, None, err.to_string())),
            }
            store
                .land(&tree, &from)
                .map_err(|reason| Failure::new(ErrorCode::Internal, None, reason))?;
            let _ = maxhq_transfer::remove_tree(&incoming);
            Ok(End::Landed)
        })
        .await;
    }
    let id = AttachmentId(task.transfer.0);
    let into = task.shared.root.join("staged").join(id.to_string());
    let to = into.join(top);
    let incoming = incoming.to_owned();
    let shared = task.shared.clone();
    disk::blocking(move || {
        let fail = |err: std::io::Error| Failure::io(&err, None);
        fs::create_dir_all(&into).map_err(fail)?;
        fs::rename(&from, &to).map_err(fail)?;
        let _ = maxhq_transfer::remove_tree(&incoming);
        let mut found = shared.store.find(id).ok_or_else(|| {
            Failure::new(
                ErrorCode::Internal,
                None,
                "an upload landed but can't be found in staging",
            )
        })?;
        found.thumbnail = shared.store.make_sidecars(&found).thumbnail;
        Ok(End::Stored(found))
    })
    .await
}

/// Relays an upload to a container's guest.
pub(super) async fn relay(
    task: &mut Task,
    link: GuestLink,
    agent: Option<AgentId>,
    source: UploadSource,
) -> Option<End> {
    let end = match source {
        UploadSource::Stream { entries, .. } => forward(task, &link, agent, entries).await,
        UploadSource::DevicePath { path } => push_path(task, &link, agent, path).await,
    };
    match end {
        Ok(end) => end,
        Err(failure) => Some(End::refused(failure)),
    }
}

/// Passes the client's chunks to the guest and the guest's credit to the
/// client, so the client never sends more than the container has room
/// for.
async fn forward(
    task: &mut Task,
    link: &GuestLink,
    agent: Option<AgentId>,
    entries: Vec<ManifestEntry>,
) -> Result<Option<End>, Failure> {
    one_top(&entries)?;
    let transfer = task.transfer;
    let total = Layout::of(&entries).total();
    let begin = HostToGuest::UploadBegin {
        transfer,
        agent,
        entries: entries.clone(),
    };
    link.message(&begin)
        .await
        .map_err(|_| push::unreachable())?;
    let mut granted = 0;
    loop {
        let event = match task.next(None).await {
            Next::Idle | Next::Tick => return Ok(None),
            Next::Event(event) => event,
        };
        match event {
            Event::ClientChunk(first) => {
                for chunk in task.client_chunks(first) {
                    let end = chunk.offset.saturating_add(chunk.data.len() as u64);
                    if end > granted || end > total {
                        link.cancel(transfer).await;
                        return Err(Failure::new(
                            ErrorCode::BadMessage,
                            None,
                            "a chunk runs past the credit granted",
                        ));
                    }
                    link.chunk(transfer, chunk.offset, chunk.data)
                        .await
                        .map_err(|_| push::unreachable())?;
                }
            }
            Event::Guest(FromGuest::Credit {
                committed,
                window_end,
            }) => {
                if committed > window_end || window_end > total {
                    link.cancel(transfer).await;
                    return Err(Failure::new(
                        ErrorCode::BadMessage,
                        None,
                        "the container sent credit past the end of the upload",
                    ));
                }
                granted = granted.max(window_end);
                task.credit((committed, window_end)).await;
            }
            Event::Guest(FromGuest::Stored(landed)) => {
                return stored(task, link, agent, entries, &landed).map(Some);
            }
            Event::Guest(FromGuest::Failed(failure)) if failure.code == ErrorCode::HashMismatch => {
                task.failed(&failure).await;
            }
            Event::Guest(FromGuest::Failed(failure)) => return Err(failure),
            Event::GuestDown => return Err(push::unreachable()),
            Event::Again { route, key } => {
                task.again(route, key);
                link.message(&begin)
                    .await
                    .map_err(|_| push::unreachable())?;
            }
            Event::Cancel => {
                link.cancel(transfer).await;
                return Ok(Some(End::Cancelled));
            }
            Event::Guest(FromGuest::Manifest(_))
            | Event::GuestChunk(_)
            | Event::ClientCredit { .. } => debug!("dropped an event an upload doesn't take"),
        }
    }
}

/// Reads a file or folder on this device into a container.
async fn push_path(
    task: &mut Task,
    link: &GuestLink,
    agent: Option<AgentId>,
    path: String,
) -> Result<Option<End>, Failure> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(Failure::new(
            ErrorCode::BadPath,
            None,
            "a path on the device must be absolute",
        ));
    }
    let walk = disk::walk(path).await?;
    one_top(&walk.entries)?;
    match push::push(task, link, agent, &walk).await {
        Ok(landed) => stored(task, link, agent, walk.entries, &landed).map(Some),
        Err(Stop::Cancelled) => Ok(Some(End::Cancelled)),
        Err(Stop::Failed(failure)) => Err(failure),
    }
}

/// Notes an upload the guest stored, so a prompt can find it later.
fn stored(
    task: &Task,
    link: &GuestLink,
    agent: Option<AgentId>,
    entries: Vec<ManifestEntry>,
    landed: &GuestStored,
) -> Result<End, Failure> {
    let (attachment, text) = guest_attachment(task.transfer, &entries, landed)?;
    task.shared.note_remote(Remote {
        workspace: link.workspace,
        agent,
        transfer: task.transfer,
        entries,
        attachment: attachment.clone(),
        text,
        at: Instant::now(),
    });
    Ok(End::Stored(attachment))
}

/// The staged attachment with this ID, if there is one.
async fn find(shared: &Arc<Shared>, id: AttachmentId) -> Option<AttachmentRef> {
    let shared = shared.clone();
    disk::blocking(move || Ok(shared.store.find(id)))
        .await
        .ok()
        .flatten()
}

/// Removes staged copies and upload folders older than a day. Folders of
/// open transfers stay.
async fn clean(shared: &Arc<Shared>) {
    let shared = shared.clone();
    let _ = disk::blocking(move || {
        let now = SystemTime::now();
        shared.store.clean(now);
        let Ok(entries) = fs::read_dir(shared.root.join("incoming")) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let open = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<TransferId>().ok())
                .is_some_and(|transfer| shared.open.is_open(transfer));
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > STALE_AFTER));
            if stale && !open {
                let _ = maxhq_transfer::remove_tree(entry.path());
            }
        }
        Ok(())
    })
    .await;
}
