// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the app tracks: projects, the workspaces they're checked out in, the
//! agents working there, and the devices and accounts behind them.
//!
//! ```text
//! Project      one repository, wherever it's checked out
//!   Workspace  a folder on a device, or a container on a device
//!     Agent    a CLI session in one workspace
//!       Agent  a sub-agent, whose parent is the agent above
//! Account      a vendor login on a device
//! Device       a machine that runs maxd
//! ```
//!
//! Container workspaces add what it takes to run them: their state, the
//! ports their agents listen on, the forwards that reach those ports, each
//! project's settings on a device, and the logins and keys a device holds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::acp::AcpFeatures;
use crate::agents::AgentOrigin;
use crate::ids::{
    AccountId, AgentId, AttentionId, BootId, ClientId, CredentialId, DeviceId, ForwardId, PortId,
    ProjectId, SessionId, WorkspaceId,
};
use crate::leases::TestHostSettings;
use crate::netaccess::NetworkAccessSettings;
use crate::pulls::{PullKey, PullLink};
use crate::time::Timestamp;

/// The company behind an agent CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    /// Anthropic's Claude Code.
    Claude,
    /// OpenAI's Codex CLI.
    Codex,
    /// Cursor's CLI, driven over the Agent Client Protocol.
    Cursor,
    /// Any other agent that speaks the Agent Client Protocol, run as a
    /// local command. Its account says which one.
    Acp,
}

impl Vendor {
    /// What the UI and messages call the vendor's agents.
    pub fn name(self) -> &'static str {
        match self {
            Vendor::Claude => "Claude",
            Vendor::Codex => "Codex",
            Vendor::Cursor => "Cursor",
            Vendor::Acp => "ACP",
        }
    }
}

/// A machine that runs `maxd`, as `maxd` describes itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Device {
    /// Assigned when `maxd` is installed, and kept across restarts.
    pub id: DeviceId,
    /// The host name, unless you renamed the device.
    pub name: String,
    /// The operating system, as Rust's `std::env::consts::OS` names it.
    pub os: String,
    /// Changes on every restart.
    pub boot_id: BootId,
    /// The `maxd` version running there.
    pub maxd_version: String,
    /// Set while the device winds down for a restart, per A-11. A client
    /// that loses the device before this keeps retrying every 2 seconds.
    pub winding_down_until: Option<Timestamp>,
    /// What Incus offers for container workspaces. Unset on a device
    /// without Incus.
    pub workspace_host: Option<WorkspaceHost>,
    /// The first 16 hex characters of the sha256 of the running `maxd`. A
    /// `maxd` from before builds leaves this out.
    #[serde(default)]
    pub build: Option<String>,
    /// The Rust target `maxd` was built for, such as
    /// `x86_64-unknown-linux-musl`. A `maxd` from before builds leaves this
    /// out.
    #[serde(default)]
    pub target: Option<String>,
}

/// The networks and storage pools a device's Incus offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct WorkspaceHost {
    /// Network names, such as "agentbr0".
    pub networks: Vec<String>,
    /// Storage pool names, such as "agents".
    pub pools: Vec<String>,
}

/// A repository, wherever it's checked out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Project {
    /// Stable across renames and new checkouts.
    pub id: ProjectId,
    /// Shown in the sidebar.
    pub name: String,
    /// The clone URL of the main remote, if there is one.
    pub repo: Option<String>,
    /// Vendor for new agents started without one.
    pub default_vendor: Option<Vendor>,
    /// Account for new agents started without one.
    pub default_account: Option<AccountId>,
    /// Workspace for new agents started without one.
    pub default_workspace: Option<WorkspaceId>,
}

/// A checkout of a project on one device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Workspace {
    /// Stable for the life of the checkout.
    pub id: WorkspaceId,
    /// The project this is a checkout of. Unset until you assign one.
    pub project: Option<ProjectId>,
    /// The device that holds the checkout.
    pub device: DeviceId,
    /// Where the checkout lives on that device.
    pub location: WorkspaceLocation,
    /// Set only for workspaces MaxHQ made and runs.
    #[serde(default)]
    pub managed: Option<ManagedWorkspace>,
    /// How a clone into this folder is going, for a folder MaxHQ clones.
    /// None once the clone is done. A `maxd` from before it leaves this
    /// out.
    #[serde(default)]
    pub clone: Option<CloneProgress>,
}

/// How a clone into a folder workspace is going.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct CloneProgress {
    /// Where the clone is.
    pub state: CloneState,
}

/// Where a clone into a folder workspace is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CloneState {
    /// git is cloning. Agents can't start in the folder yet.
    Cloning {
        /// What git is doing.
        stage: CloneStage,
        /// How far along that stage is, 0 to 100, when git says.
        percent: Option<u8>,
        /// Whether git has said nothing for 20 s.
        stalled: bool,
    },
    /// The clone failed. Retry it or remove it.
    Failed {
        /// What git said, with credentials taken out of URLs.
        reason: String,
    },
    /// You stopped the clone, and its partial folder is gone.
    Cancelled,
}

/// What `git clone` is doing, as its progress lines say.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloneStage {
    /// Reaching the remote, before git reports anything.
    Connecting,
    /// The remote is counting and compressing objects.
    Counting,
    /// Receiving objects.
    Receiving,
    /// Resolving deltas.
    Resolving,
    /// Checking out files.
    Checkout,
}

/// A container workspace that MaxHQ made, and how it's doing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ManagedWorkspace {
    /// What the container is doing.
    pub state: WorkspaceState,
    /// Whether it starts when its device does, per A-11.
    pub start_at_boot: bool,
    /// Whether `maxd guest` inside it answers.
    pub guest: GuestState,
    /// The last check that the container can't reach what W-7 blocks.
    pub egress: EgressCheck,
    /// The recipe version the container last finished.
    pub recipe: u32,
    /// When it was created.
    pub created_at: Timestamp,
    /// The public half of its guest's SSH host key, as an OpenSSH
    /// `ssh-ed25519` line, once the guest has reported it. It changes
    /// only when the container is made again.
    #[serde(default)]
    pub ssh_host_key: Option<String>,
    /// The work in its checkouts that isn't safe anywhere else yet, as its
    /// guest last said. None until a guest reports it.
    #[serde(default)]
    pub unsaved: Option<UnsavedSummary>,
    /// The memory it's held to before the host reclaims it, in bytes, when
    /// the host spec sets one.
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Number>)]
    pub memory_soft: Option<u64>,
}

/// A container workspace's unsaved work, summed over its checkouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct UnsavedSummary {
    /// Files changed and not committed.
    pub changed: u32,
    /// Commits not on any remote.
    pub commits: u32,
    /// Checkouts whose branch has no upstream.
    pub no_upstream: u32,
    /// When the guest read it.
    pub read_at: Timestamp,
}

/// What a container workspace is doing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkspaceState {
    /// Being made, one step at a time.
    Creating {
        /// The step running now, for a person to read.
        step: String,
        /// Steps finished so far.
        done: u32,
        /// Steps in all.
        of: u32,
    },
    /// Starting up.
    Starting,
    /// Running, so agents can start in it.
    Running,
    /// Shutting down.
    Stopping,
    /// Stopped, with its files kept.
    Stopped,
    /// Being deleted.
    Deleting,
    /// A-11. The container's storage pool isn't available. Shown purple.
    StorageLocked,
    /// Something went wrong that needs you.
    Failed {
        /// What went wrong, for a person to read.
        reason: String,
    },
}

