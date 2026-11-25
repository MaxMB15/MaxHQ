// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The guest's end of transfers. An upload lands in
//! `<attachments>/incoming/<transfer>` and moves, once every hash checks
//! out, to `<attachments>/<agent>/<transfer>` or, with no agent named, to
//! `<attachments>/staged/<transfer>`. A download leaves from the work
//! folder or the attachments, as the host's credit allows.
//!
//! The host's word is checked like a client's: a path stays beneath those
//! folders, and no link on the way is followed.
//!
//! A picture's preview and a document's text are made beside what lands
//! (see [`crate::sidecar`]), and the host hears of them with its path.
//!
//! Running as root, the guest gives what lands for an agent to the user
//! CLIs run as, before it moves into that agent's folder. The agent's
//! folder and each transfer's folder in it stay root's, and readable, so
//! root never writes through a folder that user can change. A holder
//! whose CLI was given an agent's folder leaves a note in
//! `<attachments>/held`, and once the host forgets that holder's
//! session, the folder goes.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use maxhq_core::fs_ext;
use maxhq_hold::RunAs;
use maxhq_os::fsmode;
use maxhq_protocol::guest::{
    GuestToHost, HostToGuest, Placement, ATTACHMENTS_DIR, MAX_MESSAGE_LEN, MAX_PICTURE_LEN,
};
use maxhq_protocol::ids::{AgentId, SessionId, TransferId};
use maxhq_protocol::message::{ErrorCode, ManifestEntry};
use maxhq_transfer::manifest::{self, Layout};
use maxhq_transfer::{send, Failure, Room, DEVICE_QUOTA};
use maxhq_wire::Closed;
use tracing::{debug, warn};

use crate::doctext::Extractor;
use crate::session::attach::STALE_AFTER;
use crate::session::clock::BoxFuture;
use crate::sidecar::{self, Sidecars};
use crate::transfer::disk::{self, Land, Pump};
use crate::transfer::registry::{Chunk, Inbox, Next, Registry};
use crate::transfer::IDLE;

/// The folder that holds the workspace's repo and each agent's worktree.
pub const WORK_DIR: &str = "/home/agent/work";

/// The most chunk bytes a job writes in one go.
const WRITE_BATCH: usize = 4 * 1024 * 1024;

/// Sends to the host. The guest link's outbox implements it.
pub trait Out: Send + Sync + 'static {
    /// Sends one message.
    fn message(&self, message: GuestToHost) -> BoxFuture<Result<(), Closed>>;
    /// Sends download bytes, split into chunk frames.
    fn chunk(
        &self,
        transfer: TransferId,
        offset: u64,
        data: Vec<u8>,
    ) -> BoxFuture<Result<(), Closed>>;
}

/// Where transfers in the guest go.
pub struct Config {
    /// Where uploads land, [`ATTACHMENTS_DIR`] in a container.
    pub attachments: PathBuf,
    /// Where downloads may come from besides the attachments,
    /// [`WORK_DIR`] in a container.
    pub work: PathBuf,
    /// How much room uploads leave on the container's disk.
    pub room: Room,
    /// The most the attachments folder may hold once an upload lands.
    pub quota: u64,
}

impl Config {
    /// The container's folders, with the default limits.
    pub fn container() -> Self {
        Self {
            attachments: PathBuf::from(ATTACHMENTS_DIR),
            work: PathBuf::from(WORK_DIR),
            room: Room::DEFAULT,
            quota: DEVICE_QUOTA,
        }
    }
}

/// What a job hears.
enum Event {
    /// The host sent `UploadBegin` again.
    Again {
        out: Arc<dyn Out>,
        agent: Option<AgentId>,
        entries: Vec<ManifestEntry>,
    },
    /// The host sent `FetchBegin` or `FetchFiles` again, which is `begin`.
    FetchAgain {
        out: Arc<dyn Out>,
        begin: HostToGuest,
    },
    /// Credit for a download.
    Credit { committed: u64, window_end: u64 },
    /// Bytes for an upload.
    Chunk(Chunk),
    /// The host gave up.
    Cancel,
    /// The host link dropped.
    Down,
}

