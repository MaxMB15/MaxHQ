// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The receiving end of a transfer: partial copies, hash checks, renames
//! and the credit it grants.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::Instant;

use maxhq_protocol::message::{ErrorCode, ManifestEntry};
use maxhq_wire::Window;
use sha2::{Digest, Sha256};

use crate::beneath::{self, Dir};
use crate::manifest::{self, Entry, Kind, Layout};
use crate::{Failure, Room, SYNC_BYTES, SYNC_EVERY};

/// Credit worth sending: the window end moved at least this far, or
/// `committed` moved at all.
const CREDIT_STEP: u64 = 64 * 1024;

/// The credit a receiver grants for one transfer, and the round trips it
/// measures to size the window.
///
/// A round trip runs from a grant going out to the first byte past the
/// previous grant arriving. That byte can't leave the sender before the
/// grant reaches it, so the time covers the trip there, the trip back and
/// whatever queue the bytes waited in, which is what the window has to
/// keep short.
#[derive(Clone, Debug)]
pub struct Grant {
    window: Window,
    end: u64,
    probe: Option<(u64, Instant)>,
}

impl Grant {
    /// A grant of nothing yet, with the window at its smallest.
    pub fn new() -> Self {
        Self {
            window: Window::new(),
            end: 0,
            probe: None,
        }
    }

    /// The window end to grant now that `written` bytes are down, never
    /// past `total` and never behind an earlier grant.
    pub fn extend(&mut self, written: u64, total: u64, now: Instant) -> u64 {
        let end = written.saturating_add(self.window.size()).min(total);
        if end > self.end {
            if self.probe.is_none() {
                self.probe = Some((self.end, now));
            }
            self.end = end;
        }
        self.end
    }

    /// Notes that bytes up to `end` arrived.
    pub fn arrived(&mut self, end: u64, now: Instant) {
        if let Some((mark, sent)) = self.probe
            && end > mark
        {
            self.window.observe(now.saturating_duration_since(sent));
            self.probe = None;
        }
    }

    /// The furthest offset granted so far.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// The window as it stands.
    pub fn window(&self) -> &Window {
        &self.window
    }
}

impl Default for Grant {
    fn default() -> Self {
        Self::new()
    }
}

/// Lands one transfer in a folder of its own.
///
/// Folders and links are made when it opens, since they hold no bytes.
/// Each file lands as a partial copy and is renamed once its hash checks
/// out. Opening the same folder again with the same manifest picks up where
/// an earlier receiver stopped.
#[derive(Debug)]
pub struct Receiver {
    root: Dir,
    entries: Vec<Entry>,
    parts: Vec<Option<String>>,
    layout: Layout,
    written: u64,
    committed: u64,
    next: usize,
    open: Option<Open>,
    unsynced: u64,
    synced_at: Instant,
    restarting: bool,
    touched: BTreeSet<Vec<String>>,
    grant: Grant,
    sent: Option<(u64, u64)>,
}

/// The file landing now.
struct Open {
    index: usize,
    file: File,
    hasher: Sha256,
}

impl std::fmt::Debug for Open {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Open").field("index", &self.index).finish()
    }
}

impl Receiver {
    /// Checks the manifest, makes `root` and everything that holds no bytes
    /// in it, finds what an earlier attempt left, and checks the disk has
    /// room for the rest.
    pub fn open(root: &Path, manifest: &[ManifestEntry], room: Room) -> Result<Self, Failure> {
        let entries = manifest::check(manifest)?;
        let dir = Dir::create(root).map_err(|err| Failure::io(&err, None))?;
        let layout = Layout::new(entries.iter().map(|entry| entry.size));
        let parts = manifest::part_names(&entries);
        let mut receiver = Self {
            root: dir,
            entries,
            parts,
            layout,
            written: 0,
            committed: 0,
            next: 0,
            open: None,
            unsynced: 0,
            synced_at: Instant::now(),
            restarting: false,
            touched: BTreeSet::new(),
            grant: Grant::new(),
            sent: None,
        };
        receiver.make_skeleton()?;
        receiver.recover()?;
        room.check(&receiver.root, receiver.layout.total() - receiver.written)?;
        receiver.settle()?;
        if receiver.is_done() {
            receiver.sync_now()?;
        }
        Ok(receiver)
    }

