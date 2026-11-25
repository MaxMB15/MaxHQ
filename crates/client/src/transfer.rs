// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! File transfers over a device link: uploads of files, folders and pasted
//! items to a device, and downloads of a file or folder an agent can see.
//!
//! The future that starts a transfer drives it, and the link's
//! [`Transfers`] hands it the messages and chunks that name it. A transfer
//! lives through a dropped connection. Once the link is back it sends its
//! `UploadBegin` or `FetchBegin` again under the same key and transfer, and
//! the far end's credit says where to pick up.
//!
//! An upload of a file or folder first copies it into a staging folder of
//! the client's own, as a clone where the disk can make one, so edits they
//! make while it's on its way don't break its hashes. A download lands
//! under `.incoming` in the downloads folder, and moves into place under a
//! free name once every file has checked out. A picture or video for the
//! timeline lands the same way in the media cache, as [`crate::media`]
//! tells. A copy to hand on to another device lands in a staging folder
//! of its own, which goes once the copy is done with.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::frame;
use maxhq_protocol::ids::{AgentId, IdempotencyKey, TransferId, WorkspaceId};
use maxhq_protocol::message::{
    ActionOutcome, Capabilities, EntryKind, ErrorCode, ManifestEntry, UploadSource, UploadTarget,
};
use maxhq_protocol::skills::TreeHash;
use maxhq_protocol::{ClientMessage, ServerMessage};
use maxhq_transfer::manifest::{self, Layout};
use maxhq_transfer::receive::sha256_hex;
use maxhq_transfer::send::RESTARTS;
use maxhq_transfer::{local, name, walk, Failure, Outgoing, Receiver, Room, Source, Walk};
use maxhq_wire::{Queue, Window};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::debug;

use crate::media::{self, Media};
use crate::LinkError;

/// How often a transfer looks again at whether it's waiting.
const TICK: Duration = Duration::from_millis(250);

/// The least time between two reports of bytes moving.
const EVERY: Duration = Duration::from_millis(100);

/// How long an upload goes without credit before it shows as waiting.
const WAITING: Duration = Duration::from_secs(1);

/// The most file data that waits for a download to write it. The device
/// sends no more than its credit, so passing this means it broke the
/// rules.
const QUEUED: u64 = 2 * Window::MAX + frame::MAX_TRANSFER_CHUNK as u64;

/// The most file data a download writes in one go.
const BATCH: usize = 1024 * 1024;

/// How long a staged copy or a partial download may sit before a later
/// transfer clears it away, such as after a crash.
const LEFT_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// The folder under the downloads folder that downloads land in first.
const INCOMING: &str = ".incoming";

/// How many names a download tries when others take them as it moves in.
const SETTLE_TRIES: usize = 8;

/// How far one transfer got, for a progress bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TransferProgress {
    /// The transfer, which [`crate::LinkHandle::cancel_transfer`] takes.
    pub transfer: TransferId,
    /// Bytes on their way so far.
    pub sent: u64,
    /// Bytes in the whole transfer, or 0 before it's known.
    pub total: u64,
    /// The file on its way now, as a path inside the transfer.
    pub file: String,
    /// What the transfer is doing.
    pub state: TransferState,
}

/// What a transfer is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Bytes are moving.
    Uploading,
    /// The far end has no room yet, so bytes wait for its credit.
    Waiting,
    /// The link dropped. It picks up again once the link is back.
    Paused,
    /// Every byte arrived.
    Done,
    /// It stopped for good.
    Failed,
}

/// Why a transfer didn't finish.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TransferError {
    /// The link couldn't carry it.
    #[error(transparent)]
    Link(#[from] LinkError),
    /// The device doesn't take uploads or serve downloads.
    #[error("the device doesn't do file transfers")]
    Unsupported,
    /// It stopped on this machine or on the device.
    #[error("{message}")]
    Failed {
        /// What went wrong, for code to act on.
        code: ErrorCode,
        /// What went wrong, for a person to read.
        message: String,
    },
    /// They cancelled it.
    #[error("cancelled")]
    Cancelled,
}

impl From<Failure> for TransferError {
    fn from(failure: Failure) -> Self {
        Self::Failed {
            code: failure.code,
            message: failure.message,
        }
    }
}

/// Where transfers keep files on this machine.
#[derive(Clone, Debug)]
pub(crate) struct Places {
    /// Where downloads land, `~/Downloads/MaxHQ`.
    pub downloads: PathBuf,
    /// Where uploads are copied before they go, and copies fetched to hand
    /// on land, a folder for each.
    pub staging: PathBuf,
    /// Where pictures and videos for the timeline are kept.
    pub media: PathBuf,
    /// The free disk a download leaves.
    pub room: Room,
}

impl Places {
    /// The usual folders for the user running this process. Nothing is
    /// made until a transfer needs it.
    pub fn new() -> Self {
        let base = directories::BaseDirs::new();
        let downloads = directories::UserDirs::new()
            .and_then(|dirs| dirs.download_dir().map(Path::to_owned))
            .or_else(|| base.as_ref().map(|base| base.home_dir().join("Downloads")))
            .unwrap_or_else(std::env::temp_dir)
            .join("MaxHQ");
        let app = if cfg!(target_os = "macos") {
            "MaxHQ"
        } else {
            "maxhq"
        };
        let cache = base
            .map(|base| base.cache_dir().join(app))
            .unwrap_or_else(|| std::env::temp_dir().join(app));
        Self {
            downloads,
            staging: cache.join("staging"),
            media: cache.join("media"),
            room: Room::DEFAULT,
        }
    }
}

/// What an upload sends.
pub(crate) enum Item {
    /// A file or folder on this machine.
    Path(PathBuf),
    /// One file's bytes held in memory, such as a pasted picture.
    Bytes {
        /// The name it shows as.
        name: String,
        /// Its media type, which gives a name with no extension one.
        mime: String,
        /// The bytes.
        bytes: Vec<u8>,
    },
}

/// What reaches a transfer from its link.
enum Event {
    /// The link connected, with the connection's writer.
    Up(maxhq_wire::Sender),
    /// The link dropped.
    Down,
    /// The link closed for good.
    Shut,
    /// They cancelled it.
    Cancel,
    /// A message that names it.
    Message(ServerMessage),
    /// File data for a download.
    Chunk { offset: u64, data: Vec<u8> },
    /// More file data came than a download's credit allows.
    Flooded,
}

