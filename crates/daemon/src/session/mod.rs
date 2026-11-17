// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The session manager. It starts agents in holders, keeps a connection to
//! each holder, runs the vendor drivers, keeps each agent's timeline and
//! answers clients. When a held CLI dies, it brings the agent back on its
//! own, following decisions A-11 and A-12.
//!
//! One task owns all the state, so nothing in here locks. Each holder
//! connection reads in a task of its own and posts what it reads to that
//! task. Starting and connecting to holders, reading history files and
//! staging attachments also run off that task and post their results back.
//! Clients reach it through [`Sessions`].

mod agent_tools;
pub mod attach;
pub mod clock;
mod command_rules;
mod consent;
pub mod dispatcher;
mod drivers;
mod finds;
mod folders;
mod held;
mod hidden;
pub mod holders;
mod idem;
mod idle;
mod inbox;
mod jobs;
mod limits;
mod loading;
mod lookups;
mod move_agent;
mod move_tool;
mod moved;
mod note_tools;
mod pace;
mod places;
mod previews;
mod pulls;
mod reads;
mod recovery;
mod recycle;
mod rename;
mod requests;
mod restart;
mod revive;
mod running;
mod runs;
mod scheduled;
mod skills;
mod switch;
mod take_over;
mod timeline;
mod tools;
pub mod vendors;
mod workflow_env;

#[cfg(test)]
pub(crate) mod fake;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{
    AccountId, AgentId, AttachmentId, AttentionId, ClientId, DeviceId, IdempotencyKey, SessionId,
    WorkspaceId,
};
use maxhq_protocol::message::{ActionOutcome, AgentSpec, ClientMessage, SendMode, ServerMessage};
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, AgentPermission, Control, DeviceSettings, ModelChoice,
    PermissionMode, Vendor,
};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    AttachmentRef, Delivery, ItemBody, ItemId, ItemStatus, NoticeLevel, TimelineEvent, TimelineItem,
};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;
use uuid::Uuid;

use crate::db::{AnswerRow, Db};
use crate::drive::{ChildExit, Staged, ToolDone};
use crate::hub::{self, Hub};

use attach::Attachments;
use clock::Clock;
use finds::Finds;
use held::{
    After, Held, Hold, HoldRecord, Internal, Open, Pending, Server, ServerKey, StartInfo, Target,
    Viewer,
};
use holders::{
    AgentFiles, AgentSummary, Credentials, Ended, Event, HolderPlaces, Holders, PlaceChange,
    Workspaces,
};
use idem::{Keys, Outcome};
use maxhq_protocol::guest::Worktree;
pub use moved::ResumeMoved;
use places::Link;
pub use places::Owners;
use reads::{Read, ReadKind};
use recovery::Restarts;
use requests::GuestStart;
use restart::Winding;
use revive::Reach;
use timeline::Timeline;
use vendors::{one_process_per_agent, History, Vendors};

/// How long a change to an agent may wait before it's stored. State
/// changes that matter after a crash are stored at once.
const FLUSH_AFTER: Duration = Duration::from_secs(1);

/// How often the history of an outside session is read while a client has
/// it open.
const POLL_EVERY: Duration = Duration::from_secs(1);

/// How many lead agents a driver may report before the manager knows them.
const MAX_STRAYS: usize = 1024;

/// How many messages a client's line holds. A client whose line is full is
/// behind: its timeline updates are dropped until it has read half of
/// them, and then it gets a reset of each timeline it missed updates to.
const CLIENT_BACKLOG: usize = 1024;

/// What the manager needs to run.
pub struct Config {
    /// The shared state clients read.
    pub hub: Hub,
    /// Where held agents, holders, answers and settings are stored.
    pub db: Db,
    /// Starts and reaches holders, on this device and in each guest.
    pub holders: Arc<dyn HolderPlaces>,
    /// The vendor drivers.
    pub vendors: Arc<dyn Vendors>,
    /// The time.
    pub clock: Arc<dyn Clock>,
    /// The attachments folder.
    pub attachments: PathBuf,
    /// The folder for the timeline items not kept in memory.
    pub timelines: PathBuf,
    /// XC's workspaces: container facts and writes to `user.maxhq.agents`.
    pub workspaces: Arc<dyn Workspaces>,
    /// XH's and XI's store: which secret a token account uses.
    pub credentials: Arc<dyn Credentials>,
    /// XF's delivery of staged attachments into a container.
    pub files: Arc<dyn AgentFiles>,
    /// The folder for A-11's one-shot restart flags, `<data>/resume`.
    pub resume: PathBuf,
    /// Keeps the Mac awake when its settings say to.
    pub power: Arc<dyn crate::power::Power>,
    /// Who owns each guest session, kept current for the guest link.
    pub owners: Owners,
    /// Checks a worktree for work that isn't saved, for auto-settle.
    pub unsaved: Arc<dyn holders::Unsaved>,
    /// The holders whose ports are watched, kept current for the port
    /// watch.
    pub ports: crate::ports::HolderWatch,
    /// The line to other devices' `maxd`, for agent tool calls.
    pub peers: Arc<dyn crate::agents::PeerCalls>,
    /// This device's test hosts, for the lease tools.
    pub leases: Arc<dyn crate::leases::Leases>,
    /// Where minted access keys are written.
    pub keys: PathBuf,
    /// W's webhook URLs, which a new hook task makes and a deleted one
    /// forgets.
    pub hooks: Arc<dyn crate::hooks::Hooks>,
    /// How this device's webhooks are reached and whether they can be,
    /// for the tasks the app shows.
    pub funnel: tokio::sync::watch::Receiver<crate::hooks::Reach>,
    /// The agents' notes for the workflow follower, and the descriptor
    /// variables launches get.
    pub workflows: crate::workflow::Shared,
    /// This device's project notes, for the notes tools.
    pub notes: crate::notes::Notes,
    /// The workspace of the dispatcher's folder, on the Mac.
    pub dispatcher: Option<WorkspaceId>,
    /// The terminals the manager opens for its agents.
    pub terminals: crate::terminals::Wiring,
}

/// Starts the manager in a task of its own. It first reconnects to the
/// holders a previous run left and brings back the agents that need it.
pub fn start(config: Config) -> Sessions {
    let (tx, rx) = mpsc::unbounded_channel();
    let terminals = config.terminals.terminals.clone();
    let mut manager = Manager::new(config, tx.clone(), rx);
    // The tasks, hidden projects and names are in the state before any
    // client syncs.
    manager.publish_tasks();
    manager.load_hidden();
    manager.load_command_rules();
    manager.load_names();
    let tasks = manager.tasks.watch();
    crate::spawn_task("the session manager", manager.run());
    Sessions {
        tx,
        next: Arc::new(AtomicU64::new(0)),
        tasks,
        terminals,
    }
}

