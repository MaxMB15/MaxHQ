// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Processes: a group of their own that one call stops, a program that
//! outlives `maxd`, and what a process ID runs.
//!
//! A group is a process group on Unix. On Windows it's a Job Object that
//! [`adopt`] puts the child in, since Windows has no process groups that
//! a signal reaches.

use std::io;
use std::path::PathBuf;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// How to stop a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Asks it to end, as `SIGTERM` does, so it can clean up.
    Soft,
    /// Ends it at once, as `SIGKILL` does.
    Hard,
}

/// Starts a command's child in a group of its own, so [`stop_group`]
/// reaches whatever it starts and a signal to this process's group
/// doesn't reach it. On Windows this sets the command's creation flags.
pub trait GroupExt {
    /// Makes the child the leader of a new group.
    fn own_group(&mut self) -> &mut Self;
}

impl GroupExt for std::process::Command {
    fn own_group(&mut self) -> &mut Self {
        imp::own_group(self)
    }
}

impl GroupExt for tokio::process::Command {
    fn own_group(&mut self) -> &mut Self {
        imp::own_group_tokio(self)
    }
}

/// Takes charge of the group `leader` leads, a child just started through
/// [`GroupExt::own_group`], so [`stop_group`] can stop it later. Unix made
/// the group as the child started, so this does nothing there.
pub fn adopt(leader: u32) -> io::Result<()> {
    imp::adopt(leader)
}

/// Stops every process in the group `leader` leads.
pub fn stop_group(leader: u32, how: Stop) -> io::Result<()> {
    imp::stop_group(leader, how)
}

/// Stops the process `pid` alone.
pub fn stop(pid: u32, how: Stop) -> io::Result<()> {
    imp::stop(pid, how)
}

/// Starts `command` in a group of its own and lets it run on after this
/// process exits. It's never waited for. The caller picks its stdio.
pub fn spawn_detached(command: &mut std::process::Command) -> io::Result<()> {
    imp::spawn_detached(command)
}

/// Whether a process `pid` is there, including one of another user's.
pub fn alive(pid: u32) -> bool {
    imp::alive(pid)
}

/// The program the process `pid` runs, as the system names it. Links
/// aren't resolved, and on Linux a program replaced while it runs ends
/// in ` (deleted)`.
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    imp::exe_of(pid)
}

/// Process `pid`'s arguments joined by spaces, or `None` when there's no
/// such process.
pub async fn cmdline(pid: u32) -> io::Result<Option<String>> {
    imp::cmdline(pid).await
}

/// Whether this process runs as root, or elevated on Windows: what a
/// file it makes may be out of the user's reach.
pub fn is_root() -> bool {
    imp::is_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive_and_runs_its_program() {
        let me = std::process::id();
        assert!(alive(me));
        let exe = exe_of(me).expect("this process's program");
        let expected = std::env::current_exe().unwrap();
        assert_eq!(
            exe.canonicalize().unwrap(),
            expected.canonicalize().unwrap()
        );
    }

    #[test]
    fn a_process_that_ended_isnt_alive() {
        let _forks = crate::forks();
        let mut child = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.args(["/c", "exit"]);
            command
        } else {
            std::process::Command::new("true")
        }
        .spawn()
        .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // Reaped, so the ID is free. Another process could take it in
        // the meantime, but not usually this soon.
        assert!(!alive(pid));
    }

    #[test]
    fn root_is_known() {
        // Whatever the answer, asking doesn't fail.
        let _ = is_root();
    }

    #[cfg(unix)]
    #[test]
    fn a_group_stops_together() {
        let _forks = crate::forks();
        crate::block_on(async {
            let mut child = tokio::process::Command::new("sleep")
                .arg("30")
                .own_group()
                .spawn()
                .unwrap();
            let pid = child.id().unwrap();
            adopt(pid).unwrap();
            assert_eq!(cmdline(pid).await.unwrap().as_deref(), Some("sleep 30"));
            stop_group(pid, Stop::Hard).unwrap();
            assert!(!child.wait().await.unwrap().success());
        });
    }

    #[cfg(windows)]
    #[test]
    fn groups_say_they_arent_built_yet() {
        let me = std::process::id();
        for err in [
            adopt(me).unwrap_err(),
            stop_group(me, Stop::Soft).unwrap_err(),
            stop(me, Stop::Hard).unwrap_err(),
        ] {
            assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_detached_program_starts() {
        let _forks = crate::forks();
        let mut command = std::process::Command::new("cmd");
        command
            .args(["/c", "exit"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        spawn_detached(&mut command).unwrap();
    }
}
