// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The driver for one held `claude -p` process. It reads the CLI's lines
//! into timeline items, agent state and prompts, and turns your actions
//! into lines for its stdin.
//!
//! A reconnect replays lines the driver may have seen. Lines at or below
//! the last sequence number it read are dropped, and replayed lines go
//! through a quiet book, which sends whole items until they're sent. A
//! replayed prompt opens only while the holder still waits on it.
//!
//! After a restart a new driver reads the open turn again from its first
//! line, which [`Driver::checkpoint`] names. The transcript it reads first
//! already has the turn's finished messages, so their replayed text adds
//! nothing, and the message still streaming is built again from its
//! pieces.

mod actions;
mod channel;
mod prompts;
mod release;
mod seed;
mod tasks;
mod waits;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use maxhq_protocol::ids::{AgentId, AttentionId};
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, AgentPermission, ContextUsage, Control, PermissionMode, Vendor,
};
use maxhq_protocol::processes::AgentRunning;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemId, ItemStatus, NoticeLevel};
use serde_json::Value;

use super::convo::{self, Convo, Ended};
use super::limits;
use super::tools;
use super::wire::{self, clip, number, text};
use crate::drive::{Action, ChildExit, Driver, Effects, HeldLine, JobDone, Login, Place, ToolDone};
use crate::observe::claude::transcript::LastTurn;

use self::actions::Ours;
use self::channel::Running;
use self::prompts::{Kind, Prompt};
use self::tasks::{Sub, Task};
use self::waits::Held;

/// How long a title made from a prompt gets.
const TITLE_CHARS: usize = 60;

/// How much of the CLI's last stderr line an exit reason keeps.
const REASON_CHARS: usize = 200;

/// What a session shows as before it has a prompt to name it.
const DEFAULT_TITLE: &str = "Claude";

/// The clock a session reads, swapped out in tests.
type Clock = Box<dyn Fn() -> Timestamp + Send>;

/// A message you sent, until the CLI starts it.
#[derive(Clone, Debug)]
struct Sent {
    /// Whether the CLI has started it.
    started: bool,
}

/// One held Claude Code session and the sub-agents it starts.
pub struct ClaudeSession {
    /// The login the session runs under.
    login: Login,
    /// Where it runs.
    place: Place,
    /// Its items and the state needed to build them.
    convo: Convo,
    /// The time now.
    clock: Clock,
    /// The permission mode you asked for.
    requested: PermissionMode,
    /// The permission mode the CLI reports.
    effective: Option<PermissionMode>,
    /// The CLI's session ID, from its first line that carries one.
    session_id: Option<String>,
    /// The CLI's process ID.
    pid: Option<u32>,
    /// The model the CLI reports.
    model: Option<String>,
    /// A short name, from the first prompt.
    title: Option<String>,
    /// When the driver started.
    started_at: Timestamp,
    /// When anything last happened.
    last_activity_at: Timestamp,
    /// Whether a turn runs.
    busy: bool,
    /// Whether the CLI runs.
    link: AgentLink,
    /// Why the CLI ended, when it ended badly.
    errored: Option<String>,
    /// The lead's last reply, for its context and prompt cache.
    last_turn: LastTurn,
    /// The model's context window, from the last result.
    window: Option<u32>,
    /// Whether `initialize` went out.
    initialized: bool,
    /// Whether the CLI ended.
    exited: bool,
    /// Whether you asked it to stop, so its end isn't an error.
    stopping: bool,
    /// Requests the driver sent that wait for a reply, by request ID.
    ours: HashMap<String, Ours>,
    /// Open prompts by their attention event.
    prompts: HashMap<AttentionId, Prompt>,
    /// Open prompts by the CLI's request ID.
    asked: HashMap<String, AttentionId>,
    /// Requests from the CLI that are answered or withdrawn.
    answered: HashSet<String>,
    /// Requests the holder still waits on, as of the last connect.
    pending: HashSet<String>,
    /// Calls to MaxHQ's own tools that run now, by the CLI's request ID.
    calls: HashMap<String, Running>,
    /// Messages you sent, by UUID.
    sends: HashMap<String, Sent>,
    /// Running sub-agents.
    subs: HashMap<AgentId, Sub>,
    /// Background tasks by the CLI's task ID.
    tasks: HashMap<String, Task>,
    /// Whether the CLI's last snapshot of its background tasks listed any.
    background: bool,
    /// The last line read, by the holder's sequence number.
    last_seq: Option<u64>,
    /// The sequence number of the line that started the running turn.
    turn_from: Option<u64>,
    /// Whether the transcript still needs reading for older items.
    seed: bool,
    /// While the transcript is read, what came meanwhile, in order.
    seeding: Option<Vec<seed::Later>>,
    /// Each agent as last sent.
    shown: HashMap<AgentId, Agent>,
    /// The error item showing an API retry.
    retrying: Option<ItemId>,
    /// The usage limit wait that stays open until the limit resets or a
    /// new turn starts.
    wait: Option<ItemId>,
    /// Messages queued at the limit, which go once it resets.
    held: Vec<Held>,
    /// Sub-agents that ended since the last flush.
    gone: Vec<AgentId>,
}

