// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The bash a pack's installer runs in, and the user it runs as. Unix has
//! bash on `PATH`. Windows has it only with Git for Windows, as Git Bash.

use std::io;
use std::path::PathBuf;
use std::process::Command;

/// What to start to run a bash script: `bash`, found on `PATH` as it
/// starts, on Unix. On Windows it's Git Bash, as [`maxhq_os::git_bash`]
/// finds it, and an error that says the installer needs it when there's
/// none.
pub fn git_bash() -> io::Result<PathBuf> {
    if !cfg!(windows) {
        return Ok(PathBuf::from("bash"));
    }
    maxhq_os::git_bash::find().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "the pack's installer needs Git Bash, which comes with Git for Windows \
             (git-scm.com). Install it, then try again.",
        )
    })
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn unix_runs_the_bash_on_path() {
        assert_eq!(git_bash().unwrap(), PathBuf::from("bash"));
    }

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
