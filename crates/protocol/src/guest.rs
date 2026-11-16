// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The link between host `maxd` and `maxd guest` in a container workspace.
//!
//! These types are Rust only, and stay out of the schema and the bindings.
//! The host opens the link through the container's Incus proxy device, per
//! A-2. The guest has no socket into the host. The frames are the client
//! link's, with kinds 0, 3 and 4 only. Host and guest must match
//! [`GUEST_PROTOCOL_VERSION`] exactly, since the host installs its own
//! build in the guest.
//!
//! Two facts about the proxy shape the link. The guest sees the host's
//! connection as uid 0 and pid 0, so it can't check its peer, and only root
//! reaches [`HOST_SOCKET`]. The proxy also drops a half-close, so no request
//! ever ends by shutting a write side. Every message is a whole frame.
//!
//! The host treats everything from the guest as untrusted. A message body
//! is at most 4 MiB, except a holder line at 16 MiB. At most 64 holder
//! connections, 32 histories and 1,024 tunnels are open at once. A session
//! gets at most 30 token requests a minute. A port list holds at most 512
//! entries. Any breach, bad JSON or unknown kind drops the link, and the
//! host reconnects with a backoff from 0.5 to 10 s. The host never uses a
//! string from the guest as a host path.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::agents::ToolImage;
use crate::find::PathMatch;
use crate::frame::{DecodeError, Frame, FrameDecoder, MAX_FRAME_LEN};
use crate::ids::{
    AgentId, BootId, DeviceId, LeaseId, SessionId, StreamId, TransferId, WorkspaceId,
};
use crate::leases::RunOutput;
use crate::message::{ErrorCode, IgnoredFile, ManifestEntry, PlaceFailure, TunnelEnd};
use crate::model::{BindAddress, PortProcess, Vendor};
use crate::processes::{AgentProcess, ProcessSignal};
use crate::secret::Secret;
use crate::skills::{PackState, TreeHash};
use crate::time::Timestamp;
use crate::timeline::TimelineItem;
use crate::workflow::{NodeDetail, RunSnapshot};

/// The guest link version. Host and guest must match it exactly.
pub const GUEST_PROTOCOL_VERSION: u32 = 11;

/// The most bytes a message body from the guest may take, 4 MiB.
pub const MAX_MESSAGE_LEN: usize = 4 * 1024 * 1024;

/// The most bytes a message carrying one holder line may take, as a line
/// or as a hello part with one pending line. That's the most a frame may
/// hold.
pub const MAX_LINE_MESSAGE_LEN: usize = MAX_FRAME_LEN;

/// The most base64 a [`HostToGuest::KeepPicture`] carries, 8 MiB, which
/// leaves room in a frame for the rest of the message.
pub const MAX_PICTURE_LEN: usize = 8 * 1024 * 1024;

/// The most holder connections open on one link at once.
pub const MAX_HOLDER_CONNECTIONS: usize = 64;

/// The most history readers open on one link at once.
pub const MAX_HISTORIES: usize = 32;

/// The most tunnel streams open on one link at once.
pub const MAX_TUNNELS: usize = 1024;

/// The most public keys an SSH session with a guest may sign in with.
pub const MAX_SSH_KEYS: usize = 64;

/// The most token requests one session may make in a minute.
pub const MAX_TOKEN_REQUESTS_PER_MINUTE: u32 = 30;

/// The most entries one port list may hold.
pub const MAX_PORTS: usize = 512;

/// The host's first wait before it reconnects a dropped link.
pub const RECONNECT_MIN: Duration = Duration::from_millis(500);

/// The host's longest wait between reconnects. The wait doubles up to it.
pub const RECONNECT_MAX: Duration = Duration::from_secs(10);

/// The socket in the container that the proxy device connects to. Root
/// owned, mode 0600.
pub const HOST_SOCKET: &str = "/run/maxhq/host.sock";

/// The socket in the container that the token and credential helpers call.
/// Mode 0666, and every caller is checked with SO_PEERCRED.
pub const AGENT_SOCKET: &str = "/run/maxhq/agent.sock";

/// Where holders in the container keep their records and logs.
pub const HOLD_DIR: &str = "/var/lib/maxhq/hold";

/// Where each agent's attachments land in the container, one folder per
/// agent, per A-15.
pub const ATTACHMENTS_DIR: &str = "/var/lib/maxhq/attachments";

/// Where the guest keeps each agent's A-10 record, as `<agent>.json`.
pub const SESSIONS_DIR: &str = "/var/lib/maxhq/sessions";

