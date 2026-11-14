// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A Codex agent's history, read from its rollout file.
//!
//! Codex writes each finished item to the rollout as an `item_completed`
//! event, with the same ID the app-server gives it, so items read here and
//! items the driver streams land on the same timeline items. Turns close
//! with `task_complete` or `turn_aborted`. Rollouts from releases that
//! wrote no whole items fall back to their `user_message` and
//! `agent_message` events. A message sent from here earlier in this
//! process keeps the local item it showed as.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use maxhq_core::context;
use maxhq_protocol::ids::{AccountId, AgentId};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemId, ItemStatus, TimelineItem, TurnId};
use serde_json::Value;

use super::items;
use super::locals::Locals;
use crate::drive::pictures::Pictures;
use crate::observe::claude::time::parse_iso8601;
use crate::observe::codex::rollout::{thread_id, Tail, Tailed};

/// How deep the search for a rollout goes. Codex files them under
/// `sessions/<year>/<month>/<day>`.
const DEPTH: usize = 4;

/// Reads one rollout a little more each call.
#[derive(Debug)]
pub struct History {
    /// The rollout.
    path: PathBuf,
    /// Where the last read stopped.
    tail: Tail,
    /// What the lines so far said.
    state: State,
    /// The local items of messages sent from here.
    pub(super) locals: Locals,
}

/// What the lines read so far said.
#[derive(Debug)]
struct State {
    /// The agent the items belong to.
    agent: AgentId,
    /// The running turn.
    turn: Option<String>,
    /// When each turn not yet closed started.
    started: HashMap<String, Timestamp>,
    /// The input tokens of the last model call, for the next summary.
    context: Option<u64>,
    /// Whether the rollout writes whole items, so the older message events
    /// are left out.
    whole_items: bool,
    /// The number of the line being read, from 1.
    line: u64,
    /// Where the pictures MCP tools gave back are kept, if anywhere.
    pictures: Option<Pictures>,
    /// The account the thread runs under, which an error from a login that
    /// ran out names, when the reader knows it.
    account: Option<AccountId>,
}

impl History {
    /// A reader for the rollout at `path`, whose items go on `agent`'s
    /// timeline.
    pub fn new(path: PathBuf, agent: AgentId) -> Self {
        Self {
            path,
            tail: Tail::default(),
            state: State::new(agent, None, None),
            locals: Locals::shared(),
        }
    }

    /// Keeps the pictures MCP tools gave back as files under `root`, the
    /// attachments root, so a read after a restart shows them again.
    pub fn keeping_pictures_in(mut self, root: PathBuf) -> Self {
        self.state.pictures = Some(Pictures::here(root));
        self
    }

    /// A reader for `thread`'s rollout under `config_dir`, the account's
    /// `CODEX_HOME`, looking in `sessions` and then `archived_sessions`.
    /// Returns `None` when there's no rollout for it.
    pub fn open(config_dir: &Path, thread: &str, agent: AgentId) -> Option<Self> {
        ["sessions", "archived_sessions"]
            .iter()
            .find_map(|dir| find(&config_dir.join(dir), thread, DEPTH))
            .map(|path| Self::new(path, agent))
    }

    /// Names `account` in errors from a login that ran out, as the live
    /// thread does.
    pub fn set_account(&mut self, account: AccountId) {
        self.state.account = Some(account);
    }

    /// The rollout it reads.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The items that are new or changed since the last call, whole. A
    /// rollout that was replaced is read again from its start.
    pub fn read(&mut self) -> io::Result<Vec<TimelineItem>> {
        let mut out = Vec::new();
        let Self {
            path,
            tail,
            state,
            locals,
        } = self;
        tail.read(path, |tailed| match tailed {
            Tailed::Restart => {
                *state = State::new(state.agent, state.pictures.take(), state.account);
            }
            Tailed::Line(text) => state.read_line(text, &mut out),
        })?;
        out.iter_mut().for_each(|item| locals.keep(item));
        Ok(out)
    }
}

impl State {
    /// Nothing read yet.
    fn new(agent: AgentId, pictures: Option<Pictures>, account: Option<AccountId>) -> Self {
        Self {
            agent,
            turn: None,
            started: HashMap::new(),
            context: None,
            whole_items: false,
            line: 0,
            pictures,
            account,
        }
    }

