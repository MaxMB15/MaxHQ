// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The requests a window makes that answer once. Each [`Request`] gets a
//! [`Response`] of the same kind, or a [`CommandError`]. What streams, a
//! timeline or a transfer's progress, has methods of its own on [`App`].

use std::io::Read;
use std::path::Path;
#[cfg(feature = "desktop")]
use std::process::Stdio;
use std::sync::Arc;

use maxhq_protocol::acp::{AcpAgent, AcpEnvValue};
use maxhq_protocol::agents::{AccessKey, MoveTo, Permission};
use maxhq_protocol::attention::AttentionAnswer;
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::find::{AgentExcerpt, GitItemSearch, PathMatch};
use maxhq_protocol::folders::FolderList;
use maxhq_protocol::hosts::MoveStep;
use maxhq_protocol::ids::{
    AccountId, AcpAgentId, AgentId, AttentionId, CredentialId, DeviceId, ForwardId, IdempotencyKey,
    LeaseId, LinkId, MoveId, PermissionId, PortId, ProjectId, SignInId, SkillProposalId,
    SkillSourceId, TaskId, TerminalId, TestHostId, TransferId, WorkspaceId,
};
use maxhq_protocol::local_models::ModelAction;
use maxhq_protocol::message::{
    ActionOutcome, AgentPlace, AgentSpec, AttachmentSource, HostLoad, IgnoredFile, SendMode,
    TakeOverMode, ToolTarget, UserInput,
};
use maxhq_protocol::model::{
    CommandRule, DeviceSettings, InboxChange, PermissionMode, ProjectSettings, WorkspaceSpec,
};
use maxhq_protocol::notes::{NoteId, NoteVersion, NoteVersionInfo, VersionHash};
use maxhq_protocol::processes::{ProcessList, ProcessSignal};
use maxhq_protocol::pulls::{PullAction, PullListState};
use maxhq_protocol::secret::Secret;
use maxhq_protocol::skills::{
    ProposalAction, ProposalSeen, SkillLock, SkillProposal, SourceKind, SourceTrust, TreeHash,
};
use maxhq_protocol::state::{Entity, EntityKey, Snapshot, ToolKind, ToolPlace};
use maxhq_protocol::tasks::{FunnelState, HookAddress, Run, Task, TaskDelivery};
use maxhq_protocol::terminal::TerminalKind;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{FindAt, ItemId, TimelineFind};
use maxhq_protocol::workflow::{NodeDetail, RunKey};
use maxhq_protocol::ClientMessage;
use serde::{Deserialize, Serialize};
use specta::Type;
use thiserror::Error;

use crate::agents::AgentDefaults;
#[cfg(feature = "desktop")]
use crate::app_keys::AppKeyState;
use crate::devices::{Connector, OnProgress, PeerLink};
#[cfg(feature = "desktop")]
use crate::devices::{PhoneDevice, PhonePairing};
#[cfg(feature = "desktop")]
use crate::editors::Editors;
use crate::fleet::{check_can, Needs};
use crate::forwards::{self, OpenedForward};
use crate::merge::Merge;
use crate::offered::Offered;
#[cfg(feature = "desktop")]
use crate::setup::{self, SetupHost};
#[cfg(feature = "desktop")]
use crate::signin;
#[cfg(feature = "desktop")]
use crate::ssh_config::SshIncludeState;
use crate::transfers::{stage_each, stage_path};
use crate::view::Shown;
use crate::{App, Caller};