    /// Makes every folder and link.
    fn make_skeleton(&mut self) -> Result<(), Failure> {
        for index in 0..self.entries.len() {
            let entry = &self.entries[index];
            let at = u32::try_from(index).ok();
            let fail = |err: io::Error| Failure::io(&err, at);
            let (leaf, parent) = entry.parts.split_last().expect("checked paths have a part");
            match &entry.kind {
                Kind::Dir => self
                    .root
                    .dir(parent)
                    .and_then(|dir| dir.make_dir(leaf, 0o755)),
                Kind::Symlink { target } => self
                    .root
                    .dir(parent)
                    .and_then(|dir| dir.symlink(target, leaf)),
                Kind::File { .. } => continue,
            }
            .map_err(fail)?;
            self.touched.insert(parent.to_vec());
        }
        Ok(())
    }

    /// Finds the first file that hasn't landed, and picks up its partial
    /// copy if there is one.
    fn recover(&mut self) -> Result<(), Failure> {
        for index in 0..self.entries.len() {
            let entry = &self.entries[index];
            if !matches!(entry.kind, Kind::File { .. }) {
                continue;
            }
            let at = u32::try_from(index).ok();
            let fail = |err: io::Error| Failure::io(&err, at);
            let (leaf, parent) = entry.parts.split_last().expect("checked paths have a part");
            let dir = self.root.dir(parent).map_err(fail)?;
            if dir.file_size(leaf).map_err(fail)? == Some(entry.size) {
                continue;
            }
            self.next = index;
            self.written = self.layout.start(index);
            self.committed = self.written;
            let part = self.parts[index]
                .as_deref()
                .expect("files have a partial name");
            match dir.file_size(part).map_err(fail)? {
                Some(len) if len <= entry.size && len > 0 => {
                    let mut file = dir.open_file(part, false).map_err(fail)?;
                    let mut hasher = Sha256::new();
                    hash_prefix(&mut file, len, &mut hasher).map_err(fail)?;
                    file.sync_data().map_err(fail)?;
                    self.written += len;
                    self.committed = self.written;
                    self.open = Some(Open {
                        index,
                        file,
                        hasher,
                    });
                    if len == entry.size {
                        // It all landed, and the rename didn't.
                        match self.complete(index) {
                            Ok(()) => continue,
                            Err(failure) if failure.code == ErrorCode::HashMismatch => {
                                self.restarting = false;
                            }
                            Err(failure) => return Err(failure),
                        }
                    }
                }
                Some(_) => dir.remove_file(part).map_err(fail)?,
                None => {}
            }
            return Ok(());
        }
        self.next = self.entries.len();
        self.written = self.layout.total();
        self.committed = self.written;
        Ok(())
    }

    /// Bytes in the whole transfer.
    pub fn total(&self) -> u64 {
        self.layout.total()
    }

    /// Bytes written so far, in order from the start.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Bytes on disk and fsynced.
    pub fn committed(&self) -> u64 {
        self.committed
    }

    /// Whether every entry has landed.
    pub fn is_done(&self) -> bool {
        self.next >= self.entries.len() && self.committed == self.layout.total()
    }

    /// The checked entries.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entries at the top, which name what was sent.
    pub fn tops(&self) -> Vec<String> {
        manifest::tops(&self.entries)
            .into_iter()
            .map(Entry::path)
            .collect()
    }

    /// Takes one chunk at `offset`. A chunk that repeats bytes already
    /// written is fine. One that skips ahead fails, unless it was on its
    /// way before the sender heard about a hash mismatch.
    pub fn write(&mut self, offset: u64, data: &[u8], now: Instant) -> Result<(), Failure> {
        let end = offset.saturating_add(data.len() as u64);
        self.grant.arrived(end, now);
        if end > self.layout.total() {
            return Err(Failure::new(
                ErrorCode::BadMessage,
                None,
                "a chunk runs past the end of the transfer",
            ));
        }
        if end > self.grant.end() {
            return Err(Failure::new(
                ErrorCode::BadMessage,
                None,
                "a chunk runs past the credit granted",
            ));
        }
        if end <= self.written {
            return Ok(());
        }
        if offset > self.written {
            if self.restarting {
                return Ok(());
            }
            return Err(Failure::new(
                ErrorCode::BadMessage,
                self.entry_at(self.written),
                "a chunk skipped ahead",
            ));
        }
        self.restarting = false;
        let skip = usize::try_from(self.written - offset).unwrap_or(data.len());
        let mut data = &data[skip..];
        while !data.is_empty() {
            self.settle()?;
            let index = self.next;
            let Some(size) = self.entries.get(index).map(|entry| entry.size) else {
                break;
            };
            let at = u32::try_from(index).ok();
            let within = self.written - self.layout.start(index);
            let take = usize::try_from(size - within)
                .unwrap_or(usize::MAX)
                .min(data.len());
            let open = self.open_part(index)?;
            open.file
                .write_all_at(&data[..take], within)
                .map_err(|err| Failure::io(&err, at))?;
            open.hasher.update(&data[..take]);
            self.written += take as u64;
            self.unsynced += take as u64;
            data = &data[take..];
            if self.written == self.layout.end(index) {
                self.complete(index)?;
            }
        }
        self.settle()?;
        if self.is_landed() {
            self.sync_now()?;
        } else if self.unsynced >= SYNC_BYTES || now.duration_since(self.synced_at) >= SYNC_EVERY {
            self.sync(now)?;
        }
        Ok(())
    }

