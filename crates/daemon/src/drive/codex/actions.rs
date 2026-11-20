// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What you ask a Codex agent to do, as requests to the app-server, and
//! what comes back.

use std::time::{Duration, Instant};

use maxhq_core::names;
use maxhq_core::tools::dispatch;
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, AgentSpec, SendMode};
use maxhq_protocol::model::{AgentLink, ModelChoice, PermissionMode, Vendor};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemStatus, TimelineItem};
use serde_json::{json, Map, Value};

use super::rpc::{self, RpcError};
use super::thread::{Outgoing, Thread, Turn};
use super::{
    done, input, items, learn_details, policy, refused, remove, upsert, CodexServer, Phase, Seen,
    Timer, RETRY_MS,
};
use crate::drive::pictures::Pictures;
use crate::drive::{Action, Effects, Message, Place, PlaceHost, Write};
use crate::usage::codex::{limits, models};

/// What a request the driver sent was for.
#[derive(Clone, Debug)]
pub enum Call {
    /// The handshake.
    Initialize,
    /// A page of the model list.
    Models,
    /// The account's rate limits.
    RateLimits,
    /// A new thread for `Action::Start`.
    ThreadStart {
        /// The action's key.
        key: IdempotencyKey,
        /// Where it runs.
        place: Place,
        /// The mode it starts in.
        mode: PermissionMode,
        /// The title you gave it.
        title: Option<String>,
        /// The effort level it starts at, or none for the model's default.
        effort: Option<String>,
        /// The first message.
        first: Option<Message>,
        /// The agent the thread is, if not a new one.
        agent: Option<AgentId>,
    },
    /// A thread taken back up.
    Resume {
        /// The thread.
        thread: String,
        /// The action's key, for `Adopt`.
        key: Option<IdempotencyKey>,
        /// Where it runs, for `Adopt`, set once the resume works.
        place: Option<Place>,
        /// Whether the thread was new to the driver, so a failed adopt
        /// forgets it.
        new: bool,
        /// Whether to read its turns as history afterwards.
        history: bool,
    },
    /// A page of a thread's turns.
    Turns {
        /// The thread.
        thread: String,
        /// The action's key.
        key: Option<IdempotencyKey>,
        /// The items read so far.
        items: Vec<TimelineItem>,
        /// The turn still running, if one is.
        active: Option<String>,
    },
    /// What a thread the driver met in passing is.
    Read {
        /// The thread.
        thread: String,
    },
    /// A turn with a message.
    TurnStart {
        /// The thread.
        thread: String,
        /// The message's client ID.
        client_id: String,
    },
    /// A message into the running turn.
    Steer {
        /// The thread.
        thread: String,
        /// The message's client ID.
        client_id: String,
        /// Whether this already retried with the turn Codex named.
        retried: bool,
    },
    /// An interrupt.
    Interrupt {
        /// The thread.
        thread: String,
        /// The turn.
        turn: String,
        /// How many tries came before.
        attempt: usize,
    },
    /// A change of mode or model.
    Settings {
        /// The thread.
        thread: String,
        /// The action's key.
        key: Option<IdempotencyKey>,
    },
    /// The name you gave a new thread.
    Name {
        /// The thread.
        thread: String,
    },
    /// A reload of the skills in some checkouts.
    Skills,
    /// Letting a thread go.
    Unsubscribe {
        /// The thread.
        thread: String,
        /// The action's key.
        key: IdempotencyKey,
    },
    /// Letting an idle thread go to free what the app-server holds for it.
    Release {
        /// The thread.
        thread: String,
    },
}

impl Call {
    /// The key of the action waiting on this request.
    pub fn key(&self) -> Option<IdempotencyKey> {
        match self {
            Call::ThreadStart { key, .. } | Call::Unsubscribe { key, .. } => Some(*key),
            Call::Resume { key, .. } | Call::Turns { key, .. } | Call::Settings { key, .. } => *key,
            _ => None,
        }
    }

    /// The thread the request is for, if one.
    pub fn thread(&self) -> Option<&str> {
        match self {
            Call::Resume { thread, .. }
            | Call::Turns { thread, .. }
            | Call::Read { thread }
            | Call::TurnStart { thread, .. }
            | Call::Steer { thread, .. }
            | Call::Interrupt { thread, .. }
            | Call::Settings { thread, .. }
            | Call::Name { thread }
            | Call::Unsubscribe { thread, .. }
            | Call::Release { thread } => Some(thread),
            Call::Initialize
            | Call::Models
            | Call::RateLimits
            | Call::ThreadStart { .. }
            | Call::Skills => None,
        }
    }
}

/// What Codex says when a thread is held by another process.
const OTHER_WRITER: &str = "active writer";

