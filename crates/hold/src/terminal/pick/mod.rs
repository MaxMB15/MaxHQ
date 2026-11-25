// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Which shell a terminal runs. On Unix it's the user's login shell, and
//! the Windows setting means nothing. On Windows the device's setting picks
//! Git Bash, PowerShell 7 or Windows PowerShell 5.1. Auto takes the first
//! of those that's installed, and a choice that isn't installed falls back
//! to Auto's order, so a terminal always starts.
//!
//! Git Bash is found where Git for Windows says it put itself, in the
//! registry, then in its usual folders, then beside the `git` on `PATH`,
//! always as `bin\bash.exe` under the install. It's never looked for as
//! `bash` on `PATH`, where the first is often System32's, WSL's launcher.

#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

use maxhq_protocol::model::WindowsShell;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// The user's shell. On Unix it's `$SHELL` when that's an absolute path,
/// or zsh on macOS and bash elsewhere. On Windows it's the one Auto picks.
pub fn login_shell() -> PathBuf {
    imp::login_shell()
}

/// The shell a terminal runs for `choice`, and whether `choice` isn't
/// installed, so it fell back to another. On Unix it's [`login_shell`],
/// whatever `choice` says.
pub(super) fn pick_shell(choice: WindowsShell) -> (PathBuf, bool) {
    imp::pick_shell(choice)
}

/// The Windows shells a machine has.
#[cfg(any(windows, test))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Found {
    git_bash: Option<PathBuf>,
    pwsh: Option<PathBuf>,
    powershell: Option<PathBuf>,
}

/// What starts Windows PowerShell when none of the shells is found where
/// it's looked for. Every Windows has it on `PATH`.
#[cfg(any(windows, test))]
const LAST_RESORT: &str = "powershell.exe";

/// The program for `choice` among what's `found`, and whether it fell
/// back: Auto takes Git Bash, then PowerShell 7, then Windows PowerShell,
/// and so does a choice that isn't there.
#[cfg(any(windows, test))]
fn pick(choice: WindowsShell, found: Found) -> (PathBuf, bool) {
    let Found {
        git_bash,
        pwsh,
        powershell,
    } = found;
    let chosen = match choice {
        WindowsShell::Auto => None,
        WindowsShell::GitBash => git_bash.clone(),
        WindowsShell::Pwsh => pwsh.clone(),
        WindowsShell::PowerShell => powershell.clone(),
    };
    if let Some(program) = chosen {
        return (program, false);
    }
    let program = (git_bash.or(pwsh).or(powershell)).unwrap_or_else(|| LAST_RESORT.into());
    (program, !choice.is_auto())
}

/// Where to look for Git Bash, in order: under each folder the registry
/// gives, under the usual install folders, then beside `git`.
#[cfg(any(windows, test))]
fn git_bash_places(
    registry: impl IntoIterator<Item = PathBuf>,
    program_files: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    git: Option<&Path>,
) -> Vec<PathBuf> {
    let installs = (program_files.map(|dir| dir.join("Git")))
        .into_iter()
        .chain(local_app_data.map(|dir| dir.join("Programs").join("Git")));
    (registry.into_iter())
        .chain(installs)
        .chain(git.and_then(git_root))
        .map(|root| root.join("bin").join("bash.exe"))
        .collect()
}

/// The Git for Windows folder `git` is in: its `cmd`, `bin` or
/// `mingw64\bin` folder.
#[cfg(any(windows, test))]
fn git_root(git: &Path) -> Option<PathBuf> {
    let dir = git.parent()?;
    let named = |dir: &Path, name: &str| {
        dir.file_name()
            .and_then(|it| it.to_str())
            .is_some_and(|it| it.eq_ignore_ascii_case(name))
    };
    let parent = dir.parent()?;
    if named(dir, "bin") && named(parent, "mingw64") {
        return parent.parent().map(Path::to_owned);
    }
    (named(dir, "cmd") || named(dir, "bin")).then(|| parent.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(git_bash: bool, pwsh: bool, powershell: bool) -> Found {
        Found {
            git_bash: git_bash.then(|| "git/bin/bash.exe".into()),
            pwsh: pwsh.then(|| "ps7/pwsh.exe".into()),
            powershell: powershell.then(|| "ps5/powershell.exe".into()),
        }
    }

    #[test]
    fn auto_takes_git_bash_then_pwsh_then_windows_powershell() {
        let program = |found| pick(WindowsShell::Auto, found);
        assert_eq!(
            program(found(true, true, true)),
            ("git/bin/bash.exe".into(), false)
        );
        assert_eq!(
            program(found(false, true, true)),
            ("ps7/pwsh.exe".into(), false)
        );
        assert_eq!(
            program(found(false, false, true)),
            ("ps5/powershell.exe".into(), false)
        );
        assert_eq!(program(Found::default()), (LAST_RESORT.into(), false));
    }

    #[test]
    fn a_choice_that_isnt_installed_falls_back_to_autos_order() {
        let all = found(true, true, true);
        assert_eq!(
            pick(WindowsShell::PowerShell, all.clone()),
            ("ps5/powershell.exe".into(), false)
        );
        assert_eq!(
            pick(WindowsShell::Pwsh, all),
            ("ps7/pwsh.exe".into(), false)
        );
        assert_eq!(
            pick(WindowsShell::GitBash, found(false, true, true)),
            ("ps7/pwsh.exe".into(), true)
        );
        assert_eq!(
            pick(WindowsShell::Pwsh, found(true, false, true)),
            ("git/bin/bash.exe".into(), true)
        );
        assert_eq!(
            pick(WindowsShell::PowerShell, Found::default()),
            (LAST_RESORT.into(), true)
        );
    }

    #[test]
    fn git_bash_is_looked_for_in_order_under_each_install() {
        let places = git_bash_places(
            ["/reg/Git".into(), "/reg/user/Git".into()],
            Some("/Program Files".into()),
            Some("/Users/<user>/AppData/Local".into()),
            Some(Path::new("/tools/Git/cmd/git.exe")),
        );
        let bash = |root: &str| Path::new(root).join("bin").join("bash.exe");
        assert_eq!(
            places,
            [
                bash("/reg/Git"),
                bash("/reg/user/Git"),
                bash("/Program Files/Git"),
                bash("/Users/<user>/AppData/Local/Programs/Git"),
                bash("/tools/Git"),
            ]
        );
        // A `git` outside an install adds nothing.
        let places = git_bash_places([], None, None, Some(Path::new("/Windows/System32/git.exe")));
        assert!(places.is_empty());
    }

    #[test]
    fn git_on_path_gives_its_install_folder() {
        for git in [
            "/Git/cmd/git.exe",
            "/Git/bin/git.exe",
            "/Git/mingw64/bin/git.exe",
            "/Git/MINGW64/BIN/git.exe",
        ] {
            assert_eq!(git_root(Path::new(git)), Some("/Git".into()), "{git}");
        }
        assert_eq!(git_root(Path::new("/usr/local/git.exe")), None);
        assert_eq!(git_root(Path::new("git.exe")), None);
    }
}
