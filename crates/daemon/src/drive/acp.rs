// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The driver for one held agent spoken to over the Agent Client Protocol
//! on its CLI's stdio. One process holds one session. What differs between
//! the CLIs that speak it, such as Cursor's own requests and the texts its
//! failures come as, sits behind a [`Flavor`].
//!
//! A fresh process gets `initialize`, then `session/load` when the agent
//! had a session and the CLI can load one, or else `session/new`. The
//! replay `session/load` sends is the transcript: it becomes the agent's
//! history, and MaxHQ keeps no copy of its own. Item IDs come from each
//! turn's number and each item's place in it, so the replay names each
//! item as it was named live.
//!
//! What the CLI sends is checked against a recording of Cursor's
//! 2026.10.22 (see `acp/tests`). A live turn sends no echo of your
//! message, no usage and nothing but `stopReason` in its result.
//! Everything between sending `session/load` and its result is the
//! replay: your messages, one whole chunk per thought and reply, and each
//! tool call whole. The result then brings the config options but no
//! `sessionId`. The config options are the one place models and modes are
//! read from, and a model's ID there isn't the name a CLI's `--model`
//! takes: Cursor's Auto is `default`. So the process starts without a
//! model, and the model you picked is asked for once the session is up.
//! Not in the recording, and still read from ACP's schema:
//! `session/cancel`, `session/set_config_option`, sub-agents, usage, and
//! the -32000 error for a sign-in.
//!
//! Requests the driver doesn't know get "method not found". A replayed
//! request the holder no longer waits on was answered by an earlier `maxd`
//! and isn't opened again. Replayed lines send whole items, never pieces,
//! so reading them twice changes nothing.
//!
//! A flavor can name replies that are failures, so a turn that ends with
//! one of them ends as an error, and can give each tool call a time limit,
//! after which the turn is cancelled.
//!
//! Models and modes come from the config options, or else from ACP v1's
//! `models` and `modes`, which change through `session/set_model` and
//! `session/set_mode`. A mode whose ID or name is "plan" is MaxHQ's Plan.
//! A flavor whose CLI signs itself in answers ACP's -32000 from a session
//! request with one `authenticate`, through the first method the CLI runs
//! itself, and asks again. A CLI that speaks ACP v2 is
//! spoken to through [`rpc::Wire`], which gives the driver v1's shapes, and
//! one whose `initialize` reply is in neither shape is refused.

mod actions;
pub(in crate::drive) mod events;
pub mod features;
mod history;
pub mod launch;
mod prompt;
mod requests;
pub(crate) mod rpc;
mod subs;

#[cfg(test)]
mod tests;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use maxhq_protocol::acp::AcpFeatures;
use maxhq_protocol::agents::McpCommand;
use maxhq_protocol::ids::{AgentId, AttentionId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, AgentPermission, CommandRule, ContextUsage, Control, ModelChoice,
    PermissionMode, Vendor,
};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, NoticeLevel, TimelineEvent, Todo};
use serde_json::Value;

use self::actions::Queued;
use self::events::{Out, Stream};
use self::requests::Prompt;
use self::rpc::{text, Ids, Incoming, RpcError, Wire};
use self::subs::Sub;
use crate::drive::{
    commands, Action, ActionKey, AttentionChange, ChildExit, Driver, Effects, HeldLine, Login,
    Place, PlaceHost, Write,
};

/// What an exit reason keeps of stderr's last line.
const REASON_CHARS: usize = 200;

/// How long a title made from a prompt gets.
const TITLE_CHARS: usize = 60;

/// What one CLI does its own way. The driver asks its flavor for names,
/// for the CLI's own notifications and requests, and for what its replies
/// and errors mean.
pub trait Flavor: Send {
    /// The CLI's name in what MaxHQ says, such as "Cursor".
    fn name(&self) -> &str;
    /// The vendor its agents have.
    fn vendor(&self) -> Vendor;
    /// What the stream's item IDs start with.
    fn base(&self) -> &str;
    /// The `_meta` that `initialize` gives with the client's capabilities.
    fn client_meta(&self) -> Value;
    /// One of the CLI's own notifications, or none for one it doesn't
    /// know.
    fn notification(&self, method: &str, params: &Value) -> Option<Ext>;
    /// One of the CLI's own requests, or none for one it doesn't know,
    /// which gets "method not found".
    fn request(&self, method: &str, params: &Value) -> Option<ExtPrompt>;
    /// What a turn's whole reply turned out to be, if it was a failure.
    fn reply_failure(&self, reply: &str) -> Option<Failure>;
    /// Whether an error answer is the usage limit.
    fn is_limit(&self, error: &RpcError) -> bool;
    /// How long one tool call may run before the driver cancels the turn,
    /// or none for no limit.
    fn tool_timeout(&self) -> Option<Duration>;
    /// The model ID that leaves the pick to the CLI, which a session is
    /// never asked for.
    fn auto_model(&self) -> Option<&str>;
    /// Whether the process starts in its mode from launch flags: plan from
    /// one, which the session can change later, and full access only from
    /// one, so changing to or from it takes a new process.
    fn modes_are_launch_flags(&self) -> bool;
    /// Whether the CLI signs itself in when asked, so a sign-in error from
    /// a session request gets `authenticate` and one more try.
    fn authenticates(&self) -> bool;
    /// Why the CLI stopped, when its stderr says more than the plain exit
    /// reason would.
    fn stop_reason(&self, _stderr: &str) -> Option<String> {
        None
    }
}

/// A CLI's own notification, as the driver reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum Ext {
    /// The todo list, whole or as changes to merge in by ID.
    Todos {
        /// Each todo by its ID.
        todos: Vec<(String, Todo)>,
        /// Whether these change the list rather than replace it.
        merge: bool,
    },
    /// A sub-agent started, changed or ended.
    Subagent(Task),
}

/// A sub-agent's task, as a CLI reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// The lead's tool call that started it.
    pub call: String,
    /// The kind the CLI gave it, such as "explore".
    pub kind: Option<String>,
    /// What the lead asked it to do.
    pub description: Option<String>,
    /// Its model, if the CLI named one.
    pub model: Option<String>,
    /// Set once it ended.
    pub ended: bool,
}

/// A CLI's own request, which waits on your answer.
#[derive(Clone, Debug, PartialEq)]
pub enum ExtPrompt {
    /// A plan to approve before the agent starts on it.
    Plan(Plan),
    /// Questions to answer.
    Question {
        /// What they're about, if the CLI said.
        title: Option<String>,
        /// The questions.
        questions: Vec<Asked>,
    },
}