/// From host `maxd` to `maxd guest`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToGuest {
    /// The host's half of the handshake, sent first.
    Hello {
        /// [`GUEST_PROTOCOL_VERSION`] on the host.
        protocol: u32,
        /// The host's `maxd` version.
        maxd_version: String,
        /// The host device.
        device: DeviceId,
        /// The workspace this container is.
        workspace: WorkspaceId,
        /// The repository the workspace checks out, the only one its
        /// credential helper serves.
        repo: Option<String>,
    },
    /// Starts a holder for a CLI.
    Spawn {
        /// Matches the [`GuestToHost::SpawnResult`].
        req: u64,
        /// What to run.
        launch: GuestLaunch,
    },
    /// Connects to a holder. Lines after `after` replay first, as
    /// `maxhq_hold::connect` does.
    Connect {
        /// Names the connection in later messages, chosen by the host.
        conn: u64,
        /// The holder's session.
        session: SessionId,
        /// The last line the host has, if it has any.
        after: Option<u64>,
    },
    /// Writes a line to a connected holder's CLI.
    Write {
        /// Matches the [`GuestToHost::WriteResult`].
        req: u64,
        /// The connection.
        conn: u64,
        /// The line, without its newline.
        line: String,
        /// The prompt the line answers, when it answers one.
        answers: Option<String>,
    },
    /// Stops a connected holder's CLI, with SIGKILL after the grace.
    Stop {
        /// Matches the [`GuestToHost::Done`].
        req: u64,
        /// The connection.
        conn: u64,
        /// How long the CLI gets after SIGTERM.
        grace_ms: u64,
    },
    /// Removes a finished session's files.
    Forget {
        /// Matches the [`GuestToHost::Done`].
        req: u64,
        /// The session.
        session: SessionId,
    },
    /// Makes sure nothing still runs for a session whose holder the host
    /// can't use, as `maxhq_hold::Holds::end` does in the guest.
    End {
        /// Matches the [`GuestToHost::EndResult`].
        req: u64,
        /// The session.
        session: SessionId,
        /// The holder's process ID, as its last hello gave it.
        holder_pid: Option<u32>,
    },
    /// Closes a holder connection. The holder keeps running.
    Disconnect {
        /// The connection.
        conn: u64,
    },
    /// Asks which sessions have holders in the container.
    ListHolders {
        /// Matches the [`GuestToHost::Holders`].
        req: u64,
    },
    /// W-4. `git worktree add` as user `agent`.
    PrepareWorktree {
        /// Matches the [`GuestToHost::WorktreeReady`].
        req: u64,
        /// The agent the worktree is for.
        agent: AgentId,
        /// The branch to make.
        branch: String,
        /// What the branch starts from, or the default branch when unset.
        base: Option<String>,
    },
    /// Looks for files and folders under `cwd` for the composer's `@`
    /// menu, the way the host looks in its own folders.
    FindPaths {
        /// Matches the [`GuestToHost::FoundPaths`].
        req: u64,
        /// The agent's folder, inside the container.
        cwd: String,
        /// What you typed after `@`.
        query: String,
    },
    /// Lists the processes in the tree under a holder, the way the guest
    /// looks for its ports. New in guest protocol 10.
    ListProcesses {
        /// Matches the [`GuestToHost::Processes`].
        req: u64,
        /// The holder.
        holder: WatchedHolder,
    },
    /// Signals one process in a holder's tree, after listing the tree
    /// again: only a process in it now, with the same `pid` and `start`,
    /// and never the CLI. New in guest protocol 10.
    SignalProcess {
        /// Matches the [`GuestToHost::Signalled`].
        req: u64,
        /// The holder.
        holder: WatchedHolder,
        /// The process, as the container sees it.
        pid: u32,
        /// When it started, from [`AgentProcess::start`].
        start: u64,
        /// The signal.
        signal: ProcessSignal,
    },
    /// Lists the files git ignores in the checkout at `cwd`, as user
    /// `agent`.
    ListIgnored {
        /// Matches the [`GuestToHost::Ignored`].
        req: u64,
        /// The checkout, inside the container.
        cwd: String,
    },
    /// Moves a staged upload's files into the checkout at `cwd`, keeping
    /// their paths, and gives them to user `agent`. The staged upload goes
    /// away after.
    Place {
        /// Matches the [`GuestToHost::Placed`].
        req: u64,
        /// The upload that landed in the container's staging folder.
        staged: TransferId,
        /// The checkout, inside the container.
        cwd: String,
        /// Whether to replace files already there.
        overwrite: bool,
    },
    /// Runs the vendor's history reader in the guest, for a held session.
    OpenHistory {
        /// Names the reader, and matches each [`GuestToHost::HistoryItems`].
        req: u64,
        /// Whose history format to read.
        vendor: Vendor,
        /// The account's config folder inside the container.
        config_dir: String,
        /// The Claude session UUID or the Codex thread ID.
        session: String,
        /// The agent the items belong to.
        agent: AgentId,
        /// The most bytes of the transcript's end the first read takes,
        /// from the host's history limit, with `u64::MAX` for all of it.
        /// None, from a host before it, leaves the reader's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
    /// Stops a history reader.
    CloseHistory {
        /// The [`HostToGuest::OpenHistory`] it opened with.
        req: u64,
    },
    /// Answers a [`GuestToHost::TokenRequest`].
    TokenReply {
        /// The request's `req`.
        req: u64,
        /// The token, or why not.
        reply: TokenReply,
    },
    /// Starts an upload into the container. Chunks follow in transfer
    /// frames.
    UploadBegin {
        /// The upload, as the client named it.
        transfer: TransferId,
        /// The agent whose attachments folder it lands in, or the
        /// container's staging folder when unset.
        agent: Option<AgentId>,
        /// Every file, folder and link, folders before what's in them.
        entries: Vec<ManifestEntry>,
    },
    /// Gives up on an upload, and removes what landed.
    UploadCancel {
        /// The upload.
        transfer: TransferId,
    },
    /// Starts a download from the container.
    FetchBegin {
        /// The download, as the client named it.
        transfer: TransferId,
        /// The path inside the container.
        path: String,
    },
    /// Starts a download of some files in the checkout at `cwd`, which
    /// answers as [`HostToGuest::FetchBegin`] does.
    FetchFiles {
        /// The download, as the client named it.
        transfer: TransferId,
        /// The checkout, inside the container.
        cwd: String,
        /// Regular files, relative to `cwd`.
        paths: Vec<String>,
    },
    /// Credit for a download, passed on from the client.
    TransferCredit {
        /// The download.
        transfer: TransferId,
        /// Bytes the client has written down for good.
        committed: u64,
        /// The guest may send up to this offset.
        window_end: u64,
    },
    /// Keeps a picture from an agent's tool result in the agent's
    /// attachments folder, named for its bytes, where the host's timeline
    /// says it is. The guest writes it as its own history reads do, and
    /// says nothing back.
    KeepPicture {
        /// The agent whose folder it goes in.
        agent: AgentId,
        /// Its media type, such as `image/png`.
        media_type: String,
        /// The picture, base64 encoded as the tool gave it, at most
        /// [`MAX_PICTURE_LEN`].
        data: String,
    },
    /// N-6. Fast is every 2 s, slow every 30 s. Replaces the last watch.
    WatchPorts {
        /// How often to look.
        pace: Pace,
        /// The holders whose process trees to look in.
        holders: Vec<WatchedHolder>,
    },
    /// Opens a stream into the container: a connection to a port on
    /// loopback, or an SSH session with the guest itself.
    TunnelOpen {
        /// Names the stream, and its bytes in tunnel frames.
        stream: StreamId,
        /// What the stream reaches.
        target: TunnelTarget,
    },
    /// Lets the guest send more of a stream's bytes.
    TunnelCredit {
        /// The stream.
        stream: StreamId,
        /// More bytes the guest may send.
        granted: u64,
    },
    /// Ends a stream, or the host's half of it.
    TunnelClose {
        /// The stream.
        stream: StreamId,
        /// How it ends.
        how: TunnelEnd,
    },
    /// The public keys an SSH session with the guest may sign in with,
    /// each as an OpenSSH `ssh-ed25519` line. Replaces the last list. At
    /// most [`MAX_SSH_KEYS`].
    SshAuthorized {
        /// The keys.
        keys: Vec<String>,
    },
    /// Checks the link. The guest answers with [`GuestToHost::Pong`].
    Ping {
        /// Echoed back.
        nonce: u64,
    },
    /// Runs one command for a lease in a guest that's a test host. The
    /// guest answers with [`GuestToHost::LeaseRan`].
    LeaseRun {
        /// Matches the answer.
        req: u64,
        /// The lease.
        lease: LeaseId,
        /// The program and its arguments, already checked by the host.
        command: Vec<String>,
        /// How long it may run before the guest stops it.
        timeout_seconds: u32,
    },
    /// Takes a screenshot for a lease. The guest answers with
    /// [`GuestToHost::LeaseScreen`].
    LeaseScreenshot {
        /// Matches the answer.
        req: u64,
        /// The lease.
        lease: LeaseId,
    },
    /// Installs an app the host uploaded for a lease. The guest answers
    /// with [`GuestToHost::LeaseDone`].
    LeaseInstall {
        /// Matches the answer.
        req: u64,
        /// The lease.
        lease: LeaseId,
        /// Where the upload landed, inside the guest.
        path: String,
    },
    /// Closes a lease's session in the guest. The guest answers with
    /// [`GuestToHost::LeaseDone`].
    LeaseClose {
        /// Matches the answer.
        req: u64,
        /// The lease.
        lease: LeaseId,
    },
    /// Links a checkout's skills to the host's read-only skills mount. The
    /// guest answers with [`GuestToHost::SkillLinksDone`].
    SkillLinks {
        /// Matches the answer.
        req: u64,
        /// The checkout's absolute path inside the guest.
        checkout: String,
        /// The skills to link, by name.
        names: Vec<String>,
        /// The skills every agent gets, by name, to link into each of
        /// `homes`.
        #[serde(default)]
        always: Vec<String>,
        /// The global skills folders the always set is linked into, such
        /// as an account's `skills` folder.
        #[serde(default)]
        homes: Vec<String>,
        /// The devflow pack's tree, which the mount holds at
        /// `.devflow-vendor`, for the guest to copy into the checkout and
        /// install. None takes MaxHQ's copy out.
        #[serde(default)]
        pack: Option<TreeHash>,
        /// Whether an install that failed runs again, as it does on an
        /// apply and not on a launch.
        #[serde(default)]
        retry: bool,
    },
    /// Reads the skill proposals in a checkout, after removing the one in
    /// `remove`. The guest answers with
    /// [`GuestToHost::SkillProposalsRead`].
    SkillProposals {
        /// Matches the answer.
        req: u64,
        /// The checkout's absolute path inside the guest.
        checkout: String,
        /// A proposal folder to remove first, by name.
        remove: Option<String>,
    },
    /// Reads what in a folder can run when a Claude agent starts there. The
    /// guest answers with [`GuestToHost::FolderTrustRead`].
    FolderTrustScan {
        /// Matches the answer.
        req: u64,
        /// The folder's absolute path inside the guest.
        cwd: String,
        /// The skills in the project's lock, by name.
        locked: Vec<String>,
    },
    /// The agents the host has in this container, all of them each time
    /// they change, for the guest to bind to its workflow runs. Only the
    /// container's own agents are ever sent.
    WorkflowAgents {
        /// The agents, leads and sub-agents.
        agents: Vec<GuestAgent>,
    },
    /// Asks for the descriptor's lists for one node of a run the guest
    /// follows. The guest answers with [`GuestToHost::WorkflowDetail`].
    WorkflowDetail {
        /// Matches the answer.
        req: u64,
        /// The run's workflow.
        workflow: String,
        /// The run's id.
        run: String,
        /// The node.
        node: String,
    },
}

/// One file in a folder that can run code when a Claude agent starts there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedFile {
    /// Its path from the checkout's root, so worktrees of a project agree.
    pub name: String,
    /// The SHA-256 of its content, or of a link's target text.
    pub digest: String,
}

