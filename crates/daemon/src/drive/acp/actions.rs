// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Your actions, made into ACP requests. ACP has no way into a running
//! turn, so a steer cancels the turn and sends the message first thing
//! after, and an interrupt does the same. A model change is read back from
//! what the CLI reports, since a CLI can keep another model than the one
//! asked for.

use std::time::Instant;

use maxhq_core::handoff;
use maxhq_protocol::ids::IdempotencyKey;
use maxhq_protocol::message::SendMode;
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    Delivery, ItemBody, ItemId, ItemStatus, NoticeLevel, TimelineEvent,
};
use serde_json::Value;

use super::events::{self, Out};
use super::rpc::{self, RpcError};
use super::{done, prompt, refused, title_of, AcpSession, Call, Phase, Picker, Turn};
use crate::drive::{Action, Effects, Message, PlaceHost};

/// A message waiting for the turn to end.
#[derive(Debug)]
pub struct Queued {
    /// Its item while it waits.
    pub item: ItemId,
    /// The message.
    pub message: Message,
    /// How it shows once sent.
    pub delivery: Delivery,
}

impl AcpSession {
    /// Carries out an action.
    pub(super) fn action(&mut self, action: Action, fx: &mut Effects) {
        let now = Timestamp::now();
        match action {
            Action::Start { key, .. } | Action::Adopt { key, .. } => {
                let reason = format!(
                    "A {} process runs one agent, so it can't start another.",
                    self.flavor.name()
                );
                refused(fx, key, &reason);
            }
            Action::Send {
                key,
                agent,
                text,
                refs,
                attachments,
                mode,
            } => {
                if agent != self.lead {
                    let reason = format!("A {} sub-agent can't take messages.", self.flavor.name());
                    refused(fx, key, &reason);
                    return;
                }
                let message = Message {
                    text,
                    refs,
                    attachments,
                };
                self.send_message(key, message, mode, now, fx);
                done(fx, key);
            }
            Action::CancelQueued { key, item, .. } => {
                match self.queue.iter().position(|queued| queued.item == item) {
                    Some(at) => {
                        if let Some(queued) = self.queue.remove(at) {
                            self.drop_queued(queued, now, fx);
                        }
                        done(fx, key);
                    }
                    None => refused(fx, key, "That message was already sent."),
                }
            }
            Action::Interrupt { key, .. } => {
                if self.turn.is_some() {
                    self.cancel_turn(now, fx);
                    done(fx, key);
                } else {
                    refused(fx, key, "The agent isn't working on anything.");
                }
            }
            Action::SetPermission { key, mode, .. } => self.set_permission(key, mode, fx),
            Action::SetModel { key, model, .. } => self.set_model(key, model, fx),
            Action::SetEffort { key, .. } => {
                refused(fx, key, "This agent has no effort level to set.");
            }
            Action::Answer {
                key,
                event,
                answer,
                by,
            } => self.answer(key, event, answer, by, fx),
            Action::Stop { key, .. } => {
                self.stopping = true;
                fx.stop = true;
                done(fx, key);
            }
        }
    }

    /// Sends a message now, or queues it while a turn runs.
    fn send_message(
        &mut self,
        key: IdempotencyKey,
        message: Message,
        mode: SendMode,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        let busy = self.turn.is_some() || self.phase != Phase::Ready;
        if !busy && self.queue.is_empty() {
            self.start_turn(message, Delivery::Sent, at, fx);
            return;
        }
        let item = ItemId(format!("{}:queued:{key}", self.flavor.base()));
        let delivery = match mode {
            SendMode::Steer => Delivery::Steer,
            SendMode::Queue | SendMode::Interrupt => Delivery::Sent,
        };
        let mut out = Out::new();
        let mut waiting = self.stream.item(
            item.clone(),
            ItemStatus::Pending,
            at,
            user_body(&message, Delivery::Queued),
        );
        waiting.turn = None;
        events::upsert(&mut out, waiting);
        self.emit(out, fx);
        let queued = Queued {
            item,
            message,
            delivery,
        };
        if mode == SendMode::Queue || self.turn.is_none() {
            self.queue.push_back(queued);
        } else {
            self.queue.push_front(queued);
            // A steer stops the turn here too, since ACP can't add to one,
            // but only a message sent now dismisses what waits on you.
            if mode == SendMode::Interrupt {
                self.dismiss(None, at, fx);
            } else {
                self.cancel_turn(at, fx);
            }
        }
    }

