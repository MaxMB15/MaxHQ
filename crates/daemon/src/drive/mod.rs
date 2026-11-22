// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How the session manager and the vendor drivers meet.
//!
//! A driver is a plain state machine for one held process. The manager feeds
//! it the CLI's lines and your actions and carries out what it returns, so
//! drivers are tested with recorded lines and no processes. A Claude driver
//! holds one session and its sub-agents. A Codex driver holds one account's
//! `codex app-server` and every thread on it.
//!
//! Ordinals are the manager's job: drivers emit items with `ordinal: 0`, and
//! the timeline store numbers each new ID as it arrives.

use std::any::Any;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use maxhq_core::context;
use maxhq_core::tools::{Call, Tool};
use maxhq_protocol::acp::AcpFeatures;
use maxhq_protocol::agents::ToolImage;
use maxhq_protocol::attention::{AttentionAnswer, AttentionEvent};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{
    AccountId, AgentId, AttentionId, ClientId, DeviceId, IdempotencyKey, WorkspaceId,
};
use maxhq_protocol::message::{ActionOutcome, AgentSpec, SendMode};
use maxhq_protocol::model::{Agent, CommandRule, EditKind, ModelChoice, PermissionMode, Usage};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{AttachmentRef, ItemId, TimelineEvent, TimelineItem};
use maxhq_protocol::Secret;

pub mod acp;
pub mod claude;
pub mod codex;
pub mod commands;
pub mod cursor;
pub mod local;
pub mod pictures;

/// A process for the holder to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The program to run, as an absolute path.
    pub program: PathBuf,
    /// Its arguments, without the program itself.
    pub args: Vec<OsString>,
    /// Variables to set on top of `maxd`'s own environment.
    pub env: Vec<(OsString, OsString)>,
    /// Variables to take out of `maxd`'s own environment.
    pub env_remove: Vec<OsString>,
    /// The folder to run it in.
    pub cwd: PathBuf,
    /// How its stdout is framed.
    pub framing: Framing,
    /// Set only in the child's environment. The holder gets them on a pipe
    /// at spawn and never writes the values down.
    pub secret_env: Vec<(OsString, Secret)>,
    /// The user the child runs as. Only a holder running as root takes it.
    pub run_as: Option<RunAs>,
}

impl Launch {
    /// The process for `maxd` to start itself, in its folder and with its
    /// variables, the secret ones included. `run_as` isn't for `maxd`.
    pub fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.program);
        command.args(&self.args).current_dir(&self.cwd);
        for name in &self.env_remove {
            command.env_remove(name);
        }
        command.envs(self.env.iter().cloned());
        for (name, value) in &self.secret_env {
            command.env(name, value.expose());
        }
        command
    }
}

/// A user for a holder running as root to start its child as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAs {
    /// The user's name.
    pub user: String,
    /// The user's uid.
    pub uid: u32,
    /// The user's primary gid.
    pub gid: u32,
    /// The user's home folder, which becomes `HOME`.
    pub home: PathBuf,
}

/// How the holder tells requests from the rest, so it can keep the
/// unanswered ones across reconnects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    /// Claude's `--output-format stream-json`, where requests are
    /// `control_request` lines.
    ClaudeStreamJson,
    /// JSON-RPC, where requests are lines with both an `id` and a `method`.
    JsonRpc,
}

/// A line the CLI printed. `replayed` is set for lines from before this
/// connection, which rebuild state but must not be answered again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldLine {
    /// The holder's number for the line, one more than the line before.
    pub seq: u64,
    /// When the holder read it.
    pub at: Timestamp,
    /// The line, without its newline.
    pub text: String,
    /// Set for lines from before this connection.
    pub replayed: bool,
}

/// How the held CLI ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildExit {
    /// The exit status, if it exited.
    pub code: Option<i32>,
    /// The signal that killed it, if one did.
    pub signal: Option<i32>,
    /// When the holder saw it end.
    pub at: Timestamp,
    /// The last of what it wrote to stderr.
    pub stderr_tail: String,
    /// Whether it went down with its device or container, as a stop or a
    /// restart of either ends it. Then its end isn't the CLI's own failure.
    pub with_machine: bool,
}