/// The text of a file a folder trust card shows in full.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownFile {
    /// Where it is.
    pub name: String,
    /// Its text, as far as it fits.
    pub text: String,
    /// Whether the text stops short of the file's end.
    pub cut: bool,
}

/// What a folder holds that can run code when a Claude agent starts there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderScan {
    /// Names the project the folder belongs to.
    pub key: String,
    /// The files that count, sorted.
    pub files: Vec<ScannedFile>,
    /// The settings and MCP files' text.
    pub shown: Vec<ShownFile>,
    /// Whether something couldn't be read, which never counts as trusted.
    pub failed: bool,
}

/// One agent in a container, as the host's hub has it: what binding needs
/// of it, and nothing more.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestAgent {
    /// The agent.
    pub id: AgentId,
    /// The agent that started it, for a sub-agent.
    pub lead: Option<AgentId>,
    /// The branch it works on, which names its worktree.
    pub branch: Option<String>,
    /// Its title. For a Claude sub-agent, the task's description.
    pub title: Option<String>,
    /// Its process inside the container, when it has one of its own.
    pub pid: Option<u32>,
    /// How it stands.
    pub state: GuestAgentState,
    /// When it started.
    pub started: Option<Timestamp>,
    /// When it last wrote anything.
    pub last_write: Option<Timestamp>,
    /// The first few kilobytes of its first prompt.
    pub prompt_head: Option<String>,
    /// Its last finished commands, oldest first.
    pub commands: Vec<String>,
}

/// How a [`GuestAgent`] stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestAgentState {
    /// It's working.
    Busy,
    /// It waits for you.
    Waiting,
    /// It's running and has nothing to do.
    Idle,
    /// It ended.
    Gone,
}

/// What a tunnel stream into a container reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelTarget {
    /// A port the guest's port watch found.
    Port(u16),
    /// An SSH session with the guest, which runs commands as `agent`.
    Ssh,
}

/// From `maxd guest` to host `maxd`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestToHost {
    /// The guest's half of the handshake, sent once it has the host's.
    Hello {
        /// [`GUEST_PROTOCOL_VERSION`] in the guest.
        protocol: u32,
        /// The guest's `maxd` version.
        maxd_version: String,
        /// The Incus instance name, as the container sees its host name.
        instance: String,
        /// The container's boot ID, which changes when it restarts.
        boot_id: BootId,
        /// The sessions with holders in the container.
        holders: Vec<SessionId>,
        /// The first 16 hex characters of the sha256 of the guest's binary.
        /// A guest from before builds leaves this out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build: Option<String>,
    },
    /// Whether a [`HostToGuest::Spawn`] started its holder.
    SpawnResult {
        /// The request's `req`.
        req: u64,
        /// Why not, if it didn't.
        result: Result<(), String>,
    },
    /// Something from a connected holder.
    Holder {
        /// The connection.
        conn: u64,
        /// What happened.
        event: HolderEvent,
    },
    /// What came of a [`HostToGuest::Write`].
    WriteResult {
        /// The request's `req`.
        req: u64,
        /// What happened to the line, or why it couldn't be written.
        outcome: Result<WriteOutcome, String>,
    },
    /// A [`HostToGuest::Stop`] or [`HostToGuest::Forget`] finished.
    Done {
        /// The request's `req`.
        req: u64,
        /// Why not, if it failed.
        result: Result<(), String>,
    },
    /// What came of a [`HostToGuest::End`].
    EndResult {
        /// The request's `req`.
        req: u64,
        /// Whether anything may still run.
        ended: Ended,
    },
    /// The answer to [`HostToGuest::ListHolders`].
    Holders {
        /// The request's `req`.
        req: u64,
        /// The sessions with holders in the container.
        sessions: Vec<SessionId>,
    },
    /// What came of a [`HostToGuest::PrepareWorktree`].
    WorktreeReady {
        /// The request's `req`.
        req: u64,
        /// The worktree, or why git refused.
        result: Result<Worktree, String>,
    },
    /// What came of a [`HostToGuest::FindPaths`].
    FoundPaths {
        /// The request's `req`.
        req: u64,
        /// The matches, best first, or why the search failed.
        result: Result<Vec<PathMatch>, String>,
    },
    /// What came of a [`HostToGuest::ListProcesses`].
    Processes {
        /// The request's `req`.
        req: u64,
        /// The processes, oldest first, or why the guest couldn't list
        /// them.
        result: Result<Vec<AgentProcess>, String>,
    },
    /// What came of a [`HostToGuest::SignalProcess`].
    Signalled {
        /// The request's `req`.
        req: u64,
        /// Done, or why the guest wouldn't.
        result: Result<(), String>,
    },
    /// What came of a [`HostToGuest::ListIgnored`]. A listing git can't
    /// make is empty.
    Ignored {
        /// The request's `req`.
        req: u64,
        /// The files, sorted by path, at most 200.
        files: Vec<IgnoredFile>,
        /// Whether git listed more than `files` holds.
        truncated: bool,
    },
    /// What came of a [`HostToGuest::Place`].
    Placed {
        /// The request's `req`.
        req: u64,
        /// What went in and what didn't, or why nothing could.
        result: Result<Placement, String>,
    },
    /// Timeline items from a history reader, in order. The reader's first
    /// read ends with an empty batch, even when it found nothing, so the
    /// host knows when it has the whole history.
    HistoryItems {
        /// The reader's `req`.
        req: u64,
        /// The items.
        items: Vec<TimelineItem>,
    },
    /// For a process in `session`'s tree, found by SO_PEERCRED.
    TokenRequest {
        /// Matches the [`HostToGuest::TokenReply`].
        req: u64,
        /// The session whose tree the caller is in.
        session: SessionId,
        /// What it asks for.
        want: TokenWant,
    },
    /// The ports the watched holders' trees listen on, all of them each
    /// time.
    Ports {
        /// At most [`MAX_PORTS`] entries.
        listening: Vec<GuestPort>,
    },
    /// An upload landed in the container.
    UploadStored {
        /// The upload.
        transfer: TransferId,
        /// Where each top-level entry landed, inside the container.
        paths: Vec<String>,
        /// A small preview of a picture that landed, as a `data:` URL,
        /// when the guest made one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumbnail: Option<String>,
        /// Where the guest saved the text it read out of a document that
        /// landed, beside it, when it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Credit for an upload, which the host passes on to the client.
    TransferCredit {
        /// The upload.
        transfer: TransferId,
        /// Bytes on disk and fsynced.
        committed: u64,
        /// The host may send up to this offset.
        window_end: u64,
    },
    /// What a download holds. Its chunks follow in transfer frames.
    FetchManifest {
        /// The download.
        transfer: TransferId,
        /// Every file, folder and link, folders before what's in them.
        entries: Vec<ManifestEntry>,
    },
    /// An upload or download stopped and won't go on.
    TransferFailed {
        /// The transfer.
        transfer: TransferId,
        /// What went wrong, for code to act on.
        code: ErrorCode,
        /// The manifest entry it stopped at, if it was one.
        entry: Option<u32>,
        /// What went wrong, for a person to read.
        message: String,
    },
    /// A stream the host opened is connected.
    TunnelOpened {
        /// The stream.
        stream: StreamId,
    },
    /// A stream the host opened couldn't connect.
    TunnelRefused {
        /// The stream.
        stream: StreamId,
        /// Why, for a person to read.
        reason: String,
    },
    /// Lets the host send more of a stream's bytes.
    TunnelCredit {
        /// The stream.
        stream: StreamId,
        /// More bytes the host may send.
        granted: u64,
    },
    /// Ends a stream, or the guest's half of it.
    TunnelClose {
        /// The stream.
        stream: StreamId,
        /// How it ends.
        how: TunnelEnd,
    },
    /// The public half of the guest's SSH host key, as an OpenSSH
    /// `ssh-ed25519` line. Sent once after the hello.
    SshHostKey {
        /// The key.
        public: String,
    },
    /// The answer to [`HostToGuest::Ping`].
    Pong {
        /// The ping's nonce.
        nonce: u64,
    },
    /// What a [`HostToGuest::LeaseRun`] printed.
    LeaseRan {
        /// The request's `req`.
        req: u64,
        /// The output, or why it couldn't run.
        result: Result<RunOutput, String>,
    },
    /// The screenshot a [`HostToGuest::LeaseScreenshot`] took.
    LeaseScreen {
        /// The request's `req`.
        req: u64,
        /// The image, or why not.
        result: Result<ToolImage, String>,
    },
    /// What came of a [`HostToGuest::LeaseInstall`] or
    /// [`HostToGuest::LeaseClose`].
    LeaseDone {
        /// The request's `req`.
        req: u64,
        /// Done, or why not.
        result: Result<(), String>,
    },
    /// A workflow run the guest follows, whole, each time it changes. The
    /// host diffs it for clients.
    WorkflowSnapshot {
        /// The run.
        snapshot: Box<RunSnapshot>,
        /// The agents that lead it, as the guest bound them.
        #[serde(default)]
        leads: Vec<AgentId>,
    },
    /// What came of a [`HostToGuest::WorkflowDetail`].
    WorkflowDetail {
        /// The request's `req`.
        req: u64,
        /// The node's lists, or why there are none.
        result: Result<NodeDetail, String>,
    },
    /// What came of a [`HostToGuest::SkillLinks`].
    SkillLinksDone {
        /// The request's `req`.
        req: u64,
        /// Done, or why not.
        result: Result<(), String>,
        /// Where the checkout's devflow pack stands. A guest from before
        /// it leaves this out.
        #[serde(default)]
        pack: Option<PackState>,
    },
    /// The proposals a [`HostToGuest::SkillProposals`] read.
    SkillProposalsRead {
        /// The request's `req`.
        req: u64,
        /// Each proposal folder, or why they couldn't be read.
        result: Result<Vec<GuestProposal>, String>,
    },
    /// What a [`HostToGuest::FolderTrustScan`] found.
    FolderTrustRead {
        /// The request's `req`.
        req: u64,
        /// The scan, or why there isn't one.
        result: Result<FolderScan, String>,
    },
    /// The work in the container's checkouts that isn't safe anywhere
    /// else yet, sent when it changes. The host sums it into
    /// [`crate::model::ManagedWorkspace::unsaved`].
    UnsavedReport {
        /// Each checkout with something unsaved, at most
        /// [`MAX_UNSAVED_WORKTREES`].
        worktrees: Vec<GuestUnsaved>,
    },
}