    /// Queues what you typed in answer to a permission request, for after
    /// the turn.
    pub(super) fn follow_up(&mut self, key: IdempotencyKey, text: String, fx: &mut Effects) {
        let message = Message {
            text,
            refs: Vec::new(),
            attachments: Vec::new(),
        };
        self.send_message(key, message, SendMode::Queue, Timestamp::now(), fx);
    }

    /// Sends the next queued message, when nothing runs.
    pub(super) fn next_queued(&mut self, fx: &mut Effects) {
        if self.phase != Phase::Ready || self.turn.is_some() {
            return;
        }
        let Some(queued) = self.queue.pop_front() else {
            return;
        };
        fx.timeline
            .push((self.lead, TimelineEvent::Remove { id: queued.item }));
        self.start_turn(queued.message, queued.delivery, Timestamp::now(), fx);
    }

    /// Takes back a queued message.
    pub(super) fn drop_queued(&mut self, queued: Queued, at: Timestamp, fx: &mut Effects) {
        let mut item = self.stream.item(
            queued.item,
            ItemStatus::Cancelled,
            at,
            user_body(&queued.message, Delivery::Cancelled),
        );
        item.turn = None;
        let mut out = Out::new();
        events::upsert(&mut out, item);
        self.emit(out, fx);
    }

    /// Starts a turn with `message`.
    fn start_turn(
        &mut self,
        message: Message,
        delivery: Delivery,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        self.limited = false;
        self.errored = None;
        let mut out = Out::new();
        self.stream.finish(ItemStatus::Completed, at, &mut out);
        self.stream.begin(self.stream.turn + 1);
        self.open = true;
        self.last_user = false;
        if self.title.is_none() {
            self.title = title_of(&message.plain());
        }
        let item = self.stream.item(
            self.stream.id("user"),
            ItemStatus::Completed,
            at,
            user_body(&message, delivery),
        );
        events::upsert(&mut out, item);
        self.emit(out, fx);
        let blocks = prompt::blocks(
            &message.text,
            &message.refs,
            &message.attachments,
            self.lead,
            self.can_image(),
            self.place.host == PlaceHost::Local,
        );
        self.send(
            "session/prompt",
            rpc::prompt(&session, blocks),
            Call::Prompt,
            fx,
        );
        self.turn = Some(Turn {
            cancelling: false,
            timed_out: false,
        });
        self.touch(at);
    }

    /// Changes the model you picked.
    fn set_model(&mut self, key: IdempotencyKey, model: String, fx: &mut Effects) {
        if let Err(reason) = self.ask_model(Some(key), model, fx) {
            refused(fx, key, &reason);
        }
    }

    /// Asks a session that just started or loaded for the model you
    /// picked, when it runs another. Auto is left to the
    /// CLI, and a line read back after a restart asks for nothing.
    pub(super) fn pick_model(&mut self, fx: &mut Effects) {
        let Some(model) = self.picked.clone() else {
            return;
        };
        if self.quiet
            || self.flavor.auto_model() == Some(model.as_str())
            || self.model.as_deref() == Some(model.as_str())
        {
            return;
        }
        // A session with no model option runs the model it has.
        let _ = self.ask_model(None, model, fx);
    }

