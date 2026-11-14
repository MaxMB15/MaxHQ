// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Shared test hosts. You pick which devices host each [`TestKind`], and
//! each one's `maxd` publishes a [`TestHost`] per kind and backend, with
//! what it found missing. An agent on that device leases one by kind and
//! drives it through the lease tools, and each [`TestLease`] is published
//! while it lasts. Leases never cross devices: an agent elsewhere starts an
//! agent on the hosting device instead.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::agents::Actor;
use crate::ids::{DeviceId, LeaseId, PermissionId, TestHostId};
use crate::time::Timestamp;

/// How long a lease lasts when the agent doesn't say, in minutes.
pub const DEFAULT_LEASE_MINUTES: u16 = 30;

/// The longest lease an agent may ask for, in minutes.
pub const LONGEST_LEASE_MINUTES: u16 = 240;

/// How long a lease may go without a call before it ends, in minutes.
pub const IDLE_LEASE_MINUTES: u16 = 10;

/// The most macOS VMs one Mac may run at once, as Apple's licence allows.
pub const MAX_VMS: u8 = 2;

/// How many of a lease's last commands it keeps for the app.
pub const RECENT_COMMANDS: usize = 20;

/// A kind of test machine.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Type,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
    /// A Linux container.
    Linux,
    /// A macOS VM.
    Macos,
    /// An iOS Simulator.
    IosSimulator,
    /// An Android emulator.
    AndroidEmulator,
}

impl TestKind {
    /// Every kind, in the order the app lists them.
    pub const ALL: [TestKind; 4] = [
        TestKind::IosSimulator,
        TestKind::AndroidEmulator,
        TestKind::Linux,
        TestKind::Macos,
    ];

    /// Its name on the wire and in the lease tools, such as
    /// `ios_simulator`.
    pub fn name(self) -> &'static str {
        match self {
            TestKind::Linux => "linux",
            TestKind::Macos => "macos",
            TestKind::IosSimulator => "ios_simulator",
            TestKind::AndroidEmulator => "android_emulator",
        }
    }

    /// The kind with this name, if there is one.
    pub fn named(name: &str) -> Option<TestKind> {
        TestKind::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// The backends that can serve it, the one to prefer first.
    pub fn backends(self) -> &'static [TestHostBackend] {
        match self {
            TestKind::Linux => &[TestHostBackend::IncusTest],
            TestKind::Macos => &[TestHostBackend::MacVm],
            TestKind::IosSimulator => &[TestHostBackend::MacVm, TestHostBackend::MacHost],
            TestKind::AndroidEmulator => &[TestHostBackend::HostEmulator],
        }
    }
}

/// What runs a test machine, and so what keeps the app under test in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestHostBackend {
    /// An Incus container, `mhq-test-linux`, restored from a snapshot
    /// between leases.
    IncusTest,
    /// A macOS VM on the Mac, cloned from its base between leases.
    MacVm,
    /// The Mac's own Simulator, which you opt into. Apps it runs can read
    /// your home folder.
    MacHost,
    /// The Android emulator on the device itself, inside the emulator's
    /// own VM, booted from a clean snapshot each lease.
    HostEmulator,
}

impl TestHostBackend {
    /// Its name on the wire, such as `mac_vm`.
    pub fn name(self) -> &'static str {
        match self {
            TestHostBackend::IncusTest => "incus_test",
            TestHostBackend::MacVm => "mac_vm",
            TestHostBackend::MacHost => "mac_host",
            TestHostBackend::HostEmulator => "host_emulator",
        }
    }
}

/// The id of the test host for `kind` on `backend` on `device`. The same
/// three always give the same id.
pub fn host_id(device: DeviceId, kind: TestKind, backend: TestHostBackend) -> TestHostId {
    TestHostId::derive(&format!("{device}/{}/{}", kind.name(), backend.name()))
}

/// Where a test host stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestHostState {
    /// The device lacks something it needs, which [`TestHost::setup`]
    /// says.
    Unavailable,
    /// It has what it needs, and waits for you to build it.
    Off,
    /// It's being built.
    Building,
    /// It's ready for a lease.
    Warm,
    /// A lease holds it.
    Leased,
    /// It's going back to a clean state after a lease.
    Resetting,
    /// A build or reset failed, which [`TestHost::error`] says.
    Failed,
}