/// A manager over fake holders and drivers that refuse everything, for the
/// tests of the modules around it.
#[cfg(test)]
pub fn for_tests(hub: Hub, db: Db, attachments: PathBuf) -> Sessions {
    let notes = crate::notes::Notes::new(db.clone(), hub.clone());
    let terminals = crate::terminals::Terminals::wire(
        maxhq_hold::Terminals::new(maxhq_hold::Dirs {
            sessions: attachments.with_file_name("terminals"),
            sockets: attachments.with_file_name("holders"),
        }),
        hub.clone(),
    );
    start(Config {
        hub,
        db,
        holders: Arc::new(holders::LocalOnly(Arc::new(fake::FakeHolders::new()))),
        vendors: Arc::new(vendors::Unwired),
        clock: Arc::new(clock::SystemClock),
        resume: attachments.with_file_name("resume"),
        timelines: attachments.with_file_name("timelines"),
        keys: attachments.with_file_name(crate::agents::KEYS_DIR),
        peers: Arc::new(crate::agents::NoPeers),
        leases: Arc::new(crate::leases::NoLeases),
        attachments,
        workspaces: Arc::new(holders::NoWorkspaces),
        credentials: Arc::new(holders::NoCredentials),
        files: Arc::new(holders::NoFiles),
        power: Arc::new(crate::power::NoPower),
        owners: Owners::default(),
        unsaved: Arc::new(holders::NoUnsaved),
        ports: crate::ports::HolderWatch::default(),
        hooks: Arc::new(crate::hooks::Installed),
        funnel: tokio::sync::watch::channel(crate::hooks::Reach::default()).1,
        workflows: crate::workflow::Shared::default(),
        dispatcher: None,
        terminals,
        notes,
    })
}

/// What a peer's call reads when the manager stopped before answering.
#[cfg_attr(not(test), allow(dead_code))]
fn stopping() -> maxhq_protocol::agents::ToolAnswer {
    maxhq_protocol::agents::ToolAnswer {
        text: "MaxHQ is stopping on that device.".into(),
        failed: true,
        images: Vec::new(),
    }
}

/// A cheap handle to the manager. Clone it into every connection.
#[derive(Clone)]
pub struct Sessions {
    tx: mpsc::UnboundedSender<Input>,
    next: Arc<AtomicU64>,
    /// This device's tasks, as the manager last sent them.
    tasks: tokio::sync::watch::Receiver<Arc<[maxhq_protocol::tasks::Task]>>,
    /// The terminals this device holds for its agents, or none in tests
    /// that fake them.
    terminals: Option<crate::terminals::Terminals>,
}

impl Sessions {
    /// A new client of the manager, for a connection whose client said
    /// hello as `by`.
    pub fn client(&self, by: ClientId) -> Client {
        self.join(by, true)
    }

    /// A new client for a connection that only carries an agent's tool
    /// calls, so no person is behind what it sends.
    pub fn tool_client(&self, by: ClientId) -> Client {
        self.join(by, false)
    }

    fn join(&self, by: ClientId, person: bool) -> Client {
        let conn = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(CLIENT_BACKLOG);
        let _ = self.tx.send(Input::Join {
            conn,
            by,
            person,
            tx,
        });
        Client {
            conn,
            tx: self.tx.clone(),
            rx,
        }
    }

    /// The terminals this device holds for its agents. Connections attach
    /// to them, type into them and manage them here. Opening one goes
    /// through [`Client::open_terminal`].
    pub fn terminals(&self) -> Option<&crate::terminals::Terminals> {
        self.terminals.as_ref()
    }

    /// Has this device's Codex app-servers stop once idle, so the next
    /// send runs the Codex binary that's installed now.
    pub fn recycle_codex(&self) {
        let _ = self.tx.send(Input::RecycleCodex);
    }

    /// Lets go of the idle CLIs this device runs on `account`, after you
    /// signed in to it again, so each starts again on the new login.
    pub fn signed_in(&self, account: AccountId) {
        let _ = self.tx.send(Input::SignedIn(account));
    }

    /// Runs a call another device's `maxd` sent for a caller whose home is
    /// `from`, and gives its answer. Device links call it.
    pub async fn peer_call(
        &self,
        from: DeviceId,
        call: maxhq_protocol::agents::PeerCall,
    ) -> maxhq_protocol::agents::ToolAnswer {
        let (done, answer) = oneshot::channel();
        let later = agent_tools::Later::Peer { from, call, done };
        if self.tx.send(Input::Agents(Box::new(later))).is_err() {
            return stopping();
        }
        answer.await.unwrap_or_else(|_| stopping())
    }

    /// Hands a caller here a wake another device sent, for an agent the
    /// caller started there. Device links call it.
    pub async fn peer_wake(
        &self,
        from: DeviceId,
        wake: maxhq_protocol::agents::PeerWake,
    ) -> Result<(), String> {
        let (done, answer) = oneshot::channel();
        let later = agent_tools::Later::Wake { from, wake, done };
        if self.tx.send(Input::Agents(Box::new(later))).is_err() {
            return Err(stopping().text);
        }
        answer.await.unwrap_or_else(|_| Err(stopping().text))
    }

    /// Stores what's waiting to be stored and ends the manager. Holders
    /// keep running, so the next `maxd` reconnects to them.
    pub async fn shutdown(&self) {
        let (done, wait) = oneshot::channel();
        if self.tx.send(Input::Shutdown { done }).is_ok() {
            let _ = wait.await;
        }
    }
}

impl Sessions {
    /// Hands the manager's scheduled tasks `later`, made with where its
    /// answer goes, and waits for the answer.
    fn ask_tasks<T: Send + 'static, E: Send + 'static>(
        &self,
        later: impl FnOnce(oneshot::Sender<Result<T, E>>) -> scheduled::Later,
        stopping: fn() -> E,
    ) -> futures::future::BoxFuture<'static, Result<T, E>> {
        let (done, answer) = oneshot::channel();
        let sent = self.tx.send(Input::Tasks(Box::new(later(done)))).is_ok();
        Box::pin(async move {
            if !sent {
                return Err(stopping());
            }
            answer.await.unwrap_or_else(|_| Err(stopping()))
        })
    }

    /// Starts the PR watcher, which reads from `source` the PRs this
    /// device's agents link and the open ones in their repositories.
    pub fn watch_pulls(&self, source: Arc<dyn crate::pulls::source::PullSource>) {
        let _ = self.tx.send(Input::WatchPulls(source));
    }

    /// Shows, with each GitHub events task, why its repository can't be
    /// polled, as `problems` says.
    pub fn follow_problems(
        &self,
        problems: tokio::sync::watch::Receiver<crate::github_events::pollers::Problems>,
    ) {
        scheduled::forward(self.tx.clone(), problems, scheduled::Later::Problems);
    }

    /// Shows, with each GitHub events task, how its repository's webhook
    /// stands, as `webhooks` says.
    pub fn follow_webhooks(
        &self,
        webhooks: tokio::sync::watch::Receiver<crate::github_events::HookStates>,
    ) {
        scheduled::forward(self.tx.clone(), webhooks, scheduled::Later::Webhooks);
    }
}

/// What a caller reads when the manager stopped before answering.
const MANAGER_STOPPING: &str = "MaxHQ is stopping.";

impl crate::tasks::Tasks for Sessions {
    fn fire(
        &self,
        task: maxhq_protocol::ids::TaskId,
        fire: crate::tasks::Fire,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::tasks::FireError>> {
        self.ask_tasks(
            |done| scheduled::Later::Fire { task, fire, done },
            || crate::tasks::FireError::Failed(MANAGER_STOPPING.into()),
        )
    }

    fn watch(&self) -> tokio::sync::watch::Receiver<Arc<[maxhq_protocol::tasks::Task]>> {
        self.tasks.clone()
    }
}

impl crate::hooks::HookEdits for Sessions {
    fn edit_hook(
        &self,
        task: maxhq_protocol::ids::TaskId,
        edit: Box<dyn FnOnce(&mut maxhq_protocol::tasks::Hook) + Send>,
    ) -> futures::future::BoxFuture<'_, Result<(), String>> {
        self.ask_tasks(
            |done| scheduled::Later::EditHook { task, edit, done },
            || MANAGER_STOPPING.to_owned(),
        )
    }
}

