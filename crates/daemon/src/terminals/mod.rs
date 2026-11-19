// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The terminals this device holds for its agents: a shell in an agent's
//! terminal drawer, or one command you ran with `!`. Each runs in a holder
//! of its own, `maxd hold --pty`, which outlives `maxd` (see
//! `maxhq_hold::Terminals`).
//!
//! [`Terminals`] publishes each one as a `Terminal` entity and keeps it
//! current from a quiet link to its holder: whether it runs, its title and
//! folder, whether it waits for a password and its size. It's busy while a
//! program other than its shell holds the foreground and, for a drawer
//! shell that marks its prompts and commands, as zsh, bash and fish do
//! while the device's shell integration is on, from a command's start to
//! its end. `maxd` keeps where each command's output starts and ends, and
//! whether its holder lets through only the shell's own marks. It sends
//! each connection that attached the output from where that connection
//! asked, and passes on what it types. What anyone types goes to the
//! holder and nowhere else: never a log, a store or an agent.
//!
//! An agent's session is one shell its `!` commands share, so `cd` and
//! `export` carry from one to the next. Each command it runs is a block
//! whose output `maxd` cuts from the shell's marks (see `session`).
//!
//! The session manager opens terminals, since it knows where an agent runs
//! and keeps the `!` runs' items in its timeline. It hears back through
//! [`RunNote`]s: a terminal opened or didn't, a run ended or moved to the
//! drawer, a session took a command or a block ended. When it has kept a
//! run's end, it lets go with [`Host::release`], and the holder goes.

pub mod access;
mod session;

pub use session::{BlockChange, SessionAsk, SessionRequest, SessionResult};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use maxhq_hold::{
    Commands, Mark, Reached, TermControl, TermEvent, TermExit, TermHello, TermLaunch, TermLink,
};
use maxhq_protocol::frame::{self, MAX_TERMINAL_CHUNK};
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, TerminalId};
use maxhq_protocol::message::{ActionOutcome, ErrorCode, ServerMessage};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use maxhq_protocol::terminal::{Terminal, TerminalKind, TerminalStatus};
use maxhq_protocol::time::Timestamp;
use maxhq_wire::{Queue, Sender};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tracing::{debug, warn};

use crate::hub::Hub;
use access::{Act, Caller, Opener};

/// What a request for a terminal this device doesn't hold reads.
pub const NO_SUCH_TERMINAL: &str = "No terminal here has that ID.";

/// What closing a terminal that still runs something reads, without
/// `force`. The app asks whether to go ahead.
pub const STILL_RUNNING: &str = "Still running. Close anyway?";

/// What a request reads when the terminal's holder can't be reached.
pub const UNREACHABLE: &str = "Couldn't reach the terminal. Try again in a moment.";

/// How long a terminal that's closed, or whose agent ended, has after its
/// hangup before it's killed.
const HANGUP_GRACE: Duration = Duration::from_secs(2);

/// How long a request waits at most for the terminals a previous `maxd`
/// left to come back.
const RESTORE_WAIT: Duration = Duration::from_secs(5);

/// Why a request that isn't an action, such as an attach or a keystroke,
/// didn't go through. The connection gets it as an error.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No terminal here has that ID, or its output is gone.
    Unknown(String),
    /// The connection may not do that.
    NotAllowed(String),
    /// The terminal's holder couldn't be reached.
    Unreachable(String),
}

impl Refusal {
    fn unknown() -> Self {
        Self::Unknown(NO_SUCH_TERMINAL.to_owned())
    }

    fn unreachable() -> Self {
        Self::Unreachable(UNREACHABLE.to_owned())
    }

    /// The error's code and message.
    pub fn into_error(self) -> (ErrorCode, String) {
        match self {
            Self::Unknown(reason) => (ErrorCode::BadMessage, reason),
            Self::NotAllowed(reason) => (ErrorCode::NotAllowed, reason),
            Self::Unreachable(reason) => (ErrorCode::Unreachable, reason),
        }
    }
}

/// What the session manager asks of the terminals.
pub trait Host: Send + Sync {
    /// Opens a terminal. The outcome comes back as [`RunNote::Opened`].
    fn open(&self, request: OpenRequest);
    /// Says the manager has kept a run's end, so the terminal may go,
    /// unless it moved to the drawer, where it stays until you close it.
    fn release(&self, terminal: TerminalId);
    /// Ends every terminal of an agent that stopped or was deleted.
    fn end_agent(&self, agent: AgentId);
    /// Does what you asked of an agent's session. The answer comes back
    /// as [`RunNote::Session`], and a fresh shell's as [`RunNote::Opened`]
    /// after it.
    fn session(&self, request: SessionRequest);
}