impl ClaudeSession {
    /// A driver for the session `agent`, run under `login` in `place` with
    /// the permission mode `mode`.
    pub fn new(login: Login, place: Place, agent: AgentId, mode: PermissionMode) -> Self {
        let now = Timestamp::now();
        let convo = fresh_convo(&login, &place, agent);
        Self {
            login,
            place,
            convo,
            clock: Box::new(Timestamp::now),
            requested: mode,
            effective: None,
            session_id: None,
            pid: None,
            model: None,
            title: None,
            started_at: now,
            last_activity_at: now,
            busy: false,
            link: AgentLink::Live,
            errored: None,
            last_turn: LastTurn::default(),
            window: None,
            initialized: false,
            exited: false,
            stopping: false,
            ours: HashMap::new(),
            prompts: HashMap::new(),
            asked: HashMap::new(),
            answered: HashSet::new(),
            pending: HashSet::new(),
            calls: HashMap::new(),
            sends: HashMap::new(),
            subs: HashMap::new(),
            tasks: HashMap::new(),
            background: false,
            last_seq: None,
            turn_from: None,
            seed: true,
            seeding: None,
            shown: HashMap::new(),
            retrying: None,
            wait: None,
            held: Vec::new(),
            gone: Vec::new(),
        }
    }

    /// The same driver reading the time from `clock`.
    pub fn with_clock(mut self, clock: impl Fn() -> Timestamp + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        let now = (self.clock)();
        self.started_at = now;
        self.last_activity_at = now;
        self
    }

    /// The lead agent.
    fn lead(&self) -> AgentId {
        self.convo.lead
    }

    /// An empty conversation for this driver's session.
    fn fresh_convo(&self) -> Convo {
        fresh_convo(&self.login, &self.place, self.lead())
    }

    /// The time now.
    fn now(&self) -> Timestamp {
        (self.clock)()
    }

    /// Notes that something happened at `at`.
    fn touch(&mut self, at: Timestamp) {
        self.last_activity_at = self.last_activity_at.max(at);
    }

    /// Notes that a turn runs, from the line read last if it's the first
    /// to say so.
    fn turn_runs(&mut self) {
        self.busy = true;
        if self.turn_from.is_none() {
            self.turn_from = self.last_seq;
        }
    }

    /// Notes that no turn runs.
    fn turn_over(&mut self) {
        self.busy = false;
        self.turn_from = None;
    }

    /// Reads one line from the CLI.
    fn take(&mut self, line: &Value, raw: &str, at: Timestamp, replayed: bool, fx: &mut Effects) {
        if self.session_id.is_none()
            && let Some(id) = text(line, "session_id")
        {
            self.session_id = Some(id.to_owned());
        }
        if let Some(call) = text(line, "parent_tool_use_id")
            && let Some(sub) = self.convo.subs.get(call)
            && let Some(sub) = self.subs.get_mut(sub)
        {
            sub.last_activity_at = sub.last_activity_at.max(at);
        }
        match text(line, "type") {
            Some("stream_event") => {
                self.clear_retry();
                self.convo.stream_event(line, at);
                self.touch(at);
            }
            Some("assistant") => {
                self.clear_retry();
                self.assistant(line, raw, at);
            }
            Some("user") => self.user(line, at, fx),
            Some("system") => self.system(line, at),
            Some("result") => self.result(line, at),
            Some("command_lifecycle") => self.lifecycle(line, at),
            Some("rate_limit_event") => self.rate_limit(line, at, fx),
            Some("control_request") => self.request(line, at, replayed, fx),
            Some("control_response") => self.reply(line, at, fx),
            Some("control_cancel_request") => self.withdrawn(line, at, fx),
            Some("auth_status") => self.auth_status(line, at),
            // Keep-alives, tool progress and anything newer carry nothing
            // the timeline shows.
            _ => {}
        }
    }

