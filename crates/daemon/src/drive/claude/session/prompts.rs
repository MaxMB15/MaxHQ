// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The CLI's requests: permission prompts, questions and plans that wait on
//! you, and the requests `maxd` answers itself. Each answer follows report
//! 10's section 5, and always carries `updatedInput`.
//!
//! The CLI never times a prompt out. It withdraws one with
//! `control_cancel_request` when something else decided it, and the item
//! then shows as decided elsewhere.

use maxhq_core::names;
use maxhq_protocol::attention::{AttentionAnswer, AttentionEvent, AttentionKind, AttentionOption};
use maxhq_protocol::ids::{AttentionId, ClientId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ApprovalKind, Decided, ItemBody, ItemId, ItemStatus, NoticeLevel, QuestionAnswer,
};
use serde_json::{json, Map, Value};

use super::ClaudeSession;
use crate::drive::claude::convo::{self, Owner};
use crate::drive::claude::tools::{
    self, plan_options, tool_options, DENIED, DISMISSED, KEEP_PLANNING,
};
use crate::drive::claude::wire::{self, clip, text};
use crate::drive::{AttentionChange, Effects, Write};

/// What a prompt waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Permission to run a tool call.
    Tool,
    /// Whether to start on a plan.
    Plan,
    /// Answers to AskUserQuestion.
    Questions,
}

/// A request from the CLI that waits on you.
#[derive(Clone, Debug)]
pub struct Prompt {
    /// The CLI's request ID.
    pub request: String,
    /// What it waits on.
    pub kind: Kind,
    /// The item that shows it: an approval, or the question tool's own item.
    pub item: ItemId,
    /// The tool call it's about.
    pub tool_use_id: Option<String>,
    /// The tool call's item, when it isn't the prompt's own item.
    pub tool_item: Option<ItemId>,
    /// What the tool call passed, sent back as `updatedInput`.
    pub input: Value,
    /// The rules the CLI offers to keep for the rest of the session.
    pub suggestions: Vec<Value>,
    /// The attention event that's open for it.
    pub event: AttentionEvent,
}

/// What `maxd` says to requests it doesn't handle.
const NOT_HANDLED: &str = "MaxHQ doesn't handle this request.";

/// The mode a plan's option starts in.
fn plan_mode(option: &str) -> Option<PermissionMode> {
    match option {
        "accept_edits" => Some(PermissionMode::AcceptEdits),
        "accept" => Some(PermissionMode::Ask),
        "auto" => Some(PermissionMode::Auto),
        _ => None,
    }
}

/// An answer that went out, and how the items show it.
struct Reply {
    /// The response body.
    body: Value,
    /// Whether the call may go ahead.
    allowed: bool,
    /// The answers to put on a question item.
    answers: Option<Vec<QuestionAnswer>>,
    /// The mode a plan starts in.
    mode: Option<PermissionMode>,
}