/// One connection's line to the manager. Dropping it closes the
/// connection's timelines.
pub struct Client {
    conn: u64,
    tx: mpsc::UnboundedSender<Input>,
    rx: mpsc::Receiver<ServerMessage>,
}

impl Client {
    /// Hands the manager a message to act on. The answer, if there is one,
    /// comes from [`Client::recv`].
    pub fn send(&self, message: ClientMessage) {
        let _ = self.tx.send(Input::Message {
            conn: self.conn,
            message: Box::new(message),
        });
    }

    /// Asks the manager to open the terminal `message` asks for, for a
    /// connection allowed to, which opens it as `opener`.
    pub fn open_terminal(&self, message: ClientMessage, opener: crate::terminals::access::Opener) {
        let _ = self.tx.send(Input::TerminalOpen {
            conn: self.conn,
            opener,
            message: Box::new(message),
        });
    }

    /// The manager's number for this connection, which terminals know it
    /// by.
    pub fn conn(&self) -> u64 {
        self.conn
    }

    /// The next message for the client, or none once the manager ended.
    pub async fn recv(&mut self) -> Option<ServerMessage> {
        self.rx.recv().await
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.tx.send(Input::Gone { conn: self.conn });
    }
}

/// What reaches the manager's loop.
enum Input {
    /// A picture's preview was made off the loop, or couldn't be.
    Preview {
        agent: AgentId,
        id: AttachmentId,
        preview: Option<String>,
    },
    /// A connection started.
    Join {
        conn: u64,
        by: ClientId,
        person: bool,
        tx: mpsc::Sender<ServerMessage>,
    },
    /// A connection sent a message.
    Message {
        conn: u64,
        message: Box<ClientMessage>,
    },
    /// A connection the server let open a terminal asked for one.
    TerminalOpen {
        conn: u64,
        opener: crate::terminals::access::Opener,
        message: Box<ClientMessage>,
    },
    /// The terminals said something about one they opened or a `!` run.
    Terminal(crate::terminals::RunNote),
    /// A connection ended.
    Gone { conn: u64 },
    /// A client that fell behind read half its line.
    Drained { conn: u64 },
    /// A holder connection read an event, or ended when `event` is none.
    Held {
        session: SessionId,
        generation: u64,
        event: Option<Event>,
    },
    /// A holder can't be reached, so its CLI counts as ended.
    Lost { session: SessionId },
    /// The manager made sure a holder it lost has ended, as far as it
    /// could. `exit` is how its CLI counts as having ended.
    Ended {
        session: SessionId,
        exit: ChildExit,
        ended: Ended,
    },
    /// A task finished starting or connecting to a holder. `seed` is the
    /// history it read for the holder's agent, when it was asked to.
    Reached {
        session: SessionId,
        reach: Reach,
        seed: Option<io::Result<Vec<TimelineItem>>>,
        verdict: Option<Box<folders::Verdict>>,
    },
    /// The folder a Codex thread loads in was checked, so the action that
    /// loads it goes on. None when the check couldn't finish.
    FolderChecked {
        target: Target,
        action: Box<Action>,
        judged: Option<Box<folders::Judged>>,
    },
    /// A history read finished.
    Read(Box<Read>),
    /// A held agent's timeline was read after a restart, or couldn't be.
    Loaded {
        agent: AgentId,
        timeline: Option<Box<Timeline>>,
    },
    /// The folder a new agent asked to run in was looked up.
    Located {
        key: IdempotencyKey,
        spec: Box<AgentSpec>,
        folder: Result<(PathBuf, WorkspaceId), String>,
    },
    /// An attachment finished staging.
    Staged {
        key: IdempotencyKey,
        result: Result<AttachmentRef, String>,
    },
    /// A guest link went up or down.
    Place(PlaceChange),
    /// A new agent's worktree in its container was made and its first
    /// message's attachments delivered, or one of them failed.
    Prepared {
        key: IdempotencyKey,
        start: Box<GuestStart>,
        result: Result<(Worktree, Vec<Staged>), String>,
    },
    /// A message's attachments reached its agent's container, or didn't.
    Delivered {
        key: IdempotencyKey,
        lead: AgentId,
        text: String,
        refs: Vec<InlineRef>,
        mode: SendMode,
        result: Result<Vec<Staged>, String>,
    },
    /// A lead agent's activity changed.
    Moved(hub::Moved),
    /// The CLI of an agent MaxHQ takes over stopped in the terminal, or
    /// didn't.
    TakenOver {
        agent: AgentId,
        result: Result<(), String>,
    },
    /// Auto-settle's check of an agent's worktree finished. `unsaved` is
    /// none when it couldn't tell.
    Checked {
        agent: AgentId,
        unsaved: Option<bool>,
    },
    /// A call to one of MaxHQ's tools finished, for the driver of
    /// `session`.
    ToolDone { session: SessionId, done: ToolDone },
    /// A job finished, for the driver of `session`. `done` is none when it
    /// failed.
    JobDone {
        session: SessionId,
        done: Option<crate::drive::JobDone>,
    },
    /// Something came back for the agent tools.
    Agents(Box<agent_tools::Later>),
    /// Something came for scheduled tasks.
    Tasks(Box<scheduled::Later>),
    /// A lookup a connection asked for finished.
    Found {
        conn: u64,
        key: IdempotencyKey,
        outcome: Box<ActionOutcome>,
    },
    /// The PR watcher starts, reading from this source.
    WatchPulls(Arc<dyn crate::pulls::source::PullSource>),
    /// The PR watcher read something new.
    Pulls(Vec<crate::pulls::watcher::Found>),
    /// What this device's holders run besides their CLIs changed.
    Jobs(crate::ports::Jobs),
    /// Codex was updated, so this device's app-servers stop once idle.
    RecycleCodex,
    /// You signed in to this account again, so its idle CLIs stop.
    SignedIn(AccountId),
    /// The skills in these checkouts changed.
    SkillsChanged(Vec<crate::skills::deliver::Changed>),
    /// A move's switch resumes its agent in the copy.
    ResumeMoved {
        resume: moved::ResumeMoved,
        done: moved::Done,
    },
    /// A move back stops the agents in the copy.
    StopIn {
        workspace: WorkspaceId,
        done: moved::Done,
    },
    /// Something came for moving an agent between devices.
    Move(Box<move_agent::Later>),
    /// `maxd` is stopping.
    Shutdown { done: oneshot::Sender<()> },
    /// Another device said which agents here its enabled tasks reach.
    TaskTargets {
        from: DeviceId,
        agents: Vec<AgentId>,
    },
}

