// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The JSON messages, which travel in [`crate::frame::FrameKind::Message`]
//! frames. Terminal bytes, file chunks and tunnel bytes have frames of
//! their own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::acp::{AcpAgent, AcpEnvValue};
use crate::agents::{
    AccessKey, AgentOrigin, ClientRole, Departure, McpCommand, MoveCheck, MoveTo, PeerBook,
    PeerRequest, Permission, ToolAnswer, ToolCall, ToolImage,
};
use crate::attention::{AnswerOutcome, AttentionAnswer};
use crate::context::InlineRef;
use crate::cursor_rules::{CursorRuleCall, CursorRules};
use crate::dispatch::{ChatAct, PlanOutcome};
use crate::find::{AgentExcerpt, GitItemSearch, PathMatch};
use crate::folders::FolderListing;
use crate::hosts::{MoveStep, WorkspaceSnapshot};
use crate::ids::{
    AccountId, AcpAgentId, AdmissionId, AgentId, AttachmentId, AttentionId, BootId, ClientId,
    CredentialId, DeviceId, ForwardId, IdempotencyKey, InstanceId, KeyId, LeaseId, ModelServerId,
    MoveId, PairId, PermissionId, PhoneId, PhoneSessionId, PortId, ProjectId, SignInId,
    SkillProposalId, SkillSourceId, StreamId, TaskId, TerminalId, TestHostId, TransferId,
    WorkspaceId,
};
use crate::local_models::{ModelAction, ServerFlavor, ServerModel, ServerProbe};
use crate::model::{
    CommandRule, DeviceSettingChange, DeviceSettings, EditKind, InboxChange, PermissionMode,
    ProjectSettings, Tool, Vendor, WorkspaceSpec,
};
use crate::notes::{NoteId, NoteVersion, NoteVersionInfo, VersionHash};
use crate::notices::{NoticeCall, NoticePage};
use crate::phones::{PhoneBookEntry, PhoneMac, PushService, PushToken};
use crate::processes::{ProcessList, ProcessSignal};
use crate::pulls::{
    ChecksState, PullAction, PullCheck, PullComment, PullDetail, PullFile, PullListState,
    PullSummary,
};
use crate::secret::Secret;
use crate::skills::{
    EffectiveSkill, GlobalsPlan, LockedSkill, ProposalAction, ProposalDiff, ProposalSeen,
    SkillLock, SkillProposal, SkillSource, SourceKind, SourceTrust, TreeHash,
};
use crate::state::{Delta, Seq, Snapshot, ToolKind, ToolPlace};
use crate::tasks::{HookAddress, Run, Task, TaskDelivery};
use crate::terminal::TerminalKind;
use crate::time::Timestamp;
use crate::timeline::{AttachmentRef, FindAt, ItemId, TimelineFind, TimelineUpdate};
use crate::workflow::{NodeDetail, RunKey, WorkflowUpdate};
use crate::{OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION};

/// The most bytes [`AttachmentSource::Bytes`] may hold once decoded, 8 MiB.
pub const MAX_INLINE_ATTACHMENT: usize = 8 * 1024 * 1024;

/// The protocol versions a peer speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProtocolRange {
    /// The newest version it speaks.
    pub current: u32,
    /// The oldest version it still speaks.
    pub oldest: u32,
}

impl ProtocolRange {
    /// The range this build speaks.
    pub const OURS: Self = Self {
        current: PROTOCOL_VERSION,
        oldest: OLDEST_PROTOCOL_VERSION,
    };

    /// The newest version both peers speak, if there is one.
    pub fn agree(self, other: Self) -> Option<u32> {
        let version = self.current.min(other.current);
        (version >= self.oldest.max(other.oldest)).then_some(version)
    }

    /// What `maxd`, speaking `self`, tells a client speaking `client` when
    /// they have no version in common. [`ProtocolRange::from_refusal`] reads
    /// `self` back out, so a client can say which side to update.
    pub fn refusal(self, client: Self) -> String {
        format!(
            "this maxd speaks protocol {} to {}, and the client {} to {}",
            self.oldest, self.current, client.oldest, client.current
        )
    }

    /// The range `maxd` named in a [`ProtocolRange::refusal`], or none when
    /// the message says something else.
    pub fn from_refusal(message: &str) -> Option<Self> {
        let rest = message.strip_prefix("this maxd speaks protocol ")?;
        let (range, _) = rest.split_once(',')?;
        let (oldest, current) = range.split_once(" to ")?;
        Some(Self {
            current: current.parse().ok()?,
            oldest: oldest.parse().ok()?,
        })
    }
}

/// Names the program on the other end, for logs and the device list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ClientInfo {
    /// Such as "maxhq-desktop" or "maxhq-ios".
    pub name: String,
    /// The program's own version.
    pub version: String,
}

/// The first message a client sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ClientHello {
    /// The versions the client speaks.
    pub protocol: ProtocolRange,
    /// The program connecting.
    pub client: ClientInfo,
    /// Stays the same across the client's reconnects, so answers it sent
    /// before a drop are still its own.
    pub client_id: ClientId,
    /// What kind of client connects. Left out, it's the app.
    #[serde(default)]
    pub role: ClientRole,
}

/// The first message `maxd` sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ServerHello {
    /// The versions `maxd` speaks.
    pub protocol: ProtocolRange,
    /// The `maxd` version.
    pub maxd_version: String,
    /// The device `maxd` runs on.
    pub device: DeviceId,
    /// The device's current boot. A change since the last hello means it
    /// restarted.
    pub boot_id: BootId,
    /// This run of `maxd`. Sequence numbers start over when it changes, so
    /// a client that sees a new one syncs with no `since`.
    pub instance: InstanceId,
    /// What this device does beyond agents in folders.
    pub capabilities: Capabilities,
    /// The first 16 hex characters of the sha256 of the running `maxd`. A
    /// `maxd` from before builds leaves this out.
    #[serde(default)]
    pub build: Option<String>,
    /// The Rust target `maxd` was built for, such as
    /// `x86_64-unknown-linux-musl`, so the app knows which binary to send.
    #[serde(default)]
    pub target: Option<String>,
    /// 32 random bytes, new for each connection, as base64url, unpadded.
    /// The client signs them in [`ClientMessage::Prove`]. A `maxd` from
    /// before [`APP_KEYS_SINCE`] leaves this out, and the client skips the
    /// proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,
}

/// What a device does beyond agents in folders. A client hides what a
/// device can't do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Capabilities {
    /// Takes uploads and serves downloads, per A-14.
    pub transfers: bool,
    /// Forwards ports from its agents, per N-6.
    pub tunnels: bool,
    /// Creates and runs container workspaces through Incus, per W-1.
    pub workspaces: bool,
    /// Checks its GitHub App when asked, with `CheckGithubApp`. A `maxd`
    /// from before it leaves this out, and reads as false.
    #[serde(default)]
    pub github_checks: bool,
    /// Keeps its agents' inboxes and takes `SetInbox`. A `maxd` from before
    /// it leaves this out, and reads as false.
    #[serde(default)]
    pub inbox: bool,
    /// Opens SSH sessions with its container workspaces' guests, with
    /// `GuestSshOpen` and `SetGuestSshKey`, for an editor's Remote-SSH. A
    /// `maxd` from before it leaves this out, and reads as false.
    #[serde(default)]
    pub guest_ssh: bool,
    /// Lists, fetches and places a checkout's ignored files, and clones
    /// projects into folders, for copying a project to another device. A
    /// `maxd` from before it leaves this out, and reads as false.
    #[serde(default)]
    pub copy: bool,
    /// Answers `ReadLoad` and `ReadDiagnostics`. A `maxd` from before it
    /// leaves this out, and reads as false.
    #[serde(default)]
    pub load: bool,
    /// Installs a new `maxd` or guest binary with `InstallUpdate`. A `maxd`
    /// from before it leaves this out, and reads as false.
    #[serde(default)]
    pub updates: bool,
    /// Reports its agent CLIs and updates them with `UpdateTools`. A `maxd`
    /// from before it leaves this out, and reads as false.
    #[serde(default)]
    pub tools: bool,
    /// Runs the agent tools, keeps permissions and access keys, and takes
    /// `CallTool`, `AuthKey`, `GrantPermission`, `RevokePermission`,
    /// `ListPermissions`, `MintKey` and `ListKeys`. A `maxd` from before it
    /// leaves this out, and reads as false.
    #[serde(default)]
    pub agent_tools: bool,
    /// Runs scheduled tasks and their webhooks, publishes
    /// [`crate::tasks::DeviceTasks`], and takes `SaveTask`, `DeleteTask`,
    /// `RunTaskNow`, `SetHookOn`, `RotateHookToken`, `SetHookSecret`,
    /// `SetHookAddress`, `ListRuns` and `ListDeliveries`. A `maxd` from before it leaves this
    /// out, and reads as false.
    #[serde(default)]
    pub scheduled_tasks: bool,
    /// Links to other devices' `maxd` for agent calls, keeps its own SSH
    /// key, and takes `PeerKey`, `AuthorizePeer`, `RevokePeer` and
    /// `SetPeerBook`. A `maxd` from before it leaves this out, and reads as
    /// false.
    #[serde(default)]
    pub peers: bool,
    /// Runs local ACP commands, publishes [`crate::acp::AcpAgent`], and
    /// takes `SetAcpAgent` and `DeleteAcpAgent`. A `maxd` from before it
    /// leaves this out, and reads as false.
    #[serde(default)]
    pub acp_agents: bool,
    /// Watches the PRs its agents link to and answers `ListPulls`,
    /// `GetPull`, `PullFiles`, `PullComments`, `PullChecks`, `PullsDue`,
    /// `LinkPull` and `UnlinkPull`. A `maxd` from before it leaves this
    /// out, and reads as false.
    #[serde(default)]
    pub pulls: bool,
    /// Can hold your GitHub sign-in, with `BeginGithubSignIn` and
    /// `CancelGithubSignIn`, and merges and approves with `PullAction`. A
    /// `maxd` from before it leaves this out, and reads as false.
    #[serde(default)]
    pub github_user: bool,
    /// Offers test hosts and leases, publishes `TestHost` and `TestLease`,
    /// and takes `CheckTestHosts`, `BuildTestHost`, `ResetTestHost`,
    /// `EndLease` and `ReadLeaseScreen`. A `maxd` from before it leaves
    /// this out, and reads as false.
    #[serde(default)]
    pub test_hosts: bool,
    /// Follows its projects' workflow runs, publishes `WorkflowRun`, and
    /// takes `OpenWorkflow`, `CloseWorkflow` and `NodeDetail`. A `maxd`
    /// from before it leaves this out, and reads as false.
    #[serde(default)]
    pub workflows: bool,
    /// Reads its `maxhq-host.toml`, publishes `HostReport`, and takes
    /// `CheckHost` and `ApplyHost`. A `maxd` from before it leaves this
    /// out, and reads as false.
    #[serde(default)]
    pub host_spec: bool,
    /// Takes `ListSnapshots` and `RestoreSnapshot` for its container
    /// workspaces. A `maxd` from before it leaves this out, and reads as
    /// false.
    #[serde(default)]
    pub snapshots: bool,
    /// Moves workspaces the old scripts made into MaxHQ, publishes `Move`,
    /// and takes `PlanMove`, `RunMoveStep`, `MoveBack` and `RetireMove`. A
    /// `maxd` from before it leaves this out, and reads as false.
    #[serde(default)]
    pub moves: bool,
    /// Installs and removes tools itself, publishes `AcpRegistry`, and
    /// takes `InstallTool`, `RemoveTool`, `InstallAcpAgent` and
    /// `AcceptLicence`. A `maxd` from before it leaves this out, and reads
    /// as false.
    #[serde(default)]
    pub tool_installs: bool,
    /// Keeps skills for its projects, publishes `SkillLibrary`,
    /// `ProjectSkills` and `SkillProposal`, and takes the skills requests
    /// and uploads to its skill store. A `maxd` from before it leaves this
    /// out, and reads as false.
    #[serde(default)]
    pub skills: bool,
    /// Pairs phones and pushes to them, publishes `Phone` and `PhoneBook`,
    /// and takes the phone requests, first frames, `SetApnsKey` and, since
    /// protocol 61, `SetFcmKey`. A `maxd` from before it leaves this out,
    /// and reads as false.
    #[serde(default)]
    pub phones: bool,
    /// Takes `SkillsTakeLibrary` and `SkillSourcePush`, and says which of
    /// its library's git sources have commits not pushed. A `maxd` from
    /// before it leaves this out, and reads as false.
    #[serde(default)]
    pub library_moves: bool,
    /// Takes `elsewhere` on `SkillProposalDiff`, and works out where a
    /// proposal goes from its library. A `maxd` from before it leaves this
    /// out, and reads as false.
    #[serde(default)]
    pub proposal_targets: bool,
    /// Takes a `Prove` with the device's own key on a link from another
    /// device, and serves the link only once it has one. A `maxd` from
    /// before it leaves this out, and reads as false.
    #[serde(default)]
    pub peer_proofs: bool,
    /// Holds terminals for its agents, publishes
    /// [`crate::terminal::Terminal`], and takes the terminal requests and
    /// frames. A `maxd` from before it leaves this out, and reads as false.
    #[serde(default)]
    pub terminals: bool,
    /// Runs or reaches model servers for local agents, publishes
    /// [`crate::local_models::ModelServer`] and
    /// [`crate::local_models::LocalModels`], and takes
    /// [`ClientMessage::ModelAction`]. A `maxd` from before it leaves this
    /// out, and reads as false.
    #[serde(default)]
    pub local_models: bool,
}

