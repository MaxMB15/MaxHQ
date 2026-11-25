// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use windows_sys::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};

use super::trimmed;

/// Asks Windows itself rather than starting `hostname`, which prints the
/// same name.
#[allow(unsafe_code)]
pub fn name() -> Option<String> {
    let mut size: u32 = 0;
    // SAFETY: with no buffer and a size of 0 the call only writes the size
    // it needs, its closing 0 included, to `size`.
    unsafe { GetComputerNameExW(ComputerNameDnsHostname, std::ptr::null_mut(), &raw mut size) };
    let mut buffer = vec![0u16; usize::try_from(size).ok()?];
    // SAFETY: `buffer` holds the `size` UTF-16 units the call is told it
    // may write.
    let ok =
        unsafe { GetComputerNameExW(ComputerNameDnsHostname, buffer.as_mut_ptr(), &raw mut size) };
    if ok == 0 {
        return None;
    }
    // On success `size` is the length of the name without its closing 0.
    buffer.truncate(usize::try_from(size).ok()?);
    trimmed(&String::from_utf16_lossy(&buffer))
}
