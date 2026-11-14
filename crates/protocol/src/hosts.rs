// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! M6's host types. Each device checks itself against its
//! `maxhq-host.toml` and publishes a [`HostReport`]. Container workspaces
//! have snapshots ([`WorkspaceSnapshot`]). A workspace the old scripts made
//! moves into MaxHQ as a [`Move`], one step you approve at a time. The ACP
//! registry's agents, as a device can install them, are an [`AcpRegistry`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{DeviceId, MoveId, WorkspaceId};
use crate::model::Vendor;
use crate::time::Timestamp;

/// How a device stands against its host spec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct HostReport {
    /// The device.
    pub device: DeviceId,
    /// Where the spec came from.
    pub source: HostSource,
    /// When the device last checked.
    pub checked_at: Timestamp,
    /// One entry for each thing checked.
    pub items: Vec<HostItem>,
}

/// Where a device's host spec came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostSource {
    /// A `maxhq-host.toml` the device read.
    File {
        /// Its path.
        path: String,
    },
    /// The NixOS module, which renders the spec to `/etc/maxhq/host.json`.
    Nix,
    /// No spec file, so the defaults.
    Defaults,
}

/// One thing a host check looked at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct HostItem {
    /// Stable across checks, such as `incus.pool`, for
    /// [`crate::message::ClientMessage::ApplyHost`].
    pub id: String,
    /// What it is, for a person to read.
    pub label: String,
    /// How it stands.
    pub state: HostItemState,
    /// How to put it right, unless it's fine.
    pub fix: Option<HostFix>,
}

/// How one [`HostItem`] stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HostItemState {
    /// It matches the spec.
    Ok,
    /// It's there, but not as the spec says.
    Drift {
        /// What the spec says.
        want: String,
        /// What's there.
        have: String,
    },
    /// It isn't there.
    Missing,
    /// The check couldn't tell.
    Unknown {
        /// Why, for a person to read.
        why: String,
    },
}

/// How to put a [`HostItem`] right.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFix {
    /// `maxd` can do it now, without root, through
    /// [`crate::message::ClientMessage::ApplyHost`].
    Now,
    /// One command to run as root.
    RootCommand {
        /// The command.
        command: String,
    },
    /// One change to the host's NixOS configuration, then a rebuild.
    NixChange {
        /// The change.
        change: String,
    },
}

/// One snapshot of a container workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct WorkspaceSnapshot {
    /// Its name in Incus, such as `auto-2026-11-02-1400` or
    /// `mhq-before-rollback`.
    pub name: String,
    /// When it was taken.
    pub taken_at: Timestamp,
    /// When Incus deletes it, or none for one that's kept.
    pub expires_at: Option<Timestamp>,
}

/// A workspace the old scripts made, moving into MaxHQ one approved step
/// at a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Move {
    /// Stable for the life of the move.
    pub id: MoveId,
    /// The device both containers are on.
    pub device: DeviceId,
    /// The container being moved, such as `ws-alpha`.
    pub source: String,
    /// The copy MaxHQ runs, such as `mhq-alpha`.
    pub target: String,
    /// The copy's workspace, once it has one.
    pub workspace: Option<WorkspaceId>,
    /// The step this move is at.
    pub step: MoveStep,
    /// How that step stands.
    pub state: MoveState,
    /// What the look found and what each step changes, once the look has
    /// run.
    pub plan: Option<MovePlan>,
    /// Whether the original changed since the copy was taken, so the switch
    /// offers to copy again first.
    #[serde(default)]
    pub source_changed: bool,
    /// When the stopped original may be retired, from the switch on.
    pub kept_until: Option<Timestamp>,
    /// What a move back or a retire found that wasn't saved, such as
    /// "2 commits not pushed", in the words the card shows.
    #[serde(default)]
    pub unsaved: Vec<String>,
    /// What's left to do by hand once the last move on the device retired
    /// its original. `maxd` never does these itself.
    #[serde(default)]
    pub checklist: Vec<String>,
}

/// One step of a [`Move`], in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoveStep {
    /// Reads the original and changes nothing.
    Look,
    /// Snapshots the original and copies the snapshot to the new name.
    Copy,
    /// Sets the copy up the way MaxHQ runs its own, without starting an
    /// agent.
    Converge,
    /// Stops the original and resumes its agent in the copy.
    Switch,
    /// Keeps the stopped original, so the move can go back.
    Keep,
    /// Deletes the original and its move snapshot.
    Retire,
}

/// How the current step of a [`Move`] stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MoveState {
    /// It waits for you to approve it.
    Ready,
    /// It's running.
    Running {
        /// What it's doing, for a person to read.
        doing: String,
    },
    /// It failed. Nothing after it ran.
    Failed {
        /// Why, for a person to read.
        reason: String,
    },
    /// The move went back: the original runs again.
    MovedBack,
    /// The original is retired. The move is over.
    Retired,
}

/// What the look step found, and what each later step changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct MovePlan {
    /// What the look found.
    pub look: MoveLook,
    /// Each later step and what it changes, in order.
    pub steps: Vec<PlannedStep>,
}

