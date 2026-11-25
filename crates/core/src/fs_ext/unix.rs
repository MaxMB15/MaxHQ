// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub fn open_regular(path: &Path, follow: bool) -> io::Result<File> {
    let mut flags = libc::O_NONBLOCK;
    if !follow {
        flags |= libc::O_NOFOLLOW;
    }
    OpenOptions::new().read(true).custom_flags(flags).open(path)
}

pub fn no_follow(options: &mut OpenOptions) -> &mut OpenOptions {
    options.custom_flags(libc::O_NOFOLLOW)
}

pub fn uid_of(meta: &Metadata) -> Option<u32> {
    Some(meta.uid())
}

pub fn chown_file(file: &File, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::fchown(file, Some(uid), Some(gid))
}

pub fn chown_link(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    std::os::unix::fs::lchown(path, Some(uid), Some(gid))
}

pub fn has_other_names(file: &File) -> io::Result<bool> {
    Ok(file.metadata()?.nlink() > 1)
}

pub fn dir_all_with_mode(path: &Path, mode: u32) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(mode).create(path)
}

pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

pub fn remove_link(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}

pub fn rename_link(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

pub fn absent(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
}

pub fn not_a_folder_now(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ELOOP | libc::ENOTDIR | libc::ENOENT)
    )
}

pub fn plain(path: PathBuf) -> PathBuf {
    path
}
