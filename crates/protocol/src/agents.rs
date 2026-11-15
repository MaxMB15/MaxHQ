// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents that run agents. An agent, or a tool outside MaxHQ holding an
//! access key, may start, message, wait on and stop other agents once you
//! allow it. A [`Permission`] says where it may act, and [`Limits`] on top
//! are optional. Every agent a permission starts carries an
//! [`AgentOrigin`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

use crate::ids::{
    AccountId, AgentId, DeviceId, KeyId, PermissionId, ProjectId, TaskId, WorkspaceId,
};
use crate::leases::TestKind;
use crate::message::AgentPlace;
use crate::model::Vendor;
use crate::notes::{NoteVersion, VersionHash};
use crate::time::Timestamp;

/// How many levels of agents a permission reaches when you don't say.
pub const DEPTH: u8 = 3;

/// Who calls the agent tools: an agent MaxHQ holds, a tool outside it
/// with an access key, or a scheduled task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// An agent.
    Agent {
        /// The agent.
        agent: AgentId,
    },
    /// A tool with an access key.
    Key {
        /// The key.
        key: KeyId,
    },
    /// A scheduled task, on each run.
    Task {
        /// The task.
        task: TaskId,
    },
}

/// What one agent or access key may do with the agent tools. The agents
/// it starts may do the same, one level less deep, and their starts count
/// against it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Permission {
    /// The permission.
    pub id: PermissionId,
    /// Who it lets act.
    pub holder: Actor,
    /// The device that counts its starts and running agents.
    pub home: DeviceId,
    /// The devices it may act on.
    pub devices: Vec<DeviceId>,
    /// The vendors it may start.
    pub vendors: Vec<Vendor>,
    /// The accounts it may start agents under, or none for any.
    pub accounts: Option<Vec<AccountId>>,
    /// The workspaces and folders it may start agents in, or none for any.
    /// A folder allows every folder inside it.
    pub places: Option<Vec<AgentPlace>>,
    /// How many levels of agents it reaches: 1 lets the holder start
    /// agents that can't start their own.
    pub depth: u8,
    /// Optional limits, all off unless set.
    #[serde(default)]
    pub limits: Limits,
    /// The kinds of test host it may lease on its devices. None unless you
    /// pick them, apart from starting agents.
    #[serde(default)]
    pub test_kinds: Vec<TestKind>,
    /// Whether an agent in a container may reach outside it under this
    /// permission: start agents on the host or another device, or message,
    /// read, wait on or stop agents there. One saved before this reads as
    /// [`LeaveContainer::Ask`].
    #[serde(default)]
    pub leave_container: LeaveContainer,
}

/// Whether an agent in a container may reach agents outside it. Those run
/// without the container's network limits and file walls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaveContainer {
    /// Each call waits for you to allow it, until you remember an answer.
    #[default]
    Ask,
    /// Every call may.
    Allow,
    /// No call may.
    Never,
}

/// Limits on a permission. Each is off when unset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Limits {
    /// The most agents it may start over its life.
    pub max_started: Option<u32>,
    /// The most agents it started that may run at once.
    pub max_running: Option<u32>,
    /// No starts on an account whose 5 hour or weekly use is above this
    /// percent.
    pub usage_cap_percent: Option<u8>,
    /// When it stops allowing anything.
    pub expires_at: Option<Timestamp>,
}

/// What an MCP client's config runs to reach the agent tools with an
/// access key. No shell reads it, so every path in it is absolute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct McpCommand {
    /// The program: `maxd` on the key's device.
    pub command: String,
    /// Its arguments, the key file among them but never the key.
    pub args: Vec<String>,
}

/// An access key a device made, which a tool outside MaxHQ signs in with.
/// It never holds the key itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AccessKey {
    /// The key.
    pub id: KeyId,
    /// The device that made it and holds its file.
    pub device: DeviceId,
    /// The name you gave it.
    pub name: String,
    /// The key file, as an absolute path on the device.
    pub path: String,
    /// The permission it calls under.
    pub permission: PermissionId,
    /// What an MCP client on the device runs to use it.
    pub command: McpCommand,
}

