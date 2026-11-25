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
//!
//! On Windows the root is a folder handle and each name opens relative to
//! it with `NtCreateFile`, a part at a time. A name that could reach a
//! drive, a share, a stream or a parent is refused before anything opens,
//! and every open asks for the reparse point itself, so a symlink or a
//! junction is refused where Unix uses `O_NOFOLLOW`.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata};
use std::io;
use std::path::Path;

use maxhq_protocol::message::ErrorCode;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub use imp::Stat;

/// How deep [`Dir::remove_tree`] goes, which also bounds the folders it holds
/// open.
const MAX_REMOVE_DEPTH: usize = 128;

/// What an entry is, without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A regular file.
    RegularFile,
    /// A folder.
    Directory,
    /// A symbolic link, or a junction on Windows.
    Symlink,
    /// A named pipe.
    Fifo,
    /// A character device.
    CharacterDevice,
    /// A block device.
    BlockDevice,
    /// A Unix socket.
    Socket,
    /// Something else.
    Unknown,
}

impl FileType {
    /// Whether it's a regular file.
    pub fn is_file(self) -> bool {
        self == Self::RegularFile
    }

    /// Whether it's a folder.
    pub fn is_dir(self) -> bool {
        self == Self::Directory
    }

    /// Whether it's a link.
    pub fn is_symlink(self) -> bool {
        self == Self::Symlink
    }
}

/// An open folder that operations stay inside.
#[derive(Debug)]
pub struct Dir {
    handle: imp::Handle,
}