/// The most checkouts a [`GuestToHost::UnsavedReport`] lists.
pub const MAX_UNSAVED_WORKTREES: usize = 256;

/// One checkout's unsaved work, for [`GuestToHost::UnsavedReport`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestUnsaved {
    /// The checkout's path in the container.
    pub path: String,
    /// Files changed and not committed.
    pub changed: u32,
    /// Commits not on any remote.
    pub commits: u32,
    /// Whether its branch has no upstream.
    pub no_upstream: bool,
}

/// The most bytes of files a [`GuestToHost::SkillProposalsRead`] carries,
/// so its base64 stays under [`MAX_MESSAGE_LEN`]. A folder past it is
/// left out.
pub const MAX_PROPOSAL_BYTES: u64 = 2 * 1024 * 1024;

/// One proposal folder in a container's checkout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestProposal {
    /// Its name under `.maxhq/proposals`.
    pub folder: String,
    /// Its files.
    pub files: Vec<GuestProposalFile>,
}

/// A file in a [`GuestProposal`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestProposalFile {
    /// Relative to the folder, with `/` between parts.
    pub path: String,
    /// Whether it lands as 0755.
    pub executable: bool,
    /// Its bytes, base64 encoded.
    pub data: String,
}

impl HostToGuest {
    /// The first guest protocol with this message. The guest drops the
    /// link on a kind it can't read, so the host never sends one to a
    /// guest whose hello says an older protocol.
    #[must_use]
    pub fn since(&self) -> u32 {
        match self {
            Self::ListProcesses { .. } | Self::SignalProcess { .. } => 10,
            _ => 0,
        }
    }
}

impl GuestToHost {
    /// The most bytes this message's JSON may take: [`MAX_LINE_MESSAGE_LEN`]
    /// for one holder line, alone or as a hello's only pending line, and
    /// [`MAX_MESSAGE_LEN`] for anything else.
    pub fn limit(&self) -> usize {
        match self {
            Self::Holder {
                event: HolderEvent::Line(_),
                ..
            } => MAX_LINE_MESSAGE_LEN,
            Self::Holder {
                event: HolderEvent::Hello(hello),
                ..
            } if hello.pending.len() == 1 => MAX_LINE_MESSAGE_LEN,
            _ => MAX_MESSAGE_LEN,
        }
    }

    /// Reads a message body from the guest, and refuses one over its
    /// [`GuestToHost::limit`].
    pub fn parse(body: &[u8]) -> Result<Self, GuestLinkError> {
        let message: Self = serde_json::from_slice(body)?;
        let limit = message.limit();
        if body.len() > limit {
            return Err(GuestLinkError::TooLong {
                len: body.len(),
                limit,
            });
        }
        Ok(message)
    }
}

/// What a holder in the guest runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuestLaunch {
    /// The session this agent belongs to. It names the session's folder.
    pub session: SessionId,
    /// Absolute, inside the container.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Variables to set, on top of the environment the holder gives it.
    pub env: Vec<(String, String)>,
    /// Variables to remove before `env` is applied.
    pub env_remove: Vec<String>,
    /// Reaches the holder over a pipe and is never written down.
    pub secret_env: Vec<(String, Secret)>,
    /// The folder the CLI starts in, inside the container.
    pub cwd: String,
    /// How the CLI frames its messages.
    pub framing: Framing,
    /// The container user the CLI runs as, `agent` in M3.
    pub user: String,
}

/// How a CLI frames its messages, as `maxhq_hold::Framing` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// Claude Code's stream-json.
    ClaudeStreamJson,
    /// JSON-RPC 2.0, as `codex app-server` speaks it.
    JsonRpc,
}

/// Something from a holder connection in the guest, as `maxhq_hold`
/// reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HolderEvent {
    /// The holder's hello, first on every connection.
    Hello(HolderHello),
    /// The CLI printed a line.
    Line(HolderLine),
    /// The CLI ended. Nothing but `Closed` comes after it.
    Exited(HolderExit),
    /// The connection is gone, and the host may connect again.
    Closed {
        /// Why, when it wasn't the host's own [`HostToGuest::Disconnect`].
        error: Option<String>,
    },
}

/// What a holder says when a connection is made, as `maxhq_hold::Hello`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderHello {
    /// The holder's process ID, as the container sees it.
    pub holder_pid: u32,
    /// Whether the CLI is running.
    pub child: ChildState,
    /// The oldest line still on disk.
    pub first_seq: u64,
    /// The newest line so far, or 0 before the first.
    pub last_seq: u64,
    /// The prompts the CLI waits on, oldest first.
    pub pending: Vec<HolderLine>,
}

/// One line the CLI printed, as `maxhq_hold::Line`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderLine {
    /// Its place in the session's output, counting from 1 with no gaps.
    pub seq: u64,
    /// When the holder read it.
    pub at: Timestamp,
    /// The line without its newline.
    pub text: String,
    /// Whether the CLI printed it before this connection was made.
    pub replayed: bool,
}

/// How the CLI ended, as `maxhq_hold::Exit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderExit {
    /// The exit code, when the CLI exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, when one did.
    pub signal: Option<i32>,
    /// When the holder saw it end.
    pub at: Timestamp,
    /// The end of what the CLI wrote to stderr.
    pub stderr_tail: String,
}

/// Whether the CLI is still running, as `maxhq_hold::ChildState`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ChildState {
    /// It's running.
    Running {
        /// Its process ID, as the container sees it.
        pid: u32,
    },
    /// It has ended.
    Exited(HolderExit),
}

/// What happened to a write, as `maxhq_hold::WriteOutcome`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome {
    /// The CLI got the line.
    Written,
    /// The line answers a prompt that's no longer pending.
    AlreadyAnswered,
}

/// What came of ending a session, as `maxhq_hold::Ended`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Ended {
    /// Neither the holder nor its CLI runs any more.
    Gone,
    /// Something may still run, but it couldn't be confirmed as the
    /// session's own, so it was left alone.
    Unconfirmed {
        /// The process that may still run, as the container sees it.
        pid: Option<u32>,
        /// Why it couldn't be confirmed or ended.
        reason: String,
    },
}

/// A worktree the guest made for an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// Its folder, inside the container.
    pub path: String,
    /// Its branch.
    pub branch: String,
}

