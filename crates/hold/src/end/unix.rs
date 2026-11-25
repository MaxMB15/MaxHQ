// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::Path;
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{Pid, Signal};

use super::{alive, read_holder, unconfirmed, wait_for, Ended, KILL_WAIT};
use crate::files::{self, EXIT};
use crate::holder::DETACHED;
use crate::Exit;

/// Ends the session's holder if it still runs. A holder that gets SIGTERM
/// stops its CLI and records the exit before it goes.
pub(super) async fn end_holder(
    dir: &Path,
    expected: Option<u32>,
    grace: Duration,
) -> Result<(), Ended> {
    if !alive(dir, expected)? {
        return Ok(());
    }
    let record = read_holder(dir)?.ok_or_else(|| {
        unconfirmed(
            expected,
            "its holder runs but hasn't recorded its process ID",
        )
    })?;
    let pid = record.holder_pid;
    if let Some(expected) = expected
        && expected != pid
    {
        return Err(unconfirmed(
            Some(pid),
            format!("its holder said it was process {expected}, but its record says {pid}"),
        ));
    }
    for (signal, wait) in [(Signal::TERM, grace), (Signal::KILL, KILL_WAIT)] {
        // Checked again before each signal, in case the holder ended and
        // its process ID went to something else.
        if !alive(dir, Some(pid))? {
            return Ok(());
        }
        let target = confirm_holder(dir, pid).await?;
        let _ = rustix::process::kill_process(target, signal);
        if wait_for(wait, || !alive(dir, Some(pid)).unwrap_or(true)).await {
            return Ok(());
        }
    }
    Err(unconfirmed(
        Some(pid),
        format!("its holder, process {pid}, didn't end, even after SIGKILL"),
    ))
}

/// Ends the CLI a dead holder left behind, if it still runs.
pub(super) async fn end_cli(dir: &Path, grace: Duration) -> Ended {
    // The holder records how the CLI ended once it has reaped it. A client
    // that found the holder dead records an exit without either, though the
    // CLI may still run.
    if let Ok(Some(exit)) = files::read_json::<Exit>(&dir.join(EXIT))
        && (exit.code.is_some() || exit.signal.is_some())
    {
        return Ended::Gone;
    }
    let record = match read_holder(dir) {
        Ok(Some(record)) => record,
        // The CLI never started.
        Ok(None) => return Ended::Gone,
        Err(ended) => return ended,
    };
    let (pid, holder) = (record.child_pid, record.holder_pid);
    for (signal, wait) in [(Signal::TERM, grace), (Signal::KILL, KILL_WAIT)] {
        let target = match cli(pid, holder) {
            Ok(Some(target)) => target,
            Ok(None) => return Ended::Gone,
            Err(reason) => return unconfirmed(Some(pid), reason),
        };
        if rustix::process::kill_process_group(target, signal).is_err() {
            let _ = rustix::process::kill_process(target, signal);
        }
        if wait_for(wait, || matches!(cli(pid, holder), Ok(None))).await {
            return Ended::Gone;
        }
    }
    unconfirmed(
        Some(pid),
        format!("its CLI, process {pid}, didn't end, even after SIGKILL"),
    )
}

fn to_pid(pid: u32) -> Option<Pid> {
    Pid::from_raw(i32::try_from(pid).ok()?)
}

/// Checks that process `pid` is the holder for `dir`.
async fn confirm_holder(dir: &Path, pid: u32) -> Result<Pid, Ended> {
    let doubt = |why: String| unconfirmed(Some(pid), why);
    let target = to_pid(pid).ok_or_else(|| doubt(format!("{pid} isn't a process ID")))?;
    let leader = rustix::process::getsid(Some(target)).is_ok_and(|sid| sid == target);
    let line = maxhq_os::proc::cmdline(pid)
        .await
        .map_err(|err| doubt(format!("couldn't read process {pid}'s command line: {err}")))?;
    let wanted = format!("{} {DETACHED}", dir.display());
    match line {
        Some(line) if leader && line.ends_with(&wanted) => Ok(target),
        _ => Err(doubt(format!(
            "process {pid} holds its lock but doesn't look like its holder"
        ))),
    }
}

/// The CLI's process while it still runs. A process with the CLI's ID that
/// doesn't lead its own group in the holder's session isn't the CLI: the
/// ID went to something else once the CLI ended.
fn cli(pid: u32, holder: u32) -> Result<Option<Pid>, String> {
    let (Some(target), Some(session)) = (to_pid(pid), to_pid(holder)) else {
        return Err(format!("{pid} or {holder} isn't a process ID"));
    };
    let group = match rustix::process::getpgid(Some(target)) {
        Ok(group) => group,
        Err(Errno::SRCH) => return Ok(None),
        Err(err) => return Err(format!("couldn't check process {pid}: {err}")),
    };
    let sid = match rustix::process::getsid(Some(target)) {
        Ok(sid) => sid,
        Err(Errno::SRCH) => return Ok(None),
        Err(err) => return Err(format!("couldn't check process {pid}: {err}")),
    };
    Ok((group == target && sid == session).then_some(target))
}