impl Dir {
    /// Opens a folder the caller trusts, such as a transfer's own, refusing
    /// it if the last part is a link.
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            handle: imp::open_dir(path)?,
        })
    }

    /// Makes the folder and any missing parents, then opens it.
    pub fn create(path: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(path)?;
        Self::open(path)
    }

    /// Opens the folder at `parts` below this one.
    pub fn dir(&self, parts: &[String]) -> io::Result<Dir> {
        if parts.is_empty() {
            return Ok(Dir {
                handle: imp::reopen(&self.handle)?,
            });
        }
        if let Some(opened) = imp::dir_beneath(&self.handle, parts) {
            return opened.map(|handle| Dir { handle });
        }
        self.dir_by_parts(parts)
    }

    /// Opens a folder below this one a part at a time, refusing a link at
    /// each step.
    pub fn dir_by_parts(&self, parts: &[String]) -> io::Result<Dir> {
        let mut at: Option<imp::Handle> = None;
        for part in parts {
            if part.is_empty() || part == "." || part == ".." || part.contains('/') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a path part isn't a plain name",
                ));
            }
            let parent = at.as_ref().unwrap_or(&self.handle);
            at = Some(imp::open_dir_at(parent, OsStr::new(part))?);
        }
        match at {
            Some(handle) => Ok(Dir { handle }),
            None => self.dir(&[]),
        }
    }

    /// Makes a folder named `name` in this one. One that's there already is
    /// fine, as long as it's a folder and not a link.
    pub fn make_dir(&self, name: &str, mode: u32) -> io::Result<()> {
        match imp::make_dir(&self.handle, OsStr::new(name), mode) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => match self.kind(name)? {
                Some(FileType::Directory) => Ok(()),
                _ => Err(err),
            },
            other => other,
        }
    }

    /// Opens the file `name` here to read and write, making it when `create`
    /// is set. A link in its place fails rather than being followed.
    pub fn open_file(&self, name: &str, create: bool) -> io::Result<File> {
        regular(imp::open_file(&self.handle, OsStr::new(name), create)?)
    }

    /// Opens the file `name` here to read, refusing a link. A pipe in its
    /// place opens without waiting for a writer.
    pub fn read_file(&self, name: &str) -> io::Result<File> {
        imp::read_file(&self.handle, OsStr::new(name))
    }

    /// Opens the regular file `name` here to read. A link or anything else
    /// that isn't a regular file is refused, and a pipe can't block the
    /// open.
    pub fn read_regular(&self, name: &str) -> io::Result<File> {
        regular(self.read_file(name)?)
    }

    /// Sets this folder's own mode, whatever its name now leads to. Windows
    /// has no modes, so there it does nothing.
    pub fn set_own_mode(&self, mode: u32) -> io::Result<()> {
        imp::set_own_mode(&self.handle, mode)
    }

    /// The names in this folder, without `.` and `..`, UTF-8 or not.
    pub fn raw_names(&self) -> io::Result<Vec<OsString>> {
        let entries = imp::entries(&self.handle)?.into_iter();
        Ok(entries.map(|(name, _)| name).collect())
    }

    /// Where the link `name` here points.
    pub fn read_link(&self, name: &str) -> io::Result<OsString> {
        imp::read_link(&self.handle, OsStr::new(name))
    }

    /// The metadata of `name` here, without following a link.
    pub fn stat(&self, name: &str) -> io::Result<Stat> {
        imp::stat(&self.handle, OsStr::new(name))
    }

    /// Renames `from` to `to`, both in this folder, replacing what's at
    /// `to`.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let (from, to) = (OsStr::new(from), OsStr::new(to));
        imp::rename(&self.handle, from, &self.handle, to, true)
    }

    /// Renames `from` here to `to` in another folder, replacing what's
    /// there.
    pub fn rename_into(&self, from: &str, into: &Dir, to: &str) -> io::Result<()> {
        let (from, to) = (OsStr::new(from), OsStr::new(to));
        imp::rename(&self.handle, from, &into.handle, to, true)
    }

    /// Makes a link `name` here pointing at `target`. One that's there
    /// already with the same target is fine.
    pub fn symlink(&self, target: &str, name: &str) -> io::Result<()> {
        match imp::symlink(target, &self.handle, OsStr::new(name)) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                if self.read_link(name)? == *target {
                    Ok(())
                } else {
                    Err(err)
                }
            }
            other => other,
        }
    }

    /// Removes the file or link `name` here. A missing one is fine.
    pub fn remove_file(&self, name: &str) -> io::Result<()> {
        gone(imp::unlink(&self.handle, OsStr::new(name), false))
    }

    /// The names in this folder, leaving out any that aren't UTF-8.
    pub fn names(&self) -> io::Result<Vec<String>> {
        let names = self.raw_names()?.into_iter();
        Ok(names.filter_map(|name| name.into_string().ok()).collect())
    }

    /// Removes `name` here and everything in it, without following a link at
    /// any depth. A missing one is fine.
    pub fn remove_tree(&self, name: &str) -> io::Result<()> {
        remove_at(&self.handle, OsStr::new(name), 0)
    }

    /// Gives `name` here to another user and group, without following a
    /// link. Only root may, and Windows can't.
    pub fn chown(&self, name: &str, uid: u32, gid: u32) -> io::Result<()> {
        imp::chown(&self.handle, OsStr::new(name), uid, gid)
    }

    /// What `name` here is, without following a link, or `None` when it's
    /// missing.
    pub fn kind(&self, name: &str) -> io::Result<Option<FileType>> {
        Ok(self.found(name)?.map(|stat| stat.kind()))
    }

    /// Whether `name` and `other` here are the same file or folder, without
    /// following a link. On a disk that folds names, two spellings can be.
    /// A missing name isn't the same as anything.
    pub fn same_entry(&self, name: &str, other: &str) -> io::Result<bool> {
        Ok(match (self.found(name)?, self.found(other)?) {
            (Some(one), Some(two)) => one.same(&two),
            _ => false,
        })
    }

    /// The size of the regular file `name` here, or `None` when it's
    /// missing or isn't one.
    pub fn file_size(&self, name: &str) -> io::Result<Option<u64>> {
        Ok(self
            .found(name)?
            .filter(|stat| stat.kind() == FileType::RegularFile)
            .and_then(|stat| stat.size()))
    }

    /// Writes this folder's entries down for good.
    pub fn sync(&self) -> io::Result<()> {
        imp::sync(&self.handle)
    }

    /// Bytes free for an unprivileged writer, and the disk's size, on the
    /// disk this folder is on.
    pub fn space(&self) -> io::Result<(u64, u64)> {
        imp::space(&self.handle)
    }

    /// The stat of `name` here, `None` when it's missing.
    fn found(&self, name: &str) -> io::Result<Option<Stat>> {
        match imp::stat(&self.handle, OsStr::new(name)) {
            Ok(stat) => Ok(Some(stat)),
            Err(err) if imp::missing(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The names here, each with whether it's a folder and not a link.
    pub(crate) fn items(&self) -> io::Result<Vec<(OsString, bool)>> {
        let mut items = Vec::new();
        for (name, kind) in imp::entries(&self.handle)? {
            let folder = match kind {
                Some(kind) => kind == FileType::Directory,
                None => imp::stat(&self.handle, &name)
                    .is_ok_and(|stat| stat.kind() == FileType::Directory),
            };
            items.push((name, folder));
        }
        Ok(items)
    }

    /// Opens the folder `name` here, refusing a link.
    pub(crate) fn subdir(&self, name: &OsStr) -> io::Result<Dir> {
        Ok(Dir {
            handle: imp::open_dir_at(&self.handle, name)?,
        })
    }

    /// Removes `name` here: an empty folder when `folder` is set, a file or
    /// link otherwise.
    pub(crate) fn unlink(&self, name: &OsStr, folder: bool) -> io::Result<()> {
        imp::unlink(&self.handle, name, folder)
    }

    /// Moves `name` here to `to` in `into`, failing if `to` is taken.
    pub(crate) fn move_to(&self, name: &OsStr, into: &Dir, to: &OsStr) -> io::Result<()> {
        imp::rename(&self.handle, name, &into.handle, to, false)
    }
}

/// `file` if it's a regular file.
fn regular(file: File) -> io::Result<File> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

/// A missing name counts as removed.
fn gone(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if imp::missing(&err) => Ok(()),
        other => other,
    }
}

fn remove_at(dir: &imp::Handle, name: &OsStr, depth: usize) -> io::Result<()> {
    let folder = match imp::stat(dir, name) {
        Ok(stat) if stat.kind() == FileType::Directory => {
            if depth >= MAX_REMOVE_DEPTH {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "folders go too deep to remove",
                ));
            }
            let inner = imp::open_dir_at(dir, name)?;
            for (entry, _) in imp::entries(&inner)? {
                remove_at(&inner, &entry, depth + 1)?;
            }
            true
        }
        Ok(_) => false,
        Err(err) if imp::missing(&err) => return Ok(()),
        Err(err) => return Err(err),
    };
    gone(imp::unlink(dir, name, folder))
}