/// What a [`HostToGuest::Place`] moved in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// How many files went in.
    pub written: u32,
    /// The files that didn't, and why.
    pub failed: Vec<PlaceFailure>,
}

/// What a helper in the container asks the host for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TokenWant {
    /// A ChatGPT access token for Codex, per W-5.
    Codex,
    /// A GitHub installation token for one repository, per W-6.
    Github {
        /// The repository, as "owner/name".
        repo: String,
    },
}

/// The host's answer to a token request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TokenReply {
    /// The token, good until `expires_at`.
    Token {
        /// The token.
        value: Secret,
        /// When it stops working.
        expires_at: Timestamp,
    },
    /// No token, and why.
    Refused {
        /// Why, for a person to read.
        reason: String,
    },
}

/// How often the guest looks for listening ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pace {
    /// Every 2 s, while a client shows the workspace.
    Fast,
    /// Every 30 s.
    Slow,
}

/// A holder whose process tree the guest looks in for ports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedHolder {
    /// The holder's session.
    pub session: SessionId,
    /// The agent it runs, or none for a Codex app-server, which runs every
    /// agent on its account.
    pub agent: Option<AgentId>,
    /// The holder's process ID, as the container sees it.
    pub pid: u32,
}

/// A port the guest found listening.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestPort {
    /// The port number.
    pub port: u16,
    /// The address the socket is bound to.
    pub bind: BindAddress,
    /// The watched holder whose tree holds the socket, when one does.
    pub session: Option<SessionId>,
    /// The process that holds the socket, when it could be found.
    pub process: Option<PortProcess>,
}

/// A decoded guest link frame, borrowed from the decoder's buffer.
#[derive(Debug, PartialEq, Eq)]
pub enum GuestFrame<'a> {
    /// JSON for a [`HostToGuest`] or [`GuestToHost`].
    Message(&'a [u8]),
    /// Part of an upload or download.
    TransferChunk {
        /// The transfer.
        transfer: TransferId,
        /// Where `data` starts in the transfer's bytes.
        offset: u64,
        /// The bytes.
        data: &'a [u8],
    },
    /// Bytes on one tunnel stream.
    TunnelData {
        /// The stream.
        stream: StreamId,
        /// The bytes.
        data: &'a [u8],
    },
}

