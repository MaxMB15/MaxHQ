// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the app-server says about its threads, turned into timeline
//! events and agent state.

use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    Delivery, ItemBody, ItemId, ItemStatus, NoticeLevel, TextField, TimelineItem, Todo, TodoState,
    TurnId,
};
use serde_json::{json, Value};

use super::thread::{Echo, Open, Turn};
use super::{
    append, apply_status, close, context_usage, end_wait, items, policy, remove, rpc, upsert,
    CodexServer, Seen, Timer,
};
use crate::drive::pictures::Pictures;
use crate::drive::{AttentionChange, Effects};
use crate::usage::codex::limits;

impl CodexServer {
    /// Acts on a notification from the app-server.
    pub(super) fn notification(
        &mut self,
        method: &str,
        params: Value,
        emitted: Option<Timestamp>,
        seen: Seen,
        fx: &mut Effects,
    ) {
        let at = emitted.unwrap_or(seen.at);
        match method {
            "thread/started" => {
                if let Some(value) = params.get("thread")
                    && let Some(id) = self.learn_thread(value, at)
                {
                    self.replay_kept(&id, fx);
                }
            }
            "account/rateLimits/updated" => {
                // A push names one limit, so the others keep their reading.
                let mut reading = limits::from_app_server(&params, at);
                reading.complete = false;
                self.take_limits(&reading, fx);
            }
            _ => {
                let Some(id) = text(&params, "threadId") else {
                    tracing::debug!(method, "codex notification without a thread");
                    return;
                };
                if self.threads.contains_key(&id) {
                    self.thread_event(&id, method, &params, at, fx);
                }
            }
        }
    }

