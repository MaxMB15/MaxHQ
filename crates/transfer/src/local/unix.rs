// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Unix: `renameat2`'s no-replace flag, the quarantine mark on macOS, and
//! links as `symlink` makes them.

use std::io;
use std::path::Path;

use rustix::fs::{renameat_with, RenameFlags, CWD};
use rustix::io::Errno;

pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // Some disks can't refuse to replace, so look first there.
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {
            if to.symlink_metadata().is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            std::fs::rename(from, to)
        }
        Err(err) => Err(err.into()),
    }
}

pub fn copy_link(from: &Path, to: &Path) -> io::Result<bool> {
    let target = std::fs::read_link(from)?;
    std::os::unix::fs::symlink(target, to)?;
    Ok(true)
}

/// The mark macOS checks before it opens a file from elsewhere.
#[cfg(target_os = "macos")]
const QUARANTINE: &str = "com.apple.quarantine";

/// The quarantine mark, as a browser sets it: flags, when, and what
/// fetched it.
#[cfg(target_os = "macos")]
pub struct Mark(String);

#[cfg(target_os = "macos")]
impl Mark {
    pub fn new() -> Option<Self> {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default();
        Some(Self(format!("0081;{seconds:08x};MaxHQ;")))
    }

    /// Sets the mark on a file or folder, not following a link.
    pub fn put(&self, path: &Path, _folder: bool) -> io::Result<()> {
        let flags = rustix::fs::XattrFlags::empty();
        Ok(rustix::fs::lsetxattr(
            path,
            QUARANTINE,
            self.0.as_bytes(),
            flags,
        )?)
    }
}

/// Linux has no mark for a file from elsewhere.
#[cfg(not(target_os = "macos"))]
pub enum Mark {}

#[cfg(not(target_os = "macos"))]
impl Mark {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn put(&self, _path: &Path, _folder: bool) -> io::Result<()> {
        match *self {}
    }
}
