// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: two CPU readings
// 200 ms apart, MemAvailable on Linux, vm_stat on macOS, and a 5 second
// cache.

//! The device's free CPU and memory.

use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use maxhq_protocol::message::HostLoad;
use maxhq_protocol::time::Timestamp;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::Mutex;

/// How long a reading is reused.
const CACHE_FOR: Duration = Duration::from_secs(5);
/// How far apart the two CPU readings are.
const CPU_GAP: Duration = Duration::from_millis(200);
/// How long `vm_stat` may take.
const VM_STAT_WAIT: Duration = Duration::from_secs(1);

static CACHED: LazyLock<Mutex<Option<(Instant, HostLoad)>>> = LazyLock::new(|| Mutex::new(None));

/// The device's load, read at most every 5 seconds. Callers that arrive
/// while a reading is under way wait for it.
pub async fn read() -> HostLoad {
    let mut cached = CACHED.lock().await;
    if let Some((at, load)) = *cached
        && at.elapsed() < CACHE_FOR
    {
        return load;
    }
    let load = sample().await;
    *cached = Some((Instant::now(), load));
    load
}

/// Reads the load now.
async fn sample() -> HostLoad {
    let cpu = tokio::task::spawn_blocking(cpu).await.unwrap_or_default();
    let mut memory = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    memory.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    let total = memory.total_memory();
    let mut available = memory.free_memory();
    if cfg!(target_os = "linux") {
        if let Some(read) = tokio::fs::read_to_string("/proc/meminfo")
            .await
            .ok()
            .and_then(|text| linux_available_memory(&text))
        {
            available = read;
        }
    } else if cfg!(target_os = "macos")
        && let Some(read) = vm_stat()
            .await
            .and_then(|text| darwin_available_memory(&text))
    {
        available = read;
    }
    HostLoad {
        sampled_at: Timestamp::now(),
        cpu_util: cpu.util.map(super::finite),
        cpu_count: cpu.count,
        mem_available: available.min(total),
        mem_total: total,
    }
}

#[derive(Default)]
struct Cpu {
    util: Option<f32>,
    count: u32,
}

/// Two readings `CPU_GAP` apart. Blocks for that long.
fn cpu() -> Cpu {
    let kind = CpuRefreshKind::nothing().with_cpu_usage();
    let mut system = System::new_with_specifics(RefreshKind::nothing().with_cpu(kind));
    let before = system.cpus().len();
    std::thread::sleep(CPU_GAP.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
    system.refresh_cpu_specifics(kind);
    let count = system.cpus().len();
    let usage = system.global_cpu_usage();
    let util = (before == count && count > 0 && usage.is_finite())
        .then(|| (usage / 100.0).clamp(0.0, 1.0));
    Cpu {
        util,
        count: u32::try_from(count).unwrap_or(u32::MAX),
    }
}

/// Runs `/usr/bin/vm_stat`, or gives none if it fails or is slow.
async fn vm_stat() -> Option<String> {
    let run = tokio::process::Command::new("/usr/bin/vm_stat")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(VM_STAT_WAIT, run).await.ok()?.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
fn linux_available_memory(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        let kb = rest.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()?;
        kb.checked_mul(1024)
    })
}

/// Free, inactive and speculative pages from `vm_stat`, in bytes.
fn darwin_available_memory(output: &str) -> Option<u64> {
    let page_size = output
        .split("page size of ")
        .nth(1)?
        .split(' ')
        .next()?
        .parse::<u64>()
        .ok()
        .filter(|size| *size > 0)?;
    let pages = |name: &str| -> Option<u64> {
        output.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.trim().strip_suffix('.')?.parse::<u64>().ok()
        })
    };
    // vm_stat leaves speculative pages out of free; add them once.
    let free = pages("Pages free")?;
    let inactive = pages("Pages inactive")?;
    let speculative = pages("Pages speculative")?;
    free.checked_add(inactive)?
        .checked_add(speculative)?
        .checked_mul(page_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_vm_stat() {
        let output = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                      Pages free:                               10.\n\
                      Pages active:                            999.\n\
                      Pages inactive:                           20.\n\
                      Pages speculative:                         5.\n\
                      Pages purgeable:                         100.\n";
        assert_eq!(darwin_available_memory(output), Some(35 * 16384));
        assert_eq!(darwin_available_memory("Pages free: 10.\n"), None);
    }

    #[test]
    fn reads_meminfo() {
        let meminfo = "MemTotal:       16000000 kB\nMemFree:         1000 kB\n\
                       MemAvailable:    8000000 kB\n";
        assert_eq!(linux_available_memory(meminfo), Some(8_000_000 * 1024));
        assert_eq!(linux_available_memory("MemFree: 1 kB\n"), None);
    }

    #[tokio::test]
    async fn reads_this_device_and_caches_it() {
        let first = read().await;
        assert!(first.cpu_count > 0);
        assert!(first.mem_total > 0);
        assert!(first.mem_available <= first.mem_total);
        if let Some(util) = first.cpu_util {
            assert!((0.0..=1.0).contains(&util));
        }
        assert_eq!(read().await, first);
    }
}
