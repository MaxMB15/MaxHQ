// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! State sync. A client gets one snapshot, then deltas, each numbered. A client
//! that wakes from sleep and finds a gap asks for a sync from the last number
//! it has, and catches up in one round trip.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::acp::AcpAgent;
use crate::admission::{Admission, AppKey};
use crate::attention::AttentionEvent;
use crate::dispatch::{Dispatcher, DispatcherChat};
use crate::hosts::{AcpRegistry, HostReport, Move, PendingLicence};
use crate::ids::{
    AccountId, AcpAgentId, AdmissionId, AgentId, AppKeyId, AttentionId, CredentialId, DeviceId,
    ForwardId, LeaseId, LinkId, ModelServerId, MoveId, NetworkId, PhoneId, PhoneSessionId, PortId,
    ProjectId, SkillProposalId, TerminalId, TestHostId, WorkflowRunId, WorkspaceId,
};
use crate::leases::{TestHost, TestLease};
use crate::link::{DeviceLink, Network};
use crate::local_models::{LocalModels, ModelServer};
use crate::model::{
    Account, Agent, CommandRules, Credential, Device, DeviceSettings, Forward, HiddenProjects,
    ListeningPort, Project, ProjectSettings, Tool, Workspace,
};
use crate::netaccess::{NetworkAccess, PhoneSession};
use crate::notes::ProjectNotes;
use crate::phones::{Phone, PhoneBook};
use crate::skills::{ProjectSkills, SkillLibrary, SkillProposal};
use crate::tasks::DeviceTasks;
use crate::terminal::Terminal;
use crate::workflow::WorkflowRun;

