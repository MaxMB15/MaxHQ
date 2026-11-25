// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The sending end of a transfer: walking a file or folder into a manifest,
//! reading its bytes end to end and keeping to the credit it's given.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use maxhq_protocol::message::{EntryKind, ErrorCode, ManifestEntry};
use sha2::{Digest, Sha256};

use crate::beneath::{self, Dir, FileType};
use crate::manifest::{self, Layout};
use crate::{name, Failure, MAX_BYTES, MAX_ENTRIES};

/// A file or folder walked into a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    /// The name it shows as, in NFC.
    pub name: String,
    /// Every file, folder and link, folders before what's in them.
    pub entries: Vec<ManifestEntry>,
    /// Where each file's bytes are read from, one for each entry and `None`
    /// for all but files.
    pub sources: Vec<Option<PathBuf>>,
    /// What was left out, and why.
    pub skipped: Vec<Skipped>,
}

impl Walk {
    /// Bytes in every file.
    pub fn total(&self) -> u64 {
        Layout::of(&self.entries).total()
    }
}

/// Something a walk left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The path, as it would have been in the manifest.
    pub path: String,
    /// Why.
    pub why: Why,
}

/// Why a walk left something out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Why {
    /// A `.git` folder or `.DS_Store`.
    #[error("left out on purpose")]
    Ignored,
    /// A second name for a file that belongs to someone else.
    #[error("a second name for someone else's file")]
    Foreign,
    /// A link that points outside what's sent, or through another link.
    #[error("a link that points outside")]
    LinkOutside,
    /// A device, socket or pipe.
    #[error("not a file, folder or link")]
    NotAFile,
    /// A name that isn't text, is too long or repeats once in NFC.
    #[error("a name that can't be sent")]
    BadName,
}

/// What a walk found before hashing.
struct Found {
    parts: Vec<String>,
    kind: What,
    path: PathBuf,
    size: u64,
    executable: bool,
}

enum What {
    File,
    Dir,
    Link(String),
}

/// Walks a file or folder into a manifest, hashing every file.
///
/// The item itself is followed if it's a link, since it's what they chose.
/// Nothing inside is: a link that stays inside is kept as a link and one
/// that points outside is skipped. Devices, sockets, pipes, `.git` folders
/// and `.DS_Store` files are skipped too. Hard links are sent as copies.
/// The walk stops early once it passes [`MAX_ENTRIES`] or [`MAX_BYTES`].
pub fn walk(chosen: &Path) -> Result<Walk, Failure> {
    let path = &std::fs::canonicalize(chosen).map_err(|err| Failure::io(&err, None))?;
    let meta = std::fs::symlink_metadata(path).map_err(|err| Failure::io(&err, None))?;
    let raw = chosen
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| Failure::new(ErrorCode::BadPath, None, "the name isn't text"))?;
    let top = name::part(raw)
        .map_err(|err| Failure::new(ErrorCode::BadPath, None, format!("{raw}: {err}")))?;
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    if meta.is_file() {
        found.push(Found {
            parts: vec![top.clone()],
            kind: What::File,
            path: path.to_owned(),
            size: meta.len(),
            executable: beneath::executable(&meta),
        });
    } else if meta.is_dir() {
        found.push(Found {
            parts: vec![top.clone()],
            kind: What::Dir,
            path: path.to_owned(),
            size: 0,
            executable: false,
        });
        let mut total = 0u64;
        let opened = Dir::open(path).map_err(|err| Failure::io(&err, None))?;
        gather(
            &opened,
            &[],
            std::slice::from_ref(&top),
            &mut found,
            &mut skipped,
            &mut total,
            None,
        )?;
        for item in &mut found[1..] {
            item.path = path.join(&item.path);
        }
    } else {
        return Err(Failure::new(
            ErrorCode::BadPath,
            None,
            "only files and folders can be sent",
        ));
    }
    drop_outside_links(&mut found, &mut skipped);
    finish(top, found, skipped, hash_file)
}

/// A walk of a folder that stays open while its files are read, for a
/// caller running as root over files someone else can change at any moment.
///
/// Every read starts from that folder and no link on the way is followed,
/// so a link swapped in after the walk stops the read instead of sending
/// what it points at.
#[derive(Debug)]
pub struct Beneath {
    held: Held,
    walk: Walk,
}

impl Beneath {
    /// Every file, folder and link, folders before what's in them.
    pub fn entries(&self) -> &[ManifestEntry] {
        &self.walk.entries
    }

    /// Reads the files this walk found.
    pub fn source(self) -> Source {
        Source {
            layout: Layout::of(&self.walk.entries),
            paths: self.walk.sources,
            held: Some(self.held),
            open: None,
            bytes: None,
        }
    }

    /// Reads the files this walk found through a second handle on its
    /// folder, so a copy that has to start over can take a fresh one.
    pub fn reader(&self) -> io::Result<Source> {
        let held = Held {
            top: self.held.top.dir(&[])?,
            owner: self.held.owner,
        };
        Ok(Source {
            layout: Layout::of(&self.walk.entries),
            paths: self.walk.sources.clone(),
            held: Some(held),
            open: None,
            bytes: None,
        })
    }
}

/// An open folder, and whose files may have a second name in it.
#[derive(Debug)]
struct Held {
    top: Dir,
    /// When set, a file with a second name is read only if this user owns
    /// it. Someone who can't read a file can't get it sent by linking it
    /// into their folder.
    owner: Option<u32>,
}

impl Held {
    /// Opens the file at `path` below the folder, without following a link.
    fn open(&self, path: &Path) -> io::Result<File> {
        let parts = path
            .iter()
            .map(|part| part.to_str().map(str::to_owned).ok_or_else(changed))
            .collect::<io::Result<Vec<String>>>()?;
        let (file, folders) = parts.split_last().ok_or_else(changed)?;
        self.open_in(&self.top.dir(folders)?, file)
    }

    /// Opens the file `name` in `parent`, a folder below this one.
    fn open_in(&self, parent: &Dir, name: &str) -> io::Result<File> {
        let file = parent.read_regular(name)?;
        let stat = beneath::stat_file(&file)?;
        if foreign(stat.second_name(), stat.owner(), self.owner) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                Why::Foreign.to_string(),
            ));
        }
        Ok(file)
    }
}

