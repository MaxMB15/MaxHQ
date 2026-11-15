// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sub-agents and background commands, which the CLI reports as tasks. A
//! sub-agent becomes an agent of its own once `task_started` names it, and
//! goes once its task ends. One whose transcript was read before its
//! `task_started` came, as after a restart, stays nested under its call,
//! where the items read are. A background command stays running on its
//! item until `task_notification` says how it ended. A foreground command
//! that runs past two seconds becomes a task too, whose end comes just
//! before the command's result. Only the result says what it printed, so
//! it ends the item.
//!
//! `task_progress` says what a running task is doing. It shows as one
//! notice under the task's call, replaced by each newer line and dropped
//! when the task ends, since the transcript keeps none of it.

use maxhq_core::names;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::model::{Activity, Agent, AgentLink, Control, Vendor};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, NoticeLevel, TimelineItem};
use serde_json::Value;

use super::{ClaudeSession, TITLE_CHARS};
use crate::drive::claude::convo::{self, ToolState};
use crate::drive::claude::tools;
use crate::drive::claude::wire::{clip, number, text};

/// How long a progress notice gets.
const PROGRESS_CHARS: usize = 200;

/// A running sub-agent.
#[derive(Clone, Debug)]
pub struct Sub {
    /// Its agent.
    pub agent: AgentId,
    /// The CLI's task ID, which `stop_task` takes.
    pub task: String,
    /// The agent that started it.
    pub parent: AgentId,
    /// What it was asked to do.
    pub title: String,
    /// When it started.
    pub started_at: Timestamp,
    /// When it last did anything.
    pub last_activity_at: Timestamp,
}

/// A task the CLI runs beside the conversation.
#[derive(Clone, Debug)]
pub struct Task {
    /// The tool call that started it.
    pub tool_use_id: Option<String>,
    /// The sub-agent it is, if it's one.
    pub sub: Option<AgentId>,
    /// What it was started to do, which progress repeats while it has
    /// nothing newer to say.
    pub description: Option<String>,
}

/// How a task ended, from its status.
fn ended(status: &str) -> Option<ItemStatus> {
    match status {
        "completed" => Some(ItemStatus::Completed),
        "failed" => Some(ItemStatus::Failed),
        "killed" | "stopped" | "cancelled" => Some(ItemStatus::Cancelled),
        _ => None,
    }
}

impl ClaudeSession {
    /// Reads a `system/task_*` line.
    pub(super) fn task(&mut self, kind: &str, line: &Value, at: Timestamp) {
        let Some(task) = text(line, "task_id") else {
            return;
        };
        match kind {
            "task_started" => self.task_started(task, line, at),
            "task_progress" => self.task_progress(task, line, at),
            "task_notification" => {
                if let Some(status) = text(line, "status").and_then(ended) {
                    self.task_ended(task, status, text(line, "summary"), at);
                }
            }
            "task_updated" => {
                let status = line
                    .get("patch")
                    .and_then(|patch| text(patch, "status"))
                    .and_then(ended);
                if let Some(status) = status {
                    self.task_ended(task, status, None, at);
                }
            }
            _ => {}
        }
    }

    /// Notes a task that started. A sub-agent's task ID is the sub-agent's
    /// own ID, which its transcript is named after.
    fn task_started(&mut self, task: &str, line: &Value, at: Timestamp) {
        let tool_use_id = text(line, "tool_use_id").map(str::to_owned);
        let is_agent = text(line, "task_type") == Some("local_agent")
            || tool_use_id
                .as_deref()
                .and_then(|call| self.convo.tools.get(call))
                .is_some_and(|tool| tools::is_subagent(&tool.name));
        let mut sub = None;
        if is_agent
            && let Some(call) = tool_use_id.as_deref()
            && !self.convo.sidechains.contains(call)
        {
            let agent = names::subagent(self.lead(), task);
            let parent = self
                .convo
                .tools
                .get(call)
                .map_or(self.lead(), |tool| tool.agent);
            let title = text(line, "description")
                .filter(|title| !title.trim().is_empty())
                .or_else(|| text(line, "subagent_type"))
                .map_or_else(|| "Sub-agent".to_owned(), |title| clip(title, TITLE_CHARS));
            self.convo.subs.insert(call.to_owned(), agent);
            self.convo.book.update(&convo::item_id(call), |item| {
                if let ItemBody::Subagent { subagent, .. } = &mut item.body {
                    *subagent = Some(agent);
                }
            });
            self.subs.entry(agent).or_insert(Sub {
                agent,
                task: task.to_owned(),
                parent,
                title,
                started_at: at,
                last_activity_at: at,
            });
            sub = Some(agent);
        }
        let description = text(line, "description").map(str::to_owned);
        self.tasks.insert(
            task.to_owned(),
            Task {
                tool_use_id,
                sub,
                description,
            },
        );
    }