    /// Reads an `assistant` line, and the lead's context from it.
    fn assistant(&mut self, line: &Value, raw: &str, at: Timestamp) {
        self.convo.assistant(line, at);
        self.touch(at);
        if text(line, "error") == Some("rate_limit")
            && text(line, "parent_tool_use_id").is_none()
            && let Some(uuid) = text(line, "uuid")
        {
            self.open_wait(convo::item_id(uuid), at);
        }
        if text(line, "parent_tool_use_id").is_none() && line.get("error").is_none() {
            self.last_turn.read_line(raw.as_bytes(), true);
            self.last_turn.ended_at = Some(at);
            if answered(line) {
                self.convo.resets_at = None;
                self.close_wait(at);
            }
        }
        if let Some(model) = line
            .get("message")
            .and_then(|message| text(message, "model"))
            .filter(|model| !model.starts_with('<'))
            && text(line, "parent_tool_use_id").is_none()
        {
            self.model = Some(model.to_owned());
        }
    }

    /// Reads a `user` line: tool results, a sub-agent's prompt, or the
    /// marker of a turn cut short.
    fn user(&mut self, line: &Value, at: Timestamp, fx: &mut Effects) {
        self.touch(at);
        for ended in self.convo.user(line, at, true) {
            self.tool_ended(&ended, at, fx);
        }
    }

    /// Acts on a tool call whose result arrived: a prompt for it is over,
    /// and a sub-agent that ran in the foreground is done.
    fn tool_ended(&mut self, ended: &Ended, at: Timestamp, fx: &mut Effects) {
        if let Some(allowed) = ended.allowed {
            self.decided_elsewhere(&ended.tool_use_id, allowed, at, fx);
        }
        if tools::is_subagent(&ended.name)
            && !ended.background
            && let Some(sub) = self.convo.subs.get(&ended.tool_use_id).copied()
        {
            self.sub_done(sub);
        }
    }

    /// Reads a `system` line. Only what the agent does is activity: a CLI
    /// that starts again sends its hooks and status too.
    fn system(&mut self, line: &Value, at: Timestamp) {
        match text(line, "subtype") {
            Some("init") => self.init(line, at),
            Some("status") => {
                if let Some(mode) = text(line, "permissionMode").and_then(wire::mode_from) {
                    self.effective = Some(mode);
                }
                return;
            }
            Some("session_state_changed") => match text(line, "state") {
                Some("running" | "requires_action") => self.turn_runs(),
                Some("idle") => self.turn_over(),
                _ => return,
            },
            Some("compact_boundary") => self.convo.compaction(line, at),
            Some("api_retry") => self.api_retry(line, at),
            Some("permission_denied") => self.permission_denied(line, at),
            Some(kind) if kind.starts_with("task_") => self.task(kind, line, at),
            // The background task snapshot isn't shown, but it says
            // whether the CLI may stop while idle.
            Some("background_tasks_changed") => {
                self.background = line
                    .get("tasks")
                    .and_then(Value::as_array)
                    .is_some_and(|tasks| !tasks.is_empty());
                return;
            }
            // Hooks and thinking counts aren't shown.
            _ => return,
        }
        self.touch(at);
    }

    /// Reads the `system/init` line that starts each turn.
    fn init(&mut self, line: &Value, at: Timestamp) {
        if let Some(model) = text(line, "model") {
            self.model = Some(model.to_owned());
        }
        if let Some(mode) = text(line, "permissionMode").and_then(wire::mode_from) {
            self.effective = Some(mode);
        }
        if self.convo.turn.is_none() {
            // A turn the CLI starts itself, such as when a background task
            // ends, has no prompt of its own.
            let prompt = line
                .get("user_message_uuids")
                .and_then(Value::as_array)
                .and_then(|uuids| uuids.first())
                .and_then(Value::as_str)
                .or_else(|| text(line, "uuid"));
            if let Some(prompt) = prompt {
                self.convo.turn = Some(convo::turn_id(prompt));
            }
        }
        // A turn the CLI starts for a task that ended may make no API call,
        // so only a reply from the API ends a limit wait.
        self.turn_runs();
        self.errored = None;
        self.touch(at);
    }