/// Whether a file is a second name for one `owner` doesn't own.
fn foreign(second_name: bool, uid: Option<u32>, owner: Option<u32>) -> bool {
    owner.is_some_and(|owner| second_name && uid != Some(owner))
}

/// [`walk`] for `parts` below the open folder `top`, kept open for the
/// reads. No link is followed, the item itself included. It shows as `shown`.
pub fn walk_beneath(
    top: Dir,
    shown: &str,
    parts: &[String],
    owner: Option<u32>,
) -> Result<Beneath, Failure> {
    let bad = |message: &str| Failure::new(ErrorCode::BadPath, None, message);
    let io = |err: io::Error| Failure::io(&err, None);
    let name = name::part(shown)
        .map_err(|err| Failure::new(ErrorCode::BadPath, None, format!("{shown}: {err}")))?;
    let held = Held { top, owner };
    let chosen: PathBuf = parts.iter().collect();
    let kind = match parts.split_last() {
        None => FileType::Directory,
        Some((leaf, parent)) => held
            .top
            .dir(parent)
            .and_then(|parent| parent.stat(leaf))
            .map(|stat| stat.kind())
            .map_err(io)?,
    };
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    match kind {
        FileType::Directory => {
            found.push(Found {
                parts: vec![name.clone()],
                kind: What::Dir,
                path: chosen,
                size: 0,
                executable: false,
            });
            let mut total = 0u64;
            gather(
                &held.top,
                parts,
                std::slice::from_ref(&name),
                &mut found,
                &mut skipped,
                &mut total,
                owner,
            )?;
        }
        FileType::RegularFile => {
            let meta = held
                .open(&chosen)
                .and_then(|file| file.metadata())
                .map_err(io)?;
            found.push(Found {
                parts: vec![name.clone()],
                kind: What::File,
                path: chosen,
                size: meta.len(),
                executable: beneath::executable(&meta),
            });
        }
        FileType::Symlink => return Err(bad("the path is a link, which MaxHQ won't follow")),
        _ => return Err(bad("only files and folders can be sent")),
    }
    drop_outside_links(&mut found, &mut skipped);
    let hash = |path: &Path, size| hash_open(held.open(path)?, size);
    let walk = finish(name, found, skipped, hash)?;
    Ok(Beneath { held, walk })
}

/// Turns what a walk found into its manifest, hashing each file.
fn finish(
    name: String,
    found: Vec<Found>,
    skipped: Vec<Skipped>,
    hash: impl Fn(&Path, u64) -> io::Result<[u8; 32]>,
) -> Result<Walk, Failure> {
    let mut entries = Vec::with_capacity(found.len());
    let mut sources = Vec::with_capacity(found.len());
    for (index, item) in found.into_iter().enumerate() {
        let at = u32::try_from(index).ok();
        let (kind, sha256) = match item.kind {
            What::File => {
                let hash = hash(&item.path, item.size).map_err(|err| Failure::io(&err, at))?;
                (EntryKind::File, Some(manifest::to_hex(&hash)))
            }
            What::Dir => (EntryKind::Dir, None),
            What::Link(target) => (EntryKind::Symlink { target }, None),
        };
        sources.push(matches!(kind, EntryKind::File).then_some(item.path));
        entries.push(ManifestEntry {
            path: item.parts.join("/"),
            kind,
            executable: item.executable,
            size: item.size,
            sha256,
        });
    }
    Ok(Walk {
        name,
        entries,
        sources,
        skipped,
    })
}

/// Walks chosen files below `root` into one manifest, such as the ignored
/// files a project copy brings along, hashing each.
///
/// Each path is relative to `root`, with `/` between parts, and must name
/// a regular file. A link, `..`, an absolute path, a `.git` part or another
/// name a walk leaves out is refused, and so is a link on the way. The
/// folders above the files come first, so the files land in the same
/// tree. A path given twice is sent once. Everything sits under one top
/// folder named after `root`, the way a walked folder does.
pub fn walk_files(root: &Path, paths: &[String]) -> Result<Walk, Failure> {
    if paths.is_empty() {
        return Err(no_files());
    }
    let raw = root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| Failure::new(ErrorCode::BadPath, None, "the name isn't text"))?;
    let top = Dir::open(root).map_err(|err| Failure::io(&err, None))?;
    let mut walk = files_in(&Held { top, owner: None }, raw, paths)?;
    for source in walk.sources.iter_mut().flatten() {
        *source = root.join(&*source);
    }
    Ok(walk)
}

/// [`walk_files`] below the open folder `top`, kept open for the reads, as
/// in [`walk_beneath`].
pub fn walk_files_beneath(
    top: Dir,
    shown: &str,
    paths: &[String],
    owner: Option<u32>,
) -> Result<Beneath, Failure> {
    if paths.is_empty() {
        return Err(no_files());
    }
    let held = Held { top, owner };
    let walk = files_in(&held, shown, paths)?;
    Ok(Beneath { held, walk })
}

fn no_files() -> Failure {
    Failure::new(ErrorCode::BadPath, None, "no files were chosen")
}

