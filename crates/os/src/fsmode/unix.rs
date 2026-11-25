// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::SystemTime;

use super::FileId;

pub fn private_dir_all(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)
}

pub fn dir_all_with_mode(path: &Path, mode: u32) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(mode).create(path)
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

pub fn set_open_mode(options: &mut OpenOptions, mode: u32) -> &mut OpenOptions {
    options.mode(mode)
}

pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, Permissions::from_mode(mode))
}

pub fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(mode))
}

pub fn mode_of(meta: &Metadata) -> u32 {
    meta.permissions().mode()
}

pub fn private_file(path: &Path) -> io::Result<()> {
    set_mode(path, 0o600)
}

/// Whether `path` is this user's alone, for the tests.
#[cfg(test)]
pub fn is_private(path: &Path) -> io::Result<bool> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o777 == 0o600)
}

pub fn owner_of(meta: &Metadata) -> Option<u32> {
    Some(meta.uid())
}

fn id_of(meta: &Metadata) -> FileId {
    FileId {
        volume: meta.dev(),
        file: u128::from(meta.ino()),
    }
}

pub fn file_id(file: &File) -> io::Result<FileId> {
    file.metadata().map(|meta| id_of(&meta))
}

pub fn id_at(path: &Path) -> io::Result<FileId> {
    fs::metadata(path).map(|meta| id_of(&meta))
}

pub fn has_other_names(file: &File) -> io::Result<bool> {
    Ok(file.metadata()?.nlink() > 1)
}

pub fn open_no_wait(options: &mut OpenOptions) -> &mut OpenOptions {
    options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
}

pub fn set_modified(path: &Path, at: SystemTime) -> io::Result<()> {
    File::open(path)?.set_modified(at)
}

pub fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
