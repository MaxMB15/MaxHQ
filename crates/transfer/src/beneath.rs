// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! File operations that stay inside one folder and never follow a link.
//!
//! Every operation opens the parent folder from the root's descriptor and
//! then acts on one name in it. On Linux the parent opens with `openat2`
//! and `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, so the kernel refuses a
//! `..`, an absolute path or a link anywhere on the way. Where `openat2`
//! is missing or a seccomp filter blocks it, and on macOS, the parent opens
//! one part at a time with `O_NOFOLLOW`, after the parts were checked to
//! hold no `/`, `.` or `..`.

use std::ffi::{CStr, CString, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use rustix::fs::{AtFlags, Mode, OFlags};
pub use rustix::fs::{FileType, Stat};

/// How deep [`Dir::remove_tree`] goes, which also bounds the folders it holds
/// open.
const MAX_REMOVE_DEPTH: usize = 128;

/// An open folder that operations stay inside.
#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
}

const DIR_FLAGS: OFlags = OFlags::DIRECTORY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::RDONLY);

impl Dir {
    /// Opens a folder the caller trusts, such as a transfer's own, refusing
    /// it if the last part is a link.
    pub fn open(path: &Path) -> io::Result<Self> {
        let fd = rustix::fs::open(path, DIR_FLAGS, Mode::empty())?;
        Ok(Self { fd })
    }

    /// Makes the folder and any missing parents, then opens it.
    pub fn create(path: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(path)?;
        Self::open(path)
    }

    /// Opens the folder at `parts` below this one.
    pub fn dir(&self, parts: &[String]) -> io::Result<Dir> {
        if parts.is_empty() {
            let fd = rustix::fs::openat(&self.fd, ".", DIR_FLAGS, Mode::empty())?;
            return Ok(Dir { fd });
        }
        #[cfg(target_os = "linux")]
        match self.dir_openat2(parts) {
            Err(err) if openat2_missing(&err) => {}
            other => return other,
        }
        self.dir_by_parts(parts)
    }

    /// Opens a folder below this one with `openat2`, which refuses `..`, an
    /// absolute path and any link on the way.
    #[cfg(target_os = "linux")]
    pub fn dir_openat2(&self, parts: &[String]) -> io::Result<Dir> {
        self.openat2_raw(&parts.join("/"))
    }

    /// `openat2` on a path as given, for the tests that check the kernel
    /// refuses an escape.
    #[cfg(target_os = "linux")]
    pub(crate) fn openat2_raw(&self, path: &str) -> io::Result<Dir> {
        use rustix::fs::ResolveFlags;
        let fd = rustix::fs::openat2(
            &self.fd,
            path,
            DIR_FLAGS,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )?;
        Ok(Dir { fd })
    }

