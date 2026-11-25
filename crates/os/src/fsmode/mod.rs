// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who may read and run a file. On Unix that's the mode bits. On Windows
//! the files MaxHQ makes sit in the user's profile, which only the user,
//! the administrators and the system can read, so the modes are no-ops
//! there, and a program is a file whose extension `PATHEXT` names.
//!
//! An owner-only DACL for Windows files outside the profile is a follow-up.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Makes the folder `path` and any parents it lacks, for this user alone
/// (0700). A folder already there keeps its mode.
pub fn private_dir_all(path: &Path) -> io::Result<()> {
    imp::private_dir_all(path)
}

/// Makes the folder `path` for this user alone (0700). Fails when it's
/// already there, or its parent isn't.
pub fn private_dir(path: &Path) -> io::Result<()> {
    imp::private_dir(path)
}

/// Gives a file that `options` creates the mode `mode`, less the umask.
pub fn set_open_mode(options: &mut OpenOptions, mode: u32) -> &mut OpenOptions {
    imp::set_open_mode(options, mode)
}

/// Sets the mode of whatever `path` names, following links.
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    imp::set_mode(path, mode)
}

/// Sets the mode of the open `file`.
pub fn set_file_mode(file: &File, mode: u32) -> io::Result<()> {
    imp::set_file_mode(file, mode)
}

/// The mode bits `meta` holds, with the file type bits on Unix. On
/// Windows it's what the profile gives: 0700 for a folder, 0600 for a
/// file and 0400 for a read-only one.
pub fn mode_of(meta: &Metadata) -> u32 {
    imp::mode_of(meta)
}

/// Whether the file `path`, whose metadata is `meta`, is a program you
/// may run: a file with an execute bit on Unix, and a file whose
/// extension `PATHEXT` names on Windows.
pub fn is_executable(path: &Path, meta: &Metadata) -> bool {
    meta.is_file() && crate::which::host_runs(path, mode_of(meta))
}

/// Opens without waiting for a FIFO's writer and without taking a
/// terminal as the controlling one. Windows opens neither by path, so
/// it's a no-op there.
pub fn open_no_wait(options: &mut OpenOptions) -> &mut OpenOptions {
    imp::open_no_wait(options)
}

/// Syncs the folder `path`, so a file just renamed into it survives a
/// power cut. Windows can't sync a folder through a handle you may open,
/// and NTFS journals the rename, so it's a no-op there.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    imp::sync_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A folder of the test's own under the system's temp folder, empty.
    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("maxhq-os-fsmode-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn private_folders_are_made() {
        let dir = temp("dirs");
        let nested = dir.join("a").join("b");
        private_dir_all(&nested).unwrap();
        private_dir_all(&nested).unwrap();
        assert!(nested.is_dir());
        let single = dir.join("single");
        private_dir(&single).unwrap();
        assert!(private_dir(&single).is_err(), "it's already there");
        let meta = fs::metadata(&single).unwrap();
        assert_eq!(mode_of(&meta) & 0o777, 0o700);
        sync_dir(&dir).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_private_file_is_this_users_alone() {
        let dir = temp("file");
        private_dir_all(&dir).unwrap();
        let path = dir.join("secret");
        let file = set_open_mode(OpenOptions::new().write(true).create_new(true), 0o600)
            .open(&path)
            .unwrap();
        set_file_mode(&file, 0o600).unwrap();
        set_mode(&path, 0o600).unwrap();
        assert_eq!(mode_of(&fs::metadata(&path).unwrap()) & 0o777, 0o600);
        drop(file);
        let read = open_no_wait(OpenOptions::new().read(true)).open(&path);
        assert!(read.is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_programs_are_executable() {
        let dir = temp("exec");
        private_dir_all(&dir).unwrap();
        let program = dir.join(if cfg!(windows) { "tool.exe" } else { "tool" });
        let text = dir.join("notes.txt");
        fs::write(&program, b"").unwrap();
        fs::write(&text, b"").unwrap();
        set_mode(&program, 0o755).unwrap();
        set_mode(&text, 0o644).unwrap();
        let runs = |path: &Path| is_executable(path, &fs::metadata(path).unwrap());
        assert!(runs(&program));
        assert!(!runs(&text));
        assert!(!runs(&dir), "a folder is no program");
        fs::remove_dir_all(&dir).unwrap();
    }
}