impl ClaudeSession {
    /// Reads a `control_request` from the CLI. A replayed one that the
    /// holder no longer waits on was answered before, so it's skipped.
    pub(super) fn request(
        &mut self,
        line: &Value,
        at: Timestamp,
        replayed: bool,
        fx: &mut Effects,
    ) {
        let Some(id) = text(line, "request_id") else {
            return;
        };
        let Some(body) = line.get("request") else {
            return;
        };
        if self.answered.contains(id) || self.asked.contains_key(id) {
            return;
        }
        if replayed && !self.pending.contains(id) {
            self.answered.insert(id.to_owned());
            return;
        }
        match text(body, "subtype") {
            Some("can_use_tool") if Self::is_ours(body) => {
                let mut answer = json!({
                    "behavior": "allow",
                    "updatedInput": body.get("input").cloned().unwrap_or_else(|| json!({})),
                });
                if let (Value::Object(fields), Some(call)) =
                    (&mut answer, text(body, "tool_use_id"))
                {
                    fields.insert("toolUseID".to_owned(), json!(call));
                }
                self.reply_to(id, wire::success(id, answer), fx);
            }
            Some("can_use_tool") => self.can_use_tool(id, body, at, fx),
            Some("mcp_message") => self.mcp_message(id, body, fx),
            Some("elicitation") => {
                // An MCP server asks for input `maxd` can't show yet.
                self.reply_to(id, wire::success(id, json!({"action": "decline"})), fx);
                let server = text(body, "mcp_server_name").unwrap_or("An MCP server");
                let message = format!("{server} asked for input, and MaxHQ declined.");
                let owner = self.convo.owner(None);
                let item = self.convo.item(
                    convo::item_id(id),
                    &owner,
                    at,
                    ItemBody::Notice {
                        level: NoticeLevel::Info,
                        message,
                    },
                );
                self.convo.book.put(item);
            }
            Some("oauth_token_refresh") => {
                // `maxd` never holds a token, so the CLI refreshes its own.
                let line = wire::failure(id, "MaxHQ doesn't refresh tokens.");
                self.reply_to(id, line, fx);
            }
            _ => self.reply_to(id, wire::failure(id, NOT_HANDLED), fx),
        }
    }

    /// Answers one of the CLI's requests, once.
    fn reply_to(&mut self, id: &str, line: String, fx: &mut Effects) {
        self.answered.insert(id.to_owned());
        fx.writes.push(Write {
            line,
            answers: Some(id.to_owned()),
        });
    }

