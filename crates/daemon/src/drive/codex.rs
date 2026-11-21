// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The driver for one account's held `codex app-server` and every thread
//! on it.
//!
//! Each account gets one private app-server, started by [`launch`] and
//! spoken to over JSON-RPC on stdio. Every Codex agent on the account is a
//! thread on it, and a thread a Codex agent spawns is a sub-agent. The
//! driver keeps each thread's turn, the items still streaming, the
//! messages waiting to go out and the prompts waiting for you, and turns
//! what the app-server says into timeline events, agent state and
//! attention.
//!
//! Requests the driver sends carry string IDs made of a per-run nonce and
//! a count, so a reply meant for an earlier `maxd` is never taken for a new
//! one. The app-server's own requests carry its integer IDs, which the
//! holder keys as their digits.

mod actions;
mod channel;
mod events;
mod history;
mod input;
mod items;
mod kept;
mod locals;
mod policy;
mod release;
pub(crate) mod requests;
pub(in crate::drive) mod rpc;
#[cfg(test)]
mod tests;
mod thread;

// The session manager reads history through this.
#[allow(unused_imports)]
pub use history::History;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use maxhq_core::{context, names};
use maxhq_protocol::attention::AttentionEvent;
use maxhq_protocol::ids::{AgentId, AttentionId, DeviceId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    Activity, Agent, AgentLink, AgentPermission, ContextUsage, Control, ModelChoice, Usage, Vendor,
};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ItemBody, ItemId, ItemStatus, TextField, TimelineEvent, TimelineItem,
};
use serde_json::{json, Value};

use crate::drive::{
    Action, ActionKey, AttentionChange, ChildExit, Driver, Effects, Framing, HeldLine, Launch,
    Login, LoginAuth, ToolDone, Write,
};
use crate::observe::codex::rollout::CACHE_WARM;
use crate::recipe;
use crate::usage::codex::account::find_codex;

use actions::Call;
use channel::Running;
use kept::Kept;
use locals::Locals;
use requests::Prompt;
use rpc::{Ids, Incoming};
use thread::{Thread, Turn};

/// How often rate limits are read when nothing reports them.
const LIMITS_EVERY: Duration = Duration::from_secs(5 * 60);

/// How long to wait before each retry of an interrupt that found no turn.
const RETRY_MS: [u64; 5] = [25, 50, 100, 200, 400];

/// The most threads whose lines wait while the driver asks what they are.
const STRAY_THREADS: usize = 32;

/// The most lines kept for each of them.
const STRAY_LINES: usize = 2000;

/// How deep a chain of sub-agents is followed to find where it runs.
const MAX_DEPTH: usize = 16;

/// The longest part of Codex's stderr an exit reason quotes.
const REASON_CHARS: usize = 200;

/// The model provider a ChatGPT sign-in runs on. Codex keeps it in each
/// thread's rollout and resumes a thread only on the same one, so it never
/// changes.
const PROVIDER: &str = "maxhq";

/// The [`PROVIDER`]'s settings: the Responses API on the plan, with each access
/// token from `maxd guest token codex`. Codex waits up to 80 seconds for
/// the command, past the guest's and the host's own waits, and asks again
/// whenever the API refuses the token.
///
/// Codex's own request to the catalog address fails inside Codex, and a
/// provider with an address of its own has no built-in list to fall back
/// on. So `maxd guest codex` fetches the catalog before Codex starts and
/// hands it over as a `model_catalog_json` file, which Codex loads instead
/// of asking any address. The address stays as it was, for a start with
/// no file yet.
fn provider_config() -> String {
    format!(
        concat!(
            "model_providers.maxhq={{",
            r#"name="MaxHQ","#,
            r#"base_url="https://api.openai.com/v1","#,
            r#"model_catalog_url="https://api.openai.com/v1/models","#,
            r#"wire_api="responses","#,
            "requires_openai_auth=false,",
            "supports_websockets=false,",
            r#"auth={{command="{maxd}",args=["guest","token","codex"],"#,
            "timeout_ms=80000,refresh_interval_ms=0}}}}",
        ),
        maxd = recipe::MAXD,
    )
}

/// The arguments before Codex's own on a ChatGPT sign-in, for
/// [`recipe::MAXD`], which fetches the model catalog and then runs Codex.
const CATALOG_HELPER: [&str; 2] = ["guest", "codex"];

/// What could point Codex at another key or server than the provider's.
pub const OTHER_AUTH: [&str; 5] = [
    "ACCESS_TOKEN",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
];

