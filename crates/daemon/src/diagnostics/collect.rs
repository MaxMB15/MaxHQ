// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the collector's
// two-stage refresh, choosing which processes to track, matching a process
// by its start time and by its external identity, the wait for a full CPU
// measurement, counting the processes it can't read and truncating at a
// UTF-8 boundary, with their tests. Each sample here also says which root
// each process descends from, so its use can be put down to an agent.

//! Samples maxd and the processes it started: maxd's own tree and each
//! holder's.

use std::collections::{HashMap, HashSet, VecDeque};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sysinfo::{
    Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, MINIMUM_CPU_UPDATE_INTERVAL,
};

use super::mask::{masked, masked_args};

/// sysinfo reads start times to the second.
const PROCESS_START_TIME_PRECISION_MS: u64 = 1_000;

/// The most of a command line a sample keeps.
pub const MAX_PROCESS_COMMAND_BYTES: usize = 1_024;

/// Linux and macOS reset a process's CPU times on any refresh, so the
/// details come from a second `System`.
const TWO_STAGE_PROCESS_REFRESH: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// A process to sample along with its descendants, and the start time it
/// must have so a reused pid isn't taken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    /// Its process ID.
    pub pid: u32,
    /// When it started, in milliseconds since 1970, if known.
    pub start_time_ms: Option<u64>,
}

/// One process in a sample.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessSample {
    /// Its process ID.
    pub pid: u32,
    /// Its parent's.
    pub ppid: u32,
    /// The root it descends from.
    pub root: u32,
    /// When it started, in milliseconds since 1970.
    pub start_time_ms: u64,
    /// Its command line with credentials taken out, cut to
    /// [`MAX_PROCESS_COMMAND_BYTES`].
    pub command: String,
    /// Its CPU, in percent of one CPU.
    pub cpu_percent: f32,
    /// Its resident memory, in bytes.
    pub resident_bytes: u64,
}

impl ProcessSample {
    /// Roughly what keeping it in history costs.
    pub fn estimated_history_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(self.command.len())
    }
}

/// Every process under the roots at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// When it was taken, in milliseconds since 1970.
    pub sampled_at_ms: u64,
    /// The roots that were alive, with their checked start times.
    pub roots: Vec<Root>,
    /// The processes, by pid.
    pub processes: Vec<ProcessSample>,
    /// How many processes were picked but couldn't be read.
    pub inaccessible: usize,
}

impl Sample {
    /// How many entries it holds, for history's entry cap.
    pub fn retained_entry_count(&self) -> usize {
        self.processes.len().saturating_add(self.roots.len())
    }

    /// Roughly what keeping it in history costs.
    pub fn estimated_history_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.processes
                    .iter()
                    .map(ProcessSample::estimated_history_bytes)
                    .sum::<usize>(),
            )
            .saturating_add(self.roots.len() * std::mem::size_of::<Root>())
    }
}

/// Reads processes through sysinfo, keeping one `System` for CPU
/// baselines between samples.
pub struct Collector {
    system: System,
    cpu_baseline_refreshed_at: Option<Instant>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    /// A collector with no baseline yet.
    pub fn new() -> Self {
        Self {
            system: System::new(),
            cpu_baseline_refreshed_at: None,
        }
    }

