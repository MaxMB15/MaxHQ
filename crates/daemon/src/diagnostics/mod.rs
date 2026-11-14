// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What `maxd`, its agents and its container workspaces use. A task
//! samples `maxd` and every holder's process tree each 5 seconds and keeps
//! the last hour. [`Monitor::read`] cuts that into buckets and finds the
//! busiest processes, and [`load`] reads the device's free CPU and memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maxhq_hold::{Found, Holds};
use maxhq_protocol::ids::{AgentId, SessionId, WorkspaceId};
use maxhq_protocol::message::{ActionOutcome, ContainerUse, ProcessUse, UseBucket};
use maxhq_protocol::model::WorkspaceLocation;
use maxhq_protocol::state::Entity;
use maxhq_protocol::time::Timestamp;
use tracing::warn;

use crate::hub::Hub;
use crate::incus::{Incus, InstanceName};

pub mod collect;
pub mod history;
pub mod load;
pub mod mask;

use collect::{Collector, Root, Sample};
use history::{HistoryRecorder, HISTORY_RETENTION_MS};

/// How often the task samples.
const SAMPLE_EVERY: Duration = Duration::from_secs(5);
/// The shortest bucket, one sample long.
const MIN_BUCKET_SECS: u32 = 5;
/// The most buckets one read gives.
const MAX_BUCKETS: u32 = 720;
/// How many of the busiest processes a read gives.
const TOP: usize = 10;
/// How much of a command line a read gives, in characters.
const COMMAND_CHARS: usize = 200;

/// Whose a root process is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    /// The agent the holder runs.
    pub agent: AgentId,
    /// The workspace the agent runs in.
    pub workspace: WorkspaceId,
}

/// The samples and whose each root was.
#[derive(Default)]
struct Kept {
    history: HistoryRecorder,
    owners: HashMap<u32, Owner>,
}

/// Samples `maxd` and its holders in the background.
pub struct Monitor {
    hub: Hub,
    incus: Option<Incus>,
    kept: Mutex<Kept>,
}

impl Monitor {
    /// Starts sampling. `incus` is the Incus to read container totals from,
    /// if the device runs one.
    pub fn start(hub: Hub, holds: Arc<Holds>, incus: Option<Incus>) -> Arc<Self> {
        let monitor = Arc::new(Self {
            hub,
            incus,
            kept: Mutex::new(Kept::default()),
        });
        let task = Arc::clone(&monitor);
        tokio::spawn(async move { task.run(holds).await });
        monitor
    }

    async fn run(&self, holds: Arc<Holds>) {
        let collector = Arc::new(Mutex::new(Collector::new()));
        {
            let collector = Arc::clone(&collector);
            let primed =
                tokio::task::spawn_blocking(move || lock(&collector).prime_cpu_usage()).await;
            if primed.is_err() {
                warn!("the diagnostics sampler stopped while starting");
                return;
            }
        }
        let mut tick = tokio::time::interval(SAMPLE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let (roots, owners) = self.roots(&holds).await;
            let collector = Arc::clone(&collector);
            let Ok(sample) =
                tokio::task::spawn_blocking(move || lock(&collector).sample(&roots)).await
            else {
                warn!("the diagnostics sampler stopped");
                return;
            };
            let mut kept = lock(&self.kept);
            kept.history.record(&sample);
            kept.owners.extend(owners);
        }
    }

    /// `maxd` and every live holder, with the agent each holder runs.
    async fn roots(&self, holds: &Holds) -> (Vec<Root>, HashMap<u32, Owner>) {
        let mut roots = vec![Root {
            pid: std::process::id(),
            start_time_ms: None,
        }];
        let agents = agents_by_session(&self.hub);
        let mut owners = HashMap::new();
        let sessions = holds.list().unwrap_or_default();
        for session in sessions {
            let Ok(Found::Live { holder_pid }) = holds.find(session).await else {
                continue;
            };
            roots.push(Root {
                pid: holder_pid,
                start_time_ms: None,
            });
            if let Some(owner) = agents.get(&session) {
                owners.insert(holder_pid, *owner);
            }
        }
        (roots, owners)
    }