    /// Shows an API retry as one error that updates in place, until the
    /// retry works.
    fn api_retry(&mut self, line: &Value, at: Timestamp) {
        let Some(uuid) = text(line, "uuid") else {
            return;
        };
        let what = match number(line, "error_status") {
            Some(529) => "The API is overloaded",
            Some(429) => "The API is rate limiting requests",
            Some(500..=599) => "The API had a server error",
            _ => "The API request failed",
        };
        let seconds = number(line, "retry_delay_ms").map(|ms| ms.div_ceil(1000));
        let message = match (
            seconds,
            number(line, "attempt"),
            number(line, "max_retries"),
        ) {
            (Some(seconds), Some(attempt), Some(max)) => {
                format!("{what}. Claude retries in {seconds} s, attempt {attempt} of {max}.")
            }
            (Some(seconds), _, _) => format!("{what}. Claude retries in {seconds} s."),
            _ => format!("{what}. Claude retries soon."),
        };
        if let Some(id) = &self.retrying {
            let body = ItemBody::Error {
                message,
                retrying: true,
                sign_in: None,
            };
            self.convo.book.update(id, |item| item.body = body);
            return;
        }
        let owner = self.convo.owner_of(line);
        let mut item = self.convo.item(
            convo::item_id(uuid),
            &owner,
            at,
            ItemBody::Error {
                message,
                retrying: true,
                sign_in: None,
            },
        );
        item.status = ItemStatus::Running;
        item.completed_at = None;
        self.retrying = Some(item.id.clone());
        self.convo.book.put(item);
    }

    /// Drops the retry notice once the API answers.
    fn clear_retry(&mut self) {
        if let Some(id) = self.retrying.take() {
            self.convo.book.remove(&id);
        }
    }

    /// Notes a call that a rule or the auto mode classifier refused.
    fn permission_denied(&mut self, line: &Value, at: Timestamp) {
        let Some(uuid) = text(line, "uuid") else {
            return;
        };
        let tool = text(line, "tool_name").unwrap_or("a tool call");
        let message = match text(line, "decision_reason").or_else(|| text(line, "reason")) {
            Some(reason) => format!("Blocked {tool}: {}", clip(reason, 200)),
            None => format!("Blocked {tool} by a permission rule."),
        };
        self.notice(uuid, line, at, NoticeLevel::Warning, message);
    }

    /// Notes login trouble the CLI reports. The account's name stays out.
    fn auth_status(&mut self, line: &Value, at: Timestamp) {
        let Some(error) = text(line, "error") else {
            return;
        };
        let Some(uuid) = text(line, "uuid") else {
            return;
        };
        let message = format!("Claude Code can't sign in: {}", clip(error, 200));
        self.notice(uuid, line, at, NoticeLevel::Warning, message);
    }

    /// Puts a notice for the line with this UUID.
    fn notice(
        &mut self,
        uuid: &str,
        line: &Value,
        at: Timestamp,
        level: NoticeLevel,
        message: String,
    ) {
        let owner = self.convo.owner_of(line);
        let item = self.convo.item(
            convo::item_id(uuid),
            &owner,
            at,
            ItemBody::Notice { level, message },
        );
        self.convo.book.put(item);
    }

    /// Reads the `result` line that ends each turn.
    fn result(&mut self, line: &Value, at: Timestamp) {
        let Some(uuid) = text(line, "uuid") else {
            return;
        };
        self.window = line
            .get("modelUsage")
            .and_then(Value::as_object)
            .and_then(|models| {
                models
                    .values()
                    .filter_map(|usage| number(usage, "contextWindow"))
                    .max()
            })
            .and_then(|window| u32::try_from(window).ok())
            .or(self.window);
        let lead = self.convo.owner(None);
        // Text still streaming lost its finished line, so it ends here.
        let streaming: Vec<ItemId> = self
            .convo
            .book
            .items()
            .filter(|item| item.agent == lead.agent && is_streaming(&item.body))
            .map(|item| item.id.clone())
            .collect();
        for id in streaming {
            self.convo.book.update(&id, |item| {
                convo::stop_streaming(&mut item.body);
                item.status = ItemStatus::Completed;
                item.completed_at = Some(at);
            });
        }
        if let Some(message) = failure(line) {
            let limit = self.limited().is_some() || number(line, "api_error_status") == Some(429);
            let id = ItemId(format!("claude:{uuid}:error"));
            let body = if limit {
                ItemBody::LimitWait {
                    account: self.convo.account,
                    resets_at: self.limited(),
                    auto_resume: false,
                }
            } else {
                ItemBody::Error {
                    message,
                    retrying: false,
                    sign_in: self.convo.account.filter(|_| signed_out(line)),
                }
            };
            let waits = self.convo.book.items().any(|item| {
                item.turn == self.convo.turn && matches!(item.body, ItemBody::LimitWait { .. })
            });
            if !(limit && waits) {
                let item = self.convo.item(id.clone(), &lead, at, body);
                self.convo.book.put(item);
                if limit {
                    self.open_wait(id, at);
                }
            }
        }
        let body = ItemBody::TurnSummary {
            duration_ms: number(line, "duration_ms"),
            context_tokens: self.context().map(|context| u64::from(context.used_tokens)),
        };
        let item = self.convo.item(convo::item_id(uuid), &lead, at, body);
        self.convo.book.put(item);
        self.clear_retry();
        self.convo.end_turn();
        self.turn_over();
        self.touch(at);
    }

