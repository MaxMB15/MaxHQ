// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Windows: a folder handle, names opened relative to it with
//! `NtCreateFile`, and reparse points opened as themselves so a symlink or
//! junction is refused rather than followed.
//!
//! A name is one plain name. One holding `\`, `/` or `:` would reach a
//! parent, a drive, a share or an alternate data stream, so it's refused
//! before anything opens, along with `.`, `..` and an empty name.

// The NT calls std doesn't wrap, which open a name relative to a folder
// handle and refuse reparse points, need unsafe. Each block says why it
// holds.
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::mem::offset_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use maxhq_os::win;
use maxhq_protocol::message::ErrorCode;
use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows_sys::Wdk::Storage::FileSystem::{
    FileDispositionInformation, FileDispositionInformationEx, FileFsFullSizeInformation,
    FileRenameInformation, FileRenameInformationEx, NtCreateFile, NtQueryVolumeInformationFile,
    NtSetInformationFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_DISPOSITION_DELETE,
    FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_POSIX_SEMANTICS,
    FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
    FILE_RENAME_IGNORE_READONLY_ATTRIBUTE, FILE_RENAME_POSIX_SEMANTICS,
    FILE_RENAME_REPLACE_IF_EXISTS, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    RtlNtStatusToDosError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_CANT_ACCESS_FILE,
    ERROR_DELETE_PENDING, ERROR_DIRECTORY, ERROR_DIR_NOT_EMPTY, ERROR_DISK_FULL,
    ERROR_DISK_QUOTA_EXCEEDED, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_HANDLE_DISK_FULL,
    ERROR_INVALID_FUNCTION, ERROR_INVALID_NAME, ERROR_INVALID_PARAMETER,
    ERROR_INVALID_REPARSE_DATA, ERROR_NOT_A_REPARSE_POINT, ERROR_NOT_SUPPORTED,
    ERROR_NO_MORE_FILES, ERROR_PRIVILEGE_NOT_HELD, ERROR_REPARSE_POINT_ENCOUNTERED,
    ERROR_STOPPED_ON_SYMLINK, HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, STATUS_DELETE_PENDING,
    UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateSymbolicLinkW, FileAttributeTagInfo, FileFullDirectoryInfo, FileFullDirectoryRestartInfo,
    FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_TRAVERSE, SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE, SYMBOLIC_LINK_FLAG_DIRECTORY,
    SYNCHRONIZE, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::IO::{DeviceIoControl, IO_STATUS_BLOCK};

use super::FileType;

/// An open folder.
pub type Handle = OwnedHandle;

const DIR_ACCESS: u32 = FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00A8;
const MAX_REPARSE: usize = 16 * 1024;
const SYMLINK_FLAG_RELATIVE: u32 = 1;

const TAG_SYMLINK: u32 = 0xA000_000C;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
const TAG_LX_SYMLINK: u32 = 0xA000_001D;
const TAG_AF_UNIX: u32 = 0x8000_0023;
const TAG_LX_FIFO: u32 = 0x8000_0024;
const TAG_LX_CHR: u32 = 0x8000_0025;
const TAG_LX_BLK: u32 = 0x8000_0026;
/// Set in the tag of a reparse point that stands for another name: a
/// symlink, a junction, a WSL link.
const TAG_SURROGATE: u32 = 0x2000_0000;

const SLASH: u16 = b'/' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// `FILE_FS_FULL_SIZE_INFORMATION`.
#[repr(C)]
#[derive(Default)]
struct FsFullSize {
    total: i64,
    caller_available: i64,
    _actual_available: i64,
    sectors_per_unit: u32,
    bytes_per_sector: u32,
}

fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

fn code(err: &io::Error) -> Option<u32> {
    err.raw_os_error().and_then(|code| u32::try_from(code).ok())
}

/// `name` as UTF-16 for `NtCreateFile`, refused unless it's one plain name
/// that can't reach a parent, a drive, a share or a stream.
fn wide_name(name: &OsStr) -> io::Result<Vec<u16>> {
    let wide: Vec<u16> = name.encode_wide().collect();
    let plain = !wide.is_empty()
        && wide != [u16::from(b'.')]
        && wide != [u16::from(b'.'), u16::from(b'.')]
        && !wide
            .iter()
            .any(|&unit| matches!(unit, 0 | SLASH | BACKSLASH | 0x3A))
        && wide.len() <= usize::from(u16::MAX / 2);
    if plain {
        Ok(wide)
    } else {
        Err(os_error(ERROR_INVALID_NAME))
    }
}

/// Opens `name` relative to `dir`. An empty name opens `dir` itself again.
fn nt_create(
    dir: &impl AsRawHandle,
    name: &[u16],
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<Handle> {
    let bytes = u16::try_from(name.len() * 2).map_err(|_| os_error(ERROR_INVALID_NAME))?;
    let object = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: if name.is_empty() {
            ptr::null_mut()
        } else {
            name.as_ptr().cast_mut()
        },
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: dir.as_raw_handle(),
        ObjectName: &object,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut handle: HANDLE = ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: every pointer is to a live local for the length of the call;
    // the name's buffer outlives `object`, `NtCreateFile` only reads it, and
    // `dir` is an open handle.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &attributes,
            &mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            SHARE_ALL,
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT,
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        return Err(nt_error(result));
    }
    // SAFETY: `NtCreateFile` succeeded, so `handle` is a new handle no one
    // else owns.
    unsafe { win::owned(handle) }
}

fn nt_error(status: NTSTATUS) -> io::Error {
    // Windows maps a pending delete to "access denied", which hides that
    // the name is on its way out.
    if status == STATUS_DELETE_PENDING {
        return os_error(ERROR_DELETE_PENDING);
    }
    // SAFETY: this only maps one number to another.
    os_error(unsafe { RtlNtStatusToDosError(status) })
}

/// The handle's attributes and its reparse tag, 0 when it isn't a reparse
/// point.
fn tag_info(handle: &impl AsRawHandle) -> io::Result<(u32, u32)> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `info` is a FILE_ATTRIBUTE_TAG_INFO of the size passed, and the
    // handle is open.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            (&raw mut info).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let tag = if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        info.ReparseTag
    } else {
        0
    };
    Ok((info.FileAttributes, tag))
}