/// A message from a client to `maxd`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Opens the session. Nothing else is accepted before it.
    Hello(ClientHello),
    /// Asks for the state. `maxd` sends the deltas after `since` if it still
    /// has them, and a snapshot otherwise.
    Sync {
        /// The last version the client holds, or none for a snapshot.
        since: Option<Seq>,
    },
    /// Answers an attention event.
    Answer {
        /// Makes a resend after a dropped connection safe.
        idempotency_key: IdempotencyKey,
        /// The event being answered.
        event: AttentionId,
        /// The answer.
        answer: AttentionAnswer,
        /// The pills in what you typed, as in [`UserInput::refs`]: a text
        /// answer's text, or each question's typed answer in turn, has
        /// U+FFFC where each placed one sits. `maxd` gives the agent each
        /// one's marker and context block, as it does for a message. None
        /// before [`ANSWER_PILLS_SINCE`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        refs: Vec<InlineRef>,
    },
    /// Says you've seen an agent's finished turn, which clears
    /// [`crate::model::Activity::DoneUnread`].
    MarkSeen {
        /// The agent you opened.
        agent: AgentId,
    },
    /// Checks usage now instead of at the next scheduled check.
    RefreshUsage {
        /// The account to check, or none for every account on the device.
        account: Option<AccountId>,
    },
    /// Checks the connection is alive.
    Ping {
        /// Echoed in the pong.
        nonce: u32,
    },
    /// Starts a new agent.
    StartAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// What to start.
        spec: AgentSpec,
    },
    /// Sends a message to an agent.
    SendInput {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to send to.
        agent: AgentId,
        /// What to send.
        input: UserInput,
        /// What to do when the agent is busy.
        mode: SendMode,
    },
    /// Takes back a queued message before it's sent.
    CancelQueued {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent the message is queued for.
        agent: AgentId,
        /// The queued message's timeline item.
        item: ItemId,
    },
    /// Stops the agent's current turn.
    Interrupt {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to interrupt.
        agent: AgentId,
    },
    /// Changes an agent's permission mode.
    SetPermission {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to change.
        agent: AgentId,
        /// The mode to run in.
        mode: PermissionMode,
    },
    /// Changes an agent's model.
    SetModel {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to change.
        agent: AgentId,
        /// A [`crate::model::ModelChoice::id`] from the agent's account, or
        /// from `account` when it's set.
        model: String,
        /// Another account on the agent's device, such as one of another
        /// vendor. The agent moves to it, carrying its conversation over.
        /// Left out, the agent keeps its account.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<AccountId>,
    },
    /// Changes the effort level an agent's next turn runs at. Only Claude
    /// Code and Codex agents take one. Since [`EFFORT_SINCE`].
    SetEffort {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to change.
        agent: AgentId,
        /// One of [`crate::model::ModelChoice::efforts`], or none for the
        /// model's [`crate::model::ModelChoice::default_effort`], which
        /// `maxd` names to the CLI. `maxd` passes a level it doesn't know
        /// on to the CLI.
        #[serde(default)]
        effort: Option<String>,
    },
    /// Ends an agent's process. The conversation stays and can be resumed.
    StopAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to stop.
        agent: AgentId,
    },
    /// Deletes a lead agent MaxHQ holds, with its sub-agents, stopping its
    /// process first if one runs. Its folder and the CLI's own transcript
    /// stay, but `maxd` never shows it again. An agent you started in your
    /// own terminal is refused.
    DeleteAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to delete.
        agent: AgentId,
    },
    /// Names an agent, held or seen outside MaxHQ. The name stands over
    /// any title its CLI or observer gives, through restarts, until the
    /// agent is deleted. `maxd` trims it and cuts it to the length of a
    /// new agent's title. A blank one clears it, so the title from before
    /// shows again.
    RenameAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent to name.
        agent: AgentId,
        /// The new name, or a blank one to clear it.
        title: String,
    },
    /// Changes where an agent sits in the sidebar.
    SetInbox {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The lead agent to change.
        agent: AgentId,
        /// What to change.
        change: InboxChange,
    },
    /// Takes over an agent you started in your own terminal, so MaxHQ holds
    /// it from now on.
    ContinueHere {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The observed agent to take over.
        agent: AgentId,
    },
    /// Stops the CLI that runs an agent in your own terminal on this
    /// device, then continues it here as [`ClientMessage::ContinueHere`]
    /// does. Only Claude Code and Codex sessions qualify.
    TakeOver {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The observed agent to take over.
        agent: AgentId,
        /// When to stop the CLI, or to cancel a wait for its turn to end.
        mode: TakeOverMode,
    },
    /// Readies a file or folder to send. The outcome names it for
    /// [`UserInput::attachments`].
    StageAttachment {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Where the attachment comes from.
        source: AttachmentSource,
    },
    /// Changes the device's settings.
    SetDeviceSettings {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The settings to keep.
        settings: DeviceSettings,
    },
    /// Changes some of the device's settings and leaves the rest as it has
    /// them. An older `maxd` doesn't read it, so a client sends a
    /// [`Self::SetDeviceSettings`] instead.
    UpdateDeviceSettings {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device, which must be the one that gets this.
        device: DeviceId,
        /// The settings to change.
        changes: Vec<DeviceSettingChange>,
    },
    /// Starts sending an agent's timeline. `maxd` sends the changes after
    /// `since` if it still has them, and a reset otherwise.
    OpenTimeline {
        /// The agent whose timeline to send.
        agent: AgentId,
        /// The last timeline version the client holds, or none for a reset.
        since: Option<Seq>,
        /// The most items a reset may hold.
        limit: u32,
        /// The lead `agent` runs under, when it's a sub-agent. A `maxd`
        /// from [`FINISHED_SUB_AGENTS_SINCE`] on reads a sub-agent it no
        /// longer lists, such as one that finished, from its transcript
        /// beside the lead's. An older one ignores it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lead: Option<AgentId>,
    },
    /// Asks for items before the oldest the client has.
    LoadOlder {
        /// The agent whose timeline it is.
        agent: AgentId,
        /// The ordinal of the oldest item the client has.
        #[specta(type = specta_typescript::Number)]
        before: u64,
        /// The most items to send.
        limit: u32,
    },
    /// Stops sending an agent's timeline.
    CloseTimeline {
        /// The agent whose timeline to stop sending.
        agent: AgentId,
    },
    /// Makes a container workspace. The outcome is
    /// [`ActionOutcome::Created`], and the workspace shows its progress.
    CreateWorkspace {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// What to make.
        spec: WorkspaceSpec,
    },
    /// Starts a stopped container workspace.
    StartWorkspace {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace to start.
        workspace: WorkspaceId,
    },
    /// Stops a running container workspace.
    StopWorkspace {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace to stop.
        workspace: WorkspaceId,
    },
    /// Deletes a container workspace. Refused while a worktree has commits
    /// or changes not on the remote, unless `discard_unpushed` is set.
    DeleteWorkspace {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace to delete.
        workspace: WorkspaceId,
        /// Deletes work that isn't on the remote too.
        discard_unpushed: bool,
    },
    /// Changes one project's settings on the device.
    SetProjectSettings {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The settings to keep.
        settings: ProjectSettings,
    },
    /// Hides a project from the sidebar, or shows it again, in the list
    /// the device keeps. Only a device that publishes
    /// [`crate::state::Entity::HiddenProjects`] takes it.
    SetProjectHidden {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device that keeps the list.
        device: DeviceId,
        /// The project.
        project: ProjectId,
        /// True hides it, false shows it again.
        hidden: bool,
    },
    /// Removes one of a project's command rules from the device that
    /// keeps them. Only a device that publishes
    /// [`crate::state::Entity::CommandRules`] for the project has any.
    RemoveCommandRule {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device that keeps the rules.
        device: DeviceId,
        /// The project.
        project: ProjectId,
        /// The rule to remove.
        rule: CommandRule,
    },
    /// Turns off a kind of file you remembered for a project, so agents in
    /// Accept edits or Auto ask again before they change one. Since
    /// protocol 57, [`ALLOWED_EDITS_SINCE`].
    RemoveAllowedEdit {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device that keeps the project's rules.
        device: DeviceId,
        /// The project.
        project: ProjectId,
        /// The kind to ask about again.
        kind: EditKind,
    },
    /// A-11. Asks busy agents to wind down and marks each one when the
    /// window closes.
    WindDown {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// How long agents get to finish what they're on.
        window_secs: u32,
    },
    /// W-5. Keeps a Claude setup-token, which gives the device a token
    /// account. Refused while the device's
    /// [`DeviceSettings::claude_setup_token`] is off.
    StoreClaudeToken {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The Mac account the token was minted for.
        origin: AccountId,
        /// Your name for the token account.
        label: String,
        /// The token.
        token: Secret,
    },
    /// Keeps a Cursor API key, which gives the device a key account. A
    /// second key replaces the first.
    StoreCursorKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Your name for the key account.
        label: String,
        /// The key.
        token: Secret,
    },
    /// W-5. Starts a Sign in with ChatGPT round for this device. The
    /// outcome is [`ActionOutcome::SignIn`].
    BeginChatGptSignIn {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
    },
    /// W-5. Hands back what the browser's callback got, which finishes the
    /// round.
    FinishChatGptSignIn {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The round this finishes.
        sign_in: SignInId,
        /// The code the callback got.
        code: Secret,
        /// The `state` the callback got, which must match the round's.
        state: String,
        /// The `client_id` the callback got, if any. A first sign-in
        /// registers the device with ChatGPT, and this names that client.
        #[serde(default)]
        client_id: Option<String>,
    },
    /// W-6. Gives the device the GitHub App it mints tokens from. The
    /// device keeps the key and never sends it back.
    SetGithubApp {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// GitHub's ID for the App.
        #[specta(type = specta_typescript::Number)]
        app_id: u64,
        /// The App's private key, as PEM.
        private_key: Secret,
    },
    /// Checks the device's GitHub App now, instead of at its next check.
    /// The device then publishes the App's credential with what it found.
    /// Only a device whose hello says `github_checks` reads this.
    CheckGithubApp {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
    },
    /// Removes a login or key from the device. For a ChatGPT sign-in the
    /// outcome is [`ActionOutcome::SignedOut`].
    ForgetCredential {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The credential to remove.
        credential: CredentialId,
    },
    /// Starts sending files, or picks a dropped upload up again with the
    /// same key and transfer. Report 13 section 7, with `target` in place
    /// of `agent`.
    UploadBegin {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Names the upload, and its chunks in transfer frames.
        transfer: TransferId,
        /// Where the files go.
        target: UploadTarget,
        /// Where the files come from.
        source: UploadSource,
    },
    /// Gives up on an upload.
    UploadCancel {
        /// The upload to stop.
        transfer: TransferId,
    },
    /// Starts sending a file or folder from an agent's place to the client.
    FetchBegin {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Names the download, and its chunks in transfer frames.
        transfer: TransferId,
        /// The agent whose place holds it.
        agent: AgentId,
        /// The path, relative to the agent's folder or absolute inside it.
        path: String,
        /// Reads a file at an absolute path outside the agent's folders,
        /// because you asked for it by name with Show anyway. Only an app's
        /// own connection may set it. A client from before sends none.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        anyway: bool,
    },
    /// How much of a download the client has, and how much more it takes.
    TransferCredit {
        /// The download.
        transfer: TransferId,
        /// Bytes written down for good.
        #[specta(type = specta_typescript::Number)]
        committed: u64,
        /// The sender may send up to this offset.
        #[specta(type = specta_typescript::Number)]
        window_end: u64,
    },
    /// Opens a forward to a listening port. The client has bound
    /// `local_port` already. The outcome is [`ActionOutcome::ForwardOpened`].
    OpenForward {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The port to reach.
        port: PortId,
        /// The port the client listens on.
        local_port: u16,
    },
    /// Closes a forward and every stream through it.
    CloseForward {
        /// The forward to close.
        forward: ForwardId,
    },
    /// Opens one connection through a forward, for a connection the client
    /// accepted on its local port.
    TunnelOpen {
        /// Names the connection, and its bytes in tunnel frames.
        stream: StreamId,
        /// The forward it goes through.
        forward: ForwardId,
    },
    /// Lets the device send more of a stream's bytes.
    TunnelCredit {
        /// The stream.
        stream: StreamId,
        /// More bytes the device may send.
        #[specta(type = specta_typescript::Number)]
        granted: u64,
    },
    /// Ends a stream, or the client's half of it.
    TunnelClose {
        /// The stream.
        stream: StreamId,
        /// How it ends.
        how: TunnelEnd,
    },
    /// Opens an SSH session with a running container workspace's guest,
    /// carried as a tunnel stream, for `maxd proxy --guest-ssh`. The answer
    /// is `TunnelOpened` or `TunnelRefused`, and the bytes then flow as for
    /// any stream. Only a device whose hello says `guest_ssh` reads this.
    GuestSshOpen {
        /// Names the session, and its bytes in tunnel frames.
        stream: StreamId,
        /// The container workspace.
        workspace: WorkspaceId,
    },
    /// Gives the device this client's SSH public key, which signs it in to
    /// its container workspaces' guests. A client has one key, and a new
    /// one replaces the last. Only a device whose hello says `guest_ssh`
    /// reads this.
    SetGuestSshKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The key, as an OpenSSH `ssh-ed25519` line.
        public_key: String,
    },
    /// Looks for files and folders in an agent's folder, for the
    /// composer's `@` menu. The outcome is [`ActionOutcome::Paths`]. A
    /// resend searches again.
    FindPaths {
        /// Matches the result.
        key: IdempotencyKey,
        /// The agent whose folder to search.
        agent: AgentId,
        /// What you typed after `@`.
        query: String,
    },
    /// Looks for pull requests and issues in the GitHub repo of an agent's
    /// folder, for the composer's `#` menu. A query of digits looks that
    /// number up first. The outcome is [`ActionOutcome::GitItems`]. A
    /// resend looks again.
    FindGitItems {
        /// Matches the result.
        key: IdempotencyKey,
        /// The agent whose repo to look in.
        agent: AgentId,
        /// What you typed after `#`.
        query: String,
        /// Another repo to look in instead, "owner/name", such as one a
        /// pasted link names. The agent's device reads it with the GitHub
        /// App or `gh` it already has, as it reads the agent's own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    /// Lists the processes in the tree under an agent's holder, for the
    /// ports popover while it's open. The outcome is
    /// [`ActionOutcome::Processes`]. A resend lists again.
    ListProcesses {
        /// Matches the result.
        key: IdempotencyKey,
        /// The agent.
        agent: AgentId,
    },
    /// Sends a signal to one process in an agent's tree. `maxd` lists the
    /// tree again first and signals only a process in it now, with the
    /// same `pid` and `start`, and never the agent's CLI. The outcome is
    /// done, or refused with why. A resend signals again.
    SignalProcess {
        /// Matches the result.
        key: IdempotencyKey,
        /// The agent whose tree it's in.
        agent: AgentId,
        /// The process, from [`crate::processes::AgentProcess::pid`].
        pid: u32,
        /// When it started, from [`crate::processes::AgentProcess::start`].
        #[specta(type = specta_typescript::Number)]
        start: u64,
        /// The signal.
        signal: ProcessSignal,
    },
    /// Reads one version of a project note from the device's copy, its
    /// head when `version` is none. The outcome is
    /// [`ActionOutcome::NoteRead`]. A resend reads again.
    ReadNote {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project the note is in.
        project: ProjectId,
        /// The note.
        note: NoteId,
        /// The version to read, or none for the head.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<VersionHash>,
    },
    /// Saves a new version of a project note. With no `base` it creates
    /// the note, and a note that already has that ID merges against an
    /// empty one. With a base that's no longer the head, `maxd` keeps the
    /// new version after its base and merges it with the head. The outcome
    /// is [`ActionOutcome::NoteSaved`]. A resend saves the same text again,
    /// which changes nothing.
    SaveNote {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project the note is in.
        project: ProjectId,
        /// The note.
        note: NoteId,
        /// The version the text was edited from, or none for a new note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<VersionHash>,
        /// The title.
        title: String,
        /// The body, as Markdown.
        body: String,
        /// Whether the note is deleted. Restoring one saves it with this
        /// false.
        deleted: bool,
    },
    /// Lists a note's versions, newest first, at most
    /// [`crate::notes::MAX_NOTE_HISTORY`], without their bodies. The
    /// outcome is [`ActionOutcome::NoteHistory`].
    NoteHistory {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project the note is in.
        project: ProjectId,
        /// The note.
        note: NoteId,
    },
    /// Lists the hash of every note version the device holds for a
    /// project, for syncing notes. The outcome is
    /// [`ActionOutcome::NoteHashes`].
    NoteHashes {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project.
        project: ProjectId,
    },
    /// Reads note versions by hash, for syncing notes. The answer may hold
    /// only the first few when they're large, and the caller asks again
    /// for the rest. The outcome is [`ActionOutcome::NoteVersions`].
    NoteVersions {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project.
        project: ProjectId,
        /// The versions to read.
        hashes: Vec<VersionHash>,
    },
    /// Hands the device note versions it lacks, parents first, for
    /// syncing notes. It checks each hash and merges any heads that
    /// result. The outcome is [`ActionOutcome::NotesPut`].
    PutNoteVersions {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project.
        project: ProjectId,
        /// The versions.
        versions: Vec<NoteVersion>,
    },
    /// Writes an agent's timeline as Markdown cut to `budget` characters,
    /// for an agent pill going where MaxHQ's read tool can't reach the
    /// agent. The outcome is [`ActionOutcome::Excerpt`].
    ExcerptAgent {
        /// Matches the result.
        key: IdempotencyKey,
        /// The agent.
        agent: AgentId,
        /// The most characters the excerpt may take.
        budget: u32,
    },
    /// Looks for text across an agent's whole timeline, for the agent
    /// view's find bar. The outcome is [`ActionOutcome::TimelineFind`]. A
    /// resend looks again.
    FindInTimeline {
        /// Matches the result.
        key: IdempotencyKey,
        /// The agent whose timeline to look in.
        agent: AgentId,
        /// What to look for. Matches ignore case. 1 to 200 characters.
        query: String,
        /// Which match to make active.
        at: FindAt,
    },
    /// Lists the files git ignores in a workspace's checkout, such as
    /// `.env`, for copying a project. Wholly ignored folders such as
    /// `node_modules` don't appear. The outcome is
    /// [`ActionOutcome::IgnoredFiles`]. A resend lists again.
    ListIgnoredFiles {
        /// Matches the result.
        key: IdempotencyKey,
        /// The workspace whose checkout to list.
        workspace: WorkspaceId,
    },
    /// Starts sending some files from a workspace's checkout to the client,
    /// as [`ClientMessage::FetchBegin`] does for an agent's file. The
    /// device answers with [`ServerMessage::FetchManifest`] and chunk
    /// frames, or [`ServerMessage::TransferFailed`].
    FetchFiles {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Names the download, and its chunks in transfer frames.
        transfer: TransferId,
        /// The workspace whose checkout holds them.
        workspace: WorkspaceId,
        /// Regular files, relative to the checkout, with `/` between parts.
        paths: Vec<String>,
    },
    /// Starts sending a tree in this device's skill store to the client,
    /// to copy to another device's store, as [`ClientMessage::FetchBegin`]
    /// does for an agent's file.
    FetchSkillTree {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Names the download, and its chunks in transfer frames.
        transfer: TransferId,
        /// The tree.
        tree: TreeHash,
    },
    /// Moves a staged upload's files into a workspace's checkout, keeping
    /// their paths. The outcome is [`ActionOutcome::Placed`].
    PlaceStaged {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace whose checkout they go into.
        workspace: WorkspaceId,
        /// The staged upload, from [`ActionOutcome::Staged`].
        attachment: AttachmentId,
        /// Whether to replace files already there.
        overwrite: bool,
    },
    /// Clones a project into a new folder on a device's host, which
    /// becomes a folder workspace. The outcome is
    /// [`ActionOutcome::Created`], and the workspace shows the clone's
    /// progress.
    CloneFolder {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The project to clone, whose repo the device clones from.
        project: ProjectId,
        /// The device to clone it on.
        device: DeviceId,
        /// The folder to clone into, as an absolute path on the device.
        parent: String,
        /// The new folder's name.
        name: String,
    },
    /// Stops a running clone and removes the folder it made.
    CloneCancel {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace being cloned.
        workspace: WorkspaceId,
    },
    /// Starts a failed or cancelled clone again.
    CloneRetry {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace that was being cloned.
        workspace: WorkspaceId,
    },
    /// Stops a clone if it runs, removes its partial folder, and forgets
    /// the workspace.
    CloneRemove {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace being cloned.
        workspace: WorkspaceId,
    },

    // Step 4: fleet tools.
    /// Installs a binary uploaded to the device's staging folder. For
    /// `maxd`, the device answers, then restarts on the new binary. For the
    /// guest binary, each container workspace MaxHQ made picks it up. The
    /// outcome is [`ActionOutcome::Done`] once the binary is in place, or a
    /// refusal.
    InstallUpdate {
        /// Matches the result.
        key: IdempotencyKey,
        /// The uploaded binary.
        staged: AttachmentId,
        /// Which binary it is.
        kind: UpdateKind,
        /// The binary's sha256 in lowercase hex, checked before anything
        /// changes.
        sha256: String,
        /// The signed release manifest that lists the binary, from the
        /// folder it came from. A `maxd` from [`SIGNED_UPDATES_SINCE`] on
        /// checks it before it runs or installs the upload, and refuses an
        /// upload without one unless `unsigned` is set. A client from
        /// before sends none, and an older `maxd` ignores it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release: Option<SignedRelease>,
        /// You chose to send a binary no release signed, such as one built
        /// from source on the Mac, after the app named the risk. Only the
        /// Mac's own connection may set it, never a phone. A client from
        /// before sends none, and reads as false.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unsigned: bool,
    },
    /// Updates agent CLIs on the device and in its workspaces. Each one's
    /// progress shows in [`crate::state::DeviceTools`]. The outcome is
    /// [`ActionOutcome::Done`] once they've all started, or a refusal.
    UpdateTools {
        /// Matches the result.
        key: IdempotencyKey,
        /// The CLIs to update.
        tools: Vec<ToolTarget>,
    },
    /// Reads the device's free CPU and memory. The outcome is
    /// [`ActionOutcome::Load`].
    ReadLoad {
        /// Matches the result.
        key: IdempotencyKey,
    },
    /// Reads what `maxd`, its agents and its container workspaces have used
    /// lately. The outcome is [`ActionOutcome::Diagnostics`].
    ReadDiagnostics {
        /// Matches the result.
        key: IdempotencyKey,
        /// How far back to look, at most an hour.
        window_secs: u32,
        /// How long each bucket is.
        bucket_secs: u32,
    },

    // Step 5: agents that run agents.
    /// Signs an `mcp` connection in with an access key. It has to be the
    /// first message after hello on that connection. The outcome is
    /// [`ActionOutcome::Done`], or a refusal, after which no call goes
    /// through.
    AuthKey {
        /// Matches the result.
        key: IdempotencyKey,
        /// The access key, as `mhq_<key id>_<secret>`.
        secret: Secret,
    },
    /// Calls an agent tool as the access key this connection signed in
    /// with. The outcome is [`ActionOutcome::Tool`].
    CallTool {
        /// Makes a resend safe: the call runs once.
        key: IdempotencyKey,
        /// The call.
        call: ToolCall,
    },
    /// Stores a permission, or replaces the one with its ID. The counts of
    /// what it started so far stay.
    GrantPermission {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The permission.
        permission: Box<Permission>,
    },
    /// Removes a permission. The agents it started keep running, but
    /// nothing can drive them through it.
    RevokePermission {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The permission.
        permission: PermissionId,
    },
    /// Lists the device's permissions. The outcome is
    /// [`ActionOutcome::Permissions`].
    ListPermissions {
        /// Matches the result.
        key: IdempotencyKey,
    },
    /// Makes an access key and writes it to `<name>.key` in the `keys`
    /// folder of `maxd`'s data folder on the device, readable only by you.
    /// `permission`'s holder becomes the new key. The outcome is
    /// [`ActionOutcome::KeyMinted`], which never holds the key itself.
    MintKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The key file's name, letters, digits, `-` and `_`.
        name: String,
        /// What the key may do.
        permission: Box<Permission>,
    },
    /// Lists the access keys the device made. The outcome is
    /// [`ActionOutcome::Keys`]. Revoking a key's permission removes the key.
    ListKeys {
        /// Matches the result.
        key: IdempotencyKey,
    },

    // Step 6: scheduled tasks.
    /// Stores a task on this device, its home, or replaces the one with its
    /// ID. The outcome is [`ActionOutcome::TaskSaved`].
    SaveTask {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The task. `maxd` sets `next_run` and `last_run`.
        task: Box<Task>,
    },
    /// Turns a task this device holds on or off, and changes nothing else
    /// about it. The outcome is [`ActionOutcome::TaskSaved`]. An older
    /// `maxd` doesn't read it, so a client sends a [`Self::SaveTask`] instead.
    SetTaskEnabled {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The task, which this device must hold.
        task: TaskId,
        /// Whether it runs.
        enabled: bool,
    },
    /// Removes a task. A run in progress goes on.
    DeleteTask {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
    },
    /// Runs a task now, whatever its trigger. The outcome is
    /// [`ActionOutcome::Done`] once the run is queued.
    RunTaskNow {
        /// Makes a resend safe: the task runs once.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
    },
    /// Turns a task's webhook on or off.
    SetHookOn {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
        /// Whether it takes requests.
        on: bool,
    },
    /// Gives a task's webhook a new URL. The old one stops working. The
    /// outcome is [`ActionOutcome::HookUrl`], the only time the URL shows.
    RotateHookToken {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
    },
    /// Stores the secret a task's webhook checks signatures with. It never
    /// comes back.
    SetHookSecret {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
        /// The secret.
        secret: Secret,
    },
    /// Picks how this device's webhooks are reached. Hooks keep their
    /// tokens, so each URL changes only in its base.
    SetHookAddress {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The new address.
        address: HookAddress,
    },
    /// Lists a task's recent runs. The outcome is [`ActionOutcome::Runs`].
    ListRuns {
        /// Matches the result.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
    },
    /// Lists a task's recent webhook requests or GitHub events. The outcome
    /// is [`ActionOutcome::Deliveries`].
    ListDeliveries {
        /// Matches the result.
        key: IdempotencyKey,
        /// The task.
        task: TaskId,
    },

    // Step 5: links between devices.
    /// Names the device whose key opened this connection. Only `maxd proxy
    /// --peer` sends it, as the connection's first frame, and the
    /// connection may then carry only peer requests and their answers.
    Peer {
        /// The device.
        device: DeviceId,
    },
    /// Asks the device for something over a peer link. The outcome
    /// depends on the request.
    PeerRequest {
        /// Makes a resend after a dropped link safe.
        key: IdempotencyKey,
        /// The request. One from a newer `maxd` reads as unknown.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = PeerRequest)]
        request: PeerRequest,
    },
    /// Answers a [`ServerMessage::PeerRequest`].
    PeerAnswer {
        /// The key the request carried.
        key: IdempotencyKey,
        /// What came of it. One from a newer `maxd` reads as unknown.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = ActionOutcome)]
        outcome: ActionOutcome,
    },
    /// The public half of the device's own SSH key, made the first time.
    /// The outcome is [`ActionOutcome::PeerKey`].
    PeerKey {
        /// Matches the result.
        key: IdempotencyKey,
    },
    /// Lets another device's key in, to run only `maxd proxy --peer` for
    /// that device.
    AuthorizePeer {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device the key belongs to.
        device: DeviceId,
        /// Its public key, as `ssh-ed25519 <base64>`.
        public_key: String,
    },
    /// Takes out another device's key and closes its links.
    RevokePeer {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device.
        device: DeviceId,
    },
    /// Replaces the devices this one may link to.
    SetPeerBook {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The devices.
        book: PeerBook,
    },

    // Step 9: any ACP agent.
    /// Stores a local ACP command on this device, or replaces the one with
    /// its ID. Only the app may send it.
    SetAcpAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The definition. `maxd` sets `device` and `problem`.
        agent: AcpAgent,
        /// Values for the variables that are new or changed. A variable in
        /// `agent.env` that isn't here keeps its value, and one that's left
        /// out of `agent.env` loses it.
        #[serde(default)]
        env: Vec<AcpEnvValue>,
    },
    /// Removes a local ACP command and its values. Refused while an agent
    /// of it runs.
    DeleteAcpAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The definition.
        id: AcpAgentId,
    },

    // Step 7: pull requests.
    /// Starts signing in to GitHub as you on this device, with the GitHub
    /// App's device flow. A second one cancels the first. The outcome is
    /// [`ActionOutcome::DeviceCode`], and the device publishes a
    /// [`crate::model::CredentialKind::GithubUser`] once you authorize it.
    /// Sign-out is [`ClientMessage::ForgetCredential`].
    BeginGithubSignIn {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The GitHub App's client ID, for a device with no App of its own.
        /// A device with an App uses its own.
        #[serde(default)]
        client_id: Option<String>,
    },
    /// Stops a GitHub sign-in that waits for you.
    CancelGithubSignIn {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The sign-in, from [`ActionOutcome::DeviceCode`].
        sign_in: SignInId,
    },
    /// Asks how a GitHub sign-in stands. [`ActionOutcome::Done`] while it
    /// waits for you and once you're signed in; [`ActionOutcome::Refused`]
    /// with why once it ended otherwise, such as when you turned it down
    /// on GitHub or its code ran out.
    CheckGithubSignIn {
        /// Matches the result.
        key: IdempotencyKey,
        /// The sign-in, from [`ActionOutcome::DeviceCode`].
        sign_in: SignInId,
    },

    // Signing in again.
    /// Runs the CLI's own login for a Claude, Codex or Cursor account that
    /// keeps its login in a config folder, on the device that holds it. A
    /// `maxd` before [`CURSOR_LOGIN_SINCE`] refuses a Cursor one. The CLI
    /// stores the login, and MaxHQ never sees it. A second one for the
    /// same account stops the first. The outcome is
    /// [`ActionOutcome::CliLogin`].
    BeginCliLogin {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The account to sign in.
        account: AccountId,
        /// Signs a Codex account in with a code to enter on any device,
        /// in place of a page that comes back to this one.
        #[serde(default)]
        device_code: bool,
    },
    /// Gives a Claude login the code its sign-in page showed. The outcome
    /// is [`ActionOutcome::CliLoginState`].
    FinishCliLogin {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The sign-in, from [`ActionOutcome::CliLogin`].
        sign_in: SignInId,
        /// The code from the page.
        code: Secret,
    },
    /// Asks how a CLI login stands. The outcome is
    /// [`ActionOutcome::CliLoginState`].
    CheckCliLogin {
        /// Matches the result.
        key: IdempotencyKey,
        /// The sign-in, from [`ActionOutcome::CliLogin`].
        sign_in: SignInId,
    },
    /// Stops a CLI login that waits for you.
    CancelCliLogin {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The sign-in, from [`ActionOutcome::CliLogin`].
        sign_in: SignInId,
    },
    /// Lists a project's PRs. The outcome is [`ActionOutcome::Pulls`].
    ListPulls {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project, whose repository to list.
        project: ProjectId,
        /// Open ones, or closed and merged ones.
        state: PullListState,
    },
    /// Lists the branches of a project's repository, for the new-agent
    /// form to suggest. The outcome is [`ActionOutcome::Branches`]. A
    /// `maxd` from before it answers with a bad message, and the form
    /// suggests what it already has.
    ListBranches {
        /// Matches the result.
        key: IdempotencyKey,
        /// The project, whose repository to list.
        project: ProjectId,
    },
    /// Reads one PR for the viewer. The outcome is [`ActionOutcome::Pull`].
    GetPull {
        /// Matches the result.
        key: IdempotencyKey,
        /// The repository, as "owner/name".
        repo: String,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Reads a page of the files a PR changes. The outcome is
    /// [`ActionOutcome::PullFiles`].
    PullFiles {
        /// Matches the result.
        key: IdempotencyKey,
        /// The repository, as "owner/name".
        repo: String,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
        /// The page, from 1.
        page: u32,
    },
    /// Reads a PR's comments. The outcome is
    /// [`ActionOutcome::PullComments`].
    PullComments {
        /// Matches the result.
        key: IdempotencyKey,
        /// The repository, as "owner/name".
        repo: String,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Reads the checks on a PR's head. The outcome is
    /// [`ActionOutcome::PullChecks`].
    PullChecks {
        /// Matches the result.
        key: IdempotencyKey,
        /// The repository, as "owner/name".
        repo: String,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Merges or approves a PR as you. Only the device holding your GitHub
    /// sign-in takes it, and only from the app. The outcome of a merge is
    /// [`ActionOutcome::PullMerged`].
    PullAction {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The repository, as "owner/name".
        repo: String,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
        /// What to do.
        action: PullAction,
        /// The head commit you saw. The action fails if the PR moved on.
        expect_sha: String,
    },
    /// Tells the device a repository's PRs changed, such as after a merge
    /// from the app, so it reads them again now.
    PullsDue {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The repository, as "owner/name".
        repo: String,
    },
    /// Links a PR in the agent's repository to the agent.
    LinkPull {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent.
        agent: AgentId,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Unlinks a PR from the agent, so a branch match doesn't bring it
    /// back.
    UnlinkPull {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent.
        agent: AgentId,
        /// The repository, as "owner/name".
        repo: String,
        /// The PR's number.
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },

    // Step 8: test hosts.
    /// Looks again at what this device can host, such as after you
    /// install Xcode or the Android SDK.
    CheckTestHosts {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
    },
    /// Builds a test host that's off or failed: its VM, container,
    /// Simulator or emulator, and agent-device inside.
    BuildTestHost {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The host.
        host: TestHostId,
    },
    /// Takes a test host back to a clean state. Ends its lease, if it has
    /// one.
    ResetTestHost {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The host.
        host: TestHostId,
    },
    /// Ends a lease now. Its holder's next call says you ended it.
    EndLease {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The lease.
        lease: LeaseId,
    },
    /// Takes a screenshot of a ready lease's machine, for the lease view.
    /// The outcome is [`ActionOutcome::LeaseScreen`].
    ReadLeaseScreen {
        /// Matches the result.
        key: IdempotencyKey,
        /// The lease.
        lease: LeaseId,
    },

    // M6: hosts, snapshots, moves and installs.
    /// Installs a tool in one place, or moves it to another version.
    InstallTool {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Where.
        place: ToolPlace,
        /// Which tool.
        tool: ToolKind,
        /// The version, or none for the latest.
        version: Option<String>,
    },
    /// Removes a tool MaxHQ installed from one place.
    RemoveTool {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Where.
        place: ToolPlace,
        /// Which tool.
        tool: ToolKind,
    },
    /// Reads the host spec and checks the device against it again.
    CheckHost {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
    },
    /// Fixes the host report's items that `maxd` can fix itself.
    ApplyHost {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The items, by [`crate::hosts::HostItem::id`].
        items: Vec<String>,
    },
    /// Lists a container workspace's snapshots. The outcome is
    /// [`ActionOutcome::Snapshots`].
    ListSnapshots {
        /// Matches the result.
        key: IdempotencyKey,
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// Takes a container workspace back to a snapshot, or makes a new
    /// workspace from it.
    RestoreSnapshot {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The workspace.
        workspace: WorkspaceId,
        /// The snapshot's name.
        snapshot: String,
        /// Makes a new workspace from the snapshot and leaves this one as
        /// it is.
        as_new: bool,
        /// Goes ahead with a roll back over unsaved work the person was
        /// shown. A client from before it leaves this out, and reads as
        /// false.
        #[serde(default)]
        discard_unsaved: bool,
    },
    /// Looks at a container the old scripts made and plans its move. The
    /// device publishes the plan as a [`crate::hosts::Move`].
    PlanMove {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The container's name.
        instance: String,
    },
    /// Runs one step of a move.
    RunMoveStep {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The move.
        moving: MoveId,
        /// The step.
        step: MoveStep,
    },
    /// Switches a move back to the old container while it's kept.
    MoveBack {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The move.
        moving: MoveId,
    },
    /// Deletes a moved workspace's old container now, before its keep ends.
    RetireMove {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The move.
        moving: MoveId,
    },
    /// Installs an agent from the ACP registry and adds it as an ACP agent.
    InstallAcpAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Its id in the registry.
        agent: String,
        /// The registry version.
        version: String,
        /// Where.
        place: ToolPlace,
        /// Installs a binary the registry gives no checksum for.
        confirm_unchecked: bool,
    },
    /// Accepts a licence a tool's install waits on.
    AcceptLicence {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The licence, by [`crate::hosts::PendingLicence::id`].
        licence: String,
        /// The SHA-256 of the text you read, so a changed text isn't
        /// accepted unseen.
        text_sha256: String,
    },

    // Workflows.
    /// Starts sending a workflow run, as a [`ServerMessage::Workflow`]
    /// reset and then patches.
    OpenWorkflow {
        /// The run.
        run: RunKey,
    },
    /// Stops sending a workflow run.
    CloseWorkflow {
        /// The run.
        run: RunKey,
    },
    /// Asks for more about one node of a run. The outcome is
    /// [`ActionOutcome::NodeDetail`].
    NodeDetail {
        /// Matches the result.
        key: IdempotencyKey,
        /// The run.
        run: RunKey,
        /// The node, by its id in the run's shape.
        node: String,
    },

    // Skills.
    /// Adds a source to the library this device holds.
    SkillSourceAdd {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// What the app calls it.
        name: String,
        /// Where its skills come from.
        kind: SourceKind,
        /// The branch or tag to fetch, none for a git repo's default.
        default_ref: Option<String>,
        /// Whose it is.
        trust: SourceTrust,
    },
    /// Fetches a source again and reads its skills.
    SkillSourceFetch {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The source.
        source: SkillSourceId,
    },
    /// Removes a source from the library. Projects keep the trees they
    /// locked.
    SkillSourceRemove {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The source.
        source: SkillSourceId,
    },
    /// Sets a project's lock on this device.
    SkillsSetProject {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The project.
        project: ProjectId,
        /// Its new lock.
        lock: SkillLock,
    },
    /// Puts a project's locked skills into each of its checkouts.
    SkillsApply {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The project.
        project: ProjectId,
    },
    /// Lists the skills each CLI sees in a place. The outcome is
    /// [`ActionOutcome::Skills`].
    SkillsScan {
        /// Matches the result.
        key: IdempotencyKey,
        /// Where to look.
        place: AgentPlace,
    },
    /// Asks which of a lock's trees this device's store lacks. The outcome
    /// is [`ActionOutcome::SkillTrees`].
    SkillsTreesNeeded {
        /// Matches the result.
        key: IdempotencyKey,
        /// The lock.
        lock: SkillLock,
    },
    /// Sets the always skills this device takes from the library's
    /// holder, each in its store already. A name outside the always set
    /// is left out.
    SkillsSetAlways {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The skills.
        skills: Vec<LockedSkill>,
    },
    /// Approves or rejects a skill proposal.
    SkillProposalAction {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The proposal.
        proposal: SkillProposalId,
        /// What to do with it.
        action: ProposalAction,
        /// Pushes the source after an approve.
        push: bool,
        /// The name an approve of a third party's skill takes in the
        /// personal source, as a fork. An app from before forks sends
        /// none.
        #[serde(default)]
        fork: Option<String>,
        /// The proposal as the device it was found on has it, when that's
        /// another device and this one keeps the library. Its tree must be
        /// in this device's store already. Only an approve takes one.
        #[serde(default)]
        elsewhere: Option<Box<SkillProposal>>,
        /// What the diff the owner read showed. An approve is refused when
        /// it no longer holds. An app from before it sends none.
        #[serde(default)]
        seen: Option<ProposalSeen>,
    },
    /// What a proposal on this device changes against its base tree. The
    /// outcome is [`ActionOutcome::SkillProposalDiff`]. It also keeps the
    /// proposal's tree in this device's store, so it can be copied to the
    /// device that keeps the library.
    SkillProposalDiff {
        /// Matches the result.
        key: IdempotencyKey,
        /// The proposal.
        proposal: SkillProposalId,
        /// The proposal as the device it was found on has it, when that's
        /// another device and this one keeps the library. Its tree must be
        /// in this device's store already. An app from before it sends
        /// none.
        #[serde(default)]
        elsewhere: Option<Box<SkillProposal>>,
    },
    /// Works out what tidying this device's global skills would do. The
    /// outcome is [`ActionOutcome::SkillsGlobalsPlan`].
    SkillsGlobalsPlan {
        /// Matches the result.
        key: IdempotencyKey,
    },
    /// Tidies this device's global skills as a plan said.
    SkillsGlobalsApply {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The plan's hash, which must still match.
        plan_hash: String,
        /// The copy you picked of each skill whose copies differ, by path.
        /// It goes into the library and the others to the backup. A skill
        /// with none picked stays as it is.
        #[serde(default)]
        picks: Vec<String>,
    },
    /// Puts back what the last tidy moved.
    SkillsGlobalsUndo {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
    },

    // The phone.
    /// Names the phone whose key opened this connection. Only `maxd proxy
    /// --phone` sends it, as the connection's first frame. The connection
    /// then has the app's rights, and is the one this device pushes for.
    Phone {
        /// The phone.
        phone: PhoneId,
    },
    /// Names the pairing whose one-time key opened this connection. Only
    /// `maxd proxy --pair` sends it, as the connection's first frame, and
    /// the connection may then send only [`ClientMessage::PairPhone`].
    Pair {
        /// The pairing.
        pair: PairId,
    },
    /// Starts pairing a phone through this device. `maxd` makes a one-time
    /// key that runs only `maxd proxy --pair`, good for one use within 10
    /// minutes. The outcome is [`ActionOutcome::PhonePair`].
    BeginPhonePair {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The devices the phone may reach, with their routes resolved.
        book: Vec<PhoneBookEntry>,
        /// The Mac that pairs it, which the phone names the book by. An
        /// older Mac leaves it out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<PhoneMac>,
    },
    /// Pairs the phone on a connection a one-time key opened. `maxd` swaps
    /// that key for the phone's own and keeps the book for it. The outcome
    /// is [`ActionOutcome::PhonePaired`].
    PairPhone {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Your name for the phone.
        name: String,
        /// The phone's own SSH key, as `ssh-ed25519 <base64>`.
        ssh_public: String,
        /// The phone's X25519 push key, as base64 of its 32 bytes.
        push_public: String,
    },
    /// Lets a phone paired through another device in here too, to run only
    /// `maxd proxy --phone` for that phone. The outcome is `Done`, or
    /// [`ActionOutcome::PhoneAdded`] when the device's network access is on.
    AddPhone {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The phone.
        phone: PhoneId,
        /// Your name for it.
        name: String,
        /// Its SSH key, as `ssh-ed25519 <base64>`.
        ssh_public: String,
        /// Its X25519 push key, as base64 of its 32 bytes.
        push_public: String,
    },
    /// Takes a phone's key out, with its push token and its book, and
    /// closes its connections.
    RemovePhone {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The phone.
        phone: PhoneId,
    },
    /// Ends one of the phone sessions this device holds, so its token
    /// stops working and its link closes.
    RevokePhoneSession {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The session.
        session: PhoneSessionId,
    },
    /// Replaces the devices a phone paired through this one may reach.
    SetPhoneBook {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The phone.
        phone: PhoneId,
        /// The devices.
        book: Vec<PhoneBookEntry>,
        /// The Mac that sends it. An older Mac leaves it out, and the
        /// device keeps the one it has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<PhoneMac>,
    },
    /// Lets a phone do what only the Mac does here, short of letting other
    /// keys in, or stops it. Only the Mac sends it.
    SetPhoneEverything {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The phone.
        phone: PhoneId,
        /// Whether the phone may.
        everything: bool,
    },
    /// Says where and what to push to the phone this connection is. Only
    /// a connection `maxd proxy --phone` opened sends it.
    SetPushToken {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The token and the kinds.
        push: PushToken,
    },
    /// Says what the phone this connection is shows. Sent every 25
    /// seconds while the app is open, and good for 45. Nothing is pushed
    /// for the agent on screen.
    Presence {
        /// The agent on screen, if any.
        agent: Option<AgentId>,
    },
    /// Gives the device the APNs key it pushes with. The device keeps the
    /// key and never sends it back.
    SetApnsKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// Apple's ID for the key.
        key_id: String,
        /// The Apple developer team the key belongs to.
        team_id: String,
        /// The `.p8` file's contents, as PEM.
        private_key: Secret,
    },
    /// Gives the device the Firebase service account it pushes to Android
    /// phones with. The device keeps the key and never sends it back.
    /// Since protocol 61.
    SetFcmKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The Firebase project the account belongs to, which names the
        /// FCM endpoint.
        project_id: String,
        /// The service account's email, which signs in to Google.
        client_email: String,
        /// The account's RSA key, as the PKCS #8 PEM in the JSON file's
        /// `private_key`.
        private_key: Secret,
    },
    /// Takes the library over from the device that held it, with its
    /// sources under the same IDs. This device clones each git source
    /// itself and rebuilds the personal source from its skills' trees,
    /// which must be in its store already. A folder source stays in the
    /// list with a problem, since its folder is on the other device. It's
    /// published once this device's settings say it holds the library.
    SkillsTakeLibrary {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// What the app calls the device that held it.
        from: String,
        /// Its sources, as it published them.
        sources: Vec<SkillSource>,
    },
    /// Pushes a source's commits to its remote.
    SkillSourcePush {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The source.
        source: SkillSourceId,
    },

    // The dispatcher.
    /// Sends the dispatcher a request. It starts a conversation when there
    /// is none, when `new` is set, or when `account` isn't the one the
    /// conversation runs under. Answered as the start or the send is.
    AskDispatcher {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The request.
        text: String,
        /// The Claude or Codex account on this device it runs under.
        account: AccountId,
        /// Starts a new conversation.
        new: bool,
        /// The pills in the request, as in [`UserInput::refs`]. The plan
        /// passes on each one the dispatcher copies into an agent's first
        /// message. None before [`ANSWER_PILLS_SINCE`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        refs: Vec<InlineRef>,
    },
    /// Runs the dispatcher's plan, item by item, and stops at the first
    /// that fails. `maxd` refuses it from the dispatcher's own session and
    /// from any agent. Answered with [`ActionOutcome::PlanRan`].
    ConfirmPlan {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The plan, which must still be the one shown.
        plan: u32,
    },
    /// Drops the dispatcher's plan without running it.
    DismissPlan {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The plan.
        plan: u32,
    },
    /// Puts back a change the dispatcher made without asking, once. `maxd`
    /// refuses it after a restart, or when something else changed the same
    /// thing since.
    UndoDispatcherAction {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The key on the change's card.
        action: IdempotencyKey,
    },
    /// Acts on one dispatcher chat, since [`DISPATCHER_CHATS_SINCE`].
    /// Answered with Done, or Refused for a chat the device doesn't hold.
    ActOnChat {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The chat's agent.
        agent: AgentId,
        /// What to do.
        action: ChatAct,
    },

    // Proving who connects.
    /// Proves this connection holds `public_key`, with a signature over the
    /// hello's [`ServerHello::challenge`] and what names the connection, as
    /// the `prove` module in `crates/core` lays them out. Sent right after
    /// the hello, before the sync. Answered with Done, or Refused for a key
    /// the device doesn't trust or a bad signature.
    Prove {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The Ed25519 public key, as base64url of its 32 bytes, unpadded.
        public_key: String,
        /// The signature, as base64url of its 64 bytes, unpadded.
        signature: String,
    },
    /// Asks the device to trust the key this connection just proved.
    /// Answered with Done, Refused, or [`ActionOutcome::Asked`] while a card
    /// waits for a yes on another device.
    EnrollAppKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// What to call the key, such as "MaxHQ on studio-mac".
        label: String,
    },
    /// Stops trusting an app key. Only a proven app may send it.
    RemoveAppKey {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The key, as [`crate::admission::AppKey::public_key`] gives it.
        public_key: String,
    },
    /// Answers an admission card. Only a proven desktop app may send it.
    AnswerAdmission {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The card.
        admission: AdmissionId,
        /// Whether to let the program in until it disconnects.
        allow: bool,
    },
    /// Moves an agent to another device: checks the device it goes to,
    /// stops the agent, starts a new one there from a summary of its
    /// conversation, marks the old one moved and points the tasks that
    /// target it at the new one. Only the device that runs the MaxHQ app
    /// takes it. The outcome is [`ActionOutcome::Started`] with the new
    /// agent. A `maxd` from before [`MOVE_AGENT_SINCE`] refuses it.
    MoveAgent {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent.
        agent: AgentId,
        /// The device it runs on.
        from: DeviceId,
        /// Where it goes.
        to: MoveTo,
        /// Whether to stop a turn that runs or a prompt that waits. Without
        /// it, a move of an agent that works is refused.
        #[serde(default)]
        stop: bool,
    },

    // Terminals, since [`TERMINALS_SINCE`]. Only the app and phones may
    // send these, never a link from another device or an agent's tools.
    /// Opens a terminal in an agent's folder, on the agent's device and in
    /// its container when it runs in one. The outcome is
    /// [`ActionOutcome::TerminalOpened`].
    TerminalOpen {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent whose folder it starts in.
        agent: AgentId,
        /// A shell for the drawer, or one command you ran with `!`.
        kind: TerminalKind,
        /// Its width in cells.
        cols: u16,
        /// Its height in cells.
        rows: u16,
    },
    /// Starts sending this connection a terminal's output frames. `maxd`
    /// answers first with [`ServerMessage::TerminalAttached`], then sends
    /// the output from there on. Sending it again for a terminal already
    /// attached starts over from `after`.
    TerminalAttach {
        /// The terminal.
        terminal: TerminalId,
        /// The offset of the last output byte the client has, or none for
        /// everything `maxd` still holds.
        #[specta(type = Option<specta_typescript::Number>)]
        after: Option<u64>,
    },
    /// Stops sending this connection a terminal's output frames.
    TerminalDetach {
        /// The terminal.
        terminal: TerminalId,
    },
    /// Sets a terminal's size. The client you last typed in owns it.
    TerminalResize {
        /// The terminal.
        terminal: TerminalId,
        /// Its width in cells.
        cols: u16,
        /// Its height in cells.
        rows: u16,
    },
    /// Writes Ctrl-C to a terminal. A phone may send it to a drawer
    /// terminal it may not type into.
    TerminalInterrupt {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The terminal.
        terminal: TerminalId,
        /// Kills the program in the terminal's foreground instead, for a
        /// session command that ignored Ctrl-C. The shell goes on. A
        /// `maxd` refuses it when only the shell runs. Since
        /// [`INPUT_MODES_SINCE`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Closes a terminal: hangs up its shell or command, then forgets it.
    TerminalClose {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The terminal.
        terminal: TerminalId,
        /// Whether to kill what still runs rather than hang it up.
        force: bool,
    },
    /// Renames a terminal's tab.
    TerminalRename {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The terminal.
        terminal: TerminalId,
        /// The new name. Empty goes back to the name the shell sets.
        title: String,
    },
    /// Moves a command you ran with `!` to the agent's terminal drawer,
    /// where it goes on as a tab. The terminal and its card in the timeline
    /// both say it moved.
    TerminalMove {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The terminal.
        terminal: TerminalId,
    },

    // Folders, since [`FOLDERS_SINCE`]. Only the app and phones may send
    // these, never a link from another device or an agent's tools.
    /// Lists one folder on a device, for a path field's suggestions and the
    /// folder browser. The outcome is [`ActionOutcome::FolderListed`], for
    /// a folder that isn't there too.
    ListFolder {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device the folder is on.
        device: DeviceId,
        /// The folder's absolute path. A leading `~` is the device's home
        /// folder.
        path: String,
        /// Whether to list files too, not only folders.
        files: bool,
        /// Whether to list the names that start with a dot.
        hidden: bool,
        /// Lists only the names that hold this, ignoring case, those that
        /// start with it first. The device filters before it cuts a big
        /// folder's listing short, so a field finds any name you type. Left
        /// out, every name lists.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<String>,
    },
    /// Makes one folder on a device, for the folder browser's New folder.
    /// Its parent must be there already. A resend under the same key gets
    /// the first answer, so a folder made before the connection dropped
    /// isn't reported as there already.
    MakeFolder {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device the folder goes on.
        device: DeviceId,
        /// The new folder's absolute path.
        path: String,
    },

    // Sessions, since [`SESSIONS_SINCE`]. Only the app and phones may send
    // these, never a link from another device or an agent's tools.
    /// Runs a command you typed after `!` in the agent's session, the one
    /// shell its commands share, and opens the session first when there's
    /// none. A command that can't go there runs in a fresh shell in the
    /// session's folder, with a `UserRun` card that says why. The outcome
    /// is [`ActionOutcome::SessionRan`] for a block in the session's card,
    /// or [`ActionOutcome::TerminalOpened`] for a fresh shell.
    SessionRun {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent whose session it runs in.
        agent: AgentId,
        /// The command line, as typed after the `!`.
        command: String,
        /// Whether you ran it with `!!`, so its block starts private.
        private: bool,
        /// Whether to run it in a fresh shell rather than the session.
        fresh: bool,
        /// The width in cells of a terminal it opens.
        cols: u16,
        /// The height in cells of a terminal it opens.
        rows: u16,
    },
    /// Keeps one block of a session card from the agent, or shares it
    /// again. Refused once the card is sealed.
    SessionShare {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent whose card it is.
        agent: AgentId,
        /// The card.
        item: ItemId,
        /// The block's ID in the card.
        block: u32,
        /// Whether the block is private.
        private: bool,
    },
    /// Moves the agent's session to a folder, for the folder chip. With no
    /// session yet, it opens one there, and the outcome is
    /// [`ActionOutcome::TerminalOpened`]. With an idle session, it runs
    /// `cd` there as a shared block, and the outcome is
    /// [`ActionOutcome::SessionRan`]. A busy session refuses.
    SessionCd {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent whose session it is.
        agent: AgentId,
        /// The folder, as the device writes it. A leading `~` is the
        /// device's home folder.
        path: String,
    },
    /// Ends the agent's session: hangs up its shell, then forgets it. Its
    /// cards stay in the timeline.
    SessionEnd {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The agent whose session it is.
        agent: AgentId,
        /// Whether to kill a command that still runs. Without it, a session
        /// with a command running refuses, so you're asked first.
        force: bool,
    },

    /// Lists, marks read or clears a device's notification log, since
    /// [`NOTICES_SINCE`]. Only the app and phones may send it.
    Notices {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device whose log it is.
        device: DeviceId,
        /// What to do with it.
        call: NoticeCall,
    },

    /// Lists the `Shell(...)` entries Cursor saved in its own allow list
    /// on a device, or removes one. The outcome is
    /// [`ActionOutcome::CursorRules`]. Only the app and phones may send it.
    /// Since protocol 57, [`CURSOR_RULES_SINCE`].
    CursorRules {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The device whose Cursor settings they are.
        device: DeviceId,
        /// What to do.
        call: CursorRuleCall,
    },

    /// Spends one of a Codex account's banked rate-limit resets through
    /// `codex app-server`, which resets the account's limits now and can't
    /// be undone. The outcome is [`ActionOutcome::ResetCredit`]. Only the
    /// app may send it, after you confirm, and only to the device the
    /// account is on: phones, peer links and agents' tools are refused.
    /// Since protocol 58, [`RESET_CREDITS_SINCE`].
    UseResetCredit {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// The Codex account to reset.
        account: AccountId,
    },

    // Local models.
    /// Acts on the device's local models: probes, adds, tests or removes a
    /// server URL, downloads, pauses, resumes or deletes a model file, or
    /// loads or unloads a model. Probing a URL answers
    /// [`ActionOutcome::ServerProbed`], adding a server
    /// [`ActionOutcome::ModelServerAdded`], the rest
    /// [`ActionOutcome::Done`]. The default model is a device setting,
    /// [`crate::model::DeviceSettings::local_models`]. Since
    /// [`LOCAL_MODELS_SINCE`].
    ModelAction {
        /// Makes a resend after a dropped connection safe.
        key: IdempotencyKey,
        /// What to do.
        action: ModelAction,
    },
}

impl ClientMessage {
    /// Reads a message another device's `maxd` sent over a peer link. Its
    /// requests and answers are read a piece at a time, and any other
    /// message past 1 MiB is refused unread.
    pub fn read_peer(json: &[u8]) -> serde_json::Result<Self> {
        crate::raw::peer(json)
    }

    /// The first protocol version with this message. A new message returns
    /// the version that adds it, and a client won't send it to an older
    /// `maxd`.
    #[must_use]
    pub fn since(&self) -> u32 {
        match self {
            Self::RemoveCommandRule { .. } => 23,
            Self::RemoveAllowedEdit { .. } => ALLOWED_EDITS_SINCE,
            Self::ListProcesses { .. } | Self::SignalProcess { .. } => 24,
            Self::UpdateDeviceSettings { changes, .. } => changes
                .iter()
                .map(DeviceSettingChange::since)
                .max()
                .unwrap_or(26),
            Self::SetTaskEnabled { .. } => 26,
            Self::ReadNote { .. }
            | Self::SaveNote { .. }
            | Self::NoteHistory { .. }
            | Self::NoteHashes { .. }
            | Self::NoteVersions { .. }
            | Self::PutNoteVersions { .. } => 27,
            Self::SetPhoneEverything { .. } => 33,
            Self::TakeOver { .. } => TAKE_OVER_SINCE,
            Self::Prove { .. }
            | Self::EnrollAppKey { .. }
            | Self::RemoveAppKey { .. }
            | Self::AnswerAdmission { .. } => APP_KEYS_SINCE,
            Self::BeginCliLogin { .. }
            | Self::FinishCliLogin { .. }
            | Self::CheckCliLogin { .. }
            | Self::CancelCliLogin { .. } => CLI_LOGIN_SINCE,
            Self::FetchBegin { anyway: true, .. } => SHOW_ANYWAY_SINCE,
            Self::SetInbox {
                change: InboxChange::ContinueAtLimit { .. },
                ..
            } => CONTINUE_AT_LIMIT_SINCE,
            Self::SetEffort { .. } => EFFORT_SINCE,
            Self::Answer {
                answer: AttentionAnswer::Dismiss,
                ..
            } => DISMISS_SINCE,
            Self::Answer { refs, .. } | Self::AskDispatcher { refs, .. } if !refs.is_empty() => {
                ANSWER_PILLS_SINCE.max(refs_since(refs))
            }
            Self::SendInput { input, .. } => input.since(),
            Self::StartAgent { spec, .. } => spec
                .first
                .as_ref()
                .map_or(OLDEST_PROTOCOL_VERSION, UserInput::since),
            Self::MoveAgent { .. } => MOVE_AGENT_SINCE,
            Self::UndoDispatcherAction { .. } => DISPATCHER_ADMIN_SINCE,
            Self::TerminalOpen {
                kind: TerminalKind::Session,
                ..
            }
            | Self::SessionRun { .. }
            | Self::SessionShare { .. }
            | Self::SessionCd { .. }
            | Self::SessionEnd { .. } => SESSIONS_SINCE,
            Self::TerminalInterrupt { force: true, .. } => INPUT_MODES_SINCE,
            Self::TerminalOpen { .. }
            | Self::TerminalAttach { .. }
            | Self::TerminalDetach { .. }
            | Self::TerminalResize { .. }
            | Self::TerminalInterrupt { .. }
            | Self::TerminalClose { .. }
            | Self::TerminalRename { .. }
            | Self::TerminalMove { .. } => TERMINALS_SINCE,
            Self::ListFolder { .. } | Self::MakeFolder { .. } => FOLDERS_SINCE,
            Self::Notices { .. } => NOTICES_SINCE,
            Self::CursorRules { .. } => CURSOR_RULES_SINCE,
            Self::UseResetCredit { .. } => RESET_CREDITS_SINCE,
            Self::ActOnChat { .. } => DISPATCHER_CHATS_SINCE,
            Self::ModelAction { .. } => LOCAL_MODELS_SINCE,
            Self::SetFcmKey { .. } => FCM_SINCE,
            Self::SetPushToken { push, .. } if push.service == PushService::Fcm => FCM_SINCE,
            _ => OLDEST_PROTOCOL_VERSION,
        }
    }

    /// Whether a peer at `version` reads this message. A peer whose version
    /// isn't known yet gets it.
    #[must_use]
    pub fn reaches(&self, version: Option<u32>) -> bool {
        reaches(self.since(), version)
    }

    /// The key an action carries, or none for a message that isn't one. A
    /// client waits on the key for the action's result, and `maxd` answers
    /// a refused action under it.
    #[must_use]
    pub fn action_key(&self) -> Option<IdempotencyKey> {
        match self {
            Self::StartAgent { key, .. }
            | Self::SendInput { key, .. }
            | Self::CancelQueued { key, .. }
            | Self::Interrupt { key, .. }
            | Self::SetPermission { key, .. }
            | Self::SetModel { key, .. }
            | Self::SetEffort { key, .. }
            | Self::StopAgent { key, .. }
            | Self::DeleteAgent { key, .. }
            | Self::RenameAgent { key, .. }
            | Self::SetInbox { key, .. }
            | Self::SetProjectHidden { key, .. }
            | Self::RemoveCommandRule { key, .. }
            | Self::RemoveAllowedEdit { key, .. }
            | Self::ContinueHere { key, .. }
            | Self::TakeOver { key, .. }
            | Self::StageAttachment { key, .. }
            | Self::SetDeviceSettings { key, .. }
            | Self::UpdateDeviceSettings { key, .. }
            | Self::CreateWorkspace { key, .. }
            | Self::StartWorkspace { key, .. }
            | Self::StopWorkspace { key, .. }
            | Self::DeleteWorkspace { key, .. }
            | Self::SetProjectSettings { key, .. }
            | Self::WindDown { key, .. }
            | Self::StoreClaudeToken { key, .. }
            | Self::StoreCursorKey { key, .. }
            | Self::BeginChatGptSignIn { key }
            | Self::FinishChatGptSignIn { key, .. }
            | Self::SetGithubApp { key, .. }
            | Self::CheckGithubApp { key }
            | Self::ForgetCredential { key, .. }
            | Self::UploadBegin { key, .. }
            | Self::FetchBegin { key, .. }
            | Self::OpenForward { key, .. }
            | Self::SetGuestSshKey { key, .. }
            | Self::FindPaths { key, .. }
            | Self::FindGitItems { key, .. }
            | Self::ListProcesses { key, .. }
            | Self::SignalProcess { key, .. }
            | Self::ReadNote { key, .. }
            | Self::SaveNote { key, .. }
            | Self::NoteHistory { key, .. }
            | Self::NoteHashes { key, .. }
            | Self::NoteVersions { key, .. }
            | Self::PutNoteVersions { key, .. }
            | Self::ExcerptAgent { key, .. }
            | Self::FindInTimeline { key, .. }
            | Self::ListIgnoredFiles { key, .. }
            | Self::FetchFiles { key, .. }
            | Self::FetchSkillTree { key, .. }
            | Self::PlaceStaged { key, .. }
            | Self::CloneFolder { key, .. }
            | Self::CloneCancel { key, .. }
            | Self::CloneRetry { key, .. }
            | Self::CloneRemove { key, .. }
            | Self::InstallUpdate { key, .. }
            | Self::UpdateTools { key, .. }
            | Self::ReadLoad { key }
            | Self::ReadDiagnostics { key, .. }
            | Self::AuthKey { key, .. }
            | Self::CallTool { key, .. }
            | Self::GrantPermission { key, .. }
            | Self::RevokePermission { key, .. }
            | Self::ListPermissions { key }
            | Self::MintKey { key, .. }
            | Self::ListKeys { key }
            | Self::SaveTask { key, .. }
            | Self::SetTaskEnabled { key, .. }
            | Self::DeleteTask { key, .. }
            | Self::RunTaskNow { key, .. }
            | Self::SetHookOn { key, .. }
            | Self::RotateHookToken { key, .. }
            | Self::SetHookSecret { key, .. }
            | Self::SetHookAddress { key, .. }
            | Self::ListRuns { key, .. }
            | Self::ListDeliveries { key, .. }
            | Self::PeerRequest { key, .. }
            | Self::PeerKey { key }
            | Self::AuthorizePeer { key, .. }
            | Self::RevokePeer { key, .. }
            | Self::SetPeerBook { key, .. }
            | Self::SetAcpAgent { key, .. }
            | Self::DeleteAcpAgent { key, .. }
            | Self::BeginGithubSignIn { key, .. }
            | Self::CancelGithubSignIn { key, .. }
            | Self::CheckGithubSignIn { key, .. }
            | Self::BeginCliLogin { key, .. }
            | Self::FinishCliLogin { key, .. }
            | Self::CheckCliLogin { key, .. }
            | Self::CancelCliLogin { key, .. }
            | Self::ListPulls { key, .. }
            | Self::ListBranches { key, .. }
            | Self::GetPull { key, .. }
            | Self::PullFiles { key, .. }
            | Self::PullComments { key, .. }
            | Self::PullChecks { key, .. }
            | Self::PullAction { key, .. }
            | Self::PullsDue { key, .. }
            | Self::LinkPull { key, .. }
            | Self::UnlinkPull { key, .. }
            | Self::CheckTestHosts { key }
            | Self::BuildTestHost { key, .. }
            | Self::ResetTestHost { key, .. }
            | Self::EndLease { key, .. }
            | Self::ReadLeaseScreen { key, .. }
            | Self::InstallTool { key, .. }
            | Self::RemoveTool { key, .. }
            | Self::CheckHost { key }
            | Self::ApplyHost { key, .. }
            | Self::ListSnapshots { key, .. }
            | Self::RestoreSnapshot { key, .. }
            | Self::PlanMove { key, .. }
            | Self::RunMoveStep { key, .. }
            | Self::MoveBack { key, .. }
            | Self::RetireMove { key, .. }
            | Self::InstallAcpAgent { key, .. }
            | Self::AcceptLicence { key, .. }
            | Self::NodeDetail { key, .. }
            | Self::SkillSourceAdd { key, .. }
            | Self::SkillSourceFetch { key, .. }
            | Self::SkillSourceRemove { key, .. }
            | Self::SkillsSetProject { key, .. }
            | Self::SkillsApply { key, .. }
            | Self::SkillsScan { key, .. }
            | Self::SkillsTreesNeeded { key, .. }
            | Self::SkillsSetAlways { key, .. }
            | Self::SkillProposalAction { key, .. }
            | Self::SkillProposalDiff { key, .. }
            | Self::SkillsGlobalsPlan { key }
            | Self::SkillsGlobalsApply { key, .. }
            | Self::SkillsGlobalsUndo { key }
            | Self::BeginPhonePair { key, .. }
            | Self::PairPhone { key, .. }
            | Self::AddPhone { key, .. }
            | Self::RemovePhone { key, .. }
            | Self::RevokePhoneSession { key, .. }
            | Self::SetPhoneBook { key, .. }
            | Self::SetPhoneEverything { key, .. }
            | Self::SetPushToken { key, .. }
            | Self::SetApnsKey { key, .. }
            | Self::SetFcmKey { key, .. }
            | Self::SkillsTakeLibrary { key, .. }
            | Self::SkillSourcePush { key, .. }
            | Self::AskDispatcher { key, .. }
            | Self::ConfirmPlan { key, .. }
            | Self::DismissPlan { key, .. }
            | Self::UndoDispatcherAction { key, .. }
            | Self::ActOnChat { key, .. }
            | Self::Prove { key, .. }
            | Self::EnrollAppKey { key, .. }
            | Self::RemoveAppKey { key, .. }
            | Self::AnswerAdmission { key, .. }
            | Self::MoveAgent { key, .. }
            | Self::TerminalOpen { key, .. }
            | Self::TerminalInterrupt { key, .. }
            | Self::TerminalClose { key, .. }
            | Self::TerminalRename { key, .. }
            | Self::TerminalMove { key, .. }
            | Self::ListFolder { key, .. }
            | Self::MakeFolder { key, .. }
            | Self::SessionRun { key, .. }
            | Self::SessionShare { key, .. }
            | Self::SessionCd { key, .. }
            | Self::SessionEnd { key, .. }
            | Self::Notices { key, .. }
            | Self::CursorRules { key, .. }
            | Self::UseResetCredit { key, .. }
            | Self::ModelAction { key, .. } => Some(*key),
            Self::Hello(_)
            | Self::Phone { .. }
            | Self::Pair { .. }
            | Self::Presence { .. }
            | Self::Sync { .. }
            | Self::Answer { .. }
            | Self::MarkSeen { .. }
            | Self::RefreshUsage { .. }
            | Self::Ping { .. }
            | Self::OpenTimeline { .. }
            | Self::LoadOlder { .. }
            | Self::CloseTimeline { .. }
            | Self::OpenWorkflow { .. }
            | Self::CloseWorkflow { .. }
            | Self::UploadCancel { .. }
            | Self::TransferCredit { .. }
            | Self::CloseForward { .. }
            | Self::TunnelOpen { .. }
            | Self::TunnelCredit { .. }
            | Self::TunnelClose { .. }
            | Self::GuestSshOpen { .. }
            | Self::Peer { .. }
            | Self::PeerAnswer { .. }
            | Self::TerminalAttach { .. }
            | Self::TerminalDetach { .. }
            | Self::TerminalResize { .. } => None,
        }
    }
}

/// Whether a peer at `version` reads a message from `since` on.
fn reaches(since: u32, version: Option<u32>) -> bool {
    version.is_none_or(|version| version >= since)
}

/// Which binary [`ClientMessage::InstallUpdate`] installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    /// `maxd` itself.
    Maxd,
    /// The binary each container workspace runs as `maxd guest`.
    Guest,
}

