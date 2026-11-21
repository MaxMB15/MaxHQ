// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Fakes for the session manager's tests. [`FakeHolders`] keeps each
//! holder's lines in memory and behaves as the hold crate does. The drivers
//! [`FakeVendors`] builds follow a script: each line a test prints is a
//! [`Say`] that names the effects to report. The rest stand in for the
//! guests, the workspaces, the secret store, file delivery and the Mac's
//! power assertion.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::stream::{BoxStream, StreamExt};
use maxhq_core::excerpt::{CONTEXT, PAGE};
use maxhq_core::tools::{too_big, Call, Page, Read, Send, Target, Tool, MAX_VISUAL_BYTES};
use maxhq_protocol::acp::AcpEnvValue;
use maxhq_protocol::attention::AttentionEvent;
use maxhq_protocol::guest::{FolderScan, Worktree};
use maxhq_protocol::ids::{
    AccountId, AcpAgentId, AgentId, AttachmentId, AttentionId, BootId, DeviceId, IdempotencyKey,
    ProjectId, SessionId, WorkspaceId,
};
use maxhq_protocol::message::{ActionOutcome, AttachmentSource};
use maxhq_protocol::model::{
    Agent, AllowedWorkflowEnv, CommandRule, EditKind, KeepAwake, LoginKind, ModelChoice,
    PermissionMode, Usage, Vendor,
};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{AttachmentKind, AttachmentRef, TimelineEvent, TimelineItem};
use maxhq_protocol::Secret;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, Semaphore};
use uuid::Uuid;

use super::attach::Attachments;
use super::clock::{BoxFuture, Clock};
use super::holders::{
    AgentFiles, AgentSummary, Checkout, ChildState, ConnectError, Connection, Container,
    Credentials, DeliverError, Ended, Event, Exit, Hello, HoldWriter, HolderPlaces, Holders,
    Launch, Line, Looked, PlaceChange, SpawnError, Unsaved, Workspaces, WriteError, WriteOutcome,
};
use super::vendors::{History, Vendors, Wired};
use crate::drive::{
    self, acp, cursor, local, Action, ActionKey, AttentionChange, ChildExit, Driver, Effects,
    Framing, HeldLine, Login, Place, Staged, ToolCall, ToolDone, ToolResult, Write,
};
use crate::power::{should_hold, Power};
use crate::recipe;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a fake Codex logs of the trust a thread loads with.
fn trust_of(place: &Place) -> String {
    format!(
        "trust {:?}",
        place.trust.as_ref().map(|trust| trust.trusted)
    )
}

/// Holders that live in memory.
#[derive(Clone, Default)]
pub struct FakeHolders {
    state: Arc<Mutex<Holds>>,
}

#[derive(Default)]
struct Holds {
    holders: HashMap<SessionId, FakeHold>,
    order: Vec<SessionId>,
    fail_spawns: usize,
    /// What the next failed spawn says, if not the usual.
    spawn_failure: Option<String>,
    /// Each end asked for, with the holder process ID it named.
    ends: Vec<(SessionId, Option<u32>)>,
    /// Spawns of a CLI while an earlier one for the same session still ran.
    overlaps: usize,
    /// Holds back each spawn while set. Closing it lets them through.
    spawn_gate: Option<Arc<Semaphore>>,
    /// While set, a stop only asks, and the CLI ends when the test ends it.
    lingers: bool,
}

/// One fake holder.
struct FakeHold {
    launch: Launch,
    /// The lines still kept.
    lines: Vec<Line>,
    /// The newest seq so far.
    last_seq: u64,
    exit: Option<Exit>,
    /// Prompts the CLI waits on, by request ID.
    pending: Vec<(String, Line)>,
    answered: HashSet<String>,
    writes: Vec<(String, Option<String>)>,
    subscribers: Vec<mpsc::UnboundedSender<Event>>,
    /// Counts dropped connections, so a writer from before one fails.
    generation: u64,
    /// Whether the holder process can be reached.
    alive: bool,
    stops: Vec<Duration>,
    forgotten: bool,
    connects: Vec<Option<u64>>,
    pid: u32,
    /// Set when the holder died before its CLI, which still runs.
    orphan: bool,
    /// Why an end can't confirm the holder, if it can't.
    unconfirmable: Option<String>,
}

impl FakeHold {
    /// Whether the CLI still runs.
    fn running(&self) -> bool {
        self.exit.is_none() || self.orphan
    }

    fn send(&mut self, event: &Event) {
        self.subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }

    fn end(&mut self, code: Option<i32>, signal: Option<i32>) {
        if self.exit.is_some() {
            return;
        }
        let exit = Exit {
            code,
            signal,
            at: Timestamp::now(),
            stderr_tail: String::new(),
        };
        self.exit = Some(exit.clone());
        self.pending.clear();
        self.send(&Event::Exited(exit));
        self.subscribers.clear();
    }

    fn print(&mut self, text: String) -> u64 {
        // The hold crate drops what the CLI prints after its exit.
        if self.exit.is_some() {
            return self.last_seq;
        }
        self.last_seq += 1;
        let line = Line {
            seq: self.last_seq,
            at: Timestamp::now(),
            text,
            replayed: false,
        };
        self.lines.push(line.clone());
        self.send(&Event::Line(line));
        self.last_seq
    }
}

