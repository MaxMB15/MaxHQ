// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the manager keeps about each agent it holds and each holder it
//! reaches. [`Held`] and [`HoldRecord`] are stored, so they survive a
//! restart of `maxd`. The rest lives only as long as this run.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use maxhq_protocol::agents::AgentOrigin;
use maxhq_protocol::ids::{
    AccountId, AgentId, AttentionId, BootId, ClientId, IdempotencyKey, SessionId, WorkspaceId,
};
use maxhq_protocol::message::ServerMessage;
use maxhq_protocol::model::{
    Activity, Agent, ContextUsage, LoginKind, PauseReason, PermissionMode, Vendor,
};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemId, TimelineItem};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, error::TrySendError};

use super::holders::HoldWriter;
use super::reads::Source;
use super::recovery::{Restarts, Resumes};
use super::vendors::one_process_per_agent;
use crate::agents::AgentKey;
use crate::drive::{acp, Action, ChildExit, Driver, Launch, Login};

/// One agent the manager holds: a Claude session, or a Codex thread.
/// Sub-agents aren't held on their own; they follow their lead.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Held {
    /// The agent as its driver last reported it, before the manager's own
    /// state is laid over it.
    pub agent: Agent,
    /// The account the agent runs under.
    pub account: AccountId,
    /// The account's `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
    pub config_dir: PathBuf,
    /// The folder the CLI runs in.
    pub cwd: PathBuf,
    /// The permission mode to run in, kept for resumes.
    pub mode: PermissionMode,
    /// The model to run, kept for resumes.
    pub model: Option<String>,
    /// The Claude session UUID or the Codex thread ID.
    pub vendor_session: String,
    /// The holder a Claude agent runs in. Codex agents share their
    /// account's app-server instead.
    pub session: Option<SessionId>,
    /// Set while the agent waits for you, and why.
    pub paused: Option<PauseReason>,
    /// Resumes since the agent last made progress.
    pub resumes: Resumes,
    /// Why the agent couldn't be started or brought back, if it couldn't.
    pub fault: Option<String>,
    /// Codex only: the app-server doesn't have this thread loaded, so the
    /// next action adopts it first.
    pub adopt: bool,
    /// Set from a restart until the new process says something.
    pub resuming: bool,
    /// The continue message the agent still needs after a crash cut its
    /// turn off.
    pub owed: Option<String>,
    /// When the agent's process went down, if it's down.
    pub down_since: Option<Timestamp>,
    /// The manager's own attention event telling you the agent paused.
    pub notice: Option<AttentionId>,
    /// The container workspace the agent runs in, or none for this device.
    #[serde(default)]
    pub guest: Option<WorkspaceId>,
    /// How the account signs in.
    #[serde(default)]
    pub login: LoginKind,
    /// The branch of the agent's worktree, in a container.
    #[serde(default)]
    pub branch: Option<String>,
    /// When the usage limit the agent waits on resets. A restart while it
    /// waits brings it back after then, not at once.
    #[serde(default)]
    pub resume_after: Option<Timestamp>,
    /// Your messages still queued for the agent, so a restart that ends its
    /// CLI doesn't lose them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queued: Vec<TimelineItem>,
    /// The title you gave a Claude or Cursor agent. It stands over the one
    /// its CLI takes from the first message. Codex keeps a thread's name
    /// itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Who started the agent with the agent tools, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentOrigin>,
    /// The session key a Cursor agent on this device signs in to the agent
    /// tools with. Kept so the CLI it holds keeps calling across a restart
    /// of `maxd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_key: Option<AgentKey>,
    /// The sessions the agent left when it moved to another vendor, so
    /// moving back takes its session up again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub natives: Vec<Native>,
    /// The conversation still to go to the vendor the agent moved to,
    /// with its next message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Handoff>,
    /// Set once the agent sat idle long enough that `maxd` stopped its CLI
    /// or let its Codex thread go. It shows as it did, and its CLI starts
    /// again for the next message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub released: bool,
    /// Claude only: the workflow variables its last start found, and what
    /// you said about them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_env: Option<super::workflow_env::WorkflowEnv>,
    /// Set while the agent waits for a usage limit that cut its turn off,
    /// so it can carry on once the limit resets. It's kept across a restart
    /// of `maxd`, but a restart of the agent's machine takes it over.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut_at_limit: bool,
    /// When the limit that cut the agent's turn off resets, while
    /// `cut_at_limit` is set. A wait that ends before then is checked again
    /// at that time, so an agent it left idle still carries on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_resets_at: Option<Timestamp>,
    /// Claude Code and Codex only: the effort level the agent's turns run
    /// at, or none for the vendor's default. Kept for restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// A session an agent left when it moved to another vendor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Native {
    /// The account it ran under.
    pub account: AccountId,
    /// Its Claude session UUID, Codex thread ID or ACP session ID.
    pub vendor_session: String,
    /// The last timeline item it saw.
    pub seen: ItemId,
    /// How full its context was when the agent left.
    pub context: Option<ContextUsage>,
}