/// Who started an agent, kept with it so it nests under its caller and a
/// stop reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AgentOrigin {
    /// The permission it was started under.
    pub permission: PermissionId,
    /// The agent or key that started it.
    pub by: Actor,
    /// The device the caller is on.
    pub device: DeviceId,
    /// How many levels below the permission's holder the agent is: 1 for
    /// one the holder started.
    #[serde(default)]
    pub level: u8,
    /// Whether the caller gets a message each time the agent finishes a
    /// turn.
    #[serde(default)]
    pub wake: bool,
    /// The container workspace the caller ran in on `device`, if it ran in
    /// one. A task an agent in a container made keeps it, so its runs stay
    /// there as the agent would.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub container: Option<WorkspaceId>,
}

/// One call to an agent tool, by the tool's own name, such as
/// `start_agent`, with its arguments as the tool's schema gives them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ToolCall {
    /// The tool.
    pub tool: String,
    /// Its arguments.
    #[specta(type = specta_typescript::Unknown)]
    pub arguments: Value,
}

/// What a tool call came to, as the caller reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ToolAnswer {
    /// The text, often JSON.
    pub text: String,
    /// Whether the call failed or was refused.
    pub failed: bool,
    /// Images that go with the text, such as a lease's screenshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ToolImage>,
}

/// One image in a tool's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ToolImage {
    /// Its type, such as `image/png`.
    pub mime: String,
    /// Its bytes, in base64.
    pub data: String,
}

/// A call one device sends another for a caller on it. The device that
/// runs it checks its own copy of `permission`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PeerCall {
    /// The caller.
    pub by: Actor,
    /// The permission it calls under.
    pub permission: PermissionId,
    /// How many levels below the permission's holder the caller is, 0 for
    /// the holder.
    pub level: u8,
    /// The call.
    pub call: ToolCall,
}

/// Which kind of client a connection is. A connection whose hello leaves
/// it out is the app's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientRole {
    /// The app, with every right.
    #[default]
    App,
    /// `maxd mcp`, which may send only `AuthKey` and then tool calls.
    Mcp,
    /// Another device's `maxd`, which may send only agent calls, wakes and
    /// their answers. Only a connection `maxd proxy --peer` opened has it.
    Peer,
}

/// What one device's `maxd` asks another's over a peer link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeerRequest {
    /// An agent tool call for a caller on the asking device. The outcome
    /// is [`crate::message::ActionOutcome::Tool`].
    Call {
        /// The call.
        call: PeerCall,
    },
    /// Tells a caller on the asked device that an agent it started on the
    /// asking device finished a turn. The outcome is
    /// [`crate::message::ActionOutcome::Done`].
    Wake {
        /// The wake.
        wake: PeerWake,
    },
    /// Lists the hash of every note version the asked device holds for a
    /// project. The outcome is
    /// [`crate::message::ActionOutcome::NoteHashes`], with none for a
    /// project the asked device doesn't have.
    NoteHashes {
        /// The project.
        project: ProjectId,
    },
    /// Reads note versions by hash. The outcome is
    /// [`crate::message::ActionOutcome::NoteVersions`], maybe only the
    /// first few.
    NoteVersions {
        /// The project.
        project: ProjectId,
        /// The versions to read.
        hashes: Vec<VersionHash>,
    },
    /// Hands the asked device note versions it lacks, parents first. The
    /// outcome is [`crate::message::ActionOutcome::NotesPut`].
    PutNoteVersions {
        /// The project.
        project: ProjectId,
        /// The versions.
        versions: Vec<NoteVersion>,
    },
    /// Tells the asked device which of its agents the asking device's
    /// enabled tasks send runs to, all of them each time, so auto-settle
    /// there leaves them be. None clears what it said before. The outcome
    /// is [`crate::message::ActionOutcome::Done`].
    TaskTargets {
        /// The agents, on the asked device.
        agents: Vec<AgentId>,
    },
    /// A request from a newer `maxd`, which this one refuses.
    Unknown,
}