impl FakeHolders {
    /// No holders yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, session: SessionId, f: impl FnOnce(&mut FakeHold) -> T) -> T {
        let mut state = lock(&self.state);
        let hold = state
            .holders
            .get_mut(&session)
            .unwrap_or_else(|| panic!("there's no fake holder {session}"));
        f(hold)
    }

    /// Every holder started, oldest first.
    pub fn sessions(&self) -> Vec<SessionId> {
        lock(&self.state).order.clone()
    }

    /// The newest holder started.
    pub fn last(&self) -> SessionId {
        *self.sessions().last().expect("no holder was started")
    }

    /// What the holder runs.
    pub fn launch(&self, session: SessionId) -> Launch {
        self.with(session, |hold| hold.launch.clone())
    }

    /// The CLI prints a line. Returns its seq.
    pub fn print(&self, session: SessionId, text: impl Into<String>) -> u64 {
        let text = text.into();
        self.with(session, |hold| hold.print(text))
    }

    /// The CLI prints a scripted line.
    pub fn say(&self, session: SessionId, say: &Say) -> u64 {
        self.print(session, say.line())
    }

    /// The CLI prints a scripted line that asks `request` and waits for an
    /// answer.
    pub fn ask(&self, session: SessionId, request: &str, say: &Say) -> u64 {
        let text = say.line();
        self.with(session, |hold| {
            let seq = hold.print(text);
            if let Some(line) = hold.lines.last() {
                hold.pending.push((request.to_owned(), line.clone()));
            }
            seq
        })
    }

    /// Someone answered a prompt in the CLI's own terminal.
    pub fn answer_elsewhere(&self, session: SessionId, request: &str) {
        self.with(session, |hold| {
            hold.pending.retain(|(id, _)| id != request);
            hold.answered.insert(request.to_owned());
        });
    }

    /// The CLI ends.
    pub fn exit(&self, session: SessionId, code: Option<i32>, signal: Option<i32>) {
        self.with(session, |hold| hold.end(code, signal));
    }

    /// The CLI dies of a signal.
    pub fn crash(&self, session: SessionId) {
        self.exit(session, None, Some(9));
    }

    /// Every connection drops, and the CLI prints `missed` before anyone
    /// connects again.
    pub fn drop_connections(&self, session: SessionId, missed: &[&str]) {
        self.with(session, |hold| {
            hold.subscribers.clear();
            hold.generation += 1;
            for text in missed {
                hold.print((*text).to_owned());
            }
        });
    }

    /// The next `count` lines rotate away before any reader gets them.
    pub fn skip(&self, session: SessionId, count: u64) {
        self.with(session, |hold| hold.last_seq += count);
    }

    /// The kept lines up to and including `through` rotate away.
    pub fn trim(&self, session: SessionId, through: u64) {
        self.with(session, |hold| hold.lines.retain(|line| line.seq > through));
    }

    /// The holder dies before its CLI, which keeps running. Its exit has
    /// no code or signal, as the hold crate makes one up.
    pub fn orphan(&self, session: SessionId) {
        self.with(session, |hold| {
            hold.orphan = true;
            hold.end(None, None);
        });
    }

    /// Ends of the holder can't confirm it's the session's own.
    pub fn unconfirmable(&self, session: SessionId) {
        self.with(session, |hold| {
            hold.unconfirmable = Some("process 7 doesn't look like its holder".into());
        });
    }

    /// The holder process ID each end of the session named.
    pub fn ends(&self, session: SessionId) -> Vec<Option<u32>> {
        lock(&self.state)
            .ends
            .iter()
            .filter(|(ended, _)| *ended == session)
            .map(|(_, pid)| *pid)
            .collect()
    }

    /// How many CLIs started while an earlier one for the same session
    /// still ran.
    pub fn overlaps(&self) -> usize {
        lock(&self.state).overlaps
    }

    /// The holder process is gone, along with its record.
    pub fn vanish(&self, session: SessionId) {
        self.with(session, |hold| {
            hold.alive = false;
            hold.subscribers.clear();
        });
    }

    /// The next `count` spawns fail.
    pub fn fail_spawns(&self, count: usize) {
        lock(&self.state).fail_spawns = count;
    }

    /// The next spawn fails, with the holder saying `message`.
    pub fn fail_spawn_with(&self, message: &str) {
        let mut state = lock(&self.state);
        state.fail_spawns = 1;
        state.spawn_failure = Some(message.to_owned());
    }

    /// What was written to the CLI, with the request each line answers.
    pub fn writes(&self, session: SessionId) -> Vec<(String, Option<String>)> {
        self.with(session, |hold| hold.writes.clone())
    }

    /// The grace each stop asked for.
    pub fn stops(&self, session: SessionId) -> Vec<Duration> {
        self.with(session, |hold| hold.stops.clone())
    }

    /// Whether the session's files were removed.
    pub fn forgotten(&self, session: SessionId) -> bool {
        self.with(session, |hold| hold.forgotten)
    }

    /// The cursor each connection asked for.
    pub fn connects(&self, session: SessionId) -> Vec<Option<u64>> {
        self.with(session, |hold| hold.connects.clone())
    }

    /// Whether the CLI has ended.
    pub fn exited(&self, session: SessionId) -> bool {
        self.with(session, |hold| hold.exit.is_some())
    }

    /// Holds back each spawn from now on until [`FakeHolders::release_spawns`].
    /// The holder exists at once, but the spawn doesn't return.
    pub fn hold_spawns(&self) {
        lock(&self.state).spawn_gate = Some(Arc::new(Semaphore::new(0)));
    }

    /// From now on each stop only asks, and the CLI ends when the test
    /// ends it, as one does that takes its time.
    pub fn linger_on_stop(&self) {
        lock(&self.state).lingers = true;
    }

    /// Lets the held spawns return.
    pub fn release_spawns(&self) {
        if let Some(gate) = lock(&self.state).spawn_gate.take() {
            gate.close();
        }
    }
}

impl Holders for FakeHolders {
    fn spawn(&self, launch: Launch) -> BoxFuture<Result<(), SpawnError>> {
        let mut state = lock(&self.state);
        let result = if state.fail_spawns > 0 {
            state.fail_spawns -= 1;
            let message = state.spawn_failure.take();
            Err(SpawnError::Holder {
                message: message.unwrap_or_else(|| "the fake holder failed".into()),
            })
        } else if state.holders.contains_key(&launch.session) {
            Err(SpawnError::Exists {
                session: launch.session,
            })
        } else {
            let session = launch.session;
            let pid = 1000 + u32::try_from(state.order.len()).unwrap_or(0);
            let same = |other: &Launch| {
                other.program == launch.program
                    && other.cwd == launch.cwd
                    && other.args.last() == launch.args.last()
            };
            if state
                .holders
                .values()
                .any(|hold| hold.running() && same(&hold.launch))
            {
                state.overlaps += 1;
            }
            state.holders.insert(
                session,
                FakeHold {
                    launch,
                    lines: Vec::new(),
                    last_seq: 0,
                    exit: None,
                    pending: Vec::new(),
                    answered: HashSet::new(),
                    writes: Vec::new(),
                    subscribers: Vec::new(),
                    generation: 0,
                    alive: true,
                    stops: Vec::new(),
                    forgotten: false,
                    connects: Vec::new(),
                    pid,
                    orphan: false,
                    unconfirmable: None,
                },
            );
            state.order.push(session);
            Ok(())
        };
        let gate = state.spawn_gate.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                // The gate never has permits, so this waits until it closes.
                let _ = gate.acquire().await;
            }
            result
        })
    }

    fn list(&self) -> BoxFuture<io::Result<Vec<SessionId>>> {
        let state = lock(&self.state);
        let alive = state
            .order
            .iter()
            .filter(|session| state.holders.get(session).is_some_and(|hold| hold.alive))
            .copied()
            .collect();
        Box::pin(async move { Ok(alive) })
    }

    fn connect(
        &self,
        session: SessionId,
        after: Option<u64>,
    ) -> BoxFuture<Result<Connection, ConnectError>> {
        let result = self.open(session, after);
        Box::pin(async move { result })
    }

    fn end(&self, session: SessionId, holder_pid: Option<u32>) -> BoxFuture<Ended> {
        let mut state = lock(&self.state);
        state.ends.push((session, holder_pid));
        let ended = match state.holders.get_mut(&session) {
            Some(hold) => match hold.unconfirmable.clone() {
                Some(reason) => Ended::Unconfirmed {
                    pid: Some(hold.pid + 10_000),
                    reason,
                },
                None => {
                    hold.alive = false;
                    hold.orphan = false;
                    hold.subscribers.clear();
                    hold.end(None, Some(15));
                    Ended::Gone
                }
            },
            None => Ended::Gone,
        };
        Box::pin(async move { ended })
    }
}

impl FakeHolders {
    fn open(&self, session: SessionId, after: Option<u64>) -> Result<Connection, ConnectError> {
        let mut state = lock(&self.state);
        let Some(hold) = state.holders.get_mut(&session).filter(|hold| hold.alive) else {
            return Err(ConnectError::NotFound { session });
        };
        hold.connects.push(after);
        let last_seq = hold.last_seq;
        let cursor = after.map_or(last_seq, |after| after.min(last_seq));
        let (tx, rx) = mpsc::unbounded_channel();
        for line in hold.lines.iter().filter(|line| line.seq > cursor) {
            let mut line = line.clone();
            line.replayed = true;
            let _ = tx.send(Event::Line(line));
        }
        let child = match &hold.exit {
            Some(exit) => {
                let _ = tx.send(Event::Exited(exit.clone()));
                ChildState::Exited(exit.clone())
            }
            None => {
                hold.subscribers.push(tx);
                ChildState::Running { pid: hold.pid }
            }
        };
        let hello = Hello {
            holder_pid: hold.pid + 10_000,
            child,
            first_seq: hold.lines.first().map_or(last_seq + 1, |line| line.seq),
            last_seq,
            pending: hold.pending.iter().map(|(_, line)| line.clone()).collect(),
        };
        let events = futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed();
        let writer = FakeWriter {
            state: self.state.clone(),
            session,
            generation: hold.generation,
        };
        Ok(Connection {
            hello,
            events,
            writer: Arc::new(writer),
        })
    }
}

/// Writes to a fake holder, as one connection.
struct FakeWriter {
    state: Arc<Mutex<Holds>>,
    session: SessionId,
    generation: u64,
}

impl FakeWriter {
    fn with<T>(
        &self,
        f: impl FnOnce(&mut FakeHold) -> Result<T, WriteError>,
    ) -> Result<T, WriteError> {
        let mut state = lock(&self.state);
        match state.holders.get_mut(&self.session) {
            Some(hold) if hold.alive && hold.generation == self.generation => f(hold),
            _ => Err(WriteError::Disconnected),
        }
    }
}