/// The manifest of chosen files, each read from the held folder. Sources
/// are relative to that folder.
fn files_in(held: &Held, shown: &str, paths: &[String]) -> Result<Walk, Failure> {
    let name = name::part(shown)
        .map_err(|err| Failure::new(ErrorCode::BadPath, None, format!("{shown}: {err}")))?;
    let top = &held.top;
    // A folder above a chosen file is `None`.
    let mut items: BTreeMap<Vec<String>, Option<ManifestEntry>> = BTreeMap::new();
    let mut sources: BTreeMap<Vec<String>, PathBuf> = BTreeMap::new();
    let mut total = 0u64;
    for path in paths {
        let bad = |why: &dyn std::fmt::Display| {
            Failure::new(ErrorCode::BadPath, None, format!("{path}: {why}"))
        };
        if path.starts_with('/') {
            return Err(bad(&name::BadPath::Absolute));
        }
        name::check_depth(path).map_err(|err| bad(&err))?;
        let raw: Vec<String> = path.split('/').map(str::to_owned).collect();
        let parts = raw
            .iter()
            .map(|part| name::part(part))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| bad(&err))?;
        if parts.iter().any(|part| name::skipped(part)) {
            return Err(bad(&Why::Ignored));
        }
        match items.get(&parts) {
            Some(Some(_)) => continue,
            Some(None) => return Err(bad(&"a folder stands where a file is")),
            None => {}
        }
        for depth in 1..parts.len() {
            if let Some(Some(_)) = items.get(&parts[..depth]) {
                return Err(bad(&"a file stands where a folder is"));
            }
            items.entry(parts[..depth].to_vec()).or_insert(None);
        }
        let (file, folders) = raw.split_last().expect("split gives a part");
        let parent = top.dir(folders).map_err(|err| bad(&err))?;
        // A pipe would block an open, so the kind is checked first.
        let not_file = || bad(&"not a regular file");
        if parent.kind(file).map_err(|err| bad(&err))? != Some(FileType::RegularFile) {
            return Err(not_file());
        }
        let opened = held.open_in(&parent, file).map_err(|err| bad(&err))?;
        let meta = opened.metadata().map_err(|err| bad(&err))?;
        total = total.saturating_add(meta.len());
        if items.len() >= MAX_ENTRIES {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!("it holds over {MAX_ENTRIES} files and folders"),
            ));
        }
        if total > MAX_BYTES {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!("it holds over {}", crate::human(MAX_BYTES)),
            ));
        }
        let hash = hash_open(opened, meta.len()).map_err(|err| bad(&err))?;
        sources.insert(parts.clone(), PathBuf::from(path));
        items.insert(
            parts.clone(),
            Some(ManifestEntry {
                path: String::new(),
                kind: EntryKind::File,
                executable: beneath::executable(&meta),
                size: meta.len(),
                sha256: Some(manifest::to_hex(&hash)),
            }),
        );
    }
    let folder = |path: String| ManifestEntry {
        path,
        kind: EntryKind::Dir,
        executable: false,
        size: 0,
        sha256: None,
    };
    let mut entries = Vec::with_capacity(items.len() + 1);
    let mut from = Vec::with_capacity(items.len() + 1);
    entries.push(folder(name.clone()));
    from.push(None);
    for (parts, entry) in items {
        from.push(sources.remove(&parts));
        let path = format!("{name}/{}", parts.join("/"));
        entries.push(match entry {
            Some(file) => ManifestEntry { path, ..file },
            None => folder(path),
        });
    }
    Ok(Walk {
        name,
        entries,
        sources: from,
        skipped: Vec::new(),
    })
}

/// One folder a walk is partway through. It keeps names and no descriptor,
/// so depth doesn't hold descriptors.
struct Level {
    at: Vec<String>,
    parts: Vec<String>,
    names: std::vec::IntoIter<(String, String)>,
    seen: HashSet<String>,
}

impl Level {
    /// Reads a folder's names, sorted, and notes the ones it can't use.
    fn read(
        dir: &Dir,
        at: Vec<String>,
        parts: Vec<String>,
        skipped: &mut Vec<Skipped>,
    ) -> Result<Self, Failure> {
        let mut named: Vec<(String, String)> = Vec::new();
        for raw in dir.raw_names().map_err(|err| Failure::io(&err, None))? {
            let shown = format!("{}/{}", parts.join("/"), raw.to_string_lossy());
            let text = raw.to_str();
            match text.map(|text| name::part(text).map(|nfc| (nfc, text.to_owned()))) {
                Some(Ok(pair)) => named.push(pair),
                _ => skipped.push(Skipped {
                    path: shown,
                    why: Why::BadName,
                }),
            }
        }
        named.sort();
        Ok(Self {
            at,
            parts,
            names: named.into_iter(),
            seen: HashSet::new(),
        })
    }
}

/// Lists the contents of the folder `at` below `top` into `found`, sorted,
/// folders before what's in them. Each found path is relative to `top`.
/// It never follows a link, and `owner` is as in [`Held`]. It keeps its own
/// list of folders, so a deep tree can't use up the stack, and has one folder
/// open at a time.
fn gather(
    top: &Dir,
    at: &[String],
    parent: &[String],
    found: &mut Vec<Found>,
    skipped: &mut Vec<Skipped>,
    total: &mut u64,
    owner: Option<u32>,
) -> Result<(), Failure> {
    let fail = |err: io::Error| Failure::io(&err, None);
    let mut dir = top.dir(at).map_err(fail)?;
    let mut levels = vec![Level::read(&dir, at.to_vec(), parent.to_vec(), skipped)?];
    while let Some(level) = levels.last_mut() {
        let Some((nfc, raw)) = level.names.next() else {
            levels.pop();
            if let Some(level) = levels.last() {
                dir = top.dir(&level.at).map_err(fail)?;
            }
            continue;
        };
        let mut parts = level.parts.clone();
        parts.push(nfc.clone());
        let shown = parts.join("/");
        if name::check_depth(&shown).is_err() {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!(
                    "it holds a path over {} folders deep or {} bytes long",
                    name::MAX_DEPTH,
                    name::MAX_PATH
                ),
            ));
        }
        if name::skipped(&nfc) {
            skipped.push(Skipped {
                path: shown,
                why: Why::Ignored,
            });
            continue;
        }
        if !level.seen.insert(nfc) {
            skipped.push(Skipped {
                path: shown,
                why: Why::BadName,
            });
            continue;
        }
        let mut below = level.at.clone();
        below.push(raw.clone());
        let path: PathBuf = below.iter().collect();
        let stat = dir.stat(&raw).map_err(fail)?;
        let item = match stat.kind() {
            FileType::Directory => Found {
                parts: parts.clone(),
                kind: What::Dir,
                path,
                size: 0,
                executable: false,
            },
            FileType::RegularFile => {
                if foreign(stat.second_name(), stat.owner(), owner) {
                    skipped.push(Skipped {
                        path: shown,
                        why: Why::Foreign,
                    });
                    continue;
                }
                let size = stat.size().unwrap_or(u64::MAX);
                *total = total.saturating_add(size);
                Found {
                    parts: parts.clone(),
                    kind: What::File,
                    path,
                    size,
                    executable: stat.executable(),
                }
            }
            FileType::Symlink => {
                let target = dir.read_link(&raw).map_err(fail)?;
                let Some(target) = target.to_str().map(str::to_owned) else {
                    skipped.push(Skipped {
                        path: shown,
                        why: Why::BadName,
                    });
                    continue;
                };
                Found {
                    parts: parts.clone(),
                    kind: What::Link(target),
                    path,
                    size: 0,
                    executable: false,
                }
            }
            _ => {
                skipped.push(Skipped {
                    path: shown,
                    why: Why::NotAFile,
                });
                continue;
            }
        };
        let is_dir = matches!(item.kind, What::Dir);
        found.push(item);
        if found.len() > MAX_ENTRIES {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!("it holds over {MAX_ENTRIES} files and folders"),
            ));
        }
        if *total > MAX_BYTES {
            return Err(Failure::new(
                ErrorCode::TooLarge,
                None,
                format!("it holds over {}", crate::human(MAX_BYTES)),
            ));
        }
        if is_dir {
            dir = top.dir(&below).map_err(fail)?;
            levels.push(Level::read(&dir, below, parts, skipped)?);
        }
    }
    Ok(())
}

