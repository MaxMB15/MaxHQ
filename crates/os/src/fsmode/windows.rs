// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Modes are no-ops: the profile already keeps these files to the user. A
//! file holding a secret gets a protected DACL that names only the user,
//! so it stays theirs outside the profile too.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr;
use std::time::SystemTime;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::Authorization::{SetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    AddAccessAllowedAce, GetLengthSid, GetTokenInformation, InitializeAcl, TokenUser,
    ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
    let user = User::current()?;
    let sid = user.sid();
    // SAFETY: `sid` points into `user`, which lives to the end.
    let sid_size = unsafe { GetLengthSid(sid) } as usize;
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
            file.as_raw_handle() as HANDLE,
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

/// The user this process runs as, as its token names them.
struct User {
    /// The TOKEN_USER and the SID after it, in u64s so the pointer in it
    /// is aligned.
    buffer: Vec<u64>,
}

impl User {
    #[allow(unsafe_code)]
    fn current() -> io::Result<User> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo handle that needs no
        // closing, and the call writes one handle to `token`.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `token` is open and nothing else owns it.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut needed = 0u32;
        // SAFETY: a null buffer of size 0 only asks for the size.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle() as HANDLE,
                TokenUser,
                ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if (needed as usize) < size_of::<TOKEN_USER>() {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
        // SAFETY: `buffer` holds at least `needed` bytes.
        let ok = unsafe {
            GetTokenInformation(
                token.as_raw_handle() as HANDLE,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(User { buffer })
    }

    /// The user's SID, valid while `self` is.
    #[allow(unsafe_code)]
    fn sid(&self) -> PSID {
        // SAFETY: GetTokenInformation wrote a TOKEN_USER at the start.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
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
    let user = User::current()?;
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle was opened with READ_CONTROL; the call writes
    // `dacl`, which points into `descriptor`, freed below.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle() as HANDLE,
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
            && EqualSid((&raw mut (*ace).SidStart).cast(), user.sid()) != 0
    };
    // SAFETY: GetSecurityInfo allocated `descriptor` with LocalAlloc.
    unsafe { LocalFree(descriptor) };
    Ok(private)
}

pub fn owner_of(_meta: &Metadata) -> Option<u32> {
    None
}

pub fn same_file(_a: &Metadata, _b: &Metadata) -> bool {
    false
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
