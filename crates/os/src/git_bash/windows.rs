// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};

use crate::win::wide;

/// The key Git for Windows writes its install folder under, machine-wide
/// or for one user.
const GIT_KEY: &str = r"SOFTWARE\GitForWindows";

/// The value there that holds the folder.
const GIT_VALUE: &str = "InstallPath";

/// The first `bin\bash.exe` there is under a Git for Windows install.
pub fn find() -> Option<PathBuf> {
    let registry = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
        .into_iter()
        .filter_map(|root| registry_string(root, GIT_KEY, GIT_VALUE))
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    let program_files = ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(env_dir);
    let git = crate::which::find("git", &[]);
    super::places(
        registry,
        program_files,
        env_dir("LOCALAPPDATA"),
        git.as_deref(),
    )
    .into_iter()
    .find(|bash| bash.is_file())
}

/// A folder the environment names, when it's absolute. A relative one
/// would depend on where the process started.
fn env_dir(name: &str) -> Option<PathBuf> {
    (std::env::var_os(name).map(PathBuf::from)).filter(|dir| dir.is_absolute())
}

/// The string `value` under `key` in `root`, when it's there and plain
/// text.
#[allow(unsafe_code)]
fn registry_string(root: HKEY, key: &str, value: &str) -> Option<OsString> {
    let (key, value) = (wide(key).ok()?, wide(value).ok()?);
    let mut size = 0u32;
    // SAFETY: the names end in a zero, a null buffer asks for the size
    // alone, and the call writes one u32.
    let asked = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if asked != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut text = vec![0u16; usize::try_from(size).ok()?.div_ceil(2)];
    let mut bytes = u32::try_from(text.len() * 2).ok()?;
    // SAFETY: the buffer holds `bytes` bytes, which the call writes at most.
    // A value that grew since the first call fails rather than overruns.
    let read = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            text.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if read != ERROR_SUCCESS {
        return None;
    }
    let len = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    (len > 0).then(|| OsString::from_wide(&text[..len]))
}