/// Whether these say a symlink, a junction or another name for something
/// else.
fn is_link(attrs: u32, tag: u32) -> bool {
    attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 && tag & TAG_SURROGATE != 0
}

fn kind_of(attrs: u32, tag: u32) -> FileType {
    match tag {
        TAG_AF_UNIX => FileType::Socket,
        TAG_LX_FIFO => FileType::Fifo,
        TAG_LX_CHR => FileType::CharacterDevice,
        TAG_LX_BLK => FileType::BlockDevice,
        _ if is_link(attrs, tag) => FileType::Symlink,
        _ if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 => FileType::Directory,
        _ => FileType::RegularFile,
    }
}

/// What a handle opened on a name, without following a link, says about it.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    attrs: u32,
    tag: u32,
    size: u64,
    links: u32,
    id: (u64, u128),
}

impl Stat {
    /// What it is. A junction counts as a link.
    pub fn kind(&self) -> FileType {
        kind_of(self.attrs, self.tag)
    }

    /// Its size in bytes.
    pub fn size(&self) -> Option<u64> {
        Some(self.size)
    }

    /// Whether the file has another name, a hard link.
    pub fn second_name(&self) -> bool {
        self.links > 1
    }

    /// Windows files have no Unix owner.
    pub fn owner(&self) -> Option<u32> {
        None
    }

    /// Windows keeps no execute bit.
    pub fn executable(&self) -> bool {
        false
    }

    /// Whether this and `other` are the same file, by volume and file id.
    pub fn same(&self, other: &Stat) -> bool {
        self.id == other.id
    }
}

