// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How the manager reaches holders, the `maxd hold` processes that keep
//! each CLI alive. [`Holders`] has the hold crate's shape one to one, with
//! the event stream and the writer behind traits so tests can fake them.
//! The real one is [`maxhq_hold::Holds`].
//!
//! [`HolderPlaces`] gives the holders on this device and in each connected
//! guest. The other traits here are what the manager needs from the
//! workspaces, the secret store, file delivery and the check for unsaved
//! work, and what the guest link needs from the manager.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, BoxStream, StreamExt};
use maxhq_hold::{Holds, Writer};
use maxhq_protocol::acp::AcpEnvValue;
use maxhq_protocol::find::PathMatch;
use maxhq_protocol::guest::{FolderScan, WatchedHolder, Worktree};
use maxhq_protocol::ids::{
    AccountId, AcpAgentId, AgentId, AttachmentId, BootId, ProjectId, SessionId, WorkspaceId,
};
use maxhq_protocol::model::{AllowedWorkflowEnv, Vendor};
use maxhq_protocol::processes::{ProcessList, ProcessSignal};
use maxhq_protocol::timeline::AttachmentRef;
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use maxhq_hold::{
    ChildState, ConnectError, Ended, Event, Exit, Hello, Launch, Line, SpawnError, WriteError,
    WriteOutcome,
};

use super::clock::BoxFuture;
use super::vendors::History;
use crate::drive::claude::own_mcp;
use crate::drive::{self, ChildExit, Framing, HeldLine, Place, Staged};
use maxhq_core::tools::Page;

/// How long a holder that can't be reached gets to stop its CLI after
/// SIGTERM. A holder that's still working needs about 6 seconds.
const END_GRACE: Duration = Duration::from_secs(10);

/// Writes to one holder's CLI and controls the holder, as
/// [`maxhq_hold::Writer`] does.
pub trait HoldWriter: Send + Sync {
    /// Writes one line to the CLI's stdin. `answers` names the request the
    /// line responds to, as the holder keys it.
    fn write(
        &self,
        line: String,
        answers: Option<String>,
    ) -> BoxFuture<Result<WriteOutcome, WriteError>>;
    /// Closes the CLI's stdin, then ends it after `grace`.
    fn stop(&self, grace: Duration) -> BoxFuture<Result<(), WriteError>>;
    /// Deletes a finished session's files.
    fn forget(&self) -> BoxFuture<Result<(), WriteError>>;
}

/// One connection to a holder, as [`maxhq_hold::Connection`] is.
pub struct Connection {
    /// What the holder said first.
    pub hello: Hello,
    /// Replayed lines, then live ones, then the exit. It ends without an
    /// exit when the connection drops while the holder runs.
    pub events: BoxStream<'static, Event>,
    /// Writes to the CLI.
    pub writer: Arc<dyn HoldWriter>,
}

/// Starts, finds and connects to holders, as [`maxhq_hold::Holds`] does.
pub trait Holders: Send + Sync + 'static {
    /// Starts a detached holder, resolving once it's ready.
    fn spawn(&self, launch: Launch) -> BoxFuture<Result<(), SpawnError>>;
    /// Every holder that's alive, or finished with an exit nobody has seen.
    /// A guest answers over its link, so this waits.
    fn list(&self) -> BoxFuture<io::Result<Vec<SessionId>>>;
    /// Connects to a holder. Lines after `after` replay first, marked as
    /// replayed.
    fn connect(
        &self,
        session: SessionId,
        after: Option<u64>,
    ) -> BoxFuture<Result<Connection, ConnectError>>;
    /// Makes sure nothing still runs for a holder that can't be reached,
    /// so a new one never runs beside it. `holder_pid` is the process ID
    /// its last connection gave, if any did.
    fn end(&self, session: SessionId, holder_pid: Option<u32>) -> BoxFuture<Ended>;
}

impl Holders for Holds {
    fn spawn(&self, mut launch: Launch) -> BoxFuture<Result<(), SpawnError>> {
        let holds = self.clone();
        Box::pin(async move {
            own_mcp::add(&mut launch).await;
            Holds::spawn(&holds, &launch).await
        })
    }