/// How to run the app-server for `login`: `codex app-server` with the
/// login's `CODEX_HOME`, unloading idle threads after a minute. It never
/// listens on a socket and never joins a shared daemon. On a ChatGPT
/// sign-in it runs on the [`PROVIDER`], and no other key reaches it, and
/// it starts through `maxd guest codex` so it gets its model catalog.
pub fn launch(login: &Login) -> Launch {
    let mut program = find_codex().unwrap_or_else(|| PathBuf::from("codex"));
    let mut args: Vec<OsString> = ["app-server", "-c", "thread_unload_delay_secs=60"]
        .into_iter()
        .map(OsString::from)
        .collect();
    let mut env_remove = Vec::new();
    if login.auth == LoginAuth::ChatGptBroker {
        let provider = format!("model_provider=\"{PROVIDER}\"");
        args.extend([
            "-c".into(),
            provider.into(),
            "-c".into(),
            provider_config().into(),
        ]);
        env_remove.extend(OTHER_AUTH.map(OsString::from));
        let helper = CATALOG_HELPER.map(OsString::from);
        let codex = std::mem::replace(&mut program, PathBuf::from(recipe::MAXD));
        args.splice(0..0, helper.into_iter().chain([codex.into_os_string()]));
    }
    Launch {
        program,
        args,
        env: vec![(
            OsString::from("CODEX_HOME"),
            login.config_dir.clone().into_os_string(),
        )],
        env_remove,
        cwd: login.config_dir.clone(),
        framing: Framing::JsonRpc,
        secret_env: Vec::new(),
        run_as: None,
    }
}

/// How to run `codex login` under the `CODEX_HOME` a [`launch`] of
/// `login` gets, so the login lands in its folder. `device_code` asks for
/// a code you type at OpenAI's page instead of a sign-in that comes back
/// to this device's localhost.
pub fn login_launch(login: &Login, device_code: bool) -> Launch {
    let mut args = vec![OsString::from("login")];
    if device_code {
        args.push("--device-auth".into());
    }
    Launch {
        program: find_codex().unwrap_or_else(|| PathBuf::from("codex")),
        args,
        ..launch(login)
    }
}

/// Points `launch` from [`launch`] at Codex's `program`, which on a
/// ChatGPT sign-in is the argument after `maxd guest codex`.
pub fn set_program(launch: &mut Launch, program: PathBuf) {
    let helped = launch.program == Path::new(recipe::MAXD)
        && launch.args.len() > CATALOG_HELPER.len()
        && launch
            .args
            .iter()
            .zip(CATALOG_HELPER)
            .all(|(arg, want)| arg == want);
    if helped {
        launch.args[CATALOG_HELPER.len()] = program.into_os_string();
    } else {
        launch.program = program;
    }
}

/// When and how a line came in.
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// When the holder read it.
    at: Timestamp,
    /// The holder's seq for it.
    seq: u64,
    /// Whether the holder replayed it from before this connection.
    replayed: bool,
}

/// Where the connection to the app-server stands.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Not connected, or the app-server exited.
    Down,
    /// `initialize` went out and hasn't come back.
    Handshake,
    /// Ready for threads.
    Ready,
    /// The app-server can't be used, for the reason given.
    Refused(String),
}

/// Something the driver waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Timer {
    /// Try an interrupt again.
    Retry {
        /// The thread.
        thread: String,
        /// The turn to interrupt.
        turn: String,
        /// How many tries came before.
        attempt: usize,
    },
    /// A thread's cache goes cold.
    Cold {
        /// The thread.
        thread: String,
    },
    /// Read rate limits again.
    Limits,
    /// A question's time runs out.
    Expire {
        /// The question's event.
        attention: AttentionId,
    },
}

/// The requests the driver sent that haven't come back.
#[derive(Debug)]
struct Rpc {
    ids: Ids,
    calls: HashMap<String, Call>,
}

impl Rpc {
    /// Writes a request and remembers what it was for.
    fn send(&mut self, method: &str, params: Value, call: Call, fx: &mut Effects) -> String {
        let id = self.ids.next();
        fx.writes.push(Write {
            line: rpc::request(&id, method, params),
            answers: None,
        });
        self.calls.insert(id.clone(), call);
        id
    }

    /// Whether a request matching `which` is still out.
    fn waiting(&self, which: impl Fn(&Call) -> bool) -> bool {
        self.calls.values().any(which)
    }
}

