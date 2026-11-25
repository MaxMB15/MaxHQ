// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What this device's hardware means for running a model: its chip, its
//! memory and how much of it the GPU may use. macOS lets the GPU wire
//! about two thirds of memory on small Macs and three quarters on larger
//! ones, unless someone set `iogpu.wired_limit_mb`. `maxd` never changes
//! that limit; the views explain it. A Mac's real figure comes from Metal,
//! which llama-server prints when it loads a model.

use std::path::Path;
use std::process::{Command, Stdio};

use maxhq_protocol::local_models::{DeviceHardware, GpuBudgetSource, GpuMemory, RuntimeSupport};

const GIB: u64 = 1 << 30;

/// This device's hardware, read once at start.
pub fn read() -> DeviceHardware {
    let ram = memory();
    let gpu = managed().then(|| budget(ram, wired_limit_mb()));
    DeviceHardware {
        chip: chip(),
        gpu_cores: gpu_cores(),
        ram,
        gpu,
    }
}

/// Whether this device runs a managed server: an Apple silicon Mac, which
/// llama.cpp's Metal build serves.
pub fn managed() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// What this device supports for local models.
pub fn support() -> RuntimeSupport {
    if managed() {
        RuntimeSupport::Managed
    } else {
        RuntimeSupport::NoGpu
    }
}

/// The GPU's budget from the memory size and a wired limit in MiB, where 0
/// or none means macOS's default.
pub fn budget(ram: u64, wired_limit_mb: Option<u64>) -> GpuMemory {
    match wired_limit_mb.filter(|mb| *mb > 0) {
        Some(mb) => GpuMemory {
            budget: (mb << 20).min(ram),
            source: GpuBudgetSource::WiredLimit,
        },
        None => GpuMemory {
            budget: if ram <= 16 * GIB {
                ram / 3 * 2
            } else {
                ram / 4 * 3
            },
            source: GpuBudgetSource::Estimate,
        },
    }
}

/// The budget Metal gives, from the line `llama-server --list-devices`
/// prints for the GPU, such as `MTL0: Apple M4 Max (28753 MiB, 28753 MiB
/// free)`.
pub fn metal_budget(line: &str) -> Option<GpuMemory> {
    let (device, rest) = line.trim().split_once(':')?;
    if !device.starts_with("MTL") {
        return None;
    }
    let (_, sizes) = rest.rsplit_once('(')?;
    let mib: u64 = sizes
        .strip_suffix(')')?
        .split(" MiB")
        .next()?
        .trim()
        .parse()
        .ok()?;
    (mib > 0).then_some(GpuMemory {
        budget: mib << 20,
        source: GpuBudgetSource::Metal,
    })
}

/// Bytes free for this user on the disk that holds `path`, or on its
/// nearest folder that exists.
pub fn disk_free(path: &Path) -> u64 {
    let mut at = Some(path);
    while let Some(dir) = at {
        if let Some(free) = free_on(dir) {
            return free;
        }
        at = dir.parent();
    }
    0
}

fn free_on(dir: &Path) -> Option<u64> {
    maxhq_os::disk::free_space(dir).ok()
}

fn memory() -> u64 {
    if cfg!(target_os = "macos") {
        sysctl("hw.memsize")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    } else {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| meminfo_total(&text))
            .unwrap_or(0)
    }
}

/// `MemTotal` from `/proc/meminfo`, in bytes.
fn meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

fn chip() -> Option<String> {
    if cfg!(target_os = "macos") {
        sysctl("machdep.cpu.brand_string")
    } else {
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find(|l| l.starts_with("model name"))
                    .and_then(|l| l.split_once(':'))
                    .map(|(_, name)| name.trim().to_owned())
            })
    }
}

fn wired_limit_mb() -> Option<u64> {
    sysctl("iogpu.wired_limit_mb").and_then(|v| v.parse().ok())
}

/// The GPU's core count, from the accelerator's registry entry.
fn gpu_cores() -> Option<u16> {
    if !managed() {
        return None;
    }
    let text = run("/usr/sbin/ioreg", &["-rc", "AGXAccelerator", "-d", "1"])?;
    text.lines()
        .find(|l| l.contains("\"gpu-core-count\""))
        .and_then(|l| l.rsplit('=').next())
        .and_then(|v| v.trim().parse().ok())
}

fn sysctl(name: &str) -> Option<String> {
    run("/usr/sbin/sysctl", &["-n", name])
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_macos_default_share_unless_a_limit_is_set() {
        let mac = budget(36 * GIB, Some(0));
        assert_eq!(mac.budget, 27 * GIB);
        assert_eq!(mac.source, GpuBudgetSource::Estimate);
        assert_eq!(budget(16 * GIB, None).budget, 16 * GIB / 3 * 2);
        let set = budget(36 * GIB, Some(30_720));
        assert_eq!(set.budget, 30 * GIB);
        assert_eq!(set.source, GpuBudgetSource::WiredLimit);
        // A limit above the memory size counts as the memory size.
        assert_eq!(budget(8 * GIB, Some(65_536)).budget, 8 * GIB);
    }

    #[test]
    fn metal_gives_the_real_budget_when_llama_server_lists_it() {
        let line = "  MTL0: Apple M4 Max (28753 MiB, 28753 MiB free)";
        let gpu = metal_budget(line).unwrap();
        assert_eq!(gpu.budget, 28_753 << 20);
        assert_eq!(gpu.source, GpuBudgetSource::Metal);
        assert_eq!(metal_budget("Available devices:"), None);
        assert_eq!(
            metal_budget("  CPU: Apple M4 Max (36864 MiB, 20000 MiB free)"),
            None
        );
        assert_eq!(metal_budget("  MTL0: Apple M4 Max (no memory)"), None);
    }

    #[test]
    fn linux_memory_comes_from_meminfo() {
        let text = "MemTotal:       16303428 kB\nMemFree:         1234 kB\n";
        assert_eq!(meminfo_total(text), Some(16_303_428 * 1024));
    }

    #[test]
    fn free_disk_falls_back_to_the_nearest_folder_that_exists() {
        let free = disk_free(&std::env::temp_dir().join("maxhq-no-such-folder/models/a/b"));
        assert!(free > 0);
    }
}
