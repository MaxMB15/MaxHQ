// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sub-agents. A flavor's [`Task`] names one by the tool call
//! that started it, and the CLI sends each sub-agent's work as updates for
//! a child session. ACP says nothing that ties a child session to its
//! task, so the first updates for a session the driver doesn't know go to
//! the newest running sub-agent that has no session yet.
//!
//! A sub-agent ends when the flavor says it ended, when its tool call
//! ends, or when the lead's turn does.

use maxhq_core::names;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::model::{Activity, Agent, Control};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemStatus};
use serde_json::{json, Value};

use super::events::{self, Out, Stream};
use super::{chunk_text, AcpSession, Task};
use crate::drive::Effects;

/// A sub-agent the lead started.
#[derive(Debug)]
pub struct Sub {
    /// Its agent.
    pub agent: AgentId,
    /// The lead's tool call that started it.
    pub call: String,
    /// The child session its updates come on, once known.
    pub session: Option<String>,
    /// Its items.
    pub stream: Stream,
    /// Set once it ended.
    pub done: bool,
    /// The kind the CLI gave it, such as "explore".
    pub agent_type: Option<String>,
    /// What the lead asked it to do.
    pub description: String,
    /// Its model, if the task named one.
    pub model: Option<String>,
    /// When it started.
    pub started_at: Timestamp,
    /// When it last did something.
    pub last_activity_at: Timestamp,
}

impl AcpSession {
    /// A sub-agent started, changed or ended.
    pub(super) fn task(&mut self, task: Task, at: Timestamp, fx: &mut Effects) {
        let Task {
            call,
            kind: agent_type,
            description,
            model,
            ended: finished,
        } = task;
        let description = description.unwrap_or_else(|| "Sub-agent".to_owned());
        let agent = names::subagent(self.lead, &call);
        if !self.subs.iter().any(|sub| sub.call == call) {
            // A `toolCallId` can hold a newline, so the sub-agent's own ID
            // names its items.
            let base = format!("{}:sub:{agent}", self.flavor.base());
            let mut stream = Stream::new(agent, &base);
            stream.begin(1);
            self.subs.push(Sub {
                agent,
                call: call.clone(),
                session: None,
                stream,
                done: false,
                agent_type: agent_type.clone(),
                description: description.clone(),
                model,
                started_at: at,
                last_activity_at: at,
            });
        }
        if let Some(sub) = self.subs.iter_mut().find(|sub| sub.call == call) {
            sub.description = description.clone();
            if agent_type.is_some() {
                sub.agent_type = agent_type.clone();
            }
            sub.last_activity_at = at;
        }

        // The lead's tool call shows as the sub-agent.
        let mut out = Out::new();
        self.begin_if_needed();
        if !self.stream.tools.contains_key(&call) && !finished {
            self.stream.tool(
                &json!({ "toolCallId": call, "kind": "other", "status": "in_progress" }),
                at,
                &mut out,
            );
        }
        if let Some(tool) = self.stream.tools.get_mut(&call) {
            let result = match &tool.item.body {
                ItemBody::Subagent { result, .. } => result.clone(),
                _ => None,
            };
            tool.item.body = ItemBody::Subagent {
                subagent: Some(agent),
                agent_type,
                description,
                result,
            };
            if finished {
                tool.item.status = ItemStatus::Completed;
                tool.item.completed_at = Some(at);
            }
            events::upsert(&mut out, tool.item.clone());
            if finished {
                self.stream.tools.remove(&call);
            }
        }
        self.emit(out, fx);
        if finished {
            self.deadlines.remove(&call);
            self.sub_tool_ended(&call, at, fx);
        }
    }

    /// A `session/update` for a child session.
    pub(super) fn sub_update(
        &mut self,
        session: &str,
        update: &Value,
        kind: &str,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        // The replay a load sends keeps only the lead's conversation.
        if self.loading.is_some() {
            return;
        }
        let index = self
            .subs
            .iter()
            .position(|sub| sub.session.as_deref() == Some(session))
            .or_else(|| {
                self.subs
                    .iter()
                    .rposition(|sub| !sub.done && sub.session.is_none())
            });
        let Some(index) = index else {
            return;
        };
        let quiet = self.quiet;
        let sub = &mut self.subs[index];
        sub.session.get_or_insert_with(|| session.to_owned());
        sub.last_activity_at = at;
        sub.stream.quiet = quiet;
        let mut out = Out::new();
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                if let Some(chunk) = chunk_text(update) {
                    sub.stream
                        .chunk(kind == "agent_thought_chunk", &chunk, at, &mut out);
                }
            }
            "tool_call" | "tool_call_update" => {
                sub.stream.tool(update, at, &mut out);
            }
            "plan" => {
                sub.stream
                    .todos(events::plan_entries(update), false, at, &mut out);
            }
            _ => {}
        }
        sub.stream.quiet = false;
        fx.timeline.extend(out);
    }

    /// The lead's tool call `call` ended, which ends its sub-agent.
    pub(super) fn sub_tool_ended(&mut self, call: &str, at: Timestamp, fx: &mut Effects) {
        if let Some(index) = self
            .subs
            .iter()
            .position(|sub| sub.call == call && !sub.done)
        {
            self.end_sub(index, at, fx);
        }
    }

    /// Ends every sub-agent still running, as the lead's turn ends.
    pub(super) fn subs_over(&mut self, at: Timestamp, fx: &mut Effects) {
        for index in 0..self.subs.len() {
            if !self.subs[index].done {
                self.end_sub(index, at, fx);
            }
        }
    }

    /// Ends one sub-agent.
    fn end_sub(&mut self, index: usize, at: Timestamp, fx: &mut Effects) {
        let sub = &mut self.subs[index];
        sub.done = true;
        let mut out = Out::new();
        sub.stream.finish(ItemStatus::Completed, at, &mut out);
        let agent = sub.agent;
        fx.timeline.extend(out);
        if self.shown.remove(&agent).is_some() {
            fx.gone.push(agent);
        }
    }

    /// A sub-agent as it stands.
    pub(super) fn sub_agent(&self, sub: &Sub) -> Agent {
        let activity = match self.waiting_on(sub.agent) {
            Some(attention) => Activity::NeedsInput { attention },
            None => Activity::Busy,
        };
        let link = self.link.clone();
        Agent {
            id: sub.agent,
            workspace: self.place.workspace,
            branch: None,
            parent: Some(self.lead),
            vendor: self.flavor.vendor(),
            account: Some(self.login.account),
            model: sub.model.clone().or_else(|| self.model.clone()),
            permission: None,
            session: None,
            vendor_session_id: sub.session.clone(),
            control: Control::Held,
            pid: None,
            link,
            title: sub.description.clone(),
            activity,
            context: None,
            started_at: sub.started_at,
            last_activity_at: sub.last_activity_at,
            inbox: Default::default(),
            origin: None,
            pulls: Vec::new(),
            taking_over: false,
            asleep: false,
            effort: None,
            running: None,
            moved: None,
        }
    }
}