/// The driver for one account's app-server and every thread on it.
#[derive(Debug)]
pub struct CodexServer {
    /// The account it runs for.
    login: Login,
    /// The device it runs on.
    device: DeviceId,
    /// Requests out.
    rpc: Rpc,
    /// Where the connection stands.
    phase: Phase,
    /// Actions that wait for the handshake.
    held: Vec<Action>,
    /// Every thread, by Codex's ID.
    threads: HashMap<String, Thread>,
    /// Each agent's thread.
    agents: HashMap<AgentId, String>,
    /// Requests from Codex waiting for an answer.
    prompts: HashMap<AttentionId, Prompt>,
    /// The holder's keys for requests it still holds unanswered.
    pending: HashSet<String>,
    /// Calls to MaxHQ's own tools that run now, by Codex's call ID.
    tool_calls: HashMap<String, Running>,
    /// The items of files sent with MaxHQ's tools that show already.
    sends_shown: HashSet<String>,
    /// Lines for threads the driver doesn't know yet.
    strays: HashMap<String, Kept>,
    /// Models read so far from a paged list.
    models: Vec<ModelChoice>,
    /// The whole model list as last read, for each model's default effort.
    catalog: Vec<ModelChoice>,
    /// The account's rate limits as last shown.
    usage: Option<Usage>,
    /// Timers, in no order.
    timers: Vec<(Instant, Timer)>,
    /// Each agent as last sent, so only changes go out.
    shown: HashMap<AgentId, Agent>,
    /// The start and last activity of agents an earlier driver drove, as
    /// `maxd` stored them, for when their threads come back.
    met: HashMap<AgentId, (Timestamp, Timestamp)>,
    /// The holder's seq for the line being taken.
    seq: u64,
    /// The local items of messages sent from here, shared with the
    /// drivers and history readers that come after it.
    locals: Locals,
}

impl CodexServer {
    /// A driver for `login`'s app-server on `device`, waiting for the
    /// holder to connect.
    pub fn new(login: Login, device: DeviceId) -> Self {
        Self::with_ids(login, device, Ids::new(), Locals::shared())
    }

    /// A driver whose request IDs come from `ids`, noting local items in
    /// `locals`.
    fn with_ids(login: Login, device: DeviceId, ids: Ids, locals: Locals) -> Self {
        Self {
            login,
            device,
            rpc: Rpc {
                ids,
                calls: HashMap::new(),
            },
            phase: Phase::Down,
            held: Vec::new(),
            threads: HashMap::new(),
            agents: HashMap::new(),
            prompts: HashMap::new(),
            pending: HashSet::new(),
            tool_calls: HashMap::new(),
            sends_shown: HashSet::new(),
            strays: HashMap::new(),
            models: Vec::new(),
            catalog: Vec::new(),
            usage: None,
            timers: Vec::new(),
            shown: HashMap::new(),
            met: HashMap::new(),
            seq: 0,
            locals,
        }
    }

    /// Sends `initialize`.
    fn handshake(&mut self, fx: &mut Effects) {
        self.phase = Phase::Handshake;
        let params = json!({
            "clientInfo": {
                "name": "maxhq",
                "title": "MaxHQ",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": { "experimentalApi": true },
        });
        self.rpc.send("initialize", params, Call::Initialize, fx);
    }

    /// The handshake is done: read models and limits, bring threads back
    /// and run what waited.
    fn ready(&mut self, fx: &mut Effects) {
        self.phase = Phase::Ready;
        self.models.clear();
        self.rpc.send("model/list", json!({}), Call::Models, fx);
        self.read_limits(fx);
        self.arm_limits(Instant::now());
        for thread in self.threads.values_mut() {
            if !thread.stopped {
                thread.link = AgentLink::Live;
            }
        }
        let mut strays: Vec<String> = self.strays.keys().cloned().collect();
        strays.sort();
        for thread in strays {
            self.ask_about(&thread, fx);
        }
        let mut waiting: Vec<String> = self
            .threads
            .values()
            .filter(|thread| !thread.stopped && !thread.queue.is_empty())
            .map(|thread| thread.id.clone())
            .collect();
        waiting.sort();
        for thread in waiting {
            self.flush(&thread, fx);
        }
        for action in std::mem::take(&mut self.held) {
            self.action(action, fx);
        }
    }

    /// The app-server can't be used: every action is refused from now on
    /// and the holder stops.
    fn refuse(&mut self, reason: String, fx: &mut Effects) {
        for action in std::mem::take(&mut self.held) {
            refused(fx, action.key(), &reason);
        }
        for thread in self.threads.values_mut() {
            thread.link = AgentLink::Down {
                reason: reason.clone(),
                since: Some(Timestamp::now()),
            };
        }
        self.phase = Phase::Refused(reason);
        fx.stop = true;
    }

    /// The app-server is gone. Running turns are cut off, prompts close and
    /// whatever waited on a reply is refused. Queued messages stay for when
    /// it comes back.
    fn down(&mut self, reason: &str, at: Timestamp, fx: &mut Effects) {
        if !matches!(self.phase, Phase::Refused(_)) {
            self.phase = Phase::Down;
        }
        // The calls the app-server asked for go with it.
        self.tool_calls.clear();
        let mut calls: Vec<(String, Call)> = self.rpc.calls.drain().collect();
        calls.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, call) in calls {
            if let Some(key) = call.key() {
                refused(fx, key, reason);
            }
        }
        let mut ids: Vec<String> = self.threads.keys().cloned().collect();
        ids.sort();
        for id in ids {
            let Some(thread) = self.threads.get_mut(&id) else {
                continue;
            };
            let mut open: Vec<_> = thread.open.drain().map(|(_, open)| open).collect();
            open.sort_by(|a, b| a.item.id.0.cmp(&b.item.id.0));
            for open in open.into_iter().filter(|open| open.shown) {
                let mut item = open.item;
                close(&mut item, ItemStatus::Interrupted, Some(at));
                upsert(fx, item);
            }
            if let Some(turn) = thread.active_turn() {
                upsert(fx, items::interrupted(thread.agent, turn, Some(at)));
            }
            if thread.busy() {
                thread.ended_at = Some(at);
                thread.touch(at);
            }
            let unsent: Vec<_> = thread
                .sent
                .drain(..)
                .chain(thread.steers.drain(..))
                .collect();
            for message in unsent {
                upsert(fx, message.item(thread.agent, ItemStatus::Failed));
                if let Some(key) = message.key {
                    refused(fx, key, reason);
                }
            }
            thread.turn = Turn::Idle;
            thread.loaded = false;
            thread.resuming = false;
            thread.buffer.clear();
            thread.interrupt_wanted = false;
            if !thread.stopped {
                thread.link = AgentLink::Down {
                    reason: reason.to_owned(),
                    since: Some(at),
                };
            }
        }
        let mut prompts: Vec<(AttentionId, Prompt)> = self.prompts.drain().collect();
        prompts.sort_by_key(|(_, prompt)| prompt.event.created_at);
        for (id, prompt) in prompts {
            let mut item = prompt.item;
            item.status = ItemStatus::Cancelled;
            upsert(fx, item);
            fx.attention.push(AttentionChange::Close(id));
        }
        for action in std::mem::take(&mut self.held) {
            refused(fx, action.key(), reason);
        }
        self.strays.clear();
        self.pending.clear();
        self.timers
            .retain(|(_, timer)| matches!(timer, Timer::Cold { .. }));
    }

