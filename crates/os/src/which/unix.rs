// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Unix's rules: `PATH` split at `:`, and any file with an execute bit.

use std::ffi::OsStr;
use std::path::PathBuf;

/// Whether a file with mode `mode` may run.
pub fn runs(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// The folders on `path`. Only a Unix host uses Unix rules, so the
/// standard library's split is Unix's.
pub fn split_paths(path: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(path).collect()
}
