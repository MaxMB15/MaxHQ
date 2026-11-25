// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[allow(unsafe_code)]
pub fn free_space(dir: &Path) -> io::Result<u64> {
    let path = CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the path holds a NUL"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a NUL-terminated string and `stat` is a buffer of
    // the right type, which `statvfs` fills when it returns 0.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        stat.assume_init()
    };
    #[allow(
        clippy::useless_conversion,
        reason = "the field types differ by platform"
    )]
    let (avail, block) = (u64::from(stat.f_bavail), u64::from(stat.f_frsize));
    Ok(avail.saturating_mul(block.max(1)))
}
