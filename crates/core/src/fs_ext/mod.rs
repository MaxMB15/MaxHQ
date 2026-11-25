// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! File calls that differ by OS, kept here so their callers carry no
//! `cfg`: opening without following a link, Unix owners, making links,
//! and paths without Windows' `\\?\` prefix. On Unix each is the call it
//! stands for. Owners, modes and a file's names are read through
//! [`maxhq_os::fsmode`].
//!
//! Windows has no Unix owners, so the calls that hand a file to another
//! user say they can't. `maxd` only runs as root for another user on Unix.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub use maxhq_os::path::plain;

/// Opens the regular file `path` to read. A pipe can't hold up the open,
/// and anything that isn't a regular file is refused. Unless `follow`, a
/// link at `path` is refused too: Unix's `O_NOFOLLOW`, and on Windows any
/// reparse point, junctions as well as links.
pub fn open_regular(path: &Path, follow: bool) -> io::Result<File> {
    let file = imp::open_regular(path, follow)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// Makes `options` refuse a link at the path it opens: `O_NOFOLLOW` on
/// Unix, and on Windows the reparse point itself, which a file created
/// new can't be. It sets the OS flags outright, so it replaces any set
/// before.
pub fn no_follow(options: &mut OpenOptions) -> &mut OpenOptions {
    imp::no_follow(options)
}

/// Gives the open `file` to the user `uid` and group `gid`.
pub fn chown_file(file: &File, uid: u32, gid: u32) -> io::Result<()> {
    imp::chown_file(file, uid, gid)
}

/// Gives what's at `path` to the user `uid` and group `gid`, the link
/// itself when it's a link.
pub fn chown_link(path: &Path, uid: u32, gid: u32) -> io::Result<()> {
    imp::chown_link(path, uid, gid)
}

/// Makes a link at `link` to `target`, which may be relative to the
/// link's folder and spelled with `/`. Windows makes a folder's link when
/// the target is a folder now, and needs Developer Mode or an
/// administrator to make one at all; without either, the error says so.
pub fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    imp::symlink(target.as_ref(), link.as_ref())
}

/// Removes the file or link at `path`. Windows keeps a link to a folder
/// as a folder, which a file's removal can't take.
pub fn remove_link(path: &Path) -> io::Result<()> {
    imp::remove_link(path)
}

/// Renames `from` to `to`, in place of the link or file already at `to`.
/// Unix swaps them at once. Windows can't rename over a folder's link, so
/// it removes that first, and `to` is briefly missing.
pub fn rename_link(from: &Path, to: &Path) -> io::Result<()> {
    imp::rename_link(from, to)
}

/// Whether `err` says only that nothing is there: no such name, or a part
/// of the path that isn't a folder.
pub fn absent(err: &io::Error) -> bool {
    imp::absent(err)
}

/// Whether `err`, from opening a folder without following links, says
/// the name stopped being a folder: it's gone, a file, or a link.
pub fn not_a_folder_now(err: &io::Error) -> bool {
    imp::not_a_folder_now(err)
}

/// [`std::fs::canonicalize`] without the `\\?\` that Windows puts in
/// front of every path it gives back, when the path means the same
/// without it.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::canonicalize(path).map(plain)
}

/// [`std::fs::read_link`] without Windows' `\\?\` on an absolute target,
/// as [`canonicalize`] does.
pub fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::fs::read_link(path).map(plain)
}

#[cfg(test)]
mod tests;