    /// Reads a `command_lifecycle` line, which tracks each message you sent
    /// through the CLI's queue.
    fn lifecycle(&mut self, line: &Value, at: Timestamp) {
        let Some(uuid) = text(line, "command_uuid") else {
            return;
        };
        let id = convo::item_id(uuid);
        match text(line, "state") {
            Some("started") => {
                let turn = convo::turn_id(uuid);
                self.convo.turn = Some(turn.clone());
                self.turn_runs();
                if let Some(sent) = self.sends.get_mut(uuid) {
                    sent.started = true;
                }
                self.convo.book.update(&id, |item| {
                    item.turn = Some(turn);
                    item.status = ItemStatus::Completed;
                    item.started_at = Some(at);
                    item.completed_at = Some(at);
                    if let ItemBody::UserMessage { delivery, .. } = &mut item.body
                        && *delivery == Delivery::Queued
                    {
                        *delivery = Delivery::Sent;
                    }
                });
                if self.title.is_none()
                    && let Some(ItemBody::UserMessage { text, .. }) =
                        self.convo.book.get(&id).map(|item| &item.body)
                    && !text.trim().is_empty()
                {
                    self.title = Some(clip(text.trim(), TITLE_CHARS));
                }
            }
            // A message the CLI already started stays sent, even when its
            // turn is cut short. The guard forgets the send either way.
            Some("cancelled") if self.sends.remove(uuid).is_some_and(|sent| !sent.started) => {
                self.convo.book.update(&id, |item| {
                    item.status = ItemStatus::Cancelled;
                    item.completed_at = Some(at);
                    if let ItemBody::UserMessage { delivery, .. } = &mut item.body {
                        *delivery = Delivery::Cancelled;
                    }
                });
            }
            Some("completed") => {
                self.sends.remove(uuid);
            }
            _ => {}
        }
        self.touch(at);
    }

    /// Reads a `rate_limit_event` into the account's usage.
    fn rate_limit(&mut self, line: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(info) = line.get("rate_limit_info") else {
            return;
        };
        if let Some(usage) = limits::usage(info, at) {
            fx.usage.push((self.login.account, usage));
        }
        match limits::rejected(info) {
            Some(resets_at) => {
                self.convo.resets_at = Some(resets_at);
                self.wait_until(resets_at);
            }
            None => self.convo.resets_at = None,
        }
    }

    /// Reads one line the dedupe let through. The first that names the
    /// session starts the read of its transcript, so a driver that meets a
    /// running or resumed session knows its older items, and the line
    /// waits for it.
    fn read(&mut self, line: &HeldLine, fx: &mut Effects) {
        let Ok(value) = serde_json::from_str::<Value>(&line.text) else {
            tracing::debug!(seq = line.seq, "skipping a Claude line that isn't JSON");
            self.after(fx);
            return;
        };
        if self.seed
            && let Some(session) = text(&value, "session_id")
        {
            self.seed = false;
            self.start_seed(session, line, fx);
            return;
        }
        self.convo.book.set_quiet(line.replayed);
        self.take(&value, &line.text, line.at, line.replayed, fx);
        self.convo.book.set_quiet(false);
        self.after(fx);
    }

    /// Puts each open approval under its tool call, in its turn. An approval the
    /// holder kept across a restart opens before the transcript is read,
    /// when its tool call isn't known yet.
    fn join_tool_turns(&mut self) {
        let pairs: Vec<(ItemId, ItemId)> = self
            .prompts
            .values()
            .filter_map(|prompt| Some((prompt.item.clone(), prompt.tool_item.clone()?)))
            .collect();
        for (approval, tool) in pairs {
            let Some(call) = self.convo.book.get(&tool) else {
                continue;
            };
            let (agent, turn) = (call.agent, call.turn.clone());
            self.convo.book.update(&approval, |item| {
                if item.agent == agent && item.turn.is_none() {
                    item.turn = turn;
                    item.parent = Some(tool);
                }
            });
        }
    }