impl CodexServer {
    /// Carries out an action once the app-server is ready.
    pub(super) fn action(&mut self, action: Action, fx: &mut Effects) {
        match action {
            Action::Start {
                key,
                place,
                spec,
                first,
                agent,
            } => self.start(key, place, *spec, first, agent, fx),
            Action::Adopt {
                key,
                place,
                agent,
                thread,
            } => {
                let new = !self.threads.contains_key(&thread);
                let known = self.register(&thread, agent, Timestamp::now());
                known.stopped = false;
                self.resume(&thread, Some(key), Some(place), new, true, fx);
            }
            Action::Send {
                key,
                agent,
                text,
                refs,
                attachments,
                mode,
            } => {
                if let Some(thread) = self.thread_for(agent, key, fx) {
                    let message = Message {
                        text,
                        refs,
                        attachments,
                    };
                    let message = Outgoing::new(message, Some(key), Timestamp::now());
                    self.send_message(&thread, message, mode, fx);
                }
            }
            Action::CancelQueued { key, agent, item } => {
                let Some(id) = self.thread_for(agent, key, fx) else {
                    return;
                };
                let Some(thread) = self.threads.get_mut(&id) else {
                    return;
                };
                let queued = thread
                    .queue
                    .iter()
                    .position(|message| message.local == item);
                let message = match queued {
                    Some(index) => thread.queue.remove(index),
                    None => thread
                        .steers
                        .iter()
                        .position(|message| message.local == item)
                        .map(|index| thread.steers.remove(index)),
                };
                match message {
                    Some(mut message) => {
                        message.delivery = Delivery::Cancelled;
                        upsert(fx, message.item(thread.agent, ItemStatus::Cancelled));
                        // With nothing left to send, the wait no longer
                        // resumes on its own.
                        if thread.queue.is_empty()
                            && let Some(wait) = &mut thread.limit_item
                            && let ItemBody::LimitWait { auto_resume, .. } = &mut wait.body
                            && *auto_resume
                        {
                            *auto_resume = false;
                            upsert(fx, wait.clone());
                        }
                        done(fx, key);
                    }
                    None => refused(fx, key, "That message already went to Codex."),
                }
            }
            Action::Interrupt { key, agent } => {
                let Some(id) = self.thread_for(agent, key, fx) else {
                    return;
                };
                self.interrupt(&id, fx);
                done(fx, key);
            }
            Action::SetPermission { key, agent, mode } => {
                let Some(id) = self.thread_for(agent, key, fx) else {
                    return;
                };
                if policy::policy(mode).is_none() {
                    refused(fx, key, "Codex has no plan mode.");
                    return;
                }
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.requested = Some(mode);
                }
                self.change_settings(&id, key, fx);
            }
            Action::SetModel { key, agent, model } => {
                let Some(id) = self.thread_for(agent, key, fx) else {
                    return;
                };
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.wanted_model = Some(model);
                }
                self.change_settings(&id, key, fx);
            }
            Action::SetEffort { key, agent, effort } => {
                let Some(id) = self.thread_for(agent, key, fx) else {
                    return;
                };
                if let Some(thread) = self.threads.get_mut(&id) {
                    thread.effort = effort;
                }
                self.change_settings(&id, key, fx);
            }
            Action::Answer {
                key,
                event,
                answer,
                by,
            } => self.answer(key, event, answer, by, fx),
            Action::Stop { key, agent } => {
                let Some(id) = self.thread_for(agent, key, fx) else {
                    return;
                };
                self.stop(&id, key, fx);
            }
        }
    }

    /// Starts a thread, then its first turn.
    fn start(
        &mut self,
        key: IdempotencyKey,
        place: Place,
        spec: AgentSpec,
        first: Option<Message>,
        agent: Option<AgentId>,
        fx: &mut Effects,
    ) {
        let mode = spec.permission.unwrap_or(PermissionMode::Ask);
        let Some(policy) = policy_in(Some(&place), Some(mode)) else {
            refused(fx, key, "Codex has no plan mode.");
            return;
        };
        let mut params = Map::new();
        params.insert(
            "cwd".into(),
            place.cwd.to_string_lossy().into_owned().into(),
        );
        if let Some(model) = &spec.model {
            params.insert("model".into(), model.clone().into());
        }
        policy.add_to_thread(&mut params);
        add_instructions(Some(&place), &mut params);
        add_trust(Some(&place), &mut params);
        // MaxHQ's own tools stay with the thread, through every resume.
        // Codex takes them only here, so a thread keeps the ones it
        // started with.
        let tools: Vec<Value> = place.tools().iter().map(|tool| tool.codex_spec()).collect();
        params.insert("dynamicTools".into(), tools.into());
        let call = Call::ThreadStart {
            key,
            place,
            mode,
            title: spec.title,
            effort: spec.effort,
            first,
            agent,
        };
        self.rpc
            .send("thread/start", Value::Object(params), call, fx);
    }

    /// Sends a message the way `mode` says, holding it when the thread
    /// can't take it yet.
    pub(super) fn send_message(
        &mut self,
        id: &str,
        mut message: Outgoing,
        mode: SendMode,
        fx: &mut Effects,
    ) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if thread.stopped {
            thread.stopped = false;
            thread.link = AgentLink::Live;
        }
        let limited_idle = thread.limited.is_some() && !thread.busy();
        if !thread.loaded || thread.resuming || (limited_idle && mode == SendMode::Queue) {
            if let Some(key) = message.key.take() {
                done(fx, key);
            }
            message.delivery = Delivery::Queued;
            upsert(fx, message.item(thread.agent, ItemStatus::Pending));
            thread.queue.push_back(message);
            if let Some(item) = &mut thread.limit_item
                && let ItemBody::LimitWait { auto_resume, .. } = &mut item.body
                && !*auto_resume
            {
                *auto_resume = true;
                upsert(fx, item.clone());
            }
            self.flush(id, fx);
            return;
        }
        match (mode, thread.turn.clone()) {
            (_, Turn::Idle) => self.turn_start(id, message, fx),
            (SendMode::Queue, _) => {
                if let Some(key) = message.key.take() {
                    done(fx, key);
                }
                message.delivery = Delivery::Queued;
                upsert(fx, message.item(thread.agent, ItemStatus::Pending));
                thread.queue.push_back(message);
            }
            (SendMode::Steer, Turn::Active(turn)) => {
                message.delivery = Delivery::Steer;
                let client_id = message.client_id.clone();
                thread.sent.push_back(message);
                self.steer(id, &client_id, &turn, false, fx);
            }
            (SendMode::Steer, Turn::Starting) => {
                message.delivery = Delivery::Steer;
                upsert(fx, message.item(thread.agent, ItemStatus::Pending));
                thread.steers.push(message);
            }
            (SendMode::Interrupt, _) => {
                if let Some(key) = message.key.take() {
                    done(fx, key);
                }
                message.delivery = Delivery::Queued;
                upsert(fx, message.item(thread.agent, ItemStatus::Pending));
                thread.queue.push_front(message);
                self.dismiss_waiting(id, fx);
                self.interrupt(id, fx);
            }
        }
    }

    /// Starts a turn with `message`.
    pub(super) fn turn_start(&mut self, id: &str, mut message: Outgoing, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        // A message that waited in the queue moves to the end as it goes,
        // after all the turn before it said. It keeps that place once
        // Codex has it.
        if message.delivery == Delivery::Queued {
            remove(fx, thread.agent, message.local.clone());
        }
        message.delivery = Delivery::Sent;
        upsert(fx, message.item(thread.agent, ItemStatus::Pending));
        let mut params = Map::new();
        params.insert("threadId".into(), id.into());
        params.insert(
            "clientUserMessageId".into(),
            message.client_id.clone().into(),
        );
        params.insert(
            "input".into(),
            input::input(
                &message.text,
                &message.refs,
                &message.attachments,
                thread.agent,
                on_this_device(thread),
            ),
        );
        if let Some(policy) = policy_in(thread.place.as_ref(), thread.requested) {
            policy.add_to_turn(&mut params);
        }
        if let Some(model) = thread.wanted_model.take() {
            params.insert("model".into(), model.clone().into());
            thread.model = Some(model);
        }
        if let Some(effort) = effort_for(thread, &self.catalog) {
            params.insert("effort".into(), effort.into());
        }
        params.insert("summary".into(), "auto".into());
        thread.dirty = false;
        thread.turn = Turn::Starting;
        thread.last_activity_at = Timestamp::now();
        let call = Call::TurnStart {
            thread: id.to_owned(),
            client_id: message.client_id.clone(),
        };
        thread.sent.push_back(message);
        self.rpc.send("turn/start", Value::Object(params), call, fx);
    }

    /// Sends the steer already in `sent` under `client_id` into `turn`.
    fn steer(&mut self, id: &str, client_id: &str, turn: &str, retried: bool, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let Some(message) = thread.sent.iter().find(|m| m.client_id == client_id) else {
            return;
        };
        if !retried {
            upsert(fx, message.item(thread.agent, ItemStatus::Pending));
        }
        let params = json!({
            "threadId": id,
            "clientUserMessageId": client_id,
            "input": input::input(
                &message.text,
                &message.refs,
                &message.attachments,
                thread.agent,
                on_this_device(thread),
            ),
            "expectedTurnId": turn,
        });
        let call = Call::Steer {
            thread: id.to_owned(),
            client_id: client_id.to_owned(),
            retried,
        };
        self.rpc.send("turn/steer", params, call, fx);
    }

    /// The turn's ID is known: send what waited for it.
    pub(super) fn turn_known(&mut self, id: &str, turn: &str, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        thread.run(turn, self.seq);
        let steers = std::mem::take(&mut thread.steers);
        let interrupt = std::mem::take(&mut thread.interrupt_wanted);
        let waiting: Vec<String> = steers.iter().map(|m| m.client_id.clone()).collect();
        thread.sent.extend(steers);
        for client_id in waiting {
            self.steer(id, &client_id, turn, false, fx);
        }
        if interrupt {
            self.interrupt_turn(id, turn, 0, fx);
        }
    }

    /// Interrupts the running turn, or the next one once its ID is known.
    pub(super) fn interrupt(&mut self, id: &str, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        match thread.turn.clone() {
            Turn::Active(turn) => self.interrupt_turn(id, &turn, 0, fx),
            Turn::Starting => thread.interrupt_wanted = true,
            Turn::Idle => {}
        }
    }

    /// Sends `turn/interrupt` for `turn`, unless it already ended.
    pub(super) fn interrupt_turn(
        &mut self,
        id: &str,
        turn: &str,
        attempt: usize,
        fx: &mut Effects,
    ) {
        let running = self
            .threads
            .get(id)
            .is_some_and(|thread| thread.active_turn() == Some(turn));
        if !running || self.phase != Phase::Ready {
            return;
        }
        let params = json!({ "threadId": id, "turnId": turn });
        let call = Call::Interrupt {
            thread: id.to_owned(),
            turn: turn.to_owned(),
            attempt,
        };
        self.rpc.send("turn/interrupt", params, call, fx);
    }

    /// Takes a thread back up on the app-server. Lines for it wait until
    /// the resume and any history read are done.
    pub(super) fn resume(
        &mut self,
        id: &str,
        key: Option<IdempotencyKey>,
        place: Option<Place>,
        new: bool,
        history: bool,
        fx: &mut Effects,
    ) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if thread.resuming && key.is_none() {
            return;
        }
        thread.resuming = true;
        let mut params = Map::new();
        params.insert("threadId".into(), id.into());
        params.insert("excludeTurns".into(), true.into());
        let at = place.as_ref().or(thread.place.as_ref());
        if let Some(policy) = policy_in(at, thread.requested) {
            policy.add_to_thread(&mut params);
        }
        add_instructions(at, &mut params);
        add_trust(at, &mut params);
        if let Some(model) = thread.wanted_model.take() {
            params.insert("model".into(), model.clone().into());
            thread.model = Some(model);
        }
        thread.dirty = false;
        let call = Call::Resume {
            thread: id.to_owned(),
            key,
            place,
            new,
            history,
        };
        self.rpc
            .send("thread/resume", Value::Object(params), call, fx);
    }

    /// Applies a mode or model change now when the thread is free, or
    /// with its next turn.
    fn change_settings(&mut self, id: &str, key: IdempotencyKey, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if thread.loaded && !thread.busy() && !thread.resuming && !thread.stopped {
            self.update_settings(id, Some(key), fx);
        } else {
            thread.dirty = true;
            done(fx, key);
        }
    }

    /// Sends the thread's wanted mode and model.
    pub(super) fn update_settings(
        &mut self,
        id: &str,
        key: Option<IdempotencyKey>,
        fx: &mut Effects,
    ) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let mut params = Map::new();
        params.insert("threadId".into(), id.into());
        if let Some(policy) = policy_in(thread.place.as_ref(), thread.requested) {
            policy.add_to_turn(&mut params);
        }
        if let Some(model) = thread.wanted_model.take() {
            params.insert("model".into(), model.clone().into());
            thread.model = Some(model);
        }
        if let Some(effort) = effort_for(thread, &self.catalog) {
            params.insert("effort".into(), effort.into());
        }
        thread.dirty = false;
        let call = Call::Settings {
            thread: id.to_owned(),
            key,
        };
        self.rpc
            .send("thread/settings/update", Value::Object(params), call, fx);
    }

    /// Ends a thread's part in MaxHQ: its turn stops, its queue empties
    /// and the app-server lets it go. A later message takes it up again.
    fn stop(&mut self, id: &str, key: IdempotencyKey, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let turn = thread.active_turn().map(str::to_owned);
        let mut dropped: Vec<Outgoing> = thread.queue.drain(..).collect();
        dropped.append(&mut thread.steers);
        for mut message in dropped {
            message.delivery = Delivery::Cancelled;
            upsert(fx, message.item(thread.agent, ItemStatus::Cancelled));
        }
        thread.interrupt_wanted = false;
        thread.stopped = true;
        thread.link = AgentLink::Down {
            reason: "Stopped".into(),
            since: Some(Timestamp::now()),
        };
        let loaded = thread.loaded;
        if let Some(turn) = turn {
            self.interrupt_turn(id, &turn, RETRY_MS.len(), fx);
        }
        if loaded {
            let call = Call::Unsubscribe {
                thread: id.to_owned(),
                key,
            };
            self.rpc
                .send("thread/unsubscribe", json!({ "threadId": id }), call, fx);
        } else {
            done(fx, key);
        }
    }

    /// Acts on the reply to one of the driver's requests.
    pub(super) fn answered(
        &mut self,
        call: Call,
        outcome: Result<Value, RpcError>,
        seen: Seen,
        fx: &mut Effects,
    ) {
        match call {
            Call::Initialize => self.initialized(outcome, fx),
            Call::Models => match outcome {
                Ok(result) => self.models_page(&result, fx),
                Err(error) => tracing::debug!(error = error.message, "codex model/list failed"),
            },
            Call::Skills => {
                if let Err(error) = outcome {
                    tracing::debug!(error = error.message, "codex skills/list failed");
                }
            }
            Call::RateLimits => match outcome {
                Ok(result) => {
                    let reading = limits::from_app_server(&result, seen.at);
                    self.take_limits(&reading, fx);
                }
                Err(error) => {
                    tracing::debug!(error = error.message, "codex rate limits read failed")
                }
            },
            Call::ThreadStart {
                key,
                place,
                mode,
                title,
                effort,
                first,
                agent,
            } => match outcome {
                Ok(result) => {
                    let named = (title, agent, effort);
                    self.thread_started(result, key, place, mode, named, first, seen, fx);
                }
                Err(error) => refused(fx, key, &error.message),
            },
            Call::Resume {
                thread,
                key,
                place,
                new,
                history,
            } => self.resumed(&thread, outcome, key, place, new, history, fx),
            Call::Turns {
                thread,
                key,
                items,
                active,
            } => self.turns_page(&thread, outcome, key, items, active, fx),
            Call::Read { thread } => {
                let value = outcome
                    .ok()
                    .and_then(|mut result| result.get_mut("thread").map(Value::take))
                    .unwrap_or_else(|| json!({ "id": thread }));
                self.learn_thread(&value, seen.at);
                self.replay_kept(&thread, fx);
            }
            Call::TurnStart { thread, client_id } => {
                self.turn_started(&thread, &client_id, outcome, fx)
            }
            Call::Steer {
                thread,
                client_id,
                retried,
            } => self.steered(&thread, &client_id, retried, outcome, fx),
            Call::Interrupt {
                thread,
                turn,
                attempt,
            } => {
                if let Err(error) = outcome {
                    self.interrupt_failed(&thread, &turn, attempt, &error, fx);
                }
            }
            Call::Settings { key, .. } => {
                if let Some(key) = key {
                    match outcome {
                        Ok(_) => done(fx, key),
                        Err(error) => refused(fx, key, &error.message),
                    }
                }
            }
            // The thread keeps its title in this run either way. Once
            // `maxd` restarts, it's named from its first message again.
            Call::Name { thread } => {
                if let Err(error) = outcome {
                    tracing::warn!(
                        thread,
                        error = error.message,
                        "codex thread/name/set failed"
                    );
                }
            }
            Call::Unsubscribe { thread, key } => {
                if let Some(known) = self.threads.get_mut(&thread) {
                    known.loaded = false;
                }
                done(fx, key);
            }
            Call::Release { thread } => {
                if let Err(error) = outcome {
                    tracing::debug!(thread, error = error.message, "Codex kept an idle thread");
                }
            }
        }
    }

    /// The handshake's reply: go on, or refuse an app-server that's too
    /// old.
    fn initialized(&mut self, outcome: Result<Value, RpcError>, fx: &mut Effects) {
        let result = match outcome {
            Ok(result) => result,
            Err(error)
                if error
                    .message
                    .to_ascii_lowercase()
                    .contains("already initialized") =>
            {
                self.ready(fx);
                return;
            }
            Err(error) => {
                self.refuse(
                    format!("Codex refused the handshake: {}", error.message),
                    fx,
                );
                return;
            }
        };
        let user_agent = result
            .get("userAgent")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (major, minor, patch) = rpc::MIN_VERSION;
        match rpc::version(user_agent) {
            Some(found) if found >= rpc::MIN_VERSION => {
                fx.writes.push(Write {
                    line: rpc::notification("initialized"),
                    answers: None,
                });
                self.ready(fx);
            }
            Some((a, b, c)) => self.refuse(
                format!(
                    "Codex {a}.{b}.{c} is too old. MaxHQ needs {major}.{minor}.{patch} or newer."
                ),
                fx,
            ),
            None => self.refuse(
                format!(
                    "Codex didn't say which version it is. MaxHQ needs {major}.{minor}.{patch} or newer."
                ),
                fx,
            ),
        }
    }

    /// One page of `model/list`. The list goes out once the last page is
    /// in.
    fn models_page(&mut self, result: &Value, fx: &mut Effects) {
        match models::page(result, &mut self.models) {
            Some(cursor) => {
                let params = json!({ "cursor": cursor });
                self.rpc.send("model/list", params, Call::Models, fx);
            }
            None => {
                self.catalog = std::mem::take(&mut self.models);
                fx.models.push((self.login.account, self.catalog.clone()));
            }
        }
    }

    /// A new thread is up: take it in, then send its first message.
    #[allow(clippy::too_many_arguments)]
    fn thread_started(
        &mut self,
        result: Value,
        key: IdempotencyKey,
        place: Place,
        mode: PermissionMode,
        (title, agent, effort): (Option<String>, Option<AgentId>, Option<String>),
        first: Option<Message>,
        seen: Seen,
        fx: &mut Effects,
    ) {
        let Some(id) = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            refused(fx, key, "Codex started a thread without an ID.");
            return;
        };
        let agent = agent.unwrap_or_else(|| names::agent(Vendor::Codex, &id));
        let effective = policy::effective_in(&result);
        let thread = self.register(&id, agent, seen.at);
        thread.place = Some(place);
        thread.requested = Some(mode);
        thread.effective = effective;
        thread.effort = effort;
        if let Some(value) = result.get("thread") {
            learn_details(thread, value);
            if let Some(at) = super::created_at(value) {
                thread.started_at = at;
                thread.last_activity_at = at;
            }
        }
        if let Some(model) = result.get("model").and_then(Value::as_str) {
            thread.model = Some(model.to_owned());
        }
        match title {
            Some(title) => {
                thread.title = Some(title.clone());
                thread.titled = true;
                // Codex keeps the name with the thread, so a resume after
                // a restart of `maxd` reads it back.
                let params = json!({ "threadId": id, "name": title });
                let call = Call::Name { thread: id.clone() };
                self.rpc.send("thread/name/set", params, call, fx);
            }
            None => {
                if let Some(title) = first
                    .as_ref()
                    .and_then(|first| items::title(&first.plain()))
                {
                    thread.title = Some(title);
                }
            }
        }
        fx.results.push((key, ActionOutcome::Started { agent }));
        self.replay_kept(&id, fx);
        if let Some(first) = first {
            let message = Outgoing::new(first, None, Timestamp::now());
            self.turn_start(&id, message, fx);
        }
    }

    /// The reply to `thread/resume`.
    #[allow(clippy::too_many_arguments)]
    fn resumed(
        &mut self,
        id: &str,
        outcome: Result<Value, RpcError>,
        key: Option<IdempotencyKey>,
        place: Option<Place>,
        new: bool,
        history: bool,
        fx: &mut Effects,
    ) {
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                let reason = if error.message.contains(OTHER_WRITER) {
                    "This thread is open in another Codex. Close it there first.".to_owned()
                } else {
                    format!("Codex couldn't resume the thread: {}", error.message)
                };
                self.resume_failed(id, key, new, &reason, fx);
                return;
            }
        };
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if let Some(place) = place {
            thread.place = Some(place);
        }
        thread.loaded = true;
        thread.link = AgentLink::Live;
        thread.effective = policy::effective_in(&result).or(thread.effective);
        if let Some(value) = result.get("thread") {
            learn_details(thread, value);
        }
        if let Some(model) = result.get("model").and_then(Value::as_str) {
            thread.model = Some(model.to_owned());
        }
        if history {
            let call = Call::Turns {
                thread: id.to_owned(),
                key,
                items: Vec::new(),
                active: None,
            };
            self.rpc
                .send("thread/turns/list", turns_params(id, None), call, fx);
            return;
        }
        self.resume_done(id, key, fx);
    }

    /// A resume didn't work: refuse the action, fail what waited on it and
    /// forget a thread that was only being adopted.
    fn resume_failed(
        &mut self,
        id: &str,
        key: Option<IdempotencyKey>,
        new: bool,
        reason: &str,
        fx: &mut Effects,
    ) {
        if let Some(key) = key {
            refused(fx, key, reason);
        }
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        thread.resuming = false;
        let buffered = thread.buffer.take();
        if new {
            let agent = thread.agent;
            self.threads.remove(id);
            self.agents.remove(&agent);
            return;
        }
        for message in thread.queue.drain(..) {
            upsert(fx, message.item(thread.agent, ItemStatus::Failed));
        }
        for (text, seen) in buffered {
            self.take(&text, seen, fx);
        }
    }

    /// One page of a thread's turns, read as history once the last page is
    /// in.
    fn turns_page(
        &mut self,
        id: &str,
        outcome: Result<Value, RpcError>,
        key: Option<IdempotencyKey>,
        mut read: Vec<TimelineItem>,
        mut active: Option<String>,
        fx: &mut Effects,
    ) {
        let Some(agent) = self.threads.get(id).map(|thread| thread.agent) else {
            return;
        };
        let pictures = self
            .threads
            .get(id)
            .and_then(|thread| thread.place.as_ref())
            .and_then(Pictures::at);
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                tracing::debug!(error = error.message, "codex thread/turns/list failed");
                Value::Null
            }
        };
        let mut settled = Vec::new();
        for turn in result
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let running = turn.get("status").and_then(Value::as_str) == Some("inProgress");
            if running {
                active = turn.get("id").and_then(Value::as_str).map(str::to_owned);
            }
            settled.extend(items::settled(turn, running));
            let kept = pictures.as_ref();
            read.extend(items::turn(
                agent,
                Some(self.login.account),
                turn,
                running,
                kept,
                &mut fx.pictures,
            ));
        }
        if let Some(thread) = self.threads.get_mut(id) {
            thread.settled.extend(settled);
        }
        if let Some(cursor) = result.get("nextCursor").and_then(Value::as_str) {
            let call = Call::Turns {
                thread: id.to_owned(),
                key,
                items: read,
                active,
            };
            self.rpc.send(
                "thread/turns/list",
                turns_params(id, Some(cursor)),
                call,
                fx,
            );
            return;
        }
        // A message sent from here stays the item it showed as, so the
        // history read again doesn't show it twice.
        read.iter_mut().for_each(|item| self.locals.keep(item));
        fx.history.push((agent, read));
        if let Some(turn) = active
            && let Some(thread) = self.threads.get_mut(id)
            && !thread.busy()
        {
            thread.run(&turn, self.seq);
        }
        self.resume_done(id, key, fx);
    }

    /// A resume is done: run the lines that waited and what's queued.
    fn resume_done(&mut self, id: &str, key: Option<IdempotencyKey>, fx: &mut Effects) {
        if let Some(thread) = self.threads.get_mut(id) {
            thread.resuming = false;
        }
        if let Some(key) = key {
            done(fx, key);
        }
        self.replay_kept(id, fx);
        if let Some(thread) = self.threads.get_mut(id) {
            thread.settled.clear();
        }
        self.flush(id, fx);
    }

    /// The reply to `turn/start`.
    fn turn_started(
        &mut self,
        id: &str,
        client_id: &str,
        outcome: Result<Value, RpcError>,
        fx: &mut Effects,
    ) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let index = thread.sent.iter().position(|m| m.client_id == client_id);
        match outcome {
            Ok(result) => {
                if let Some(key) = index.and_then(|index| thread.sent[index].key.take()) {
                    done(fx, key);
                }
                let turn = result.pointer("/turn/id").and_then(Value::as_str);
                if let Some(turn) = turn
                    && thread.turn == Turn::Starting
                {
                    let turn = turn.to_owned();
                    self.turn_known(id, &turn, fx);
                }
            }
            Err(error) => {
                if thread.turn == Turn::Starting {
                    thread.turn = Turn::Idle;
                }
                let Some(mut message) = index.and_then(|index| thread.sent.remove(index)) else {
                    return;
                };
                if error.message.contains("not loaded") && message.key.is_none() {
                    thread.loaded = false;
                    message.delivery = Delivery::Queued;
                    thread.queue.push_front(message);
                    self.flush(id, fx);
                    return;
                }
                upsert(fx, message.item(thread.agent, ItemStatus::Failed));
                if let Some(key) = message.key {
                    refused(fx, key, &error.message);
                }
                let steers = std::mem::take(&mut thread.steers);
                for message in steers.into_iter().rev() {
                    thread.queue.push_front(message);
                }
                self.flush(id, fx);
            }
        }
    }

    /// The reply to `turn/steer`.
    fn steered(
        &mut self,
        id: &str,
        client_id: &str,
        retried: bool,
        outcome: Result<Value, RpcError>,
        fx: &mut Effects,
    ) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let Some(index) = thread.sent.iter().position(|m| m.client_id == client_id) else {
            return;
        };
        let error = match outcome {
            Ok(_) => {
                if let Some(key) = thread.sent[index].key.take() {
                    done(fx, key);
                }
                return;
            }
            Err(error) => error,
        };
        if !retried && let Some(found) = found_turn(&error.message) {
            thread.run(&found, self.seq);
            self.steer(id, client_id, &found, true, fx);
            return;
        }
        let Some(mut message) = thread.sent.remove(index) else {
            return;
        };
        let lower = error.message.to_ascii_lowercase();
        if lower.contains("no active turn") || lower.contains("not steerable") {
            if let Some(key) = message.key.take() {
                done(fx, key);
            }
            if thread.busy() || lower.contains("not steerable") {
                message.delivery = Delivery::Queued;
                upsert(fx, message.item(thread.agent, ItemStatus::Pending));
                thread.queue.push_back(message);
                self.flush(id, fx);
            } else {
                self.turn_start(id, message, fx);
            }
            return;
        }
        upsert(fx, message.item(thread.agent, ItemStatus::Failed));
        if let Some(key) = message.key {
            refused(fx, key, &error.message);
        }
    }

    /// An interrupt found no turn. It may not have started on Codex's side
    /// yet, so try again a little later, and after the last try take the
    /// turn as over.
    fn interrupt_failed(
        &mut self,
        id: &str,
        turn: &str,
        attempt: usize,
        error: &RpcError,
        fx: &mut Effects,
    ) {
        if !error
            .message
            .to_ascii_lowercase()
            .contains("no active turn")
        {
            tracing::debug!(error = error.message, "codex turn/interrupt failed");
            return;
        }
        if let Some(delay) = RETRY_MS.get(attempt) {
            let timer = Timer::Retry {
                thread: id.to_owned(),
                turn: turn.to_owned(),
                attempt,
            };
            self.timers
                .push((Instant::now() + Duration::from_millis(*delay), timer));
            return;
        }
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        if thread.active_turn() == Some(turn) {
            thread.turn = Turn::Idle;
            self.flush(id, fx);
        }
    }
}

