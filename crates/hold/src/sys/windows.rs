// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsHandle, IntoRawHandle};
use std::process::Command;

use windows_sys::Win32::Foundation::{
    CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Console::{
    GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, CREATE_NO_WINDOW};

/// The file that reads as empty and takes any write.
pub(crate) const NULL: &str = "NUL";

fn std_handle(which: STD_HANDLE) -> Option<HANDLE> {
    // SAFETY: the call takes no pointers.
    #[allow(unsafe_code)]
    let handle = unsafe { GetStdHandle(which) };
    (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(handle)
}

/// Keeps the spawner's pipes out of the CLI. Windows hands a child every
/// handle marked for inheriting, not only its stdio, so the holder's own
/// stdio is unmarked before it starts anything. `spawn_detached` already
/// left the spawner's console and job behind. The holder outlives whatever
/// folder it was started in, and Windows won't delete a folder a process
/// is in, so it moves to the system's.
#[allow(unsafe_code)]
pub(crate) fn leave_session() -> Result<(), String> {
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        if let Some(handle) = std_handle(which) {
            // SAFETY: the handle is the process's own, open for the call.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
        }
    }
    if let Some(root) = std::env::var_os("SystemRoot") {
        let _ = std::env::set_current_dir(root);
    }
    Ok(())
}

/// Runs the second step with no console, in a group of its own and out of
/// the spawner's job when the job allows it.
pub(crate) fn spawn_second(command: &mut Command) -> io::Result<()> {
    maxhq_os::proc::spawn_detached(command)
}

/// Whether the holder is still in a job, which happens when its spawner's
/// job doesn't let it break away. Ending that job then ends the holder,
/// so it won't outlive a `maxd` whose service manager ends the job.
#[allow(unsafe_code)]
pub(crate) fn in_spawner_job() -> bool {
    let mut inside = 0;
    // SAFETY: the current process's pseudo handle needs no closing, a
    // null job asks about any job, and the call writes one BOOL.
    let asked = unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut inside) };
    asked != 0 && inside != 0
}

/// Keeps a console window from showing for the holder's first step when
/// `maxd` has no console of its own.
pub(crate) fn hide_window(command: &mut tokio::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}

/// A copy of the holder's stdin, to read from.
pub(crate) fn stdin_file() -> io::Result<File> {
    io::stdin().as_handle().try_clone_to_owned().map(File::from)
}

/// Makes a copy of `file` the std handle `which`, and closes the one it
/// had, as `dup2` does.
#[allow(unsafe_code)]
fn replace(which: STD_HANDLE, file: &File, closed: &mut Vec<HANDLE>) {
    let Ok(copy) = file.try_clone() else {
        return;
    };
    let old = std_handle(which);
    let copy = copy.into_raw_handle();
    // SAFETY: `copy` is open, and from here on the process owns it as its
    // std handle.
    if unsafe { SetStdHandle(which, copy) } == 0 {
        // SAFETY: the copy was never handed over, so it's closed here.
        unsafe { CloseHandle(copy) };
        return;
    }
    if let Some(old) = old
        && !closed.contains(&old)
    {
        closed.push(old);
        // SAFETY: the old handle is no longer the std handle, and only it
        // held the spawner's end. One handle shared by two std handles is
        // closed once.
        unsafe { CloseHandle(old) };
    }
}

/// Points stdin at `file`.
pub(crate) fn set_stdin(file: &File) {
    replace(STD_INPUT_HANDLE, file, &mut Vec::new());
}

/// Points stdout and stderr at `file`.
pub(crate) fn set_output(file: &File) {
    let mut closed = Vec::new();
    replace(STD_OUTPUT_HANDLE, file, &mut closed);
    replace(STD_ERROR_HANDLE, file, &mut closed);
}

/// What tells the holder to go. A holder has no console, so no Ctrl-C or
/// close event reaches it, and `maxd` ends it by ending its process or
/// asks it over its pipe instead.
pub(crate) struct Quit;

impl Quit {
    pub(crate) fn watch() -> io::Result<Self> {
        Ok(Self)
    }

    /// Waits for what never comes.
    pub(crate) async fn next(&mut self) {
        std::future::pending::<()>().await;
    }
}

/// An OS string's bytes: UTF-8 when it's valid UTF-16, and otherwise
/// WTF-8, which writes an unpaired surrogate as UTF-8 would write its code
/// point, so a name Windows allows is kept whole.
pub(crate) fn os_bytes(text: &OsStr) -> Cow<'_, [u8]> {
    if let Some(text) = text.to_str() {
        return text.as_bytes().into();
    }
    let mut bytes = Vec::new();
    for unit in char::decode_utf16(text.encode_wide()) {
        match unit {
            Ok(c) => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            Err(lone) => {
                let unit = lone.unpaired_surrogate();
                bytes.extend_from_slice(&[
                    0xE0 | (unit >> 12) as u8,
                    0x80 | ((unit >> 6) & 0x3F) as u8,
                    0x80 | (unit & 0x3F) as u8,
                ]);
            }
        }
    }
    bytes.into()
}

/// The OS string [`os_bytes`] gave `bytes` for, or `None` for bytes it
/// can't have given.
pub(crate) fn os_from_bytes(bytes: Vec<u8>) -> Option<OsString> {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Some(text.into()),
        Err(err) => err.into_bytes(),
    };
    let mut wide = Vec::with_capacity(bytes.len());
    let mut rest = &bytes[..];
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                wide.extend(text.encode_utf16());
                break;
            }
            Err(err) => {
                let (good, bad) = rest.split_at(err.valid_up_to());
                wide.extend(std::str::from_utf8(good).ok()?.encode_utf16());
                // Only a surrogate written as three bytes is allowed past
                // the valid part.
                let [0xED, second @ 0xA0..=0xBF, third @ 0x80..=0xBF, ..] = *bad else {
                    return None;
                };
                wide.push(0xD000 | (u16::from(second & 0x3F) << 6) | u16::from(third & 0x3F));
                rest = &bad[3..];
            }
        }
    }
    Some(OsString::from_wide(&wide))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_surrogate_survives_the_bytes_and_bad_bytes_are_refused() {
        let name = OsString::from_wide(&[u16::from(b'a'), 0xD800, u16::from(b'b')]);
        let bytes = os_bytes(&name).into_owned();
        assert_eq!(bytes, [b'a', 0xED, 0xA0, 0x80, b'b']);
        assert_eq!(os_from_bytes(bytes), Some(name));
        assert_eq!(os_from_bytes("plain".into()), Some("plain".into()));
        assert_eq!(os_from_bytes(vec![b'a', 0xFF]), None);
    }
}