/// Whether `maxd guest` in a container answers its host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GuestState {
    /// The link to the guest is up.
    Connected {
        /// The guest's `maxd` version.
        version: String,
    },
    /// The host is putting its own `maxd` in the container.
    Installing,
    /// The link is down.
    Down {
        /// Why, such as "container stopped".
        reason: String,
        /// When the host noticed.
        since: Timestamp,
    },
}

/// What came of the probe that tries what a container mustn't reach.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EgressCheck {
    /// The probe hasn't run yet.
    NotRun,
    /// Nothing W-7 blocks answered.
    Passed {
        /// When the probe ran.
        at: Timestamp,
    },
    /// Something W-7 blocks answered. Agents won't start until a check passes.
    Failed {
        /// When the probe ran.
        at: Timestamp,
        /// Each target that answered, such as "192.168.1.1:443".
        reached: Vec<String>,
    },
}

/// What a new container workspace should be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct WorkspaceSpec {
    /// The project to check out in it.
    pub project: ProjectId,
    /// The device whose Incus runs it.
    pub device: DeviceId,
    /// The container's name after `mhq-`. The project's name when unset.
    pub name: Option<String>,
    /// Whether it starts when its device does, per A-11.
    pub start_at_boot: bool,
}

/// One project's settings on one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ProjectSettings {
    /// The project these settings are for.
    pub project: ProjectId,
    /// The device that keeps them.
    pub device: DeviceId,
    /// A-11. On unless you turn it off.
    pub resume_on_restart: bool,
    /// N-6. Ports never offered, on top of the fixed ignore list.
    pub ignored_ports: Vec<u16>,
    /// The commit author for agents in this project's containers.
    pub git_author: Option<GitAuthor>,
    /// U-8. Days an agent in this project sits idle before auto-settle
    /// settles it, from 1 to 90, or 0 for never. Unset uses the device's.
    #[serde(default)]
    pub auto_settle_days: Option<u16>,
    /// Whether an agent settles when its linked PR merges. On unless you
    /// turn it off; a `maxd` from before PRs leaves it out.
    #[serde(default = "yes")]
    pub settle_on_merge: bool,
    /// Whether Claude agents MaxHQ starts here can get the variables a
    /// workflow's descriptor names, once you allow them. On unless you turn
    /// it off; a `maxd` from before workflows leaves it out.
    #[serde(default = "yes")]
    pub workflow_env: bool,
    /// W-8. Each of this project's containers' soft memory limit, in
    /// bytes. Unset uses the host spec's.
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Number>)]
    pub memory_soft: Option<u64>,
    /// The workflow variables you allowed agents here to get, kept from a
    /// card's Remember box. Only `maxd` sets it, and it ignores what a
    /// client sends; turning `workflow_env` off forgets it. An older peer
    /// leaving it out reads as none allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub workflow_env_allowed: Option<AllowedWorkflowEnv>,
}

/// The workflow variables you allowed for a project's agents on a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AllowedWorkflowEnv {
    /// Each variable, by name. A change to any asks again.
    pub vars: Vec<AllowedWorkflowVar>,
    /// When you allowed them.
    pub at: Timestamp,
}

/// One workflow variable you allowed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AllowedWorkflowVar {
    /// Its name.
    pub name: String,
    /// The SHA-256 of its value, in hex, so the value itself stays in the
    /// workflow's files.
    pub digest: String,
}

/// The projects you hid from the sidebar, as one device's `maxd` keeps
/// them. Clients hide a project on the Mac's `maxd` and treat a project
/// any device lists as hidden, on every device it's on. It still shows
/// while one of its agents needs something.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct HiddenProjects {
    /// The device that keeps the list.
    pub device: DeviceId,
    /// The hidden projects.
    pub projects: Vec<ProjectId>,
}

/// A shell command an agent may run without asking, which you saved by
/// answering "Always allow". The `maxd` that runs the agent answers the
/// agent's own prompt for it in every mode but Plan.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
pub struct CommandRule {
    /// The command's words, quoted the way a shell reads them back.
    pub command: String,
    /// True when only this exact command passes, as with one that
    /// deletes, moves or reaches the network. False when it's a command
    /// and its subcommand, such as `git show`, and any arguments pass.
    pub exact: bool,
}

/// The commands agents may run without asking in one project, as the
/// device that runs those agents keeps them. A device publishes no list
/// for a project with no rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct CommandRules {
    /// The project.
    pub project: ProjectId,
    /// The device that keeps the rules.
    pub device: DeviceId,
    /// The rules, in the order they were saved.
    pub rules: Vec<CommandRule>,
}

/// The name and email that commits carry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GitAuthor {
    /// Git's `user.name`.
    pub name: String,
    /// Git's `user.email`.
    pub email: String,
}

/// A port a process in a workspace listens on, per N-6.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ListeningPort {
    /// Stable while the port stays open.
    pub id: PortId,
    /// The workspace the process runs in.
    pub workspace: WorkspaceId,
    /// The agent whose process tree holds the socket.
    pub agent: Option<AgentId>,
    /// The port number.
    pub port: u16,
    /// The address the socket is bound to.
    pub bind: BindAddress,
    /// The process that holds the socket, when it could be found.
    pub process: Option<PortProcess>,
    /// A URL the agent printed for the port, such as a dev server's.
    pub url_hint: Option<String>,
    /// When the port was first seen open.
    pub first_seen: Timestamp,
    /// When the port was first seen gone. It stays listed for a while after.
    pub gone_since: Option<Timestamp>,
}

/// The address a listening socket is bound to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BindAddress {
    /// 127.0.0.1 or ::1, so only the machine itself reaches it.
    Loopback,
    /// 0.0.0.0 or ::, so every interface reaches it.
    Any,
    /// One other address.
    Address {
        /// The address, such as "10.0.3.15".
        ip: String,
    },
}

/// The process that holds a listening socket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PortProcess {
    /// Its process ID, as its container sees it for a container workspace.
    pub pid: u32,
    /// Its short name, such as "node".
    pub name: String,
    /// Its command line.
    pub command: String,
}

/// A client's local port that carries connections to a [`ListeningPort`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Forward {
    /// Stable while the forward is open.
    pub id: ForwardId,
    /// The port it reaches.
    pub port: PortId,
    /// The client that opened it and listens on `local_port`.
    pub client: ClientId,
    /// The port the client listens on. It differs from the port it reaches
    /// when that one was taken on the client.
    pub local_port: u16,
    /// When it opened.
    pub opened_at: Timestamp,
}

/// A login or key a device holds. Never carries the secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Credential {
    /// Stable while the device keeps it.
    pub id: CredentialId,
    /// The device that holds it.
    pub device: DeviceId,
    /// What it is.
    pub kind: CredentialKind,
    /// When the device stored it.
    pub stored_at: Timestamp,
    /// The last thing that went wrong using it, such as a refused token.
    pub problem: Option<String>,
}

/// What a [`Credential`] is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialKind {
    /// W-5. A setup-token, and the account it gives this device.
    ClaudeToken {
        /// The Mac account the token was minted for.
        origin: AccountId,
        /// The token account on this device.
        account: AccountId,
        /// Your name for it.
        label: String,
    },
    /// W-5. This device's own Sign in with ChatGPT session.
    ChatGpt {
        /// The sign-in account on this device.
        account: AccountId,
        /// The email OpenAI gave for the session.
        email: Option<String>,
    },
    /// A Cursor user API key, and the account it gives this device.
    CursorKey {
        /// The key account on this device.
        account: AccountId,
        /// Your name for it.
        label: String,
    },
    /// W-6. The App this device mints installation tokens from.
    GithubApp {
        /// GitHub's ID for the App.
        #[specta(type = specta_typescript::Number)]
        app_id: u64,
        /// The App's URL name, once GitHub has said it.
        slug: Option<String>,
        /// Hex SHA-256 of the private key, so you can tell keys apart.
        key_sha256: String,
        /// The repositories the App's installations reach, as "owner/name".
        repos: Vec<String>,
    },
    /// Your own GitHub sign-in, which merges and approves PRs as you. One
    /// device holds it, and the token never leaves that device.
    GithubUser {
        /// Your GitHub login.
        login: String,
    },
    /// The APNs key this device signs its pushes to phones with.
    Apns {
        /// Apple's ID for the key, from the `.p8` file's name.
        key_id: String,
        /// The Apple developer team the key belongs to.
        team_id: String,
    },
}