/// Skips links whose targets leave the chosen folder, by the rule a
/// receiver checks with the folder as its root.
fn drop_outside_links(found: &mut Vec<Found>, skipped: &mut Vec<Skipped>) {
    let links: HashSet<&[String]> = found
        .iter()
        .filter(|item| matches!(item.kind, What::Link(_)))
        .map(|item| &item.parts[1..])
        .collect();
    let outside: HashSet<Vec<String>> = found
        .iter()
        .filter(|item| {
            let What::Link(target) = &item.kind else {
                return false;
            };
            let parent = &item.parts[1..item.parts.len() - 1];
            !name::link_stays_inside(parent, target, |path| links.contains(path))
        })
        .map(|item| item.parts.clone())
        .collect();
    found.retain(|item| {
        if !outside.contains(&item.parts) {
            return true;
        }
        skipped.push(Skipped {
            path: item.parts.join("/"),
            why: Why::LinkOutside,
        });
        false
    });
}

/// The SHA-256 of a file's first `size` bytes, failing if it has fewer.
fn hash_file(path: &Path, size: u64) -> io::Result<[u8; 32]> {
    hash_open(beneath::open_nofollow(path)?, size)
}

/// [`hash_file`] for a file already open.
fn hash_open(file: File, size: u64) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut reader = file.take(size);
    let mut buf = vec![0u8; 256 * 1024];
    let mut read = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        read += n as u64;
    }
    if read != size {
        return Err(changed());
    }
    Ok(hasher.finalize().into())
}

fn changed() -> io::Error {
    io::Error::other("a file changed while it was read")
}

/// Reads a transfer's bytes at any offset, across files laid end to end.
#[derive(Debug)]
pub struct Source {
    layout: Layout,
    paths: Vec<Option<PathBuf>>,
    /// The folder `paths` are relative to, when they aren't absolute.
    held: Option<Held>,
    open: Option<(usize, File)>,
    bytes: Option<Vec<u8>>,
}

impl Source {
    /// Reads the files a walk found.
    pub fn new(walk: &Walk) -> Self {
        Self {
            layout: Layout::of(&walk.entries),
            paths: walk.sources.clone(),
            held: None,
            open: None,
            bytes: None,
        }
    }

    /// Reads one file's bytes held in memory, such as a pasted image.
    pub fn bytes(bytes: Vec<u8>) -> Self {
        Self {
            layout: Layout::new([bytes.len() as u64]),
            paths: vec![None],
            held: None,
            open: None,
            bytes: Some(bytes),
        }
    }

    /// Bytes in the whole transfer.
    pub fn total(&self) -> u64 {
        self.layout.total()
    }

    /// The entry holding the byte at `offset`.
    pub fn entry_at(&self, offset: u64) -> Option<usize> {
        self.layout.locate(offset).map(|(index, _)| index)
    }

    /// Up to `max` bytes from `offset`, fewer only at the end.
    pub fn read(&mut self, offset: u64, max: usize) -> io::Result<Vec<u8>> {
        let end = offset.saturating_add(max as u64).min(self.layout.total());
        let mut out = Vec::with_capacity(usize::try_from(end.saturating_sub(offset)).unwrap_or(0));
        let mut at = offset;
        while at < end {
            let (index, within) = self.layout.locate(at).ok_or_else(changed)?;
            let left = self.layout.end(index) - at;
            let take = usize::try_from(left.min(end - at)).unwrap_or(usize::MAX);
            let start = out.len();
            out.resize(start + take, 0);
            if let Some(bytes) = &self.bytes {
                let from = usize::try_from(within).map_err(|_| changed())?;
                out[start..].copy_from_slice(&bytes[from..from + take]);
            } else {
                let file = self.file(index)?;
                beneath::read_exact_at(file, &mut out[start..], within).map_err(|err| match err
                    .kind()
                {
                    io::ErrorKind::UnexpectedEof => changed(),
                    _ => err,
                })?;
            }
            at += take as u64;
        }
        Ok(out)
    }

    fn file(&mut self, index: usize) -> io::Result<&File> {
        if self.open.as_ref().is_none_or(|(open, _)| *open != index) {
            let path = self
                .paths
                .get(index)
                .and_then(Option::as_ref)
                .ok_or_else(changed)?;
            let opened = match &self.held {
                Some(held) => held.open(path)?,
                None => beneath::open_nofollow(path)?,
            };
            self.open = Some((index, opened));
        }
        Ok(&self.open.as_ref().expect("just opened").1)
    }
}

/// How far an upload got, and how far it may go: the sender's half of the
/// credit window.
///
/// It checks every credit it's given, since the receiver may be a guest
/// the host doesn't trust: offsets only move forward, and neither may pass
/// the transfer's end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    total: u64,
    sent: u64,
    window_end: u64,
    committed: u64,
    restarts: u32,
}

/// How many times a file whose hash didn't match is sent again before the
/// upload gives up.
pub const RESTARTS: u32 = 2;

impl Outgoing {
    /// An upload of `total` bytes, with no credit yet.
    pub fn new(total: u64) -> Self {
        Self {
            total,
            sent: 0,
            window_end: 0,
            committed: 0,
            restarts: 0,
        }
    }

    /// Takes a credit from the receiver.
    pub fn credit(&mut self, committed: u64, window_end: u64) -> Result<(), Failure> {
        self.check(committed, window_end)?;
        self.window_end = self.window_end.max(window_end);
        self.committed = self.committed.max(committed);
        Ok(())
    }

