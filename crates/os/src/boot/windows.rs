// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};

use crate::win::wide;

/// Where Windows counts its boots, one more each time it starts.
const KEY: &str =
    r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters";
const VALUE: &str = "BootId";

#[allow(unsafe_code)]
pub fn id() -> Option<String> {
    let (key, value) = (wide(KEY), wide(VALUE));
    let mut count: u32 = 0;
    let mut size = u32::try_from(size_of::<u32>()).ok()?;
    // SAFETY: both names end in a 0, and `count` is a DWORD whose size
    // `size` gives, which is all a REG_DWORD value writes.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut count).cast(),
            &raw mut size,
        )
    };
    (status == ERROR_SUCCESS).then(|| count.to_string())
}