struct Shared {
    config: Config,
    open: Registry<Event, ()>,
    /// The user that gets what lands for an agent.
    owner: Option<String>,
    /// Reads the text out of the documents that land.
    extractor: Extractor,
}

impl Shared {
    /// The note naming the agents whose folders `session`'s CLI was given.
    fn note(&self, session: SessionId) -> PathBuf {
        self.config
            .attachments
            .join("held")
            .join(session.to_string())
    }

    /// The agent whose attachments are at `path`, if that's an agent's
    /// folder.
    fn agent_of(&self, path: &Path) -> Option<AgentId> {
        if path.parent() != Some(self.config.attachments.as_path()) {
            return None;
        }
        path.file_name()?.to_str()?.parse().ok()
    }

    /// The user this guest's files belong to, when it runs as root for one.
    fn owner_as_root(&self) -> Result<Option<RunAs>, String> {
        match self.owner.as_deref() {
            Some(owner) if running_as_root() => RunAs::lookup(owner)
                .map(Some)
                .map_err(|err| format!("couldn't find user {owner}: {err}")),
            _ => Ok(None),
        }
    }

    /// Gives what an upload for `agent` holds at `top`, and everything
    /// beneath it, to the owner. Only root can, so a guest that isn't
    /// leaves it be. It runs before the upload moves into the agent's
    /// folder, while every folder above `top` is still root's.
    fn hand_over(&self, agent: AgentId, top: &Path) {
        let Some(owner) = self.owner.as_deref() else {
            return;
        };
        if !running_as_root() {
            return;
        }
        let user = match RunAs::lookup(owner) {
            Ok(user) => user,
            Err(err) => {
                warn!("couldn't find user {owner} to give an upload to: {err}");
                return;
            }
        };
        if let Err(err) = give(top, &user) {
            warn!(%agent, "couldn't give an upload to {owner}: {err}");
        }
    }

    fn incoming(&self, transfer: TransferId) -> PathBuf {
        self.config
            .attachments
            .join("incoming")
            .join(transfer.to_string())
    }

    /// The folder an upload for `agent` lands in, or the staging folder.
    fn landing(&self, agent: Option<AgentId>) -> PathBuf {
        match agent {
            Some(agent) => self.config.attachments.join(agent.to_string()),
            None => self.config.attachments.join("staged"),
        }
    }
}

/// The guest's transfers. Clones share them.
#[derive(Clone)]
pub struct Files {
    shared: Arc<Shared>,
}

impl Files {
    /// Transfers into and out of the folders in `config`, where what lands
    /// keeps the guest's own user.
    #[cfg(test)]
    pub fn new(config: Config) -> Self {
        Self::with_owner(config, None)
    }

    /// Transfers into and out of the folders in `config`, where what lands
    /// for an agent goes to `user` while the guest runs as root.
    pub fn owned_by(config: Config, user: &str) -> Self {
        Self::with_owner(config, Some(user.to_owned()))
    }

    fn with_owner(config: Config, owner: Option<String>) -> Self {
        Self::reading(config, owner, Extractor::default())
    }

    /// Transfers into and out of the folders in `config`, reading
    /// documents' text with `extractor`.
    #[cfg(test)]
    pub fn with_extractor(config: Config, extractor: Extractor) -> Self {
        Self::reading(config, None, extractor)
    }

    /// The text of documents is read as the owner too, while the guest
    /// runs as root.
    fn reading(config: Config, owner: Option<String>, extractor: Extractor) -> Self {
        Self {
            shared: Arc::new(Shared {
                config,
                open: Registry::default(),
                extractor: extractor.owned_by(owner.clone()),
                owner,
            }),
        }
    }

    /// Notes the agents' folders in a new holder's `args`, so they go
    /// once the host forgets `session`. The note outlives the guest's
    /// restarts, as the holder does.
    pub async fn held(&self, session: SessionId, args: Vec<String>) {
        let shared = self.shared.clone();
        let agents: Vec<AgentId> = args
            .iter()
            .filter_map(|arg| shared.agent_of(Path::new(arg)))
            .collect();
        if agents.is_empty() {
            return;
        }
        let _ = disk::blocking(move || {
            let note = shared.note(session);
            let text: String = agents.iter().map(|agent| format!("{agent}\n")).collect();
            let written = match note.parent() {
                Some(parent) => fs::create_dir_all(parent),
                None => Ok(()),
            }
            .and_then(|()| fs::write(&note, text));
            if let Err(err) = written {
                warn!(%session, "couldn't note a holder's attachments: {err}");
            }
            Ok(())
        })
        .await;
    }

