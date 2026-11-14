// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Finds the `codex` processes worth watching and the files they hold open.
//! Only lists and reads. Nothing here signals a process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `ps` itself. A LaunchAgent's `PATH` is short, so macOS gets the full path.
const PS: &str = if cfg!(target_os = "macos") {
    "/bin/ps"
} else {
    "ps"
};

/// `lsof`, on macOS. Linux reads `/proc` instead.
#[cfg(not(target_os = "linux"))]
const LSOF: &str = "/usr/sbin/lsof";

/// The ChatGPT app runs its own `codex` processes for the Codex desktop app,
/// which aren't sessions you started.
const CHATGPT_APP: &str = "/ChatGPT.app/";

/// One line of `ps`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    /// The process ID.
    pub pid: u32,
    /// Its parent's process ID.
    pub ppid: u32,
    /// The executable. macOS gives the full path and Linux the name.
    pub command: String,
}

/// What one process holds open that the observer reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenFiles {
    /// The working folder.
    pub cwd: Option<PathBuf>,
    /// Open rollout files, one per thread the process is running.
    pub rollouts: Vec<PathBuf>,
}

/// Every process on the device. Empty when `ps` fails.
pub fn list() -> Vec<Process> {
    match Command::new(PS)
        .args(["-A", "-o", "pid=,ppid=,comm="])
        .output()
    {
        Ok(output) if output.status.success() => parse_ps(&String::from_utf8_lossy(&output.stdout)),
        Ok(output) => {
            tracing::debug!(status = %output.status, "ps failed");
            Vec::new()
        }
        Err(error) => {
            tracing::debug!(%error, "couldn't run ps");
            Vec::new()
        }
    }
}

/// Parses `ps -A -o pid=,ppid=,comm=`. The command can hold spaces, so it's
/// the rest of the line.
pub fn parse_ps(output: &str) -> Vec<Process> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let (ppid, command) = rest.trim_start().split_once(char::is_whitespace)?;
            Some(Process {
                pid: pid.parse().ok()?,
                ppid: ppid.parse().ok()?,
                command: command.trim().to_owned(),
            })
        })
        .collect()
}

/// The processes that are Codex sessions: named `codex`, not part of the
/// ChatGPT app, and not started by the process `own`, which is `maxd`
/// running its own app-servers.
pub fn sessions(processes: &[Process], own: u32) -> Vec<u32> {
    let parents: HashMap<u32, u32> = processes.iter().map(|p| (p.pid, p.ppid)).collect();
    processes
        .iter()
        .filter(|p| {
            Path::new(&p.command)
                .file_name()
                .is_some_and(|name| name == "codex")
        })
        .filter(|p| !p.command.contains(CHATGPT_APP))
        .filter(|p| !descends_from(p.pid, own, &parents))
        .map(|p| p.pid)
        .collect()
}

/// Whether `pid` is `ancestor` or was started by it, directly or not.
fn descends_from(pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut current = pid;
    // A loop in the table can't happen, but a bound costs nothing.
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match parents.get(&current) {
            Some(&parent) if parent != current && parent != 0 => current = parent,
            _ => return false,
        }
    }
    false
}

/// The working folder and open rollouts of each process. A process that
/// exited in the meantime is missing.
#[cfg(not(target_os = "linux"))]
pub fn open_files(pids: &[u32]) -> HashMap<u32, OpenFiles> {
    if pids.is_empty() {
        return HashMap::new();
    }
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    // lsof exits 1 when one of the processes is gone, and still prints the
    // rest, so the status says nothing useful.
    match Command::new(LSOF)
        .args(["-n", "-P", "-w", "-p", &list.join(","), "-Fn"])
        .output()
    {
        Ok(output) => parse_lsof(&String::from_utf8_lossy(&output.stdout)),
        Err(error) => {
            tracing::debug!(%error, "couldn't run lsof");
            HashMap::new()
        }
    }
}