/// Whether removing a folder failed because something is still in it.
pub(crate) fn not_empty(err: &io::Error) -> bool {
    imp::not_empty(err)
}

/// Whether a move failed because its new name is taken.
pub(crate) fn name_taken(err: &io::Error) -> bool {
    imp::name_taken(err)
}

/// Whether an operation failed because the name is gone.
pub(crate) fn missing(err: &io::Error) -> bool {
    imp::missing(err)
}

/// Removes `path` if it's a link, without following it, and says whether
/// it was one.
pub(crate) fn remove_if_link(path: &Path) -> io::Result<bool> {
    imp::remove_if_link(path)
}

/// Sets a file's mode to 0755 when it's executable and 0644 otherwise.
/// Windows keeps no modes, so there it does nothing.
pub fn set_mode(file: &File, executable: bool) -> io::Result<()> {
    maxhq_os::fsmode::set_file_mode(file, if executable { 0o755 } else { 0o644 })
}

/// Whether any execute bit is set in `meta`. Never on Windows.
pub fn executable(meta: &Metadata) -> bool {
    maxhq_os::fsmode::mode_of(meta) & 0o111 != 0
}

/// What an open file is.
pub fn stat_file(file: &File) -> io::Result<Stat> {
    imp::stat_file(file)
}

/// Opens `path` to read, refusing a link as its last part.
pub fn open_nofollow(path: &Path) -> io::Result<File> {
    imp::open_nofollow(path)
}

/// Fills `buf` from `file` at `offset`.
pub fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    imp::read_exact_at(file, buf, offset)
}

/// Writes all of `buf` to `file` at `offset`.
pub fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    imp::write_all_at(file, buf, offset)
}

/// Whether a link couldn't be made on this disk or by this user, so a
/// receiver skips it and says so. Never on Unix.
pub fn cannot_link(err: &io::Error) -> bool {
    imp::cannot_link(err)
}

