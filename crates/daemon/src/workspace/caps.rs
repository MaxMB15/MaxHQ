// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! W-8's caps on container workspaces: each one runs under the host
//! spec's parent cgroup through its own `raw.lxc`, with a soft memory
//! limit and no CPU limit of its own. The parent holds the shared limits,
//! which the host's own units set. maxd only checks they're there before a
//! start, and writes a container's keys while it's stopped, so they take
//! effect at its next start and nothing restarts to apply them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use maxhq_core::host_spec::{self, Size};
use tracing::warn;

use crate::incus::Instance;

/// Where cgroup v2 is mounted.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// The keys every workspace has, whatever the caps.
pub(super) const BASE: &[(&str, &str)] = &[
    ("security.idmap.isolated", "true"),
    ("limits.memory.enforce", "soft"),
    // A-11 starts workspaces at boot, so Incus doesn't.
    ("boot.autostart", "false"),
];

// The keys the caps set. An empty value takes a key off, as M3's CPU
// limit goes.
const RAW_LXC: &str = "raw.lxc";
const MEMORY: &str = "limits.memory";
const CPU: &str = "limits.cpu";

/// The keys W-10's snapshots set: Incus takes one on the spec's schedule
/// while the container runs, names it by the time, and lets it expire.
pub(super) const SNAPSHOT_KEYS: &str = "snapshots.";
const SCHEDULE: &str = "snapshots.schedule";
const EXPIRY: &str = "snapshots.expiry";
const STOPPED: &str = "snapshots.schedule.stopped";
const PATTERN: &str = "snapshots.pattern";
const PATTERN_VALUE: &str = "auto-{{ creation_date|date:'2006-01-02-1504' }}";

/// M3's guard, which a host whose spec names no parent keeps.
const GUARD_CPU: &str = "4";
const GUARD_MEMORY: u64 = 8 << 30;

/// The answer when the parent cgroup isn't there.
const NO_PARENT: &str = "The workspace cgroup isn't set up on this device. Run maxd host check.";

/// The host spec's caps and snapshot schedule, and where to find the
/// cgroups the caps name.
#[derive(Clone, Debug)]
pub struct Caps {
    /// What the host spec says.
    pub spec: host_spec::Caps,
    /// The spec's snapshot schedule.
    pub snapshots: host_spec::Snapshots,
    /// Where cgroup v2 is mounted, [`CGROUP_ROOT`] outside tests.
    pub root: PathBuf,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            spec: host_spec::Caps::default(),
            snapshots: host_spec::Snapshots::default(),
            root: PathBuf::from(CGROUP_ROOT),
        }
    }
}

impl Caps {
    /// The caps and snapshot schedule in this host's spec, as
    /// [`crate::host::load`] finds it. With no spec the defaults name no
    /// parent and so keep M3's guard. A spec that doesn't read gets the
    /// defaults too, and `maxd host check` says why.
    pub fn read() -> Self {
        let (spec, snapshots) = crate::host::Env::this_device()
            .and_then(|env| crate::host::load(&env))
            .map_err(|error| warn!(%error, "the host spec doesn't read, so M3's guard applies"))
            .map(|loaded| (loaded.spec.caps, loaded.spec.snapshots))
            .unwrap_or_default();
        Self {
            spec,
            snapshots,
            ..Self::default()
        }
    }

    /// The soft memory limit for a workspace in a project that asks for
    /// `project` bytes, else the spec's default, else M3's guard while the
    /// spec names no parent.
    pub(super) fn memory(&self, project: Option<u64>) -> Option<u64> {
        project
            .or(self.spec.workspace_memory.map(|size| size.0))
            .or(self.spec.parent.is_none().then_some(GUARD_MEMORY))
    }