/// A plan a CLI asks you to approve.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The plan, as Markdown.
    pub markdown: String,
    /// Its todos, by ID.
    pub todos: Vec<(String, Todo)>,
    /// What the plan is about, in a line.
    pub detail: Option<String>,
}

/// One question a CLI asks.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    /// Its ID, to echo in the answer.
    pub id: String,
    /// What it asks.
    pub text: String,
    /// Its options as ID and label.
    pub options: Vec<(String, String)>,
    /// Whether more than one option may be picked.
    pub multi: bool,
}

/// What a reply turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The CLI's sign-in no longer works.
    SignIn,
    /// The turn failed, with the CLI's words for why.
    Error(String),
}

/// What MaxHQ kept of an agent from its earlier processes.
#[derive(Clone, Debug, Default)]
pub struct Kept {
    /// The ACP session, which a fresh process loads.
    pub session: Option<String>,
    /// The model you picked, as the config option's ID.
    pub model: Option<String>,
    /// The title the agent last had.
    pub title: Option<String>,
    /// The `maxd mcp` that serves the agent tools, which each session
    /// request names. None leaves the CLI without them.
    pub tools: Option<McpCommand>,
}

/// How far the process has come.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// No process is connected.
    Down,
    /// `initialize` and the session request are out.
    Handshake,
    /// The session is up.
    Ready,
    /// The CLI can't run this agent, for the reason given.
    Refused(String),
}

/// A request the driver sent, by its ID.
#[derive(Clone, Debug)]
enum Call {
    /// `initialize`.
    Initialize,
    /// `session/new`.
    New,
    /// `session/load`.
    Load,
    /// `session/prompt`.
    Prompt,
    /// A model change.
    SetModel {
        /// The client's key, or none for the model a new session is
        /// asked for.
        key: Option<IdempotencyKey>,
        /// The model asked for.
        model: String,
    },
    /// A mode change, for plan mode.
    SetMode {
        /// The client's key, or none for the mode a new session is asked
        /// for.
        key: Option<IdempotencyKey>,
        /// The mode asked for.
        mode: PermissionMode,
    },
    /// `authenticate`, after which the session request goes again.
    Authenticate {
        /// Whether that request is `session/load`.
        load: bool,
    },
}

/// How a session's model or mode changes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Picker {
    /// Through the config option with this ID.
    Option(String),
    /// Through ACP v1's `session/set_model` or `session/set_mode`.
    Legacy,
}

/// The turn `session/prompt` is running.
#[derive(Debug)]
struct Turn {
    /// Set once `session/cancel` is out.
    cancelling: bool,
    /// Set when a tool call ran out of time.
    timed_out: bool,
}

/// The driver for one agent.
pub struct AcpSession {
    flavor: Box<dyn Flavor>,
    login: Login,
    place: Place,
    lead: AgentId,
    ids: Ids,
    /// The connection's ACP, v1 or v2.
    wire: Wire,
    calls: HashMap<String, Call>,
    phase: Phase,
    /// The ACP session, once the CLI gave one, which a fresh process
    /// loads.
    session_id: Option<String>,
    /// The `maxd mcp` each session request names.
    tools: Option<McpCommand>,
    /// What `initialize` and the session said the CLI can do, once it
    /// said.
    features: Option<AcpFeatures>,
    /// The features last sent, for an ACP agent's account.
    shown_features: Option<AcpFeatures>,
    /// The sign-in method `authenticate` uses, if the CLI runs one itself.
    sign_in: Option<String>,
    /// Set once `authenticate` went out, so it's tried once.
    authenticated: bool,
    requested: PermissionMode,
    effective: PermissionMode,
    /// Whether the process started with its sandbox off, which lasts for
    /// its life.
    launched_full: bool,
    model: Option<String>,
    /// The model you picked, which each session is asked for.
    picked: Option<String>,
    /// How the model changes, if the CLI lets it.
    model_option: Option<Picker>,
    /// How the mode changes, if the CLI lets it.
    mode_option: Option<Picker>,
    /// The CLI's mode that is MaxHQ's Plan, if it has one.
    plan_mode: Option<String>,
    /// The mode leaving plan goes back to: the last other one it ran.
    work_mode: Option<String>,
    /// The running turn, if a prompt is out.
    turn: Option<Turn>,
    /// Messages waiting for the turn to end.
    queue: VecDeque<Queued>,
    /// Actions that came before the session was up.
    held: Vec<Action>,
    /// The lead's items.
    stream: Stream,
    /// Whether the stream has a turn that hasn't ended.
    open: bool,
    /// Whether the last update was a piece of a user message.
    last_user: bool,
    /// The user message a replay is sending, so far.
    user_sent: String,
    /// Events from a `session/load` replay, which become history.
    loading: Option<Out>,
    /// Set from a replay's first user message to the next reply, the
    /// load's. A new `maxd` reads the replay a load sent from the
    /// holder's log too, and what it shows isn't activity.
    replaying: bool,
    prompts: HashMap<AttentionId, Prompt>,
    /// Requests the holder still waits on an answer to.
    pending: HashSet<String>,
    subs: Vec<Sub>,
    /// When each open tool call runs out of time, by `toolCallId`.
    deadlines: HashMap<String, Instant>,
    /// Set when the last turn ended on a usage limit.
    limited: bool,
    errored: Option<String>,
    title: Option<String>,
    /// Whether the title is the CLI's or one kept from before, which a
    /// replayed first message doesn't replace.
    named: bool,
    context: Option<ContextUsage>,
    started_at: Timestamp,
    last_activity_at: Timestamp,
    link: AgentLink,
    exited: bool,
    stopping: bool,
    /// Whether the line being read is a replayed one.
    quiet: bool,
    first_seq: Option<u64>,
    last_seq: Option<u64>,
    shown: HashMap<AgentId, Agent>,
    /// The commands the project's rules let run without asking.
    command_rules: Vec<CommandRule>,
}

impl AcpSession {
    /// A driver for `agent`, which loads the kept session when it has
    /// one.
    pub fn new(
        flavor: Box<dyn Flavor>,
        login: &Login,
        place: &Place,
        agent: AgentId,
        kept: Kept,
        mode: PermissionMode,
    ) -> Self {
        Self::with_ids(flavor, login, place, agent, kept, mode, Ids::new())
    }