/// A release's fleet manifest, `maxd-release.json`, and its signature,
/// `maxd-release.json.sig`, as the release put them beside its binaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SignedRelease {
    /// The manifest's text: the release's version and the sha256 of each
    /// `maxd-<target>` it carries.
    pub manifest: String,
    /// The signature's text: base64 of a minisign signature over the
    /// manifest's bytes, as `tauri signer sign` writes it.
    pub signature: String,
}

/// One CLI in one place, for [`ClientMessage::UpdateTools`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
pub struct ToolTarget {
    /// Which CLI.
    pub tool: Tool,
    /// Where it's installed.
    pub place: ToolPlace,
}

/// A device's free CPU and memory, for choosing where a new agent goes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct HostLoad {
    /// When the device read it.
    pub sampled_at: Timestamp,
    /// How busy the CPUs were, from 0 to 1, or none when it couldn't be
    /// read.
    pub cpu_util: Option<f32>,
    /// How many CPUs the device has.
    pub cpu_count: u32,
    /// Bytes of memory free for new work.
    #[specta(type = specta_typescript::Number)]
    pub mem_available: u64,
    /// Bytes of memory in all.
    #[specta(type = specta_typescript::Number)]
    pub mem_total: u64,
}

// The numbers are never NaN, so equality is total, and the CPU figures
// below go to TypeScript as plain numbers.
impl Eq for HostLoad {}