    /// Takes one line, noting its seq while it runs.
    fn take(&mut self, text: &str, seen: Seen, fx: &mut Effects) {
        let outer = std::mem::replace(&mut self.seq, seen.seq);
        self.sort(text, seen, fx);
        self.seq = outer;
    }

    /// Sorts one line and acts on it.
    fn sort(&mut self, text: &str, seen: Seen, fx: &mut Effects) {
        let Some(incoming) = rpc::parse(text) else {
            tracing::debug!("codex app-server printed a line that isn't JSON-RPC");
            return;
        };
        match incoming {
            Incoming::Response { id, outcome } => {
                // The app-server's replies echo the driver's string IDs, so
                // anything else, or an ID from an earlier run, isn't ours.
                let Some(id) = id.as_str() else {
                    return;
                };
                if !self.rpc.ids.ours(id) {
                    return;
                }
                if let Some(call) = self.rpc.calls.remove(id) {
                    self.answered(call, outcome, seen, fx);
                }
            }
            Incoming::Request { id, method, params } => {
                if self.held_back(text, &method, &params, seen, fx) {
                    return;
                }
                self.request(id, &method, params, seen, fx);
            }
            Incoming::Notification {
                method,
                params,
                emitted,
            } => {
                if method != "thread/started" && self.held_back(text, &method, &params, seen, fx) {
                    return;
                }
                self.notification(&method, params, emitted, seen, fx);
            }
        }
    }

    /// Keeps a line for later when its thread is resuming or not known
    /// yet, and says whether it did.
    fn held_back(
        &mut self,
        text: &str,
        method: &str,
        params: &Value,
        seen: Seen,
        fx: &mut Effects,
    ) -> bool {
        let Some(id) = params.get("threadId").and_then(Value::as_str) else {
            return false;
        };
        if let Some(thread) = self.threads.get_mut(id) {
            if thread.resuming {
                thread.keep(text, method, params, seen);
                return true;
            }
            return false;
        }
        if !self.strays.contains_key(id) && self.strays.len() >= STRAY_THREADS {
            tracing::debug!(thread = id, "dropped a line for an unknown codex thread");
            return true;
        }
        self.strays
            .entry(id.to_owned())
            .or_default()
            .keep(text, method, params, seen, STRAY_LINES);
        if self.phase == Phase::Ready {
            self.ask_about(id, fx);
        }
        true
    }

    /// Asks the app-server what a thread is, once.
    fn ask_about(&mut self, thread: &str, fx: &mut Effects) {
        if self
            .rpc
            .waiting(|call| matches!(call, Call::Read { thread: asked } if asked == thread))
        {
            return;
        }
        let params = json!({ "threadId": thread });
        let call = Call::Read {
            thread: thread.to_owned(),
        };
        self.rpc.send("thread/read", params, call, fx);
    }