/// What the manager knows about where an agent runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The device `maxd` runs on.
    pub device: DeviceId,
    /// The workspace the agent runs in.
    pub workspace: WorkspaceId,
    /// The folder the CLI runs in.
    pub cwd: PathBuf,
    /// The folder staged attachments are moved into for this agent.
    pub attachments: PathBuf,
    /// Whose holders run the agent, and so whose paths `cwd` and
    /// `attachments` are.
    pub host: PlaceHost,
    /// Whether it's the dispatcher's own folder. Its CLI there can't
    /// write, and it gets only the dispatcher's tools.
    pub dispatcher: bool,
    /// What maxd decided about the folder's own files, for a CLI that
    /// takes it with the thread. None leaves the CLI to its own config.
    pub trust: Option<FolderTrust>,
    /// Whether a Claude Code CLI may use its tools for claude.ai pages, as
    /// the device's settings say when it starts.
    pub claude_artifacts: bool,
}

/// Whether a CLI may load a folder's own files, and the folders it looks
/// the answer up at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderTrust {
    /// Whether it may.
    pub trusted: bool,
    /// Each folder the CLI checks, as it names them.
    pub paths: Vec<String>,
}

impl Place {
    /// The MaxHQ tools an agent here gets.
    pub fn tools(&self) -> &'static [Tool] {
        if self.dispatcher {
            &Tool::DISPATCHER
        } else {
            &Tool::ALL
        }
    }

    /// The MaxHQ tools an agent here under `login` gets: a local agent
    /// that asked for fewer gets the lean set, except as the dispatcher.
    pub fn tools_for(&self, login: &Login) -> &'static [Tool] {
        match &login.auth {
            LoginAuth::ModelServer(server) if server.lean && !self.dispatcher => &Tool::LEAN,
            _ => self.tools(),
        }
    }
}

/// Where an agent's holder runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceHost {
    /// On this device, as in M2.
    Local,
    /// In a container workspace, through its guest.
    Guest {
        /// The workspace.
        workspace: WorkspaceId,
    },
}

/// The login a driver runs under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Login {
    /// The account the login is.
    pub account: AccountId,
    /// Its `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
    pub config_dir: PathBuf,
    /// What the CLI signs in with.
    pub auth: LoginAuth,
}

/// What a login's CLI signs in with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginAuth {
    /// The login in `config_dir`, as in M2.
    ConfigDir,
    /// W-5. The launch sets `CLAUDE_CODE_OAUTH_TOKEN` as a secret variable
    /// and leaves it out of the C-5 scrub for this launch only.
    SetupToken(Secret),
    /// W-5. Codex runs on the fixed `maxhq` provider, whose auth command is
    /// `maxd guest token codex`.
    ChatGptBroker,
    /// A Cursor user API key. The launch sets `CURSOR_API_KEY` as a secret
    /// variable and nothing writes it to disk.
    CursorApiKey(Secret),
    /// A local ACP command. The launch runs its program and arguments, and
    /// sets its variables' values as secret variables.
    AcpCommand(Arc<local::AcpCommand>),
    /// A local agent: Claude Code pointed at a model server, with the
    /// server's token as a secret variable.
    ModelServer(Arc<ServerLogin>),
}

/// How a local agent reaches its model server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerLogin {
    /// The server's Anthropic API base URL.
    pub base_url: String,
    /// Its token.
    pub token: Secret,
    /// Its context size in tokens, which Claude Code compacts before.
    pub context: Option<u32>,
    /// The model an agent with none of its own uses.
    pub model: Option<String>,
    /// Whether the agent gets the lean set of MaxHQ tools.
    pub lean: bool,
}

/// An attachment moved into the agent's folder, ready for the CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    /// The attachment as clients see it.
    pub attachment: AttachmentRef,
    /// Where it now is, inside [`Place::attachments`].
    pub path: PathBuf,
    /// Where the text read out of a document was saved beside it, when it
    /// was.
    pub text: Option<PathBuf>,
}