/// Numbers each version of the state. Each delta is one more than the last.
///
/// TypeScript sees a plain `number`, which is exact up to 2^53.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
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
#[serde(transparent)]
pub struct Seq(#[specta(type = specta_typescript::Number)] pub u64);

impl Seq {
    /// The number after this one.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Anything the state holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "entity", rename_all = "snake_case")]
pub enum Entity {
    /// A device, as its `maxd` describes it.
    Device(Device),
    /// The settings a device's `maxd` keeps.
    DeviceSettings(DeviceSettings),
    /// The client's connection to a device. Never sent by `maxd`.
    DeviceLink(DeviceLink),
    /// A network the client reaches devices through. Never sent by `maxd`.
    Network(Network),
    /// A project.
    Project(Project),
    /// A workspace.
    Workspace(Workspace),
    /// An agent or sub-agent.
    Agent(Agent),
    /// A vendor login.
    Account(Account),
    /// An open attention event.
    Attention(AttentionEvent),
    /// A port a process in a workspace listens on.
    ListeningPort(ListeningPort),
    /// A client's local port that reaches a listening port.
    Forward(Forward),
    /// One project's settings on one device.
    ProjectSettings(ProjectSettings),
    /// A login or key a device holds, without its secret.
    Credential(Credential),
    /// The agent CLIs on a device and in its workspaces.
    DeviceTools(DeviceTools),
    /// The scheduled tasks a device holds.
    DeviceTasks(DeviceTasks),
    /// A local ACP command a device runs, without its variables' values.
    AcpAgent(AcpAgent),
    /// A kind of test machine a device offers.
    TestHost(TestHost),
    /// A lease of a test host, while it lasts.
    TestLease(TestLease),
    /// A workflow run a device follows.
    WorkflowRun(WorkflowRun),
    /// How a device stands against its host spec.
    HostReport(HostReport),
    /// A workspace the old scripts made, moving into MaxHQ.
    Move(Move),
    /// The ACP registry's agents, as a device can install them.
    AcpRegistry(AcpRegistry),
    /// The skill library, as the device that holds it publishes it.
    SkillLibrary(SkillLibrary),
    /// A project's skills on one device.
    ProjectSkills(ProjectSkills),
    /// A changed skill an agent proposes.
    SkillProposal(SkillProposal),
    /// A paired phone, as one device that lets it in has it.
    Phone(Phone),
    /// The devices a phone may reach, from the device it paired through.
    PhoneBook(PhoneBook),
    /// How a device's network access stands.
    NetworkAccess(NetworkAccess),
    /// A token a device holds for a phone.
    PhoneSession(PhoneSession),
    /// The projects a device keeps hidden from the sidebar.
    HiddenProjects(HiddenProjects),
    /// A device's dispatcher.
    Dispatcher(Dispatcher),
    /// The commands agents may run without asking in one project.
    CommandRules(CommandRules),
    /// The notes one device holds for one project.
    ProjectNotes(ProjectNotes),
    /// An app key a device trusts.
    AppKey(AppKey),
    /// A card that asks whether to let in a program that proved no app key.
    Admission(Admission),
    /// A terminal a device holds for an agent: a shell in its drawer or a
    /// command you ran with `!`. Since protocol 45.
    Terminal(Terminal),
    /// One conversation with a device's dispatcher. Since protocol 53.
    DispatcherChat(DispatcherChat),
    /// A model server a device runs or reaches. Since
    /// [`crate::message::LOCAL_MODELS_SINCE`].
    ModelServer(ModelServer),
    /// A device's hardware, model catalog and model files. Since
    /// [`crate::message::LOCAL_MODELS_SINCE`].
    LocalModels(LocalModels),
}

/// Names one entity, for removal and lookup.
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
#[serde(tag = "entity", content = "id", rename_all = "snake_case")]
pub enum EntityKey {
    /// A device.
    Device(DeviceId),
    /// A device's settings, named by the device.
    DeviceSettings(DeviceId),
    /// The client's connection to a device.
    DeviceLink(LinkId),
    /// A network the client reaches devices through.
    Network(NetworkId),
    /// A project.
    Project(ProjectId),
    /// A workspace.
    Workspace(WorkspaceId),
    /// An agent or sub-agent.
    Agent(AgentId),
    /// A vendor login.
    Account(AccountId),
    /// An attention event.
    Attention(AttentionId),
    /// A listening port.
    ListeningPort(PortId),
    /// A forward.
    Forward(ForwardId),
    /// One project's settings, named by the project and the device.
    ProjectSettings {
        /// The project.
        project: ProjectId,
        /// The device that keeps them.
        device: DeviceId,
    },
    /// A login or key a device holds.
    Credential(CredentialId),
    /// A device's agent CLIs, named by the device.
    DeviceTools(DeviceId),
    /// A device's scheduled tasks, named by the device.
    DeviceTasks(DeviceId),
    /// A local ACP command.
    AcpAgent(AcpAgentId),
    /// A test host.
    TestHost(TestHostId),
    /// A lease of a test host.
    TestLease(LeaseId),
    /// A workflow run.
    WorkflowRun(WorkflowRunId),
    /// A device's host report, named by the device.
    HostReport(DeviceId),
    /// A move.
    Move(MoveId),
    /// A device's ACP registry list, named by the device.
    AcpRegistry(DeviceId),
    /// The skill library, named by the device that holds it.
    SkillLibrary(DeviceId),
    /// One project's skills, named by the project and the device.
    ProjectSkills {
        /// The project.
        project: ProjectId,
        /// The device that keeps them.
        device: DeviceId,
    },
    /// A skill proposal.
    SkillProposal(SkillProposalId),
    /// A paired phone, named by the phone and the device that has it.
    Phone {
        /// The phone.
        phone: PhoneId,
        /// The device that lets it in.
        device: DeviceId,
    },
    /// A phone's book, named by the phone.
    PhoneBook(PhoneId),
    /// A device's network access, named by the device.
    NetworkAccess(DeviceId),
    /// A phone session.
    PhoneSession(PhoneSessionId),
    /// A device's hidden projects, named by the device.
    HiddenProjects(DeviceId),
    /// A device's dispatcher, named by the device.
    Dispatcher(DeviceId),
    /// One project's command rules, named by the project and the device.
    CommandRules {
        /// The project.
        project: ProjectId,
        /// The device that keeps them.
        device: DeviceId,
    },
    /// One project's notes, named by the project and the device.
    ProjectNotes {
        /// The project.
        project: ProjectId,
        /// The device that holds them.
        device: DeviceId,
    },
    /// An app key a device trusts.
    AppKey(AppKeyId),
    /// An admission card.
    Admission(AdmissionId),
    /// A terminal.
    Terminal(TerminalId),
    /// A dispatcher chat, named by its agent.
    DispatcherChat(AgentId),
    /// A model server.
    ModelServer(ModelServerId),
    /// A device's local models, named by the device.
    LocalModels(DeviceId),
}

impl Entity {
    /// The key that names this entity.
    pub fn key(&self) -> EntityKey {
        match self {
            Self::Device(device) => EntityKey::Device(device.id),
            Self::DeviceSettings(settings) => EntityKey::DeviceSettings(settings.device),
            Self::DeviceLink(link) => EntityKey::DeviceLink(link.id),
            Self::Network(network) => EntityKey::Network(network.id),
            Self::Project(project) => EntityKey::Project(project.id),
            Self::Workspace(workspace) => EntityKey::Workspace(workspace.id),
            Self::Agent(agent) => EntityKey::Agent(agent.id),
            Self::Account(account) => EntityKey::Account(account.id),
            Self::Attention(event) => EntityKey::Attention(event.id),
            Self::ListeningPort(port) => EntityKey::ListeningPort(port.id),
            Self::Forward(forward) => EntityKey::Forward(forward.id),
            Self::ProjectSettings(settings) => EntityKey::ProjectSettings {
                project: settings.project,
                device: settings.device,
            },
            Self::Credential(credential) => EntityKey::Credential(credential.id),
            Self::DeviceTools(tools) => EntityKey::DeviceTools(tools.device),
            Self::DeviceTasks(tasks) => EntityKey::DeviceTasks(tasks.device),
            Self::AcpAgent(agent) => EntityKey::AcpAgent(agent.id),
            Self::TestHost(host) => EntityKey::TestHost(host.id),
            Self::TestLease(lease) => EntityKey::TestLease(lease.id),
            Self::WorkflowRun(run) => EntityKey::WorkflowRun(run.id),
            Self::HostReport(report) => EntityKey::HostReport(report.device),
            Self::Move(moving) => EntityKey::Move(moving.id),
            Self::AcpRegistry(registry) => EntityKey::AcpRegistry(registry.device),
            Self::SkillLibrary(library) => EntityKey::SkillLibrary(library.device),
            Self::ProjectSkills(skills) => EntityKey::ProjectSkills {
                project: skills.project,
                device: skills.device,
            },
            Self::SkillProposal(proposal) => EntityKey::SkillProposal(proposal.id),
            Self::Phone(phone) => EntityKey::Phone {
                phone: phone.id,
                device: phone.device,
            },
            Self::PhoneBook(book) => EntityKey::PhoneBook(book.phone),
            Self::NetworkAccess(access) => EntityKey::NetworkAccess(access.device),
            Self::PhoneSession(session) => EntityKey::PhoneSession(session.id),
            Self::HiddenProjects(hidden) => EntityKey::HiddenProjects(hidden.device),
            Self::Dispatcher(dispatcher) => EntityKey::Dispatcher(dispatcher.device),
            Self::CommandRules(rules) => EntityKey::CommandRules {
                project: rules.project,
                device: rules.device,
            },
            Self::ProjectNotes(notes) => EntityKey::ProjectNotes {
                project: notes.project,
                device: notes.device,
            },
            Self::AppKey(key) => EntityKey::AppKey(key.id),
            Self::Admission(admission) => EntityKey::Admission(admission.id),
            Self::Terminal(terminal) => EntityKey::Terminal(terminal.id),
            Self::DispatcherChat(chat) => EntityKey::DispatcherChat(chat.agent),
            Self::ModelServer(server) => EntityKey::ModelServer(server.id),
            Self::LocalModels(models) => EntityKey::LocalModels(models.device),
        }
    }
}

/// The tools on one device: on the device itself and in each container
/// workspace MaxHQ made. The agent CLIs, the helpers MaxHQ installs, and
/// ACP registry agents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct DeviceTools {
    /// The device.
    pub device: DeviceId,
    /// One entry for each CLI in each place.
    pub tools: Vec<ToolInfo>,
}

/// One tool in one place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ToolInfo {
    /// Which tool.
    pub kind: ToolKind,
    /// Where it's installed.
    pub place: ToolPlace,
    /// The binary's path, when found.
    pub path: Option<String>,
    /// The installed version, when it answered.
    pub version: Option<String>,
    /// The newest release, when known.
    pub latest: Option<String>,
    /// Who installed it.
    pub owner: ToolOwner,
    /// How it was installed, such as `native` or `npm`.
    pub method: String,
    /// Whether MaxHQ updates it.
    pub update: ToolUpdate,
    /// What it's doing now.
    pub state: ToolState,
}