/// The uploads and downloads in flight on one link. The link's task tells
/// it when the connection comes and goes, and hands it what names a
/// transfer.
pub(crate) struct Transfers {
    places: Places,
    live: Mutex<Live>,
}

#[derive(Default)]
struct Live {
    /// The connection's writer and what the device does, while connected.
    wire: Option<(maxhq_wire::Sender, Capabilities)>,
    /// Whether the link closed for good.
    shut: bool,
    entries: HashMap<TransferId, Entry>,
    /// The transfer each key belongs to, for their action results.
    keys: HashMap<IdempotencyKey, TransferId>,
    /// Cancels that wait for the link to come back.
    cancels: Vec<TransferId>,
}

/// One transfer, as the registry sees it.
struct Entry {
    key: IdempotencyKey,
    download: bool,
    events: mpsc::UnboundedSender<Event>,
    /// Bytes of file data waiting in `events`.
    queued: Arc<AtomicU64>,
    flooded: bool,
}

impl Transfers {
    /// No transfers yet, keeping files in `places`.
    pub fn new(places: Places) -> Self {
        Self {
            places,
            live: Mutex::new(Live::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hands every transfer the new connection's writer, so each sends its
    /// begin again, and sends the cancels that waited for it.
    pub fn connected(&self, sender: maxhq_wire::Sender, capabilities: Capabilities) {
        let mut live = self.lock();
        for transfer in std::mem::take(&mut live.cancels) {
            try_control(&sender, &ClientMessage::UploadCancel { transfer });
        }
        for entry in live.entries.values() {
            let _ = entry.events.send(Event::Up(sender.clone()));
        }
        live.wire = Some((sender, capabilities));
    }

    /// Tells every transfer the connection dropped, so each shows paused.
    pub fn dropped(&self) {
        let mut live = self.lock();
        live.wire = None;
        for entry in live.entries.values() {
            let _ = entry.events.send(Event::Down);
        }
    }

    /// Ends every transfer, since the link closed for good.
    pub fn shut(&self) {
        let mut live = self.lock();
        live.shut = true;
        live.wire = None;
        for entry in live.entries.values() {
            let _ = entry.events.send(Event::Shut);
        }
    }

    /// Takes a message that names a transfer in flight, and hands back any
    /// other.
    pub fn take(&self, message: ServerMessage) -> Option<ServerMessage> {
        let live = self.lock();
        let transfer = match &message {
            ServerMessage::UploadDone { transfer, .. }
            | ServerMessage::FetchManifest { transfer, .. }
            | ServerMessage::TransferCredit { transfer, .. }
            | ServerMessage::TransferFailed { transfer, .. } => *transfer,
            ServerMessage::ActionResult { key, .. } => match live.keys.get(key) {
                Some(transfer) => *transfer,
                None => return Some(message),
            },
            _ => return Some(message),
        };
        match live.entries.get(&transfer) {
            Some(entry) => {
                let _ = entry.events.send(Event::Message(message));
                None
            }
            None => Some(message),
        }
    }

    /// Hands a download a chunk of its file data.
    pub fn chunk(&self, transfer: TransferId, offset: u64, data: &[u8]) {
        let mut live = self.lock();
        let Some(entry) = live
            .entries
            .get_mut(&transfer)
            .filter(|entry| entry.download)
        else {
            debug!(%transfer, "file data for no download");
            return;
        };
        if entry.flooded {
            return;
        }
        let len = data.len() as u64;
        if entry.queued.fetch_add(len, Ordering::Relaxed) + len > QUEUED {
            entry.flooded = true;
            let _ = entry.events.send(Event::Flooded);
            return;
        }
        let _ = entry.events.send(Event::Chunk {
            offset,
            data: data.to_vec(),
        });
    }

    /// Cancels a transfer in flight. One that ended already is left be.
    pub fn cancel(&self, transfer: TransferId) {
        if let Some(entry) = self.lock().entries.get(&transfer) {
            let _ = entry.events.send(Event::Cancel);
        }
    }

    /// Adds a transfer under `key`, if the link is connected to a device
    /// that does transfers.
    fn register(
        &self,
        key: IdempotencyKey,
        download: bool,
    ) -> Result<(Ticket<'_>, maxhq_wire::Sender), TransferError> {
        let mut live = self.lock();
        if live.shut {
            return Err(LinkError::Closed.into());
        }
        let Some((sender, capabilities)) = &live.wire else {
            return Err(LinkError::NotConnected.into());
        };
        if !capabilities.transfers {
            return Err(TransferError::Unsupported);
        }
        let sender = sender.clone();
        let transfer = TransferId::generate();
        let (events_tx, events) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicU64::new(0));
        live.entries.insert(
            transfer,
            Entry {
                key,
                download,
                events: events_tx,
                queued: Arc::clone(&queued),
                flooded: false,
            },
        );
        live.keys.insert(key, transfer);
        let ticket = Ticket {
            transfers: self,
            transfer,
            events,
            queued,
            told: false,
            over: false,
            scratch: None,
        };
        Ok((ticket, sender))
    }

    /// Takes a transfer out, and tells the device to drop it unless it
    /// knows it's over.
    fn forget(&self, transfer: TransferId, cancel: bool) {
        let mut live = self.lock();
        if let Some(entry) = live.entries.remove(&transfer) {
            live.keys.remove(&entry.key);
        }
        if !cancel || live.shut {
            return;
        }
        match &live.wire {
            Some((sender, _)) => try_control(sender, &ClientMessage::UploadCancel { transfer }),
            None => live.cancels.push(transfer),
        }
    }

    /// Sends one file, folder or pasted item to the device and waits for
    /// what came of it.
    pub async fn upload(
        &self,
        target: UploadTarget,
        item: Item,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<ActionOutcome, TransferError> {
        let key = IdempotencyKey::generate();
        let (mut ticket, sender) = self.register(key, false)?;
        let shown = match &item {
            Item::Path(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Item::Bytes { name, .. } => name.clone(),
        };
        let mut progress = Progress::new(ticket.transfer, on_progress, shown);
        let result = self
            .send(&mut ticket, key, target, item, sender, &mut progress)
            .await;
        ticket.clean().await;
        progress.end(match result {
            Ok(ActionOutcome::Refused { .. } | ActionOutcome::UnknownAgent) | Err(_) => {
                TransferState::Failed
            }
            Ok(_) => TransferState::Done,
        });
        result
    }

    async fn send(
        &self,
        ticket: &mut Ticket<'_>,
        key: IdempotencyKey,
        target: UploadTarget,
        item: Item,
        sender: maxhq_wire::Sender,
        progress: &mut Progress<'_>,
    ) -> Result<ActionOutcome, TransferError> {
        let transfer = ticket.transfer;
        let mut sender = Some(sender);
        let Prepared {
            name,
            entries,
            source,
        } = match item {
            Item::Path(path) => self.stage(ticket, path, &mut sender).await?,
            Item::Bytes { name, mime, bytes } => pasted(&name, &mime, bytes)?,
        };
        let files = Files::new(&entries);
        let total = source.total();
        let begin = ClientMessage::UploadBegin {
            key,
            transfer,
            target,
            source: UploadSource::Stream { name, entries },
        };
        ticket.told = true;
        let source = Arc::new(Mutex::new(source));
        let mut outgoing = Outgoing::new(total);
        // Set from a begin until the credit that answers it, which says
        // where to pick up.
        let mut begun = false;
        if let Some(sender) = &sender {
            control(sender, &begin).await;
            begun = true;
        }
        let mut in_flight: Option<BoxFuture<'static, Sent>> = None;
        let mut stalled = Some(Instant::now());
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            if in_flight.is_none()
                && !begun
                && let Some(sender) = &sender
                && let Some((offset, len)) = outgoing.next(frame::MAX_TRANSFER_CHUNK)
            {
                in_flight = Some(send_chunk(
                    Arc::clone(&source),
                    sender.clone(),
                    transfer,
                    offset,
                    len,
                ));
            }
            if in_flight.is_some() || outgoing.is_sent() {
                stalled = None;
            } else {
                stalled.get_or_insert_with(Instant::now);
            }
            let state = if sender.is_none() {
                TransferState::Paused
            } else if stalled.is_some_and(|since| since.elapsed() >= WAITING) {
                TransferState::Waiting
            } else {
                TransferState::Uploading
            };
            progress.update(outgoing.sent(), total, files.at(outgoing.sent()), state);

            tokio::select! {
                sent = maybe(&mut in_flight) => {
                    in_flight = None;
                    let (offset, len) = sent
                        .map_err(|(offset, err)| Failure::io(&err, files.entry(offset)))?;
                    if offset == outgoing.sent() {
                        outgoing.sent_chunk(len);
                    }
                }
                event = ticket.events.recv() => match event {
                    Some(Event::Up(new)) => {
                        in_flight = None;
                        control(&new, &begin).await;
                        begun = true;
                        sender = Some(new);
                    }
                    Some(Event::Down) => {
                        in_flight = None;
                        sender = None;
                    }
                    Some(Event::Shut) | None => return Err(LinkError::Closed.into()),
                    Some(Event::Cancel) => return Err(TransferError::Cancelled),
                    Some(Event::Message(message)) => match message {
                        ServerMessage::TransferCredit {
                            committed,
                            window_end,
                            ..
                        } => {
                            if begun {
                                in_flight = None;
                                begun = false;
                                outgoing.resume(committed, window_end)?;
                            } else {
                                outgoing.credit(committed, window_end)?;
                            }
                        }
                        ServerMessage::UploadDone { attachment, .. } => {
                            ticket.over = true;
                            return Ok(ActionOutcome::Staged { attachment });
                        }
                        ServerMessage::ActionResult { outcome, .. } => {
                            ticket.over = true;
                            return Ok(outcome);
                        }
                        ServerMessage::TransferFailed {
                            code: ErrorCode::HashMismatch,
                            message,
                            ..
                        } => {
                            // The file changed between its hash and its
                            // bytes, so it goes again from where the
                            // device's credit says.
                            if !outgoing.restart() {
                                return Err(TransferError::Failed {
                                    code: ErrorCode::HashMismatch,
                                    message,
                                });
                            }
                            in_flight = None;
                            if let Some(sender) = &sender {
                                control(sender, &begin).await;
                            }
                            begun = true;
                        }
                        ServerMessage::TransferFailed { code, message, .. } => {
                            ticket.over = true;
                            return Err(TransferError::Failed { code, message });
                        }
                        other => debug!(?other, "a message an upload doesn't take"),
                    },
                    Some(Event::Chunk { .. } | Event::Flooded) => {}
                },
                _ = tick.tick() => {}
            }
        }
    }

    /// Copies a file or folder into its own staging folder and walks the
    /// copy, keeping track of the link meanwhile.
    async fn stage(
        &self,
        ticket: &mut Ticket<'_>,
        path: PathBuf,
        sender: &mut Option<maxhq_wire::Sender>,
    ) -> Result<Prepared, TransferError> {
        let staging = self.places.staging.clone();
        let into = staging.join(ticket.transfer.to_string());
        ticket.scratch = Some(into.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let mut staged = tokio::task::spawn_blocking(move || {
            prune(&staging);
            stage(&path, &into, &stopped)
        });
        let walk = loop {
            tokio::select! {
                staged = &mut staged => break joined(staged)?,
                event = ticket.events.recv() => {
                    let end = match event {
                        Some(Event::Up(new)) => {
                            *sender = Some(new);
                            continue;
                        }
                        Some(Event::Down) => {
                            *sender = None;
                            continue;
                        }
                        Some(Event::Cancel) => TransferError::Cancelled,
                        Some(Event::Shut) | None => LinkError::Closed.into(),
                        Some(_) => continue,
                    };
                    // The copy stops at its next file. It's done before
                    // its folder goes.
                    stop.store(true, Ordering::Relaxed);
                    let _ = staged.await;
                    return Err(end);
                }
            }
        };
        if !walk.skipped.is_empty() {
            debug!(
                skipped = walk.skipped.len(),
                "an upload left some things out"
            );
        }
        Ok(Prepared {
            source: Source::new(&walk),
            name: walk.name,
            entries: walk.entries,
        })
    }

    /// Fetches a file or folder an agent can see into the downloads folder
    /// and gives where it landed, with the links in it this machine
    /// couldn't make.
    pub async fn download(
        &self,
        agent: AgentId,
        path: String,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Saved, TransferError> {
        let shown = name::download_name(&path);
        let begin = fetch_begin(agent, path, false);
        let fetched = self
            .fetch_into(begin, shown, Destination::Downloads, on_progress)
            .await?;
        match fetched {
            Fetched::Saved(saved) => Ok(saved),
            _ => unreachable!("a download lands in the downloads folder"),
        }
    }

    /// Fetches a file or folder an agent can see into a staging folder of
    /// its own, to hand on, such as to stage it for another agent. The
    /// folder goes when the copy drops.
    pub async fn copy(
        &self,
        agent: AgentId,
        path: String,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Copied, TransferError> {
        let shown = name::download_name(&path);
        let begin = fetch_begin(agent, path, false);
        self.copy_into(begin, shown, on_progress).await
    }

    /// Fetches some files in a workspace's checkout, by their paths there,
    /// into a staging folder of their own, to hand on to another device.
    /// They come as one folder that holds them at those paths. The folder
    /// goes when the copy drops.
    pub async fn copy_files(
        &self,
        workspace: WorkspaceId,
        paths: Vec<String>,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Copied, TransferError> {
        let shown = paths.first().map(|path| name::download_name(path));
        let begin = move |key, transfer| ClientMessage::FetchFiles {
            key,
            transfer,
            workspace,
            paths,
        };
        self.copy_into(begin, shown.unwrap_or_default(), on_progress)
            .await
    }

    /// Fetches a tree in the device's skill store into a staging folder of
    /// its own, to hand on to another device's store. The folder goes when
    /// the copy drops.
    pub async fn copy_tree(
        &self,
        tree: TreeHash,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Copied, TransferError> {
        let shown = tree.0.clone();
        let begin = move |key, transfer| ClientMessage::FetchSkillTree {
            key,
            transfer,
            tree,
        };
        self.copy_into(begin, shown, on_progress).await
    }

    async fn copy_into(
        &self,
        begin: impl FnOnce(IdempotencyKey, TransferId) -> ClientMessage,
        shown: String,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Copied, TransferError> {
        match self
            .fetch_into(begin, shown, Destination::Copy, on_progress)
            .await?
        {
            Fetched::Copied(copied) => Ok(copied),
            _ => unreachable!("a copy lands in staging"),
        }
    }

    /// Fetches a picture or video an agent can see into the media cache,
    /// unless it's there already or is larger than `limit` bytes. A file
    /// the cache doesn't keep is refused before anything goes. `anyway`
    /// asks for one file outside the agent's folders, and only your click
    /// on Show anyway may set it. A path the device refuses comes back as
    /// [`Media::Refused`].
    pub async fn media(
        &self,
        agent: AgentId,
        path: String,
        limit: u64,
        anyway: bool,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Media, TransferError> {
        let Some(extension) = media::extension(&path) else {
            return Err(
                Failure::new(ErrorCode::NotAllowed, None, "that isn't a file MaxHQ shows").into(),
            );
        };
        let into = Destination::Media {
            extension,
            limit: limit.min(media::MOST),
        };
        let shown = name::download_name(&path);
        let begin = fetch_begin(agent, path, anyway);
        match self.fetch_into(begin, shown, into, on_progress).await {
            Ok(Fetched::Media(media)) => Ok(media),
            Ok(_) => unreachable!("media lands in the cache"),
            Err(TransferError::Failed {
                code: code @ (ErrorCode::Outside | ErrorCode::BadPath),
                message,
            }) => Ok(Media::Refused {
                outside: code == ErrorCode::Outside,
                message,
            }),
            Err(err) => Err(err),
        }
    }

    /// Fetches into `into` what `begin` asks the device for, showing
    /// `shown` until the first file comes.
    async fn fetch_into(
        &self,
        begin: impl FnOnce(IdempotencyKey, TransferId) -> ClientMessage,
        shown: String,
        into: Destination,
        on_progress: &(dyn Fn(TransferProgress) + Send + Sync),
    ) -> Result<Fetched, TransferError> {
        let key = IdempotencyKey::generate();
        let (mut ticket, sender) = self.register(key, true)?;
        let mut progress = Progress::new(ticket.transfer, on_progress, shown);
        let begin = begin(key, ticket.transfer);
        let result = self
            .fetch(&mut ticket, begin, into, sender, &mut progress)
            .await;
        ticket.clean().await;
        progress.end(if result.is_ok() {
            TransferState::Done
        } else {
            TransferState::Failed
        });
        result
    }

    async fn fetch(
        &self,
        ticket: &mut Ticket<'_>,
        begin: ClientMessage,
        into: Destination,
        sender: maxhq_wire::Sender,
        progress: &mut Progress<'_>,
    ) -> Result<Fetched, TransferError> {
        let transfer = ticket.transfer;
        let folder = match into {
            Destination::Downloads => self.places.downloads.join(INCOMING),
            Destination::Media { .. } => self.places.media.join(INCOMING),
            // A folder of its own, as an upload's copy has.
            Destination::Copy => self.places.staging.clone(),
        };
        let root = folder.join(transfer.to_string());
        // Where a picture or video goes in the cache, once its manifest
        // says what it is.
        let mut cached: Option<(PathBuf, u64)> = None;
        ticket.scratch = Some(root.clone());
        ticket.told = true;
        control(&sender, &begin).await;
        let mut sender = Some(sender);
        let mut landing: Option<Landing> = None;
        let mut restarts = 0;
        // An event taken while gathering chunks, to act on next.
        let mut next: Option<Event> = None;
        let mut tick = tokio::time::interval(maxhq_transfer::SYNC_EVERY);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            if let Some(landed) = landing.as_ref().filter(|landing| landing.done) {
                // Every byte is in, so the device is done with it too.
                ticket.over = true;
                let from = root.join(&landed.top);
                let skipped_links = landed.skipped_links.clone();
                if !skipped_links.is_empty() {
                    debug!(
                        skipped = skipped_links.len(),
                        "this machine couldn't make some links in a download"
                    );
                }
                if let Some((to, size)) = cached.take() {
                    let folder = self.places.media.clone();
                    let name = blocking(move || {
                        keep(&from, &to, &folder).map_err(|err| Failure::io(&err, None))
                    })
                    .await?;
                    return Ok(Fetched::Media(Media::Cached { name, size }));
                }
                if let Destination::Copy = into {
                    // The copy keeps its folder until it drops.
                    ticket.scratch = None;
                    let copied = Copied::new(root, from).skipping(skipped_links);
                    return Ok(Fetched::Copied(copied));
                }
                let top = landed.top.clone();
                let downloads = self.places.downloads.clone();
                let path = blocking(move || {
                    settle(&from, &top, &downloads).map_err(|err| Failure::io(&err, None))
                })
                .await?;
                return Ok(Fetched::Saved(Saved {
                    path,
                    skipped_links,
                }));
            }
            let state = if sender.is_none() {
                TransferState::Paused
            } else {
                TransferState::Uploading
            };
            match &landing {
                Some(landing) => progress.update(
                    landing.written,
                    landing.files.total(),
                    landing.files.at(landing.written),
                    state,
                ),
                None => {
                    let file = progress.now.file.clone();
                    progress.update(0, 0, &file, state);
                }
            }

            let event = match next.take() {
                Some(event) => event,
                None => tokio::select! {
                    event = ticket.events.recv() => match event {
                        Some(event) => event,
                        None => return Err(LinkError::Closed.into()),
                    },
                    _ = tick.tick() => {
                        if let Some(landing) = &mut landing {
                            let credit = landing
                                .disk(|receiver| {
                                    let now = std::time::Instant::now();
                                    receiver.sync_if_due(now)?;
                                    Ok(receiver.credit(now))
                                })
                                .await?;
                            if let (Some(credit), Some(sender)) = (credit, &sender) {
                                control(sender, &credit_message(transfer, credit)).await;
                            }
                        }
                        continue;
                    }
                },
            };
            match event {
                Event::Up(new) => {
                    control(&new, &begin).await;
                    sender = Some(new);
                }
                Event::Down => sender = None,
                Event::Shut => return Err(LinkError::Closed.into()),
                Event::Cancel => return Err(TransferError::Cancelled),
                Event::Flooded => {
                    return Err(Failure::new(
                        ErrorCode::BadMessage,
                        None,
                        "the device sent more than its credit",
                    )
                    .into());
                }
                Event::Message(ServerMessage::FetchManifest { entries, .. }) => {
                    // What lands here goes on into another checkout, so a
                    // device gets to send only what the owner picked.
                    if let ClientMessage::FetchFiles { paths, .. } = &begin {
                        manifest::only_chosen(&entries, paths)?;
                    }
                    if let Destination::Media { extension, limit } = into {
                        let (to, size) = in_cache(&entries, &self.places.media, extension)?;
                        if let Some(name) = blocking({
                            let to = to.clone();
                            move || Ok(hit(&to, size))
                        })
                        .await?
                        {
                            return Ok(Fetched::Media(Media::Cached { name, size }));
                        }
                        if size > limit {
                            return Ok(Fetched::Media(Media::TooLarge { size }));
                        }
                        cached = Some((to, size));
                    }
                    let mut landed = match landing.take() {
                        Some(landed) if landed.manifest == entries => landed,
                        // A first manifest, or one for a file that changed
                        // since, which starts again from nothing.
                        _ => Landing::open(&root, entries, self.places.room).await?,
                    };
                    let credit = landed
                        .disk(|receiver| receiver.resume(std::time::Instant::now()))
                        .await?;
                    landing = Some(landed);
                    if let Some(sender) = &sender {
                        control(sender, &credit_message(transfer, credit)).await;
                    }
                }
                Event::Message(ServerMessage::ActionResult { outcome, .. }) => match outcome {
                    // Every byte is on its way, and some may still come.
                    ActionOutcome::Done => {}
                    ActionOutcome::Refused { reason } => {
                        ticket.over = true;
                        return Err(TransferError::Failed {
                            code: ErrorCode::NotAllowed,
                            message: reason,
                        });
                    }
                    ActionOutcome::UnknownAgent => {
                        ticket.over = true;
                        return Err(TransferError::Failed {
                            code: ErrorCode::NoSuchAgent,
                            message: "no such agent".into(),
                        });
                    }
                    other => debug!(?other, "an outcome a download doesn't take"),
                },
                Event::Message(ServerMessage::TransferFailed { code, message, .. }) => {
                    ticket.over = true;
                    return Err(TransferError::Failed { code, message });
                }
                Event::Message(other) => debug!(?other, "a message a download doesn't take"),
                Event::Chunk { offset, data } => {
                    let mut bytes = data.len();
                    let mut batch = vec![(offset, data)];
                    while bytes < BATCH {
                        match ticket.events.try_recv() {
                            Ok(Event::Chunk { offset, data }) => {
                                bytes += data.len();
                                batch.push((offset, data));
                            }
                            Ok(other) => {
                                next = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    ticket.queued.fetch_sub(bytes as u64, Ordering::Relaxed);
                    let Some(landed) = &mut landing else {
                        debug!(%transfer, "file data before its manifest");
                        continue;
                    };
                    let written = landed
                        .disk(move |receiver| {
                            let now = std::time::Instant::now();
                            for (offset, data) in batch {
                                receiver.write(offset, &data, now)?;
                            }
                            Ok(receiver.credit(now))
                        })
                        .await;
                    match written {
                        Ok(credit) => {
                            if let (Some(credit), Some(sender)) = (credit, &sender) {
                                control(sender, &credit_message(transfer, credit)).await;
                            }
                        }
                        // The file changed on the device as it went, so
                        // ask for it again. The new manifest's credit says
                        // where to pick up.
                        Err(failure)
                            if failure.code == ErrorCode::HashMismatch && restarts < RESTARTS =>
                        {
                            restarts += 1;
                            if let Some(sender) = &sender {
                                control(sender, &begin).await;
                            }
                        }
                        Err(failure) => return Err(failure.into()),
                    }
                }
            }
        }
    }
}

/// What asks for a file or folder an agent can see, or with `anyway` for
/// one file outside its folders.
fn fetch_begin(
    agent: AgentId,
    path: String,
    anyway: bool,
) -> impl FnOnce(IdempotencyKey, TransferId) -> ClientMessage {
    move |key, transfer| ClientMessage::FetchBegin {
        key,
        transfer,
        agent,
        path,
        anyway,
    }
}

/// A transfer in the registry, taken out again when it drops. One that
/// drops before the device knows it's over gets a cancel.
struct Ticket<'a> {
    transfers: &'a Transfers,
    transfer: TransferId,
    events: mpsc::UnboundedReceiver<Event>,
    /// Bytes of file data waiting in `events`.
    queued: Arc<AtomicU64>,
    /// Whether its begin went out, or waits for the link to.
    told: bool,
    /// Whether the device knows it's over.
    over: bool,
    /// A folder on this machine to remove once it's over.
    scratch: Option<PathBuf>,
}

impl Ticket<'_> {
    /// Removes the scratch folder.
    async fn clean(&mut self) {
        if let Some(scratch) = self.scratch.take() {
            let _ = tokio::task::spawn_blocking(move || remove(&scratch)).await;
        }
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.transfers
            .forget(self.transfer, self.told && !self.over);
        if let Some(scratch) = self.scratch.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => drop(runtime.spawn_blocking(move || remove(&scratch))),
                Err(_) => remove(&scratch),
            }
        }
    }
}

/// Where a fetch lands.
#[derive(Clone, Copy)]
enum Destination {
    /// The downloads folder, under a free name.
    Downloads,
    /// The media cache, unless it's larger than `limit` bytes.
    Media { extension: &'static str, limit: u64 },
    /// A staging folder of its own.
    Copy,
}

/// Where a fetch landed.
enum Fetched {
    Saved(Saved),
    Media(Media),
    Copied(Copied),
}

/// A download in the downloads folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// Where it went.
    pub path: PathBuf,
    /// The links in it this machine couldn't make, by their paths inside
    /// it with `/` between parts. Only Windows leaves any out, where
    /// neither Developer Mode nor an administrator lets it make links.
    pub skipped_links: Vec<String>,
}

/// A file or folder fetched from a device to hand on, in a staging folder
/// of its own that goes when this drops.
#[derive(Debug)]
pub struct Copied {
    folder: PathBuf,
    path: PathBuf,
    skipped_links: Vec<String>,
}

impl Copied {
    /// A copy at `path`, inside `folder`, which goes when it drops.
    pub fn new(folder: PathBuf, path: PathBuf) -> Self {
        Self {
            folder,
            path,
            skipped_links: Vec::new(),
        }
    }

    /// The copy without `links`, which this machine couldn't make.
    fn skipping(mut self, links: Vec<String>) -> Self {
        self.skipped_links = links;
        self
    }

    /// Where the copy is, under its name on the device.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The links the copy left out, as [`Saved::skipped_links`] says.
    pub fn skipped_links(&self) -> &[String] {
        &self.skipped_links
    }
}

impl Drop for Copied {
    fn drop(&mut self) {
        let folder = std::mem::take(&mut self.folder);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn_blocking(move || remove(&folder))),
            Err(_) => remove(&folder),
        }
    }
}

/// Where the one file in a media fetch's manifest goes in the cache, and
/// its size. A folder, a link or more than one file isn't a picture.
fn in_cache(
    entries: &[ManifestEntry],
    folder: &Path,
    extension: &str,
) -> Result<(PathBuf, u64), Failure> {
    let not_one = || Failure::new(ErrorCode::NotAllowed, None, "that isn't one file");
    let [entry] = entries else {
        return Err(not_one());
    };
    if entry.kind != EntryKind::File {
        return Err(not_one());
    }
    let name = entry
        .sha256
        .as_deref()
        .and_then(|sha| media::cached_name(sha, extension))
        .ok_or_else(|| Failure::new(ErrorCode::BadMessage, None, "the file came with no hash"))?;
    Ok((folder.join(name), entry.size))
}

/// The cached file's name, when `to` holds `size` bytes already, marked as
/// shown now.
fn hit(to: &Path, size: u64) -> Option<String> {
    let meta = std::fs::metadata(to).ok()?;
    if !meta.is_file() || meta.len() != size {
        return None;
    }
    media::touch(to);
    to.file_name()?.to_str().map(str::to_owned)
}

/// Moves a landed picture or video to its place in the cache, then makes
/// room for it, and gives its name. A copy that landed there first has the
/// same bytes, so either one does, and when it can't be replaced, as on
/// Windows while the webview has it open, it stays.
fn keep(from: &Path, to: &Path, folder: &Path) -> io::Result<String> {
    if let Err(err) = std::fs::rename(from, to) {
        let size = std::fs::metadata(from)?.len();
        if hit(to, size).is_none() {
            return Err(err);
        }
        debug!(%err, "kept the copy already in the media cache");
    }
    if let Err(err) = media::prune(folder, media::CACHE_LIMIT, to) {
        debug!(%err, "couldn't make room in the media cache");
    }
    to.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| io::ErrorKind::InvalidInput.into())
}

/// An upload ready to go.
struct Prepared {
    name: String,
    entries: Vec<ManifestEntry>,
    source: Source,
}

/// One chunk's way out: the offset and length sent, or the offset and why
/// it couldn't be read.
type Sent = Result<(u64, usize), (u64, io::Error)>;

/// Reads one chunk on a blocking thread and queues it as bulk.
fn send_chunk(
    source: Arc<Mutex<Source>>,
    sender: maxhq_wire::Sender,
    transfer: TransferId,
    offset: u64,
    len: usize,
) -> BoxFuture<'static, Sent> {
    Box::pin(async move {
        let read = tokio::task::spawn_blocking(move || {
            source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .read(offset, len)
        })
        .await
        .unwrap_or_else(|err| Err(io::Error::other(err)));
        let data = read.map_err(|err| (offset, err))?;
        let mut out = Vec::with_capacity(data.len() + 32);
        frame::encode_transfer_chunks(&mut out, transfer, offset, &data);
        if sender.send(Queue::Bulk, out).await.is_err() {
            // The link dropped, which comes as its own event.
            std::future::pending::<()>().await;
        }
        Ok((offset, data.len()))
    })
}

/// The output of a future that may not be there, waiting forever if not.
async fn maybe<F: Future + Unpin>(future: &mut Option<F>) -> F::Output {
    match future {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

/// Queues a message ahead of file data. A closed writer drops it, since
/// the link dropped and each transfer sends again once it's back.
async fn control(sender: &maxhq_wire::Sender, message: &ClientMessage) {
    let mut out = Vec::new();
    if let Err(err) = frame::encode_message(&mut out, message) {
        debug!(%err, "couldn't encode a transfer message");
        return;
    }
    let _ = sender.send(Queue::Control, out).await;
}

/// Queues a message ahead of file data now, if there's room.
fn try_control(sender: &maxhq_wire::Sender, message: &ClientMessage) {
    let mut out = Vec::new();
    if let Err(err) = frame::encode_message(&mut out, message) {
        debug!(%err, "couldn't encode a transfer message");
        return;
    }
    if let Err(err) = sender.try_send(Queue::Control, out) {
        debug!(?err, "couldn't queue a transfer message");
    }
}

fn credit_message(transfer: TransferId, (committed, window_end): (u64, u64)) -> ClientMessage {
    ClientMessage::TransferCredit {
        transfer,
        committed,
        window_end,
    }
}

/// Runs disk work on a blocking thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    joined(tokio::task::spawn_blocking(work).await)
}

fn joined<T>(result: Result<Result<T, Failure>, tokio::task::JoinError>) -> Result<T, Failure> {
    result.unwrap_or_else(|err| Err(Failure::new(ErrorCode::Internal, None, err.to_string())))
}

/// Tells the caller how a transfer goes: on every change of state, and at
/// most every [`EVERY`] as bytes move.
struct Progress<'a> {
    report: &'a (dyn Fn(TransferProgress) + Send + Sync),
    now: TransferProgress,
    at: Option<Instant>,
}

impl<'a> Progress<'a> {
    fn new(
        transfer: TransferId,
        report: &'a (dyn Fn(TransferProgress) + Send + Sync),
        file: String,
    ) -> Self {
        Self {
            report,
            now: TransferProgress {
                transfer,
                sent: 0,
                total: 0,
                file,
                state: TransferState::Uploading,
            },
            at: None,
        }
    }

    fn update(&mut self, sent: u64, total: u64, file: &str, state: TransferState) {
        let changed = self.at.is_none() || state != self.now.state || total != self.now.total;
        let moved = sent != self.now.sent && self.at.is_some_and(|at| at.elapsed() >= EVERY);
        if !changed && !moved {
            return;
        }
        self.now.sent = sent;
        self.now.total = total;
        if self.now.file != file {
            file.clone_into(&mut self.now.file);
        }
        self.now.state = state;
        self.at = Some(Instant::now());
        (self.report)(self.now.clone());
    }

    fn end(&mut self, state: TransferState) {
        if state == TransferState::Done {
            self.now.sent = self.now.total;
        }
        self.now.state = state;
        (self.report)(self.now.clone());
    }
}

/// Which file an offset in a transfer falls in.
struct Files {
    layout: Layout,
    paths: Vec<String>,
    /// The path to show once every byte is through.
    last: String,
}

impl Files {
    fn new(entries: &[ManifestEntry]) -> Self {
        let last = entries
            .iter()
            .rev()
            .find(|entry| entry.size > 0)
            .or(entries.first())
            .map(|entry| entry.path.clone())
            .unwrap_or_default();
        Self {
            layout: Layout::of(entries),
            paths: entries.iter().map(|entry| entry.path.clone()).collect(),
            last,
        }
    }

    fn total(&self) -> u64 {
        self.layout.total()
    }

    fn at(&self, offset: u64) -> &str {
        self.layout
            .locate(offset)
            .map_or(&self.last, |(index, _)| &self.paths[index])
    }

    fn entry(&self, offset: u64) -> Option<u32> {
        self.layout
            .locate(offset)
            .and_then(|(index, _)| u32::try_from(index).ok())
    }
}

/// A download's landing folder, and how far it got.
struct Landing {
    receiver: Arc<Mutex<Receiver>>,
    manifest: Vec<ManifestEntry>,
    files: Files,
    /// The name of what was sent.
    top: String,
    /// The links this machine couldn't make, by their paths inside `top`.
    skipped_links: Vec<String>,
    written: u64,
    done: bool,
}

impl Landing {
    /// Clears `root` and opens a receiver there for `manifest`, leaving
    /// `room` free.
    async fn open(root: &Path, manifest: Vec<ManifestEntry>, room: Room) -> Result<Self, Failure> {
        let files = Files::new(&manifest);
        let root = root.to_owned();
        let entries = manifest.clone();
        let receiver = blocking(move || {
            remove(&root);
            if let Some(incoming) = root.parent() {
                std::fs::create_dir_all(incoming).map_err(|err| Failure::io(&err, None))?;
                prune(incoming);
            }
            Receiver::open(&root, &entries, room)
        })
        .await?;
        let [top] = <[String; 1]>::try_from(receiver.tops()).map_err(|_| {
            Failure::new(
                ErrorCode::BadMessage,
                None,
                "a download holds one file or folder",
            )
        })?;
        let skipped_links = receiver
            .skipped_links()
            .iter()
            .map(|path| inside(path, &top))
            .collect();
        Ok(Self {
            skipped_links,
            written: receiver.written(),
            done: receiver.is_done(),
            receiver: Arc::new(Mutex::new(receiver)),
            manifest,
            files,
            top,
        })
    }

    /// Runs `work` on the receiver on a blocking thread, then notes how far
    /// it got.
    async fn disk<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut Receiver) -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, Failure> {
        let receiver = Arc::clone(&self.receiver);
        let (result, written, done) = tokio::task::spawn_blocking(move || {
            let mut receiver = receiver.lock().unwrap_or_else(PoisonError::into_inner);
            let result = work(&mut receiver);
            (result, receiver.written(), receiver.is_done())
        })
        .await
        .map_err(|err| Failure::new(ErrorCode::Internal, None, err.to_string()))?;
        self.written = written;
        self.done = done;
        result
    }
}

/// A path in a manifest as a path inside `top`, the one file or folder in
/// it.
fn inside(path: &str, top: &str) -> String {
    path.strip_prefix(top)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(path)
        .to_owned()
}

/// One file's bytes as an upload of its own, with an extension from its
/// media type when the name has none.
fn pasted(name: &str, mime: &str, bytes: Vec<u8>) -> Result<Prepared, TransferError> {
    let shown = if name.trim().is_empty() {
        "pasted"
    } else {
        name
    };
    let mut name = name::part(shown).map_err(|err| TransferError::Failed {
        code: ErrorCode::BadPath,
        message: format!("{shown}: {err}"),
    })?;
    if Path::new(&name).extension().is_none()
        && let Some(extension) = extension(mime)
    {
        let named = format!("{name}.{extension}");
        if named.len() <= name::MAX_NAME {
            name = named;
        }
    }
    let entry = ManifestEntry {
        path: name.clone(),
        kind: EntryKind::File,
        executable: false,
        size: bytes.len() as u64,
        sha256: Some(sha256_hex(&bytes)),
    };
    Ok(Prepared {
        name,
        entries: vec![entry],
        source: Source::bytes(bytes),
    })
}

/// The usual extension for a media type a paste can hold.
fn extension(mime: &str) -> Option<&'static str> {
    let essence = mime.split(';').next().unwrap_or_default().trim();
    Some(match essence.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "image/tiff" => "tiff",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        _ => return None,
    })
}

/// How much a staged copy holds so far, to stop at the same limits a walk
/// does before copying all of a huge folder.
#[derive(Default)]
struct Count {
    entries: usize,
    bytes: u64,
}

impl Count {
    fn add(&mut self, bytes: u64) -> Result<(), Failure> {
        self.entries += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        if self.entries > maxhq_transfer::MAX_ENTRIES {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!(
                    "it holds over {} files and folders",
                    maxhq_transfer::MAX_ENTRIES
                ),
            ));
        }
        if self.bytes > maxhq_transfer::MAX_BYTES {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!(
                    "it holds over {}",
                    maxhq_transfer::human(maxhq_transfer::MAX_BYTES)
                ),
            ));
        }
        Ok(())
    }
}

