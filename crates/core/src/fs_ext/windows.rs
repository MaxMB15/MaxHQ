// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A link is a reparse point, opened as itself with
//! `FILE_FLAG_OPEN_REPARSE_POINT`. Windows has no Unix owners.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::windows::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Opens a reparse point itself rather than what it points at.
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Marks a reparse point: a link, a junction, or a filter's own kind.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// ERROR_PRIVILEGE_NOT_HELD, which making a link without Developer Mode
/// or an administrator gives.
const PRIVILEGE_NOT_HELD: i32 = 1314;

pub fn open_regular(path: &Path, follow: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    if !follow {
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !follow && file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "it's a link, which isn't followed here",
        ));
    }
    Ok(file)
}

pub fn no_follow(options: &mut OpenOptions) -> &mut OpenOptions {
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
}

pub fn uid_of(_meta: &Metadata) -> Option<u32> {
    None
}

fn no_owners() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows files have no Unix owner to give them to",
    )
}

pub fn chown_file(_file: &File, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(no_owners())
}

pub fn chown_link(_path: &Path, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(no_owners())
}

pub fn has_other_names(_file: &File) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "a file's names can't be counted on Windows",
    ))
}

pub fn dir_all_with_mode(path: &Path, _mode: u32) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    // A link's target reads `\` alone between its parts.
    let target: PathBuf = target.components().collect();
    let points_at = match link.parent() {
        Some(folder) => folder.join(&target),
        None => target.clone(),
    };
    let made = if fs::metadata(&points_at).is_ok_and(|meta| meta.is_dir()) {
        std::os::windows::fs::symlink_dir(&target, link)
    } else {
        std::os::windows::fs::symlink_file(&target, link)
    };
    made.map_err(|err| {
        if err.raw_os_error() == Some(PRIVILEGE_NOT_HELD) {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Windows makes links only with Developer Mode on (Settings, System, \
                 For developers) or for an administrator",
            )
        } else {
            err
        }
    })
}

/// Whether what's at `path` is a link to a folder, which Windows removes
/// as a folder.
fn folder_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink_dir())
}

pub fn remove_link(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if folder_link(path) => fs::remove_dir(path).map_err(|_| err),
        done => done,
    }
}

pub fn rename_link(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(_) if fs::symlink_metadata(to).is_ok_and(|meta| meta.file_type().is_symlink()) => {
            remove_link(to)?;
            fs::rename(from, to)
        }
        done => done,
    }
}

pub fn absent(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

pub fn not_a_folder_now(err: &io::Error) -> bool {
    absent(err)
}

pub fn plain(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(super::unverbatim) {
        Some(text) => PathBuf::from(text),
        None => path,
    }
}