/// A message ready for a CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// What you typed, with U+FFFC where each placed pill sits.
    pub text: String,
    /// Its pills, in order.
    pub refs: Vec<InlineRef>,
    /// Its attachments, in order.
    pub attachments: Vec<Staged>,
}

impl Message {
    /// The text with each pill shown as words, for a title.
    pub fn plain(&self) -> String {
        context::plain(&self.text, &self.refs, |id| {
            self.attachments
                .iter()
                .find(|staged| staged.attachment.id == *id)
                .map(|staged| staged.attachment.name.clone())
        })
    }
}

/// What a driver is asked to do. Each carries the client's key, so the
/// outcome can come back later through `Effects::results`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Codex only: a new thread on this app-server. Claude agents start
    /// with their own holder, from `claude::launch`.
    Start {
        /// The client's key.
        key: IdempotencyKey,
        /// Where the thread runs.
        place: Place,
        /// What to start.
        spec: Box<AgentSpec>,
        /// The first message, if there is one.
        first: Option<Message>,
        /// The agent the thread is, for one moving to Codex from another
        /// vendor. Left out, the thread names a new agent.
        agent: Option<AgentId>,
    },
    /// Codex only: resume a thread made elsewhere, for "Continue in MaxHQ".
    Adopt {
        /// The client's key.
        key: IdempotencyKey,
        /// Where the thread runs.
        place: Place,
        /// The agent the thread is now.
        agent: AgentId,
        /// The Codex thread ID.
        thread: String,
    },
    /// Sends a message.
    Send {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent to send to.
        agent: AgentId,
        /// What you typed.
        text: String,
        /// Its pills, in order.
        refs: Vec<InlineRef>,
        /// Attachments, in order.
        attachments: Vec<Staged>,
        /// What to do when the agent is busy.
        mode: SendMode,
    },
    /// Takes back a queued message.
    CancelQueued {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent the message is queued for.
        agent: AgentId,
        /// The queued message's item.
        item: ItemId,
    },
    /// Stops the current turn.
    Interrupt {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent to interrupt.
        agent: AgentId,
    },
    /// Changes the permission mode.
    SetPermission {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent to change.
        agent: AgentId,
        /// The mode to run in.
        mode: PermissionMode,
    },
    /// Changes the model.
    SetModel {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent to change.
        agent: AgentId,
        /// The model to run.
        model: String,
    },
    /// Changes the effort level from the next turn on.
    SetEffort {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent to change.
        agent: AgentId,
        /// The level, or none for the vendor's default.
        effort: Option<String>,
    },
    /// Answers an approval or a question.
    Answer {
        /// The client's key.
        key: IdempotencyKey,
        /// The event being answered.
        event: AttentionId,
        /// The answer.
        answer: AttentionAnswer,
        /// The client that sent it.
        by: ClientId,
        /// Whether it came from a phone whose switch is off, so it can't
        /// put the agent in a mode that needs the switch.
        held_back: bool,
    },
    /// Ends the agent. For Claude this stops the holder too.
    Stop {
        /// The client's key.
        key: IdempotencyKey,
        /// The agent to stop.
        agent: AgentId,
    },
}

/// The client's key on any action.
pub trait ActionKey {
    /// The key the action carries.
    fn key(&self) -> IdempotencyKey;
}

impl ActionKey for Action {
    fn key(&self) -> IdempotencyKey {
        match self {
            Action::Start { key, .. }
            | Action::Adopt { key, .. }
            | Action::Send { key, .. }
            | Action::CancelQueued { key, .. }
            | Action::Interrupt { key, .. }
            | Action::SetPermission { key, .. }
            | Action::SetModel { key, .. }
            | Action::SetEffort { key, .. }
            | Action::Answer { key, .. }
            | Action::Stop { key, .. } => *key,
        }
    }
}

/// A line for the CLI's stdin. `answers` names the request it responds
/// to, as the holder keys it, so a second answer is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    /// The line, without its newline.
    pub line: String,
    /// The request this answers, if it answers one.
    pub answers: Option<String>,
}