    /// Opens a folder below this one a part at a time, refusing a link at
    /// each step.
    pub fn dir_by_parts(&self, parts: &[String]) -> io::Result<Dir> {
        let mut at: Option<OwnedFd> = None;
        for part in parts {
            if part.is_empty() || part == "." || part == ".." || part.contains('/') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a path part isn't a plain name",
                ));
            }
            let parent = at.as_ref().map_or(self.fd.as_fd(), AsFd::as_fd);
            at = Some(rustix::fs::openat(
                parent,
                part.as_str(),
                DIR_FLAGS,
                Mode::empty(),
            )?);
        }
        match at {
            Some(fd) => Ok(Dir { fd }),
            None => self.dir(&[]),
        }
    }

    /// Makes a folder named `name` in this one. One that's there already is
    /// fine, as long as it's a folder and not a link.
    pub fn make_dir(&self, name: &str, mode: u32) -> io::Result<()> {
        let mode = rustix::fs::RawMode::try_from(mode)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        match rustix::fs::mkdirat(&self.fd, name, Mode::from_raw_mode(mode)) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => match self.kind(name)? {
                Some(FileType::Directory) => Ok(()),
                _ => Err(io::Error::from(rustix::io::Errno::EXIST)),
            },
            Err(err) => Err(err.into()),
        }
    }

    /// Opens the file `name` here to read and write, making it when `create`
    /// is set. A link in its place fails rather than being followed.
    pub fn open_file(&self, name: &str, create: bool) -> io::Result<File> {
        let mut flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        if create {
            flags |= OFlags::CREATE;
        }
        let fd = rustix::fs::openat(&self.fd, name, flags, Mode::from_raw_mode(0o600))?;
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    /// Opens the file `name` here to read, refusing a link. A pipe in its
    /// place opens without waiting for a writer.
    pub fn read_file(&self, name: &str) -> io::Result<File> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let fd = rustix::fs::openat(&self.fd, name, flags, Mode::empty())?;
        Ok(File::from(fd))
    }

    /// Opens the regular file `name` here to read. A link or anything else
    /// that isn't a regular file is refused, and a pipe can't block the
    /// open.
    pub fn read_regular(&self, name: &str) -> io::Result<File> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.fd, name, flags, Mode::empty())?;
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(file)
    }

    /// Sets this folder's own mode, whatever its name now leads to.
    pub fn set_own_mode(&self, mode: u32) -> io::Result<()> {
        let mode = rustix::fs::RawMode::try_from(mode)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(rustix::fs::fchmod(&self.fd, Mode::from_raw_mode(mode))?)
    }

    /// The names in this folder, without `.` and `..`, UTF-8 or not.
    pub fn raw_names(&self) -> io::Result<Vec<OsString>> {
        let names = self.entries()?.into_iter();
        Ok(names
            .map(|name| OsString::from_vec(name.into_bytes()))
            .collect())
    }

    /// Where the link `name` here points.
    pub fn read_link(&self, name: &str) -> io::Result<OsString> {
        let target = rustix::fs::readlinkat(&self.fd, name, Vec::new())?;
        Ok(OsString::from_vec(target.into_bytes()))
    }

    /// The metadata of `name` here, without following a link.
    pub fn stat(&self, name: &str) -> io::Result<Stat> {
        Ok(rustix::fs::statat(
            &self.fd,
            name,
            AtFlags::SYMLINK_NOFOLLOW,
        )?)
    }

    /// Renames `from` to `to`, both in this folder.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        Ok(rustix::fs::renameat(&self.fd, from, &self.fd, to)?)
    }

    /// Renames `from` here to `to` in another folder.
    pub fn rename_into(&self, from: &str, into: &Dir, to: &str) -> io::Result<()> {
        Ok(rustix::fs::renameat(&self.fd, from, &into.fd, to)?)
    }

    /// Makes a link `name` here pointing at `target`. One that's there
    /// already with the same target is fine.
    pub fn symlink(&self, target: &str, name: &str) -> io::Result<()> {
        match rustix::fs::symlinkat(target, &self.fd, name) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => {
                let existing = rustix::fs::readlinkat(&self.fd, name, Vec::new())?;
                if existing.as_bytes() == target.as_bytes() {
                    Ok(())
                } else {
                    Err(io::Error::from(rustix::io::Errno::EXIST))
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Removes the file or link `name` here. A missing one is fine.
    pub fn remove_file(&self, name: &str) -> io::Result<()> {
        match rustix::fs::unlinkat(&self.fd, name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// The names in this folder, leaving out any that aren't UTF-8.
    pub fn names(&self) -> io::Result<Vec<String>> {
        let names = self.entries()?.into_iter();
        Ok(names.filter_map(|name| name.into_string().ok()).collect())
    }

    fn entries(&self) -> io::Result<Vec<CString>> {
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(&self.fd)? {
            let entry = entry?;
            let name = entry.file_name();
            if !matches!(name.to_bytes(), b"." | b"..") {
                names.push(name.to_owned());
            }
        }
        Ok(names)
    }

    /// Removes `name` here and everything in it, without following a link at
    /// any depth. A missing one is fine.
    pub fn remove_tree(&self, name: &str) -> io::Result<()> {
        let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
        self.remove_at(&name, 0)
    }

    fn remove_at(&self, name: &CStr, depth: usize) -> io::Result<()> {
        let flags = match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
                if depth >= MAX_REMOVE_DEPTH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "folders go too deep to remove",
                    ));
                }
                let fd = rustix::fs::openat(&self.fd, name, DIR_FLAGS, Mode::empty())?;
                let folder = Dir { fd };
                for entry in folder.entries()? {
                    folder.remove_at(&entry, depth + 1)?;
                }
                AtFlags::REMOVEDIR
            }
            Ok(_) => AtFlags::empty(),
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        match rustix::fs::unlinkat(&self.fd, name, flags) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Gives `name` here to another user and group, without following a
    /// link. Only root may.
    pub fn chown(&self, name: &str, uid: u32, gid: u32) -> io::Result<()> {
        Ok(rustix::fs::chownat(
            &self.fd,
            name,
            Some(rustix::fs::Uid::from_raw(uid)),
            Some(rustix::fs::Gid::from_raw(gid)),
            AtFlags::SYMLINK_NOFOLLOW,
        )?)
    }

    /// What `name` here is, without following a link, or `None` when it's
    /// missing.
    pub fn kind(&self, name: &str) -> io::Result<Option<FileType>> {
        match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(FileType::from_raw_mode(stat.st_mode))),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Whether `name` and `other` here are the same file or folder, without
    /// following a link. On a disk that folds names, two spellings can be.
    /// A missing name isn't the same as anything.
    pub fn same_entry(&self, name: &str, other: &str) -> io::Result<bool> {
        let identity =
            |name: &str| match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => Ok(Some((stat.st_dev, stat.st_ino))),
                Err(rustix::io::Errno::NOENT) => Ok(None),
                Err(err) => Err(io::Error::from(err)),
            };
        Ok(match (identity(name)?, identity(other)?) {
            (Some(one), Some(two)) => one == two,
            _ => false,
        })
    }

    /// The size of the regular file `name` here, or `None` when it's
    /// missing or isn't one.
    pub fn file_size(&self, name: &str) -> io::Result<Option<u64>> {
        match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => {
                Ok(u64::try_from(stat.st_size).ok())
            }
            Ok(_) | Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Writes this folder's entries down for good.
    pub fn sync(&self) -> io::Result<()> {
        Ok(rustix::fs::fsync(&self.fd)?)
    }

    /// Bytes free for an unprivileged writer, and the disk's size, on the
    /// disk this folder is on.
    pub fn space(&self) -> io::Result<(u64, u64)> {
        let stat = rustix::fs::fstatvfs(&self.fd)?;
        let block = stat.f_frsize.max(1);
        Ok((
            stat.f_bavail.saturating_mul(block),
            stat.f_blocks.saturating_mul(block),
        ))
    }
}

/// Whether `openat2` failed because the kernel lacks it or a filter
/// blocks it, rather than because the path broke a rule.
#[cfg(target_os = "linux")]
fn openat2_missing(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(code) if code == rustix::io::Errno::NOSYS.raw_os_error()
            || code == rustix::io::Errno::PERM.raw_os_error()
    )
}