impl ToolInfo {
    /// The agent CLI, for an entry that is one.
    pub fn tool(&self) -> Option<Tool> {
        match self.kind {
            ToolKind::Agent { tool } => Some(tool),
            ToolKind::Helper { .. } | ToolKind::Acp { .. } => None,
        }
    }
}

/// Which tool a [`ToolInfo`] is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolKind {
    /// An agent CLI.
    Agent {
        /// Which one.
        tool: Tool,
    },
    /// A tool MaxHQ installs for the agents and itself.
    Helper {
        /// Which one.
        tool: HelperTool,
    },
    /// An agent from the ACP registry.
    Acp {
        /// Its id in the registry.
        id: String,
    },
}

/// A tool MaxHQ installs that isn't an agent CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HelperTool {
    /// Node.js, for npm packages and npx ACP agents.
    Node,
    /// uv, for uvx ACP agents.
    Uv,
    /// The GitHub CLI.
    Gh,
    /// agent-device, which drives test machines.
    AgentDevice,
    /// A Java runtime, for the Android SDK.
    Jdk,
    /// The Android SDK's command-line tools.
    AndroidSdk,
}

/// Where a CLI is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolPlace {
    /// On the device itself.
    Host,
    /// In a container workspace.
    Workspace {
        /// The workspace.
        workspace: WorkspaceId,
    },
}

/// Who installed a CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolOwner {
    /// MaxHQ, in a workspace it made.
    MaxHq,
    /// You.
    User,
}

/// Whether MaxHQ updates a CLI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolUpdate {
    /// MaxHQ installed it, so it updates it.
    Auto,
    /// You ticked it for MaxHQ to update.
    Allowed,
    /// MaxHQ could update it once you tick it.
    NeedsTick,
    /// MaxHQ can't update it. Run the command yourself.
    Manual {
        /// What to run.
        command: String,
    },
    /// MaxHQ never updates it, as with Cursor: it has no package to
    /// check, so you or the CLI itself keep it current.
    Never,
}

/// What a tool's install or update is doing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolState {
    /// Nothing.
    Idle,
    /// Installing or updating.
    Running {
        /// The latest progress line.
        message: String,
        /// Where an install is, when the installer runs it. An update from
        /// before installs leaves this out.
        #[serde(default)]
        phase: Option<ToolPhase>,
        /// Bytes downloaded so far, while downloading.
        #[serde(default)]
        #[specta(type = Option<specta_typescript::Number>)]
        bytes_done: Option<u64>,
        /// Bytes to download in all, when known.
        #[serde(default)]
        #[specta(type = Option<specta_typescript::Number>)]
        bytes_total: Option<u64>,
    },
    /// The install waits for you to accept licences, with
    /// [`crate::message::ClientMessage::AcceptLicence`].
    Licences {
        /// The licences not yet accepted.
        pending: Vec<PendingLicence>,
    },
    /// The last update failed.
    Failed {
        /// Why.
        reason: String,
    },
}

