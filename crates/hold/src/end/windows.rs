// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::Path;
use std::time::Duration;

use maxhq_os::proc::{self, Stop};

use super::{alive, read_holder, unconfirmed, wait_for, Ended, KILL_WAIT};
use crate::files::{self, HolderFile, EXIT};
use crate::holder::DETACHED;
use crate::Exit;

/// Ends the session's holder if it still runs. Windows can't ask a holder
/// with no console to end, so its CLI's tree is ended first, while the
/// holder still keeps the job's name, and the holder gets `grace` to
/// record the exit before it's ended too.
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
    confirm_holder(dir, &record).await?;
    if let Ok(true) = cli(&record) {
        end_tree(record.child_pid);
        wait_for(grace, || {
            exit_recorded(dir) || !alive(dir, Some(pid)).unwrap_or(true)
        })
        .await;
    }
    // Checked again, in case the holder ended and its ID went to
    // something else.
    if !alive(dir, Some(pid))? {
        return Ok(());
    }
    confirm_holder(dir, &record).await?;
    let _ = proc::stop(pid, Stop::Hard);
    if wait_for(KILL_WAIT, || !alive(dir, Some(pid)).unwrap_or(true)).await {
        return Ok(());
    }
    Err(unconfirmed(
        Some(pid),
        format!("its holder, process {pid}, didn't end, even when terminated"),
    ))
}

/// Ends the CLI a dead holder left behind, if it still runs. With the
/// holder gone the job has lost its name, so only the CLI itself can be
/// ended, and what it started runs on.
pub(super) async fn end_cli(dir: &Path, _grace: Duration) -> Ended {
    if exit_recorded(dir) {
        return Ended::Gone;
    }
    let record = match read_holder(dir) {
        Ok(Some(record)) => record,
        // The CLI never started.
        Ok(None) => return Ended::Gone,
        Err(ended) => return ended,
    };
    let pid = record.child_pid;
    match cli(&record) {
        Ok(true) => end_tree(pid),
        Ok(false) => return Ended::Gone,
        Err(reason) => return unconfirmed(Some(pid), reason),
    }
    if wait_for(KILL_WAIT, || matches!(cli(&record), Ok(false))).await {
        return Ended::Gone;
    }
    unconfirmed(
        Some(pid),
        format!("its CLI, process {pid}, didn't end, even when terminated"),
    )
}

/// Whether the holder recorded how the CLI ended. A client that found the
/// holder dead records an exit without either, though the CLI may still
/// run.
fn exit_recorded(dir: &Path) -> bool {
    matches!(
        files::read_json::<Exit>(&dir.join(EXIT)),
        Ok(Some(exit)) if exit.code.is_some() || exit.signal.is_some()
    )
}

/// Ends the CLI's job, or the CLI alone when the job can't be found.
fn end_tree(pid: u32) {
    if proc::stop_group(pid, Stop::Hard).is_err() {
        let _ = proc::stop(pid, Stop::Hard);
    }
}

/// Checks that the holder `record` names is the holder for `dir`: it
/// started when the record says and runs `maxd hold` on the folder.
async fn confirm_holder(dir: &Path, record: &HolderFile) -> Result<(), Ended> {
    let pid = record.holder_pid;
    let doubt = |why: String| unconfirmed(Some(pid), why);
    let Some(started) = record.holder_started else {
        return Err(doubt(format!(
            "{} doesn't say when process {pid} started",
            crate::files::HOLDER
        )));
    };
    if proc::started(pid) != Some(started) {
        return Err(doubt(format!(
            "process {pid} holds its lock but isn't the process that started as its holder"
        )));
    }
    let line = proc::cmdline(pid)
        .await
        .map_err(|err| doubt(format!("couldn't read process {pid}'s command line: {err}")))?;
    let wanted = format!("{} {DETACHED}", dir.display());
    match line {
        Some(line) if line.ends_with(&wanted) => Ok(()),
        _ => Err(doubt(format!(
            "process {pid} holds its lock but doesn't look like its holder"
        ))),
    }
}

/// Whether the CLI still runs: a process has its ID and started when the
/// record says. One that started at another time got the ID later.
fn cli(record: &HolderFile) -> Result<bool, String> {
    let pid = record.child_pid;
    let Some(started) = proc::started(pid) else {
        return Ok(false);
    };
    match record.child_started {
        Some(recorded) => Ok(recorded == started),
        None => Err(format!(
            "{} doesn't say when process {pid} started",
            crate::files::HOLDER
        )),
    }
}
