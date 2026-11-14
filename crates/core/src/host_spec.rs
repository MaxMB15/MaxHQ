// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The host spec, `maxhq-host.toml`: what a host MaxHQ manages should
//! have. maxd reads it, and the NixOS module reads the same file with
//! `lib.importTOML` and renders it as JSON, which reads here too. A host
//! with no spec gets [`HostSpec::default`], the same as an empty file.
//!
//! It describes the host only. Workspaces stay things maxd makes through
//! the Incus API.

use std::net::Ipv4Addr;

use maxhq_protocol::model::Tool;
use maxhq_protocol::state::{HelperTool, ToolKind};
use serde::Deserialize;

/// The spec's file name.
pub const FILE: &str = "maxhq-host.toml";

/// The only spec version there is.
pub const VERSION: u32 = 1;

/// The tools the system part may name, since they need root.
pub const SYSTEM_TOOLS: &[&str] = &["git", "incus"];

/// A whole host spec.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostSpec {
    /// The spec's format, [`VERSION`].
    pub version: u32,
    /// Who runs maxd and where it keeps its data.
    pub service: Service,
    /// Incus, its pool and its bridge.
    pub incus: Incus,
    /// The limits all workspaces share, and each one's own.
    pub caps: Caps,
    /// Incus's own snapshot schedule for each workspace.
    pub snapshots: Snapshots,
    /// What happens when the host shuts down.
    pub shutdown: Shutdown,
    /// The tools MaxHQ installs, and the ones that need root.
    pub tools: Tools,
}

impl Default for HostSpec {
    fn default() -> Self {
        Self {
            version: VERSION,
            service: Service::default(),
            incus: Incus::default(),
            caps: Caps::default(),
            snapshots: Snapshots::default(),
            shutdown: Shutdown::default(),
            tools: Tools::default(),
        }
    }
}

/// Who runs maxd and where it keeps its data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Service {
    /// The user maxd runs as. None for whoever runs it.
    pub user: Option<String>,
    /// Where maxd keeps its secrets and resume flags. None for its usual
    /// data folder.
    pub data_dir: Option<String>,
}

/// Incus, its pool and its bridge.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Incus {
    /// Whether the host runs Incus for container workspaces.
    pub enable: bool,
    /// The storage pool workspaces go on. None for the one maxd finds.
    pub pool: Option<Pool>,
    /// The bridge workspaces join. None for the one maxd finds.
    pub bridge: Option<Bridge>,
}

impl Default for Incus {
    fn default() -> Self {
        Self {
            enable: true,
            pool: None,
            bridge: None,
        }
    }
}

/// An Incus storage pool.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    /// Its name in Incus.
    pub name: String,
    /// Its driver: `btrfs`, `zfs`, `lvm` or `dir`.
    #[serde(default = "btrfs")]
    pub driver: String,
    /// The image file it lives in, for a loop-backed pool.
    #[serde(default)]
    pub image: Option<String>,
    /// How big it's made.
    #[serde(default)]
    pub size: Option<Size>,
    /// A mount that must be there before the pool is used.
    #[serde(default)]
    pub needs_mount: Option<String>,
}

fn btrfs() -> String {
    "btrfs".to_owned()
}

/// An Incus bridge.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bridge {
    /// Its name, which is also the network interface's.
    pub name: String,
    /// Its own address and prefix, such as `10.10.0.1/24`. None for what
    /// Incus picks.
    #[serde(default)]
    pub subnet: Option<String>,
}

/// The limits all workspaces share, and each one's own. Each is soft, so
/// no limit here picks a process to kill.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Caps {
    /// The cgroup every workspace runs under, below the root. None runs
    /// them where Incus puts them, with no shared limits.
    pub parent: Option<String>,
    /// Whether maxd's units make the parent and set its limits. Off while
    /// something else on the host owns it.
    pub manage_parent: bool,
    /// The parent's `memory.high`.
    pub memory_high: Option<Size>,
    /// The parent's `cpu.max`, in CPUs.
    pub cpu_max: Option<u32>,
    /// The parent's `cpu.weight`, 1 to 10000. The host's services sit at
    /// 100.
    pub cpu_weight: Option<u32>,
    /// Each workspace's soft memory limit, unless its project sets one.
    pub workspace_memory: Option<Size>,
    /// The memory kept for the host's own services, as `MemoryLow`.
    pub system_memory_low: Option<Size>,
    /// Whether the host swaps to compressed memory.
    pub zram: bool,
}

/// Incus's own snapshot schedule for each workspace.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Snapshots {
    /// When Incus takes one: `@hourly`, `@daily` and the like, or five
    /// cron fields.
    pub schedule: String,
    /// How long Incus keeps each.
    pub expiry: Span,
}

