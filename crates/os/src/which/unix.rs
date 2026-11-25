// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Unix's rules: `PATH` split at `:`, and any file with an execute bit.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Whether a file with mode `mode` may run.
pub fn runs(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// The folders on `path`, split at `:`. A Unix host takes the standard
/// library's split, which keeps names that aren't UTF-8; another host, where
/// only tests use Unix rules, splits the text.
pub fn split_paths(path: &OsStr) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        std::env::split_paths(path).collect()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy()
            .split(':')
            .map(PathBuf::from)
            .collect()
    }
}

/// Whether `dir` may be searched: Unix searches only absolute folders.
/// `has_root` is Unix's `is_absolute`, and it also holds for `/usr/bin` on a
/// Windows host, which has no drive to make it absolute there.
pub fn is_absolute(dir: &Path) -> bool {
    dir.has_root()
}