    /// Acts on a notification about one known thread.
    fn thread_event(
        &mut self,
        id: &str,
        method: &str,
        params: &Value,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        match method {
            "thread/status/changed" => {
                let unloaded =
                    params.pointer("/status/type").and_then(Value::as_str) == Some("notLoaded");
                if let Some(thread) = self.threads.get_mut(id)
                    && let Some(status) = params.get("status")
                {
                    apply_status(thread, status);
                }
                if unloaded {
                    self.unloaded(id, at, fx);
                }
            }
            "thread/closed" | "thread/archived" => self.unloaded(id, at, fx),
            "thread/name/updated" => {
                let name = text(params, "threadName").filter(|name| !name.is_empty());
                if let Some(thread) = self.threads.get_mut(id)
                    && !thread.titled
                    && let Some(name) = name
                {
                    thread.title = Some(name);
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(thread) = self.threads.get_mut(id)
                    && let Some(usage) = params.get("tokenUsage").and_then(context_usage)
                {
                    thread.context = Some(usage);
                }
            }
            "thread/settings/updated" => {
                let settings = params.get("threadSettings").unwrap_or(&Value::Null);
                if let Some(thread) = self.threads.get_mut(id) {
                    thread.effective = policy::effective_in(settings).or(thread.effective);
                    if let Some(model) = text(settings, "model") {
                        thread.model = Some(model);
                    }
                }
            }
            "turn/started" => self.turn_began(id, params, at, fx),
            "turn/completed" => self.turn_ended(id, params, at, fx),
            "turn/plan/updated" => self.plan_updated(id, params, at, fx),
            "model/rerouted" => self.rerouted(id, params, at, fx),
            "warning" => self.warning(id, params, at, fx),
            "error" => self.turn_error(id, params, at, fx),
            "item/started" => self.item_started(id, params, at, fx),
            "item/completed" => self.item_completed(id, params, at, fx),
            "item/agentMessage/delta" | "item/plan/delta" => {
                self.text_delta(id, params, TextField::Text, fx)
            }
            "item/commandExecution/outputDelta" => {
                self.text_delta(id, params, TextField::Output, fx)
            }
            "item/reasoning/summaryTextDelta" => self.reasoning_delta(id, params, fx),
            "item/reasoning/summaryPartAdded" => self.reasoning_part(id, params, fx),
            "item/fileChange/patchUpdated" => self.patch_updated(id, params, fx),
            "serverRequest/resolved" => self.resolved(id, params, at, fx),
            _ => tracing::debug!(method, "codex notification the driver doesn't use"),
        }
    }

    /// The app-server let a thread go. Items still open stop where they
    /// are, and the next message resumes it.
    fn unloaded(&mut self, id: &str, at: Timestamp, fx: &mut Effects) {
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        thread.loaded = false;
        let turn = thread.active_turn().map(str::to_owned);
        thread.turn = Turn::Idle;
        let mut open: Vec<Open> = thread.open.drain().map(|(_, open)| open).collect();
        open.sort_by(|a, b| a.item.id.0.cmp(&b.item.id.0));
        for mut open in open.into_iter().filter(|open| open.shown) {
            close(&mut open.item, ItemStatus::Interrupted, Some(at));
            upsert(fx, open.item);
        }
        if let Some(turn) = turn {
            upsert(fx, items::interrupted(thread.agent, &turn, Some(at)));
            thread.ended_at = Some(at);
            thread.touch(at);
        }
    }

    /// A turn started.
    fn turn_began(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(turn) = params.pointer("/turn/id").and_then(Value::as_str) else {
            return;
        };
        let seq = self.seq;
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        thread.errored = None;
        thread.touch(at);
        thread.ended_at = None;
        thread.cold = false;
        thread.loaded = true;
        if thread.limited.is_some() {
            end_wait(thread, at, fx);
        }
        let known = thread.active_turn() == Some(turn);
        if known {
            // Read again after a restart, the turn starts here at the latest.
            thread.run(turn, seq);
        }
        self.timers
            .retain(|(_, timer)| !matches!(timer, Timer::Cold { thread } if thread == id));
        if !known {
            self.turn_known(id, turn, fx);
        }
    }

    /// A turn ended. Items still open close with the turn, prompts it
    /// raised go away, its closing marker goes on the timeline and the
    /// next queued message goes out.
    fn turn_ended(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(value) = params.get("turn") else {
            return;
        };
        let Some(turn) = text(value, "id") else {
            return;
        };
        let status = text(value, "status").unwrap_or_else(|| "completed".into());
        let error = value.get("error").cloned().unwrap_or(Value::Null);
        if status == "failed" && items::limit_hit(&error) {
            let limited = self.threads.get(id).is_some_and(|thread| {
                thread
                    .limit_item
                    .as_ref()
                    .is_some_and(|item| item.id == items::error_id(&turn))
            });
            if !limited {
                self.limit_reached(id, &turn, at, fx);
            }
        }
        self.drop_prompts(id, &turn, at, fx);
        let started_at = secs(value, "startedAt");
        let duration = value.get("durationMs").and_then(Value::as_u64);
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let agent = thread.agent;
        let closing = match status.as_str() {
            "interrupted" => ItemStatus::Interrupted,
            "failed" => ItemStatus::Failed,
            _ => ItemStatus::Completed,
        };
        let mut keys: Vec<String> = thread
            .open
            .iter()
            .filter(|(_, open)| open.item.turn.as_ref().is_some_and(|t| t.0 == turn))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        for key in keys {
            let Some(mut open) = thread.open.remove(&key) else {
                continue;
            };
            if !open.shown {
                continue;
            }
            // Codex leaves a command running when the turn stops and sends
            // its completion later, so it stays open as a background
            // command until then.
            if closing != ItemStatus::Completed
                && open.kind == "commandexecution"
                && let ItemBody::Command { background, .. } = &mut open.item.body
            {
                *background = true;
                upsert(fx, open.item.clone());
                thread.open.insert(key, open);
                continue;
            }
            close(&mut open.item, closing, Some(at));
            upsert(fx, open.item);
        }
        let marker = match status.as_str() {
            "interrupted" => Some(items::interrupted(agent, &turn, Some(at))),
            "failed" if items::limit_hit(&error) => None,
            "failed" => Some(items::failure(
                agent,
                Some(self.login.account),
                &turn,
                &error,
                None,
                false,
                Some(at),
            )),
            _ => Some(items::summary(
                agent,
                &turn,
                duration,
                thread.context.map(|usage| u64::from(usage.used_tokens)),
                started_at,
                Some(at),
            )),
        };
        if let Some(marker) = marker {
            upsert(fx, marker);
        }
        thread.echoes.clear();
        thread.interrupt_wanted = false;
        thread.ended_at = Some(at);
        thread.touch(at);
        if thread.active_turn() == Some(turn.as_str()) {
            thread.turn = Turn::Idle;
        }
        if thread.turn == Turn::Idle {
            let steers = std::mem::take(&mut thread.steers);
            for mut message in steers.into_iter().rev() {
                message.delivery = Delivery::Queued;
                upsert(fx, message.item(agent, ItemStatus::Pending));
                thread.queue.push_front(message);
            }
        }
        // Messages queued during the turn go once the limit resets.
        let queued = !thread.queue.is_empty();
        if let Some(item) = &mut thread.limit_item
            && let ItemBody::LimitWait { auto_resume, .. } = &mut item.body
            && *auto_resume != queued
        {
            *auto_resume = queued;
            upsert(fx, item.clone());
        }
        self.timers
            .retain(|(_, timer)| !matches!(timer, Timer::Retry { thread, .. } if thread == id));
        self.arm_cold(id, at);
        self.flush(id, fx);
    }

    /// Closes the prompts a turn raised that nobody answered.
    fn drop_prompts(&mut self, id: &str, turn: &str, at: Timestamp, fx: &mut Effects) {
        let mut ended: Vec<_> = self
            .prompts
            .iter()
            .filter(|(_, prompt)| prompt.thread == id && prompt.turn.as_deref() == Some(turn))
            .map(|(attention, prompt)| (prompt.event.created_at, *attention))
            .collect();
        ended.sort();
        for (_, attention) in ended {
            let Some(prompt) = self.prompts.remove(&attention) else {
                continue;
            };
            self.pending.remove(&prompt.key);
            let mut item = prompt.item;
            item.status = ItemStatus::Cancelled;
            item.completed_at = Some(at);
            upsert(fx, item);
            fx.attention.push(AttentionChange::Close(attention));
        }
    }

    /// The account ran out during a turn: the turn shows a wait for the
    /// reset, and queued messages hold until then.
    fn limit_reached(&mut self, id: &str, turn: &str, at: Timestamp, fx: &mut Effects) {
        let resets = self.resets_at();
        let account = self.login.account;
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let limit = Some((Some(account), resets));
        let mut item = items::failure(
            thread.agent,
            Some(account),
            turn,
            &Value::Null,
            limit,
            false,
            Some(at),
        );
        if let ItemBody::LimitWait { auto_resume, .. } = &mut item.body {
            *auto_resume = !thread.queue.is_empty();
        }
        // The wait stays open until the limit lets go.
        item.status = ItemStatus::Running;
        item.completed_at = None;
        thread.limited = Some(resets);
        thread.limit_item = Some(item.clone());
        upsert(fx, item);
        self.read_limits(fx);
    }

    /// An error during a turn.
    fn turn_error(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let error = params.get("error").cloned().unwrap_or(Value::Null);
        let Some(turn) = text(params, "turnId") else {
            return;
        };
        if items::limit_hit(&error) {
            self.limit_reached(id, &turn, at, fx);
            return;
        }
        let retrying = params.get("willRetry").and_then(Value::as_bool) == Some(true);
        let account = Some(self.login.account);
        if let Some(thread) = self.threads.get(id) {
            let item = items::failure(
                thread.agent,
                account,
                &turn,
                &error,
                None,
                retrying,
                Some(at),
            );
            upsert(fx, item);
        }
    }

    /// The turn's plan changed. It shows as the turn's to-do list.
    fn plan_updated(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(turn) = text(params, "turnId") else {
            return;
        };
        let Some(thread) = self.threads.get(id) else {
            return;
        };
        let todos = params
            .get("plan")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|step| {
                let state = match text(step, "status").as_deref() {
                    Some("completed") => TodoState::Done,
                    Some("inProgress") => TodoState::InProgress,
                    _ => TodoState::Pending,
                };
                Some(Todo {
                    text: text(step, "step")?,
                    state,
                })
            })
            .collect();
        upsert(
            fx,
            TimelineItem {
                id: ItemId(format!("codex:todos:{turn}")),
                agent: thread.agent,
                turn: Some(TurnId(turn)),
                parent: None,
                ordinal: 0,
                status: ItemStatus::Completed,
                started_at: Some(at),
                completed_at: Some(at),
                body: ItemBody::Todos { todos },
            },
        );
    }

