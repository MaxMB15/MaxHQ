// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Files an agent sends with MaxHQ's send tool. Each one is copied out of
//! the agent's folders into staging on this device, where its preview is
//! made, and then moved into the agent's attachments folder as a delivery
//! would. A file in a container comes over the guest link as a download
//! does, and goes back into the container's attachments folder. What the
//! `@` menu finds in a container's folder comes out the same way, and waits
//! in staging for the message it goes with.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

use maxhq_protocol::guest::HostToGuest;
use maxhq_protocol::ids::{AttachmentId, TransferId, WorkspaceId};
use maxhq_protocol::message::{ErrorCode, ManifestEntry};
use maxhq_protocol::timeline::AttachmentRef;
use maxhq_transfer::manifest::{self, Kind, Layout};
use maxhq_transfer::send::RESTARTS;
use maxhq_transfer::{Failure, Source};
use tracing::debug;

use super::disk::{self, remove, ticker, Land};
use super::push;
use super::registry::{Chunk, Peer};
use super::{guest_path, Event, FromGuest, GuestLink, Next, Task, Transfers, STALL, WRITE_BATCH};
use crate::drive::{Place, PlaceHost};
use maxhq_core::tools::{too_big, Page, MAX_VISUAL_BYTES};

impl Transfers {
    /// Copies the files at `paths` out of the agent's folders for the tool
    /// call `call`, and gives them as the agent's attachments. Every file
    /// is copied before any is moved, so one that's refused sends none.
    pub(super) async fn collect_files(
        &self,
        place: &Place,
        call: &str,
        paths: &[String],
    ) -> Result<Vec<AttachmentRef>, String> {
        let ids: Vec<AttachmentId> = (0..paths.len())
            .map(|index| sent_id(place, call, index))
            .collect();
        let mut staged = Vec::with_capacity(paths.len());
        for (path, id) in paths.iter().zip(&ids) {
            match self.pull(place, *id, path, Takes::File).await {
                Ok(found) => staged.push(found),
                Err(failure) => {
                    for id in &ids {
                        remove(&self.staged_dir(*id)).await;
                    }
                    return Err(format!("{path}: {}", failure.message));
                }
            }
        }
        let mut sent = Vec::with_capacity(staged.len());
        for found in staged {
            let mut moved = match self.deliver_one(place, found.id).await {
                Ok(moved) => moved.attachment,
                Err(err) => {
                    for id in &ids {
                        remove(&self.staged_dir(*id)).await;
                    }
                    return Err(err.to_string());
                }
            };
            moved.thumbnail = found.thumbnail;
            sent.push(moved);
        }
        Ok(sent)
    }

    /// Copies the file or folder the `@` menu found at `path` out of the
    /// agent's folder into staging as `id`.
    pub(super) async fn stage_found(
        &self,
        place: &Place,
        id: AttachmentId,
        path: &str,
    ) -> Result<AttachmentRef, String> {
        self.pull(place, id, path, Takes::FileOrFolder)
            .await
            .map_err(|failure| format!("{path}: {}", failure.message))
    }

    /// Copies a page the show tool shows into the agent's attachments as
    /// `name`, and gives it with the page's text. A file comes out of the
    /// agent's folders as the send tool's do. A call run again keeps the
    /// same ID.
    pub(super) async fn show_page(
        &self,
        place: &Place,
        call: &str,
        page: Page,
        name: &str,
    ) -> Result<(AttachmentRef, String), String> {
        let id = AttachmentId::derive(&format!("visual:{}:{call}", place.attachments.display()));
        let staged = match page {
            Page::Path(path) => self
                .pull_as(place, id, &path, Takes::Page, Some(name))
                .await
                .map_err(|failure| format!("{path}: {}", failure.message)),
            Page::Html(html) if html.len() > MAX_VISUAL_BYTES => Err(too_big(html.len() as u64)),
            Page::Html(html) => {
                let name = name.to_owned();
                self.settle(id, move |into| fs::write(into.join(name), html))
                    .await
                    .map_err(|failure| failure.message)
            }
        };
        let staged = staged?;
        let read = std::path::PathBuf::from(&staged.path);
        let text = disk::blocking(move || {
            fs::read(&read)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .map_err(|err| Failure::io(&err, None))
        })
        .await;
        let text = match text {
            Ok(text) => text,
            Err(failure) => {
                remove(&self.staged_dir(id)).await;
                return Err(failure.message);
            }
        };
        match self.deliver_one(place, id).await {
            Ok(moved) => {
                let mut shown = moved.attachment;
                shown.thumbnail = staged.thumbnail;
                Ok((shown, text))
            }
            Err(err) => {
                remove(&self.staged_dir(id)).await;
                Err(err.to_string())
            }
        }
    }

    fn staged_dir(&self, id: AttachmentId) -> std::path::PathBuf {
        self.shared.root.join("staged").join(id.to_string())
    }

