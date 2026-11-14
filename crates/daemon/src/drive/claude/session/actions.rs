// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Your actions as lines for the CLI, and the CLI's replies to the requests
//! `maxd` sends. A request's outcome waits for its reply, so a refusal
//! from the CLI reaches the client that asked.

use maxhq_core::handoff;
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, SendMode};
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Delivery, ItemBody, ItemId, ItemStatus, TimelineItem};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{channel, ClaudeSession, Sent};
use crate::drive::claude::attach;
use crate::drive::claude::convo;
use crate::drive::claude::effort::Saved;
use crate::drive::claude::limits;
use crate::drive::claude::wire::{self, text};
use crate::drive::{Action, ActionKey, Effects, PlaceHost, Staged, Write};

/// A request `maxd` sent, waiting for the CLI's reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ours {
    /// `initialize`, whose reply holds the CLI's state.
    Initialize,
    /// `set_permission_mode`. The mode asked for shows until the CLI
    /// answers, and the one before comes back if the CLI refuses it.
    Mode {
        /// The client's key.
        key: IdempotencyKey,
        /// The mode asked for.
        mode: PermissionMode,
        /// The mode asked for before this one.
        before: PermissionMode,
    },
    /// `set_model`.
    Model {
        /// The client's key.
        key: IdempotencyKey,
        /// The model asked for.
        model: String,
    },
    /// `apply_flag_settings` with a new effort level.
    Effort {
        /// The client's key.
        key: IdempotencyKey,
    },
    /// `interrupt`.
    Interrupt {
        /// The client's key.
        key: IdempotencyKey,
    },
    /// `cancel_async_message`.
    Cancel {
        /// The client's key.
        key: IdempotencyKey,
        /// The message taken back.
        uuid: String,
    },
    /// `stop_task`, for a sub-agent or a background command.
    StopTask {
        /// The client's key.
        key: IdempotencyKey,
    },
    /// `mcp_message` with the notice that MaxHQ's tool list changed.
    ToolsChanged,
}

impl Ours {
    /// The key of the action that sent it, if one did.
    pub fn key(&self) -> Option<IdempotencyKey> {
        match self {
            Self::Initialize | Self::ToolsChanged => None,
            Self::Mode { key, .. }
            | Self::Model { key, .. }
            | Self::Effort { key }
            | Self::Interrupt { key }
            | Self::Cancel { key, .. }
            | Self::StopTask { key } => Some(*key),
        }
    }
}

/// The request that sets the effort level the CLI's next turn runs at. A
/// null level clears the one set before, which leaves the CLI to its own
/// default. `maxd` names a level whenever it knows the model's default.
pub(super) fn effort_body(effort: Option<&str>) -> Value {
    json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": effort}})
}

/// Why actions fail once the CLI has ended.
const NOT_RUNNING: &str = "Claude isn't running.";

impl ClaudeSession {
    /// Sends `initialize`. The full one asks for sub-agent text and
    /// progress, and a bare one only reads the CLI's state back. Both name
    /// MaxHQ's own tool server, so a CLI that started without it gains it
    /// on the next connect; one that has it already keeps it.
    pub(super) fn initialize(&mut self, full: bool, fx: &mut Effects) {
        let mut body = if full {
            json!({
                "subtype": "initialize",
                "forwardSubagentText": true,
                "agentProgressSummaries": true,
                "perTaskStopAffordance": true,
            })
        } else {
            json!({"subtype": "initialize"})
        };
        channel::declare(&mut body);
        self.send_request(body, Ours::Initialize, fx);
        self.initialized = true;
    }

    /// Sends a request and remembers it until the reply.
    pub(super) fn send_request(&mut self, body: Value, ours: Ours, fx: &mut Effects) {
        let id = format!("maxd-{}", Uuid::now_v7());
        fx.writes.push(Write {
            line: wire::request(&id, body),
            answers: None,
        });
        self.ours.insert(id, ours);
    }

