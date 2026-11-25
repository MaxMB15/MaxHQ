// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

use rustix::process::{Pid, Signal, WaitOptions};
use tokio::signal::unix::{signal, SignalKind};

use super::{Sig, Status};

/// A process group, as a terminal's foreground one.
pub(crate) type Group = Pid;

/// Starts the program as the leader of a process group of its own.
pub(crate) fn prepare(command: &mut Command) {
    command.process_group(0);
}

/// The program, by the ID of the group it leads.
pub(crate) struct Tree {
    pid: Pid,
}

fn signal_of(sig: Sig) -> Signal {
    match sig {
        Sig::Hangup => Signal::HUP,
        Sig::Term => Signal::TERM,
        Sig::Kill => Signal::KILL,
    }
}

impl Tree {
    /// Takes charge of a program a command that went through [`prepare`],
    /// or a pty, started, once its pipes are taken.
    pub(crate) fn adopt(child: Child) -> io::Result<Self> {
        let pid = i32::try_from(child.id())
            .ok()
            .and_then(Pid::from_raw)
            .ok_or_else(|| io::Error::other("the program has no process ID"))?;
        // The holder reaps the program itself, so it can signal its group
        // only while the ID still belongs to it.
        drop(child);
        Ok(Self { pid })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid.as_raw_nonzero().get().unsigned_abs()
    }

    /// Whether `group` is the program's own.
    pub(crate) fn leads(&self, group: Group) -> bool {
        group == self.pid
    }

    /// Reaps every child that has exited, and says how the program ended
    /// when it was one of them. The caller holds its state lock, so the
    /// program's ID is never signalled once it could be reused.
    pub(crate) fn reap(&self) -> Option<Status> {
        let mut ended = None;
        loop {
            match rustix::process::wait(WaitOptions::NOHANG) {
                Ok(Some((pid, status))) => {
                    if pid == self.pid {
                        ended = Some(Status {
                            code: status.exit_status(),
                            signal: status.terminating_signal(),
                        });
                    }
                }
                Ok(None) => break,
                Err(rustix::io::Errno::INTR) => {}
                // No children left.
                Err(_) => break,
            }
        }
        ended
    }

    /// Signals the program's group, or the program alone if it has left
    /// the group. Returns whether the signal could be sent, which on Unix
    /// it always can.
    pub(crate) fn signal(&self, sig: Sig) -> bool {
        let signal = signal_of(sig);
        if rustix::process::kill_process_group(self.pid, signal).is_err() {
            let _ = rustix::process::kill_process(self.pid, signal);
        }
        true
    }

    /// What wakes the reaper: SIGCHLD.
    pub(crate) fn reaper(&self) -> Reaper {
        Reaper(signal(SignalKind::child()).ok())
    }
}

/// Signals another group, such as a terminal's foreground one.
pub(crate) fn signal_group(group: Group, sig: Sig) {
    let _ = rustix::process::kill_process_group(group, signal_of(sig));
}

/// Wakes the reaper when a child may have exited.
pub(crate) struct Reaper(Option<tokio::signal::unix::Signal>);

impl Reaper {
    /// Waits for the next SIGCHLD, or forever when there's no watching
    /// for it. The reaper also looks on a tick.
    pub(crate) async fn next(&mut self) {
        match self.0.as_mut() {
            Some(sigchld) => {
                sigchld.recv().await;
            }
            None => std::future::pending().await,
        }
    }
}
