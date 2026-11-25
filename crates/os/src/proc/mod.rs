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

#[cfg(any(windows, test))]
mod args;
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
///
/// On Windows a soft stop is a Ctrl-Break to the group, which only a
/// group started through [`GroupExt::own_group`] on this process's
/// console gets, and only while its leader runs. Otherwise the call
/// fails, so the caller can stop the group hard.
pub fn stop_group(leader: u32, how: Stop) -> io::Result<()> {
    imp::stop_group(leader, how)
}

/// Stops the process `pid` alone. Windows can't ask one process to end,
/// so a soft stop fails there with [`io::ErrorKind::Unsupported`].
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

/// When the process `pid` started, as a number that only compares: a
/// process that later gets the same ID has another. `None` when there's
/// no such process, or the system doesn't say.
pub fn started(pid: u32) -> Option<u64> {
    imp::started(pid)
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

/// Raises this process's soft limit on open files as far as the system
/// lets it. launchd and systemd start `maxd` low, and every agent, link and
/// client counts against it. Windows has no such limit on handles.
pub fn raise_open_files() -> io::Result<()> {
    imp::raise_open_files()
}

/// What this process's stderr writes to, as a file of its own, such as the
/// log a service manager opened for it.
pub fn stderr_file() -> io::Result<std::fs::File> {
    imp::stderr_file()
}

/// Points this process's stdout and stderr at `file`, so what it writes
/// from now on goes there. On Windows, whose launcher hands `maxd` its log,
/// it's unsupported.
pub fn send_output_to(file: &std::fs::File) -> io::Result<()> {
    imp::send_output_to(file)
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

    #[test]
    fn a_start_time_tells_processes_apart() {
        let _forks = crate::forks();
        let me = started(std::process::id());
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            assert!(me.is_some());
        }
        assert_eq!(started(std::process::id()), me);
        let mut child = if cfg!(windows) {
            let mut command = std::process::Command::new("ping");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        } else {
            let mut command = std::process::Command::new("sleep");
            command.arg("30");
            command
        }
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
        let theirs = started(child.id());
        assert_eq!(theirs.is_some(), me.is_some());
        if me.is_some() {
            assert!(theirs >= me);
        }
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        // Windows keeps an ended process while a handle to it is open.
        drop(child);
        assert_eq!(started(pid), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_job_stops_the_whole_tree() {
        let _forks = crate::forks();
        crate::block_on(async {
            // cmd starts a ping of its own, which lands in the job too.
            let mut child = tokio::process::Command::new("cmd")
                .args([
                    "/c",
                    "ping -n 30 127.0.0.1 > nul & ping -n 30 127.0.0.1 > nul",
                ])
                .own_group()
                .spawn()
                .unwrap();
            let pid = child.id().unwrap();
            adopt(pid).unwrap();
            let line = cmdline(pid).await.unwrap().unwrap();
            assert!(
                line.ends_with("ping -n 30 127.0.0.1 > nul & ping -n 30 127.0.0.1 > nul"),
                "{line}"
            );
            stop_group(pid, Stop::Hard).unwrap();
            let status = child.wait().await.unwrap();
            assert_eq!(status.code(), Some(137));
            assert!(stop_group(pid, Stop::Hard).is_err());
        });
    }

    #[cfg(windows)]
    #[test]
    fn a_group_found_by_name_stops_too() {
        let _forks = crate::forks();
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .own_group()
            .spawn()
            .unwrap();
        let pid = child.id();
        adopt(pid).unwrap();
        // As a maxd that started later finds it: by its name, not the
        // handle this process kept. This one still holds its own.
        let (job, runs) = super::imp::job_by_name(pid).unwrap();
        assert!(runs);
        drop(job);
        stop_group(pid, Stop::Hard).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(137));
    }

    #[cfg(windows)]
    #[test]
    fn one_process_stops_hard_but_not_soft() {
        let _forks = crate::forks();
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert_eq!(
            stop(pid, Stop::Soft).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(alive(pid));
        stop(pid, Stop::Hard).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(137));
        assert_eq!(
            stop(u32::MAX - 3, Stop::Hard).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_command_line_comes_back_as_its_arguments() {
        let _forks = crate::forks();
        let dir = std::env::temp_dir().join("maxhq os args");
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30"])
            .arg(&dir)
            .arg("127.0.0.1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let line = crate::block_on(cmdline(child.id())).unwrap().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            line.ends_with(&format!("-n 30 {} 127.0.0.1", dir.display())),
            "{line}"
        );
        assert_eq!(crate::block_on(cmdline(u32::MAX - 3)).unwrap(), None);
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