    /// Runs the lines kept for `thread` while it was unknown or resuming,
    /// in the order the holder logged them. Requests it still held came
    /// ahead of its log, so they move back into place.
    fn replay_kept(&mut self, thread: &str, fx: &mut Effects) {
        let mut lines = self
            .strays
            .remove(thread)
            .map(|mut kept| kept.take())
            .unwrap_or_default();
        if let Some(known) = self.threads.get_mut(thread) {
            lines.extend(known.buffer.take());
        }
        lines.sort_by_key(|(_, seen)| seen.seq);
        for (text, seen) in lines {
            self.take(&text, seen, fx);
        }
    }

    /// Adds a thread, or gives the one already known. A thread an earlier
    /// driver drove keeps the times `maxd` stored.
    fn register(&mut self, id: &str, agent: AgentId, at: Timestamp) -> &mut Thread {
        let met = &mut self.met;
        let thread = self.threads.entry(id.to_owned()).or_insert_with(|| {
            let mut thread = Thread::new(id.to_owned(), agent, at);
            if let Some((started, last)) = met.remove(&agent) {
                thread.started_at = started;
                thread.last_activity_at = last;
            }
            thread
        });
        if thread.agent != agent {
            self.agents.remove(&thread.agent);
            thread.agent = agent;
        }
        self.agents.insert(agent, id.to_owned());
        thread
    }

    /// Adds a thread from a `Thread` object Codex sent, as a sub-agent when
    /// it has a parent.
    fn learn_thread(&mut self, value: &Value, at: Timestamp) -> Option<String> {
        let id = value.get("id")?.as_str()?.to_owned();
        let parent = parent_of(value);
        let agent = match &parent {
            Some(parent) => {
                let parent_agent = self
                    .threads
                    .get(parent)
                    .map(|thread| thread.agent)
                    .unwrap_or_else(|| names::agent(Vendor::Codex, parent));
                names::subagent(parent_agent, &id)
            }
            None => self
                .threads
                .get(&id)
                .map(|thread| thread.agent)
                .unwrap_or_else(|| names::agent(Vendor::Codex, &id)),
        };
        let created = created_at(value);
        let thread = self.register(&id, agent, created.unwrap_or(at));
        if let Some(created) = created {
            thread.started_at = created;
        }
        if parent.is_some() {
            thread.parent = parent;
            if thread.title.is_none() {
                thread.title = ["agentNickname", "agentRole", "name"]
                    .iter()
                    .find_map(|field| value.get(*field).and_then(Value::as_str))
                    .map(str::to_owned)
                    .or_else(|| Some("Sub-agent".into()));
            }
        }
        learn_details(thread, value);
        Some(id)
    }

    /// Sends the next queued message, or settings that waited, once the
    /// thread is free. A thread the app-server let go is resumed first.
    fn flush(&mut self, id: &str, fx: &mut Effects) {
        if self.phase != Phase::Ready {
            return;
        }
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if thread.resuming || thread.busy() || thread.limited.is_some() {
            return;
        }
        if thread.queue.is_empty() {
            // Settings alone never bring a thread back. They go with its
            // next turn or resume.
            if thread.dirty && thread.loaded && !thread.stopped {
                self.update_settings(id, None, fx);
            }
            return;
        }
        if !thread.loaded || thread.stopped {
            self.resume(id, None, None, false, false, fx);
            return;
        }
        if let Some(message) = thread.queue.pop_front() {
            self.turn_start(id, message, fx);
        }
    }

    /// Reads the account's rate limits unless a read is out.
    fn read_limits(&mut self, fx: &mut Effects) {
        if self.rpc.waiting(|call| matches!(call, Call::RateLimits)) {
            return;
        }
        let params = json!({ "excludeResetCreditDetails": true });
        self.rpc
            .send("account/rateLimits/read", params, Call::RateLimits, fx);
    }

    /// Shows a rate-limit reading, and lets limited threads go on once
    /// every window has room.
    fn take_limits(&mut self, reading: &crate::usage::codex::limits::Reading, fx: &mut Effects) {
        let Some(usage) = crate::usage::codex::limits::merge(self.usage.take(), reading) else {
            return;
        };
        fx.usage.push((self.login.account, usage.clone()));
        let room = usage
            .windows
            .iter()
            .all(|window| window.used_percent < 100.0);
        self.usage = Some(usage);
        self.arm_limits(Instant::now());
        if !room {
            let resets = self.resets_at();
            let mut limited: Vec<&mut Thread> = self
                .threads
                .values_mut()
                .filter(|thread| thread.limited.is_some_and(|known| known != resets))
                .collect();
            limited.sort_by(|a, b| a.id.cmp(&b.id));
            for thread in limited {
                thread.limited = Some(resets);
                if let Some(item) = &mut thread.limit_item
                    && let ItemBody::LimitWait { resets_at, .. } = &mut item.body
                {
                    *resets_at = resets;
                    upsert(fx, item.clone());
                }
            }
            return;
        }
        let mut freed: Vec<String> = self
            .threads
            .values()
            .filter(|thread| thread.limited.is_some())
            .map(|thread| thread.id.clone())
            .collect();
        freed.sort();
        let now = Timestamp::now();
        for id in freed {
            if let Some(thread) = self.threads.get_mut(&id) {
                end_wait(thread, now, fx);
            }
            self.flush(&id, fx);
        }
    }

