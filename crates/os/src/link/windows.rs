// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Directory junctions, which any user can make where a symlink needs
//! admin rights or Developer Mode. A junction points at a folder, never
//! at a file, so the Windows update layout links a version's folder.
//!
//! An existing junction is re-pointed in place: one
//! `FSCTL_SET_REPARSE_POINT` replaces what it points at, so a reader
//! finds the old folder or the new one. Should the system refuse that,
//! the new junction is made beside it and renamed into place, which
//! leaves the name missing for a moment.

use std::fs;
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};
use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
use windows_sys::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::win;

pub fn point(link: &Path, target: &Path) -> io::Result<()> {
    let target = std::path::absolute(target)?;
    if !target.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a junction points at a folder, and {} isn't one",
                target.display()
            ),
        ));
    }
    let data = mount_point(&target)?;
    let temp = super::temp_for(link);
    // What a crash left behind: a junction or an empty folder.
    match fs::remove_dir(&temp) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    match fs::symlink_metadata(link) {
        Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            if set(link, &data).is_ok() {
                return Ok(());
            }
            make(&temp, &data)?;
            swap(link, &temp)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is there and isn't a junction", link.display()),
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            make(&temp, &data)?;
            fs::rename(&temp, link).inspect_err(|_| {
                let _ = fs::remove_dir(&temp);
            })
        }
        Err(err) => Err(err),
    }
}

pub fn resolve(link: &Path) -> io::Result<PathBuf> {
    // A junction names its target as `\??\C:\…`, which comes back as
    // `\\?\C:\…`. Without the prefix it's the folder [`point`] was given.
    fs::read_link(link).map(crate::path::plain)
}

/// Makes a junction at `path`, a name nothing has, with `data`.
fn make(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::create_dir(path)?;
    set(path, data).inspect_err(|_| {
        let _ = fs::remove_dir(path);
    })
}

/// Moves the junction at `link` aside, `temp` into its place, and drops
/// the old one, which leaves the folder it pointed at alone.
fn swap(link: &Path, temp: &Path) -> io::Result<()> {
    let mut old = link.as_os_str().to_owned();
    old.push(".maxhq-old");
    let old = PathBuf::from(old);
    let _ = fs::remove_dir(&old);
    fs::rename(link, &old)?;
    if let Err(err) = fs::rename(temp, link) {
        let _ = fs::rename(&old, link);
        let _ = fs::remove_dir(temp);
        return Err(err);
    }
    fs::remove_dir(&old)
}

/// Writes `data` as the reparse point of the folder at `path`.
#[allow(unsafe_code)]
fn set(path: &Path, data: &[u8]) -> io::Result<()> {
    let folder = fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let len = u32::try_from(data.len()).map_err(io::Error::other)?;
    let mut returned = 0u32;
    // SAFETY: the folder is open for the call, `data` is `len` bytes,
    // and the call writes nothing back but `returned`.
    win::check(unsafe {
        DeviceIoControl(
            win::raw(&folder),
            FSCTL_SET_REPARSE_POINT,
            data.as_ptr().cast(),
            len,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    })
}

/// A mount point's `REPARSE_DATA_BUFFER` for `target`, an absolute
/// folder: the tag, the data's length, then the substitute name the
/// system follows (`\??\` and the path) and the name a listing prints,
/// each ending in a NUL.
fn mount_point(target: &Path) -> io::Result<Vec<u8>> {
    let text = target.to_string_lossy();
    let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let substitute: Vec<u16> = format!(r"\??\{plain}").encode_utf16().collect();
    let print: Vec<u16> = plain.encode_utf16().collect();
    let too_long = || io::Error::new(io::ErrorKind::InvalidInput, "the path is too long");
    let u16_of = |n: usize| u16::try_from(n).map_err(|_| too_long());
    let substitute_len = u16_of(substitute.len() * 2)?;
    let print_offset = u16_of((substitute.len() + 1) * 2)?;
    let print_len = u16_of(print.len() * 2)?;
    let names = (substitute.len() + print.len() + 2) * 2;
    let data_len = u16_of(8 + names)?;

    let mut buffer = Vec::with_capacity(8 + usize::from(data_len));
    buffer.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
    buffer.extend_from_slice(&data_len.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend_from_slice(&substitute_len.to_le_bytes());
    buffer.extend_from_slice(&print_offset.to_le_bytes());
    buffer.extend_from_slice(&print_len.to_le_bytes());
    for unit in substitute.iter().chain(&[0]).chain(&print).chain(&[0]) {
        buffer.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(buffer)
}
