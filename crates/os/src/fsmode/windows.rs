// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! No-ops: the profile already keeps these files to the user. An
//! owner-only DACL is the follow-up for files outside it.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::path::Path;

pub fn private_dir_all(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).create(path)
}

pub fn dir_all_with_mode(path: &Path, _mode: u32) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).create(path)
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

pub fn set_open_mode(options: &mut OpenOptions, _mode: u32) -> &mut OpenOptions {
    options
}

pub fn set_mode(path: &Path, _mode: u32) -> io::Result<()> {
    // Still fails for a path that isn't there, as chmod would.
    fs::metadata(path).map(drop)
}

pub fn set_file_mode(_file: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

pub fn mode_of(meta: &Metadata) -> u32 {
    if meta.is_dir() {
        0o700
    } else if meta.permissions().readonly() {
        0o400
    } else {
        0o600
    }
}

pub fn owner_of(_meta: &Metadata) -> Option<u32> {
    None
}

pub fn same_file(_a: &Metadata, _b: &Metadata) -> bool {
    false
}

pub fn open_no_wait(options: &mut OpenOptions) -> &mut OpenOptions {
    options
}

pub fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}