/// The conversation an agent's next message carries to the vendor it
/// moved to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    /// The marker in the timeline where the agent moved.
    pub marker: ItemId,
    /// The last item the session saw before, when the agent went back to
    /// one it had, so only what came after goes.
    pub since: Option<ItemId>,
    /// How full that session's context was.
    pub context: Option<ContextUsage>,
}

impl Held {
    /// A newly held agent that runs as `info` says, with nothing wrong yet.
    pub fn new(agent: Agent, info: StartInfo, vendor_session: String) -> Self {
        Self {
            agent,
            account: info.account,
            config_dir: info.config_dir,
            cwd: info.cwd,
            mode: info.mode,
            model: info.model,
            effort: info.effort,
            vendor_session,
            session: None,
            paused: None,
            resumes: Resumes::default(),
            fault: None,
            adopt: false,
            resuming: false,
            owed: None,
            down_since: None,
            notice: None,
            guest: info.guest,
            login: info.login,
            branch: info.branch,
            resume_after: None,
            queued: Vec::new(),
            title: None,
            origin: info.origin,
            tools_key: None,
            natives: Vec::new(),
            handoff: None,
            released: false,
            workflow_env: None,
            cut_at_limit: false,
            limit_resets_at: None,
        }
    }

    /// The agent's vendor.
    pub fn vendor(&self) -> Vendor {
        self.agent.vendor
    }

    /// Whether the agent has a CLI of its own, as Claude and ACP agents
    /// do. Codex agents share their account's app-server.
    pub fn own_process(&self) -> bool {
        one_process_per_agent(self.vendor())
    }

    /// What an ACP agent's next process loads: its session, model and
    /// title. The manager adds its agent tools.
    pub fn kept(&self) -> acp::Kept {
        acp::Kept {
            session: Some(self.vendor_session.clone()).filter(|session| !session.is_empty()),
            model: self.model.clone(),
            title: self
                .title
                .clone()
                .or_else(|| Some(self.agent.title.clone())),
            tools: None,
        }
    }

    /// The app-server a Codex agent's thread runs on.
    pub fn server_key(&self) -> ServerKey {
        ServerKey {
            account: self.account,
            workspace: self.guest,
        }
    }
}

/// Whether an agent is in the middle of a turn.
pub fn is_busy(activity: &Activity) -> bool {
    matches!(
        activity,
        Activity::Busy | Activity::BusyInSubagents | Activity::NeedsInput { .. }
    )
}

/// What's stored about each holder, so a restarted `maxd` can rebuild its
/// driver.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HoldRecord {
    /// The vendor of the CLI it runs.
    pub vendor: Vendor,
    /// The account the CLI runs under.
    pub account: AccountId,
    /// The account's config folder.
    pub config_dir: PathBuf,
    /// The agent, for a Claude holder. A Codex holder runs every thread of
    /// its account.
    pub agent: Option<AgentId>,
    /// The container workspace whose guest runs the holder, or none for
    /// this device.
    #[serde(default)]
    pub workspace: Option<WorkspaceId>,
    /// The boot ID of the host, or of the container for a guest's holder,
    /// when the holder started. A different one now means it died with a
    /// restart.
    #[serde(default)]
    pub boot: Option<BootId>,
    /// How the account signs in.
    #[serde(default)]
    pub login: LoginKind,
    /// The folder the CLI runs in.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// The Claude session UUID, for a Claude holder.
    #[serde(default)]
    pub vendor_session: Option<String>,
    /// The CLI's arguments, which name its launch flags.
    #[serde(default)]
    pub args: Vec<String>,
    /// The names of the variables set for the CLI. Their values aren't
    /// kept, since some are secret.
    #[serde(default)]
    pub env: Vec<String>,
}