/// A request from a window that answers once.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// The app and protocol versions.
    AppInfo,
    /// The whole view. Ask once at startup, then apply each `ViewChanged`.
    ViewSnapshot,
    /// Records that you looked at an agent, which clears its done-unread
    /// state on every client.
    MarkSeen { agent: AgentId },
    /// Tells the app what this window's panes show. A prompt stays quiet
    /// for the agent whose pane has focus. A finished turn, a usage limit
    /// and a resume stay quiet for any agent a window shows.
    SetShown { shown: Shown },
    /// Checks usage now, for one account or every account.
    RefreshUsage { account: Option<AccountId> },
    /// Probes the connection to one device, or every device, now rather
    /// than at the next backoff step.
    RefreshLinks { link: Option<LinkId> },
    /// Adds a device by its SSH destination, such as `dev@devbox`, and
    /// starts connecting to it.
    AddDevice { host: String },
    /// Gives a device the name the app shows for it.
    RenameDevice { link: LinkId, name: String },
    /// Forgets a device and closes its connection. Nothing on the device
    /// changes.
    RemoveDevice { link: LinkId },
    /// Installs `maxd` on this Mac as a login service and starts it, for
    /// when the app finds no local `maxd` to connect to.
    #[cfg(feature = "desktop")]
    StartLocalMaxd,
    /// Stops this Mac's `maxd` and every agent it runs, and takes it out
    /// of the login items, then probes its link. With `delete_data` it
    /// also deletes `maxd`'s data and the app's own, then quits the app.
    #[cfg(feature = "desktop")]
    UninstallLocalMaxd { delete_data: bool },
    /// Restarts the `maxd` behind a link through its service manager,
    /// `launchctl` or `systemctl --user`, on this machine or over SSH. It
    /// asks nothing of `maxd`, so it works on one too old for the app.
    #[cfg(feature = "desktop")]
    RestartMaxd { link: LinkId },
    /// Takes back one open of an agent's timeline in this window. It never
    /// fails.
    CloseTimeline { agent: AgentId },
    /// Asks for the items before ordinal `before` in a timeline open in
    /// this window. They come as an `older` update on this window's
    /// timeline channel, and not to other windows.
    LoadOlder { agent: AgentId, before: Ordinal },
    /// Takes back one open of a workflow run in this window. It never
    /// fails.
    CloseWorkflow { run: RunKey },
    /// Asks the run's device for more about one of its nodes.
    NodeDetail { run: RunKey, node: String },
    /// Starts a new agent in a workspace or a folder.
    StartAgent { spec: AgentSpec },
    /// Sends a message to an agent, queued, as a steer or as an interrupt.
    SendInput {
        agent: AgentId,
        input: UserInput,
        mode: SendMode,
    },
    /// Takes back a queued message before the agent gets it.
    CancelQueued { agent: AgentId, item: ItemId },
    /// Stops an agent's current turn.
    Interrupt { agent: AgentId },
    /// Changes an agent's permission mode.
    SetPermission {
        agent: AgentId,
        mode: PermissionMode,
    },
    /// Changes an agent's model. With another account, such as one of
    /// another vendor, the agent moves to it.
    SetModel {
        agent: AgentId,
        model: String,
        #[serde(default)]
        account: Option<AccountId>,
    },
    /// Changes the effort level a Claude Code or Codex agent runs at, from
    /// its next message. None goes back to the vendor's default.
    SetEffort {
        agent: AgentId,
        #[serde(default)]
        effort: Option<String>,
    },
    /// Ends an agent's process. Its conversation stays.
    StopAgent { agent: AgentId },
    /// Deletes an agent MaxHQ holds, with its sub-agents, stopping it
    /// first. Its folder and its CLI's transcript stay.
    DeleteAgent { agent: AgentId },
    /// Names an agent. A blank title clears the name.
    RenameAgent { agent: AgentId, title: String },
    /// Changes where agents sit in the sidebar, one result for each.
    SetInbox {
        agents: Vec<AgentId>,
        change: InboxChange,
    },
    /// Takes over an agent you started in your own terminal.
    ContinueHere { agent: AgentId },
    /// Stops the CLI of an agent you started in your own terminal, now or
    /// once its turn ends, and takes it over. `cancel` stops a wait.
    TakeOver { agent: AgentId, mode: TakeOverMode },
    /// Searches an agent's folder for the `@` menu, on the device the
    /// agent runs on. It gives at most 50 files and folders, relative to
    /// the folder, best first, and leaves out what `.gitignore` does.
    FindPaths { agent: AgentId, query: String },
    /// Looks up pull requests and issues in the GitHub repo of an agent's
    /// folder for the `#` menu, or in `repo`, "owner/name", such as one a
    /// pasted link names. A query of digits looks that number up first.
    /// When the device can't look, the search says why.
    FindGitItems {
        agent: AgentId,
        query: String,
        #[serde(default)]
        repo: Option<String>,
    },
    /// An agent's timeline as Markdown of at most `budget` characters, to
    /// attach as a file for an agent the read tool can't reach.
    ExcerptAgent { agent: AgentId, budget: u32 },
    /// How often `query` shows in an agent's timeline, which match `at`
    /// picks, and the items around it with their counts.
    FindInTimeline {
        agent: AgentId,
        query: String,
        at: FindAt,
    },
    /// The processes running in an agent's holder tree, for the Running
    /// list, oldest first. A `maxd` too old to list them answers
    /// `too_old` rather than failing.
    ListProcesses { agent: AgentId },
    /// Sends `signal` to one process an agent started, by the pid and
    /// start time the Running list gave. The device sends it only while
    /// that same process is still in the agent's tree.
    SignalProcess {
        agent: AgentId,
        pid: u32,
        #[specta(type = specta_typescript::Number)]
        start: u64,
        signal: ProcessSignal,
    },
    /// One version of a project note from a device's copy, its head when
    /// `version` is none.
    ReadNote {
        device: DeviceId,
        project: ProjectId,
        note: NoteId,
        #[serde(default)]
        version: Option<VersionHash>,
    },
    /// Saves a project note on a device. No `base` creates it. A base
    /// that's no longer the head merges, and the answer says so.
    SaveNote {
        device: DeviceId,
        project: ProjectId,
        note: NoteId,
        #[serde(default)]
        base: Option<VersionHash>,
        title: String,
        body: String,
        deleted: bool,
    },
    /// A note's versions on a device, newest first, without their bodies.
    NoteHistory {
        device: DeviceId,
        project: ProjectId,
        note: NoteId,
    },
    /// Answers an approval or a question. An answer that came too late
    /// comes back refused, with why. `refs` are the pills in what you
    /// typed, as in a message.
    Answer {
        event: AttentionId,
        answer: AttentionAnswer,
        #[serde(default)]
        refs: Vec<InlineRef>,
    },
    /// Readies dropped, pasted or picked files or folders on a device to
    /// send to an agent there, with one result for each path, in order. A
    /// remote device that takes files gets a copy, and this machine reads
    /// them where they are. Any other path fails without reaching the
    /// device.
    StageOffered {
        device: DeviceId,
        paths: Vec<String>,
    },
    /// Readies files or folders the `@` menu found in an agent's folder, by
    /// the paths the search gave, with one result for each, in order. They
    /// stage on the agent's device from the folder the search looked in,
    /// and nothing is read on this machine.
    StageFound { agent: AgentId, paths: Vec<String> },
    /// Readies pasted bytes on a device to send to an agent there. `data`
    /// is the bytes in base64, at most 8 MiB once decoded.
    StagePasted {
        device: DeviceId,
        name: String,
        mime: String,
        data: String,
    },
    /// Stops an upload or download. Its channel hears that it failed.
    CancelTransfer { transfer: TransferId },
    /// Forwards a port that listens in a workspace to the same port here,
    /// or the next free one. A port that's forwarded already gives the
    /// forward it has.
    OpenForward { port: PortId },
    /// Closes a forward this machine opened.
    CloseForward { forward: ForwardId },
    /// Opens `http://localhost` at `port` in the browser, for a port an
    /// agent on this machine listens on or one a forward listens on here.
    OpenLocalhost { port: u16 },
    /// Changes a device's settings.
    SetDeviceSettings { settings: DeviceSettings },
    /// Makes a container workspace for a project on the device the spec
    /// names. The workspace shows its progress in the view as it goes.
    CreateWorkspace { spec: WorkspaceSpec },
    /// Starts a stopped container workspace.
    StartWorkspace { workspace: WorkspaceId },
    /// Stops a container workspace, keeping its files.
    StopWorkspace { workspace: WorkspaceId },
    /// Deletes a container workspace. The device refuses while it has
    /// unpushed work, unless `discard_unpushed` is set.
    DeleteWorkspace {
        workspace: WorkspaceId,
        discard_unpushed: bool,
    },
    /// Whether an editor can reach a container workspace over SSH, and
    /// whether your `~/.ssh/config` includes MaxHQ's yet.
    #[cfg(feature = "desktop")]
    SshInclude { workspace: WorkspaceId },
    /// Adds the `Include` line for MaxHQ's SSH config at the top of your
    /// `~/.ssh/config`, or with `add` false remembers that you said no.
    /// It never changes any other line.
    #[cfg(feature = "desktop")]
    SetupSshInclude { workspace: WorkspaceId, add: bool },
    /// The files git ignores in a workspace's checkout, sorted by path, for
    /// a copy of its project to take along. Wholly ignored folders such as
    /// `node_modules` aren't there.
    ListIgnoredFiles { workspace: WorkspaceId },
    /// Stops a folder's clone and removes what it cloned so far.
    CloneCancel { workspace: WorkspaceId },
    /// Runs a failed or cancelled clone again.
    CloneRetry { workspace: WorkspaceId },
    /// Forgets a folder whose clone failed or was cancelled.
    CloneRemove { workspace: WorkspaceId },
    /// The editors on this machine a workspace can open in.
    #[cfg(feature = "desktop")]
    ListEditors,
    /// Opens a workspace in an editor, or the file at `path` in it at
    /// `line` and `column`. A relative path is in the workspace's checkout.
    /// One on another device opens over ssh.
    #[cfg(feature = "desktop")]
    OpenInEditor {
        editor: String,
        workspace: WorkspaceId,
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Changes one project's settings on the device they name.
    SetProjectSettings { settings: ProjectSettings },
    /// Hides a project from the sidebar on every device, or shows it
    /// again.
    SetProjectHidden { project: ProjectId, hidden: bool },
    /// Takes one "Always allow" command rule off a project on the device
    /// that keeps it.
    RemoveCommandRule {
        device: DeviceId,
        project: ProjectId,
        rule: CommandRule,
    },
    /// This Mac's global git `user.name` and `user.email`, the author a
    /// project's commits default to.
    #[cfg(feature = "desktop")]
    GitIdentity,
    /// Winds a device's agents down before it restarts, waiting at most
    /// `window_secs` for them.
    WindDown { device: DeviceId, window_secs: u32 },
    /// Sends a Claude setup-token to each device, one result for each, in
    /// order. The app keeps no copy once it's sent, and never logs it.
    StoreClaudeToken {
        devices: Vec<DeviceId>,
        origin: AccountId,
        label: String,
        token: Secret,
    },
    /// Sends a Cursor API key to each device, one result for each, in
    /// order. The app keeps no copy once it's sent, and never logs it.
    StoreCursorKey {
        devices: Vec<DeviceId>,
        label: String,
        token: Secret,
    },
    /// Runs a whole Sign in with ChatGPT round for a device, browser and
    /// callback included. It gives up after 10 minutes.
    #[cfg(feature = "desktop")]
    SignInChatgpt { device: DeviceId },
    /// Sends a GitHub App to each device, one result for each, in order.
    /// The app reads the private key at `key_path`, a file you picked or
    /// dropped, so a window never holds it. It keeps no copy once it's
    /// sent.
    SetGithubApp {
        devices: Vec<DeviceId>,
        app_id: GithubAppId,
        key_path: String,
    },
    /// Has a device check its GitHub App now, instead of at its next check.
    CheckGithubApp { device: DeviceId },
    /// Sends the APNs key that pushes to phones to each device, one result
    /// for each, in order. The app reads the `.p8` at `key_path`, a file
    /// you picked or dropped, so a window never holds it, and keeps no copy
    /// once it's sent.
    SetApnsKey {
        devices: Vec<DeviceId>,
        key_id: String,
        team_id: String,
        key_path: String,
    },
    /// Makes a device forget a login or key it holds.
    ForgetCredential {
        device: DeviceId,
        credential: CredentialId,
    },
    /// Saves a local ACP agent on a device, or replaces the one with its
    /// ID. `env` holds only the values that are new or changed. The app
    /// keeps no copy once they're sent.
    SetAcpAgent {
        device: DeviceId,
        agent: AcpAgent,
        env: Vec<AcpEnvValue>,
    },
    /// Removes a local ACP agent and its values from a device.
    DeleteAcpAgent { device: DeviceId, id: AcpAgentId },
    // Pull requests. Each answers with the device's outcome.
    /// A project's open or closed PRs, from a device with a checkout of it.
    ListPulls {
        project: ProjectId,
        state: PullListState,
    },
    /// The branches of a project's repository, from the device that reads
    /// its PRs.
    ListBranches { project: ProjectId },
    /// One PR, from the device named.
    GetPull {
        device: DeviceId,
        repo: String,
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// One page of a PR's changed files, from the device named. Pages
    /// count from 1.
    PullFiles {
        device: DeviceId,
        repo: String,
        #[specta(type = specta_typescript::Number)]
        number: u64,
        page: u32,
    },
    /// A PR's conversation, from the device named.
    PullComments {
        device: DeviceId,
        repo: String,
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// A PR's checks, from the device named.
    PullChecks {
        device: DeviceId,
        repo: String,
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Merges or approves a PR as you, on the device holding your GitHub
    /// sign-in, if its head is still `expect_sha`.
    PullAction {
        repo: String,
        #[specta(type = specta_typescript::Number)]
        number: u64,
        action: PullAction,
        expect_sha: String,
    },
    /// Has the device named read a repository's linked PRs again now.
    PullsDue { device: DeviceId, repo: String },
    /// Links a PR of the agent's project to the agent, on its device.
    LinkPull {
        agent: AgentId,
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Takes a PR off the agent, on its device.
    UnlinkPull {
        agent: AgentId,
        repo: String,
        #[specta(type = specta_typescript::Number)]
        number: u64,
    },
    /// Starts your GitHub sign-in on the device that will hold it. The
    /// outcome holds the code to enter on GitHub.
    BeginGithubSignIn {
        device: DeviceId,
        #[serde(default)]
        client_id: Option<String>,
    },
    /// Stops a GitHub sign-in that's waiting for its code.
    CancelGithubSignIn { device: DeviceId, sign_in: SignInId },
    /// Asks how a GitHub sign-in stands: refused with why once GitHub
    /// turned it down or its code ran out.
    CheckGithubSignIn { device: DeviceId, sign_in: SignInId },
    /// Runs the CLI's own login again for a Claude, Codex or Cursor account
    /// on the device that holds it. The outcome holds the page to sign in on.
    BeginCliLogin {
        device: DeviceId,
        account: AccountId,
        #[serde(default)]
        device_code: bool,
    },
    /// Gives a Claude login the code its sign-in page showed. The app
    /// keeps no copy once it's sent, and never logs it.
    FinishCliLogin {
        device: DeviceId,
        sign_in: SignInId,
        code: Secret,
    },
    /// Asks how a CLI login stands.
    CheckCliLogin { device: DeviceId, sign_in: SignInId },
    /// Stops a CLI login that waits for you.
    CancelCliLogin { device: DeviceId, sign_in: SignInId },
    // Fleet tools. Updating every device and setting one up stream their
    // progress, so they have commands of their own.
    /// Reads each device's CPU and memory, one result for each, in order.
    ReadLoad { devices: Vec<DeviceId> },
    /// Reads what MaxHQ's processes on a device use, over the last
    /// `window_secs` in buckets `bucket_secs` wide.
    ReadDiagnostics {
        device: DeviceId,
        window_secs: u32,
        bucket_secs: u32,
    },
    /// Updates some of a device's agent CLIs.
    UpdateTools {
        device: DeviceId,
        tools: Vec<ToolTarget>,
    },
    /// Answers a setup that waits for your OK on a host key.
    #[cfg(feature = "desktop")]
    ConfirmHostKey { setup: String, ok: bool },
    /// Sets the hosts a device is reached at, in the order to try them.
    SetRoutes { link: LinkId, hosts: Vec<String> },
    /// Answers whether a link waiting on you reaches the device its hello
    /// named. The same device pins it there, so either link reaches it; a
    /// different one keeps it cut off with a note.
    AnswerLinkClaim { link: LinkId, same: bool },
    /// Asks again about a link you said is a different device.
    AskLinkClaimAgain { link: LinkId },
    /// Forgets which device a link reaches and what you said about it, so
    /// its next hello counts as its first.
    ForgetLinkIdentity { link: LinkId },
    /// What the question about a link waiting on you shows.
    #[cfg(feature = "desktop")]
    LinkClaimDetails { link: LinkId },
    /// The hosts setup offers: those in your SSH config, then tailnet
    /// peers.
    #[cfg(feature = "desktop")]
    ListSetupHosts,
    // Agents that run agents.
    /// Lists one device's permissions, or those of every connected device
    /// that holds them.
    ListPermissions { device: Option<DeviceId> },
    /// Writes a permission to its home and each device it names.
    GrantPermission { permission: Permission },
    /// Takes a permission off every connected device.
    RevokePermission { permission: PermissionId },
    /// Makes an access key on a device, which holds `permission`.
    MintKey {
        device: DeviceId,
        name: String,
        permission: Permission,
    },
    /// Lists one device's access keys, or those of every connected device
    /// that makes them.
    ListKeys { device: Option<DeviceId> },
    /// Lists the other devices a device links to, with where each stands.
    ListDeviceLinks { device: DeviceId },
    /// Turns the link between two devices off, or allows it again.
    SetDeviceLink {
        device: DeviceId,
        peer: DeviceId,
        on: bool,
    },
    // Phones.
    /// Lists the devices a phone can pair with, with whether it can reach
    /// each.
    #[cfg(feature = "desktop")]
    ListPhoneDevices,
    /// Starts pairing a phone through `through`, for it to reach each of
    /// `devices`, and gives the QR code to scan.
    #[cfg(feature = "desktop")]
    BeginPhonePair {
        through: DeviceId,
        devices: Vec<DeviceId>,
    },
    /// Takes a phone off every device that has it.
    #[cfg(feature = "desktop")]
    RemovePhone { phone: maxhq_protocol::ids::PhoneId },
    /// Lets a phone do everything but let other keys in, or stops it, on
    /// each connected device that has it.
    #[cfg(feature = "desktop")]
    SetPhoneEverything {
        phone: maxhq_protocol::ids::PhoneId,
        everything: bool,
    },
    /// Has each connected device that has a phone with its network access
    /// on, but no sign-in from it, make a new one-time code for its book.
    #[cfg(feature = "desktop")]
    RenewPhoneCodes { phone: maxhq_protocol::ids::PhoneId },
    /// Sets the address a phone dials a device on, or clears it with none.
    #[cfg(feature = "desktop")]
    SetPhoneAddress {
        device: DeviceId,
        address: Option<String>,
    },
    /// Ends a phone's network access session on the device that holds it.
    #[cfg(feature = "desktop")]
    RevokePhoneSession {
        session: maxhq_protocol::ids::PhoneSessionId,
    },
    // This Mac's app key.
    /// This Mac's app key and how the devices stand with it.
    #[cfg(feature = "desktop")]
    AppKey,
    /// Has a device stop trusting an app key. This Mac's own stays.
    #[cfg(feature = "desktop")]
    RemoveAppKey {
        device: DeviceId,
        public_key: String,
    },
    /// Makes a new app key, which each device moves to as the app next
    /// reaches it.
    #[cfg(feature = "desktop")]
    RotateAppKey,
    /// Answers a card that asks whether to let a program in, or trust its
    /// key, on the device that raised it.
    #[cfg(feature = "desktop")]
    AnswerAdmission {
        admission: maxhq_protocol::ids::AdmissionId,
        allow: bool,
    },
    /// What a new permission starts with.
    AgentDefaults,
    /// Saves what a new permission starts with.
    SetAgentDefaults { defaults: AgentDefaults },
    // Scheduled tasks.
    /// Lists one device's tasks, or those of every device the view has.
    ListTasks { device: Option<DeviceId> },
    /// Stores a task on `device`, its home, replacing the one with its ID.
    /// A save that makes a webhook answers with its URL this once.
    SaveTask { device: DeviceId, task: Box<Task> },
    /// Turns a task `device` holds on or off, changing nothing else about
    /// it.
    SetTaskEnabled {
        device: DeviceId,
        task: TaskId,
        enabled: bool,
    },
    /// Removes a task. A run in progress goes on.
    DeleteTask { task: TaskId },
    /// Runs a task now, whatever its trigger.
    RunTaskNow { task: TaskId },
    /// Turns a task's webhook on or off.
    SetHookOn { task: TaskId, on: bool },
    /// Gives a task's webhook a new URL, which the answer shows this once.
    RotateHookToken { task: TaskId },
    /// Stores the secret a task's webhook checks signatures with.
    SetHookSecret { task: TaskId, secret: Secret },
    /// Picks how `device`'s webhooks are reached.
    SetHookAddress {
        device: DeviceId,
        address: HookAddress,
    },
    /// Lists a task's recent runs.
    ListRuns { task: TaskId },
    /// Lists a task's recent webhook requests or GitHub events.
    ListDeliveries { task: TaskId },
    /// Whether a device's webhooks reach the internet.
    FunnelState { device: DeviceId },
    // Test hosts. Each goes to the device named and answers with its
    // outcome.
    /// Looks again at what a device can host.
    CheckTestHosts { device: DeviceId },
    /// Builds a test host that's off or failed.
    BuildTestHost { device: DeviceId, host: TestHostId },
    /// Takes a test host back to a clean state, ending its lease.
    ResetTestHost { device: DeviceId, host: TestHostId },
    /// Ends a lease now.
    EndLease { device: DeviceId, lease: LeaseId },
    /// A screenshot of a ready lease's machine, as `lease_screen`.
    ReadLeaseScreen { device: DeviceId, lease: LeaseId },
    // Skills. Each goes to the device named, or a proposal's to the device
    // it was found on, and answers with its outcome.
    /// Adds a source to the library `device` holds. `origin` is the
    /// source's kind, named so it clears the request's own tag.
    SkillSourceAdd {
        device: DeviceId,
        name: String,
        origin: SourceKind,
        default_ref: Option<String>,
        trust: SourceTrust,
    },
    /// Fetches a source of the library `device` holds again.
    SkillSourceFetch {
        device: DeviceId,
        source: SkillSourceId,
    },
    /// Removes a source from the library `device` holds.
    SkillSourceRemove {
        device: DeviceId,
        source: SkillSourceId,
    },
    /// Sets a project's lock on the device that owns it.
    SkillsSetProject {
        device: DeviceId,
        project: ProjectId,
        lock: SkillLock,
    },
    /// Puts a project's locked skills into each of its checkouts on
    /// `device`.
    SkillsApply {
        device: DeviceId,
        project: ProjectId,
    },
    /// The skills each CLI sees in a place on `device`, as `skills`.
    SkillsScan { device: DeviceId, place: AgentPlace },
    /// The trees of a lock `device`'s store lacks, as `skill_trees`.
    SkillsTreesNeeded { device: DeviceId, lock: SkillLock },
    /// Approves or rejects a skill proposal, pushing its source after an
    /// approve when `push` is set. A third party's skill is approved as a
    /// fork into the personal source, under the name in `fork`.
    SkillProposalAction {
        proposal: SkillProposalId,
        action: ProposalAction,
        push: bool,
        #[serde(default)]
        fork: Option<String>,
        /// What the diff you read showed, which an approve must still match.
        #[serde(default)]
        seen: Option<ProposalSeen>,
    },
    /// What a skill proposal changes, as `skill_proposal_diff`.
    SkillProposalDiff { proposal: SkillProposalId },
    /// What tidying `device`'s global skills would do, as
    /// `skills_globals_plan`.
    SkillsGlobalsPlan { device: DeviceId },
    /// Tidies `device`'s global skills as the plan with this hash said,
    /// keeping the copies at `picks` of skills whose copies differ.
    SkillsGlobalsApply {
        device: DeviceId,
        plan_hash: String,
        #[serde(default)]
        picks: Vec<String>,
    },
    /// Puts back what the last tidy on `device` moved.
    SkillsGlobalsUndo { device: DeviceId },
    /// Makes `device` the one that holds the skill library, moving it from
    /// the device that holds it now.
    MoveSkillLibrary { device: DeviceId },
    /// Pushes a source of the library `device` holds to its remote.
    SkillSourcePush {
        device: DeviceId,
        source: SkillSourceId,
    },
    /// Has `device` read its host spec and check itself against it again.
    CheckHost { device: DeviceId },
    /// Fixes the host report's `items` that `device`'s `maxd` can fix
    /// itself.
    ApplyHost {
        device: DeviceId,
        items: Vec<String>,
    },
    /// Installs a tool on `device`, at `version` or else the latest.
    InstallTool {
        device: DeviceId,
        place: ToolPlace,
        tool: ToolKind,
        #[serde(default)]
        version: Option<String>,
    },
    /// Removes a tool from `device`.
    RemoveTool {
        device: DeviceId,
        place: ToolPlace,
        tool: ToolKind,
    },
    /// A container workspace's snapshots, as `snapshots`.
    ListSnapshots { workspace: WorkspaceId },
    /// Takes a container workspace back to a snapshot, or makes a new
    /// workspace from it when `as_new` is set. A roll back over unsaved
    /// work goes ahead only with `discard_unsaved`.
    RestoreSnapshot {
        workspace: WorkspaceId,
        snapshot: String,
        #[serde(default)]
        as_new: bool,
        #[serde(default)]
        discard_unsaved: bool,
    },
    /// Starts a move of `device`'s container `instance` into MaxHQ.
    PlanMove { device: DeviceId, instance: String },
    /// Runs one step of a move.
    RunMoveStep { moving: MoveId, step: MoveStep },
    /// Switches a move back to the old container while it's kept.
    MoveBack { moving: MoveId },
    /// Deletes a moved workspace's old container now.
    RetireMove { moving: MoveId },
    /// Installs an agent from `device`'s ACP registry and adds it as an ACP
    /// agent. A binary the registry gives no checksum for needs
    /// `confirm_unchecked`.
    InstallAcpAgent {
        device: DeviceId,
        agent: String,
        version: String,
        place: ToolPlace,
        #[serde(default)]
        confirm_unchecked: bool,
    },
    /// Accepts a licence an install on `device` waits on, as read.
    AcceptLicence {
        device: DeviceId,
        licence: String,
        text_sha256: String,
    },
    // The dispatcher. Each goes to the device it runs on.
    /// Sends the dispatcher a request, under `account`, in a new
    /// conversation when `new` is set. `refs` are the pills in it.
    AskDispatcher {
        device: DeviceId,
        text: String,
        account: AccountId,
        new: bool,
        #[serde(default)]
        refs: Vec<InlineRef>,
    },
    /// Runs the plan the dispatcher shows, as `plan_ran`.
    ConfirmPlan { device: DeviceId, plan: u32 },
    /// Drops the plan the dispatcher shows.
    DismissPlan { device: DeviceId, plan: u32 },
    /// Moves an agent on `from` to another device, through the `maxd` on
    /// `device`, the one the app runs with. `stop` stops a turn that runs
    /// or a prompt that waits first.
    MoveAgent {
        device: DeviceId,
        agent: AgentId,
        from: DeviceId,
        to: MoveTo,
        #[serde(default)]
        stop: bool,
    },
    // Terminals. Each goes to the device of the agent it runs for, and
    // only a device whose `maxd` holds terminals takes one.
    /// Opens a terminal for `agent`, `cols` cells wide and `rows` high,
    /// that `runs` a shell in its drawer or a command typed after `!`.
    /// Answers with the new terminal, as `terminal_opened`. The field
    /// isn't `kind`, which names the request.
    TerminalOpen {
        agent: AgentId,
        runs: TerminalKind,
        cols: u16,
        rows: u16,
    },
    /// Asks the device to send this app a terminal's output, from just
    /// past byte `after` or from all it still holds. The device answers on
    /// the link, so this answers once the request went out.
    TerminalAttach {
        terminal: TerminalId,
        #[specta(type = Option<specta_typescript::Number>)]
        after: Option<u64>,
    },
    /// Asks the device to stop sending this app a terminal's output.
    TerminalDetach { terminal: TerminalId },
    /// Sets a terminal's size. The client you last typed in owns it.
    TerminalResize {
        terminal: TerminalId,
        cols: u16,
        rows: u16,
    },
    /// Sends a terminal Ctrl-C.
    TerminalInterrupt { terminal: TerminalId },
    /// Closes a terminal: hangs up what runs in it, or kills it with
    /// `force`, then forgets it.
    TerminalClose {
        terminal: TerminalId,
        #[serde(default)]
        force: bool,
    },
    /// Renames a terminal's tab. Empty goes back to the name the shell
    /// sets.
    TerminalRename { terminal: TerminalId, title: String },
    /// Moves a command you ran with `!` to the agent's terminal drawer.
    TerminalMove { terminal: TerminalId },
    /// What a folder on `device` holds, for a path field's suggestions and
    /// the folder browser. A `~` at the start of `path` is that device's
    /// home. With `filter`, only the names that hold it list, ignoring
    /// case. A `maxd` too old to list folders answers `too_old` rather
    /// than failing.
    ListFolder {
        device: DeviceId,
        path: String,
        files: bool,
        hidden: bool,
        #[serde(default)]
        filter: Option<String>,
    },
    /// Makes one folder on `device`, in a folder that's there, for the
    /// folder browser's New folder.
    MakeFolder { device: DeviceId, path: String },
    // Sessions. Each goes to the device of the agent it's for, and a
    // `maxd` before protocol 48 refuses it.
    /// Runs a command typed after `!` in the agent's session, or with
    /// `fresh` (⌥↩) in a shell of its own. Answers with where it went:
    /// `session_ran` for a block in the session's card, or
    /// `terminal_opened` for a fresh shell, whose `UserRun` card says why.
    SessionRun {
        agent: AgentId,
        command: String,
        private: bool,
        fresh: bool,
        cols: u16,
        rows: u16,
    },
    /// Keeps a block of the agent's open session card from the agent, or
    /// shares it again. Refused once the card is sealed.
    SessionShare {
        agent: AgentId,
        item: ItemId,
        block: u32,
        private: bool,
    },
    /// Moves the agent's session to a folder on its device, for the folder
    /// chip: opens the session there (`terminal_opened`), or runs `cd` in
    /// it as a shared block (`session_ran`). A busy session refuses.
    SessionCd { agent: AgentId, path: String },
    /// Ends the agent's session. Without `force`, one with a command
    /// running refuses, so you're asked first.
    SessionEnd {
        agent: AgentId,
        #[serde(default)]
        force: bool,
    },
    /// Acts on `device`'s local models: adds, tests or removes a server
    /// URL, downloads, pauses, resumes or deletes a model file, or loads or
    /// unloads a model. The default model is a device setting, saved with
    /// `set_device_settings`.
    ModelAction {
        device: DeviceId,
        action: ModelAction,
    },
}

/// The answer to a [`Request`], of the same kind. A request that only
/// does something answers with its kind alone.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Response {
    AppInfo(AppInfo),
    ViewSnapshot(Snapshot),
    MarkSeen,
    SetShown,
    RefreshUsage,
    RefreshLinks,
    AddDevice(LinkId),
    RenameDevice,
    RemoveDevice,
    #[cfg(feature = "desktop")]
    StartLocalMaxd,
    #[cfg(feature = "desktop")]
    UninstallLocalMaxd,
    #[cfg(feature = "desktop")]
    RestartMaxd,
    CloseTimeline,
    LoadOlder,
    CloseWorkflow,
    NodeDetail(NodeDetail),
    StartAgent(ActionOutcome),
    SendInput(ActionOutcome),
    CancelQueued(ActionOutcome),
    Interrupt(ActionOutcome),
    SetPermission(ActionOutcome),
    SetModel(ActionOutcome),
    SetEffort(ActionOutcome),
    StopAgent(ActionOutcome),
    DeleteAgent(ActionOutcome),
    RenameAgent(ActionOutcome),
    SetInbox(Vec<ActionReply>),
    ContinueHere(ActionOutcome),
    TakeOver(ActionOutcome),
    FindPaths(Vec<PathMatch>),
    FindGitItems(GitItemSearch),
    ExcerptAgent(AgentExcerpt),
    FindInTimeline(TimelineFind),
    ListProcesses(ProcessList),
    SignalProcess(ActionOutcome),
    ReadNote(NoteVersion),
    SaveNote(SavedNote),
    NoteHistory(Vec<NoteVersionInfo>),
    Answer(ActionOutcome),
    StageOffered(Vec<ActionReply>),
    StageFound(Vec<ActionReply>),
    StagePasted(ActionOutcome),
    CancelTransfer,
    OpenForward(OpenedForward),
    CloseForward,
    OpenLocalhost,
    SetDeviceSettings(ActionOutcome),
    CreateWorkspace(ActionOutcome),
    StartWorkspace(ActionOutcome),
    StopWorkspace(ActionOutcome),
    DeleteWorkspace(ActionOutcome),
    #[cfg(feature = "desktop")]
    SshInclude {
        state: SshIncludeState,
    },
    #[cfg(feature = "desktop")]
    SetupSshInclude {
        state: SshIncludeState,
    },
    ListIgnoredFiles(IgnoredFiles),
    CloneCancel(ActionOutcome),
    CloneRetry(ActionOutcome),
    CloneRemove(ActionOutcome),
    #[cfg(feature = "desktop")]
    ListEditors(Editors),
    #[cfg(feature = "desktop")]
    OpenInEditor,
    SetProjectSettings(ActionOutcome),
    SetProjectHidden(ActionOutcome),
    RemoveCommandRule(ActionOutcome),
    #[cfg(feature = "desktop")]
    GitIdentity(GitIdentity),
    WindDown(ActionOutcome),
    StoreClaudeToken(Vec<ActionReply>),
    StoreCursorKey(Vec<ActionReply>),
    #[cfg(feature = "desktop")]
    SignInChatgpt(ActionOutcome),
    SetGithubApp(Vec<ActionReply>),
    CheckGithubApp(ActionOutcome),
    SetApnsKey(Vec<ActionReply>),
    ForgetCredential(ActionOutcome),
    SetAcpAgent(ActionOutcome),
    DeleteAcpAgent(ActionOutcome),
    ListPulls(ActionOutcome),
    ListBranches(ActionOutcome),
    GetPull(ActionOutcome),
    PullFiles(ActionOutcome),
    PullComments(ActionOutcome),
    PullChecks(ActionOutcome),
    PullAction(ActionOutcome),
    PullsDue(ActionOutcome),
    LinkPull(ActionOutcome),
    UnlinkPull(ActionOutcome),
    BeginGithubSignIn(ActionOutcome),
    CancelGithubSignIn(ActionOutcome),
    CheckGithubSignIn(ActionOutcome),
    BeginCliLogin(ActionOutcome),
    FinishCliLogin(ActionOutcome),
    CheckCliLogin(ActionOutcome),
    CancelCliLogin(ActionOutcome),
    ReadLoad(Vec<DeviceLoad>),
    ReadDiagnostics(ActionOutcome),
    UpdateTools(ActionOutcome),
    #[cfg(feature = "desktop")]
    ConfirmHostKey,
    SetRoutes,
    AnswerLinkClaim,
    AskLinkClaimAgain,
    ForgetLinkIdentity,
    #[cfg(feature = "desktop")]
    LinkClaimDetails(setup::LinkClaimDetails),
    #[cfg(feature = "desktop")]
    ListSetupHosts(Vec<SetupHost>),
    ListPermissions(Vec<Permission>),
    GrantPermission(ActionOutcome),
    RevokePermission(ActionOutcome),
    MintKey(ActionOutcome),
    ListKeys(Vec<AccessKey>),
    ListDeviceLinks(Vec<PeerLink>),
    SetDeviceLink,
    #[cfg(feature = "desktop")]
    ListPhoneDevices(Vec<PhoneDevice>),
    #[cfg(feature = "desktop")]
    BeginPhonePair(PhonePairing),
    #[cfg(feature = "desktop")]
    RemovePhone,
    /// The devices that didn't take the switch, by name.
    #[cfg(feature = "desktop")]
    SetPhoneEverything(Vec<String>),
    /// The devices that didn't get a new code, by name.
    #[cfg(feature = "desktop")]
    RenewPhoneCodes(Vec<String>),
    #[cfg(feature = "desktop")]
    SetPhoneAddress,
    #[cfg(feature = "desktop")]
    RevokePhoneSession(ActionOutcome),
    #[cfg(feature = "desktop")]
    AppKey(AppKeyState),
    #[cfg(feature = "desktop")]
    RemoveAppKey(ActionOutcome),
    #[cfg(feature = "desktop")]
    RotateAppKey,
    #[cfg(feature = "desktop")]
    AnswerAdmission(ActionOutcome),
    AgentDefaults(AgentDefaults),
    SetAgentDefaults,
    ListTasks(Vec<Task>),
    SaveTask(ActionOutcome),
    SetTaskEnabled(ActionOutcome),
    DeleteTask(ActionOutcome),
    RunTaskNow(ActionOutcome),
    SetHookOn(ActionOutcome),
    RotateHookToken(ActionOutcome),
    SetHookSecret(ActionOutcome),
    SetHookAddress(ActionOutcome),
    ListRuns(Vec<Run>),
    ListDeliveries(Vec<TaskDelivery>),
    FunnelState(FunnelState),
    CheckTestHosts(ActionOutcome),
    BuildTestHost(ActionOutcome),
    ResetTestHost(ActionOutcome),
    EndLease(ActionOutcome),
    ReadLeaseScreen(ActionOutcome),
    SkillSourceAdd(ActionOutcome),
    SkillSourceFetch(ActionOutcome),
    SkillSourceRemove(ActionOutcome),
    SkillsSetProject(ActionOutcome),
    SkillsApply(ActionOutcome),
    SkillsScan(ActionOutcome),
    SkillsTreesNeeded(ActionOutcome),
    SkillProposalAction(ActionOutcome),
    SkillProposalDiff(ActionOutcome),
    SkillsGlobalsPlan(ActionOutcome),
    SkillsGlobalsApply(ActionOutcome),
    SkillsGlobalsUndo(ActionOutcome),
    MoveSkillLibrary(ActionOutcome),
    SkillSourcePush(ActionOutcome),
    CheckHost(ActionOutcome),
    ApplyHost(ActionOutcome),
    InstallTool(ActionOutcome),
    RemoveTool(ActionOutcome),
    ListSnapshots(ActionOutcome),
    RestoreSnapshot(ActionOutcome),
    PlanMove(ActionOutcome),
    RunMoveStep(ActionOutcome),
    MoveBack(ActionOutcome),
    RetireMove(ActionOutcome),
    InstallAcpAgent(ActionOutcome),
    AcceptLicence(ActionOutcome),
    AskDispatcher(ActionOutcome),
    ConfirmPlan(ActionOutcome),
    DismissPlan(ActionOutcome),
    MoveAgent(ActionOutcome),
    TerminalOpen(ActionOutcome),
    TerminalAttach,
    TerminalDetach,
    TerminalResize,
    TerminalInterrupt(ActionOutcome),
    TerminalClose(ActionOutcome),
    TerminalRename(ActionOutcome),
    TerminalMove(ActionOutcome),
    ListFolder(FolderList),
    MakeFolder(ActionOutcome),
    SessionRun(ActionOutcome),
    SessionShare(ActionOutcome),
    SessionCd(ActionOutcome),
    SessionEnd(ActionOutcome),
    ModelAction(ActionOutcome),
}

impl Request {
    /// The request's kind, as its `kind` field names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AppInfo => "app_info",
            Self::ViewSnapshot => "view_snapshot",
            Self::MarkSeen { .. } => "mark_seen",
            Self::SetShown { .. } => "set_shown",
            Self::RefreshUsage { .. } => "refresh_usage",
            Self::RefreshLinks { .. } => "refresh_links",
            Self::AddDevice { .. } => "add_device",
            Self::RenameDevice { .. } => "rename_device",
            Self::RemoveDevice { .. } => "remove_device",
            #[cfg(feature = "desktop")]
            Self::StartLocalMaxd => "start_local_maxd",
            #[cfg(feature = "desktop")]
            Self::UninstallLocalMaxd { .. } => "uninstall_local_maxd",
            #[cfg(feature = "desktop")]
            Self::RestartMaxd { .. } => "restart_maxd",
            Self::CloseTimeline { .. } => "close_timeline",
            Self::LoadOlder { .. } => "load_older",
            Self::CloseWorkflow { .. } => "close_workflow",
            Self::NodeDetail { .. } => "node_detail",
            Self::StartAgent { .. } => "start_agent",
            Self::SendInput { .. } => "send_input",
            Self::CancelQueued { .. } => "cancel_queued",
            Self::Interrupt { .. } => "interrupt",
            Self::SetPermission { .. } => "set_permission",
            Self::SetModel { .. } => "set_model",
            Self::SetEffort { .. } => "set_effort",
            Self::StopAgent { .. } => "stop_agent",
            Self::DeleteAgent { .. } => "delete_agent",
            Self::RenameAgent { .. } => "rename_agent",
            Self::SetInbox { .. } => "set_inbox",
            Self::ContinueHere { .. } => "continue_here",
            Self::TakeOver { .. } => "take_over",
            Self::FindPaths { .. } => "find_paths",
            Self::FindGitItems { .. } => "find_git_items",
            Self::ExcerptAgent { .. } => "excerpt_agent",
            Self::FindInTimeline { .. } => "find_in_timeline",
            Self::ListProcesses { .. } => "list_processes",
            Self::SignalProcess { .. } => "signal_process",
            Self::ReadNote { .. } => "read_note",
            Self::SaveNote { .. } => "save_note",
            Self::NoteHistory { .. } => "note_history",
            Self::Answer { .. } => "answer",
            Self::StageOffered { .. } => "stage_offered",
            Self::StageFound { .. } => "stage_found",
            Self::StagePasted { .. } => "stage_pasted",
            Self::CancelTransfer { .. } => "cancel_transfer",
            Self::OpenForward { .. } => "open_forward",
            Self::CloseForward { .. } => "close_forward",
            Self::OpenLocalhost { .. } => "open_localhost",
            Self::SetDeviceSettings { .. } => "set_device_settings",
            Self::CreateWorkspace { .. } => "create_workspace",
            Self::StartWorkspace { .. } => "start_workspace",
            Self::StopWorkspace { .. } => "stop_workspace",
            Self::DeleteWorkspace { .. } => "delete_workspace",
            #[cfg(feature = "desktop")]
            Self::SshInclude { .. } => "ssh_include",
            #[cfg(feature = "desktop")]
            Self::SetupSshInclude { .. } => "setup_ssh_include",
            Self::ListIgnoredFiles { .. } => "list_ignored_files",
            Self::CloneCancel { .. } => "clone_cancel",
            Self::CloneRetry { .. } => "clone_retry",
            Self::CloneRemove { .. } => "clone_remove",
            #[cfg(feature = "desktop")]
            Self::ListEditors => "list_editors",
            #[cfg(feature = "desktop")]
            Self::OpenInEditor { .. } => "open_in_editor",
            Self::SetProjectSettings { .. } => "set_project_settings",
            Self::SetProjectHidden { .. } => "set_project_hidden",
            Self::RemoveCommandRule { .. } => "remove_command_rule",
            #[cfg(feature = "desktop")]
            Self::GitIdentity => "git_identity",
            Self::WindDown { .. } => "wind_down",
            Self::StoreClaudeToken { .. } => "store_claude_token",
            Self::StoreCursorKey { .. } => "store_cursor_key",
            #[cfg(feature = "desktop")]
            Self::SignInChatgpt { .. } => "sign_in_chatgpt",
            Self::SetGithubApp { .. } => "set_github_app",
            Self::CheckGithubApp { .. } => "check_github_app",
            Self::SetApnsKey { .. } => "set_apns_key",
            Self::ForgetCredential { .. } => "forget_credential",
            Self::SetAcpAgent { .. } => "set_acp_agent",
            Self::DeleteAcpAgent { .. } => "delete_acp_agent",
            Self::ListPulls { .. } => "list_pulls",
            Self::ListBranches { .. } => "list_branches",
            Self::GetPull { .. } => "get_pull",
            Self::PullFiles { .. } => "pull_files",
            Self::PullComments { .. } => "pull_comments",
            Self::PullChecks { .. } => "pull_checks",
            Self::PullAction { .. } => "pull_action",
            Self::PullsDue { .. } => "pulls_due",
            Self::LinkPull { .. } => "link_pull",
            Self::UnlinkPull { .. } => "unlink_pull",
            Self::BeginGithubSignIn { .. } => "begin_github_sign_in",
            Self::CancelGithubSignIn { .. } => "cancel_github_sign_in",
            Self::CheckGithubSignIn { .. } => "check_github_sign_in",
            Self::BeginCliLogin { .. } => "begin_cli_login",
            Self::FinishCliLogin { .. } => "finish_cli_login",
            Self::CheckCliLogin { .. } => "check_cli_login",
            Self::CancelCliLogin { .. } => "cancel_cli_login",
            Self::ReadLoad { .. } => "read_load",
            Self::ReadDiagnostics { .. } => "read_diagnostics",
            Self::UpdateTools { .. } => "update_tools",
            #[cfg(feature = "desktop")]
            Self::ConfirmHostKey { .. } => "confirm_host_key",
            Self::SetRoutes { .. } => "set_routes",
            Self::AnswerLinkClaim { .. } => "answer_link_claim",
            Self::AskLinkClaimAgain { .. } => "ask_link_claim_again",
            Self::ForgetLinkIdentity { .. } => "forget_link_identity",
            #[cfg(feature = "desktop")]
            Self::LinkClaimDetails { .. } => "link_claim_details",
            #[cfg(feature = "desktop")]
            Self::ListSetupHosts => "list_setup_hosts",
            Self::ListPermissions { .. } => "list_permissions",
            Self::GrantPermission { .. } => "grant_permission",
            Self::RevokePermission { .. } => "revoke_permission",
            Self::MintKey { .. } => "mint_key",
            Self::ListKeys { .. } => "list_keys",
            Self::ListDeviceLinks { .. } => "list_device_links",
            Self::SetDeviceLink { .. } => "set_device_link",
            #[cfg(feature = "desktop")]
            Self::ListPhoneDevices => "list_phone_devices",
            #[cfg(feature = "desktop")]
            Self::BeginPhonePair { .. } => "begin_phone_pair",
            #[cfg(feature = "desktop")]
            Self::RemovePhone { .. } => "remove_phone",
            #[cfg(feature = "desktop")]
            Self::SetPhoneEverything { .. } => "set_phone_everything",
            #[cfg(feature = "desktop")]
            Self::RenewPhoneCodes { .. } => "renew_phone_codes",
            #[cfg(feature = "desktop")]
            Self::SetPhoneAddress { .. } => "set_phone_address",
            #[cfg(feature = "desktop")]
            Self::RevokePhoneSession { .. } => "revoke_phone_session",
            #[cfg(feature = "desktop")]
            Self::AppKey => "app_key",
            #[cfg(feature = "desktop")]
            Self::RemoveAppKey { .. } => "remove_app_key",
            #[cfg(feature = "desktop")]
            Self::RotateAppKey => "rotate_app_key",
            #[cfg(feature = "desktop")]
            Self::AnswerAdmission { .. } => "answer_admission",
            Self::AgentDefaults => "agent_defaults",
            Self::SetAgentDefaults { .. } => "set_agent_defaults",
            Self::ListTasks { .. } => "list_tasks",
            Self::SaveTask { .. } => "save_task",
            Self::SetTaskEnabled { .. } => "set_task_enabled",
            Self::DeleteTask { .. } => "delete_task",
            Self::RunTaskNow { .. } => "run_task_now",
            Self::SetHookOn { .. } => "set_hook_on",
            Self::RotateHookToken { .. } => "rotate_hook_token",
            Self::SetHookSecret { .. } => "set_hook_secret",
            Self::SetHookAddress { .. } => "set_hook_address",
            Self::ListRuns { .. } => "list_runs",
            Self::ListDeliveries { .. } => "list_deliveries",
            Self::FunnelState { .. } => "funnel_state",
            Self::CheckTestHosts { .. } => "check_test_hosts",
            Self::BuildTestHost { .. } => "build_test_host",
            Self::ResetTestHost { .. } => "reset_test_host",
            Self::EndLease { .. } => "end_lease",
            Self::ReadLeaseScreen { .. } => "read_lease_screen",
            Self::SkillSourceAdd { .. } => "skill_source_add",
            Self::SkillSourceFetch { .. } => "skill_source_fetch",
            Self::SkillSourceRemove { .. } => "skill_source_remove",
            Self::SkillsSetProject { .. } => "skills_set_project",
            Self::SkillsApply { .. } => "skills_apply",
            Self::SkillsScan { .. } => "skills_scan",
            Self::SkillsTreesNeeded { .. } => "skills_trees_needed",
            Self::SkillProposalAction { .. } => "skill_proposal_action",
            Self::SkillProposalDiff { .. } => "skill_proposal_diff",
            Self::SkillsGlobalsPlan { .. } => "skills_globals_plan",
            Self::SkillsGlobalsApply { .. } => "skills_globals_apply",
            Self::SkillsGlobalsUndo { .. } => "skills_globals_undo",
            Self::MoveSkillLibrary { .. } => "move_skill_library",
            Self::SkillSourcePush { .. } => "skill_source_push",
            Self::CheckHost { .. } => "check_host",
            Self::ApplyHost { .. } => "apply_host",
            Self::InstallTool { .. } => "install_tool",
            Self::RemoveTool { .. } => "remove_tool",
            Self::ListSnapshots { .. } => "list_snapshots",
            Self::RestoreSnapshot { .. } => "restore_snapshot",
            Self::PlanMove { .. } => "plan_move",
            Self::RunMoveStep { .. } => "run_move_step",
            Self::MoveBack { .. } => "move_back",
            Self::RetireMove { .. } => "retire_move",
            Self::InstallAcpAgent { .. } => "install_acp_agent",
            Self::AcceptLicence { .. } => "accept_licence",
            Self::AskDispatcher { .. } => "ask_dispatcher",
            Self::ConfirmPlan { .. } => "confirm_plan",
            Self::DismissPlan { .. } => "dismiss_plan",
            Self::MoveAgent { .. } => "move_agent",
            Self::TerminalOpen { .. } => "terminal_open",
            Self::TerminalAttach { .. } => "terminal_attach",
            Self::TerminalDetach { .. } => "terminal_detach",
            Self::TerminalResize { .. } => "terminal_resize",
            Self::TerminalInterrupt { .. } => "terminal_interrupt",
            Self::TerminalClose { .. } => "terminal_close",
            Self::TerminalRename { .. } => "terminal_rename",
            Self::TerminalMove { .. } => "terminal_move",
            Self::ListFolder { .. } => "list_folder",
            Self::MakeFolder { .. } => "make_folder",
            Self::SessionRun { .. } => "session_run",
            Self::SessionShare { .. } => "session_share",
            Self::SessionCd { .. } => "session_cd",
            Self::SessionEnd { .. } => "session_end",
            Self::ModelAction { .. } => "model_action",
        }
    }
}

impl Response {
    /// The response's kind, the same as its request's.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AppInfo(_) => "app_info",
            Self::ViewSnapshot(_) => "view_snapshot",
            Self::MarkSeen => "mark_seen",
            Self::SetShown => "set_shown",
            Self::RefreshUsage => "refresh_usage",
            Self::RefreshLinks => "refresh_links",
            Self::AddDevice(_) => "add_device",
            Self::RenameDevice => "rename_device",
            Self::RemoveDevice => "remove_device",
            #[cfg(feature = "desktop")]
            Self::StartLocalMaxd => "start_local_maxd",
            #[cfg(feature = "desktop")]
            Self::UninstallLocalMaxd => "uninstall_local_maxd",
            #[cfg(feature = "desktop")]
            Self::RestartMaxd => "restart_maxd",
            Self::CloseTimeline => "close_timeline",
            Self::LoadOlder => "load_older",
            Self::CloseWorkflow => "close_workflow",
            Self::NodeDetail(_) => "node_detail",
            Self::StartAgent(_) => "start_agent",
            Self::SendInput(_) => "send_input",
            Self::CancelQueued(_) => "cancel_queued",
            Self::Interrupt(_) => "interrupt",
            Self::SetPermission(_) => "set_permission",
            Self::SetModel(_) => "set_model",
            Self::SetEffort(_) => "set_effort",
            Self::StopAgent(_) => "stop_agent",
            Self::DeleteAgent(_) => "delete_agent",
            Self::RenameAgent(_) => "rename_agent",
            Self::SetInbox(_) => "set_inbox",
            Self::ContinueHere(_) => "continue_here",
            Self::TakeOver(_) => "take_over",
            Self::FindPaths(_) => "find_paths",
            Self::FindGitItems(_) => "find_git_items",
            Self::ExcerptAgent(_) => "excerpt_agent",
            Self::FindInTimeline(_) => "find_in_timeline",
            Self::ListProcesses(_) => "list_processes",
            Self::SignalProcess(_) => "signal_process",
            Self::ReadNote(_) => "read_note",
            Self::SaveNote(_) => "save_note",
            Self::NoteHistory(_) => "note_history",
            Self::Answer(_) => "answer",
            Self::StageOffered(_) => "stage_offered",
            Self::StageFound(_) => "stage_found",
            Self::StagePasted(_) => "stage_pasted",
            Self::CancelTransfer => "cancel_transfer",
            Self::OpenForward(_) => "open_forward",
            Self::CloseForward => "close_forward",
            Self::OpenLocalhost => "open_localhost",
            Self::SetDeviceSettings(_) => "set_device_settings",
            Self::CreateWorkspace(_) => "create_workspace",
            Self::StartWorkspace(_) => "start_workspace",
            Self::StopWorkspace(_) => "stop_workspace",
            Self::DeleteWorkspace(_) => "delete_workspace",
            #[cfg(feature = "desktop")]
            Self::SshInclude { .. } => "ssh_include",
            #[cfg(feature = "desktop")]
            Self::SetupSshInclude { .. } => "setup_ssh_include",
            Self::ListIgnoredFiles(_) => "list_ignored_files",
            Self::CloneCancel(_) => "clone_cancel",
            Self::CloneRetry(_) => "clone_retry",
            Self::CloneRemove(_) => "clone_remove",
            #[cfg(feature = "desktop")]
            Self::ListEditors(_) => "list_editors",
            #[cfg(feature = "desktop")]
            Self::OpenInEditor => "open_in_editor",
            Self::SetProjectSettings(_) => "set_project_settings",
            Self::SetProjectHidden(_) => "set_project_hidden",
            Self::RemoveCommandRule(_) => "remove_command_rule",
            #[cfg(feature = "desktop")]
            Self::GitIdentity(_) => "git_identity",
            Self::WindDown(_) => "wind_down",
            Self::StoreClaudeToken(_) => "store_claude_token",
            Self::StoreCursorKey(_) => "store_cursor_key",
            #[cfg(feature = "desktop")]
            Self::SignInChatgpt(_) => "sign_in_chatgpt",
            Self::SetGithubApp(_) => "set_github_app",
            Self::CheckGithubApp(_) => "check_github_app",
            Self::SetApnsKey(_) => "set_apns_key",
            Self::ForgetCredential(_) => "forget_credential",
            Self::SetAcpAgent(_) => "set_acp_agent",
            Self::DeleteAcpAgent(_) => "delete_acp_agent",
            Self::ListPulls(_) => "list_pulls",
            Self::ListBranches(_) => "list_branches",
            Self::GetPull(_) => "get_pull",
            Self::PullFiles(_) => "pull_files",
            Self::PullComments(_) => "pull_comments",
            Self::PullChecks(_) => "pull_checks",
            Self::PullAction(_) => "pull_action",
            Self::PullsDue(_) => "pulls_due",
            Self::LinkPull(_) => "link_pull",
            Self::UnlinkPull(_) => "unlink_pull",
            Self::BeginGithubSignIn(_) => "begin_github_sign_in",
            Self::CancelGithubSignIn(_) => "cancel_github_sign_in",
            Self::CheckGithubSignIn(_) => "check_github_sign_in",
            Self::BeginCliLogin(_) => "begin_cli_login",
            Self::FinishCliLogin(_) => "finish_cli_login",
            Self::CheckCliLogin(_) => "check_cli_login",
            Self::CancelCliLogin(_) => "cancel_cli_login",
            Self::ReadLoad(_) => "read_load",
            Self::ReadDiagnostics(_) => "read_diagnostics",
            Self::UpdateTools(_) => "update_tools",
            #[cfg(feature = "desktop")]
            Self::ConfirmHostKey => "confirm_host_key",
            Self::SetRoutes => "set_routes",
            Self::AnswerLinkClaim => "answer_link_claim",
            Self::AskLinkClaimAgain => "ask_link_claim_again",
            Self::ForgetLinkIdentity => "forget_link_identity",
            #[cfg(feature = "desktop")]
            Self::LinkClaimDetails(_) => "link_claim_details",
            #[cfg(feature = "desktop")]
            Self::ListSetupHosts(_) => "list_setup_hosts",
            Self::ListPermissions(_) => "list_permissions",
            Self::GrantPermission(_) => "grant_permission",
            Self::RevokePermission(_) => "revoke_permission",
            Self::MintKey(_) => "mint_key",
            Self::ListKeys(_) => "list_keys",
            Self::ListDeviceLinks(_) => "list_device_links",
            Self::SetDeviceLink => "set_device_link",
            #[cfg(feature = "desktop")]
            Self::ListPhoneDevices(_) => "list_phone_devices",
            #[cfg(feature = "desktop")]
            Self::BeginPhonePair(_) => "begin_phone_pair",
            #[cfg(feature = "desktop")]
            Self::RemovePhone => "remove_phone",
            #[cfg(feature = "desktop")]
            Self::SetPhoneEverything(_) => "set_phone_everything",
            #[cfg(feature = "desktop")]
            Self::RenewPhoneCodes(_) => "renew_phone_codes",
            #[cfg(feature = "desktop")]
            Self::SetPhoneAddress => "set_phone_address",
            #[cfg(feature = "desktop")]
            Self::RevokePhoneSession(_) => "revoke_phone_session",
            #[cfg(feature = "desktop")]
            Self::AppKey(_) => "app_key",
            #[cfg(feature = "desktop")]
            Self::RemoveAppKey(_) => "remove_app_key",
            #[cfg(feature = "desktop")]
            Self::RotateAppKey => "rotate_app_key",
            #[cfg(feature = "desktop")]
            Self::AnswerAdmission(_) => "answer_admission",
            Self::AgentDefaults(_) => "agent_defaults",
            Self::SetAgentDefaults => "set_agent_defaults",
            Self::ListTasks(_) => "list_tasks",
            Self::SaveTask(_) => "save_task",
            Self::SetTaskEnabled(_) => "set_task_enabled",
            Self::DeleteTask(_) => "delete_task",
            Self::RunTaskNow(_) => "run_task_now",
            Self::SetHookOn(_) => "set_hook_on",
            Self::RotateHookToken(_) => "rotate_hook_token",
            Self::SetHookSecret(_) => "set_hook_secret",
            Self::SetHookAddress(_) => "set_hook_address",
            Self::ListRuns(_) => "list_runs",
            Self::ListDeliveries(_) => "list_deliveries",
            Self::FunnelState(_) => "funnel_state",
            Self::CheckTestHosts(_) => "check_test_hosts",
            Self::BuildTestHost(_) => "build_test_host",
            Self::ResetTestHost(_) => "reset_test_host",
            Self::EndLease(_) => "end_lease",
            Self::ReadLeaseScreen(_) => "read_lease_screen",
            Self::SkillSourceAdd(_) => "skill_source_add",
            Self::SkillSourceFetch(_) => "skill_source_fetch",
            Self::SkillSourceRemove(_) => "skill_source_remove",
            Self::SkillsSetProject(_) => "skills_set_project",
            Self::SkillsApply(_) => "skills_apply",
            Self::SkillsScan(_) => "skills_scan",
            Self::SkillsTreesNeeded(_) => "skills_trees_needed",
            Self::SkillProposalAction(_) => "skill_proposal_action",
            Self::SkillProposalDiff(_) => "skill_proposal_diff",
            Self::SkillsGlobalsPlan(_) => "skills_globals_plan",
            Self::SkillsGlobalsApply(_) => "skills_globals_apply",
            Self::SkillsGlobalsUndo(_) => "skills_globals_undo",
            Self::MoveSkillLibrary(_) => "move_skill_library",
            Self::SkillSourcePush(_) => "skill_source_push",
            Self::CheckHost(_) => "check_host",
            Self::ApplyHost(_) => "apply_host",
            Self::InstallTool(_) => "install_tool",
            Self::RemoveTool(_) => "remove_tool",
            Self::ListSnapshots(_) => "list_snapshots",
            Self::RestoreSnapshot(_) => "restore_snapshot",
            Self::PlanMove(_) => "plan_move",
            Self::RunMoveStep(_) => "run_move_step",
            Self::MoveBack(_) => "move_back",
            Self::RetireMove(_) => "retire_move",
            Self::InstallAcpAgent(_) => "install_acp_agent",
            Self::AcceptLicence(_) => "accept_licence",
            Self::AskDispatcher(_) => "ask_dispatcher",
            Self::ConfirmPlan(_) => "confirm_plan",
            Self::DismissPlan(_) => "dismiss_plan",
            Self::MoveAgent(_) => "move_agent",
            Self::TerminalOpen(_) => "terminal_open",
            Self::TerminalAttach => "terminal_attach",
            Self::TerminalDetach => "terminal_detach",
            Self::TerminalResize => "terminal_resize",
            Self::TerminalInterrupt(_) => "terminal_interrupt",
            Self::TerminalClose(_) => "terminal_close",
            Self::TerminalRename(_) => "terminal_rename",
            Self::TerminalMove(_) => "terminal_move",
            Self::ListFolder(_) => "list_folder",
            Self::MakeFolder(_) => "make_folder",
            Self::SessionRun(_) => "session_run",
            Self::SessionShare(_) => "session_share",
            Self::SessionCd(_) => "session_cd",
            Self::SessionEnd(_) => "session_end",
            Self::ModelAction(_) => "model_action",
        }
    }
}