    /// Removes the agents' folders the holder for `session` was given,
    /// since the host forgot it. A folder another held session names
    /// stays.
    pub async fn forgotten(&self, session: SessionId) {
        let shared = self.shared.clone();
        let _ = disk::blocking(move || {
            let note = shared.note(session);
            let Ok(text) = fs::read_to_string(&note) else {
                return Ok(());
            };
            let _ = fs::remove_file(&note);
            let others = still_held(&shared.config.attachments.join("held"));
            for agent in agents_in(&text) {
                if others.contains(&agent) {
                    continue;
                }
                match maxhq_transfer::remove_tree(shared.landing(Some(agent))) {
                    Ok(()) => debug!(%agent, %session, "removed a forgotten agent's attachments"),
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => warn!(%agent, "couldn't remove an agent's attachments: {err}"),
                }
            }
            Ok(())
        })
        .await;
    }

    /// Writes a picture from one of `agent`'s tool results into its
    /// folder, where the host's timeline names it. It doesn't wait.
    pub fn keep_picture(&self, agent: AgentId, media_type: String, data: String) {
        if data.len() > MAX_PICTURE_LEN {
            debug!(%agent, "left out a picture too large to keep");
            return;
        }
        let root = self.shared.config.attachments.clone();
        tokio::task::spawn_blocking(move || {
            crate::drive::pictures::land(&root, agent, &media_type, &data);
        });
    }

    /// Handles `UploadBegin`, `UploadCancel`, `FetchBegin`, `FetchFiles` or
    /// `TransferCredit` from the host, answering on `out`. It doesn't
    /// wait, so the link's reader can call it.
    pub fn message(&self, message: HostToGuest, out: Arc<dyn Out>) {
        match message {
            HostToGuest::UploadBegin {
                transfer,
                agent,
                entries,
            } => {
                let again = Event::Again {
                    out: out.clone(),
                    agent,
                    entries: entries.clone(),
                };
                self.begin(transfer, again, |job| upload(job, agent, entries), out);
            }
            HostToGuest::FetchBegin { transfer, ref path } => {
                let wanted = Wanted::Path(path.clone());
                self.fetch(transfer, wanted, message, out);
            }
            HostToGuest::FetchFiles {
                transfer,
                ref cwd,
                ref paths,
            } => {
                let wanted = Wanted::Files {
                    cwd: cwd.clone(),
                    paths: paths.clone(),
                };
                self.fetch(transfer, wanted, message, out);
            }
            HostToGuest::UploadCancel { transfer } => {
                if self.shared.open.send(transfer, (), Event::Cancel).is_err() {
                    // An upload whose job went idle still has its landing.
                    let incoming = self.shared.incoming(transfer);
                    tokio::spawn(async move { disk::remove(&incoming).await });
                }
            }
            HostToGuest::TransferCredit {
                transfer,
                committed,
                window_end,
            } => {
                let credit = Event::Credit {
                    committed,
                    window_end,
                };
                if self.shared.open.send(transfer, (), credit).is_err() {
                    debug!(%transfer, "dropped credit for a transfer that isn't open");
                }
            }
            _ => debug!("dropped a message that isn't about a transfer"),
        }
    }

    /// `Place` from the host: moves the upload staged as `staged` into the
    /// checkout at `cwd`, under the work folder, as
    /// [`deliver::place`](crate::transfer::deliver::place) does. Running
    /// as root, what moves in becomes the owner's. The staged upload goes
    /// once it's placed.
    pub async fn place(
        &self,
        staged: TransferId,
        cwd: String,
        overwrite: bool,
    ) -> Result<Placement, String> {
        let shared = self.shared.clone();
        let placed = tokio::task::spawn_blocking(move || {
            let work = shared.config.work.clone();
            let checkout = disk::inside(&[work], &cwd)
                .and_then(|found| found.folder())
                .map_err(|failure| failure.message.clone())?;
            let folder = shared.landing(None).join(staged.to_string());
            let top = staged_top(&folder)?;
            let owner = shared.owner_as_root()?;
            let placement =
                crate::transfer::deliver::place_into(&top, &checkout, overwrite, owner.as_ref())?;
            if let Err(err) = maxhq_transfer::remove_tree(&folder) {
                warn!(%staged, "couldn't remove a placed upload: {err}");
            }
            Ok(placement)
        });
        placed
            .await
            .unwrap_or_else(|_| Err("placing the files stopped".to_owned()))
    }