/// Where a workspace lives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceLocation {
    /// A folder on the device itself.
    Folder {
        /// Absolute path on the device.
        path: String,
    },
    /// A folder inside a container on the device.
    Container {
        /// The Incus instance name.
        instance: String,
        /// Absolute path inside the container.
        path: String,
    },
}

/// An agent CLI session, or a sub-agent inside one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Agent {
    /// Stable across restarts and resumes.
    pub id: AgentId,
    /// The workspace the agent runs in.
    pub workspace: WorkspaceId,
    /// The git branch the agent works on, when `maxd` knows it. In a
    /// container each agent has a worktree on its own branch.
    pub branch: Option<String>,
    /// Set for a sub-agent, and names the agent that started it.
    pub parent: Option<AgentId>,
    /// Which CLI this is.
    pub vendor: Vendor,
    /// The login it runs under. Sub-agents use their parent's.
    pub account: Option<AccountId>,
    /// The model it runs, as the CLI names it. Unset for observed agents.
    pub model: Option<String>,
    /// The permission mode it runs in. Unset for observed agents.
    pub permission: Option<AgentPermission>,
    /// The terminal session holding the CLI. Sub-agents have none.
    pub session: Option<SessionId>,
    /// The CLI's own session ID, which a resume passes back to it. Claude
    /// calls it the session ID and Codex the thread ID.
    pub vendor_session_id: Option<String>,
    /// Who holds the CLI's terminal.
    pub control: Control,
    /// The CLI's process ID on its device, or inside its container. Unset
    /// for sub-agents and once the process is gone.
    pub pid: Option<u32>,
    /// Whether `maxd` can reach the agent itself. Decision N-2 shows a
    /// device that answers with an agent that doesn't as purple.
    pub link: AgentLink,
    /// A short name, from the CLI or from the first prompt.
    pub title: String,
    /// What the agent is doing.
    pub activity: Activity,
    /// How full its context window is, when the CLI reports it.
    pub context: Option<ContextUsage>,
    /// When the agent first started.
    pub started_at: Timestamp,
    /// When the agent last produced output or changed activity.
    pub last_activity_at: Timestamp,
    /// Where the agent sits in the sidebar: settled, snoozed or pinned. A
    /// `maxd` from before inboxes leaves this out, and reads as active.
    #[serde(default)]
    pub inbox: Inbox,
    /// Who started it through the agent tools, if anyone did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentOrigin>,
    /// The PRs linked to its work. A `maxd` from before PRs leaves this
    /// out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pulls: Vec<PullLink>,
    /// Set while `maxd` waits for an agent's turn in your own terminal to
    /// end, to stop the CLI there and continue the session here. A `maxd`
    /// from before leaves this out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub taking_over: bool,
    /// Set while `maxd` has stopped the CLI because it sat idle. The next
    /// message starts it again, and that clears it. A `maxd` from before
    /// leaves this out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asleep: bool,
    /// The effort level its next turn runs at, as the CLI names it. None
    /// when it runs at its model's [`ModelChoice::default_effort`], as an
    /// agent started before `maxd` named a level does. A `maxd` from
    /// before [`crate::message::EFFORT_SINCE`] leaves this out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// What the agent started that still runs, and what it waits on. Unset
    /// when there's neither, for sub-agents, and from a `maxd` from before
    /// [`crate::message::PROCESSES_SINCE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<crate::processes::AgentRunning>,
}

/// Where a lead agent sits in the sidebar. The agent's own `maxd` keeps it,
/// so auto-settle runs with the app closed. Sub-agents keep the default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Inbox {
    /// Set while the agent is settled: out of the active list, and for an
    /// agent MaxHQ holds, with its process stopped.
    pub settled: Option<Settled>,
    /// You brought it back by hand, so auto-settle leaves it alone until you
    /// next send it a message.
    pub kept_active: bool,
    /// You turned auto-settle off for this agent.
    pub auto_settle_off: bool,
    /// Out of the active list until then, unless it needs you sooner. A
    /// client compares it with its own clock, so a snooze ends on time even
    /// before `maxd` clears it.
    pub snoozed_until: Option<Timestamp>,
    /// When the snooze began. Only what happens after it wakes the agent
    /// early.
    pub snoozed_at: Option<Timestamp>,
    /// When a snooze last ended without you, until you dismiss the marker.
    pub woke_at: Option<Timestamp>,
    /// Set while the agent is pinned.
    pub pin: Option<Pin>,
    /// Whether the agent carries on once a usage limit that cut its turn
    /// off resets. None follows its device's default for its vendor, as
    /// [`DeviceSettings::continues_at_limit`] gives it. Only Claude Code and
    /// Codex agents MaxHQ holds carry on. A `maxd` from before this leaves
    /// it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_at_limit: Option<bool>,
}

/// When and why an agent was settled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Settled {
    /// When. Auto-settle sets it to the agent's last activity.
    pub at: Timestamp,
    /// Who settled it.
    pub by: Settler,
}

/// Who settled an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Settler {
    /// You did.
    You,
    /// Auto-settle did, after the agent sat idle for the set number of days.
    Auto,
    /// Its PR merged.
    Merged(PullKey),
    /// Its PR closed without merging while the agent sat idle.
    Closed(PullKey),
}

/// A pinned agent's place among the pins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Pin {
    /// When you pinned it.
    pub at: Timestamp,
    /// Orders the pins across devices: a fractional index the client picks,
    /// compared as text. Ties sort by `at`.
    pub key: String,
}

/// A change to an agent's [`Inbox`], sent with
/// [`crate::ClientMessage::SetInbox`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum InboxChange {
    /// Settles the agent, which also unpins it and ends its snooze. An agent
    /// MaxHQ holds has its process stopped; an observed one only leaves the
    /// list. Refused while the agent needs input, works or has a message
    /// queued.
    Settle,
    /// Brings a settled agent back, and keeps auto-settle off it until you
    /// next send it a message. Its process stays stopped until then.
    Unsettle,
    /// Snoozes the agent until a time in the future.
    Snooze {
        /// When it comes back.
        until: Timestamp,
    },
    /// Ends the snooze now.
    Wake,
    /// Clears the marker a snooze that ended by itself leaves.
    DismissWoke,
    /// Pins the agent, which also unsettles it and ends its snooze.
    Pin {
        /// Its place among the pins, as [`Pin::key`] describes.
        key: String,
    },
    /// Unpins the agent.
    Unpin,
    /// Turns auto-settle on or off for this agent.
    AutoSettle {
        /// Whether auto-settle may settle it.
        on: bool,
    },
    /// Sets whether the agent carries on after a usage limit, over its
    /// device's default. A `maxd` before
    /// [`crate::message::CONTINUE_AT_LIMIT_SINCE`] refuses it.
    ContinueAtLimit {
        /// Whether it carries on.
        on: bool,
    },
    /// Puts back the inbox an undo restores. A snooze that has ended since
    /// stays ended, and restoring a settled inbox settles the agent as
    /// [`InboxChange::Settle`] does.
    Restore {
        /// The inbox to put back.
        inbox: Inbox,
    },
}