impl HoldWriter for FakeWriter {
    fn write(
        &self,
        line: String,
        answers: Option<String>,
    ) -> BoxFuture<Result<WriteOutcome, WriteError>> {
        let result = self.with(|hold| {
            if hold.exit.is_some() {
                return Err(WriteError::Finished);
            }
            if let Some(request) = &answers {
                if hold.answered.contains(request) {
                    return Ok(WriteOutcome::AlreadyAnswered);
                }
                // Answers to requests the holder doesn't track pass through.
                if hold.pending.iter().any(|(id, _)| id == request) {
                    hold.pending.retain(|(id, _)| id != request);
                    hold.answered.insert(request.clone());
                }
            }
            hold.writes.push((line, answers));
            Ok(WriteOutcome::Written)
        });
        Box::pin(async move { result })
    }

    fn stop(&self, grace: Duration) -> BoxFuture<Result<(), WriteError>> {
        let lingers = lock(&self.state).lingers;
        let result = self.with(|hold| {
            hold.stops.push(grace);
            if !lingers {
                hold.end(None, Some(15));
            }
            Ok(())
        });
        Box::pin(async move { result })
    }

    fn forget(&self) -> BoxFuture<Result<(), WriteError>> {
        let mut state = lock(&self.state);
        let result = match state.holders.get_mut(&self.session) {
            Some(hold) if hold.exit.is_none() => Err(WriteError::StillRunning),
            Some(hold) => {
                hold.forgotten = true;
                hold.alive = false;
                Ok(())
            }
            None => Err(WriteError::Disconnected),
        };
        Box::pin(async move { result })
    }
}

/// A call to MaxHQ's read tool: its key, the agent that made it, the agent
/// it names, the item to read back from, the one item to read whole, how
/// many messages around it to read too and the position of a quote in it
/// to show in its place.
pub type ReadCall = (
    String,
    AgentId,
    String,
    Option<u64>,
    Option<String>,
    Option<usize>,
    Option<u32>,
);

/// What a scripted line has the fake driver report. Every field is
/// optional in the JSON.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Say {
    /// Agents to report.
    pub agents: Vec<Agent>,
    /// Timeline events.
    pub timeline: Vec<(AgentId, TimelineEvent)>,
    /// History to replace.
    pub history: Vec<(AgentId, Vec<TimelineItem>)>,
    /// PNGs whose previews to make: the agent, attachment and bytes.
    pub previews: Vec<(AgentId, AttachmentId, Vec<u8>)>,
    /// Attention events to open.
    pub open: Vec<AttentionEvent>,
    /// Attention events to close.
    pub close: Vec<AttentionId>,
    /// Agents that no longer exist.
    pub gone: Vec<AgentId>,
    /// Outcomes for actions.
    pub results: Vec<(IdempotencyKey, ActionOutcome)>,
    /// Lines to write back to the CLI.
    pub writes: Vec<String>,
    /// Rate limits.
    pub usage: Vec<(AccountId, Usage)>,
    /// Models.
    pub models: Vec<(AccountId, Vec<ModelChoice>)>,
    /// Asks for a tick this long from now.
    pub wake_ms: Option<u64>,
    /// Stops the holder.
    pub stop: bool,
    /// Starts a turn at this line, or with false ends the one open.
    pub turn: Option<bool>,
    /// Calls to MaxHQ's send tool: each call's key, the agent that made
    /// it and the paths it names.
    pub sends: Vec<(String, AgentId, Vec<String>)>,
    /// Calls to MaxHQ's read tool.
    pub reads: Vec<ReadCall>,
    /// Calls to any of MaxHQ's tools: each call's key, the agent that made
    /// it, the tool's name and its arguments.
    pub tools: Vec<(String, AgentId, String, Value)>,
    /// Commands answered "Always allow", as rules to keep.
    pub command_rules: Vec<CommandRule>,
    /// Kinds of file a card remembered for the agent's project.
    pub allowed_edits: Vec<EditKind>,
    /// With true, something only the running CLI holds keeps it from
    /// stopping when idle, and with false nothing does any more.
    pub keeps: Option<bool>,
}

impl Say {
    /// The line that carries it.
    pub fn line(&self) -> String {
        serde_json::to_string(self).expect("a scripted line serializes")
    }
}

/// Vendors whose drivers follow a script.
#[derive(Clone)]
pub struct FakeVendors {
    state: Arc<Mutex<Script>>,
    clock: Arc<dyn Clock>,
    gate: Arc<ReadGate>,
}

/// History reads a test holds back.
#[derive(Default)]
struct ReadGate {
    state: Mutex<Gated>,
    opened: Condvar,
}

#[derive(Default)]
struct Gated {
    /// The sessions whose reads wait.
    held: HashSet<String>,
    /// How many reads wait now.
    waiting: usize,
}

impl ReadGate {
    /// Waits while reads of `session` are held. It gives up after a while,
    /// so a failed test can't keep its runtime from ending.
    fn pass(&self, session: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut state = lock(&self.state);
        state.waiting += 1;
        while state.held.contains(session) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .opened
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.waiting -= 1;
    }
}

#[derive(Default)]
struct Script {
    /// What the drivers were asked to do, in order.
    log: Vec<String>,
    /// The pills each message a driver sent carried, by agent.
    sent_refs: Vec<(AgentId, Vec<maxhq_protocol::context::InlineRef>)>,
    /// Each Claude launch: its session UUID and whether it resumes.
    launches: Vec<(Uuid, bool)>,
    /// The kind of sign-in each Claude launch ran with.
    claude_logins: Vec<LoginKind>,
    /// Where each Claude session's driver was made for.
    claude_places: Vec<Place>,
    /// Each launch of an ACP agent.
    acp_launches: Vec<drive::Launch>,
    /// What each ACP driver was given from earlier processes.
    acp_kept: Vec<acp::Kept>,
    /// The history of each vendor session.
    histories: HashMap<String, Vec<TimelineItem>>,
    /// Each history read, by session, in the order they started.
    reads: Vec<String>,
    /// Sessions some other terminal still runs.
    elsewhere: HashSet<String>,
    /// Each agent a driver was handed as stored: its ID, start and last
    /// activity.
    met: Vec<(AgentId, Timestamp, Timestamp)>,
    /// Whether Claude sessions get the real driver and read their real
    /// transcripts, as a benchmark of reading them wants.
    real_claude: bool,
}

impl FakeVendors {
    /// Vendors with no history and nothing running elsewhere.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            state: Arc::default(),
            clock,
            gate: Arc::default(),
        }
    }

    /// What the drivers were asked to do, in order.
    pub fn log(&self) -> Vec<String> {
        lock(&self.state).log.clone()
    }

    /// The pills each message a driver sent carried, by agent, in order.
    pub fn sent_refs(&self) -> Vec<(AgentId, Vec<maxhq_protocol::context::InlineRef>)> {
        lock(&self.state).sent_refs.clone()
    }

    /// Each Claude launch: its session UUID and whether it resumes.
    pub fn launches(&self) -> Vec<(Uuid, bool)> {
        lock(&self.state).launches.clone()
    }

    /// The kind of sign-in each Claude launch ran with.
    pub fn claude_logins(&self) -> Vec<LoginKind> {
        lock(&self.state).claude_logins.clone()
    }

    /// Where each Claude session's driver was made for.
    pub fn claude_places(&self) -> Vec<Place> {
        lock(&self.state).claude_places.clone()
    }

    /// Each launch of an ACP agent.
    pub fn acp_launches(&self) -> Vec<drive::Launch> {
        lock(&self.state).acp_launches.clone()
    }

    /// What each ACP driver was given from earlier processes.
    pub fn acp_kept(&self) -> Vec<acp::Kept> {
        lock(&self.state).acp_kept.clone()
    }

    /// Each agent a new driver was handed as stored, in order: its ID,
    /// start and last activity.
    pub fn met(&self) -> Vec<(AgentId, Timestamp, Timestamp)> {
        lock(&self.state).met.clone()
    }

    /// Sets a session's history. A Claude sub-agent's is under
    /// `<session>/<sub-agent ID>`.
    pub fn set_history(&self, session: &str, items: Vec<TimelineItem>) {
        lock(&self.state)
            .histories
            .insert(session.to_owned(), items);
    }

    /// Holds back reads of a session's history until
    /// [`FakeVendors::release_reads`].
    pub fn hold_reads(&self, session: &str) {
        lock(&self.gate.state).held.insert(session.to_owned());
    }

    /// Lets the held reads of a session go on.
    pub fn release_reads(&self, session: &str) {
        lock(&self.gate.state).held.remove(session);
        self.gate.opened.notify_all();
    }

    /// The sessions whose history was read, in the order the reads
    /// started.
    pub fn reads(&self) -> Vec<String> {
        lock(&self.state).reads.clone()
    }

    /// How many history reads wait now.
    pub fn waiting_reads(&self) -> usize {
        lock(&self.gate.state).waiting
    }

    /// Gives Claude sessions the real driver, which reads the transcripts
    /// under the account's config folder. Launches stay fake.
    pub fn use_real_claude(&self) {
        lock(&self.state).real_claude = true;
    }

    /// Marks a session as running in another terminal, or not.
    pub fn run_elsewhere(&self, session: &str, running: bool) {
        let mut state = lock(&self.state);
        if running {
            state.elsewhere.insert(session.to_owned());
        } else {
            state.elsewhere.remove(session);
        }
    }

    fn driver(&self, vendor: Vendor) -> Box<dyn Driver> {
        Box::new(Scripted {
            vendor,
            state: self.state.clone(),
            clock: self.clock.clone(),
            turn_from: None,
            keeps: false,
            edits: false,
        })
    }
}