impl HoldRecord {
    /// A record for a holder that hasn't started yet.
    pub fn new(vendor: Vendor, account: AccountId, config_dir: PathBuf) -> Self {
        Self {
            vendor,
            account,
            config_dir,
            agent: None,
            workspace: None,
            boot: None,
            login: LoginKind::default(),
            cwd: None,
            vendor_session: None,
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// The app-server this holder is, for a Codex holder.
    pub fn server_key(&self) -> ServerKey {
        ServerKey {
            account: self.account,
            workspace: self.workspace,
        }
    }

    /// Notes what a launch runs, naming its variables but keeping none of
    /// their values.
    pub fn note_launch(&mut self, launch: &Launch) {
        self.cwd = Some(launch.cwd.clone());
        self.args = launch
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        self.env = launch
            .env
            .iter()
            .map(|(name, _)| name)
            .chain(launch.secret_env.iter().map(|(name, _)| name))
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
    }
}

/// One holder the manager reaches, with the driver for its CLI.
pub struct Hold {
    /// What's stored about it.
    pub record: HoldRecord,
    /// The vendor's state machine for the CLI.
    pub driver: Box<dyn Driver>,
    /// Counts connections, so events from an older one are dropped.
    pub generation: u64,
    /// Writes to the CLI, while connected.
    pub writer: Option<Arc<dyn HoldWriter>>,
    /// The newest line read.
    pub last_seq: u64,
    /// After a restart that reads the open turn again: the newest line
    /// the previous run read, which reading goes on from instead when the
    /// holder no longer has the turn's first line.
    pub read_before: Option<u64>,
    /// The CLI's process ID, while it runs.
    pub pid: Option<u32>,
    /// When the driver wants its next tick.
    pub wake_at: Option<Instant>,
    /// Set once the manager asked the holder to stop, so the exit isn't a
    /// crash.
    pub stopping: bool,
    /// Set when the manager stops the CLI to start it again with other
    /// flags, so its agent comes straight back.
    pub relaunch: bool,
    /// Claude and Cursor: the agent whose folder's files are checked before
    /// its CLI starts, and where.
    pub folder_check: Option<(AgentId, super::folders::FolderCheck)>,
    /// Claude only: the workflow variables this start found, until its CLI
    /// comes up and its agent keeps them.
    pub env_ask: Option<super::workflow_env::WorkflowEnv>,
    /// Codex only: when the app-server must have said its first line.
    pub handshake_by: Option<Instant>,
    /// Connections in a row that ended before a line arrived.
    pub retries: u32,
    /// The holder's process ID, from its last connection.
    pub holder_pid: Option<u32>,
    /// Set while the manager makes sure a holder it lost has ended.
    pub ending: bool,
    /// Why the manager couldn't confirm that a holder it lost has ended,
    /// so its agents mustn't start again on their own.
    pub stranded: Option<String>,
    /// Set while a task starts the holder or connects to it, with what
    /// comes next once it's reached.
    pub reaching: Option<Reaching>,
    /// Actions that came while the holder was being reached, in order.
    pub parked: Vec<(Target, Action)>,
    /// Set once the holder is known to have died with a restart of its
    /// device or container, so its agents come back as after a restart
    /// rather than a crash.
    pub restarted: bool,
    /// A reach or an end put off until the holder's guest link is up.
    pub waiting: Option<Waiting>,
    /// When to try a guest's holder again after it couldn't be reached.
    pub retry_at: Option<Instant>,
    /// Set when the manager stops an idle CLI to free its memory, so its
    /// agent stays as it was rather than going down.
    pub release: bool,
    /// How many of the driver's jobs haven't come back yet.
    pub jobs: usize,
    /// How the CLI ended, while that waits on the driver's jobs.
    pub exit: Option<ChildExit>,
}

/// A reach or an end put off until the holder's guest link is up.
pub struct Waiting {
    /// The holder to start first, if it isn't running yet.
    pub launch: Option<super::holders::Launch>,
    /// The line to replay from.
    pub after: u64,
    /// The agent whose history to read on the way.
    pub seed: Option<(AgentId, Source)>,
    /// Set when the holder was lost and must be ended, with how its CLI
    /// counts as having ended.
    pub end: Option<ChildExit>,
}

impl Hold {
    /// A holder not connected yet.
    pub fn new(record: HoldRecord, driver: Box<dyn Driver>) -> Self {
        Self {
            record,
            driver,
            generation: 0,
            writer: None,
            last_seq: 0,
            read_before: None,
            pid: None,
            wake_at: None,
            stopping: false,
            relaunch: false,
            folder_check: None,
            env_ask: None,
            handshake_by: None,
            retries: 0,
            holder_pid: None,
            ending: false,
            stranded: None,
            reaching: None,
            parked: Vec::new(),
            restarted: false,
            waiting: None,
            retry_at: None,
            release: false,
            jobs: 0,
            exit: None,
        }
    }
}

/// Why the manager is starting or connecting to a holder, which says what
/// to do once it's reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaching {
    /// Connecting again after the connection dropped.
    Again,
    /// Connecting to a holder a previous run of `maxd` left.
    Restored,
    /// Starting a new Claude agent for this request.
    Start(IdempotencyKey),
    /// Resuming an outside Claude session for this request.
    Continue(IdempotencyKey),
    /// Resuming a held Claude agent's session.
    Resume,
    /// Starting an account's Codex app-server. `restart` says it replaces
    /// one that crashed.
    Codex {
        /// Whether it replaces one that crashed.
        restart: bool,
    },
}

/// Which `codex app-server` a Codex thread runs on: one for each account on
/// this device, and one for each account in each container workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ServerKey {
    /// The account it runs under.
    pub account: AccountId,
    /// The container workspace it runs in, or none for this device.
    pub workspace: Option<WorkspaceId>,
}