    /// Asks for `model` through the config option or v1's request, or
    /// says why it can't. The recording never changed one, so the request
    /// and its answer follow ACP's schema.
    fn ask_model(
        &mut self,
        key: Option<IdempotencyKey>,
        model: String,
        fx: &mut Effects,
    ) -> Result<(), String> {
        let session = self.live_session()?;
        let picker = self.model_option.clone().ok_or_else(|| {
            format!(
                "{} didn't offer a way to change the model.",
                self.flavor.name()
            )
        })?;
        let value = model.clone();
        self.pick(&picker, &session, &value, Call::SetModel { key, model }, fx);
        Ok(())
    }

    /// The session, or why there's none yet.
    fn live_session(&self) -> Result<String, String> {
        self.session_id
            .clone()
            .ok_or_else(|| "The agent's session isn't up yet.".to_owned())
    }

    /// Sends the request that picks `value` for a model or mode change.
    fn pick(&mut self, picker: &Picker, session: &str, value: &str, call: Call, fx: &mut Effects) {
        let (method, params) = match (picker, &call) {
            (Picker::Option(option), _) => (
                "session/set_config_option",
                rpc::set_config(session, option, value),
            ),
            (Picker::Legacy, Call::SetModel { .. }) => {
                ("session/set_model", rpc::set_model(session, value))
            }
            (Picker::Legacy, _) => ("session/set_mode", rpc::set_mode(session, value)),
        };
        self.send(method, params, call, fx);
    }