/// Copies what they chose into `into`, under its own name, and walks the
/// copy. The chosen item is followed if it's a link, as a walk does, and
/// links inside are copied as links, so the walk judges them the same. A
/// link this machine can't make, as on Windows without Developer Mode,
/// stays behind.
fn stage(chosen: &Path, into: &Path, stop: &AtomicBool) -> Result<Walk, Failure> {
    let io = |err: io::Error| Failure::io(&err, None);
    let name = chosen
        .file_name()
        .ok_or_else(|| Failure::new(ErrorCode::BadPath, None, "nothing to send"))?;
    let meta = std::fs::metadata(chosen).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => Failure::new(ErrorCode::BadPath, None, "nothing is there"),
        _ => io(err),
    })?;
    std::fs::create_dir_all(into).map_err(io)?;
    let top = into.join(name);
    let mut count = Count::default();
    if meta.is_file() {
        count.add(meta.len())?;
        clone_file(chosen, &top).map_err(io)?;
    } else if meta.is_dir() {
        let from = std::fs::canonicalize(chosen).map_err(io)?;
        count.add(0)?;
        copy_dir(&from, &top, &mut count, stop)?;
    } else {
        return Err(Failure::new(
            ErrorCode::BadPath,
            None,
            "only files and folders can be sent",
        ));
    }
    walk(&top)
}

