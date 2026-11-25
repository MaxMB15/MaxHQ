// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

#[allow(unsafe_code)]
pub fn free_space(dir: &Path) -> io::Result<u64> {
    let path = crate::win::wide(dir)?;
    let mut free = 0u64;
    // SAFETY: `path` is NUL-terminated and lives for the call, which only
    // writes `free`; the totals it may also fill are left out.
    crate::win::check(unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    })?;
    Ok(free)
}