    /// A model change answered. The model the CLI reports wins, and a
    /// different one than asked for is said on the timeline. A change you
    /// asked for becomes the model each later session is asked for.
    pub(super) fn model_set(
        &mut self,
        key: Option<IdempotencyKey>,
        wanted: &str,
        outcome: Result<Value, RpcError>,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                let reason = format!(
                    "{} didn't change the model: {}",
                    self.flavor.name(),
                    error.message
                );
                match key {
                    Some(key) => refused(fx, key, &reason),
                    None => self.notice(NoticeLevel::Warning, &reason, at, fx),
                }
                return;
            }
        };
        let reported = result.get("configOptions").is_some();
        if reported {
            self.options(&result, fx);
        } else {
            self.model = Some(wanted.to_owned());
        }
        if let Some(kept) = self.model.clone().filter(|kept| kept != wanted) {
            self.notice(
                NoticeLevel::Warning,
                &format!(
                    "{} kept the model {kept} instead of {wanted}.",
                    self.flavor.name()
                ),
                at,
                fx,
            );
        }
        if let Some(key) = key {
            self.picked = Some(wanted.to_owned());
            done(fx, key);
        }
    }

    /// Changes the permission mode between plan and agent, which is the
    /// CLI's to change in its session. The sandbox is set when the process
    /// starts, so the manager starts the CLI again for a change that turns
    /// full access on or off, and one never reaches here.
    fn set_permission(&mut self, key: IdempotencyKey, mode: PermissionMode, fx: &mut Effects) {
        let into_plan = mode == PermissionMode::Plan;
        if into_plan == (self.effective == PermissionMode::Plan) {
            self.requested = mode;
            self.apply_mode(mode);
            done(fx, key);
            return;
        }
        if let Err(reason) = self.ask_mode(Some(key), mode, fx) {
            refused(fx, key, &reason);
        }
    }

    /// Asks a session that just started or loaded for plan mode, when you
    /// picked Plan and the CLI has no launch flag for it. True when the
    /// session waits for the answer, so no prompt runs outside plan. One
    /// with no plan mode runs in Ask.
    pub(super) fn pick_mode(&mut self, fx: &mut Effects) -> bool {
        if self.quiet
            || self.requested != PermissionMode::Plan
            || self.flavor.modes_are_launch_flags()
        {
            return false;
        }
        if self.plan_mode.is_none() {
            self.effective = PermissionMode::Ask;
            return false;
        }
        if self.effective == PermissionMode::Plan {
            return false;
        }
        let asked = self.ask_mode(None, PermissionMode::Plan, fx).is_ok();
        if !asked {
            self.effective = PermissionMode::Ask;
        }
        asked
    }

    /// Asks for plan mode, or for the mode leaving it goes to, or says why
    /// it can't.
    fn ask_mode(
        &mut self,
        key: Option<IdempotencyKey>,
        mode: PermissionMode,
        fx: &mut Effects,
    ) -> Result<(), String> {
        let session = self.live_session()?;
        let name = self.flavor.name();
        let picker = self
            .mode_option
            .clone()
            .ok_or_else(|| format!("{name} didn't offer a way to change the mode."))?;
        let value = if mode == PermissionMode::Plan {
            self.plan_mode
                .clone()
                .ok_or_else(|| format!("{name} has no plan mode."))?
        } else {
            self.work_mode
                .clone()
                .ok_or_else(|| format!("{name} has no mode to leave plan for."))?
        };
        self.pick(&picker, &session, &value, Call::SetMode { key, mode }, fx);
        Ok(())
    }

    /// A mode change answered.
    pub(super) fn mode_set(
        &mut self,
        key: Option<IdempotencyKey>,
        mode: PermissionMode,
        outcome: Result<Value, RpcError>,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        match (outcome, key) {
            (Ok(_), key) => {
                self.requested = mode;
                self.apply_mode(mode);
                if let Some(key) = key {
                    done(fx, key);
                }
            }
            (Err(error), key) => {
                let reason = format!(
                    "{} didn't change the mode: {}",
                    self.flavor.name(),
                    error.message
                );
                match key {
                    Some(key) => refused(fx, key, &reason),
                    None => {
                        self.effective = PermissionMode::Ask;
                        self.notice(NoticeLevel::Warning, &reason, at, fx);
                    }
                }
            }
        }
        // The session waited for the mode it started in.
        if key.is_none() && self.phase == Phase::Handshake {
            self.ready(fx);
        }
    }

    /// What runs once `mode` is in force.
    fn apply_mode(&mut self, mode: PermissionMode) {
        self.effective = if mode == PermissionMode::Plan {
            PermissionMode::Plan
        } else if self.launched_full {
            PermissionMode::FullAccess
        } else {
            mode
        };
    }

    /// Cancels the turn when a tool call ran out of time.
    pub(super) fn timeouts(&mut self, now: Instant, fx: &mut Effects) {
        let expired: Vec<String> = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(call, _)| call.clone())
            .collect();
        if expired.is_empty() {
            return;
        }
        let at = Timestamp::now();
        let minutes = self.flavor.tool_timeout().unwrap_or_default().as_secs() / 60;
        let mut out = Out::new();
        for call in &expired {
            self.deadlines.remove(call);
            // Named after the tool's item, as the call's ID can hold a
            // newline.
            let id = match self.stream.end_tool(call, ItemStatus::Failed, at, &mut out) {
                Some(tool) => ItemId(format!("{}:timeout", tool.0)),
                None => self.stream.id("timeout"),
            };
            let item = self.stream.item(
                id,
                ItemStatus::Completed,
                at,
                ItemBody::Error {
                    message: format!(
                        "A tool call ran for {minutes} minutes without finishing, so MaxHQ stopped the turn."
                    ),
                    retrying: false,
                    sign_in: None,
                },
            );
            events::upsert(&mut out, item);
        }
        self.emit(out, fx);
        if self.turn.is_some() {
            self.cancel_turn(at, fx);
            if let Some(turn) = &mut self.turn {
                turn.timed_out = true;
            }
        }
    }
}

/// A user message's body.
fn user_body(message: &Message, delivery: Delivery) -> ItemBody {
    ItemBody::UserMessage {
        text: handoff::strip(&message.text).to_owned(),
        attachments: message
            .attachments
            .iter()
            .map(|staged| staged.attachment.clone())
            .collect(),
        refs: message.refs.clone(),
        delivery,
    }
}