    /// Reads one line, adding what it shows to `out`.
    fn read_line(&mut self, text: &str, out: &mut Vec<TimelineItem>) {
        self.line += 1;
        // Only events matter, and most lines are model input and output.
        if !text.contains("\"event_msg\"") {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            return;
        }
        let at = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_iso8601);
        let payload = &value["payload"];
        let turn = string(payload, "turn_id").or_else(|| self.turn.clone());
        match payload.get("type").and_then(Value::as_str) {
            Some("task_started" | "turn_started") => {
                if let Some(turn) = turn {
                    let started = secs(payload, "started_at").or(at);
                    if let Some(started) = started {
                        self.started.insert(turn.clone(), started);
                    }
                    self.turn = Some(turn);
                }
            }
            Some("item_completed") => {
                self.whole_items = true;
                self.item(payload, turn.as_deref(), at, out);
            }
            Some("token_count") => {
                if let Some(used) = payload
                    .pointer("/info/last_token_usage/input_tokens")
                    .and_then(Value::as_u64)
                {
                    self.context = Some(used);
                }
            }
            Some("task_complete" | "turn_complete") => {
                if let Some(turn) = turn {
                    self.finished(payload, &turn, at, out);
                }
            }
            Some("turn_aborted") => {
                if let Some(turn) = turn {
                    self.started.remove(&turn);
                    let at = secs(payload, "completed_at").or(at);
                    out.push(items::interrupted(self.agent, &turn, at));
                    self.end(&turn);
                }
            }
            Some("error") => {
                if let Some(turn) = turn {
                    out.push(self.failure(payload, &turn, at));
                }
            }
            Some("user_message") if !self.whole_items => {
                let images: Vec<String> = payload
                    .get("local_images")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let message = string(payload, "message").unwrap_or_default();
                // The event joins the message's text parts, labels and all.
                let message = context::without_labels(&message);
                let (text, attachments, refs) = items::typed_and_attached(message, &images);
                let body = ItemBody::UserMessage {
                    text,
                    attachments,
                    refs,
                    delivery: Delivery::Sent,
                };
                out.push(self.legacy(turn, body, at));
            }
            Some("agent_message") if !self.whole_items => {
                let body = ItemBody::AssistantMessage {
                    text: string(payload, "message").unwrap_or_default(),
                    streaming: false,
                };
                out.push(self.legacy(turn, body, at));
            }
            _ => {}
        }
    }

    /// A finished item.
    fn item(
        &self,
        payload: &Value,
        turn: Option<&str>,
        at: Option<Timestamp>,
        out: &mut Vec<TimelineItem>,
    ) {
        let Some(mut native) = payload
            .get("item")
            .and_then(|item| items::native(item, true))
        else {
            return;
        };
        if let ItemBody::Reasoning { text, .. } = &native.body
            && text.is_empty()
        {
            return;
        }
        if let Some(raw) = payload.get("item") {
            let pictures = self.pictures.as_ref();
            items::keep_pictures(
                raw,
                &mut native.body,
                self.agent,
                pictures,
                &mut Default::default(),
            );
        }
        let millis = |name: &str| payload.get(name).and_then(Value::as_u64).map(Timestamp);
        let completed_at = millis("completed_at_ms").or(at);
        let started_at = millis("started_at_ms").or(completed_at);
        out.push(items::timeline(
            self.agent,
            turn,
            native,
            started_at,
            completed_at,
        ));
        if let Some(item) = payload.get("item") {
            out.extend(items::sent(self.agent, turn, item, completed_at));
        }
    }

    /// A turn that finished, with its summary or its error.
    fn finished(
        &mut self,
        payload: &Value,
        turn: &str,
        at: Option<Timestamp>,
        out: &mut Vec<TimelineItem>,
    ) {
        let started_at = self
            .started
            .remove(turn)
            .or_else(|| secs(payload, "started_at"));
        let completed_at = secs(payload, "completed_at").or(at);
        let item = match payload.get("error").filter(|error| !error.is_null()) {
            Some(error) => self.failure(error, turn, completed_at),
            None => items::summary(
                self.agent,
                turn,
                payload.get("duration_ms").and_then(Value::as_u64),
                self.context,
                started_at,
                completed_at,
            ),
        };
        out.push(item);
        self.end(turn);
    }

    /// What an error during `turn` shows.
    fn failure(&self, error: &Value, turn: &str, at: Option<Timestamp>) -> TimelineItem {
        let limit = items::limit_hit(error).then_some((None, None));
        items::failure(self.agent, self.account, turn, error, limit, false, at)
    }

    /// Forgets `turn` as the running one.
    fn end(&mut self, turn: &str) {
        if self.turn.as_deref() == Some(turn) {
            self.turn = None;
        }
    }

    /// An item from an older rollout's message event, named by its line.
    fn legacy(&self, turn: Option<String>, body: ItemBody, at: Option<Timestamp>) -> TimelineItem {
        TimelineItem {
            id: ItemId(format!("codex:rollout:{}", self.line)),
            agent: self.agent,
            turn: turn.map(TurnId),
            parent: None,
            ordinal: 0,
            status: ItemStatus::Completed,
            started_at: at,
            completed_at: at,
            body,
        }
    }
}

/// Finds `thread`'s rollout under `dir`, going `depth` folders down at
/// most.
fn find(dir: &Path, thread: &str, depth: usize) -> Option<PathBuf> {
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_file() && thread_id(&path).as_deref() == Some(thread) {
            return Some(path);
        }
        if kind.is_dir() {
            folders.push(path);
        }
    }
    if depth == 0 {
        return None;
    }
    folders.sort();
    // The newest folders hold the likeliest match.
    folders
        .iter()
        .rev()
        .find_map(|folder| find(folder, thread, depth - 1))
}

/// A string field.
fn string(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_owned)
}

/// A time given in whole seconds.
fn secs(value: &Value, name: &str) -> Option<Timestamp> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .map(|secs| Timestamp(secs.saturating_mul(1000)))
}