impl Vendors for FakeVendors {
    fn claude_launch(
        &self,
        login: &Login,
        place: &Place,
        session: Uuid,
        resume: bool,
        _model: Option<&str>,
        _mode: PermissionMode,
    ) -> drive::Launch {
        {
            let mut state = lock(&self.state);
            state.launches.push((session, resume));
            state
                .claude_logins
                .push(super::places::kind_of(&login.auth));
        }
        let flag = if resume { "--resume" } else { "--session-id" };
        drive::Launch {
            program: "fake-claude".into(),
            args: vec![flag.into(), session.to_string().into()],
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: place.cwd.clone(),
            framing: Framing::ClaudeStreamJson,
            secret_env: Vec::new(),
            run_as: None,
        }
    }

    fn claude_session(
        &self,
        login: &Login,
        place: &Place,
        agent: AgentId,
        mode: PermissionMode,
    ) -> Box<dyn Driver> {
        let real = {
            let mut state = lock(&self.state);
            state.claude_places.push(place.clone());
            state.real_claude
        };
        if real {
            return Wired { pictures: None }.claude_session(login, place, agent, mode);
        }
        self.driver(Vendor::Claude)
    }

    fn acp_launch(
        &self,
        vendor: Vendor,
        login: &Login,
        place: &Place,
        mode: PermissionMode,
    ) -> drive::Launch {
        // The real command lines. Cursor's only looks for the CLI, so it
        // gets a program that isn't it, and a local agent runs what it
        // names.
        let launch = match vendor {
            Vendor::Acp => local::launch(login, place),
            _ => drive::Launch {
                program: "fake-cursor".into(),
                ..cursor::launch(login, place, mode)
            },
        };
        lock(&self.state).acp_launches.push(launch.clone());
        launch
    }

    fn acp_session(
        &self,
        vendor: Vendor,
        _login: &Login,
        _place: &Place,
        _agent: AgentId,
        kept: acp::Kept,
        _mode: PermissionMode,
    ) -> Box<dyn Driver> {
        lock(&self.state).acp_kept.push(kept);
        self.driver(vendor)
    }

    fn codex_launch(&self, login: &Login) -> drive::Launch {
        drive::Launch {
            program: "fake-codex".into(),
            args: vec!["app-server".into()],
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: login.config_dir.clone(),
            framing: Framing::JsonRpc,
            secret_env: Vec::new(),
            run_as: None,
        }
    }

    fn codex_server(&self, _login: &Login, _device: DeviceId) -> Box<dyn Driver> {
        self.driver(Vendor::Codex)
    }

    fn can_continue(&self, _vendor: Vendor, _config_dir: &Path, session: &str) -> bool {
        !lock(&self.state).elsewhere.contains(session)
    }

    /// Tests stand in a `sleep` they start for the terminal's CLI.
    fn terminal_cli(&self, _vendor: Vendor) -> &'static str {
        "sleep"
    }

    fn history(
        &self,
        vendor: Vendor,
        config_dir: &Path,
        session: &str,
        subagent: Option<&str>,
        agent: AgentId,
    ) -> Option<Box<dyn History>> {
        if vendor == Vendor::Claude && lock(&self.state).real_claude {
            return Wired { pictures: None }.history(vendor, config_dir, session, subagent, agent);
        }
        let session = match subagent {
            Some(id) => format!("{session}/{id}"),
            None => session.to_owned(),
        };
        lock(&self.state).histories.contains_key(&session).then(|| {
            Box::new(FakeHistory {
                state: self.state.clone(),
                gate: self.gate.clone(),
                session,
                read: 0,
            }) as Box<dyn History>
        })
    }

    fn subagents(&self, vendor: Vendor, config_dir: &Path, session: &str) -> Vec<String> {
        let state = lock(&self.state);
        if vendor == Vendor::Claude && state.real_claude {
            return Wired { pictures: None }.subagents(vendor, config_dir, session);
        }
        let prefix = format!("{session}/");
        state
            .histories
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix))
            .map(str::to_owned)
            .collect()
    }
}

/// A session's history, read from the script.
struct FakeHistory {
    state: Arc<Mutex<Script>>,
    gate: Arc<ReadGate>,
    session: String,
    read: usize,
}

impl History for FakeHistory {
    fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        lock(&self.state).reads.push(self.session.clone());
        self.gate.pass(&self.session);
        let state = lock(&self.state);
        let items = state
            .histories
            .get(&self.session)
            .cloned()
            .unwrap_or_default();
        let new = items.into_iter().skip(self.read).collect::<Vec<_>>();
        self.read += new.len();
        Ok(new)
    }
}

/// A driver that reports what each line's [`Say`] names, and logs every
/// call. Claude finishes each action at once. Codex starts a thread only
/// once a line reports it, as the real app-server does.
struct Scripted {
    vendor: Vendor,
    state: Arc<Mutex<Script>>,
    clock: Arc<dyn Clock>,
    /// The seq of the line that started the open turn.
    turn_from: Option<u64>,
    /// Whether the script said the CLI holds something it can't stop with.
    keeps: bool,
    /// Whether the driver was given remembered kinds of file.
    edits: bool,
}

impl Scripted {
    fn log(&self, entry: String) {
        lock(&self.state).log.push(entry);
    }

    fn report(&self, say: Say, fx: &mut Effects) {
        fx.agents.extend(say.agents);
        fx.timeline.extend(say.timeline);
        fx.history.extend(say.history);
        fx.pictures
            .previews
            .extend(say.previews.into_iter().map(|(agent, id, bytes)| {
                crate::drive::pictures::ForPreview {
                    agent,
                    id,
                    mime: "image/png",
                    bytes,
                }
            }));
        fx.attention
            .extend(say.open.into_iter().map(AttentionChange::Open));
        fx.attention
            .extend(say.close.into_iter().map(AttentionChange::Close));
        fx.gone.extend(say.gone);
        fx.results.extend(say.results);
        fx.writes.extend(say.writes.into_iter().map(|line| Write {
            line,
            answers: None,
        }));
        fx.usage.extend(say.usage);
        fx.models.extend(say.models);
        fx.calls
            .extend(say.sends.into_iter().map(|(key, agent, paths)| ToolCall {
                call_id: key.clone(),
                key,
                agent,
                call: Call::SendAttachments(Send {
                    paths,
                    message: None,
                    to: Target::Own,
                }),
                arguments: Value::Null,
            }));
        fx.calls.extend(say.reads.into_iter().map(
            |(key, agent, name, before, item, around, position)| ToolCall {
                call_id: key.clone(),
                key,
                agent,
                call: Call::ReadAgent(Read {
                    agent: name,
                    before,
                    limit: PAGE,
                    item,
                    around,
                    position,
                    context: CONTEXT,
                    device: None,
                }),
                arguments: Value::Null,
            },
        ));
        fx.calls
            .extend(say.tools.into_iter().map(|(key, agent, tool, arguments)| {
                let call = Tool::named(&tool)
                    .expect("the fake calls a tool MaxHQ has")
                    .parse(&arguments)
                    .expect("the fake's arguments check out");
                ToolCall {
                    call_id: key.clone(),
                    key,
                    agent,
                    call,
                    arguments,
                }
            }));
        if let Some(ms) = say.wake_ms {
            fx.wake_at = Some(self.clock.now() + Duration::from_millis(ms));
        }
        fx.stop |= say.stop;
        fx.command_rules.extend(say.command_rules);
        fx.allowed_edits.extend(say.allowed_edits);
    }
}