    /// When the account's limits allow new turns again, from the windows
    /// that are full.
    fn resets_at(&self) -> Option<Timestamp> {
        self.usage
            .as_ref()?
            .windows
            .iter()
            .filter(|window| window.used_percent >= 100.0)
            .filter_map(|window| window.resets_at)
            .max()
    }

    /// Sets the next rate-limit read for `LIMITS_EVERY` after `now`.
    fn arm_limits(&mut self, now: Instant) {
        self.timers.retain(|(_, timer)| *timer != Timer::Limits);
        self.timers.push((now + LIMITS_EVERY, Timer::Limits));
    }

    /// Runs a timer that came due.
    fn fire(&mut self, timer: Timer, now: Instant, fx: &mut Effects) {
        match timer {
            Timer::Retry {
                thread,
                turn,
                attempt,
            } => self.interrupt_turn(&thread, &turn, attempt + 1, fx),
            Timer::Cold { thread } => {
                if let Some(thread) = self.threads.get_mut(&thread) {
                    thread.cold = true;
                }
            }
            Timer::Limits => {
                if self.phase == Phase::Ready {
                    self.read_limits(fx);
                }
                self.arm_limits(now);
            }
            Timer::Expire { attention } => self.expire(attention, fx),
        }
    }

    /// Starts the warm-cache countdown for a thread whose turn ended at
    /// `ended`.
    fn arm_cold(&mut self, thread: &str, ended: Timestamp) {
        self.timers
            .retain(|(_, timer)| !matches!(timer, Timer::Cold { thread: t } if t == thread));
        let gone = Duration::from_millis(Timestamp::now().0.saturating_sub(ended.0));
        let left = CACHE_WARM.saturating_sub(gone);
        if let Some(known) = self.threads.get_mut(thread) {
            known.cold = left.is_zero();
        }
        if !left.is_zero() {
            let timer = Timer::Cold {
                thread: thread.to_owned(),
            };
            self.timers.push((Instant::now() + left, timer));
        }
    }

    /// Where a thread runs: its own place, or the nearest ancestor's.
    fn place_of(&self, id: &str) -> Option<&crate::drive::Place> {
        let mut thread = self.threads.get(id)?;
        for _ in 0..MAX_DEPTH {
            if let Some(place) = &thread.place {
                return Some(place);
            }
            thread = self.threads.get(thread.parent.as_ref()?)?;
        }
        None
    }

    /// Whether any sub-agent of `id` has a turn running.
    fn child_busy(&self, id: &str) -> bool {
        self.threads
            .values()
            .any(|thread| thread.parent.as_deref() == Some(id) && thread.busy())
    }

    /// The agent a thread shows as, or `None` until it has a place.
    fn agent(&self, id: &str) -> Option<Agent> {
        let thread = self.threads.get(id)?;
        let place = self.place_of(id)?;
        let parent = thread.parent.as_ref().map(|parent| {
            self.threads
                .get(parent)
                .map(|known| known.agent)
                .unwrap_or_else(|| names::agent(Vendor::Codex, parent))
        });
        let waiting = self
            .prompts
            .iter()
            .filter(|(_, prompt)| prompt.thread == thread.id)
            .min_by_key(|(event, prompt)| (prompt.event.created_at, **event))
            .map(|(event, _)| *event);
        let activity = if let Some(attention) = waiting {
            Activity::NeedsInput { attention }
        } else if let Some(resets_at) = thread.limited {
            Activity::WaitingForLimitReset { resets_at }
        } else if let Some(reason) = &thread.errored {
            Activity::Error {
                reason: reason.clone(),
            }
        } else if thread.resuming {
            Activity::Starting
        } else if self.child_busy(id) {
            Activity::BusyInSubagents
        } else if thread.busy() {
            Activity::Busy
        } else if let (Some(ended), false) = (thread.ended_at, thread.cold) {
            Activity::IdleCacheWarm {
                warm_until: ended.after(CACHE_WARM),
            }
        } else {
            Activity::Idle
        };
        let permission = thread
            .requested
            .or(thread.effective)
            .map(|requested| AgentPermission {
                requested,
                effective: thread.effective,
            });
        Some(Agent {
            id: thread.agent,
            workspace: place.workspace,
            parent,
            vendor: Vendor::Codex,
            account: Some(self.login.account),
            model: thread.model.clone(),
            permission,
            session: None,
            vendor_session_id: Some(thread.id.clone()),
            control: Control::Held,
            pid: None,
            link: thread.link.clone(),
            title: thread.title.clone().unwrap_or_else(|| "Codex".into()),
            activity,
            context: thread.context,
            started_at: thread.started_at,
            last_activity_at: thread.last_activity_at,
            branch: None,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
            local: None,
        })
    }