/// Sets a file's mode to 0755 when it's executable and 0644 otherwise.
pub fn set_mode(file: &File, executable: bool) -> io::Result<()> {
    let mode = if executable { 0o755 } else { 0o644 };
    Ok(rustix::fs::fchmod(file, Mode::from_raw_mode(mode))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn parts(path: &str) -> Vec<String> {
        path.split('/').map(str::to_owned).collect()
    }

    #[test]
    fn a_tree_goes_without_following_a_link_in_it() {
        let temp = TempDir::new();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("kept"), "kept").unwrap();
        let root = Dir::create(&temp.path().join("root")).unwrap();
        let tree = temp.path().join("root").join("tree");
        std::fs::create_dir_all(tree.join("a").join("b")).unwrap();
        std::fs::write(tree.join("a").join("b").join("file"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("a").join("link")).unwrap();

        let mut names = root.dir(&parts("tree/a")).unwrap().names().unwrap();
        names.sort();
        assert_eq!(names, ["b", "link"]);

        root.remove_tree("tree").unwrap();
        assert_eq!(root.kind("tree").unwrap(), None);
        assert!(outside.join("kept").exists());
        root.remove_tree("tree").unwrap();
    }

    #[test]
    fn a_tree_too_deep_stays() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        let mut folder = root.dir(&[]).unwrap();
        for _ in 0..=MAX_REMOVE_DEPTH {
            folder.make_dir("d", 0o755).unwrap();
            folder = folder.dir(&["d".to_owned()]).unwrap();
        }
        assert!(root.remove_tree("d").is_err());
        assert_eq!(root.kind("d").unwrap(), Some(FileType::Directory));
    }

    #[test]
    fn same_entry_goes_by_the_file_and_not_the_name() {
        let temp = TempDir::new();
        std::fs::write(temp.path().join("one"), b"x").unwrap();
        std::fs::write(temp.path().join("other"), b"x").unwrap();
        std::fs::hard_link(temp.path().join("one"), temp.path().join("two")).unwrap();
        std::os::unix::fs::symlink("one", temp.path().join("link")).unwrap();
        let dir = Dir::open(temp.path()).unwrap();
        assert!(dir.same_entry("one", "two").unwrap());
        assert!(!dir.same_entry("one", "other").unwrap());
        assert!(!dir.same_entry("one", "missing").unwrap());
        assert!(!dir.same_entry("missing", "missing").unwrap());
        // A link is its own entry, not what it points at.
        assert!(!dir.same_entry("one", "link").unwrap());
    }

    #[test]
    fn folders_open_part_by_part_and_never_through_a_link() {
        let temp = TempDir::new();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let root_path = temp.path().join("root");
        let root = Dir::create(&root_path).unwrap();
        root.make_dir("a", 0o755).unwrap();
        root.dir(&parts("a")).unwrap().make_dir("b", 0o755).unwrap();
        std::os::unix::fs::symlink(&outside, root_path.join("a/out")).unwrap();

        assert!(root.dir_by_parts(&parts("a/b")).is_ok());
        let err = root.dir_by_parts(&parts("a/out")).unwrap_err();
        assert!(
            matches!(err.raw_os_error(), Some(code) if code == libc_eloop() || code == libc_enotdir()),
            "{err}"
        );
        assert!(root.dir_by_parts(&parts("a/../a")).is_err());
        assert!(root.dir(&parts("a/out")).is_err());

        // A file can't be made through the link either.
        let into = root.dir(&parts("a")).unwrap();
        assert!(into.open_file("out", true).is_err());
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[test]
    fn a_link_in_place_of_a_file_is_never_followed() {
        let temp = TempDir::new();
        let target = temp.path().join("secret");
        std::fs::write(&target, b"keep").unwrap();
        let root_path = temp.path().join("root");
        let root = Dir::create(&root_path).unwrap();
        std::os::unix::fs::symlink(&target, root_path.join("x.part")).unwrap();
        assert!(root.open_file("x.part", true).is_err());
        assert!(root.read_file("x.part").is_err());
        assert!(root.make_dir("x.part", 0o755).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert_eq!(root.file_size("x.part").unwrap(), None);
        assert_eq!(root.kind("x.part").unwrap(), Some(FileType::Symlink));
    }

    #[test]
    fn links_and_modes_land_as_asked() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        root.symlink("run.sh", "latest").unwrap();
        root.symlink("run.sh", "latest").unwrap();
        assert!(root.symlink("other", "latest").is_err());
        let file = root.open_file("run.sh", true).unwrap();
        set_mode(&file, true).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(temp.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o755);
        set_mode(&file, false).unwrap();
        let mode = std::fs::metadata(temp.path().join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o644);
        let (free, size) = root.space().unwrap();
        assert!(free > 0 && size >= free);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn openat2_refuses_links_dots_and_absolute_paths() {
        let temp = TempDir::new();
        let root_path = temp.path().join("root");
        let root = Dir::create(&root_path).unwrap();
        root.make_dir("a", 0o755).unwrap();
        std::os::unix::fs::symlink(temp.path(), root_path.join("a/out")).unwrap();
        let errno = |result: io::Result<Dir>| result.unwrap_err().raw_os_error();
        assert!(root.dir_openat2(&parts("a")).is_ok());
        // A link on the way is refused by `RESOLVE_NO_SYMLINKS`. A link as
        // the last part isn't followed, and Linux refuses it as not a
        // folder before `O_NOFOLLOW` can say it's a link.
        assert_eq!(
            errno(root.dir_openat2(&parts("a/out/root"))),
            Some(libc_eloop())
        );
        assert!(matches!(
            errno(root.dir_openat2(&parts("a/out"))),
            Some(code) if code == libc_eloop() || code == libc_enotdir()
        ));
        assert_eq!(errno(root.openat2_raw("../root")), Some(libc_exdev()));
        assert_eq!(
            errno(root.openat2_raw(&temp.path().display().to_string())),
            Some(libc_exdev())
        );
    }

    fn libc_eloop() -> i32 {
        rustix::io::Errno::LOOP.raw_os_error()
    }

    fn libc_enotdir() -> i32 {
        rustix::io::Errno::NOTDIR.raw_os_error()
    }

    #[cfg(target_os = "linux")]
    fn libc_exdev() -> i32 {
        rustix::io::Errno::XDEV.raw_os_error()
    }
}