    fn list(&self) -> BoxFuture<io::Result<Vec<SessionId>>> {
        let holds = self.clone();
        Box::pin(async move { Holds::list(&holds) })
    }

    fn connect(
        &self,
        session: SessionId,
        after: Option<u64>,
    ) -> BoxFuture<Result<Connection, ConnectError>> {
        let holds = self.clone();
        Box::pin(async move {
            let connection = Holds::connect(&holds, session, after).await?;
            Ok(Connection {
                hello: connection.hello,
                events: connection.events.boxed(),
                writer: Arc::new(connection.writer),
            })
        })
    }

    fn end(&self, session: SessionId, holder_pid: Option<u32>) -> BoxFuture<Ended> {
        let holds = self.clone();
        Box::pin(async move { Holds::end(&holds, session, holder_pid, END_GRACE).await })
    }
}

impl HoldWriter for Writer {
    fn write(
        &self,
        line: String,
        answers: Option<String>,
    ) -> BoxFuture<Result<WriteOutcome, WriteError>> {
        let writer = self.clone();
        Box::pin(async move { Writer::write(&writer, line, answers).await })
    }

    fn stop(&self, grace: Duration) -> BoxFuture<Result<(), WriteError>> {
        let writer = self.clone();
        Box::pin(async move { Writer::stop(&writer, grace).await })
    }

    fn forget(&self) -> BoxFuture<Result<(), WriteError>> {
        let writer = self.clone();
        Box::pin(async move { Writer::forget(&writer).await })
    }
}

// Guests, containers, the secret store and file delivery come in later
// streams. Until then nothing calls most of what follows, so it allows
// dead code.

/// Holders on this device and in each connected guest.
#[allow(dead_code)]
pub trait HolderPlaces: Send + Sync + 'static {
    /// This device's holders.
    fn local(&self) -> Arc<dyn Holders>;
    /// A workspace's holders. None while its guest link is down.
    fn guest(&self, workspace: WorkspaceId) -> Option<Arc<dyn Holders>>;
    /// Guest links coming up, with the container's boot ID, and going down.
    fn changes(&self) -> BoxStream<'static, PlaceChange>;
    /// Has a workspace's guest read what in a folder can run when a Claude
    /// agent starts there.
    fn folder_scan(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        locked: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        let _ = (workspace, cwd, locked);
        Box::pin(async { Err("This device has no guests.".to_owned()) })
    }
    /// W-4. Has a workspace's guest add a worktree on a new branch, as user
    /// `agent`, from `base` or the repo's default branch.
    fn worktree(
        &self,
        workspace: WorkspaceId,
        agent: AgentId,
        branch: String,
        base: Option<String>,
    ) -> BoxFuture<Result<Worktree, String>>;
    /// Has a workspace's guest search `cwd` for the files and folders that
    /// match `query`, for the composer's `@` menu.
    fn find_paths(
        &self,
        workspace: WorkspaceId,
        cwd: String,
        query: String,
    ) -> BoxFuture<Result<Vec<PathMatch>, String>> {
        let _ = (workspace, cwd, query);
        Box::pin(async { Err("this maxd has no guest links yet".to_owned()) })
    }
    /// Has a workspace's guest list the processes in `holder`'s tree, for
    /// the Running list. `TooOld` when the guest can't.
    fn list_processes(
        &self,
        workspace: WorkspaceId,
        holder: WatchedHolder,
    ) -> BoxFuture<Result<ProcessList, String>> {
        let _ = (workspace, holder);
        Box::pin(async { Err("this maxd has no guest links yet".to_owned()) })
    }
    /// Has a workspace's guest send `signal` to the process `pid`, started
    /// at `start`, if it's in `holder`'s tree.
    fn signal_process(
        &self,
        workspace: WorkspaceId,
        holder: WatchedHolder,
        pid: u32,
        start: u64,
        signal: ProcessSignal,
    ) -> BoxFuture<Result<(), String>> {
        let _ = (workspace, holder, pid, start, signal);
        Box::pin(async { Err("this maxd has no guest links yet".to_owned()) })
    }
    /// A history reader that runs in a workspace's guest, for a session held
    /// there. It sends `OpenHistory`, and each `read` hands over the items
    /// that came back since the last one. Its first read takes at most
    /// `bytes` of the newest history, or all of it with no limit. None
    /// while the guest link is down.
    fn history(
        &self,
        workspace: WorkspaceId,
        vendor: Vendor,
        config_dir: String,
        session: String,
        agent: AgentId,
        bytes: Option<u64>,
    ) -> Option<Box<dyn History>>;
    /// Has a container's guest write a picture from a tool result where
    /// the timeline names it, without waiting. While the link is down
    /// nothing is written, and the container's next history read writes
    /// it instead.
    fn keep_picture(&self, picture: drive::pictures::ForGuest) {
        let _ = picture;
    }
}