    /// A driver whose request IDs come from `ids`, for tests.
    fn with_ids(
        flavor: Box<dyn Flavor>,
        login: &Login,
        place: &Place,
        agent: AgentId,
        kept: Kept,
        mode: PermissionMode,
        ids: Ids,
    ) -> Self {
        let now = Timestamp::now();
        let launched_full = flavor.modes_are_launch_flags() && mode == PermissionMode::FullAccess;
        let stream = Stream::new(agent, flavor.base());
        Self {
            flavor,
            login: login.clone(),
            place: place.clone(),
            lead: agent,
            ids,
            wire: Wire::default(),
            calls: HashMap::new(),
            phase: Phase::Down,
            session_id: kept.session,
            tools: kept.tools,
            features: None,
            shown_features: None,
            sign_in: None,
            authenticated: false,
            requested: mode,
            effective: mode,
            launched_full,
            model: None,
            picked: kept.model,
            model_option: None,
            mode_option: None,
            plan_mode: None,
            work_mode: None,
            turn: None,
            queue: VecDeque::new(),
            held: Vec::new(),
            stream,
            open: false,
            last_user: false,
            user_sent: String::new(),
            loading: None,
            replaying: false,
            prompts: HashMap::new(),
            pending: HashSet::new(),
            subs: Vec::new(),
            deadlines: HashMap::new(),
            limited: false,
            errored: None,
            named: kept.title.is_some(),
            title: kept.title,
            context: None,
            started_at: now,
            last_activity_at: now,
            link: AgentLink::Unknown,
            exited: false,
            stopping: false,
            quiet: false,
            first_seq: None,
            last_seq: None,
            shown: HashMap::new(),
            command_rules: Vec::new(),
        }
    }

    /// Sends a request and remembers what it was.
    fn send(&mut self, method: &str, params: Value, call: Call, fx: &mut Effects) {
        let id = self.ids.next();
        fx.writes.push(Write {
            line: self.wire.request(&id, method, params),
            answers: None,
        });
        self.calls.insert(id, call);
    }

    /// The working folder as the CLI takes it.
    fn cwd(&self) -> String {
        self.place.cwd.to_string_lossy().into_owned()
    }

    /// The folders a command may reach without asking. A guest's paths aren't
    /// on this file system, so they're compared as they're written.
    fn folders(&self) -> commands::Folders {
        commands::Folders::new(
            &self.place.cwd,
            &self.place.attachments,
            self.place.host == PlaceHost::Local,
        )
    }

    /// Starts the handshake with a fresh process.
    fn handshake(&mut self, fx: &mut Effects) {
        self.phase = Phase::Handshake;
        // A fresh process says again which ACP it speaks.
        self.wire = Wire::default();
        let params = rpc::initialize(self.flavor.client_meta());
        self.send("initialize", params, Call::Initialize, fx);
    }

    /// Reads one line, live or replayed.
    fn take(&mut self, text: &str, at: Timestamp, replayed: bool, fx: &mut Effects) {
        let Some(incoming) = self.wire.read(text) else {
            tracing::debug!(
                cli = self.flavor.name(),
                "skipping a line that isn't JSON-RPC or has no use"
            );
            return;
        };
        self.quiet = replayed;
        self.stream.quiet = replayed;
        match incoming {
            Incoming::Response { id, outcome } => self.response(&id, outcome, at, fx),
            Incoming::Request { id, method, params } => {
                self.request(id, &method, &params, at, replayed, fx)
            }
            Incoming::Notification { method, params } => {
                self.notification(&method, &params, at, fx)
            }
        }
        self.quiet = false;
        self.stream.quiet = false;
    }

    /// The answer to a request. One the driver didn't send, such as one
    /// from before a restart, is read by its shape.
    fn response(
        &mut self,
        id: &Value,
        outcome: Result<Value, RpcError>,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        self.replaying = false;
        let call = rpc::key(id).and_then(|key| self.calls.remove(&key));
        let ours = call.is_some();
        let call = match call {
            Some(call) => call,
            None => match &outcome {
                Ok(result) if result.get("stopReason").is_some() => Call::Prompt,
                Ok(result) if result.get("sessionId").is_some() => Call::New,
                Ok(result) if result.get("agentCapabilities").is_some() => Call::Initialize,
                Ok(_) if self.loading.is_some() => Call::Load,
                Ok(result) => {
                    self.options(result, fx);
                    return;
                }
                Err(_) => return,
            },
        };
        match call {
            Call::Initialize => self.initialized(outcome, ours, fx),
            Call::New => self.created(outcome, fx),
            Call::Load => self.loaded(outcome, at, fx),
            Call::Prompt => self.prompted(outcome, at, fx),
            Call::SetModel { key, model } => self.model_set(key, &model, outcome, at, fx),
            Call::SetMode { key, mode } => self.mode_set(key, mode, outcome, at, fx),
            Call::Authenticate { load } => self.signed_in(load, outcome, fx),
        }
    }

