// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The bash a pack's installer runs in, and the user it runs as. Unix has
//! bash on `PATH`. Windows has it only with Git for Windows, as Git Bash.
//!
//! The Windows port's shared shell finder (stream B) replaces the search
//! here once it lands.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What to start to run a bash script: `bash`, found on `PATH` as it
/// starts, on Unix. On Windows it's Git Bash, from Git for Windows'
/// folders or else from `PATH`, and an error that says the installer
/// needs it when there's none.
pub fn git_bash() -> io::Result<PathBuf> {
    if !cfg!(windows) {
        return Ok(PathBuf::from("bash"));
    }
    let env = |name: &str| std::env::var_os(name);
    let found = git_folders(env)
        .into_iter()
        .map(|dir| dir.join("bash.exe"))
        .find(|file| maxhq_os::which::runnable(file))
        .or_else(|| {
            maxhq_os::which::find("bash", &[])
                .filter(|found| !windows_own(found, env("SystemRoot").as_deref()))
        });
    found.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the pack's installer needs Git Bash, which comes with Git for Windows \
             (git-scm.com). Install it, then try again.",
        )
    })
}

/// The folders Git for Windows keeps `bash.exe` in, for a machine-wide
/// install and a user's own, as `env` names them.
fn git_folders(env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let places: [(&str, &[&str]); 3] = [
        ("ProgramFiles", &["Git"]),
        ("ProgramFiles(x86)", &["Git"]),
        ("LOCALAPPDATA", &["Programs", "Git"]),
    ];
    places
        .into_iter()
        .filter_map(|(var, under)| {
            let base = PathBuf::from(env(var)?);
            // A relative folder would depend on where `maxd` started.
            base.is_absolute()
                .then(|| under.iter().fold(base, |dir, part| dir.join(part)))
        })
        .map(|git| git.join("bin"))
        .collect()
}

/// Whether `found` is in Windows' own folder, `system_root`, where
/// `bash.exe` starts WSL rather than Git Bash.
fn windows_own(found: &Path, system_root: Option<&std::ffi::OsStr>) -> bool {
    let Some(root) = system_root else {
        return false;
    };
    let lower = |path: &Path| path.to_string_lossy().to_lowercase();
    let root = lower(Path::new(root));
    let found = lower(found);
    found
        .strip_prefix(root.trim_end_matches('\\'))
        .is_some_and(|rest| rest.starts_with('\\'))
}

/// Makes `command` run as the user `uid` in the group `gid`. Only Unix
/// runs `maxd` as root for another user; Windows says it can't.
pub fn as_user(command: &mut Command, uid: u32, gid: u32) -> io::Result<&mut Command> {
    imp::as_user(command, uid, gid)
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub fn as_user(command: &mut Command, uid: u32, gid: u32) -> io::Result<&mut Command> {
        Ok(command.uid(uid).gid(gid))
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::process::Command;

    pub fn as_user(_command: &mut Command, _uid: u32, _gid: u32) -> io::Result<&mut Command> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows can't start the installer as another user",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_runs_the_bash_on_path() {
        assert_eq!(git_bash().unwrap(), PathBuf::from("bash"));
    }

    #[test]
    fn git_for_windows_is_looked_for_where_it_installs() {
        let base = std::env::temp_dir();
        let env = |name: &str| match name {
            "ProgramFiles" => Some(base.join("pf").into_os_string()),
            "LOCALAPPDATA" => Some(base.join("local").into_os_string()),
            "ProgramFiles(x86)" => Some("relative".into()),
            _ => None,
        };
        assert_eq!(
            git_folders(env),
            [
                base.join("pf").join("Git").join("bin"),
                base.join("local").join("Programs").join("Git").join("bin"),
            ]
        );
        assert!(git_folders(|_| None).is_empty());
    }

    #[test]
    fn the_bash_in_windows_own_folder_is_wsl_s() {
        let root = Some(std::ffi::OsStr::new(r"C:\Windows"));
        assert!(windows_own(
            Path::new(r"C:\Windows\System32\bash.exe"),
            root
        ));
        assert!(windows_own(
            Path::new(r"c:\WINDOWS\system32\bash.exe"),
            root
        ));
        assert!(!windows_own(Path::new(r"C:\WindowsApps\bash.exe"), root));
        assert!(!windows_own(
            Path::new(r"C:\Program Files\Git\bin\bash.exe"),
            root
        ));
        assert!(!windows_own(
            Path::new(r"C:\Windows\System32\bash.exe"),
            None
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unix_starts_it_as_another_user() {
        let mut command = Command::new("true");
        let mine = (
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        );
        assert!(as_user(&mut command, mine.0, mine.1).is_ok());
    }
}