/// A terminal to open for an agent.
#[derive(Clone, Debug)]
pub struct OpenRequest {
    /// The request's key, which [`RunNote::Opened`] carries back.
    pub key: IdempotencyKey,
    /// The agent whose folder it starts in.
    pub agent: AgentId,
    /// What it runs.
    pub kind: TerminalKind,
    /// The folder it starts in.
    pub cwd: PathBuf,
    /// Its width in cells.
    pub cols: u16,
    /// Its height in cells.
    pub rows: u16,
    /// Who asked for it.
    pub opener: Opener,
    /// The manager's ID for the connection that asked, whose size it has
    /// until someone types.
    pub conn: u64,
}

/// What the terminals tell the session manager, in the order it happened.
#[derive(Debug)]
pub enum RunNote {
    /// A terminal opened, or why it didn't.
    Opened {
        /// The request's key.
        key: IdempotencyKey,
        /// The agent it's for.
        agent: AgentId,
        /// The terminal as published, or why it didn't open.
        result: Result<Terminal, String>,
    },
    /// A `!` run ended. The manager keeps its end, then calls
    /// [`Host::release`].
    Ended {
        /// Its agent.
        agent: AgentId,
        /// The terminal.
        terminal: TerminalId,
        /// How it ended, or none when its holder went without saying.
        exit: Option<TermExit>,
    },
    /// A `!` run moved to the drawer.
    Moved {
        /// Its agent.
        agent: AgentId,
        /// The terminal.
        terminal: TerminalId,
    },
    /// What came of a request to an agent's session.
    Session {
        /// The request's key.
        key: IdempotencyKey,
        /// The agent whose session it is.
        agent: AgentId,
        /// What came of it.
        result: SessionResult,
    },
    /// A command a session runs started or ended.
    Block {
        /// The agent whose session it is.
        agent: AgentId,
        /// The session's terminal.
        terminal: TerminalId,
        /// The block's ID in the session.
        block: u32,
        /// What happened.
        change: BlockChange,
    },
}

/// How the terminals and the session manager reach each other.
pub struct Wiring {
    /// What the manager calls.
    pub host: Arc<dyn Host>,
    /// What the terminals tell the manager.
    pub notes: mpsc::UnboundedReceiver<RunNote>,
    /// The terminals themselves, for the connections. Session tests that
    /// fake the host have none.
    pub terminals: Option<Terminals>,
}

/// What `maxd` keeps in a terminal's folder, so the next `maxd` can
/// publish it again.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Note {
    agent: AgentId,
    kind: TerminalKind,
    /// The folder it started in, as the device writes it.
    cwd: String,
    started_at: Timestamp,
    opener: Opener,
    #[serde(default)]
    moved: bool,
    /// What you renamed it to.
    #[serde(default)]
    name: Option<String>,
    /// A session's blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<session::SessionNote>,
}

/// A terminal this `maxd` holds.
struct Open {
    entity: Terminal,
    note: Note,
    /// The quiet link's control, while it's up.
    control: Option<TermControl>,
    /// The title the shell set last.
    shell_title: Option<String>,
    /// The connection whose size applies: the last one to type, or the
    /// opener's until someone does.
    sizer: Option<u64>,
    /// The size each connection last asked for.
    sizes: HashMap<u64, (u16, u16)>,
    /// The output pump to each connection that attached.
    pumps: HashMap<u64, AbortHandle>,
    /// Whether you closed it, or its agent ended, so it goes once it ends.
    closing: bool,
    /// The commands its shell marked, as the quiet link heard them.
    commands: Commands,
    /// Whether the holder last said a program other than the shell holds
    /// the foreground, or a `!` run hasn't ended.
    foreground: bool,
    /// For a session, whether what you typed may have left something at
    /// its prompt.
    keys: session::Typed,
    /// For a session, the block that runs, and where to say you sent a
    /// password while it waited for one.
    block: Option<(u32, mpsc::UnboundedSender<()>)>,
}

impl Open {
    fn new(note: Note, hello: Option<&TermHello>, device: DeviceId, terminal: TerminalId) -> Self {
        let mut open = Self {
            entity: Terminal {
                id: terminal,
                agent: note.agent,
                device,
                kind: note.kind.clone(),
                moved: note.moved,
                title: String::new(),
                cwd: note.cwd.clone(),
                status: TerminalStatus::Running,
                busy: false,
                marks: false,
                password_prompt: false,
                started_at: note.started_at,
                cols: 80,
                rows: 24,
            },
            note,
            control: None,
            shell_title: None,
            sizer: None,
            sizes: HashMap::new(),
            pumps: HashMap::new(),
            closing: false,
            commands: Commands::default(),
            foreground: false,
            keys: session::Typed::default(),
            block: None,
        };
        if let Some(hello) = hello {
            open.hello(hello);
        }
        open.retitle();
        open
    }

    /// Takes in what a holder said as a link came up. The link sends
    /// every mark the holder keeps again.
    fn hello(&mut self, hello: &TermHello) {
        self.commands = Commands::default();
        self.entity.password_prompt = hello.password;
        self.entity.cols = hello.cols;
        self.entity.rows = hello.rows;
        self.entity.marks = hello.marks;
        self.foreground = hello.busy;
        self.rebusy();
        self.shell_title = hello.title.clone();
        if let Some(cwd) = &hello.cwd {
            self.entity.cwd = shown(Path::new(cwd));
        }
        self.retitle();
    }

