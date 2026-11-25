// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! File calls that differ by OS, kept here so their callers carry no
//! `cfg`: opening without following a link, Unix owners, making links,
//! and paths without Windows' `\\?\` prefix. On Unix each is the call it
//! stands for.
//!
//! Windows has no Unix owners, so the calls that hand a file to another
//! user say they can't. `maxd` only runs as root for another user on Unix.

use std::fs::{File, Metadata, OpenOptions};
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

/// The Unix user that owns what `meta` describes. Windows has none.
pub fn uid_of(meta: &Metadata) -> Option<u32> {
    imp::uid_of(meta)
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

/// Whether the open `file` has a second name, a hard link that could be
/// someone else's. Windows can't tell through std, so it says it can't.
pub fn has_other_names(file: &File) -> io::Result<bool> {
    imp::has_other_names(file)
}

/// Makes the folder `path` and the parents it lacks with mode `mode`,
/// less the umask. Windows has no modes, so it only makes them.
pub fn dir_all_with_mode(path: &Path, mode: u32) -> io::Result<()> {
    imp::dir_all_with_mode(path, mode)
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

/// `path` without Windows' `\\?\` prefix, when it means the same without
/// it: a drive or share path short enough for the old limit, with no
/// part that the plain form would read differently. Any other path,
/// and every path on Unix, comes back as it is.
pub fn plain(path: PathBuf) -> PathBuf {
    imp::plain(path)
}

/// Windows' path text `text` without its `\\?\`, or none when it has no
/// such prefix or means something else without it. The plain form reads
/// `.`, `..`, a trailing dot or space, a stream's `:` and device names
/// such as `NUL` its own way, and stops at 260 characters.
#[cfg_attr(not(windows), allow(dead_code))]
fn unverbatim(text: &str) -> Option<String> {
    let (plain, names) = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        let mut parts = rest.splitn(3, '\\');
        let (server, share) = (parts.next()?, parts.next()?);
        if server.is_empty() || share.is_empty() {
            return None;
        }
        (format!(r"\\{rest}"), parts.next().unwrap_or(""))
    } else {
        let rest = text.strip_prefix(r"\\?\")?;
        let names = match rest.as_bytes() {
            [letter, b':', b'\\', ..] if letter.is_ascii_alphabetic() => &rest[3..],
            _ => return None,
        };
        (rest.to_owned(), names)
    };
    if plain.encode_utf16().count() >= 260 || plain.contains('/') {
        return None;
    }
    let names: Vec<&str> = names.split('\\').collect();
    let last = names.len() - 1;
    for (n, name) in names.iter().enumerate() {
        let odd = if name.is_empty() {
            n != last
        } else {
            *name == "."
                || *name == ".."
                || name.ends_with(['.', ' '])
                || name.contains(':')
                || is_device(name)
        };
        if odd {
            return None;
        }
    }
    Some(plain)
}

/// Whether Windows reads the name `name` as a device in a plain path,
/// with or without an extension: `CON`, `NUL`, `COM1` and the like.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            let mut chars = upper.chars();
            let head: String = chars.by_ref().take(3).collect();
            let digit = chars.next();
            (head == "COM" || head == "LPT")
                && chars.next().is_none()
                && digit.is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
        }
    }
}

#[cfg(test)]
mod tests;
