// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A group is a Job Object: [`adopt`] puts a child that
//! [`own_group`] started in a job of its own, and whatever it starts
//! later lands in the job too, so [`stop_group`] reaches the whole tree.
//!
//! The job is named after its leader's ID and start time, so a `maxd`
//! that started after the leader can still find it while the leader
//! runs. This process keeps its handle to each job it made, which finds
//! a group whose leader ended too.
//!
//! Windows has no `SIGTERM`. A soft stop sends Ctrl-Break to the group's
//! console process group, which only a group [`own_group`] started on
//! this process's console gets. Anything else is stopped hard.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::OwnedHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE, STATUS_INFO_LENGTH_MISMATCH,
    UNICODE_STRING, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, OpenJobObjectW,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK,
};
use windows_sys::Win32::System::SystemServices::{JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE};
use windows_sys::Win32::System::Threading::{
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
    CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

use super::Stop;
use crate::win;

/// The exit code a process stopped hard gets, as `SIGKILL` would give.
const KILLED: u32 = 137;

/// A job this process made, by its leader's ID.
struct Group {
    job: OwnedHandle,
    started: u64,
}

static GROUPS: Mutex<BTreeMap<u32, Group>> = Mutex::new(BTreeMap::new());

fn groups() -> std::sync::MutexGuard<'static, BTreeMap<u32, Group>> {
    GROUPS.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn own_group(command: &mut std::process::Command) -> &mut std::process::Command {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP)
}

pub fn own_group_tokio(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    command.creation_flags(CREATE_NEW_PROCESS_GROUP)
}

/// The name of the job the process that started at `started` leads.
fn job_name(leader: u32, started: u64) -> Vec<u16> {
    win::wide(format!(r"Local\maxhq-group-{leader}-{started}"))
}

/// An ID that names no process, as `ESRCH` says on Unix.
fn no_process(pid: u32) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("no process {pid}"))
}

/// Opens process `pid`, saying [`no_process`] when there's none.
fn open(pid: u32, access: u32) -> io::Result<OwnedHandle> {
    win::open_process(pid, access).map_err(|err| {
        if err.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            no_process(pid)
        } else {
            err
        }
    })
}

#[allow(unsafe_code)]
pub fn adopt(leader: u32) -> io::Result<()> {
    let process = open(
        leader,
        PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
    )?;
    let started = win::started(win::raw(&process))?;
    let name = job_name(leader, started);
    // SAFETY: `name` is NUL-terminated, and no attributes is the default
    // security, this user's.
    let job = win::owned(unsafe { CreateJobObjectW(std::ptr::null(), name.as_ptr()) })?;
    // What the group starts may leave it on purpose, as `setsid` lets a
    // process leave its group on Unix.
    // SAFETY: the struct is plain data, for which zero is valid.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
        .map_err(io::Error::other)?;
    // SAFETY: `limits` is `size` bytes, the struct the class names.
    win::check(unsafe {
        SetInformationJobObject(
            win::raw(&job),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size,
        )
    })?;
    // SAFETY: both handles are open for the call.
    win::check(unsafe { AssignProcessToJobObject(win::raw(&job), win::raw(&process)) })?;
    groups().insert(leader, Group { job, started });
    Ok(())
}

/// The job of the group `leader` leads, and whether its leader still
/// runs, or [`no_process`] when there's none to find.
fn job_of(leader: u32) -> io::Result<(OwnedHandle, bool)> {
    if let Some(group) = groups().get(&leader) {
        let job = group.job.try_clone()?;
        return Ok((job, started(leader) == Some(group.started)));
    }
    job_by_name(leader)
}

/// The job of the group `leader` leads, found by its name, which only a
/// running leader gives.
#[allow(unsafe_code)]
pub(super) fn job_by_name(leader: u32) -> io::Result<(OwnedHandle, bool)> {
    let started = started(leader).ok_or_else(|| no_process(leader))?;
    let name = job_name(leader, started);
    // SAFETY: `name` is NUL-terminated.
    let job = unsafe { OpenJobObjectW(JOB_OBJECT_TERMINATE | JOB_OBJECT_QUERY, 0, name.as_ptr()) };
    let job = win::owned(job).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("process {leader} leads no group MaxHQ made"),
        )
    })?;
    Ok((job, true))
}

#[allow(unsafe_code)]
pub fn stop_group(leader: u32, how: Stop) -> io::Result<()> {
    let (job, leader_runs) = job_of(leader)?;
    match how {
        Stop::Hard => {
            // SAFETY: the job is open for the call.
            win::check(unsafe { TerminateJobObject(win::raw(&job), KILLED) })?;
            groups().remove(&leader);
            Ok(())
        }
        // Once the leader is gone its ID may name another group, or none,
        // and a Ctrl-Break to an ID that leads no group reaches every
        // process on the console. So only a running leader is asked.
        Stop::Soft if leader_runs => {
            // SAFETY: the call takes no pointers.
            win::check(unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, leader) })
        }
        Stop::Soft => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("process {leader}'s group has no leader left to ask, so stop it hard"),
        )),
    }
}