/// A guest link coming up or going down, or its workspace deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PlaceChange {
    /// The guest said hello.
    Up {
        /// The workspace.
        workspace: WorkspaceId,
        /// The container's boot ID. A new one means its holders died.
        boot_id: BootId,
        /// The holders the guest has.
        holders: Vec<SessionId>,
    },
    /// The link dropped.
    Down {
        /// The workspace.
        workspace: WorkspaceId,
        /// Why, for the agents' link state.
        reason: String,
    },
    /// The workspace was deleted, so its guest and every CLI in it are
    /// gone for good.
    Gone {
        /// The workspace.
        workspace: WorkspaceId,
    },
}

/// The places before guests: this device's holders and nothing else.
pub struct LocalOnly(pub Arc<dyn Holders>);

impl HolderPlaces for LocalOnly {
    fn local(&self) -> Arc<dyn Holders> {
        self.0.clone()
    }

    fn guest(&self, _workspace: WorkspaceId) -> Option<Arc<dyn Holders>> {
        None
    }

    fn changes(&self) -> BoxStream<'static, PlaceChange> {
        stream::pending().boxed()
    }

    fn worktree(
        &self,
        _workspace: WorkspaceId,
        _agent: AgentId,
        _branch: String,
        _base: Option<String>,
    ) -> BoxFuture<Result<Worktree, String>> {
        Box::pin(async { Err("this maxd has no guest links yet".to_owned()) })
    }

    fn history(
        &self,
        _workspace: WorkspaceId,
        _vendor: Vendor,
        _config_dir: String,
        _session: String,
        _agent: AgentId,
        _bytes: Option<u64>,
    ) -> Option<Box<dyn History>> {
        None
    }
}

/// XC's workspaces, as the manager needs them: container facts and writes
/// to `user.maxhq.agents`.
#[allow(dead_code)]
pub trait Workspaces: Send + Sync + 'static {
    /// The container behind a workspace this device manages, or None for a
    /// folder workspace.
    fn container(&self, workspace: WorkspaceId) -> Option<Container>;
    /// A-10. Replaces the workspace's `user.maxhq.agents` with these.
    fn record_agents(
        &self,
        workspace: WorkspaceId,
        agents: Vec<AgentSummary>,
    ) -> BoxFuture<Result<(), String>>;
    /// Keeps the workflow variables you allowed for a project's agents on
    /// this device, in its settings.
    fn allow_workflow_env(
        &self,
        project: ProjectId,
        allowed: AllowedWorkflowEnv,
    ) -> Result<(), String>;
}

/// A container workspace, as the manager starts agents in it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Container {
    /// The Incus instance.
    pub instance: String,
    /// The repo's folder inside the container, under `/home/agent/work`.
    pub repo: String,
    /// Why agents can't start there now, such as a failed egress check.
    /// None when they can.
    pub blocked: Option<String>,
}

/// One agent in `user.maxhq.agents`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AgentSummary {
    /// The agent.
    pub agent: AgentId,
    /// Its CLI.
    pub vendor: Vendor,
    /// The account it runs under.
    pub account: AccountId,
    /// The Claude session UUID or the Codex thread ID.
    pub session: String,
    /// Its worktree's branch.
    pub branch: String,
    /// Its worktree, inside the container.
    pub worktree: String,
}