    /// Shows what a running task is doing under its call. A task the
    /// driver never saw start, as after a reconnect, is known by the call
    /// the line names while that call still runs.
    fn task_progress(&mut self, task: &str, line: &Value, at: Timestamp) {
        if !self.tasks.contains_key(task)
            && let Some(call) = text(line, "tool_use_id")
            && self
                .convo
                .tools
                .get(call)
                .is_some_and(|tool| tool.state != ToolState::Ended)
        {
            let known = Task {
                tool_use_id: Some(call.to_owned()),
                sub: None,
                description: None,
            };
            self.tasks.insert(task.to_owned(), known);
        }
        let Some(known) = self.tasks.get(task) else {
            return;
        };
        if let Some(sub) = known.sub
            && let Some(sub) = self.subs.get_mut(&sub)
        {
            sub.last_activity_at = sub.last_activity_at.max(at);
        }
        let Some(call) = known.tool_use_id.clone() else {
            return;
        };
        let Some(message) = progress(line, known.description.as_deref()) else {
            return;
        };
        let Some(tool) = self.convo.book.get(&convo::item_id(&call)) else {
            return;
        };
        let id = progress_id(&call);
        let started_at = self
            .convo
            .book
            .get(&id)
            .and_then(|shown| shown.started_at)
            .or(Some(at));
        let item = TimelineItem {
            id,
            agent: tool.agent,
            turn: tool.turn.clone(),
            parent: Some(tool.id.clone()),
            ordinal: 0,
            status: ItemStatus::Running,
            started_at,
            completed_at: None,
            body: ItemBody::Notice {
                level: NoticeLevel::Info,
                message,
            },
        };
        self.convo.book.put(item);
    }

    /// Drops the progress shown for a task.
    pub(super) fn drop_progress(&mut self, task: &Task) {
        if let Some(call) = task.tool_use_id.as_deref() {
            self.convo.book.remove(&progress_id(call));
        }
    }

    /// Ends a task, on its item and, for a sub-agent, its agent. A command
    /// still in the foreground waits for its result instead.
    fn task_ended(&mut self, task: &str, status: ItemStatus, summary: Option<&str>, at: Timestamp) {
        let Some(task) = self.tasks.remove(task) else {
            return;
        };
        self.drop_progress(&task);
        let foreground = task.tool_use_id.as_deref().is_some_and(|call| {
            self.convo.tools.get(call).is_some_and(|tool| {
                tool.state == ToolState::Running && !tools::is_subagent(&tool.name)
            })
        });
        if let Some(call) = task.tool_use_id.as_deref()
            && !foreground
        {
            let summary = summary.map(str::to_owned);
            self.convo.book.update(&convo::item_id(call), |item| {
                if !matches!(
                    item.status,
                    ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
                ) {
                    return;
                }
                item.status = status;
                item.completed_at = Some(at);
                if let ItemBody::Subagent { result, .. } = &mut item.body
                    && result.as_deref().is_none_or(str::is_empty)
                    && let Some(summary) = summary
                {
                    *result = Some(summary);
                }
            });
            if let Some(tool) = self.convo.tools.get_mut(call) {
                tool.state = ToolState::Ended;
            }
        }
        if let Some(sub) = task.sub {
            self.sub_done(sub);
        }
    }

    /// Ends a sub-agent's agent.
    pub(super) fn sub_done(&mut self, agent: AgentId) {
        if self.subs.remove(&agent).is_some() {
            let ended: Vec<String> = self
                .tasks
                .iter()
                .filter(|(_, task)| task.sub == Some(agent))
                .map(|(id, _)| id.clone())
                .collect();
            for id in ended {
                if let Some(task) = self.tasks.remove(&id) {
                    self.drop_progress(&task);
                }
            }
            self.shown.remove(&agent);
            self.gone.push(agent);
        }
    }

    /// A sub-agent as it stands.
    pub(super) fn sub_agent(&self, sub: &Sub) -> Agent {
        let activity = match self.waiting_on(sub.agent) {
            Some(attention) => Activity::NeedsInput { attention },
            None => Activity::Busy,
        };
        Agent {
            id: sub.agent,
            workspace: self.place.workspace,
            parent: Some(sub.parent),
            vendor: Vendor::Claude,
            account: Some(self.login.account),
            model: None,
            permission: None,
            session: None,
            vendor_session_id: Some(sub.task.clone()),
            control: Control::Held,
            pid: self.pid,
            link: AgentLink::Live,
            title: sub.title.clone(),
            activity,
            context: None,
            started_at: sub.started_at,
            last_activity_at: sub.last_activity_at,
            branch: None,
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

/// The item that shows a task's progress under the call `call`.
fn progress_id(call: &str) -> ItemId {
    convo::item_id(&format!("{call}:progress"))
}

/// What a `task_progress` line says: its summary, else its description
/// when that's newer than `started`, else the tool it last used, with the
/// count of tool calls so far. None when it says nothing new.
fn progress(line: &Value, started: Option<&str>) -> Option<String> {
    let said = |key| {
        text(line, key)
            .map(str::trim)
            .filter(|said| !said.is_empty())
    };
    let what = said("summary")
        .map(str::to_owned)
        .or_else(|| {
            said("description")
                .filter(|description| Some(*description) != started.map(str::trim))
                .map(str::to_owned)
        })
        .or_else(|| said("last_tool_name").map(|tool| format!("Using {tool}")))?;
    let what = clip(&what, PROGRESS_CHARS);
    let calls = line
        .get("usage")
        .and_then(|usage| number(usage, "tool_uses"));
    Some(match calls {
        Some(1) => format!("{what} · 1 tool call"),
        Some(calls) if calls > 1 => format!("{what} · {calls} tool calls"),
        _ => what,
    })
}
