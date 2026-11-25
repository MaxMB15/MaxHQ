// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, HANDLE, WAIT_TIMEOUT};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

use super::Stop;

// Job Objects, which stop a whole tree, are stream B's to add here. Until
// then a group is only a console process group, which nothing signals.

pub fn own_group(command: &mut std::process::Command) -> &mut std::process::Command {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP)
}

pub fn own_group_tokio(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP)
}

pub fn adopt(_leader: u32) -> io::Result<()> {
    Err(crate::unsupported("Taking charge of a process group"))
}

pub fn stop_group(_leader: u32, _how: Stop) -> io::Result<()> {
    Err(crate::unsupported("Stopping a process group"))
}

pub fn stop(_pid: u32, _how: Stop) -> io::Result<()> {
    Err(crate::unsupported("Stopping a process"))
}

pub fn spawn_detached(command: &mut std::process::Command) -> io::Result<()> {
    // No console of its own or this one's, and a group of its own, so a
    // Ctrl-C to this console doesn't reach it.
    command
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(drop)
}

/// Opens the process `pid` with `access`, or says why it can't.
#[allow(unsafe_code)]
fn open(pid: u32, access: u32) -> io::Result<OwnedHandle> {
    // SAFETY: OpenProcess takes no pointers. A handle it returns is this
    // process's to close, which `OwnedHandle` does.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` is open and nothing else owns it.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

#[allow(unsafe_code)]
pub fn alive(pid: u32) -> bool {
    match open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE) {
        // A process that ended stays openable while a handle to it is
        // open, and is signalled.
        // SAFETY: the handle is open for the call, which only reads it.
        Ok(process) => unsafe {
            WaitForSingleObject(process.as_raw_handle() as HANDLE, 0) == WAIT_TIMEOUT
        },
        // A process of another user's or an elevated one is there too.
        Err(err) => err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32),
    }
}

/// The longest path Windows gives a program, in UTF-16 units.
const PATH_MAX: usize = 32 * 1024;

#[allow(unsafe_code)]
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut path = vec![0u16; PATH_MAX];
    let mut len = u32::try_from(path.len()).ok()?;
    // SAFETY: the buffer holds `len` units, and the call writes at most
    // that many and sets `len` to how many it wrote.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle() as HANDLE,
            PROCESS_NAME_WIN32,
            path.as_mut_ptr(),
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    path.truncate(usize::try_from(len).ok()?);
    Some(PathBuf::from(OsString::from_wide(&path)))
}

pub async fn cmdline(_pid: u32) -> io::Result<Option<String>> {
    Err(crate::unsupported("Reading another process's command line"))
}

pub fn is_root() -> bool {
    elevated().unwrap_or(false)
}

/// Whether this process's token is elevated.
#[allow(unsafe_code)]
fn elevated() -> io::Result<bool> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no
    // closing, and the call writes one handle to `token`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `token` is open and nothing else owns it.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut written = 0u32;
    let size = u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>())
        .map_err(|_| io::Error::other("TOKEN_ELEVATION is too large"))?;
    // SAFETY: `elevation` is `size` bytes, the size TokenElevation writes.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenElevation,
            (&raw mut elevation).cast(),
            size,
            &mut written,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(elevation.TokenIsElevated != 0)
}