/// One `codex app-server`.
pub struct Server {
    /// The login it runs under.
    pub login: Login,
    /// Its holder, while it has one.
    pub session: Option<SessionId>,
    /// Its recent crashes.
    pub restarts: Restarts,
    /// When to start it again after a crash.
    pub restart_at: Option<Instant>,
    /// Actions that arrived while a restart was due, with where each was
    /// going.
    pub waiting: Vec<(Target, Action)>,
    /// Set once it crashed too often to restart on its own.
    pub failed: Option<String>,
}

impl Server {
    /// An app-server that isn't running.
    pub fn new(login: Login) -> Self {
        Self {
            login,
            session: None,
            restarts: Restarts::default(),
            restart_at: None,
            waiting: Vec::new(),
            failed: None,
        }
    }
}

/// Where an action goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The driver holding this agent or its lead.
    Agent(AgentId),
    /// A Codex app-server, for starts and adopts.
    Codex(ServerKey),
}

/// An action whose agent doesn't exist until its driver answers.
pub enum Pending {
    /// A Codex thread being started.
    Start(Box<StartInfo>),
    /// An outside Codex thread being adopted.
    Adopt(Box<Held>),
}

/// What the manager needs to start an agent, and to hold a new Codex thread
/// once it exists.
pub struct StartInfo {
    /// The account it runs under.
    pub account: AccountId,
    /// The account's config folder.
    pub config_dir: PathBuf,
    /// The folder it runs in.
    pub cwd: PathBuf,
    /// The workspace for that folder.
    pub workspace: WorkspaceId,
    /// The container workspace it runs in, or none for this device.
    pub guest: Option<WorkspaceId>,
    /// How the account signs in.
    pub login: LoginKind,
    /// The branch of its worktree, in a container.
    pub branch: Option<String>,
    /// Its permission mode.
    pub mode: PermissionMode,
    /// Its model.
    pub model: Option<String>,
    /// Its effort level, or none for the vendor's default.
    pub effort: Option<String>,
    /// Its title, until the driver names it.
    pub title: String,
    /// The title you gave it, if you gave one.
    pub asked_title: Option<String>,
    /// Who started it with the agent tools, if one did.
    pub origin: Option<AgentOrigin>,
}

