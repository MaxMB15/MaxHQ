// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A folder for one test, removed when the test ends.

use std::fs;
use std::path::PathBuf;

/// A new empty folder under the system's temp folder.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    /// A folder named `maxhq-<tag>-<uuid>`.
    pub(crate) fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("maxhq-{tag}-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
