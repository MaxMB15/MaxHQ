// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

use rustix::process::{Pid, Signal};

use super::Stop;

pub fn own_group(command: &mut std::process::Command) -> &mut std::process::Command {
    command.process_group(0)
}

pub fn own_group_tokio(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    command.process_group(0)
}

/// A process group needs nothing kept to reach it.
#[derive(Debug)]
pub struct Kept;

pub fn adopt(_leader: u32) -> io::Result<Kept> {
    Ok(Kept)
}

pub fn stop_group(leader: u32, how: Stop) -> io::Result<()> {
    rustix::process::kill_process_group(pid(leader)?, signal(how)).map_err(io::Error::from)
}

pub fn stop(raw: u32, how: Stop) -> io::Result<()> {
    rustix::process::kill_process(pid(raw)?, signal(how)).map_err(io::Error::from)
}

pub fn spawn_detached(command: &mut std::process::Command) -> io::Result<()> {
    // A group of its own, so it isn't stopped with this process.
    command.process_group(0).spawn().map(drop)
}

fn pid(raw: u32) -> io::Result<Pid> {
    i32::try_from(raw)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no such process ID"))
}

fn signal(how: Stop) -> Signal {
    match how {
        Stop::Soft => Signal::TERM,
        Stop::Hard => Signal::KILL,
    }
}

pub fn alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    // Signal 0 only checks that the process exists, and one of another
    // user's is there too.
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(err) => err == rustix::io::Errno::PERM,
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let pid = i32::try_from(pid).ok()?;
    let size = usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE).ok()?;
    let mut path = vec![0u8; size];
    let capacity = u32::try_from(path.len()).ok()?;
    // SAFETY: the buffer is `capacity` bytes, and the call writes at most
    // that many and says how many it wrote.
    let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), capacity) };
    let len = usize::try_from(len).ok().filter(|len| *len > 0)?;
    path.truncate(len);
    Some(PathBuf::from(OsString::from_vec(path)))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn started(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok()?;
    // SAFETY: the struct is plain data, for which zero is valid.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = i32::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: `info` is `size` bytes, the struct the flavor names.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (written == size).then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// In clock ticks since boot: the 22nd field of `/proc/<pid>/stat`, the
/// 20th after the command's name, which ends at the last `)`.
#[cfg(target_os = "linux")]
pub fn started(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn started(_pid: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn exe_of(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(target_os = "linux")]
pub async fn cmdline(pid: u32) -> io::Result<Option<String>> {
    match tokio::fs::read(format!("/proc/{pid}/cmdline")).await {
        Ok(bytes) => {
            let bytes = bytes.strip_suffix(&[0u8]).unwrap_or(&bytes[..]);
            Ok(Some(String::from_utf8_lossy(bytes).replace('\0', " ")))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// How long `ps` has to answer.
#[cfg(not(target_os = "linux"))]
const PS_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(not(target_os = "linux"))]
pub async fn cmdline(pid: u32) -> io::Result<Option<String>> {
    use std::process::Stdio;

    let output = tokio::process::Command::new("/bin/ps")
        .args(["-ww", "-o", "args=", "-p"])
        .arg(pid.to_string())
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(PS_WAIT, output)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ps didn't answer"))??;
    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.trim_end_matches('\n');
    // `ps` prints nothing and fails when there's no such process.
    if line.is_empty() && !output.status.success() {
        return Ok(None);
    }
    Ok(Some(line.to_owned()))
}

pub fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

pub const GENTLEST: Stop = Stop::Soft;

pub fn own_ids() -> Option<(u32, u32)> {
    Some((
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw(),
    ))
}

pub fn user_id() -> Option<String> {
    Some(rustix::process::getuid().as_raw().to_string())
}