/// Who holds an agent's terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    /// You started it in your own terminal. MaxHQ shows its state but can't
    /// type into it, so you answer its prompts there.
    Observed,
    /// MaxHQ started it and holds its terminal.
    Held,
}

/// Whether `maxd` can reach an agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentLink {
    /// The agent's process runs and `maxd` can see it.
    Live,
    /// The device answers but the agent doesn't run.
    Down {
        /// Why, such as "exited", "container stopped" or "interrupted by a
        /// restart".
        reason: String,
        /// When `maxd` noticed, if it knows.
        since: Option<Timestamp>,
    },
    /// `maxd` can't tell yet, such as just after it started.
    Unknown,
}

/// What an agent is doing. Decision N-1 lists the states and how each looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Activity {
    /// Launching, or resuming after a restart.
    Starting,
    /// Working.
    Busy,
    /// Waiting on its own sub-agents, which are still working.
    BusyInSubagents,
    /// Blocked on a permission prompt or a question.
    NeedsInput {
        /// The prompt to answer.
        attention: AttentionId,
    },
    /// Finished a turn you haven't looked at yet.
    DoneUnread,
    /// Waiting for a prompt, and the vendor still has the conversation cached,
    /// so the next turn is cheaper and faster.
    IdleCacheWarm {
        /// When the cache is expected to expire.
        warm_until: Timestamp,
    },
    /// Waiting for a prompt.
    Idle,
    /// Hit a usage limit, and continues when it resets.
    WaitingForLimitReset {
        /// When the limit resets, if the vendor said.
        resets_at: Option<Timestamp>,
    },
    /// Not running until you press Resume.
    Paused {
        /// Why it paused.
        reason: PauseReason,
    },
    /// The CLI exited or crashed.
    Error {
        /// The exit status or crash message.
        reason: String,
    },
    /// The app can't reach the agent's device or workspace.
    NotAccessible,
}

/// Why an agent is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// You paused it.
    ByYou,
    /// It was resumed 3 times in a row without making progress.
    ResumesMadeNoProgress,
    /// Its device or container restarted, and its project has A-11's
    /// resume on restart turned off.
    ResumeOff,
}

/// How full an agent's context window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ContextUsage {
    /// Tokens in the conversation now.
    pub used_tokens: u32,
    /// The model's window size.
    pub window_tokens: u32,
}

/// How much an agent may do without asking you.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Asks before edits and commands.
    Ask,
    /// Edits files without asking, and asks before commands.
    AcceptEdits,
    /// Lets the vendor's reviewer approve what looks safe, and asks about
    /// the rest.
    Auto,
    /// Plans only, and changes nothing. Codex has no such mode.
    Plan,
    /// Does anything without asking.
    FullAccess,
}

/// The mode you asked for, and the one the CLI reports running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AgentPermission {
    /// The mode you asked for.
    pub requested: PermissionMode,
    /// The mode the CLI reports running, once it has. It differs from
    /// `requested` when the CLI refused or changed it.
    pub effective: Option<PermissionMode>,
}

/// One model an account can run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ModelChoice {
    /// The name the CLI takes, such as "opus".
    pub id: String,
    /// The name to show.
    pub label: String,
    /// Whether the CLI picks this one when told nothing.
    pub default: bool,
    /// The effort levels the model takes, as the CLI names them, such as
    /// "low" or "high". Empty when it takes none or the CLI doesn't say. A
    /// `maxd` from before [`crate::message::EFFORT_SINCE`] leaves this out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    /// The level the model runs at when told none: the one Codex names,
    /// or for Claude Code the one the login's settings save for it, else
    /// the CLI's own. `maxd` starts agents at a named level, so this is
    /// also where a new agent starts when nothing else names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// Set when the model can't read images, so `maxd` sends an attached
    /// image by its path only. A `maxd` from before leaves this out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_images: bool,
}

/// The effort levels MaxHQ knows, lowest first.
const EFFORT_ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

impl ModelChoice {
    /// The level this model runs at when asked for `level`: `level` when
    /// the model lists it, else the highest listed level below it, as the
    /// CLIs bring a level the model can't take down. None when nothing
    /// listed is below it, or `level` is one MaxHQ doesn't know.
    #[must_use]
    pub fn fit_effort(&self, level: &str) -> Option<String> {
        if self.efforts.iter().any(|listed| listed == level) {
            return Some(level.to_owned());
        }
        let rank = |level: &str| EFFORT_ORDER.iter().position(|known| *known == level);
        let asked = rank(level)?;
        self.efforts
            .iter()
            .filter_map(|listed| Some((rank(listed)?, listed)))
            .filter(|(at, _)| *at < asked)
            .max_by_key(|(at, _)| *at)
            .map(|(_, listed)| listed.clone())
    }
}

/// Settings `maxd` keeps for its device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct DeviceSettings {
    /// The device these settings are for.
    pub device: DeviceId,
    /// The mode for new agents started without one.
    pub default_permission: PermissionMode,
    /// The Incus network new container workspaces join, one of
    /// [`WorkspaceHost::networks`].
    pub workspace_network: Option<String>,
    /// The Incus storage pool new container workspaces live on, one of
    /// [`WorkspaceHost::pools`].
    pub workspace_pool: Option<String>,
    /// U-8. Days an agent sits idle before auto-settle settles it, from 1 to
    /// 90, or 0 for never. A `maxd` from before inboxes leaves this out.
    #[serde(default = "default_auto_settle_days")]
    pub auto_settle_days: u16,
    /// Snoozes an agent that hits a usage limit until the limit resets.
    #[serde(default)]
    pub snooze_at_limit: bool,
    /// Whether a Claude Code agent whose turn a usage limit cut off gets a
    /// message to carry on once the limit resets. It's the default for each
    /// agent, which [`Inbox::continue_at_limit`] can turn on or off. A
    /// `maxd` from before this leaves it out, which reads as off.
    #[serde(default)]
    pub continue_claude_at_limit: bool,
    /// The same for Codex agents.
    #[serde(default)]
    pub continue_codex_at_limit: bool,
    /// The effort level new Claude Code agents start at, or none for each
    /// model's [`ModelChoice::default_effort`]. A `maxd` from before
    /// [`crate::message::EFFORT_SINCE`] leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_effort: Option<String>,
    /// The same for Codex agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_effort: Option<String>,
    /// How readily new agents go to this device when you don't pick one.
    #[serde(default)]
    pub placement: Placement,
    /// The CLIs on the device itself that MaxHQ may update. Empty means you
    /// update them yourself. CLIs in container workspaces MaxHQ made update
    /// on their own.
    #[serde(default)]
    pub tool_updates: Vec<Tool>,
    /// Lets a Mac's `maxd` read the Cursor CLI's login from the Keychain
    /// and ask Cursor for the plan's usage this month. Off unless you turn
    /// it on.
    #[serde(default)]
    pub cursor_usage: bool,
    /// When a Mac's `maxd` keeps it from idling to sleep. Other systems
    /// ignore it.
    #[serde(default)]
    pub keep_awake: KeepAwake,
    /// The test hosts this device offers.
    #[serde(default)]
    pub test_hosts: TestHostSettings,
    /// Whether this device keeps the skill library. None leaves it to the
    /// default, which is that a Mac keeps it and other devices don't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_library: Option<bool>,
    /// Whether phones reach this device over the network, and on which
    /// port.
    #[serde(default)]
    pub network: NetworkAccessSettings,
    /// Minutes an agent sits idle before `maxd` stops its CLI to free the
    /// memory, or 0 for never. The agent stays as it is and its CLI starts
    /// again for the next message. A `maxd` from before this leaves it out.
    #[serde(default = "default_idle_release_minutes")]
    pub idle_release_minutes: u16,
    /// How many of each agent's newest items `maxd` keeps in memory, from
    /// [`MEMORY_ITEMS`]. The rest wait on disk until a client scrolls back.
    /// A `maxd` from before this leaves it out.
    #[serde(default = "default_memory_items")]
    pub memory_items: u16,
    /// How much of each agent's history `maxd` keeps, in MiB from
    /// [`HISTORY_MIB`], or 0 for no limit. Past it the oldest items leave
    /// the agent's timeline on this device, though the CLI's own history
    /// keeps them. A `maxd` from before this leaves it out.
    #[serde(default = "default_history_mib")]
    pub history_mib: u16,
    /// Whether this device takes a Claude setup-token: stores it and hands
    /// it to Claude Code agents when they start. It's meant for personal
    /// use and is off unless you turn it on; signing in with `claude` on
    /// the device needs no token. A `maxd` from before this leaves it out,
    /// and always takes one.
    #[serde(default)]
    pub claude_setup_token: bool,
}