    /// What was used over the last `window_secs`, in buckets of
    /// `bucket_secs`.
    pub async fn read(&self, window_secs: u32, bucket_secs: u32) -> ActionOutcome {
        let now = collect::unix_time_ms();
        let (buckets, top) = {
            let kept = lock(&self.kept);
            let window_ms = u64::from(window_secs) * 1_000;
            let samples = kept.history.read(window_ms, now);
            summarize(&samples, &kept.owners, window_secs, bucket_secs, now)
        };
        let (containers, error) = match &self.incus {
            Some(incus) => containers(incus, &self.hub).await,
            None => (Vec::new(), None),
        };
        ActionOutcome::Diagnostics {
            buckets,
            top,
            containers,
            error,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Each agent with a session, by that session.
fn agents_by_session(hub: &Hub) -> HashMap<SessionId, Owner> {
    hub.snapshot()
        .entities
        .into_iter()
        .filter_map(|entity| match entity {
            Entity::Agent(agent) => agent.session.map(|session| {
                (
                    session,
                    Owner {
                        agent: agent.id,
                        workspace: agent.workspace,
                    },
                )
            }),
            _ => None,
        })
        .collect()
}

/// Incus's totals for each container workspace on the device.
async fn containers(incus: &Incus, hub: &Hub) -> (Vec<ContainerUse>, Option<String>) {
    let workspaces: Vec<(WorkspaceId, String)> = hub
        .snapshot()
        .entities
        .into_iter()
        .filter_map(|entity| match entity {
            Entity::Workspace(workspace) => match workspace.location {
                WorkspaceLocation::Container { instance, .. } => Some((workspace.id, instance)),
                WorkspaceLocation::Folder { .. } => None,
            },
            _ => None,
        })
        .collect();
    let mut used = Vec::new();
    let mut error = None;
    for (workspace, instance) in workspaces {
        let Ok(name) = InstanceName::new(&instance) else {
            continue;
        };
        match incus.usage(&name).await {
            Ok(Some((cpu_ns, memory))) => used.push(ContainerUse {
                workspace,
                cpu_ns,
                memory,
            }),
            Ok(None) => {}
            Err(e) => {
                error.get_or_insert_with(|| format!("Couldn't read container use: {e}"));
            }
        }
    }
    (used, error)
}

/// `value`, or 0 when it is NaN or infinite, which JSON would write as null.
fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

/// Buckets and the busiest processes from `samples`, oldest first.
fn summarize(
    samples: &[Sample],
    owners: &HashMap<u32, Owner>,
    window_secs: u32,
    bucket_secs: u32,
    now_ms: u64,
) -> (Vec<UseBucket>, Vec<ProcessUse>) {
    let window_ms = (u64::from(window_secs) * 1_000).min(HISTORY_RETENTION_MS);
    let bucket_secs = bucket_secs
        .max(MIN_BUCKET_SECS)
        .max(window_secs.div_ceil(MAX_BUCKETS));
    let bucket_ms = u64::from(bucket_secs) * 1_000;
    let count = window_ms.div_ceil(bucket_ms).max(1);
    let start = now_ms.saturating_sub(count * bucket_ms);

    let mut buckets: Vec<UseBucket> = (0..count)
        .map(|i| UseBucket {
            started_at: Timestamp(start + i * bucket_ms),
            ended_at: Timestamp(start + (i + 1) * bucket_ms),
            avg_cpu: 0.0,
            max_cpu: 0.0,
            max_rss: 0,
            max_processes: 0,
        })
        .collect();
    let mut sums = vec![(0.0f32, 0u32); buckets.len()];

    // Each process by pid and start time, so a reused pid counts apart.
    struct Seen {
        pid: u32,
        root: u32,
        command: String,
        cpu_sum: f32,
        cpu_max: f32,
        samples: u32,
        rss_max: u64,
        cpu_now: f32,
        rss_now: u64,
    }
    let mut seen: HashMap<(u32, u64), Seen> = HashMap::new();
    let latest = samples.last().map(|sample| sample.sampled_at_ms);

    for sample in samples {
        if sample.sampled_at_ms < start {
            continue;
        }
        let index = ((sample.sampled_at_ms - start) / bucket_ms).min(count - 1) as usize;
        let cpu: f32 = sample.processes.iter().map(|p| p.cpu_percent).sum();
        let rss: u64 = sample.processes.iter().map(|p| p.resident_bytes).sum();
        let bucket = &mut buckets[index];
        bucket.max_cpu = bucket.max_cpu.max(cpu);
        bucket.max_rss = bucket.max_rss.max(rss);
        bucket.max_processes = bucket
            .max_processes
            .max(u32::try_from(sample.processes.len()).unwrap_or(u32::MAX));
        sums[index].0 += cpu;
        sums[index].1 += 1;

        let is_latest = Some(sample.sampled_at_ms) == latest;
        for process in &sample.processes {
            let entry = seen
                .entry((process.pid, process.start_time_ms))
                .or_insert_with(|| Seen {
                    pid: process.pid,
                    root: process.root,
                    command: process.command.clone(),
                    cpu_sum: 0.0,
                    cpu_max: 0.0,
                    samples: 0,
                    rss_max: 0,
                    cpu_now: 0.0,
                    rss_now: 0,
                });
            entry.cpu_sum += process.cpu_percent;
            entry.cpu_max = entry.cpu_max.max(process.cpu_percent);
            entry.samples += 1;
            entry.rss_max = entry.rss_max.max(process.resident_bytes);
            if is_latest {
                entry.cpu_now = process.cpu_percent;
                entry.rss_now = process.resident_bytes;
            }
        }
    }
    for (bucket, (sum, n)) in buckets.iter_mut().zip(sums) {
        if n > 0 {
            bucket.avg_cpu = finite(sum / n as f32);
        }
        bucket.max_cpu = finite(bucket.max_cpu);
    }

    let mut top: Vec<ProcessUse> = seen
        .into_values()
        .map(|seen| {
            let owner = owners.get(&seen.root);
            ProcessUse {
                agent: owner.map(|owner| owner.agent),
                workspace: owner.map(|owner| owner.workspace),
                pid: seen.pid,
                command: seen.command.chars().take(COMMAND_CHARS).collect(),
                cpu_now: finite(seen.cpu_now),
                cpu_avg: finite(seen.cpu_sum / seen.samples.max(1) as f32),
                cpu_max: finite(seen.cpu_max),
                rss_now: seen.rss_now,
                rss_max: seen.rss_max,
            }
        })
        .collect();
    top.sort_by(|a, b| {
        b.cpu_avg
            .total_cmp(&a.cpu_avg)
            .then(b.rss_max.cmp(&a.rss_max))
            .then(a.pid.cmp(&b.pid))
    });
    top.truncate(TOP);
    (buckets, top)
}

#[cfg(test)]
mod tests {
    use super::collect::ProcessSample;
    use super::*;

    fn process(pid: u32, root: u32, cpu: f32, rss: u64) -> ProcessSample {
        ProcessSample {
            pid,
            ppid: root,
            root,
            start_time_ms: 1_000,
            command: format!("process {pid} {}", "x".repeat(300)),
            cpu_percent: cpu,
            resident_bytes: rss,
        }
    }

    fn sample(at: u64, processes: Vec<ProcessSample>) -> Sample {
        Sample {
            sampled_at_ms: at,
            roots: Vec::new(),
            processes,
            inaccessible: 0,
        }
    }

    #[test]
    fn a_holders_processes_are_put_down_to_its_agent() {
        let maxd = 100;
        let holder = 200;
        let owner = Owner {
            agent: AgentId::generate(),
            workspace: WorkspaceId::generate(),
        };
        let owners = HashMap::from([(holder, owner)]);
        let now = 60_000;
        let samples = vec![
            sample(
                50_000,
                vec![
                    process(maxd, maxd, 1.0, 10),
                    process(201, holder, 50.0, 300),
                ],
            ),
            sample(
                55_000,
                vec![
                    process(maxd, maxd, 3.0, 20),
                    process(201, holder, 30.0, 200),
                ],
            ),
        ];

        let (buckets, top) = summarize(&samples, &owners, 60, 10, now);

        assert_eq!(buckets.len(), 6);
        assert_eq!(buckets[0].started_at, Timestamp(0));
        assert_eq!(buckets[5].ended_at, Timestamp(now));
        let last = &buckets[5];
        assert_eq!(last.max_cpu, 51.0);
        assert_eq!(last.avg_cpu, (51.0 + 33.0) / 2.0);
        assert_eq!(last.max_rss, 310);
        assert_eq!(last.max_processes, 2);
        assert_eq!(buckets[0].max_processes, 0);

        assert_eq!(top.len(), 2);
        let busiest = &top[0];
        assert_eq!(busiest.pid, 201);
        assert_eq!(busiest.agent, Some(owner.agent));
        assert_eq!(busiest.workspace, Some(owner.workspace));
        assert_eq!(busiest.cpu_avg, 40.0);
        assert_eq!(busiest.cpu_max, 50.0);
        assert_eq!(busiest.cpu_now, 30.0);
        assert_eq!(busiest.rss_now, 200);
        assert_eq!(busiest.rss_max, 300);
        assert_eq!(busiest.command.chars().count(), COMMAND_CHARS);
        assert_eq!(top[1].pid, maxd);
        assert_eq!(top[1].agent, None);
    }

    #[test]
    fn a_read_keeps_to_the_bucket_limits() {
        let (buckets, top) = summarize(&[], &HashMap::new(), 3_600, 1, 3_600_000);
        assert_eq!(buckets.len(), 720);
        assert!(top.is_empty());
        let (buckets, _) = summarize(&[], &HashMap::new(), 7_200, 60, 7_200_000);
        assert_eq!(buckets.len(), 60);
    }

    #[test]
    fn a_reading_that_is_not_a_number_comes_out_as_zero() {
        let samples = vec![sample(
            1_000,
            vec![process(1, 1, f32::NAN, 0), process(2, 1, f32::INFINITY, 0)],
        )];
        let (buckets, top) = summarize(&samples, &HashMap::new(), 60, 60, 2_000);
        for bucket in &buckets {
            assert!(bucket.avg_cpu.is_finite() && bucket.max_cpu.is_finite());
        }
        for process in &top {
            for cpu in [process.cpu_now, process.cpu_avg, process.cpu_max] {
                assert_eq!(cpu, 0.0);
            }
        }
        assert_eq!(finite(f32::NEG_INFINITY), 0.0);
        assert_eq!(finite(2.5), 2.5);
    }

    #[test]
    fn only_the_ten_busiest_are_given() {
        let processes = (1..=15).map(|pid| process(pid, 1, pid as f32, 0)).collect();
        let (_, top) = summarize(&[sample(1_000, processes)], &HashMap::new(), 60, 60, 2_000);
        let pids: Vec<u32> = top.iter().map(|p| p.pid).collect();
        assert_eq!(pids, (6..=15).rev().collect::<Vec<_>>());
    }
}