    /// Marks tool calls and questions as waiting while a prompt for them
    /// is open.
    fn sync_waiting(&mut self) {
        let waiting: Vec<ItemId> = self
            .prompts
            .values()
            .filter_map(|prompt| match prompt.kind {
                Kind::Questions => Some(prompt.item.clone()),
                Kind::Tool | Kind::Plan => prompt.tool_item.clone(),
            })
            .collect();
        for id in waiting {
            self.convo.book.update(&id, |item| {
                if item.status == ItemStatus::Running {
                    item.status = ItemStatus::Waiting;
                }
            });
        }
    }

    /// Sends what changed: timeline events, then agents, then the next
    /// time to wake.
    fn after(&mut self, fx: &mut Effects) {
        self.resume(fx);
        self.sync_waiting();
        fx.timeline.extend(self.convo.book.flush());
        self.convo.forget_settled();
        fx.pictures.append(&mut self.convo.kept);
        fx.gone.append(&mut self.gone);
        self.settle(fx);
    }

    /// The lead's context now.
    fn context(&self) -> Option<ContextUsage> {
        let context = self.last_turn.context?;
        Some(ContextUsage {
            used_tokens: context.used_tokens,
            window_tokens: self.window.unwrap_or(context.window_tokens),
        })
    }

    /// The oldest open prompt of `agent`.
    fn waiting_on(&self, agent: AgentId) -> Option<AttentionId> {
        self.prompts
            .iter()
            .filter(|(_, prompt)| prompt.event.agent == agent)
            .min_by_key(|(id, prompt)| (prompt.event.created_at, **id))
            .map(|(id, _)| *id)
    }

    /// When the account's usage limit resets, while it applies: from the
    /// CLI's last limit event, else from the open wait.
    fn limited(&self) -> Option<Timestamp> {
        self.convo
            .resets_at
            .or_else(|| self.wait_resets())
            .filter(|at| *at > self.now())
    }

    /// When the prompt cache from the last turn goes cold, while it's warm.
    fn warm_until(&self) -> Option<Timestamp> {
        self.last_turn.warm_until().filter(|at| *at > self.now())
    }

    /// Whether a turn runs, or a message went to the CLI that it hasn't
    /// started yet. The CLI can report itself idle in between, as the reply
    /// to `initialize` does when your first message follows it.
    fn working(&self) -> bool {
        self.busy || self.sends.values().any(|sent| !sent.started)
    }

    /// The lead agent as it stands.
    fn lead_agent(&self) -> Agent {
        let activity = if let Some(attention) = self.waiting_on(self.lead()) {
            Activity::NeedsInput { attention }
        } else if let (Some(resets_at), false) = (self.limited(), self.working()) {
            Activity::WaitingForLimitReset {
                resets_at: Some(resets_at),
            }
        } else if let Some(reason) = &self.errored {
            Activity::Error {
                reason: reason.clone(),
            }
        } else if self.last_seq.is_none() && !self.exited {
            Activity::Starting
        } else if !self.subs.is_empty() {
            Activity::BusyInSubagents
        } else if self.working() {
            Activity::Busy
        } else if let (Some(warm_until), false) = (self.warm_until(), self.exited) {
            Activity::IdleCacheWarm { warm_until }
        } else {
            Activity::Idle
        };
        Agent {
            id: self.lead(),
            workspace: self.place.workspace,
            parent: None,
            vendor: Vendor::Claude,
            account: Some(self.login.account),
            model: self.model.clone(),
            permission: Some(AgentPermission {
                requested: self.requested,
                effective: self.effective,
            }),
            session: None,
            vendor_session_id: self.session_id.clone(),
            control: Control::Held,
            pid: self.pid,
            link: self.link.clone(),
            title: self.title.clone().unwrap_or_else(|| self.default_title()),
            activity,
            context: self.context(),
            started_at: self.started_at,
            last_activity_at: self.last_activity_at,
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: self.waits_on().map(|waiting| AgentRunning {
                waiting: Some(waiting),
                ..AgentRunning::default()
            }),
            moved: None,
            local: None,
        }
    }