    /// Picks up from where the receiver says it has bytes for good, after a
    /// dropped link or a file whose hash didn't match.
    pub fn resume(&mut self, committed: u64, window_end: u64) -> Result<(), Failure> {
        self.check(committed, window_end)?;
        self.sent = committed;
        self.committed = committed;
        self.window_end = window_end;
        Ok(())
    }

    /// Counts a file sent again after its hash didn't match, and says
    /// whether that's allowed. It isn't once [`RESTARTS`] are used up.
    pub fn restart(&mut self) -> bool {
        if self.restarts >= RESTARTS {
            return false;
        }
        self.restarts += 1;
        true
    }

    fn check(&self, committed: u64, window_end: u64) -> Result<(), Failure> {
        if committed > self.total || window_end > self.total || committed > window_end {
            return Err(Failure::new(
                ErrorCode::BadMessage,
                None,
                "a credit runs past the end of the transfer",
            ));
        }
        Ok(())
    }

    /// The next chunk to send, as an offset and a length up to `max`, or
    /// `None` while the credit is used up or everything is sent.
    pub fn next(&self, max: usize) -> Option<(u64, usize)> {
        if self.sent >= self.window_end {
            return None;
        }
        let len = (self.window_end - self.sent).min(max as u64);
        Some((self.sent, usize::try_from(len).unwrap_or(max)))
    }

    /// Notes that `len` bytes from the current offset went out.
    pub fn sent_chunk(&mut self, len: usize) {
        self.sent = (self.sent + len as u64).min(self.window_end);
    }

    /// Bytes sent so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Bytes the receiver has for good.
    pub fn committed(&self) -> u64 {
        self.committed
    }

    /// How far the receiver lets the sender go.
    pub fn window_end(&self) -> u64 {
        self.window_end
    }

    /// Bytes in the whole upload.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Whether every byte is out.
    pub fn is_sent(&self) -> bool {
        self.sent >= self.total
    }

    /// Whether the receiver has every byte for good.
    pub fn is_committed(&self) -> bool {
        self.committed >= self.total
    }

    /// Whether the sender waits for credit with bytes still to send.
    pub fn is_waiting(&self) -> bool {
        self.sent >= self.window_end && self.sent < self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receive::{sha256_hex, Receiver};
    use crate::testing::{self, TempDir};
    use crate::Room;
    use std::time::Instant;

    const ROOM: Room = Room {
        keep_bytes: 0,
        keep_percent: 0,
        keep_at_most: 0,
    };

    fn write(path: &Path, bytes: &[u8], mode: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
        maxhq_os::fsmode::set_mode(path, mode).unwrap();
    }

    fn paths(walk: &Walk) -> Vec<&str> {
        walk.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }

    /// Makes `depth` folders one inside the next under `root`.
    fn nest(root: &Path, depth: usize) {
        let mut at = crate::beneath::Dir::open(root).unwrap();
        for _ in 0..depth {
            at.make_dir("d", 0o755).unwrap();
            at = at.dir(&["d".to_owned()]).unwrap();
        }
    }

    #[test]
    fn a_walk_goes_as_deep_as_a_receiver_takes_and_refuses_past_that() {
        let dir = TempDir::new();
        // The chosen folder is the first part of every path.
        let fits = dir.path().join("fits");
        std::fs::create_dir(&fits).unwrap();
        nest(&fits, name::MAX_DEPTH - 1);
        let walked = walk(&fits).unwrap();
        assert_eq!(walked.entries.len(), name::MAX_DEPTH);
        assert!(manifest::check(&walked.entries).is_ok());

        for depth in [name::MAX_DEPTH, 600] {
            let over = dir.path().join(format!("over{depth}"));
            std::fs::create_dir(&over).unwrap();
            nest(&over, depth);
            let refused = walk(&over).unwrap_err();
            assert_eq!(refused.code, ErrorCode::TooLarge, "{}", refused.message);
            crate::remove_tree(&over).unwrap();
        }
    }

    #[test]
    fn a_walk_of_chosen_files_refuses_a_path_too_deep() {
        let dir = TempDir::new();
        let path = vec!["d"; name::MAX_DEPTH + 1].join("/");
        let refused = walk_files(dir.path(), &[path]).unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadPath);
        assert!(
            refused.message.contains("folders deep"),
            "{}",
            refused.message
        );
    }

    /// Sends a walk to a receiver through the credit window.
    fn send(walk: &Walk, receiver: &mut Receiver) {
        let mut source = Source::new(walk);
        let mut outgoing = Outgoing::new(source.total());
        let now = Instant::now();
        if let Some((committed, end)) = receiver.credit(now) {
            outgoing.credit(committed, end).unwrap();
        }
        while let Some((offset, len)) = outgoing.next(maxhq_protocol::frame::MAX_TRANSFER_CHUNK) {
            let bytes = source.read(offset, len).unwrap();
            receiver.write(offset, &bytes, now).unwrap();
            outgoing.sent_chunk(bytes.len());
            if let Some((committed, end)) = receiver.credit(now) {
                outgoing.credit(committed, end).unwrap();
            }
        }
        assert!(outgoing.is_sent());
        assert!(receiver.is_done());
    }

    #[test]
    fn a_folder_walks_sorted_with_hashes_and_modes() {
        let temp = TempDir::new();
        let root = temp.path().join("project");
        write(&root.join("run.sh"), b"#!/bin/sh\n", 0o755);
        write(&root.join("src/main.rs"), b"fn main() {}\n", 0o600);
        write(&root.join("a.txt"), b"a", 0o644);
        std::fs::create_dir_all(root.join("empty")).unwrap();
        let walk = walk(&root).unwrap();
        assert_eq!(walk.name, "project");
        assert_eq!(
            paths(&walk),
            [
                "project",
                "project/a.txt",
                "project/empty",
                "project/run.sh",
                "project/src",
                "project/src/main.rs"
            ]
        );
        let run = &walk.entries[3];
        // Windows keeps no execute bit.
        assert_eq!(run.executable, cfg!(unix));
        assert_eq!(
            run.sha256.as_deref(),
            Some(sha256_hex(b"#!/bin/sh\n").as_str())
        );
        assert!(!walk.entries[5].executable);
        assert_eq!(walk.total(), 10 + 13 + 1);
        assert!(walk.sources[0].is_none() && walk.sources[1].is_some());
        assert!(manifest::check(&walk.entries).is_ok());
    }