    /// Copies what `takes` allows into staging as `id`, with its preview.
    async fn pull(
        &self,
        place: &Place,
        id: AttachmentId,
        path: &str,
        takes: Takes,
    ) -> Result<AttachmentRef, Failure> {
        self.pull_as(place, id, path, takes, None).await
    }

    /// [`Self::pull`], renaming the file `name` when one's given.
    async fn pull_as(
        &self,
        place: &Place,
        id: AttachmentId,
        path: &str,
        takes: Takes,
        name: Option<&str>,
    ) -> Result<AttachmentRef, Failure> {
        let transfer = TransferId::generate();
        let incoming = self.shared.incoming(transfer);
        let landed = match place.host {
            PlaceHost::Local => self.pull_local(place, &incoming, path, takes).await,
            PlaceHost::Guest { workspace } => {
                self.pull_guest(place, workspace, transfer, &incoming, path, takes)
                    .await
            }
        };
        let staged = match landed {
            Ok(land) => self.stage(&incoming, &land, id, name).await,
            Err(failure) => Err(failure),
        };
        remove(&incoming).await;
        staged
    }

    /// Copies a file from a local agent's folders.
    async fn pull_local(
        &self,
        place: &Place,
        incoming: &Path,
        path: &str,
        takes: Takes,
    ) -> Result<Land, Failure> {
        let roots = vec![place.cwd.clone(), place.attachments.clone()];
        let path = path.to_owned();
        let found = disk::blocking(move || disk::resolve(&roots, &path)).await?;
        let walk = disk::walk(found).await?;
        takes.check(&walk.entries)?;
        let shared = self.shared.clone();
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
        }
        Ok(land)
    }

    /// Downloads a file from an agent's folders in its container.
    async fn pull_guest(
        &self,
        place: &Place,
        workspace: WorkspaceId,
        transfer: TransferId,
        incoming: &Path,
        path: &str,
        takes: Takes,
    ) -> Result<Land, Failure> {
        let inside = guest_path(place, path)?;
        // The guest keeps a download inside the container's own folders,
        // and this keeps it inside the agent's.
        let wanted = Path::new(&inside);
        if !wanted.starts_with(&place.cwd) && !wanted.starts_with(&place.attachments) {
            return Err(Failure::new(
                ErrorCode::BadPath,
                None,
                "the path is outside the agent's folders",
            ));
        }
        let link = self.shared.link(workspace).ok_or_else(push::unreachable)?;
        let inbox = self
            .shared
            .open
            .open(transfer, vec![Peer::Guest(workspace)])
            .ok_or_else(|| Failure::new(ErrorCode::Internal, None, "the copy couldn't start"))?;
        let mut task = Task {
            shared: self.shared.clone(),
            inbox,
            route: None,
            key: None,
            transfer,
            stash: None,
        };
        let begin = HostToGuest::FetchBegin {
            transfer,
            path: inside,
        };
        link.message(&begin)
            .await
            .map_err(|_| push::unreachable())?;
        let landed = self
            .land_from_guest(&mut task, &link, incoming, takes)
            .await;
        if landed.is_err() {
            link.cancel(transfer).await;
        }
        landed
    }

    async fn land_from_guest(
        &self,
        task: &mut Task,
        link: &GuestLink,
        incoming: &Path,
        takes: Takes,
    ) -> Result<Land, Failure> {
        let entries = loop {
            match task.next_within(STALL).await {
                Some(Event::Guest(FromGuest::Manifest(entries))) => break entries,
                Some(Event::Guest(FromGuest::Failed(failure))) => return Err(failure),
                Some(Event::GuestDown) => return Err(push::unreachable()),
                None => return Err(stalled()),
                Some(_) => debug!("dropped an event a send doesn't take"),
            }
        };
        takes.check(&entries)?;
        let shared = self.shared.clone();
        disk::check_quota(
            shared.root.clone(),
            incoming.to_owned(),
            Layout::of(&entries).total(),
            shared.quota,
        )
        .await?;
        let mut land = Land::open(incoming.to_owned(), entries, shared.room).await?;
        credit(link, task.transfer, land.resume().await?).await?;
        let mut tick = ticker();
        let mut heard = Instant::now();
        while !land.is_done() {
            match task.next(Some(&mut tick)).await {
                Next::Idle => return Err(stalled()),
                Next::Tick => {
                    if heard.elapsed() >= STALL {
                        return Err(stalled());
                    }
                    if let Some(moved) = land.tick().await? {
                        credit(link, task.transfer, moved).await?;
                    }
                }
                Next::Event(Event::GuestChunk(first)) => {
                    heard = Instant::now();
                    let chunks = guest_chunks(task, first);
                    if let Some(moved) = land.write(chunks).await? {
                        credit(link, task.transfer, moved).await?;
                    }
                }
                Next::Event(Event::Guest(FromGuest::Failed(failure))) => return Err(failure),
                Next::Event(Event::GuestDown) => return Err(push::unreachable()),
                Next::Event(_) => debug!("dropped an event a send doesn't take"),
            }
        }
        // Credit for all of it lets the guest's download end.
        credit(link, task.transfer, land.resume().await?).await?;
        Ok(land)
    }

    /// Moves a landed file into staging as `id`, named `name` when one's
    /// given, and describes it with its preview. A call run again replaces
    /// what it staged before.
    async fn stage(
        &self,
        incoming: &Path,
        land: &Land,
        id: AttachmentId,
        name: Option<&str>,
    ) -> Result<AttachmentRef, Failure> {
        let tops = land.tops();
        let [top] = tops.as_slice() else {
            return Err(Failure::new(
                ErrorCode::Internal,
                None,
                "the copy landed with more than one thing at its top",
            ));
        };
        let from = incoming.join(top);
        let to = name.map_or_else(|| top.clone(), Into::into);
        self.settle(id, move |into| fs::rename(&from, into.join(to)))
            .await
    }

    /// Empties `id`'s staging folder, lets `put` place the file in it, and
    /// describes the file with its preview.
    async fn settle(
        &self,
        id: AttachmentId,
        put: impl FnOnce(&Path) -> io::Result<()> + Send + 'static,
    ) -> Result<AttachmentRef, Failure> {
        let into = self.staged_dir(id);
        let shared = self.shared.clone();
        disk::blocking(move || {
            let fail = |err: io::Error| Failure::io(&err, None);
            match maxhq_transfer::remove_tree(&into) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(fail(err)),
            }
            fs::create_dir_all(&into).map_err(fail)?;
            put(&into).map_err(fail)?;
            let mut found = shared.store.find(id).ok_or_else(|| {
                Failure::new(
                    ErrorCode::Internal,
                    None,
                    "the copy landed but can't be found in staging",
                )
            })?;
            found.thumbnail = shared.store.make_sidecars(&found).thumbnail;
            Ok(found)
        })
        .await
    }
}