/// Whether a failed disk operation means the disk is full, a path broke a
/// rule, or something else.
pub(crate) fn code_of(err: &io::Error) -> ErrorCode {
    imp::code_of(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TempDir};
    use crate::Failure;

    fn parts(path: &str) -> Vec<String> {
        path.split('/').map(str::to_owned).collect()
    }

    fn bad_path(err: &io::Error) -> bool {
        Failure::io(err, None).code == ErrorCode::BadPath
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
        testing::dir_link(&outside, &tree.join("a").join("link"));

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
        let linked = testing::symlink("one", &temp.path().join("link"));
        let dir = Dir::open(temp.path()).unwrap();
        assert!(dir.same_entry("one", "two").unwrap());
        assert!(!dir.same_entry("one", "other").unwrap());
        assert!(!dir.same_entry("one", "missing").unwrap());
        assert!(!dir.same_entry("missing", "missing").unwrap());
        // A link is its own entry, not what it points at.
        if linked {
            assert!(!dir.same_entry("one", "link").unwrap());
        }
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
        testing::dir_link(&outside, &root_path.join("a").join("out"));

        assert!(root.dir_by_parts(&parts("a/b")).is_ok());
        let err = root.dir_by_parts(&parts("a/out")).unwrap_err();
        assert!(bad_path(&err), "{err}");
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
        if !testing::symlink("../secret", &root_path.join("x.part")) {
            return;
        }
        assert!(root.open_file("x.part", true).is_err());
        assert!(root.read_file("x.part").is_err());
        assert!(root.make_dir("x.part", 0o755).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert_eq!(root.file_size("x.part").unwrap(), None);
        assert_eq!(root.kind("x.part").unwrap(), Some(FileType::Symlink));
        let err = root.open_file("x.part", false).unwrap_err();
        assert!(bad_path(&err), "{err}");
    }

    #[test]
    fn links_and_modes_land_as_asked() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        let file = root.open_file("run.sh", true).unwrap();
        match root.symlink("run.sh", "latest") {
            Err(err) if cannot_link(&err) => {}
            other => {
                other.unwrap();
                root.symlink("run.sh", "latest").unwrap();
                assert!(root.symlink("other", "latest").is_err());
                assert_eq!(root.read_link("latest").unwrap(), "run.sh");
            }
        }
        set_mode(&file, true).unwrap();
        let mode = || testing::mode(&temp.path().join("run.sh"));
        if let Some(mode) = mode() {
            assert_eq!(mode, 0o755);
        }
        set_mode(&file, false).unwrap();
        if let Some(mode) = mode() {
            assert_eq!(mode, 0o644);
        }
        let (free, size) = root.space().unwrap();
        assert!(free > 0 && size >= free);
    }

    #[test]
    fn a_rename_replaces_and_a_move_never_lands_on_a_file() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        std::fs::write(temp.path().join("a"), "a").unwrap();
        std::fs::write(temp.path().join("b"), "b").unwrap();
        std::fs::write(temp.path().join("c"), "c").unwrap();
        root.rename("a", "b").unwrap();
        assert_eq!(std::fs::read(temp.path().join("b")).unwrap(), b"a");
        // A folder never lands on a file.
        root.make_dir("f", 0o755).unwrap();
        let err = root
            .move_to(OsStr::new("f"), &root, OsStr::new("c"))
            .unwrap_err();
        assert!(name_taken(&err), "{err}");
        assert_eq!(std::fs::read(temp.path().join("c")).unwrap(), b"c");
        root.move_to(OsStr::new("f"), &root, OsStr::new("d"))
            .unwrap();
        assert!(temp.path().join("d").is_dir());
    }

    #[test]
    fn a_partial_file_writes_and_reads_at_offsets() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        let file = root.open_file("part", true).unwrap();
        write_all_at(&file, b"world", 6).unwrap();
        write_all_at(&file, b"hello ", 0).unwrap();
        let mut out = [0u8; 5];
        read_exact_at(&file, &mut out, 6).unwrap();
        assert_eq!(&out, b"world");
        let err = read_exact_at(&file, &mut out, 9).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(root.file_size("part").unwrap(), Some(11));
        assert!(stat_file(&file).unwrap().same(&root.stat("part").unwrap()));
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

    #[cfg(target_os = "linux")]
    fn libc_eloop() -> i32 {
        rustix::io::Errno::LOOP.raw_os_error()
    }

    #[cfg(target_os = "linux")]
    fn libc_enotdir() -> i32 {
        rustix::io::Errno::NOTDIR.raw_os_error()
    }

    #[cfg(target_os = "linux")]
    fn libc_exdev() -> i32 {
        rustix::io::Errno::XDEV.raw_os_error()
    }
}
