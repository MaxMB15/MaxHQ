// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Which session processes still run, checked the way Claude Code checks
//! them. It stores each process's start time as `LC_ALL=C TZ=UTC ps -o
//! lstart= -p <pid>` prints it, so the same `ps` call both proves the PID
//! runs and tells the session's process from a later one that reused the
//! PID. `ps` only reads the process table and never signals anything.
//! Windows has no `ps` of its own, and Git's takes other options, so there
//! the system says which PIDs run and their start times go unchecked.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};

/// Where to look for `ps` after `PATH`. NixOS has none in `/bin`.
const PS: [&str; 4] = [
    "ps",
    "/bin/ps",
    "/usr/bin/ps",
    "/run/current-system/sw/bin/ps",
];

/// The processes that run among the PIDs asked about.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Processes {
    /// Each running PID and its start time with spaces collapsed, or none
    /// when the start time couldn't be read.
    started: HashMap<u32, Option<String>>,
}

impl Processes {
    /// Looks up `pids` with one `ps` call. A PID that `ps` rejects fails
    /// the whole call, so then each one is asked about alone. Windows skips
    /// `ps`.
    pub fn lookup(pids: &[u32]) -> Self {
        let mut pids: Vec<u32> = pids.iter().copied().filter(|&pid| pid > 1).collect();
        pids.sort_unstable();
        pids.dedup();
        if pids.is_empty() {
            return Self::default();
        }
        let listed = if cfg!(windows) { None } else { run_ps(&pids) };
        match listed {
            Some(Ps::Listed(found)) => found,
            Some(Ps::Rejected) if pids.len() > 1 => {
                let mut found = Self::default();
                for pid in pids {
                    if let Some(Ps::Listed(one)) = run_ps(&[pid]) {
                        found.started.extend(one.started);
                    }
                }
                found
            }
            Some(Ps::Rejected) => Self::default(),
            None => Self::without_ps(&pids),
        }
    }

    /// Reads `ps -o pid= -o lstart=` output.
    pub fn parse(output: &str) -> Self {
        let started = output
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let (pid, start) = line.split_once(char::is_whitespace)?;
                Some((pid.parse().ok()?, Some(collapse(start))))
            })
            .collect();
        Self { started }
    }

    /// Whether `pid` runs and is the process that wrote `proc_start`. A
    /// session file without `procStart`, or a process whose start time
    /// can't be read, needs only the PID to run.
    pub fn runs(&self, pid: u32, proc_start: Option<&str>) -> bool {
        match (self.started.get(&pid), proc_start) {
            (None, _) => false,
            (Some(Some(started)), Some(expected)) => *started == collapse(expected),
            (Some(_), _) => true,
        }
    }

    /// The fallback without `ps`: on Linux a PID runs while `/proc/<pid>`
    /// exists, and on Windows while the system says it does. The start time
    /// goes unchecked.
    fn without_ps(pids: &[u32]) -> Self {
        let runs: fn(&u32) -> bool = if cfg!(target_os = "linux") {
            |pid| Path::new(&format!("/proc/{pid}")).exists()
        } else if cfg!(windows) {
            |&pid| maxhq_os::proc::alive(pid)
        } else {
            tracing::warn!("can't run ps, so no Claude session counts as running");
            return Self::default();
        };
        let started = pids
            .iter()
            .filter(|pid| runs(pid))
            .map(|&pid| (pid, None))
            .collect();
        Self { started }
    }
}

/// What one `ps` call gave.
enum Ps {
    /// The processes it found, which may be none.
    Listed(Processes),
    /// It refused the arguments, such as a PID past the system's limit.
    Rejected,
}

/// Runs `ps` for `pids`, or returns none when there's no `ps` to run.
fn run_ps(pids: &[u32]) -> Option<Ps> {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    for ps in PS {
        let output = Command::new(ps)
            .args(["-o", "pid=", "-o", "lstart=", "-p", &list])
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .stdin(Stdio::null())
            .output();
        match output {
            Ok(output) => {
                // `ps` exits 1 both when no PID runs and when it refuses
                // one. Only a refusal says why.
                let listed = Processes::parse(&String::from_utf8_lossy(&output.stdout));
                if !output.status.success()
                    && listed.started.is_empty()
                    && !output.stderr.is_empty()
                {
                    return Some(Ps::Rejected);
                }
                return Some(Ps::Listed(listed));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(%error, "ps failed");
                return None;
            }
        }
    }
    None
}

/// Collapses runs of whitespace, since `ps` pads a one-digit day and its
/// columns.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "    1 Tue Sep 29 10:37:01 2026    \n41001 Sun Oct 25 00:13:20 2026\n41002 Mon Oct  5 09:00:00 2026\n";

    #[test]
    fn a_running_pid_with_its_start_time_runs() {
        let processes = Processes::parse(OUTPUT);
        assert!(processes.runs(41001, Some("Sun Oct 25 00:13:20 2026")));
        assert!(processes.runs(41002, Some("Mon Oct  5 09:00:00 2026")));
        assert!(processes.runs(41002, Some("Mon Oct 5 09:00:00 2026 ")));
    }

    #[test]
    fn a_reused_pid_does_not_run() {
        let processes = Processes::parse(OUTPUT);
        // Same PID, but a process that started later than the session's.
        assert!(!processes.runs(41001, Some("Sat Oct 24 22:01:07 2026")));
    }

    #[test]
    fn a_missing_pid_does_not_run() {
        let processes = Processes::parse(OUTPUT);
        assert!(!processes.runs(41003, Some("Sun Oct 25 00:13:20 2026")));
        assert!(!processes.runs(41003, None));
        assert!(processes.runs(41001, None));
    }

    #[test]
    fn finds_real_processes() {
        let me = std::process::id();
        let found = Processes::lookup(&[me, 1, 0]);
        assert!(found.runs(me, None));
        assert!(!found.runs(1, None), "PID 1 is never a session");
    }

    // Unix only: there `ps` gives start times, and Windows checks the PID alone.
    #[cfg(unix)]
    #[test]
    fn ps_tells_a_reused_pid_by_its_start_time() {
        let me = std::process::id();
        let found = Processes::lookup(&[me]);
        let start = found
            .started
            .get(&me)
            .cloned()
            .flatten()
            .expect("a start time");
        assert!(found.runs(me, Some(&start)));
        assert!(!found.runs(me, Some("Thu Jan  1 00:00:00 1970")));
    }

    #[test]
    fn a_pid_ps_refuses_does_not_hide_the_others() {
        let me = std::process::id();
        let found = Processes::lookup(&[me, 4_000_000_000]);
        assert!(found.runs(me, None));
        assert!(!found.runs(4_000_000_000, None));
    }
}