/// No container workspaces, which is every device before XC.
pub struct NoWorkspaces;

impl Workspaces for NoWorkspaces {
    fn container(&self, _workspace: WorkspaceId) -> Option<Container> {
        None
    }

    fn record_agents(
        &self,
        _workspace: WorkspaceId,
        _agents: Vec<AgentSummary>,
    ) -> BoxFuture<Result<(), String>> {
        Box::pin(async { Err("this maxd manages no workspaces yet".to_owned()) })
    }

    fn allow_workflow_env(
        &self,
        _project: ProjectId,
        _allowed: AllowedWorkflowEnv,
    ) -> Result<(), String> {
        Err("this maxd keeps no project settings".to_owned())
    }
}

/// XH's and XI's store: which secret a token account uses.
#[allow(dead_code)]
pub trait Credentials: Send + Sync + 'static {
    /// The setup-token behind a Claude token account.
    fn claude_token(&self, account: AccountId) -> Option<Secret>;
    /// Whether the store holds a Claude setup-token, whether or not the
    /// device takes them now.
    fn holds_claude_token(&self) -> bool;
    /// Whether a Codex account has a ChatGPT sign-in to broker.
    fn has_chatgpt(&self, account: AccountId) -> bool;
    /// The API key behind a Cursor key account.
    fn cursor_key(&self, account: AccountId) -> Option<Secret>;
    /// The values stored for a local ACP agent's variables.
    fn acp_env(&self, agent: AcpAgentId) -> Vec<AcpEnvValue>;
}

/// An empty store, which every device has before XH and XI.
pub struct NoCredentials;

impl Credentials for NoCredentials {
    fn claude_token(&self, _account: AccountId) -> Option<Secret> {
        None
    }

    fn holds_claude_token(&self) -> bool {
        false
    }

    fn has_chatgpt(&self, _account: AccountId) -> bool {
        false
    }

    fn cursor_key(&self, _account: AccountId) -> Option<Secret> {
        None
    }

    fn acp_env(&self, _agent: AcpAgentId) -> Vec<AcpEnvValue> {
        Vec::new()
    }
}

/// XF's delivery of staged attachments into a container.
#[allow(dead_code)]
pub trait AgentFiles: Send + Sync + 'static {
    /// Copies staged attachments into the place's attachments folder.
    fn deliver(
        &self,
        place: &Place,
        staged: Vec<AttachmentId>,
    ) -> BoxFuture<Result<Vec<Staged>, DeliverError>>;

    /// Copies the files an agent sends with MaxHQ's send tool out of its
    /// folders, and gives them as attachments in its attachments folder.
    /// `call` names the tool call, so a call run again keeps the same IDs.
    /// An error is what the agent reads.
    fn collect(
        &self,
        place: &Place,
        call: &str,
        paths: Vec<String>,
    ) -> BoxFuture<Result<Vec<AttachmentRef>, String>> {
        let _ = (place, call, paths);
        Box::pin(async { Err("This maxd can't take files from agents.".to_owned()) })
    }

    /// Copies the file or folder at `path`, relative to the agent's folder
    /// in its container, into staging as `id`, for the `@` menu. An error
    /// is what the composer shows.
    fn stage_found(
        &self,
        place: &Place,
        id: AttachmentId,
        path: &str,
    ) -> BoxFuture<Result<AttachmentRef, String>> {
        let _ = (place, id, path);
        Box::pin(async { Err("This maxd can't copy files out of containers.".to_owned()) })
    }

    /// Checks the file an agent opens with MaxHQ's open tool, without
    /// reading it. An error is what the agent reads.
    fn look(&self, place: &Place, path: String) -> BoxFuture<Result<Looked, String>> {
        let _ = (place, path);
        Box::pin(async { Err("This maxd can't open files for the user.".to_owned()) })
    }

    /// Copies the page an agent shows with MaxHQ's show tool into its
    /// attachments folder as `name`, and gives it with the page's text.
    /// `call` names the tool call, so a call run again keeps the same ID.
    /// An error is what the agent reads.
    fn show(
        &self,
        place: &Place,
        call: &str,
        page: Page,
        name: String,
    ) -> BoxFuture<Result<(AttachmentRef, String), String>> {
        let _ = (place, call, page, name);
        Box::pin(async { Err("This maxd can't show visuals.".to_owned()) })
    }
}

