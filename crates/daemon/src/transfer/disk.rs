// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The transfer crate's blocking ends, run off the async tasks. [`Land`]
//! takes a transfer onto this disk and [`Pump`] reads one off it. The
//! host and the guest both use them.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use maxhq_protocol::frame::MAX_TRANSFER_CHUNK;
use maxhq_protocol::message::{ErrorCode, ManifestEntry};
use maxhq_transfer::beneath::Dir;
use maxhq_transfer::manifest::{self, Kind};
use maxhq_transfer::{Failure, Outgoing, Receiver, Room, Source, Walk};
use tokio::time::{interval_at, Interval, MissedTickBehavior};
use tracing::debug;

/// How often a landing fsyncs and sends the credit that moved.
pub const TICK: Duration = Duration::from_secs(1);

/// The most file bytes one read off the disk takes, as several chunks.
const READ_BATCH: usize = 4 * MAX_TRANSFER_CHUNK;

/// The most bytes one call to [`Land::fill`] copies, so a cancel waits
/// for no more than this.
const FILL_BATCH: u64 = 16 * 1024 * 1024;

/// A failure for a blocking task that panicked or was cancelled.
fn lost() -> Failure {
    Failure::new(
        ErrorCode::Internal,
        None,
        "the transfer's disk work stopped",
    )
}

/// Runs `work` on a blocking thread.
pub async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| Err(lost()))
}

/// One transfer landing in a folder on this disk.
pub struct Land {
    receiver: Option<Receiver>,
}

impl Land {
    /// Opens the landing folder, picking up what an earlier attempt left.
    pub async fn open(
        root: PathBuf,
        entries: Vec<ManifestEntry>,
        room: Room,
    ) -> Result<Self, Failure> {
        let receiver = blocking(move || Receiver::open(&root, &entries, room)).await?;
        Ok(Self {
            receiver: Some(receiver),
        })
    }

    async fn with<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut Receiver) -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, Failure> {
        let mut receiver = self.receiver.take().ok_or_else(lost)?;
        let (receiver, result) = tokio::task::spawn_blocking(move || {
            let result = work(&mut receiver);
            (receiver, result)
        })
        .await
        .map_err(|_| lost())?;
        self.receiver = Some(receiver);
        result
    }

    /// The credit for a sender that asked again, fsynced first.
    pub async fn resume(&mut self) -> Result<(u64, u64), Failure> {
        self.with(|receiver| receiver.resume(Instant::now())).await
    }

    /// Writes chunks in the order they came, and gives the credit to send
    /// if it moved. A hash mismatch leaves the landing open, for the
    /// sender to start the file again.
    pub async fn write(
        &mut self,
        chunks: Vec<(u64, Vec<u8>)>,
    ) -> Result<Option<(u64, u64)>, Failure> {
        self.with(move |receiver| {
            let now = Instant::now();
            for (offset, data) in &chunks {
                receiver.write(*offset, data, now)?;
            }
            Ok(receiver.credit(now))
        })
        .await
    }

    /// Fsyncs when it's due, and gives the credit to send if it moved.
    pub async fn tick(&mut self) -> Result<Option<(u64, u64)>, Failure> {
        self.with(|receiver| {
            let now = Instant::now();
            receiver.sync_if_due(now)?;
            Ok(receiver.credit(now))
        })
        .await
    }

    /// Copies up to [`FILL_BATCH`] more bytes from `source`, a file or
    /// folder on this disk, and gives the source back. A hash mismatch
    /// means a file changed while it was read. The landing is rewound to
    /// that file's start, so calling again tries it once more.
    pub async fn fill(&mut self, mut source: Source) -> Result<Source, Failure> {
        self.with(move |receiver| {
            let mut copied = 0;
            while copied < FILL_BATCH && receiver.written() < receiver.total() {
                let now = Instant::now();
                let end = receiver.extend_grant(now);
                let mut offset = receiver.written();
                while offset < end {
                    let len = usize::try_from(end - offset)
                        .unwrap_or(MAX_TRANSFER_CHUNK)
                        .min(MAX_TRANSFER_CHUNK);
                    let entry = source
                        .entry_at(offset)
                        .and_then(|index| u32::try_from(index).ok());
                    let data = source
                        .read(offset, len)
                        .map_err(|err| Failure::io(&err, entry))?;
                    receiver.write(offset, &data, now)?;
                    offset += data.len() as u64;
                    copied += data.len() as u64;
                }
                receiver.sync_if_due(now)?;
            }
            if receiver.written() == receiver.total() {
                receiver.sync(Instant::now())?;
            }
            Ok(source)
        })
        .await
    }

    /// Whether every entry has landed and is on disk for good.
    pub fn is_done(&self) -> bool {
        self.receiver.as_ref().is_some_and(Receiver::is_done)
    }

    /// Whether everything written is fsynced, so a tick has nothing to do.
    pub fn is_synced(&self) -> bool {
        self.receiver
            .as_ref()
            .is_none_or(|receiver| receiver.committed() == receiver.written())
    }

    /// The names at the top of the landing folder.
    pub fn tops(&self) -> Vec<String> {
        self.receiver
            .as_ref()
            .map(Receiver::tops)
            .unwrap_or_default()
    }
}