    /// Works out whether it's busy: a command its shell marked hasn't
    /// ended, or the foreground says so. Either alone misses some: a
    /// subshell, or a shell that took this one's place with exec, marks
    /// nothing, and a builtin like `read` holds no foreground. Returns
    /// whether it changed.
    fn rebusy(&mut self) -> bool {
        let busy = self.running() && (self.foreground || self.commands.running());
        let changed = self.entity.busy != busy;
        self.entity.busy = busy;
        changed
    }

    /// The name you gave it, or the title its shell set, or what it runs.
    fn retitle(&mut self) {
        self.entity.title = (self.note.name.clone())
            .or_else(|| {
                self.shell_title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
            })
            .unwrap_or_else(|| match &self.note.kind {
                TerminalKind::Shell | TerminalKind::Session => {
                    maxhq_hold::login_shell().file_name().map_or_else(
                        || "shell".to_owned(),
                        |name| name.to_string_lossy().into_owned(),
                    )
                }
                TerminalKind::Run { command, .. } => command.clone(),
            });
    }

    fn running(&self) -> bool {
        self.entity.status == TerminalStatus::Running
    }

    fn check(&self, caller: Caller, act: Act) -> Result<(), String> {
        access::check(caller, act, &self.entity.kind, self.note.opener).map_err(str::to_owned)
    }

    /// Sets the size, when `conn` owns it.
    fn resize(&mut self, conn: u64, cols: u16, rows: u16) -> bool {
        self.sizes.insert(conn, (cols, rows));
        if self.sizer.is_some_and(|sizer| sizer != conn) {
            return false;
        }
        self.sizer = Some(conn);
        self.apply_size(cols, rows)
    }

    /// Notes that `conn` typed, so the size it asked for applies. Returns
    /// whether the size changed.
    fn typed(&mut self, conn: u64) -> bool {
        if self.sizer == Some(conn) {
            return false;
        }
        self.sizer = Some(conn);
        match self.sizes.get(&conn) {
            Some(&(cols, rows)) => self.apply_size(cols, rows),
            None => false,
        }
    }

    fn apply_size(&mut self, cols: u16, rows: u16) -> bool {
        if (self.entity.cols, self.entity.rows) == (cols, rows) {
            return false;
        }
        if let Some(control) = &self.control {
            control.resize(cols, rows);
        }
        self.entity.cols = cols;
        self.entity.rows = rows;
        true
    }
}