/// The manager's state, owned by its task.
struct Manager {
    hub: Hub,
    db: Db,
    holders: Arc<dyn Holders>,
    vendors: Arc<dyn Vendors>,
    clock: Arc<dyn Clock>,
    attachments: Attachments,
    device: DeviceId,
    rx: mpsc::UnboundedReceiver<Input>,
    tx: mpsc::UnboundedSender<Input>,
    /// Holders on this device and in each guest.
    places: Arc<dyn HolderPlaces>,
    /// Container workspaces.
    workspaces: Arc<dyn Workspaces>,
    /// The secret store.
    credentials: Arc<dyn Credentials>,
    /// Delivery of attachments into containers.
    files: Arc<dyn AgentFiles>,
    /// Keeps the Mac awake when its settings say to.
    power: Arc<dyn crate::power::Power>,
    /// Who owns each guest session.
    owners: Owners,
    /// Checks worktrees for unsaved work.
    unsaved: Arc<dyn holders::Unsaved>,
    /// Snoozes and auto-settle's checks.
    inbox: inbox::Inboxes,
    /// Previews of pictures in tool results, made off the loop.
    previews: previews::Previews,
    /// Drivers' slow work, run off the loop.
    jobs: jobs::Jobs,
    /// The repositories the PR watcher reads at its next pass.
    pulls_due: crate::pulls::watcher::Due,
    /// The holders whose ports are watched.
    ports: crate::ports::HolderWatch,
    /// What each holder on this device runs besides its CLI, as the port
    /// poll last found it.
    jobs_running: crate::ports::Jobs,
    /// Where restart flags are kept.
    resume_dir: PathBuf,
    /// Where timelines put the items they don't keep in memory.
    timeline_dir: PathBuf,
    /// What each timeline keeps, from this device's settings.
    limits: timeline::Limits,
    /// The timelines still read after a restart.
    loading: loading::Loading,
    /// Permissions, access keys and calls in flight for the agent tools.
    agent_tools: agent_tools::AgentTools,
    /// Scheduled tasks and their runs.
    tasks: scheduled::Book,
    /// The dispatcher behind the header's text field.
    dispatcher: dispatcher::Dispatch,
    /// What the workflow follower reads of agents, and what launches get
    /// from descriptors.
    workflows: crate::workflow::Shared,
    /// This device's project notes, which the notes tools read and write.
    notes: crate::notes::Notes,
    /// The terminals the manager opens for its agents.
    terminals: Arc<dyn crate::terminals::Host>,
    /// What the terminals tell the manager, until its loop takes it.
    terminal_notes: Option<mpsc::UnboundedReceiver<crate::terminals::RunNote>>,
    /// Who asked for each terminal being opened, and on which connection.
    openers: HashMap<IdempotencyKey, (crate::terminals::access::Opener, u64)>,

    /// Each connection, by its number.
    clients: HashMap<u64, Viewer>,
    /// Client keys in flight and recently finished.
    keys: Keys<u64>,
    /// Attachments staged by this run, which keep the type they were
    /// staged with.
    staged: HashMap<AttachmentId, AttachmentRef>,

    /// Each holder this run reaches.
    holds: HashMap<SessionId, Hold>,
    /// Each project's command rules.
    command_rules: command_rules::Rules,
    /// Each lead agent held.
    held: HashMap<AgentId, Held>,
    /// Each held sub-agent's lead.
    parents: HashMap<AgentId, AgentId>,
    /// Each held sub-agent, as its driver reported it.
    subs: HashMap<AgentId, Agent>,
    /// Each agent as clients were last sent it, and what waits.
    pace: pace::Pace,
    /// Lead agents a driver reported before the manager knew them, such as
    /// a Codex thread reported before its start finished.
    strays: HashMap<AgentId, Agent>,
    /// Each Codex app-server, by account and workspace.
    servers: HashMap<ServerKey, Server>,
    /// App-servers to stop once idle, so they run a new Codex binary.
    recycle: HashSet<ServerKey>,
    /// Agents to start again once idle so they see new skills, with the
    /// container and checkout each is in.
    skill_restarts: HashMap<AgentId, (Option<WorkspaceId>, PathBuf)>,
    /// The agents kept from their folder's files, as stored.
    restricted: HashMap<AgentId, folders::Restricted>,
    /// What each Codex load in flight found in its folder, with its agent
    /// once known, for when Codex has the thread.
    codex_checks: HashMap<IdempotencyKey, (Option<AgentId>, Box<folders::Judged>)>,
    /// The agents to start again once they're idle, since you trusted their
    /// folder.
    trust_restarts: HashSet<AgentId>,
    /// The ones of those with no transcript yet, which start anew rather
    /// than resume.
    trust_fresh: HashSet<AgentId>,
    /// Moves waiting for the agent they resume to be up.
    moved: HashMap<IdempotencyKey, moved::Done>,
    /// Agents that move between devices.
    moves: move_agent::Moves,
    /// Each container workspace's guest link, as last heard.
    links: HashMap<WorkspaceId, Link>,
    /// The agent summaries each container workspace was last given.
    summaries: HashMap<WorkspaceId, Vec<AgentSummary>>,
    /// Agents that wait for a limit to reset before they resume, and when.
    limit_waits: HashMap<AgentId, Instant>,
    /// The wind-down in progress, if any.
    winding: Option<Winding>,
    /// Each agent's timeline, held or open from outside.
    timelines: HashMap<AgentId, Timeline>,
    /// History readers for outside sessions a client has open. A reader is
    /// none while a read has it.
    outside: HashMap<AgentId, Option<Box<dyn History>>>,
    /// Outside agents whose CLI MaxHQ stops in the terminal to take them
    /// over, or waits to.
    take_overs: HashMap<AgentId, take_over::TakeOver>,
    /// The newest history read in flight for each agent, and what it's for.
    reads: HashMap<AgentId, (u64, ReadKind)>,
    /// The last read's number.
    next_read: u64,
    /// The last seq of each timeline dropped this run, so a new one starts
    /// above it.
    retired: HashMap<AgentId, u64>,
    /// What find keeps of the timelines it looked through.
    finds: Finds,

    /// Open attention events for held agents.
    attention: HashMap<AttentionId, Open>,
    /// Answers already given.
    answers: HashMap<AttentionId, AnswerRow>,

    /// Actions whose agent the manager creates once they finish.
    pending: HashMap<IdempotencyKey, Pending>,
    /// Actions the manager sent on its own.
    internal: HashMap<IdempotencyKey, Internal>,
    /// Actions that run once another finishes.
    followups: HashMap<IdempotencyKey, Vec<(Target, Action)>>,
    /// Answers in flight, with the event and who answered.
    answering: HashMap<IdempotencyKey, (AttentionId, ClientId)>,
    /// What to record once an action is confirmed.
    after: HashMap<IdempotencyKey, (AgentId, After)>,
    /// The adopt in flight for each Codex agent.
    adopting: HashMap<AgentId, IdempotencyKey>,
    /// Actions a driver took without a result yet, with their holder.
    routed: HashMap<IdempotencyKey, SessionId>,

    /// Actions to hand to drivers, in order.
    work: VecDeque<(Target, Action)>,
    /// Agents changed since they were stored.
    dirty: HashSet<AgentId>,
    /// Holders read since their seq was stored.
    dirty_seq: HashSet<SessionId>,
    /// Holders the manager lost and couldn't confirm have ended. Their
    /// rows stay stored, so a restarted `maxd` tries to end them again,
    /// until a new holder replaces them.
    stranded: HashMap<SessionId, HoldRecord>,
    /// Recent ends of each Claude agent's CLI that cut nothing off.
    idle_ends: HashMap<AgentId, Restarts>,
    /// What finds agents idle long enough to stop their CLIs.
    idle: idle::Idle,
    /// When to store what's dirty.
    flush_at: Option<Instant>,
    /// When to read outside histories next.
    next_poll: Option<Instant>,
}

