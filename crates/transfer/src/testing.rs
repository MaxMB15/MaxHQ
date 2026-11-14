// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Helpers for this crate's tests.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A folder of its own under the system temp folder, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Makes a new, empty folder.
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        loop {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.subsec_nanos())
                .unwrap_or_default();
            let name = format!(
                "maxhq-transfer-{}-{}-{nanos}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("couldn't make {}: {err}", path.display()),
            }
        }
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