    /// Starts the download `begin` asks for, or hands it to the open one.
    fn fetch(&self, transfer: TransferId, wanted: Wanted, begin: HostToGuest, out: Arc<dyn Out>) {
        let again = Event::FetchAgain {
            out: out.clone(),
            begin,
        };
        self.begin(transfer, again, |job| fetch(job, wanted), out);
    }

    /// Handles upload bytes from the host.
    pub fn chunk(&self, transfer: TransferId, offset: u64, data: Vec<u8>) {
        self.shared
            .open
            .chunk(transfer, (), offset, data, Event::Chunk);
    }

    /// Ends every job, since the host link dropped. What uploads landed
    /// stays for a resend on the next link.
    pub fn link_down(&self) {
        self.shared.open.each((), || Event::Down);
    }

    /// Hands a begin to the open job, or starts one.
    fn begin<F>(
        &self,
        transfer: TransferId,
        again: Event,
        start: impl FnOnce(Job) -> F,
        out: Arc<dyn Out>,
    ) where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut again = again;
        let mut start = Some(start);
        // A job that just ended frees its slot under the same lock a send
        // takes, so one of these goes through.
        for _ in 0..3 {
            again = match self.shared.open.send(transfer, (), again) {
                Ok(()) => return,
                Err(again) => again,
            };
            if let Some(inbox) = self.shared.open.open(transfer, vec![()]) {
                let Some(start) = start.take() else {
                    return;
                };
                let job = Job {
                    shared: self.shared.clone(),
                    inbox,
                    out,
                    transfer,
                    stash: None,
                };
                tokio::spawn(start(job));
                return;
            }
        }
        warn!(%transfer, "couldn't open a transfer");
    }
}

/// One transfer's task in the guest.
struct Job {
    shared: Arc<Shared>,
    inbox: Inbox<Event, ()>,
    out: Arc<dyn Out>,
    transfer: TransferId,
    /// An event taken early, handled next.
    stash: Option<Event>,
}

impl Job {
    async fn next(&mut self, tick: Option<&mut tokio::time::Interval>) -> Next<Event> {
        match self.stash.take() {
            Some(event) => Next::Event(event),
            None => self.inbox.next(IDLE, tick).await,
        }
    }

    fn has_event(&mut self) -> bool {
        if self.stash.is_none() {
            self.stash = self.inbox.try_recv();
        }
        self.stash.is_some()
    }

    /// `first` and the chunks that wait after it, up to [`WRITE_BATCH`]
    /// bytes.
    fn chunks(&mut self, first: Chunk) -> Vec<(u64, Vec<u8>)> {
        let mut len = first.data.len();
        let mut chunks = vec![(first.offset, first.data)];
        while len < WRITE_BATCH && self.stash.is_none() {
            match self.inbox.try_recv() {
                Some(Event::Chunk(chunk)) => {
                    len += chunk.data.len();
                    chunks.push((chunk.offset, chunk.data));
                }
                Some(other) => self.stash = Some(other),
                None => break,
            }
        }
        chunks
    }

    async fn send(&self, message: GuestToHost) {
        if self.out.message(message).await.is_err() {
            debug!(transfer = %self.transfer, "the host link went away");
        }
    }

    async fn credit(&self, (committed, window_end): (u64, u64)) {
        self.send(GuestToHost::TransferCredit {
            transfer: self.transfer,
            committed,
            window_end,
        })
        .await;
    }

    async fn failed(&self, failure: &Failure) {
        self.send(GuestToHost::TransferFailed {
            transfer: self.transfer,
            code: failure.code,
            entry: failure.entry,
            message: failure.message.clone(),
        })
        .await;
    }