use crate::drive::Action;

impl Manager {
    fn new(
        config: Config,
        tx: mpsc::UnboundedSender<Input>,
        rx: mpsc::UnboundedReceiver<Input>,
    ) -> Self {
        let device = config.hub.device().id;
        let agent_tools =
            agent_tools::AgentTools::load(&config.db, config.peers, config.leases, config.keys);
        let tasks = scheduled::Book::load(&config.db, config.hooks, config.funnel);
        Self {
            agent_tools,
            tasks,
            dispatcher: dispatcher::Dispatch::new(config.dispatcher),
            workflows: config.workflows,
            notes: config.notes,
            terminals: config.terminals.host,
            terminal_notes: Some(config.terminals.notes),
            openers: HashMap::new(),
            hub: config.hub,
            db: config.db,
            holders: config.holders.local(),
            vendors: config.vendors,
            clock: config.clock,
            attachments: Attachments::new(config.attachments),
            device,
            rx,
            tx,
            places: config.holders,
            workspaces: config.workspaces,
            credentials: config.credentials,
            files: config.files,
            power: config.power,
            owners: config.owners,
            unsaved: config.unsaved,
            inbox: inbox::Inboxes::default(),
            previews: previews::Previews::default(),
            jobs: jobs::Jobs::default(),
            pulls_due: crate::pulls::watcher::Due::default(),
            ports: config.ports,
            jobs_running: crate::ports::Jobs::new(),
            resume_dir: config.resume,
            timeline_dir: config.timelines,
            limits: timeline::Limits::default(),
            loading: loading::Loading::default(),
            clients: HashMap::new(),
            keys: Keys::new(),
            staged: HashMap::new(),
            holds: HashMap::new(),
            command_rules: command_rules::Rules::new(),
            held: HashMap::new(),
            parents: HashMap::new(),
            subs: HashMap::new(),
            pace: pace::Pace::default(),
            strays: HashMap::new(),
            servers: HashMap::new(),
            recycle: HashSet::new(),
            skill_restarts: HashMap::new(),
            restricted: HashMap::new(),
            codex_checks: HashMap::new(),
            trust_restarts: HashSet::new(),
            trust_fresh: HashSet::new(),
            moved: HashMap::new(),
            moves: move_agent::Moves::default(),
            links: HashMap::new(),
            summaries: HashMap::new(),
            limit_waits: HashMap::new(),
            winding: None,
            timelines: HashMap::new(),
            outside: HashMap::new(),
            take_overs: HashMap::new(),
            reads: HashMap::new(),
            next_read: 0,
            retired: HashMap::new(),
            finds: Finds::default(),
            attention: HashMap::new(),
            answers: HashMap::new(),
            pending: HashMap::new(),
            internal: HashMap::new(),
            followups: HashMap::new(),
            answering: HashMap::new(),
            after: HashMap::new(),
            adopting: HashMap::new(),
            routed: HashMap::new(),
            work: VecDeque::new(),
            dirty: HashSet::new(),
            dirty_seq: HashSet::new(),
            stranded: HashMap::new(),
            idle_ends: HashMap::new(),
            idle: idle::Idle::default(),
            flush_at: None,
            next_poll: None,
        }
    }