fn stat_of(handle: &impl AsRawHandle) -> io::Result<Stat> {
    let raw = handle.as_raw_handle();
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `info` is the struct the call fills, and the handle is open.
    if unsafe { GetFileInformationByHandle(raw, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let attrs = info.dwFileAttributes;
    let tag = if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        tag_info(handle)?.1
    } else {
        0
    };
    let mut full = FILE_ID_INFO::default();
    // SAFETY: `full` is a FILE_ID_INFO of the size passed, and the handle is
    // open.
    let got = unsafe {
        GetFileInformationByHandleEx(
            raw,
            FileIdInfo,
            (&raw mut full).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    // ReFS ids don't fit the 64 bits the older call gives.
    let id = if got != 0 {
        (
            full.VolumeSerialNumber,
            u128::from_le_bytes(full.FileId.Identifier),
        )
    } else {
        (
            u64::from(info.dwVolumeSerialNumber),
            (u128::from(info.nFileIndexHigh) << 32) | u128::from(info.nFileIndexLow),
        )
    };
    Ok(Stat {
        attrs,
        tag,
        size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        links: info.nNumberOfLinks,
        id,
    })
}

/// Keeps `first`, opened as the name itself, unless it's a reparse point. A
/// link is refused the way `O_NOFOLLOW` refuses one. Any other reparse point,
/// a cloud placeholder say, opens again through `reopen` and must still be
/// the same file.
fn follow_if_plain(
    first: Handle,
    reopen: impl FnOnce() -> io::Result<Handle>,
) -> io::Result<Handle> {
    let (attrs, tag) = tag_info(&first)?;
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        return Ok(first);
    }
    if is_link(attrs, tag) {
        return Err(os_error(ERROR_STOPPED_ON_SYMLINK));
    }
    let before = stat_of(&first)?;
    let opened = reopen()?;
    if !stat_of(&opened)?.same(&before) {
        return Err(os_error(ERROR_STOPPED_ON_SYMLINK));
    }
    Ok(opened)
}

/// Opens the plain name `name` in `dir`, refusing a link in its place.
fn open_checked(
    dir: &Handle,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<Handle> {
    let wide = wide_name(name)?;
    let first = nt_create(
        dir,
        &wide,
        access,
        disposition,
        options | FILE_OPEN_REPARSE_POINT,
    )?;
    follow_if_plain(first, || nt_create(dir, &wide, access, FILE_OPEN, options))
}

pub fn open_dir(path: &Path) -> io::Result<Handle> {
    let open = |flags: u32| -> io::Result<Handle> {
        let file = OpenOptions::new()
            .access_mode(DIR_ACCESS)
            .share_mode(SHARE_ALL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | flags)
            .open(path)?;
        Ok(OwnedHandle::from(file))
    };
    let handle = follow_if_plain(open(FILE_FLAG_OPEN_REPARSE_POINT)?, || open(0))?;
    if tag_info(&handle)?.0 & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(os_error(ERROR_DIRECTORY));
    }
    Ok(handle)
}

pub fn reopen(dir: &Handle) -> io::Result<Handle> {
    nt_create(dir, &[], DIR_ACCESS, FILE_OPEN, FILE_DIRECTORY_FILE)
}

pub fn open_dir_at(dir: &Handle, name: &OsStr) -> io::Result<Handle> {
    open_checked(dir, name, DIR_ACCESS, FILE_OPEN, FILE_DIRECTORY_FILE)
}

/// Windows has no one call that opens a path beneath a folder, so the
/// caller goes a part at a time.
pub fn dir_beneath(_dir: &Handle, _parts: &[String]) -> Option<io::Result<Handle>> {
    None
}

/// Makes the folder `name`. Windows has no mode bits: the folder takes its
/// parent's access list.
pub fn make_dir(dir: &Handle, name: &OsStr, _mode: u32) -> io::Result<()> {
    nt_create(
        dir,
        &wide_name(name)?,
        FILE_LIST_DIRECTORY,
        FILE_CREATE,
        FILE_DIRECTORY_FILE,
    )
    .map(drop)
}

pub fn open_file(dir: &Handle, name: &OsStr, create: bool) -> io::Result<File> {
    let disposition = if create { FILE_OPEN_IF } else { FILE_OPEN };
    let access = FILE_GENERIC_READ | FILE_GENERIC_WRITE;
    open_checked(dir, name, access, disposition, FILE_NON_DIRECTORY_FILE).map(File::from)
}

pub fn read_file(dir: &Handle, name: &OsStr) -> io::Result<File> {
    open_checked(dir, name, FILE_GENERIC_READ, FILE_OPEN, 0).map(File::from)
}

/// Windows has no mode bits to set.
pub fn set_own_mode(_dir: &Handle, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// The names in `dir` without `.` and `..`, each with what the listing says
/// it is. A reparse point's kind is `None`, so the caller asks the name
/// itself.
pub fn entries(dir: &Handle) -> io::Result<Vec<(OsString, Option<FileType>)>> {
    let listing = reopen(dir)?;
    let mut buffer = vec![0u64; 2048];
    let mut names = Vec::new();
    let mut class = FileFullDirectoryRestartInfo;
    loop {
        // SAFETY: the buffer is writable for the length passed, in bytes,
        // and 8-byte aligned, and the handle is an open folder.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                listing.as_raw_handle(),
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            match code(&err) {
                Some(ERROR_NO_MORE_FILES) => break,
                Some(ERROR_FILE_NOT_FOUND) if class == FileFullDirectoryRestartInfo => break,
                _ => return Err(err),
            }
        }
        class = FileFullDirectoryInfo;
        let bytes: Vec<u8> = buffer.iter().flat_map(|word| word.to_ne_bytes()).collect();
        read_listing(&bytes, &mut names)?;
    }
    Ok(names)
}

fn bad_listing() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "a folder listing ran short")
}

fn u32_at(bytes: &[u8], at: usize) -> io::Result<u32> {
    let four = bytes.get(at..at + 4).ok_or_else(bad_listing)?;
    Ok(u32::from_ne_bytes([four[0], four[1], four[2], four[3]]))
}

fn u16_at(bytes: &[u8], at: usize) -> io::Result<u16> {
    let two = bytes.get(at..at + 2).ok_or_else(bad_listing)?;
    Ok(u16::from_ne_bytes([two[0], two[1]]))
}

fn units(bytes: &[u8]) -> Vec<u16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().copied().map(u16::from_ne_bytes).collect()
}

