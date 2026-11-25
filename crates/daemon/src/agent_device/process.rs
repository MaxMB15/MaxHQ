// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Running one command for a lease: its output cut at a cap, and its
//! whole process group stopped at a time limit.

use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use maxhq_protocol::leases::RunOutput;
use tokio::io::{AsyncRead, AsyncReadExt};

/// How long the output may keep coming once the command ended, from
/// anything it started that still holds the pipes open.
const DRAIN: Duration = Duration::from_millis(500);

/// One stream's output so far.
#[derive(Default)]
struct Kept {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Runs `command` in a process group of its own, with no input, keeping at
/// most `cap` bytes of each of stdout and stderr. Past `timeout` the group
/// is killed and the output so far kept.
pub async fn run(
    mut command: std::process::Command,
    timeout: Duration,
    cap: usize,
) -> io::Result<RunOutput> {
    use maxhq_os::proc::{self, GroupExt, Stop};

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .own_group();
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = crate::tools::retry_busy(|| std::future::ready(command.spawn())).await?;
    let pid = child.id();
    let _group = pid.map(proc::adopt).transpose()?;
    let (out, err) = (Arc::<Mutex<Kept>>::default(), Arc::<Mutex<Kept>>::default());
    let readers = [
        tokio::spawn(keep(child.stdout.take(), out.clone(), cap)),
        tokio::spawn(keep(child.stderr.take(), err.clone(), cap)),
    ];
    let (exit_code, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => (status?.code(), false),
        Err(_) => {
            if let Some(pid) = pid {
                let _ = proc::stop_group(pid, Stop::Hard);
            }
            let _ = child.kill().await;
            (None, true)
        }
    };
    for reader in readers {
        let abort = reader.abort_handle();
        if tokio::time::timeout(DRAIN, reader).await.is_err() {
            abort.abort();
        }
    }
    let take = |kept: &Mutex<Kept>| {
        let kept = std::mem::take(&mut *kept.lock().unwrap_or_else(PoisonError::into_inner));
        (
            String::from_utf8_lossy(&kept.bytes).into_owned(),
            kept.truncated,
        )
    };
    let ((stdout, cut_out), (stderr, cut_err)) = (take(&out), take(&err));
    Ok(RunOutput {
        exit_code,
        stdout,
        stderr,
        truncated: cut_out || cut_err,
        timed_out,
    })
}

/// Reads `stream` to its end into `kept`, keeping its first `cap` bytes.
async fn keep(stream: Option<impl AsyncRead + Unpin>, kept: Arc<Mutex<Kept>>, cap: usize) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut buffer = [0u8; 8192];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
        let room = cap.saturating_sub(kept.bytes.len());
        kept.bytes.extend_from_slice(&buffer[..read.min(room)]);
        kept.truncated |= read > room;
    }
}

/// The last line `output` printed, from stderr first, for a failure.
pub fn last_line(output: &RunOutput) -> Option<&str> {
    [&output.stderr, &output.stdout]
        .into_iter()
        .find_map(|text| {
            text.lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
        })
}