impl Driver for Scripted {
    fn connected(&mut self, fresh: bool, pending: &[HeldLine], fx: &mut Effects) {
        self.log(format!("connected fresh={fresh} pending={}", pending.len()));
        // The prompts the CLI still waits on open again.
        for line in pending {
            if let Ok(say) = serde_json::from_str::<Say>(&line.text) {
                fx.attention
                    .extend(say.open.into_iter().map(AttentionChange::Open));
            }
        }
    }

    fn line(&mut self, line: &HeldLine, fx: &mut Effects) {
        self.log(format!("line {} replayed={}", line.seq, line.replayed));
        if let Ok(say) = serde_json::from_str::<Say>(&line.text) {
            match say.turn {
                Some(true) if self.turn_from.is_none() => self.turn_from = Some(line.seq),
                Some(false) => self.turn_from = None,
                _ => {}
            }
            if let Some(keeps) = say.keeps {
                self.keeps = keeps;
            }
            self.report(say, fx);
        }
    }

    fn act(&mut self, action: Action, fx: &mut Effects) {
        let key = action.key();
        match action {
            Action::Start { place, .. } if self.vendor == Vendor::Codex => {
                self.log(trust_of(&place));
                self.log(format!("start {key}"));
                return;
            }
            Action::Adopt { thread, place, .. } if self.vendor == Vendor::Codex => {
                self.log(trust_of(&place));
                self.log(format!("adopt {thread}"));
            }
            Action::Start { .. } | Action::Adopt { .. } => {
                fx.results.push((
                    key,
                    ActionOutcome::Refused {
                        reason: "Claude can't do that".into(),
                    },
                ));
                return;
            }
            Action::Send {
                agent,
                text,
                refs,
                attachments,
                ..
            } => {
                self.log(format!("send {text}"));
                lock(&self.state).sent_refs.push((agent, refs));
                let paths: Vec<String> = attachments
                    .iter()
                    .map(|staged| staged.path.display().to_string())
                    .collect();
                let line = serde_json::json!({ "send": text, "attachments": paths });
                fx.writes.push(Write {
                    line: line.to_string(),
                    answers: None,
                });
            }
            Action::Answer { event, answer, .. } => {
                self.log(format!("answer {event}"));
                self.log(format!("answered {}", serde_json::json!(answer)));
                let line = serde_json::json!({ "answer": event.to_string() });
                fx.writes.push(Write {
                    line: line.to_string(),
                    answers: Some(event.to_string()),
                });
                fx.attention.push(AttentionChange::Close(event));
            }
            Action::CancelQueued { item, .. } => self.log(format!("cancel {}", item.0)),
            Action::Interrupt { .. } => self.log("interrupt".into()),
            Action::SetPermission { mode, .. } => self.log(format!("mode {mode:?}")),
            Action::SetModel { model, .. } => self.log(format!("model {model}")),
            Action::SetEffort { effort, .. } => {
                self.log(format!("effort {}", effort.as_deref().unwrap_or("default")));
            }
            Action::Stop { .. } => {
                self.log("stop".into());
                fx.stop = self.vendor != Vendor::Codex;
            }
        }
        fx.results.push((key, ActionOutcome::Done));
    }

    fn exited(&mut self, exit: &ChildExit, _fx: &mut Effects) {
        self.log(format!(
            "exited code={:?} signal={:?} with_machine={}",
            exit.code, exit.signal, exit.with_machine
        ));
    }

    fn tick(&mut self, _now: Instant, _fx: &mut Effects) {
        self.log("tick".into());
    }

    fn tool_done(&mut self, done: ToolDone, _fx: &mut Effects) {
        match done.result {
            Ok(ToolResult::Sent(sent)) => {
                let paths: Vec<&str> = sent.iter().map(|file| file.path.as_str()).collect();
                self.log(format!("tool {} sent {}", done.key, paths.join(" ")));
            }
            Ok(ToolResult::Opened(opened)) => self.log(format!(
                "tool {} opened {} line={:?} outside={}",
                done.key, opened.path, opened.line, opened.outside
            )),
            Ok(ToolResult::Visual(visual)) => self.log(format!(
                "tool {} showed {} at {} replaces={:?} stretches={}",
                done.key,
                visual.title,
                visual.attachment.path,
                visual.replaces.map(|id| id.0),
                visual.stretches
            )),
            Ok(ToolResult::Text(text)) => self.log(format!("tool {} read {text}", done.key)),
            Ok(ToolResult::Mixed { text, images }) => self.log(format!(
                "tool {} read {text} with {} images",
                done.key,
                images.len()
            )),
            Err(reason) => self.log(format!("tool {} failed {reason}", done.key)),
        }
    }

    fn met_again(&mut self, known: &Agent) {
        lock(&self.state)
            .met
            .push((known.id, known.started_at, known.last_activity_at));
    }

    fn checkpoint(&self) -> Option<u64> {
        self.turn_from
    }

    fn can_release(&self, _agent: AgentId) -> bool {
        !self.keeps && self.turn_from.is_none()
    }

    fn release(&mut self, agent: AgentId, _fx: &mut Effects) {
        self.log(format!("release {agent}"));
    }

    fn command_rules(&mut self, rules: &[CommandRule]) {
        // Most tests have none, and keep their logs as they were.
        if rules.is_empty() {
            return;
        }
        let rules: Vec<&str> = rules.iter().map(|rule| rule.command.as_str()).collect();
        self.log(format!("rules {}", rules.join(", ")));
    }

    fn allowed_edits(&mut self, edits: Option<&[EditKind]>) {
        // Logged from the first that names any, as with the rules.
        let edits = edits.unwrap_or_default();
        if edits.is_empty() && !self.edits {
            return;
        }
        self.edits = true;
        let names: Vec<&str> = edits.iter().map(|kind| kind.name()).collect();
        self.log(format!("edits {}", names.join(", ")).trim_end().to_owned());
    }
}

/// Holder places over fake holders: this device's, and one set for each
/// container workspace a test brings up.
#[derive(Clone)]
pub struct FakePlaces {
    local: FakeHolders,
    vendors: FakeVendors,
    state: Arc<Mutex<Guests>>,
}

#[derive(Default)]
struct Guests {
    /// Each guest's holders, which outlive its link.
    holders: HashMap<WorkspaceId, FakeHolders>,
    /// The guests whose link is up, with their container's boot ID.
    up: HashMap<WorkspaceId, BootId>,
    /// Where link changes go, for the newest manager.
    changes: Option<mpsc::UnboundedSender<PlaceChange>>,
    /// Each worktree asked for: its workspace, agent and branch.
    worktrees: Vec<(WorkspaceId, AgentId, String)>,
    /// Each history opened in a guest: its workspace and config folder.
    histories: Vec<(WorkspaceId, String)>,
    /// What a folder scan in each guest answers, when it isn't empty.
    scans: HashMap<WorkspaceId, Result<FolderScan, String>>,
}

impl FakePlaces {
    /// This device's holders and no guests.
    pub fn new(local: FakeHolders, vendors: FakeVendors) -> Self {
        Self {
            local,
            vendors,
            state: Arc::default(),
        }
    }

    /// A guest's holders, up or down.
    pub fn guest_holders(&self, workspace: WorkspaceId) -> FakeHolders {
        lock(&self.state)
            .holders
            .entry(workspace)
            .or_default()
            .clone()
    }

