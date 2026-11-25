// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Which shell a terminal runs. On Unix it's the user's login shell, and
//! the Windows setting means nothing. On Windows the device's setting picks
//! Git Bash, PowerShell 7 or Windows PowerShell 5.1. Auto takes the first
//! of those that's installed, and a choice that isn't installed falls back
//! to Auto's order, so a terminal always starts. Git Bash is the one
//! [`maxhq_os::git_bash`] finds.

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
}
