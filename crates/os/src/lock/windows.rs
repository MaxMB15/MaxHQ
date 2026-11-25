// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::fs::{File, TryLockError};

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_LOCK_VIOLATION};
use windows_sys::Win32::Storage::FileSystem::{
    LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
};
use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};

use crate::win;

/// The one byte the lock covers: 1 TiB in, past anything written here, and
/// inside the whole-file range [`File::try_lock`] takes, so the two still
/// bar each other.
const AT: u64 = 1 << 40;

#[allow(unsafe_code)]
pub fn try_hold(file: &File) -> Result<(), TryLockError> {
    let mut overlapped = OVERLAPPED {
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 {
                Offset: AT as u32,
                OffsetHigh: (AT >> 32) as u32,
            },
        },
        ..OVERLAPPED::default()
    };
    // SAFETY: the handle stays open for the call, which `file` outlives,
    // and `overlapped` is a valid OVERLAPPED that outlives it too. Failing
    // at once, the call never leaves I/O pending on it.
    let locked = unsafe {
        LockFileEx(
            win::raw(file),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    match win::check(locked) {
        Ok(()) => Ok(()),
        Err(err)
            if [ERROR_LOCK_VIOLATION, ERROR_IO_PENDING]
                .iter()
                .any(|code| err.raw_os_error() == Some(*code as i32)) =>
        {
            Err(TryLockError::WouldBlock)
        }
        Err(err) => Err(TryLockError::Error(err)),
    }
}
