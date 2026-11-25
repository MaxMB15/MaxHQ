// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! This device's keys on Windows: an administrator's may be under
//! `%ProgramData%`, and a line names `maxd.exe` by its full path.

use std::fs;
use std::path::{Path, PathBuf};

use super::{
    in_administrators, windows_admin_file, windows_admin_keys_apply, windows_command, windows_line,
};
use crate::paths::MaxdLayout;

/// The `authorized_keys` this device's sshd reads for this account:
/// [`windows_admin_file`] for an Administrators member when `sshd_config`
/// sends their keys there, as it does from install, or else
/// `.ssh\authorized_keys` in `home`.
pub fn file_here(home: &Path) -> PathBuf {
    admin_file_here().unwrap_or_else(|| super::in_home(home))
}

/// [`windows_admin_file`] when sshd reads this account's keys from it.
fn admin_file_here() -> Option<PathBuf> {
    let program_data = PathBuf::from(std::env::var_os("ProgramData")?);
    let config = fs::read_to_string(program_data.join("ssh").join("sshd_config")).ok();
    (windows_admin_keys_apply(config.as_deref()) && admin())
        .then(|| windows_admin_file(&program_data))
}

/// Whether this account is in Administrators, from `whoami /groups`.
fn admin() -> bool {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    std::process::Command::new(root.join("System32").join("whoami.exe"))
        .args(["/groups", "/fo", "csv", "/nh"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|out| {
            out.status.success() && in_administrators(&String::from_utf8_lossy(&out.stdout))
        })
}

/// The stable `maxd.exe` on this machine, the one its service runs, or an
/// error naming what it lacks.
fn maxd() -> Result<String, String> {
    let layout = MaxdLayout::from_env()
        .ok_or("LOCALAPPDATA isn't set, so the key line can't name maxd.exe")?;
    Ok(layout.program().to_string_lossy().into_owned())
}

/// The line [`windows_line`] gives for this machine's `maxd.exe`,
/// [`WINDOWS_MAXD`](crate::paths::WINDOWS_MAXD) under `%LOCALAPPDATA%`.
/// `home` isn't read.
pub fn line_here(_home: &str, public_key: &str, peer: Option<&str>) -> Result<String, String> {
    windows_line(&maxd()?, public_key, peer)
}

/// `"<maxd.exe>" proxy` with `flags` for a line added by hand.
pub(super) fn forced_command(flags: &str) -> Result<String, String> {
    windows_command(&maxd()?, &Some(flags.to_owned()))
}

#[cfg(test)]
mod tests {
    use crate::authorized_keys::phone_line;
    use crate::authorized_keys::tests::KEY;

    #[test]
    fn a_phone_line_names_maxd_exe_under_local_app_data() {
        let line = phone_line(KEY, "p1").expect("line");
        assert!(line.contains(r"\MaxHQ\bin\current\maxd.exe"), "{line}");
        assert!(line.contains(" proxy --phone p1\" "), "{line}");
        assert!(line.ends_with(" maxhq-phone-p1"), "{line}");
    }
}