impl Default for Snapshots {
    fn default() -> Self {
        Self {
            schedule: "@hourly".to_owned(),
            expiry: Span(7 * DAY),
        }
    }
}

/// What happens when the host shuts down.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shutdown {
    /// How long agents get to finish their turn before the workspaces stop.
    pub quiesce_window: Span,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self {
            quiesce_window: Span(20 * MINUTE),
        }
    }
}

/// The tools MaxHQ installs, and the ones that need root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tools {
    /// Tools MaxHQ installs for the service user, by name: an agent CLI
    /// such as `claude`, or a helper such as `node`.
    pub user: Vec<String>,
    /// Tools that need root, from [`SYSTEM_TOOLS`].
    pub system: Vec<String>,
}

impl Tools {
    /// The user tools as kinds. Names that aren't tools are left out,
    /// which [`HostSpec::check`] doesn't allow.
    pub fn user_kinds(&self) -> Vec<ToolKind> {
        self.user
            .iter()
            .filter_map(|name| tool_kind(name))
            .collect()
    }
}

/// The tool a name in `tools.user` stands for.
fn tool_kind(name: &str) -> Option<ToolKind> {
    let name = serde_json::Value::from(name);
    if let Ok(tool) = serde_json::from_value::<Tool>(name.clone()) {
        return Some(ToolKind::Agent { tool });
    }
    serde_json::from_value::<HelperTool>(name)
        .ok()
        .map(|tool| ToolKind::Helper { tool })
}

/// A size in bytes, written as a number and a unit such as `3GiB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Size(pub u64);

impl TryFrom<String> for Size {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        let unit = &text[digits.len()..];
        let scale: u64 = match unit {
            "B" => 1,
            "KiB" => 1 << 10,
            "MiB" => 1 << 20,
            "GiB" => 1 << 30,
            "TiB" => 1 << 40,
            _ => return Err(format!("{text:?} needs a unit: B, KiB, MiB, GiB or TiB")),
        };
        digits
            .parse::<u64>()
            .ok()
            .filter(|&n| n > 0)
            .and_then(|n| n.checked_mul(scale))
            .map(Size)
            .ok_or_else(|| format!("{text:?} isn't a size above zero"))
    }
}

impl std::fmt::Display for Size {
    /// The largest unit that divides it, such as `12GiB`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let units = [("TiB", 40), ("GiB", 30), ("MiB", 20), ("KiB", 10)];
        for (unit, shift) in units {
            if self.0 != 0 && self.0.is_multiple_of(1 << shift) {
                return write!(f, "{}{unit}", self.0 >> shift);
            }
        }
        write!(f, "{}B", self.0)
    }
}

const MINUTE: u64 = 60;
const DAY: u64 = 24 * 60 * MINUTE;

/// A length of time in seconds, written as a number and `s`, `m`, `h` or
/// `d`, such as `20m`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Span(pub u64);

impl TryFrom<String> for Span {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let (digits, unit) = text.split_at(text.len().saturating_sub(1));
        let scale = match unit {
            "s" => 1,
            "m" => MINUTE,
            "h" => 60 * MINUTE,
            "d" => DAY,
            _ => return Err(format!("{text:?} needs a unit: s, m, h or d")),
        };
        digits
            .parse::<u64>()
            .ok()
            .filter(|&n| n > 0)
            .and_then(|n| n.checked_mul(scale))
            .map(Span)
            .ok_or_else(|| format!("{text:?} isn't a time above zero"))
    }
}

/// Why a spec can't be used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpecError {
    /// It isn't TOML in the spec's shape.
    #[error("{FILE} doesn't read: {0}")]
    Unreadable(String),
    /// A key's value isn't allowed.
    #[error("{FILE} has {key} = {why}")]
    Invalid {
        /// The key, such as `caps.cpu_weight`.
        key: &'static str,
        /// What's wrong with it.
        why: String,
    },
}

impl HostSpec {
    /// Reads a spec from `maxhq-host.toml`'s text and checks it.
    pub fn from_toml(text: &str) -> Result<Self, SpecError> {
        let spec: Self = toml::from_str(text).map_err(|e| SpecError::Unreadable(e.to_string()))?;
        spec.check()?;
        Ok(spec)
    }

    /// Reads a spec from the JSON the NixOS module renders, and checks it.
    pub fn from_json(text: &str) -> Result<Self, SpecError> {
        let spec: Self =
            serde_json::from_str(text).map_err(|e| SpecError::Unreadable(e.to_string()))?;
        spec.check()?;
        Ok(spec)
    }