/// What `maxd` and everything it started used over one stretch of time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct UseBucket {
    /// When the stretch starts.
    pub started_at: Timestamp,
    /// When it ends.
    pub ended_at: Timestamp,
    /// The average CPU over the samples in it, in percent of one CPU.
    #[specta(type = specta_typescript::Number)]
    pub avg_cpu: f32,
    /// The highest CPU in one sample.
    #[specta(type = specta_typescript::Number)]
    pub max_cpu: f32,
    /// The most memory in one sample, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub max_rss: u64,
    /// The most processes in one sample.
    pub max_processes: u32,
}

impl Eq for UseBucket {}

/// One process `maxd` started, or `maxd` itself, over the window read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProcessUse {
    /// The agent whose holder started it, if any.
    pub agent: Option<AgentId>,
    /// The workspace that agent runs in, if any.
    pub workspace: Option<WorkspaceId>,
    /// Its process ID.
    pub pid: u32,
    /// Its command line, cut to 200 characters.
    pub command: String,
    /// Its CPU in the latest sample, in percent of one CPU.
    #[specta(type = specta_typescript::Number)]
    pub cpu_now: f32,
    /// Its average CPU.
    #[specta(type = specta_typescript::Number)]
    pub cpu_avg: f32,
    /// Its highest CPU.
    #[specta(type = specta_typescript::Number)]
    pub cpu_max: f32,
    /// Its memory in the latest sample, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub rss_now: u64,
    /// Its most memory, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub rss_max: u64,
}

impl Eq for ProcessUse {}

/// What one container workspace has used in all, as Incus counts it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ContainerUse {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// CPU time since it started, in nanoseconds.
    #[specta(type = specta_typescript::Number)]
    pub cpu_ns: u64,
    /// Memory in use now, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub memory: u64,
}

/// Where uploaded files go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UploadTarget {
    /// The device's staging folder, for a new agent or one in a folder.
    Device,
    /// A workspace's container, for an agent not started yet.
    Workspace {
        /// The workspace.
        workspace: WorkspaceId,
    },
    /// A running agent's attachments folder.
    Agent {
        /// The agent.
        agent: AgentId,
    },
    /// The device's skill store, as one skill folder. `maxd` checks its
    /// hash before it lands.
    SkillStore {
        /// The folder's hash.
        tree: TreeHash,
    },
}

/// Where uploaded files come from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UploadSource {
    /// Bytes the client sends in transfer frames, one entry per file.
    Stream {
        /// The name the upload shows as, such as the folder's.
        name: String,
        /// Every file, folder and link, folders before what's in them.
        entries: Vec<ManifestEntry>,
    },
    /// A path on the device itself, as [`AttachmentSource::Path`] is.
    DevicePath {
        /// Absolute path on the device.
        path: String,
    },
}

/// One file, folder or link in an upload or download.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ManifestEntry {
    /// Relative, `/` separated, NFC, with no `.` or `..` parts.
    pub path: String,
    /// What the entry is.
    pub kind: EntryKind,
    /// Lands as 0755 when set and 0644 otherwise.
    pub executable: bool,
    /// Bytes in the file, and 0 for anything else.
    #[specta(type = specta_typescript::Number)]
    pub size: u64,
    /// Hex SHA-256, for files only.
    pub sha256: Option<String>,
}

/// What a [`ManifestEntry`] is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryKind {
    /// A regular file, whose bytes follow in transfer frames.
    File,
    /// A folder.
    Dir,
    /// A symbolic link, which is recreated and never followed.
    Symlink {
        /// What the link points at, as it was written.
        target: String,
    },
}

/// How a tunnel stream ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunnelEnd {
    /// The sender is done writing, and still reads.
    Write,
    /// The connection is aborted both ways.
    Reset,
}

/// What a new agent should be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AgentSpec {
    /// Which CLI to run.
    pub vendor: Vendor,
    /// The login to run it under.
    pub account: AccountId,
    /// Where to run it.
    pub place: AgentPlace,
    /// The model to run, or none for the account's default.
    pub model: Option<String>,
    /// Unset means the device's default.
    pub permission: Option<PermissionMode>,
    /// A name to show, or none to take one from the CLI or the first message.
    pub title: Option<String>,
    /// The first message, sent once the agent is up.
    pub first: Option<UserInput>,
    /// The git branch for the agent's worktree in a container. `maxd`
    /// names one when it's unset.
    pub branch: Option<String>,
    /// Who started it through the agent tools, set by `maxd`. A client
    /// leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Box<AgentOrigin>>,
    /// The effort level it starts at, or none for the device's default for
    /// its vendor. Only Claude Code and Codex take one. A `maxd` from
    /// before [`EFFORT_SINCE`] ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Where a new agent runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentPlace {
    /// In a workspace `maxd` already knows.
    Workspace {
        /// The workspace to run in.
        workspace: WorkspaceId,
    },
    /// In any folder on the device.
    Folder {
        /// Absolute path on the device.
        path: String,
    },
}

/// A message you send to an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct UserInput {
    /// What you typed, with U+FFFC where each placed ref sits.
    pub text: String,
    /// Attachments staged with [`ClientMessage::StageAttachment`], in order.
    pub attachments: Vec<AttachmentId>,
    /// The context in the text, in text order. An attachment without a ref
    /// is listed after the text. A client from before sends none.
    #[serde(default)]
    pub refs: Vec<InlineRef>,
}

impl UserInput {
    /// The first protocol version that reads this message: 28 once it has
    /// a note pill and 39 once it has a visual pill, which an older `maxd`
    /// can't parse.
    #[must_use]
    pub fn since(&self) -> u32 {
        refs_since(&self.refs)
    }
}

/// The first protocol version that reads every one of these pills.
#[must_use]
pub fn refs_since(refs: &[InlineRef]) -> u32 {
    refs.iter()
        .map(|one| match one {
            InlineRef::Note { .. } => NOTE_PILLS_SINCE,
            InlineRef::Visual { .. } => VISUALS_SINCE,
            InlineRef::Output { .. } => TERMINALS_SINCE,
            InlineRef::Session { .. } => SESSIONS_SINCE,
            _ => OLDEST_PROTOCOL_VERSION,
        })
        .max()
        .unwrap_or(OLDEST_PROTOCOL_VERSION)
}

/// The protocol version whose `maxd` first reads a note pill.
pub const NOTE_PILLS_SINCE: u32 = 28;

/// The protocol version whose `maxd` first reads pills in an answer and in
/// a request to the dispatcher. A client sends an older one the pills as
/// plain words.
pub const ANSWER_PILLS_SINCE: u32 = 31;

/// The protocol version whose `maxd` first reads a file outside the agent's
/// folders when you ask, and refuses one with [`ErrorCode::Outside`].
pub const SHOW_ANYWAY_SINCE: u32 = 29;

/// The protocol version whose `maxd` first sends a challenge and takes a
/// proof, app keys and answers to admission cards.
pub const APP_KEYS_SINCE: u32 = 34;

/// The protocol version whose `maxd` first runs a CLI's own login, marks an
/// error a new sign-in fixes, and answers [`ClientMessage::BeginCliLogin`].
pub const CLI_LOGIN_SINCE: u32 = 35;

/// The protocol version whose `maxd` first runs Cursor's own login through
/// [`ClientMessage::BeginCliLogin`] and keeps a signed-out Cursor login.
pub const CURSOR_LOGIN_SINCE: u32 = 36;

/// The protocol version whose `maxd` first keeps each agent's own switch for
/// carrying on after a usage limit, [`InboxChange::ContinueAtLimit`], and a
/// default for Claude Code and one for Codex.
pub const CONTINUE_AT_LIMIT_SINCE: u32 = 38;

/// The protocol version whose `maxd` first records the pages agents show
/// with show_visual, [`crate::timeline::ItemBody::AgentVisual`], and reads a
/// visual pill, [`InlineRef::Visual`].
pub const VISUALS_SINCE: u32 = 39;

/// The protocol version whose `maxd` first keeps the switch for Claude
/// setup-tokens, [`DeviceSettingChange::ClaudeSetupToken`], and refuses a
/// token while it's off. An older one always takes them.
pub const SETUP_TOKENS_SINCE: u32 = 40;

/// The protocol version whose `maxd` first says whether a workflow run
/// failed, [`crate::workflow::WorkflowRun::failed`], and which agents its
/// nodes are bound to, [`crate::workflow::WorkflowRun::agents`]. An older
/// `maxd` sends neither, so its runs read as not failed and their node
/// agents stay in the agent list.
pub const WORKFLOW_RUN_AGENTS_SINCE: u32 = 41;

/// The protocol version whose `maxd` first keeps an effort level for each
/// Claude Code and Codex agent, takes [`ClientMessage::SetEffort`], lists
/// the levels each model offers and keeps a default for new agents.
pub const EFFORT_SINCE: u32 = 41;

/// The protocol version whose `maxd` first sends what an agent left
/// running, [`crate::model::Agent::running`]. An older one sends nothing,
/// and its agents look as they did.
pub const PROCESSES_SINCE: u32 = 41;

/// The protocol version whose `maxd` first stops a terminal's CLI to take
/// its session over, [`ClientMessage::TakeOver`], keeps a terminal session
/// that ended, and marks one waiting for its turn to end,
/// [`crate::model::Agent::taking_over`].
pub const TAKE_OVER_SINCE: u32 = 41;

/// The protocol version whose `maxd` first reads
/// [`AttentionAnswer::Dismiss`], and dismisses what waits on the agent
/// before a [`SendMode::Interrupt`] message. An older one interrupts the
/// turn, and its prompts close without an answer.
pub const DISMISS_SINCE: u32 = 42;

/// The protocol version whose `maxd` first checks the signed release
/// manifest of an update, [`SignedRelease`], before it runs or installs the
/// upload, and refuses one no release signed unless you chose to send it.
/// An older one ignores both and installs the upload as before, so the app
/// says it doesn't check signatures.
pub const SIGNED_UPDATES_SINCE: u32 = 43;

/// The protocol version whose `maxd` first moves agents between devices:
/// [`ClientMessage::MoveAgent`], [`crate::agents::PeerRequest::Move`], the
/// `move_agent` tool, the dispatcher's move step and the `device` that
/// `list_targets` takes. An older one refuses each, so the app says the
/// move needs a newer `maxd`.
pub const MOVE_AGENT_SINCE: u32 = 44;

/// The protocol version whose `maxd` first opens the timeline of a Claude
/// sub-agent that finished, read once from its transcript, when
/// [`ClientMessage::OpenTimeline`] names its lead. An older one sends an
/// empty timeline, so the app says the sub-agent isn't there.
pub const FINISHED_SUB_AGENTS_SINCE: u32 = 44;

/// The protocol version whose `maxd` first holds terminals: the terminal
/// requests from [`ClientMessage::TerminalOpen`] to
/// [`ClientMessage::TerminalMove`], the terminal frames, the
/// [`crate::terminal::Terminal`] entity, the `UserRun` timeline item and
/// the output pill. An older one refuses each, so the app hides the
/// terminal drawer and shell mode. A `maxd` at this version may still say
/// [`Capabilities::terminals`] is false.
pub const TERMINALS_SINCE: u32 = 45;

/// The protocol version whose `maxd` first lists and makes folders for the
/// path fields and the folder browser: [`ClientMessage::ListFolder`] and
/// [`ClientMessage::MakeFolder`]. An older one refuses both, so the app
/// suggests nothing there and browses that device's folders only on this
/// Mac, in the system picker.
pub const FOLDERS_SINCE: u32 = 46;

/// The protocol version whose `maxd` first keeps the switch for shell
/// integration, [`DeviceSettingChange::ShellIntegration`], and adds hooks to
/// the zsh, bash and fish it starts in terminals. An older one adds none,
/// so the app hides the switch.
pub const SHELL_MARKS_SINCE: u32 = 47;

/// The protocol version whose `maxd` first lets every agent list and read
/// other agents without a permission, and keeps the switch for it,
/// [`DeviceSettingChange::ReadOtherAgents`]. An older one lets agents read
/// only what they're allowed, so the app hides the switch.
pub const READ_RIGHTS_SINCE: u32 = 50;

/// The protocol version whose `maxd` first keeps a session for each agent,
/// the one shell its `!` commands share: the requests from
/// [`ClientMessage::SessionRun`] to [`ClientMessage::SessionEnd`], the
/// [`crate::terminal::TerminalKind::Session`] terminal, the `UserSession`
/// timeline item and the session pill. An older one refuses each, so the
/// app runs every `!` in a terminal of its own, as before.
pub const SESSIONS_SINCE: u32 = 48;

/// The protocol version whose `maxd` first keeps a notification log, for
/// [`ClientMessage::Notices`]. An older one refuses it, so the app lists
/// nothing earlier from that device.
pub const NOTICES_SINCE: u32 = 49;
/// The protocol version whose `maxd` first lets the dispatcher act: it
/// makes safe changes at once and plans the rest, with
/// [`crate::dispatch::PlanStep::Admin`], the `DispatcherAction` timeline
/// item, [`ClientMessage::UndoDispatcherAction`] and
/// [`crate::model::DeviceSettingChange::DispatcherActs`]. An older one
/// only plans, so the app hides the switch.
pub const DISPATCHER_ADMIN_SINCE: u32 = 52;

/// The protocol version whose `maxd` first keeps the dispatcher's chats:
/// the [`crate::dispatch::DispatcherChat`] entity,
/// [`ClientMessage::ActOnChat`] and
/// [`crate::model::DeviceSettingChange::ChatGapMinutes`]. An older one
/// keeps one conversation, so the app hides the Chats list.
pub const DISPATCHER_CHATS_SINCE: u32 = 53;

/// The protocol version whose `maxd` first runs local models: the
/// [`crate::local_models::ModelServer`] and
/// [`crate::local_models::LocalModels`] entities,
/// [`crate::model::LoginKind::ModelServer`] accounts, the local models
/// device setting and [`ClientMessage::ModelAction`]. An older one refuses
/// the action, and an older client leaves the entities and the accounts
/// out.
pub const LOCAL_MODELS_SINCE: u32 = 54;

/// The protocol version whose `maxd` first knows Claude Code's artifact
/// tools: the `artifact` part of a tool call, an approval's `deny_first`
/// and `reason`, and [`crate::model::DeviceSettingChange::ClaudeArtifacts`].
/// An older one shows them as plain tool calls and always lets agents use
/// them, so the app hides the switch.
pub const ARTIFACTS_SINCE: u32 = 55;

/// The protocol version whose `maxd` first lists and removes the shell
/// rules Cursor saved, for [`ClientMessage::CursorRules`]. An older one
/// refuses it, so Settings says that device needs a newer `maxd`. Since
/// protocol 57.
pub const CURSOR_RULES_SINCE: u32 = 57;

/// The protocol version whose `maxd` first asks before an agent in Accept
/// edits or Auto changes a hook, CI or task file: the card's
/// [`crate::timeline::Consent::edit`], the kinds in
/// [`crate::model::CommandRules::edits`] and
/// [`ClientMessage::RemoveAllowedEdit`]. An older one lets such edits
/// pass, so the app shows nothing to turn off.
pub const ALLOWED_EDITS_SINCE: u32 = 57;

/// The protocol version whose `maxd` first reports a Codex account's
/// banked resets in [`crate::model::Usage::reset_credits`] and spends one
/// for [`ClientMessage::UseResetCredit`]. An older one refuses the
/// message, so the app hides the button. Since protocol 58.
pub const RESET_CREDITS_SINCE: u32 = 58;

/// The protocol version whose `maxd` first answers the queries a program
/// sends its terminal, such as where the cursor is, and says how the
/// program reads input: [`crate::terminal::Terminal`]'s `reads_keys`,
/// `full_screen`, `app_cursor`, `waiting` and `answers`, the status's
/// `stopped`, the output ref's `signal` and `stopped`, and
/// [`ClientMessage::TerminalInterrupt`]'s `force`. With an older one a
/// client's view answers the queries, as before, and a session command
/// has no Force stop.
pub const INPUT_MODES_SINCE: u32 = 59;

/// The protocol version whose `maxd` first records the commands you type
/// by hand in an agent's session terminal as private blocks in its card,
/// [`crate::terminal::SessionBlock`]'s `typed`. An older one records
/// only the commands sent with `!`. Since protocol 60.
pub const TYPED_BLOCKS_SINCE: u32 = 60;

/// The protocol version whose `maxd` first reads a secret line, frame kind
/// [`crate::frame::FrameKind::TerminalSecret`]: it sends the bytes as
/// terminal input and records that a secret was sent, never the text. An
/// older one drops a frame of a kind it doesn't know, so a client sends
/// the line as plain input there. Since protocol 60.
pub const SECRET_INPUT_SINCE: u32 = 60;

/// The protocol version whose `maxd` first pushes to Android phones
/// through Firebase Cloud Messaging: it takes [`ClientMessage::SetFcmKey`]
/// and a [`ClientMessage::SetPushToken`] whose service is
/// [`PushService::Fcm`]. An older one knows APNs only, so a client doesn't
/// send it either. Since protocol 61.
pub const FCM_SINCE: u32 = 61;

/// When [`ClientMessage::TakeOver`] stops the terminal's CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TakeOverMode {
    /// Once the turn in progress ends, or now when none runs. The action is
    /// done once the wait starts.
    AfterTurn,
    /// Now, cutting off a turn in progress. The action is done once the
    /// session continues here.
    Now,
    /// Cancels a wait for the turn to end.
    Cancel,
}

/// Queue waits for the turn to end. Steer goes into the running turn.
/// Interrupt stops the turn and sends. All three just send when idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendMode {
    /// Waits for the turn to end.
    Queue,
    /// Goes into the running turn.
    Steer,
    /// Stops the turn and sends.
    Interrupt,
}

/// Where an attachment comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentSource {
    /// A file or folder on the device `maxd` runs on.
    Path {
        /// Absolute path on the device.
        path: String,
    },
    /// Pasted bytes, at most `MAX_INLINE_ATTACHMENT` (8 MiB) once decoded.
    Bytes {
        /// The file name to give it.
        name: String,
        /// The media type, such as "image/png".
        mime: String,
        /// The bytes, base64 encoded.
        data: String,
    },
    /// A file or folder the `@` menu found in an agent's folder. It stages
    /// on the agent's device, from its container when it runs in one.
    Found {
        /// The agent whose folder the search looked in.
        agent: AgentId,
        /// The path as the search gave it, relative to that folder.
        path: String,
    },
}