    /// Codex moved the turn to another model.
    fn rerouted(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(turn) = text(params, "turnId") else {
            return;
        };
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let to = text(params, "toModel");
        let message = match (text(params, "fromModel"), &to) {
            (Some(from), Some(to)) => format!("Codex switched from {from} to {to}."),
            (None, Some(to)) => format!("Codex switched to {to}."),
            _ => "Codex switched models.".to_owned(),
        };
        if let Some(to) = to {
            thread.model = Some(to);
        }
        let item = notice(
            thread.agent,
            ItemId(format!("codex:rerouted:{turn}")),
            Some(turn),
            NoticeLevel::Info,
            message,
            at,
        );
        upsert(fx, item);
    }

    /// A warning about a thread.
    fn warning(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(message) = text(params, "message") else {
            return;
        };
        let Some(agent) = self.threads.get(id).map(|thread| thread.agent) else {
            return;
        };
        // The line's seq keeps the ID when the line is read again.
        let item_id = ItemId(format!("codex:warning:{}:{}", at.0, self.seq));
        let item = notice(agent, item_id, None, NoticeLevel::Warning, message, at);
        upsert(fx, item);
    }

    /// An item started.
    fn item_started(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(native) = params
            .get("item")
            .and_then(|item| items::native(item, false))
        else {
            return;
        };
        // An item a history read found finished stays as read when its
        // lines come again after a restart. A user message still goes
        // through, so it lands on its local item.
        if native.kind != "usermessage"
            && self
                .threads
                .get(id)
                .is_some_and(|thread| thread.settled.contains(&native.id))
        {
            return;
        }
        let turn = text(params, "turnId");
        let started_at = millis(params, "startedAtMs").unwrap_or(at);
        let echo = if native.kind == "usermessage" {
            self.echo_for(id, &native.id, native.client_id.as_deref(), fx)
        } else {
            None
        };
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let key = native.id.clone();
        let kind = native.kind.clone();
        let mut item = items::timeline(
            thread.agent,
            turn.as_deref(),
            native,
            Some(started_at),
            None,
        );
        if let Some(echo) = &echo {
            apply_echo(&mut item, echo);
        }
        self.locals.keep(&mut item);
        let shown = !matches!(&item.body, ItemBody::Reasoning { text, .. } if text.is_empty());
        if shown {
            upsert(fx, item.clone());
        }
        thread.open.insert(key, Open { kind, item, shown });
    }

