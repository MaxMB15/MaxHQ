// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Windows: `MoveFileExW` without its replace flag, the Mark of the Web
//! as a `Zone.Identifier` stream, and links made as a file's or a folder's
//! link to match the one copied.

// MoveFileExW, which std doesn't wrap without its replace flag, needs
// unsafe. The block says why it holds.
#![allow(unsafe_code)]

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{FileTypeExt, OpenOptionsExt};
use std::path::Path;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
};
use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, FILE_FLAG_OPEN_REPARSE_POINT};

/// How many times a move tries while something has a file in it open,
/// waiting twice as long each time from [`FIRST_WAIT`]: about 1.5 seconds
/// in all.
const TRIES: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_millis(100);

/// The stream a browser writes on a download, which SmartScreen, Office
/// and Explorer read.
const ZONE_STREAM: &str = ":Zone.Identifier";

/// Zone 3 is the internet, which is what a browser says of a download.
const ZONE: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// Whether a move failed because another program has the file or a file
/// in the folder open, which a scanner does for a moment after a write.
fn held_open(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error().and_then(|code| u32::try_from(code).ok()),
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_ACCESS_DENIED)
    )
}

pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    let (source, target) = (wide(from), wide(to));
    let mut wait = FIRST_WAIT;
    let mut tries = 1;
    loop {
        // SAFETY: both are NUL-terminated wide strings that outlive the
        // call. No flags: it moves on the same disk and never replaces.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if tries == TRIES || !held_open(&err) || to.symlink_metadata().is_ok() {
            return Err(err);
        }
        std::thread::sleep(wait);
        wait *= 2;
        tries += 1;
    }
}

pub fn copy_link(from: &Path, to: &Path) -> io::Result<bool> {
    let target = std::fs::read_link(from)?;
    let made = if std::fs::symlink_metadata(from)?
        .file_type()
        .is_symlink_dir()
    {
        std::os::windows::fs::symlink_dir(&target, to)
    } else {
        std::os::windows::fs::symlink_file(&target, to)
    };
    match made {
        Ok(()) => Ok(true),
        Err(err) if crate::beneath::cannot_link(&err) => Ok(false),
        Err(err) => Err(err),
    }
}

/// The Mark of the Web.
pub struct Mark;

impl Mark {
    pub fn new() -> Option<Self> {
        Some(Self)
    }

    /// Writes the zone stream on a file. Windows reads it on files only,
    /// so a folder gets none. The stream opens on the name itself, so one
    /// that became a link since gets it on the link, not what it leads to.
    pub fn put(&self, path: &Path, folder: bool) -> io::Result<()> {
        if folder {
            return Ok(());
        }
        let mut stream = OsString::from(path.as_os_str());
        stream.push(ZONE_STREAM);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(stream)?;
        file.write_all(ZONE)
    }
}