impl DeviceSettings {
    /// What makes `old` into these settings, or every setting when there's
    /// no `old`. Naming each field here means a new one won't compile until
    /// it has a change.
    #[must_use]
    pub fn changes_from(&self, old: Option<&Self>) -> Vec<DeviceSettingChange> {
        use DeviceSettingChange as Change;
        let Self {
            device: _,
            default_permission,
            workspace_network,
            workspace_pool,
            auto_settle_days,
            snooze_at_limit,
            continue_claude_at_limit,
            continue_codex_at_limit,
            claude_effort,
            codex_effort,
            placement,
            tool_updates,
            cursor_usage,
            keep_awake,
            test_hosts,
            skill_library,
            network,
            idle_release_minutes,
            memory_items,
            history_mib,
            claude_setup_token,
        } = self;
        let differs = |same: &dyn Fn(&Self) -> bool| old.is_none_or(|old| !same(old));
        let mut changes = Vec::new();
        if differs(&|old| old.default_permission == *default_permission) {
            changes.push(Change::DefaultPermission {
                value: *default_permission,
            });
        }
        if differs(&|old| old.workspace_network == *workspace_network) {
            changes.push(Change::WorkspaceNetwork {
                value: workspace_network.clone(),
            });
        }
        if differs(&|old| old.workspace_pool == *workspace_pool) {
            changes.push(Change::WorkspacePool {
                value: workspace_pool.clone(),
            });
        }
        if differs(&|old| old.auto_settle_days == *auto_settle_days) {
            changes.push(Change::AutoSettleDays {
                value: *auto_settle_days,
            });
        }
        if differs(&|old| old.snooze_at_limit == *snooze_at_limit) {
            changes.push(Change::SnoozeAtLimit {
                value: *snooze_at_limit,
            });
        }
        if differs(&|old| old.continue_claude_at_limit == *continue_claude_at_limit) {
            changes.push(Change::ContinueClaudeAtLimit {
                value: *continue_claude_at_limit,
            });
        }
        if differs(&|old| old.continue_codex_at_limit == *continue_codex_at_limit) {
            changes.push(Change::ContinueCodexAtLimit {
                value: *continue_codex_at_limit,
            });
        }
        if differs(&|old| old.claude_effort == *claude_effort) {
            changes.push(Change::ClaudeEffort {
                value: claude_effort.clone(),
            });
        }
        if differs(&|old| old.codex_effort == *codex_effort) {
            changes.push(Change::CodexEffort {
                value: codex_effort.clone(),
            });
        }
        if differs(&|old| old.placement == *placement) {
            changes.push(Change::Placement { value: *placement });
        }
        if differs(&|old| old.tool_updates == *tool_updates) {
            changes.push(Change::ToolUpdates {
                value: tool_updates.clone(),
            });
        }
        if differs(&|old| old.cursor_usage == *cursor_usage) {
            changes.push(Change::CursorUsage {
                value: *cursor_usage,
            });
        }
        if differs(&|old| old.keep_awake == *keep_awake) {
            changes.push(Change::KeepAwake { value: *keep_awake });
        }
        if differs(&|old| old.test_hosts == *test_hosts) {
            changes.push(Change::TestHosts {
                value: test_hosts.clone(),
            });
        }
        if differs(&|old| old.skill_library == *skill_library) {
            changes.push(Change::SkillLibrary {
                value: *skill_library,
            });
        }
        if differs(&|old| old.network == *network) {
            changes.push(Change::Network { value: *network });
        }
        if differs(&|old| old.idle_release_minutes == *idle_release_minutes) {
            changes.push(Change::IdleReleaseMinutes {
                value: *idle_release_minutes,
            });
        }
        if differs(&|old| old.memory_items == *memory_items) {
            changes.push(Change::MemoryItems {
                value: *memory_items,
            });
        }
        if differs(&|old| old.history_mib == *history_mib) {
            changes.push(Change::HistoryMib {
                value: *history_mib,
            });
        }
        if differs(&|old| old.claude_setup_token == *claude_setup_token) {
            changes.push(Change::ClaudeSetupToken {
                value: *claude_setup_token,
            });
        }
        changes
    }

    /// Applies one change.
    pub fn change(&mut self, change: DeviceSettingChange) {
        match change {
            DeviceSettingChange::DefaultPermission { value } => self.default_permission = value,
            DeviceSettingChange::WorkspaceNetwork { value } => self.workspace_network = value,
            DeviceSettingChange::WorkspacePool { value } => self.workspace_pool = value,
            DeviceSettingChange::AutoSettleDays { value } => self.auto_settle_days = value,
            DeviceSettingChange::SnoozeAtLimit { value } => self.snooze_at_limit = value,
            DeviceSettingChange::ContinueClaudeAtLimit { value } => {
                self.continue_claude_at_limit = value;
            }
            DeviceSettingChange::ContinueCodexAtLimit { value } => {
                self.continue_codex_at_limit = value;
            }
            DeviceSettingChange::ClaudeEffort { value } => self.claude_effort = value,
            DeviceSettingChange::CodexEffort { value } => self.codex_effort = value,
            DeviceSettingChange::Placement { value } => self.placement = value,
            DeviceSettingChange::ToolUpdates { value } => self.tool_updates = value,
            DeviceSettingChange::CursorUsage { value } => self.cursor_usage = value,
            DeviceSettingChange::KeepAwake { value } => self.keep_awake = value,
            DeviceSettingChange::TestHosts { value } => self.test_hosts = value,
            DeviceSettingChange::SkillLibrary { value } => self.skill_library = value,
            DeviceSettingChange::Network { value } => self.network = value,
            DeviceSettingChange::IdleReleaseMinutes { value } => self.idle_release_minutes = value,
            DeviceSettingChange::MemoryItems { value } => self.memory_items = value,
            DeviceSettingChange::HistoryMib { value } => self.history_mib = value,
            DeviceSettingChange::ClaudeSetupToken { value } => self.claude_setup_token = value,
        }
    }

