// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Modes are no-ops: the profile already keeps these files to the user. A
//! file holding a secret gets a protected DACL that names only the user,
//! so it stays theirs outside the profile too.

use std::fs::{self, File, Metadata, OpenOptions};
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::ptr;
use std::time::SystemTime;

use windows_sys::Win32::Security::Authorization::{SetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    AddAccessAllowedAce, InitializeAcl, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION,
    DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES, READ_CONTROL, WRITE_DAC,
};

use super::FileId;
use crate::win;

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

pub fn private_dir_all(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).create(path)
}

pub fn dir_all_with_mode(path: &Path, _mode: u32) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).create(path)
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

pub fn set_open_mode(options: &mut OpenOptions, _mode: u32) -> &mut OpenOptions {
    options
}

pub fn set_mode(path: &Path, _mode: u32) -> io::Result<()> {
    // Still fails for a path that isn't there, as chmod would.
    fs::metadata(path).map(drop)
}

pub fn set_file_mode(_file: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

pub fn mode_of(meta: &Metadata) -> u32 {
    if meta.is_dir() {
        0o700
    } else if meta.permissions().readonly() {
        0o400
    } else {
        0o600
    }
}

/// Opens `path` to read or rewrite its DACL. Backup semantics let it open
/// a folder too.
fn open_for_dacl(path: &Path, access: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[allow(unsafe_code)]
pub fn private_file(path: &Path) -> io::Result<()> {
    let file = open_for_dacl(path, READ_CONTROL | WRITE_DAC)?;
    let sid = own_sid()?;
    let sid_size = win::own_user()?.len();
    // One ACL header and one entry, whose SID starts at its last u32.
    let size = (size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_size)
        .next_multiple_of(size_of::<u32>());
    let length = u32::try_from(size).map_err(|_| io::Error::other("the SID is too large"))?;
    // u32s, so the ACL sits on the 4-byte boundary Windows asks for.
    let mut buffer = vec![0u32; size / size_of::<u32>()];
    let acl = buffer.as_mut_ptr().cast::<ACL>();
    // SAFETY: `acl` is `length` bytes, aligned, and the ACL's alone.
    if unsafe { InitializeAcl(acl, length, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the ACL was sized above for exactly this entry.
    if unsafe { AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle was opened with WRITE_DAC, and a null owner,
    // group and SACL leave those alone. Protecting the DACL drops what the
    // folder grants.
    let status = unsafe {
        SetSecurityInfo(
            win::raw(&file),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}

/// The SID of the user this process runs as, for a call that takes one.
/// It lives as long as the process.
fn own_sid() -> io::Result<PSID> {
    Ok(win::own_user()?.as_ptr().cast_mut().cast())
}

/// Whether `path`'s DACL is protected and holds one entry, for this user.
#[cfg(test)]
#[allow(unsafe_code)]
pub fn is_private(path: &Path) -> io::Result<bool> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::GetSecurityInfo;
    use windows_sys::Win32::Security::{
        AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        ACL_SIZE_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };
    // ACCESS_ALLOWED_ACE_TYPE, from a feature the crate doesn't take.
    const ALLOWED: u8 = 0;

    let file = open_for_dacl(path, READ_CONTROL)?;
    let sid = own_sid()?;
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle was opened with READ_CONTROL; the call writes
    // `dacl`, which points into `descriptor`, freed below.
    let status = unsafe {
        GetSecurityInfo(
            win::raw(&file),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // SAFETY: `descriptor` and `dacl` stay valid until the LocalFree.
    let private = unsafe {
        let mut control = 0u16;
        let mut revision = 0u32;
        let mut sizes = ACL_SIZE_INFORMATION::default();
        let mut ace: *mut ACCESS_ALLOWED_ACE = ptr::null_mut();
        !dacl.is_null()
            && GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
            && control & SE_DACL_PROTECTED != 0
            && GetAclInformation(
                dacl,
                (&raw mut sizes).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            ) != 0
            && sizes.AceCount == 1
            && GetAce(dacl, 0, (&raw mut ace).cast()) != 0
            && (*ace).Header.AceType == ALLOWED
            && EqualSid((&raw mut (*ace).SidStart).cast(), sid) != 0
    };
    // SAFETY: GetSecurityInfo allocated `descriptor` with LocalAlloc.
    unsafe { LocalFree(descriptor) };
    Ok(private)
}

pub fn owner_of(_meta: &Metadata) -> Option<u32> {
    None
}

pub fn group_of(_meta: &Metadata) -> Option<u32> {
    None
}

pub fn owned_by_other(_meta: &Metadata) -> bool {
    false
}

pub fn writable_dir(path: &Path) -> bool {
    let name = format!(
        ".maxhq-{}-{:x}",
        std::process::id(),
        RandomState::new().hash_one(path)
    );
    let probe = path.join(name);
    let made = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if made {
        let _ = fs::remove_file(&probe);
    }
    made
}

pub fn file_id(file: &File) -> io::Result<FileId> {
    let (volume, file) = win::file_id(file)?;
    Ok(FileId { volume, file })
}

pub fn id_at(path: &Path) -> io::Result<FileId> {
    // No access at all is enough to ask what a file is, and keeps a writer
    // or a lock from getting in the way. Backup semantics opens a folder
    // too.
    let there = OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    file_id(&there)
}

pub fn has_other_names(file: &File) -> io::Result<bool> {
    Ok(win::file_info(file)?.nNumberOfLinks > 1)
}

pub fn open_no_wait(options: &mut OpenOptions) -> &mut OpenOptions {
    options
}

pub fn set_modified(path: &Path, at: SystemTime) -> io::Result<()> {
    // Backup semantics opens a folder too, and the time takes only the
    // right to write attributes.
    OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .set_modified(at)
}

pub fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}