/// Whether a thread's files are on this device. One whose place isn't known
/// yet counts as not, since nothing here should open a guest's paths.
fn on_this_device(thread: &Thread) -> bool {
    thread
        .place
        .as_ref()
        .is_some_and(|place| place.host == PlaceHost::Local)
}

/// The settings a thread in `place` runs with in `mode`: the read-only
/// ones in the dispatcher's folder, whatever the mode, or else the mode's.
fn policy_in(place: Option<&Place>, mode: Option<PermissionMode>) -> Option<policy::Policy> {
    if place.is_some_and(|place| place.dispatcher) {
        return Some(policy::READ_ONLY);
    }
    mode.and_then(policy::policy)
}

/// Gives the dispatcher's thread what it reads about its part.
fn add_instructions(place: Option<&Place>, params: &mut Map<String, Value>) {
    if place.is_some_and(|place| place.dispatcher) {
        params.insert(
            "developerInstructions".into(),
            dispatch::instructions().into(),
        );
    }
}

/// Tells Codex whether the folder's own files load, at each folder it
/// looks trust up at. This holds for the thread alone and writes nothing
/// to Codex's config, and it wins over a project Codex trusts there.
fn add_trust(place: Option<&Place>, params: &mut Map<String, Value>) {
    let Some(trust) = place.and_then(|place| place.trust.as_ref()) else {
        return;
    };
    let level = if trust.trusted {
        "trusted"
    } else {
        "untrusted"
    };
    let projects: Map<String, Value> = trust
        .paths
        .iter()
        .map(|path| (path.clone(), json!({ "trust_level": level })))
        .collect();
    params.insert("config".into(), json!({ "projects": projects }));
}

