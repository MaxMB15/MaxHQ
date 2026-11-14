// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `stderr.log`: what the CLI writes to stderr, kept to a cap.
//!
//! The holder keeps the newest bytes in memory up to the cap. When the file
//! grows past the cap, it's rewritten with the newest half, so it stays
//! between half the cap and the cap.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use maxhq_core::fs::{write_atomic, Atomic};

use crate::files;

/// The CLI's stderr, on disk and in memory.
pub(crate) struct StderrLog {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    cap: usize,
    ring: VecDeque<u8>,
}

impl StderrLog {
    /// Starts an empty log at `path` that keeps at most `cap` bytes.
    pub fn create(path: &Path, cap: u64) -> io::Result<Self> {
        let file = files::append(path)?;
        file.set_len(0)?;
        Ok(Self {
            path: path.to_owned(),
            file: Some(file),
            size: 0,
            cap: usize::try_from(cap).unwrap_or(usize::MAX).max(2),
            ring: VecDeque::new(),
        })
    }

    /// Adds `bytes`. Failing to write the file only costs the file, so the
    /// error goes to the holder's log and the bytes stay in memory.
    pub fn append(&mut self, bytes: &[u8]) {
        self.ring.extend(bytes);
        if self.ring.len() > self.cap {
            let excess = self.ring.len() - self.cap;
            self.ring.drain(..excess);
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if let Err(err) = file.write_all(bytes) {
            eprintln!("couldn't write {}: {err}", self.path.display());
            self.file = None;
            return;
        }
        self.size += bytes.len() as u64;
        if self.size > self.cap as u64
            && let Err(err) = self.trim()
        {
            eprintln!("couldn't trim {}: {err}", self.path.display());
        }
    }

    fn trim(&mut self) -> io::Result<()> {
        let keep = self.cap / 2;
        let start = self.ring.len().saturating_sub(keep);
        let newest: Vec<u8> = self.ring.range(start..).copied().collect();
        let mode = Atomic {
            mode: 0o600,
            ..Atomic::PLAIN
        };
        write_atomic(&self.path, &newest, mode)?;
        self.file = Some(files::append(&self.path)?);
        self.size = newest.len() as u64;
        Ok(())
    }

    /// The newest `limit` bytes, as text.
    pub fn tail(&self, limit: usize) -> String {
        let start = self.ring.len().saturating_sub(limit);
        let bytes: Vec<u8> = self.ring.range(start..).copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn the_file_stays_under_its_cap_and_keeps_the_newest_bytes() {
        let id = maxhq_protocol::ids::SessionId::generate()
            .0
            .simple()
            .to_string();
        let dir = std::env::temp_dir().join(format!("hold-err-{}", &id[20..]));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stderr.log");
        let mut log = StderrLog::create(&path, 100).unwrap();
        for n in 0..50 {
            log.append(format!("{n:03}\n").as_bytes());
        }
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(on_disk.len() <= 100, "{} bytes", on_disk.len());
        assert!(on_disk.ends_with("049\n"));
        assert_eq!(log.tail(8), "048\n049\n");
        assert!(log.tail(1000).len() <= 100);
        fs::remove_dir_all(&dir).unwrap();
    }
}