/// What the look step found in the original.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct MoveLook {
    /// The agent CLI it runs, when one is found.
    pub vendor: Option<Vendor>,
    /// The agent's session, to resume in the copy.
    pub session: Option<String>,
    /// How the agent stands, for a person to read, such as "idle".
    pub agent_state: Option<String>,
    /// The repo's folder, which the copy keeps.
    pub repo_path: Option<String>,
    /// The original's profiles.
    pub profiles: Vec<String>,
    /// The original's devices, by name.
    pub devices: Vec<String>,
    /// Bytes its disk uses, when known.
    #[specta(type = Option<specta_typescript::Number>)]
    pub disk_used: Option<u64>,
    /// Whether an old resume flag is set for it.
    pub resume_flag: bool,
    /// When the next scheduled reboot can fire, when known.
    pub next_reboot: Option<Timestamp>,
}

/// One later step of a move and what it changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PlannedStep {
    /// The step.
    pub step: MoveStep,
    /// What it changes, one line each, for a person to read.
    pub changes: Vec<String>,
}

/// The ACP registry's agents as one device can install them: fetched,
/// cached, and with the agents MaxHQ runs natively left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AcpRegistry {
    /// The device.
    pub device: DeviceId,
    /// When the list was fetched, or none before the first fetch.
    pub fetched_at: Option<Timestamp>,
    /// Why the last fetch failed, while the cached list stands in.
    pub problem: Option<String>,
    /// The agents.
    pub agents: Vec<RegistryAgent>,
}

/// One agent in the ACP registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RegistryAgent {
    /// Its id in the registry.
    pub id: String,
    /// Its name.
    pub name: String,
    /// The registry's version of it.
    pub version: String,
    /// What it says it is.
    pub description: Option<String>,
    /// How it would install on this device, or none when it can't.
    pub install: Option<RegistryInstall>,
    /// The ways it signs in.
    pub auth: Vec<RegistryAuth>,
    /// The version installed on the device, when it is.
    pub installed: Option<String>,
}

/// How a [`RegistryAgent`] installs on a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RegistryInstall {
    /// A binary for the device's target.
    Binary {
        /// Whether the registry gives its SHA-256. Without one, it
        /// installs only once you confirm.
        checked: bool,
    },
    /// An npm package at an exact version, through `npx`.
    Npx {
        /// The package.
        package: String,
    },
    /// A Python package at an exact version, through `uvx`.
    Uvx {
        /// The package.
        package: String,
    },
}

/// One way a [`RegistryAgent`] signs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct RegistryAuth {
    /// Its id, as the agent lists it.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Whether it runs in a terminal.
    pub terminal: bool,
}

/// A licence a tool's installer asks you to accept, such as one of the
/// Android SDK's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PendingLicence {
    /// Its id, such as `android-sdk-license`.
    pub id: String,
    /// Its full text.
    pub text: String,
    /// The text's SHA-256, in lowercase hex.
    pub text_sha256: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_host_report_reads_back_and_names_each_state() {
        let report = HostReport {
            device: DeviceId::derive("devbox"),
            source: HostSource::File {
                path: "/home/dev/.config/maxhq/maxhq-host.toml".into(),
            },
            checked_at: Timestamp(1_700_000_000_000),
            items: vec![
                HostItem {
                    id: "incus.pool".into(),
                    label: "Storage pool".into(),
                    state: HostItemState::Ok,
                    fix: None,
                },
                HostItem {
                    id: "caps.memory_high".into(),
                    label: "Memory cap".into(),
                    state: HostItemState::Drift {
                        want: "12GiB".into(),
                        have: "8GiB".into(),
                    },
                    fix: Some(HostFix::RootCommand {
                        command: "sudo maxd host apply --system".into(),
                    }),
                },
                HostItem {
                    id: "zram".into(),
                    label: "zram".into(),
                    state: HostItemState::Unknown {
                        why: "no permission".into(),
                    },
                    fix: Some(HostFix::NixChange {
                        change: "zramSwap.enable = true;".into(),
                    }),
                },
            ],
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            value["source"],
            json!({ "type": "file", "path": "/home/dev/.config/maxhq/maxhq-host.toml" })
        );
        assert_eq!(value["items"][0]["state"], json!({ "state": "ok" }));
        assert_eq!(value["items"][1]["state"]["state"], json!("drift"));
        assert_eq!(value["items"][1]["fix"]["type"], json!("root_command"));
        assert_eq!(serde_json::from_value::<HostReport>(value).unwrap(), report);
    }

    #[test]
    fn a_move_reads_back_and_an_old_one_has_no_source_change() {
        let mut value = json!({
            "id": "0199a7c2-0000-7000-8000-0000000000f1",
            "device": DeviceId::derive("devbox"),
            "source": "ws-alpha",
            "target": "mhq-alpha",
            "workspace": null,
            "step": "copy",
            "state": { "state": "ready" },
            "plan": null,
            "kept_until": null,
        });
        let read: Move = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(read.step, MoveStep::Copy);
        assert!(!read.source_changed);
        assert!(read.unsaved.is_empty() && read.checklist.is_empty());
        value["source_changed"] = json!(false);
        value["unsaved"] = json!([]);
        value["checklist"] = json!([]);
        assert_eq!(serde_json::to_value(&read).unwrap(), value);
    }
}