/// Reads the `FILE_FULL_DIR_INFO` records one listing call left in `bytes`.
fn read_listing(bytes: &[u8], names: &mut Vec<(OsString, Option<FileType>)>) -> io::Result<()> {
    let mut at = 0;
    loop {
        let next = u32_at(bytes, at)?;
        let attrs = u32_at(bytes, at + offset_of!(FILE_FULL_DIR_INFO, FileAttributes))?;
        let length = u32_at(bytes, at + offset_of!(FILE_FULL_DIR_INFO, FileNameLength))?;
        let start = at + offset_of!(FILE_FULL_DIR_INFO, FileName);
        let length = usize::try_from(length).map_err(|_| bad_listing())?;
        let name = bytes.get(start..start + length).ok_or_else(bad_listing)?;
        let name = OsString::from_wide(&units(name));
        if name != "." && name != ".." {
            let kind = if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                None
            } else if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
                Some(FileType::Directory)
            } else {
                Some(FileType::RegularFile)
            };
            names.push((name, kind));
        }
        if next == 0 {
            return Ok(());
        }
        at += usize::try_from(next).map_err(|_| bad_listing())?;
    }
}

/// Opens the name itself, whatever it is, to look at or remove it.
fn open_entry(dir: &Handle, name: &OsStr, access: u32) -> io::Result<Handle> {
    nt_create(
        dir,
        &wide_name(name)?,
        access,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT,
    )
}

pub fn stat(dir: &Handle, name: &OsStr) -> io::Result<Stat> {
    stat_of(&open_entry(dir, name, 0)?)
}

pub fn stat_file(file: &File) -> io::Result<Stat> {
    stat_of(file)
}

/// Renames `from` in `from_dir` to `to` in `to_dir`, the link itself if it's
/// one. With POSIX semantics the name it replaces may be open.
pub fn rename(
    from_dir: &Handle,
    from: &OsStr,
    to_dir: &Handle,
    to: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let to = wide_name(to)?;
    let source = open_entry(from_dir, from, DELETE)?;
    let flags = FILE_RENAME_POSIX_SEMANTICS
        | FILE_RENAME_IGNORE_READONLY_ATTRIBUTE
        | if replace {
            FILE_RENAME_REPLACE_IF_EXISTS
        } else {
            0
        };
    match set_rename(&source, to_dir, &to, flags, FileRenameInformationEx) {
        // Before Windows 10 1709, and on disks that lack it.
        Err(err) if unsupported(&err) => set_rename(
            &source,
            to_dir,
            &to,
            u32::from(replace),
            FileRenameInformation,
        ),
        other => other,
    }
}

/// `FILE_RENAME_INFO` for `name` in `root`, whose first field is the flags
/// or, for the older class, the replace byte.
fn set_rename(
    source: &Handle,
    root: &Handle,
    name: &[u16],
    flags: u32,
    class: i32,
) -> io::Result<()> {
    let name_at = offset_of!(FILE_RENAME_INFO, FileName);
    let root_at = offset_of!(FILE_RENAME_INFO, RootDirectory);
    let length_at = offset_of!(FILE_RENAME_INFO, FileNameLength);
    let length = u32::try_from(name.len() * 2).map_err(|_| os_error(ERROR_INVALID_NAME))?;
    let mut bytes = vec![0u8; size_of::<FILE_RENAME_INFO>() + name.len() * 2];
    bytes[..4].copy_from_slice(&flags.to_ne_bytes());
    let root = root.as_raw_handle().addr().to_ne_bytes();
    bytes[root_at..root_at + root.len()].copy_from_slice(&root);
    bytes[length_at..length_at + 4].copy_from_slice(&length.to_ne_bytes());
    for (index, unit) in name.iter().enumerate() {
        let at = name_at + index * 2;
        bytes[at..at + 2].copy_from_slice(&unit.to_ne_bytes());
    }
    set_info(source, &bytes, class)
}

/// `NtSetInformationFile` with `bytes`, copied to an 8-byte aligned buffer.
fn set_info(handle: &Handle, bytes: &[u8], class: i32) -> io::Result<()> {
    let mut aligned = vec![0u64; bytes.len().div_ceil(8)];
    for (word, chunk) in aligned.iter_mut().zip(bytes.chunks(8)) {
        let mut eight = [0u8; 8];
        eight[..chunk.len()].copy_from_slice(chunk);
        *word = u64::from_ne_bytes(eight);
    }
    let length = u32::try_from(bytes.len()).map_err(|_| os_error(ERROR_INVALID_PARAMETER))?;
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: `aligned` holds `length` readable bytes laid out as `class`
    // expects, and the handle is open.
    let result = unsafe {
        NtSetInformationFile(
            handle.as_raw_handle(),
            &mut status,
            aligned.as_ptr().cast(),
            length,
            class,
        )
    };
    if result < 0 {
        return Err(nt_error(result));
    }
    Ok(())
}

/// Whether a call failed because this Windows or this disk lacks it.
fn unsupported(err: &io::Error) -> bool {
    matches!(
        code(err),
        Some(ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION)
    )
}

