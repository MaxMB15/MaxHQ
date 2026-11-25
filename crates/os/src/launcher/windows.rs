// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::os::windows::process::CommandExt;

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, CREATE_NO_WINDOW};

use crate::win;

/// The job is unnamed, and this process holds the one handle to it until
/// it exits. Windows then closes it, which ends everything left in the
/// job. A process started with `CREATE_BREAKAWAY_FROM_JOB` leaves it.
#[allow(unsafe_code)]
pub fn tie_children() -> io::Result<()> {
    // SAFETY: no attributes and no name make an unnamed job with the
    // default security.
    let job = win::owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
    // SAFETY: the struct is plain data, for which zero is valid.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
        .map_err(io::Error::other)?;
    // SAFETY: `limits` is `size` bytes, the struct the class names.
    win::check(unsafe {
        SetInformationJobObject(
            win::raw(&job),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size,
        )
    })?;
    // SAFETY: the job is open for the call, and GetCurrentProcess returns
    // a pseudo handle that needs no closing.
    win::check(unsafe { AssignProcessToJobObject(win::raw(&job), GetCurrentProcess()) })?;
    // Kept open until this process exits, on purpose.
    std::mem::forget(job);
    Ok(())
}

/// A console of its own that has no window, which what it starts shares,
/// so no window shows for any of them.
pub fn without_window(command: &mut std::process::Command) {
    command.creation_flags(CREATE_NO_WINDOW);
}