    /// Closes the transfer. A begin that came in the meantime starts it
    /// again, so it's answered from what's on disk now.
    fn close(mut self) {
        let mut begins = Vec::new();
        loop {
            while let Some(event) = self.stash.take().or_else(|| self.inbox.try_recv()) {
                match event {
                    Event::Again {
                        out,
                        agent,
                        entries,
                    } => begins.push((
                        HostToGuest::UploadBegin {
                            transfer: self.transfer,
                            agent,
                            entries,
                        },
                        out,
                    )),
                    Event::FetchAgain { out, begin } => begins.push((begin, out)),
                    _ => {}
                }
            }
            if self.inbox.close_if_empty() {
                break;
            }
        }
        let files = Files {
            shared: self.shared.clone(),
        };
        drop(self);
        for (message, out) in begins {
            files.message(message, out);
        }
    }
}

/// How an upload's landing ended.
enum Landed {
    /// It's in place at these paths, with what was made beside it.
    Stored(Vec<String>, Sidecars),
    /// The host went quiet or its link dropped, and what landed stays.
    Kept,
    /// The host gave up.
    Cancelled,
}

async fn upload(mut job: Job, agent: Option<AgentId>, entries: Vec<ManifestEntry>) {
    let incoming = job.shared.incoming(job.transfer);
    match land(&mut job, agent, entries).await {
        Ok(Landed::Stored(paths, made)) => {
            job.send(GuestToHost::UploadStored {
                transfer: job.transfer,
                paths,
                thumbnail: made.thumbnail,
                text: made.text.and_then(|text| text.to_str().map(str::to_owned)),
            })
            .await;
        }
        Ok(Landed::Kept) => {}
        Ok(Landed::Cancelled) => disk::remove(&incoming).await,
        Err(failure) => {
            job.failed(&failure).await;
            disk::remove(&incoming).await;
        }
    }
    job.close();
}

/// Lands an upload, or finds it landed already.
async fn land(
    job: &mut Job,
    mut agent: Option<AgentId>,
    entries: Vec<ManifestEntry>,
) -> Result<Landed, Failure> {
    let checked = disk::one_top(&entries)?;
    let top = manifest::tops(&checked)
        .first()
        .and_then(|entry| entry.parts.first().cloned())
        .ok_or_else(|| Failure::new(ErrorCode::BadMessage, None, "an upload has nothing in it"))?;
    if let Some((paths, made)) = placed(&job.shared, job.transfer, agent, &top).await? {
        return Ok(Landed::Stored(paths, made));
    }
    clean(&job.shared).await;
    let shared = job.shared.clone();
    let incoming = shared.incoming(job.transfer);
    let total = Layout::of(&entries).total();
    disk::check_quota(
        shared.config.attachments.clone(),
        incoming.clone(),
        total,
        shared.config.quota,
    )
    .await?;
    let mut land = Land::open(incoming.clone(), entries.clone(), shared.config.room).await?;
    job.credit(land.resume().await?).await;
    let mut tick = disk::ticker();
    loop {
        if land.is_done() {
            return finish(&shared, job.transfer, agent, &incoming, &top)
                .await
                .map(|(paths, made)| Landed::Stored(paths, made));
        }
        let ticking = !land.is_synced();
        match job.next(ticking.then_some(&mut tick)).await {
            Next::Idle => return Ok(Landed::Kept),
            Next::Tick => {
                if let Some(credit) = land.tick().await? {
                    job.credit(credit).await;
                }
            }
            Next::Event(Event::Chunk(first)) => {
                let chunks = job.chunks(first);
                match land.write(chunks).await {
                    Ok(Some(credit)) => job.credit(credit).await,
                    Ok(None) => {}
                    // The landing is back at the file's start, and the host
                    // sends it again.
                    Err(failure) if failure.code == ErrorCode::HashMismatch => {
                        job.failed(&failure).await;
                    }
                    Err(failure) => return Err(failure),
                }
            }
            Next::Event(Event::Again {
                out,
                agent: wanted,
                entries: resent,
            }) => {
                job.out = out;
                if resent != entries {
                    return Err(Failure::new(
                        ErrorCode::BadMessage,
                        None,
                        "the upload was begun again with other files",
                    ));
                }
                agent = wanted;
                job.credit(land.resume().await?).await;
            }
            Next::Event(Event::Cancel) => return Ok(Landed::Cancelled),
            Next::Event(Event::Down) => return Ok(Landed::Kept),
            Next::Event(Event::Credit { .. } | Event::FetchAgain { .. }) => {
                debug!("dropped an event an upload doesn't take");
            }
        }
    }
}