/// What an action came to. A repeated key gets the first outcome again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ActionOutcome {
    /// It's done.
    Done,
    /// A new agent is starting.
    Started {
        /// The new agent.
        agent: AgentId,
    },
    /// The attachment is ready to send.
    Staged {
        /// The staged attachment.
        attachment: AttachmentRef,
    },
    /// `maxd` won't do it.
    Refused {
        /// Why, for a person to read.
        reason: String,
    },
    /// `maxd` has no agent with that ID.
    UnknownAgent,
    /// A container workspace is being made.
    Created {
        /// The new workspace, which shows its progress.
        workspace: WorkspaceId,
    },
    /// A Sign in with ChatGPT round is waiting. Open `url` in the browser
    /// and listen on 127.0.0.1:`callback_port`.
    SignIn {
        /// The round, for [`ClientMessage::FinishChatGptSignIn`].
        sign_in: SignInId,
        /// The page to open.
        url: String,
        /// The local port the browser comes back to.
        callback_port: u16,
    },
    /// A forward is open.
    ForwardOpened {
        /// The new forward.
        forward: ForwardId,
    },
    /// What a [`ClientMessage::FindPaths`] found, best first.
    Paths {
        /// At most [`crate::find::MAX_PATH_MATCHES`].
        matches: Vec<PathMatch>,
    },
    /// What a [`ClientMessage::FindGitItems`] found.
    GitItems {
        /// The repo and what matched, or why nothing could be looked up.
        search: GitItemSearch,
    },
    /// What a [`ClientMessage::ListProcesses`] found.
    Processes {
        /// The agent's processes, or that its guest is too old to list
        /// them.
        list: ProcessList,
    },
    /// The note version a [`ClientMessage::ReadNote`] asked for.
    NoteRead {
        /// The version, with its body.
        version: NoteVersion,
    },
    /// What a [`ClientMessage::SaveNote`] made the note's head.
    NoteSaved {
        /// The new head.
        version: NoteVersion,
        /// Whether the save merged with changes made since its base, so
        /// the head holds more than what was sent.
        merged: bool,
    },
    /// The versions a [`ClientMessage::NoteHistory`] listed.
    NoteHistory {
        /// Newest first.
        versions: Vec<NoteVersionInfo>,
    },
    /// The hashes a [`ClientMessage::NoteHashes`] listed.
    NoteHashes {
        /// Every version the device holds for the project, sorted.
        hashes: Vec<VersionHash>,
    },
    /// The versions a [`ClientMessage::NoteVersions`] read.
    NoteVersions {
        /// Those it holds of the ones asked for, in the order asked, maybe
        /// only the first few.
        versions: Vec<NoteVersion>,
    },
    /// How many versions a [`ClientMessage::PutNoteVersions`] kept.
    NotesPut {
        /// The versions it didn't hold before.
        stored: u32,
    },
    /// The excerpt a [`ClientMessage::ExcerptAgent`] asked for.
    Excerpt {
        /// The Markdown and how much it holds.
        excerpt: AgentExcerpt,
    },
    /// What a [`ClientMessage::FindInTimeline`] found.
    TimelineFind {
        /// The matches and the active one.
        find: TimelineFind,
    },
    /// What a [`ClientMessage::ListIgnoredFiles`] found.
    IgnoredFiles {
        /// The files, sorted by path, at most 200.
        files: Vec<IgnoredFile>,
        /// Whether git listed more than `files` holds.
        truncated: bool,
    },
    /// What a [`ClientMessage::PlaceStaged`] moved in.
    Placed {
        /// How many files went in.
        written: u32,
        /// The files that didn't, and why.
        failed: Vec<PlaceFailure>,
    },
    /// The device dropped its ChatGPT sign-in.
    SignedOut {
        /// Whether ChatGPT confirmed it revoked the session. When it didn't,
        /// you disconnect the app in ChatGPT's settings.
        revoked: bool,
    },

    // Step 4: fleet tools.
    /// The device's free CPU and memory, for [`ClientMessage::ReadLoad`].
    Load(HostLoad),
    /// What a [`ClientMessage::ReadDiagnostics`] found.
    Diagnostics {
        /// The window cut into buckets, oldest first.
        buckets: Vec<UseBucket>,
        /// The processes that used the most CPU, most first.
        top: Vec<ProcessUse>,
        /// Each running container workspace's totals.
        containers: Vec<ContainerUse>,
        /// Why some of it couldn't be read, if something went wrong.
        error: Option<String>,
    },

    // Step 5: agents that run agents.
    /// What a [`ClientMessage::CallTool`] came to.
    Tool {
        /// The answer.
        answer: ToolAnswer,
    },
    /// What a [`ClientMessage::ListPermissions`] found.
    Permissions {
        /// The permissions, oldest first.
        permissions: Vec<Permission>,
    },
    /// The key [`ClientMessage::MintKey`] made.
    KeyMinted {
        /// The key's ID.
        id: KeyId,
        /// The key file, as an absolute path on the device.
        path: String,
        /// The permission, with the key as its holder.
        permission: PermissionId,
        /// What an MCP client on the device runs to use the key.
        command: McpCommand,
    },
    /// What a [`ClientMessage::ListKeys`] found.
    Keys {
        /// The keys, by ID.
        keys: Vec<AccessKey>,
    },

    // Step 6: scheduled tasks.
    /// The task [`ClientMessage::SaveTask`] stored.
    TaskSaved {
        /// The task, as `maxd` keeps it.
        task: Box<Task>,
        /// The webhook's URL, when this save made the webhook. It shows only
        /// this once.
        hook_url: Option<Secret>,
    },
    /// The new URL [`ClientMessage::RotateHookToken`] made. It shows only
    /// this once.
    HookUrl {
        /// The URL.
        url: Secret,
    },
    /// What a [`ClientMessage::ListRuns`] found.
    Runs {
        /// The runs, newest first.
        runs: Vec<Run>,
    },
    /// What a [`ClientMessage::ListDeliveries`] found.
    Deliveries {
        /// The deliveries, newest first.
        deliveries: Vec<TaskDelivery>,
    },
    /// The device's own SSH key, as `ssh-ed25519 <base64>`.
    PeerKey {
        /// The public half. The private half never leaves the device.
        public_key: String,
    },

    // Step 7: pull requests.
    /// A GitHub sign-in waits for you: open `verification_uri` and enter
    /// `user_code`.
    DeviceCode {
        /// The sign-in, for [`ClientMessage::CancelGithubSignIn`].
        sign_in: SignInId,
        /// The code to enter on GitHub.
        user_code: String,
        /// The page to enter it on.
        verification_uri: String,
        /// When the code stops working.
        expires_at: Timestamp,
    },
    // Signing in again.
    /// A CLI login waits for you: open `url` and sign in there.
    CliLogin {
        /// The sign-in, for [`ClientMessage::CheckCliLogin`].
        sign_in: SignInId,
        /// The sign-in page.
        url: String,
        /// The code to enter on that page, for a device code sign-in.
        user_code: Option<String>,
        /// Whether the page shows a code to paste back with
        /// [`ClientMessage::FinishCliLogin`].
        paste: bool,
    },
    /// How a CLI login stands.
    CliLoginState {
        /// Where it is now.
        state: CliLoginState,
    },

    /// What a [`ClientMessage::ListPulls`] found.
    Pulls {
        /// The PRs, newest change first.
        pulls: Vec<PullSummary>,
    },
    /// What a [`ClientMessage::ListBranches`] found.
    Branches {
        /// The branches by name, at most the first 100.
        branches: Vec<String>,
        /// The repository's default branch, when GitHub said.
        default_branch: Option<String>,
    },
    /// What a [`ClientMessage::GetPull`] read.
    Pull {
        /// The PR.
        pull: Box<PullDetail>,
    },
    /// What a [`ClientMessage::PullFiles`] read.
    PullFiles {
        /// The page's files.
        files: Vec<PullFile>,
        /// Whether another page follows.
        more: bool,
    },
    /// What a [`ClientMessage::PullComments`] read.
    PullComments {
        /// The comments, oldest first.
        comments: Vec<PullComment>,
    },
    /// What a [`ClientMessage::PullChecks`] read.
    PullChecks {
        /// The checks together. Unknown when the GitHub App can't read
        /// checks yet.
        state: ChecksState,
        /// Each check.
        checks: Vec<PullCheck>,
    },
    /// What a merge from [`ClientMessage::PullAction`] came to.
    PullMerged {
        /// GitHub took the merge but the PR is still open, as in a merge
        /// queue.
        queued: bool,
    },
    /// A workspace's snapshots, from [`ClientMessage::ListSnapshots`].
    Snapshots {
        /// Each one, oldest first.
        snapshots: Vec<WorkspaceSnapshot>,
    },
    /// A lease's screenshot, from [`ClientMessage::ReadLeaseScreen`].
    LeaseScreen {
        /// The image, a PNG.
        image: ToolImage,
        /// When it was taken.
        at: Timestamp,
    },
    /// More about a node, from [`ClientMessage::NodeDetail`].
    NodeDetail {
        /// The detail.
        detail: NodeDetail,
    },
    /// The skills each CLI sees, from [`ClientMessage::SkillsScan`].
    Skills {
        /// Every skill found, with the CLI that sees it.
        skills: Vec<EffectiveSkill>,
    },
    /// The trees the store lacks, from
    /// [`ClientMessage::SkillsTreesNeeded`].
    SkillTrees {
        /// The trees to upload.
        trees: Vec<TreeHash>,
    },
    /// What tidying would do, from [`ClientMessage::SkillsGlobalsPlan`].
    SkillsGlobalsPlan {
        /// The plan.
        plan: GlobalsPlan,
    },
    /// What a proposal changes, from [`ClientMessage::SkillProposalDiff`].
    SkillProposalDiff {
        /// The changed files and the flags.
        diff: ProposalDiff,
    },

    // The phone.
    /// A pairing waits for its phone, from [`ClientMessage::BeginPhonePair`].
    /// The Mac shows these in the QR code with the device's routes.
    PhonePair {
        /// The pairing, for `maxd proxy --pair`.
        pair: PairId,
        /// The one-time key's 32-byte Ed25519 seed, as base64.
        seed: Secret,
        /// This device's SSH host key, as `ssh-ed25519 <base64>`.
        host_key: String,
        /// When the one-time key stops working.
        expires_at: Timestamp,
    },
    /// The phone is paired, from [`ClientMessage::PairPhone`].
    PhonePaired {
        /// The phone's ID, for `maxd proxy --phone`.
        phone: PhoneId,
        /// The devices it may reach.
        book: Vec<PhoneBookEntry>,
        /// The Mac that paired it, when the Mac said.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mac: Option<PhoneMac>,
    },
    /// The phone is let in, from [`ClientMessage::AddPhone`] on a device
    /// whose network access is on.
    PhoneAdded {
        /// A one-time code bound to the phone, which the Mac puts in the
        /// phone's book for this device.
        code: Secret,
    },

    // The dispatcher.
    /// How each item of a confirmed plan went, from
    /// [`ClientMessage::ConfirmPlan`], in the plan's order. A failed item
    /// is the last.
    PlanRan {
        /// One for each item that ran. One from a newer `maxd` reads as
        /// unknown.
        #[serde(deserialize_with = "crate::tolerant::unknowns")]
        #[specta(type = Vec<PlanOutcome>)]
        results: Vec<PlanOutcome>,
    },

    // Proving who connects.
    /// A card on another device asks whether to trust the key, for
    /// [`ClientMessage::EnrollAppKey`].
    Asked,

    // Moving agents.
    /// What an agent took along when it left this device, for
    /// [`crate::agents::AgentMoveStep::Leave`].
    Departed {
        /// What it took.
        departure: Box<Departure>,
    },
    /// What an agent would run as on this device, for
    /// [`crate::agents::AgentMoveStep::Check`].
    Checked {
        /// What it would run as.
        check: Box<MoveCheck>,
    },

    // Terminals.
    /// A terminal is open, for [`ClientMessage::TerminalOpen`]. For
    /// [`ClientMessage::SessionRun`], the command runs in this fresh shell,
    /// whose `UserRun` card says why. For [`ClientMessage::SessionCd`], the
    /// session opened in the folder.
    TerminalOpened {
        /// The new terminal.
        terminal: TerminalId,
    },

    // Sessions.
    /// A command went to the agent's session as a block in its open card,
    /// for [`ClientMessage::SessionRun`] and [`ClientMessage::SessionCd`].
    SessionRan {
        /// The session's terminal.
        terminal: TerminalId,
        /// The card.
        item: ItemId,
        /// The block's ID in the card.
        block: u32,
    },

    // Folders.
    /// What a folder holds, for [`ClientMessage::ListFolder`].
    FolderListed {
        /// The listing.
        listing: FolderListing,
    },

    /// A page of the notification log, for [`ClientMessage::Notices`].
    Notices {
        /// The page.
        page: NoticePage,
    },

    /// A device's Cursor rules, for [`ClientMessage::CursorRules`]. Since
    /// protocol 57.
    CursorRules {
        /// The entries.
        rules: CursorRules,
    },

    /// What spending a banked reset came to, for
    /// [`ClientMessage::UseResetCredit`]. Since protocol 58.
    ResetCredit {
        /// What Codex answered.
        outcome: ResetCreditOutcome,
    },

    // Local models.
    /// The server a [`ModelAction::AddServer`] added, which its probe fills
    /// in.
    ModelServerAdded {
        /// The new server.
        server: ModelServerId,
    },
    /// What a [`ModelAction::ProbeServer`] found at a URL it didn't add.
    ServerProbed {
        /// What kind of server answered.
        flavor: ServerFlavor,
        /// Which APIs answered, or why nothing did.
        probe: ServerProbe,
        /// The models it serves.
        models: Vec<ServerModel>,
    },

    /// An outcome from a newer `maxd`. The action went through, and what
    /// it came to needs a newer app to show.
    Unknown,
}

/// What Codex answered when asked to spend a banked reset, from
/// [`ActionOutcome::ResetCredit`]. Since protocol 58.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResetCreditOutcome {
    /// It spent one and reset the account's limits.
    Reset,
    /// No limit needed a reset, so it spent nothing.
    NothingToReset,
    /// The account has no resets left.
    NoCredit,
    /// The same request had already spent one, as when a dropped answer
    /// was sent again. The limits are reset.
    AlreadyRedeemed,
}

/// Where a CLI login stands, from [`ActionOutcome::CliLoginState`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CliLoginState {
    /// The CLI waits for you to sign in.
    Waiting,
    /// The CLI stored the login.
    SignedIn,
    /// The login ended without signing in.
    Failed {
        /// Why, for a person to read.
        message: String,
    },
}

/// A file git ignores in a checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct IgnoredFile {
    /// The path relative to the checkout, with `/` between parts.
    pub path: String,
    /// Its size in bytes.
    #[specta(type = specta_typescript::Number)]
    pub size: u64,
}

/// A staged file that didn't go into a checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PlaceFailure {
    /// The path relative to the checkout, with `/` between parts.
    pub path: String,
    /// Why, for a person to read.
    pub reason: String,
}

/// A message from `maxd` to a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Answers the client's hello.
    Hello(ServerHello),
    /// The whole state.
    Snapshot(Snapshot),
    /// The next change to the state.
    Delta(Delta),
    /// What happened to an answer.
    AnswerResult {
        /// The key the answer carried.
        idempotency_key: IdempotencyKey,
        /// The result. One from a newer `maxd` reads as unknown.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = AnswerOutcome)]
        outcome: AnswerOutcome,
    },
    /// Answers a ping.
    Pong {
        /// The ping's nonce.
        nonce: u32,
    },
    /// The client sent something `maxd` couldn't act on.
    Error {
        /// What went wrong, for code to act on.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = ErrorCode)]
        code: ErrorCode,
        /// What went wrong, for a person to read.
        message: String,
    },
    /// What came of an action.
    ActionResult {
        /// The key the action carried.
        key: IdempotencyKey,
        /// The result. One from a newer `maxd` reads as unknown.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = ActionOutcome)]
        outcome: ActionOutcome,
    },
    /// A change to a timeline the client opened.
    Timeline {
        /// The agent whose timeline it is.
        agent: AgentId,
        /// The change.
        update: TimelineUpdate,
    },
    /// A change to a workflow run the client opened.
    Workflow {
        /// The run.
        run: RunKey,
        /// The change.
        update: Box<WorkflowUpdate>,
    },
    /// An upload has landed. `attachment` is what
    /// [`ActionOutcome::Staged`] carries.
    UploadDone {
        /// The upload.
        transfer: TransferId,
        /// What to send the agent.
        attachment: AttachmentRef,
    },
    /// What a download holds. Its chunks follow in transfer frames.
    FetchManifest {
        /// The download.
        transfer: TransferId,
        /// Every file, folder and link, folders before what's in them.
        entries: Vec<ManifestEntry>,
    },
    /// How much of an upload the device has, and how much more it takes.
    /// After a reconnect, `committed` is where the client picks up.
    TransferCredit {
        /// The upload.
        transfer: TransferId,
        /// Bytes written down for good.
        #[specta(type = specta_typescript::Number)]
        committed: u64,
        /// The client may send up to this offset.
        #[specta(type = specta_typescript::Number)]
        window_end: u64,
    },
    /// An upload or download stopped and won't go on.
    TransferFailed {
        /// The transfer.
        transfer: TransferId,
        /// What went wrong, for code to act on.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = ErrorCode)]
        code: ErrorCode,
        /// The manifest entry it stopped at, if it was one.
        entry: Option<u32>,
        /// What went wrong, for a person to read.
        message: String,
    },
    /// A stream the client opened is connected.
    TunnelOpened {
        /// The stream.
        stream: StreamId,
    },
    /// A stream the client opened couldn't connect.
    TunnelRefused {
        /// The stream.
        stream: StreamId,
        /// Why, for code to act on.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = ErrorCode)]
        code: ErrorCode,
        /// Why, for a person to read.
        message: String,
    },
    /// Lets the client send more of a stream's bytes.
    TunnelCredit {
        /// The stream.
        stream: StreamId,
        /// More bytes the client may send.
        #[specta(type = specta_typescript::Number)]
        granted: u64,
    },
    /// Ends a stream, or the device's half of it.
    TunnelClose {
        /// The stream.
        stream: StreamId,
        /// How it ends.
        how: TunnelEnd,
    },
    /// Asks the client, another device's `maxd` on a peer link it opened,
    /// for something. It answers with [`ClientMessage::PeerAnswer`].
    PeerRequest {
        /// Makes a resend after a dropped link safe.
        key: IdempotencyKey,
        /// The request. One from a newer `maxd` reads as unknown.
        #[serde(deserialize_with = "crate::tolerant::or_unknown")]
        #[specta(type = PeerRequest)]
        request: PeerRequest,
    },
    /// Answers [`ClientMessage::TerminalAttach`]: the terminal's output
    /// frames to this connection start at `from`.
    TerminalAttached {
        /// The terminal.
        terminal: TerminalId,
        /// The offset of the first output byte that follows.
        #[specta(type = specta_typescript::Number)]
        from: u64,
        /// Whether the client must clear what it shows and draw again from
        /// `from`, since what it had ended before what `maxd` still holds.
        reset: bool,
    },
}

impl ServerMessage {
    /// Reads a message, or none for one this build can't read, such as a
    /// type from a newer `maxd`, or one too large for its kind. Lists,
    /// timeline items, outcomes and requests are read a piece at a time.
    /// Each type it leaves out is logged once.
    pub fn read(json: &[u8]) -> Option<Self> {
        crate::raw::server(json)
    }
}

