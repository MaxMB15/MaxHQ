// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Stopping the CLI that runs a session in your own terminal. MaxHQ only
//! signals a process that runs as you and whose command names the CLI, and
//! checks before each signal that the pid still belongs to the process it
//! found, so a pid the system gave to another process is never signalled.
//! The CLI gets SIGTERM to quit on its own first, and SIGKILL only if it
//! still runs after the grace period. Only that process gets them, never
//! its group, so the terminal and its shell stay. Windows can't ask one
//! process to quit, so there the CLI is ended at once.

use std::ffi::OsStr;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use maxhq_os::proc::Stop;
#[cfg(unix)]
use maxhq_protocol::processes::ProcessSignal;
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

#[cfg(unix)]
use crate::ports::processes::send;

/// How long the CLI gets to quit after SIGTERM.
pub const GRACE: Duration = Duration::from_secs(5);

/// How often a stop looks whether the CLI quit.
const POLL: Duration = Duration::from_millis(100);

/// The CLI as MaxHQ found it, to stop later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// When it started, in seconds since the epoch, which tells it from a
    /// later process given the same pid.
    pub started: u64,
}

/// What the system says about a pid now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Who runs it, as the system names its user: a uid on Unix and a
    /// SID on Windows.
    pub uid: Option<String>,
    pub started: u64,
    pub name: String,
    pub command: Vec<String>,
    /// It ended, but its parent hasn't reaped it yet.
    pub zombie: bool,
}

pub const GONE: &str = "The CLI already ended in the terminal.";
const REUSED: &str = "The CLI already ended in the terminal, and another process has its pid now.";
const OTHER_USER: &str = "That CLI runs as another user, so MaxHQ won't stop it.";

/// Finds the process `pid` and checks it is `cli` running as you.
pub fn find(pid: u32, cli: &str) -> Result<Process, String> {
    check(look(pid).as_ref(), me().as_deref(), cli).map(|started| Process { pid, started })
}

/// The user `maxd` runs as.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn me() -> Option<String> {
    Some(rustix::process::getuid().as_raw().to_string())
}

/// The user `maxd` runs as. Windows names it by the SID in a process's
/// token, which the system reads for `maxd` as for any process.
#[cfg(windows)]
fn me() -> Option<String> {
    look(std::process::id())?.uid
}

/// Checks what the system says about a pid against the CLI MaxHQ looks for,
/// and gives its start time. A process whose user isn't known, or when
/// `maxd`'s own isn't, counts as another user's.
pub fn check(seen: Option<&Seen>, me: Option<&str>, cli: &str) -> Result<u64, String> {
    let seen = seen.filter(|seen| !seen.zombie).ok_or(GONE)?;
    if me.is_none() || seen.uid.as_deref() != me {
        return Err(OTHER_USER.into());
    }
    if !names(seen, cli) {
        return Err(REUSED.into());
    }
    Ok(seen.started)
}

/// Whether the process is `cli`: its name, or the program or script its
/// command runs, as with `node /path/to/claude`.
fn names(seen: &Seen, cli: &str) -> bool {
    let named = |text: &str| {
        Path::new(text)
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| {
                name == cli
                    || name
                        .strip_prefix(cli)
                        .is_some_and(|rest| matches!(rest, ".js" | ".mjs" | ".cjs"))
            })
    };
    !cli.is_empty() && (named(&seen.name) || seen.command.iter().take(2).any(|arg| named(arg)))
}

/// Stops the process: SIGTERM, up to `grace` for it to quit, then SIGKILL.
/// Done once it no longer runs, including when it had already quit.
pub fn stop(process: Process, grace: Duration) -> Result<(), String> {
    if !runs(process) {
        return Ok(());
    }
    signal(process, Stop::Soft)?;
    if quits_within(process, grace) {
        return Ok(());
    }
    signal(process, Stop::Hard)?;
    if quits_within(process, grace) {
        return Ok(());
    }
    Err("The CLI in the terminal didn't stop.".into())
}