/// The paths of an upload that landed already, and what's beside it,
/// moving it out of the staging folder when an agent is named now.
async fn placed(
    shared: &Arc<Shared>,
    transfer: TransferId,
    agent: Option<AgentId>,
    top: &str,
) -> Result<Option<(Vec<String>, Sidecars)>, Failure> {
    let id = transfer.to_string();
    let wanted = shared.landing(agent).join(&id);
    let staged = shared.landing(None).join(&id);
    let top = top.to_owned();
    let shared = shared.clone();
    disk::blocking(move || {
        let fail = |err: std::io::Error| Failure::io(&err, None);
        let found = |path: PathBuf| path_of(&path).map(|at| Some((vec![at], sidecar::read(&path))));
        if exists(&wanted.join(&top)) {
            return found(wanted.join(&top));
        }
        if let Some(agent) = agent.filter(|_| exists(&staged.join(&top))) {
            shared.hand_over(agent, &staged.join(&top));
            if let Some(parent) = wanted.parent() {
                fs::create_dir_all(parent).map_err(fail)?;
            }
            fs::rename(&staged, &wanted).map_err(fail)?;
            return found(wanted.join(&top));
        }
        Ok(None)
    })
    .await
}

/// Moves a landed upload into place, makes its preview or text, and
/// gives its path and those.
async fn finish(
    shared: &Arc<Shared>,
    transfer: TransferId,
    agent: Option<AgentId>,
    incoming: &Path,
    top: &str,
) -> Result<(Vec<String>, Sidecars), Failure> {
    let into = shared.landing(agent).join(transfer.to_string());
    let from = incoming.join(top);
    let to = into.join(top);
    let incoming = incoming.to_owned();
    let shared = shared.clone();
    disk::blocking(move || {
        let fail = |err: std::io::Error| Failure::io(&err, None);
        if let Some(agent) = agent {
            shared.hand_over(agent, &from);
        }
        fs::create_dir_all(&into).map_err(fail)?;
        fs::rename(&from, &to).map_err(fail)?;
        let _ = maxhq_transfer::remove_tree(&incoming);
        let path = path_of(&to)?;
        Ok((vec![path], sidecar::make(&to, None, &shared.extractor)))
    })
    .await
}

/// Whether something is at `path`, without following a link there.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Whether the guest runs as root. Off Linux there's no `/proc`, which
/// reads as not root.
fn running_as_root() -> bool {
    fs::metadata("/proc/self").is_ok_and(|meta| fsmode::owner_of(&meta) == Some(0))
}

/// Gives `path` and everything beneath it to `user`, never following a
/// link. A folder goes only once what it holds is given, so `user` can't
/// change a folder this still walks. A folder that root doesn't own, or
/// that others can write to, isn't walked at all.
fn give(path: &Path, user: &RunAs) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && fsmode::owner_of(&meta) == Some(0) && fsmode::mode_of(&meta) & 0o022 == 0 {
        for entry in fs::read_dir(path)? {
            give(&entry?.path(), user)?;
        }
    }
    fs_ext::chown_link(path, user.uid, user.gid)
}

/// The one folder a staged upload holds at its top.
fn staged_top(folder: &Path) -> Result<PathBuf, String> {
    let missing = || "the copied files aren't staged here any more".to_owned();
    let entries = fs::read_dir(folder).map_err(|_| missing())?;
    let mut folders = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()));
    match (folders.next(), folders.next()) {
        (Some(top), None) => Ok(top.path()),
        (None, _) => Err(missing()),
        (Some(_), Some(_)) => Err("the staged upload holds more than one folder".to_owned()),
    }
}