/// One transfer read off this disk, as credit allows.
pub struct Pump {
    source: Option<Source>,
    outgoing: Outgoing,
}

impl Pump {
    /// Reads `source`, with no credit yet.
    pub fn new(source: Source) -> Self {
        Self {
            outgoing: Outgoing::new(source.total()),
            source: Some(source),
        }
    }

    /// The sender's half of the credit.
    pub fn outgoing(&self) -> &Outgoing {
        &self.outgoing
    }

    /// Takes a credit from the receiver.
    pub fn credit(&mut self, committed: u64, window_end: u64) -> Result<(), Failure> {
        self.outgoing.credit(committed, window_end)
    }

    /// Goes back to where the receiver has bytes for good.
    pub fn resume(&mut self, committed: u64, window_end: u64) -> Result<(), Failure> {
        self.outgoing.resume(committed, window_end)
    }

    /// Counts a file sent again after its hash didn't match, and says
    /// whether that's allowed.
    pub fn restart(&mut self) -> bool {
        self.outgoing.restart()
    }

    /// The next chunks the credit allows, each an offset and its bytes,
    /// or `None` while it allows none.
    pub async fn next(&mut self) -> Result<Option<Vec<(u64, Vec<u8>)>>, Failure> {
        let mut plan = Vec::new();
        let mut ahead = self.outgoing.clone();
        let mut planned = 0;
        while planned < READ_BATCH {
            let Some((offset, len)) = ahead.next(MAX_TRANSFER_CHUNK) else {
                break;
            };
            ahead.sent_chunk(len);
            plan.push((offset, len));
            planned += len;
        }
        if plan.is_empty() {
            return Ok(None);
        }
        let mut source = self.source.take().ok_or_else(lost)?;
        let (source, chunks) = tokio::task::spawn_blocking(move || {
            let chunks = plan
                .into_iter()
                .map(|(offset, len)| Ok((offset, source.read(offset, len)?)))
                .collect::<io::Result<Vec<_>>>();
            (source, chunks)
        })
        .await
        .map_err(|_| lost())?;
        self.source = Some(source);
        let chunks = chunks.map_err(|err| Failure::io(&err, None))?;
        self.outgoing = ahead;
        Ok(Some(chunks))
    }
}

/// Walks a file or folder on this disk into a manifest, off the async
/// task.
pub async fn walk(path: PathBuf) -> Result<Walk, Failure> {
    blocking(move || maxhq_transfer::walk(&path)).await
}

/// Walks chosen files below a checkout, off the async threads.
pub async fn walk_files(root: PathBuf, paths: Vec<String>) -> Result<Walk, Failure> {
    blocking(move || maxhq_transfer::send::walk_files(&root, &paths)).await
}