    #[test]
    fn git_folders_finder_files_and_odd_kinds_are_left_out() {
        let temp = TempDir::new();
        let root = temp.path().join("repo");
        write(&root.join(".git/HEAD"), b"ref", 0o644);
        write(&root.join(".DS_Store"), b"junk", 0o644);
        write(&root.join(".gitignore"), b"target\n", 0o644);
        let fifo = root.join("pipe");
        let made = cfg!(unix)
            && std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .is_ok_and(|status| status.success());
        let walk = walk(&root).unwrap();
        assert_eq!(paths(&walk), ["repo", "repo/.gitignore"]);
        let why: Vec<(&str, Why)> = walk
            .skipped
            .iter()
            .map(|skip| (skip.path.as_str(), skip.why))
            .collect();
        assert!(why.contains(&("repo/.git", Why::Ignored)));
        assert!(why.contains(&("repo/.DS_Store", Why::Ignored)));
        if made {
            assert!(why.contains(&("repo/pipe", Why::NotAFile)));
        }
    }

    #[test]
    fn links_inside_are_kept_and_links_outside_are_never_followed() {
        let temp = TempDir::new();
        let secret = temp.path().join("secret.txt");
        write(&secret, b"private", 0o600);
        let root = temp.path().join("notes");
        write(&root.join("today.md"), b"hello", 0o644);
        if !testing::symlink("today.md", &root.join("latest")) {
            return;
        }
        testing::symlink(&secret, &root.join("absolute"));
        testing::symlink("../secret.txt", &root.join("climbs"));
        testing::dir_link(temp.path(), &root.join("folder"));
        let walk = walk(&root).unwrap();
        assert_eq!(paths(&walk), ["notes", "notes/latest", "notes/today.md"]);
        assert_eq!(
            walk.entries[1].kind,
            EntryKind::Symlink {
                target: "today.md".into()
            }
        );
        let outside: Vec<&str> = walk
            .skipped
            .iter()
            .filter(|skip| skip.why == Why::LinkOutside)
            .map(|skip| skip.path.as_str())
            .collect();
        assert_eq!(outside, ["notes/absolute", "notes/climbs", "notes/folder"]);
        // Nothing from outside made it into the bytes.
        let mut source = Source::new(&walk);
        assert_eq!(source.read(0, 1024).unwrap(), b"hello");
    }

    #[test]
    fn the_chosen_item_is_followed_once_if_it_is_a_link() {
        let temp = TempDir::new();
        let real = temp.path().join("real.txt");
        write(&real, b"bytes", 0o644);
        let chosen = temp.path().join("alias.txt");
        if !testing::symlink(&real, &chosen) {
            return;
        }
        let walk = walk(&chosen).unwrap();
        assert_eq!(walk.name, "alias.txt");
        assert_eq!(walk.entries[0].kind, EntryKind::File);
        assert_eq!(walk.entries[0].size, 5);
    }

    #[test]
    fn names_in_nfd_are_sent_in_nfc() {
        let temp = TempDir::new();
        let root = temp.path().join("d");
        write(&root.join("cafe\u{301}.txt"), b"x", 0o644);
        let walk = walk(&root).unwrap();
        assert_eq!(paths(&walk), ["d", "d/caf\u{e9}.txt"]);
        let mut source = Source::new(&walk);
        assert_eq!(source.read(0, 10).unwrap(), b"x");
    }