    /// The keys a container named `instance` gets, with `memory` from
    /// [`Caps::memory`]. Under a parent it runs in the parent's cgroup
    /// with no CPU limit of its own. With no parent it keeps M3's guard,
    /// and its `raw.lxc` stays as it is. Either way it takes the spec's
    /// snapshots.
    pub(super) fn keys(&self, instance: &str, memory: Option<u64>) -> BTreeMap<String, String> {
        let memory = memory.map_or_else(String::new, incus_size);
        let snapshots = [
            (SCHEDULE, self.snapshots.schedule.clone()),
            (EXPIRY, incus_span(self.snapshots.expiry.0)),
            (STOPPED, "false".to_owned()),
            (PATTERN, PATTERN_VALUE.to_owned()),
        ];
        let mut keys = match self.spec.parent.as_deref() {
            Some(parent) => vec![
                (
                    RAW_LXC,
                    format!(
                        "lxc.cgroup.dir.container = {parent}/{instance}\n\
                         lxc.cgroup.dir.monitor = {parent}.monitor/{instance}\n"
                    ),
                ),
                (MEMORY, memory),
                (CPU, String::new()),
            ],
            None => vec![(MEMORY, memory), (CPU, GUARD_CPU.to_owned())],
        };
        keys.extend(snapshots);
        keys.into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    /// The keys a new container gets.
    pub(super) fn create_keys(
        &self,
        instance: &str,
        memory: Option<u64>,
    ) -> BTreeMap<String, String> {
        let mut keys: BTreeMap<String, String> = self
            .keys(instance, memory)
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .collect();
        for (key, value) in BASE {
            keys.insert((*key).to_owned(), (*value).to_owned());
        }
        keys
    }

    /// The keys from [`Caps::keys`] that `instance` doesn't have yet, so a
    /// container that has them all is left alone.
    pub(super) fn changes(
        &self,
        instance: &Instance,
        memory: Option<u64>,
    ) -> BTreeMap<String, String> {
        self.keys(&instance.name, memory)
            .into_iter()
            .filter(|(key, value)| instance.config.get(key).map_or("", String::as_str) != value)
            .collect()
    }

    /// Why a container can't start under the parent cgroup now. Fine when
    /// it can, or when the spec names no parent.
    pub(super) fn check_parent(&self) -> Result<(), String> {
        let Some(parent) = &self.spec.parent else {
            return Ok(());
        };
        let dir = self.root.join(parent);
        if !dir.is_dir() {
            return Err(NO_PARENT.to_owned());
        }
        let missing = |file: &str, unset: &str| match read(&dir.join(file)) {
            Some(value) => value.split_whitespace().next() == Some(unset),
            None => true,
        };
        let limits = [
            ("memory.high", self.spec.memory_high.is_some(), "max"),
            ("cpu.max", self.spec.cpu_max.is_some(), "max"),
            ("cpu.weight", self.spec.cpu_weight.is_some(), ""),
        ];
        for (file, wanted, unset) in limits {
            if wanted && missing(file, unset) {
                return Err(format!(
                    "The workspace cgroup on this device has no {file} limit. Run maxd host check."
                ));
            }
        }
        Ok(())
    }
}

/// The soft memory limit an instance's keys set, in bytes.
pub(super) fn memory_of(instance: &Instance) -> Option<u64> {
    let value = instance.key(MEMORY)?;
    Size::try_from(value.to_owned()).ok().map(|size| size.0)
}

/// Bytes as Incus takes them, in the largest unit that holds them whole.
fn incus_size(bytes: u64) -> String {
    for (unit, scale) in [
        ("TiB", 1u64 << 40),
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
    ] {
        if bytes.is_multiple_of(scale) {
            return format!("{}{unit}", bytes / scale);
        }
    }
    format!("{bytes}B")
}

/// Seconds as Incus's snapshot expiry takes them, in the largest unit that
/// holds them whole. Incus counts no finer than a minute, so a part minute
/// counts as one.
fn incus_span(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60).max(1);
    for (unit, scale) in [("d", 24 * 60), ("H", 60)] {
        if minutes.is_multiple_of(scale) {
            return format!("{}{unit}", minutes / scale);
        }
    }
    format!("{minutes}M")
}

