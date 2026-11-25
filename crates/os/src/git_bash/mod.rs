// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Git Bash, the bash that comes with Git for Windows, for a Windows
//! terminal and for the scripts only bash can run there.
//!
//! It's found where Git for Windows says it put itself, in the registry,
//! then in its usual folders, then beside the `git` on `PATH`, always as
//! `bin\bash.exe` under the install. It's never looked for as `bash` on
//! `PATH`, where the first is often System32's, WSL's launcher, or another
//! bash such as MSYS2's or Cygwin's.

#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Git Bash's `bash.exe`, when Git for Windows is installed. Only Windows
/// has it, so elsewhere there's none.
pub fn find() -> Option<PathBuf> {
    imp::find()
}

/// Where to look for Git Bash, in order: under each folder the registry
/// gives, under the usual install folders in each of `program_files` and
/// in `local_app_data`, then beside `git`.
#[cfg(any(windows, test))]
fn places(
    registry: impl IntoIterator<Item = PathBuf>,
    program_files: impl IntoIterator<Item = PathBuf>,
    local_app_data: Option<PathBuf>,
    git: Option<&Path>,
) -> Vec<PathBuf> {
    let installs = (program_files.into_iter().map(|dir| dir.join("Git")))
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

    #[test]
    fn git_bash_is_looked_for_in_order_under_each_install() {
        let places = places(
            ["/reg/Git".into(), "/reg/user/Git".into()],
            ["/Program Files".into(), "/Program Files (x86)".into()],
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
                bash("/Program Files (x86)/Git"),
                bash("/Users/<user>/AppData/Local/Programs/Git"),
                bash("/tools/Git"),
            ]
        );
        // A `git` outside an install adds nothing.
        let places = super::places([], [], None, Some(Path::new("/Windows/System32/git.exe")));
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

    #[cfg(unix)]
    #[test]
    fn only_windows_has_git_bash() {
        assert_eq!(find(), None);
    }
}
