// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};

use maxhq_os::proc::{self, Stop};
use tokio::sync::oneshot;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenThread, ResumeThread, WaitForSingleObject, CREATE_NEW_PROCESS_GROUP,
    CREATE_NO_WINDOW, CREATE_SUSPENDED, INFINITE, THREAD_SUSPEND_RESUME,
};

use super::{Sig, Status};

/// A terminal's foreground process group, which Windows doesn't have: a
/// pseudoconsole doesn't say which process has the user's attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {}

/// The creation flags of a program the holder starts: suspended until
/// it's in its job, a group of its own so a Ctrl-Break to another group
/// misses it, and a hidden console, since the holder has none to share.
pub(crate) const FLAGS: u32 = CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;

/// Starts the program suspended, so [`Tree::adopt`] puts it in its job
/// before it can start anything.
pub(crate) fn prepare(command: &mut Command) {
    command.creation_flags(FLAGS);
}

/// The program, by its ID and a handle that keeps the ID from going to
/// another process.
pub(crate) struct Tree {
    pid: u32,
    process: OwnedHandle,
}

fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle() as HANDLE
}

impl Tree {
    /// Takes charge of a program [`prepare`] started, once its pipes are
    /// taken: puts it in a job of its own, then lets its first thread run.
    pub(crate) fn adopt(child: Child) -> io::Result<Self> {
        let pid = child.id();
        let tree = Self {
            pid,
            process: OwnedHandle::from(child),
        };
        tree.join_job();
        if let Err(err) = resume_threads(pid) {
            tree.signal(Sig::Kill);
            return Err(err);
        }
        Ok(tree)
    }

    /// Takes charge of a program started suspended through a raw
    /// `CreateProcessW`, which also gives its first thread.
    #[allow(unsafe_code)]
    pub(crate) fn resume(pid: u32, process: OwnedHandle, thread: &OwnedHandle) -> io::Result<Self> {
        let tree = Self { pid, process };
        tree.join_job();
        // SAFETY: the handle is open for the call, with the access
        // `CreateProcessW` gives.
        if unsafe { ResumeThread(raw(thread)) } == u32::MAX {
            let err = io::Error::last_os_error();
            tree.signal(Sig::Kill);
            return Err(err);
        }
        Ok(tree)
    }

    /// Puts the program in a job of its own. Without one, ending the tree
    /// ends the program alone, which is said in `holder.log`.
    fn join_job(&self) {
        if let Err(err) = proc::adopt(self.pid) {
            eprintln!(
                "couldn't put process {} in a job of its own, so ending it won't end what it starts: {err}",
                self.pid
            );
        }
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

    /// Kills the job, or the program alone when it has none. Returns false
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

/// Lets every thread of the suspended process `pid` run. A process that
/// starts suspended has one, and std doesn't hand its handle out yet.
#[allow(unsafe_code)]
fn resume_threads(pid: u32) -> io::Result<()> {
    // SAFETY: the call takes no pointers.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the snapshot is a handle the call just opened, owned here.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
    // SAFETY: the struct is plain data, for which zero is valid.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = u32::try_from(size_of::<THREADENTRY32>()).map_err(io::Error::other)?;
    let mut resumed = 0;
    // SAFETY: `entry` is the size its `dwSize` says.
    let mut more = unsafe { Thread32First(raw(&snapshot), &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the call takes no pointers.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !thread.is_null() {
                // SAFETY: the handle was just opened and is owned here.
                let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
                // SAFETY: the handle is open with the access the call needs.
                if unsafe { ResumeThread(raw(&thread)) } != u32::MAX {
                    resumed += 1;
                }
            }
        }
        // SAFETY: as for `Thread32First`.
        more = unsafe { Thread32Next(raw(&snapshot), &mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other(format!(
            "couldn't find process {pid}'s thread to start it"
        )));
    }
    Ok(())
}