    /// Does what an action asks.
    pub(super) fn action(&mut self, action: Action, fx: &mut Effects) {
        let key = action.key();
        if self.exited {
            let outcome = match action {
                Action::Stop { .. } => {
                    fx.stop = true;
                    ActionOutcome::Done
                }
                _ => refused(NOT_RUNNING),
            };
            fx.results.push((key, outcome));
            return;
        }
        if !self.initialized {
            self.initialize(true, fx);
        }
        match action {
            Action::Start { .. } | Action::Adopt { .. } => {
                fx.results.push((
                    key,
                    refused("A Claude agent starts with a holder of its own."),
                ));
            }
            Action::Send {
                agent,
                text,
                refs,
                attachments,
                mode,
                ..
            } => {
                if let Some(outcome) = self.not_lead(agent, "You can't message a sub-agent.") {
                    fx.results.push((key, outcome));
                    return;
                }
                self.send(&text, refs, &attachments, mode, fx);
                fx.results.push((key, ActionOutcome::Done));
            }
            Action::CancelQueued { agent, item, .. } => {
                if let Some(outcome) = self.not_lead(agent, "A sub-agent has no queue.") {
                    fx.results.push((key, outcome));
                    return;
                }
                self.cancel_queued(key, &item, fx);
            }
            Action::Interrupt { agent, .. } => {
                if agent == self.lead() {
                    self.send_request(json!({"subtype": "interrupt"}), Ours::Interrupt { key }, fx);
                } else {
                    self.stop_task(key, agent, fx);
                }
            }
            Action::SetPermission { agent, mode, .. } => {
                if let Some(outcome) = self.not_lead(agent, "A sub-agent runs in its lead's mode.")
                {
                    fx.results.push((key, outcome));
                    return;
                }
                let before = std::mem::replace(&mut self.requested, mode);
                let body = json!({"subtype": "set_permission_mode", "mode": wire::mode_name(mode)});
                self.send_request(body, Ours::Mode { key, mode, before }, fx);
            }
            Action::SetModel { agent, model, .. } => {
                if let Some(outcome) = self.not_lead(agent, "A sub-agent runs its lead's model.") {
                    fx.results.push((key, outcome));
                    return;
                }
                let body = json!({"subtype": "set_model", "model": model});
                self.send_request(body, Ours::Model { key, model }, fx);
            }
            Action::SetEffort { agent, effort, .. } => {
                if let Some(outcome) =
                    self.not_lead(agent, "A sub-agent runs at its lead's effort.")
                {
                    fx.results.push((key, outcome));
                    return;
                }
                self.send_request(effort_body(effort.as_deref()), Ours::Effort { key }, fx);
            }
            Action::Answer {
                event, answer, by, ..
            } => self.answer(key, event, answer, by, fx),
            Action::Stop { agent, .. } => {
                if agent == self.lead() {
                    self.stopping = true;
                    fx.stop = true;
                    fx.results.push((key, ActionOutcome::Done));
                } else {
                    self.stop_task(key, agent, fx);
                }
            }
        }
    }

    /// The outcome for an action only the lead takes, when `agent` isn't
    /// the lead.
    fn not_lead(&self, agent: AgentId, why: &str) -> Option<ActionOutcome> {
        if agent == self.lead() {
            None
        } else if self.subs.contains_key(&agent) {
            Some(refused(why))
        } else {
            Some(ActionOutcome::UnknownAgent)
        }
    }