    /// Checks what the types alone don't.
    pub fn check(&self) -> Result<(), SpecError> {
        let invalid = |key, why: String| Err(SpecError::Invalid { key, why });
        if self.version != VERSION {
            return invalid(
                "version",
                format!("{}, and only {VERSION} is known", self.version),
            );
        }
        if let Some(user) = &self.service.user {
            check_name("service.user", user, 32)?;
        }
        if let Some(dir) = &self.service.data_dir {
            check_path("service.data_dir", dir)?;
        }
        if let Some(pool) = &self.incus.pool {
            check_name("incus.pool.name", &pool.name, 63)?;
            if !["btrfs", "zfs", "lvm", "dir"].contains(&pool.driver.as_str()) {
                return invalid(
                    "incus.pool.driver",
                    format!("{:?}, not btrfs, zfs, lvm or dir", pool.driver),
                );
            }
            if let Some(image) = &pool.image {
                check_path("incus.pool.image", image)?;
            }
            if let Some(mount) = &pool.needs_mount {
                check_path("incus.pool.needs_mount", mount)?;
            }
        }
        if let Some(bridge) = &self.incus.bridge {
            // A network interface's name takes at most 15 bytes.
            check_name("incus.bridge.name", &bridge.name, 15)?;
            if let Some(subnet) = &bridge.subnet {
                check_subnet(subnet)?;
            }
        }
        if let Some(parent) = &self.caps.parent {
            check_cgroup(parent)?;
        } else if self.caps.manage_parent {
            return invalid(
                "caps.manage_parent",
                "true with no caps.parent to manage".to_owned(),
            );
        }
        if self.caps.cpu_max == Some(0) {
            return invalid("caps.cpu_max", "0, and it needs at least 1".to_owned());
        }
        if let Some(weight) = self.caps.cpu_weight
            && !(1..=10_000).contains(&weight)
        {
            return invalid("caps.cpu_weight", format!("{weight}, outside 1 to 10000"));
        }
        if !valid_schedule(&self.snapshots.schedule) {
            return invalid(
                "snapshots.schedule",
                format!(
                    "{:?}, not @hourly, @daily, @weekly, @monthly or five cron fields",
                    self.snapshots.schedule
                ),
            );
        }
        if let Some(name) = self
            .tools
            .user
            .iter()
            .find(|name| tool_kind(name).is_none())
        {
            return invalid(
                "tools.user",
                format!("{name:?}, which isn't a tool MaxHQ installs"),
            );
        }
        if let Some(name) = self
            .tools
            .system
            .iter()
            .find(|name| !SYSTEM_TOOLS.contains(&name.as_str()))
        {
            return invalid(
                "tools.system",
                format!("{name:?}, not one of {}", SYSTEM_TOOLS.join(", ")),
            );
        }
        Ok(())
    }
}

/// A user, pool or interface name: a lowercase letter first, then
/// lowercase letters, digits, `-` and `_`.
fn check_name(key: &'static str, name: &str, most: usize) -> Result<(), SpecError> {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_ok =
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if first_ok && rest_ok && name.len() <= most {
        return Ok(());
    }
    Err(SpecError::Invalid {
        key,
        why: format!("{name:?}, not a name of at most {most} lowercase letters, digits, - and _"),
    })
}

/// An absolute path with no `..` in it.
fn check_path(key: &'static str, path: &str) -> Result<(), SpecError> {
    if path.starts_with('/') && !path.split('/').any(|part| part == "..") {
        return Ok(());
    }
    Err(SpecError::Invalid {
        key,
        why: format!("{path:?}, not an absolute path"),
    })
}

/// A cgroup below the root: names of letters, digits, `-`, `_` and `.`,
/// split by `/`, with none of them `.` or `..`.
fn check_cgroup(parent: &str) -> Result<(), SpecError> {
    let part_ok = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    if parent.split('/').all(part_ok) {
        return Ok(());
    }
    Err(SpecError::Invalid {
        key: "caps.parent",
        why: format!("{parent:?}, not a cgroup path below the root"),
    })
}

/// An IPv4 address and a prefix from 8 to 30.
fn check_subnet(subnet: &str) -> Result<(), SpecError> {
    let ok = subnet.split_once('/').is_some_and(|(address, prefix)| {
        address.parse::<Ipv4Addr>().is_ok()
            && prefix.parse::<u8>().is_ok_and(|p| (8..=30).contains(&p))
    });
    if ok {
        return Ok(());
    }
    Err(SpecError::Invalid {
        key: "incus.bridge.subnet",
        why: format!("{subnet:?}, not an IPv4 address and a prefix such as 10.10.0.1/24"),
    })
}

