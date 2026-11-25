// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::PathBuf;

use maxhq_os::which;
use maxhq_protocol::model::WindowsShell;

use super::{pick, Found};

/// The shell Auto picks.
pub(super) fn login_shell() -> PathBuf {
    pick_shell(WindowsShell::Auto).0
}

/// The shell for `choice` among those installed.
pub(super) fn pick_shell(choice: WindowsShell) -> (PathBuf, bool) {
    let found = Found {
        git_bash: maxhq_os::git_bash::find(),
        pwsh: pwsh(),
        powershell: windows_powershell(),
    };
    pick(choice, found)
}

/// A folder the environment names, when it's absolute.
fn env_dir(name: &str) -> Option<PathBuf> {
    (std::env::var_os(name).map(PathBuf::from)).filter(|dir| dir.is_absolute())
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