/// The agents a holder's note names.
fn agents_in(text: &str) -> impl Iterator<Item = AgentId> + '_ {
    text.lines().filter_map(|line| line.trim().parse().ok())
}

/// Every agent the notes in `held` name.
fn still_held(held: &Path) -> HashSet<AgentId> {
    let Ok(notes) = fs::read_dir(held) else {
        return HashSet::new();
    };
    notes
        .flatten()
        .filter_map(|note| fs::read_to_string(note.path()).ok())
        .flat_map(|text| agents_in(&text).collect::<Vec<_>>())
        .collect()
}

fn path_of(path: &Path) -> Result<String, Failure> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        Failure::new(
            ErrorCode::Internal,
            None,
            "an upload landed at a path that isn't UTF-8",
        )
    })
}

/// Removes landings and staged uploads older than a day. Those of open
/// transfers stay.
async fn clean(shared: &Arc<Shared>) {
    let shared = shared.clone();
    let _ = disk::blocking(move || {
        let now = SystemTime::now();
        for folder in ["incoming", "staged"] {
            let Ok(entries) = fs::read_dir(shared.config.attachments.join(folder)) else {
                continue;
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
        }
        Ok(())
    })
    .await;
}

/// What a download sends.
enum Wanted {
    /// A file or folder in the work folder or the attachments.
    Path(String),
    /// Files in the checkout at `cwd`, under the work folder.
    Files { cwd: String, paths: Vec<String> },
}

async fn fetch(mut job: Job, wanted: Wanted) {
    if let Err(failure) = serve(&mut job, wanted).await {
        job.failed(&failure).await;
    }
    job.close();
}

/// Sends a file or folder in the work folder or the attachments.
///
/// This guest may run as root and the agent's user owns what it sends, so
/// every read goes through a folder held open and follows no link.
async fn serve(job: &mut Job, wanted: Wanted) -> Result<(), Failure> {
    let shared = job.shared.clone();
    let walk = disk::blocking(move || {
        let work = shared.config.work.clone();
        let owner = shared
            .owner_as_root()
            .map_err(|message| Failure::new(ErrorCode::Internal, None, message))?
            .map(|user| user.uid);
        match wanted {
            Wanted::Path(path) => {
                let roots = [work, shared.config.attachments.clone()];
                let found = disk::inside(&roots, &path)?;
                send::walk_beneath(found.top, &found.name, &found.parts, owner)
            }
            Wanted::Files { cwd, paths } => {
                let checkout = disk::inside(&[work], &cwd)?;
                let top = checkout.folder()?;
                send::walk_files_beneath(top, &checkout.name, &paths, owner)
            }
        }
    })
    .await?;
    let manifest = GuestToHost::FetchManifest {
        transfer: job.transfer,
        entries: walk.entries().to_vec(),
    };
    let len = serde_json::to_vec(&manifest).map_or(usize::MAX, |json| json.len());
    if len > MAX_MESSAGE_LEN {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            None,
            "the download holds too many files to list in one message",
        ));
    }
    job.send(manifest.clone()).await;
    let mut pump = Pump::new(walk.source());
    // The first credit after a manifest says where the host picks up.
    let mut fresh = true;
    loop {
        if pump.outgoing().is_committed() {
            return Ok(());
        }
        while !job.has_event() {
            let Some(chunks) = pump.next().await? else {
                break;
            };
            for (offset, data) in chunks {
                if job.out.chunk(job.transfer, offset, data).await.is_err() {
                    return Ok(());
                }
            }
        }
        match job.next(None).await {
            Next::Idle | Next::Tick => return Ok(()),
            Next::Event(Event::Credit {
                committed,
                window_end,
            }) => {
                if fresh {
                    pump.resume(committed, window_end)?;
                } else {
                    pump.credit(committed, window_end)?;
                }
                fresh = false;
            }
            Next::Event(Event::FetchAgain { out, .. }) => {
                job.out = out;
                job.send(manifest.clone()).await;
                fresh = true;
            }
            Next::Event(Event::Cancel | Event::Down) => return Ok(()),
            Next::Event(Event::Chunk(_) | Event::Again { .. }) => {
                debug!("dropped an event a download doesn't take");
            }
        }
    }
}