/// A schedule Incus takes.
fn valid_schedule(schedule: &str) -> bool {
    match schedule.strip_prefix('@') {
        Some(name) => {
            ["hourly", "daily", "weekly", "monthly", "annually", "yearly"].contains(&name)
        }
        None => {
            let fields: Vec<&str> = schedule.split_whitespace().collect();
            fields.len() == 5
                && fields.iter().all(|field| {
                    field
                        .chars()
                        .all(|c| c.is_ascii_digit() || "*/,-".contains(c))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::model::Tool;

    use super::*;

    /// The tracked example, which the NixOS module's test reads too.
    const EXAMPLE: &str = include_str!("../../../nix/maxhq-host.example.toml");

    #[test]
    fn an_empty_file_is_the_defaults() {
        let spec = HostSpec::from_toml("").unwrap();
        assert_eq!(spec, HostSpec::default());
        assert!(spec.incus.enable && spec.caps.parent.is_none());
        assert_eq!(spec.snapshots.schedule, "@hourly");
        assert_eq!(spec.snapshots.expiry, Span(7 * DAY));
        assert_eq!(spec.shutdown.quiesce_window, Span(20 * 60));
        assert_eq!(HostSpec::from_json("{}").unwrap(), spec);
    }

    #[test]
    fn the_example_reads() {
        let spec = HostSpec::from_toml(EXAMPLE).unwrap();
        assert_eq!(spec.service.user.as_deref(), Some("dev"));
        let pool = spec.incus.pool.as_ref().unwrap();
        assert_eq!(pool.size, Some(Size(100 << 30)));
        assert_eq!(spec.incus.bridge.as_ref().unwrap().name, "agentbr0");
        assert_eq!(spec.caps.workspace_memory, Some(Size(3 << 30)));
        assert_eq!(spec.caps.cpu_weight, Some(50));
        assert_eq!(
            spec.tools.user_kinds(),
            [
                ToolKind::Agent { tool: Tool::Claude },
                ToolKind::Agent { tool: Tool::Codex },
                ToolKind::Agent { tool: Tool::Cursor },
                ToolKind::Helper {
                    tool: HelperTool::Node
                },
            ]
        );
    }

    #[test]
    fn the_rendered_json_reads_the_same() {
        let toml: toml::Table = toml::from_str(EXAMPLE).unwrap();
        let json = serde_json::to_string(&toml).unwrap();
        assert_eq!(
            HostSpec::from_json(&json).unwrap(),
            HostSpec::from_toml(EXAMPLE).unwrap()
        );
    }

    #[test]
    fn sizes_and_times_need_units() {
        assert_eq!(Size::try_from("512MiB".to_owned()), Ok(Size(512 << 20)));
        assert!(Size::try_from("3G".to_owned()).is_err());
        assert!(Size::try_from("0GiB".to_owned()).is_err());
        assert!(Size::try_from("GiB".to_owned()).is_err());
        assert_eq!(Size(12 << 30).to_string(), "12GiB");
        assert_eq!(Size(1536 << 20).to_string(), "1536MiB");
        assert_eq!(Size(100).to_string(), "100B");
        assert_eq!(Span::try_from("90s".to_owned()), Ok(Span(90)));
        assert!(Span::try_from("20".to_owned()).is_err());
        assert!(Span::try_from("".to_owned()).is_err());
    }

    #[test]
    fn bad_values_name_their_key() {
        for (text, key) in [
            ("version = 2", "version"),
            ("[caps]\ncpu_weight = 0", "caps.cpu_weight"),
            ("[caps]\ncpu_max = 0", "caps.cpu_max"),
            ("[caps]\nmanage_parent = true", "caps.manage_parent"),
            ("[caps]\nparent = \"../escape\"", "caps.parent"),
            ("[service]\nuser = \"Dev\"", "service.user"),
            ("[service]\ndata_dir = \"srv/maxhq\"", "service.data_dir"),
            (
                "[incus]\nbridge = { name = \"agentbr0\", subnet = \"10.10.0.1\" }",
                "incus.bridge.subnet",
            ),
            (
                "[incus]\nbridge = { name = \"a-very-long-bridge\" }",
                "incus.bridge.name",
            ),
            (
                "[incus]\npool = { name = \"agents\", driver = \"ceph\" }",
                "incus.pool.driver",
            ),
            ("[snapshots]\nschedule = \"often\"", "snapshots.schedule"),
            ("[tools]\nuser = [\"vim\"]", "tools.user"),
            ("[tools]\nsystem = [\"kvm\"]", "tools.system"),
        ] {
            match HostSpec::from_toml(text) {
                Err(SpecError::Invalid { key: got, .. }) => assert_eq!(got, key, "{text}"),
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_keys_and_bad_units_dont_read() {
        for text in [
            "[backup]\nexport = \"/srv/backup\"",
            "[caps]\nmemory_high = \"12G\"",
            "[shutdown]\nquiesce_window = \"soon\"",
        ] {
            assert!(
                matches!(HostSpec::from_toml(text), Err(SpecError::Unreadable(_))),
                "{text}"
            );
        }
    }
}