    /// `initialize` answered: reads what the CLI can do and asks for the
    /// session.
    fn initialized(&mut self, outcome: Result<Value, RpcError>, ours: bool, fx: &mut Effects) {
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                let reason = format!("{} didn't start: {}", self.flavor.name(), error.message);
                self.refuse(&reason, fx);
                return;
            }
        };
        let mut found = features::features(&result);
        found.plan = self.plan_mode.is_some();
        self.features = Some(found);
        self.sign_in = features::agent_sign_in(&result);
        // One read back after a restart says what the CLI can do, and what
        // follows it up to the session's result may be a load's replay.
        if !ours {
            if self.can_load() && self.session_id.is_some() && self.turn.is_none() {
                self.start_load();
            }
            return;
        }
        if self.phase != Phase::Handshake {
            return;
        }
        if self.wire.generation().is_none() {
            let reason = format!("{}.", rpc::unspoken(self.flavor.name(), &result));
            self.refuse(&reason, fx);
            return;
        }
        match self.session_id.clone() {
            Some(session) if self.can_load() => self.load_session(&session, fx),
            Some(_) => {
                let message = format!(
                    "This version of {} can't load the earlier conversation, so the agent starts a new one.",
                    self.flavor.name()
                );
                self.notice(NoticeLevel::Warning, &message, Timestamp::now(), fx);
                self.session_id = None;
                self.new_session(fx);
            }
            None => self.new_session(fx),
        }
    }

    /// Asks for a new session.
    fn new_session(&mut self, fx: &mut Effects) {
        let params = rpc::new_session(&self.cwd(), self.tools.as_ref());
        self.send("session/new", params, Call::New, fx);
    }

    /// Asks to load `session`, reading its replay into history.
    fn load_session(&mut self, session: &str, fx: &mut Effects) {
        self.start_load();
        let params = rpc::load_session(session, &self.cwd(), self.tools.as_ref());
        self.send("session/load", params, Call::Load, fx);
    }

    /// Whether the CLI loads an earlier session.
    fn can_load(&self) -> bool {
        self.features.as_ref().is_some_and(|found| found.load)
    }

    /// Whether the CLI takes images inline.
    fn can_image(&self) -> bool {
        self.features.as_ref().is_some_and(|found| found.images)
    }

    /// Answers a sign-in error from a session request with `authenticate`,
    /// once, when the flavor's CLI signs itself in through a method it
    /// listed. False when it can't, and the error stands.
    fn authenticate(&mut self, load: bool, fx: &mut Effects) -> bool {
        if !self.flavor.authenticates() || self.authenticated {
            return false;
        }
        let Some(method) = self.sign_in.clone() else {
            return false;
        };
        self.authenticated = true;
        let params = rpc::authenticate(&method);
        self.send("authenticate", params, Call::Authenticate { load }, fx);
        true
    }

    /// `authenticate` answered: asks for the session again, or ends here.
    fn signed_in(&mut self, load: bool, outcome: Result<Value, RpcError>, fx: &mut Effects) {
        if outcome.is_err() {
            let reason = self.signed_out_text();
            self.refuse(&reason, fx);
            return;
        }
        match self.session_id.clone() {
            Some(session) if load => self.load_session(&session, fx),
            _ => self.new_session(fx),
        }
    }

    /// Starts reading a replay into history. The replay counts its turns
    /// from the first, so the stream does too.
    fn start_load(&mut self) {
        self.loading = Some(Vec::new());
        self.stream = Stream::new(self.lead, self.flavor.base());
        self.open = false;
        self.last_user = false;
    }

    /// `session/new` answered.
    fn created(&mut self, outcome: Result<Value, RpcError>, fx: &mut Effects) {
        // A log read back after a restart can't tell a new session from a
        // load until this comes.
        self.loading = None;
        match outcome {
            Ok(result) => {
                if let Some(session) = text(&result, "sessionId") {
                    self.session_id = Some(session.to_owned());
                }
                self.options(&result, fx);
                self.pick_model(fx);
                if !self.pick_mode(fx) {
                    self.ready(fx);
                }
            }
            Err(error) if self.rpc_signed_out(&error) && self.authenticate(false, fx) => {}
            Err(error) if self.rpc_signed_out(&error) => {
                let reason = self.signed_out_text();
                // A CLI that can't sign itself in signs in again from the
                // feed, through the error that names its account.
                if !self.flavor.authenticates() {
                    let body = ItemBody::Error {
                        message: reason.clone(),
                        retrying: false,
                        sign_in: Some(self.login.account),
                    };
                    self.aside("error", body, Timestamp::now(), fx);
                }
                self.refuse(&reason, fx);
            }
            Err(error) => {
                let reason = format!(
                    "{} couldn't start a session: {}",
                    self.flavor.name(),
                    error.message
                );
                self.refuse(&reason, fx);
            }
        }
    }

    /// `session/load` answered: the replay so far is the history. Its
    /// result has no `sessionId`.
    fn loaded(&mut self, outcome: Result<Value, RpcError>, at: Timestamp, fx: &mut Effects) {
        match outcome {
            Ok(result) => {
                let mut replay = self.loading.take().unwrap_or_default();
                self.stream.finish(ItemStatus::Completed, at, &mut replay);
                self.open = false;
                self.last_user = false;
                let items = history::fold(self.lead, replay);
                fx.history.push((self.lead, items));
                self.options(&result, fx);
                self.pick_model(fx);
                if !self.pick_mode(fx) {
                    self.ready(fx);
                }
            }
            Err(error) if self.flavor.authenticates() && self.rpc_signed_out(&error) => {
                // A new session would need the same sign-in.
                self.loading = None;
                if !self.authenticate(true, fx) {
                    let reason = self.signed_out_text();
                    self.refuse(&reason, fx);
                }
            }
            Err(error) => {
                // The CLI may have lost the session. A new one keeps the
                // agent going, without the earlier conversation.
                self.loading = None;
                self.stream = Stream::new(self.lead, self.flavor.base());
                self.open = false;
                self.session_id = None;
                self.notice(
                    NoticeLevel::Warning,
                    &format!(
                        "{} couldn't load the earlier conversation ({}), so the agent starts a new one.",
                        self.flavor.name(),
                        error.message
                    ),
                    at,
                    fx,
                );
                self.new_session(fx);
            }
        }
    }

    /// The session is up: runs what waited for it.
    pub(super) fn ready(&mut self, fx: &mut Effects) {
        self.phase = Phase::Ready;
        self.link = AgentLink::Live;
        for action in std::mem::take(&mut self.held) {
            self.action(action, fx);
        }
        self.next_queued(fx);
    }

    /// Ends the agent's chances on this process.
    fn refuse(&mut self, reason: &str, fx: &mut Effects) {
        self.phase = Phase::Refused(reason.to_owned());
        self.errored = Some(reason.to_owned());
        self.loading = None;
        for action in std::mem::take(&mut self.held) {
            refused(fx, action.key(), reason);
        }
        for queued in std::mem::take(&mut self.queue) {
            self.drop_queued(queued, Timestamp::now(), fx);
        }
    }

    /// A notification from the CLI. What the agent says or does is
    /// activity, unless a load replays it.
    fn notification(&mut self, method: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let news = if method == "session/update" {
            self.update(params, at, fx)
        } else {
            match self.flavor.notification(method, params) {
                Some(Ext::Todos { todos, merge }) => {
                    let mut out = Out::new();
                    self.begin_if_needed();
                    self.stream.todos(todos, merge, at, &mut out);
                    self.emit(out, fx);
                    true
                }
                Some(Ext::Subagent(task)) => {
                    self.task(task, at, fx);
                    true
                }
                None => false,
            }
        };
        if news && self.loading.is_none() && !self.replaying {
            self.touch(at);
        }
    }

    /// One `session/update`, and whether it's the agent's work rather than
    /// the session's settings.
    fn update(&mut self, params: &Value, at: Timestamp, fx: &mut Effects) -> bool {
        let Some(update) = params.get("update") else {
            return false;
        };
        let kind = text(update, "sessionUpdate").unwrap_or_default();
        if let Some(session) = text(params, "sessionId")
            && self
                .session_id
                .as_deref()
                .is_some_and(|ours| ours != session)
        {
            self.sub_update(session, update, kind, at, fx);
            return true;
        }
        let news = matches!(
            kind,
            "agent_message_chunk"
                | "agent_thought_chunk"
                | "tool_call"
                | "tool_call_update"
                | "plan"
        );
        let mut out = Out::new();
        let user = kind == "user_message_chunk";
        match kind {
            "user_message_chunk" => self.user_chunk(update, at, &mut out),
            "agent_message_chunk" | "agent_thought_chunk" => {
                if let Some(chunk) = chunk_text(update) {
                    self.begin_if_needed();
                    self.stream
                        .chunk(kind == "agent_thought_chunk", &chunk, at, &mut out);
                }
            }
            "tool_call" | "tool_call_update" => {
                self.begin_if_needed();
                if let Some((call, ended)) = self.stream.tool(update, at, &mut out) {
                    self.tool_seen(&call, ended, at, fx);
                }
            }
            "plan" => {
                self.begin_if_needed();
                self.stream
                    .todos(events::plan_entries(update), false, at, &mut out);
            }
            "current_mode_update" => {
                if let Some(mode) = text(update, "currentModeId") {
                    self.mode_is(mode);
                }
            }
            "config_option_update" => self.options(update, fx),
            "session_info_update" => {
                if let Some(title) = text(update, "title").and_then(title_of) {
                    self.title = Some(title);
                    self.named = true;
                }
            }
            // Not in the recording: no turn of 2026.10.22 sent usage.
            "usage_update" => {
                let used = update.get("used").and_then(Value::as_u64);
                let size = update.get("size").and_then(Value::as_u64);
                if let (Some(used), Some(size)) = (used, size) {
                    self.context = Some(ContextUsage {
                        used_tokens: u32::try_from(used).unwrap_or(u32::MAX),
                        window_tokens: u32::try_from(size).unwrap_or(u32::MAX),
                    });
                }
            }
            _ => {}
        }
        if !user {
            self.last_user = false;
        }
        self.emit(out, fx);
        news
    }

    /// A piece of a user message, which only a replay sends. Each new one
    /// starts a turn.
    fn user_chunk(&mut self, update: &Value, at: Timestamp, out: &mut Out) {
        // A live turn sends no echo, and its own message is already on the
        // timeline if one ever comes.
        if self.turn.is_some() {
            return;
        }
        self.replaying = true;
        let Some(chunk) = chunk_text(update) else {
            return;
        };
        if !self.last_user {
            self.stream.finish(ItemStatus::Completed, at, out);
            self.stream.begin(self.stream.turn + 1);
            self.open = true;
            self.user_sent.clear();
        }
        self.user_sent.push_str(&chunk);
        self.last_user = true;
        if self.stream.turn == 1 && !self.named {
            self.title = title_of(&maxhq_core::context::readable(&self.user_sent));
        }
        let item = self.stream.item(
            self.stream.id("user"),
            ItemStatus::Completed,
            at,
            history::user_message(self.flavor.base(), &self.user_sent),
        );
        events::upsert(out, item);
    }

    /// Starts a turn for output that came without one, as replayed output
    /// does.
    fn begin_if_needed(&mut self) {
        if !self.open {
            self.stream.begin(self.stream.turn + 1);
            self.open = true;
        }
    }

    /// Sends events where they belong: to the replay while a load runs,
    /// and to the timeline otherwise.
    fn emit(&mut self, out: Out, fx: &mut Effects) {
        match &mut self.loading {
            Some(replay) => replay.extend(out),
            None => fx.timeline.extend(out),
        }
    }

    /// A tool call changed. Its clock restarts, unless a prompt waits on
    /// you for it. A call to the agent tools has none: `maxd` answers it,
    /// and a wait may rightly run longer than the flavor's time limit.
    fn tool_seen(&mut self, call: &str, ended: bool, at: Timestamp, fx: &mut Effects) {
        let ours = self.stream.tool_raw(call).is_some_and(events::ours);
        if ended {
            self.deadlines.remove(call);
            self.prompts_done_for(call, at, fx);
            self.sub_tool_ended(call, at, fx);
        } else if ours {
            self.deadlines.remove(call);
        } else if let Some(timeout) = self.flavor.tool_timeout()
            && self.loading.is_none()
            && !self.quiet_waiting(call)
        {
            self.deadlines
                .insert(call.to_owned(), Instant::now() + timeout);
        }
    }

    /// Whether a prompt for the call waits on you.
    fn quiet_waiting(&self, call: &str) -> bool {
        self.prompts
            .values()
            .any(|prompt| prompt.call.as_deref() == Some(call))
    }

    /// `session/prompt` answered: the turn is over.
    fn prompted(&mut self, outcome: Result<Value, RpcError>, at: Timestamp, fx: &mut Effects) {
        if !self.open {
            // A turn that sent nothing at all, read back after a restart.
            self.stream.begin(self.stream.turn + 1);
            self.open = true;
        }
        let turn = self.turn.take();
        let timed_out = turn.as_ref().is_some_and(|turn| turn.timed_out);
        let mut out = Out::new();
        match outcome {
            Ok(result) => {
                let reason = text(&result, "stopReason").unwrap_or("end_turn");
                match reason {
                    "cancelled" => {
                        self.stream.finish(ItemStatus::Interrupted, at, &mut out);
                        if !timed_out {
                            let item = self.stream.item(
                                self.stream.id("interrupted"),
                                ItemStatus::Completed,
                                at,
                                ItemBody::Interrupted,
                            );
                            events::upsert(&mut out, item);
                        }
                    }
                    "max_tokens" => {
                        let message = format!(
                            "The reply ran past the longest one {} allows.",
                            self.flavor.name()
                        );
                        self.turn_notice(&message, at, &mut out);
                    }
                    "max_turn_requests" => {
                        let message = format!(
                            "The turn hit {}'s limit on model requests.",
                            self.flavor.name()
                        );
                        self.turn_notice(&message, at, &mut out);
                    }
                    "refusal" => {
                        let message =
                            format!("{} wouldn't carry on with this turn.", self.flavor.name());
                        self.turn_notice(&message, at, &mut out);
                    }
                    _ => self.ended(at, &mut out),
                }
            }
            Err(error) => self.turn_failed(&error, at, &mut out),
        }
        self.stream.finish(ItemStatus::Completed, at, &mut out);
        self.open = false;
        self.emit(out, fx);
        self.turn_over(at, fx);
    }

    /// A turn that ended as the CLI says it should. A reply that is one of
    /// the flavor's failure texts becomes an error.
    fn ended(&mut self, at: Timestamp, out: &mut Out) {
        self.stream.close_text(at, out);
        let Some(reply) = self.stream.reply.clone() else {
            return;
        };
        let said = match &reply.body {
            ItemBody::AssistantMessage { text, .. } => text.clone(),
            _ => return,
        };
        let Some(failure) = self.flavor.reply_failure(&said) else {
            return;
        };
        out.push((self.lead, TimelineEvent::Remove { id: reply.id }));
        let (message, sign_in) = match failure {
            Failure::SignIn => {
                let message = self.signed_out_text();
                self.errored = Some(message.clone());
                (message, Some(self.login.account))
            }
            Failure::Error(message) => (message, None),
        };
        let item = self.stream.item(
            self.stream.id("error"),
            ItemStatus::Completed,
            at,
            ItemBody::Error {
                message,
                retrying: false,
                sign_in,
            },
        );
        events::upsert(out, item);
    }

    /// A turn the CLI answered with an error.
    fn turn_failed(&mut self, error: &RpcError, at: Timestamp, out: &mut Out) {
        self.stream.finish(ItemStatus::Failed, at, out);
        let body = if self.flavor.is_limit(error) {
            self.limited = true;
            ItemBody::LimitWait {
                account: Some(self.login.account),
                resets_at: None,
                auto_resume: false,
            }
        } else if self.rpc_signed_out(error) {
            let message = self.signed_out_text();
            self.errored = Some(message.clone());
            ItemBody::Error {
                message,
                retrying: false,
                sign_in: Some(self.login.account),
            }
        } else {
            ItemBody::Error {
                message: format!("{} stopped the turn: {}", self.flavor.name(), error.message),
                retrying: false,
                sign_in: None,
            }
        };
        let item = self
            .stream
            .item(self.stream.id("error"), ItemStatus::Completed, at, body);
        events::upsert(out, item);
    }

    /// A notice that ends a turn.
    fn turn_notice(&mut self, message: &str, at: Timestamp, out: &mut Out) {
        self.stream.close_text(at, out);
        let item = self.stream.item(
            self.stream.id("stop"),
            ItemStatus::Completed,
            at,
            ItemBody::Notice {
                level: NoticeLevel::Warning,
                message: message.to_owned(),
            },
        );
        events::upsert(out, item);
    }

    /// Tidies up after a turn and sends the next queued message.
    fn turn_over(&mut self, at: Timestamp, fx: &mut Effects) {
        self.deadlines.clear();
        self.drop_prompts(at, fx);
        self.subs_over(at, fx);
        self.touch(at);
        self.next_queued(fx);
    }

    /// A notice on its own, outside any tool call.
    fn notice(&mut self, level: NoticeLevel, message: &str, at: Timestamp, fx: &mut Effects) {
        let body = ItemBody::Notice {
            level,
            message: message.to_owned(),
        };
        self.aside("notice", body, at, fx);
    }

    /// An item on its own, outside any tool call, with an ID that starts
    /// with `kind`.
    fn aside(&mut self, kind: &str, body: ItemBody, at: Timestamp, fx: &mut Effects) {
        let id = ItemId(format!("{}:{kind}:{}", self.flavor.base(), self.ids.next()));
        let item = self.stream.item(id, ItemStatus::Completed, at, body);
        let mut out = Out::new();
        events::upsert(&mut out, item);
        self.emit(out, fx);
    }

    /// Reads the session's models and modes from a result or an update:
    /// its config options, or else v1's `models` and `modes`. Cursor sends
    /// both, which say the same things, and then only `configOptions` is
    /// read.
    fn options(&mut self, result: &Value, fx: &mut Effects) {
        let mut models = None;
        let mut modes = None;
        if let Some(options) = result.get("configOptions").and_then(Value::as_array) {
            for option in options {
                let category = text(option, "category");
                let Some(id) = text(option, "id") else {
                    continue;
                };
                let current = text(option, "currentValue");
                match category {
                    Some("model") => {
                        self.model_option = Some(Picker::Option(id.to_owned()));
                        if let Some(current) = current {
                            self.model = Some(current.to_owned());
                        }
                        models = Some(choices(option, current));
                    }
                    Some("mode") => {
                        self.mode_option = Some(Picker::Option(id.to_owned()));
                        modes = Some((choices(option, current), current.map(str::to_owned)));
                    }
                    _ => {}
                }
            }
        }
        if models.is_none()
            && let Some((listed, current)) = features::legacy(result, &features::MODELS)
        {
            self.model_option = Some(Picker::Legacy);
            if current.is_some() {
                self.model = current;
            }
            models = Some(listed);
        }
        if modes.is_none() {
            modes = features::legacy(result, &features::MODES);
            if modes.is_some() {
                self.mode_option = Some(Picker::Legacy);
            }
        }
        if let Some((listed, current)) = modes {
            self.modes_are(&listed);
            if let Some(current) = current {
                self.mode_is(&current);
            }
        }
        if let Some(models) = models.filter(|models| !models.is_empty()) {
            fx.models.push((self.login.account, models));
        }
    }

    /// Notes which of the CLI's modes is plan, and which one leaving plan
    /// goes to until the CLI runs another.
    fn modes_are(&mut self, modes: &[ModelChoice]) {
        if modes.is_empty() {
            return;
        }
        self.plan_mode = features::plan_mode(modes).map(str::to_owned);
        if self.work_mode.is_none() {
            self.work_mode = modes
                .iter()
                .find(|mode| Some(mode.id.as_str()) != self.plan_mode.as_deref())
                .map(|mode| mode.id.clone());
        }
        if let Some(found) = &mut self.features {
            found.plan = self.plan_mode.is_some();
        }
    }

    /// Whether `mode` is the CLI's plan mode. Before the CLI lists its
    /// modes, plan is the one called "plan".
    fn is_plan(&self, mode: &str) -> bool {
        match &self.plan_mode {
            Some(plan) => plan == mode,
            None => mode == features::PLAN,
        }
    }

    /// The CLI says which mode it's in.
    fn mode_is(&mut self, mode: &str) {
        if self.is_plan(mode) {
            self.effective = PermissionMode::Plan;
            return;
        }
        self.work_mode = Some(mode.to_owned());
        if self.effective == PermissionMode::Plan {
            self.effective = match self.requested {
                PermissionMode::Plan => PermissionMode::Ask,
                other => other,
            };
            if self.launched_full {
                self.effective = PermissionMode::FullAccess;
            }
        }
    }

    /// Notes that something happened now.
    fn touch(&mut self, at: Timestamp) {
        if at.0 > self.last_activity_at.0 {
            self.last_activity_at = at;
        }
    }

    /// The lead as it stands.
    fn lead_agent(&self) -> Agent {
        let activity = if let Some(attention) = self.waiting_on(self.lead) {
            Activity::NeedsInput { attention }
        } else if self.limited && self.turn.is_none() {
            Activity::WaitingForLimitReset { resets_at: None }
        } else if let Some(reason) = &self.errored {
            Activity::Error {
                reason: reason.clone(),
            }
        } else if matches!(self.phase, Phase::Down | Phase::Handshake) && !self.exited {
            Activity::Starting
        } else if self.turn.is_some() && self.subs.iter().any(|sub| !sub.done) {
            Activity::BusyInSubagents
        } else if self.turn.is_some() {
            Activity::Busy
        } else {
            Activity::Idle
        };
        Agent {
            id: self.lead,
            workspace: self.place.workspace,
            branch: None,
            parent: None,
            vendor: self.flavor.vendor(),
            account: Some(self.login.account),
            model: self.model.clone(),
            permission: Some(AgentPermission {
                requested: self.requested,
                effective: Some(self.effective),
            }),
            session: None,
            vendor_session_id: self.session_id.clone(),
            control: Control::Held,
            pid: None,
            link: self.link.clone(),
            title: self.title.clone().unwrap_or_else(|| self.default_title()),
            activity,
            context: self.context,
            started_at: self.started_at,
            last_activity_at: self.last_activity_at,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
        }
    }

    /// The name of the working folder, for a session with no prompt yet.
    fn default_title(&self) -> String {
        self.place
            .cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("{} agent", self.flavor.name()))
    }

    /// What an error item says when the CLI's sign-in has run out, or
    /// for a CLI that signs itself in, that it needs one.
    fn signed_out_text(&self) -> String {
        if self.flavor.authenticates() {
            return needs_sign_in(self.flavor.name());
        }
        format!(
            "{}'s sign-in has run out. Sign in again to keep going.",
            self.flavor.name()
        )
    }

    /// Whether an error says the CLI needs a sign-in. ACP gives -32000 for
    /// that.
    fn rpc_signed_out(&self, error: &RpcError) -> bool {
        error.code == -32000 || self.flavor.reply_failure(&error.message) == Some(Failure::SignIn)
    }

    /// The prompt `agent` waits on, if it waits on one.
    fn waiting_on(&self, agent: AgentId) -> Option<AttentionId> {
        self.prompts
            .iter()
            .filter(|(_, prompt)| prompt.item.agent == agent)
            .min_by_key(|(_, prompt)| prompt.item.started_at.map(|at| at.0))
            .map(|(id, _)| *id)
    }

    /// Sends every agent that changed and the next time to wake.
    fn settle(&mut self, fx: &mut Effects) {
        let mut agents = vec![self.lead_agent()];
        agents.extend(
            self.subs
                .iter()
                .filter(|sub| !sub.done)
                .map(|sub| self.sub_agent(sub)),
        );
        for agent in agents {
            if self.shown.get(&agent.id) != Some(&agent) {
                self.shown.insert(agent.id, agent.clone());
                fx.agents.push(agent);
            }
        }
        fx.wake_at = self.deadlines.values().min().copied();
        if self.flavor.vendor() == Vendor::Acp
            && let Some(found) = &self.features
            && self.shown_features.as_ref() != Some(found)
        {
            self.shown_features = Some(found.clone());
            fx.features.push((self.login.account, found.clone()));
        }
    }

    /// Ends what was running when the CLI ended.
    fn shut(&mut self, at: Timestamp, fx: &mut Effects) {
        let mut out = Out::new();
        self.stream.finish(ItemStatus::Interrupted, at, &mut out);
        self.loading = None;
        self.open = false;
        self.emit(out, fx);
        self.turn = None;
        self.deadlines.clear();
        self.drop_prompts(at, fx);
        self.subs_over(at, fx);
        for queued in std::mem::take(&mut self.queue) {
            self.drop_queued(queued, at, fx);
        }
        for action in std::mem::take(&mut self.held) {
            let reason = format!("The agent's {} process ended.", self.flavor.name());
            refused(fx, action.key(), &reason);
        }
        self.calls.clear();
        self.phase = Phase::Down;
    }

    /// Cancels the running turn.
    fn cancel_turn(&mut self, at: Timestamp, fx: &mut Effects) {
        // ACP asks the client to answer what it still waits on as
        // cancelled.
        if self.write_cancel(fx) {
            self.drop_prompts(at, fx);
        }
    }

    /// Sends `session/cancel` for the running turn, unless it went
    /// already. Says whether it went now.
    fn write_cancel(&mut self, fx: &mut Effects) -> bool {
        let Some(session) = self.session_id.clone() else {
            return false;
        };
        if let Some(turn) = &mut self.turn {
            if turn.cancelling {
                return false;
            }
            turn.cancelling = true;
        }
        fx.writes.push(Write {
            line: rpc::cancel(&session),
            answers: None,
        });
        true
    }
}