/// The VM behind a [`TestHostBackend::MacVm`] host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct VmSize {
    /// The base disk on the Mac, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub disk_bytes: u64,
    /// The memory it runs with, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub memory_bytes: u64,
}

/// One kind of test machine on one backend on one device, as that
/// device's `maxd` publishes it for each kind it hosts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TestHost {
    /// The host, from [`host_id`].
    pub id: TestHostId,
    /// The device it runs on.
    pub device: DeviceId,
    /// What it offers.
    pub kind: TestKind,
    /// What runs it.
    pub backend: TestHostBackend,
    /// Where it stands.
    pub state: TestHostState,
    /// What the device lacks, as one or two sentences, while it's
    /// unavailable.
    pub setup: Option<String>,
    /// Why the last build or reset failed, while it's failed.
    pub error: Option<String>,
    /// The agent-device version it drives with, once installed.
    pub agent_device: Option<String>,
    /// When it last went back to a clean state.
    pub last_reset: Option<Timestamp>,
    /// How many lease requests wait for it.
    pub queue: u32,
    /// The VM's size, for a [`TestHostBackend::MacVm`] host that has one.
    pub vm: Option<VmSize>,
}

/// What a device's `maxd` keeps about the test hosts it offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TestHostSettings {
    /// The kinds this device hosts. None unless you pick them.
    #[serde(default)]
    pub kinds: Vec<TestKind>,
    /// Lets iOS leases use the Mac's own Simulator. Off unless you turn it
    /// on.
    #[serde(default)]
    pub mac_host: bool,
    /// How many macOS VMs may run at once, 1 or [`MAX_VMS`].
    #[serde(default = "default_max_vms")]
    pub max_vms: u8,
    /// How long a lease lasts when the agent doesn't say, in minutes.
    #[serde(default = "default_minutes")]
    pub default_minutes: u16,
    /// The longest lease an agent may ask for, in minutes.
    #[serde(default = "longest_minutes")]
    pub longest_minutes: u16,
    /// How long a lease may go without a call before it ends, in minutes.
    #[serde(default = "idle_minutes")]
    pub idle_minutes: u16,
}

fn default_max_vms() -> u8 {
    1
}

fn default_minutes() -> u16 {
    DEFAULT_LEASE_MINUTES
}

fn longest_minutes() -> u16 {
    LONGEST_LEASE_MINUTES
}

fn idle_minutes() -> u16 {
    IDLE_LEASE_MINUTES
}

impl Default for TestHostSettings {
    fn default() -> Self {
        Self {
            kinds: Vec::new(),
            mac_host: false,
            max_vms: default_max_vms(),
            default_minutes: DEFAULT_LEASE_MINUTES,
            longest_minutes: LONGEST_LEASE_MINUTES,
            idle_minutes: IDLE_LEASE_MINUTES,
        }
    }
}

/// Where a lease stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    /// It waits in its host's queue.
    Queued,
    /// Its host is getting the machine ready.
    Starting,
    /// Its holder can drive the machine.
    Ready,
    /// The machine is closing for it.
    Ending,
    /// It's over, for the reason in [`TestLease::ended`].
    Ended,
}

/// Why a lease ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaseEnd {
    /// Its holder closed it.
    Closed,
    /// It reached its time limit.
    TimeUp,
    /// It went without a call for too long.
    Idle,
    /// Its holder was archived or deleted.
    HolderGone,
    /// You ended it in the app.
    EndedByYou,
    /// Its host failed, or `maxd` restarted while it ran.
    HostFailed,
}

/// One command a lease's holder ran, as the app shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LeaseCommand {
    /// When it started.
    pub at: Timestamp,
    /// What ran, as the agent gave it.
    pub command: Vec<String>,
    /// How it exited, none when it timed out or couldn't start.
    pub exit_code: Option<i32>,
}