    /// Takes a CPU baseline, so the first sample has CPU numbers.
    pub fn prime_cpu_usage(&mut self) {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            process_discovery_refresh_kind(),
        );
        self.cpu_baseline_refreshed_at = Some(Instant::now());
    }

    /// Samples every process under `roots`.
    pub fn sample(&mut self, roots: &[Root]) -> Sample {
        if let Some(delay) =
            remaining_cpu_measurement_delay(self.cpu_baseline_refreshed_at.take(), Instant::now())
        {
            thread::sleep(delay);
        }
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            process_discovery_refresh_kind(),
        );
        self.cpu_baseline_refreshed_at = Some(Instant::now());

        let rows = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| {
                let pid = pid.as_u32();
                let ppid = process.parent().map(Pid::as_u32).unwrap_or(0);
                (pid, ppid, process.start_time().saturating_mul(1_000))
            })
            .collect::<Vec<_>>();
        let live_roots = roots
            .iter()
            .filter_map(|root| {
                let (_, _, actual) = rows.iter().find(|(pid, _, _)| *pid == root.pid)?;
                matches_external_identity(*actual, root.start_time_ms).then_some(Root {
                    pid: root.pid,
                    start_time_ms: Some(*actual),
                })
            })
            .collect::<Vec<_>>();
        let root_pids = live_roots.iter().map(|root| root.pid).collect::<Vec<_>>();
        let tracked = select_tracked_roots(&rows, &root_pids);
        let tracked_count = tracked.len();
        let process_details = if TWO_STAGE_PROCESS_REFRESH && !tracked.is_empty() {
            let monitor_pid = Pid::from_u32(std::process::id());
            let mut detail_pids = tracked
                .keys()
                .copied()
                .map(Pid::from_u32)
                .collect::<Vec<_>>();
            if !tracked.contains_key(&monitor_pid.as_u32()) {
                detail_pids.push(monitor_pid);
            }
            let mut details = System::new();
            details.refresh_processes_specifics(
                ProcessesToUpdate::Some(&detail_pids),
                true,
                process_refresh_kind().without_cpu(),
            );
            // This process can't be replaced while it samples. Its start
            // time shows any shift in the boot epoch between the two.
            let start_time_offset = self.system.process(monitor_pid).and_then(|process| {
                details.process(monitor_pid).map(|detail| {
                    i128::from(detail.start_time()) - i128::from(process.start_time())
                })
            });
            Some((details, start_time_offset))
        } else {
            None
        };
        let sample_details = process_details
            .as_ref()
            .map_or(&self.system, |(details, _)| details);
        let start_time_offset = process_details
            .as_ref()
            .map_or(Some(0), |(_, offset)| *offset);
        let mut processes = tracked
            .into_iter()
            .filter_map(|(pid, root)| {
                let process = self.system.process(Pid::from_u32(pid))?;
                let details = sample_details.process(Pid::from_u32(pid))?;
                if !matches_process_start_time(
                    process.start_time(),
                    details.start_time(),
                    start_time_offset?,
                ) {
                    return None;
                }
                let command = if details.cmd().is_empty() {
                    masked(&process.name().to_string_lossy())
                } else {
                    masked_args(details.cmd().iter().map(|part| part.to_string_lossy()))
                };
                Some(ProcessSample {
                    pid,
                    ppid: process.parent().map(Pid::as_u32).unwrap_or(0),
                    root,
                    start_time_ms: process.start_time().saturating_mul(1_000),
                    command: truncate_utf8(command, MAX_PROCESS_COMMAND_BYTES),
                    cpu_percent: process.cpu_usage(),
                    resident_bytes: details.memory(),
                })
            })
            .collect::<Vec<_>>();
        drop(process_details);
        processes.sort_by_key(|process| process.pid);
        Sample {
            sampled_at_ms: unix_time_ms(),
            inaccessible: inaccessible_process_count(tracked_count, processes.len()),
            roots: live_roots,
            processes,
        }
    }
}

// CPU baselines stay apart. Even a metadata refresh resets Linux process
// times.
fn process_discovery_refresh_kind() -> ProcessRefreshKind {
    if TWO_STAGE_PROCESS_REFRESH {
        ProcessRefreshKind::nothing().with_cpu().without_tasks()
    } else {
        process_refresh_kind()
    }
}

fn process_refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_cmd(UpdateKind::Always)
        .without_tasks()
}

fn inaccessible_process_count(selected: usize, materialized: usize) -> usize {
    selected.saturating_sub(materialized)
}

fn matches_process_start_time(discovered: u64, detail: u64, epoch_offset: i128) -> bool {
    i128::from(detail) - epoch_offset == i128::from(discovered)
}

fn remaining_cpu_measurement_delay(
    baseline_refreshed_at: Option<Instant>,
    now: Instant,
) -> Option<Duration> {
    baseline_refreshed_at
        .and_then(|baseline| MINIMUM_CPU_UPDATE_INTERVAL.checked_sub(now.duration_since(baseline)))
        .filter(|delay| !delay.is_zero())
}

fn matches_external_identity(
    actual_start_time_ms: u64,
    expected_start_time_ms: Option<u64>,
) -> bool {
    // sysinfo reads whole seconds. This takes no reused pid from the
    // seconds next to it.
    expected_start_time_ms.is_none_or(|expected| {
        actual_start_time_ms == expected - (expected % PROCESS_START_TIME_PRECISION_MS)
    })
}

/// Every process under `roots`, each with the root it descends from. A
/// child that started before its parent belongs to an earlier owner of the
/// parent's pid, so it and its children are left out.
pub fn select_tracked_roots(rows: &[(u32, u32, u64)], roots: &[u32]) -> HashMap<u32, u32> {
    let mut children_by_parent = HashMap::<u32, Vec<(u32, u64)>>::new();
    let mut start_time_by_pid = HashMap::<u32, u64>::new();
    for (pid, ppid, start_time_ms) in rows {
        children_by_parent
            .entry(*ppid)
            .or_default()
            .push((*pid, *start_time_ms));
        start_time_by_pid.insert(*pid, *start_time_ms);
    }

    let mut tracked = HashMap::new();
    let mut visited_identities = HashSet::new();
    let mut queue = roots
        .iter()
        .filter_map(|pid| {
            start_time_by_pid
                .get(pid)
                .map(|start_time_ms| (*pid, *start_time_ms, *pid))
        })
        .collect::<VecDeque<_>>();

    while let Some((pid, start_time_ms, root)) = queue.pop_front() {
        if !visited_identities.insert((pid, start_time_ms)) {
            continue;
        }
        tracked.entry(pid).or_insert(root);
        if let Some(children) = children_by_parent.get(&pid) {
            queue.extend(
                children
                    .iter()
                    .copied()
                    .filter(|(_, child_start_time_ms)| *child_start_time_ms >= start_time_ms)
                    .map(|(child, child_start)| (child, child_start, root)),
            );
        }
    }

    tracked
}