    /// Sends every agent that changed and the next time to wake.
    fn settle(&mut self, fx: &mut Effects) {
        let mut ids: Vec<String> = self.threads.keys().cloned().collect();
        ids.sort();
        for id in ids {
            let Some(agent) = self.agent(&id) else {
                continue;
            };
            if self.shown.get(&agent.id) != Some(&agent) {
                self.shown.insert(agent.id, agent.clone());
                fx.agents.push(agent);
            }
        }
        fx.wake_at = self.timers.iter().map(|(at, _)| *at).min();
    }

    /// The thread an action's agent is, or answers `UnknownAgent`.
    fn thread_for(&self, agent: AgentId, key: IdempotencyKey, fx: &mut Effects) -> Option<String> {
        let thread = self.agents.get(&agent).cloned();
        if thread.is_none() {
            fx.results.push((key, ActionOutcome::UnknownAgent));
        }
        thread
    }
}

impl Driver for CodexServer {
    fn connected(&mut self, fresh: bool, pending: &[HeldLine], fx: &mut Effects) {
        if fresh && self.phase != Phase::Down && !matches!(self.phase, Phase::Refused(_)) {
            self.down("Codex restarted", Timestamp::now(), fx);
        }
        self.pending = pending
            .iter()
            .filter_map(|line| rpc::parse(&line.text))
            .filter_map(|incoming| match incoming {
                Incoming::Request { id, .. } => rpc::key(&id),
                _ => None,
            })
            .collect();
        // A new `maxd` meets an app-server an earlier one set up, which
        // answers "Already initialized", so the handshake runs either way.
        if self.phase == Phase::Down {
            self.handshake(fx);
        }
        for line in pending {
            let seen = Seen {
                at: line.at,
                seq: line.seq,
                replayed: false,
            };
            self.take(&line.text, seen, fx);
        }
        self.settle(fx);
    }

    fn line(&mut self, line: &HeldLine, fx: &mut Effects) {
        let seen = Seen {
            at: line.at,
            seq: line.seq,
            replayed: line.replayed,
        };
        self.take(&line.text, seen, fx);
        self.settle(fx);
    }

    fn act(&mut self, action: Action, fx: &mut Effects) {
        match &self.phase {
            Phase::Ready => self.action(action, fx),
            Phase::Refused(reason) => {
                let reason = reason.clone();
                refused(fx, action.key(), &reason);
            }
            Phase::Down | Phase::Handshake => self.held.push(action),
        }
        self.settle(fx);
    }

    fn exited(&mut self, exit: &ChildExit, fx: &mut Effects) {
        let reason = exit_reason(exit);
        self.down(&reason, exit.at, fx);
        self.settle(fx);
    }

    fn tool_done(&mut self, done: ToolDone, fx: &mut Effects) {
        self.call_done(done, fx);
        self.settle(fx);
    }

    /// An app-server holds threads for several agents, which come back one
    /// by one as `maxd` adopts them, so their times wait until then.
    fn met_again(&mut self, known: &Agent) {
        let times = (known.started_at, known.last_activity_at);
        match self
            .agents
            .get(&known.id)
            .and_then(|id| self.threads.get_mut(id))
        {
            Some(thread) => {
                thread.started_at = times.0;
                thread.last_activity_at = times.1;
            }
            None => {
                self.met.insert(known.id, times);
            }
        }
    }

    /// Codex caches each folder's skills, so it reads them again with
    /// `skills/list` and `forceReload`. An app-server that
    /// isn't up yet reads them fresh when it starts.
    fn skills_changed(&mut self, cwds: &[String], fx: &mut Effects) {
        if self.phase == Phase::Ready && !cwds.is_empty() {
            let params = json!({ "cwds": cwds, "forceReload": true });
            self.rpc.send("skills/list", params, Call::Skills, fx);
        }
    }

    fn tick(&mut self, now: Instant, fx: &mut Effects) {
        let mut due = Vec::new();
        self.timers.retain(|(at, timer)| {
            if *at <= now {
                due.push((*at, timer.clone()));
                false
            } else {
                true
            }
        });
        due.sort_by_key(|(at, _)| *at);
        for (_, timer) in due {
            self.fire(timer, now, fx);
        }
        self.settle(fx);
    }

    fn can_release(&self, agent: AgentId) -> bool {
        self.releasable(agent)
    }

    fn release(&mut self, agent: AgentId, fx: &mut Effects) {
        self.release_thread(agent, fx);
    }

