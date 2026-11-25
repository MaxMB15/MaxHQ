// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::os::windows::io::OwnedHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};

use maxhq_os::proc::{self, Stop};
use maxhq_os::win::raw;
use tokio::sync::oneshot;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, WaitForSingleObject, CREATE_NO_WINDOW, INFINITE,
};

use super::{Sig, Status};

/// A terminal's foreground process group, which Windows doesn't have: a
/// pseudoconsole doesn't say which process has the user's attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {}

/// The creation flags of a program the holder starts: a group of its own,
/// suspended until it's in its job, and a hidden console, since the
/// holder has none to share.
pub(crate) const FLAGS: u32 = proc::GROUP_FLAGS | CREATE_NO_WINDOW;

/// Starts the program suspended, so [`Tree::adopt`] puts it in its job
/// before it can start anything.
pub(crate) fn prepare(command: &mut Command) {
    command.creation_flags(FLAGS);
}

/// The program, by its ID and a handle that keeps the ID from going to
/// another process, and the job it runs in.
pub(crate) struct Tree {
    pid: u32,
    process: OwnedHandle,
    _group: proc::Group,
}

impl Tree {
    /// Takes charge of a program [`prepare`] started, once its pipes are
    /// taken.
    pub(crate) fn adopt(child: Child) -> io::Result<Self> {
        Self::start(child.id(), OwnedHandle::from(child))
    }

    /// Takes charge of a program started suspended, here or through a raw
    /// `CreateProcessW`: puts it in a job of its own, then lets it run. On
    /// an error it was stopped before it ran anything.
    pub(crate) fn start(pid: u32, process: OwnedHandle) -> io::Result<Self> {
        let group = proc::adopt(pid)?;
        Ok(Self {
            pid,
            process,
            _group: group,
        })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether `group` is the program's own, which no group is.
    pub(crate) fn leads(&self, group: Group) -> bool {
        match group {}
    }

    /// How the program ended, once it has. Windows keeps nothing to reap,
    /// and the handle keeps the ID the program's until the tree goes.
    #[allow(unsafe_code)]
    pub(crate) fn reap(&self) -> Option<Status> {
        // SAFETY: the handle is open for the call, which only reads it.
        if unsafe { WaitForSingleObject(raw(&self.process), 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        // SAFETY: the call writes one u32.
        if unsafe { GetExitCodeProcess(raw(&self.process), &mut code) } == 0 {
            return None;
        }
        Some(Status {
            // An NTSTATUS such as 0xC000013A reads as a negative code, as
            // the process's own `exit` would give it.
            code: Some(code.cast_signed()),
            signal: None,
        })
    }

    /// Kills the job, or the program alone if that fails. Returns false
    /// for a hangup or a SIGTERM, which Windows can't send to a program
    /// with no console the holder shares, so the caller goes on without
    /// waiting for it.
    pub(crate) fn signal(&self, sig: Sig) -> bool {
        match sig {
            Sig::Hangup | Sig::Term => false,
            Sig::Kill => {
                if proc::stop_group(self.pid, Stop::Hard).is_err() {
                    let _ = proc::stop(self.pid, Stop::Hard);
                }
                true
            }
        }
    }

    /// What wakes the reaper: a thread that waits for the program to end.
    #[allow(unsafe_code)]
    pub(crate) fn reaper(&self) -> Reaper {
        let Ok(process) = self.process.try_clone() else {
            return Reaper(None);
        };
        let (ended, wait) = oneshot::channel();
        crate::holder::spawn_thread("reaper", move || {
            // SAFETY: the handle is this thread's own copy, open for the
            // call.
            unsafe { WaitForSingleObject(raw(&process), INFINITE) };
            let _ = ended.send(());
        });
        Reaper(Some(wait))
    }
}

/// Signals another group, which Windows has none of.
pub(crate) fn signal_group(group: Group, _sig: Sig) {
    match group {}
}

/// Wakes the reaper once the program has ended.
pub(crate) struct Reaper(Option<oneshot::Receiver<()>>);

impl Reaper {
    /// Waits for the program to end, then forever. The reaper also looks
    /// on a tick.
    pub(crate) async fn next(&mut self) {
        match self.0.as_mut() {
            Some(wait) => {
                let _ = wait.await;
                self.0 = None;
            }
            None => std::future::pending().await,
        }
    }
}