impl Driver for AcpSession {
    fn connected(&mut self, fresh: bool, pending: &[HeldLine], fx: &mut Effects) {
        if fresh && self.phase != Phase::Down && !matches!(self.phase, Phase::Refused(_)) {
            self.shut(Timestamp::now(), fx);
        }
        self.exited = false;
        self.pending = pending
            .iter()
            .filter_map(|line| rpc::parse(&line.text))
            .filter_map(|incoming| match incoming {
                Incoming::Request { id, .. } => rpc::key(&id),
                _ => None,
            })
            .collect();
        if fresh {
            self.link = AgentLink::Live;
            self.handshake(fx);
        } else {
            self.link = AgentLink::Live;
            if self.phase == Phase::Down {
                // The process ran on while `maxd` was away, and has its
                // session already.
                self.phase = if self.session_id.is_some() {
                    Phase::Ready
                } else {
                    Phase::Handshake
                };
            }
        }
        for line in pending {
            self.take(&line.text, line.at, false, fx);
        }
        self.settle(fx);
    }

    fn line(&mut self, line: &HeldLine, fx: &mut Effects) {
        if self.last_seq.is_some_and(|seen| line.seq <= seen) {
            return;
        }
        self.first_seq.get_or_insert(line.seq);
        self.last_seq = Some(line.seq);
        self.take(&line.text, line.at, line.replayed, fx);
        self.settle(fx);
    }