    /// Whether every byte is written, fsynced or not.
    fn is_landed(&self) -> bool {
        self.next >= self.entries.len() && self.written == self.layout.total()
    }

    /// Completes the entries at the current offset that hold no bytes.
    fn settle(&mut self) -> Result<(), Failure> {
        while let Some(entry) = self.entries.get(self.next) {
            match entry.kind {
                Kind::File { .. } if entry.size == 0 => self.complete(self.next)?,
                Kind::File { .. } => return Ok(()),
                _ => self.next += 1,
            }
        }
        Ok(())
    }

    /// The partial copy of `index`, opened or made.
    fn open_part(&mut self, index: usize) -> Result<&mut Open, Failure> {
        if self.open.as_ref().is_some_and(|open| open.index != index) {
            self.open = None;
        }
        if self.open.is_none() {
            let at = u32::try_from(index).ok();
            let fail = |err: io::Error| Failure::io(&err, at);
            let entry = &self.entries[index];
            let parent = &entry.parts[..entry.parts.len() - 1];
            let part = self.parts[index]
                .as_deref()
                .expect("files have a partial name");
            let dir = self.root.dir(parent).map_err(fail)?;
            let file = dir.open_file(part, true).map_err(fail)?;
            file.set_len(0).map_err(fail)?;
            self.touched.insert(parent.to_vec());
            self.open = Some(Open {
                index,
                file,
                hasher: Sha256::new(),
            });
        }
        Ok(self.open.as_mut().expect("just opened"))
    }

    /// Checks a whole file's hash, then sets its mode, fsyncs it and renames
    /// it into place. A mismatch removes the copy and goes back to the
    /// file's first byte.
    fn complete(&mut self, index: usize) -> Result<(), Failure> {
        let at = u32::try_from(index).ok();
        let fail = |err: io::Error| Failure::io(&err, at);
        self.open_part(index)?;
        let open = self.open.take().expect("just opened");
        let entry = &self.entries[index];
        let Kind::File { sha256 } = &entry.kind else {
            unreachable!("only files complete");
        };
        let (leaf, parent) = entry.parts.split_last().expect("checked paths have a part");
        let part = self.parts[index]
            .as_deref()
            .expect("files have a partial name");
        let dir = self.root.dir(parent).map_err(fail)?;
        let digest: [u8; 32] = open.hasher.finalize().into();
        if &digest != sha256 {
            drop(open.file);
            dir.remove_file(part).map_err(fail)?;
            let path = entry.path();
            self.written = self.layout.start(index);
            self.committed = self.committed.min(self.written);
            self.unsynced = 0;
            self.restarting = true;
            return Err(Failure::new(
                ErrorCode::HashMismatch,
                at,
                format!("{path} didn't match its hash"),
            ));
        }
        beneath::set_mode(&open.file, entry.executable).map_err(fail)?;
        open.file.sync_all().map_err(fail)?;
        drop(open.file);
        dir.rename(part, leaf).map_err(fail)?;
        self.touched.insert(parent.to_vec());
        self.next = index + 1;
        self.committed = self.written;
        self.unsynced = 0;
        Ok(())
    }

    /// Fsyncs what's written when it's due, so `committed` moves on.
    pub fn sync_if_due(&mut self, now: Instant) -> Result<(), Failure> {
        if self.unsynced > 0 && now.duration_since(self.synced_at) >= SYNC_EVERY {
            self.sync(now)?;
        }
        Ok(())
    }

