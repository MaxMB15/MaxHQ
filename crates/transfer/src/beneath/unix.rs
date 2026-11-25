// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Unix: descriptors, the `*at` calls and `O_NOFOLLOW`, through rustix.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use maxhq_protocol::message::ErrorCode;
use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::io::Errno;

use super::FileType;

/// An open folder.
pub type Handle = OwnedFd;

const DIR_FLAGS: OFlags = OFlags::DIRECTORY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::RDONLY);

/// What `statat` says about a name, without following a link.
#[derive(Clone, Copy)]
pub struct Stat(rustix::fs::Stat);

impl std::fmt::Debug for Stat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stat")
            .field("kind", &self.kind())
            .field("size", &self.0.st_size)
            .finish_non_exhaustive()
    }
}

impl Stat {
    /// What it is.
    pub fn kind(&self) -> FileType {
        kind_of(rustix::fs::FileType::from_raw_mode(self.0.st_mode))
    }

    /// Its size in bytes, `None` if the disk gives a negative one.
    pub fn size(&self) -> Option<u64> {
        u64::try_from(self.0.st_size).ok()
    }

    /// Whether the file has another name, a hard link.
    pub fn second_name(&self) -> bool {
        self.0.st_nlink > 1
    }

    /// The user that owns it.
    pub fn owner(&self) -> Option<u32> {
        Some(self.0.st_uid)
    }

    /// Whether any of its execute bits is set.
    pub fn executable(&self) -> bool {
        Mode::from_raw_mode(self.0.st_mode).intersects(Mode::XUSR | Mode::XGRP | Mode::XOTH)
    }

    /// Whether this and `other` are the same file, by device and inode.
    pub fn same(&self, other: &Stat) -> bool {
        (self.0.st_dev, self.0.st_ino) == (other.0.st_dev, other.0.st_ino)
    }
}

fn kind_of(kind: rustix::fs::FileType) -> FileType {
    use rustix::fs::FileType as Raw;
    match kind {
        Raw::RegularFile => FileType::RegularFile,
        Raw::Directory => FileType::Directory,
        Raw::Symlink => FileType::Symlink,
        Raw::Fifo => FileType::Fifo,
        Raw::CharacterDevice => FileType::CharacterDevice,
        Raw::BlockDevice => FileType::BlockDevice,
        Raw::Socket => FileType::Socket,
        _ => FileType::Unknown,
    }
}

pub fn open_dir(path: &Path) -> io::Result<Handle> {
    Ok(rustix::fs::open(path, DIR_FLAGS, Mode::empty())?)
}

pub fn reopen(dir: &Handle) -> io::Result<Handle> {
    open_dir_at(dir, OsStr::new("."))
}

pub fn open_dir_at(dir: &Handle, name: &OsStr) -> io::Result<Handle> {
    Ok(rustix::fs::openat(dir, name, DIR_FLAGS, Mode::empty())?)
}

/// On Linux, opens `parts` below `dir` in one `openat2` call. `None` where
/// that's missing, so the caller goes a part at a time.
pub fn dir_beneath(dir: &Handle, parts: &[String]) -> Option<io::Result<Handle>> {
    #[cfg(target_os = "linux")]
    match openat2(dir, &parts.join("/")) {
        Err(err) if openat2_missing(&err) => None,
        other => Some(other),
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (dir, parts);
        None
    }
}

#[cfg(target_os = "linux")]
fn openat2(dir: &Handle, path: &str) -> io::Result<Handle> {
    use rustix::fs::ResolveFlags;
    Ok(rustix::fs::openat2(
        dir,
        path,
        DIR_FLAGS,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )?)
}

/// Whether `openat2` failed because the kernel lacks it or a filter
/// blocks it, rather than because the path broke a rule.
#[cfg(target_os = "linux")]
fn openat2_missing(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(code) if code == Errno::NOSYS.raw_os_error()
            || code == Errno::PERM.raw_os_error()
    )
}

#[cfg(target_os = "linux")]
impl super::Dir {
    /// Opens a folder below this one with `openat2`, which refuses `..`, an
    /// absolute path and any link on the way.
    pub fn dir_openat2(&self, parts: &[String]) -> io::Result<super::Dir> {
        self.openat2_raw(&parts.join("/"))
    }

    /// `openat2` on a path as given, for the tests that check the kernel
    /// refuses an escape.
    pub(crate) fn openat2_raw(&self, path: &str) -> io::Result<super::Dir> {
        Ok(super::Dir {
            handle: openat2(&self.handle, path)?,
        })
    }
}

fn raw_mode(mode: u32) -> io::Result<Mode> {
    let mode = rustix::fs::RawMode::try_from(mode)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    Ok(Mode::from_raw_mode(mode))
}

pub fn make_dir(dir: &Handle, name: &OsStr, mode: u32) -> io::Result<()> {
    Ok(rustix::fs::mkdirat(dir, name, raw_mode(mode)?)?)
}