/// Deletes what `handle` has open. POSIX semantics take the name away at
/// once, even while another handle holds the file.
fn set_disposition(handle: &Handle) -> io::Result<()> {
    let flags = FILE_DISPOSITION_DELETE
        | FILE_DISPOSITION_POSIX_SEMANTICS
        | FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE;
    match set_info(handle, &flags.to_ne_bytes(), FileDispositionInformationEx) {
        Err(err) if unsupported(&err) => set_info(handle, &[1], FileDispositionInformation),
        other => other,
    }
}

/// Removes `name`, the link itself if it's one. `folder` says whether a
/// real folder is meant, as `AT_REMOVEDIR` does.
pub fn unlink(dir: &Handle, name: &OsStr, folder: bool) -> io::Result<()> {
    let handle = open_entry(dir, name, DELETE)?;
    let (attrs, tag) = tag_info(&handle)?;
    let real_dir = attrs & FILE_ATTRIBUTE_DIRECTORY != 0 && !is_link(attrs, tag);
    if real_dir && !folder {
        return Err(os_error(ERROR_ACCESS_DENIED));
    }
    if !real_dir && folder {
        return Err(os_error(ERROR_DIRECTORY));
    }
    set_disposition(&handle)
}

/// Makes the link `name` in `dir` pointing at the relative `target`, with
/// `/` between its parts.
///
/// A target with `\` or `:`, or one starting with `/`, is refused: Windows
/// reads `\` as a separator and `C:` as a drive, so a target that reads as
/// staying inside on Unix could leave the folder here. A refused link
/// counts as one that can't be made.
///
/// Windows makes links by path, so the link lands under a fresh name, is
/// checked through `dir`'s handle to be a link, and then renamed to `name`
/// relative to the handle.
pub fn symlink(target: &str, dir: &Handle, name: &OsStr) -> io::Result<()> {
    wide_name(name)?;
    if target.is_empty() || target.starts_with('/') || target.contains(['\\', ':', '\0']) {
        return Err(os_error(ERROR_INVALID_REPARSE_DATA));
    }
    let folder = final_path(dir)?;
    let temp = format!(
        ".maxhq-link-{:016x}",
        RandomState::new().hash_one((target, name))
    );
    let link = win::wide(folder.join(&temp))?;
    let to = win::ended(
        target
            .encode_utf16()
            .map(|unit| if unit == SLASH { BACKSLASH } else { unit })
            .collect(),
    )?;
    let kind = if points_at_folder(&folder, target) {
        SYMBOLIC_LINK_FLAG_DIRECTORY
    } else {
        0
    };
    match create_link(
        &link,
        &to,
        kind | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE,
    ) {
        // Windows before 10 1703 doesn't know the unprivileged flag.
        Err(err) if code(&err) == Some(ERROR_INVALID_PARAMETER) => create_link(&link, &to, kind),
        other => other,
    }?;
    let temp = OsString::from(temp);
    match stat(dir, &temp)?.kind() {
        FileType::Symlink => {}
        // Not the link just made, so not this call's to remove.
        _ => return Err(os_error(ERROR_CANT_ACCESS_FILE)),
    }
    let moved = rename(dir, &temp, dir, name, false);
    if moved.is_err() {
        let _ = unlink(dir, &temp, false);
    }
    moved
}