fn copy_dir(from: &Path, to: &Path, count: &mut Count, stop: &AtomicBool) -> Result<(), Failure> {
    let io = |err: io::Error| Failure::io(&err, None);
    let mut todo = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((from, to)) = todo.pop() {
        std::fs::create_dir(&to).map_err(io)?;
        for item in std::fs::read_dir(&from).map_err(io)? {
            if stop.load(Ordering::Relaxed) {
                return Err(Failure::new(ErrorCode::Internal, None, "cancelled"));
            }
            let item = item.map_err(io)?;
            let name = item.file_name();
            if name.to_str().is_some_and(name::skipped) {
                continue;
            }
            let meta = item.metadata().map_err(io)?;
            let (source, copy) = (item.path(), to.join(&name));
            let kind = meta.file_type();
            if kind.is_dir() {
                count.add(0)?;
                todo.push((source, copy));
            } else if kind.is_file() {
                count.add(meta.len())?;
                clone_file(&source, &copy).map_err(io)?;
            } else if kind.is_symlink() {
                count.add(0)?;
                if !local::copy_link(&source, &copy).map_err(io)? {
                    debug!(link = %source.display(), "couldn't copy a link for an upload");
                }
            }
            // Devices, sockets and pipes stay behind, as the walk skips them.
        }
    }
    Ok(())
}