impl ToolState {
    /// Running with a progress line and nothing more.
    pub fn running(message: String) -> Self {
        Self::Running {
            message,
            phase: None,
            bytes_done: None,
            bytes_total: None,
        }
    }
}

/// Where an install is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolPhase {
    /// Working out what to fetch.
    Resolving,
    /// Downloading.
    Downloading,
    /// Checking the download's SHA-256.
    Verifying,
    /// Unpacking or installing into a staging folder.
    Installing,
    /// Publishing the staged install and its links.
    Linking,
}

/// The whole state at one sequence number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Snapshot {
    /// The version this snapshot shows.
    pub seq: Seq,
    /// Every entity, in no particular order. One this build can't read,
    /// such as a kind from a newer `maxd`, is left out.
    #[serde(deserialize_with = "crate::tolerant::list")]
    #[specta(type = Vec<Entity>)]
    pub entities: Vec<Entity>,
}

/// The changes from one version of the state to the next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Delta {
    /// The version after these changes, one more than the version before.
    pub seq: Seq,
    /// Applied in order. One this build can't read is left out.
    #[serde(deserialize_with = "crate::tolerant::list")]
    #[specta(type = Vec<Change>)]
    pub changes: Vec<Change>,
}

/// One change in a delta.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
// Nearly every change is an upsert, so boxing the entity would add an
// allocation to the common case to shrink the rare one.
#[allow(clippy::large_enum_variant)]
pub enum Change {
    /// Adds the entity, or replaces the one with the same key.
    Upsert {
        /// The entity as it is now.
        value: Entity,
    },
    /// Removes the entity, if it's there.
    Remove {
        /// The entity to remove.
        key: EntityKey,
    },
}

/// A delta that doesn't follow the state a replica holds. The fix is a sync
/// from [`SeqGap::have`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("delta {got:?} doesn't follow {have:?}")]
pub struct SeqGap {
    /// The replica's version, or none before its first snapshot.
    pub have: Option<Seq>,
    /// The delta's version.
    pub got: Seq,
}

/// A client's copy of the state, kept current by snapshots and deltas.
#[derive(Clone, Debug, Default)]
pub struct Replica {
    seq: Option<Seq>,
    entities: BTreeMap<EntityKey, Entity>,
}

impl Replica {
    /// The version held, or none before the first snapshot.
    pub fn seq(&self) -> Option<Seq> {
        self.seq
    }