/// One holder's time on a test host. The device that hosts it grants it,
/// and only a caller on that device holds one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TestLease {
    /// The lease.
    pub id: LeaseId,
    /// The device that grants and holds it.
    pub device: DeviceId,
    /// The host it's on.
    pub host: TestHostId,
    /// The kind it asked for.
    pub kind: TestKind,
    /// Who holds it, on this device.
    pub holder: Actor,
    /// The permission it was granted under.
    pub permission: PermissionId,
    /// When it was asked for.
    pub started_at: Timestamp,
    /// When its time is up.
    pub ends_at: Timestamp,
    /// When it ends unless another call comes first.
    pub idle_ends_at: Timestamp,
    /// Where it stands.
    pub state: LeaseState,
    /// The Simulator's UDID or the emulator's serial, once ready.
    pub target: Option<String>,
    /// Why it ended, once it has.
    pub ended: Option<LeaseEnd>,
    /// Its last commands, oldest first, at most [`RECENT_COMMANDS`].
    #[serde(default)]
    pub recent: Vec<LeaseCommand>,
}

/// What one command in a lease printed, for the agent and the guest link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RunOutput {
    /// How it exited, none when it timed out or a signal ended it.
    pub exit_code: Option<i32>,
    /// What it printed to stdout, cut at the cap.
    pub stdout: String,
    /// What it printed to stderr, cut at the cap.
    pub stderr: String,
    /// Whether the output was cut.
    pub truncated: bool,
    /// Whether it ran past its timeout and was stopped.
    pub timed_out: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::AgentId;
    use serde_json::json;

    #[test]
    fn kinds_have_one_name_everywhere() {
        for kind in TestKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.name()));
            assert_eq!(TestKind::named(kind.name()), Some(kind));
            for backend in kind.backends() {
                assert_eq!(
                    serde_json::to_value(backend).unwrap(),
                    json!(backend.name())
                );
            }
        }
        assert_eq!(TestKind::named("ios"), None);
    }

    #[test]
    fn a_host_id_depends_on_its_device_kind_and_backend() {
        let mac = DeviceId::derive("mac");
        let vm = host_id(mac, TestKind::IosSimulator, TestHostBackend::MacVm);
        assert_eq!(
            vm,
            host_id(mac, TestKind::IosSimulator, TestHostBackend::MacVm)
        );
        assert_ne!(
            vm,
            host_id(mac, TestKind::IosSimulator, TestHostBackend::MacHost)
        );
        assert_ne!(vm, host_id(mac, TestKind::Macos, TestHostBackend::MacVm));
        assert_ne!(
            vm,
            host_id(
                DeviceId::derive("devbox"),
                TestKind::IosSimulator,
                TestHostBackend::MacVm
            )
        );
    }

    #[test]
    fn settings_from_before_test_hosts_read_as_the_defaults() {
        let settings: TestHostSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(settings, TestHostSettings::default());
        assert!(settings.kinds.is_empty() && !settings.mac_host);
        assert_eq!(
            (
                settings.max_vms,
                settings.default_minutes,
                settings.longest_minutes,
                settings.idle_minutes
            ),
            (1, 30, 240, 10)
        );
    }

    #[test]
    fn a_lease_round_trips() {
        let lease = TestLease {
            id: LeaseId::derive("lease"),
            device: DeviceId::derive("mac"),
            host: host_id(
                DeviceId::derive("mac"),
                TestKind::AndroidEmulator,
                TestHostBackend::HostEmulator,
            ),
            kind: TestKind::AndroidEmulator,
            holder: Actor::Agent {
                agent: AgentId::derive("a"),
            },
            permission: PermissionId::derive("p"),
            started_at: Timestamp(1_000),
            ends_at: Timestamp(1_801_000),
            idle_ends_at: Timestamp(601_000),
            state: LeaseState::Ready,
            target: Some("emulator-5554".into()),
            ended: None,
            recent: vec![LeaseCommand {
                at: Timestamp(2_000),
                command: vec!["snapshot".into(), "-i".into()],
                exit_code: Some(0),
            }],
        };
        let value = serde_json::to_value(&lease).unwrap();
        assert_eq!(value["kind"], json!("android_emulator"));
        assert_eq!(value["state"], json!("ready"));
        assert_eq!(serde_json::from_value::<TestLease>(value).unwrap(), lease);
        assert_eq!(
            serde_json::to_value(LeaseEnd::EndedByYou).unwrap(),
            json!("ended_by_you")
        );
    }
}