/// Copies one file, as a clone that shares its blocks where the disk can.
fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use rustix::fs::{fclonefileat, CloneFlags, CWD};
        let source = std::fs::File::open(from)?;
        match fclonefileat(&source, CWD, to, CloneFlags::empty()) {
            Ok(()) => return Ok(()),
            Err(err) => debug!(%err, "couldn't clone a file, so copying it"),
        }
    }
    std::fs::copy(from, to).map(drop)
}

/// Moves a landed download into the downloads folder under the first free
/// name, marks it as from elsewhere, as macOS and Windows check, and gives
/// where it went.
fn settle(from: &Path, top: &str, downloads: &Path) -> io::Result<PathBuf> {
    let wanted = name::download_name(top);
    for _ in 0..SETTLE_TRIES {
        // The disk may tell case apart, so names that differ only in case
        // count as taken too, since they'd clash on a Mac.
        let taken: HashSet<String> = std::fs::read_dir(downloads)?
            .filter_map(Result::ok)
            .filter_map(|item| item.file_name().into_string().ok())
            .map(|name| fold(&name))
            .collect();
        let free = name::free_name(&wanted, |name| taken.contains(&fold(name)));
        let to = downloads.join(&free);
        match local::rename_new(from, &to) {
            Ok(()) => {
                if let Err(err) = local::mark_from_elsewhere(&to) {
                    debug!(%err, "couldn't mark a download as from elsewhere");
                }
                return Ok(to);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name in the downloads folder",
    ))
}

/// A name as a Mac compares it.
fn fold(name: &str) -> String {
    name::nfc(name).to_lowercase()
}

/// Removes a folder and what's in it, if it's there.
fn remove(path: &Path) {
    if let Err(err) = maxhq_transfer::remove_tree(path)
        && err.kind() != io::ErrorKind::NotFound
    {
        debug!(%err, path = %path.display(), "couldn't remove a transfer's folder");
    }
}

/// Removes what transfers left in `dir` over [`LEFT_FOR`] ago.
fn prune(dir: &Path) {
    let Ok(items) = std::fs::read_dir(dir) else {
        return;
    };
    for item in items.flatten() {
        let old = item
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > LEFT_FOR);
        if old {
            remove(&item.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::IdempotencyKey;

    /// Going down a deep folder takes no stack to speak of, and the walk of
    /// the copy then refuses it.
    #[test]
    fn a_deep_folder_stages_on_a_small_stack_and_the_walk_refuses_it() {
        let dir = std::env::temp_dir().join(format!("maxhq-client-{}", IdempotencyKey::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let chosen = dir.join("chosen");
        std::fs::create_dir(&chosen).unwrap();
        let mut at = chosen.clone();
        for _ in 0..400 {
            at.push("d");
            std::fs::create_dir(&at).unwrap();
        }
        let into = dir.join("staged");
        let staged = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || stage(&chosen, &into, &AtomicBool::new(false)))
            .unwrap()
            .join()
            .unwrap();
        maxhq_transfer::remove_tree(&dir).unwrap();
        assert_eq!(staged.unwrap_err().code, ErrorCode::TooLarge);
    }

    #[test]
    fn a_skipped_link_is_named_inside_what_was_sent() {
        assert_eq!(inside("site/docs/latest", "site"), "docs/latest");
        // A name that only starts like the top isn't inside it.
        assert_eq!(inside("sites/latest", "site"), "sites/latest");
        assert_eq!(inside("site", "site"), "site");
    }
}