    /// The earliest line of any turn still running, or of lines kept and
    /// not taken yet. A command that outlived its turn doesn't count.
    fn checkpoint(&self) -> Option<u64> {
        let turns = self
            .threads
            .values()
            .filter(|thread| thread.active_turn().is_some())
            .filter_map(|thread| thread.turn_from);
        let kept = self
            .strays
            .values()
            .chain(self.threads.values().map(|thread| &thread.buffer))
            .filter_map(Kept::first_seq);
        turns.chain(kept).min()
    }
}

/// Why the app-server ended, with the last line it printed to stderr.
fn exit_reason(exit: &ChildExit) -> String {
    let how = match (exit.code, exit.signal) {
        (Some(code), _) => format!("Codex exited with code {code}"),
        (None, Some(signal)) => format!("Codex was stopped by signal {signal}"),
        (None, None) => "Codex exited".to_owned(),
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

/// The thread that spawned a `Thread` object's thread, if it's a sub-agent.
fn parent_of(value: &Value) -> Option<String> {
    if let Some(parent) = value.get("parentThreadId").and_then(Value::as_str) {
        return Some(parent.to_owned());
    }
    value
        .pointer("/source/subAgent/thread_spawn/parent_thread_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// When a `Thread` object's thread was made. Codex gives seconds.
fn created_at(value: &Value) -> Option<Timestamp> {
    let secs = value.get("createdAt")?.as_u64()?;
    Some(Timestamp(secs.saturating_mul(1000)))
}

/// Takes a `Thread` object's name, model and status into `thread`. A
/// title from the preview, which is the first message as Codex got it,
/// shows its pills as the app does and leaves out their context block.
fn learn_details(thread: &mut Thread, value: &Value) {
    if !thread.titled {
        let named = value
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let previewed = || {
            value
                .get("preview")
                .and_then(Value::as_str)
                .and_then(|preview| items::title(&context::readable(preview)))
        };
        if let Some(title) = named.or_else(|| thread.title.is_none().then(previewed).flatten()) {
            thread.title = Some(title);
        }
    }
    if let Some(model) = value.get("model").and_then(Value::as_str) {
        thread.model = Some(model.to_owned());
    }
    if let Some(status) = value.get("status") {
        apply_status(thread, status);
    }
}

/// Takes a thread status such as `{"type":"active"}` into `thread`.
fn apply_status(thread: &mut Thread, status: &Value) {
    match status.get("type").and_then(Value::as_str) {
        Some("active") => {
            thread.loaded = true;
            thread.errored = None;
        }
        Some("idle") => thread.loaded = true,
        Some("systemError") => {
            thread.loaded = true;
            thread.errored = Some("Codex hit a system error".into());
        }
        Some("notLoaded") => {
            thread.loaded = false;
            thread.turn = Turn::Idle;
        }
        _ => {}
    }
}

/// How full a context window is, from a `tokenUsage` object.
fn context_usage(usage: &Value) -> Option<ContextUsage> {
    let used = usage.pointer("/last/inputTokens")?.as_u64()?;
    let window = usage.get("modelContextWindow")?.as_u64()?;
    Some(ContextUsage {
        used_tokens: u32::try_from(used).unwrap_or(u32::MAX),
        window_tokens: u32::try_from(window).unwrap_or(u32::MAX),
    })
}

/// Marks an item finished with `status`, ending any streaming.
fn close(item: &mut TimelineItem, status: ItemStatus, at: Option<Timestamp>) {
    item.status = status;
    item.completed_at = item.completed_at.or(at);
    match &mut item.body {
        ItemBody::AssistantMessage { streaming, .. }
        | ItemBody::Reasoning { streaming, .. }
        | ItemBody::Plan { streaming, .. } => *streaming = false,
        _ => {}
    }
}

/// Adds or replaces an item.
fn upsert(fx: &mut Effects, item: TimelineItem) {
    fx.timeline
        .push((item.agent, TimelineEvent::Upsert { item }));
}

/// Closes a thread's limit wait, once the limit no longer holds it.
fn end_wait(thread: &mut Thread, at: Timestamp, fx: &mut Effects) {
    thread.limited = None;
    if let Some(mut item) = thread.limit_item.take() {
        item.status = ItemStatus::Completed;
        item.completed_at = Some(at);
        upsert(fx, item);
    }
}

/// Takes an item off a timeline.
fn remove(fx: &mut Effects, agent: AgentId, id: ItemId) {
    fx.timeline.push((agent, TimelineEvent::Remove { id }));
}

/// Adds text to the end of an item's field.
fn append(fx: &mut Effects, agent: AgentId, id: ItemId, field: TextField, text: &str) {
    fx.timeline.push((
        agent,
        TimelineEvent::Append {
            id,
            field,
            text: text.to_owned(),
        },
    ));
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

/// Opens an attention event.
fn open_attention(fx: &mut Effects, event: AttentionEvent) {
    fx.attention.push(AttentionChange::Open(event));
}