/// The parameters for one page of a thread's turns, oldest first.
fn turns_params(thread: &str, cursor: Option<&str>) -> Value {
    let mut params = rpc::object([
        ("threadId", thread.into()),
        ("sortDirection", "asc".into()),
        ("itemsView", "full".into()),
    ]);
    if let (Some(cursor), Some(map)) = (cursor, params.as_object_mut()) {
        map.insert("cursor".into(), cursor.into());
    }
    params
}

/// The turn Codex named in "expected active turn id `a` but found `b`".
fn found_turn(message: &str) -> Option<String> {
    let (_, rest) = message.split_once("but found `")?;
    let (turn, _) = rest.split_once('`')?;
    (!turn.is_empty()).then(|| turn.to_owned())
}

/// The effort level a thread's next turn runs at: the one you picked, or
/// else the default of the model it runs, from `catalog`. Codex keeps a
/// turn's level for the turns after it, so naming the default is how a
/// level you cleared goes back to it. None when nothing is known.
fn effort_for(thread: &Thread, catalog: &[ModelChoice]) -> Option<String> {
    if let Some(effort) = &thread.effort {
        return Some(effort.clone());
    }
    let model = thread.model.as_deref();
    catalog
        .iter()
        .find(|choice| match model {
            Some(model) => choice.id == model,
            None => choice.default,
        })
        .and_then(|choice| choice.default_effort.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_runs_at_its_effort_or_its_models_default() {
        let choice = |id: &str, default: bool, effort: Option<&str>| ModelChoice {
            id: id.into(),
            label: id.into(),
            default,
            efforts: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: effort.map(str::to_owned),
            no_images: false,
        };
        let catalog = [
            choice("gpt-sol", true, Some("medium")),
            choice("gpt-luna", false, Some("low")),
        ];
        let mut thread = Thread::new("thread".into(), AgentId::derive("agent"), Timestamp::now());
        // No model named: the default model's default.
        assert_eq!(effort_for(&thread, &catalog).as_deref(), Some("medium"));
        thread.model = Some("gpt-luna".into());
        assert_eq!(effort_for(&thread, &catalog).as_deref(), Some("low"));
        thread.effort = Some("high".into());
        assert_eq!(effort_for(&thread, &catalog).as_deref(), Some("high"));
        // Nothing known: Codex keeps what it has.
        thread.effort = None;
        assert_eq!(effort_for(&thread, &[]), None);
    }

    #[test]
    fn the_found_turn_comes_from_the_error() {
        let message = "expected active turn id `not-the-turn` but found `01a1-real`";
        assert_eq!(found_turn(message).as_deref(), Some("01a1-real"));
        assert_eq!(found_turn("no active turn to steer"), None);
    }
}