    /// Sends a message. While a turn runs the CLI keeps it in its queue: a
    /// queued message waits for the turn to end, a steer joins the turn at
    /// its next step, and an interrupt stops the turn and runs it now,
    /// dismissing what waits on you first. A message queued while the
    /// agent waits idle for a usage limit to reset waits with `maxd`
    /// instead.
    fn send(
        &mut self,
        said: &str,
        refs: Vec<InlineRef>,
        staged: &[Staged],
        mode: SendMode,
        fx: &mut Effects,
    ) {
        let at = self.now();
        let uuid = Uuid::now_v7().to_string();
        let here = self.place.host == PlaceHost::Local;
        let content = attach::content(said, &refs, staged, self.lead(), here);
        let hold = self.holds(mode);
        let delivery = match (hold || self.working(), mode) {
            (false, _) | (true, SendMode::Interrupt) => Delivery::Sent,
            (true, SendMode::Queue) => Delivery::Queued,
            (true, SendMode::Steer) => Delivery::Steer,
        };
        self.convo.book.put(TimelineItem {
            id: convo::item_id(&uuid),
            agent: self.lead(),
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Pending,
            started_at: Some(at),
            completed_at: None,
            body: ItemBody::UserMessage {
                text: handoff::strip(said).to_owned(),
                attachments: content.attachments,
                refs,
                delivery,
            },
        });
        if hold {
            self.hold(uuid, content.blocks);
        } else {
            if mode == SendMode::Interrupt {
                self.dismiss_waiting(fx);
            }
            self.write_message(&uuid, content.blocks, mode, fx);
        }
        self.touch(at);
    }

    /// Writes a message for the CLI, with the place `mode` gives it in
    /// the CLI's queue while a turn runs.
    pub(super) fn write_message(
        &mut self,
        uuid: &str,
        blocks: Vec<Value>,
        mode: SendMode,
        fx: &mut Effects,
    ) {
        let priority = match (self.working(), mode) {
            (false, _) => None,
            (true, SendMode::Queue) => Some("later"),
            (true, SendMode::Steer) => Some("next"),
            (true, SendMode::Interrupt) => Some("now"),
        };
        let mut line = json!({
            "type": "user",
            "uuid": uuid,
            "parent_tool_use_id": null,
            "message": {"role": "user", "content": blocks},
        });
        if let (Value::Object(fields), Some(priority)) = (&mut line, priority) {
            fields.insert("priority".to_owned(), json!(priority));
        }
        fx.writes.push(Write {
            line: line.to_string(),
            answers: None,
        });
        self.sends.insert(uuid.to_owned(), Sent { started: false });
    }

    /// Takes back a message the CLI hasn't started, or one held for a
    /// usage limit's reset.
    fn cancel_queued(&mut self, key: IdempotencyKey, item: &ItemId, fx: &mut Effects) {
        let Some(uuid) = item.0.strip_prefix("claude:") else {
            fx.results
                .push((key, refused("That isn't a queued message.")));
            return;
        };
        if self.unhold(uuid, self.now()) {
            fx.results.push((key, ActionOutcome::Done));
            return;
        }
        match self.sends.get(uuid) {
            Some(sent) if !sent.started => {
                let body = json!({"subtype": "cancel_async_message", "message_uuid": uuid});
                let ours = Ours::Cancel {
                    key,
                    uuid: uuid.to_owned(),
                };
                self.send_request(body, ours, fx);
            }
            Some(_) => fx
                .results
                .push((key, refused("That message already started."))),
            None => fx
                .results
                .push((key, refused("That isn't a queued message."))),
        }
    }

    /// Stops a sub-agent or background task.
    fn stop_task(&mut self, key: IdempotencyKey, agent: AgentId, fx: &mut Effects) {
        let Some(task) = self.subs.get(&agent).map(|sub| sub.task.clone()) else {
            fx.results.push((key, ActionOutcome::UnknownAgent));
            return;
        };
        let body = json!({"subtype": "stop_task", "task_id": task});
        self.send_request(body, Ours::StopTask { key }, fx);
    }