/// A cgroup file's text, trimmed.
fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    const GIB: u64 = 1 << 30;

    fn caps(root: &Path) -> Caps {
        Caps {
            spec: host_spec::Caps {
                parent: Some("agent-workspaces".to_owned()),
                memory_high: Some(Size(12 * GIB)),
                cpu_max: Some(8),
                cpu_weight: Some(50),
                workspace_memory: Some(Size(3 * GIB)),
                ..host_spec::Caps::default()
            },
            snapshots: host_spec::Snapshots::default(),
            root: root.to_owned(),
        }
    }

    fn parent(root: &Path, files: &[(&str, &str)]) {
        let dir = root.join("agent-workspaces");
        std::fs::create_dir_all(&dir).unwrap();
        for (file, value) in files {
            std::fs::write(dir.join(file), format!("{value}\n")).unwrap();
        }
    }

    #[test]
    fn sizes_go_in_the_largest_whole_unit() {
        assert_eq!(incus_size(3 * GIB), "3GiB");
        assert_eq!(incus_size(1536 << 20), "1536MiB");
        assert_eq!(incus_size(1000), "1000B");
        assert_eq!(incus_size(2 << 40), "2TiB");
    }

    #[test]
    fn spans_go_in_the_largest_whole_unit() {
        assert_eq!(incus_span(7 * 24 * 3600), "7d");
        assert_eq!(incus_span(3 * 24 * 3600), "3d");
        assert_eq!(incus_span(36 * 3600), "36H");
        assert_eq!(incus_span(90 * 60), "90M");
        assert_eq!(incus_span(30), "1M");
    }

    #[test]
    fn every_workspace_takes_the_spec_s_snapshots() {
        let dir = TempDir::new();
        for caps in [caps(dir.path()), Caps::default()] {
            let keys = caps.create_keys("mhq-maxhq", caps.memory(None));
            assert_eq!(keys["snapshots.schedule"], "@hourly");
            assert_eq!(keys["snapshots.expiry"], "7d");
            assert_eq!(keys["snapshots.schedule.stopped"], "false");
            assert_eq!(
                keys["snapshots.pattern"],
                "auto-{{ creation_date|date:'2006-01-02-1504' }}"
            );
        }
        let caps = Caps {
            snapshots: host_spec::Snapshots {
                schedule: "@daily".to_owned(),
                expiry: host_spec::Span(2 * 24 * 3600),
            },
            ..Caps::default()
        };
        let keys = caps.keys("mhq-maxhq", None);
        assert_eq!(keys["snapshots.schedule"], "@daily");
        assert_eq!(keys["snapshots.expiry"], "2d");
    }

    #[test]
    fn a_project_s_memory_wins_over_the_spec_s() {
        let dir = TempDir::new();
        let caps = caps(dir.path());
        assert_eq!(caps.memory(None), Some(3 * GIB));
        assert_eq!(caps.memory(Some(8 * GIB)), Some(8 * GIB));
        // With no parent, M3's 8 GiB guard, unless a project or the spec
        // asks for another.
        let guard = Caps::default();
        assert_eq!(guard.memory(None), Some(8 * GIB));
        assert_eq!(guard.memory(Some(2 * GIB)), Some(2 * GIB));
    }

    #[test]
    fn the_keys_name_the_parent_and_take_the_cpu_limit_off() {
        let dir = TempDir::new();
        let keys = caps(dir.path()).create_keys("mhq-maxhq", Some(3 * GIB));
        assert_eq!(
            keys["raw.lxc"],
            "lxc.cgroup.dir.container = agent-workspaces/mhq-maxhq\n\
             lxc.cgroup.dir.monitor = agent-workspaces.monitor/mhq-maxhq\n"
        );
        assert_eq!(keys["limits.memory"], "3GiB");
        assert_eq!(keys["limits.memory.enforce"], "soft");
        assert!(!keys.contains_key("limits.cpu"));

        // Under a parent, an old CPU limit comes off.
        let changed = caps(dir.path()).keys("mhq-maxhq", Some(3 * GIB));
        assert_eq!(changed["limits.cpu"], "");
    }

    #[test]
    fn with_no_parent_the_keys_are_m3_s_guard() {
        let guard = Caps::default();
        let keys = guard.create_keys("mhq-maxhq", guard.memory(None));
        assert_eq!(keys["limits.cpu"], "4");
        assert_eq!(keys["limits.memory"], "8GiB");
        assert_eq!(keys["limits.memory.enforce"], "soft");
        assert!(!keys.contains_key("raw.lxc"));

        // A project's memory replaces only the 8 GiB.
        let keys = guard.keys("mhq-maxhq", guard.memory(Some(2 * GIB)));
        assert_eq!(keys["limits.memory"], "2GiB");
        assert_eq!(keys["limits.cpu"], "4");
        assert!(!keys.contains_key("raw.lxc"));
    }

    #[test]
    fn a_start_needs_the_parent_and_its_limits() {
        let dir = TempDir::new();
        let caps = caps(dir.path());
        assert_eq!(caps.check_parent().unwrap_err(), NO_PARENT);

        parent(
            dir.path(),
            &[
                ("memory.high", "max"),
                ("cpu.max", "800000 100000"),
                ("cpu.weight", "50"),
            ],
        );
        let error = caps.check_parent().unwrap_err();
        assert!(error.contains("memory.high"), "{error}");

        parent(
            dir.path(),
            &[("memory.high", "12884901888"), ("cpu.max", "max 100000")],
        );
        let error = caps.check_parent().unwrap_err();
        assert!(error.contains("cpu.max"), "{error}");

        parent(dir.path(), &[("cpu.max", "800000 100000")]);
        assert_eq!(caps.check_parent(), Ok(()));

        // No parent in the spec, so nothing to check.
        assert_eq!(Caps::default().check_parent(), Ok(()));
    }
}