    /// Opens a prompt for a `can_use_tool` request.
    fn can_use_tool(&mut self, id: &str, body: &Value, at: Timestamp, fx: &mut Effects) {
        let name = text(body, "tool_name").unwrap_or_default().to_owned();
        let input = body
            .get("input")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let tool_use_id = text(body, "tool_use_id").map(str::to_owned);
        let suggestions: Vec<Value> = body
            .get("permission_suggestions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let owner = self.prompt_owner(body, tool_use_id.as_deref());
        let tool_item = tool_use_id.as_deref().map(convo::item_id);
        let (kind, item, event) = match name.as_str() {
            "AskUserQuestion" => {
                let call = tool_use_id.clone().unwrap_or_else(|| id.to_owned());
                let item = self.question_item(&call, &input, &owner, at);
                let questions = tools::questions(&input);
                let summary = questions
                    .first()
                    .map(|question| clip(&question.text, 120))
                    .unwrap_or_else(|| "Claude has a question.".to_owned());
                // One question's choices work as buttons on a notification.
                let options = match questions.as_slice() {
                    [only] => only
                        .options
                        .iter()
                        .map(|option| AttentionOption {
                            id: option.label.clone(),
                            label: option.label.clone(),
                            kind: None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let event = AttentionEvent {
                    id: tools::attention(&call),
                    agent: owner.agent,
                    kind: AttentionKind::Question,
                    summary,
                    options,
                    accepts_text: true,
                    created_at: at,
                    expires_at: None,
                };
                (Kind::Questions, item, event)
            }
            _ => {
                let plan = name == "ExitPlanMode";
                if plan && let (Some(plan), Some(tool_item)) = (text(&input, "plan"), &tool_item) {
                    let plan = plan.to_owned();
                    self.convo.book.update(tool_item, |item| {
                        if let ItemBody::Plan { markdown, .. } = &mut item.body {
                            markdown.clone_from(&plan);
                        }
                    });
                }
                let attention = AttentionId::derive(&format!("claude:{id}"));
                let options = if plan {
                    plan_options()
                } else {
                    tool_options(!suggestions.is_empty())
                };
                let title = tools::approval_title(&name, &input, &self.place.cwd);
                let body = ItemBody::Approval {
                    attention,
                    approval_kind: if plan {
                        ApprovalKind::Plan
                    } else {
                        tools::approval_kind(&name)
                    },
                    title: title.clone(),
                    detail: tools::approval_detail(&name, &input, &self.place.cwd),
                    cwd: tools::approval_cwd(&input, &self.place.cwd),
                    options: options.clone(),
                    decided: None,
                    deny_first: false,
                    reason: None,
                };
                // The approval is named after its call, as a transcript can
                // name it too. A second request for the same call keeps its
                // own.
                let item_id = tool_use_id
                    .as_deref()
                    .map(convo::approval_id)
                    .filter(|approval| !self.convo.book.contains(approval))
                    .unwrap_or_else(|| convo::item_id(id));
                let mut item = self.convo.item(item_id, &owner, at, body);
                item.status = ItemStatus::Waiting;
                item.completed_at = None;
                // An approval sits under the call it's for.
                if let Some(tool) = tool_item
                    .as_ref()
                    .filter(|tool| self.convo.book.contains(tool))
                {
                    item.parent = Some(tool.clone());
                }
                let item_id = item.id.clone();
                self.convo.book.put(item);
                let event = AttentionEvent {
                    id: attention,
                    agent: owner.agent,
                    kind: AttentionKind::Permission,
                    summary: title,
                    options,
                    accepts_text: true,
                    created_at: at,
                    expires_at: None,
                };
                (if plan { Kind::Plan } else { Kind::Tool }, item_id, event)
            }
        };
        let tool_item = tool_item.filter(|tool| *tool != item);
        fx.attention.push(AttentionChange::Open(event.clone()));
        self.asked.insert(id.to_owned(), event.id);
        self.prompts.insert(
            event.id,
            Prompt {
                request: id.to_owned(),
                kind,
                item,
                tool_use_id,
                tool_item,
                input,
                suggestions,
                event,
            },
        );
        self.touch(at);
    }

    /// The timeline a prompt goes on: the tool call's, else the sub-agent
    /// that asked, else the lead's.
    fn prompt_owner(&self, body: &Value, tool_use_id: Option<&str>) -> Owner {
        if let Some(item) = tool_use_id
            .map(convo::item_id)
            .and_then(|id| self.convo.book.get(&id))
        {
            return Owner {
                agent: item.agent,
                parent: item.parent.clone(),
            };
        }
        if let Some(sub) = text(body, "agent_id").map(|task| names::subagent(self.lead(), task))
            && self.subs.contains_key(&sub)
        {
            return Owner {
                agent: sub,
                parent: None,
            };
        }
        self.convo.owner(None)
    }

    /// The question tool's item, made from the request when the tool call
    /// hasn't been seen.
    fn question_item(&mut self, call: &str, input: &Value, owner: &Owner, at: Timestamp) -> ItemId {
        let id = convo::item_id(call);
        if !self.convo.book.contains(&id) {
            let body = tools::start("AskUserQuestion", call, input);
            let mut item = self.convo.item(id.clone(), owner, at, body);
            item.status = ItemStatus::Running;
            item.completed_at = None;
            self.convo.book.put(item);
        }
        id
    }

    /// Answers a prompt.
    pub(super) fn answer(
        &mut self,
        key: IdempotencyKey,
        event: AttentionId,
        answer: AttentionAnswer,
        by: ClientId,
        fx: &mut Effects,
    ) {
        let Some(prompt) = self.prompts.get(&event).cloned() else {
            fx.results.push((
                key,
                ActionOutcome::Refused {
                    reason: "That request is no longer open.".to_owned(),
                },
            ));
            return;
        };
        let outcome = match self.resolve(&prompt, answer, Some(by), fx) {
            Ok(()) => ActionOutcome::Done,
            Err(reason) => ActionOutcome::Refused { reason },
        };
        fx.results.push((key, outcome));
    }

    /// Dismisses every prompt that waits on the lead, for a message that
    /// stops the turn. A sub-agent's prompts stay open, since stopping the
    /// lead's turn leaves a background sub-agent running.
    pub(super) fn dismiss_waiting(&mut self, fx: &mut Effects) {
        let lead = self.lead();
        let mut open: Vec<Prompt> = self
            .prompts
            .values()
            .filter(|prompt| prompt.event.agent == lead)
            .cloned()
            .collect();
        open.sort_by_key(|prompt| prompt.event.created_at);
        for prompt in open {
            // A dismissal always fits, so nothing refuses it.
            let _ = self.resolve(&prompt, AttentionAnswer::Dismiss, None, fx);
        }
    }

    /// Sends `answer` to a prompt and closes it, with its items as it
    /// leaves them.
    fn resolve(
        &mut self,
        prompt: &Prompt,
        answer: AttentionAnswer,
        by: Option<ClientId>,
        fx: &mut Effects,
    ) -> Result<(), String> {
        let reply = match prompt.kind {
            Kind::Tool => tool_reply(prompt, &answer),
            Kind::Plan => plan_reply(prompt, &answer),
            Kind::Questions => self.questions_reply(prompt, &answer),
        }?;
        let at = self.now();
        let line = wire::success(&prompt.request, reply.body);
        self.reply_to(&prompt.request, line, fx);
        if let Some(mode) = reply.mode {
            self.requested = mode;
        }
        match (prompt.kind, answer) {
            (_, AttentionAnswer::Dismiss) => self.decide(prompt, None, ItemStatus::Declined, at),
            (Kind::Questions, _) => {
                let answers = reply.answers;
                self.convo.book.update(&prompt.item, |item| {
                    if let ItemBody::Question { answers: shown, .. } = &mut item.body {
                        *shown = answers;
                    }
                });
            }
            (Kind::Tool | Kind::Plan, answer) => {
                let decided = Decided { answer, by, at };
                self.decide(prompt, Some(decided), ended(reply.allowed), at);
            }
        }
        self.close(prompt, fx);
        Ok(())
    }

    /// Shows how a prompt ended, on its item and on its tool call. An
    /// approval shows what was decided; one you dismissed shows nothing.
    fn decide(
        &mut self,
        prompt: &Prompt,
        decided: Option<Decided>,
        status: ItemStatus,
        at: Timestamp,
    ) {
        self.convo.book.update(&prompt.item, |item| {
            if let ItemBody::Approval { decided: shown, .. } = &mut item.body {
                *shown = decided;
            }
            item.status = status;
            item.completed_at = Some(at);
        });
        if let Some(tool) = &prompt.tool_item {
            self.convo.book.update(tool, |item| {
                if item.status == ItemStatus::Waiting {
                    item.status = match status {
                        ItemStatus::Declined => ItemStatus::Declined,
                        _ => ItemStatus::Running,
                    };
                }
            });
        }
    }

    /// Closes a prompt's attention event and forgets it. A question's item
    /// runs on until its tool result arrives.
    fn close(&mut self, prompt: &Prompt, fx: &mut Effects) {
        if prompt.kind == Kind::Questions {
            self.convo.book.update(&prompt.item, |item| {
                if item.status == ItemStatus::Waiting {
                    item.status = ItemStatus::Running;
                }
            });
        }
        self.prompts.remove(&prompt.event.id);
        self.asked.remove(&prompt.request);
        self.answered.insert(prompt.request.clone());
        fx.attention.push(AttentionChange::Close(prompt.event.id));
    }

    /// The prompt for a tool call that ended, decided some other way, such
    /// as by a hook.
    pub(super) fn decided_elsewhere(
        &mut self,
        tool_use_id: &str,
        allowed: bool,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        let Some(prompt) = self
            .prompts
            .values()
            .find(|prompt| prompt.tool_use_id.as_deref() == Some(tool_use_id))
            .cloned()
        else {
            return;
        };
        if prompt.kind != Kind::Questions {
            let answer = AttentionAnswer::Option {
                id: if allowed { "allow" } else { "deny" }.to_owned(),
            };
            let decided = Decided {
                answer,
                by: None,
                at,
            };
            self.decide(&prompt, Some(decided), ended(allowed), at);
        }
        self.close(&prompt, fx);
    }

    /// Reads a `control_cancel_request`: the CLI no longer waits on the
    /// prompt, since something else decided it.
    pub(super) fn withdrawn(&mut self, line: &Value, at: Timestamp, fx: &mut Effects) {
        let Some(id) = text(line, "request_id") else {
            return;
        };
        self.answered.insert(id.to_owned());
        self.pending.remove(id);
        let Some(prompt) = self
            .asked
            .get(id)
            .and_then(|event| self.prompts.get(event))
            .cloned()
        else {
            return;
        };
        if prompt.kind != Kind::Questions {
            self.decide(&prompt, None, ItemStatus::Cancelled, at);
        }
        self.close(&prompt, fx);
    }

    /// Reads the prompts an `initialize` reply says the CLI waits on. Ones
    /// it no longer lists were decided while `maxd` was away.
    pub(super) fn still_waiting(&mut self, frames: &[Value], at: Timestamp, fx: &mut Effects) {
        let listed: Vec<String> = frames
            .iter()
            .filter_map(|frame| text(frame, "request_id").map(str::to_owned))
            .collect();
        let gone: Vec<Prompt> = self
            .prompts
            .values()
            .filter(|prompt| !listed.contains(&prompt.request))
            .cloned()
            .collect();
        for prompt in gone {
            if prompt.kind != Kind::Questions {
                self.decide(&prompt, None, ItemStatus::Cancelled, at);
            }
            self.close(&prompt, fx);
        }
        for frame in frames {
            if let Some(id) = text(frame, "request_id") {
                self.pending.insert(id.to_owned());
            }
            self.request(frame, at, false, fx);
        }
    }

    /// Ends every open prompt, when the CLI ends.
    pub(super) fn drop_prompts(&mut self, at: Timestamp, fx: &mut Effects) {
        let open: Vec<Prompt> = self.prompts.values().cloned().collect();
        for prompt in open {
            if prompt.kind != Kind::Questions {
                self.decide(&prompt, None, ItemStatus::Cancelled, at);
            }
            self.close(&prompt, fx);
        }
    }

    /// The answer to AskUserQuestion: the tool's input with an `answers`
    /// map from each question's text to the labels you picked.
    fn questions_reply(&self, prompt: &Prompt, answer: &AttentionAnswer) -> Result<Reply, String> {
        let questions = tools::questions(&prompt.input);
        let answers: Vec<QuestionAnswer> = match answer {
            AttentionAnswer::Dismiss => return Ok(dismissal(prompt)),
            AttentionAnswer::Questions { answers } => answers.clone(),
            AttentionAnswer::Option { id: value } | AttentionAnswer::Text { text: value } => {
                match questions.as_slice() {
                    [only] => vec![QuestionAnswer {
                        id: only.id.clone(),
                        values: vec![value.clone()],
                    }],
                    _ => return Err("Answer each question.".to_owned()),
                }
            }
        };
        let mut map = Map::new();
        for question in &questions {
            if let Some(given) = answers.iter().find(|given| given.id == question.id)
                && !given.values.is_empty()
            {
                map.insert(
                    question.text.clone(),
                    Value::String(given.values.join(", ")),
                );
            }
        }
        if map.is_empty() {
            return Err("Answer at least one question.".to_owned());
        }
        let mut input = prompt.input.clone();
        if let Value::Object(fields) = &mut input {
            fields.insert("answers".to_owned(), Value::Object(map));
        }
        let mut body = json!({"behavior": "allow", "updatedInput": input});
        add_tool_use_id(&mut body, prompt);
        Ok(Reply {
            body,
            allowed: true,
            answers: Some(answers),
            mode: None,
        })
    }
}

/// The answer to a permission prompt.
fn tool_reply(prompt: &Prompt, answer: &AttentionAnswer) -> Result<Reply, String> {
    let (mut body, allowed) = match answer {
        AttentionAnswer::Dismiss => return Ok(dismissal(prompt)),
        AttentionAnswer::Option { id } => match id.as_str() {
            "allow" => (
                json!({
                    "behavior": "allow",
                    "updatedInput": prompt.input,
                    "decisionClassification": "user_temporary",
                }),
                true,
            ),
            "allow_session" | "session" => {
                if prompt.suggestions.is_empty() {
                    return Err("This request has no rule to keep for the session.".to_owned());
                }
                // Only the session's own rules change, never a settings file.
                let rules: Vec<Value> = prompt
                    .suggestions
                    .iter()
                    .cloned()
                    .map(|mut rule| {
                        if let Value::Object(fields) = &mut rule {
                            fields.insert("destination".to_owned(), json!("session"));
                        }
                        rule
                    })
                    .collect();
                (
                    json!({
                        "behavior": "allow",
                        "updatedInput": prompt.input,
                        "updatedPermissions": rules,
                        "decisionClassification": "user_permanent",
                    }),
                    true,
                )
            }
            "deny" => (deny(DENIED, false), false),
            other => return Err(format!("This request has no option {other}.")),
        },
        AttentionAnswer::Text { text } => (deny(text, false), false),
        AttentionAnswer::Questions { .. } => {
            return Err("This request takes an option, not answers.".to_owned());
        }
    };
    add_tool_use_id(&mut body, prompt);
    Ok(Reply {
        body,
        allowed,
        answers: None,
        mode: None,
    })
}

/// The answer to a finished plan. Starting on it switches the session's
/// mode, and keeping on planning stops the turn so you can say more.
fn plan_reply(prompt: &Prompt, answer: &AttentionAnswer) -> Result<Reply, String> {
    let (mut body, allowed, mode) = match answer {
        AttentionAnswer::Dismiss => return Ok(dismissal(prompt)),
        AttentionAnswer::Option { id } if id == "deny" => (deny(KEEP_PLANNING, true), false, None),
        AttentionAnswer::Option { id } => {
            let Some(mode) = plan_mode(id) else {
                return Err(format!("This plan has no option {id}."));
            };
            (
                json!({
                    "behavior": "allow",
                    "updatedInput": prompt.input,
                    "updatedPermissions": [{
                        "type": "setMode",
                        "mode": wire::mode_name(mode),
                        "destination": "session",
                    }],
                }),
                true,
                Some(mode),
            )
        }
        AttentionAnswer::Text { text } => (deny(text, false), false, None),
        AttentionAnswer::Questions { .. } => {
            return Err("This plan takes an option, not answers.".to_owned());
        }
    };
    add_tool_use_id(&mut body, prompt);
    Ok(Reply {
        body,
        allowed,
        answers: None,
        mode,
    })
}

/// How a decided approval's items end.
fn ended(allowed: bool) -> ItemStatus {
    if allowed {
        ItemStatus::Completed
    } else {
        ItemStatus::Declined
    }
}

/// The answer that dismisses a prompt: a denial that stops the turn, as
/// Esc does in the CLI, so a queued message starts the next one.
fn dismissal(prompt: &Prompt) -> Reply {
    let mut body = deny(DISMISSED, true);
    add_tool_use_id(&mut body, prompt);
    Reply {
        body,
        allowed: false,
        answers: None,
        mode: None,
    }
}

/// A denial with a message for the model.
fn deny(message: &str, interrupt: bool) -> Value {
    json!({
        "behavior": "deny",
        "message": message,
        "interrupt": interrupt,
        "decisionClassification": "user_reject",
    })
}

/// Adds the call's ID, which the CLI matches the answer to.
fn add_tool_use_id(body: &mut Value, prompt: &Prompt) {
    if let (Value::Object(fields), Some(id)) = (body, &prompt.tool_use_id) {
        fields.insert("toolUseID".to_owned(), json!(id));
    }
}