/// The working folder and open rollouts of each process. A process that
/// exited in the meantime is missing.
#[cfg(target_os = "linux")]
pub fn open_files(pids: &[u32]) -> HashMap<u32, OpenFiles> {
    pids.iter()
        .filter_map(|&pid| {
            let proc = PathBuf::from(format!("/proc/{pid}"));
            let cwd = std::fs::read_link(proc.join("cwd")).ok();
            let rollouts = std::fs::read_dir(proc.join("fd"))
                .ok()?
                .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
                .filter(|path| is_rollout(path))
                .collect();
            Some((pid, OpenFiles { cwd, rollouts }))
        })
        .collect()
}

/// Parses `lsof -Fn`, where `p` starts a process, `f` names a descriptor
/// and `n` gives its path.
#[cfg(any(not(target_os = "linux"), test))]
pub fn parse_lsof(output: &str) -> HashMap<u32, OpenFiles> {
    let mut all: HashMap<u32, OpenFiles> = HashMap::new();
    let mut pid = None;
    let mut descriptor = String::new();
    for line in output.lines() {
        let mut chars = line.chars();
        let Some(field) = chars.next() else {
            continue;
        };
        let value = chars.as_str();
        match field {
            'p' => {
                pid = value.parse().ok();
                if let Some(pid) = pid {
                    all.entry(pid).or_default();
                }
            }
            'f' => value.clone_into(&mut descriptor),
            'n' => {
                let Some(files) = pid.and_then(|pid| all.get_mut(&pid)) else {
                    continue;
                };
                let path = PathBuf::from(value);
                if descriptor == "cwd" {
                    files.cwd = Some(path);
                } else if is_rollout(&path) && !files.rollouts.contains(&path) {
                    files.rollouts.push(path);
                }
            }
            _ => {}
        }
    }
    all
}

/// Whether a path names a rollout that's still being written. Compressed
/// `.jsonl.zst` rollouts are finished, so they never count.
pub fn is_rollout(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS_OUTPUT: &str = include_str!("fixtures/ps.txt");
    const LSOF_OUTPUT: &str = include_str!("fixtures/lsof.txt");

    #[test]
    fn ps_lines_keep_spaces_in_commands() {
        let processes = parse_ps(PS_OUTPUT);
        assert_eq!(processes.len(), 11, "the last line isn't a process");
        let service = processes.iter().find(|p| p.pid == 79631).unwrap();
        assert_eq!(service.ppid, 79594);
        assert!(service.command.ends_with("Codex (Service)"));
    }

    #[test]
    fn only_your_own_codex_sessions_count() {
        let processes = parse_ps(PS_OUTPUT);
        // 41000 is maxd, which runs an app-server as 41010 and that one's
        // helper as 41011. 79699 is the ChatGPT app's. 52000 is the npm
        // launcher, and 52001 the native binary it starts.
        assert_eq!(sessions(&processes, 41000), vec![50100, 52001]);
    }

    #[test]
    fn lsof_gives_the_cwd_and_open_rollouts() {
        let open = parse_lsof(LSOF_OUTPUT);
        let tui = &open[&50100];
        assert_eq!(tui.cwd.as_deref(), Some(Path::new("/Users/dev/src/my app")));
        assert_eq!(
            tui.rollouts,
            vec![
                PathBuf::from(
                    "/Users/dev/.codex/sessions/2026/10/24/rollout-2026-10-24T09-12-03-0199f0a1-7c1e-7a52-9d1e-3f4a5b6c7d8e.jsonl"
                ),
                PathBuf::from(
                    "/Users/dev/.codex/sessions/2026/10/24/rollout-2026-10-24T09-30-41-0199f0b2-1111-7222-8333-944455566677.jsonl"
                ),
            ]
        );
        let other = &open[&52001];
        assert_eq!(other.cwd.as_deref(), Some(Path::new("/Users/dev/src/api")));
        assert!(
            other.rollouts.is_empty(),
            "a new session writes nothing yet"
        );
    }

    #[test]
    fn compressed_rollouts_are_never_read() {
        assert!(is_rollout(Path::new(
            "/x/rollout-2026-10-24T09-12-03-a.jsonl"
        )));
        assert!(!is_rollout(Path::new(
            "/x/rollout-2026-10-24T09-12-03-a.jsonl.zst"
        )));
        assert!(!is_rollout(Path::new("/x/history.jsonl")));
    }
}
