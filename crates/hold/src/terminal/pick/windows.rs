// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use maxhq_os::which;
use maxhq_os::win::wide;
use maxhq_protocol::model::WindowsShell;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};

use super::{git_bash_places, pick, Found};

/// The key Git for Windows writes its install folder under, machine-wide
/// or for one user.
const GIT_KEY: &str = r"SOFTWARE\GitForWindows";

/// The value there that holds the folder.
const GIT_VALUE: &str = "InstallPath";

/// The shell Auto picks.
pub(super) fn login_shell() -> PathBuf {
    pick_shell(WindowsShell::Auto).0
}

/// The shell for `choice` among those installed.
pub(super) fn pick_shell(choice: WindowsShell) -> (PathBuf, bool) {
    let found = Found {
        git_bash: git_bash(),
        pwsh: pwsh(),
        powershell: windows_powershell(),
    };
    pick(choice, found)
}

/// A folder the environment names, when it's absolute.
fn env_dir(name: &str) -> Option<PathBuf> {
    (std::env::var_os(name).map(PathBuf::from)).filter(|dir| dir.is_absolute())
}

/// The first `bin\bash.exe` there is under a Git for Windows install.
fn git_bash() -> Option<PathBuf> {
    let registry = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
        .into_iter()
        .filter_map(|root| registry_string(root, GIT_KEY, GIT_VALUE))
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute());
    let git = which::find("git", &[]);
    git_bash_places(
        registry,
        env_dir("ProgramFiles"),
        env_dir("LOCALAPPDATA"),
        git.as_deref(),
    )
    .into_iter()
    .find(|bash| bash.is_file())
}

/// PowerShell 7, from `PATH` or its usual folder.
fn pwsh() -> Option<PathBuf> {
    let usual: Vec<PathBuf> = (env_dir("ProgramFiles").map(|dir| dir.join("PowerShell").join("7")))
        .into_iter()
        .collect();
    which::find("pwsh", &usual)
}

/// Windows PowerShell 5.1, from where Windows keeps it or from `PATH`.
fn windows_powershell() -> Option<PathBuf> {
    (env_dir("SystemRoot").map(|root| root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")))
        .filter(|path| path.is_file())
        .or_else(|| which::find("powershell", &[]))
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