/// The ID of the `index`th file a tool call sends. It's the same each time
/// the call runs.
fn sent_id(place: &Place, call: &str, index: usize) -> AttachmentId {
    AttachmentId::derive(&format!(
        "sent:{}:{call}:{index}",
        place.attachments.display()
    ))
}

/// What a copy out of an agent's folders takes.
#[derive(Clone, Copy)]
enum Takes {
    /// One file, as the send tool sends.
    File,
    /// One file or folder, as the `@` menu finds.
    FileOrFolder,
    /// One page no bigger than a visual holds, as the show tool shows.
    Page,
}

impl Takes {
    /// Refuses a link on its own, and a folder unless this takes one.
    fn check(self, entries: &[ManifestEntry]) -> Result<(), Failure> {
        let checked = manifest::check(entries)?;
        match (self, manifest::tops(&checked).as_slice()) {
            (_, [top]) if matches!(top.kind, Kind::Symlink { .. }) => Err(Failure::new(
                ErrorCode::BadPath,
                None,
                "the path is a link, which MaxHQ won't follow",
            )),
            (Self::Page, _) if Layout::of(entries).total() > MAX_VISUAL_BYTES as u64 => {
                Err(Failure::new(
                    ErrorCode::TooLarge,
                    None,
                    too_big(Layout::of(entries).total()),
                ))
            }
            (_, [top]) if matches!(top.kind, Kind::File { .. }) => Ok(()),
            (Self::FileOrFolder, [_]) => Ok(()),
            (Self::File | Self::Page, _) => Err(Failure::new(
                ErrorCode::BadPath,
                None,
                "the path is a folder. Send the files in it instead",
            )),
            (Self::FileOrFolder, _) => Err(Failure::new(
                ErrorCode::BadPath,
                None,
                "the path holds more than one file or folder",
            )),
        }
    }
}

fn stalled() -> Failure {
    Failure::new(
        ErrorCode::Unreachable,
        None,
        "the container stopped sending the file",
    )
}

/// Sends the guest credit for a download this device lands.
async fn credit(
    link: &GuestLink,
    transfer: TransferId,
    (committed, window_end): (u64, u64),
) -> Result<(), Failure> {
    let credit = HostToGuest::TransferCredit {
        transfer,
        committed,
        window_end,
    };
    link.message(&credit).await.map_err(|_| push::unreachable())
}

/// `first` and the guest's chunks that wait after it, up to
/// [`WRITE_BATCH`] bytes.
fn guest_chunks(task: &mut Task, first: Chunk) -> Vec<(u64, Vec<u8>)> {
    let mut len = first.data.len();
    let mut chunks = vec![(first.offset, first.data)];
    while len < WRITE_BATCH && task.stash.is_none() {
        match task.inbox.try_recv() {
            Some(Event::GuestChunk(chunk)) => {
                len += chunk.data.len();
                chunks.push((chunk.offset, chunk.data));
            }
            Some(other) => task.stash = Some(other),
            None => break,
        }
    }
    chunks
}