fn create_link(link: &[u16], target: &[u16], flags: u32) -> io::Result<()> {
    // SAFETY: both are NUL-terminated UTF-16 strings that outlive the call.
    if unsafe { CreateSymbolicLinkW(link.as_ptr(), target.as_ptr(), flags) } {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Where `dir` is now, as a `\\?\` path.
fn final_path(dir: &Handle) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 512];
    loop {
        let room = u32::try_from(buffer.len()).map_err(|_| os_error(ERROR_INVALID_NAME))?;
        // SAFETY: the buffer is writable for `room` units, and the handle
        // is open.
        let got = unsafe {
            GetFinalPathNameByHandleW(
                dir.as_raw_handle(),
                buffer.as_mut_ptr(),
                room,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        let got = usize::try_from(got).map_err(|_| os_error(ERROR_INVALID_NAME))?;
        if got == 0 {
            return Err(io::Error::last_os_error());
        }
        if got < buffer.len() {
            buffer.truncate(got);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        if got > 32 * 1024 + 1 {
            return Err(os_error(ERROR_INVALID_NAME));
        }
        buffer.resize(got, 0);
    }
}

/// Whether the relative `target` from `folder` is a folder now, which a
/// Windows link must say when it's made. Windows resolves `..` in a link by
/// the name, so this does too.
fn points_at_folder(folder: &Path, target: &str) -> bool {
    let mut path = folder.to_path_buf();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            part => path.push(part),
        }
    }
    std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir())
}

/// Where the link `name` points: a relative symlink's target with `/`
/// between its parts, and anything absolute, a junction's target say, as a
/// path starting with `/`, so it never reads as inside.
pub fn read_link(dir: &Handle, name: &OsStr) -> io::Result<OsString> {
    let handle = open_entry(dir, name, 0)?;
    let mut buffer = vec![0u64; MAX_REPARSE / 8];
    let mut got = 0u32;
    // SAFETY: the out buffer is writable for the length passed, `got` is a
    // live local, there's no in buffer, and the handle is open for
    // synchronous use.
    let ok = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            FSCTL_GET_REPARSE_POINT,
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            MAX_REPARSE as u32,
            &mut got,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let got = usize::try_from(got).unwrap_or(MAX_REPARSE);
    let bytes: Vec<u8> = buffer
        .iter()
        .flat_map(|word| word.to_ne_bytes())
        .take(got)
        .collect();
    link_target(&bytes)
}

/// The target in a `REPARSE_DATA_BUFFER`.
fn link_target(bytes: &[u8]) -> io::Result<OsString> {
    let bad = |_: io::Error| os_error(ERROR_INVALID_REPARSE_DATA);
    // A symlink's and a junction's substitute name, after their fixed part.
    let substitute = |base: usize| -> io::Result<Vec<u16>> {
        let start = base + usize::from(u16_at(bytes, 8)?);
        let length = usize::from(u16_at(bytes, 10)?);
        let raw = bytes.get(start..start + length).ok_or_else(bad_listing)?;
        Ok(units(raw))
    };
    match u32_at(bytes, 0).map_err(bad)? {
        TAG_SYMLINK => {
            let target = substitute(20).map_err(bad)?;
            if u32_at(bytes, 16).map_err(bad)? & SYMLINK_FLAG_RELATIVE == 0 {
                return Ok(absolute(&target));
            }
            let target: Vec<u16> = target
                .into_iter()
                .map(|unit| if unit == BACKSLASH { SLASH } else { unit })
                .collect();
            Ok(OsString::from_wide(&target))
        }
        TAG_MOUNT_POINT => Ok(absolute(&substitute(16).map_err(bad)?)),
        TAG_LX_SYMLINK => {
            let end = 8 + usize::from(u16_at(bytes, 4).map_err(bad)?);
            let text = bytes
                .get(12..end)
                .and_then(|raw| std::str::from_utf8(raw).ok())
                .ok_or_else(|| os_error(ERROR_INVALID_REPARSE_DATA))?;
            Ok(OsString::from(text))
        }
        _ => Err(os_error(ERROR_NOT_A_REPARSE_POINT)),
    }
}

/// An absolute NT target as a path that starts with `/`.
fn absolute(target: &[u16]) -> OsString {
    let text = String::from_utf16_lossy(target);
    let text = text
        .strip_prefix(r"\??\")
        .or_else(|| text.strip_prefix(r"\\?\"))
        .unwrap_or(&text);
    let text = match text.strip_prefix(r"UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.to_owned(),
    };
    let text = text.replace('\\', "/");
    if text.starts_with('/') {
        OsString::from(text)
    } else {
        OsString::from(format!("/{text}"))
    }
}

/// Windows can't give a name to another user this way.
pub fn chown(_dir: &Handle, _name: &OsStr, _uid: u32, _gid: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows files have no Unix owner",
    ))
}

/// Windows writes a folder's entries down with the files in it.
pub fn sync(_dir: &Handle) -> io::Result<()> {
    Ok(())
}

pub fn space(dir: &Handle) -> io::Result<(u64, u64)> {
    let mut info = FsFullSize::default();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: `info` is the struct the class fills, of the size passed, and
    // the handle is open.
    let result = unsafe {
        NtQueryVolumeInformationFile(
            dir.as_raw_handle(),
            &mut status,
            (&raw mut info).cast(),
            size_of::<FsFullSize>() as u32,
            FileFsFullSizeInformation,
        )
    };
    if result < 0 {
        return Err(nt_error(result));
    }
    let unit = u64::from(info.sectors_per_unit)
        .saturating_mul(u64::from(info.bytes_per_sector))
        .max(1);
    let bytes = |count: i64| u64::try_from(count).unwrap_or(0).saturating_mul(unit);
    Ok((bytes(info.caller_available), bytes(info.total)))
}

pub fn open_nofollow(path: &Path) -> io::Result<File> {
    let open = |flags: u32| -> io::Result<Handle> {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(SHARE_ALL)
            .custom_flags(flags)
            .open(path)?;
        Ok(OwnedHandle::from(file))
    };
    follow_if_plain(open(FILE_FLAG_OPEN_REPARSE_POINT)?, || open(0)).map(File::from)
}

/// Removes `path` if it's a link or junction, without following it.
pub fn remove_if_link(path: &Path) -> io::Result<bool> {
    if !std::fs::symlink_metadata(path)?.is_symlink() {
        return Ok(false);
    }
    let file = OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let handle = OwnedHandle::from(file);
    // It may have turned into a folder since it was looked at.
    let (attrs, tag) = tag_info(&handle)?;
    if !is_link(attrs, tag) {
        return Ok(false);
    }
    set_disposition(&handle)?;
    Ok(true)
}

pub fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ))
            }
            Ok(read) => {
                buf = &mut buf[read..];
                offset += read as u64;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ))
            }
            Ok(wrote) => {
                buf = &buf[wrote..];
                offset += wrote as u64;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub fn code_of(err: &io::Error) -> ErrorCode {
    match code(err) {
        Some(ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL | ERROR_DISK_QUOTA_EXCEEDED) => {
            ErrorCode::NoSpace
        }
        Some(
            ERROR_STOPPED_ON_SYMLINK
            | ERROR_REPARSE_POINT_ENCOUNTERED
            | ERROR_DIRECTORY
            | ERROR_ALREADY_EXISTS
            | ERROR_FILE_EXISTS
            | ERROR_INVALID_NAME
            | ERROR_INVALID_REPARSE_DATA
            | ERROR_CANT_ACCESS_FILE,
        ) => ErrorCode::BadPath,
        _ => ErrorCode::Internal,
    }
}

pub fn missing(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound || code(err) == Some(ERROR_DELETE_PENDING)
}

pub fn not_empty(err: &io::Error) -> bool {
    code(err) == Some(ERROR_DIR_NOT_EMPTY)
}

pub fn name_taken(err: &io::Error) -> bool {
    matches!(
        code(err),
        Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS | ERROR_DELETE_PENDING)
    )
}

/// Whether a link couldn't be made here: no right to make links, a target
/// Windows would read differently, or a disk without links.
pub fn cannot_link(err: &io::Error) -> bool {
    matches!(
        code(err),
        Some(
            ERROR_PRIVILEGE_NOT_HELD
                | ERROR_INVALID_REPARSE_DATA
                | ERROR_NOT_SUPPORTED
                | ERROR_INVALID_FUNCTION
        )
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use maxhq_os::win;
    use maxhq_protocol::message::ErrorCode;
    use std::os::windows::fs::FileTypeExt;
    use windows_sys::Win32::Foundation::ERROR_PRIVILEGE_NOT_HELD;

    use super::super::{cannot_link, Dir, FileType};
    use crate::testing::{self, TempDir};
    use crate::Failure;

    /// Names that would reach a parent, a drive, a share, a stream or a
    /// second level if they were opened as paths.
    const ESCAPES: &[&str] = &[
        "..",
        ".",
        "",
        r"a\b",
        r"..\x",
        "C:x",
        r"C:\Windows",
        r"\\server\share\x",
        r"\\?\C:\x",
        "name:stream",
        "name::$DATA",
        "x/y",
        "a\0b",
        r"\x",
        "/x",
    ];

    fn parts(path: &str) -> Vec<String> {
        path.split('/').map(str::to_owned).collect()
    }

    fn listing(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_name_never_reaches_out_of_its_folder() {
        let temp = TempDir::new();
        std::fs::write(temp.path().join("x"), "kept").unwrap();
        let root_path = temp.path().join("root");
        let root = Dir::create(&root_path).unwrap();
        std::fs::write(root_path.join("name"), "inside").unwrap();
        let before = (listing(temp.path()), listing(&root_path));

        for &name in ESCAPES {
            assert!(root.open_file(name, true).is_err(), "open_file {name:?}");
            assert!(root.make_dir(name, 0o755).is_err(), "make_dir {name:?}");
            assert!(root.read_file(name).is_err(), "read_file {name:?}");
            assert!(root.read_regular(name).is_err(), "read_regular {name:?}");
            assert!(root.kind(name).is_err(), "kind {name:?}");
            assert!(root.stat(name).is_err(), "stat {name:?}");
            assert!(root.file_size(name).is_err(), "file_size {name:?}");
            assert!(root.read_link(name).is_err(), "read_link {name:?}");
            assert!(root.symlink("t", name).is_err(), "symlink {name:?}");
            assert!(root.dir(&[name.to_owned()]).is_err(), "dir {name:?}");
            assert!(root.remove_file(name).is_err(), "remove_file {name:?}");
            assert!(root.remove_tree(name).is_err(), "remove_tree {name:?}");
            assert!(root.rename("name", name).is_err(), "rename to {name:?}");
            assert!(root.rename(name, "moved").is_err(), "rename from {name:?}");
        }

        assert_eq!((listing(temp.path()), listing(&root_path)), before);
        assert_eq!(std::fs::read(temp.path().join("x")).unwrap(), b"kept");
        assert_eq!(std::fs::read(root_path.join("name")).unwrap(), b"inside");
        let stream = std::fs::metadata(root_path.join("name:stream")).unwrap_err();
        assert_eq!(stream.kind(), std::io::ErrorKind::NotFound);

        // A target Windows reads as a parent, a drive or a path from the
        // top is never made, and counts as a link that can't be.
        for target in [r"..\x", r"C:\Windows", "C:x", "/x", "a:b", r"a\..\..\x"] {
            let err = root.symlink(target, "l").unwrap_err();
            assert!(cannot_link(&err), "{target:?}: {err}");
            assert_eq!(root.kind("l").unwrap(), None, "{target:?}");
        }
        assert_eq!((listing(temp.path()), listing(&root_path)), before);
    }

    #[test]
    fn a_junction_is_never_followed() {
        let temp = TempDir::new();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("kept"), "kept").unwrap();
        let root_path = temp.path().join("root");
        let root = Dir::create(&root_path).unwrap();
        root.make_dir("a", 0o755).unwrap();
        testing::junction(&outside, &root_path.join("j"));
        testing::junction(&outside, &root_path.join("a").join("j"));

        assert_eq!(root.kind("j").unwrap(), Some(FileType::Symlink));
        assert!(root.dir(&parts("j")).is_err());
        assert!(root.dir(&parts("a/j")).is_err());
        assert!(root.dir(&parts("j/kept")).is_err());
        assert!(root.open_file("j", true).is_err());
        assert!(root.read_file("j").is_err());
        assert!(root.read_regular("j").is_err());
        assert!(root.make_dir("j", 0o755).is_err());
        assert!(root.dir(&parts("a")).unwrap().open_file("j", true).is_err());
        let err = Dir::open(&root_path.join("j")).unwrap_err();
        assert_eq!(Failure::io(&err, None).code, ErrorCode::BadPath, "{err}");
        let target = root.read_link("j").unwrap();
        assert!(target.to_string_lossy().starts_with('/'), "{target:?}");

        root.remove_tree("j").unwrap();
        assert_eq!(root.kind("j").unwrap(), None);
        let beside = temp.path().join("j2");
        testing::junction(&outside, &beside);
        crate::remove_tree(&beside).unwrap();
        assert!(std::fs::symlink_metadata(&beside).is_err());

        drop(root);
        crate::remove_tree(&root_path).unwrap();
        assert!(std::fs::symlink_metadata(&root_path).is_err());
        assert_eq!(listing(&outside), ["kept"]);
    }

    #[test]
    fn a_link_to_a_folder_is_a_folder_link() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        root.make_dir("d", 0o755).unwrap();
        match root.symlink("d", "ld") {
            // No Developer Mode and no right to make links: nothing to test.
            Err(err) if cannot_link(&err) => return,
            other => other.unwrap(),
        }
        let kind = |name: &str| {
            std::fs::symlink_metadata(temp.path().join(name))
                .unwrap()
                .file_type()
        };
        assert!(kind("ld").is_symlink_dir());
        assert_eq!(root.read_link("ld").unwrap(), "d");
        assert!(temp.path().join("ld").is_dir());

        root.make_dir("s", 0o755).unwrap();
        let s = root.dir(&parts("s")).unwrap();
        s.symlink("../d", "up").unwrap();
        assert!(kind("s/up").is_symlink_dir());
        assert_eq!(s.read_link("up").unwrap(), "../d");

        std::fs::write(temp.path().join("f"), "f").unwrap();
        root.symlink("f", "lf").unwrap();
        assert!(kind("lf").is_symlink_file());
        assert_eq!(root.kind("lf").unwrap(), Some(FileType::Symlink));

        assert!(root.dir(&parts("ld")).is_err());
        root.symlink("d", "ld").unwrap();
        assert!(root.symlink("f", "ld").is_err());
        let names = root.names().unwrap();
        assert!(
            names.iter().all(|name| !name.starts_with(".maxhq-link-")),
            "{names:?}"
        );
        assert!(cannot_link(&std::io::Error::from_raw_os_error(
            ERROR_PRIVILEGE_NOT_HELD as i32
        )));
    }

    #[test]
    fn a_rename_lands_over_an_open_file() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        std::fs::write(temp.path().join("a"), "new").unwrap();
        std::fs::write(temp.path().join("b"), "old").unwrap();
        let held = std::fs::File::open(temp.path().join("b")).unwrap();
        root.rename("a", "b").unwrap();
        assert_eq!(std::fs::read(temp.path().join("b")).unwrap(), b"new");
        assert_eq!(root.kind("a").unwrap(), None);
        drop(held);
    }

    #[test]
    fn a_folder_lists_and_removes_by_handle() {
        let temp = TempDir::new();
        let root = Dir::create(temp.path()).unwrap();
        root.make_dir("sub", 0o755).unwrap();
        let file = root.open_file("file", true).unwrap();
        drop(file);
        let mut names = root.names().unwrap();
        names.sort();
        assert_eq!(names, ["file", "sub"]);
        assert_eq!(root.kind("sub").unwrap(), Some(FileType::Directory));
        assert_eq!(root.kind("file").unwrap(), Some(FileType::RegularFile));
        assert!(root.remove_file("sub").is_err());
        root.remove_file("file").unwrap();
        root.remove_tree("sub").unwrap();
        assert!(root.names().unwrap().is_empty());
    }
}
