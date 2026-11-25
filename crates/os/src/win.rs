// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the Windows halves share: handles to processes and their tokens,
//! users' SIDs, and wide strings. Other crates that call Windows
//! themselves use these too, so it's public, though not part of the API.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;

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

/// `s` as UTF-16 with the NUL Windows wants at the end, refused if it
/// holds a NUL already.
pub fn wide(s: impl AsRef<OsStr>) -> io::Result<Vec<u16>> {
    ended(s.as_ref().encode_wide().collect())
}

/// `units` with the NUL Windows wants at the end, refused if they hold one
/// already: Windows would read only up to it, and act on a shorter name
/// than the one asked for.
pub fn ended(mut units: Vec<u16>) -> io::Result<Vec<u16>> {
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a name, path or argument holds a NUL, which Windows reads as its end",
        ));
    }
    units.push(0);
    Ok(units)
}

/// The raw handle a call takes.
pub fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle() as HANDLE
}

/// Takes charge of a handle a call returned, or the error it left.
///
/// # Safety
///
/// `handle` is null, `INVALID_HANDLE_VALUE`, or a handle a call just
/// opened, which nothing else owns or closes.
#[allow(unsafe_code)]
pub unsafe fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
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
    // SAFETY: OpenProcess takes no pointers, and its handle is owned here.
    unsafe { owned(OpenProcess(access, 0, pid)) }
}

/// The token of the process `process` names, for reading.
#[allow(unsafe_code)]
pub fn token_of(process: impl AsHandle) -> io::Result<OwnedHandle> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the process handle is open for the call, which writes one
    // handle to `token`, owned here.
    unsafe {
        check(OpenProcessToken(
            raw(&process.as_handle()),
            TOKEN_QUERY,
            &mut token,
        ))?;
        owned(token)
    }
}

/// This process's token.
#[allow(unsafe_code)]
pub fn own_token() -> io::Result<OwnedHandle> {
    // SAFETY: GetCurrentProcess returns a pseudo handle that is always
    // open and needs no closing.
    token_of(unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) })
}

/// The token of process `pid`.
pub fn token_of_pid(pid: u32) -> io::Result<OwnedHandle> {
    token_of(open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION)?)
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

/// Whether `token` is elevated. The class wants a buffer of exactly its
/// struct's size, and refuses a bigger one with `ERROR_BAD_LENGTH`.
#[allow(unsafe_code)]
pub fn elevated(token: &OwnedHandle) -> io::Result<bool> {
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let capacity =
        u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>()).map_err(io::Error::other)?;
    let mut size = 0u32;
    // SAFETY: the buffer is one TOKEN_ELEVATION, `capacity` bytes, and the
    // call writes at most that.
    check(unsafe {
        GetTokenInformation(
            raw(token),
            TokenElevation,
            (&raw mut elevation).cast(),
            capacity,
            &mut size,
        )
    })?;
    Ok(elevation.TokenIsElevated != 0)
}

/// The user this process runs as, as its SID's bytes. A process keeps its
/// user for life, so it's read once.
pub fn own_user() -> io::Result<&'static [u8]> {
    static USER: OnceLock<Vec<u8>> = OnceLock::new();
    if let Some(user) = USER.get() {
        return Ok(user);
    }
    let user = user_of(&own_token()?)?;
    Ok(USER.get_or_init(|| user))
}

/// [`own_user`] in its `S-1-5-…` form, read once too.
pub fn own_sid() -> io::Result<&'static str> {
    static SID: OnceLock<String> = OnceLock::new();
    if let Some(sid) = SID.get() {
        return Ok(sid);
    }
    let sid = sid_string(own_user()?)?;
    Ok(SID.get_or_init(|| sid))
}

/// A SID in its `S-1-5-…` form.
#[allow(unsafe_code)]
pub fn sid_string(sid: &[u8]) -> io::Result<String> {
    // A SID is its revision, the count of its parts, a 6-byte authority and
    // a 4-byte word per part, and the call reads as many as it says.
    let whole = sid.len() >= 8 && sid.len() >= 8 + 4 * usize::from(sid[1]);
    if !whole {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a SID"));
    }
    let mut text: *mut u16 = std::ptr::null_mut();
    // SAFETY: `sid` holds a whole SID, as checked, and the call writes one
    // pointer to a string it allocated with LocalAlloc.
    check(unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut text) })?;
    // SAFETY: the call allocated `text` with LocalAlloc.
    Ok(unsafe { take_local_string(text) })
}

/// The SID a string names, as its bytes: the `S-1-5-…` form, or the two
/// letters SDDL writes for a well-known one, such as `LA` for the local
/// administrator.
#[cfg(test)]
#[allow(unsafe_code)]
pub fn sid_from_string(text: &str) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;

    let wide = wide(text)?;
    let mut sid: windows_sys::Win32::Security::PSID = std::ptr::null_mut();
    // SAFETY: `wide` ends in a NUL, and the call writes one pointer to a
    // SID it allocated with LocalAlloc.
    check(unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) })?;
    // SAFETY: `sid` is a valid SID the call wrote.
    let len = usize::try_from(unsafe { GetLengthSid(sid) }).map_err(io::Error::other);
    // SAFETY: the SID is `len` bytes, copied before it's freed once.
    let bytes =
        len.map(|len| unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), len) }.to_vec());
    // SAFETY: the call allocated `sid` with LocalAlloc.
    unsafe { LocalFree(sid) };
    bytes
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
pub fn started(process: impl AsHandle) -> io::Result<u64> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    let process = raw(&process.as_handle());
    // SAFETY: the handle is open for the call, which writes one FILETIME
    // to each.
    check(unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) })?;
    Ok(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
}