    /// The settings with each value `maxd` keeps to a range brought into
    /// it, so a value out of range is taken as the nearest one in range.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.memory_items = items_in_range(self.memory_items);
        self.history_mib = mib_in_range(self.history_mib);
        self
    }

    /// Whether an agent of `vendor` carries on after a usage limit when its
    /// own switch is left alone. Only Claude Code and Codex say when a limit
    /// resets, so other vendors never do.
    #[must_use]
    pub fn continues_at_limit(&self, vendor: Vendor) -> bool {
        match vendor {
            Vendor::Claude => self.continue_claude_at_limit,
            Vendor::Codex => self.continue_codex_at_limit,
            _ => false,
        }
    }

    /// The effort level a new agent of `vendor` starts at, or none for its
    /// model's default. Only Claude Code and Codex take one.
    #[must_use]
    pub fn default_effort(&self, vendor: Vendor) -> Option<&str> {
        match vendor {
            Vendor::Claude => self.claude_effort.as_deref(),
            Vendor::Codex => self.codex_effort.as_deref(),
            _ => None,
        }
    }

    /// How many of each agent's newest items stay in memory.
    #[must_use]
    pub fn items_in_memory(&self) -> usize {
        usize::from(items_in_range(self.memory_items))
    }

    /// The most bytes of each agent's history kept, or none for no limit.
    #[must_use]
    pub fn history_bytes(&self) -> Option<u64> {
        match mib_in_range(self.history_mib) {
            0 => None,
            mib => Some(u64::from(mib) << 20),
        }
    }
}

impl DeviceSettingChange {
    /// The first protocol version whose `maxd` reads this change.
    #[must_use]
    pub fn since(&self) -> u32 {
        match self {
            Self::ClaudeEffort { .. } | Self::CodexEffort { .. } => crate::message::EFFORT_SINCE,
            Self::ClaudeSetupToken { .. } => crate::message::SETUP_TOKENS_SINCE,
            _ => 26,
        }
    }
}

/// `items` brought into [`MEMORY_ITEMS`].
fn items_in_range(items: u16) -> u16 {
    items.clamp(MEMORY_ITEMS.0, MEMORY_ITEMS.1)
}

/// `mib` brought into [`HISTORY_MIB`], with 0 left as no limit.
fn mib_in_range(mib: u16) -> u16 {
    match mib {
        0 => 0,
        mib => mib.clamp(HISTORY_MIB.0, HISTORY_MIB.1),
    }
}

/// One setting to change in a device's [`DeviceSettings`], which leaves the
/// others as the device has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum DeviceSettingChange {
    /// See [`DeviceSettings::default_permission`].
    DefaultPermission {
        /// What it becomes.
        value: PermissionMode,
    },
    /// See [`DeviceSettings::workspace_network`].
    WorkspaceNetwork {
        /// What it becomes.
        value: Option<String>,
    },
    /// See [`DeviceSettings::workspace_pool`].
    WorkspacePool {
        /// What it becomes.
        value: Option<String>,
    },
    /// See [`DeviceSettings::auto_settle_days`].
    AutoSettleDays {
        /// What it becomes.
        value: u16,
    },
    /// See [`DeviceSettings::snooze_at_limit`].
    SnoozeAtLimit {
        /// What it becomes.
        value: bool,
    },
    /// See [`DeviceSettings::continue_claude_at_limit`].
    ContinueClaudeAtLimit {
        /// What it becomes.
        value: bool,
    },
    /// See [`DeviceSettings::continue_codex_at_limit`].
    ContinueCodexAtLimit {
        /// What it becomes.
        value: bool,
    },
    /// See [`DeviceSettings::claude_effort`].
    ClaudeEffort {
        /// What it becomes.
        value: Option<String>,
    },
    /// See [`DeviceSettings::codex_effort`].
    CodexEffort {
        /// What it becomes.
        value: Option<String>,
    },
    /// See [`DeviceSettings::placement`].
    Placement {
        /// What it becomes.
        value: Placement,
    },
    /// See [`DeviceSettings::tool_updates`].
    ToolUpdates {
        /// What it becomes.
        value: Vec<Tool>,
    },
    /// See [`DeviceSettings::cursor_usage`].
    CursorUsage {
        /// What it becomes.
        value: bool,
    },
    /// See [`DeviceSettings::keep_awake`].
    KeepAwake {
        /// What it becomes.
        value: KeepAwake,
    },
    /// See [`DeviceSettings::test_hosts`].
    TestHosts {
        /// What it becomes.
        value: TestHostSettings,
    },
    /// See [`DeviceSettings::skill_library`].
    SkillLibrary {
        /// What it becomes.
        value: Option<bool>,
    },
    /// See [`DeviceSettings::network`].
    Network {
        /// What it becomes.
        value: NetworkAccessSettings,
    },
    /// See [`DeviceSettings::idle_release_minutes`].
    IdleReleaseMinutes {
        /// What it becomes.
        value: u16,
    },
    /// See [`DeviceSettings::memory_items`].
    MemoryItems {
        /// What it becomes.
        value: u16,
    },
    /// See [`DeviceSettings::history_mib`].
    HistoryMib {
        /// What it becomes.
        value: u16,
    },
    /// See [`DeviceSettings::claude_setup_token`].
    ClaudeSetupToken {
        /// What it becomes.
        value: bool,
    },
}

/// How readily new agents go to a device when you don't pick one.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Picked first when it has room.
    Prefer,
    /// Picked by free CPU and memory.
    #[default]
    Normal,
}

/// When a Mac's `maxd` keeps it from idling to sleep.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum KeepAwake {
    /// On external power, or while an agent here is working.
    #[default]
    Charger,
    /// Whenever `maxd` runs.
    Always,
    /// While an agent here is working.
    Working,
    /// Never.
    Off,
}

/// An agent CLI that MaxHQ can update.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    /// Claude Code.
    Claude,
    /// The Codex CLI.
    Codex,
    /// The Cursor CLI.
    Cursor,
}

/// U-8's default: agents settle after 5 idle days.
pub const DEFAULT_AUTO_SETTLE_DAYS: u16 = 5;

/// An idle agent's CLI stops after an hour unless you pick otherwise.
pub const DEFAULT_IDLE_RELEASE_MINUTES: u16 = 60;

/// An agent keeps its newest 256 items in memory unless you pick otherwise.
pub const DEFAULT_MEMORY_ITEMS: u16 = 256;

/// The fewest and most items [`DeviceSettings::memory_items`] keeps.
pub const MEMORY_ITEMS: (u16, u16) = (64, 4096);

/// An agent keeps 256 MiB of history unless you pick otherwise.
pub const DEFAULT_HISTORY_MIB: u16 = 256;

/// The least and most MiB [`DeviceSettings::history_mib`] keeps, besides 0
/// for no limit.
pub const HISTORY_MIB: (u16, u16) = (32, 4096);

/// True, for a setting that's on unless you turn it off.
fn yes() -> bool {
    true
}

fn default_auto_settle_days() -> u16 {
    DEFAULT_AUTO_SETTLE_DAYS
}

fn default_idle_release_minutes() -> u16 {
    DEFAULT_IDLE_RELEASE_MINUTES
}

fn default_memory_items() -> u16 {
    DEFAULT_MEMORY_ITEMS
}

fn default_history_mib() -> u16 {
    DEFAULT_HISTORY_MIB
}

/// A vendor login on one device, such as a `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Account {
    /// Stable across logins and plan changes.
    pub id: AccountId,
    /// The device whose config folder holds the login.
    pub device: DeviceId,
    /// Which vendor the login is for.
    pub vendor: Vendor,
    /// Your name for it, such as "Personal".
    pub label: String,
    /// How the login works.
    #[serde(default)]
    pub login: LoginKind,
    /// The Mac account a token or sign-in account stands for, so the app
    /// can show them as one.
    pub origin: Option<AccountId>,
    /// The `CLAUDE_CONFIG_DIR` or `CODEX_HOME` that holds the login, as an
    /// absolute path on the device. For a token or sign-in account it names
    /// the folder inside containers. Empty for a local ACP command.
    pub config_dir: String,
    /// The login's email, as the vendor's CLI reports it.
    pub email: Option<String>,
    /// The organization the login belongs to, when the vendor reports one.
    pub organization: Option<String>,
    /// The plan as the vendor's CLI reports it, such as "max" or "plus".
    /// The app shows what it's told and never guesses.
    pub plan: Option<String>,
    /// The models the login can run. Empty until a held CLI for it has
    /// reported them.
    pub models: Vec<ModelChoice>,
    /// The last good usage check. Unset until one succeeds, so the app never
    /// shows limits it hasn't read.
    pub usage: Option<Usage>,
    /// The last check that failed, when it's newer than the last good one.
    pub usage_failure: Option<UsageFailure>,
    /// What a local ACP command said it can do, the last time it started.
    /// Unset for every other account.
    #[serde(default)]
    pub acp: Option<AcpFeatures>,
}

