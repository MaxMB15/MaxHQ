// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Downloads from an agent's folders, or chosen files from a checkout, to
//! a client: a manifest, then chunks as the client's credit allows.

use std::path::PathBuf;

use maxhq_protocol::guest::HostToGuest;
use maxhq_protocol::message::{EntryKind, ErrorCode, ManifestEntry};
use maxhq_protocol::ServerMessage;
use maxhq_transfer::manifest::{self, Layout};
use maxhq_transfer::{Failure, Source, Walk};
use tracing::debug;

use super::disk::{self, Pump};
use super::push;
use super::{End, Event, FromGuest, GuestLink, Next, Place, Task};

/// Sends a file or folder in a local agent's folders. With `anyway`, it's
/// the one file you asked for by name with Show anyway, wherever it is.
pub(super) async fn local(
    task: &mut Task,
    place: Place,
    path: String,
    anyway: bool,
) -> Option<End> {
    let walked = async {
        if anyway {
            let found = disk::blocking(move || disk::named(&path)).await?;
            let walk = disk::walk(found).await?;
            return one_file(walk);
        }
        let roots = [place.cwd, place.attachments];
        let found = disk::blocking(move || disk::resolve(&roots, &path)).await?;
        disk::walk(found).await
    };
    match walked.await {
        Ok(walk) => serve(task, walk).await,
        Err(failure) => Some(End::refused(failure)),
    }
}

/// Checks a walk for Show anyway still holds one file the media cache
/// takes, in case what's at the path changed since it was looked at.
fn one_file(walk: Walk) -> Result<Walk, Failure> {
    match walk.entries.as_slice() {
        [entry] if entry.kind == EntryKind::File && entry.size <= maxhq_client::media::MOST => {
            Ok(walk)
        }
        [entry] if entry.kind == EntryKind::File => Err(Failure::new(
            ErrorCode::TooLarge,
            None,
            "the file is larger than MaxHQ shows",
        )),
        _ => Err(Failure::new(
            ErrorCode::BadPath,
            None,
            "Show anyway opens only a file",
        )),
    }
}

/// Sends chosen files from a checkout on this device.
pub(super) async fn files(task: &mut Task, root: PathBuf, paths: Vec<String>) -> Option<End> {
    match disk::walk_files(root, paths).await {
        Ok(walk) => serve(task, walk).await,
        Err(failure) => Some(End::refused(failure)),
    }
}

/// Sends a folder on this device, such as a tree in the skill store.
pub(super) async fn folder(task: &mut Task, path: PathBuf) -> Option<End> {
    match disk::walk(path).await {
        Ok(walk) => serve(task, walk).await,
        Err(failure) => Some(End::refused(failure)),
    }
}

async fn serve(task: &mut Task, walk: Walk) -> Option<End> {
    match pump(task, walk).await {
        Ok(end) => end,
        Err(failure) => Some(End::refused(failure)),
    }
}

async fn pump(task: &mut Task, walk: Walk) -> Result<Option<End>, Failure> {
    let manifest = ServerMessage::FetchManifest {
        transfer: task.transfer,
        entries: walk.entries.clone(),
    };
    task.send(manifest.clone()).await;
    let mut pump = Pump::new(Source::new(&walk));
    // The first credit after a manifest says where the client picks up.
    let mut fresh = true;
    loop {
        if pump.outgoing().is_committed() {
            return Ok(Some(End::Fetched));
        }
        while !task.has_event() {
            match pump.next().await? {
                Some(chunks) => {
                    if !task.chunks(chunks).await {
                        break;
                    }
                }
                None => break,
            }
        }
        let event = match task.next(None).await {
            Next::Idle | Next::Tick => return Ok(None),
            Next::Event(event) => event,
        };
        match event {
            Event::ClientCredit {
                committed,
                window_end,
            } => {
                if fresh {
                    pump.resume(committed, window_end)?;
                } else {
                    pump.credit(committed, window_end)?;
                }
                fresh = false;
            }
            Event::Again { route, key } => {
                task.again(route, key);
                task.send(manifest.clone()).await;
                fresh = true;
            }
            Event::Cancel => return Ok(Some(End::Cancelled)),
            _ => debug!("dropped an event a download doesn't take"),
        }
    }
}