/// What a driver wants done, collected over one call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effects {
    /// Lines for the CLI's stdin, in order.
    pub writes: Vec<Write>,
    /// Items from before this process, replacing the agent's older history.
    pub history: Vec<(AgentId, Vec<TimelineItem>)>,
    /// Changes to timelines, in order.
    pub timeline: Vec<(AgentId, TimelineEvent)>,
    /// Pictures from tool results for containers' guests to write, and
    /// their previews to make.
    pub pictures: pictures::Kept,
    /// Agents to add or replace whole, including sub-agents.
    pub agents: Vec<Agent>,
    /// Agents that no longer exist.
    pub gone: Vec<AgentId>,
    /// Attention events to open or close.
    pub attention: Vec<AttentionChange>,
    /// Rate limits the CLI reported.
    pub usage: Vec<(AccountId, Usage)>,
    /// Models the CLI reported for an account.
    pub models: Vec<(AccountId, Vec<ModelChoice>)>,
    /// What an ACP agent's CLI said it can do, for its account.
    pub features: Vec<(AccountId, AcpFeatures)>,
    /// Outcomes for actions, by the client's key.
    pub results: Vec<(IdempotencyKey, ActionOutcome)>,
    /// Calls to MaxHQ's own tools to run off the loop. Each one's outcome
    /// comes back through [`Driver::tool_done`].
    pub calls: Vec<ToolCall>,
    /// Slow work to run off the loop, such as reading a long transcript.
    /// What each returns comes back through [`Driver::job_done`].
    pub jobs: Vec<Job>,
    /// When to call `tick`, if the driver is waiting on a timer.
    pub wake_at: Option<Instant>,
    /// Stop the holder once the writes are out.
    pub stop: bool,
    /// Commands you answered "Always allow" to, which the agent's project
    /// keeps as rules.
    pub command_rules: Vec<CommandRule>,
    /// Kinds of file you allowed with a card's "Remember for this project"
    /// box, which the agent's project keeps, so its agents change them
    /// without asking.
    pub allowed_edits: Vec<EditKind>,
}

/// What a [`Job`] returns, for the driver that asked for it to read back.
pub type JobDone = Box<dyn Any + Send>;

/// The work in a [`Job`].
type Work = Box<dyn FnOnce() -> JobDone + Send>;

/// Slow work a driver hands off the loop. It runs once, whichever of its
/// copies runs it.
#[derive(Clone)]
pub struct Job(Arc<Mutex<Option<Work>>>);

impl Job {
    /// A job that runs `work`.
    pub fn new(work: impl FnOnce() -> JobDone + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(work)))))
    }

    /// Runs the job, or gives none when a copy of it already ran.
    pub fn run(self) -> Option<JobDone> {
        let work = self
            .0
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()?;
        Some(work())
    }
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Job")
    }
}

/// An agent called one of MaxHQ's tools, and its arguments passed their
/// checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    /// The driver's key for the call, which comes back on its outcome.
    pub key: String,
    /// The vendor's ID for the call. It stays the same when the call is run
    /// again after a reconnect, so the files it keeps do too.
    pub call_id: String,
    /// The agent that called it, or its lead.
    pub agent: AgentId,
    /// What it asked for.
    pub call: Call,
    /// Its arguments as the CLI gave them, for a call that goes on to
    /// another device.
    pub arguments: serde_json::Value,
}

/// How a [`ToolCall`] went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDone {
    /// The call's [`ToolCall::key`].
    pub key: String,
    /// What it did, or what the agent reads about why it failed.
    pub result: Result<ToolResult, String>,
}

/// What a tool call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolResult {
    /// The files are on the timeline, as MaxHQ keeps them.
    Sent(Vec<AttachmentRef>),
    /// The file is one the viewer opens, so a card for it goes on the
    /// timeline.
    Opened(maxhq_core::tools::Opened),
    /// The page is in the attachments, so a visual goes on the timeline.
    Visual(maxhq_core::tools::Visual),
    /// Text for the agent to read, such as another agent's timeline.
    Text(String),
    /// Text with images for the agent to see, such as a lease's
    /// screenshot.
    Mixed {
        /// The text.
        text: String,
        /// The images, after the text.
        images: Vec<ToolImage>,
    },
}

