// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::path::Path;
use std::time::Duration;

use maxhq_os::proc::{self, Stop};

use super::{
    alive, exit_recorded, not_its_holder, runs_holder, unconfirmed, wait_for, Ended, KILL_WAIT,
};
use crate::files::{HolderFile, HOLDER};
use crate::tree::{self, Sig};

/// Ends the holder `record` names. Windows can't ask a holder with no
/// console to end, so its CLI's tree is ended first, while the holder
/// still keeps the job's name, and the holder gets `grace` to record the
/// exit before it's ended too.
pub(super) async fn end_holder(
    dir: &Path,
    record: &HolderFile,
    grace: Duration,
) -> Result<(), Ended> {
    let pid = record.holder_pid;
    confirm_holder(dir, record).await?;
    if let Ok(true) = cli(record) {
        tree::signal_tree(record.child_pid, Sig::Kill);
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
    confirm_holder(dir, record).await?;
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
pub(super) async fn end_cli(record: &HolderFile, _grace: Duration) -> Ended {
    let pid = record.child_pid;
    match cli(record) {
        Ok(true) => {
            tree::signal_tree(pid, Sig::Kill);
        }
        Ok(false) => return Ended::Gone,
        Err(reason) => return unconfirmed(Some(pid), reason),
    }
    if wait_for(KILL_WAIT, || matches!(cli(record), Ok(false))).await {
        return Ended::Gone;
    }
    unconfirmed(
        Some(pid),
        format!("its CLI, process {pid}, didn't end, even when terminated"),
    )
}

/// Checks that the holder `record` names is the holder for `dir`: it
/// started when the record says and runs `maxd hold` on the folder.
async fn confirm_holder(dir: &Path, record: &HolderFile) -> Result<(), Ended> {
    let pid = record.holder_pid;
    let doubt = |why: String| unconfirmed(Some(pid), why);
    let Some(started) = record.holder_started else {
        return Err(doubt(format!(
            "{HOLDER} doesn't say when process {pid} started"
        )));
    };
    if proc::started(pid) != Some(started) {
        return Err(doubt(format!(
            "process {pid} holds its lock but isn't the process that started as its holder"
        )));
    }
    if runs_holder(dir, pid).await? {
        Ok(())
    } else {
        Err(not_its_holder(pid))
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
        None => Err(format!("{HOLDER} doesn't say when process {pid} started")),
    }
}