    /// An item finished. What it says now replaces whatever streamed.
    fn item_completed(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(native) = params
            .get("item")
            .and_then(|item| items::native(item, true))
        else {
            return;
        };
        if native.kind != "usermessage"
            && self.threads.get(id).is_some_and(|thread| {
                thread.settled.contains(&native.id) && !thread.open.contains_key(&native.id)
            })
        {
            return;
        }
        let turn = text(params, "turnId");
        let completed_at = millis(params, "completedAtMs").unwrap_or(at);
        let echo = if native.kind == "usermessage" {
            self.echo_for(id, &native.id, native.client_id.as_deref(), fx)
        } else {
            None
        };
        let Some(thread) = self.threads.get_mut(id) else {
            return;
        };
        let agent = thread.agent;
        let open = thread.open.remove(&native.id);
        thread.echoes.remove(&native.id);
        if matches!(&native.body, ItemBody::Reasoning { text, .. } if text.is_empty()) {
            if let Some(open) = open
                && open.shown
            {
                remove(fx, agent, open.item.id);
            }
            return;
        }
        let started_at = open
            .as_ref()
            .and_then(|open| open.item.started_at)
            .unwrap_or(completed_at);
        let mut item = items::timeline(
            agent,
            turn.as_deref(),
            native,
            Some(started_at),
            Some(completed_at),
        );
        if let Some(echo) = &echo {
            apply_echo(&mut item, echo);
        }
        self.locals.keep(&mut item);
        if let Some(raw) = params.get("item") {
            let kept = thread.place.as_ref().and_then(Pictures::at);
            items::keep_pictures(raw, &mut item.body, agent, kept.as_ref(), &mut fx.pictures);
        }
        if let Some(open) = &open
            && let ItemBody::Command {
                output: streamed,
                background: was_background,
                ..
            } = &open.item.body
            && let ItemBody::Command {
                output, background, ..
            } = &mut item.body
        {
            // A command that outlived its turn completes without its output.
            if output.is_empty() {
                output.clone_from(streamed);
            }
            *background |= *was_background;
        }
        upsert(fx, item);
        if let Some(raw) = params.get("item") {
            self.show_sent(agent, turn.as_deref(), raw, completed_at, fx);
        }
    }