    /// Reads a `control_response`, the CLI's reply to a request `maxd` sent.
    pub(super) fn reply(&mut self, line: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(response) = line.get("response") else {
            return;
        };
        let Some(ours) = text(response, "request_id").and_then(|id| self.ours.remove(id)) else {
            return;
        };
        let payload = response.get("response").unwrap_or(&Value::Null);
        if text(response, "subtype") != Some("success") {
            let error = text(response, "error").unwrap_or("Claude refused the request.");
            if let Ours::Mode { mode, before, .. } = ours {
                self.mode_failed(mode, before);
            }
            match ours.key() {
                Some(key) => fx.results.push((key, refused(error))),
                None => tracing::debug!(error, request = ?ours, "Claude refused a request"),
            }
            return;
        }
        match ours {
            Ours::Initialize => self.initialized_with(response, payload, at, fx),
            Ours::ToolsChanged => {}
            Ours::Mode { key, .. } => {
                if let Some(mode) = text(payload, "mode").and_then(wire::mode_from) {
                    self.effective = Some(mode);
                }
                fx.results.push((key, ActionOutcome::Done));
            }
            Ours::Model { key, model } => {
                self.model = Some(model);
                fx.results.push((key, ActionOutcome::Done));
            }
            Ours::Cancel { key, uuid } => {
                self.sends.remove(&uuid);
                self.convo.book.update(&convo::item_id(&uuid), |item| {
                    item.status = ItemStatus::Cancelled;
                    item.completed_at = Some(at);
                    if let ItemBody::UserMessage { delivery, .. } = &mut item.body {
                        *delivery = Delivery::Cancelled;
                    }
                });
                fx.results.push((key, ActionOutcome::Done));
            }
            Ours::Effort { key } | Ours::Interrupt { key } | Ours::StopTask { key } => {
                fx.results.push((key, ActionOutcome::Done));
            }
        }
    }

    /// Reads the reply to `initialize`: the CLI's process, mode, models and
    /// state, and the prompts it waits on.
    fn initialized_with(
        &mut self,
        response: &Value,
        payload: &Value,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        if let Some(pid) = wire::number(payload, "pid").and_then(|pid| u32::try_from(pid).ok()) {
            self.pid = Some(pid);
        }
        if let Some(mode) = text(payload, "current_permission_mode").and_then(wire::mode_from) {
            self.effective = Some(mode);
        }
        match text(payload, "session_state") {
            Some("running" | "requires_action") => self.busy = true,
            Some("idle") => self.turn_over(),
            _ => {}
        }
        if let Some(models) = limits::models(payload, &Saved::read(&self.login.config_dir)) {
            fx.models.push((self.login.account, models));
        }
        let frames: Vec<Value> = [
            "pending_permission_requests",
            "pending_user_dialog_requests",
        ]
        .into_iter()
        .filter_map(|key| response.get(key).or_else(|| payload.get(key)))
        .filter_map(Value::as_array)
        .flatten()
        .cloned()
        .collect();
        self.still_waiting(&frames, at, fx);
    }

    /// Puts back the mode asked for before a change the CLI didn't make.
    /// A later change still waiting keeps showing, and falls back past
    /// this one if it fails too.
    fn mode_failed(&mut self, mode: PermissionMode, before: PermissionMode) {
        if self.requested == mode {
            self.requested = before;
        }
        for ours in self.ours.values_mut() {
            if let Ours::Mode {
                before: earlier, ..
            } = ours
                && *earlier == mode
            {
                *earlier = before;
            }
        }
    }

    /// Fails every request still waiting on a reply, when the CLI ends.
    /// Mode changes go back newest first, which leaves the mode asked for
    /// before the oldest of them.
    pub(super) fn drop_ours(&mut self, fx: &mut Effects) {
        // Request IDs hold a time ordered UUID, so they sort oldest first.
        let mut waiting: Vec<(String, Ours)> = self.ours.drain().collect();
        waiting.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (_, ours) in waiting.iter().rev() {
            if let Ours::Mode { mode, before, .. } = ours {
                self.mode_failed(*mode, *before);
            }
        }
        let mut keys: Vec<IdempotencyKey> = waiting
            .into_iter()
            .filter_map(|(_, ours)| ours.key())
            .collect();
        keys.sort();
        for key in keys {
            fx.results
                .push((key, refused("Claude ended before it answered.")));
        }
    }
}

/// A refusal with this reason.
fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}