    /// The guest's link comes up from a container with this boot ID.
    /// Sets what a folder scan in a guest answers.
    pub fn scan(&self, workspace: WorkspaceId, scan: Result<FolderScan, String>) {
        lock(&self.state).scans.insert(workspace, scan);
    }

    pub fn up(&self, workspace: WorkspaceId, boot: &str) {
        let holders = self.guest_holders(workspace).sessions();
        let mut state = lock(&self.state);
        let boot_id = BootId(boot.into());
        state.up.insert(workspace, boot_id.clone());
        if let Some(tx) = &state.changes {
            let _ = tx.send(PlaceChange::Up {
                workspace,
                boot_id,
                holders,
            });
        }
    }

    /// The guest's link goes down, and every connection to a holder there
    /// goes with it. The holders and their CLIs run on.
    pub fn down(&self, workspace: WorkspaceId, reason: &str) {
        {
            let mut state = lock(&self.state);
            state.up.remove(&workspace);
            if let Some(tx) = &state.changes {
                let _ = tx.send(PlaceChange::Down {
                    workspace,
                    reason: reason.into(),
                });
            }
        }
        let holders = self.guest_holders(workspace);
        for session in holders.sessions() {
            holders.drop_connections(session, &[]);
        }
    }

    /// The workspace is deleted, and its container goes with every holder
    /// and CLI in it.
    pub fn gone(&self, workspace: WorkspaceId) {
        let mut state = lock(&self.state);
        state.up.remove(&workspace);
        state.holders.remove(&workspace);
        if let Some(tx) = &state.changes {
            let _ = tx.send(PlaceChange::Gone { workspace });
        }
    }

    /// The container restarts: its link goes down, every CLI in it dies
    /// with the holders keeping their lines, and the link comes back with
    /// a new boot ID.
    pub fn reboot(&self, workspace: WorkspaceId, boot: &str) {
        self.down(workspace, "the container restarted");
        let holders = self.guest_holders(workspace);
        for session in holders.sessions() {
            if !holders.exited(session) {
                holders.crash(session);
            }
        }
        self.up(workspace, boot);
    }

    /// Each worktree asked for: its workspace, agent and branch.
    pub fn worktrees(&self) -> Vec<(WorkspaceId, AgentId, String)> {
        lock(&self.state).worktrees.clone()
    }

    /// Each history opened in a guest: its workspace and config folder.
    pub fn histories(&self) -> Vec<(WorkspaceId, String)> {
        lock(&self.state).histories.clone()
    }
}

impl HolderPlaces for FakePlaces {
    fn folder_scan(
        &self,
        workspace: WorkspaceId,
        _cwd: String,
        _locked: Vec<String>,
    ) -> BoxFuture<Result<FolderScan, String>> {
        let scan = lock(&self.state).scans.get(&workspace).cloned();
        Box::pin(async move { scan.unwrap_or_else(|| Ok(FolderScan::default())) })
    }

    fn local(&self) -> Arc<dyn Holders> {
        Arc::new(self.local.clone())
    }

    fn guest(&self, workspace: WorkspaceId) -> Option<Arc<dyn Holders>> {
        let up = lock(&self.state).up.contains_key(&workspace);
        up.then(|| Arc::new(self.guest_holders(workspace)) as Arc<dyn Holders>)
    }

    fn changes(&self) -> BoxStream<'static, PlaceChange> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = lock(&self.state);
        // A new manager hears at once about the links already up, as the
        // guest link's own start tells it.
        let up: Vec<(WorkspaceId, BootId)> = state
            .up
            .iter()
            .map(|(workspace, boot)| (*workspace, boot.clone()))
            .collect();
        for (workspace, boot_id) in up {
            let holders = state
                .holders
                .get(&workspace)
                .map(FakeHolders::sessions)
                .unwrap_or_default();
            let _ = tx.send(PlaceChange::Up {
                workspace,
                boot_id,
                holders,
            });
        }
        state.changes = Some(tx);
        futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|change| (change, rx))
        })
        .boxed()
    }

    fn worktree(
        &self,
        workspace: WorkspaceId,
        agent: AgentId,
        branch: String,
        _base: Option<String>,
    ) -> BoxFuture<Result<Worktree, String>> {
        let mut state = lock(&self.state);
        let result = if state.up.contains_key(&workspace) {
            state.worktrees.push((workspace, agent, branch.clone()));
            Ok(Worktree {
                path: format!("{}/.worktrees/{branch}", recipe::WORK),
                branch,
            })
        } else {
            Err("the guest isn't connected".to_owned())
        };
        Box::pin(async move { result })
    }

    fn history(
        &self,
        workspace: WorkspaceId,
        vendor: Vendor,
        config_dir: String,
        session: String,
        agent: AgentId,
        _bytes: Option<u64>,
    ) -> Option<Box<dyn History>> {
        {
            let mut state = lock(&self.state);
            if !state.up.contains_key(&workspace) {
                return None;
            }
            state.histories.push((workspace, config_dir.clone()));
        }
        self.vendors
            .history(vendor, Path::new(&config_dir), &session, None, agent)
    }
}

/// Container workspaces a test sets up, and what the manager recorded of
/// their agents.
#[derive(Clone, Default)]
pub struct FakeWorkspaces {
    state: Arc<Mutex<Managed>>,
}

#[derive(Default)]
struct Managed {
    containers: HashMap<WorkspaceId, Container>,
    recorded: Vec<(WorkspaceId, Vec<AgentSummary>)>,
    allowed: Vec<(ProjectId, AllowedWorkflowEnv)>,
}

impl FakeWorkspaces {
    /// Manages a workspace's container.
    pub fn add(&self, workspace: WorkspaceId, container: Container) {
        lock(&self.state).containers.insert(workspace, container);
    }

    /// Stops managing a workspace's container, as when it's deleted.
    pub fn remove(&self, workspace: WorkspaceId) {
        lock(&self.state).containers.remove(&workspace);
    }

    /// Each `user.maxhq.agents` written, oldest first.
    pub fn recorded(&self) -> Vec<(WorkspaceId, Vec<AgentSummary>)> {
        lock(&self.state).recorded.clone()
    }

    /// Each project's workflow variables kept, oldest first.
    pub fn allowed(&self) -> Vec<(ProjectId, AllowedWorkflowEnv)> {
        lock(&self.state).allowed.clone()
    }
}

impl Workspaces for FakeWorkspaces {
    fn container(&self, workspace: WorkspaceId) -> Option<Container> {
        lock(&self.state).containers.get(&workspace).cloned()
    }

    fn record_agents(
        &self,
        workspace: WorkspaceId,
        agents: Vec<AgentSummary>,
    ) -> BoxFuture<Result<(), String>> {
        lock(&self.state).recorded.push((workspace, agents));
        Box::pin(async { Ok(()) })
    }

    fn allow_workflow_env(
        &self,
        project: ProjectId,
        allowed: AllowedWorkflowEnv,
    ) -> Result<(), String> {
        lock(&self.state).allowed.push((project, allowed));
        Ok(())
    }
}

/// Answers to the check for unsaved work that a test sets, by checkout.
/// A checkout without one can't be told.
#[derive(Clone, Default)]
pub struct FakeUnsaved {
    state: Arc<Mutex<Checks>>,
}

#[derive(Default)]
struct Checks {
    answers: HashMap<Checkout, Option<bool>>,
    asked: Vec<Checkout>,
}

impl FakeUnsaved {
    /// Answers `unsaved` for the checkout from now on.
    pub fn answer(&self, checkout: Checkout, unsaved: Option<bool>) {
        lock(&self.state).answers.insert(checkout, unsaved);
    }

    /// Each checkout asked about, oldest first.
    pub fn asked(&self) -> Vec<Checkout> {
        lock(&self.state).asked.clone()
    }
}

impl Unsaved for FakeUnsaved {
    fn unsaved(&self, checkout: Checkout) -> BoxFuture<Option<bool>> {
        let mut state = lock(&self.state);
        let answer = state.answers.get(&checkout).copied().flatten();
        state.asked.push(checkout);
        Box::pin(async move { answer })
    }
}