    fn act(&mut self, action: Action, fx: &mut Effects) {
        match &self.phase {
            Phase::Ready => self.action(action, fx),
            Phase::Refused(reason) => match action {
                Action::Stop { key, .. } => {
                    self.stopping = true;
                    fx.stop = true;
                    done(fx, key);
                }
                other => {
                    let reason = reason.clone();
                    refused(fx, other.key(), &reason);
                }
            },
            Phase::Down | Phase::Handshake => match action {
                Action::Stop { key, .. } => {
                    self.stopping = true;
                    fx.stop = true;
                    done(fx, key);
                }
                Action::Send { .. } => self.action(action, fx),
                other => self.held.push(other),
            },
        }
        self.settle(fx);
    }

    fn exited(&mut self, exit: &ChildExit, fx: &mut Effects) {
        let reason = self
            .flavor
            .stop_reason(&exit.stderr_tail)
            .filter(|_| exit.code != Some(0))
            .unwrap_or_else(|| exit_reason(self.flavor.name(), exit));
        let failed =
            exit.code != Some(0) && !self.stopping && !exit.with_machine && self.errored.is_none();
        // A CLI that stops while it waits for you cuts nothing off, so its
        // exit isn't activity.
        let news = failed
            || self.turn.is_some()
            || !self.prompts.is_empty()
            || !self.queue.is_empty()
            || self.subs.iter().any(|sub| !sub.done);
        if failed {
            self.errored = Some(reason.clone());
        }
        self.link = AgentLink::Down {
            reason,
            since: Some(exit.at),
        };
        self.exited = true;
        self.shut(exit.at, fx);
        if news {
            self.touch(exit.at);
        }
        self.settle(fx);
    }