    /// The name of the working folder, for a session with no prompt yet.
    fn default_title(&self) -> String {
        self.place
            .cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| DEFAULT_TITLE.to_owned())
    }

    /// Sends every agent that changed and the next time to wake.
    fn settle(&mut self, fx: &mut Effects) {
        let mut agents = vec![self.lead_agent()];
        let mut subs: Vec<&Sub> = self.subs.values().collect();
        subs.sort_by_key(|sub| (sub.started_at, sub.agent));
        agents.extend(subs.into_iter().map(|sub| self.sub_agent(sub)));
        for agent in agents {
            if self.shown.get(&agent.id) != Some(&agent) {
                self.shown.insert(agent.id, agent.clone());
                fx.agents.push(agent);
            }
        }
        let now = self.now();
        let next = [
            self.limited(),
            (!self.working()).then(|| self.warm_until()).flatten(),
        ]
        .into_iter()
        .flatten()
        .min();
        fx.wake_at =
            next.map(|at| Instant::now() + Duration::from_millis(at.0.saturating_sub(now.0)));
    }

    /// Ends what was running when the CLI ended.
    fn shut(&mut self, at: Timestamp, fx: &mut Effects) {
        self.drop_prompts(at, fx);
        self.drop_ours(fx);
        for (_, task) in std::mem::take(&mut self.tasks) {
            self.drop_progress(&task);
        }
        // Held messages and the open wait close below, with the rest.
        self.held.clear();
        self.wait = None;
        let running: Vec<ItemId> = self
            .convo
            .book
            .items()
            .filter(|item| {
                matches!(
                    item.status,
                    ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
                )
            })
            .map(|item| item.id.clone())
            .collect();
        for id in running {
            self.convo.book.update(&id, |item| {
                convo::stop_streaming(&mut item.body);
                if let ItemBody::UserMessage { delivery, .. } = &mut item.body {
                    *delivery = Delivery::Cancelled;
                    item.status = ItemStatus::Cancelled;
                } else {
                    item.status = ItemStatus::Interrupted;
                }
                item.completed_at = Some(at);
            });
        }
        self.clear_retry();
        self.sends.clear();
        let subs: Vec<AgentId> = self.subs.keys().copied().collect();
        for sub in subs {
            self.sub_done(sub);
        }
        self.convo.end_turn();
        self.turn_over();
    }
}

impl Driver for ClaudeSession {
    fn connected(&mut self, fresh: bool, pending: &[HeldLine], fx: &mut Effects) {
        if self.wait_for_seed(|| seed::Later::Connected(fresh, pending.to_vec())) {
            return;
        }
        if fresh && !self.initialized {
            self.initialize(true, fx);
        } else {
            // A reconnect asks again only to read back the CLI's state and
            // the prompts it waits on.
            self.initialize(false, fx);
        }
        if !fresh {
            // The CLI ran on while `maxd` was away, and read its tools
            // from a `maxd` that may have had fewer.
            self.tools_changed(fx);
        }
        self.pending.clear();
        let lines: Vec<(Value, Timestamp)> = pending
            .iter()
            .filter_map(|line| Some((serde_json::from_str::<Value>(&line.text).ok()?, line.at)))
            .collect();
        for (line, _) in &lines {
            if let Some(id) = text(line, "request_id") {
                self.pending.insert(id.to_owned());
                // The holder holds no answer for it, so any answer sent
                // before never arrived.
                self.answered.remove(id);
            }
        }
        for (line, at) in &lines {
            self.request(line, *at, false, fx);
        }
        self.after(fx);
    }

    fn line(&mut self, line: &HeldLine, fx: &mut Effects) {
        if self.last_seq.is_some_and(|seen| line.seq <= seen) {
            return;
        }
        self.last_seq = Some(line.seq);
        if !self.wait_for_seed(|| seed::Later::Line(line.clone())) {
            self.read(line, fx);
        }
    }

    fn act(&mut self, action: Action, fx: &mut Effects) {
        if let Some(waiting) = &mut self.seeding {
            waiting.push(seed::Later::Act(action));
            return;
        }
        self.action(action, fx);
        self.after(fx);
    }

    fn exited(&mut self, exit: &ChildExit, fx: &mut Effects) {
        if self.wait_for_seed(|| seed::Later::Exited(exit.clone())) {
            return;
        }
        let reason = exit_reason(exit);
        let failed = exit.code != Some(0) && !self.stopping && !exit.with_machine;
        // A CLI that stops while it waits for you cuts nothing off, so its
        // exit isn't activity.
        let news = failed
            || self.working()
            || !self.prompts.is_empty()
            || !self.subs.is_empty()
            || !self.tasks.is_empty();
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
        self.after(fx);
    }