/// A path found below one of a guest's folders, with that folder open.
pub struct Inside {
    /// The folder the path is under, which stays open.
    pub top: Dir,
    /// The parts of the path below `top`.
    pub parts: Vec<String>,
    /// The name of what's at the path.
    pub name: String,
    /// The path itself, with the folder as the guest knows it.
    pub path: PathBuf,
}

impl Inside {
    /// Opens the folder at the path, without following a link.
    pub fn folder(&self) -> Result<Dir, Failure> {
        self.top
            .dir(&self.parts)
            .map_err(|err| Failure::io(&err, None))
    }
}

/// Finds `path` below one of `roots` for a guest acting for a user who can
/// change what's there, even swap a folder in `roots` for a link. So unlike
/// [`resolve`], the folder opens by the name the guest knows it by and is
/// refused if that's a link.
pub fn inside(roots: &[PathBuf], path: &str) -> Result<Inside, Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadPath, None, message);
    let io = |err: std::io::Error| Failure::io(&err, None);
    // A guest reads only inside its folders, with no Show anyway, so a
    // path outside them is just a bad one.
    let (root, parts) = below(roots, path).map_err(|failure| match failure.code {
        ErrorCode::Outside => Failure {
            code: ErrorCode::BadPath,
            ..failure
        },
        _ => failure,
    })?;
    let top = Dir::open(&root).map_err(|err| {
        if std::fs::symlink_metadata(&root).is_ok_and(|meta| meta.is_symlink()) {
            bad("the agent's folder is a link, which MaxHQ won't follow")
        } else {
            io(err)
        }
    })?;
    if let Some((leaf, parent)) = parts.split_last() {
        let dir = top.dir(parent).map_err(io)?;
        match dir.kind(leaf).map_err(io)? {
            None => return Err(bad("nothing is at that path")),
            Some(kind) if kind.is_symlink() => {
                return Err(bad("the path is a link, which MaxHQ won't follow"));
            }
            Some(_) => {}
        }
    }
    let name = match parts.last() {
        Some(leaf) => leaf.clone(),
        None => root
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| bad("the name isn't text"))?
            .to_owned(),
    };
    let path = parts.iter().fold(root, |path, part| path.join(part));
    Ok(Inside {
        top,
        parts,
        name,
        path,
    })
}

/// Refuses an upload of `total` bytes into `root` when the attachments
/// there would pass `quota`. What already landed in `incoming` counts as
/// part of the upload.
pub async fn check_quota(
    root: PathBuf,
    incoming: PathBuf,
    total: u64,
    quota: u64,
) -> Result<(), Failure> {
    blocking(move || {
        let used = bytes_or_none(&root)?.saturating_sub(bytes_or_none(&incoming)?);
        maxhq_transfer::check_quota(used, total, quota)
    })
    .await
}

fn bytes_or_none(path: &Path) -> Result<u64, Failure> {
    match maxhq_transfer::folder_bytes(path) {
        Ok(bytes) => Ok(bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(Failure::io(&err, None)),
    }
}

/// Checks that a manifest holds one file or folder at its top, as an
/// upload does.
pub fn one_top(entries: &[ManifestEntry]) -> Result<Vec<manifest::Entry>, Failure> {
    let checked = manifest::check(entries)?;
    let tops = manifest::tops(&checked);
    match tops.as_slice() {
        [top] if !matches!(top.kind, Kind::Symlink { .. }) => {}
        [_] => {
            return Err(Failure::new(
                ErrorCode::BadMessage,
                Some(0),
                "an upload can't be a link on its own",
            ))
        }
        _ => {
            return Err(Failure::new(
                ErrorCode::BadMessage,
                None,
                "an upload holds one file or folder",
            ))
        }
    }
    Ok(checked)
}

/// Finds a file or folder inside one of `roots`, without following a link
/// on the way. An absolute path must be inside one of them, as written or
/// with links in the roots themselves resolved, and a relative one is from
/// the first.
pub fn resolve(roots: &[PathBuf], path: &str) -> Result<PathBuf, Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadPath, None, message);
    let io = |err: std::io::Error| Failure::io(&err, None);
    let (root, parts) = below(roots, path)?;
    let root = root.canonicalize().map_err(io)?;
    let Some((leaf, parent)) = parts.split_last() else {
        return Ok(root);
    };
    let dir = Dir::open(&root)
        .and_then(|dir| dir.dir(parent))
        .map_err(io)?;
    match dir.kind(leaf).map_err(io)? {
        None => Err(bad("nothing is at that path")),
        Some(kind) if kind.is_symlink() => Err(bad("the path is a link, which MaxHQ won't follow")),
        Some(_) => Ok(parts.iter().fold(root, |path, part| path.join(part))),
    }
}