/// What went wrong with a client's message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No protocol version both sides speak. `maxd` closes the connection.
    UnsupportedProtocol,
    /// A message arrived before the hello.
    NoHello,
    /// The message didn't parse.
    BadMessage,
    /// `maxd` failed while handling it.
    Internal,
    /// The device won't do it, such as an upload to a place it doesn't own.
    NotAllowed,
    /// The device has no agent with that ID.
    NoSuchAgent,
    /// The device has no workspace with that ID.
    NoSuchWorkspace,
    /// The device has no listening port with that ID.
    NoSuchPort,
    /// The port was listening and has closed.
    PortGone,
    /// It's over a size or count limit.
    TooLarge,
    /// The device would run short of disk space.
    NoSpace,
    /// The device's transfer quota is used up.
    QuotaExceeded,
    /// A file's bytes don't match its SHA-256.
    HashMismatch,
    /// A path is absolute, escapes its folder, or goes through a link.
    BadPath,
    /// An absolute path is outside the agent's folders. You can still ask
    /// for that one file with Show anyway. A `maxd` sends
    /// [`ErrorCode::BadPath`] instead to a client from before
    /// [`SHOW_ANYWAY_SINCE`].
    Outside,
    /// The device can't reach where it would go, such as a stopped
    /// container.
    Unreachable,
    /// A code from a newer `maxd`. The message says what went wrong.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_message_goes_only_to_a_maxd_that_reads_it() {
        assert!(!reaches(21, Some(20)));
        assert!(reaches(21, Some(21)));
        assert!(reaches(21, Some(22)));
        assert!(reaches(21, None), "sent before the hello says");
        let ping = ClientMessage::Ping { nonce: 1 };
        assert!(ping.reaches(Some(OLDEST_PROTOCOL_VERSION)));
        assert!(!ping.reaches(Some(OLDEST_PROTOCOL_VERSION - 1)));
    }

    #[test]
    fn an_action_carries_its_key_and_other_messages_none() {
        let undo = ClientMessage::SkillsGlobalsUndo { key: key() };
        assert_eq!(undo.action_key(), Some(key()));
        assert_eq!(ClientMessage::Ping { nonce: 1 }.action_key(), None);
    }

    #[test]
    fn process_lists_and_signals_survive_a_round_trip() {
        use crate::processes::AgentProcess;

        let list = ClientMessage::ListProcesses {
            key: key(),
            agent: agent(),
        };
        assert_eq!(round_trip(&list)["type"], "list_processes");
        let signal = ClientMessage::SignalProcess {
            key: key(),
            agent: agent(),
            pid: 4242,
            start: 1_761_000_000_123_456,
            signal: ProcessSignal::Term,
        };
        assert_eq!(round_trip(&signal)["signal"], "term");
        for message in [&list, &signal] {
            assert_eq!(message.since(), 24);
            assert!(!message.reaches(Some(23)), "an older maxd never gets it");
        }

        let listed = round_trip(&ActionOutcome::Processes {
            list: ProcessList::Listed {
                processes: vec![AgentProcess {
                    pid: 4242,
                    parent: 4200,
                    start: 1_761_000_000_123_456,
                    run_ms: 65_000,
                    cpu_ms: 1_200,
                    memory: 52_428_800,
                    name: "node".into(),
                    command: "node vite --port 5173".into(),
                    cli: false,
                    ports: vec![5173],
                }],
            },
        });
        assert_eq!(listed["status"], "processes");
        assert_eq!(listed["list"]["status"], "listed");
        assert_eq!(listed["list"]["processes"][0]["ports"], json!([5173]));
        let old = round_trip(&ActionOutcome::Processes {
            list: ProcessList::TooOld,
        });
        assert_eq!(old["list"], json!({ "status": "too_old" }));
    }

    #[test]
    fn note_requests_and_answers_survive_a_round_trip() {
        use crate::notes::NoteAuthor;

        let project = ProjectId::derive("atlas");
        let note = NoteId::derive("release");
        let hash = VersionHash("ab".repeat(32));
        let version = NoteVersion {
            hash: hash.clone(),
            project,
            note,
            parents: Vec::new(),
            title: "Release checklist".into(),
            body: "- [ ] Tag\n".into(),
            deleted: false,
            at: Timestamp(1_761_000_000_000),
            by: Some(NoteAuthor {
                device: DeviceId::derive("devbox"),
                agent: None,
            }),
        };
        let messages = [
            ClientMessage::ReadNote {
                key: key(),
                project,
                note,
                version: None,
            },
            ClientMessage::SaveNote {
                key: key(),
                project,
                note,
                base: Some(hash.clone()),
                title: "Release checklist".into(),
                body: "- [x] Tag\n".into(),
                deleted: false,
            },
            ClientMessage::NoteHistory {
                key: key(),
                project,
                note,
            },
            ClientMessage::NoteHashes {
                key: key(),
                project,
            },
            ClientMessage::NoteVersions {
                key: key(),
                project,
                hashes: vec![hash.clone()],
            },
            ClientMessage::PutNoteVersions {
                key: key(),
                project,
                versions: vec![version.clone()],
            },
        ];
        let types: Vec<_> = messages
            .iter()
            .map(|message| round_trip(message)["type"].clone())
            .collect();
        assert_eq!(
            types,
            [
                "read_note",
                "save_note",
                "note_history",
                "note_hashes",
                "note_versions",
                "put_note_versions"
            ]
        );
        assert!(round_trip(&messages[0]).get("version").is_none());
        for message in &messages {
            assert_eq!(message.since(), 27);
            assert!(!message.reaches(Some(26)), "an older maxd never gets it");
        }

        let saved = round_trip(&ActionOutcome::NoteSaved {
            version: version.clone(),
            merged: true,
        });
        assert_eq!(saved["status"], "note_saved");
        assert_eq!(saved["version"]["hash"], json!(hash.0));
        let history = round_trip(&ActionOutcome::NoteHistory {
            versions: vec![NoteVersionInfo {
                hash: hash.clone(),
                parents: Vec::new(),
                title: version.title.clone(),
                deleted: false,
                at: version.at,
                by: None,
                size: 10,
            }],
        });
        assert_eq!(history["versions"][0]["size"], 10);
        round_trip(&ActionOutcome::NoteRead { version });
        round_trip(&ActionOutcome::NoteHashes { hashes: vec![hash] });
        round_trip(&ActionOutcome::NoteVersions {
            versions: Vec::new(),
        });
        assert_eq!(
            round_trip(&ActionOutcome::NotesPut { stored: 2 })["status"],
            "notes_put"
        );
    }

    #[test]
    fn peers_agree_on_the_newest_shared_version() {
        let old = ProtocolRange {
            current: 2,
            oldest: 1,
        };
        let new = ProtocolRange {
            current: 4,
            oldest: 2,
        };
        assert_eq!(old.agree(new), Some(2));
        assert_eq!(new.agree(old), Some(2));
        let newer = ProtocolRange {
            current: 5,
            oldest: 3,
        };
        assert_eq!(old.agree(newer), None);
    }

    #[test]
    fn a_refusal_names_the_range_maxd_speaks() {
        let maxd = ProtocolRange {
            current: 46,
            oldest: 45,
        };
        let message = maxd.refusal(ProtocolRange::OURS);
        assert_eq!(
            message,
            format!(
                "this maxd speaks protocol 45 to 46, and the client {} to {}",
                OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION
            )
        );
        assert_eq!(ProtocolRange::from_refusal(&message), Some(maxd));
        assert_eq!(ProtocolRange::from_refusal("bad frame"), None);
        assert_eq!(
            ProtocolRange::from_refusal("this maxd speaks protocol x to 46, and"),
            None
        );
    }

    #[test]
    fn version_one_peers_are_refused() {
        let v1 = ProtocolRange {
            current: 1,
            oldest: 1,
        };
        assert_eq!(ProtocolRange::OURS.agree(v1), None);
        assert_eq!(v1.agree(ProtocolRange::OURS), None);
        assert_eq!(
            ProtocolRange::OURS.agree(ProtocolRange::OURS),
            Some(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn a_peer_at_the_oldest_version_still_agrees() {
        let oldest = ProtocolRange {
            current: OLDEST_PROTOCOL_VERSION,
            oldest: OLDEST_PROTOCOL_VERSION,
        };
        assert_eq!(
            ProtocolRange::OURS.agree(oldest),
            Some(OLDEST_PROTOCOL_VERSION)
        );
        assert_eq!(
            oldest.agree(ProtocolRange::OURS),
            Some(OLDEST_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn version_two_to_six_peers_are_refused() {
        for version in [
            2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
        ] {
            let old = ProtocolRange {
                current: version,
                oldest: version,
            };
            assert_eq!(ProtocolRange::OURS.agree(old), None);
            assert_eq!(old.agree(ProtocolRange::OURS), None);
        }
        let reaching_back = ProtocolRange {
            current: 20,
            oldest: 1,
        };
        assert_eq!(ProtocolRange::OURS.agree(reaching_back), Some(20));
    }

    fn key() -> IdempotencyKey {
        "0199a7c2-0000-7000-8000-00000000000a".parse().unwrap()
    }

    fn agent() -> AgentId {
        "0199a7c2-0000-7000-8000-000000000001".parse().unwrap()
    }

    fn round_trip<T>(message: &T) -> serde_json::Value
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let value = serde_json::to_value(message).unwrap();
        let parsed: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&parsed, message, "{value}");
        value
    }

    #[test]
    fn a_message_with_a_note_pill_skips_an_older_maxd() {
        let plain = UserInput {
            text: "see \u{FFFC}".into(),
            attachments: Vec::new(),
            refs: vec![InlineRef::Quote {
                agent: agent(),
                agent_title: "api".into(),
                item: ItemId("claude:msg_01".into()),
                text: "the cache".into(),
                position: 0,
                source: crate::context::QuoteSource::Reply,
                from: None,
            }],
        };
        let noted = UserInput {
            refs: vec![InlineRef::Note {
                project: ProjectId::derive("project"),
                note: NoteId::derive("release"),
                title: "GTFS release steps".into(),
                copy: None,
            }],
            ..plain.clone()
        };
        let send = |input: &UserInput| ClientMessage::SendInput {
            key: key(),
            agent: agent(),
            input: input.clone(),
            mode: SendMode::Queue,
        };
        assert_eq!(send(&plain).since(), OLDEST_PROTOCOL_VERSION);
        assert_eq!(send(&noted).since(), NOTE_PILLS_SINCE);
        assert!(!send(&noted).reaches(Some(NOTE_PILLS_SINCE - 1)));
        assert!(send(&noted).reaches(Some(NOTE_PILLS_SINCE)));
        let start = ClientMessage::StartAgent {
            key: key(),
            spec: AgentSpec {
                effort: None,
                vendor: Vendor::Claude,
                account: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
                place: AgentPlace::Folder {
                    path: "/work".into(),
                },
                model: None,
                permission: None,
                title: None,
                first: Some(noted),
                branch: None,
                origin: None,
            },
        };
        assert_eq!(start.since(), NOTE_PILLS_SINCE);
    }

    #[test]
    fn an_effort_change_skips_an_older_maxd() {
        let set = ClientMessage::SetEffort {
            key: key(),
            agent: agent(),
            effort: Some("xhigh".into()),
        };
        let wire = round_trip(&set);
        assert_eq!(wire["type"], "set_effort");
        assert_eq!(wire["effort"], "xhigh");
        assert_eq!(set.since(), EFFORT_SINCE);
        assert!(!set.reaches(Some(EFFORT_SINCE - 1)));
        assert!(set.reaches(Some(EFFORT_SINCE)));
        assert_eq!(set.action_key(), Some(key()));
        // Left out, the effort is the vendor's default.
        let back: ClientMessage = serde_json::from_value(serde_json::json!({
            "type": "set_effort",
            "key": wire["key"],
            "agent": wire["agent"],
        }))
        .unwrap();
        assert!(matches!(
            back,
            ClientMessage::SetEffort { effort: None, .. }
        ));
        // A settings change waits for the same version only when it
        // carries an effort default.
        let device: DeviceId = "0199a7c2-0000-7000-8000-000000000007".parse().unwrap();
        let change = |changes| ClientMessage::UpdateDeviceSettings {
            key: key(),
            device,
            changes,
        };
        let plain = change(vec![DeviceSettingChange::SnoozeAtLimit { value: true }]);
        assert_eq!(plain.since(), 26);
        let effort = change(vec![
            DeviceSettingChange::SnoozeAtLimit { value: true },
            DeviceSettingChange::CodexEffort {
                value: Some("high".into()),
            },
        ]);
        assert_eq!(effort.since(), EFFORT_SINCE);
        assert_eq!(round_trip(&effort)["changes"][1]["field"], "codex_effort");
        assert_eq!(EFFORT_SINCE, 41);
    }

    #[test]
    fn a_message_with_a_visual_pill_skips_an_older_maxd() {
        let visual = InlineRef::Visual {
            attachment: "0199a7c2-0000-7000-8000-00000000000a".parse().unwrap(),
            title: "Weekly usage".into(),
            from: None,
        };
        let input = UserInput {
            text: "Per \u{FFFC}, make the bars thinner \u{FFFC}".into(),
            attachments: vec!["0199a7c2-0000-7000-8000-00000000000a".parse().unwrap()],
            refs: vec![
                InlineRef::Note {
                    project: ProjectId::derive("project"),
                    note: NoteId::derive("release"),
                    title: "GTFS release steps".into(),
                    copy: None,
                },
                visual.clone(),
            ],
        };
        let send = ClientMessage::SendInput {
            key: key(),
            agent: agent(),
            input,
            mode: SendMode::Queue,
        };
        assert_eq!(send.since(), VISUALS_SINCE);
        assert!(!send.reaches(Some(VISUALS_SINCE - 1)));
        assert!(send.reaches(Some(VISUALS_SINCE)));
        let answer = ClientMessage::Answer {
            idempotency_key: key(),
            event: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            answer: AttentionAnswer::Text {
                text: "Like \u{FFFC}".into(),
            },
            refs: vec![visual],
        };
        assert_eq!(answer.since(), VISUALS_SINCE);
        assert!(!answer.reaches(Some(VISUALS_SINCE - 1)));
        assert_eq!(VISUALS_SINCE, 39);
        const { assert!(VISUALS_SINCE <= crate::PROTOCOL_VERSION) };
    }

    #[test]
    fn a_sub_agents_lead_is_a_hint_an_older_maxd_ignores() {
        let open = |lead| ClientMessage::OpenTimeline {
            agent: agent(),
            since: None,
            limit: 50,
            lead,
        };
        assert!(round_trip(&open(None)).get("lead").is_none());
        let lead = AgentId::derive("lead");
        assert_eq!(round_trip(&open(Some(lead)))["lead"], json!(lead));
        // An older maxd reads past it, so it goes to every maxd.
        assert_eq!(open(Some(lead)).since(), OLDEST_PROTOCOL_VERSION);
        let old = json!({ "type": "open_timeline", "agent": agent(), "since": null, "limit": 50 });
        let read: ClientMessage = serde_json::from_value(old).unwrap();
        assert!(matches!(
            read,
            ClientMessage::OpenTimeline { lead: None, .. }
        ));
        assert_eq!(FINISHED_SUB_AGENTS_SINCE, 44);
    }

    #[test]
    fn show_anyway_goes_only_to_a_maxd_that_reads_it() {
        let fetch = |anyway| ClientMessage::FetchBegin {
            key: key(),
            transfer: transfer(),
            agent: agent(),
            path: "/tmp/example/shot.png".into(),
            anyway,
        };
        assert_eq!(fetch(false).since(), OLDEST_PROTOCOL_VERSION);
        assert!(round_trip(&fetch(false)).get("anyway").is_none());
        assert_eq!(round_trip(&fetch(true))["anyway"], true);
        assert_eq!(fetch(true).since(), SHOW_ANYWAY_SINCE);
        assert!(!fetch(true).reaches(Some(SHOW_ANYWAY_SINCE - 1)));
        assert!(fetch(true).reaches(Some(SHOW_ANYWAY_SINCE)));
        let old = json!({
            "type": "fetch_begin",
            "key": key(),
            "transfer": transfer(),
            "agent": agent(),
            "path": "a.png",
        });
        let read: ClientMessage = serde_json::from_value(old).unwrap();
        assert!(matches!(
            read,
            ClientMessage::FetchBegin { anyway: false, .. }
        ));
    }

    #[test]
    fn an_agents_limit_switch_goes_only_to_a_maxd_that_keeps_it() {
        let set = |change| ClientMessage::SetInbox {
            key: key(),
            agent: agent(),
            change,
        };
        let switch = set(InboxChange::ContinueAtLimit { on: true });
        assert_eq!(
            round_trip(&switch)["change"],
            json!({ "change": "continue_at_limit", "on": true })
        );
        assert_eq!(switch.since(), CONTINUE_AT_LIMIT_SINCE);
        assert!(!switch.reaches(Some(CONTINUE_AT_LIMIT_SINCE - 1)));
        assert!(switch.reaches(Some(CONTINUE_AT_LIMIT_SINCE)));
        assert_eq!(CONTINUE_AT_LIMIT_SINCE, 38);
        let settle = set(InboxChange::AutoSettle { on: false });
        assert_eq!(settle.since(), OLDEST_PROTOCOL_VERSION);
    }

    #[test]
    fn a_take_over_goes_only_to_a_maxd_that_stops_the_terminal() {
        let take = |mode| ClientMessage::TakeOver {
            key: key(),
            agent: agent(),
            mode,
        };
        assert_eq!(round_trip(&take(TakeOverMode::Now))["mode"], "now");
        assert_eq!(
            round_trip(&take(TakeOverMode::AfterTurn))["mode"],
            "after_turn"
        );
        for mode in [
            TakeOverMode::AfterTurn,
            TakeOverMode::Now,
            TakeOverMode::Cancel,
        ] {
            assert_eq!(take(mode).since(), TAKE_OVER_SINCE);
            assert!(!take(mode).reaches(Some(TAKE_OVER_SINCE - 1)));
            assert!(take(mode).reaches(Some(TAKE_OVER_SINCE)));
            assert_eq!(take(mode).action_key(), Some(key()));
        }
        assert_eq!(TAKE_OVER_SINCE, 41);
    }

    #[test]
    fn a_dismiss_goes_only_to_a_maxd_that_reads_it() {
        let answer = |answer| ClientMessage::Answer {
            idempotency_key: key(),
            event: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            answer,
            refs: Vec::new(),
        };
        let dismiss = answer(AttentionAnswer::Dismiss);
        assert_eq!(
            round_trip(&dismiss)["answer"],
            serde_json::json!({ "type": "dismiss" })
        );
        assert_eq!(dismiss.since(), DISMISS_SINCE);
        assert!(!dismiss.reaches(Some(DISMISS_SINCE - 1)));
        assert!(dismiss.reaches(Some(DISMISS_SINCE)));
        let pick = answer(AttentionAnswer::Option { id: "a".into() });
        assert_eq!(pick.since(), OLDEST_PROTOCOL_VERSION);
        assert_eq!(DISMISS_SINCE, 42);
        const { assert!(DISMISS_SINCE <= crate::PROTOCOL_VERSION) };
    }

    #[test]
    fn an_update_carries_its_release_and_still_reaches_an_older_maxd() {
        let update = |release, unsigned| ClientMessage::InstallUpdate {
            key: key(),
            staged: "0199a7c2-0000-7000-8000-000000000003".parse().unwrap(),
            kind: UpdateKind::Maxd,
            sha256: "0".repeat(64),
            release,
            unsigned,
        };
        let signed = update(
            Some(SignedRelease {
                manifest: "{}".into(),
                signature: "c2ln".into(),
            }),
            false,
        );
        let json = round_trip(&signed);
        assert_eq!(json["release"]["manifest"], "{}");
        assert!(json.get("unsigned").is_none());
        let unsigned = update(None, true);
        let json = round_trip(&unsigned);
        assert!(json.get("release").is_none());
        assert_eq!(json["unsigned"], true);
        // An older maxd ignores both and installs as before, so neither
        // holds the update back from it.
        assert_eq!(signed.since(), OLDEST_PROTOCOL_VERSION);
        assert_eq!(unsigned.since(), OLDEST_PROTOCOL_VERSION);
        // A client from before sends neither.
        let old: ClientMessage = serde_json::from_value(serde_json::json!({
            "type": "install_update",
            "key": key(),
            "staged": "0199a7c2-0000-7000-8000-000000000003",
            "kind": "maxd",
            "sha256": "0".repeat(64),
        }))
        .unwrap();
        assert_eq!(old, update(None, false));
        assert_eq!(SIGNED_UPDATES_SINCE, 43);
        const { assert!(SIGNED_UPDATES_SINCE <= crate::PROTOCOL_VERSION) };
    }

    #[test]
    fn new_client_messages_survive_a_round_trip() {
        let attachment: AttachmentId = "0199a7c2-0000-7000-8000-000000000003".parse().unwrap();
        let input = UserInput {
            text: "Fix the build in \u{FFFC}, see #12".into(),
            attachments: vec![attachment],
            refs: vec![
                InlineRef::Attachment { id: attachment },
                InlineRef::GitItem {
                    repo: "owner/repo".into(),
                    number: 12,
                    kind: crate::context::GitItemKind::Issue,
                    title: "Build fails".into(),
                    state: crate::context::GitItemState::Open,
                    url: "https://github.com/owner/repo/issues/12".into(),
                    head: None,
                    base: None,
                    body: String::new(),
                    checked_at: crate::time::Timestamp(1_761_350_400_000),
                },
            ],
        };
        let messages = [
            (
                "start_agent",
                ClientMessage::StartAgent {
                    key: key(),
                    spec: AgentSpec {
                        effort: None,
                        vendor: Vendor::Claude,
                        account: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
                        place: AgentPlace::Workspace {
                            workspace: "0199a7c2-0000-7000-8000-000000000006".parse().unwrap(),
                        },
                        model: Some("opus".into()),
                        permission: None,
                        title: None,
                        first: Some(input.clone()),
                        branch: Some("maxhq/0199a7c2".into()),
                        origin: None,
                    },
                },
            ),
            (
                "start_agent",
                ClientMessage::StartAgent {
                    key: key(),
                    spec: AgentSpec {
                        effort: None,
                        vendor: Vendor::Codex,
                        account: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
                        place: AgentPlace::Folder {
                            path: "/work/scratch".into(),
                        },
                        model: None,
                        permission: Some(PermissionMode::FullAccess),
                        title: Some("Scratch".into()),
                        first: None,
                        branch: None,
                        origin: None,
                    },
                },
            ),
            (
                "send_input",
                ClientMessage::SendInput {
                    key: key(),
                    agent: agent(),
                    input,
                    mode: SendMode::Steer,
                },
            ),
            (
                "cancel_queued",
                ClientMessage::CancelQueued {
                    key: key(),
                    agent: agent(),
                    item: ItemId("maxhq:queued_1".into()),
                },
            ),
            (
                "interrupt",
                ClientMessage::Interrupt {
                    key: key(),
                    agent: agent(),
                },
            ),
            (
                "set_permission",
                ClientMessage::SetPermission {
                    key: key(),
                    agent: agent(),
                    mode: PermissionMode::AcceptEdits,
                },
            ),
            (
                "set_model",
                ClientMessage::SetModel {
                    key: key(),
                    agent: agent(),
                    model: "sonnet".into(),
                    account: None,
                },
            ),
            (
                "stop_agent",
                ClientMessage::StopAgent {
                    key: key(),
                    agent: agent(),
                },
            ),
            (
                "delete_agent",
                ClientMessage::DeleteAgent {
                    key: key(),
                    agent: agent(),
                },
            ),
            (
                "rename_agent",
                ClientMessage::RenameAgent {
                    key: key(),
                    agent: agent(),
                    title: "Release notes".into(),
                },
            ),
            (
                "continue_here",
                ClientMessage::ContinueHere {
                    key: key(),
                    agent: agent(),
                },
            ),
            (
                "take_over",
                ClientMessage::TakeOver {
                    key: key(),
                    agent: agent(),
                    mode: TakeOverMode::AfterTurn,
                },
            ),
            (
                "stage_attachment",
                ClientMessage::StageAttachment {
                    key: key(),
                    source: AttachmentSource::Path {
                        path: "/Users/max/shot.png".into(),
                    },
                },
            ),
            (
                "stage_attachment",
                ClientMessage::StageAttachment {
                    key: key(),
                    source: AttachmentSource::Bytes {
                        name: "paste.png".into(),
                        mime: "image/png".into(),
                        data: "iVBORw0KGgo=".into(),
                    },
                },
            ),
            (
                "stage_attachment",
                ClientMessage::StageAttachment {
                    key: key(),
                    source: AttachmentSource::Found {
                        agent: agent(),
                        path: "src/main.rs".into(),
                    },
                },
            ),
            (
                "set_task_enabled",
                ClientMessage::SetTaskEnabled {
                    key: key(),
                    task: "0199a7c2-0000-7000-8000-000000000042".parse().unwrap(),
                    enabled: false,
                },
            ),
            (
                "update_device_settings",
                ClientMessage::UpdateDeviceSettings {
                    key: key(),
                    device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
                    changes: vec![
                        DeviceSettingChange::SnoozeAtLimit { value: true },
                        DeviceSettingChange::WorkspaceNetwork { value: None },
                    ],
                },
            ),
            (
                "set_device_settings",
                ClientMessage::SetDeviceSettings {
                    key: key(),
                    settings: DeviceSettings {
                        device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
                        default_permission: PermissionMode::Auto,
                        workspace_network: Some("bridge0".into()),
                        workspace_pool: Some("agents".into()),
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
                        claude_artifacts: true,
                    },
                },
            ),
            (
                "open_timeline",
                ClientMessage::OpenTimeline {
                    agent: agent(),
                    since: Some(Seq(9)),
                    limit: 200,
                    lead: None,
                },
            ),
            (
                "load_older",
                ClientMessage::LoadOlder {
                    agent: agent(),
                    before: 400,
                    limit: 100,
                },
            ),
            (
                "close_timeline",
                ClientMessage::CloseTimeline { agent: agent() },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
        }
    }

    #[test]
    fn lookups_and_what_they_find_survive_a_round_trip() {
        use crate::context::{GitItemKind, GitItemState};
        use crate::find::PathKind;
        use crate::timeline::{FindItem, FindMatch};

        let asks = [
            (
                "find_paths",
                ClientMessage::FindPaths {
                    key: key(),
                    agent: agent(),
                    query: "src/ma".into(),
                },
            ),
            (
                "find_git_items",
                ClientMessage::FindGitItems {
                    key: key(),
                    agent: agent(),
                    query: "12".into(),
                    repo: None,
                },
            ),
            (
                "find_git_items",
                ClientMessage::FindGitItems {
                    key: key(),
                    agent: agent(),
                    query: "7".into(),
                    repo: Some("someone/else".into()),
                },
            ),
            (
                "excerpt_agent",
                ClientMessage::ExcerptAgent {
                    key: key(),
                    agent: agent(),
                    budget: 24_000,
                },
            ),
            (
                "guest_ssh_open",
                ClientMessage::GuestSshOpen {
                    stream: stream(),
                    workspace: workspace(),
                },
            ),
            (
                "set_guest_ssh_key",
                ClientMessage::SetGuestSshKey {
                    key: key(),
                    public_key: "ssh-ed25519 AAAA".into(),
                },
            ),
            (
                "find_in_timeline",
                ClientMessage::FindInTimeline {
                    key: key(),
                    agent: agent(),
                    query: "cache".into(),
                    at: FindAt::Index { index: 3 },
                },
            ),
            (
                "find_in_timeline",
                ClientMessage::FindInTimeline {
                    key: key(),
                    agent: agent(),
                    query: "cache".into(),
                    at: FindAt::From {
                        from: FindMatch {
                            item: ItemId("claude:msg_01".into()),
                            occurrence: 1,
                        },
                        offset: -1,
                    },
                },
            ),
        ];
        for (tag, message) in asks {
            assert_eq!(round_trip(&message)["type"], tag);
        }
        let paths = round_trip(&ActionOutcome::Paths {
            matches: vec![PathMatch {
                path: "src/main.rs".into(),
                kind: PathKind::File,
            }],
        });
        assert_eq!(
            paths,
            json!({
                "status": "paths",
                "matches": [{ "path": "src/main.rs", "kind": "file" }],
            })
        );
        let items = round_trip(&ActionOutcome::GitItems {
            search: GitItemSearch {
                repo: Some("owner/repo".into()),
                items: vec![InlineRef::GitItem {
                    repo: "owner/repo".into(),
                    number: 12,
                    kind: GitItemKind::PullRequest,
                    title: "Fix the build".into(),
                    state: GitItemState::Draft,
                    url: "https://github.com/owner/repo/pull/12".into(),
                    head: Some("fix/build".into()),
                    base: Some("main".into()),
                    body: String::new(),
                    checked_at: crate::time::Timestamp(1_761_350_400_000),
                }],
                unavailable: None,
            },
        });
        assert_eq!(items["status"], "git_items");
        assert_eq!(items["search"]["items"][0]["type"], "git_item");
        assert_eq!(items["search"]["unavailable"], json!(null));
        let excerpt = round_trip(&ActionOutcome::Excerpt {
            excerpt: AgentExcerpt {
                name: "agent-api-refactor.md".into(),
                text: "# Agent: api refactor".into(),
                messages: 3,
                omitted: 0,
            },
        });
        assert_eq!(excerpt["status"], "excerpt");
        assert_eq!(excerpt["excerpt"]["messages"], 3);
        let at = serde_json::to_value(FindAt::From {
            from: FindMatch {
                item: ItemId("claude:msg_01".into()),
                occurrence: 2,
            },
            offset: 1,
        })
        .unwrap();
        assert_eq!(
            at,
            json!({
                "type": "from",
                "from": { "item": "claude:msg_01", "occurrence": 2 },
                "offset": 1,
            })
        );
        let find = round_trip(&ActionOutcome::TimelineFind {
            find: TimelineFind {
                total: 4,
                active: 1,
                found: Some(FindMatch {
                    item: ItemId("claude:msg_01".into()),
                    occurrence: 1,
                }),
                near: vec![FindItem {
                    item: ItemId("claude:msg_01".into()),
                    first: 0,
                    count: 4,
                }],
            },
        });
        assert_eq!(find["status"], "timeline_find");
        assert_eq!(find["find"]["found"]["occurrence"], 1);
        assert_eq!(find["find"]["near"][0]["count"], 4);
    }

    #[test]
    fn new_server_messages_survive_a_round_trip() {
        use crate::timeline::{AttachmentKind, ItemBody, ItemStatus, TimelineEvent, TimelineItem};

        let item = TimelineItem {
            id: ItemId("claude:msg_01".into()),
            agent: agent(),
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Running,
            started_at: None,
            completed_at: None,
            body: ItemBody::AssistantMessage {
                text: "Looking".into(),
                streaming: true,
            },
        };
        let outcomes = [
            ActionOutcome::Done,
            ActionOutcome::Started { agent: agent() },
            ActionOutcome::Staged {
                attachment: AttachmentRef {
                    id: "0199a7c2-0000-7000-8000-000000000003".parse().unwrap(),
                    name: "notes".into(),
                    kind: AttachmentKind::Folder,
                    bytes: 10_240,
                    mime: None,
                    path: "/tmp/maxhq/notes".into(),
                    thumbnail: None,
                },
            },
            ActionOutcome::Refused {
                reason: "not yet".into(),
            },
            ActionOutcome::UnknownAgent,
        ];
        for outcome in outcomes {
            let value = round_trip(&ServerMessage::ActionResult {
                key: key(),
                outcome,
            });
            assert_eq!(value["type"], "action_result");
            assert!(value["outcome"]["status"].is_string());
        }
        let updates = [
            TimelineUpdate::Reset {
                seq: Seq(3),
                items: vec![item.clone()],
                has_older: true,
            },
            TimelineUpdate::Events {
                seq: Seq(4),
                events: vec![TimelineEvent::Upsert { item: item.clone() }],
            },
            TimelineUpdate::Older {
                items: vec![item],
                has_older: false,
            },
        ];
        for update in updates {
            let value = round_trip(&ServerMessage::Timeline {
                agent: agent(),
                update,
            });
            assert_eq!(value["type"], "timeline");
        }
    }

    fn transfer() -> TransferId {
        "0199a7c2-0000-7000-8000-0000000000d1".parse().unwrap()
    }

    fn stream() -> StreamId {
        "0199a7c2-0000-7000-8000-0000000000d2".parse().unwrap()
    }

    fn forward() -> ForwardId {
        "0199a7c2-0000-7000-8000-0000000000d3".parse().unwrap()
    }

    fn workspace() -> WorkspaceId {
        "0199a7c2-0000-7000-8000-000000000006".parse().unwrap()
    }

    fn entries() -> Vec<ManifestEntry> {
        vec![
            ManifestEntry {
                path: "notes".into(),
                kind: EntryKind::Dir,
                executable: false,
                size: 0,
                sha256: None,
            },
            ManifestEntry {
                path: "notes/run.sh".into(),
                kind: EntryKind::File,
                executable: true,
                size: 120,
                sha256: Some(
                    "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
                ),
            },
            ManifestEntry {
                path: "notes/latest".into(),
                kind: EntryKind::Symlink {
                    target: "run.sh".into(),
                },
                executable: false,
                size: 0,
                sha256: None,
            },
        ]
    }

    #[test]
    fn m3_client_messages_survive_a_round_trip() {
        let project = "0199a7c2-0000-7000-8000-000000000008".parse().unwrap();
        let device = "0199a7c2-0000-7000-8000-000000000007".parse().unwrap();
        let messages = [
            (
                "create_workspace",
                ClientMessage::CreateWorkspace {
                    key: key(),
                    spec: WorkspaceSpec {
                        project,
                        device,
                        name: Some("maxhq".into()),
                        start_at_boot: true,
                    },
                },
            ),
            (
                "start_workspace",
                ClientMessage::StartWorkspace {
                    key: key(),
                    workspace: workspace(),
                },
            ),
            (
                "stop_workspace",
                ClientMessage::StopWorkspace {
                    key: key(),
                    workspace: workspace(),
                },
            ),
            (
                "delete_workspace",
                ClientMessage::DeleteWorkspace {
                    key: key(),
                    workspace: workspace(),
                    discard_unpushed: false,
                },
            ),
            (
                "set_project_settings",
                ClientMessage::SetProjectSettings {
                    key: key(),
                    settings: ProjectSettings {
                        project,
                        device,
                        resume_on_restart: true,
                        ignored_ports: vec![9229],
                        git_author: Some(crate::model::GitAuthor {
                            name: "Max Boksem".into(),
                            email: "max@example.com".into(),
                        }),
                        auto_settle_days: None,
                        settle_on_merge: true,
                        workflow_env: true,
                        memory_soft: None,
                        workflow_env_allowed: None,
                    },
                },
            ),
            (
                "set_project_hidden",
                ClientMessage::SetProjectHidden {
                    key: key(),
                    device,
                    project,
                    hidden: true,
                },
            ),
            (
                "remove_command_rule",
                ClientMessage::RemoveCommandRule {
                    key: key(),
                    device,
                    project,
                    rule: CommandRule {
                        command: "rm -rf build".into(),
                        exact: true,
                    },
                },
            ),
            (
                "remove_allowed_edit",
                ClientMessage::RemoveAllowedEdit {
                    key: key(),
                    device,
                    project,
                    kind: EditKind::GitHooks,
                },
            ),
            (
                "wind_down",
                ClientMessage::WindDown {
                    key: key(),
                    window_secs: 1200,
                },
            ),
            (
                "store_claude_token",
                ClientMessage::StoreClaudeToken {
                    key: key(),
                    origin: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
                    label: "Personal".into(),
                    token: Secret::new("test-not-a-real-token".into()),
                },
            ),
            (
                "store_cursor_key",
                ClientMessage::StoreCursorKey {
                    key: key(),
                    label: "Personal".into(),
                    token: Secret::new("test-not-a-real-key".into()),
                },
            ),
            (
                "begin_chat_gpt_sign_in",
                ClientMessage::BeginChatGptSignIn { key: key() },
            ),
            (
                "finish_chat_gpt_sign_in",
                ClientMessage::FinishChatGptSignIn {
                    key: key(),
                    sign_in: "0199a7c2-0000-7000-8000-0000000000d4".parse().unwrap(),
                    code: Secret::new("test-not-a-real-code".into()),
                    state: "test-state".into(),
                    client_id: Some("oaiapp_test".into()),
                },
            ),
            (
                "set_github_app",
                ClientMessage::SetGithubApp {
                    key: key(),
                    app_id: 1_234_567,
                    private_key: Secret::new("test-not-a-real-key".into()),
                },
            ),
            (
                "check_github_app",
                ClientMessage::CheckGithubApp { key: key() },
            ),
            (
                "forget_credential",
                ClientMessage::ForgetCredential {
                    key: key(),
                    credential: "0199a7c2-0000-7000-8000-0000000000d5".parse().unwrap(),
                },
            ),
            (
                "upload_begin",
                ClientMessage::UploadBegin {
                    key: key(),
                    transfer: transfer(),
                    target: UploadTarget::Agent { agent: agent() },
                    source: UploadSource::Stream {
                        name: "notes".into(),
                        entries: entries(),
                    },
                },
            ),
            (
                "upload_begin",
                ClientMessage::UploadBegin {
                    key: key(),
                    transfer: transfer(),
                    target: UploadTarget::Workspace {
                        workspace: workspace(),
                    },
                    source: UploadSource::DevicePath {
                        path: "/home/dev/shot.png".into(),
                    },
                },
            ),
            (
                "upload_begin",
                ClientMessage::UploadBegin {
                    key: key(),
                    transfer: transfer(),
                    target: UploadTarget::Device,
                    source: UploadSource::Stream {
                        name: "empty".into(),
                        entries: vec![],
                    },
                },
            ),
            (
                "upload_cancel",
                ClientMessage::UploadCancel {
                    transfer: transfer(),
                },
            ),
            (
                "fetch_begin",
                ClientMessage::FetchBegin {
                    key: key(),
                    transfer: transfer(),
                    agent: agent(),
                    path: "dist/report.pdf".into(),
                    anyway: false,
                },
            ),
            (
                "transfer_credit",
                ClientMessage::TransferCredit {
                    transfer: transfer(),
                    committed: 512 * 1024,
                    window_end: 8 * 1024 * 1024,
                },
            ),
            (
                "open_forward",
                ClientMessage::OpenForward {
                    key: key(),
                    port: "0199a7c2-0000-7000-8000-0000000000d6".parse().unwrap(),
                    local_port: 5173,
                },
            ),
            (
                "close_forward",
                ClientMessage::CloseForward { forward: forward() },
            ),
            (
                "tunnel_open",
                ClientMessage::TunnelOpen {
                    stream: stream(),
                    forward: forward(),
                },
            ),
            (
                "tunnel_credit",
                ClientMessage::TunnelCredit {
                    stream: stream(),
                    granted: 256 * 1024,
                },
            ),
            (
                "tunnel_close",
                ClientMessage::TunnelClose {
                    stream: stream(),
                    how: TunnelEnd::Write,
                },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
        }
    }

    #[test]
    fn copy_messages_survive_a_round_trip() {
        let project = ProjectId::derive("git:github.com/owner/name");
        let device: DeviceId = "0199a7c2-0000-7000-8000-000000000007".parse().unwrap();
        let attachment: AttachmentId = "0199a7c2-0000-7000-8000-000000000003".parse().unwrap();
        let messages = [
            (
                "list_ignored_files",
                ClientMessage::ListIgnoredFiles {
                    key: key(),
                    workspace: workspace(),
                },
            ),
            (
                "fetch_files",
                ClientMessage::FetchFiles {
                    key: key(),
                    transfer: transfer(),
                    workspace: workspace(),
                    paths: vec![".env".into(), "config/local.toml".into()],
                },
            ),
            (
                "place_staged",
                ClientMessage::PlaceStaged {
                    key: key(),
                    workspace: workspace(),
                    attachment,
                    overwrite: false,
                },
            ),
            (
                "clone_folder",
                ClientMessage::CloneFolder {
                    key: key(),
                    project,
                    device,
                    parent: "/home/dev/src".into(),
                    name: "name".into(),
                },
            ),
            (
                "clone_cancel",
                ClientMessage::CloneCancel {
                    key: key(),
                    workspace: workspace(),
                },
            ),
            (
                "clone_retry",
                ClientMessage::CloneRetry {
                    key: key(),
                    workspace: workspace(),
                },
            ),
            (
                "clone_remove",
                ClientMessage::CloneRemove {
                    key: key(),
                    workspace: workspace(),
                },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
        }
        assert_eq!(
            round_trip(&ActionOutcome::IgnoredFiles {
                files: vec![IgnoredFile {
                    path: ".env".into(),
                    size: 120,
                }],
                truncated: false,
            }),
            json!({
                "status": "ignored_files",
                "files": [{ "path": ".env", "size": 120 }],
                "truncated": false,
            })
        );
        assert_eq!(
            round_trip(&ActionOutcome::Placed {
                written: 1,
                failed: vec![PlaceFailure {
                    path: ".env.local".into(),
                    reason: "a file is already there".into(),
                }],
            }),
            json!({
                "status": "placed",
                "written": 1,
                "failed": [{ "path": ".env.local", "reason": "a file is already there" }],
            })
        );
    }

    #[test]
    fn secrets_in_messages_stay_out_of_debug_output() {
        let message = ClientMessage::StoreClaudeToken {
            key: key(),
            origin: "0199a7c2-0000-7000-8000-000000000005".parse().unwrap(),
            label: "Personal".into(),
            token: Secret::new("test-not-a-real-token".into()),
        };
        let printed = format!("{message:?}");
        assert!(printed.contains("<secret>"), "{printed}");
        assert!(!printed.contains("test-not-a-real-token"), "{printed}");
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["token"], "test-not-a-real-token");
    }

    #[test]
    fn m3_server_messages_survive_a_round_trip() {
        use crate::timeline::AttachmentKind;

        let messages = [
            (
                "upload_done",
                ServerMessage::UploadDone {
                    transfer: transfer(),
                    attachment: AttachmentRef {
                        id: "0199a7c2-0000-7000-8000-000000000003".parse().unwrap(),
                        name: "notes".into(),
                        kind: AttachmentKind::Folder,
                        bytes: 120,
                        mime: None,
                        path: "/var/lib/maxhq/attachments/notes".into(),
                        thumbnail: None,
                    },
                },
            ),
            (
                "fetch_manifest",
                ServerMessage::FetchManifest {
                    transfer: transfer(),
                    entries: entries(),
                },
            ),
            (
                "transfer_credit",
                ServerMessage::TransferCredit {
                    transfer: transfer(),
                    committed: 0,
                    window_end: 512 * 1024,
                },
            ),
            (
                "transfer_failed",
                ServerMessage::TransferFailed {
                    transfer: transfer(),
                    code: ErrorCode::HashMismatch,
                    entry: Some(1),
                    message: "notes/run.sh doesn't match its hash".into(),
                },
            ),
            (
                "tunnel_opened",
                ServerMessage::TunnelOpened { stream: stream() },
            ),
            (
                "tunnel_refused",
                ServerMessage::TunnelRefused {
                    stream: stream(),
                    code: ErrorCode::PortGone,
                    message: "port 5173 closed".into(),
                },
            ),
            (
                "tunnel_credit",
                ServerMessage::TunnelCredit {
                    stream: stream(),
                    granted: 64 * 1024,
                },
            ),
            (
                "tunnel_close",
                ServerMessage::TunnelClose {
                    stream: stream(),
                    how: TunnelEnd::Reset,
                },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
        }

        let outcomes = [
            (
                "created",
                ActionOutcome::Created {
                    workspace: workspace(),
                },
            ),
            (
                "sign_in",
                ActionOutcome::SignIn {
                    sign_in: "0199a7c2-0000-7000-8000-0000000000d4".parse().unwrap(),
                    url: "https://auth.example.com/authorize".into(),
                    callback_port: 1455,
                },
            ),
            (
                "forward_opened",
                ActionOutcome::ForwardOpened { forward: forward() },
            ),
            ("signed_out", ActionOutcome::SignedOut { revoked: false }),
        ];
        for (status, outcome) in outcomes {
            let value = round_trip(&ServerMessage::ActionResult {
                key: key(),
                outcome,
            });
            assert_eq!(value["outcome"]["status"], status);
        }
    }

    #[test]
    fn new_error_codes_are_snake_case() {
        let codes = [
            (ErrorCode::NotAllowed, "not_allowed"),
            (ErrorCode::NoSuchAgent, "no_such_agent"),
            (ErrorCode::NoSuchWorkspace, "no_such_workspace"),
            (ErrorCode::NoSuchPort, "no_such_port"),
            (ErrorCode::PortGone, "port_gone"),
            (ErrorCode::TooLarge, "too_large"),
            (ErrorCode::NoSpace, "no_space"),
            (ErrorCode::QuotaExceeded, "quota_exceeded"),
            (ErrorCode::HashMismatch, "hash_mismatch"),
            (ErrorCode::BadPath, "bad_path"),
            (ErrorCode::Outside, "outside"),
            (ErrorCode::Unreachable, "unreachable"),
        ];
        for (code, name) in codes {
            assert_eq!(round_trip(&code), json!(name));
        }
    }

    #[test]
    fn server_hellos_carry_capabilities() {
        let hello = ServerMessage::Hello(ServerHello {
            protocol: ProtocolRange::OURS,
            maxd_version: "0.1.0".into(),
            build: Some("0123456789abcdef".into()),
            target: Some("x86_64-unknown-linux-musl".into()),
            device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
            boot_id: BootId("4f1c".into()),
            instance: "0199a7c2-0000-7000-8000-0000000000d7".parse().unwrap(),
            capabilities: Capabilities {
                transfers: true,
                tunnels: false,
                workspaces: true,
                github_checks: true,
                inbox: false,
                guest_ssh: true,
                copy: true,
                load: true,
                updates: false,
                tools: true,
                agent_tools: false,
                scheduled_tasks: false,
                peers: true,
                acp_agents: false,
                github_user: false,
                pulls: false,
                test_hosts: false,
                workflows: false,
                host_spec: false,
                snapshots: false,
                moves: false,
                tool_installs: false,
                skills: false,
                phones: false,
                library_moves: false,
                proposal_targets: false,
                peer_proofs: false,
                terminals: false,
                local_models: false,
            },
            challenge: None,
        });
        let value = round_trip(&hello);
        assert_eq!(
            value["capabilities"],
            json!({
                "transfers": true,
                "tunnels": false,
                "workspaces": true,
                "github_checks": true,
                "inbox": false,
                "guest_ssh": true,
                "copy": true,
                "load": true,
                "updates": false,
                "tools": true,
                "agent_tools": false,
                "scheduled_tasks": false,
                "peers": true,
                "acp_agents": false,
                "github_user": false,
                "pulls": false,
                "test_hosts": false,
                "workflows": false,
                "host_spec": false,
                "snapshots": false,
                "moves": false,
                "tool_installs": false,
                "skills": false,
                "phones": false,
                "library_moves": false,
                "proposal_targets": false,
                "peer_proofs": false,
                "terminals": false,
                "local_models": false,
            })
        );
        assert_eq!(value["build"], json!("0123456789abcdef"));
        assert_eq!(value["target"], json!("x86_64-unknown-linux-musl"));
        assert_eq!(
            value["protocol"],
            json!({ "current": PROTOCOL_VERSION, "oldest": OLDEST_PROTOCOL_VERSION })
        );
        assert_eq!(
            Capabilities::default(),
            Capabilities {
                transfers: false,
                tunnels: false,
                workspaces: false,
                github_checks: false,
                inbox: false,
                ..Capabilities::default()
            }
        );
    }

    #[test]
    fn acp_agent_messages_round_trip() {
        use crate::acp::{AcpAgent, AcpEnvValue};
        let agent = AcpAgent {
            id: "0199a7c2-0000-7000-8000-0000000000e2".parse().unwrap(),
            device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
            name: "Gemini".into(),
            program: "gemini".into(),
            args: vec!["--experimental-acp".into()],
            env: vec!["GEMINI_API_KEY".into()],
            containers: true,
            problem: None,
            skills: Default::default(),
        };
        let set = ClientMessage::SetAcpAgent {
            key: key(),
            agent: agent.clone(),
            env: vec![AcpEnvValue {
                name: "GEMINI_API_KEY".into(),
                value: Secret::new("test-not-a-real-key".into()),
            }],
        };
        let value = round_trip(&set);
        assert_eq!(value["type"], json!("set_acp_agent"));
        assert_eq!(value["env"][0]["name"], json!("GEMINI_API_KEY"));
        let delete = ClientMessage::DeleteAcpAgent {
            key: key(),
            id: agent.id,
        };
        assert_eq!(round_trip(&delete)["type"], json!("delete_acp_agent"));
        // A set with no changed values may leave `env` out.
        let mut bare = serde_json::to_value(&set).unwrap();
        bare.as_object_mut().unwrap().remove("env");
        let parsed: ClientMessage = serde_json::from_value(bare).unwrap();
        assert!(matches!(parsed, ClientMessage::SetAcpAgent { env, .. } if env.is_empty()));
    }

    #[test]
    fn test_host_messages_round_trip() {
        let host = crate::leases::host_id(
            "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
            crate::leases::TestKind::Linux,
            crate::leases::TestHostBackend::IncusTest,
        );
        let lease: LeaseId = "0199a7c2-0000-7000-8000-0000000000e3".parse().unwrap();
        for (message, kind) in [
            (
                ClientMessage::CheckTestHosts { key: key() },
                "check_test_hosts",
            ),
            (
                ClientMessage::BuildTestHost { key: key(), host },
                "build_test_host",
            ),
            (
                ClientMessage::ResetTestHost { key: key(), host },
                "reset_test_host",
            ),
            (ClientMessage::EndLease { key: key(), lease }, "end_lease"),
            (
                ClientMessage::ReadLeaseScreen { key: key(), lease },
                "read_lease_screen",
            ),
        ] {
            assert_eq!(round_trip(&message)["type"], json!(kind));
        }
        let screen = ActionOutcome::LeaseScreen {
            image: ToolImage {
                mime: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
            },
            at: Timestamp(5),
        };
        let value = serde_json::to_value(&screen).unwrap();
        assert_eq!(value["image"]["mime"], json!("image/png"));
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(value).unwrap(),
            screen
        );
        // A `maxd` from before test hosts reads as offering none.
        let old: Capabilities = serde_json::from_value(json!({
            "transfers": true, "tunnels": true, "workspaces": true
        }))
        .unwrap();
        assert!(!old.test_hosts);
    }

    #[test]
    fn workflow_and_skills_messages_round_trip() {
        use crate::skills::{SkillLock, SourceKind, SourceTrust, TreeHash};
        use crate::workflow::RunKey;
        let run = RunKey {
            device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
            project: ProjectId::derive("git:github.com/owner/repo"),
            workflow: "devflow".into(),
            run: "run-1".into(),
        };
        let project = run.project;
        let source = crate::ids::SkillSourceId::derive("owner/repo");
        let proposal: SkillProposalId = "0199a7c2-0000-7000-8000-0000000000e4".parse().unwrap();
        for (message, kind) in [
            (
                ClientMessage::OpenWorkflow { run: run.clone() },
                "open_workflow",
            ),
            (
                ClientMessage::CloseWorkflow { run: run.clone() },
                "close_workflow",
            ),
            (
                ClientMessage::NodeDetail {
                    key: key(),
                    run: run.clone(),
                    node: "plan".into(),
                },
                "node_detail",
            ),
            (
                ClientMessage::SkillSourceAdd {
                    key: key(),
                    name: "agent-skills".into(),
                    kind: SourceKind::Local {
                        path: "/home/agent/skills".into(),
                    },
                    default_ref: None,
                    trust: SourceTrust::Own,
                },
                "skill_source_add",
            ),
            (
                ClientMessage::SkillSourceFetch { key: key(), source },
                "skill_source_fetch",
            ),
            (
                ClientMessage::SkillSourceRemove { key: key(), source },
                "skill_source_remove",
            ),
            (
                ClientMessage::SkillsSetProject {
                    key: key(),
                    project,
                    lock: SkillLock::default(),
                },
                "skills_set_project",
            ),
            (
                ClientMessage::SkillsApply {
                    key: key(),
                    project,
                },
                "skills_apply",
            ),
            (
                ClientMessage::SkillsScan {
                    key: key(),
                    place: AgentPlace::Folder {
                        path: "/home/agent/work".into(),
                    },
                },
                "skills_scan",
            ),
            (
                ClientMessage::SkillsTreesNeeded {
                    key: key(),
                    lock: SkillLock::default(),
                },
                "skills_trees_needed",
            ),
            (
                ClientMessage::SkillProposalAction {
                    key: key(),
                    proposal,
                    action: ProposalAction::Approve,
                    push: true,
                    fork: Some("tidy-fork".into()),
                    elsewhere: None,
                    seen: None,
                },
                "skill_proposal_action",
            ),
            (
                ClientMessage::SkillProposalDiff {
                    key: key(),
                    proposal,
                    elsewhere: None,
                },
                "skill_proposal_diff",
            ),
            (
                ClientMessage::SkillsTakeLibrary {
                    key: key(),
                    from: "Mac".into(),
                    sources: Vec::new(),
                },
                "skills_take_library",
            ),
            (
                ClientMessage::SkillSourcePush { key: key(), source },
                "skill_source_push",
            ),
            (
                ClientMessage::SkillsGlobalsPlan { key: key() },
                "skills_globals_plan",
            ),
            (
                ClientMessage::SkillsGlobalsApply {
                    key: key(),
                    plan_hash: "ab12".into(),
                    picks: vec!["/home/dev/.claude/skills/grill-me".into()],
                },
                "skills_globals_apply",
            ),
            (
                ClientMessage::SkillsSetAlways {
                    key: key(),
                    skills: Vec::new(),
                },
                "skills_set_always",
            ),
            (
                ClientMessage::FetchSkillTree {
                    key: key(),
                    transfer: transfer(),
                    tree: TreeHash("ab12".into()),
                },
                "fetch_skill_tree",
            ),
            (
                ClientMessage::SkillsGlobalsUndo { key: key() },
                "skills_globals_undo",
            ),
        ] {
            assert_eq!(round_trip(&message)["type"], json!(kind));
        }
        // An app from before picks sends none.
        let apply: ClientMessage = serde_json::from_value(json!({
            "type": "skills_globals_apply", "key": key(), "plan_hash": "ab12"
        }))
        .unwrap();
        assert!(
            matches!(apply, ClientMessage::SkillsGlobalsApply { picks, .. } if picks.is_empty())
        );
        let target = UploadTarget::SkillStore {
            tree: TreeHash("ab12".into()),
        };
        let value = serde_json::to_value(&target).unwrap();
        assert_eq!(value, json!({ "type": "skill_store", "tree": "ab12" }));
        assert_eq!(
            serde_json::from_value::<UploadTarget>(value).unwrap(),
            target
        );
        let trees = ActionOutcome::SkillTrees {
            trees: vec![TreeHash("ab12".into())],
        };
        let value = serde_json::to_value(&trees).unwrap();
        assert_eq!(value["status"], json!("skill_trees"));
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(value).unwrap(),
            trees
        );
        // A `maxd` from before workflows and skills reads as offering
        // neither.
        let old: Capabilities = serde_json::from_value(json!({
            "transfers": true, "tunnels": true, "workspaces": true
        }))
        .unwrap();
        assert!(!old.workflows && !old.skills);
    }

    #[test]
    fn a_task_switch_and_a_settings_change_don_t_reach_an_older_maxd() {
        let device: DeviceId = "0199a7c2-0000-7000-8000-000000000007".parse().unwrap();
        let switch = ClientMessage::SetTaskEnabled {
            key: key(),
            task: "0199a7c2-0000-7000-8000-000000000042".parse().unwrap(),
            enabled: true,
        };
        assert_eq!(round_trip(&switch)["enabled"], true);
        let change = ClientMessage::UpdateDeviceSettings {
            key: key(),
            device,
            changes: vec![DeviceSettingChange::WorkspacePool { value: None }],
        };
        let wire = round_trip(&change);
        assert_eq!(wire["changes"][0]["field"], "workspace_pool");
        assert_eq!(wire["changes"][0]["value"], serde_json::Value::Null);
        for message in [&switch, &change] {
            assert_eq!(message.since(), 26);
            assert!(!message.reaches(Some(25)), "an older maxd never gets it");
            assert!(message.reaches(Some(26)));
        }
    }

    #[test]
    fn the_setup_token_switch_goes_only_to_a_maxd_that_keeps_it() {
        let device: DeviceId = "0199a7c2-0000-7000-8000-000000000007".parse().unwrap();
        let update = |changes| ClientMessage::UpdateDeviceSettings {
            key: key(),
            device,
            changes,
        };
        let switch = update(vec![
            DeviceSettingChange::WorkspacePool { value: None },
            DeviceSettingChange::ClaudeSetupToken { value: true },
        ]);
        assert_eq!(
            round_trip(&switch)["changes"][1],
            json!({ "field": "claude_setup_token", "value": true })
        );
        assert_eq!(switch.since(), SETUP_TOKENS_SINCE);
        assert!(!switch.reaches(Some(SETUP_TOKENS_SINCE - 1)));
        assert!(switch.reaches(Some(SETUP_TOKENS_SINCE)));
        let pool = update(vec![DeviceSettingChange::WorkspacePool { value: None }]);
        assert_eq!(pool.since(), 26);
        let shells = update(vec![DeviceSettingChange::ShellIntegration { value: false }]);
        assert_eq!(
            round_trip(&shells)["changes"][0],
            json!({ "field": "shell_integration", "value": false })
        );
        assert_eq!(shells.since(), SHELL_MARKS_SINCE);
        const { assert!(SHELL_MARKS_SINCE <= crate::PROTOCOL_VERSION) };
        let reads = update(vec![DeviceSettingChange::ReadOtherAgents { value: false }]);
        assert_eq!(
            round_trip(&reads)["changes"][0],
            json!({ "field": "read_other_agents", "value": false })
        );
        assert_eq!(reads.since(), READ_RIGHTS_SINCE);
        assert!(!reads.reaches(Some(READ_RIGHTS_SINCE - 1)));
        const { assert!(READ_RIGHTS_SINCE <= crate::PROTOCOL_VERSION) };
    }

    #[test]
    fn settings_changes_name_only_what_differs_and_rebuild_the_rest() {
        let device: DeviceId = "0199a7c2-0000-7000-8000-000000000007".parse().unwrap();
        let old = DeviceSettings {
            device,
            default_permission: PermissionMode::Ask,
            workspace_network: Some("bridge0".into()),
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
            claude_artifacts: true,
        };
        assert_eq!(old.changes_from(Some(&old)), []);
        let new = DeviceSettings {
            workspace_network: None,
            snooze_at_limit: true,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            ..old.clone()
        };
        let changes = new.changes_from(Some(&old));
        assert_eq!(
            changes,
            [
                DeviceSettingChange::WorkspaceNetwork { value: None },
                DeviceSettingChange::SnoozeAtLimit { value: true },
            ]
        );
        let mut rebuilt = old.clone();
        changes.into_iter().for_each(|one| rebuilt.change(one));
        assert_eq!(rebuilt, new);
        // With nothing to compare with, it names every setting.
        let mut whole = DeviceSettings {
            default_permission: PermissionMode::Auto,
            ..old.clone()
        };
        new.changes_from(None)
            .into_iter()
            .for_each(|one| whole.change(one));
        assert_eq!(whole, new);
    }

    #[test]
    fn fcm_messages_reach_only_a_maxd_that_knows_fcm() {
        use crate::phones::{PushEnvironment, PushKind};
        let token = |service| ClientMessage::SetPushToken {
            key: key(),
            push: PushToken {
                token: Secret::new("test-not-a-real-token".into()),
                topic: "com.example.maxhq".into(),
                environment: PushEnvironment::Production,
                kinds: vec![PushKind::NeedsInput],
                service,
            },
        };
        let apns = token(PushService::Apns);
        assert!(apns.reaches(Some(OLDEST_PROTOCOL_VERSION)));
        let fcm = token(PushService::Fcm);
        assert!(!fcm.reaches(Some(FCM_SINCE - 1)));
        assert!(fcm.reaches(Some(FCM_SINCE)));
        assert!(fcm.reaches(None), "a peer not known yet gets it");
        let set = ClientMessage::SetFcmKey {
            key: key(),
            project_id: "<PROJECT_ID>".into(),
            client_email: "pusher@example.invalid".into(),
            private_key: Secret::new("test-not-a-real-key".into()),
        };
        assert!(!set.reaches(Some(FCM_SINCE - 1)));
        assert!(set.reaches(Some(FCM_SINCE)));
        assert!(set.action_key().is_some());
    }

    #[test]
    fn phone_messages_round_trip() {
        use crate::phones::{PhoneRoute, PushEnvironment, PushKind};
        let phone = PhoneId::derive("phone");
        let pair = PairId::derive("pair");
        let book = vec![PhoneBookEntry {
            device: DeviceId::derive("devbox"),
            name: "devbox".into(),
            routes: vec![PhoneRoute {
                user: "dev".into(),
                host: "devbox.local".into(),
                port: 22,
            }],
            host_key: "ssh-ed25519 AAAAexample".into(),
            network: None,
        }];
        for (message, kind) in [
            (ClientMessage::Phone { phone }, "phone"),
            (ClientMessage::Pair { pair }, "pair"),
            (
                ClientMessage::BeginPhonePair {
                    key: key(),
                    book: book.clone(),
                    mac: None,
                },
                "begin_phone_pair",
            ),
            (
                ClientMessage::PairPhone {
                    key: key(),
                    name: "iPhone".into(),
                    ssh_public: "ssh-ed25519 AAAAexample".into(),
                    push_public: "AAAAexample".into(),
                },
                "pair_phone",
            ),
            (
                ClientMessage::AddPhone {
                    key: key(),
                    phone,
                    name: "iPhone".into(),
                    ssh_public: "ssh-ed25519 AAAAexample".into(),
                    push_public: "AAAAexample".into(),
                },
                "add_phone",
            ),
            (
                ClientMessage::RemovePhone { key: key(), phone },
                "remove_phone",
            ),
            (
                ClientMessage::RevokePhoneSession {
                    key: key(),
                    session: PhoneSessionId::derive("session"),
                },
                "revoke_phone_session",
            ),
            (
                ClientMessage::SetPhoneBook {
                    key: key(),
                    phone,
                    book: book.clone(),
                    mac: None,
                },
                "set_phone_book",
            ),
            (
                ClientMessage::SetPhoneEverything {
                    key: key(),
                    phone,
                    everything: true,
                },
                "set_phone_everything",
            ),
            (
                ClientMessage::SetPushToken {
                    key: key(),
                    push: PushToken {
                        token: Secret::new("test-not-a-real-token".into()),
                        topic: "com.example.maxhq".into(),
                        environment: PushEnvironment::Production,
                        kinds: vec![PushKind::NeedsInput],
                        service: PushService::Apns,
                    },
                },
                "set_push_token",
            ),
            (
                ClientMessage::Presence {
                    agent: Some(agent()),
                },
                "presence",
            ),
            (
                ClientMessage::SetApnsKey {
                    key: key(),
                    key_id: "<KEY_ID>".into(),
                    team_id: "<TEAM_ID>".into(),
                    private_key: Secret::new("test-not-a-real-key".into()),
                },
                "set_apns_key",
            ),
            (
                ClientMessage::SetFcmKey {
                    key: key(),
                    project_id: "<PROJECT_ID>".into(),
                    client_email: "pusher@example.invalid".into(),
                    private_key: Secret::new("test-not-a-real-key".into()),
                },
                "set_fcm_key",
            ),
        ] {
            let printed = format!("{message:?}");
            assert!(!printed.contains("test-not-a-real"), "{printed}");
            assert_eq!(round_trip(&message)["type"], json!(kind));
        }
        for (outcome, status) in [
            (
                ActionOutcome::PhonePair {
                    pair,
                    seed: Secret::new("test-not-a-real-seed".into()),
                    host_key: "ssh-ed25519 AAAAexample".into(),
                    expires_at: Timestamp(600_000),
                },
                "phone_pair",
            ),
            (
                ActionOutcome::PhonePaired {
                    phone,
                    book,
                    mac: None,
                },
                "phone_paired",
            ),
            (
                ActionOutcome::PhoneAdded {
                    code: Secret::new("test-not-a-real-code".into()),
                },
                "phone_added",
            ),
        ] {
            assert!(!format!("{outcome:?}").contains("test-not-a-real"));
            let value = serde_json::to_value(&outcome).unwrap();
            assert_eq!(value["status"], json!(status));
            assert_eq!(
                serde_json::from_value::<ActionOutcome>(value).unwrap(),
                outcome
            );
        }
        // A `maxd` from before phones reads as pairing none.
        let old: Capabilities = serde_json::from_value(json!({
            "transfers": true, "tunnels": true, "workspaces": true
        }))
        .unwrap();
        assert!(!old.phones);
    }

    #[test]
    fn dispatcher_messages_round_trip() {
        use crate::dispatch::{Dispatcher, Plan, PlanItem, PlanStep, PlanTarget};
        use crate::state::Entity;
        use crate::tasks::{Filter, FilterTest, HookRules, Trigger};

        let device = DeviceId::derive("mac");
        let task = TaskId::derive("task");
        for (message, kind) in [
            (
                ClientMessage::AskDispatcher {
                    key: key(),
                    text: "Review new PRs on owner/repo".into(),
                    account: AccountId::derive("claude"),
                    new: false,
                    refs: Vec::new(),
                },
                "ask_dispatcher",
            ),
            (
                ClientMessage::ConfirmPlan {
                    key: key(),
                    plan: 2,
                },
                "confirm_plan",
            ),
            (
                ClientMessage::DismissPlan {
                    key: key(),
                    plan: 2,
                },
                "dismiss_plan",
            ),
            (
                ClientMessage::UndoDispatcherAction {
                    key: key(),
                    action: key(),
                },
                "undo_dispatcher_action",
            ),
            (
                ClientMessage::ActOnChat {
                    key: key(),
                    agent: agent(),
                    action: ChatAct::Rename {
                        title: "Release notes".into(),
                    },
                },
                "act_on_chat",
            ),
        ] {
            assert_eq!(round_trip(&message)["type"], json!(kind));
        }
        let ran = ActionOutcome::PlanRan {
            results: vec![
                PlanOutcome::Started { agent: agent() },
                PlanOutcome::TaskSaved { task },
                PlanOutcome::Failed {
                    reason: "that account is gone".into(),
                },
            ],
        };
        let value = round_trip(&ran);
        assert_eq!(value["status"], json!("plan_ran"));
        assert_eq!(value["results"][1]["type"], json!("task_saved"));

        let spec = AgentSpec {
            effort: None,
            vendor: Vendor::Cursor,
            account: AccountId::derive("cursor"),
            place: AgentPlace::Folder {
                path: "/work/repo".into(),
            },
            model: None,
            permission: None,
            title: Some("PR reviews".into()),
            first: None,
            branch: None,
            origin: None,
        };
        let rules = HookRules {
            filters: vec![Filter {
                path: "body.pull_request.user.login".into(),
                test: FilterTest::Equals {
                    value: "octocat".into(),
                },
            }],
            coalesce_key: Some("{{body.repository.full_name}}".into()),
            coalesce_secs: Some(900),
            ..HookRules::default()
        };
        let entity = Entity::Dispatcher(Dispatcher {
            device,
            workspace: WorkspaceId::derive("dispatcher"),
            agent: Some(agent()),
            plan: Some(Plan {
                id: 2,
                items: vec![
                    PlanItem {
                        reason: "An agent to review the PRs".into(),
                        step: PlanStep::StartAgent {
                            device,
                            spec: Box::new(spec),
                        },
                    },
                    PlanItem {
                        reason: "Sends it each batch of PR changes".into(),
                        step: PlanStep::ScheduleTask {
                            device,
                            name: "Review PRs".into(),
                            prompt: "Review {{body.pull_request.html_url}}".into(),
                            trigger: Box::new(Trigger::GithubEvents {
                                repo: "owner/repo".into(),
                                events: vec!["pull_request".into()],
                                rules,
                            }),
                            target: PlanTarget::Item { item: 0 },
                        },
                    },
                    PlanItem {
                        reason: "You asked to rename it".into(),
                        step: PlanStep::Admin {
                            device,
                            action: crate::dispatch::AdminAction::RenameAgent {
                                agent: agent(),
                                title: "Auth fix".into(),
                            },
                        },
                    },
                ],
            }),
            undo: vec![key()],
        });
        let value = round_trip(&entity);
        assert_eq!(value["entity"], json!("dispatcher"));
        assert_eq!(value["plan"]["items"][2]["step"]["type"], json!("admin"));
        assert_eq!(
            value["plan"]["items"][2]["step"]["action"]["action"],
            json!("rename_agent")
        );
        assert_eq!(
            value["plan"]["items"][0]["step"]["type"],
            json!("start_agent")
        );
        assert_eq!(
            value["plan"]["items"][1]["step"]["target"]["type"],
            json!("item")
        );
        assert_eq!(entity.key(), crate::state::EntityKey::Dispatcher(device));

        let chat = Entity::DispatcherChat(crate::dispatch::DispatcherChat {
            agent: agent(),
            device,
            number: 3,
            title: "Release notes".into(),
            summary: "Drafting the notes for the next release".into(),
            project: None,
            active_at: crate::time::Timestamp(1_800_000_000_000),
            archived: false,
            moved_to: None,
        });
        let value = round_trip(&chat);
        assert_eq!(value["entity"], json!("dispatcher_chat"));
        assert_eq!(value["active_at"], json!(1_800_000_000_000_u64));
        assert_eq!(chat.key(), crate::state::EntityKey::DispatcherChat(agent()));
        assert_eq!(
            round_trip(&ChatAct::Archive { archived: true })["type"],
            json!("archive")
        );
        assert_eq!(
            ClientMessage::ActOnChat {
                key: key(),
                agent: agent(),
                action: ChatAct::Pick,
            }
            .since(),
            DISPATCHER_CHATS_SINCE
        );
    }

    #[test]
    fn host_move_and_install_messages_round_trip() {
        use crate::hosts::{MoveStep, WorkspaceSnapshot};
        use crate::state::{HelperTool, ToolKind, ToolPlace, ToolState};
        let workspace: WorkspaceId = "0199a7c2-0000-7000-8000-0000000000f1".parse().unwrap();
        let moving: MoveId = "0199a7c2-0000-7000-8000-0000000000f2".parse().unwrap();
        for (message, kind) in [
            (
                ClientMessage::InstallTool {
                    key: key(),
                    place: ToolPlace::Host,
                    tool: ToolKind::Helper {
                        tool: HelperTool::AndroidSdk,
                    },
                    version: None,
                },
                "install_tool",
            ),
            (
                ClientMessage::RemoveTool {
                    key: key(),
                    place: ToolPlace::Workspace { workspace },
                    tool: ToolKind::Acp {
                        id: "gemini".into(),
                    },
                },
                "remove_tool",
            ),
            (ClientMessage::CheckHost { key: key() }, "check_host"),
            (
                ClientMessage::ApplyHost {
                    key: key(),
                    items: vec!["incus.bridge".into()],
                },
                "apply_host",
            ),
            (
                ClientMessage::ListSnapshots {
                    key: key(),
                    workspace,
                },
                "list_snapshots",
            ),
            (
                ClientMessage::RestoreSnapshot {
                    key: key(),
                    workspace,
                    snapshot: "auto-0001".into(),
                    as_new: false,
                    discard_unsaved: true,
                },
                "restore_snapshot",
            ),
            (
                ClientMessage::PlanMove {
                    key: key(),
                    instance: "ws-alpha".into(),
                },
                "plan_move",
            ),
            (
                ClientMessage::RunMoveStep {
                    key: key(),
                    moving,
                    step: MoveStep::Copy,
                },
                "run_move_step",
            ),
            (ClientMessage::MoveBack { key: key(), moving }, "move_back"),
            (
                ClientMessage::RetireMove { key: key(), moving },
                "retire_move",
            ),
            (
                ClientMessage::InstallAcpAgent {
                    key: key(),
                    agent: "gemini".into(),
                    version: "0.2.0".into(),
                    place: ToolPlace::Host,
                    confirm_unchecked: false,
                },
                "install_acp_agent",
            ),
            (
                ClientMessage::AcceptLicence {
                    key: key(),
                    licence: "android-sdk-license".into(),
                    text_sha256: "ab12".into(),
                },
                "accept_licence",
            ),
        ] {
            assert_eq!(round_trip(&message)["type"], json!(kind));
        }
        let listed = ActionOutcome::Snapshots {
            snapshots: vec![WorkspaceSnapshot {
                name: "auto-0001".into(),
                taken_at: Timestamp(5),
                expires_at: None,
            }],
        };
        let value = serde_json::to_value(&listed).unwrap();
        assert_eq!(value["status"], json!("snapshots"));
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(value).unwrap(),
            listed
        );
        // An update from before installs has no phase or bytes.
        let running: ToolState =
            serde_json::from_value(json!({ "type": "running", "message": "Updating" })).unwrap();
        assert_eq!(
            running,
            ToolState::Running {
                message: "Updating".into(),
                phase: None,
                bytes_done: None,
                bytes_total: None,
            }
        );
        // A `maxd` from before M6 reads as offering none of it.
        let old: Capabilities = serde_json::from_value(json!({
            "transfers": true, "tunnels": true, "workspaces": true
        }))
        .unwrap();
        assert!(!old.host_spec && !old.snapshots && !old.moves && !old.tool_installs);
    }

    #[test]
    fn a_restore_from_before_the_confirm_discards_nothing() {
        let old = json!({
            "type": "restore_snapshot",
            "key": IdempotencyKey::derive("restore"),
            "workspace": WorkspaceId::derive("ws-alpha"),
            "snapshot": "auto-0001",
            "as_new": false,
        });
        let ClientMessage::RestoreSnapshot {
            discard_unsaved, ..
        } = serde_json::from_value(old).unwrap()
        else {
            panic!("not a restore");
        };
        assert!(!discard_unsaved);
    }

    #[test]
    fn a_proposal_answer_from_before_forks_names_none() {
        let proposal = SkillProposalId::derive("p");
        let answer: ClientMessage = serde_json::from_value(json!({
            "type": "skill_proposal_action", "key": key(),
            "proposal": proposal, "action": "approve", "push": false
        }))
        .unwrap();
        assert!(matches!(
            answer,
            ClientMessage::SkillProposalAction {
                fork: None,
                elsewhere: None,
                seen: None,
                ..
            }
        ));
        let diff = ActionOutcome::SkillProposalDiff {
            diff: ProposalDiff {
                files: Vec::new(),
                flags: crate::skills::SkillFlags::default(),
                tree: None,
                target: None,
            },
        };
        let value = serde_json::to_value(&diff).unwrap();
        assert_eq!(value["status"], json!("skill_proposal_diff"));
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(value).unwrap(),
            diff
        );
    }

    #[test]
    fn cli_login_messages_round_trip() {
        let sign_in = crate::ids::SignInId::derive("cli login");
        let account = AccountId::derive("claude");
        let messages = [
            (
                "begin_cli_login",
                ClientMessage::BeginCliLogin {
                    key: key(),
                    account,
                    device_code: true,
                },
            ),
            (
                "finish_cli_login",
                ClientMessage::FinishCliLogin {
                    key: key(),
                    sign_in,
                    code: Secret::new("test-not-a-real-code".into()),
                },
            ),
            (
                "check_cli_login",
                ClientMessage::CheckCliLogin {
                    key: key(),
                    sign_in,
                },
            ),
            (
                "cancel_cli_login",
                ClientMessage::CancelCliLogin {
                    key: key(),
                    sign_in,
                },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
            assert_eq!(message.since(), CLI_LOGIN_SINCE);
            assert_eq!(message.action_key(), Some(key()));
            assert!(!message.reaches(Some(CLI_LOGIN_SINCE - 1)), "{tag}");
        }
        assert_eq!(CLI_LOGIN_SINCE, 35);
        assert_eq!(CURSOR_LOGIN_SINCE, 36);

        // A client that leaves the flag out asks for the browser sign-in.
        let begin: ClientMessage = serde_json::from_value(json!({
            "type": "begin_cli_login",
            "key": key(),
            "account": account,
        }))
        .unwrap();
        assert!(matches!(
            begin,
            ClientMessage::BeginCliLogin {
                device_code: false,
                ..
            }
        ));

        // The code stays out of debug output.
        let finish = ClientMessage::FinishCliLogin {
            key: key(),
            sign_in,
            code: Secret::new("test-not-a-real-code".into()),
        };
        let printed = format!("{finish:?}");
        assert!(!printed.contains("test-not-a-real-code"), "{printed}");
        assert_eq!(round_trip(&finish)["code"], "test-not-a-real-code");

        let outcomes = [
            (
                "cli_login",
                ActionOutcome::CliLogin {
                    sign_in,
                    url: "https://example.com/oauth/authorize?code=true&state=test-state".into(),
                    user_code: None,
                    paste: true,
                },
            ),
            (
                "cli_login",
                ActionOutcome::CliLogin {
                    sign_in,
                    url: "https://example.com/codex/device".into(),
                    user_code: Some("ABCD-1234".into()),
                    paste: false,
                },
            ),
            (
                "cli_login_state",
                ActionOutcome::CliLoginState {
                    state: CliLoginState::Waiting,
                },
            ),
        ];
        for (status, outcome) in outcomes {
            let message = ServerMessage::ActionResult {
                key: key(),
                outcome,
            };
            assert_eq!(round_trip(&message)["outcome"]["status"], status);
        }
        let states = [
            ("waiting", CliLoginState::Waiting),
            ("signed_in", CliLoginState::SignedIn),
            (
                "failed",
                CliLoginState::Failed {
                    message: "Login failed.".into(),
                },
            ),
        ];
        for (tag, state) in states {
            let value = round_trip(&ActionOutcome::CliLoginState { state });
            assert_eq!(value["state"]["state"], tag);
        }
    }

    #[test]
    fn pr_messages_round_trip() {
        let key: IdempotencyKey = "0199a7c2-0000-7000-8000-0000000000e2".parse().unwrap();
        let begin: ClientMessage =
            serde_json::from_value(json!({ "type": "begin_github_sign_in", "key": key })).unwrap();
        assert_eq!(
            begin,
            ClientMessage::BeginGithubSignIn {
                key,
                client_id: None
            }
        );
        let merge = ClientMessage::PullAction {
            key,
            repo: "owner/repo".into(),
            number: 12,
            action: PullAction::Merge {
                method: crate::pulls::MergeMethod::Squash,
            },
            expect_sha: "abc".into(),
        };
        let value = round_trip(&merge);
        assert_eq!(value["type"], json!("pull_action"));
        assert_eq!(
            value["action"],
            json!({ "action": "merge", "method": "squash" })
        );
        let code = ActionOutcome::DeviceCode {
            sign_in: crate::ids::SignInId::derive("sign in"),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_at: crate::time::Timestamp(1_761_350_400_000),
        };
        assert_eq!(round_trip(&code)["status"], json!("device_code"));
        let checks = ActionOutcome::PullChecks {
            state: ChecksState::Pending,
            checks: Vec::new(),
        };
        assert_eq!(round_trip(&checks)["state"], json!("pending"));
    }

    #[test]
    fn fleet_requests_round_trip() {
        let key: IdempotencyKey = "0199a7c2-0000-7000-8000-0000000000e1".parse().unwrap();
        let read = ClientMessage::ReadDiagnostics {
            key,
            window_secs: 3600,
            bucket_secs: 60,
        };
        let value = round_trip(&read);
        assert_eq!(value["type"], json!("read_diagnostics"));
        assert_eq!(value["window_secs"], json!(3600));
        let update = ClientMessage::UpdateTools {
            key,
            tools: vec![ToolTarget {
                tool: crate::model::Tool::Codex,
                place: crate::state::ToolPlace::Host,
            }],
        };
        let value = round_trip(&update);
        assert_eq!(value["type"], json!("update_tools"));
        assert_eq!(value["tools"][0]["tool"], json!("codex"));
        assert_eq!(
            round_trip(&ClientMessage::ReadLoad { key })["type"],
            json!("read_load")
        );
    }

    #[test]
    fn a_hello_from_before_fleet_tools_has_no_build() {
        let older: ServerHello = serde_json::from_value(json!({
            "protocol": { "current": 5, "oldest": 5 },
            "maxd_version": "0.1.0",
            "device": "0199a7c2-0000-7000-8000-000000000007",
            "boot_id": "4f1c",
            "instance": "0199a7c2-0000-7000-8000-0000000000d7",
            "capabilities": { "transfers": true, "tunnels": true, "workspaces": true },
        }))
        .unwrap();
        assert_eq!(older.build, None);
        assert_eq!(older.target, None);
        assert_eq!(older.challenge, None);
        assert!(!older.capabilities.updates);
    }

    #[test]
    fn a_hello_from_before_github_checks_reads_as_without_them() {
        let older: Capabilities = serde_json::from_value(
            json!({ "transfers": true, "tunnels": true, "workspaces": true }),
        )
        .unwrap();
        assert_eq!(
            older,
            Capabilities {
                transfers: true,
                tunnels: true,
                workspaces: true,
                github_checks: false,
                inbox: false,
                ..Capabilities::default()
            }
        );
    }

    #[test]
    fn input_from_before_refs_reads_as_without_them() {
        let parsed: UserInput = serde_json::from_value(json!({
            "text": "hi",
            "attachments": ["0199a7c2-0000-7000-8000-000000000003"],
        }))
        .unwrap();
        assert!(parsed.refs.is_empty());
        assert_eq!(parsed.attachments.len(), 1);
    }

    #[test]
    fn refusals_say_why() {
        assert_eq!(
            serde_json::to_value(ServerMessage::ActionResult {
                key: key(),
                outcome: ActionOutcome::Refused {
                    reason: "not yet".into(),
                },
            })
            .unwrap(),
            json!({
                "type": "action_result",
                "key": "0199a7c2-0000-7000-8000-00000000000a",
                "outcome": { "status": "refused", "reason": "not yet" },
            })
        );
    }

    #[test]
    fn question_answers_are_tagged_too() {
        use crate::timeline::QuestionAnswer;

        let message = ClientMessage::Answer {
            idempotency_key: key(),
            event: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            answer: AttentionAnswer::Questions {
                answers: vec![QuestionAnswer {
                    id: "q1".into(),
                    values: vec!["protocol".into(), "daemon".into()],
                }],
            },
            refs: Vec::new(),
        };
        let value = round_trip(&message);
        assert_eq!(value["answer"]["type"], "questions");
    }

    #[test]
    fn pills_in_an_answer_or_a_request_go_only_to_a_maxd_that_reads_them() {
        let pill = InlineRef::Agent {
            agent: agent(),
            title: "Fix login".into(),
            vendor: Vendor::Claude,
            device: DeviceId::derive("mac"),
            excerpt: None,
        };
        let answer = |refs: Vec<InlineRef>| ClientMessage::Answer {
            idempotency_key: key(),
            event: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            answer: AttentionAnswer::Text {
                text: "Ask \u{FFFC}".into(),
            },
            refs,
        };
        let ask = |refs: Vec<InlineRef>| ClientMessage::AskDispatcher {
            key: key(),
            text: "Start one like \u{FFFC}".into(),
            account: AccountId::derive("claude"),
            new: false,
            refs,
        };
        for (plain, pilled) in [
            (answer(Vec::new()), answer(vec![pill.clone()])),
            (ask(Vec::new()), ask(vec![pill.clone()])),
        ] {
            assert_eq!(plain.since(), OLDEST_PROTOCOL_VERSION);
            assert!(round_trip(&plain).get("refs").is_none());
            assert_eq!(pilled.since(), ANSWER_PILLS_SINCE);
            assert!(!pilled.reaches(Some(ANSWER_PILLS_SINCE - 1)));
            assert!(pilled.reaches(Some(ANSWER_PILLS_SINCE)));
            assert_eq!(round_trip(&pilled)["refs"][0]["type"], "agent");
        }
        let old: ClientMessage = serde_json::from_value(json!({
            "type": "answer",
            "idempotency_key": key(),
            "event": "0199a7c2-0000-7000-8000-000000000002",
            "answer": { "type": "text", "text": "No" },
        }))
        .unwrap();
        assert!(matches!(old, ClientMessage::Answer { refs, .. } if refs.is_empty()));
    }

    #[test]
    fn messages_are_tagged_by_type() {
        let ping = ClientMessage::Ping { nonce: 7 };
        assert_eq!(
            serde_json::to_value(&ping).unwrap(),
            json!({ "type": "ping", "nonce": 7 })
        );
        let parsed: ClientMessage =
            serde_json::from_value(json!({ "type": "sync", "since": 12 })).unwrap();
        assert_eq!(
            parsed,
            ClientMessage::Sync {
                since: Some(Seq(12))
            }
        );
    }

    #[test]
    fn losing_answers_say_who_won() {
        let result = ServerMessage::AnswerResult {
            idempotency_key: "0199a7c2-0000-7000-8000-00000000000a".parse().unwrap(),
            outcome: AnswerOutcome::AlreadyAnswered {
                answered_at: crate::time::Timestamp(1_761_350_400_000),
                answered_by: "0199a7c2-0000-7000-8000-00000000000b".parse().unwrap(),
            },
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "type": "answer_result",
                "idempotency_key": "0199a7c2-0000-7000-8000-00000000000a",
                "outcome": {
                    "status": "already_answered",
                    "answered_at": 1_761_350_400_000u64,
                    "answered_by": "0199a7c2-0000-7000-8000-00000000000b",
                },
            })
        );
    }
}