    /// What a user message Codex sent back shows: the message as you typed
    /// it, matched by its client ID or else the oldest one sent. Codex's
    /// copy then updates the local item that stood in for it, which keeps
    /// its ID and its place.
    fn echo_for(
        &mut self,
        id: &str,
        item: &str,
        client_id: Option<&str>,
        fx: &mut Effects,
    ) -> Option<Echo> {
        let thread = self.threads.get_mut(id)?;
        if let Some(echo) = thread.echoes.get(item) {
            return Some(echo.clone());
        }
        let mut message = thread.take_sent(client_id)?;
        self.locals
            .insert(items::item_id(item), message.local.clone());
        if let Some(key) = message.key.take() {
            super::done(fx, key);
        }
        let echo = message.echo();
        thread.echoes.insert(item.to_owned(), echo.clone());
        Some(echo)
    }

    /// Text streamed into a message, a plan or a command's output.
    fn text_delta(&mut self, id: &str, params: &Value, field: TextField, fx: &mut Effects) {
        let (Some(item), Some(delta)) = (text(params, "itemId"), text(params, "delta")) else {
            return;
        };
        let Some(open) = self
            .threads
            .get_mut(id)
            .and_then(|thread| thread.open.get_mut(&item))
        else {
            return;
        };
        match (&mut open.item.body, field) {
            (
                ItemBody::AssistantMessage { text, .. } | ItemBody::Plan { markdown: text, .. },
                TextField::Text,
            ) => text.push_str(&delta),
            (ItemBody::Command { output, .. }, TextField::Output) => output.push_str(&delta),
            _ => return,
        }
        grow(fx, open, field, &delta);
    }

    /// Text streamed into a reasoning summary. The reasoning shows from its
    /// first text on.
    fn reasoning_delta(&mut self, id: &str, params: &Value, fx: &mut Effects) {
        let (Some(item), Some(delta)) = (text(params, "itemId"), text(params, "delta")) else {
            return;
        };
        let Some(open) = self
            .threads
            .get_mut(id)
            .and_then(|thread| thread.open.get_mut(&item))
        else {
            return;
        };
        let ItemBody::Reasoning { text, .. } = &mut open.item.body else {
            return;
        };
        text.push_str(&delta);
        grow(fx, open, TextField::Text, &delta);
    }