    fn tick(&mut self, now: Instant, fx: &mut Effects) {
        self.timeouts(now, fx);
        self.settle(fx);
    }

    fn met_again(&mut self, known: &Agent) {
        if known.id == self.lead {
            self.started_at = known.started_at;
            self.last_activity_at = known.last_activity_at;
        }
    }

    fn checkpoint(&self) -> Option<u64> {
        // The replay a load sent and every turn since name their items by
        // turn number, so a new driver reads the process's whole log.
        self.first_seq
    }

    fn command_rules(&mut self, rules: &[CommandRule]) {
        rules.clone_into(&mut self.command_rules);
    }

    /// Only a CLI that loads its sessions comes back whole, and only
    /// between turns, with nothing asked or sent still open.
    fn can_release(&self, _agent: AgentId) -> bool {
        self.phase == Phase::Ready
            && self.can_load()
            && self.session_id.is_some()
            && !self.exited
            && !self.stopping
            && self.turn.is_none()
            && !self.open
            && self.loading.is_none()
            && self.queue.is_empty()
            && self.held.is_empty()
            && self.calls.is_empty()
            && self.prompts.is_empty()
            && self.pending.is_empty()
            && self.deadlines.is_empty()
            && self.subs.iter().all(|sub| sub.done)
    }
}