/// Finds the file at `path` for Show anyway, wherever it is, since you
/// asked for it by name. The path must be absolute, and links on the way
/// are followed. What it names must be a regular file of a kind the viewer
/// opens, by both the name asked for and the name it resolves to, and no
/// larger than the media cache takes.
pub fn named(path: &str) -> Result<PathBuf, Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadPath, None, message);
    if path.contains('\0') {
        return Err(bad("the path has a zero byte"));
    }
    let wanted = Path::new(path);
    if !wanted.is_absolute() {
        return Err(bad("Show anyway needs an absolute path"));
    }
    let shown = |path: &Path| path.to_str().and_then(maxhq_client::media::extension);
    if shown(wanted).is_none() {
        return Err(Failure::new(
            ErrorCode::NotAllowed,
            None,
            "that isn't a file MaxHQ shows",
        ));
    }
    let found = wanted.canonicalize().map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => bad("there's no file at that path"),
        _ => Failure::io(&err, None),
    })?;
    if shown(&found).is_none() {
        return Err(Failure::new(
            ErrorCode::NotAllowed,
            None,
            "the path links to a file MaxHQ doesn't show",
        ));
    }
    let meta = std::fs::metadata(&found).map_err(|err| Failure::io(&err, None))?;
    if !meta.is_file() {
        return Err(bad("Show anyway opens only a file"));
    }
    if meta.len() > maxhq_client::media::MOST {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            None,
            "the file is larger than MaxHQ shows",
        ));
    }
    Ok(found)
}

/// The folder in `roots` that `path` is under, named as `roots` names it,
/// and the parts of the path below it.
fn below(roots: &[PathBuf], path: &str) -> Result<(PathBuf, Vec<String>), Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadPath, None, message);
    if path.contains('\0') {
        return Err(bad("the path has a zero byte"));
    }
    let wanted = Path::new(path);
    let (root, rest) = if wanted.is_absolute() {
        roots
            .iter()
            .find_map(|root| {
                let rest = wanted.strip_prefix(root).ok().or_else(|| {
                    let real = root.canonicalize().ok()?;
                    wanted.strip_prefix(real).ok()
                })?;
                Some((root.clone(), rest.to_owned()))
            })
            .ok_or_else(|| {
                Failure::new(
                    ErrorCode::Outside,
                    None,
                    "the path is outside the agent's folders",
                )
            })?
    } else {
        let first = roots
            .first()
            .ok_or_else(|| bad("there's no folder to look in"))?;
        (first.clone(), wanted.to_owned())
    };
    let mut parts = Vec::new();
    for part in rest.components() {
        match part {
            Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| bad("the path isn't UTF-8"))?
                    .to_owned(),
            ),
            Component::CurDir => {}
            _ => return Err(bad("the path has a `..` part")),
        }
    }
    Ok((root, parts))
}

/// A tick for fsyncing and sending credit that moved, a second after the
/// last.
pub fn ticker() -> Interval {
    let mut tick = interval_at(tokio::time::Instant::now() + TICK, TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tick
}

/// Removes a folder an upload landed in, if it's there.
pub async fn remove(dir: &Path) {
    let dir = dir.to_owned();
    let _ = blocking(move || {
        match maxhq_transfer::remove_tree(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => debug!(%err, dir = %dir.display(), "couldn't remove an upload's folder"),
        }
        Ok(())
    })
    .await;
}