impl<C: Connector> App<C> {
    /// Answers a request from `caller`'s window with a response of its
    /// kind.
    pub async fn request(
        &self,
        caller: &Caller,
        request: Request,
    ) -> Result<Response, CommandError> {
        let kind = request.kind();
        let response = self.answer(caller, request).await?;
        debug_assert_eq!(response.kind(), kind, "a request answered in another kind");
        Ok(response)
    }

    async fn answer(&self, caller: &Caller, request: Request) -> Result<Response, CommandError> {
        let devices = &self.devices;
        // A request sends at most one action, and each action needs a
        // fresh key.
        let key = IdempotencyKey::generate();
        Ok(match request {
            Request::AppInfo => Response::AppInfo(app_info()),
            Request::ViewSnapshot => Response::ViewSnapshot(self.view.snapshot()),
            Request::MarkSeen { agent } => {
                devices.mark_seen(agent).await?;
                Response::MarkSeen
            }
            Request::SetShown { shown } => {
                let shown = if caller.on_screen {
                    shown
                } else {
                    Shown::default()
                };
                self.view.set_shown(&caller.window, shown);
                Response::SetShown
            }
            Request::RefreshUsage { account } => {
                devices.refresh_usage(account).await?;
                Response::RefreshUsage
            }
            Request::RefreshLinks { link } => {
                devices.refresh_links(link)?;
                Response::RefreshLinks
            }
            Request::AddDevice { host } => Response::AddDevice(devices.add_device(&host)?),
            Request::RenameDevice { link, name } => {
                devices.rename_device(link, &name)?;
                Response::RenameDevice
            }
            Request::RemoveDevice { link } => {
                devices.remove_device(link)?;
                Response::RemoveDevice
            }
            #[cfg(feature = "desktop")]
            Request::StartLocalMaxd => {
                devices.start_local_maxd().await?;
                Response::StartLocalMaxd
            }
            #[cfg(feature = "desktop")]
            Request::UninstallLocalMaxd { delete_data } => {
                devices.uninstall_local_maxd(delete_data).await?;
                if delete_data {
                    devices.erase_app_data(self.shell.app_data()).await?;
                    // The answer goes back before the app goes.
                    self.shell.quit();
                }
                Response::UninstallLocalMaxd
            }
            #[cfg(feature = "desktop")]
            Request::RestartMaxd { link } => {
                devices.restart_maxd(link).await?;
                Response::RestartMaxd
            }
            Request::CloseTimeline { agent } => {
                devices.close_timeline(&caller.window, agent).await;
                Response::CloseTimeline
            }
            Request::LoadOlder { agent, before } => {
                devices.load_older(&caller.window, agent, before.0).await?;
                Response::LoadOlder
            }
            Request::CloseWorkflow { run } => {
                devices.close_workflow(&caller.window, &run).await;
                Response::CloseWorkflow
            }
            Request::NodeDetail { run, node } => {
                Response::NodeDetail(devices.node_detail(run, node).await?)
            }
            Request::StartAgent { spec } => {
                Response::StartAgent(devices.act(ClientMessage::StartAgent { key, spec }).await?)
            }
            Request::SendInput { agent, input, mode } => {
                let action = ClientMessage::SendInput {
                    key,
                    agent,
                    input,
                    mode,
                };
                Response::SendInput(devices.act(action).await?)
            }
            Request::CancelQueued { agent, item } => {
                let action = ClientMessage::CancelQueued { key, agent, item };
                Response::CancelQueued(devices.act(action).await?)
            }
            Request::Interrupt { agent } => {
                Response::Interrupt(devices.act(ClientMessage::Interrupt { key, agent }).await?)
            }
            Request::SetPermission { agent, mode } => {
                let action = ClientMessage::SetPermission { key, agent, mode };
                Response::SetPermission(devices.act(action).await?)
            }
            Request::SetModel {
                agent,
                model,
                account,
            } => {
                let action = ClientMessage::SetModel {
                    key,
                    agent,
                    model,
                    account,
                };
                Response::SetModel(devices.act(action).await?)
            }
            Request::SetEffort { agent, effort } => {
                let action = ClientMessage::SetEffort { key, agent, effort };
                Response::SetEffort(devices.act(action).await?)
            }
            Request::StopAgent { agent } => {
                Response::StopAgent(devices.act(ClientMessage::StopAgent { key, agent }).await?)
            }
            Request::DeleteAgent { agent } => Response::DeleteAgent(
                devices
                    .act(ClientMessage::DeleteAgent { key, agent })
                    .await?,
            ),
            Request::RenameAgent { agent, title } => {
                let action = ClientMessage::RenameAgent { key, agent, title };
                Response::RenameAgent(devices.act(action).await?)
            }
            Request::SetInbox { agents, change } => {
                let outcomes = devices.set_inbox(&agents, &change).await;
                Response::SetInbox(outcomes.into_iter().map(ActionReply::from).collect())
            }
            Request::ContinueHere { agent } => {
                let action = ClientMessage::ContinueHere { key, agent };
                Response::ContinueHere(devices.act(action).await?)
            }
            Request::TakeOver { agent, mode } => {
                let action = ClientMessage::TakeOver { key, agent, mode };
                Response::TakeOver(devices.act(action).await?)
            }
            Request::FindPaths { agent, query } => {
                let action = ClientMessage::FindPaths { key, agent, query };
                match devices.act(action).await? {
                    ActionOutcome::Paths { matches } => Response::FindPaths(matches),
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::FindGitItems { agent, query, repo } => {
                let action = ClientMessage::FindGitItems {
                    key,
                    agent,
                    query,
                    repo,
                };
                match devices.act(action).await? {
                    ActionOutcome::GitItems { search } => Response::FindGitItems(search),
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::ExcerptAgent { agent, budget } => {
                let action = ClientMessage::ExcerptAgent { key, agent, budget };
                match devices.act(action).await? {
                    ActionOutcome::Excerpt { excerpt } => Response::ExcerptAgent(excerpt),
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::FindInTimeline { agent, query, at } => {
                let action = ClientMessage::FindInTimeline {
                    key,
                    agent,
                    query,
                    at,
                };
                match devices.act(action).await? {
                    ActionOutcome::TimelineFind { find } => Response::FindInTimeline(find),
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::ListProcesses { agent } => {
                let action = ClientMessage::ListProcesses { key, agent };
                match devices.act_unless_old(action).await? {
                    None => Response::ListProcesses(ProcessList::TooOld),
                    Some(ActionOutcome::Processes { list }) => Response::ListProcesses(list),
                    Some(other) => return Err(lookup_failed(other)),
                }
            }
            Request::SignalProcess {
                agent,
                pid,
                start,
                signal,
            } => {
                let action = ClientMessage::SignalProcess {
                    key,
                    agent,
                    pid,
                    start,
                    signal,
                };
                Response::SignalProcess(devices.act(action).await?)
            }
            Request::ReadNote {
                device,
                project,
                note,
                version,
            } => {
                let action = ClientMessage::ReadNote {
                    key,
                    project,
                    note,
                    version,
                };
                match devices.act_on(device, action).await? {
                    ActionOutcome::NoteRead { version } => Response::ReadNote(version),
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::SaveNote {
                device,
                project,
                note,
                base,
                title,
                body,
                deleted,
            } => {
                let action = ClientMessage::SaveNote {
                    key,
                    project,
                    note,
                    base,
                    title,
                    body,
                    deleted,
                };
                match devices.act_on(device, action).await? {
                    ActionOutcome::NoteSaved { version, merged } => {
                        Response::SaveNote(SavedNote { version, merged })
                    }
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::NoteHistory {
                device,
                project,
                note,
            } => {
                let action = ClientMessage::NoteHistory { key, project, note };
                match devices.act_on(device, action).await? {
                    ActionOutcome::NoteHistory { versions } => Response::NoteHistory(versions),
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::Answer {
                event,
                answer,
                refs,
            } => Response::Answer(devices.answer(event, answer, refs).await?),
            Request::StageOffered { device, paths } => {
                let stage = |path| stage_path(devices, device, path);
                Response::StageOffered(stage_each(&self.offered, paths, stage).await)
            }
            Request::StageFound { agent, paths } => {
                let mut replies = Vec::with_capacity(paths.len());
                for path in paths {
                    let action = ClientMessage::StageAttachment {
                        key: IdempotencyKey::generate(),
                        source: AttachmentSource::Found { agent, path },
                    };
                    replies.push(devices.act(action).await.into());
                }
                Response::StageFound(replies)
            }
            Request::StagePasted {
                device,
                name,
                mime,
                data,
            } => {
                let source = AttachmentSource::Bytes { name, mime, data };
                Response::StagePasted(devices.stage(device, source).await?)
            }
            Request::CancelTransfer { transfer } => {
                devices.cancel_transfer(transfer);
                Response::CancelTransfer
            }
            Request::OpenForward { port } => {
                let forwarded = devices.open_forward(port).await?;
                Response::OpenForward(OpenedForward::from(forwarded))
            }
            Request::CloseForward { forward } => {
                devices.close_forward(forward)?;
                Response::CloseForward
            }
            Request::OpenLocalhost { port } => {
                forwards::open_localhost(&*self.shell, port).await?;
                Response::OpenLocalhost
            }
            Request::SetDeviceSettings { settings } => {
                Response::SetDeviceSettings(devices.save_settings(settings).await?)
            }
            Request::CreateWorkspace { spec } => {
                let action = ClientMessage::CreateWorkspace { key, spec };
                Response::CreateWorkspace(devices.act(action).await?)
            }
            Request::StartWorkspace { workspace } => {
                let action = ClientMessage::StartWorkspace { key, workspace };
                Response::StartWorkspace(devices.act(action).await?)
            }
            Request::StopWorkspace { workspace } => {
                let action = ClientMessage::StopWorkspace { key, workspace };
                Response::StopWorkspace(devices.act(action).await?)
            }
            Request::DeleteWorkspace {
                workspace,
                discard_unpushed,
            } => {
                let action = ClientMessage::DeleteWorkspace {
                    key,
                    workspace,
                    discard_unpushed,
                };
                Response::DeleteWorkspace(devices.act(action).await?)
            }
            #[cfg(feature = "desktop")]
            Request::SshInclude { workspace } => Response::SshInclude {
                state: devices.ssh_include(workspace, None).await?,
            },
            #[cfg(feature = "desktop")]
            Request::SetupSshInclude { workspace, add } => Response::SetupSshInclude {
                state: devices.ssh_include(workspace, Some(add)).await?,
            },
            Request::ListIgnoredFiles { workspace } => {
                match devices.list_ignored_files(workspace).await? {
                    ActionOutcome::IgnoredFiles { files, truncated } => {
                        Response::ListIgnoredFiles(IgnoredFiles { files, truncated })
                    }
                    other => return Err(lookup_failed(other)),
                }
            }
            Request::CloneCancel { workspace } => {
                let action = ClientMessage::CloneCancel { key, workspace };
                Response::CloneCancel(devices.act(action).await?)
            }
            Request::CloneRetry { workspace } => {
                let action = ClientMessage::CloneRetry { key, workspace };
                Response::CloneRetry(devices.act(action).await?)
            }
            Request::CloneRemove { workspace } => {
                let action = ClientMessage::CloneRemove { key, workspace };
                Response::CloneRemove(devices.act(action).await?)
            }
            #[cfg(feature = "desktop")]
            Request::ListEditors => Response::ListEditors(self.list_editors().await),
            #[cfg(feature = "desktop")]
            Request::OpenInEditor {
                editor,
                workspace,
                path,
                line,
                column,
            } => {
                self.open_in_editor(&editor, workspace, path, line, column)
                    .await?;
                Response::OpenInEditor
            }
            Request::SetProjectSettings { settings } => {
                let action = ClientMessage::SetProjectSettings { key, settings };
                Response::SetProjectSettings(devices.act(action).await?)
            }
            Request::SetProjectHidden { project, hidden } => Response::SetProjectHidden(
                crate::hidden::set_hidden(devices, &self.view, project, hidden).await?,
            ),
            Request::RemoveCommandRule {
                device,
                project,
                rule,
            } => {
                let action = ClientMessage::RemoveCommandRule {
                    key,
                    device,
                    project,
                    rule,
                };
                Response::RemoveCommandRule(devices.act(action).await?)
            }
            #[cfg(feature = "desktop")]
            Request::GitIdentity => {
                let git = || tokio::process::Command::new("git");
                Response::GitIdentity(read_git_identity(git).await)
            }
            Request::WindDown {
                device,
                window_secs,
            } => {
                let action = ClientMessage::WindDown { key, window_secs };
                Response::WindDown(devices.act_on(device, action).await?)
            }
            Request::StoreClaudeToken {
                devices: to,
                origin,
                label,
                token,
            } => {
                let store = |key, token| ClientMessage::StoreClaudeToken {
                    key,
                    origin,
                    label: label.clone(),
                    token,
                };
                let missing = "Paste the setup-token first";
                Response::StoreClaudeToken(self.send_pasted(&to, token, missing, store).await)
            }
            Request::StoreCursorKey {
                devices: to,
                label,
                token,
            } => {
                let store = |key, token| ClientMessage::StoreCursorKey {
                    key,
                    label: label.clone(),
                    token,
                };
                let missing = "Paste the API key first";
                Response::StoreCursorKey(self.send_pasted(&to, token, missing, store).await)
            }
            #[cfg(feature = "desktop")]
            Request::SignInChatgpt { device } => {
                let outcome = signin::sign_in_chatgpt(devices, &*self.shell, device).await?;
                Response::SignInChatgpt(outcome)
            }
            Request::SetGithubApp {
                devices: to,
                app_id,
                key_path,
            } => Response::SetGithubApp(self.set_github_app(&to, app_id, &key_path).await),
            Request::CheckGithubApp { device } => {
                Response::CheckGithubApp(devices.check_github_app(device).await?)
            }
            Request::SetApnsKey {
                devices: to,
                key_id,
                team_id,
                key_path,
            } => Response::SetApnsKey(self.set_apns_key(&to, &key_id, &team_id, &key_path).await),
            Request::ForgetCredential { device, credential } => {
                let action = ClientMessage::ForgetCredential { key, credential };
                Response::ForgetCredential(devices.act_on(device, action).await?)
            }
            Request::SetAcpAgent { device, agent, env } => {
                let action = ClientMessage::SetAcpAgent { key, agent, env };
                Response::SetAcpAgent(devices.act_on(device, action).await?)
            }
            Request::DeleteAcpAgent { device, id } => {
                let action = ClientMessage::DeleteAcpAgent { key, id };
                Response::DeleteAcpAgent(devices.act_on(device, action).await?)
            }
            Request::ListPulls { project, state } => {
                Response::ListPulls(self.list_pulls(project, state).await?)
            }
            Request::ListBranches { project } => {
                Response::ListBranches(self.list_branches(project).await?)
            }
            Request::GetPull {
                device,
                repo,
                number,
            } => Response::GetPull(
                self.act_on_pulls(device, |key| ClientMessage::GetPull { key, repo, number })
                    .await?,
            ),
            Request::PullFiles {
                device,
                repo,
                number,
                page,
            } => Response::PullFiles(
                self.act_on_pulls(device, |key| ClientMessage::PullFiles {
                    key,
                    repo,
                    number,
                    page,
                })
                .await?,
            ),
            Request::PullComments {
                device,
                repo,
                number,
            } => Response::PullComments(
                self.act_on_pulls(device, |key| ClientMessage::PullComments {
                    key,
                    repo,
                    number,
                })
                .await?,
            ),
            Request::PullChecks {
                device,
                repo,
                number,
            } => Response::PullChecks(
                self.act_on_pulls(device, |key| ClientMessage::PullChecks {
                    key,
                    repo,
                    number,
                })
                .await?,
            ),
            Request::PullAction {
                repo,
                number,
                action,
                expect_sha,
            } => Response::PullAction(self.pull_action(repo, number, action, expect_sha).await?),
            Request::PullsDue { device, repo } => Response::PullsDue(
                self.act_on_pulls(device, |key| ClientMessage::PullsDue { key, repo })
                    .await?,
            ),
            Request::LinkPull { agent, number } => Response::LinkPull(
                self.act_on_pull_link(agent, |key| ClientMessage::LinkPull { key, agent, number })
                    .await?,
            ),
            Request::UnlinkPull {
                agent,
                repo,
                number,
            } => Response::UnlinkPull(
                self.act_on_pull_link(agent, |key| ClientMessage::UnlinkPull {
                    key,
                    agent,
                    repo,
                    number,
                })
                .await?,
            ),
            Request::BeginGithubSignIn { device, client_id } => Response::BeginGithubSignIn(
                self.act_on_github_sign_in(device, |key| ClientMessage::BeginGithubSignIn {
                    key,
                    client_id,
                })
                .await?,
            ),
            Request::CancelGithubSignIn { device, sign_in } => Response::CancelGithubSignIn(
                self.act_on_github_sign_in(device, |key| ClientMessage::CancelGithubSignIn {
                    key,
                    sign_in,
                })
                .await?,
            ),
            Request::CheckGithubSignIn { device, sign_in } => Response::CheckGithubSignIn(
                self.act_on_github_sign_in(device, |key| ClientMessage::CheckGithubSignIn {
                    key,
                    sign_in,
                })
                .await?,
            ),
            Request::BeginCliLogin {
                device,
                account,
                device_code,
            } => {
                let action = ClientMessage::BeginCliLogin {
                    key,
                    account,
                    device_code,
                };
                Response::BeginCliLogin(devices.act_on(device, action).await?)
            }
            Request::FinishCliLogin {
                device,
                sign_in,
                code,
            } => {
                // A pasted code can carry a line break that was never part
                // of it.
                let code = code.without_whitespace();
                if code.is_empty() {
                    return Err(CommandError::Failed {
                        message: "Paste the code from the page first".into(),
                    });
                }
                let action = ClientMessage::FinishCliLogin { key, sign_in, code };
                Response::FinishCliLogin(devices.act_on(device, action).await?)
            }
            Request::CheckCliLogin { device, sign_in } => {
                let action = ClientMessage::CheckCliLogin { key, sign_in };
                Response::CheckCliLogin(devices.act_on(device, action).await?)
            }
            Request::CancelCliLogin { device, sign_in } => {
                let action = ClientMessage::CancelCliLogin { key, sign_in };
                Response::CancelCliLogin(devices.act_on(device, action).await?)
            }
            Request::ReadLoad { devices: on } => {
                let mut loads = Vec::with_capacity(on.len());
                for device in on {
                    let checked = self
                        .view
                        .read(|merge| check_can(merge, device, Needs::Load));
                    let read = ClientMessage::ReadLoad {
                        key: IdempotencyKey::generate(),
                    };
                    let outcome = match checked {
                        Ok(()) => devices.act_on(device, read).await,
                        Err(error) => Err(error),
                    };
                    loads.push(DeviceLoad {
                        device,
                        load: match outcome {
                            Ok(ActionOutcome::Load(load)) => Some(load),
                            _ => None,
                        },
                        received_at: Timestamp::now(),
                    });
                }
                Response::ReadLoad(loads)
            }
            Request::ReadDiagnostics {
                device,
                window_secs,
                bucket_secs,
            } => {
                self.view
                    .read(|merge| check_can(merge, device, Needs::Load))?;
                let action = ClientMessage::ReadDiagnostics {
                    key,
                    window_secs,
                    bucket_secs,
                };
                Response::ReadDiagnostics(devices.act_on(device, action).await?)
            }
            Request::UpdateTools { device, tools } => {
                self.view
                    .read(|merge| check_can(merge, device, Needs::Tools))?;
                let action = ClientMessage::UpdateTools { key, tools };
                Response::UpdateTools(devices.act_on(device, action).await?)
            }
            #[cfg(feature = "desktop")]
            Request::ConfirmHostKey { setup: id, ok } => {
                if !setup::confirm_host_key(&id, ok) {
                    return Err(CommandError::Failed {
                        message: "That setup isn't waiting any more".to_owned(),
                    });
                }
                Response::ConfirmHostKey
            }
            Request::SetRoutes { link, hosts } => {
                devices.set_routes(link, &hosts)?;
                Response::SetRoutes
            }
            Request::AnswerLinkClaim { link, same } => {
                devices.answer_claim(link, same)?;
                Response::AnswerLinkClaim
            }
            Request::AskLinkClaimAgain { link } => {
                devices.ask_claim_again(link)?;
                Response::AskLinkClaimAgain
            }
            Request::ForgetLinkIdentity { link } => {
                devices.forget_identity(link)?;
                Response::ForgetLinkIdentity
            }
            #[cfg(feature = "desktop")]
            Request::LinkClaimDetails { link } => {
                Response::LinkClaimDetails(self.link_claim_details(link).await?)
            }
            #[cfg(feature = "desktop")]
            Request::ListSetupHosts => {
                let tailnet = devices.tailnet_hosts();
                Response::ListSetupHosts(self.view.read(|merge| setup::list_hosts(merge, tailnet)))
            }
            Request::ListPermissions { device } => {
                Response::ListPermissions(self.list_permissions(device).await?)
            }
            Request::GrantPermission { permission } => {
                Response::GrantPermission(self.grant_permission(permission).await?)
            }
            Request::RevokePermission { permission } => {
                Response::RevokePermission(self.revoke_permission(permission).await?)
            }
            Request::MintKey {
                device,
                name,
                permission,
            } => Response::MintKey(self.mint_key(device, name, permission).await?),
            Request::ListKeys { device } => Response::ListKeys(self.list_keys(device).await?),
            Request::ListDeviceLinks { device } => {
                Response::ListDeviceLinks(devices.device_links(device)?)
            }
            Request::SetDeviceLink { device, peer, on } => {
                devices.set_device_link(device, peer, on).await?;
                Response::SetDeviceLink
            }
            #[cfg(feature = "desktop")]
            Request::ListPhoneDevices => Response::ListPhoneDevices(devices.phone_devices().await?),
            #[cfg(feature = "desktop")]
            Request::BeginPhonePair {
                through,
                devices: to,
            } => Response::BeginPhonePair(devices.begin_phone_pair(through, &to).await?),
            #[cfg(feature = "desktop")]
            Request::RemovePhone { phone } => {
                devices.remove_phone(phone).await?;
                Response::RemovePhone
            }
            #[cfg(feature = "desktop")]
            Request::SetPhoneEverything { phone, everything } => {
                Response::SetPhoneEverything(devices.set_phone_everything(phone, everything).await?)
            }
            #[cfg(feature = "desktop")]
            Request::RenewPhoneCodes { phone } => {
                Response::RenewPhoneCodes(devices.renew_phone_codes(phone).await?)
            }
            #[cfg(feature = "desktop")]
            Request::SetPhoneAddress { device, address } => {
                devices.set_phone_address(device, address.as_deref())?;
                Response::SetPhoneAddress
            }
            #[cfg(feature = "desktop")]
            Request::RevokePhoneSession { session } => {
                let action = ClientMessage::RevokePhoneSession { key, session };
                Response::RevokePhoneSession(devices.act(action).await?)
            }
            #[cfg(feature = "desktop")]
            Request::AppKey => Response::AppKey(devices.app_key()?),
            #[cfg(feature = "desktop")]
            Request::RemoveAppKey { device, public_key } => {
                Response::RemoveAppKey(devices.remove_app_key(device, public_key).await?)
            }
            #[cfg(feature = "desktop")]
            Request::RotateAppKey => {
                devices.rotate_app_key()?;
                Response::RotateAppKey
            }
            #[cfg(feature = "desktop")]
            Request::AnswerAdmission { admission, allow } => {
                let action = ClientMessage::AnswerAdmission {
                    key,
                    admission,
                    allow,
                };
                Response::AnswerAdmission(devices.act(action).await?)
            }
            Request::AgentDefaults => Response::AgentDefaults(self.agent_defaults()?),
            Request::SetAgentDefaults { defaults } => {
                self.set_agent_defaults(defaults)?;
                Response::SetAgentDefaults
            }
            Request::ListTasks { device } => Response::ListTasks(self.list_tasks(device)?),
            Request::SaveTask { device, task } => {
                Response::SaveTask(self.save_task(device, *task).await?)
            }
            Request::SetTaskEnabled {
                device,
                task,
                enabled,
            } => Response::SetTaskEnabled(self.set_task_enabled(device, task, enabled).await?),
            Request::DeleteTask { task } => Response::DeleteTask(self.delete_task(task).await?),
            Request::RunTaskNow { task } => {
                let run = |key| ClientMessage::RunTaskNow { key, task };
                Response::RunTaskNow(self.act_on_task(task, run).await?)
            }
            Request::SetHookOn { task, on } => {
                let set = |key| ClientMessage::SetHookOn { key, task, on };
                Response::SetHookOn(self.act_on_task(task, set).await?)
            }
            Request::RotateHookToken { task } => {
                let rotate = |key| ClientMessage::RotateHookToken { key, task };
                Response::RotateHookToken(self.act_on_task(task, rotate).await?)
            }
            Request::SetHookSecret { task, secret } => {
                Response::SetHookSecret(self.set_hook_secret(task, secret).await?)
            }
            Request::SetHookAddress { device, address } => {
                Response::SetHookAddress(self.set_hook_address(device, address).await?)
            }
            Request::ListRuns { task } => Response::ListRuns(self.list_runs(task).await?),
            Request::ListDeliveries { task } => {
                Response::ListDeliveries(self.list_deliveries(task).await?)
            }
            Request::FunnelState { device } => Response::FunnelState(self.funnel_state(device)?),
            Request::CheckTestHosts { device } => {
                let check = |key| ClientMessage::CheckTestHosts { key };
                Response::CheckTestHosts(self.act_needing(device, Needs::TestHosts, check).await?)
            }
            Request::BuildTestHost { device, host } => {
                let build = |key| ClientMessage::BuildTestHost { key, host };
                Response::BuildTestHost(self.act_needing(device, Needs::TestHosts, build).await?)
            }
            Request::ResetTestHost { device, host } => {
                let reset = |key| ClientMessage::ResetTestHost { key, host };
                Response::ResetTestHost(self.act_needing(device, Needs::TestHosts, reset).await?)
            }
            Request::EndLease { device, lease } => {
                let end = |key| ClientMessage::EndLease { key, lease };
                Response::EndLease(self.act_needing(device, Needs::TestHosts, end).await?)
            }
            Request::ReadLeaseScreen { device, lease } => {
                let read = |key| ClientMessage::ReadLeaseScreen { key, lease };
                Response::ReadLeaseScreen(self.act_needing(device, Needs::TestHosts, read).await?)
            }
            Request::SkillSourceAdd {
                device,
                name,
                origin,
                default_ref,
                trust,
            } => {
                let add = |key| ClientMessage::SkillSourceAdd {
                    key,
                    name,
                    kind: origin,
                    default_ref,
                    trust,
                };
                Response::SkillSourceAdd(self.act_needing(device, Needs::Skills, add).await?)
            }
            Request::SkillSourceFetch { device, source } => {
                let fetch = |key| ClientMessage::SkillSourceFetch { key, source };
                Response::SkillSourceFetch(self.act_needing(device, Needs::Skills, fetch).await?)
            }
            Request::SkillSourceRemove { device, source } => {
                let remove = |key| ClientMessage::SkillSourceRemove { key, source };
                Response::SkillSourceRemove(self.act_needing(device, Needs::Skills, remove).await?)
            }
            Request::SkillsSetProject {
                device,
                project,
                lock,
            } => {
                let set = |key| ClientMessage::SkillsSetProject { key, project, lock };
                Response::SkillsSetProject(self.act_needing(device, Needs::Skills, set).await?)
            }
            Request::SkillsApply { device, project } => {
                self.view
                    .read(|merge| check_can(merge, device, Needs::Skills))?;
                crate::skills::bring_trees(&self.devices, &self.view, device, project).await?;
                let apply = |key| ClientMessage::SkillsApply { key, project };
                Response::SkillsApply(self.act_needing(device, Needs::Skills, apply).await?)
            }
            Request::SkillsScan { device, place } => {
                let scan = |key| ClientMessage::SkillsScan { key, place };
                Response::SkillsScan(self.act_needing(device, Needs::Skills, scan).await?)
            }
            Request::SkillsTreesNeeded { device, lock } => {
                let needed = |key| ClientMessage::SkillsTreesNeeded { key, lock };
                Response::SkillsTreesNeeded(self.act_needing(device, Needs::Skills, needed).await?)
            }
            Request::SkillProposalAction {
                proposal,
                action,
                push,
                fork,
                seen,
            } => Response::SkillProposalAction(
                self.skill_proposal_action(proposal, action, push, fork, seen)
                    .await?,
            ),
            Request::SkillProposalDiff { proposal } => {
                Response::SkillProposalDiff(self.skill_proposal_diff(proposal).await?)
            }
            Request::SkillsGlobalsPlan { device } => {
                let plan = |key| ClientMessage::SkillsGlobalsPlan { key };
                Response::SkillsGlobalsPlan(self.act_needing(device, Needs::Skills, plan).await?)
            }
            Request::SkillsGlobalsApply {
                device,
                plan_hash,
                picks,
            } => {
                let apply = |key| ClientMessage::SkillsGlobalsApply {
                    key,
                    plan_hash,
                    picks,
                };
                Response::SkillsGlobalsApply(self.act_needing(device, Needs::Skills, apply).await?)
            }
            Request::SkillsGlobalsUndo { device } => {
                let undo = |key| ClientMessage::SkillsGlobalsUndo { key };
                Response::SkillsGlobalsUndo(self.act_needing(device, Needs::Skills, undo).await?)
            }
            Request::MoveSkillLibrary { device } => Response::MoveSkillLibrary(
                crate::skills::move_library(&self.devices, &self.view, device).await?,
            ),
            Request::SkillSourcePush { device, source } => {
                let push = |key| ClientMessage::SkillSourcePush { key, source };
                Response::SkillSourcePush(
                    self.act_needing(device, Needs::LibraryMoves, push).await?,
                )
            }
            Request::CheckHost { device } => {
                let check = |key| ClientMessage::CheckHost { key };
                Response::CheckHost(self.act_needing(device, Needs::HostSpec, check).await?)
            }
            Request::ApplyHost { device, items } => {
                let apply = |key| ClientMessage::ApplyHost { key, items };
                Response::ApplyHost(self.act_needing(device, Needs::HostSpec, apply).await?)
            }
            Request::InstallTool {
                device,
                place,
                tool,
                version,
            } => {
                let install = |key| ClientMessage::InstallTool {
                    key,
                    place,
                    tool,
                    version,
                };
                Response::InstallTool(
                    self.act_needing(device, Needs::ToolInstalls, install)
                        .await?,
                )
            }
            Request::RemoveTool {
                device,
                place,
                tool,
            } => {
                let remove = |key| ClientMessage::RemoveTool { key, place, tool };
                Response::RemoveTool(
                    self.act_needing(device, Needs::ToolInstalls, remove)
                        .await?,
                )
            }
            Request::ListSnapshots { workspace } => {
                let device = self
                    .view
                    .read(|merge| crate::devices::workspace_device(merge, workspace))?;
                let list = |key| ClientMessage::ListSnapshots { key, workspace };
                Response::ListSnapshots(self.act_needing(device, Needs::Snapshots, list).await?)
            }
            Request::RestoreSnapshot {
                workspace,
                snapshot,
                as_new,
                discard_unsaved,
            } => {
                let device = self
                    .view
                    .read(|merge| crate::devices::workspace_device(merge, workspace))?;
                let restore = |key| ClientMessage::RestoreSnapshot {
                    key,
                    workspace,
                    snapshot,
                    as_new,
                    discard_unsaved,
                };
                Response::RestoreSnapshot(
                    self.act_needing(device, Needs::Snapshots, restore).await?,
                )
            }
            Request::PlanMove { device, instance } => {
                let plan = |key| ClientMessage::PlanMove { key, instance };
                Response::PlanMove(self.act_needing(device, Needs::Moves, plan).await?)
            }
            Request::RunMoveStep { moving, step } => {
                let device = self.view.read(|merge| move_device(merge, moving))?;
                let run = |key| ClientMessage::RunMoveStep { key, moving, step };
                Response::RunMoveStep(self.act_needing(device, Needs::Moves, run).await?)
            }
            Request::MoveBack { moving } => {
                let device = self.view.read(|merge| move_device(merge, moving))?;
                let back = |key| ClientMessage::MoveBack { key, moving };
                Response::MoveBack(self.act_needing(device, Needs::Moves, back).await?)
            }
            Request::RetireMove { moving } => {
                let device = self.view.read(|merge| move_device(merge, moving))?;
                let retire = |key| ClientMessage::RetireMove { key, moving };
                Response::RetireMove(self.act_needing(device, Needs::Moves, retire).await?)
            }
            Request::InstallAcpAgent {
                device,
                agent,
                version,
                place,
                confirm_unchecked,
            } => {
                let install = |key| ClientMessage::InstallAcpAgent {
                    key,
                    agent,
                    version,
                    place,
                    confirm_unchecked,
                };
                Response::InstallAcpAgent(
                    self.act_needing(device, Needs::ToolInstalls, install)
                        .await?,
                )
            }
            Request::AcceptLicence {
                device,
                licence,
                text_sha256,
            } => {
                let accept = |key| ClientMessage::AcceptLicence {
                    key,
                    licence,
                    text_sha256,
                };
                Response::AcceptLicence(
                    self.act_needing(device, Needs::ToolInstalls, accept)
                        .await?,
                )
            }
            Request::AskDispatcher {
                device,
                text,
                account,
                new,
                refs,
            } => {
                let action = ClientMessage::AskDispatcher {
                    key,
                    text,
                    account,
                    new,
                    refs,
                };
                Response::AskDispatcher(devices.act_on(device, action).await?)
            }
            Request::ConfirmPlan { device, plan } => {
                let action = ClientMessage::ConfirmPlan { key, plan };
                Response::ConfirmPlan(devices.act_on(device, action).await?)
            }
            Request::DismissPlan { device, plan } => {
                let action = ClientMessage::DismissPlan { key, plan };
                Response::DismissPlan(devices.act_on(device, action).await?)
            }
            Request::MoveAgent {
                device,
                agent,
                from,
                to,
                stop,
            } => {
                let action = ClientMessage::MoveAgent {
                    key,
                    agent,
                    from,
                    to,
                    stop,
                };
                Response::MoveAgent(devices.act_on(device, action).await?)
            }
            Request::TerminalOpen {
                agent,
                runs,
                cols,
                rows,
            } => {
                let action = ClientMessage::TerminalOpen {
                    key,
                    agent,
                    kind: runs,
                    cols,
                    rows,
                };
                Response::TerminalOpen(devices.act(action).await?)
            }
            Request::TerminalAttach { terminal, after } => {
                devices
                    .send_terminal(terminal, ClientMessage::TerminalAttach { terminal, after })
                    .await?;
                Response::TerminalAttach
            }
            Request::TerminalDetach { terminal } => {
                devices
                    .send_terminal(terminal, ClientMessage::TerminalDetach { terminal })
                    .await?;
                Response::TerminalDetach
            }
            Request::TerminalResize {
                terminal,
                cols,
                rows,
            } => {
                let resize = ClientMessage::TerminalResize {
                    terminal,
                    cols,
                    rows,
                };
                devices.send_terminal(terminal, resize).await?;
                Response::TerminalResize
            }
            Request::TerminalInterrupt { terminal } => {
                let action = ClientMessage::TerminalInterrupt { key, terminal };
                Response::TerminalInterrupt(devices.act(action).await?)
            }
            Request::TerminalClose { terminal, force } => {
                let action = ClientMessage::TerminalClose {
                    key,
                    terminal,
                    force,
                };
                Response::TerminalClose(devices.act(action).await?)
            }
            Request::TerminalRename { terminal, title } => {
                let action = ClientMessage::TerminalRename {
                    key,
                    terminal,
                    title,
                };
                Response::TerminalRename(devices.act(action).await?)
            }
            Request::TerminalMove { terminal } => {
                let action = ClientMessage::TerminalMove { key, terminal };
                Response::TerminalMove(devices.act(action).await?)
            }
            Request::ListFolder {
                device,
                path,
                files,
                hidden,
                filter,
            } => {
                let action = ClientMessage::ListFolder {
                    key,
                    device,
                    path,
                    files,
                    hidden,
                    filter,
                };
                match devices.act_unless_old(action).await? {
                    None => Response::ListFolder(FolderList::TooOld),
                    Some(ActionOutcome::FolderListed { listing }) => {
                        Response::ListFolder(FolderList::Listed { listing })
                    }
                    Some(other) => return Err(lookup_failed(other)),
                }
            }
            Request::MakeFolder { device, path } => {
                let action = ClientMessage::MakeFolder { key, device, path };
                Response::MakeFolder(devices.act(action).await?)
            }
            Request::SessionRun {
                agent,
                command,
                private,
                fresh,
                cols,
                rows,
            } => {
                let action = ClientMessage::SessionRun {
                    key,
                    agent,
                    command,
                    private,
                    fresh,
                    cols,
                    rows,
                };
                Response::SessionRun(devices.act(action).await?)
            }
            Request::SessionShare {
                agent,
                item,
                block,
                private,
            } => {
                let action = ClientMessage::SessionShare {
                    key,
                    agent,
                    item,
                    block,
                    private,
                };
                Response::SessionShare(devices.act(action).await?)
            }
            Request::SessionCd { agent, path } => {
                let action = ClientMessage::SessionCd { key, agent, path };
                Response::SessionCd(devices.act(action).await?)
            }
            Request::SessionEnd { agent, force } => {
                let action = ClientMessage::SessionEnd { key, agent, force };
                Response::SessionEnd(devices.act(action).await?)
            }
            Request::ModelAction { device, action } => {
                let act = |key| ClientMessage::ModelAction { key, action };
                Response::ModelAction(self.act_needing(device, Needs::LocalModels, act).await?)
            }
        })
    }

    /// Sends a pasted token or key to each device under a fresh key, one
    /// result for each. `missing` is the answer when nothing was pasted.
    async fn send_pasted(
        &self,
        to: &[DeviceId],
        token: Secret,
        missing: &str,
        store: impl Fn(IdempotencyKey, Secret) -> ClientMessage,
    ) -> Vec<ActionReply> {
        // A long token wraps in the terminal, so its copy can carry spaces
        // and line breaks that were never part of it.
        let token = token.without_whitespace();
        if token.is_empty() {
            return each(to, failed(missing));
        }
        let outcomes = self
            .devices
            .act_on_each(to, |key| store(key, token.clone()))
            .await;
        outcomes.into_iter().map(ActionReply::from).collect()
    }

    /// Sends a GitHub App to each device under a fresh key, one result for
    /// each.
    async fn set_github_app(
        &self,
        to: &[DeviceId],
        app_id: GithubAppId,
        key_path: &str,
    ) -> Vec<ActionReply> {
        if app_id.0 == 0 {
            return each(to, failed("Enter the App ID from the app's settings page"));
        }
        let private_key = match read_key_file(&self.offered, Path::new(key_path), &GITHUB_KEY) {
            Ok(key) => key,
            Err(error) => return each(to, error),
        };
        let send = |key| ClientMessage::SetGithubApp {
            key,
            app_id: app_id.0,
            private_key: private_key.clone(),
        };
        let outcomes = self.devices.act_on_each(to, send).await;
        outcomes.into_iter().map(ActionReply::from).collect()
    }

    /// Sends the APNs key to each device that pushes to phones, under a
    /// fresh key for each, one result for each. A device whose `maxd`
    /// doesn't push gets a refusal in words and never sees the key.
    async fn set_apns_key(
        &self,
        to: &[DeviceId],
        key_id: &str,
        team_id: &str,
        key_path: &str,
    ) -> Vec<ActionReply> {
        let (key_id, team_id) = (key_id.trim(), team_id.trim());
        if key_id.is_empty() {
            return each(
                to,
                failed("Enter the Key ID from the key's page in your Apple developer account"),
            );
        }
        if team_id.is_empty() {
            return each(to, failed("Enter your Apple team ID"));
        }
        let private_key = match read_key_file(&self.offered, Path::new(key_path), &APNS_KEY) {
            Ok(key) => key,
            Err(error) => return each(to, error),
        };
        let mut replies = Vec::with_capacity(to.len());
        for &device in to {
            let send = |key| ClientMessage::SetApnsKey {
                key,
                key_id: key_id.to_owned(),
                team_id: team_id.to_owned(),
                private_key: private_key.clone(),
            };
            replies.push(self.act_needing(device, Needs::Phones, send).await.into());
        }
        replies
    }

    /// Reads what a proposal changes. One found where the library isn't
    /// goes on to the holder, which shows it against its own copy of the
    /// skill, since approving it writes there. A holder from before that
    /// can't, so the device it was found on answers then.
    async fn skill_proposal_diff(
        &self,
        proposal: SkillProposalId,
    ) -> Result<ActionOutcome, CommandError> {
        let (found, holder) = self.view.read(|merge| {
            let found = proposal_of(merge, proposal)?;
            let holder = library_holder(merge).filter(|holder| {
                *holder != found.device
                    && crate::devices::capabilities(merge, *holder)
                        .is_some_and(|does| does.proposal_targets)
            });
            Ok::<_, CommandError>((found, holder))
        })?;
        let diff = |elsewhere: Option<Box<SkillProposal>>| {
            move |key| ClientMessage::SkillProposalDiff {
                key,
                proposal,
                elsewhere,
            }
        };
        // The diff keeps the proposal's tree in the store where it is.
        let there = self
            .act_needing(found.device, Needs::Skills, diff(None))
            .await?;
        let (Some(holder), ActionOutcome::SkillProposalDiff { .. }) = (holder, &there) else {
            return Ok(there);
        };
        if let Err(why) = self.bring_tree(found.device, holder, &found.tree).await {
            return Ok(ActionOutcome::Refused {
                reason: format!("The change couldn't reach the skill library: {why}"),
            });
        }
        let held = diff(Some(Box::new(found)));
        self.act_needing(holder, Needs::Skills, held).await
    }

    /// Sends a proposal's answer to the device the proposal was found on.
    /// An approve of one found where the library isn't goes to the holder
    /// instead, once the proposal's tree is in the holder's store, and the
    /// device it was found on then drops it.
    async fn skill_proposal_action(
        &self,
        proposal: SkillProposalId,
        action: ProposalAction,
        push: bool,
        fork: Option<String>,
        seen: Option<ProposalSeen>,
    ) -> Result<ActionOutcome, CommandError> {
        let (found, holder) = self.view.read(|merge| {
            let found = proposal_of(merge, proposal)?;
            Ok::<_, CommandError>((found, library_holder(merge)))
        })?;
        let answer = |elsewhere: Option<Box<SkillProposal>>,
                      fork: Option<String>,
                      seen: Option<ProposalSeen>| {
            move |key| ClientMessage::SkillProposalAction {
                key,
                proposal,
                action,
                push,
                fork,
                elsewhere,
                seen,
            }
        };
        let holder = match holder {
            Some(holder) if action == ProposalAction::Approve && holder != found.device => holder,
            _ => {
                let here = answer(None, fork, seen);
                return self.act_needing(found.device, Needs::Skills, here).await;
            }
        };
        // The diff keeps the proposal's tree in the store where it is.
        let diff = |key| ClientMessage::SkillProposalDiff {
            key,
            proposal,
            elsewhere: None,
        };
        match self.act_needing(found.device, Needs::Skills, diff).await? {
            ActionOutcome::SkillProposalDiff { .. } => {}
            other => return Ok(other),
        }
        if let Err(why) = self.bring_tree(found.device, holder, &found.tree).await {
            return Ok(ActionOutcome::Refused {
                reason: format!("The change couldn't reach the skill library: {why}"),
            });
        }
        let approve = answer(Some(Box::new(found.clone())), fork, seen);
        let approved = self.act_needing(holder, Needs::Skills, approve).await?;
        // A push that failed still leaves the change in the source.
        let landed = match &approved {
            ActionOutcome::Done => true,
            ActionOutcome::Refused { reason } => reason.starts_with(COMMITTED_UNPUSHED),
            _ => false,
        };
        if landed {
            let drop = |key| ClientMessage::SkillProposalAction {
                key,
                proposal,
                action: ProposalAction::Reject,
                push: false,
                fork: None,
                elsewhere: None,
                seen: None,
            };
            if let ActionOutcome::Refused { reason } =
                self.act_needing(found.device, Needs::Skills, drop).await?
            {
                return Ok(ActionOutcome::Refused {
                    reason: format!(
                        "The change is in the library, but its proposal couldn't be removed: {reason}"
                    ),
                });
            }
        }
        Ok(approved)
    }

    /// Copies a skill tree from one device's store into another's.
    async fn bring_tree(
        &self,
        from: DeviceId,
        to: DeviceId,
        tree: &TreeHash,
    ) -> Result<(), CommandError> {
        self.devices.check_copies(from)?;
        self.devices.check_copies(to)?;
        let quiet: OnProgress = Arc::new(|_| {});
        crate::skills::send_tree(&self.devices, from, to, tree.clone(), quiet).await
    }
}

/// How the holder starts its answer when the commit went in but the push
/// didn't.
const COMMITTED_UNPUSHED: &str = "The change is committed";

/// The device that keeps the skill library, as far as the app knows.
fn library_holder(merge: &Merge) -> Option<DeviceId> {
    crate::skills::held_library(merge).map(|library| library.device)
}

/// The proposal the app knows by `proposal`.
fn proposal_of(merge: &Merge, proposal: SkillProposalId) -> Result<SkillProposal, CommandError> {
    match merge.store().get(&EntityKey::SkillProposal(proposal)) {
        Some(Entity::SkillProposal(found)) => Ok(found.clone()),
        _ => Err(CommandError::NotFound {
            what: "This proposal".to_owned(),
        }),
    }
}

/// The device a move the app knows by `moving` runs on.
fn move_device(merge: &Merge, moving: MoveId) -> Result<DeviceId, CommandError> {
    match merge.store().get(&EntityKey::Move(moving)) {
        Some(Entity::Move(found)) => Ok(found.device),
        _ => Err(CommandError::NotFound {
            what: "This move".to_owned(),
        }),
    }
}

/// What a note save made the note's head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct SavedNote {
    /// The new head.
    pub version: NoteVersion,
    /// Whether the save merged with changes made since its base, so the
    /// head holds more than what was sent.
    pub merged: bool,
}

/// One device's latest CPU and memory, or none when it couldn't say.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct DeviceLoad {
    pub device: DeviceId,
    pub load: Option<HostLoad>,
    /// When the app got it, by its own clock.
    pub received_at: Timestamp,
}

/// What the footer shows, and what a bug report needs to say which build it
/// came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct AppInfo {
    /// The app's version, from the workspace manifest.
    pub version: String,
    /// The protocol version this build speaks to `maxd`.
    pub protocol_version: u32,
    /// The `maxd` builds this app carries for other devices.
    pub bundled: Vec<crate::fleet::BundledBuild>,
}

/// The app and protocol versions.
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: maxhq_protocol::PROTOCOL_VERSION,
        bundled: bundled_builds(),
    }
}

/// The `maxd` builds this app carries. The phone carries none.
fn bundled_builds() -> Vec<crate::fleet::BundledBuild> {
    #[cfg(feature = "desktop")]
    {
        crate::fleet::BinaryDirs::for_app().bundled()
    }
    #[cfg(not(feature = "desktop"))]
    {
        Vec::new()
    }
}

/// Why a request failed, in words a window can show as they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type, Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandError {
    /// Nothing has that ID, such as an agent that just ended.
    #[error("{what} is gone")]
    NotFound {
        /// What the request was looking for.
        what: String,
    },
    /// The device the request needs isn't connected.
    #[error("{device} isn't connected")]
    NotConnected {
        /// The device's name.
        device: String,
    },
    /// Anything else.
    #[error("{message}")]
    Failed {
        /// What went wrong.
        message: String,
    },
}

/// The files git ignores in a checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct IgnoredFiles {
    /// The files, sorted by path.
    pub files: Vec<IgnoredFile>,
    /// Whether there were more than the list holds.
    pub truncated: bool,
}

/// One action's result, for a request that answers with several.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ActionReply {
    /// The device answered.
    Ok {
        /// What came of the action.
        data: ActionOutcome,
    },
    /// The action never reached the device, or no result came.
    Error {
        /// Why.
        error: CommandError,
    },
}

impl From<Result<ActionOutcome, CommandError>> for ActionReply {
    fn from(result: Result<ActionOutcome, CommandError>) -> Self {
        match result {
            Ok(data) => Self::Ok { data },
            Err(error) => Self::Error { error },
        }
    }
}

/// An item's position in a timeline, which a window holds as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(transparent)]
pub struct Ordinal(#[specta(type = specta_typescript::Number)] pub u64);

/// Who commits in a project whose settings name no author: this Mac's
/// global git identity. A part git has no value for is left out.
#[cfg(feature = "desktop")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct GitIdentity {
    /// The global `user.name`.
    pub name: Option<String>,
    /// The global `user.email`.
    pub email: Option<String>,
}

/// Reads the global identity with the git that `git` starts.
#[cfg(feature = "desktop")]
async fn read_git_identity(git: impl Fn() -> tokio::process::Command) -> GitIdentity {
    GitIdentity {
        name: global_git(git(), "user.name").await,
        email: global_git(git(), "user.email").await,
    }
}

/// One key from the global git config, or None when it's unset or git
/// doesn't run. The repository variables a git hook sets are dropped, so
/// git reads the user's config and no repository's.
#[cfg(feature = "desktop")]
async fn global_git(mut git: tokio::process::Command, key: &str) -> Option<String> {
    let output = git
        .args(["config", "--global", "--get", key])
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// A GitHub App's ID, which a window holds as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(transparent)]
pub struct GithubAppId(#[specta(type = specta_typescript::Number)] pub u64);

/// The largest private key file the app reads. A GitHub App's is about
/// 1.7 KB, and an APNs `.p8` well under that.
const KEY_FILE_LIMIT: usize = 16 * 1024;

/// A kind of private key file the app reads and sends on.
struct KeyFile {
    /// The PEM labels it may carry.
    labels: &'static [&'static str],
    /// Why a file that isn't one is refused.
    not_a_key: &'static str,
}

/// A GitHub App's key, in PKCS #1 as GitHub gives it or in PKCS #8.
const GITHUB_KEY: KeyFile = KeyFile {
    labels: &["RSA PRIVATE KEY", "PRIVATE KEY"],
    not_a_key: "That file isn't a GitHub App private key. Its name ends in .private-key.pem",
};

/// The APNs key Apple gives as a `.p8`, in PKCS #8.
const APNS_KEY: KeyFile = KeyFile {
    labels: &["PRIVATE KEY"],
    not_a_key: "That file isn't an Apple push key. Its name starts with AuthKey_ and ends in .p8",
};

/// Reads a private key from a file you picked or dropped. It goes into a
/// [`Secret`] at once, and no error says what the file holds.
fn read_key_file(offered: &Offered, path: &Path, kind: &KeyFile) -> Result<Secret, CommandError> {
    if !offered.contains(path) {
        return Err(failed(
            "Choose the private key file again, since MaxHQ only reads files you pick",
        ));
    }
    let not_a_key = || failed(kind.not_a_key);
    let file = std::fs::File::open(path)
        .map_err(|error| failed(&format!("The private key file didn't open: {error}")))?;
    // Room for the whole limit up front means the buffer never moves, so
    // it leaves no copy of the key behind.
    let mut bytes = Vec::with_capacity(KEY_FILE_LIMIT + 1);
    file.take(KEY_FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| failed(&format!("The private key file didn't read: {error}")))?;
    let key = Secret::new(String::from_utf8(bytes).map_err(|_| not_a_key())?);
    let text = key.expose();
    let pem = kind.labels.iter().any(|label| {
        text.contains(&format!("-----BEGIN {label}-----"))
            && text.contains(&format!("-----END {label}-----"))
    });
    if text.len() > KEY_FILE_LIMIT || !pem {
        return Err(not_a_key());
    }
    Ok(key)
}

/// The same error for each device.
fn each(devices: &[DeviceId], error: CommandError) -> Vec<ActionReply> {
    devices
        .iter()
        .map(|_| ActionReply::Error {
            error: error.clone(),
        })
        .collect()
}

fn failed(message: &str) -> CommandError {
    CommandError::Failed {
        message: message.to_owned(),
    }
}

/// Why a lookup came back without what it looked for: the device's
/// reason, or that it answered with something else.
pub(crate) fn lookup_failed(outcome: ActionOutcome) -> CommandError {
    match outcome {
        ActionOutcome::Refused { reason } => CommandError::Failed { message: reason },
        ActionOutcome::UnknownAgent => CommandError::NotFound {
            what: "This agent".to_owned(),
        },
        _ => failed("The device answered the lookup with something else"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use maxhq_protocol::ids::{AdmissionId, AttachmentId};
    use maxhq_protocol::model::HiddenProjects;
    use maxhq_protocol::tasks::DeviceTasks;
    use serde_json::{json, Value};

    use super::*;
    use crate::devices::tests::{connect_doing, two_devices, Call, Harness};
    #[cfg(feature = "desktop")]
    use crate::editors::tests::Scratch;
    #[cfg(feature = "desktop")]
    use crate::editors::Installed;
    use crate::tests::{block_on, FakeShell};

    /// A file in the temporary folder that goes away with the test.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "maxhq-commands-{}-{}-{name}",
                std::process::id(),
                IdempotencyKey::generate()
            ));
            std::fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    const KEY: &str = "-----BEGIN RSA PRIVATE KEY-----\ntest-not-a-real-key\n\
                       -----END RSA PRIVATE KEY-----\n";

    #[test]
    fn a_github_key_is_read_only_from_a_file_you_picked() {
        let offered = Offered::default();
        let file = TempFile::new("app.private-key.pem", KEY.as_bytes());
        assert!(matches!(
            read_key_file(&offered, &file.0, &GITHUB_KEY),
            Err(CommandError::Failed { .. })
        ));

        offered.offer([file.0.clone()]);
        let key = read_key_file(&offered, &file.0, &GITHUB_KEY).unwrap();
        assert_eq!(key.expose(), KEY);
    }

    #[test]
    fn a_file_that_isnt_a_key_is_refused_without_saying_what_it_holds() {
        let offered = Offered::default();
        let notes = TempFile::new("notes.txt", b"test-not-a-real-token");
        let huge = TempFile::new(
            "huge.pem",
            format!("{KEY}{}", "x".repeat(KEY_FILE_LIMIT)).as_bytes(),
        );
        let binary = TempFile::new("key.der", &[0xff, 0xfe, 0x00, 0x01]);
        let missing = std::env::temp_dir().join("maxhq-commands-missing.pem");
        offered.offer([
            notes.0.clone(),
            huge.0.clone(),
            binary.0.clone(),
            missing.clone(),
        ]);
        for path in [&notes.0, &huge.0, &binary.0, &missing] {
            let Err(CommandError::Failed { message }) = read_key_file(&offered, path, &GITHUB_KEY)
            else {
                panic!("{} should be refused", path.display());
            };
            assert!(!message.contains("test-not-a-real"), "{message}");
        }
    }

    #[test]
    fn an_error_for_all_goes_to_each_device() {
        let devices = [DeviceId::derive("mac"), DeviceId::derive("nl")];
        let error = failed("Paste the setup-token first");
        assert_eq!(
            each(&devices, error.clone()),
            [
                ActionReply::Error {
                    error: error.clone()
                },
                ActionReply::Error { error }
            ]
        );
    }

    /// Git reading `config` as its global config, with a hook's repository
    /// variables set the way a commit sets them.
    #[cfg(feature = "desktop")]
    fn git_with(config: &Path) -> impl Fn() -> tokio::process::Command + '_ {
        move || {
            let mut git = tokio::process::Command::new("git");
            git.env("GIT_CONFIG_GLOBAL", config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_DIR", "/nonexistent/maxhq-test")
                .env("GIT_INDEX_FILE", "/nonexistent/maxhq-test/index")
                .env("GIT_WORK_TREE", "/nonexistent/maxhq-test");
            git
        }
    }

    #[cfg(feature = "desktop")]
    #[test]
    fn the_git_identity_comes_from_the_global_config() {
        let config = TempFile::new(
            "gitconfig",
            b"[user]
	name = Test Person
	email = test@example.com
",
        );
        let identity = block_on(read_git_identity(git_with(&config.0)));
        assert_eq!(
            identity,
            GitIdentity {
                name: Some("Test Person".into()),
                email: Some("test@example.com".into()),
            }
        );

        let empty = TempFile::new("empty-gitconfig", b"");
        let identity = block_on(read_git_identity(git_with(&empty.0)));
        assert_eq!(
            identity,
            GitIdentity {
                name: None,
                email: None,
            }
        );
    }

    /// One request of each kind, as a window sends it. Removing devbox
    /// comes last, since the others want it there.
    fn one_of_each(devbox: LinkId) -> Vec<Value> {
        let (a, w) = (AgentId::derive("a"), WorkspaceId::derive("w"));
        let (mac, project) = (DeviceId::derive("mac"), ProjectId::derive("p"));
        let personal = AccountId::derive("personal");
        let permission = json!({
            "id": PermissionId::derive("p"),
            "holder": {"kind": "agent", "agent": a},
            "home": mac,
            "devices": [mac],
            "vendors": ["claude"],
            "accounts": null,
            "places": null,
            "depth": 3,
        });
        let task = serde_json::to_value(crate::tasks::tests::task("t", "mac")).unwrap();
        let run = json!({"device": mac, "project": project, "workflow": "build", "run": "r1"});
        let terminal = TerminalId::derive("terminal");
        vec![
            json!({"kind": "app_info"}),
            json!({"kind": "view_snapshot"}),
            json!({"kind": "mark_seen", "agent": a}),
            json!({"kind": "set_shown", "shown": {"focused": a, "visible": [a]}}),
            json!({"kind": "refresh_usage", "account": null}),
            json!({"kind": "refresh_links", "link": devbox}),
            json!({"kind": "add_device", "host": "dev@other"}),
            json!({"kind": "rename_device", "link": devbox, "name": "Devbox"}),
            json!({"kind": "start_local_maxd"}),
            json!({"kind": "uninstall_local_maxd", "delete_data": false}),
            json!({"kind": "restart_maxd", "link": devbox}),
            json!({"kind": "close_timeline", "agent": a}),
            json!({"kind": "load_older", "agent": a, "before": 40}),
            json!({"kind": "close_workflow", "run": run}),
            json!({"kind": "node_detail", "run": run, "node": "build"}),
            json!({"kind": "start_agent", "spec": {
                "vendor": "claude",
                "account": personal,
                "place": {"type": "workspace", "workspace": w},
                "model": null,
                "permission": null,
                "title": null,
                "first": null,
                "branch": null,
            }}),
            json!({
                "kind": "send_input",
                "agent": a,
                "input": {"text": "hi", "attachments": []},
                "mode": "queue",
            }),
            json!({"kind": "cancel_queued", "agent": a, "item": "item-1"}),
            json!({"kind": "interrupt", "agent": a}),
            json!({"kind": "set_permission", "agent": a, "mode": "ask"}),
            json!({"kind": "set_model", "agent": a, "model": "opus"}),
            json!({"kind": "stop_agent", "agent": a}),
            json!({"kind": "delete_agent", "agent": a}),
            json!({"kind": "rename_agent", "agent": a, "title": "Release notes"}),
            json!({"kind": "set_inbox", "agents": [a], "change": {"change": "settle"}}),
            json!({"kind": "continue_here", "agent": a}),
            json!({"kind": "take_over", "agent": a, "mode": "after_turn"}),
            json!({"kind": "find_paths", "agent": a, "query": "src/ma"}),
            json!({"kind": "list_processes", "agent": a}),
            json!({
                "kind": "signal_process",
                "agent": a,
                "pid": 4242,
                "start": 1_700_000_000_000_000_u64,
                "signal": "term",
            }),
            json!({"kind": "read_note", "device": mac, "project": project,
                "note": NoteId::derive("n")}),
            json!({"kind": "save_note", "device": mac, "project": project,
                "note": NoteId::derive("n"), "title": "Release checklist",
                "body": "- [ ] Tag\n", "deleted": false}),
            json!({"kind": "note_history", "device": mac, "project": project,
                "note": NoteId::derive("n")}),
            json!({"kind": "find_git_items", "agent": a, "query": "12"}),
            json!({"kind": "excerpt_agent", "agent": a, "budget": 20000}),
            json!({
                "kind": "find_in_timeline",
                "agent": a,
                "query": "flaky test",
                "at": {"type": "from", "from": {"item": "claude:msg_01", "occurrence": 0}, "offset": 1},
            }),
            json!({
                "kind": "answer",
                "event": AttentionId::derive("q"),
                "answer": {"type": "text", "text": "yes"},
            }),
            json!({"kind": "stage_offered", "device": mac, "paths": ["/tmp/a.png"]}),
            json!({"kind": "stage_found", "agent": a, "paths": ["src/main.rs"]}),
            json!({
                "kind": "stage_pasted",
                "device": mac,
                "name": "paste.png",
                "mime": "image/png",
                "data": "aGk=",
            }),
            json!({"kind": "cancel_transfer", "transfer": TransferId::derive("t")}),
            json!({"kind": "open_forward", "port": PortId::derive("5173")}),
            json!({"kind": "close_forward", "forward": ForwardId::derive("f")}),
            json!({"kind": "open_localhost", "port": 5173}),
            json!({"kind": "set_device_settings", "settings": {
                "device": mac,
                "default_permission": "ask",
                "workspace_network": null,
                "workspace_pool": null,
            }}),
            json!({"kind": "create_workspace", "spec": {
                "project": project,
                "device": mac,
                "name": null,
                "start_at_boot": false,
            }}),
            json!({"kind": "start_workspace", "workspace": w}),
            json!({"kind": "stop_workspace", "workspace": w}),
            json!({"kind": "delete_workspace", "workspace": w, "discard_unpushed": false}),
            json!({"kind": "ssh_include", "workspace": w}),
            json!({"kind": "setup_ssh_include", "workspace": w, "add": false}),
            json!({"kind": "list_ignored_files", "workspace": w}),
            json!({"kind": "clone_cancel", "workspace": w}),
            json!({"kind": "clone_retry", "workspace": w}),
            json!({"kind": "clone_remove", "workspace": w}),
            json!({"kind": "list_editors"}),
            json!({"kind": "open_in_editor", "editor": "cursor", "workspace": w,
                "path": "src/main.rs", "line": 3, "column": null}),
            json!({"kind": "set_project_settings", "settings": {
                "project": project,
                "device": mac,
                "resume_on_restart": true,
                "ignored_ports": [],
                "git_author": null,
            }}),
            json!({"kind": "set_project_hidden", "project": project, "hidden": true}),
            json!({"kind": "remove_command_rule", "device": mac, "project": project,
                "rule": {"command": "git show", "exact": false}}),
            json!({"kind": "git_identity"}),
            json!({"kind": "wind_down", "device": mac, "window_secs": 30}),
            json!({
                "kind": "store_claude_token",
                "devices": [mac],
                "origin": personal,
                "label": "Work",
                "token": "test-not-a-real-token",
            }),
            json!({
                "kind": "store_cursor_key",
                "devices": [mac],
                "label": "Work",
                "token": "test-not-a-real-key",
            }),
            json!({"kind": "sign_in_chatgpt", "device": mac}),
            json!({
                "kind": "set_github_app",
                "devices": [mac],
                "app_id": 1,
                "key_path": "/tmp/app.private-key.pem",
            }),
            json!({"kind": "check_github_app", "device": mac}),
            json!({
                "kind": "set_apns_key",
                "devices": [mac],
                "key_id": "ABC123DEFG",
                "team_id": "<TEAM_ID>",
                "key_path": "/tmp/AuthKey_ABC123DEFG.p8",
            }),
            json!({
                "kind": "forget_credential",
                "device": mac,
                "credential": CredentialId::derive("c"),
            }),
            json!({
                "kind": "set_acp_agent",
                "device": mac,
                "agent": {
                    "id": AcpAgentId::derive("gemini"),
                    "device": mac,
                    "name": "Gemini",
                    "program": "gemini",
                    "args": ["--experimental-acp"],
                    "env": ["API_KEY"],
                    "containers": false,
                    "problem": null,
                },
                "env": [{"name": "API_KEY", "value": "test-not-a-real-key"}],
            }),
            json!({"kind": "delete_acp_agent", "device": mac, "id": AcpAgentId::derive("gemini")}),
            json!({"kind": "list_pulls", "project": project, "state": "open"}),
            json!({"kind": "list_branches", "project": project}),
            json!({"kind": "get_pull", "device": mac, "repo": "owner/repo", "number": 7}),
            json!({"kind": "pull_files", "device": mac, "repo": "owner/repo", "number": 7, "page": 1}),
            json!({"kind": "pull_comments", "device": mac, "repo": "owner/repo", "number": 7}),
            json!({"kind": "pull_checks", "device": mac, "repo": "owner/repo", "number": 7}),
            json!({
                "kind": "pull_action",
                "repo": "owner/repo",
                "number": 7,
                "action": {"action": "merge", "method": "squash"},
                "expect_sha": "abc",
            }),
            json!({"kind": "pulls_due", "device": mac, "repo": "owner/repo"}),
            json!({"kind": "link_pull", "agent": a, "number": 7}),
            json!({"kind": "unlink_pull", "agent": a, "repo": "owner/repo", "number": 7}),
            json!({"kind": "begin_github_sign_in", "device": mac}),
            json!({"kind": "cancel_github_sign_in", "device": mac, "sign_in": SignInId::derive("s")}),
            json!({"kind": "check_github_sign_in", "device": mac, "sign_in": SignInId::derive("s")}),
            json!({"kind": "begin_cli_login", "device": mac, "account": AccountId::derive("a")}),
            json!({"kind": "finish_cli_login", "device": mac, "sign_in": SignInId::derive("s"), "code": "abc"}),
            json!({"kind": "check_cli_login", "device": mac, "sign_in": SignInId::derive("s")}),
            json!({"kind": "cancel_cli_login", "device": mac, "sign_in": SignInId::derive("s")}),
            json!({"kind": "read_load", "devices": [mac]}),
            json!({"kind": "read_diagnostics", "device": mac, "window_secs": 3600, "bucket_secs": 60}),
            json!({"kind": "update_tools", "device": mac, "tools": [
                {"tool": "claude", "place": {"type": "host"}},
            ]}),
            json!({"kind": "confirm_host_key", "setup": "setup-0", "ok": false}),
            json!({"kind": "set_routes", "link": devbox, "hosts": ["devbox", "devbox-tail"]}),
            json!({"kind": "answer_link_claim", "link": devbox, "same": false}),
            json!({"kind": "ask_link_claim_again", "link": devbox}),
            json!({"kind": "link_claim_details", "link": devbox}),
            json!({"kind": "answer_admission", "admission": AdmissionId::derive("card"), "allow": false}),
            json!({"kind": "list_setup_hosts"}),
            json!({"kind": "list_permissions", "device": null}),
            json!({"kind": "grant_permission", "permission": permission.clone()}),
            json!({"kind": "revoke_permission", "permission": permission["id"]}),
            json!({"kind": "mint_key", "device": mac, "name": "ci-bot", "permission": permission}),
            json!({"kind": "list_keys", "device": mac}),
            json!({"kind": "list_device_links", "device": mac}),
            json!({"kind": "set_device_link", "device": mac, "peer": DeviceId::derive("nl"), "on": false}),
            json!({"kind": "agent_defaults"}),
            json!({"kind": "set_agent_defaults", "defaults": {"limits": {}, "depth": 2}}),
            json!({"kind": "list_tasks", "device": null}),
            json!({"kind": "save_task", "device": mac, "task": task}),
            json!({"kind": "delete_task", "task": task["id"]}),
            json!({"kind": "run_task_now", "task": task["id"]}),
            json!({"kind": "set_hook_on", "task": task["id"], "on": true}),
            json!({"kind": "rotate_hook_token", "task": task["id"]}),
            json!({"kind": "set_hook_secret", "task": task["id"], "secret": "test-not-a-real-secret"}),
            json!({"kind": "set_hook_address", "device": mac, "address": {"kind": "local"}}),
            json!({"kind": "list_runs", "task": task["id"]}),
            json!({"kind": "list_deliveries", "task": task["id"]}),
            json!({"kind": "funnel_state", "device": mac}),
            json!({"kind": "check_test_hosts", "device": mac}),
            json!({"kind": "build_test_host", "device": mac, "host": TestHostId::derive("h")}),
            json!({"kind": "reset_test_host", "device": mac, "host": TestHostId::derive("h")}),
            json!({"kind": "end_lease", "device": mac, "lease": LeaseId::derive("l")}),
            json!({"kind": "read_lease_screen", "device": mac, "lease": LeaseId::derive("l")}),
            json!({
                "kind": "move_agent",
                "device": mac,
                "agent": a,
                "from": DeviceId::derive("nl"),
                "to": {"device": mac, "account": personal, "place": {"type": "folder", "path": "/srv/app"}},
                "stop": true,
            }),
            json!({
                "kind": "terminal_open",
                "agent": a,
                "runs": {"type": "run", "command": "pnpm test", "private": false},
                "cols": 80,
                "rows": 24,
            }),
            json!({"kind": "terminal_attach", "terminal": terminal, "after": 4096}),
            json!({"kind": "terminal_detach", "terminal": terminal}),
            json!({"kind": "terminal_resize", "terminal": terminal, "cols": 120, "rows": 32}),
            json!({"kind": "terminal_interrupt", "terminal": terminal}),
            json!({"kind": "terminal_close", "terminal": terminal}),
            json!({"kind": "terminal_rename", "terminal": terminal, "title": "dev server"}),
            json!({"kind": "terminal_move", "terminal": terminal}),
            json!({
                "kind": "list_folder",
                "device": mac,
                "path": "~/code",
                "files": false,
                "hidden": false,
                "filter": "ex",
            }),
            json!({"kind": "make_folder", "device": mac, "path": "/Users/dev/code/new"}),
            json!({
                "kind": "model_action",
                "device": mac,
                "action": {"action": "download", "model": "qwen3.6-35b-a3b", "file": "model-Q4.gguf"},
            }),
        ]
        .into_iter()
        .chain(skill_requests(mac))
        .chain(host_requests(mac, w, MoveId::derive("m")))
        .chain([
            json!({"kind": "forget_link_identity", "link": devbox}),
            json!({"kind": "remove_device", "link": devbox}),
        ])
        .collect()
    }

    /// One of each skills request, each to `device`. The proposal names
    /// one found there by `skill_proposal`.
    fn skill_requests(device: DeviceId) -> Vec<Value> {
        let source = SkillSourceId::derive("s");
        let lock = json!({"skills": [], "committed": false});
        vec![
            json!({"kind": "skill_source_add", "device": device, "name": "agent-skills",
                "origin": {"kind": "git", "url": "https://example.com/example/agent-skills"},
                "default_ref": null, "trust": "own"}),
            json!({"kind": "skill_source_fetch", "device": device, "source": source}),
            json!({"kind": "skill_source_remove", "device": device, "source": source}),
            json!({"kind": "skills_set_project", "device": device,
                "project": ProjectId::derive("p"), "lock": lock}),
            json!({"kind": "skills_apply", "device": device, "project": ProjectId::derive("p")}),
            json!({"kind": "skills_scan", "device": device,
                "place": {"type": "folder", "path": "/work/repo"}}),
            json!({"kind": "skills_trees_needed", "device": device, "lock": lock}),
            json!({"kind": "skill_proposal_action", "proposal": SkillProposalId::derive("sp"),
                "action": "approve", "push": true}),
            json!({"kind": "skill_proposal_diff", "proposal": SkillProposalId::derive("sp")}),
            json!({"kind": "skills_globals_plan", "device": device}),
            json!({"kind": "skills_globals_apply", "device": device, "plan_hash": "h"}),
            json!({"kind": "skills_globals_undo", "device": device}),
        ]
    }

    /// A proposal for `git-flow` found on `device`.
    fn skill_proposal(device: &str) -> Entity {
        Entity::SkillProposal(maxhq_protocol::skills::SkillProposal {
            id: SkillProposalId::derive("sp"),
            device: DeviceId::derive(device),
            project: ProjectId::derive("p"),
            agent: None,
            name: "git-flow".into(),
            base: None,
            tree: maxhq_protocol::skills::TreeHash("t".into()),
            checkout: "/work/repo".into(),
            created_at: Timestamp(1),
            flags: Default::default(),
            source: None,
        })
    }

    #[test]
    fn a_skills_request_goes_to_its_device_and_answers_with_its_outcome() {
        let (h, _, nl) = two_devices();
        let does = maxhq_protocol::message::Capabilities {
            skills: true,
            ..Default::default()
        };
        connect_doing(&h, &nl, "nl", does);
        h.report(
            nl.id,
            maxhq_client::LinkEvent::Synced(vec![skill_proposal("nl")]),
        );
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let device = DeviceId::derive("nl");
        let requests = skill_requests(device);
        let outcomes: Vec<ActionOutcome> = (0..requests.len())
            .map(|_| ActionOutcome::Refused {
                reason: "Skills aren't set up yet.".into(),
            })
            .collect();
        h.fake.record().acts.extend(outcomes.iter().cloned());
        drop(h.fake.calls());
        for request in requests {
            let request: Request = serde_json::from_value(request).unwrap();
            let kind = request.kind();
            let response = block_on(app.request(&caller, request)).unwrap();
            let value = serde_json::to_value(&response).unwrap();
            assert_eq!(value["kind"], kind);
            assert_eq!(value["value"]["reason"], "Skills aren't set up yet.");
        }
        let calls = h.fake.calls();
        let sent: Vec<_> = calls
            .iter()
            .map(|(link, call)| {
                assert_eq!(*link, nl.id);
                match call {
                    Call::Send(message) => message,
                    other => panic!("{other:?}"),
                }
            })
            .collect();
        assert_eq!(sent.len(), 12);
        // The proposal's answer reaches the device it was found on, and an
        // added source keeps its kind.
        assert!(sent.iter().any(|message| matches!(
            message,
            ClientMessage::SkillProposalAction {
                action: ProposalAction::Approve,
                push: true,
                ..
            }
        )));
        assert!(sent.iter().any(|message| matches!(message,
            ClientMessage::SkillSourceAdd { kind: SourceKind::Git { url }, .. }
                if url == "https://example.com/example/agent-skills")));
    }

    #[test]
    fn a_proposal_nobody_reported_is_not_found() {
        let (h, _, _) = two_devices();
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let answer = block_on(app.request(
            &caller,
            Request::SkillProposalAction {
                proposal: SkillProposalId::derive("gone"),
                action: ProposalAction::Reject,
                push: false,
                fork: None,
                seen: None,
            },
        ));
        assert!(
            matches!(answer, Err(CommandError::NotFound { .. })),
            "{answer:?}"
        );
        assert!(h.fake.calls().is_empty());
    }

    /// Has `git-flow` found on `nl` and the library on `local`, whose `maxd`
    /// works out targets when `targets` is set.
    fn proposal_away_from_the_library(
        h: &Harness,
        local: &maxhq_client::LinkSpec,
        nl: &maxhq_client::LinkSpec,
        targets: bool,
    ) {
        let does = maxhq_protocol::message::Capabilities {
            skills: true,
            copy: true,
            ..Default::default()
        };
        let holder = maxhq_protocol::message::Capabilities {
            proposal_targets: targets,
            ..does
        };
        connect_doing(h, local, "mac", holder);
        connect_doing(h, nl, "nl", does);
        let library = Entity::SkillLibrary(maxhq_protocol::skills::SkillLibrary {
            device: DeviceId::derive("mac"),
            sources: Vec::new(),
            always: Vec::new(),
            always_skills: Vec::new(),
        });
        h.report(local.id, maxhq_client::LinkEvent::Synced(vec![library]));
        h.report(
            nl.id,
            maxhq_client::LinkEvent::Synced(vec![skill_proposal("nl")]),
        );
    }

    fn a_diff(files: Vec<maxhq_protocol::pulls::PullFile>) -> ActionOutcome {
        ActionOutcome::SkillProposalDiff {
            diff: maxhq_protocol::skills::ProposalDiff {
                files,
                flags: Default::default(),
                tree: None,
                target: None,
            },
        }
    }

    #[test]
    fn the_diff_of_a_proposal_away_from_the_library_comes_from_the_holder() {
        let (h, local, nl) = two_devices();
        proposal_away_from_the_library(&h, &local, &nl, true);
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let from_holder = a_diff(vec![maxhq_protocol::pulls::PullFile {
            path: "SKILL.md".into(),
            status: maxhq_protocol::pulls::PullFileStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: None,
        }]);
        {
            let mut record = h.fake.record();
            record.acts.push_back(a_diff(Vec::new()));
            record.acts.push_back(from_holder.clone());
        }
        drop(h.fake.calls());
        let shown = block_on(app.request(
            &caller,
            Request::SkillProposalDiff {
                proposal: SkillProposalId::derive("sp"),
            },
        ))
        .unwrap();
        assert_eq!(shown, Response::SkillProposalDiff(from_holder));
        let calls = h.fake.calls();
        assert!(matches!(
            &calls[0],
            (link, Call::Send(ClientMessage::SkillProposalDiff { elsewhere: None, .. }))
                if *link == nl.id
        ));
        assert!(matches!(
            calls.last(),
            Some((link, Call::Send(ClientMessage::SkillProposalDiff { elsewhere: Some(found), .. })))
                if *link == local.id && found.name == "git-flow"
        ));
        // The tree reached the holder between the two.
        assert!(
            calls.iter().any(|(link, call)| *link == local.id
                && matches!(
                    call,
                    Call::Upload(maxhq_protocol::message::UploadTarget::SkillStore { .. }, _)
                )),
            "{calls:?}"
        );
    }

    #[test]
    fn a_holder_from_before_leaves_a_proposals_diff_to_its_device() {
        let (h, local, nl) = two_devices();
        proposal_away_from_the_library(&h, &local, &nl, false);
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        h.fake.record().acts.push_back(a_diff(Vec::new()));
        drop(h.fake.calls());
        let shown = block_on(app.request(
            &caller,
            Request::SkillProposalDiff {
                proposal: SkillProposalId::derive("sp"),
            },
        ))
        .unwrap();
        assert_eq!(shown, Response::SkillProposalDiff(a_diff(Vec::new())));
        let calls = h.fake.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(matches!(
            &calls[0],
            (link, Call::Send(ClientMessage::SkillProposalDiff { elsewhere: None, .. }))
                if *link == nl.id
        ));
    }

    #[test]
    fn approving_a_proposal_away_from_the_library_brings_its_tree_first() {
        let (h, local, nl) = two_devices();
        let does = maxhq_protocol::message::Capabilities {
            skills: true,
            copy: true,
            ..Default::default()
        };
        connect_doing(&h, &local, "mac", does);
        connect_doing(&h, &nl, "nl", does);
        let library = Entity::SkillLibrary(maxhq_protocol::skills::SkillLibrary {
            device: DeviceId::derive("mac"),
            sources: Vec::new(),
            always: Vec::new(),
            always_skills: Vec::new(),
        });
        h.report(local.id, maxhq_client::LinkEvent::Synced(vec![library]));
        h.report(
            nl.id,
            maxhq_client::LinkEvent::Synced(vec![skill_proposal("nl")]),
        );
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        h.fake
            .record()
            .acts
            .push_back(ActionOutcome::SkillProposalDiff {
                diff: maxhq_protocol::skills::ProposalDiff {
                    files: Vec::new(),
                    flags: Default::default(),
                    tree: None,
                    target: None,
                },
            });
        drop(h.fake.calls());
        let seen = ProposalSeen {
            tree: maxhq_protocol::skills::TreeHash("t".into()),
            base: None,
        };
        let answer = block_on(app.request(
            &caller,
            Request::SkillProposalAction {
                proposal: SkillProposalId::derive("sp"),
                action: ProposalAction::Approve,
                push: false,
                fork: None,
                seen: Some(seen.clone()),
            },
        ))
        .unwrap();
        // The device it was found on keeps its tree in its store first, and
        // the holder only hears of it once the tree is there. The device it
        // was found on then drops it.
        let calls = h.fake.calls();
        assert!(matches!(
            &calls[0],
            (link, Call::Send(ClientMessage::SkillProposalDiff { .. })) if *link == nl.id
        ));
        let tree = maxhq_protocol::skills::TreeHash("t".into());
        assert_eq!(calls[1], (nl.id, Call::CopyTree(tree)), "{calls:?}");
        assert!(
            matches!(
                &calls[2],
                (link, Call::Upload(maxhq_protocol::message::UploadTarget::SkillStore { .. }, _)) if *link == local.id
            ),
            "{calls:?}"
        );
        assert!(matches!(
            &calls[3],
            (link, Call::Send(ClientMessage::SkillProposalAction {
                action: ProposalAction::Approve,
                elsewhere: Some(_),
                seen: Some(sent),
                ..
            })) if *link == local.id && *sent == seen
        ));
        assert!(matches!(
            &calls[4],
            (link, Call::Send(ClientMessage::SkillProposalAction {
                action: ProposalAction::Reject,
                ..
            })) if *link == nl.id
        ));
        assert_eq!(calls.len(), 5, "{calls:?}");
        assert_eq!(answer, Response::SkillProposalAction(ActionOutcome::Done));
    }

    #[test]
    fn the_apns_key_goes_only_to_devices_that_push_to_phones() {
        let (h, local, nl) = two_devices();
        let pushes = maxhq_protocol::message::Capabilities {
            phones: true,
            ..Default::default()
        };
        connect_doing(&h, &local, "mac", pushes);
        connect_doing(&h, &nl, "nl", Default::default());
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        const P8: &str = "-----BEGIN PRIVATE KEY-----\ntest-not-a-real-key\n\
                          -----END PRIVATE KEY-----\n";
        let file = TempFile::new("AuthKey_ABC123DEFG.p8", P8.as_bytes());
        let ask = |key_id: &str, team_id: &str| {
            let request = Request::SetApnsKey {
                devices: vec![DeviceId::derive("mac"), DeviceId::derive("nl")],
                key_id: key_id.into(),
                team_id: team_id.into(),
                key_path: file.0.to_string_lossy().into_owned(),
            };
            let Ok(Response::SetApnsKey(replies)) = block_on(app.request(&caller, request)) else {
                panic!("a reply for each device");
            };
            replies
        };

        // Nothing goes out for a file you didn't pick, or a blank ID.
        let not_picked = ask("ABC123DEFG", "<TEAM_ID>");
        assert!(matches!(
            &not_picked[..],
            [ActionReply::Error { .. }, ActionReply::Error { .. }]
        ));
        app.offered().offer([file.0.clone()]);
        for (key_id, team_id) in [(" ", "<TEAM_ID>"), ("ABC123DEFG", "")] {
            let replies = ask(key_id, team_id);
            assert!(matches!(
                &replies[..],
                [ActionReply::Error { .. }, ActionReply::Error { .. }]
            ));
        }
        assert!(h.fake.calls().is_empty());

        h.fake.record().acts.push_back(ActionOutcome::Done);
        let replies = ask(" ABC123DEFG ", " <TEAM_ID> ");
        assert_eq!(
            replies[0],
            ActionReply::Ok {
                data: ActionOutcome::Done
            }
        );
        let ActionReply::Error {
            error: CommandError::Failed { message },
        } = &replies[1]
        else {
            panic!("{replies:?}");
        };
        assert!(message.ends_with("maxd is too old to push to phones. Update it first"));
        assert!(!format!("{replies:?}").contains("test-not-a-real-key"));
        let calls = h.fake.calls();
        let [(
            link,
            Call::Send(ClientMessage::SetApnsKey {
                key_id,
                team_id,
                private_key,
                ..
            }),
        )] = &calls[..]
        else {
            panic!("{calls:?}");
        };
        assert_eq!(*link, local.id);
        assert_eq!(
            (key_id.as_str(), team_id.as_str()),
            ("ABC123DEFG", "<TEAM_ID>")
        );
        assert_eq!(private_key.expose(), P8);
    }

    #[test]
    fn a_maxd_too_old_for_fleet_tools_is_refused_in_words() {
        let (h, _, nl) = two_devices();
        connect_doing(
            &h,
            &nl,
            "nl",
            maxhq_protocol::message::Capabilities::default(),
        );
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let device = DeviceId::derive("nl");
        let ask = |request| block_on(app.request(&caller, request));
        let too_old = |result: Result<Response, CommandError>, what: &str| match result {
            Err(CommandError::Failed { message }) => {
                assert!(message.ends_with(&format!("maxd is too old to {what}. Update it first")))
            }
            other => panic!("{other:?}"),
        };
        too_old(
            ask(Request::ReadDiagnostics {
                device,
                window_secs: 3600,
                bucket_secs: 60,
            }),
            "report its load",
        );
        too_old(
            ask(Request::UpdateTools {
                device,
                tools: Vec::new(),
            }),
            "update its agent CLIs",
        );
        too_old(
            ask(Request::SkillsScan {
                device,
                place: AgentPlace::Folder {
                    path: "/work/repo".into(),
                },
            }),
            "keep skills",
        );
        let Ok(Response::ReadLoad(replies)) = ask(Request::ReadLoad {
            devices: vec![device],
        }) else {
            panic!("read_load answers in its kind");
        };
        assert!(matches!(replies[..], [DeviceLoad { load: None, .. }]));
        for request in test_host_requests(device) {
            too_old(ask(request), "offer test hosts");
        }
        let sent = h.fake.calls();
        assert!(
            !sent.iter().any(|(_, call)| matches!(call, Call::Send(_))),
            "sent {sent:?}"
        );
    }

    /// One of each test host request, all for `device`.
    fn test_host_requests(device: DeviceId) -> [Request; 5] {
        let (host, lease) = (TestHostId::derive("h"), LeaseId::derive("l"));
        [
            Request::CheckTestHosts { device },
            Request::BuildTestHost { device, host },
            Request::ResetTestHost { device, host },
            Request::EndLease { device, lease },
            Request::ReadLeaseScreen { device, lease },
        ]
    }

    #[test]
    fn a_test_host_request_goes_to_its_device_and_answers_with_its_outcome() {
        let (h, _, nl) = two_devices();
        let does = maxhq_protocol::message::Capabilities {
            test_hosts: true,
            ..Default::default()
        };
        connect_doing(&h, &nl, "nl", does);
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let screen = ActionOutcome::LeaseScreen {
            image: maxhq_protocol::agents::ToolImage {
                mime: "image/png".into(),
                data: "aGk=".into(),
            },
            at: Timestamp(1),
        };
        let refused = ActionOutcome::Refused {
            reason: "Test hosts aren't set up yet.".into(),
        };
        let outcomes = [
            ActionOutcome::Done,
            refused,
            ActionOutcome::Done,
            ActionOutcome::Done,
            screen,
        ];
        h.fake.record().acts.extend(outcomes.iter().cloned());
        drop(h.fake.calls());
        let device = DeviceId::derive("nl");
        let mut answered = Vec::new();
        for request in test_host_requests(device) {
            let response = block_on(app.request(&caller, request)).unwrap();
            answered.push(serde_json::to_value(&response).unwrap()["value"].clone());
        }
        let expected: Vec<Value> = outcomes
            .iter()
            .map(|outcome| serde_json::to_value(outcome).unwrap())
            .collect();
        assert_eq!(answered, expected);
        // The screenshot reaches the window in the shape the views read.
        assert_eq!(
            answered[4],
            json!({"status": "lease_screen", "image": {"mime": "image/png", "data": "aGk="}, "at": 1})
        );

        let (host, lease) = (TestHostId::derive("h"), LeaseId::derive("l"));
        let calls = h.fake.calls();
        let sent: Vec<_> = calls
            .iter()
            .map(|(link, call)| {
                assert_eq!(*link, nl.id);
                match call {
                    Call::Send(message) => message,
                    other => panic!("{other:?}"),
                }
            })
            .collect();
        let [ClientMessage::CheckTestHosts { .. }, ClientMessage::BuildTestHost { host: built, .. }, ClientMessage::ResetTestHost { host: reset, .. }, ClientMessage::EndLease { lease: ended, .. }, ClientMessage::ReadLeaseScreen { lease: read, .. }] =
            sent.as_slice()
        else {
            panic!("sent {sent:?}");
        };
        assert_eq!((*built, *reset, *ended, *read), (host, host, lease, lease));
    }

    /// One of each host, tool, snapshot and move request, all for `device`,
    /// its workspace `workspace` and its move `moving`.
    fn host_requests(device: DeviceId, workspace: WorkspaceId, moving: MoveId) -> Vec<Value> {
        let host = json!({"type": "host"});
        let node = json!({"type": "helper", "tool": "node"});
        vec![
            json!({"kind": "check_host", "device": device}),
            json!({"kind": "apply_host", "device": device, "items": ["caps.parent"]}),
            json!({"kind": "install_tool", "device": device, "place": host, "tool": node, "version": null}),
            json!({"kind": "remove_tool", "device": device, "place": host, "tool": node}),
            json!({"kind": "list_snapshots", "workspace": workspace}),
            json!({"kind": "restore_snapshot", "workspace": workspace, "snapshot": "snap-1", "as_new": true, "discard_unsaved": false}),
            json!({"kind": "plan_move", "device": device, "instance": "ws-alpha"}),
            json!({"kind": "run_move_step", "moving": moving, "step": "copy"}),
            json!({"kind": "move_back", "moving": moving}),
            json!({"kind": "retire_move", "moving": moving}),
            json!({"kind": "install_acp_agent", "device": device, "agent": "gemini", "version": "0.9.0",
                "place": host, "confirm_unchecked": false}),
            json!({"kind": "accept_licence", "device": device, "licence": "android-sdk-license",
                "text_sha256": "0".repeat(64)}),
        ]
    }

    /// A move on `device`, at its copy.
    fn moving(device: &str) -> Entity {
        Entity::Move(maxhq_protocol::hosts::Move {
            id: MoveId::derive("m"),
            device: DeviceId::derive(device),
            source: "ws-alpha".into(),
            target: "mhq-alpha".into(),
            workspace: None,
            step: MoveStep::Copy,
            state: maxhq_protocol::hosts::MoveState::Ready,
            plan: None,
            source_changed: false,
            kept_until: None,
            unsaved: Vec::new(),
            checklist: Vec::new(),
        })
    }

    /// Asks devbox, which has a move and does `does`, each host request,
    /// with `outcomes` as its answers. Gives back the answers and what was
    /// sent.
    fn ask_hosts(
        does: maxhq_protocol::message::Capabilities,
        outcomes: Vec<ActionOutcome>,
    ) -> (Vec<Result<Response, CommandError>>, Vec<ClientMessage>) {
        let (h, _, nl) = two_devices();
        let value = moving("nl");
        h.report(
            nl.id,
            maxhq_client::LinkEvent::Changed(vec![maxhq_protocol::state::Change::Upsert { value }]),
        );
        connect_doing(&h, &nl, "nl", does);
        h.fake.record().acts.extend(outcomes);
        drop(h.fake.calls());
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let samples = host_requests(
            DeviceId::derive("nl"),
            WorkspaceId::derive("w-nl"),
            MoveId::derive("m"),
        );
        let answers = samples
            .into_iter()
            .map(|sample| {
                let request: Request = serde_json::from_value(sample).unwrap();
                let kind = request.kind();
                let answer = block_on(app.request(&caller, request));
                if let Ok(response) = &answer {
                    assert_eq!(response.kind(), kind);
                }
                answer
            })
            .collect();
        let sent = h
            .fake
            .calls()
            .into_iter()
            .map(|(link, call)| {
                assert_eq!(link, nl.id);
                match call {
                    Call::Send(message) => message,
                    other => panic!("{other:?}"),
                }
            })
            .collect();
        (answers, sent)
    }

    #[test]
    fn a_host_request_to_a_maxd_too_old_for_it_is_refused_in_words() {
        let (answers, sent) = ask_hosts(Default::default(), Vec::new());
        let what = [
            "check its host",
            "check its host",
            "install tools",
            "install tools",
            "keep snapshots",
            "keep snapshots",
            "move workspaces",
            "move workspaces",
            "move workspaces",
            "move workspaces",
            "install tools",
            "install tools",
        ];
        assert_eq!(answers.len(), what.len());
        for (answer, what) in answers.into_iter().zip(what) {
            match answer {
                Err(CommandError::Failed { message }) => assert_eq!(
                    message,
                    format!("devbox's maxd is too old to {what}. Update it first")
                ),
                other => panic!("{other:?}"),
            }
        }
        assert!(sent.is_empty(), "sent {sent:?}");
    }

    #[test]
    fn a_host_request_goes_to_its_device_and_answers_with_its_outcome() {
        let does = maxhq_protocol::message::Capabilities {
            host_spec: true,
            snapshots: true,
            moves: true,
            tool_installs: true,
            ..Default::default()
        };
        let listed = ActionOutcome::Snapshots {
            snapshots: Vec::new(),
        };
        let mut outcomes = vec![ActionOutcome::Done; 12];
        outcomes[4] = listed.clone();
        let (answers, sent) = ask_hosts(does, outcomes);
        assert_eq!(
            answers[4].as_ref().unwrap(),
            &Response::ListSnapshots(listed)
        );
        assert!(answers.iter().all(Result::is_ok), "{answers:?}");
        assert_eq!(sent.len(), 12, "{sent:?}");
        assert!(matches!(
            &sent[4],
            ClientMessage::ListSnapshots { workspace, .. } if *workspace == WorkspaceId::derive("w-nl")
        ));
        assert!(matches!(
            &sent[7],
            ClientMessage::RunMoveStep {
                step: MoveStep::Copy,
                ..
            }
        ));
        assert!(matches!(
            &sent[10],
            ClientMessage::InstallAcpAgent {
                confirm_unchecked: false,
                ..
            }
        ));
    }

    #[test]
    fn a_move_the_app_doesnt_know_is_not_found() {
        let (h, _, _) = two_devices();
        let app = App::new(h.view.clone(), h.devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let moving = MoveId::derive("gone");
        let answer = block_on(app.request(&caller, Request::MoveBack { moving }));
        assert!(matches!(answer, Err(CommandError::NotFound { what }) if what == "This move"));
        assert!(h.fake.calls().is_empty());
    }

    #[test]
    fn every_request_gets_an_answer_of_its_kind() {
        let (h, local, devbox) = two_devices();
        crate::tasks::tests::publish(
            &h,
            &local,
            DeviceTasks {
                device: DeviceId::derive("mac"),
                tasks: vec![crate::tasks::tests::task("t", "mac")],
                address: HookAddress::Local,
                port: None,
                funnel: FunnelState::Off,
                problems: Vec::new(),
                webhooks: Vec::new(),
            },
        );
        let hidden = Entity::HiddenProjects(HiddenProjects {
            device: DeviceId::derive("mac"),
            projects: Vec::new(),
        });
        let shell = Entity::Terminal(maxhq_protocol::terminal::Terminal {
            id: TerminalId::derive("terminal"),
            agent: AgentId::derive("a"),
            device: DeviceId::derive("mac"),
            kind: TerminalKind::Shell,
            moved: false,
            title: "zsh".into(),
            cwd: "~/code/example-app".into(),
            status: maxhq_protocol::terminal::TerminalStatus::Running,
            busy: false,
            marks: false,
            password_prompt: false,
            started_at: Timestamp(1_793_368_920_000),
            cols: 80,
            rows: 24,
        });
        for value in [moving("mac"), hidden, shell] {
            h.report(
                local.id,
                maxhq_client::LinkEvent::Changed(vec![maxhq_protocol::state::Change::Upsert {
                    value,
                }]),
            );
        }
        let Harness { devices, view, .. } = h;
        #[cfg_attr(not(feature = "desktop"), allow(unused_mut))]
        let mut app = App::new(view, devices, FakeShell::default());
        #[cfg(feature = "desktop")]
        let editors = Scratch::new();
        #[cfg(feature = "desktop")]
        {
            editors.program("bin/cursor", true);
            app.editors = Installed::new(editors.places());
        }
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let mut kinds = BTreeSet::new();
        let mut failed = BTreeSet::new();
        let mut mac_only = BTreeSet::new();
        for sample in one_of_each(devbox.id) {
            // The phone build has none of the Mac's own requests.
            let Ok(request) = serde_json::from_value::<Request>(sample.clone()) else {
                if cfg!(feature = "desktop") {
                    panic!("{sample} doesn't parse");
                }
                mac_only.insert(sample["kind"].as_str().unwrap().to_owned());
                continue;
            };
            let kind = request.kind();
            assert_eq!(sample["kind"], kind);
            assert!(kinds.insert(kind), "{kind} comes twice");
            match block_on(app.request(&caller, request)) {
                Ok(response) => {
                    assert_eq!(response.kind(), kind);
                    assert_eq!(serde_json::to_value(&response).unwrap()["kind"], kind);
                }
                Err(_) => {
                    failed.insert(kind);
                }
            }
        }
        assert_eq!(kinds.len() + mac_only.len(), 161);
        // The harness has no maxd to install or uninstall, no timeline open, no
        // listening port, no setup waiting on a host key, no GitHub
        // sign-in to merge with, no skill proposal, no link or card waiting
        // on you, and its connector runs no commands over SSH.
        let mut expected = BTreeSet::from([
            "answer_link_claim",
            "ask_link_claim_again",
            "load_older",
            "open_forward",
            "pull_action",
            "skill_proposal_action",
            "skill_proposal_diff",
        ]);
        if cfg!(feature = "desktop") {
            expected.extend([
                "answer_admission",
                "confirm_host_key",
                "link_claim_details",
                "restart_maxd",
                "start_local_maxd",
                "uninstall_local_maxd",
            ]);
        }
        assert_eq!(failed, expected);
    }

    #[test]
    fn a_message_takes_its_pills_to_the_device() {
        let (
            Harness {
                devices,
                view,
                fake,
                ..
            },
            local,
            _,
        ) = two_devices();
        let app = App::new(view, devices, FakeShell::default());
        let caller = Caller {
            window: "main".into(),
            on_screen: true,
        };
        let shot = AttachmentId::derive("shot");
        let refs = json!([
            {"type": "attachment", "id": shot},
            {
                "type": "git_item",
                "repo": "owner/repo",
                "number": 123,
                "kind": "pull_request",
                "title": "Fix cache expiry",
                "state": "open",
                "url": "https://github.com/owner/repo/pull/123",
                "head": "fix/cache",
                "base": "main",
                "body": "Closes #45",
                "checked_at": 1_761_834_131_000u64,
            },
        ]);
        let request: Request = serde_json::from_value(json!({
            "kind": "send_input",
            "agent": AgentId::derive("a"),
            "input": {"text": "see \u{FFFC} and #123", "attachments": [shot], "refs": refs},
            "mode": "queue",
        }))
        .unwrap();
        block_on(app.request(&caller, request)).unwrap();
        let calls = fake.calls();
        let [(link, Call::Send(ClientMessage::SendInput { input, .. }))] = calls.as_slice() else {
            panic!("sent {calls:?}");
        };
        assert_eq!(*link, local.id);
        assert_eq!(input.text, "see \u{FFFC} and #123");
        assert_eq!(serde_json::to_value(&input.refs).unwrap(), refs);
    }

    #[test]
    fn a_lookup_the_device_refused_fails_with_its_reason() {
        let reason = "That agent's folder is on another device.";
        let refused = ActionOutcome::Refused {
            reason: reason.to_owned(),
        };
        assert_eq!(lookup_failed(refused), failed(reason));
        assert_eq!(
            lookup_failed(ActionOutcome::UnknownAgent).to_string(),
            "This agent is gone"
        );
        assert!(matches!(
            lookup_failed(ActionOutcome::Done),
            CommandError::Failed { .. }
        ));
    }

    #[test]
    fn app_info_reports_this_build() {
        let info = app_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.protocol_version, maxhq_protocol::PROTOCOL_VERSION);
    }
}
