// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Ending a holder that can't be reached, so a new one never runs beside
//! it.
//!
//! A process is only signalled once it's confirmed as the session's own.
//! The holder is confirmed by its lock, which goes with its process, by
//! `holder.json`, by leading a session of its own and by its command line,
//! which ends with the session's folder and `--detached`. Its CLI is
//! confirmed by leading its own process group inside the holder's session,
//! and an ID that's still in use as a session ID isn't given to a new
//! process. Anything that can't be confirmed is left alone.
//!
//! Windows has no sessions or groups to confirm by, and reuses IDs, so
//! there a process is confirmed by when it started, as `holder.json`
//! recorded it, and the holder by its command line too. The CLI's tree is
//! its Job Object, found by a name only a running CLI gives. The name
//! lasts only while a handle to the job is open, and the holder keeps the
//! only one, so the tree is ended before the holder rather than after.

use std::path::Path;
use std::time::{Duration, Instant};

use maxhq_protocol::ids::SessionId;

use crate::files::{self, HolderFile, EXIT, HOLDER};
use crate::holder::DETACHED;
use crate::{Exit, Holds};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// How long a holder or CLI has to go once it gets SIGKILL.
pub(super) const KILL_WAIT: Duration = Duration::from_secs(2);

/// How often to check whether a process has gone.
const POLL: Duration = Duration::from_millis(50);

/// How [`Holds::end`] went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// Neither the holder nor its CLI runs any more.
    Gone,
    /// Something may still run for the session, but it couldn't be
    /// confirmed as the session's own, so it was left alone.
    Unconfirmed {
        /// The process that may still run, when known.
        pid: Option<u32>,
        /// Why it couldn't be confirmed or ended.
        reason: String,
    },
}

pub(super) fn unconfirmed(pid: Option<u32>, reason: impl Into<String>) -> Ended {
    Ended::Unconfirmed {
        pid,
        reason: reason.into(),
    }
}

pub(crate) async fn end(
    holds: &Holds,
    session: SessionId,
    holder_pid: Option<u32>,
    grace: Duration,
) -> Ended {
    let sessions = match std::path::absolute(&holds.dirs.sessions) {
        Ok(sessions) => sessions,
        Err(err) => return unconfirmed(holder_pid, format!("couldn't find its folder: {err}")),
    };
    let dir = files::session_dir(&sessions, &session);
    if let Err(ended) = end_holder(&dir, holder_pid, grace).await {
        return ended;
    }
    if exit_recorded(&dir) {
        return Ended::Gone;
    }
    match read_holder(&dir) {
        Ok(Some(record)) => imp::end_cli(&record, grace).await,
        // The CLI never started.
        Ok(None) => Ended::Gone,
        Err(ended) => ended,
    }
}

/// Ends the session's holder if it still runs, once its record names the
/// process `expected` says it is.
async fn end_holder(dir: &Path, expected: Option<u32>, grace: Duration) -> Result<(), Ended> {
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
    imp::end_holder(dir, &record, grace).await
}

/// Whether the session's holder runs.
pub(super) fn alive(dir: &Path, pid: Option<u32>) -> Result<bool, Ended> {
    files::holder_alive(dir)
        .map_err(|err| unconfirmed(pid, format!("couldn't check its holder's lock: {err}")))
}

pub(super) fn read_holder(dir: &Path) -> Result<Option<HolderFile>, Ended> {
    files::read_json(&dir.join(HOLDER))
        .map_err(|err| unconfirmed(None, format!("couldn't read {HOLDER}: {err}")))
}

/// Whether the holder recorded how the CLI ended, which it does once it
/// has reaped it. A client that found the holder dead records an exit
/// without either, though the CLI may still run.
pub(super) fn exit_recorded(dir: &Path) -> bool {
    matches!(
        files::read_json::<Exit>(&dir.join(EXIT)),
        Ok(Some(exit)) if exit.code.is_some() || exit.signal.is_some()
    )
}

/// Whether process `pid` runs `maxd hold` on `dir`: its command line ends
/// with the folder and `--detached`.
pub(super) async fn runs_holder(dir: &Path, pid: u32) -> Result<bool, Ended> {
    let line = maxhq_os::proc::cmdline(pid).await.map_err(|err| {
        unconfirmed(
            Some(pid),
            format!("couldn't read process {pid}'s command line: {err}"),
        )
    })?;
    let wanted = format!("{} {DETACHED}", dir.display());
    Ok(line.is_some_and(|line| line.ends_with(&wanted)))
}

/// Why process `pid`, which holds the lock, isn't taken for the holder.
pub(super) fn not_its_holder(pid: u32) -> Ended {
    unconfirmed(
        Some(pid),
        format!("process {pid} holds its lock but doesn't look like its holder"),
    )
}

/// Waits up to `limit` for `done`. Returns whether it came.
pub(super) async fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}