/// A secret store a test fills.
#[derive(Clone, Default)]
pub struct FakeCredentials {
    state: Arc<Mutex<Stored>>,
}

#[derive(Default)]
struct Stored {
    tokens: HashMap<AccountId, Secret>,
    signed_in: HashSet<AccountId>,
    keys: HashMap<AccountId, Secret>,
    acp: HashMap<AcpAgentId, Vec<AcpEnvValue>>,
    servers: HashMap<AccountId, crate::drive::ServerLogin>,
}

impl FakeCredentials {
    /// Points a local agent's account at a model server.
    pub fn set_model_server(&self, account: AccountId, server: crate::drive::ServerLogin) {
        lock(&self.state).servers.insert(account, server);
    }

    /// Stores a setup-token for a Claude account.
    pub fn set_token(&self, account: AccountId, token: &str) {
        lock(&self.state)
            .tokens
            .insert(account, Secret::new(token.to_owned()));
    }

    /// Forgets a Claude account's setup-token.
    pub fn forget_token(&self, account: AccountId) {
        lock(&self.state).tokens.remove(&account);
    }

    /// Gives a Codex account a ChatGPT sign-in.
    pub fn sign_in(&self, account: AccountId) {
        lock(&self.state).signed_in.insert(account);
    }

    /// Stores an API key for a Cursor account.
    pub fn set_key(&self, account: AccountId, key: &str) {
        lock(&self.state)
            .keys
            .insert(account, Secret::new(key.to_owned()));
    }

    /// Stores a value for a local ACP agent's variable, in place of the
    /// one it had.
    pub fn set_acp_value(&self, agent: AcpAgentId, name: &str, value: &str) {
        let mut state = lock(&self.state);
        let values = state.acp.entry(agent).or_default();
        values.retain(|kept| kept.name != name);
        values.push(AcpEnvValue {
            name: name.to_owned(),
            value: Secret::new(value.to_owned()),
        });
    }
}

impl Credentials for FakeCredentials {
    fn claude_token(&self, account: AccountId) -> Option<Secret> {
        lock(&self.state).tokens.get(&account).cloned()
    }

    fn holds_claude_token(&self) -> bool {
        !lock(&self.state).tokens.is_empty()
    }

    fn has_chatgpt(&self, account: AccountId) -> bool {
        lock(&self.state).signed_in.contains(&account)
    }

    fn cursor_key(&self, account: AccountId) -> Option<Secret> {
        lock(&self.state).keys.get(&account).cloned()
    }

    fn acp_env(&self, agent: AcpAgentId) -> Vec<AcpEnvValue> {
        lock(&self.state)
            .acp
            .get(&agent)
            .cloned()
            .unwrap_or_default()
    }

    fn model_server(&self, account: AccountId) -> Option<crate::drive::ServerLogin> {
        lock(&self.state).servers.get(&account).cloned()
    }
}

/// One delivery: where to and which attachments.
pub type Delivered = (Place, Vec<AttachmentId>);

/// One send the fake collected: where, for which call, and the paths.
pub type Collected = (Place, String, Vec<String>);

/// One path the `@` menu found that the fake copied out: where, and the path.
pub type Found = (Place, String);

/// File delivery that moves staged attachments into the place's folder by
/// name only, and records each delivery.
#[derive(Clone)]
pub struct FakeFiles {
    attachments: Attachments,
    root: PathBuf,
    uploaded: Arc<Mutex<HashMap<AttachmentId, AttachmentRef>>>,
    /// Each staged attachment delivered, under the folder it went to.
    moved: Arc<Mutex<HashMap<AttachmentId, Staged>>>,
    delivered: Arc<Mutex<Vec<Delivered>>>,
    collected: Arc<Mutex<Vec<Collected>>>,
    found: Arc<Mutex<Vec<Found>>>,
    looked: Arc<Mutex<Vec<Found>>>,
    shown: Arc<Mutex<Vec<(Place, String, String)>>>,
}

impl FakeFiles {
    /// Delivers what's staged under `root`, the manager's attachments
    /// folder.
    pub fn new(root: PathBuf) -> Self {
        Self {
            attachments: Attachments::new(root.clone()),
            root,
            uploaded: Arc::default(),
            moved: Arc::default(),
            delivered: Arc::default(),
            collected: Arc::default(),
            found: Arc::default(),
            looked: Arc::default(),
            shown: Arc::default(),
        }
    }

    /// Each page shown, with its call and name, in order.
    pub fn shown(&self) -> Vec<(Place, String, String)> {
        lock(&self.shown).clone()
    }

    /// An upload that went straight into a container, which only the
    /// delivery knows, as with the transfers.
    pub fn uploaded(&self, attachment: AttachmentRef) {
        lock(&self.uploaded).insert(attachment.id, attachment);
    }

    /// Each delivery, in order.
    pub fn delivered(&self) -> Vec<Delivered> {
        lock(&self.delivered).clone()
    }

    /// Each send collected, in order.
    pub fn collected(&self) -> Vec<Collected> {
        lock(&self.collected).clone()
    }

    /// Each file an open checked, and where, in order.
    pub fn looked(&self) -> Vec<Found> {
        lock(&self.looked).clone()
    }

    /// Each path copied out for the `@` menu, in order.
    pub fn found(&self) -> Vec<Found> {
        lock(&self.found).clone()
    }
}

impl AgentFiles for FakeFiles {
    fn deliver(
        &self,
        place: &Place,
        staged: Vec<AttachmentId>,
    ) -> BoxFuture<Result<Vec<Staged>, DeliverError>> {
        let mut moved = Vec::new();
        for id in &staged {
            let id = *id;
            if let Some(attachment) = lock(&self.uploaded).remove(&id) {
                let path = PathBuf::from(&attachment.path);
                moved.push(Staged {
                    attachment,
                    path,
                    text: None,
                });
                continue;
            }
            // One delivered already stays where it went, and only that
            // folder's agent may name it again, as XF's delivery does.
            if let Some(staged) = lock(&self.moved).get(&id) {
                if !staged.path.starts_with(&place.attachments) {
                    return Box::pin(async move { Err(DeliverError::Missing(id)) });
                }
                moved.push(staged.clone());
                continue;
            }
            let Some(mut attachment) = self.attachments.find(id) else {
                return Box::pin(async move { Err(DeliverError::Missing(id)) });
            };
            let path = place
                .attachments
                .join(id.to_string())
                .join(&attachment.name);
            attachment.path = path.display().to_string();
            let _ = std::fs::remove_dir_all(self.root.join("staged").join(id.to_string()));
            let staged = Staged {
                attachment,
                path,
                text: None,
            };
            lock(&self.moved).insert(id, staged.clone());
            moved.push(staged);
        }
        lock(&self.delivered).push((place.clone(), staged));
        Box::pin(async move { Ok(moved) })
    }

    /// Names each file where the agent's attachments folder would keep it,
    /// without copying anything.
    fn collect(
        &self,
        place: &Place,
        call: &str,
        paths: Vec<String>,
    ) -> BoxFuture<Result<Vec<AttachmentRef>, String>> {
        let sent = paths
            .iter()
            .map(|path| {
                let name = Path::new(path)
                    .file_name()
                    .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
                AttachmentRef {
                    id: AttachmentId::derive(&format!("{call}:{path}")),
                    path: place
                        .attachments
                        .join(call)
                        .join(&name)
                        .display()
                        .to_string(),
                    name,
                    kind: AttachmentKind::File,
                    bytes: 0,
                    mime: None,
                    thumbnail: None,
                }
            })
            .collect();
        lock(&self.collected).push((place.clone(), call.to_owned(), paths));
        Box::pin(async move { Ok(sent) })
    }