/// Whether the process quits before `wait` runs out.
fn quits_within(process: Process, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if !runs(process) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// Sends the signal, after checking the pid still belongs to the process.
/// One that quit in between counts as sent.
fn signal(process: Process, how: Stop) -> Result<(), String> {
    if !runs(process) {
        return Ok(());
    }
    match send_stop(process.pid, how) {
        Ok(()) => Ok(()),
        Err(_) if !runs(process) => Ok(()),
        Err(err) => Err(err),
    }
}

/// SIGTERM for a soft stop and SIGKILL for a hard one, to `pid` alone.
#[cfg(unix)]
fn send_stop(pid: u32, how: Stop) -> Result<(), String> {
    let signal = match how {
        Stop::Soft => ProcessSignal::Term,
        Stop::Hard => ProcessSignal::Kill,
    };
    send(pid, signal)
}

/// Ends `pid` at once, whichever stop, since Windows has no SIGTERM to
/// ask one process with. The CLI then gets no grace.
#[cfg(windows)]
fn send_stop(pid: u32, _how: Stop) -> Result<(), String> {
    maxhq_os::proc::stop(pid, Stop::Hard).map_err(|err| format!("the CLI wasn't stopped: {err}"))
}

/// Whether the process still runs under its pid.
fn runs(process: Process) -> bool {
    look(process.pid).is_some_and(|seen| !seen.zombie && seen.started == process.started)
}

fn look(pid: u32) -> Option<Seen> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_user(UpdateKind::Always)
            .without_tasks(),
    );
    let process = system.process(pid)?;
    Some(Seen {
        uid: process.user_id().map(|uid| (**uid).to_string()),
        started: process.start_time(),
        name: process.name().to_string_lossy().into_owned(),
        command: process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        zombie: process.status() == ProcessStatus::Zombie,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::process::{Child, Command, Stdio};
    #[cfg(unix)]
    use std::sync::mpsc;

    use super::*;

    /// A long-running child this test starts itself, reaped as soon as it
    /// quits so it doesn't linger as a zombie. It runs `sleep` and `sh`,
    /// which only Unix is sure to have.
    #[cfg(unix)]
    struct Fake {
        pid: u32,
        quit: mpsc::Receiver<()>,
    }

    #[cfg(unix)]
    impl Fake {
        /// `sleep`, which quits on SIGTERM.
        fn obeying() -> Self {
            Self::start(Command::new("sleep").arg("120"))
        }

        /// A shell that ignores SIGTERM while its `sleep` runs, so only
        /// SIGKILL stops it.
        fn stubborn() -> Self {
            let fake = Self::start(
                Command::new("/bin/sh").args(["-c", "trap '' TERM; while :; do sleep 1; done"]),
            );
            // Long enough for the shell to set its trap.
            thread::sleep(Duration::from_millis(300));
            fake
        }

        fn start(command: &mut Command) -> Self {
            let mut child: Child = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let pid = child.id();
            let (tx, quit) = mpsc::channel();
            thread::spawn(move || {
                let _ = child.wait();
                let _ = tx.send(());
            });
            Self { pid, quit }
        }

        fn found(&self, cli: &str) -> Process {
            find(self.pid, cli).unwrap()
        }

        fn quit_within(&self, wait: Duration) -> bool {
            self.quit.recv_timeout(wait).is_ok()
        }
    }

    #[cfg(unix)]
    impl Drop for Fake {
        fn drop(&mut self) {
            if self.quit.try_recv().is_err() {
                let _ = send_stop(self.pid, Stop::Hard);
            }
        }
    }

    fn seen(uid: &str, name: &str, command: &[&str]) -> Seen {
        Seen {
            uid: Some(uid.into()),
            started: 1_700_000_000,
            name: name.into(),
            command: command.iter().map(|arg| (*arg).into()).collect(),
            zombie: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_stops_a_cli_that_quits_on_its_own() {
        let fake = Fake::obeying();
        let process = fake.found("sleep");
        let began = Instant::now();
        stop(process, Duration::from_secs(5)).unwrap();
        assert!(
            began.elapsed() < Duration::from_secs(4),
            "it didn't wait for the grace"
        );
        assert!(fake.quit_within(Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn sigkill_follows_once_the_grace_runs_out() {
        let fake = Fake::stubborn();
        let process = fake.found("sh");
        let began = Instant::now();
        stop(process, Duration::from_millis(600)).unwrap();
        assert!(
            began.elapsed() >= Duration::from_millis(600),
            "it waited first"
        );
        assert!(fake.quit_within(Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn a_reused_pid_is_never_signalled() {
        let fake = Fake::obeying();
        let found = fake.found("sleep");
        let other = Process {
            started: found.started - 60,
            ..found
        };
        stop(other, Duration::from_millis(300)).unwrap();
        assert!(
            !fake.quit_within(Duration::from_millis(500)),
            "it still runs"
        );
        assert!(runs(found));
    }

    #[test]
    fn only_a_cli_of_yours_is_found() {
        let me = Some("501");
        let claude = seen("501", "claude", &["claude", "--model", "opus"]);
        assert_eq!(check(Some(&claude), me, "claude"), Ok(1_700_000_000));
        let node = seen(
            "501",
            "node",
            &["node", "/opt/lib/node_modules/.bin/claude"],
        );
        assert!(check(Some(&node), me, "claude").is_ok());
        let script = seen(
            "501",
            "node",
            &["node", "/opt/codex/bin/codex.js", "resume"],
        );
        assert!(check(Some(&script), me, "codex").is_ok());
        assert_eq!(
            check(Some(&claude), Some("502"), "claude"),
            Err(OTHER_USER.into())
        );
        assert_eq!(check(Some(&claude), None, "claude"), Err(OTHER_USER.into()));
        let unknown = Seen {
            uid: None,
            ..claude.clone()
        };
        assert_eq!(check(Some(&unknown), me, "claude"), Err(OTHER_USER.into()));
        let other = seen("501", "vim", &["vim", "claude.md"]);
        assert_eq!(check(Some(&other), me, "claude"), Err(REUSED.into()));
        let lookalike = seen("501", "claude-helper", &["claude-helper"]);
        assert_eq!(check(Some(&lookalike), me, "claude"), Err(REUSED.into()));
        assert_eq!(check(None, me, "claude"), Err(GONE.into()));
        let zombie = Seen {
            zombie: true,
            ..claude
        };
        assert_eq!(check(Some(&zombie), me, "claude"), Err(GONE.into()));
        assert_eq!(
            check(Some(&seen("501", "x", &[])), me, ""),
            Err(REUSED.into())
        );
    }

    #[test]
    fn maxd_knows_which_user_it_runs_as() {
        let me = me().expect("the system says");
        let seen = look(std::process::id()).expect("maxd is there");
        assert_eq!(seen.uid.as_deref(), Some(me.as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn a_process_that_quit_counts_as_stopped() {
        let fake = Fake::obeying();
        let process = fake.found("sleep");
        send_stop(fake.pid, Stop::Hard).unwrap();
        assert!(fake.quit_within(Duration::from_secs(2)));
        assert_eq!(find(fake.pid, "sleep"), Err(GONE.into()));
        stop(process, Duration::from_millis(300)).unwrap();
    }
}
