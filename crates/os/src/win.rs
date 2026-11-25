// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the Windows halves share: handles to processes and their tokens,
//! users' SIDs, and wide strings.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{LocalFree, FILETIME, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, TokenElevation, TokenUser, TOKEN_ELEVATION,
    TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// `s` as UTF-16 with the NUL Windows wants at the end.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// The raw handle a call takes.
pub fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle() as HANDLE
}

/// Takes charge of a handle a call returned, or the error it left.
#[allow(unsafe_code)]
pub fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the caller passes a handle a call just opened, which
    // nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Turns a call's `BOOL` into its error.
pub fn check(ok: windows_sys::core::BOOL) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Opens the process `pid` with `access`, or says why it can't.
#[allow(unsafe_code)]
pub fn open_process(pid: u32, access: u32) -> io::Result<OwnedHandle> {
    // SAFETY: OpenProcess takes no pointers.
    owned(unsafe { OpenProcess(access, 0, pid) })
}

/// The token of the process `process` names, for reading.
#[allow(unsafe_code)]
pub fn token_of(process: HANDLE) -> io::Result<OwnedHandle> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the call writes one handle to `token`.
    check(unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) })?;
    owned(token)
}

/// This process's token.
#[allow(unsafe_code)]
pub fn own_token() -> io::Result<OwnedHandle> {
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no
    // closing.
    token_of(unsafe { GetCurrentProcess() })
}

/// The token of process `pid`.
pub fn token_of_pid(pid: u32) -> io::Result<OwnedHandle> {
    let process = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    token_of(raw(&process))
}

/// What `class` says of `token`, in a buffer aligned for any struct the
/// call writes.
#[allow(unsafe_code)]
fn token_info(token: &OwnedHandle, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
    let mut size = 0u32;
    // SAFETY: asked with no buffer, the call only writes the size it
    // needs to `size`.
    unsafe { GetTokenInformation(raw(token), class, std::ptr::null_mut(), 0, &mut size) };
    let words = usize::try_from(size).map_err(io::Error::other)?.div_ceil(8);
    let mut buffer = vec![0u64; words.max(1)];
    let capacity = u32::try_from(buffer.len() * 8).map_err(io::Error::other)?;
    // SAFETY: the buffer is `capacity` bytes, and the call writes at most
    // that many.
    check(unsafe {
        GetTokenInformation(
            raw(token),
            class,
            buffer.as_mut_ptr().cast(),
            capacity,
            &mut size,
        )
    })?;
    Ok(buffer)
}

/// The SID of the user `token` runs as, as its bytes. Two SIDs are the
/// same user when their bytes are equal.
#[allow(unsafe_code)]
pub fn user_of(token: &OwnedHandle) -> io::Result<Vec<u8>> {
    let buffer = token_info(token, TokenUser)?;
    // SAFETY: TokenUser writes a TOKEN_USER at the start of the buffer,
    // which is aligned for it, and its SID points into the buffer.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    // SAFETY: `sid` is a valid SID the call wrote.
    let len = usize::try_from(unsafe { GetLengthSid(sid) }).map_err(io::Error::other)?;
    // SAFETY: the SID is `len` bytes, all inside the buffer, which lives
    // until the copy is made.
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), len) }.to_vec())
}

/// Whether `token` is elevated.
#[allow(unsafe_code)]
pub fn elevated(token: &OwnedHandle) -> io::Result<bool> {
    let buffer = token_info(token, TokenElevation)?;
    // SAFETY: TokenElevation writes a TOKEN_ELEVATION at the start of the
    // buffer, which is aligned for it.
    Ok(unsafe { (*buffer.as_ptr().cast::<TOKEN_ELEVATION>()).TokenIsElevated } != 0)
}

/// The user this process runs as.
pub fn own_user() -> io::Result<Vec<u8>> {
    user_of(&own_token()?)
}

/// A SID in its `S-1-5-…` form.
#[allow(unsafe_code)]
pub fn sid_string(sid: &[u8]) -> io::Result<String> {
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` holds a SID `user_of` copied, and the call writes one
    // pointer to a string it allocated with LocalAlloc.
    check(unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut text) })?;
    // SAFETY: the call allocated `text` with LocalAlloc.
    Ok(unsafe { take_local_string(text) })
}

/// Copies a NUL-terminated string a call allocated with LocalAlloc, and
/// frees it.
///
/// # Safety
///
/// `text` is such a string, and nothing uses it afterwards.
#[allow(unsafe_code)]
pub unsafe fn take_local_string(text: *mut u16) -> String {
    // SAFETY: the string ends in a NUL, so every unit read is in it.
    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    // SAFETY: the string is `len` units long.
    let string = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: `text` came from LocalAlloc and is freed once.
    unsafe { LocalFree(text.cast()) };
    string
}

/// When the process `process` names started, in 100 ns units since 1601:
/// two processes with one ID differ in it.
#[allow(unsafe_code)]
pub fn started(process: HANDLE) -> io::Result<u64> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the call writes one FILETIME to each.
    check(unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) })?;
    Ok(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}