/// A file the open tool may show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Looked {
    /// Its absolute path, on the agent's device or in its container.
    pub path: String,
    /// Set when it's outside the agent's folders, so the app asks first.
    pub outside: bool,
}

/// Why staged attachments didn't reach a container.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum DeliverError {
    /// The attachment isn't staged on this device, or another agent has it.
    #[error("{}", super::attach::GONE)]
    Missing(AttachmentId),
    /// The copy failed.
    #[error("couldn't copy the attachments in: {0}")]
    Copy(String),
}

/// No delivery into containers, which every device has before XF.
pub struct NoFiles;

impl AgentFiles for NoFiles {
    fn deliver(
        &self,
        _place: &Place,
        _staged: Vec<AttachmentId>,
    ) -> BoxFuture<Result<Vec<Staged>, DeliverError>> {
        Box::pin(async {
            Err(DeliverError::Copy(
                "this maxd can't deliver into containers yet".to_owned(),
            ))
        })
    }
}

/// A checkout an agent works in, for auto-settle's check for unsaved work.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Checkout {
    /// A folder on this device.
    Folder(PathBuf),
    /// A folder inside a container workspace this device manages.
    Container {
        /// The workspace.
        workspace: WorkspaceId,
        /// The folder, inside the container.
        dir: String,
    },
}

/// Whether a checkout has work that would be lost from view: changes that
/// aren't committed, or commits that aren't on its remote.
pub trait Unsaved: Send + Sync + 'static {
    /// True when it has, false when it hasn't, and None when that can't be
    /// told, such as when git fails or the container is stopped.
    fn unsaved(&self, checkout: Checkout) -> BoxFuture<Option<bool>>;
}

/// Never knows, so auto-settle never settles anything.
#[cfg(test)]
pub struct NoUnsaved;

#[cfg(test)]
impl Unsaved for NoUnsaved {
    fn unsaved(&self, _checkout: Checkout) -> BoxFuture<Option<bool>> {
        Box::pin(async { None })
    }
}

/// The manager answers this for the guest link, so a token goes only to a
/// live session in its own workspace.
#[allow(dead_code)]
pub trait SessionOwners: Send + Sync + 'static {
    /// Who a live session belongs to, or None for one the manager doesn't
    /// hold.
    fn owner(&self, session: SessionId) -> Option<SessionOwner>;
}

/// Who a held session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct SessionOwner {
    /// The workspace it runs in.
    pub workspace: WorkspaceId,
    /// The account it runs under.
    pub account: AccountId,
    /// Its CLI.
    pub vendor: Vendor,
}

/// The holder's launch for a driver's launch.
pub fn launch(session: SessionId, launch: drive::Launch) -> Launch {
    Launch {
        session,
        program: launch.program,
        args: launch.args,
        env: launch.env,
        env_remove: launch.env_remove,
        secret_env: launch.secret_env,
        run_as: launch.run_as.map(|user| maxhq_hold::RunAs {
            user: user.user,
            uid: user.uid,
            gid: user.gid,
            home: user.home,
        }),
        cwd: launch.cwd,
        framing: match launch.framing {
            Framing::ClaudeStreamJson => maxhq_hold::Framing::ClaudeStreamJson,
            Framing::JsonRpc => maxhq_hold::Framing::JsonRpc,
        },
    }
}

/// The driver's line for a holder's line.
pub fn held_line(line: Line) -> HeldLine {
    HeldLine {
        seq: line.seq,
        at: line.at,
        text: line.text,
        replayed: line.replayed,
    }
}

/// The driver's exit for a holder's exit.
pub fn child_exit(exit: Exit) -> ChildExit {
    ChildExit {
        code: exit.code,
        signal: exit.signal,
        at: exit.at,
        stderr_tail: exit.stderr_tail,
        with_machine: false,
    }
}