pub fn open_file(dir: &Handle, name: &OsStr, create: bool) -> io::Result<File> {
    let mut flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if create {
        flags |= OFlags::CREATE;
    }
    let fd = rustix::fs::openat(dir, name, flags, Mode::from_raw_mode(0o600))?;
    Ok(File::from(fd))
}

pub fn read_file(dir: &Handle, name: &OsStr) -> io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let fd = rustix::fs::openat(dir, name, flags, Mode::empty())?;
    Ok(File::from(fd))
}

pub fn set_own_mode(dir: &Handle, mode: u32) -> io::Result<()> {
    Ok(rustix::fs::fchmod(dir, raw_mode(mode)?)?)
}

/// The names in `dir` without `.` and `..`, each with what the folder
/// listing says it is, `None` where the disk doesn't say.
pub fn entries(dir: &Handle) -> io::Result<Vec<(OsString, Option<FileType>)>> {
    let mut names = Vec::new();
    for entry in rustix::fs::Dir::read_from(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let kind = match entry.file_type() {
            rustix::fs::FileType::Unknown => None,
            kind => Some(kind_of(kind)),
        };
        names.push((OsString::from_vec(name.to_bytes().to_vec()), kind));
    }
    Ok(names)
}

pub fn read_link(dir: &Handle, name: &OsStr) -> io::Result<OsString> {
    let target = rustix::fs::readlinkat(dir, name, Vec::new())?;
    Ok(OsString::from_vec(target.into_bytes()))
}

pub fn stat(dir: &Handle, name: &OsStr) -> io::Result<Stat> {
    Ok(Stat(rustix::fs::statat(
        dir,
        name,
        AtFlags::SYMLINK_NOFOLLOW,
    )?))
}

pub fn stat_file(file: &File) -> io::Result<Stat> {
    Ok(Stat(rustix::fs::fstat(file)?))
}

/// `renameat`, which replaces what's at `to` whatever `_replace` says.
pub fn rename(
    from_dir: &Handle,
    from: &OsStr,
    to_dir: &Handle,
    to: &OsStr,
    _replace: bool,
) -> io::Result<()> {
    Ok(rustix::fs::renameat(from_dir, from, to_dir, to)?)
}

pub fn symlink(target: &str, dir: &Handle, name: &OsStr) -> io::Result<()> {
    Ok(rustix::fs::symlinkat(target, dir, name)?)
}

pub fn unlink(dir: &Handle, name: &OsStr, folder: bool) -> io::Result<()> {
    let flags = if folder {
        AtFlags::REMOVEDIR
    } else {
        AtFlags::empty()
    };
    Ok(rustix::fs::unlinkat(dir, name, flags)?)
}

pub fn chown(dir: &Handle, name: &OsStr, uid: u32, gid: u32) -> io::Result<()> {
    Ok(rustix::fs::chownat(
        dir,
        name,
        Some(rustix::fs::Uid::from_raw(uid)),
        Some(rustix::fs::Gid::from_raw(gid)),
        AtFlags::SYMLINK_NOFOLLOW,
    )?)
}

pub fn sync(dir: &Handle) -> io::Result<()> {
    Ok(rustix::fs::fsync(dir)?)
}

pub fn space(dir: &Handle) -> io::Result<(u64, u64)> {
    let stat = rustix::fs::fstatvfs(dir)?;
    let block = stat.f_frsize.max(1);
    Ok((
        stat.f_bavail.saturating_mul(block),
        stat.f_blocks.saturating_mul(block),
    ))
}

pub fn open_nofollow(path: &Path) -> io::Result<File> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(fd))
}

pub fn remove_if_link(path: &Path) -> io::Result<bool> {
    if std::fs::symlink_metadata(path)?.is_symlink() {
        std::fs::remove_file(path)?;
        return Ok(true);
    }
    Ok(false)
}

fn errno(err: &io::Error) -> Option<Errno> {
    err.raw_os_error().map(Errno::from_raw_os_error)
}

pub fn code_of(err: &io::Error) -> ErrorCode {
    match errno(err) {
        Some(Errno::NOSPC | Errno::DQUOT) => ErrorCode::NoSpace,
        Some(Errno::LOOP | Errno::XDEV | Errno::NOTDIR | Errno::EXIST) => ErrorCode::BadPath,
        _ => ErrorCode::Internal,
    }
}

pub fn missing(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound
}

pub fn not_empty(err: &io::Error) -> bool {
    matches!(errno(err), Some(Errno::NOTEMPTY | Errno::EXIST))
}

pub fn name_taken(err: &io::Error) -> bool {
    matches!(
        errno(err),
        Some(Errno::NOTEMPTY | Errno::EXIST | Errno::NOTDIR | Errno::ISDIR)
    )
}

/// Unix makes any link it's asked for.
pub fn cannot_link(_err: &io::Error) -> bool {
    false
}