/// A change to the open attention events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttentionChange {
    /// Opens the event, or replaces the one with its ID.
    Open(AttentionEvent),
    /// Closes the event, such as once it's answered.
    Close(AttentionId),
}

/// One vendor's state machine for one held process.
pub trait Driver: Send + 'static {
    /// Connected to the holder. `pending` holds the requests still
    /// unanswered, already among the replayed lines or older than them.
    fn connected(&mut self, fresh: bool, pending: &[HeldLine], fx: &mut Effects);
    /// The CLI printed a line, or the holder replayed one.
    fn line(&mut self, line: &HeldLine, fx: &mut Effects);
    /// You asked for something.
    fn act(&mut self, action: Action, fx: &mut Effects);
    /// The CLI ended.
    fn exited(&mut self, exit: &ChildExit, fx: &mut Effects);
    /// The time in an earlier [`Effects::wake_at`] has come.
    fn tick(&mut self, now: Instant, fx: &mut Effects);
    /// A call from [`Effects::calls`] finished.
    fn tool_done(&mut self, done: ToolDone, fx: &mut Effects) {
        let _ = (done, fx);
    }
    /// A new driver takes up a lead agent an earlier one drove, as `maxd`
    /// stored it: after a restart, or when a resume starts its CLI again.
    /// Meeting a CLI again isn't activity, so the agent keeps its times and
    /// only what the CLI does from here moves them.
    fn met_again(&mut self, known: &Agent) {
        let _ = known;
    }
    /// A job from [`Effects::jobs`] finished with what it returned, or
    /// none when it failed. The manager hands the driver nothing else
    /// about the CLI's end until its jobs are done.
    fn job_done(&mut self, done: Option<JobDone>, fx: &mut Effects) {
        let _ = (done, fx);
    }
    /// The seq of the line that started the turn still open, if one is.
    /// A new driver after a restart reads the holder's log from there, so
    /// it rebuilds that turn whole. None reads on from the last line.
    fn checkpoint(&self) -> Option<u64> {
        None
    }
    /// The skills in the checkouts at `cwds` changed. A CLI that reloads
    /// its skills on request does so; the others see the change on their
    /// own or after a restart.
    fn skills_changed(&mut self, cwds: &[String], fx: &mut Effects) {
        let _ = (cwds, fx);
    }
    /// A banked reset lifted the account's usage limit before its reset
    /// time. A CLI that waits out a limit by reading it reads it again
    /// now, so its waits end; the others find out on their own.
    fn limits_lifted(&mut self, fx: &mut Effects) {
        let _ = fx;
    }
    /// The commands the agent's project lets run without asking. A driver
    /// that answers its CLI's command prompts itself reads them; the
    /// others leave prompts to the CLI.
    fn command_rules(&mut self, rules: &[CommandRule]) {
        let _ = rules;
    }
    /// The kinds of file the agent's project lets agents in Accept edits or
    /// Auto change without asking, or none when the agent has no project
    /// to remember more for. A driver that decides its CLI's edits itself
    /// reads them; the others leave edits to the CLI.
    fn allowed_edits(&mut self, edits: Option<&[EditKind]>) {
        let _ = edits;
    }
    /// How many of each agent's newest items the timeline keeps in memory.
    /// A driver that keeps settled items of its own keeps as many.
    fn keep_items(&mut self, items: usize) {
        let _ = items;
    }
    /// Whether `agent`'s session can go now and come back later with
    /// nothing lost: no turn, prompt or background work runs, and the CLI
    /// loads the session again. A driver that can't tell says no.
    fn can_release(&self, agent: AgentId) -> bool {
        let _ = agent;
        false
    }
    /// Lets `agent`'s session go on a CLI that holds several, so it keeps
    /// nothing for it. A CLI with one session stops instead.
    fn release(&mut self, agent: AgentId, fx: &mut Effects) {
        let _ = (agent, fx);
    }
}