    /// Restores the previous run, then handles inputs and timers until
    /// `maxd` stops. It watches the guest links first, so a workspace
    /// deleted while it restores is still heard of.
    async fn run(mut self) {
        let mut moves = self.hub.watch_moves();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(moved) = moves.recv().await {
                if tx.send(Input::Moved(moved)).is_err() {
                    return;
                }
            }
        });
        if let Some(mut notes) = self.terminal_notes.take() {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                while let Some(note) = notes.recv().await {
                    if tx.send(Input::Terminal(note)).is_err() {
                        return;
                    }
                }
            });
        }
        let mut jobs = self.ports.jobs();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while jobs.changed().await.is_ok() {
                let now = jobs.borrow_and_update().clone();
                if tx.send(Input::Jobs(now)).is_err() {
                    return;
                }
            }
        });
        self.load_inboxes();
        self.load_pulls();
        let mut changes = self.places.changes();
        self.restore().await;
        self.restore_dispatcher();
        self.start_tasks();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            while let Some(change) = changes.next().await {
                if tx.send(Input::Place(change)).is_err() {
                    return;
                }
            }
        });
        loop {
            while let Some((target, action)) = self.work.pop_front() {
                self.dispatch(target, action).await;
            }
            self.check_winding();
            self.recycle_idle();
            self.release_idle().await;
            self.restart_for_skills();
            let queued = self.work.len();
            self.restart_for_trust();
            self.sync();
            // A Codex thread let go to load its files goes before any wait.
            if self.work.len() > queued {
                continue;
            }
            let sleep = match self.deadline() {
                Some(at) => self.clock.sleep_until(at),
                None => Box::pin(std::future::pending()),
            };
            tokio::select! {
                input = self.rx.recv() => match input {
                    Some(Input::Shutdown { done }) => {
                        self.flush();
                        let _ = done.send(());
                        return;
                    }
                    Some(input) => self.input(input).await,
                    None => {
                        self.flush();
                        return;
                    }
                },
                () = sleep => self.timers().await,
            }
        }
    }

    /// The soonest timer. A holder being reached has none until it is.
    fn deadline(&self) -> Option<Instant> {
        let holds = self
            .holds
            .values()
            .filter(|hold| hold.reaching.is_none())
            .flat_map(|hold| [hold.wake_at, hold.handshake_by])
            .flatten();
        let retries = self.holds.values().filter_map(|hold| hold.retry_at);
        let servers = self.servers.values().filter_map(|server| server.restart_at);
        holds
            .chain(retries)
            .chain(servers)
            .chain(self.limit_waits.values().copied())
            .chain(self.carry_on_deadline())
            .chain(self.winding_until())
            .chain(self.flush_at)
            .chain(self.next_poll)
            .chain(self.inbox_deadline())
            .chain(self.agent_tools.deadline())
            .chain(self.moves.deadline())
            .chain(self.task_deadline())
            .chain(self.idle.deadline())
            .chain(self.pace.deadline())
            .min()
    }

    /// Runs every timer that's due.
    async fn timers(&mut self) {
        let now = self.clock.now();
        let due: Vec<SessionId> = self
            .holds
            .iter()
            .filter(|(_, hold)| hold.reaching.is_none() && hold.wake_at.is_some_and(|at| at <= now))
            .map(|(session, _)| *session)
            .collect();
        for session in due {
            let Some(hold) = self.holds.get_mut(&session) else {
                continue;
            };
            hold.wake_at = None;
            let mut fx = crate::drive::Effects::default();
            hold.driver.tick(now, &mut fx);
            self.apply_fx(session, fx).await;
        }

        let late: Vec<SessionId> = self
            .holds
            .iter()
            .filter(|(_, hold)| {
                hold.reaching.is_none() && hold.handshake_by.is_some_and(|at| at <= now)
            })
            .map(|(session, _)| *session)
            .collect();
        for session in late {
            warn!(%session, "the Codex app-server didn't answer in time, so maxd restarts it");
            self.end_hold(session, false);
        }

        let restarts: Vec<ServerKey> = self
            .servers
            .iter()
            .filter(|(_, server)| server.restart_at.is_some_and(|at| at <= now))
            .map(|(key, _)| *key)
            .collect();
        for key in restarts {
            self.restart_codex(key);
        }
        self.retry_due();
        self.limits_reset();
        self.carry_ons_due();
        self.inbox_timers();
        self.settle_waits();
        self.task_timers();
        self.send_due();

        if self.next_poll.is_some_and(|at| at <= now) {
            self.poll_outside();
            self.next_poll = (!self.outside.is_empty()).then(|| now + POLL_EVERY);
        }
        if self.flush_at.is_some_and(|at| at <= now) {
            self.flush();
        }
    }

    /// Handles one input.
    async fn input(&mut self, input: Input) {
        match input {
            Input::Join {
                conn,
                by,
                person,
                tx,
            } => {
                self.clients.insert(conn, Viewer::new(by, person, tx));
            }
            Input::Message { conn, message } => self.message(conn, *message),
            Input::TerminalOpen {
                conn,
                opener,
                message,
            } => self.terminal_open(conn, opener, *message),
            Input::Terminal(note) => self.run_note(note),
            Input::Gone { conn } => {
                self.agent_tools.gone(conn);
                self.gone_client(conn);
            }
            Input::Drained { conn } => self.drained(conn),
            Input::Held {
                session,
                generation,
                event,
            } => self.held_event(session, generation, event).await,
            Input::Lost { session } => {
                // A holder being reached counts as lost only if the task
                // reaching it can't.
                if self
                    .holds
                    .get(&session)
                    .is_some_and(|hold| hold.writer.is_none() && hold.reaching.is_none())
                {
                    self.lost(session);
                }
            }
            Input::Ended {
                session,
                exit,
                ended,
            } => self.ended(session, exit, ended).await,
            Input::Reached {
                session,
                reach,
                seed,
                verdict,
            } => self.reached(session, reach, seed, verdict).await,
            Input::FolderChecked {
                target,
                action,
                judged,
            } => self.codex_checked(target, *action, judged),
            Input::Read(read) => self.landed(*read),
            Input::Loaded { agent, timeline } => self.loaded(agent, timeline),
            Input::Located { key, spec, folder } => {
                if let Some(outcome) = self.located(key, *spec, Some(folder)) {
                    self.finish(key, Outcome::Action(outcome));
                }
            }
            Input::Staged { key, result } => {
                let outcome = match result {
                    Ok(attachment) => {
                        self.staged.insert(attachment.id, attachment.clone());
                        ActionOutcome::Staged { attachment }
                    }
                    Err(reason) => ActionOutcome::Refused { reason },
                };
                self.finish(key, Outcome::Action(outcome));
            }
            Input::Place(change) => self.place_change(change),
            Input::Prepared { key, start, result } => {
                if let Some(outcome) = self.prepared(key, *start, result) {
                    self.finish(key, Outcome::Action(outcome));
                }
            }
            Input::Delivered {
                key,
                lead,
                text,
                refs,
                mode,
                result,
            } => {
                if let Some(outcome) = self.delivered(key, lead, (text, refs), mode, result) {
                    self.finish(key, Outcome::Action(outcome));
                }
            }
            Input::Moved(moved) => {
                self.agent_moved(moved.agent, moved.from.as_ref(), &moved.to);
                self.continue_at_limit(&moved);
                self.take_over_moved(&moved);
                self.inbox_moved(moved);
            }
            Input::TakenOver { agent, result } => self.taken_over(agent, result),
            Input::Checked { agent, unsaved } => self.checked(agent, unsaved),
            Input::ToolDone { session, done } => self.tool_done(session, done).await,
            Input::JobDone { session, done } => self.driver_job_done(session, done).await,
            Input::Preview { agent, id, preview } => self.preview_made(agent, id, preview),
            Input::Agents(later) => self.agent_later(*later).await,
            Input::Tasks(later) => self.task_later(*later),
            Input::Found { conn, key, outcome } => self.found(conn, key, *outcome),
            Input::WatchPulls(source) => self.watch_pulls(source),
            Input::Pulls(found) => self.pulls_found(found),
            Input::Jobs(jobs) => self.jobs_changed(jobs),
            Input::RecycleCodex => self.recycle_codex(),
            Input::SignedIn(account) => self.signed_in(account).await,
            Input::SkillsChanged(changed) => self.skills_changed(changed).await,
            Input::ResumeMoved { resume, done } => self.resume_moved(resume, done),
            Input::StopIn { workspace, done } => self.stop_in(workspace, done),
            Input::Move(later) => self.move_later(*later),
            // The loop handles it before it gets here.
            Input::Shutdown { done } => {
                let _ = done.send(());
            }
            Input::TaskTargets { from, agents } => self.heard_task_targets(from, agents),
        }
    }

    /// Stores every agent and holder seq that changed. A holder's seq goes
    /// with its driver's checkpoint, so the two always match.
    fn flush(&mut self) {
        self.flush_at = None;
        for agent in std::mem::take(&mut self.dirty) {
            self.store(agent);
        }
        for session in std::mem::take(&mut self.dirty_seq) {
            if let Some(hold) = self.holds.get(&session)
                && let Err(err) =
                    self.db
                        .set_holder_seq(session, hold.last_seq, hold.driver.checkpoint())
            {
                warn!(%err, %session, "couldn't store how far the holder was read");
            }
        }
    }

    /// Stores the agent now.
    fn save(&mut self, agent: AgentId) {
        self.dirty.remove(&agent);
        self.store(agent);
    }

    /// Stores the agent with the messages queued on it, which are only
    /// MaxHQ's until they're sent.
    fn store(&mut self, agent: AgentId) {
        let queued: Vec<TimelineItem> = self
            .timelines
            .get(&agent)
            .map(|timeline| {
                timeline
                    .kept()
                    .filter(|item| is_queued(item))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(held) = self.held.get_mut(&agent) {
            held.queued = queued;
            if let Err(err) = self.db.put_held(agent, held) {
                warn!(%err, %agent, "couldn't store the agent");
            }
        }
    }

    /// Stores the agent within [`FLUSH_AFTER`].
    fn mark_dirty(&mut self, agent: AgentId) {
        self.dirty.insert(agent);
        self.flush_soon();
    }

    fn flush_soon(&mut self) {
        if self.flush_at.is_none() {
            self.flush_at = Some(self.clock.now() + FLUSH_AFTER);
        }
    }

    /// Stores a holder so a restarted `maxd` finds it.
    fn store_holder(&self, session: SessionId, record: &HoldRecord) {
        if let Err(err) = self.db.put_holder(session, record) {
            warn!(%err, %session, "couldn't store the holder");
        }
    }

    /// Tells every waiter on a key how it ended.
    fn finish(&mut self, key: IdempotencyKey, outcome: Outcome) {
        self.agent_action_done(key, &outcome);
        self.moved_done(key, &outcome);
        self.move_done(key, &outcome);
        self.dispatch_done(key, &outcome);
        let message = outcome.message(key);
        for conn in self.keys.finish(key, outcome) {
            self.tell(conn, message.clone());
        }
    }

    /// Sends one connection a message.
    fn tell(&mut self, conn: u64, message: ServerMessage) {
        if let Some(viewer) = self.clients.get_mut(&conn)
            && viewer.send(message)
        {
            wake_when_read(conn, viewer, &self.tx);
        }
    }

    /// Sends a client that fell behind what it missed: the results that
    /// waited, then a reset of each timeline whose updates were dropped.
    fn drained(&mut self, conn: u64) {
        let Some(viewer) = self.clients.get_mut(&conn) else {
            return;
        };
        let Some(behind) = viewer.behind.take() else {
            return;
        };
        let mut owed: Vec<ServerMessage> = behind.results.into();
        for agent in behind.stale {
            // A read still filling the timeline resets it once it's done.
            let filling = self.loading.waits(agent)
                || self
                    .reads
                    .get(&agent)
                    .is_some_and(|(_, kind)| *kind == ReadKind::Outside);
            if let (Some(limit), Some(timeline), false) =
                (viewer.open.get(&agent), self.timelines.get(&agent), filling)
            {
                owed.push(ServerMessage::Timeline {
                    agent,
                    update: timeline.reset(*limit),
                });
            }
        }
        for message in owed {
            if viewer.send(message) {
                wake_when_read(conn, viewer, &self.tx);
            }
        }
    }

    /// Posts an input to the loop, for after the current one.
    fn post(&self, input: Input) {
        let _ = self.tx.send(input);
    }

    /// The lead an agent belongs to, which is itself for a lead.
    fn lead_of(&self, agent: AgentId) -> AgentId {
        self.parents.get(&agent).copied().unwrap_or(agent)
    }

    /// The holder a held agent runs in: its own for Claude and ACP agents,
    /// or its account's app-server for Codex.
    fn session_of(&self, held: &Held) -> Option<SessionId> {
        match held.vendor() {
            vendor if one_process_per_agent(vendor) => held.session,
            _ => self
                .servers
                .get(&held.server_key())
                .and_then(|server| server.session),
        }
        .filter(|session| self.holds.contains_key(session))
    }

    /// The lead agents a holder runs.
    fn leads_of(&self, session: SessionId) -> Vec<AgentId> {
        let Some(hold) = self.holds.get(&session) else {
            return Vec::new();
        };
        match hold.record.vendor {
            vendor if one_process_per_agent(vendor) => hold
                .record
                .agent
                .filter(|agent| self.held.contains_key(agent))
                .into_iter()
                .collect(),
            _ => self.codex_leads(hold.record.server_key()),
        }
    }

    /// Every held Codex agent on an app-server.
    fn codex_leads(&self, key: ServerKey) -> Vec<AgentId> {
        self.held
            .values()
            .filter(|held| held.vendor() == Vendor::Codex && held.server_key() == key)
            .map(|held| held.agent.id)
            .collect()
    }

    /// The held sub-agents of a lead.
    fn subs_of(&self, lead: AgentId) -> Vec<AgentId> {
        self.parents
            .iter()
            .filter(|(_, parent)| **parent == lead)
            .map(|(sub, _)| *sub)
            .collect()
    }

    /// The agent as clients see it: its driver's report with the manager's
    /// own state laid over it.
    fn view(&self, lead: AgentId) -> Option<Agent> {
        let held = self.held.get(&lead)?;
        let session = self.session_of(held);
        let hold = session.and_then(|session| self.holds.get(&session));
        let mut agent = held.agent.clone();
        agent.control = Control::Held;
        agent.session = session;
        agent.pid = hold.and_then(|hold| hold.pid);
        let link = held.guest.and_then(|guest| self.links.get(&guest));
        agent.link = if let Some((reason, since)) = link.and_then(|link| link.down.clone()) {
            AgentLink::Down {
                reason,
                since: Some(since),
            }
        } else if held.released || hold.is_some_and(|hold| hold.writer.is_some()) {
            // An agent whose idle CLI stopped shows as it ran, marked asleep.
            AgentLink::Live
        } else {
            AgentLink::Down {
                reason: "The agent's process isn't running".into(),
                since: held.down_since,
            }
        };
        agent.asleep = held.released;
        agent.running = self.running(&agent, session);
        agent.permission = Some(AgentPermission {
            requested: held.mode,
            effective: agent.permission.as_ref().and_then(|p| p.effective),
        });
        if agent.model.is_none() {
            agent.model.clone_from(&held.model);
        }
        agent.effort.clone_from(&held.effort);
        if let Some(title) = &held.title {
            agent.title.clone_from(title);
        }
        agent.account = Some(held.account);
        agent.origin = held.origin;
        agent.moved.clone_from(&held.moved);
        // A Cursor agent's session is named once its CLI makes it.
        agent.vendor_session_id =
            Some(held.vendor_session.clone()).filter(|session| !session.is_empty());
        if agent.branch.is_none() {
            agent.branch.clone_from(&held.branch);
        }
        if let Some(reason) = held.paused {
            agent.activity = Activity::Paused { reason };
        } else if let Some(reason) = &held.fault {
            agent.activity = Activity::Error {
                reason: reason.clone(),
            };
        } else if held.resuming || hold.is_some_and(|hold| hold.restarted) {
            // A CLI that died with its machine comes back as A-11 says, so
            // what it last reported no longer holds.
            agent.activity = Activity::Starting;
        } else if link.is_some_and(|link| link.stopped) {
            // Nothing runs it while its container is stopped.
            agent.activity = Activity::NotAccessible;
        }
        Some(agent)
    }

    /// Publishes a lead and its sub-agents as clients see them, at the
    /// pace [`pace`] sets.
    fn publish(&self, lead: AgentId) {
        let Some(agent) = self.view(lead) else {
            return;
        };
        let mut agents = Vec::new();
        for sub in self.subs_of(lead) {
            if let Some(sub) = self.subs.get(&sub) {
                let mut sub = sub.clone();
                sub.control = Control::Held;
                sub.session = agent.session;
                sub.pid = agent.pid;
                sub.link = agent.link.clone();
                sub.account = agent.account;
                agents.push(sub);
            }
        }
        agents.insert(0, agent);
        self.send_agents(self.pace.pass(agents, self.clock.now()));
    }

    /// The seq a new timeline for the agent starts after. It's the time in
    /// microseconds, so a seq a client kept from an earlier run never
    /// matches, and it's above any timeline this run dropped.
    fn base(&self, agent: AgentId) -> u64 {
        let now = self.clock.wall().0.saturating_mul(1000);
        let retired = self.retired.get(&agent).map_or(0, |seq| seq + 1);
        now.max(retired)
    }

    /// An empty timeline for the agent, as [`Manager::timeline_mut`] makes
    /// one.
    fn new_timeline(&self, agent: AgentId) -> Timeline {
        Timeline::new(self.base(agent))
            .on(self.device)
            .noting(agent, self.workflows.notes.clone())
            .spilling(self.timeline_dir.clone())
            .limited(self.limits)
    }

    /// The agent's timeline, made empty if it has none.
    fn timeline_mut(&mut self, agent: AgentId) -> &mut Timeline {
        let base = self.base(agent);
        let device = self.device;
        let notes = self.workflows.notes.clone();
        let dir = &self.timeline_dir;
        let limits = self.limits;
        self.timelines.entry(agent).or_insert_with(|| {
            Timeline::new(base)
                .on(device)
                .noting(agent, notes)
                .spilling(dir.clone())
                .limited(limits)
        })
    }

    /// Keeps every timeline, and each CLI's own conversation, to what
    /// `settings` say from now on.
    fn keep_to(&mut self, settings: &DeviceSettings) {
        let limits = timeline::Limits::of(settings);
        if limits == self.limits {
            return;
        }
        self.limits = limits;
        for timeline in self.timelines.values_mut() {
            timeline.limit(limits);
        }
        for hold in self.holds.values_mut() {
            hold.driver.keep_items(limits.items);
        }
    }

    /// An agent to show until the driver reports one.
    fn placeholder(
        &self,
        id: AgentId,
        vendor: Vendor,
        info: &StartInfo,
        vendor_session: Option<String>,
        activity: Activity,
    ) -> Agent {
        let now = self.clock.wall();
        Agent {
            id,
            workspace: info.workspace,
            parent: None,
            vendor,
            account: Some(info.account),
            model: info.model.clone(),
            permission: Some(AgentPermission {
                requested: info.mode,
                effective: None,
            }),
            session: None,
            vendor_session_id: vendor_session,
            control: Control::Held,
            pid: None,
            link: AgentLink::Unknown,
            title: info.title.clone(),
            activity,
            context: None,
            started_at: now,
            last_activity_at: now,
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
        }
    }

    /// Adds a warning from MaxHQ to the agent's timeline.
    fn notice(&mut self, agent: AgentId, message: &str) {
        let now = self.clock.wall();
        let item = TimelineItem {
            id: ItemId(format!("maxhq-notice-{}", Uuid::now_v7())),
            agent,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Completed,
            started_at: Some(now),
            completed_at: Some(now),
            body: ItemBody::Notice {
                level: NoticeLevel::Warning,
                message: message.to_owned(),
            },
        };
        self.timeline_events(vec![(agent, TimelineEvent::Upsert { item })]);
    }

    /// The permission mode new agents get on this device.
    fn default_mode(&self) -> PermissionMode {
        self.device_settings().default_permission
    }

    /// The effort level an agent of `vendor` under `account` starts at:
    /// `asked`, else this device's default for the vendor, brought down to
    /// a level its model takes, else the model's own default. `maxd` names
    /// a level whenever it knows the model's, so the level the apps show is
    /// the one the CLI runs at. Only Claude Code and Codex take one.
    fn start_effort(
        &self,
        vendor: Vendor,
        account: AccountId,
        model: Option<&str>,
        asked: Option<String>,
    ) -> Option<String> {
        if !vendors::takes_effort(vendor) {
            return None;
        }
        let wanted = asked.or_else(|| {
            self.device_settings()
                .default_effort(vendor)
                .map(str::to_owned)
        });
        match self.model_choice(account, model) {
            Some(choice) if !choice.efforts.is_empty() => wanted
                .and_then(|level| choice.fit_effort(&level))
                .or(choice.default_effort),
            _ => wanted,
        }
    }

    /// The level a Claude Code or Codex model under `account` runs at when
    /// told none.
    fn model_effort(
        &self,
        vendor: Vendor,
        account: AccountId,
        model: Option<&str>,
    ) -> Option<String> {
        if !vendors::takes_effort(vendor) {
            return None;
        }
        self.model_choice(account, model)?.default_effort
    }

    /// The model `model` names among the ones `account` runs, else the one
    /// it runs when told none, as the apps pick it.
    fn model_choice(&self, account: AccountId, model: Option<&str>) -> Option<ModelChoice> {
        let Some(Entity::Account(account)) = self.hub.get(&EntityKey::Account(account)) else {
            return None;
        };
        let named = model.and_then(|model| account.models.iter().position(|one| one.id == model));
        let at = named.or_else(|| account.models.iter().position(|one| one.default))?;
        account.models.into_iter().nth(at)
    }

    /// Moves staged attachments into an agent's folder. One a message moved
    /// there already, such as a queued message's taken back and sent again,
    /// stays where it is.
    fn take_attachments(
        &mut self,
        ids: &[AttachmentId],
        into: &std::path::Path,
    ) -> Result<Vec<Staged>, String> {
        let found = self.find_attachments(ids, Some(into))?;
        self.move_attachments(&found, into)
    }

    /// Looks staged attachments up, and with `sent_to` set, the ones a
    /// message already moved into that agent's folder. No other agent's
    /// folder is looked in.
    fn find_attachments(
        &self,
        ids: &[AttachmentId],
        sent_to: Option<&std::path::Path>,
    ) -> Result<Vec<AttachmentRef>, String> {
        ids.iter()
            .map(|id| {
                self.staged
                    .get(id)
                    .cloned()
                    .or_else(|| self.attachments.find(*id))
                    .or_else(|| sent_to.and_then(|into| self.attachments.sent(*id, into)))
                    .ok_or_else(|| attach::GONE.to_owned())
            })
            .collect()
    }

    /// Moves found attachments into an agent's folder. Each one moves out
    /// of staging once, so only that agent's messages can name it again.
    fn move_attachments(
        &mut self,
        found: &[AttachmentRef],
        into: &std::path::Path,
    ) -> Result<Vec<Staged>, String> {
        found
            .iter()
            .map(|attachment| {
                let moved = self
                    .attachments
                    .take(attachment, into)
                    .map_err(|err| format!("couldn't move {}: {err}", attachment.name))?;
                self.staged.remove(&attachment.id);
                Ok(moved)
            })
            .collect()
    }

    /// Records an answer so a second one is refused, even after a restart.
    fn record_answer(&mut self, event: AttentionId, key: IdempotencyKey, by: ClientId) {
        let row = AnswerRow {
            event,
            key,
            at: self.clock.wall(),
            by,
        };
        if let Err(err) = self.db.put_answer(&row) {
            warn!(%err, %event, "couldn't store the answer");
        }
        self.answers.insert(event, row);
    }

    /// Closes an attention event if it's open.
    fn close_attention(&mut self, event: AttentionId) {
        if self.attention.remove(&event).is_some() {
            self.hub.apply_held([Change::Remove {
                key: EntityKey::Attention(event),
            }]);
        }
    }

    /// When attention answers from long ago stop counting.
    fn answers_since(&self) -> Timestamp {
        const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;
        Timestamp(self.clock.wall().0.saturating_sub(WEEK_MS))
    }
}

/// Whether an item is a message queued for its agent and not yet sent.
fn is_queued(item: &TimelineItem) -> bool {
    item.status == ItemStatus::Pending
        && matches!(
            item.body,
            ItemBody::UserMessage {
                delivery: Delivery::Queued,
                ..
            }
        )
}

/// A refusal with this reason.
fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

/// Posts [`Input::Drained`] once a client that fell behind has read half
/// its line. Nothing is posted if the client goes first.
fn wake_when_read(conn: u64, viewer: &Viewer, tx: &mpsc::UnboundedSender<Input>) {
    let line = viewer.tx.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        // The permits go back as soon as they're had. Only the manager
        // sends on the line, so the room stays.
        if line.reserve_many(CLIENT_BACKLOG / 2).await.is_ok() {
            let _ = tx.send(Input::Drained { conn });
        }
    });
}