/// Passes a download from a container's guest to the client, and the
/// client's credit back to the guest.
pub(super) async fn relay(task: &mut Task, link: GuestLink, begin: HostToGuest) -> Option<End> {
    match from_guest(task, &link, begin).await {
        Ok(end) => end,
        Err(failure) => Some(End::refused(failure)),
    }
}

async fn from_guest(
    task: &mut Task,
    link: &GuestLink,
    begin: HostToGuest,
) -> Result<Option<End>, Failure> {
    let transfer = task.transfer;
    link.message(&begin)
        .await
        .map_err(|_| push::unreachable())?;
    let mut known: Option<Vec<ManifestEntry>> = None;
    let mut total = 0;
    // Chunks wait for a manifest, so ones still on their way from before a
    // begin sent again don't reach the client.
    let mut waiting = true;
    let mut granted = 0;
    let fail = |message: &str| Failure::new(ErrorCode::BadMessage, None, message);
    loop {
        let event = match task.next(None).await {
            Next::Idle | Next::Tick => return Ok(None),
            Next::Event(event) => event,
        };
        match event {
            Event::Guest(FromGuest::Manifest(entries)) => {
                let checked = manifest::check(&entries).and_then(|_| match &begin {
                    HostToGuest::FetchFiles { paths, .. } => manifest::only_chosen(&entries, paths),
                    _ => Ok(()),
                });
                if let Err(failure) = checked {
                    link.cancel(transfer).await;
                    return Err(failure);
                }
                if known.as_ref().is_some_and(|known| *known != entries) {
                    link.cancel(transfer).await;
                    return Err(fail(
                        "the files in the container changed during the download",
                    ));
                }
                total = Layout::of(&entries).total();
                task.send(ServerMessage::FetchManifest {
                    transfer,
                    entries: entries.clone(),
                })
                .await;
                known = Some(entries);
                waiting = false;
                if total == 0 {
                    return Ok(Some(End::Fetched));
                }
            }
            Event::ClientCredit {
                committed,
                window_end,
            } => {
                if waiting {
                    debug!(%transfer, "dropped credit for a download with no manifest yet");
                    continue;
                }
                if committed > window_end || window_end > total {
                    link.cancel(transfer).await;
                    return Err(fail("the client sent credit past the end of the download"));
                }
                granted = granted.max(window_end);
                let credit = HostToGuest::TransferCredit {
                    transfer,
                    committed,
                    window_end,
                };
                link.message(&credit)
                    .await
                    .map_err(|_| push::unreachable())?;
                if committed == total {
                    return Ok(Some(End::Fetched));
                }
            }
            Event::GuestChunk(chunk) => {
                if waiting {
                    continue;
                }
                let end = chunk.offset.saturating_add(chunk.data.len() as u64);
                if end > granted {
                    link.cancel(transfer).await;
                    return Err(fail("the container sent more than the client allowed"));
                }
                if !task.chunks(vec![(chunk.offset, chunk.data)]).await {
                    debug!(%transfer, "a download's client went away");
                }
            }
            Event::Guest(FromGuest::Failed(failure)) => return Err(failure),
            Event::GuestDown => return Err(push::unreachable()),
            Event::Again { route, key } => {
                task.again(route, key);
                link.message(&begin)
                    .await
                    .map_err(|_| push::unreachable())?;
                waiting = true;
                granted = 0;
            }
            Event::Cancel => {
                link.cancel(transfer).await;
                return Ok(Some(End::Cancelled));
            }
            Event::Guest(FromGuest::Credit { .. } | FromGuest::Stored(_))
            | Event::ClientChunk(_) => debug!("dropped an event a download doesn't take"),
        }
    }
}