/// An action the manager sent on its own, with what to do once it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    /// Nothing more.
    Plain,
    /// The continue message for this agent, which it no longer owes once
    /// sent.
    Owed(AgentId),
    /// Loads this Codex agent's thread on the app-server.
    Adopt(AgentId),
    /// Stops this agent because it was settled.
    Stop(AgentId),
    /// Stops this Codex agent's thread so it loads again with the
    /// folder's files you trusted.
    Reload(AgentId),
}

/// Something to record on the agent once the driver confirms an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum After {
    /// The permission mode changed.
    Mode(PermissionMode),
    /// The model changed.
    Model(String),
    /// The effort level changed.
    Effort(Option<String>),
    /// A Codex thread stopped, so it needs adopting again.
    Stopped,
    /// A message you sent went through, which brings the agent back into
    /// the inbox.
    Sent,
    /// The agent stopped, or tried to, so it goes.
    Deleted,
}

/// An open attention event.
pub struct Open {
    /// The agent it's for.
    pub agent: AgentId,
    /// Set for the manager's own events, which no driver answers.
    pub ours: bool,
    /// When it stops taking answers.
    pub expires_at: Option<Timestamp>,
}

/// One client connection.
pub struct Viewer {
    /// The client.
    pub by: ClientId,
    /// Whether a person is behind it, rather than an agent's tool calls.
    pub person: bool,
    /// Its messages. The line holds only so many, so a client that stops
    /// reading falls behind.
    pub tx: mpsc::Sender<ServerMessage>,
    /// The timelines it has open, with the page size it asked for.
    pub open: HashMap<AgentId, usize>,
    /// What the client is owed, while it's behind.
    pub behind: Option<Behind>,
}

/// What a client that fell behind gets once it reads again.
#[derive(Default)]
pub struct Behind {
    /// Results that didn't fit, in order.
    pub results: VecDeque<ServerMessage>,
    /// Open timelines whose updates were dropped. Each gets a reset.
    pub stale: HashSet<AgentId>,
}

impl Viewer {
    /// A client that has nothing open yet.
    pub fn new(by: ClientId, person: bool, tx: mpsc::Sender<ServerMessage>) -> Self {
        Self {
            by,
            person,
            tx,
            open: HashMap::new(),
            behind: None,
        }
    }

    /// Sends the client a message. While it's behind, a timeline update is
    /// dropped and the timeline marked for a reset, and anything else
    /// waits. Returns true when this message found the client behind.
    pub fn send(&mut self, message: ServerMessage) -> bool {
        if self.behind.is_none() {
            match self.tx.try_send(message) {
                Ok(()) | Err(TrySendError::Closed(_)) => return false,
                Err(TrySendError::Full(message)) => {
                    self.behind = Some(Behind::default());
                    self.owe(message);
                    return true;
                }
            }
        }
        self.owe(message);
        false
    }

    fn owe(&mut self, message: ServerMessage) {
        let Some(behind) = &mut self.behind else {
            return;
        };
        match message {
            ServerMessage::Timeline { agent, .. } => {
                if self.open.contains_key(&agent) {
                    behind.stale.insert(agent);
                }
            }
            other => behind.results.push_back(other),
        }
    }
}

/// How one round of writes went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    /// The holder refused an answer, since another came first.
    pub already: bool,
    /// A write didn't reach the CLI.
    pub failed: bool,
}

/// A key the manager makes for its own actions.
pub fn internal_key() -> IdempotencyKey {
    IdempotencyKey::generate()
}