    /// Stages an empty file named for the path, as if it came out of the
    /// container.
    fn stage_found(
        &self,
        place: &Place,
        id: AttachmentId,
        path: &str,
    ) -> BoxFuture<Result<AttachmentRef, String>> {
        lock(&self.found).push((place.clone(), path.to_owned()));
        let name = Path::new(path).file_name().map_or_else(
            || path.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let empty = AttachmentSource::Bytes {
            name,
            mime: "text/plain".to_owned(),
            data: String::new(),
        };
        let staged = self
            .attachments
            .stage(id, &empty)
            .map_err(|err| err.to_string());
        Box::pin(async move { staged })
    }

    /// Checks the file on this device, as the transfers do for a local
    /// agent.
    fn look(&self, place: &Place, path: String) -> BoxFuture<Result<Looked, String>> {
        lock(&self.looked).push((place.clone(), path.clone()));
        let roots = [place.cwd.clone(), place.attachments.clone()];
        let looked = crate::transfer::look::look_local(&roots, &path);
        Box::pin(async move { looked })
    }

    /// Reads a file on this device, named from the agent's folder, and
    /// names the page where the agent's attachments folder would keep it.
    fn show(
        &self,
        place: &Place,
        call: &str,
        page: Page,
        name: String,
    ) -> BoxFuture<Result<(AttachmentRef, String), String>> {
        let text = match page {
            Page::Html(html) => Ok(html),
            Page::Path(path) => std::fs::read_to_string(place.cwd.join(&path))
                .map_err(|err| format!("{path}: {err}")),
        };
        let shown = text.and_then(|text| {
            if text.len() > MAX_VISUAL_BYTES {
                return Err(too_big(text.len() as u64));
            }
            let attachment = AttachmentRef {
                id: AttachmentId::derive(&format!("visual:{call}")),
                path: place
                    .attachments
                    .join(call)
                    .join(&name)
                    .display()
                    .to_string(),
                name: name.clone(),
                kind: AttachmentKind::File,
                bytes: text.len() as u64,
                mime: Some("text/html".to_owned()),
                thumbnail: None,
            };
            Ok((attachment, text))
        });
        lock(&self.shown).push((place.clone(), call.to_owned(), name));
        Box::pin(async move { shown })
    }
}

/// Power control that remembers each change of its answer. It starts on
/// battery, and holds nothing until it's told a mode.
#[derive(Clone, Default)]
pub struct FakePower {
    state: Arc<Mutex<PowerState>>,
}

#[derive(Default)]
struct PowerState {
    mode: Option<KeepAwake>,
    busy: bool,
    on_charger: bool,
    changes: Vec<bool>,
}

impl FakePower {
    /// Each change, oldest first.
    pub fn changes(&self) -> Vec<bool> {
        lock(&self.state).changes.clone()
    }

    /// Plugs the device in or out, as the next charger check would find.
    pub fn plug(&self, on_charger: bool) {
        self.update(|state| state.on_charger = on_charger);
    }

    fn update(&self, change: impl FnOnce(&mut PowerState)) {
        let mut state = lock(&self.state);
        change(&mut state);
        let hold = state
            .mode
            .is_some_and(|mode| should_hold(mode, state.busy, state.on_charger));
        if state.changes.last().copied().unwrap_or(false) != hold {
            state.changes.push(hold);
        }
    }
}

impl Power for FakePower {
    fn set_mode(&self, mode: KeepAwake) {
        self.update(|state| state.mode = Some(mode));
    }

    fn set_busy(&self, busy: bool) {
        self.update(|state| state.busy = busy);
    }
}

/// Other devices, all answered by one manager here: each call another
/// device would get runs in the test's own manager, as if the caller's
/// home sent it. It counts the calls it carried.
#[derive(Clone, Default)]
pub struct FakePeers {
    sessions: Arc<Mutex<Option<(super::Sessions, DeviceId)>>>,
    calls: Arc<Mutex<Vec<(DeviceId, maxhq_protocol::agents::PeerCall)>>>,
    down: Arc<std::sync::atomic::AtomicBool>,
    home: Arc<Mutex<Option<DeviceId>>>,
    names: Arc<Mutex<HashMap<DeviceId, String>>>,
    claimed_mode: Arc<Mutex<Option<PermissionMode>>>,
}

impl FakePeers {
    /// Answers each call with `sessions`, as sent from `home`.
    pub fn answer_with(&self, sessions: &super::Sessions, home: DeviceId) {
        *lock(&self.sessions) = Some((sessions.clone(), home));
    }

    /// Takes the line down, or brings it back up.
    pub fn set_down(&self, down: bool) {
        self.down.store(down, std::sync::atomic::Ordering::SeqCst);
    }

    fn is_down(&self) -> bool {
        self.down.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Names the device that runs the MaxHQ app, as a device's link book
    /// does.
    pub fn set_home(&self, home: DeviceId) {
        *lock(&self.home) = Some(home);
    }

    /// Names `device` in the link book.
    pub fn name(&self, device: DeviceId, name: &str) {
        lock(&self.names).insert(device, name.to_owned());
    }

    /// Has each agent that leaves claim it ran in `mode`, as a device that
    /// lies would.
    pub fn claim_mode(&self, mode: PermissionMode) {
        *lock(&self.claimed_mode) = Some(mode);
    }

    /// The manager that answers, unless the line is down.
    fn answering(&self) -> Option<(super::Sessions, DeviceId)> {
        lock(&self.sessions).clone().filter(|_| !self.is_down())
    }

    /// Each call carried, with the device it went to.
    pub fn calls(&self) -> Vec<(DeviceId, maxhq_protocol::agents::PeerCall)> {
        lock(&self.calls).clone()
    }
}

impl crate::agents::PeerCalls for FakePeers {
    fn call(
        &self,
        device: DeviceId,
        call: maxhq_protocol::agents::PeerCall,
    ) -> crate::agents::Answer<'_> {
        lock(&self.calls).push((device, call.clone()));
        let answering = self.answering();
        Box::pin(async move {
            match answering {
                Some((sessions, home)) => Ok(sessions.peer_call(home, call).await),
                None => Err(crate::agents::Unreachable("no line to that device".into())),
            }
        })
    }

    fn move_step(
        &self,
        _device: DeviceId,
        step: maxhq_protocol::agents::AgentMoveStep,
    ) -> crate::agents::Stepped<'_> {
        let answering = self.answering();
        let claimed = *lock(&self.claimed_mode);
        Box::pin(async move {
            let Some((sessions, home)) = answering else {
                return Err(crate::agents::Unreachable("no line to that device".into()));
            };
            let mut outcome = sessions.peer_move(home, step).await;
            if let (ActionOutcome::Departed { departure }, Some(mode)) = (&mut outcome, claimed) {
                departure.mode = mode;
            }
            Ok(outcome)
        })
    }

    fn home(&self) -> Option<DeviceId> {
        *lock(&self.home)
    }

    fn name_of(&self, device: DeviceId) -> Option<String> {
        lock(&self.names).get(&device).cloned()
    }

    fn reach(&self, _device: DeviceId) -> crate::agents::Woken<'_> {
        let down = self.is_down();
        Box::pin(async move {
            if down {
                Err(crate::agents::Unreachable("no line to that device".into()))
            } else {
                Ok(())
            }
        })
    }
}

/// Test hosts that answer every lease call with one scripted result, and
/// remember each call and who made it.
#[derive(Clone)]
pub struct FakeLeases {
    calls: Arc<Mutex<Vec<(crate::leases::LeaseCaller, maxhq_core::leases::LeaseCall)>>>,
    answer: Arc<Mutex<Result<ToolResult, String>>>,
}

impl Default for FakeLeases {
    fn default() -> Self {
        Self {
            calls: Arc::default(),
            answer: Arc::new(Mutex::new(Ok(ToolResult::Text("leased".into())))),
        }
    }
}

impl FakeLeases {
    /// Answers each call from now on with `answer`.
    pub fn answer(&self, answer: Result<ToolResult, String>) {
        *lock(&self.answer) = answer;
    }

    /// Each call that reached the hosts, with its caller.
    pub fn calls(&self) -> Vec<(crate::leases::LeaseCaller, maxhq_core::leases::LeaseCall)> {
        lock(&self.calls).clone()
    }
}

impl crate::leases::Leases for FakeLeases {
    fn run(
        &self,
        caller: crate::leases::LeaseCaller,
        call: maxhq_core::leases::LeaseCall,
    ) -> crate::leases::LeaseAnswer<'_> {
        lock(&self.calls).push((caller, call));
        let answer = lock(&self.answer).clone();
        Box::pin(async move { answer })
    }
}