    /// Replaces everything with the snapshot.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.seq = Some(snapshot.seq);
        self.entities = snapshot
            .entities
            .into_iter()
            .map(|entity| (entity.key(), entity))
            .collect();
    }

    /// Applies the next delta. A delta for a version already held is a
    /// repeat and changes nothing. Anything further ahead is a gap.
    pub fn apply_delta(&mut self, delta: Delta) -> Result<(), SeqGap> {
        let gap = SeqGap {
            have: self.seq,
            got: delta.seq,
        };
        let have = self.seq.ok_or(gap)?;
        if delta.seq <= have {
            return Ok(());
        }
        if delta.seq != have.next() {
            return Err(gap);
        }
        for change in delta.changes {
            match change {
                Change::Upsert { value } => {
                    self.entities.insert(value.key(), value);
                }
                Change::Remove { key } => {
                    self.entities.remove(&key);
                }
            }
        }
        self.seq = Some(delta.seq);
        Ok(())
    }

    /// The entity with this key.
    pub fn get(&self, key: &EntityKey) -> Option<&Entity> {
        self.entities.get(key)
    }

    /// Every entity, ordered by key.
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::BootId;
    use crate::model::CommandRule;
    use serde_json::json;

    fn device(name: &str) -> Device {
        Device {
            id: "0199a7c2-0000-7000-8000-000000000001".parse().unwrap(),
            name: name.into(),
            os: "linux".into(),
            boot_id: BootId("4f1c".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
        }
    }

    fn snapshot(seq: u64) -> Snapshot {
        Snapshot {
            seq: Seq(seq),
            entities: vec![Entity::Device(device("devbox"))],
        }
    }

    #[test]
    fn deltas_apply_in_order() {
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot(4));
        let renamed = Entity::Device(device("home"));
        let key = renamed.key();
        replica
            .apply_delta(Delta {
                seq: Seq(5),
                changes: vec![Change::Upsert {
                    value: renamed.clone(),
                }],
            })
            .unwrap();
        assert_eq!(replica.get(&key), Some(&renamed));

        replica
            .apply_delta(Delta {
                seq: Seq(6),
                changes: vec![Change::Remove { key }],
            })
            .unwrap();
        assert_eq!(replica.entities().count(), 0);
        assert_eq!(replica.seq(), Some(Seq(6)));
    }

    #[test]
    fn repeated_deltas_change_nothing() {
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot(4));
        let key = device("devbox").id;
        let remove = Delta {
            seq: Seq(4),
            changes: vec![Change::Remove {
                key: EntityKey::Device(key),
            }],
        };
        replica.apply_delta(remove).unwrap();
        assert_eq!(replica.entities().count(), 1);
    }

    #[test]
    fn gaps_ask_for_a_sync() {
        let mut replica = Replica::default();
        let delta = Delta {
            seq: Seq(1),
            changes: vec![],
        };
        assert_eq!(
            replica.apply_delta(delta.clone()),
            Err(SeqGap {
                have: None,
                got: Seq(1)
            })
        );

        replica.apply_snapshot(snapshot(4));
        let ahead = Delta {
            seq: Seq(6),
            changes: vec![],
        };
        assert_eq!(
            replica.apply_delta(ahead),
            Err(SeqGap {
                have: Some(Seq(4)),
                got: Seq(6)
            })
        );
    }

    #[test]
    fn changes_are_tagged() {
        let key = EntityKey::Device(device("devbox").id);
        assert_eq!(
            serde_json::to_value(Change::Remove { key }).unwrap(),
            json!({ "op": "remove", "key": { "entity": "device", "id": "0199a7c2-0000-7000-8000-000000000001" } })
        );
        let upsert = serde_json::to_value(Change::Upsert {
            value: Entity::Device(device("devbox")),
        })
        .unwrap();
        assert_eq!(upsert["value"]["entity"], "device");
        assert_eq!(upsert["value"]["name"], "devbox");
    }

    #[test]
    fn device_settings_are_named_by_their_device() {
        let id = device("devbox").id;
        let settings = Entity::DeviceSettings(crate::model::DeviceSettings {
            device: id,
            default_permission: crate::model::PermissionMode::FullAccess,
            workspace_network: None,
            workspace_pool: None,
            auto_settle_days: 5,
            snooze_at_limit: false,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            placement: Default::default(),
            tool_updates: Vec::new(),
            cursor_usage: false,
            keep_awake: Default::default(),
            test_hosts: Default::default(),
            skill_library: None,
            network: Default::default(),
            idle_release_minutes: 60,
            memory_items: 256,
            history_mib: 256,
            claude_setup_token: false,
            shell_integration: true,
            read_other_agents: true,
            dispatcher_acts: true,
            chat_gap_minutes: 30,
            local_models: Default::default(),
        });
        assert_eq!(settings.key(), EntityKey::DeviceSettings(id));
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            json!({
                "entity": "device_settings",
                "device": "0199a7c2-0000-7000-8000-000000000001",
                "default_permission": "full_access",
                "workspace_network": null,
                "workspace_pool": null,
                "auto_settle_days": 5,
                "snooze_at_limit": false,
                "continue_claude_at_limit": false,
                "continue_codex_at_limit": false,
                "placement": "normal",
                "tool_updates": [],
                "cursor_usage": false,
                "keep_awake": "charger",
                "test_hosts": {
                    "kinds": [],
                    "mac_host": false,
                    "max_vms": 1,
                    "default_minutes": 30,
                    "longest_minutes": 240,
                    "idle_minutes": 10,
                },
                "network": { "on": false, "port": 7466 },
                "idle_release_minutes": 60,
                "memory_items": 256,
                "history_mib": 256,
                "claude_setup_token": false,
                "shell_integration": true,
                "read_other_agents": true,
                "dispatcher_acts": true,
                "chat_gap_minutes": 30,
                "local_models": {
                    "idle_unload_minutes": 15,
                    "kv_type": "f16",
                    "context": 65536,
                    "fewer_tools": true,
                    "default_model": null,
                    "linked_devices": true,
                },
            })
        );
        // A `maxd` from before these leaves them out, which reads as their defaults.
        let mut old = serde_json::to_value(&settings).unwrap();
        old.as_object_mut().unwrap().remove("shell_integration");
        old.as_object_mut().unwrap().remove("read_other_agents");
        old.as_object_mut().unwrap().remove("dispatcher_acts");
        old.as_object_mut().unwrap().remove("chat_gap_minutes");
        old.as_object_mut().unwrap().remove("local_models");
        assert_eq!(serde_json::from_value::<Entity>(old).unwrap(), settings);
    }

    #[test]
    fn test_hosts_are_named_by_their_id() {
        use crate::leases::{host_id, TestHost, TestHostBackend, TestHostState, TestKind};
        let device = device("devbox").id;
        let id = host_id(device, TestKind::Linux, TestHostBackend::IncusTest);
        let host = Entity::TestHost(TestHost {
            id,
            device,
            kind: TestKind::Linux,
            backend: TestHostBackend::IncusTest,
            state: TestHostState::Off,
            setup: None,
            error: None,
            agent_device: None,
            last_reset: None,
            queue: 0,
            vm: None,
        });
        assert_eq!(host.key(), EntityKey::TestHost(id));
        let value = serde_json::to_value(&host).unwrap();
        assert_eq!(value["entity"], json!("test_host"));
        assert_eq!(value["backend"], json!("incus_test"));
        assert_eq!(serde_json::from_value::<Entity>(value).unwrap(), host);
        assert_eq!(
            serde_json::to_value(EntityKey::TestHost(id)).unwrap(),
            json!({ "entity": "test_host", "id": id }),
        );
    }

    #[test]
    fn workflow_runs_and_project_skills_have_their_keys() {
        use crate::skills::{ProjectSkills, SkillLock};
        use crate::workflow::{Progress, RunKey, WorkflowRun};
        let device = device("devbox").id;
        let project = ProjectId::derive("git:github.com/owner/repo");
        let key = RunKey {
            device,
            project,
            workflow: "devflow".into(),
            run: "run-1".into(),
        };
        let run = Entity::WorkflowRun(WorkflowRun {
            id: key.id(),
            key: key.clone(),
            label: "Add sign-in".into(),
            progress: Progress { done: 0, total: 4 },
            current: Vec::new(),
            needs_you: false,
            orchestrators: Vec::new(),
            last_activity: None,
            done: false,
            failed: false,
            agents: Vec::new(),
        });
        assert_eq!(run.key(), EntityKey::WorkflowRun(key.id()));
        let value = serde_json::to_value(&run).unwrap();
        assert_eq!(value["entity"], json!("workflow_run"));
        assert_eq!(serde_json::from_value::<Entity>(value).unwrap(), run);

        let skills = Entity::ProjectSkills(ProjectSkills {
            project,
            device,
            lock: SkillLock::default(),
            checkouts: Vec::new(),
        });
        assert_eq!(skills.key(), EntityKey::ProjectSkills { project, device });
        let value = serde_json::to_value(&skills).unwrap();
        assert_eq!(value["entity"], json!("project_skills"));
        assert_eq!(serde_json::from_value::<Entity>(value).unwrap(), skills);
        assert_eq!(
            serde_json::to_value(EntityKey::SkillLibrary(device)).unwrap(),
            json!({ "entity": "skill_library", "id": device }),
        );
    }

    #[test]
    fn phones_are_named_by_the_phone_and_their_device() {
        use crate::phones::{Phone, PhoneBook, PushKind};
        let device = device("devbox").id;
        let id = PhoneId::derive("phone");
        let phone = Entity::Phone(Phone {
            id,
            device,
            name: "iPhone".into(),
            ssh_public: "ssh-ed25519 AAAAexample".into(),
            push_public: "AAAAexample".into(),
            paired_at: crate::time::Timestamp(5),
            push: vec![PushKind::NeedsInput],
            everything: false,
        });
        assert_eq!(phone.key(), EntityKey::Phone { phone: id, device });
        let value = serde_json::to_value(&phone).unwrap();
        assert_eq!(value["entity"], json!("phone"));
        assert_eq!(serde_json::from_value::<Entity>(value).unwrap(), phone);

        let book = Entity::PhoneBook(PhoneBook {
            phone: id,
            device,
            devices: Vec::new(),
            mac: None,
        });
        assert_eq!(book.key(), EntityKey::PhoneBook(id));
        assert_eq!(
            serde_json::to_value(EntityKey::PhoneBook(id)).unwrap(),
            json!({ "entity": "phone_book", "id": id }),
        );
        assert_eq!(
            serde_json::to_value(EntityKey::Phone { phone: id, device }).unwrap(),
            json!({ "entity": "phone", "id": { "phone": id, "device": device } }),
        );
    }

    #[test]
    fn device_tools_are_named_by_their_device() {
        let id = device("devbox").id;
        let tools = Entity::DeviceTools(DeviceTools {
            device: id,
            tools: vec![ToolInfo {
                kind: ToolKind::Agent {
                    tool: crate::model::Tool::Claude,
                },
                place: ToolPlace::Host,
                path: Some("/home/dev/.local/bin/claude".into()),
                version: Some("2.1.0".into()),
                latest: Some("2.2.0".into()),
                owner: ToolOwner::User,
                method: "native".into(),
                update: ToolUpdate::NeedsTick,
                state: ToolState::Idle,
            }],
        });
        assert_eq!(tools.key(), EntityKey::DeviceTools(id));
        let value = serde_json::to_value(&tools).unwrap();
        assert_eq!(value["entity"], "device_tools");
        assert_eq!(value["tools"][0]["place"], json!({ "type": "host" }));
        assert_eq!(
            value["tools"][0]["kind"],
            json!({ "type": "agent", "tool": "claude" })
        );
        assert_eq!(value["tools"][0]["update"], json!({ "type": "needs_tick" }));
        let back: Entity = serde_json::from_value(value).unwrap();
        assert_eq!(back, tools);
    }

    #[test]
    fn device_settings_from_before_inboxes_settle_after_5_days() {
        let settings: crate::model::DeviceSettings = serde_json::from_value(json!({
            "device": "0199a7c2-0000-7000-8000-000000000001",
            "default_permission": "full_access",
            "workspace_network": null,
            "workspace_pool": null,
        }))
        .unwrap();
        assert_eq!(
            settings.auto_settle_days,
            crate::model::DEFAULT_AUTO_SETTLE_DAYS
        );
        assert!(!settings.snooze_at_limit);
    }

    fn round_trip(entity: &Entity) -> serde_json::Value {
        let value = serde_json::to_value(entity).unwrap();
        let back: Entity = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&back, entity);
        let key = serde_json::to_value(entity.key()).unwrap();
        let key_back: EntityKey = serde_json::from_value(key).unwrap();
        assert_eq!(key_back, entity.key());
        value
    }

    #[test]
    fn m3_entities_survive_a_round_trip() {
        use crate::model::{
            BindAddress, CloneProgress, CloneStage, CloneState, CredentialKind, EgressCheck,
            GitAuthor, GuestState, LoginKind, ManagedWorkspace, PortProcess, Vendor, WorkspaceHost,
            WorkspaceLocation, WorkspaceState,
        };
        use crate::time::Timestamp;

        let at = Timestamp(1_792_920_600_000);
        let device_id = device("devbox").id;
        let project: ProjectId = "0199a7c2-0000-7000-8000-000000000002".parse().unwrap();
        let workspace: WorkspaceId = "0199a7c2-0000-7000-8000-000000000003".parse().unwrap();
        let port: PortId = "0199a7c2-0000-7000-8000-000000000004".parse().unwrap();
        let account: AccountId = "0199a7c2-0000-7000-8000-000000000005".parse().unwrap();
        let mac_account: AccountId = "0199a7c2-0000-7000-8000-000000000006".parse().unwrap();

        let mut host = device("devbox");
        host.winding_down_until = Some(at);
        host.workspace_host = Some(WorkspaceHost {
            networks: vec!["agentbr0".into()],
            pools: vec!["agents".into()],
        });

        let entities = [
            ("device", Entity::Device(host)),
            (
                "workspace",
                Entity::Workspace(Workspace {
                    id: workspace,
                    project: Some(project),
                    device: device_id,
                    location: WorkspaceLocation::Container {
                        instance: "mhq-maxhq".into(),
                        path: "/home/agent/work/maxhq".into(),
                    },
                    managed: Some(ManagedWorkspace {
                        state: WorkspaceState::Creating {
                            step: "Installing git".into(),
                            done: 3,
                            of: 11,
                        },
                        start_at_boot: true,
                        guest: GuestState::Down {
                            reason: "container stopped".into(),
                            since: at,
                        },
                        egress: EgressCheck::Failed {
                            at,
                            reached: vec!["192.168.1.1:443".into()],
                        },
                        recipe: 1,
                        created_at: at,
                        ssh_host_key: None,
                        unsaved: None,
                        memory_soft: None,
                    }),
                    clone: None,
                }),
            ),
            (
                "workspace",
                Entity::Workspace(Workspace {
                    id: "0199a7c2-0000-7000-8000-00000000000c".parse().unwrap(),
                    project: Some(project),
                    device: device_id,
                    location: WorkspaceLocation::Folder {
                        path: "/home/dev/src/maxhq".into(),
                    },
                    managed: None,
                    clone: Some(CloneProgress {
                        state: CloneState::Cloning {
                            stage: CloneStage::Receiving,
                            percent: Some(42),
                            stalled: false,
                        },
                    }),
                }),
            ),
            (
                "listening_port",
                Entity::ListeningPort(ListeningPort {
                    id: port,
                    workspace,
                    agent: Some("0199a7c2-0000-7000-8000-000000000007".parse().unwrap()),
                    port: 5173,
                    bind: BindAddress::Address {
                        ip: "10.0.3.14".into(),
                    },
                    process: Some(PortProcess {
                        pid: 4242,
                        name: "node".into(),
                        command: "node vite".into(),
                    }),
                    url_hint: Some("http://localhost:5173/".into()),
                    first_seen: at,
                    gone_since: None,
                }),
            ),
            (
                "forward",
                Entity::Forward(Forward {
                    id: "0199a7c2-0000-7000-8000-000000000008".parse().unwrap(),
                    port,
                    client: "0199a7c2-0000-7000-8000-000000000009".parse().unwrap(),
                    local_port: 5174,
                    opened_at: at,
                }),
            ),
            (
                "project_settings",
                Entity::ProjectSettings(ProjectSettings {
                    project,
                    device: device_id,
                    resume_on_restart: false,
                    ignored_ports: vec![9229, 24678],
                    git_author: Some(GitAuthor {
                        name: "Max Boksem".into(),
                        email: "max@example.com".into(),
                    }),
                    auto_settle_days: None,
                    settle_on_merge: true,
                    workflow_env: true,
                    memory_soft: None,
                    workflow_env_allowed: None,
                }),
            ),
            (
                "hidden_projects",
                Entity::HiddenProjects(HiddenProjects {
                    device: device_id,
                    projects: vec![project],
                }),
            ),
            (
                "command_rules",
                Entity::CommandRules(CommandRules {
                    project,
                    device: device_id,
                    rules: vec![CommandRule {
                        command: "git show".into(),
                        exact: false,
                    }],
                }),
            ),
            (
                "project_notes",
                Entity::ProjectNotes(crate::notes::ProjectNotes {
                    device: device_id,
                    project,
                    notes: vec![crate::notes::NoteSummary {
                        note: crate::notes::NoteId::derive("release"),
                        title: "Release checklist".into(),
                        head: crate::notes::VersionHash("ab".repeat(32)),
                        at,
                        by: None,
                        created_on: device_id,
                        deleted: false,
                        size: 42,
                    }],
                }),
            ),
            (
                "credential",
                Entity::Credential(Credential {
                    id: "0199a7c2-0000-7000-8000-00000000000a".parse().unwrap(),
                    device: device_id,
                    kind: CredentialKind::ClaudeToken {
                        origin: mac_account,
                        account,
                        label: "Personal".into(),
                    },
                    stored_at: at,
                    problem: None,
                }),
            ),
            (
                "credential",
                Entity::Credential(Credential {
                    id: "0199a7c2-0000-7000-8000-00000000000b".parse().unwrap(),
                    device: device_id,
                    kind: CredentialKind::ChatGpt {
                        account,
                        email: Some("max@example.com".into()),
                    },
                    stored_at: at,
                    problem: Some("refresh refused".into()),
                }),
            ),
            (
                "credential",
                Entity::Credential(Credential {
                    id: "0199a7c2-0000-7000-8000-00000000000c".parse().unwrap(),
                    device: device_id,
                    kind: CredentialKind::GithubApp {
                        app_id: 1_234_567,
                        slug: Some("maxhq-agents".into()),
                        key_sha256: "9f86d081".into(),
                        repos: vec!["example-user/maxhq".into()],
                    },
                    stored_at: at,
                    problem: None,
                }),
            ),
            (
                "account",
                Entity::Account(Account {
                    id: account,
                    device: device_id,
                    vendor: Vendor::Claude,
                    label: "Personal".into(),
                    login: LoginKind::SetupToken,
                    origin: Some(mac_account),
                    config_dir: "/home/agent/.claude".into(),
                    email: None,
                    organization: None,
                    plan: None,
                    models: vec![],
                    usage: None,
                    usage_failure: None,
                    acp: None,
                    model_server: None,
                }),
            ),
        ];
        for (tag, entity) in entities {
            assert_eq!(round_trip(&entity)["entity"], tag);
        }

        let key = EntityKey::ProjectSettings {
            project,
            device: device_id,
        };
        assert_eq!(
            serde_json::to_value(key).unwrap(),
            json!({
                "entity": "project_settings",
                "id": {
                    "project": "0199a7c2-0000-7000-8000-000000000002",
                    "device": "0199a7c2-0000-7000-8000-000000000001",
                },
            })
        );
        let key = EntityKey::CommandRules {
            project,
            device: device_id,
        };
        assert_eq!(
            serde_json::to_value(key).unwrap()["entity"],
            "command_rules"
        );
        let key = EntityKey::ProjectNotes {
            project,
            device: device_id,
        };
        assert_eq!(
            serde_json::to_value(key).unwrap()["entity"],
            "project_notes"
        );
    }

    #[test]
    fn acp_agents_and_their_accounts_survive_a_round_trip() {
        use crate::acp::{AcpAgent, AcpFeatures};
        use crate::model::{LoginKind, Vendor};

        let device_id = device("devbox").id;
        let agent = Entity::AcpAgent(AcpAgent {
            id: "0199a7c2-0000-7000-8000-0000000000e1".parse().unwrap(),
            device: device_id,
            name: "Gemini".into(),
            program: "/usr/local/bin/gemini".into(),
            args: vec!["--experimental-acp".into()],
            env: vec!["GEMINI_API_KEY".into()],
            containers: false,
            problem: Some("gemini isn't installed".into()),
            skills: Default::default(),
        });
        let value = round_trip(&agent);
        assert_eq!(value["entity"], "acp_agent");
        assert_eq!(
            serde_json::to_value(agent.key()).unwrap(),
            json!({ "entity": "acp_agent", "id": "0199a7c2-0000-7000-8000-0000000000e1" })
        );

        let account = Entity::Account(Account {
            id: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
            device: device_id,
            vendor: Vendor::Acp,
            label: "Gemini".into(),
            login: LoginKind::AcpCommand,
            origin: None,
            config_dir: String::new(),
            email: None,
            organization: None,
            plan: None,
            models: vec![],
            usage: None,
            usage_failure: None,
            acp: Some(AcpFeatures {
                images: true,
                ..AcpFeatures::default()
            }),
            model_server: None,
        });
        let value = round_trip(&account);
        assert_eq!(value["vendor"], "acp");
        assert_eq!(value["login"], "acp_command");
        assert_eq!(value["acp"]["images"], true);
    }

    #[test]
    fn m2_workspaces_and_accounts_still_parse() {
        let workspace: Workspace = serde_json::from_value(json!({
            "id": "0199a7c2-0000-7000-8000-000000000003",
            "project": null,
            "device": "0199a7c2-0000-7000-8000-000000000001",
            "location": { "type": "folder", "path": "/Users/max/Code/maxhq" },
        }))
        .unwrap();
        assert_eq!(workspace.managed, None);

        let account: Account = serde_json::from_value(json!({
            "id": "0199a7c2-0000-7000-8000-000000000005",
            "device": "0199a7c2-0000-7000-8000-000000000001",
            "vendor": "claude",
            "label": "Personal",
            "config_dir": "/Users/max/.claude",
            "email": null,
            "organization": null,
            "plan": "max",
            "models": [],
            "usage": null,
            "usage_failure": null,
        }))
        .unwrap();
        assert_eq!(account.login, crate::model::LoginKind::ConfigDir);
        assert_eq!(account.origin, None);
        assert_eq!(account.acp, None);
    }
}