/// How an account logs in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginKind {
    /// The CLI's own login in its config folder, as in M2.
    #[default]
    ConfigDir,
    /// W-5. A Claude setup-token the device keeps and hands to each launch.
    SetupToken,
    /// W-5. The device's own Sign in with ChatGPT session, which `maxd`
    /// brokers to Codex.
    ChatGptSignIn,
    /// A Cursor user API key the device keeps and hands to each launch.
    CursorApiKey,
    /// A local ACP command, which signs in through its own CLI.
    AcpCommand,
}

/// An account's rate limits as the vendor last reported them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Usage {
    /// When the vendor reported these numbers, which can be earlier than
    /// when `maxd` read them from a cache.
    pub checked_at: Timestamp,
    /// Where the numbers came from.
    pub source: UsageSource,
    /// Every limit, the account's own first, shortest window first.
    pub windows: Vec<UsageWindow>,
}

/// Where a usage reading came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The figures Claude Code keeps in `.claude.json` after its own check.
    ClaudeCache,
    /// A `get_usage` control request to a short-lived `claude` process.
    ClaudeProbe,
    /// `account/rateLimits/read` from `codex app-server`.
    CodexAppServer,
    /// The rate limits a Codex session wrote to its rollout file.
    CodexRollout,
    /// Cursor's dashboard, asked for the current billing cycle.
    CursorDashboard,
}

/// One usage limit, such as a 5-hour or a weekly window, or a month of a
/// plan's billing cycle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct UsageWindow {
    /// Unset for the account's own limits. Set for a limit that covers part
    /// of it, such as one model's weekly limit or a Codex limit other than
    /// `codex`. A billing cycle's parts are `auto`, `api` and `on_demand`.
    pub scope: Option<String>,
    /// The window's length. 300 is the 5-hour window and 10080 the weekly
    /// one. A billing cycle has no fixed length and sends 0.
    pub window_minutes: u32,
    /// Whether the window rolls or follows the billing cycle. A `maxd` from
    /// before billing cycles leaves this out.
    #[serde(default)]
    pub period: UsagePeriod,
    /// How much of the limit is used, from 0 to 100.
    pub used_percent: f64,
    /// When the window resets, if the vendor said.
    pub resets_at: Option<Timestamp>,
    /// What a limit set in money has spent, such as Cursor's on-demand
    /// spend. `used_percent` is the same figure as a share of the limit.
    #[serde(default)]
    pub spend: Option<UsageSpend>,
}

/// How a usage window moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsagePeriod {
    /// It covers the last `window_minutes`, as Claude's and Codex's do.
    #[default]
    Rolling,
    /// It covers the plan's billing cycle and resets when the cycle ends.
    BillingCycle,
}

/// Money spent against a limit, in US cents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct UsageSpend {
    /// What's been spent.
    pub used_cents: u32,
    /// The most it may spend.
    pub limit_cents: u32,
}

/// A usage check that didn't work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Type, JsonSchema)]
pub struct UsageFailure {
    /// When the check ran.
    pub at: Timestamp,
    /// What went wrong, such as "logged out" or "claude not found".
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn activity_is_tagged_by_state() {
        let paused = Activity::Paused {
            reason: PauseReason::ResumesMadeNoProgress,
        };
        assert_eq!(
            serde_json::to_value(&paused).unwrap(),
            json!({ "state": "paused", "reason": "resumes_made_no_progress" })
        );
        assert_eq!(
            serde_json::to_value(Activity::BusyInSubagents).unwrap(),
            json!({ "state": "busy_in_subagents" })
        );
    }

    #[test]
    fn effort_defaults_belong_to_claude_and_codex() {
        let mut settings: DeviceSettings = serde_json::from_value(json!({
            "device": "0199a7c2-0000-7000-8000-000000000007",
            "default_permission": "ask",
            "workspace_network": null,
            "workspace_pool": null,
        }))
        .unwrap();
        assert_eq!(
            settings.claude_effort, None,
            "a maxd from before leaves it out"
        );
        let old = settings.clone();
        settings.claude_effort = Some("max".into());
        settings.codex_effort = Some("low".into());
        assert_eq!(settings.default_effort(Vendor::Claude), Some("max"));
        assert_eq!(settings.default_effort(Vendor::Codex), Some("low"));
        assert_eq!(settings.default_effort(Vendor::Cursor), None);
        let changes = settings.changes_from(Some(&old));
        assert_eq!(
            changes,
            [
                DeviceSettingChange::ClaudeEffort {
                    value: Some("max".into())
                },
                DeviceSettingChange::CodexEffort {
                    value: Some("low".into())
                },
            ]
        );
        let mut rebuilt = old.clone();
        changes.into_iter().for_each(|one| rebuilt.change(one));
        assert_eq!(rebuilt, settings);
    }

    #[test]
    fn a_model_from_before_efforts_lists_none() {
        let model: ModelChoice =
            serde_json::from_value(json!({"id": "opus", "label": "Opus", "default": true}))
                .unwrap();
        assert!(model.efforts.is_empty());
        assert_eq!(model.default_effort, None);
        let wire = serde_json::to_value(&model).unwrap();
        assert!(wire.get("efforts").is_none());
        assert!(wire.get("default_effort").is_none());
    }

    #[test]
    fn a_level_a_model_cant_take_comes_down_to_one_it_can() {
        let model = ModelChoice {
            id: "haiku".into(),
            label: "Haiku".into(),
            default: false,
            efforts: ["low", "medium", "high"].map(String::from).into(),
            default_effort: Some("medium".into()),
            no_images: false,
        };
        assert_eq!(model.fit_effort("medium").as_deref(), Some("medium"));
        assert_eq!(model.fit_effort("max").as_deref(), Some("high"));
        assert_eq!(model.fit_effort("xhigh").as_deref(), Some("high"));
        // Nothing listed is below it, or MaxHQ doesn't know where it goes.
        assert_eq!(model.fit_effort("minimal"), None);
        assert_eq!(model.fit_effort("turbo"), None);
        // A level MaxHQ doesn't know still fits a model that lists it.
        let odd = ModelChoice {
            efforts: vec!["turbo".into()],
            ..model.clone()
        };
        assert_eq!(odd.fit_effort("turbo").as_deref(), Some("turbo"));
        let none = ModelChoice {
            efforts: Vec::new(),
            ..model
        };
        assert_eq!(none.fit_effort("high"), None);
    }