/// Why the guest link must drop. Every one of these drops it.
#[derive(Debug, Error)]
pub enum GuestLinkError {
    /// The bytes aren't valid frames, or a frame's kind is unknown.
    #[error(transparent)]
    Frame(#[from] DecodeError),
    /// A terminal frame, which the guest link never carries.
    #[error("frame kind {0} doesn't belong on the guest link")]
    WrongKind(u8),
    /// A message body is over its limit.
    #[error("message is {len} bytes, more than the {limit} allowed")]
    TooLong {
        /// The body's length.
        len: usize,
        /// The limit for that message.
        limit: usize,
    },
    /// A message body isn't a message.
    #[error("bad message: {0}")]
    Json(#[from] serde_json::Error),
}

/// Splits the guest link's bytes into frames. Unlike the client link, an
/// unknown kind is an error rather than a frame to skip.
#[derive(Debug, Default)]
pub struct GuestDecoder {
    frames: FrameDecoder,
}

impl GuestDecoder {
    /// A decoder with nothing buffered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes read from the link.
    pub fn push(&mut self, bytes: &[u8]) {
        self.frames.push(bytes);
    }

    /// Bytes buffered that haven't been returned as frames yet.
    pub fn buffered(&self) -> usize {
        self.frames.buffered()
    }

    /// The next whole frame, or `None` until more bytes arrive. Any error
    /// means the link should drop.
    pub fn next_frame(&mut self) -> Result<Option<GuestFrame<'_>>, GuestLinkError> {
        let Some(frame) = self.frames.next_frame()? else {
            return Ok(None);
        };
        match frame {
            Frame::Message(body) => Ok(Some(GuestFrame::Message(body))),
            Frame::TransferChunk {
                transfer,
                offset,
                data,
            } => Ok(Some(GuestFrame::TransferChunk {
                transfer,
                offset,
                data,
            })),
            Frame::TunnelData { stream, data } => Ok(Some(GuestFrame::TunnelData { stream, data })),
            Frame::TerminalOutput { .. } => Err(GuestLinkError::WrongKind(1)),
            Frame::TerminalInput { .. } => Err(GuestLinkError::WrongKind(2)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{encode_message, encode_terminal_output, encode_transfer_chunks};
    use crate::message::EntryKind;

    fn session() -> SessionId {
        "0199a7c2-0000-7000-8000-0000000000e1".parse().unwrap()
    }

    fn agent() -> AgentId {
        "0199a7c2-0000-7000-8000-0000000000e2".parse().unwrap()
    }

    fn run_snapshot() -> crate::workflow::RunSnapshot {
        crate::workflow::RunSnapshot {
            key: crate::workflow::RunKey {
                device: crate::ids::DeviceId::derive("devbox"),
                project: crate::ids::ProjectId::derive("git:github.com/owner/repo"),
                workflow: "devflow".into(),
                run: "run-1".into(),
            },
            rev: 1,
            shape: crate::workflow::RunShape {
                nodes: Vec::new(),
                edges: Vec::new(),
                groups: Vec::new(),
                hash: "abc".into(),
            },
            nodes: Vec::new(),
            loops: Vec::new(),
            header: crate::workflow::RunHeader {
                label: "Add sign-in".into(),
                status: crate::workflow::NodeStatus::Queued,
                progress: crate::workflow::Progress { done: 0, total: 0 },
                lines: Vec::new(),
            },
            notice: None,
        }
    }

    fn transfer() -> TransferId {
        "0199a7c2-0000-7000-8000-0000000000e3".parse().unwrap()
    }

    fn stream() -> StreamId {
        "0199a7c2-0000-7000-8000-0000000000e4".parse().unwrap()
    }

    fn line(text: &str) -> HolderLine {
        HolderLine {
            seq: 7,
            at: Timestamp(1_792_920_600_000),
            text: text.into(),
            replayed: false,
        }
    }

    fn exit() -> HolderExit {
        HolderExit {
            code: Some(0),
            signal: None,
            at: Timestamp(1_792_920_600_000),
            stderr_tail: String::new(),
        }
    }

    fn entries() -> Vec<ManifestEntry> {
        vec![ManifestEntry {
            path: "shot.png".into(),
            kind: EntryKind::File,
            executable: false,
            size: 4,
            sha256: Some("88d4266fd4e6338d13b845fcf289579d209c897823b9217da3e161936f031589".into()),
        }]
    }

    fn round_trip<T>(message: &T) -> serde_json::Value
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let value = serde_json::to_value(message).unwrap();
        let back: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&back, message);
        value
    }

    #[test]
    fn process_requests_need_guest_protocol_10() {
        let holder = WatchedHolder {
            session: session(),
            agent: None,
            pid: 50,
        };
        let list = HostToGuest::ListProcesses { req: 1, holder };
        assert_eq!(list.since(), 10);
        assert_eq!(HostToGuest::ListHolders { req: 1 }.since(), 0);
    }

    #[test]
    fn host_messages_survive_a_round_trip() {
        let messages = [
            (
                "hello",
                HostToGuest::Hello {
                    protocol: GUEST_PROTOCOL_VERSION,
                    maxd_version: "0.1.0".into(),
                    device: "0199a7c2-0000-7000-8000-0000000000e5".parse().unwrap(),
                    workspace: "0199a7c2-0000-7000-8000-0000000000e6".parse().unwrap(),
                    repo: Some("https://github.com/example-user/maxhq.git".into()),
                },
            ),
            (
                "spawn",
                HostToGuest::Spawn {
                    req: 1,
                    launch: GuestLaunch {
                        session: session(),
                        program: "/usr/local/bin/claude".into(),
                        args: vec!["--output-format".into(), "stream-json".into()],
                        env: vec![("CLAUDE_CONFIG_DIR".into(), "/home/agent/.claude".into())],
                        env_remove: vec!["ANTHROPIC_API_KEY".into()],
                        secret_env: vec![(
                            "CLAUDE_CODE_OAUTH_TOKEN".into(),
                            Secret::new("test-not-a-real-token".into()),
                        )],
                        cwd: "/home/agent/work/.worktrees/maxhq-0199a7c2".into(),
                        framing: Framing::ClaudeStreamJson,
                        user: "agent".into(),
                    },
                },
            ),
            (
                "connect",
                HostToGuest::Connect {
                    conn: 3,
                    session: session(),
                    after: Some(41),
                },
            ),
            (
                "write",
                HostToGuest::Write {
                    req: 2,
                    conn: 3,
                    line: "{}".into(),
                    answers: Some("req_1".into()),
                },
            ),
            (
                "stop",
                HostToGuest::Stop {
                    req: 3,
                    conn: 3,
                    grace_ms: 5000,
                },
            ),
            (
                "forget",
                HostToGuest::Forget {
                    req: 4,
                    session: session(),
                },
            ),
            (
                "end",
                HostToGuest::End {
                    req: 4,
                    session: session(),
                    holder_pid: Some(4021),
                },
            ),
            ("disconnect", HostToGuest::Disconnect { conn: 3 }),
            ("list_holders", HostToGuest::ListHolders { req: 5 }),
            (
                "prepare_worktree",
                HostToGuest::PrepareWorktree {
                    req: 6,
                    agent: agent(),
                    branch: "maxhq/fix-login".into(),
                    base: None,
                },
            ),
            (
                "find_paths",
                HostToGuest::FindPaths {
                    req: 9,
                    cwd: "/home/agent/work".into(),
                    query: "src/ma".into(),
                },
            ),
            (
                "list_processes",
                HostToGuest::ListProcesses {
                    req: 9,
                    holder: WatchedHolder {
                        session: session(),
                        agent: Some(agent()),
                        pid: 50,
                    },
                },
            ),
            (
                "signal_process",
                HostToGuest::SignalProcess {
                    req: 9,
                    holder: WatchedHolder {
                        session: session(),
                        agent: None,
                        pid: 50,
                    },
                    pid: 61,
                    start: 4_000,
                    signal: ProcessSignal::Kill,
                },
            ),
            (
                "list_ignored",
                HostToGuest::ListIgnored {
                    req: 10,
                    cwd: "/home/agent/work/name".into(),
                },
            ),
            (
                "place",
                HostToGuest::Place {
                    req: 11,
                    staged: transfer(),
                    cwd: "/home/agent/work/name".into(),
                    overwrite: true,
                },
            ),
            (
                "fetch_files",
                HostToGuest::FetchFiles {
                    transfer: transfer(),
                    cwd: "/home/agent/work/name".into(),
                    paths: vec![".env".into()],
                },
            ),
            (
                "open_history",
                HostToGuest::OpenHistory {
                    req: 7,
                    vendor: Vendor::Codex,
                    config_dir: "/home/agent/.codex".into(),
                    session: "thr_01".into(),
                    agent: agent(),
                    max_bytes: Some(256 << 20),
                },
            ),
            ("close_history", HostToGuest::CloseHistory { req: 7 }),
            (
                "token_reply",
                HostToGuest::TokenReply {
                    req: 8,
                    reply: TokenReply::Token {
                        value: Secret::new("test-not-a-real-token".into()),
                        expires_at: Timestamp(1_792_924_200_000),
                    },
                },
            ),
            (
                "token_reply",
                HostToGuest::TokenReply {
                    req: 9,
                    reply: TokenReply::Refused {
                        reason: "session not in this workspace".into(),
                    },
                },
            ),
            (
                "upload_begin",
                HostToGuest::UploadBegin {
                    transfer: transfer(),
                    agent: Some(agent()),
                    entries: entries(),
                },
            ),
            (
                "upload_cancel",
                HostToGuest::UploadCancel {
                    transfer: transfer(),
                },
            ),
            (
                "fetch_begin",
                HostToGuest::FetchBegin {
                    transfer: transfer(),
                    path: "/home/agent/work/maxhq/dist".into(),
                },
            ),
            (
                "transfer_credit",
                HostToGuest::TransferCredit {
                    transfer: transfer(),
                    committed: 0,
                    window_end: 512 * 1024,
                },
            ),
            (
                "keep_picture",
                HostToGuest::KeepPicture {
                    agent: agent(),
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                },
            ),
            (
                "watch_ports",
                HostToGuest::WatchPorts {
                    pace: Pace::Fast,
                    holders: vec![
                        WatchedHolder {
                            session: session(),
                            agent: Some(agent()),
                            pid: 812,
                        },
                        WatchedHolder {
                            session: session(),
                            agent: None,
                            pid: 990,
                        },
                    ],
                },
            ),
            (
                "tunnel_open",
                HostToGuest::TunnelOpen {
                    stream: stream(),
                    target: TunnelTarget::Port(5173),
                },
            ),
            (
                "tunnel_open",
                HostToGuest::TunnelOpen {
                    stream: stream(),
                    target: TunnelTarget::Ssh,
                },
            ),
            (
                "ssh_authorized",
                HostToGuest::SshAuthorized {
                    keys: vec!["ssh-ed25519 AAAA".into()],
                },
            ),
            (
                "tunnel_credit",
                HostToGuest::TunnelCredit {
                    stream: stream(),
                    granted: 64 * 1024,
                },
            ),
            (
                "tunnel_close",
                HostToGuest::TunnelClose {
                    stream: stream(),
                    how: TunnelEnd::Write,
                },
            ),
            ("ping", HostToGuest::Ping { nonce: 11 }),
            (
                "lease_run",
                HostToGuest::LeaseRun {
                    req: 12,
                    lease: LeaseId::derive("lease"),
                    command: vec!["snapshot".into(), "-i".into()],
                    timeout_seconds: 120,
                },
            ),
            (
                "lease_screenshot",
                HostToGuest::LeaseScreenshot {
                    req: 13,
                    lease: LeaseId::derive("lease"),
                },
            ),
            (
                "lease_install",
                HostToGuest::LeaseInstall {
                    req: 14,
                    lease: LeaseId::derive("lease"),
                    path: "/var/lib/maxhq/staging/App.app".into(),
                },
            ),
            (
                "lease_close",
                HostToGuest::LeaseClose {
                    req: 15,
                    lease: LeaseId::derive("lease"),
                },
            ),
            (
                "skill_links",
                HostToGuest::SkillLinks {
                    req: 16,
                    checkout: "/home/agent/work".into(),
                    names: vec!["git-flow".into()],
                    always: vec!["unslop".into()],
                    homes: vec!["/home/agent/.agents/skills".into()],
                    pack: Some(TreeHash("ab12".into())),
                    retry: true,
                },
            ),
            (
                "skill_proposals",
                HostToGuest::SkillProposals {
                    req: 17,
                    checkout: "/home/agent/work".into(),
                    remove: Some("git-flow".into()),
                },
            ),
            (
                "folder_trust_scan",
                HostToGuest::FolderTrustScan {
                    req: 18,
                    cwd: "/home/agent/work".into(),
                    locked: vec!["git-flow".into()],
                },
            ),
            (
                "workflow_agents",
                HostToGuest::WorkflowAgents {
                    agents: vec![GuestAgent {
                        id: agent(),
                        lead: None,
                        branch: Some("feature/sign-in".into()),
                        title: Some("Add sign-in".into()),
                        pid: Some(4022),
                        state: GuestAgentState::Waiting,
                        started: Some(Timestamp(1)),
                        last_write: Some(Timestamp(2)),
                        prompt_head: Some("maxhq-node: run-1 plan".into()),
                        commands: vec!["devflow next run-1".into()],
                    }],
                },
            ),
            (
                "workflow_detail",
                HostToGuest::WorkflowDetail {
                    req: 18,
                    workflow: "devflow".into(),
                    run: "run-1".into(),
                    node: "plan".into(),
                },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
        }
    }

    #[test]
    fn a_history_opened_by_an_older_host_names_no_limit() {
        let text = r#"{"type":"open_history","req":7,"vendor":"codex","config_dir":"/c","session":"s","agent":"0199a7c2-0000-7000-8000-0000000000e2"}"#;
        let message: HostToGuest = serde_json::from_str(text).unwrap();
        let HostToGuest::OpenHistory { max_bytes, .. } = message else {
            panic!("expected an open history");
        };
        assert_eq!(max_bytes, None);
        let open = HostToGuest::OpenHistory {
            req: 7,
            vendor: Vendor::Codex,
            config_dir: "/c".into(),
            session: "s".into(),
            agent: agent(),
            max_bytes: None,
        };
        assert!(round_trip(&open).get("max_bytes").is_none());
    }

    #[test]
    fn skill_links_from_an_older_host_link_no_always_set() {
        let text =
            r#"{"type":"skill_links","req":1,"checkout":"/home/agent/work","names":["git-flow"]}"#;
        let message: HostToGuest = serde_json::from_str(text).unwrap();
        assert_eq!(
            message,
            HostToGuest::SkillLinks {
                req: 1,
                checkout: "/home/agent/work".into(),
                names: vec!["git-flow".into()],
                always: Vec::new(),
                homes: Vec::new(),
                pack: None,
                retry: false,
            }
        );
    }

    #[test]
    fn guest_messages_survive_a_round_trip() {
        use crate::timeline::{ItemBody, ItemId, ItemStatus};

        let item = TimelineItem {
            id: ItemId("codex:item_1".into()),
            agent: agent(),
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Running,
            started_at: None,
            completed_at: None,
            body: ItemBody::AssistantMessage {
                text: "Looking".into(),
                streaming: false,
            },
        };
        let messages = [
            (
                "hello",
                GuestToHost::Hello {
                    protocol: GUEST_PROTOCOL_VERSION,
                    maxd_version: "0.1.0".into(),
                    instance: "mhq-maxhq".into(),
                    boot_id: BootId("9a0e".into()),
                    holders: vec![session()],
                    build: Some("ba7816bf8f01cfea".into()),
                },
            ),
            (
                "spawn_result",
                GuestToHost::SpawnResult {
                    req: 1,
                    result: Ok(()),
                },
            ),
            (
                "spawn_result",
                GuestToHost::SpawnResult {
                    req: 1,
                    result: Err("no such program".into()),
                },
            ),
            (
                "holder",
                GuestToHost::Holder {
                    conn: 3,
                    event: HolderEvent::Hello(HolderHello {
                        holder_pid: 811,
                        child: ChildState::Running { pid: 812 },
                        first_seq: 1,
                        last_seq: 41,
                        pending: vec![line("{\"type\":\"control_request\"}")],
                    }),
                },
            ),
            (
                "holder",
                GuestToHost::Holder {
                    conn: 3,
                    event: HolderEvent::Hello(HolderHello {
                        holder_pid: 811,
                        child: ChildState::Exited(exit()),
                        first_seq: 1,
                        last_seq: 41,
                        pending: vec![],
                    }),
                },
            ),
            (
                "holder",
                GuestToHost::Holder {
                    conn: 3,
                    event: HolderEvent::Line(line("{}")),
                },
            ),
            (
                "holder",
                GuestToHost::Holder {
                    conn: 3,
                    event: HolderEvent::Exited(exit()),
                },
            ),
            (
                "holder",
                GuestToHost::Holder {
                    conn: 3,
                    event: HolderEvent::Closed {
                        error: Some("holder went away".into()),
                    },
                },
            ),
            (
                "write_result",
                GuestToHost::WriteResult {
                    req: 2,
                    outcome: Ok(WriteOutcome::AlreadyAnswered),
                },
            ),
            (
                "done",
                GuestToHost::Done {
                    req: 3,
                    result: Ok(()),
                },
            ),
            (
                "end_result",
                GuestToHost::EndResult {
                    req: 4,
                    ended: Ended::Unconfirmed {
                        pid: Some(4022),
                        reason: "process 4022 doesn't look like its CLI".into(),
                    },
                },
            ),
            (
                "holders",
                GuestToHost::Holders {
                    req: 5,
                    sessions: vec![session()],
                },
            ),
            (
                "worktree_ready",
                GuestToHost::WorktreeReady {
                    req: 6,
                    result: Ok(Worktree {
                        path: "/home/agent/work/.worktrees/maxhq-fix-login".into(),
                        branch: "maxhq/fix-login".into(),
                    }),
                },
            ),
            (
                "found_paths",
                GuestToHost::FoundPaths {
                    req: 9,
                    result: Ok(vec![PathMatch {
                        path: "src/main.rs".into(),
                        kind: crate::find::PathKind::File,
                    }]),
                },
            ),
            (
                "processes",
                GuestToHost::Processes {
                    req: 9,
                    result: Ok(vec![AgentProcess {
                        pid: 61,
                        parent: 50,
                        start: 4_000,
                        run_ms: 2_000,
                        cpu_ms: 30,
                        memory: 8_192,
                        name: "sleep".into(),
                        command: "sleep 60".into(),
                        cli: false,
                        ports: Vec::new(),
                    }]),
                },
            ),
            (
                "signalled",
                GuestToHost::Signalled {
                    req: 9,
                    result: Err("That process isn't in the agent's tree any more.".into()),
                },
            ),
            (
                "ignored",
                GuestToHost::Ignored {
                    req: 10,
                    files: vec![IgnoredFile {
                        path: ".env".into(),
                        size: 64,
                    }],
                    truncated: false,
                },
            ),
            (
                "placed",
                GuestToHost::Placed {
                    req: 11,
                    result: Ok(Placement {
                        written: 1,
                        failed: vec![PlaceFailure {
                            path: ".env.local".into(),
                            reason: "a file is already there".into(),
                        }],
                    }),
                },
            ),
            (
                "history_items",
                GuestToHost::HistoryItems {
                    req: 7,
                    items: vec![item],
                },
            ),
            (
                "token_request",
                GuestToHost::TokenRequest {
                    req: 8,
                    session: session(),
                    want: TokenWant::Codex,
                },
            ),
            (
                "token_request",
                GuestToHost::TokenRequest {
                    req: 9,
                    session: session(),
                    want: TokenWant::Github {
                        repo: "example-user/maxhq".into(),
                    },
                },
            ),
            (
                "ports",
                GuestToHost::Ports {
                    listening: vec![GuestPort {
                        port: 5173,
                        bind: BindAddress::Any,
                        session: Some(session()),
                        process: Some(PortProcess {
                            pid: 900,
                            name: "node".into(),
                            command: "node vite".into(),
                        }),
                    }],
                },
            ),
            (
                "upload_stored",
                GuestToHost::UploadStored {
                    transfer: transfer(),
                    paths: vec!["/var/lib/maxhq/attachments/a/shot.png".into()],
                    thumbnail: Some("data:image/png;base64,iVBORw0KGgo=".into()),
                    text: None,
                },
            ),
            (
                "transfer_credit",
                GuestToHost::TransferCredit {
                    transfer: transfer(),
                    committed: 4,
                    window_end: 512 * 1024,
                },
            ),
            (
                "fetch_manifest",
                GuestToHost::FetchManifest {
                    transfer: transfer(),
                    entries: entries(),
                },
            ),
            (
                "transfer_failed",
                GuestToHost::TransferFailed {
                    transfer: transfer(),
                    code: ErrorCode::NoSpace,
                    entry: None,
                    message: "disk full".into(),
                },
            ),
            (
                "tunnel_opened",
                GuestToHost::TunnelOpened { stream: stream() },
            ),
            (
                "tunnel_refused",
                GuestToHost::TunnelRefused {
                    stream: stream(),
                    reason: "connection refused".into(),
                },
            ),
            (
                "tunnel_credit",
                GuestToHost::TunnelCredit {
                    stream: stream(),
                    granted: 1,
                },
            ),
            (
                "tunnel_close",
                GuestToHost::TunnelClose {
                    stream: stream(),
                    how: TunnelEnd::Reset,
                },
            ),
            (
                "ssh_host_key",
                GuestToHost::SshHostKey {
                    public: "ssh-ed25519 AAAA".into(),
                },
            ),
            ("pong", GuestToHost::Pong { nonce: 11 }),
            (
                "lease_ran",
                GuestToHost::LeaseRan {
                    req: 12,
                    result: Ok(RunOutput {
                        exit_code: Some(0),
                        stdout: "ok".into(),
                        stderr: String::new(),
                        truncated: false,
                        timed_out: false,
                    }),
                },
            ),
            (
                "lease_screen",
                GuestToHost::LeaseScreen {
                    req: 13,
                    result: Ok(ToolImage {
                        mime: "image/png".into(),
                        data: "iVBORw0KGgo=".into(),
                    }),
                },
            ),
            (
                "lease_done",
                GuestToHost::LeaseDone {
                    req: 14,
                    result: Err("not a test host".into()),
                },
            ),
            (
                "workflow_snapshot",
                GuestToHost::WorkflowSnapshot {
                    snapshot: Box::new(run_snapshot()),
                    leads: vec![agent()],
                },
            ),
            (
                "workflow_detail",
                GuestToHost::WorkflowDetail {
                    req: 18,
                    result: Ok(NodeDetail {
                        lists: vec![crate::workflow::DetailList {
                            label: "Files".into(),
                            items: vec!["src/main.rs".into()],
                        }],
                    }),
                },
            ),
            (
                "workflow_detail",
                GuestToHost::WorkflowDetail {
                    req: 19,
                    result: Err("That run isn't followed on this device.".into()),
                },
            ),
            (
                "skill_links_done",
                GuestToHost::SkillLinksDone {
                    req: 16,
                    result: Ok(()),
                    pack: Some(PackState::Installed {
                        tree: TreeHash("ab12".into()),
                    }),
                },
            ),
            (
                "folder_trust_read",
                GuestToHost::FolderTrustRead {
                    req: 18,
                    result: Ok(FolderScan {
                        key: "/home/agent/work".into(),
                        files: vec![ScannedFile {
                            name: ".mcp.json".into(),
                            digest: "abc".into(),
                        }],
                        shown: vec![ShownFile {
                            name: ".mcp.json".into(),
                            text: "{}".into(),
                            cut: false,
                        }],
                        failed: false,
                    }),
                },
            ),
            (
                "skill_proposals_read",
                GuestToHost::SkillProposalsRead {
                    req: 17,
                    result: Ok(vec![GuestProposal {
                        folder: "git-flow".into(),
                        files: vec![GuestProposalFile {
                            path: "SKILL.md".into(),
                            executable: false,
                            data: "LS0t".into(),
                        }],
                    }]),
                },
            ),
            (
                "unsaved_report",
                GuestToHost::UnsavedReport {
                    worktrees: vec![GuestUnsaved {
                        path: "/home/dev/maxhq".into(),
                        changed: 2,
                        commits: 1,
                        no_upstream: false,
                    }],
                },
            ),
        ];
        for (tag, message) in messages {
            assert_eq!(round_trip(&message)["type"], tag);
        }
    }

    #[test]
    fn a_tunnel_names_its_target() {
        let port = round_trip(&HostToGuest::TunnelOpen {
            stream: stream(),
            target: TunnelTarget::Port(5173),
        });
        assert_eq!(port["target"], serde_json::json!({ "port": 5173 }));
        let ssh = round_trip(&HostToGuest::TunnelOpen {
            stream: stream(),
            target: TunnelTarget::Ssh,
        });
        assert_eq!(ssh["target"], "ssh");
    }

    #[test]
    fn an_upload_stored_names_only_the_sidecars_it_has() {
        let bare = GuestToHost::UploadStored {
            transfer: transfer(),
            paths: vec!["/var/lib/maxhq/attachments/a/notes.txt".into()],
            thumbnail: None,
            text: None,
        };
        let value = round_trip(&bare);
        assert!(value.get("thumbnail").is_none() && value.get("text").is_none());
        let doc = GuestToHost::UploadStored {
            transfer: transfer(),
            paths: vec!["/var/lib/maxhq/attachments/a/notes.docx".into()],
            thumbnail: None,
            text: Some("/var/lib/maxhq/attachments/a/notes.docx.txt".into()),
        };
        assert_eq!(
            round_trip(&doc)["text"],
            "/var/lib/maxhq/attachments/a/notes.docx.txt"
        );
    }

    #[test]
    fn secrets_on_the_guest_link_stay_out_of_debug_output() {
        let reply = HostToGuest::TokenReply {
            req: 8,
            reply: TokenReply::Token {
                value: Secret::new("test-not-a-real-token".into()),
                expires_at: Timestamp(0),
            },
        };
        assert!(!format!("{reply:?}").contains("test-not-a-real-token"));
    }

    #[test]
    fn unknown_kinds_drop_the_link() {
        let mut decoder = GuestDecoder::new();
        decoder.push(&[0, 0, 0, 2, 9, 0]);
        assert!(matches!(
            decoder.next_frame(),
            Err(GuestLinkError::Frame(DecodeError::UnknownKind(9)))
        ));
    }

    #[test]
    fn a_workflow_snapshot_without_leads_has_none() {
        let mut value = round_trip(&GuestToHost::WorkflowSnapshot {
            snapshot: Box::new(run_snapshot()),
            leads: vec![agent()],
        });
        value.as_object_mut().unwrap().remove("leads");
        let Ok(GuestToHost::WorkflowSnapshot { leads, .. }) = serde_json::from_value(value) else {
            panic!("a snapshot without leads parses");
        };
        assert!(leads.is_empty());
    }

    #[test]
    fn hello_from_before_builds_parses() {
        let line = format!(
            r#"{{"type":"hello","protocol":{GUEST_PROTOCOL_VERSION},"maxd_version":"0.1.0","instance":"mhq-maxhq","boot_id":"9a0e","holders":[]}}"#
        );
        let Ok(GuestToHost::Hello { build, .. }) = GuestToHost::parse(line.as_bytes()) else {
            panic!("a hello without a build parses");
        };
        assert_eq!(build, None);
    }

    #[test]
    fn terminal_kinds_drop_the_link() {
        let mut out = Vec::new();
        encode_terminal_output(&mut out, crate::ids::TerminalId::generate(), 0, b"ls\r\n");
        let mut decoder = GuestDecoder::new();
        decoder.push(&out);
        assert!(matches!(
            decoder.next_frame(),
            Err(GuestLinkError::WrongKind(1))
        ));
    }

    #[test]
    fn messages_and_chunks_pass_through() {
        let mut out = Vec::new();
        encode_message(&mut out, &HostToGuest::Ping { nonce: 1 }).unwrap();
        encode_transfer_chunks(&mut out, transfer(), 8, b"abcd");
        crate::frame::encode_tunnel_data(&mut out, stream(), b"GET /");

        let mut decoder = GuestDecoder::new();
        decoder.push(&out);
        let Some(GuestFrame::Message(body)) = decoder.next_frame().unwrap() else {
            panic!("expected a message");
        };
        assert_eq!(
            serde_json::from_slice::<HostToGuest>(body).unwrap(),
            HostToGuest::Ping { nonce: 1 }
        );
        assert_eq!(
            decoder.next_frame().unwrap(),
            Some(GuestFrame::TransferChunk {
                transfer: transfer(),
                offset: 8,
                data: b"abcd",
            })
        );
        assert_eq!(
            decoder.next_frame().unwrap(),
            Some(GuestFrame::TunnelData {
                stream: stream(),
                data: b"GET /",
            })
        );
        assert_eq!(decoder.next_frame().unwrap(), None);
        assert_eq!(decoder.buffered(), 0);
    }

    #[test]
    fn only_holder_lines_go_past_four_mebibytes() {
        let big = "x".repeat(MAX_MESSAGE_LEN);
        let as_line = serde_json::to_vec(&GuestToHost::Holder {
            conn: 3,
            event: HolderEvent::Line(line(&big)),
        })
        .unwrap();
        assert!(as_line.len() > MAX_MESSAGE_LEN);
        assert!(GuestToHost::parse(&as_line).is_ok());

        let hello = |pending: Vec<HolderLine>| {
            serde_json::to_vec(&GuestToHost::Holder {
                conn: 3,
                event: HolderEvent::Hello(HolderHello {
                    holder_pid: 4021,
                    child: ChildState::Running { pid: 4022 },
                    first_seq: 1,
                    last_seq: 2,
                    pending,
                }),
            })
            .unwrap()
        };
        assert!(GuestToHost::parse(&hello(vec![line(&big)])).is_ok());
        let half = "x".repeat(MAX_MESSAGE_LEN / 2);
        assert!(matches!(
            GuestToHost::parse(&hello(vec![line(&half), line(&half)])),
            Err(GuestLinkError::TooLong {
                limit: MAX_MESSAGE_LEN,
                ..
            })
        ));

        let as_refusal = serde_json::to_vec(&GuestToHost::TunnelRefused {
            stream: stream(),
            reason: big,
        })
        .unwrap();
        assert!(matches!(
            GuestToHost::parse(&as_refusal),
            Err(GuestLinkError::TooLong {
                limit: MAX_MESSAGE_LEN,
                ..
            })
        ));
        assert!(matches!(
            GuestToHost::parse(b"{\"type\":\"pong\"}"),
            Err(GuestLinkError::Json(_))
        ));
    }
}