    /// Fsyncs everything written, so `committed` catches up with it.
    pub fn sync(&mut self, now: Instant) -> Result<(), Failure> {
        let at = self
            .open
            .as_ref()
            .and_then(|open| u32::try_from(open.index).ok());
        if let Some(open) = &self.open {
            open.file.sync_data().map_err(|err| Failure::io(&err, at))?;
        }
        self.sync_dirs()?;
        self.committed = self.written;
        self.unsynced = 0;
        self.synced_at = now;
        Ok(())
    }

    fn sync_now(&mut self) -> Result<(), Failure> {
        self.sync(Instant::now())
    }

    fn sync_dirs(&mut self) -> Result<(), Failure> {
        for parts in std::mem::take(&mut self.touched) {
            self.root
                .dir(&parts)
                .and_then(|dir| dir.sync())
                .map_err(|err| Failure::io(&err, None))?;
        }
        Ok(())
    }

    /// The credit to send now, or `None` when the last one sent still
    /// stands.
    pub fn credit(&mut self, now: Instant) -> Option<(u64, u64)> {
        let total = self.layout.total();
        let end = self.grant.extend(self.written, total, now);
        let next = (self.committed, end);
        if let Some((committed, sent_end)) = self.sent {
            let moved = end.saturating_sub(sent_end) >= CREDIT_STEP
                || self.committed.abs_diff(committed) >= CREDIT_STEP;
            let finished = (end == total && sent_end < total)
                || (self.committed == total && committed < total);
            if !moved && !finished {
                return None;
            }
        }
        self.sent = Some(next);
        Some(next)
    }

    /// The credit as it stands, fsynced first, for an answer to a sender
    /// that came back after a dropped link. It picks up from `committed`.
    pub fn resume(&mut self, now: Instant) -> Result<(u64, u64), Failure> {
        self.sync(now)?;
        let end = self.grant.extend(self.written, self.layout.total(), now);
        self.sent = Some((self.committed, end));
        Ok((self.committed, end))
    }

    /// Extends the grant as a credit would, and gives its end. A sender on
    /// the same disk uses it in place of credit messages.
    pub fn extend_grant(&mut self, now: Instant) -> u64 {
        self.grant.extend(self.written, self.layout.total(), now)
    }

    /// The entry holding the byte at `offset`.
    fn entry_at(&self, offset: u64) -> Option<u32> {
        self.layout
            .locate(offset)
            .and_then(|(index, _)| u32::try_from(index).ok())
    }

    /// The landing folder.
    pub fn root(&self) -> &Dir {
        &self.root
    }
}

/// Feeds the first `len` bytes of a file to a hasher.
fn hash_prefix(file: &mut File, len: u64, hasher: &mut Sha256) -> io::Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut reader = Read::by_ref(file).take(len);
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}