/// A folder as the device writes it, with `~` for the home folder.
fn shown(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .filter(|home| home.is_absolute())
        .map(|home| path.strip_prefix(home))
    {
        Some(Ok(rest)) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(Ok(rest)) => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// The terminals this device holds. Clones share them.
#[derive(Clone)]
pub struct Terminals {
    inner: Arc<Inner>,
}

struct Inner {
    holds: maxhq_hold::Terminals,
    hub: Hub,
    device: DeviceId,
    open: Mutex<HashMap<TerminalId, Open>>,
    notes: mpsc::UnboundedSender<RunNote>,
    /// False while the terminals a previous `maxd` left come back.
    restored: watch::Sender<bool>,
    /// One for each agent, so its session takes one request at a time.
    locks: Mutex<HashMap<AgentId, Arc<tokio::sync::Mutex<()>>>>,
    /// What tests change in each launch, such as the shell.
    #[cfg(test)]
    hook: Mutex<Option<LaunchHook>>,
}

/// Changes a launch, for tests.
#[cfg(test)]
pub(crate) type LaunchHook = Arc<dyn Fn(&mut TermLaunch) + Send + Sync>;

impl Terminals {
    /// Terminals in `holds`'s folders, published on `hub`. Call
    /// [`Terminals::restoring`] before the server takes connections, and
    /// run what it returns once the session manager runs.
    pub fn wire(holds: maxhq_hold::Terminals, hub: Hub) -> Wiring {
        let (notes, notes_rx) = mpsc::unbounded_channel();
        let terminals = Self {
            inner: Arc::new(Inner {
                holds,
                device: hub.device().id,
                hub,
                open: Mutex::new(HashMap::new()),
                notes,
                restored: watch::Sender::new(true),
                locks: Mutex::new(HashMap::new()),
                #[cfg(test)]
                hook: Mutex::new(None),
            }),
        };
        Wiring {
            host: Arc::new(terminals.clone()),
            notes: notes_rx,
            terminals: Some(terminals),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<TerminalId, Open>> {
        self.inner
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn note(&self, note: RunNote) {
        let _ = self.inner.notes.send(note);
    }

    fn publish(&self, open: &Open) {
        let value = Entity::Terminal(open.entity.clone());
        self.inner.hub.apply([Change::Upsert { value }]);
    }

    fn save(&self, terminal: TerminalId, note: &Note) {
        if let Err(err) = self.inner.holds.save_note(terminal, note) {
            warn!(%err, %terminal, "couldn't keep what maxd knows about a terminal");
        }
    }

    /// What publishes again the terminals a previous `maxd` left. From
    /// this call until it ends, [`Terminals::restored`] waits, so a client
    /// that reconnects as soon as the socket opens still finds the
    /// terminals it had.
    pub fn restoring(&self) -> impl Future<Output = ()> + Send + 'static {
        self.inner.restored.send_replace(false);
        let terminals = self.clone();
        async move {
            terminals.restore().await;
            terminals.inner.restored.send_replace(true);
        }
    }

    /// Waits until the terminals a previous `maxd` left are back, or
    /// [`RESTORE_WAIT`] at most.
    pub async fn restored(&self) {
        let mut restored = self.inner.restored.subscribe();
        let _ = tokio::time::timeout(RESTORE_WAIT, restored.wait_for(|done| *done)).await;
    }

    /// Publishes again the terminals a previous `maxd` left running, and
    /// tells the manager about the runs that ended while none ran.
    async fn restore(&self) {
        if let Err(err) = self.inner.holds.sweep() {
            warn!(%err, "couldn't clear what a previous maxd left writing shell scripts");
        }
        let terminals = match self.inner.holds.list() {
            Ok(terminals) => terminals,
            Err(err) => {
                warn!(%err, "couldn't list the terminals a previous maxd left");
                return;
            }
        };
        for terminal in terminals {
            let note = match self.inner.holds.load_note::<Note>(terminal) {
                Ok(note) => note,
                Err(err) => {
                    warn!(%err, %terminal, "couldn't read what maxd knew about a terminal");
                    continue;
                }
            };
            let reached = match self.inner.holds.watch(terminal).await {
                Ok(reached) => reached,
                Err(err) => {
                    warn!(%err, %terminal, "couldn't reach a terminal's holder");
                    continue;
                }
            };
            match (note, reached) {
                (Some(note), Reached::Live(link)) => {
                    let block = (note.session.as_ref()).and_then(|session| session.current.clone());
                    let (entity, events) = self.hold(terminal, note, link, None);
                    tokio::spawn(self.clone().follow(terminal, events));
                    // A command a session ran while no `maxd` did.
                    if let Some(block) = block {
                        self.resume_block(terminal, &entity, block);
                    }
                }
                (Some(note), Reached::Ended(exit)) => {
                    let open = Open::new(note, None, self.inner.device, terminal);
                    self.lock().insert(terminal, open);
                    self.ended(terminal, exit);
                }
                // A `maxd` that stopped between starting a holder and
                // noting what it's for: nobody can reach it, so it ends.
                (None, Reached::Live(link)) => {
                    warn!(%terminal, "ended a terminal no agent owns");
                    tokio::spawn(end_stray(link));
                }
                (None, Reached::Ended(_)) => {
                    let _ = self.inner.holds.remove(terminal);
                }
                (_, Reached::Missing) => {}
            }
        }
    }

    /// Keeps a terminal whose holder `link` reached and publishes it.
    /// Returns it as published, and the link's events for
    /// [`Terminals::follow`].
    fn hold(
        &self,
        terminal: TerminalId,
        note: Note,
        link: TermLink,
        sizer: Option<u64>,
    ) -> (Terminal, mpsc::Receiver<TermEvent>) {
        let TermLink {
            hello,
            events,
            control,
        } = link;
        let mut open = Open::new(note, Some(&hello), self.inner.device, terminal);
        open.control = Some(control);
        open.sizer = sizer;
        if let Some(conn) = sizer {
            open.sizes.insert(conn, (hello.cols, hello.rows));
        }
        let entity = open.entity.clone();
        self.publish(&open);
        self.lock().insert(terminal, open);
        (entity, events)
    }

    /// Opens a terminal, as the manager asked. The outcome goes to the
    /// manager as [`RunNote::Opened`], before anything else about it.
    fn start(&self, request: OpenRequest) {
        let this = self.clone();
        tokio::spawn(async move {
            let (key, agent) = (request.key, request.agent);
            match this.spawn(request).await {
                Ok((terminal, events)) => {
                    let id = terminal.id;
                    this.note(RunNote::Opened {
                        key,
                        agent,
                        result: Ok(terminal),
                    });
                    this.follow(id, events).await;
                }
                Err(reason) => this.note(RunNote::Opened {
                    key,
                    agent,
                    result: Err(reason),
                }),
            }
        });
    }

    async fn spawn(
        &self,
        request: OpenRequest,
    ) -> Result<(Terminal, mpsc::Receiver<TermEvent>), String> {
        let terminal = TerminalId::generate();
        let mut launch = self.launch(
            terminal,
            &request.kind,
            request.cwd.clone(),
            request.cols,
            request.rows,
        );
        self.integrate(&mut launch);
        self.spawn_launch(
            &launch,
            request.agent,
            request.kind,
            request.opener,
            request.conn,
            None,
        )
        .await
    }

    /// What starts a terminal of `kind` in `cwd`.
    fn launch(
        &self,
        terminal: TerminalId,
        kind: &TerminalKind,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
    ) -> TermLaunch {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut launch = match kind {
            TerminalKind::Shell | TerminalKind::Session => {
                TermLaunch::shell(terminal, cwd, cols, rows)
            }
            TerminalKind::Run { command, .. } => {
                TermLaunch::run(terminal, cwd, command, cols, rows)
            }
        };
        self.tweak(&mut launch);
        launch
    }

    #[cfg(test)]
    fn tweak(&self, launch: &mut TermLaunch) {
        let hook = (self.inner.hook.lock())
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook(launch);
        }
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    fn tweak(&self, _launch: &mut TermLaunch) {}

    /// Sets what tests change in each launch.
    #[cfg(test)]
    pub(crate) fn set_launch_hook(&self, hook: LaunchHook) {
        *(self.inner.hook.lock()).unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
    }

    /// Starts `launch` for an agent, and keeps and publishes the terminal.
    async fn spawn_launch(
        &self,
        launch: &TermLaunch,
        agent: AgentId,
        kind: TerminalKind,
        opener: Opener,
        conn: u64,
        session: Option<session::SessionNote>,
    ) -> Result<(Terminal, mpsc::Receiver<TermEvent>), String> {
        let terminal = launch.terminal;
        if let Err(err) = self.inner.holds.spawn(launch).await {
            warn!(%err, %agent, "couldn't start a terminal");
            return Err(format!("Couldn't start the terminal: {err}"));
        }
        let link = match self.inner.holds.watch(terminal).await {
            Ok(Reached::Live(link)) => link,
            Ok(Reached::Ended(_) | Reached::Missing) => {
                let _ = self.inner.holds.remove(terminal);
                return Err("The terminal ended as it started.".to_owned());
            }
            Err(err) => {
                warn!(%err, %terminal, "couldn't reach a terminal that started");
                return Err(format!("Couldn't reach the terminal: {err}"));
            }
        };
        let note = Note {
            agent,
            kind,
            cwd: shown(&launch.cwd),
            started_at: link.hello.started_at,
            opener,
            moved: false,
            name: None,
            session,
        };
        self.save(terminal, &note);
        Ok(self.hold(terminal, note, link, Some(conn)))
    }

    /// Whether the device's shell integration is on.
    fn integration_on(&self) -> bool {
        let key = EntityKey::DeviceSettings(self.inner.device);
        !matches!(
            self.inner.hub.get(&key),
            Some(Entity::DeviceSettings(settings)) if !settings.shell_integration
        )
    }

    /// Gives a drawer shell hooks that mark its prompts and commands,
    /// unless the device's shell integration is off. A shell it can't give
    /// them starts as it would without. Returns whether it has them.
    fn integrate(&self, launch: &mut TermLaunch) -> bool {
        if !self.integration_on() {
            return false;
        }
        match self.inner.holds.integrate(launch) {
            Ok(integrated) => integrated,
            Err(err) => {
                warn!(%err, "couldn't write the shell integration scripts");
                false
            }
        }
    }

    /// Follows a terminal's quiet link: its password prompt, whether it's
    /// busy, its title, folder, marks and exit. When the link drops before
    /// the exit, it reaches the holder again.
    async fn follow(self, terminal: TerminalId, mut events: mpsc::Receiver<TermEvent>) {
        let mut exited = false;
        loop {
            while let Some(event) = events.recv().await {
                match event {
                    TermEvent::Output { .. } => {}
                    TermEvent::Password(on) => self.update(terminal, |open| {
                        let changed = open.entity.password_prompt != on;
                        open.entity.password_prompt = on;
                        changed
                    }),
                    TermEvent::Busy(on) => self.update(terminal, |open| {
                        open.foreground = on;
                        open.rebusy()
                    }),
                    TermEvent::Mark { offset, at, mark } => self.update(terminal, |open| {
                        if matches!(mark, Mark::Prompt) {
                            open.keys.prompt();
                        }
                        open.commands.mark(offset, at, &mark);
                        open.rebusy()
                    }),
                    TermEvent::Title(title) => self.update(terminal, |open| {
                        open.shell_title = Some(title);
                        let was = open.entity.title.clone();
                        open.retitle();
                        open.entity.title != was
                    }),
                    TermEvent::Cwd(cwd) => self.update(terminal, |open| {
                        let cwd = shown(Path::new(&cwd));
                        let changed = open.entity.cwd != cwd;
                        open.entity.cwd = cwd;
                        changed
                    }),
                    TermEvent::Exit(exit) => {
                        exited = true;
                        self.ended(terminal, Some(exit));
                    }
                }
            }
            // The holder went: you said goodbye, it lingered long enough
            // after the exit, or it crashed.
            let Some(control) = self
                .lock()
                .get_mut(&terminal)
                .and_then(|open| open.control.take())
            else {
                return;
            };
            drop(control);
            if exited {
                return;
            }
            match self.inner.holds.watch(terminal).await {
                Ok(Reached::Live(link)) => {
                    let TermLink {
                        hello,
                        events: more,
                        control,
                    } = link;
                    events = more;
                    let mut opens = self.lock();
                    let Some(open) = opens.get_mut(&terminal) else {
                        return;
                    };
                    open.control = Some(control);
                    open.hello(&hello);
                    self.publish(open);
                }
                Ok(Reached::Ended(exit)) => return self.ended(terminal, exit),
                Ok(Reached::Missing) => return self.finish(terminal),
                Err(err) => {
                    warn!(%err, %terminal, "lost a terminal's holder");
                    return self.ended(terminal, None);
                }
            }
        }
    }

    /// Changes a terminal with `change`, and publishes it when that says
    /// it changed what clients see.
    fn update(&self, terminal: TerminalId, change: impl FnOnce(&mut Open) -> bool) {
        let mut opens = self.lock();
        if let Some(open) = opens.get_mut(&terminal)
            && change(open)
        {
            self.publish(open);
        }
    }

    /// Marks a terminal ended. A run's end goes to the manager, which
    /// releases it once kept. A shell stays as a tab that says it ended
    /// until you close it. A session's block that nothing follows ends
    /// with it.
    fn ended(&self, terminal: TerminalId, exit: Option<TermExit>) {
        let mut opens = self.lock();
        let Some(open) = opens.get_mut(&terminal) else {
            return;
        };
        if !open.running() {
            return;
        }
        open.entity.status = TerminalStatus::Exited {
            code: exit.as_ref().and_then(|exit| exit.code),
            signal: exit.as_ref().and_then(|exit| exit.signal),
        };
        open.entity.password_prompt = false;
        open.entity.busy = false;
        self.publish(open);
        let (agent, run, closing) = (
            open.note.agent,
            matches!(open.note.kind, TerminalKind::Run { .. }),
            open.closing,
        );
        // A block whose follower went with a `maxd` that stopped, and
        // whose shell ended before this one followed it again.
        let orphan = if open.block.is_none() {
            let current = (open.note.session.as_mut()).and_then(|session| session.current.take());
            if current.is_some() {
                self.save(terminal, &open.note);
            }
            current
        } else {
            None
        };
        drop(opens);
        if let Some(block) = orphan {
            self.note(RunNote::Block {
                agent,
                terminal,
                block: block.id,
                change: BlockChange::Ended {
                    exit: if block.done { block.exit } else { None },
                    at: Timestamp::now(),
                    cwd_after: None,
                    output: maxhq_term::Transcript {
                        text: String::new(),
                        lines: 0,
                        truncated: true,
                    },
                    secret_prompts: 0,
                },
            });
        }
        if run {
            self.note(RunNote::Ended {
                agent,
                terminal,
                exit,
            });
        } else if closing {
            self.finish(terminal);
        }
    }

    /// Forgets a terminal: tells its holder it may go, which deletes its
    /// folder, and takes it out of the state. Pumps run until the holder
    /// has gone, so they send the last of its output.
    fn finish(&self, terminal: TerminalId) {
        let Some(open) = self.lock().remove(&terminal) else {
            return;
        };
        if let Some(control) = &open.control {
            control.goodbye();
        }
        // A holder that has gone already left its folder behind.
        if let Err(err) = self.inner.holds.remove(terminal) {
            debug!(%err, %terminal, "couldn't remove an ended terminal's folder");
        }
        self.inner.hub.apply([Change::Remove {
            key: EntityKey::Terminal(terminal),
        }]);
    }

    /// Starts sending `wire` a terminal's output after `after`, the offset
    /// of the last byte the connection has. It first sends
    /// `TerminalAttached`, then the output, with no gap. When the
    /// connection falls so far behind that the holder no longer has what
    /// comes next, it sends `TerminalAttached` again with `reset` and goes
    /// on from a whole line.
    pub async fn attach(
        &self,
        conn: u64,
        caller: Caller,
        wire: Sender,
        terminal: TerminalId,
        after: Option<u64>,
    ) -> Result<(), Refusal> {
        {
            let opens = self.lock();
            let open = opens.get(&terminal).ok_or_else(Refusal::unknown)?;
            open.check(caller, Act::Watch)
                .map_err(Refusal::NotAllowed)?;
        }
        // The holder counts from the next byte the connection needs.
        let next = after.map(|last| last.saturating_add(1));
        let link = match self.inner.holds.connect(terminal, next).await {
            Ok(Reached::Live(link)) => link,
            Ok(Reached::Ended(_) | Reached::Missing) => {
                return Err(Refusal::Unknown(
                    "That terminal has ended, and its output is gone.".to_owned(),
                ));
            }
            Err(err) => {
                warn!(%err, %terminal, "couldn't reach a terminal's holder");
                return Err(Refusal::unreachable());
            }
        };
        let TermLink {
            hello,
            events,
            control,
        } = link;
        let pump = tokio::spawn(async move {
            // The link stays up while the pump holds its control.
            let _control = control;
            pump(terminal, hello.from, hello.reset, events, wire).await;
        });
        let old = (self.lock().get_mut(&terminal))
            .and_then(|open| open.pumps.insert(conn, pump.abort_handle()));
        if let Some(old) = old {
            old.abort();
        }
        Ok(())
    }

    /// Stops sending a connection a terminal's output.
    pub fn detach(&self, conn: u64, terminal: TerminalId) {
        if let Some(pump) =
            (self.lock().get_mut(&terminal)).and_then(|open| open.pumps.remove(&conn))
        {
            pump.abort();
        }
    }

    /// Types `data` into a terminal. The connection then owns its size.
    /// The bytes go to the holder and nowhere else. In a session, `maxd`
    /// notes only whether they may have left something at its prompt, and
    /// that a password went when one waited.
    pub fn input(
        &self,
        conn: u64,
        caller: Caller,
        terminal: TerminalId,
        data: Vec<u8>,
    ) -> Result<(), Refusal> {
        let mut opens = self.lock();
        let open = opens.get_mut(&terminal).ok_or_else(Refusal::unknown)?;
        open.check(caller, Act::Type).map_err(Refusal::NotAllowed)?;
        let Some(control) = open.control.clone() else {
            return Err(Refusal::unreachable());
        };
        if matches!(open.note.kind, TerminalKind::Session) {
            match &open.block {
                Some((_, secrets)) if open.entity.password_prompt => {
                    let _ = secrets.send(());
                }
                _ if !open.entity.busy => open.keys.keys(&data),
                _ => {}
            }
        }
        control.input(data);
        if open.typed(conn) {
            self.publish(open);
        }
        Ok(())
    }

    /// Sets a terminal's size for a connection. It applies when that
    /// connection typed last, or when nobody has typed yet.
    pub fn resize(
        &self,
        conn: u64,
        caller: Caller,
        terminal: TerminalId,
        cols: u16,
        rows: u16,
    ) -> Result<(), Refusal> {
        let mut opens = self.lock();
        let open = opens.get_mut(&terminal).ok_or_else(Refusal::unknown)?;
        open.check(caller, Act::Type).map_err(Refusal::NotAllowed)?;
        if open.resize(conn, cols.max(1), rows.max(1)) {
            self.publish(open);
        }
        Ok(())
    }

    /// Types Ctrl-C into a terminal.
    pub fn interrupt(&self, caller: Caller, terminal: TerminalId) -> ActionOutcome {
        outcome(self.with(terminal, |open| {
            open.check(caller, Act::Watch)?;
            match &open.control {
                Some(control) if control.interrupt() => Ok(()),
                _ => Err(UNREACHABLE.to_owned()),
            }
        }))
    }

    /// Closes a terminal: hangs up what runs in it, then kills it after a
    /// moment, or at once with `force`. Without `force`, one that still
    /// runs a command asks first.
    pub fn close(&self, caller: Caller, terminal: TerminalId, force: bool) -> ActionOutcome {
        let mut opens = self.lock();
        // A resend after the terminal went finds it gone.
        let Some(open) = opens.get_mut(&terminal) else {
            return ActionOutcome::Done;
        };
        if let Err(reason) = open.check(caller, Act::Manage) {
            return ActionOutcome::Refused { reason };
        }
        if !open.running() {
            drop(opens);
            self.finish(terminal);
            return ActionOutcome::Done;
        }
        let run = matches!(open.entity.kind, TerminalKind::Run { .. });
        if !force && (run || open.entity.busy) {
            return ActionOutcome::Refused {
                reason: STILL_RUNNING.to_owned(),
            };
        }
        let Some(control) = &open.control else {
            return ActionOutcome::Refused {
                reason: UNREACHABLE.to_owned(),
            };
        };
        control.kill(if force { Duration::ZERO } else { HANGUP_GRACE });
        open.closing = true;
        ActionOutcome::Done
    }

    /// Renames a terminal's tab. A blank name goes back to the title its
    /// shell sets.
    pub fn rename(&self, caller: Caller, terminal: TerminalId, title: &str) -> ActionOutcome {
        outcome(self.with(terminal, |open| {
            open.check(caller, Act::Manage)?;
            let title = title.trim();
            open.note.name = (!title.is_empty()).then(|| title.to_owned());
            open.retitle();
            self.save(terminal, &open.note);
            self.publish(open);
            Ok(())
        }))
    }

    /// Moves a `!` run to its agent's drawer, where it goes on as a tab.
    pub fn move_run(&self, caller: Caller, terminal: TerminalId) -> ActionOutcome {
        let moved = self.with(terminal, |open| {
            open.check(caller, Act::Manage)?;
            if !matches!(open.entity.kind, TerminalKind::Run { .. }) {
                return Err("Only a command you ran with ! moves to the drawer.".to_owned());
            }
            if open.note.moved {
                return Ok(None);
            }
            open.note.moved = true;
            open.entity.moved = true;
            self.save(terminal, &open.note);
            self.publish(open);
            Ok(Some(open.note.agent))
        });
        if let Ok(Some(agent)) = moved {
            self.note(RunNote::Moved { agent, terminal });
        }
        outcome(moved.map(|_| ()))
    }

    /// Forgets a connection that closed: stops its pumps, and its size no
    /// longer applies.
    pub fn gone(&self, conn: u64) {
        for open in self.lock().values_mut() {
            if let Some(pump) = open.pumps.remove(&conn) {
                pump.abort();
            }
            open.sizes.remove(&conn);
            if open.sizer == Some(conn) {
                open.sizer = None;
            }
        }
    }

    /// Keeps a terminal with no holder behind it, as one whose holder
    /// can't be reached, for tests of who may do what.
    #[cfg(test)]
    pub(crate) fn plant(&self, agent: AgentId, kind: TerminalKind, opener: Opener) -> TerminalId {
        let terminal = TerminalId::generate();
        let note = Note {
            agent,
            kind,
            cwd: "~".to_owned(),
            started_at: Timestamp(1),
            opener,
            moved: false,
            name: None,
            session: None,
        };
        let open = Open::new(note, None, self.inner.device, terminal);
        self.publish(&open);
        self.lock().insert(terminal, open);
        terminal
    }

    /// Runs `act` on a terminal this device holds.
    fn with<T>(
        &self,
        terminal: TerminalId,
        act: impl FnOnce(&mut Open) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut opens = self.lock();
        let open = opens.get_mut(&terminal).ok_or(NO_SUCH_TERMINAL)?;
        act(open)
    }
}

impl Host for Terminals {
    fn open(&self, request: OpenRequest) {
        self.start(request);
    }

    fn release(&self, terminal: TerminalId) {
        let go = (self.lock().get(&terminal)).is_some_and(|open| open.closing || !open.note.moved);
        if go {
            self.finish(terminal);
        }
    }

    fn end_agent(&self, agent: AgentId) {
        (self.inner.locks.lock())
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&agent);
        let mut ended = Vec::new();
        for (terminal, open) in self.lock().iter_mut() {
            if open.note.agent != agent {
                continue;
            }
            open.closing = true;
            match &open.control {
                Some(control) if open.running() => {
                    control.kill(HANGUP_GRACE);
                }
                _ => ended.push(*terminal),
            }
        }
        for terminal in ended {
            self.finish(terminal);
        }
    }

    fn session(&self, request: SessionRequest) {
        self.serve_session(request);
    }
}

/// The answer to a request that either worked or says why not.
fn outcome(result: Result<(), String>) -> ActionOutcome {
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(reason) => ActionOutcome::Refused { reason },
    }
}

/// Ends a terminal nobody owns, then lets its holder go.
async fn end_stray(mut link: TermLink) {
    link.control.kill(Duration::ZERO);
    while let Some(event) = link.events.recv().await {
        if matches!(event, TermEvent::Exit(_)) {
            link.control.goodbye();
        }
    }
}

/// Sends `wire` a terminal's output from `events`, which starts at `from`.
/// A jump in the offsets means the connection fell behind what the holder
/// keeps, so it gets `TerminalAttached` with `reset` before the bytes.
async fn pump(
    terminal: TerminalId,
    from: u64,
    reset: bool,
    mut events: mpsc::Receiver<TermEvent>,
    wire: Sender,
) {
    let mut next = from;
    if attached(&wire, terminal, from, reset).await.is_err() {
        return;
    }
    while let Some(event) = events.recv().await {
        let TermEvent::Output { offset, data } = event else {
            continue;
        };
        if offset != next && attached(&wire, terminal, offset, true).await.is_err() {
            return;
        }
        let mut at = offset;
        for chunk in data.chunks(MAX_TERMINAL_CHUNK) {
            let mut out = Vec::with_capacity(chunk.len() + 32);
            frame::encode_terminal_output(&mut out, terminal, at, chunk);
            // A connection that's behind waits here, and the holder keeps
            // the output meanwhile. The terminal itself never waits.
            if wire.send(Queue::Timeline, out).await.is_err() {
                return;
            }
            at += chunk.len() as u64;
        }
        next = at;
    }
}

/// Tells a connection where a terminal's output frames start.
async fn attached(wire: &Sender, terminal: TerminalId, from: u64, reset: bool) -> Result<(), ()> {
    let message = ServerMessage::TerminalAttached {
        terminal,
        from,
        reset,
    };
    let mut out = Vec::new();
    frame::encode_message(&mut out, &message).map_err(|err| {
        warn!(%err, "couldn't encode a terminal's attach");
    })?;
    wire.send(Queue::Timeline, out).await.map_err(|_| ())
}

#[cfg(test)]
mod tests;
