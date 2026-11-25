// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The items a timeline keeps on disk instead of in memory. They go in
//! files of their own under `maxd`'s data folder, each unlinked as soon
//! as it's made, so it's gone once the timeline drops it or `maxd` ends,
//! however it ends. Windows lets an open file be deleted, as std opens it
//! for that, and keeps its bytes until the last handle closes.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::Arc;

use maxhq_protocol::timeline::TimelineItem;
use uuid::Uuid;

/// How long a file grows before the next one starts. Files are dropped
/// whole, oldest first, so this is how many bytes of items go at once.
const FILE_SIZE: u64 = 16 << 20;

/// Where an item is on disk.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    /// The number of its file.
    file: u64,
    at: u64,
    len: u32,
}

impl Place {
    /// The number of the item's file.
    pub fn file(&self) -> u64 {
        self.file
    }
}

/// An item on disk, which any thread can read back.
#[derive(Clone)]
pub struct Spilled {
    file: Arc<File>,
    at: u64,
    len: u32,
}

impl Spilled {
    /// Reads the item back. Several threads may at once, and the file is
    /// open to append, so a read that moves its cursor, as on Windows,
    /// doesn't move where the next item is written.
    pub fn load(&self) -> io::Result<TimelineItem> {
        let mut bytes = vec![0; self.len as usize];
        maxhq_os::fsmode::read_exact_at(&self.file, &mut bytes, self.at)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// One timeline's files of items.
pub struct Disk {
    /// The folder they're made in.
    dir: PathBuf,
    /// Each file with its length, oldest first.
    files: VecDeque<(Arc<File>, u64)>,
    /// The number of the oldest file.
    first: u64,
    /// The bytes in every file.
    size: u64,
}

impl Disk {
    /// No files yet. The first write makes `dir` if it isn't there.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            files: VecDeque::new(),
            first: 0,
            size: 0,
        }
    }

    /// The bytes in every file.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Writes `items` out, oldest first, and gives where each went.
    pub fn write<'a>(
        &mut self,
        items: impl IntoIterator<Item = &'a TimelineItem>,
    ) -> io::Result<Vec<Place>> {
        let mut places = Vec::new();
        let mut bytes = Vec::new();
        for item in items {
            let full = self
                .files
                .back()
                .is_none_or(|(_, len)| len + bytes.len() as u64 >= FILE_SIZE);
            if full {
                self.append(&bytes)?;
                bytes.clear();
                self.start()?;
            }
            let start = bytes.len();
            serde_json::to_writer(&mut bytes, item)?;
            let len = u32::try_from(bytes.len() - start).map_err(io::Error::other)?;
            let (_, end) = self
                .files
                .back()
                .ok_or_else(|| io::Error::other("no file"))?;
            places.push(Place {
                file: self.first + self.files.len() as u64 - 1,
                at: end + start as u64,
                len,
            });
        }
        self.append(&bytes)?;
        Ok(places)
    }

    /// The item at `place`, to read back.
    pub fn spilled(&self, place: Place) -> io::Result<Spilled> {
        let (file, _) = place
            .file
            .checked_sub(self.first)
            .and_then(|at| self.files.get(usize::try_from(at).ok()?))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the file is gone"))?;
        Ok(Spilled {
            file: file.clone(),
            at: place.at,
            len: place.len,
        })
    }

    /// Reads back the item at `place`.
    pub fn read(&self, place: Place) -> io::Result<TimelineItem> {
        self.spilled(place)?.load()
    }

    /// Drops the oldest file, and gives its number.
    pub fn drop_oldest(&mut self) -> Option<u64> {
        let (_, len) = self.files.pop_front()?;
        self.size -= len;
        self.first += 1;
        Some(self.first - 1)
    }

    /// Drops every file. A reader still holding one keeps it until it's
    /// done.
    pub fn clear(&mut self) {
        self.first += self.files.len() as u64;
        self.files.clear();
        self.size = 0;
    }

    /// Adds `bytes` to the end of the newest file.
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let (file, len) = self
            .files
            .back_mut()
            .ok_or_else(|| io::Error::other("no file"))?;
        let mut writer: &File = file;
        writer.write_all(bytes)?;
        *len += bytes.len() as u64;
        self.size += bytes.len() as u64;
        Ok(())
    }

    /// Starts a new file. Only this timeline needs it, so it has no name.
    fn start(&mut self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(Uuid::now_v7().simple().to_string());
        let mut options = OpenOptions::new();
        options.read(true).append(true).create_new(true);
        let file = maxhq_os::fsmode::set_open_mode(&mut options, 0o600).open(&path)?;
        fs::remove_file(&path)?;
        self.files.push_back((Arc::new(file), 0));
        Ok(())
    }
}
