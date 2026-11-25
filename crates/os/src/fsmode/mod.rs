// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who may read and run a file. On Unix that's the mode bits. On Windows
//! the files MaxHQ makes sit in the user's profile, which only the user,
//! the administrators and the system can read, so the modes are no-ops
//! there, and a program is a file whose extension `PATHEXT` names.
//!
//! A file that holds a secret goes through [`private_file`], which on
//! Windows gives it a DACL of its own that names only the user, so it
//! stays theirs wherever it sits.
//!
//! It also holds what tells one file from another ([`file_id`]) and
//! reads and writes at an offset ([`read_exact_at`]), which std spells
//! differently on each OS.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::Path;
use std::time::SystemTime;

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

/// Makes the folder `path` and any parents it lacks with the mode `mode`,
/// less the umask. A folder already there keeps its mode. On Windows the
/// mode is a no-op, as the module's notes say.
pub fn dir_all_with_mode(path: &Path, mode: u32) -> io::Result<()> {
    imp::dir_all_with_mode(path, mode)
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

/// The user ID that owns what `meta` describes. Windows has no user IDs,
/// so it's `None` there.
pub fn owner_of(meta: &Metadata) -> Option<u32> {
    imp::owner_of(meta)
}

/// What tells a file from every other on the machine: its device and
/// inode on Unix, and its volume and 128-bit file ID on Windows. Two names
/// or handles with one ID are one file, hard links included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    volume: u64,
    file: u128,
}

/// The ID of the open `file`.
pub fn file_id(file: &File) -> io::Result<FileId> {
    imp::file_id(file)
}

/// Whether `path`, following links, still names the file `file` has
/// open, and not one made in its place.
pub fn same_file(file: &File, path: &Path) -> io::Result<bool> {
    Ok(imp::id_at(path)? == file_id(file)?)
}

/// Whether the open `file` has a second name, a hard link that could be
/// someone else's.
pub fn has_other_names(file: &File) -> io::Result<bool> {
    imp::has_other_names(file)
}

/// Fills `buf` from `file` at `offset`, which several threads may do at
/// once. Each read names its offset, but on Windows it also moves the
/// file's cursor, so a file that's written as well as read this way is
/// opened to append, or written with [`write_all_at`].
pub fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    imp::read_exact_at(file, buf, offset)
}

/// Writes all of `buf` to `file` at `offset`. On Windows it also moves the
/// file's cursor, as [`read_exact_at`] says.
pub fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    imp::write_all_at(file, buf, offset)
}

/// Whether the file `path`, whose metadata is `meta`, is a program you
/// may run: a file with an execute bit on Unix, and a file whose
/// extension `PATHEXT` names on Windows.
pub fn is_executable(path: &Path, meta: &Metadata) -> bool {
    meta.is_file() && crate::which::host_runs(path, mode_of(meta))
}

/// Keeps the file `path` to this user alone, for a file that holds a
/// secret: mode 0600 on Unix, and on Windows a protected DACL with one
/// entry, full access for the user running this, so nothing the folder
/// grants reaches it. Call it before the secret is written.
pub fn private_file(path: &Path) -> io::Result<()> {
    imp::private_file(path)
}

/// Opens without waiting for a FIFO's writer and without taking a
/// terminal as the controlling one. Windows opens neither by path, so
/// it's a no-op there.
pub fn open_no_wait(options: &mut OpenOptions) -> &mut OpenOptions {
    imp::open_no_wait(options)
}

/// Sets when the file or folder `path` last changed, following links.
/// Windows opens a folder only by asking for one, so it can't go through
/// `File::open` there.
pub fn set_modified(path: &Path, at: SystemTime) -> io::Result<()> {
    imp::set_modified(path, at)
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
    fn a_folder_takes_the_time_it_is_given() {
        let dir = temp("time");
        fs::create_dir_all(&dir).unwrap();
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        set_modified(&dir, at).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().modified().unwrap(), at);
        fs::remove_dir_all(&dir).unwrap();
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
    fn a_folder_gets_its_mode_and_a_file_its_owner() {
        let dir = temp("mode");
        let nested = dir.join("a").join("b");
        dir_all_with_mode(&nested, 0o755).unwrap();
        dir_all_with_mode(&nested, 0o700).unwrap();
        let meta = fs::metadata(&nested).unwrap();
        if cfg!(unix) {
            assert_eq!(mode_of(&meta) & 0o777, 0o755, "it keeps its first mode");
            assert!(owner_of(&meta).is_some());
        } else {
            assert_eq!(owner_of(&meta), None);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_writes_and_reads_at_offsets() {
        let dir = temp("offsets");
        fs::create_dir_all(&dir).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(dir.join("part"))
            .unwrap();
        write_all_at(&file, b"world", 6).unwrap();
        write_all_at(&file, b"hello ", 0).unwrap();
        let mut out = [0u8; 5];
        read_exact_at(&file, &mut out, 6).unwrap();
        assert_eq!(&out, b"world");
        read_exact_at(&file, &mut out, 0).unwrap();
        assert_eq!(&out, b"hello");
        let err = read_exact_at(&file, &mut out, 9).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(file.metadata().unwrap().len(), 11);
        drop(file);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_is_known_by_its_id_under_any_name() {
        let dir = temp("same");
        fs::create_dir_all(&dir).unwrap();
        let (path, other) = (dir.join("file"), dir.join("other"));
        fs::write(&path, b"").unwrap();
        fs::write(&other, b"").unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(
            file_id(&file).unwrap(),
            file_id(&File::open(&path).unwrap()).unwrap()
        );
        assert!(same_file(&file, &path).unwrap());
        assert!(!same_file(&file, &other).unwrap());
        assert!(!has_other_names(&file).unwrap());

        let again = dir.join("again");
        fs::hard_link(&path, &again).unwrap();
        assert!(
            same_file(&file, &again).unwrap(),
            "a hard link is the same file"
        );
        assert!(has_other_names(&file).unwrap());

        let gone = same_file(&file, &dir.join("gone")).unwrap_err();
        assert_eq!(gone.kind(), io::ErrorKind::NotFound);
        drop(file);
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
    fn a_secret_file_is_kept_to_its_owner() {
        let dir = temp("secret");
        private_dir_all(&dir).unwrap();
        let path = dir.join("key");
        fs::write(&path, b"").unwrap();
        set_mode(&path, 0o644).unwrap();
        assert!(!imp::is_private(&path).unwrap(), "a plain file is shared");
        private_file(&path).unwrap();
        assert!(imp::is_private(&path).unwrap());
        fs::write(&path, b"secret").unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"secret",
            "the owner still reads it"
        );
        assert!(private_file(&dir.join("missing")).is_err());
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