    #[test]
    fn a_folder_arrives_whole_through_the_window() {
        let temp = TempDir::new();
        let root = temp.path().join("site");
        let big: Vec<u8> = (0..3_000_000u32).map(|n| n as u8).collect();
        write(&root.join("index.html"), b"<h1>hi</h1>", 0o644);
        write(&root.join("assets/big.bin"), &big, 0o644);
        write(&root.join("bin/serve"), b"#!/bin/sh\n", 0o755);
        write(&root.join("empty.txt"), b"", 0o644);
        let linked = testing::symlink("index.html", &root.join("home.html"));
        let walk = walk(&root).unwrap();
        let landing = temp.path().join("landing");
        let mut receiver = Receiver::open(&landing, &walk.entries, ROOM).unwrap();
        send(&walk, &mut receiver);
        let site = landing.join("site");
        assert_eq!(std::fs::read(site.join("assets/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(site.join("empty.txt")).unwrap(), b"");
        if linked && receiver.skipped_links().is_empty() {
            assert_eq!(
                std::fs::read_link(site.join("home.html")).unwrap(),
                Path::new("index.html")
            );
        }
        if let Some(mode) = testing::mode(&site.join("bin/serve")) {
            assert_eq!(mode, 0o755);
        }
        if let Some(mode) = testing::mode(&site.join("index.html")) {
            assert_eq!(mode, 0o644);
        }
    }

    #[test]
    fn a_dropped_link_resumes_from_what_was_committed() {
        let temp = TempDir::new();
        let file = temp.path().join("video.mp4");
        let bytes: Vec<u8> = (0..5_000_000u32).map(|n| (n * 7) as u8).collect();
        write(&file, &bytes, 0o644);
        let walk = walk(&file).unwrap();
        let landing = temp.path().join("landing");
        let mut source = Source::new(&walk);
        let mut outgoing = Outgoing::new(source.total());
        let now = Instant::now();
        {
            let mut receiver = Receiver::open(&landing, &walk.entries, ROOM).unwrap();
            let (committed, end) = receiver.credit(now).unwrap();
            outgoing.credit(committed, end).unwrap();
            for _ in 0..3 {
                let (offset, len) = outgoing.next(256 * 1024).unwrap();
                let chunk = source.read(offset, len).unwrap();
                receiver.write(offset, &chunk, now).unwrap();
                outgoing.sent_chunk(chunk.len());
                if let Some((committed, end)) = receiver.credit(now) {
                    outgoing.credit(committed, end).unwrap();
                }
            }
            // The link drops here, with chunks in flight that never land.
            outgoing.sent_chunk(256 * 1024);
        }
        let mut receiver = Receiver::open(&landing, &walk.entries, ROOM).unwrap();
        let (committed, end) = receiver.resume(now).unwrap();
        assert_eq!(committed, 3 * 256 * 1024);
        outgoing.resume(committed, end).unwrap();
        assert_eq!(outgoing.sent(), committed);
        while let Some((offset, len)) = outgoing.next(256 * 1024) {
            let chunk = source.read(offset, len).unwrap();
            receiver.write(offset, &chunk, now).unwrap();
            outgoing.sent_chunk(chunk.len());
            if let Some((committed, end)) = receiver.credit(now) {
                outgoing.credit(committed, end).unwrap();
            }
        }
        assert!(receiver.is_done());
        assert_eq!(std::fs::read(landing.join("video.mp4")).unwrap(), bytes);
    }

    #[test]
    fn a_file_that_changed_on_the_way_fails_its_hash() {
        let temp = TempDir::new();
        let file = temp.path().join("log.txt");
        write(&file, &[b'a'; 1000], 0o644);
        let walk = walk(&file).unwrap();
        // It changes after the walk hashed it.
        write(&file, &[b'b'; 1000], 0o644);
        let landing = temp.path().join("landing");
        let mut receiver = Receiver::open(&landing, &walk.entries, ROOM).unwrap();
        let now = Instant::now();
        receiver.credit(now);
        let bytes = Source::new(&walk).read(0, 1000).unwrap();
        let failure = receiver.write(0, &bytes, now).unwrap_err();
        assert_eq!(failure.code, ErrorCode::HashMismatch);
        assert_eq!(failure.entry, Some(0));
        let mut outgoing = Outgoing::new(1000);
        assert!(outgoing.restart());
        assert!(outgoing.restart());
        assert!(!outgoing.restart());
        assert!(!landing.join("log.txt").exists());
    }

    #[test]
    fn pasted_bytes_read_like_a_file() {
        let mut source = Source::bytes(b"pasted image".to_vec());
        assert_eq!(source.total(), 12);
        assert_eq!(source.read(7, 100).unwrap(), b"image");
        assert_eq!(source.entry_at(0), Some(0));
    }

    #[test]
    fn credit_from_a_receiver_that_lies_is_refused() {
        let mut outgoing = Outgoing::new(100);
        assert!(outgoing.credit(0, 50).is_ok());
        assert_eq!(outgoing.next(1000), Some((0, 50)));
        outgoing.sent_chunk(50);
        assert!(outgoing.is_waiting());
        assert_eq!(
            outgoing.credit(0, 101).unwrap_err().code,
            ErrorCode::BadMessage
        );
        assert!(outgoing.credit(60, 50).is_err());
        // An old credit doesn't take the window back.
        outgoing.credit(10, 80).unwrap();
        outgoing.credit(5, 60).unwrap();
        assert_eq!((outgoing.committed(), outgoing.window_end()), (10, 80));
        assert_eq!(outgoing.next(1000), Some((50, 30)));
    }

    #[test]
    fn a_walk_stops_once_it_passes_the_entry_limit() {
        let temp = TempDir::new();
        let root = temp.path().join("many");
        std::fs::create_dir_all(&root).unwrap();
        for index in 0..=MAX_ENTRIES {
            File::create(root.join(index.to_string())).unwrap();
        }
        assert_eq!(walk(&root).unwrap_err().code, ErrorCode::TooLarge);
    }

    #[test]
    fn chosen_files_walk_with_their_folders_and_arrive_byte_for_byte() {
        let temp = TempDir::new();
        let root = temp.path().join("checkout");
        let binary: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        write(&root.join(".env"), b"TOKEN=1\n", 0o600);
        write(&root.join("config/keys/dev.bin"), &binary, 0o644);
        write(&root.join("config/run.sh"), b"#!/bin/sh\n", 0o755);
        write(&root.join("src/main.rs"), b"fn main() {}\n", 0o644);
        let chosen = [
            "config/keys/dev.bin".to_owned(),
            ".env".to_owned(),
            "config/run.sh".to_owned(),
            ".env".to_owned(),
        ];
        let walk = walk_files(&root, &chosen).unwrap();
        assert_eq!(
            paths(&walk),
            [
                "checkout",
                "checkout/.env",
                "checkout/config",
                "checkout/config/keys",
                "checkout/config/keys/dev.bin",
                "checkout/config/run.sh"
            ]
        );
        assert_eq!(walk.name, "checkout");
        assert!(walk.entries[5].executable);
        manifest::check(&walk.entries).unwrap();

        let landing = temp.path().join("landing");
        let mut receiver = Receiver::open(&landing, &walk.entries, ROOM).unwrap();
        send(&walk, &mut receiver);
        assert_eq!(
            std::fs::read(landing.join("checkout/config/keys/dev.bin")).unwrap(),
            binary
        );
        assert_eq!(
            std::fs::read(landing.join("checkout/.env")).unwrap(),
            b"TOKEN=1\n"
        );
    }

    #[test]
    fn chosen_files_refuse_links_escapes_and_git() {
        let temp = TempDir::new();
        let root = temp.path().join("checkout");
        write(&root.join(".env"), b"x", 0o644);
        write(&root.join(".git/config"), b"[core]\n", 0o644);
        write(&temp.path().join("outside/secret"), b"secret", 0o644);
        testing::symlink("/etc/hosts", &root.join(".env.link"));
        testing::dir_link(&temp.path().join("outside"), &root.join("linked"));
        std::fs::create_dir_all(root.join("folder")).unwrap();
        for path in [
            ".env.link",
            "linked/secret",
            "../outside/secret",
            "a/../.env",
            "/etc/hosts",
            ".git/config",
            "sub/.git/config",
            ".DS_Store",
            "folder",
            "missing",
            "",
            "a//b",
        ] {
            let refused = walk_files(&root, &[path.to_owned()]).unwrap_err();
            assert_eq!(refused.code, ErrorCode::BadPath, "{path}");
        }
        assert_eq!(walk_files(&root, &[]).unwrap_err().code, ErrorCode::BadPath);
        let both = [".env".to_owned(), ".env/inside".to_owned()];
        assert_eq!(
            walk_files(&root, &both).unwrap_err().code,
            ErrorCode::BadPath
        );
    }

    /// A walk of the folder `pick` in `root`, read through `root`.
    fn beneath(root: &Path, owner: Option<u32>) -> Result<Beneath, Failure> {
        let top = Dir::open(root).unwrap();
        walk_beneath(top, "pick", &["pick".to_owned()], owner)
    }

    #[test]
    fn a_walk_beneath_finds_and_reads_what_a_walk_does() {
        let dir = TempDir::new();
        let pick = dir.path().join("pick");
        write(&pick.join("a.txt"), b"alpha", 0o644);
        write(&pick.join("sub/run.sh"), b"#!/bin/sh\n", 0o755);
        write(&pick.join(".git/config"), b"x", 0o644);
        testing::symlink("../a.txt", &pick.join("sub").join("inside"));
        testing::symlink("/etc/hosts", &pick.join("outside"));
        let plain = walk(&pick).unwrap();
        let held = beneath(dir.path(), None).unwrap();
        assert_eq!(held.entries(), plain.entries);
        assert_eq!(held.walk.skipped, plain.skipped);
        let total = usize::try_from(plain.total()).unwrap();
        let bytes = Source::new(&plain).read(0, total).unwrap();
        assert_eq!(held.source().read(0, total).unwrap(), bytes);
    }

    #[test]
    fn a_read_beneath_stops_at_a_folder_swapped_for_a_link() {
        let dir = TempDir::new();
        let pick = dir.path().join("pick");
        write(&pick.join("sub/file"), b"mine!", 0o644);
        write(&dir.path().join("secret/file"), b"other", 0o600);
        let held = beneath(dir.path(), None).unwrap();
        std::fs::rename(pick.join("sub"), pick.join("old")).unwrap();
        testing::symlink("../secret", &pick.join("sub"));
        assert!(held.source().read(0, 5).is_err());
    }

    #[test]
    fn a_read_beneath_stops_at_a_file_swapped_for_a_link() {
        let dir = TempDir::new();
        let pick = dir.path().join("pick");
        write(&pick.join("file"), b"mine!", 0o644);
        write(&dir.path().join("secret"), b"other", 0o600);
        let held = beneath(dir.path(), None).unwrap();
        std::fs::remove_file(pick.join("file")).unwrap();
        testing::symlink("../secret", &pick.join("file"));
        assert!(held.source().read(0, 5).is_err());
    }

    #[test]
    fn a_second_reader_reads_the_same_and_stops_at_a_link() {
        let dir = TempDir::new();
        let pick = dir.path().join("pick");
        write(&pick.join("file"), b"mine!", 0o644);
        write(&dir.path().join("secret"), b"other", 0o600);
        let held = beneath(dir.path(), None).unwrap();
        assert_eq!(held.reader().unwrap().read(0, 5).unwrap(), b"mine!");
        assert_eq!(held.reader().unwrap().read(0, 5).unwrap(), b"mine!");
        std::fs::remove_file(pick.join("file")).unwrap();
        testing::symlink("../secret", &pick.join("file"));
        assert!(held.reader().unwrap().read(0, 5).is_err());
    }

    #[test]
    fn chosen_files_read_beneath_stop_at_a_folder_swapped_for_a_link() {
        let dir = TempDir::new();
        let pick = dir.path().join("pick");
        write(&pick.join("sub/file"), b"mine!", 0o644);
        write(&dir.path().join("secret/file"), b"other", 0o600);
        let top = Dir::open(&pick).unwrap();
        let held = walk_files_beneath(top, "pick", &["sub/file".to_owned()], None).unwrap();
        assert_eq!(held.entries().len(), 3);
        std::fs::rename(pick.join("sub"), pick.join("old")).unwrap();
        testing::symlink("../secret", &pick.join("sub"));
        assert!(held.source().read(0, 5).is_err());
    }

    #[test]
    fn a_walk_beneath_refuses_a_chosen_link() {
        let dir = TempDir::new();
        write(&dir.path().join("secret"), b"other", 0o600);
        if !testing::symlink("secret", &dir.path().join("pick")) {
            return;
        }
        let failed = beneath(dir.path(), None).unwrap_err();
        assert_eq!(failed.code, ErrorCode::BadPath);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_name_for_someone_elses_file_isnt_sent() {
        use std::os::unix::fs::MetadataExt;
        let dir = TempDir::new();
        let pick = dir.path().join("pick");
        write(&pick.join("plain"), b"plain", 0o644);
        write(&dir.path().join("secret"), b"secret", 0o600);
        std::fs::hard_link(dir.path().join("secret"), pick.join("linked")).unwrap();
        let me = std::fs::metadata(&pick).unwrap().uid();
        let other = Some(me + 1);

        let held = beneath(dir.path(), other).unwrap();
        assert_eq!(paths(&held.walk), ["pick", "pick/plain"]);
        assert_eq!(
            held.walk.skipped,
            [Skipped {
                path: "pick/linked".to_owned(),
                why: Why::Foreign,
            }]
        );
        let linked = ["linked".to_owned()];
        let one = walk_beneath(Dir::open(&pick).unwrap(), "linked", &linked, other);
        assert!(one.is_err());
        let chosen = walk_files_beneath(Dir::open(&pick).unwrap(), "pick", &linked, other);
        assert!(chosen.is_err());

        // A user's own hard links still go, as a package manager makes them.
        let own = beneath(dir.path(), Some(me)).unwrap();
        assert_eq!(paths(&own.walk), ["pick", "pick/linked", "pick/plain"]);

        // A second name made after the walk stops the read.
        let plain = ["plain".to_owned()];
        let held = walk_beneath(Dir::open(&pick).unwrap(), "plain", &plain, other).unwrap();
        std::fs::hard_link(pick.join("plain"), dir.path().join("again")).unwrap();
        assert!(held.source().read(0, 5).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn opening_a_pipe_to_read_doesnt_block() {
        let dir = TempDir::new();
        let fifo = dir.path().join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.unwrap().success());
        let top = Dir::open(dir.path()).unwrap();
        let (done, back) = std::sync::mpsc::channel();
        std::thread::spawn(move || done.send(top.read_regular("pipe").is_err()));
        let refused = back
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the open blocked");
        assert!(refused);
    }

    #[test]
    fn a_walk_stops_at_folders_too_deep() {
        let dir = TempDir::new();
        let mut deep = dir.path().join("pick");
        for _ in 0..=name::MAX_DEPTH {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        let failed = beneath(dir.path(), None).unwrap_err();
        assert_eq!(failed.code, ErrorCode::TooLarge);
        assert_eq!(
            walk(&dir.path().join("pick")).unwrap_err().code,
            ErrorCode::TooLarge
        );
    }
}