/// The pids [`select_tracked_roots`] picks.
#[cfg(test)]
fn select_tracked_pids(rows: &[(u32, u32, u64)], roots: &HashSet<u32>) -> HashSet<u32> {
    let roots = roots.iter().copied().collect::<Vec<_>>();
    select_tracked_roots(rows, &roots).into_keys().collect()
}

/// `value` cut to at most `max_bytes`, at a character boundary.
pub fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    value.truncate(boundary);
    value
}

/// Milliseconds since 1970.
pub fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_roots_and_all_descendants() {
        let rows = vec![
            (10, 1, 1_000),
            (11, 10, 1_100),
            (12, 11, 1_200),
            (20, 1, 2_000),
            (21, 20, 2_100),
            (30, 99, 3_000),
        ];
        let tracked = select_tracked_pids(&rows, &HashSet::from([10, 20]));

        assert_eq!(tracked, HashSet::from([10, 11, 12, 20, 21]));
    }

    #[test]
    fn rejects_descendants_older_than_a_reused_parent_pid() {
        let rows = vec![
            (20, 1, 5_000),
            (21, 20, 4_000),
            (22, 20, 5_100),
            (23, 21, 5_200),
        ];
        let tracked = select_tracked_pids(&rows, &HashSet::from([20]));

        assert_eq!(tracked, HashSet::from([20, 22]));
    }

    #[test]
    fn ignores_missing_roots() {
        let rows = vec![(10, 1, 1_000), (11, 10, 1_100)];
        let tracked = select_tracked_pids(&rows, &HashSet::from([99]));

        assert!(tracked.is_empty());
    }

    #[test]
    fn each_process_names_the_root_it_descends_from() {
        let rows = vec![
            (10, 1, 1_000),
            (11, 10, 1_100),
            (20, 1, 2_000),
            (21, 20, 2_100),
            (22, 21, 2_200),
        ];
        let tracked = select_tracked_roots(&rows, &[10, 20]);

        assert_eq!(
            tracked,
            HashMap::from([(10, 10), (11, 10), (20, 20), (21, 20), (22, 20)])
        );
    }

    #[test]
    fn validates_external_process_start_identity() {
        assert!(matches_external_identity(10_000, None));
        assert!(matches_external_identity(10_000, Some(10_999)));
        assert!(!matches_external_identity(10_000, Some(11_000)));
        assert!(!matches_external_identity(10_000, Some(9_999)));
    }

    #[test]
    fn accepts_clock_shifts_without_accepting_reused_process_starts() {
        assert!(matches_process_start_time(10_000, 13_600, 3_600));
        assert!(!matches_process_start_time(10_000, 13_601, 3_600));
        assert!(matches_process_start_time(10_000, 6_400, -3_600));
        assert!(!matches_process_start_time(10_000, 6_401, -3_600));
        assert!(matches_process_start_time(10_000, 10_000, 0));
        assert!(!matches_process_start_time(10_000, 10_001, 0));
    }

    #[test]
    fn counts_selected_processes_that_could_not_be_materialized() {
        assert_eq!(inaccessible_process_count(5, 3), 2);
        assert_eq!(inaccessible_process_count(3, 5), 0);
    }

    #[test]
    fn waits_for_a_cpu_measurement_window_after_priming() {
        let baseline = Instant::now();

        assert_eq!(
            remaining_cpu_measurement_delay(Some(baseline), baseline),
            Some(MINIMUM_CPU_UPDATE_INTERVAL)
        );
        assert_eq!(
            remaining_cpu_measurement_delay(Some(baseline), baseline + MINIMUM_CPU_UPDATE_INTERVAL),
            None
        );
        assert_eq!(remaining_cpu_measurement_delay(None, baseline), None);
    }

    #[test]
    fn truncates_process_strings_at_utf8_boundaries() {
        let value = "é".repeat(MAX_PROCESS_COMMAND_BYTES);
        let truncated = truncate_utf8(value, MAX_PROCESS_COMMAND_BYTES - 1);

        assert!(truncated.len() < MAX_PROCESS_COMMAND_BYTES);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn samples_this_process() {
        let mut collector = Collector::new();
        collector.prime_cpu_usage();
        let me = std::process::id();
        let sample = collector.sample(&[Root {
            pid: me,
            start_time_ms: None,
        }]);
        let mine = sample.processes.iter().find(|process| process.pid == me);
        let mine = mine.expect("the test process is in its own sample");
        assert_eq!(mine.root, me);
        assert!(mine.resident_bytes > 0);
        assert_eq!(sample.roots.len(), 1);
    }
}