#[allow(unsafe_code)]
pub fn stop(pid: u32, how: Stop) -> io::Result<()> {
    match how {
        Stop::Hard => {
            let process = open(pid, PROCESS_TERMINATE)?;
            // SAFETY: the handle is open for the call.
            win::check(unsafe { TerminateProcess(win::raw(&process), KILLED) })
        }
        Stop::Soft => Err(crate::unsupported(
            "Asking one process to end (Windows has no SIGTERM, so stop it hard)",
        )),
    }
}

pub fn spawn_detached(command: &mut std::process::Command) -> io::Result<()> {
    // No console of its own or this one's, and a group of its own, so a
    // Ctrl-C to this console doesn't reach it. Out of this process's job
    // too, if the job allows it, so ending the job doesn't end it.
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    match command
        .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(err) if err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
            command.creation_flags(flags).spawn().map(drop)
        }
        Err(err) => Err(err),
    }
}

pub fn alive(pid: u32) -> bool {
    match win::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE) {
        Ok(process) => still_running(&process),
        // A process of another user's or an elevated one is there too.
        Err(err) => err.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32),
    }
}

/// Whether the process `process` names still runs. One that ended stays
/// openable while any handle to it is open, and is signalled.
#[allow(unsafe_code)]
fn still_running(process: &OwnedHandle) -> bool {
    // SAFETY: the handle is open for the call, which only reads it.
    unsafe { WaitForSingleObject(win::raw(process), 0) == WAIT_TIMEOUT }
}

pub fn started(pid: u32) -> Option<u64> {
    let process =
        win::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE).ok()?;
    if !still_running(&process) {
        return None;
    }
    win::started(win::raw(&process)).ok()
}

/// The longest path Windows gives a program, in UTF-16 units.
const PATH_MAX: usize = 32 * 1024;

#[allow(unsafe_code)]
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    let process = win::open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut path = vec![0u16; PATH_MAX];
    let mut len = u32::try_from(path.len()).ok()?;
    // SAFETY: the buffer holds `len` units, and the call writes at most
    // that many and sets `len` to how many it wrote.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            win::raw(&process),
            PROCESS_NAME_WIN32,
            path.as_mut_ptr(),
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    path.truncate(usize::try_from(len).ok()?);
    Some(PathBuf::from(OsString::from_wide(&path)))
}

pub async fn cmdline(pid: u32) -> io::Result<Option<String>> {
    let process = match open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(process) => process,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let line = command_line(win::raw(&process))?;
    Ok(Some(super::args::split(&line).join(" ")))
}

/// The command line the process `process` names started with, as its
/// parent wrote it.
#[allow(unsafe_code)]
fn command_line(process: HANDLE) -> io::Result<String> {
    let mut size = 0u32;
    // SAFETY: asked with no buffer, the call only writes the size it
    // needs to `size`.
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            ProcessCommandLineInformation,
            std::ptr::null_mut(),
            0,
            &mut size,
        )
    };
    if status != STATUS_INFO_LENGTH_MISMATCH || size == 0 {
        return Err(io::Error::other(format!(
            "couldn't size the command line (NTSTATUS {status:#x})"
        )));
    }
    // Aligned for the UNICODE_STRING at its start.
    let words = usize::try_from(size).map_err(io::Error::other)?.div_ceil(8);
    let mut buffer = vec![0u64; words];
    let capacity = u32::try_from(words * 8).map_err(io::Error::other)?;
    // SAFETY: the buffer is `capacity` bytes, and the call writes at most
    // that many.
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            ProcessCommandLineInformation,
            buffer.as_mut_ptr().cast(),
            capacity,
            &mut size,
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!(
            "couldn't read the command line (NTSTATUS {status:#x})"
        )));
    }
    // SAFETY: the call wrote a UNICODE_STRING at the start, whose buffer
    // points into ours, `Length` bytes long.
    let text = unsafe {
        let string = &*buffer.as_ptr().cast::<UNICODE_STRING>();
        if string.Buffer.is_null() {
            return Ok(String::new());
        }
        std::slice::from_raw_parts(string.Buffer, usize::from(string.Length) / 2)
    };
    Ok(String::from_utf16_lossy(text))
}

pub fn is_root() -> bool {
    win::own_token()
        .and_then(|token| win::elevated(&token))
        .unwrap_or(false)
}

pub fn raise_open_files() -> io::Result<()> {
    Ok(())
}

pub fn stderr_file() -> io::Result<std::fs::File> {
    use std::os::windows::io::AsHandle;

    io::stderr()
        .as_handle()
        .try_clone_to_owned()
        .map(std::fs::File::from)
}

pub fn send_output_to(_file: &std::fs::File) -> io::Result<()> {
    Err(crate::unsupported(
        "pointing this process's output at a file",
    ))
}
