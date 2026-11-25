// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Files written whole: the new bytes go to a file beside the target that
//! is renamed over it, so a reader sees the old file or the new one and
//! never half of either.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use maxhq_os::fsmode;

/// Opens a regular file to read, following links. Anything else is refused:
/// a repo can link a name to `/dev/zero`, a terminal or a pipe, where a read
/// never ends or never starts. The stat keeps a device from being opened at
/// all. Opening without waiting, and checking what came back, covers a file
/// swapped in after the stat.
pub fn open_regular(path: &Path) -> io::Result<File> {
    let refused = || io::Error::new(io::ErrorKind::InvalidInput, "it isn't a regular file");
    if !fs::metadata(path)?.is_file() {
        return Err(refused());
    }
    let file = fsmode::open_no_wait(OpenOptions::new().read(true)).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(refused());
    }
    Ok(file)
}

/// Reads a regular file up to `limit` bytes and one more, so the caller can
/// tell it went over. See [`open_regular`].
pub fn read_regular(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_regular(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The text of a regular file, refused if it's over `limit` bytes. See
/// [`open_regular`].
pub fn read_text(path: &Path, limit: u64) -> io::Result<String> {
    let bytes = read_regular(path, limit)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "it's too large"));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "it isn't text"))
}

/// Tells apart the temporary files of writes running at once.
static WRITES: AtomicU64 = AtomicU64::new(0);

/// How [`write_atomic`] makes the new file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Atomic {
    /// The new file's permission bits.
    pub mode: u32,
    /// Whether the file gets exactly `mode`. Otherwise the umask narrows
    /// it, as it does for `fs::write`.
    pub exact: bool,
    /// Whether the file and then its folder are synced, so the new file
    /// survives a power cut once the write returns.
    pub sync: bool,
}

impl Atomic {
    /// A synced file that only its owner may read: exactly 0600.
    pub const PRIVATE: Self = Self::exact(0o600);

    /// What `fs::write` makes: 0666 less the umask, not synced.
    pub const PLAIN: Self = Self {
        mode: 0o666,
        exact: false,
        sync: false,
    };

    /// A synced file with exactly `mode`.
    pub const fn exact(mode: u32) -> Self {
        Self {
            mode,
            exact: true,
            sync: true,
        }
    }
}

/// Replaces `path` with `bytes` through a file beside it that's renamed
/// into place. The temporary file is named `.<name>.<pid>-<n>.tmp`, so
/// writes running at once never share one, and it's removed when the
/// write fails.
pub fn write_atomic(path: &Path, bytes: &[u8], how: Atomic) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"))?;
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_file_name(format!(
        ".{}.{}-{write}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let written = (|| {
        let mut file =
            fsmode::set_open_mode(OpenOptions::new().write(true).create_new(true), how.mode)
                .open(&temp)?;
        if how.exact {
            fsmode::set_file_mode(&file, how.mode)?;
        }
        file.write_all(bytes)?;
        if how.sync {
            file.sync_all()?;
        }
        drop(file);
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
        return written;
    }
    if how.sync {
        let folder = match path.parent() {
            Some(folder) if !folder.as_os_str().is_empty() => folder,
            _ => Path::new("."),
        };
        fsmode::sync_dir(folder)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TempDir;

    fn mode(path: &Path) -> u32 {
        fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // Windows keeps no mode bits to check.
    #[cfg(unix)]
    #[test]
    fn an_exact_mode_holds_whatever_the_umask() {
        let dir = TempDir::new("atomic");
        let path = dir.0.join("shared");
        // 0662 has bits a usual umask of 022 would take away.
        write_atomic(&path, b"one", Atomic::exact(0o662)).unwrap();
        assert_eq!(mode(&path), 0o662);
        write_atomic(&path, b"two", Atomic::PRIVATE).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn a_plain_write_takes_the_mode_fs_write_would() {
        let dir = TempDir::new("atomic");
        let plain = dir.0.join("plain");
        let reference = dir.0.join("reference");
        write_atomic(&plain, b"text", Atomic::PLAIN).unwrap();
        fs::write(&reference, b"text").unwrap();
        assert_eq!(mode(&plain), mode(&reference));
    }

    #[test]
    fn a_write_replaces_the_file_there() {
        let dir = TempDir::new("atomic");
        let path = dir.0.join("config");
        fs::write(&path, b"old and longer").unwrap();
        fsmode::set_mode(&path, 0o644).unwrap();
        write_atomic(&path, b"new", Atomic::PRIVATE).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(names(&dir.0), ["config"], "no temp file is left behind");
    }

    #[test]
    fn a_failed_write_leaves_no_temp_file() {
        let dir = TempDir::new("atomic");
        // A folder with a file in it can't be renamed over.
        let path = dir.0.join("taken");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside"), b"").unwrap();
        assert!(write_atomic(&path, b"text", Atomic::PRIVATE).is_err());
        assert_eq!(names(&dir.0), ["taken"]);
        assert_eq!(names(&path), ["inside"]);
    }

    #[test]
    fn a_path_with_no_file_name_is_refused() {
        let err = write_atomic(Path::new("/"), b"", Atomic::PLAIN).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