/// What an agent that signs itself in says when it needs a sign-in, which
/// the app's sign-in card goes with.
pub fn needs_sign_in(name: &str) -> String {
    format!("{name} needs a sign-in.")
}

/// The text of a content chunk.
fn chunk_text(update: &Value) -> Option<String> {
    let content = update.get("content")?;
    match text(content, "type") {
        Some("text") => text(content, "text").map(str::to_owned),
        _ => None,
    }
}

/// The models a config option offers, flat, with `current` as the
/// default. Options can come in groups.
fn choices(option: &Value, current: Option<&str>) -> Vec<ModelChoice> {
    let mut found = Vec::new();
    let mut stack: Vec<&Value> = option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    stack.reverse();
    while let Some(entry) = stack.pop() {
        if let Some(group) = entry.get("options").and_then(Value::as_array) {
            stack.extend(group.iter().rev());
            continue;
        }
        let Some(value) = text(entry, "value") else {
            continue;
        };
        found.push(ModelChoice {
            id: value.to_owned(),
            label: text(entry, "name").unwrap_or(value).to_owned(),
            default: Some(value) == current,
            efforts: Vec::new(),
            default_effort: None,
        });
    }
    found
}

/// A title from the first line of a prompt.
fn title_of(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().nth(TITLE_CHARS).is_some() {
        title.truncate(title.trim_end().len());
        title.push('…');
    }
    Some(title)
}

/// Why the CLI `name` ended, for the agent's link.
fn exit_reason(name: &str, exit: &ChildExit) -> String {
    let how = match (exit.code, exit.signal) {
        (Some(code), _) => format!("{name} exited with code {code}"),
        (None, Some(signal)) => format!("{name} was stopped by signal {signal}"),
        (None, None) => format!("{name} exited"),
    };
    let last = exit
        .stderr_tail
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty());
    match last {
        Some(line) => {
            let line: String = line.chars().take(REASON_CHARS).collect();
            format!("{how}: {line}")
        }
        None => how,
    }
}

/// Answers an action as done.
fn done(fx: &mut Effects, key: IdempotencyKey) {
    fx.results.push((key, ActionOutcome::Done));
}

/// Answers an action as refused.
fn refused(fx: &mut Effects, key: IdempotencyKey, reason: &str) {
    fx.results.push((
        key,
        ActionOutcome::Refused {
            reason: reason.to_owned(),
        },
    ));
}

/// Closes an attention event.
fn close_attention(fx: &mut Effects, id: AttentionId) {
    fx.attention.push(AttentionChange::Close(id));
}