    /// A new part of a reasoning summary starts, after a blank line.
    fn reasoning_part(&mut self, id: &str, params: &Value, fx: &mut Effects) {
        let index = params.get("summaryIndex").and_then(Value::as_u64);
        let Some(item) = text(params, "itemId") else {
            return;
        };
        let Some(open) = self
            .threads
            .get_mut(id)
            .and_then(|thread| thread.open.get_mut(&item))
        else {
            return;
        };
        let ItemBody::Reasoning { text, .. } = &mut open.item.body else {
            return;
        };
        if index.unwrap_or(0) == 0 || text.is_empty() {
            return;
        }
        text.push_str("\n\n");
        grow(fx, open, TextField::Text, "\n\n");
    }

    /// A file change's patch grew while Codex wrote it.
    fn patch_updated(&mut self, id: &str, params: &Value, fx: &mut Effects) {
        let Some(item) = text(params, "itemId") else {
            return;
        };
        let changes = params.get("changes").cloned().unwrap_or(Value::Null);
        let shape = json!({ "type": "fileChange", "id": item, "changes": changes });
        let Some(native) = items::native(&shape, false) else {
            return;
        };
        let Some(open) = self
            .threads
            .get_mut(id)
            .and_then(|thread| thread.open.get_mut(&item))
        else {
            return;
        };
        if matches!(open.item.body, ItemBody::FileChange { .. }) {
            open.item.body = native.body;
            open.shown = true;
            upsert(fx, open.item.clone());
        }
    }

    /// Codex no longer waits on one of its requests. One the driver still
    /// shows went away unanswered, such as when its turn stopped.
    fn resolved(&mut self, id: &str, params: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(request) = params.get("requestId") else {
            return;
        };
        if let Some(key) = rpc::key(request) {
            self.pending.remove(&key);
        }
        let found = self
            .prompts
            .iter()
            .find(|(_, prompt)| prompt.thread == id && &prompt.id == request)
            .map(|(attention, _)| *attention);
        let Some(prompt) = found.and_then(|attention| self.prompts.remove(&attention)) else {
            return;
        };
        let mut item = prompt.item;
        item.status = ItemStatus::Cancelled;
        item.completed_at = Some(at);
        upsert(fx, item);
        fx.attention.push(AttentionChange::Close(prompt.event.id));
    }
}

/// Sends text that grew an open item: as an append once the item shows,
/// or as the whole item the first time.
fn grow(fx: &mut Effects, open: &mut Open, field: TextField, delta: &str) {
    if open.shown {
        append(fx, open.item.agent, open.item.id.clone(), field, delta);
    } else {
        open.shown = true;
        upsert(fx, open.item.clone());
    }
}

/// Shows a user message as you typed it.
fn apply_echo(item: &mut TimelineItem, echo: &Echo) {
    if let ItemBody::UserMessage {
        text,
        attachments,
        refs,
        delivery,
    } = &mut item.body
    {
        text.clone_from(&echo.text);
        attachments.clone_from(&echo.attachments);
        refs.clone_from(&echo.refs);
        *delivery = echo.delivery;
    }
}

/// A notice on `agent`'s timeline.
fn notice(
    agent: maxhq_protocol::ids::AgentId,
    id: ItemId,
    turn: Option<String>,
    level: NoticeLevel,
    message: String,
    at: Timestamp,
) -> TimelineItem {
    TimelineItem {
        id,
        agent,
        turn: turn.map(TurnId),
        parent: None,
        ordinal: 0,
        status: ItemStatus::Completed,
        started_at: Some(at),
        completed_at: Some(at),
        body: ItemBody::Notice { level, message },
    }
}

/// A string field.
fn text(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_owned)
}

/// A time Codex gives in milliseconds.
fn millis(value: &Value, name: &str) -> Option<Timestamp> {
    value.get(name).and_then(Value::as_u64).map(Timestamp)
}

/// A time Codex gives in seconds.
fn secs(value: &Value, name: &str) -> Option<Timestamp> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .map(|secs| Timestamp(secs.saturating_mul(1000)))
}