    #[test]
    fn an_agent_from_before_asleep_reads_as_awake() {
        let old = json!({
            "id": AgentId::derive("a"),
            "workspace": WorkspaceId::derive("w"),
            "branch": null,
            "parent": null,
            "vendor": "claude",
            "account": null,
            "model": null,
            "permission": null,
            "session": null,
            "vendor_session_id": null,
            "control": "held",
            "pid": null,
            "link": { "state": "live" },
            "title": "Fix login",
            "activity": { "state": "idle" },
            "context": null,
            "started_at": 100,
            "last_activity_at": 900,
        });
        let mut agent: Agent = serde_json::from_value(old.clone()).unwrap();
        assert!(!agent.asleep);
        // Awake, it leaves the field out, as a `maxd` from before does.
        let mut sent = serde_json::to_value(&agent).unwrap();
        sent.as_object_mut().unwrap().remove("inbox");
        assert_eq!(sent, old);
        agent.asleep = true;
        assert_eq!(serde_json::to_value(&agent).unwrap()["asleep"], json!(true));
    }

    #[test]
    fn a_merge_settles_and_your_sign_in_reads() {
        let key = PullKey {
            repo: "owner/repo".into(),
            number: 12,
        };
        let merged = serde_json::to_value(Settler::Merged(key.clone())).unwrap();
        assert_eq!(
            merged,
            json!({ "merged": { "repo": "owner/repo", "number": 12 } })
        );
        assert_eq!(
            serde_json::from_value::<Settler>(merged).unwrap(),
            Settler::Merged(key)
        );
        assert_eq!(serde_json::to_value(Settler::You).unwrap(), json!("you"));
        let user = CredentialKind::GithubUser {
            login: "octocat".into(),
        };
        let value = serde_json::to_value(&user).unwrap();
        assert_eq!(value, json!({ "type": "github_user", "login": "octocat" }));
        assert_eq!(
            serde_json::from_value::<CredentialKind>(value).unwrap(),
            user
        );
    }

    #[test]
    fn an_apns_key_names_its_key_and_team() {
        let apns = CredentialKind::Apns {
            key_id: "<KEY_ID>".into(),
            team_id: "<TEAM_ID>".into(),
        };
        let value = serde_json::to_value(&apns).unwrap();
        assert_eq!(
            value,
            json!({ "type": "apns", "key_id": "<KEY_ID>", "team_id": "<TEAM_ID>" })
        );
        assert_eq!(
            serde_json::from_value::<CredentialKind>(value).unwrap(),
            apns
        );
    }

    #[test]
    fn old_project_settings_settle_on_merge() {
        let old = json!({
            "project": ProjectId::derive("p"),
            "device": DeviceId::derive("d"),
            "resume_on_restart": true,
            "ignored_ports": [],
            "git_author": null,
        });
        let settings: ProjectSettings = serde_json::from_value(old).unwrap();
        assert!(settings.settle_on_merge);
        assert!(settings.workflow_env);
        assert_eq!(settings.auto_settle_days, None);
    }

    #[test]
    fn old_device_settings_keep_a_mac_awake_on_the_charger() {
        let old = json!({
            "device": DeviceId::derive("d"),
            "default_permission": "auto",
            "workspace_network": null,
            "workspace_pool": null,
        });
        let settings: DeviceSettings = serde_json::from_value(old).unwrap();
        assert_eq!(settings.keep_awake, KeepAwake::Charger);
    }

    #[test]
    fn old_device_settings_release_idle_agents_after_an_hour() {
        let old = json!({
            "device": DeviceId::derive("d"),
            "default_permission": "auto",
            "workspace_network": null,
            "workspace_pool": null,
        });
        let settings: DeviceSettings = serde_json::from_value(old).unwrap();
        assert_eq!(settings.idle_release_minutes, DEFAULT_IDLE_RELEASE_MINUTES);
        assert_eq!(DEFAULT_IDLE_RELEASE_MINUTES, 60);
        let never = DeviceSettingChange::IdleReleaseMinutes { value: 0 };
        assert_eq!(
            serde_json::to_value(&never).unwrap(),
            json!({ "field": "idle_release_minutes", "value": 0 })
        );
        let mut changed = settings.clone();
        changed.change(never.clone());
        assert_eq!(changed.idle_release_minutes, 0);
        assert_eq!(changed.changes_from(Some(&settings)), [never]);
    }

    #[test]
    fn old_device_settings_keep_256_items_and_256_mib_per_agent() {
        let old = json!({
            "device": DeviceId::derive("d"),
            "default_permission": "auto",
            "workspace_network": null,
            "workspace_pool": null,
        });
        let settings: DeviceSettings = serde_json::from_value(old).unwrap();
        assert_eq!(settings.memory_items, 256);
        assert_eq!(settings.history_mib, 256);
        assert_eq!(settings.items_in_memory(), 256);
        assert_eq!(settings.history_bytes(), Some(256 << 20));
    }

    #[test]
    fn memory_settings_round_trip() {
        let settings: DeviceSettings = serde_json::from_value(json!({
            "device": DeviceId::derive("d"),
            "default_permission": "auto",
            "workspace_network": null,
            "workspace_pool": null,
        }))
        .unwrap();
        for (change, text) in [
            (
                DeviceSettingChange::MemoryItems { value: 4096 },
                json!({ "field": "memory_items", "value": 4096 }),
            ),
            (
                DeviceSettingChange::HistoryMib { value: 1024 },
                json!({ "field": "history_mib", "value": 1024 }),
            ),
            (
                DeviceSettingChange::HistoryMib { value: 0 },
                json!({ "field": "history_mib", "value": 0 }),
            ),
        ] {
            assert_eq!(serde_json::to_value(&change).unwrap(), text);
            assert_eq!(
                serde_json::from_value::<DeviceSettingChange>(text).unwrap(),
                change
            );
            let mut changed = settings.clone();
            changed.change(change.clone());
            assert_eq!(changed.changes_from(Some(&settings)), [change]);
            let back: DeviceSettings =
                serde_json::from_value(serde_json::to_value(&changed).unwrap()).unwrap();
            assert_eq!(back, changed);
        }
        let mut unlimited = settings;
        unlimited.change(DeviceSettingChange::HistoryMib { value: 0 });
        assert_eq!(unlimited.history_bytes(), None);
    }

    #[test]
    fn memory_settings_out_of_range_clamp() {
        let mut settings: DeviceSettings = serde_json::from_value(json!({
            "device": DeviceId::derive("d"),
            "default_permission": "auto",
            "workspace_network": null,
            "workspace_pool": null,
            "memory_items": 1,
            "history_mib": 1,
        }))
        .unwrap();
        assert_eq!(settings.items_in_memory(), 64);
        assert_eq!(settings.history_bytes(), Some(32 << 20));
        let clamped = settings.clone().clamped();
        assert_eq!((clamped.memory_items, clamped.history_mib), (64, 32));

        settings.memory_items = u16::MAX;
        settings.history_mib = u16::MAX;
        assert_eq!(settings.items_in_memory(), 4096);
        assert_eq!(settings.history_bytes(), Some(4 << 30));
        let clamped = settings.clamped();
        assert_eq!((clamped.memory_items, clamped.history_mib), (4096, 4096));
    }

    #[test]
    fn keep_awake_round_trips() {
        for (mode, text) in [
            (KeepAwake::Charger, "charger"),
            (KeepAwake::Always, "always"),
            (KeepAwake::Working, "working"),
            (KeepAwake::Off, "off"),
        ] {
            assert_eq!(serde_json::to_value(mode).unwrap(), json!(text));
            assert_eq!(
                serde_json::from_value::<KeepAwake>(json!(text)).unwrap(),
                mode
            );
        }
    }

    #[test]
    fn container_workspaces_name_their_instance() {
        let location = WorkspaceLocation::Container {
            instance: "ws-maxhq".into(),
            path: "/work/maxhq".into(),
        };
        assert_eq!(
            serde_json::to_value(&location).unwrap(),
            json!({ "type": "container", "instance": "ws-maxhq", "path": "/work/maxhq" })
        );
    }
}