    fn tick(&mut self, _now: Instant, fx: &mut Effects) {
        // The read of the transcript ends with an update of its own.
        if self.seeding.is_none() {
            self.after(fx);
        }
    }

    fn tool_done(&mut self, done: ToolDone, fx: &mut Effects) {
        if let Some(waiting) = &mut self.seeding {
            waiting.push(seed::Later::ToolDone(done));
            return;
        }
        self.call_done(done, fx);
        self.after(fx);
    }

    fn met_again(&mut self, known: &Agent) {
        if known.id == self.lead() {
            self.started_at = known.started_at;
            self.last_activity_at = known.last_activity_at;
        }
    }

    fn job_done(&mut self, done: Option<JobDone>, fx: &mut Effects) {
        self.seed_done(done, fx);
    }

    fn checkpoint(&self) -> Option<u64> {
        // Lines that wait for the transcript's read have to be read again
        // after a restart.
        self.turn_from.into_iter().chain(self.waiting_from()).min()
    }

    fn can_release(&self, _agent: AgentId) -> bool {
        self.releasable()
    }

    fn keep_items(&mut self, items: usize) {
        self.convo.keep = items;
    }
}

/// An empty conversation for `agent`'s session, run under `login` in
/// `place`.
fn fresh_convo(login: &Login, place: &Place, agent: AgentId) -> Convo {
    let mut convo = Convo::new(agent);
    convo.account = Some(login.account);
    convo.cwd.clone_from(&place.cwd);
    convo.pictures = crate::drive::pictures::Pictures::at(place);
    convo
}

/// What went wrong in a turn, from its `result` line. A turn cut short by
/// an interrupt or a steer isn't an error. One OpenRouter turned down over
/// money says so in plain words.
fn failure(line: &Value) -> Option<String> {
    if let Some(reason) = text(line, "startup_failure_reason") {
        return Some(format!("Claude Code couldn't start: {}", clip(reason, 200)));
    }
    if !wire::flag(line, "is_error") {
        return None;
    }
    if text(line, "terminal_reason").is_some_and(|reason| reason.starts_with("aborted")) {
        return None;
    }
    let said = text(line, "result")
        .map(str::to_owned)
        .or_else(|| {
            line.get("errors")
                .and_then(Value::as_array)
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|error| !error.starts_with("[ede_diagnostic]"))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|errors| !errors.is_empty())
        })
        .unwrap_or_else(|| match text(line, "subtype") {
            Some("error_max_turns") => "The turn hit its limit on steps.".to_owned(),
            Some("error_max_budget_usd") => "The turn hit its spending limit.".to_owned(),
            _ => "The turn failed.".to_owned(),
        });
    Some(crate::local_models::turned_down(&said).map_or(said, str::to_owned))
}

/// Whether an `assistant` line is a reply from the API. The CLI writes its
/// own notes, such as the one for a call the usage limit turned away, as
/// the `<synthetic>` model.
fn answered(line: &Value) -> bool {
    line.get("message")
        .and_then(|message| text(message, "model"))
        .is_some_and(|model| !model.starts_with('<'))
}

/// Whether a failed turn's `result` line says the login ran out or there
/// is none, so signing in again fixes it.
fn signed_out(line: &Value) -> bool {
    if let Some(reason) = text(line, "startup_failure_reason") {
        let reason = reason.to_lowercase();
        return reason.contains("login") || reason.contains("logged in");
    }
    number(line, "api_error_status") == Some(401)
}

/// Whether text is still streaming into an item.
fn is_streaming(body: &ItemBody) -> bool {
    matches!(
        body,
        ItemBody::AssistantMessage {
            streaming: true,
            ..
        } | ItemBody::Reasoning {
            streaming: true,
            ..
        }
    )
}

/// Why the CLI ended, with the last line it printed to stderr.
fn exit_reason(exit: &ChildExit) -> String {
    let how = match (exit.code, exit.signal) {
        (Some(0), _) => "Claude exited".to_owned(),
        (Some(code), _) => format!("Claude exited with code {code}"),
        (None, Some(signal)) => format!("Claude was stopped by signal {signal}"),
        (None, None) => "Claude exited".to_owned(),
    };
    let last = exit
        .stderr_tail
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty());
    match (exit.code, last) {
        (Some(0), _) | (_, None) => how,
        (_, Some(line)) => format!("{how}: {}", clip(line, REASON_CHARS)),
    }
}