/// A turn's end, for the agent that started the agent on another device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PeerWake {
    /// The permission the agent was started under.
    pub permission: PermissionId,
    /// The agent that started it, on the asked device.
    pub parent: AgentId,
    /// The agent that finished.
    pub agent: AgentId,
    /// Its title.
    pub title: String,
    /// Its vendor.
    pub vendor: Vendor,
    /// The asking device's name.
    pub device_name: String,
    /// How its turn ended.
    pub turn: WakeTurn,
}

/// How a turn ended, for a wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WakeTurn {
    /// It finished.
    Finished,
    /// It waits for input.
    NeedsInput,
    /// It stopped.
    Stopped,
}

/// The other devices a `maxd` may link to, as the app sends them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PeerBook {
    /// Whether this device takes SSH logins. One that doesn't only dials,
    /// and keeps a link up to the home of each permission that names it.
    pub ssh: bool,
    /// Every other device.
    pub peers: Vec<PeerEntry>,
}

/// One other device in a [`PeerBook`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PeerEntry {
    /// The device.
    pub device: DeviceId,
    /// Its name.
    pub name: String,
    /// Where to reach it over SSH, in order. None for a device that takes
    /// no SSH logins.
    pub routes: Vec<PeerRoute>,
    /// Its SSH host key, as `ssh-ed25519 <base64>`. Empty for a device
    /// with no routes, which is never dialed.
    pub host_key: String,
    /// The kinds of test host it offers, for telling an agent where to
    /// start another that can lease one.
    #[serde(default)]
    pub test_kinds: Vec<TestKind>,
}

/// One way to reach a device over SSH, resolved by the app, with no jump
/// host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PeerRoute {
    /// The host name or address.
    pub host: String,
    /// The SSH port.
    pub port: u16,
    /// The login.
    pub user: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_from_before_wake_reads_as_no_wake() {
        let json = serde_json::json!({
            "permission": PermissionId::derive("p"),
            "by": {"kind": "key", "key": KeyId::derive("k")},
            "device": DeviceId::derive("d"),
        });
        let origin: AgentOrigin = serde_json::from_value(json).expect("the origin reads");
        assert!(!origin.wake);
        assert_eq!(origin.level, 0);
    }

    #[test]
    fn a_permission_round_trips() {
        let permission = Permission {
            id: PermissionId::derive("p"),
            holder: Actor::Agent {
                agent: AgentId::derive("a"),
            },
            home: DeviceId::derive("d"),
            devices: vec![DeviceId::derive("d")],
            vendors: vec![Vendor::Codex],
            accounts: None,
            places: Some(vec![AgentPlace::Folder {
                path: "/work".into(),
            }]),
            depth: DEPTH,
            limits: Limits {
                max_started: Some(4),
                ..Limits::default()
            },
            test_kinds: vec![TestKind::IosSimulator],
            leave_container: LeaveContainer::Never,
        };
        let mut json = serde_json::to_value(&permission).expect("it serializes");
        assert_eq!(json["holder"]["kind"], "agent");
        assert_eq!(json["test_kinds"], serde_json::json!(["ios_simulator"]));
        let back: Permission = serde_json::from_value(json.clone()).expect("it reads back");
        assert_eq!(back, permission);
        assert_eq!(json["leave_container"], "never");
        // One from before test hosts leases nothing, and one from before
        // containers asked asks.
        json.as_object_mut().unwrap().remove("test_kinds");
        json.as_object_mut().unwrap().remove("leave_container");
        let old: Permission = serde_json::from_value(json).expect("an old one reads");
        assert!(old.test_kinds.is_empty());
        assert_eq!(old.leave_container, LeaveContainer::Ask);
    }

    #[test]
    fn an_answer_without_images_reads_and_writes_as_before() {
        let old = serde_json::json!({ "text": "ok", "failed": false });
        let answer: ToolAnswer = serde_json::from_value(old.clone()).expect("it reads");
        assert!(answer.images.is_empty());
        assert_eq!(serde_json::to_value(&answer).unwrap(), old);
        let entry: PeerEntry = serde_json::from_value(serde_json::json!({
            "device": DeviceId::derive("d"),
            "name": "devbox",
            "routes": [],
            "host_key": "",
        }))
        .expect("an old entry reads");
        assert!(entry.test_kinds.is_empty());
    }
}