/// The SHA-256 of some bytes, as a manifest writes it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    manifest::to_hex(&digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use maxhq_protocol::message::EntryKind;
    use std::time::Duration;

    /// No room kept, so the tests pass on a full disk.
    const ROOM: Room = Room {
        keep_bytes: 0,
        keep_percent: 0,
        keep_at_most: 0,
    };

    fn dir(path: &str) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::Dir,
            executable: false,
            size: 0,
            sha256: None,
        }
    }

    fn file(path: &str, bytes: &[u8], executable: bool) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::File,
            executable,
            size: bytes.len() as u64,
            sha256: Some(sha256_hex(bytes)),
        }
    }

    fn link(path: &str, target: &str) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::Symlink {
                target: target.into(),
            },
            executable: false,
            size: 0,
            sha256: None,
        }
    }

    /// Sends `stream` in chunks of `size`, granting credit as it goes.
    fn pour(receiver: &mut Receiver, stream: &[u8], from: u64, size: usize) {
        let now = Instant::now();
        receiver.credit(now);
        let mut offset = from;
        while offset < stream.len() as u64 {
            let end = receiver.grant.end();
            let take = (end - offset).min(size as u64) as usize;
            let start = offset as usize;
            receiver
                .write(offset, &stream[start..start + take], now)
                .unwrap();
            offset += take as u64;
            receiver.credit(now);
        }
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn a_folder_lands_with_its_links_and_modes() {
        let temp = TempDir::new();
        let script = b"#!/bin/sh\necho hi\n".to_vec();
        let notes = vec![7u8; 700_000];
        let manifest = [
            dir("notes"),
            file("notes/run.sh", &script, true),
            link("notes/latest", "run.sh"),
            file("notes/empty", b"", false),
            dir("notes/deep"),
            file("notes/deep/big.bin", &notes, false),
        ];
        let root = temp.path().join("in");
        let mut receiver = Receiver::open(&root, &manifest, ROOM).unwrap();
        assert_eq!(receiver.total(), (script.len() + notes.len()) as u64);
        let stream = [script.clone(), notes.clone()].concat();
        pour(&mut receiver, &stream, 0, 100_000);
        assert!(receiver.is_done());
        assert_eq!(receiver.committed(), receiver.total());
        assert_eq!(std::fs::read(root.join("notes/run.sh")).unwrap(), script);
        assert_eq!(
            std::fs::read(root.join("notes/deep/big.bin")).unwrap(),
            notes
        );
        assert_eq!(std::fs::read(root.join("notes/empty")).unwrap(), b"");
        assert_eq!(
            std::fs::read_link(root.join("notes/latest")).unwrap(),
            Path::new("run.sh")
        );
        assert_eq!(mode(&root.join("notes/run.sh")), 0o755);
        assert_eq!(mode(&root.join("notes/deep/big.bin")), 0o644);
        assert!(!root.join("notes/deep/big.bin.part").exists());
        assert_eq!(receiver.tops(), ["notes"]);
    }

    #[test]
    fn a_hash_mismatch_names_the_entry_and_the_sender_starts_it_again() {
        let temp = TempDir::new();
        let good = vec![1u8; 300_000];
        let manifest = [
            dir("d"),
            file("d/a", b"first", false),
            file("d/b", &good, false),
        ];
        let root = temp.path().join("in");
        let mut receiver = Receiver::open(&root, &manifest, ROOM).unwrap();
        let now = Instant::now();
        receiver.credit(now);
        receiver.write(0, b"first", now).unwrap();
        let mut bad = good.clone();
        bad[299_999] = 2;
        receiver.write(5, &bad[..200_000], now).unwrap();
        receiver.credit(now);
        let failure = receiver.write(200_005, &bad[200_000..], now).unwrap_err();
        assert_eq!(failure.code, ErrorCode::HashMismatch);
        assert_eq!(failure.entry, Some(2));
        assert!(!root.join("d/b").exists());
        assert!(!root.join("d/b.part").exists());
        assert_eq!(receiver.written(), 5);
        // A chunk already on its way is dropped, then the sender starts over.
        receiver.write(200_005, &bad[200_000..], now).unwrap();
        receiver.write(5, &good[..250_000], now).unwrap();
        receiver.credit(now);
        receiver.write(250_005, &good[250_000..], now).unwrap();
        assert!(receiver.is_done());
        assert_eq!(std::fs::read(root.join("d/b")).unwrap(), good);
        assert_eq!(std::fs::read(root.join("d/a")).unwrap(), b"first");
    }

    #[test]
    fn a_new_receiver_picks_up_where_the_last_one_stopped() {
        let temp = TempDir::new();
        let a = vec![3u8; 100_000];
        let b = vec![4u8; 400_000];
        let manifest = [dir("d"), file("d/a", &a, false), file("d/b", &b, true)];
        let root = temp.path().join("in");
        let stream = [a.clone(), b.clone()].concat();
        {
            let mut receiver = Receiver::open(&root, &manifest, ROOM).unwrap();
            let now = Instant::now();
            receiver.credit(now);
            receiver.write(0, &stream[..300_000], now).unwrap();
            // Dropped without a sync, as a crash would.
        }
        assert!(root.join("d/a").exists());
        assert!(root.join("d/b.part").exists());
        let mut receiver = Receiver::open(&root, &manifest, ROOM).unwrap();
        let (committed, end) = receiver.resume(Instant::now()).unwrap();
        assert_eq!(committed, 300_000);
        assert!(end > committed);
        pour(&mut receiver, &stream, committed, 64 * 1024);
        assert!(receiver.is_done());
        assert_eq!(std::fs::read(root.join("d/b")).unwrap(), b);
        assert_eq!(mode(&root.join("d/b")), 0o755);
    }

    #[test]
    fn a_copy_that_landed_whole_but_was_not_renamed_finishes_on_reopen() {
        let temp = TempDir::new();
        let a = b"all of it".to_vec();
        let manifest = [file("a.txt", &a, false)];
        let root = temp.path().join("in");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt.part"), &a).unwrap();
        let receiver = Receiver::open(&root, &manifest, ROOM).unwrap();
        assert!(receiver.is_done());
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), a);
    }

    #[test]
    fn the_same_bytes_again_after_a_resume_change_nothing() {
        let temp = TempDir::new();
        let a = vec![9u8; 50_000];
        let manifest = [file("a", &a, false)];
        let mut receiver = Receiver::open(&temp.path().join("in"), &manifest, ROOM).unwrap();
        let now = Instant::now();
        receiver.credit(now);
        receiver.write(0, &a[..30_000], now).unwrap();
        receiver.write(10_000, &a[10_000..30_000], now).unwrap();
        receiver.write(20_000, &a[20_000..], now).unwrap();
        assert!(receiver.is_done());
    }

    #[test]
    fn chunks_that_skip_ahead_or_overrun_are_refused() {
        let temp = TempDir::new();
        let a = vec![9u8; 50_000];
        let manifest = [file("a", &a, false)];
        let mut receiver = Receiver::open(&temp.path().join("in"), &manifest, ROOM).unwrap();
        let now = Instant::now();
        // Nothing granted yet.
        assert_eq!(
            receiver.write(0, &a[..10], now).unwrap_err().code,
            ErrorCode::BadMessage
        );
        receiver.credit(now);
        assert_eq!(
            receiver.write(10, &a[10..20], now).unwrap_err().code,
            ErrorCode::BadMessage
        );
        assert_eq!(
            receiver.write(49_990, &[0u8; 20], now).unwrap_err().code,
            ErrorCode::BadMessage
        );
    }

    #[test]
    fn a_planted_link_is_never_written_through() {
        let temp = TempDir::new();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        let root = temp.path().join("in");
        std::fs::create_dir_all(root.join("d")).unwrap();
        // Something put a link where the partial copy goes.
        std::os::unix::fs::symlink(&outside, root.join("d/a.part")).unwrap();
        let a = vec![1u8; 10];
        let manifest = [dir("d"), file("d/a", &a, false)];
        let failure = Receiver::open(&root, &manifest, ROOM)
            .and_then(|mut receiver| {
                let now = Instant::now();
                receiver.credit(now);
                receiver.write(0, &a, now)
            })
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::BadPath, "{failure:?}");
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }

    #[test]
    fn a_planted_link_in_place_of_a_folder_is_refused() {
        let temp = TempDir::new();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let root = temp.path().join("in");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("d")).unwrap();
        let manifest = [dir("d"), file("d/a", b"x", false)];
        let failure = Receiver::open(&root, &manifest, ROOM).unwrap_err();
        assert_eq!(failure.code, ErrorCode::BadPath, "{failure:?}");
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[test]
    fn a_disk_without_room_refuses_before_writing() {
        let temp = TempDir::new();
        let manifest = [file("a", b"tiny", false)];
        let greedy = Room {
            keep_bytes: u64::MAX / 2,
            keep_percent: 0,
            keep_at_most: 0,
        };
        let failure = Receiver::open(&temp.path().join("in"), &manifest, greedy).unwrap_err();
        assert_eq!(failure.code, ErrorCode::NoSpace);
    }

    #[test]
    fn credit_grows_with_quick_round_trips_and_waits_for_a_step() {
        let temp = TempDir::new();
        let a = vec![5u8; 20 * 1024 * 1024];
        let manifest = [file("a", &a, false)];
        let mut receiver = Receiver::open(&temp.path().join("in"), &manifest, ROOM).unwrap();
        let start = Instant::now();
        assert_eq!(receiver.credit(start), Some((0, Window::MIN)));
        assert_eq!(receiver.credit(start), None);
        let mut offset = 0u64;
        let mut now = start;
        for _ in 0..6 {
            now += Duration::from_millis(10);
            let end = receiver.grant.end();
            let take = (end - offset) as usize;
            receiver
                .write(offset, &a[offset as usize..offset as usize + take], now)
                .unwrap();
            offset += take as u64;
            receiver.credit(now);
        }
        assert_eq!(receiver.grant.window().size(), Window::MAX);
    }
}
