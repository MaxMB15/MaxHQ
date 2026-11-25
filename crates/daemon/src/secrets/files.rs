// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The store's folder. It's 0700, and each value sits in a 0600 file that
//! is written whole and renamed into place, so a crash leaves the old
//! value or the new one and never half of either.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use maxhq_core::fs::{write_atomic, Atomic};
use maxhq_os::fsmode::{mode_of, private_dir_all, set_mode};
use maxhq_protocol::Secret;

/// The 0700 folder that holds the secrets.
pub struct Folder {
    path: PathBuf,
}

impl Folder {
    /// Makes the folder if it's missing, and narrows it to 0700 if it's
    /// wider.
    pub fn open(path: PathBuf) -> io::Result<Self> {
        private_dir_all(&path)?;
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            return Err(io::Error::other(format!(
                "{} isn't a folder",
                path.display()
            )));
        }
        if mode_of(&meta) & 0o777 != 0o700 {
            set_mode(&path, 0o700)?;
        }
        Ok(Self { path })
    }

    /// The folder itself.
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Replaces `name` with `contents`. They go to a new 0600 file, which
    /// is synced, renamed over `name`, and then the folder is synced too.
    pub fn write(&self, name: &str, contents: &[u8]) -> io::Result<()> {
        write_atomic(&self.path.join(name), contents, Atomic::PRIVATE)
    }

    /// What `name` holds, or none when it's missing. A file anyone else
    /// could read is narrowed to 0600 first.
    pub fn read(&self, name: &str) -> io::Result<Option<Secret>> {
        let path = self.path.join(name);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        if !meta.is_file() {
            return Err(io::Error::other(format!("{name} isn't a plain file")));
        }
        if mode_of(&meta) & 0o077 != 0 {
            set_mode(&path, 0o600)?;
        }
        fs::read_to_string(&path).map(|text| Some(Secret::new(text)))
    }

    /// Deletes `name`. One that's already gone counts as deleted.
    pub fn remove(&self, name: &str) -> io::Result<()> {
        match fs::remove_file(self.path.join(name)) {
            Ok(()) => sync(&self.path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// The names of the files in the folder.
    pub fn names(&self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            if let Ok(name) = entry?.file_name().into_string() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// Deletes what writes cut short by a crash left behind.
    pub fn clear_leftovers(&self) {
        let Ok(names) = self.names() else { return };
        for name in names {
            if name.starts_with('.') && name.ends_with(".tmp") {
                let _ = fs::remove_file(self.path.join(name));
            }
        }
    }
}

/// Syncs a folder, so a rename or delete in it survives a crash.
fn sync(folder: &Path) -> io::Result<()> {
    maxhq_os::fsmode::sync_dir(folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn mode(path: &Path) -> u32 {
        mode_of(&fs::metadata(path).unwrap()) & 0o777
    }

    #[test]
    fn makes_a_private_folder_and_private_files() {
        let temp = TempDir::new();
        let folder = Folder::open(temp.path().join("secrets")).unwrap();
        assert_eq!(mode(folder.path()), 0o700);

        folder.write("a.token", b"test-not-a-real-token").unwrap();
        assert_eq!(mode(&folder.path().join("a.token")), 0o600);
        let read = folder.read("a.token").unwrap().unwrap();
        assert_eq!(read.expose(), "test-not-a-real-token");

        folder.write("a.token", b"second").unwrap();
        assert_eq!(folder.read("a.token").unwrap().unwrap().expose(), "second");
        assert_eq!(folder.names().unwrap(), ["a.token"]);

        folder.remove("a.token").unwrap();
        folder.remove("a.token").unwrap();
        assert!(folder.read("a.token").unwrap().is_none());
        assert!(folder.names().unwrap().is_empty());
    }

    #[test]
    fn narrows_a_folder_and_files_that_are_too_wide() {
        let temp = TempDir::new();
        let path = temp.path().join("secrets");
        fs::create_dir(&path).unwrap();
        set_mode(&path, 0o755).unwrap();
        fs::write(path.join("b.token"), "test-not-a-real-token").unwrap();
        set_mode(&path.join("b.token"), 0o644).unwrap();

        let folder = Folder::open(path.clone()).unwrap();
        assert_eq!(mode(&path), 0o700);
        folder.read("b.token").unwrap().unwrap();
        assert_eq!(mode(&path.join("b.token")), 0o600);
    }

    #[test]
    fn clears_what_a_crash_left() {
        let temp = TempDir::new();
        let folder = Folder::open(temp.path().join("secrets")).unwrap();
        folder.write("a.token", b"test-not-a-real-token").unwrap();
        fs::write(folder.path().join(".a.token.1-0.tmp"), "half").unwrap();
        folder.clear_leftovers();
        assert_eq!(folder.names().unwrap(), ["a.token"]);
    }

    #[test]
    fn refuses_a_file_where_the_folder_goes() {
        let temp = TempDir::new();
        let path = temp.path().join("secrets");
        fs::write(&path, "").unwrap();
        assert!(Folder::open(path).is_err());
    }
}
