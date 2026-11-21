// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Requests the app-server sends. Approvals and questions become prompts
//! on the timeline and in your inbox, and your answers go back as their
//! replies. Calls to MaxHQ's own tools run as `channel` says. Anything
//! else gets a "method not found" error at once, since a request nobody
//! answers blocks its turn forever.

use std::time::{Duration, Instant};

use maxhq_protocol::attention::{AttentionAnswer, AttentionEvent, AttentionKind, AttentionOption};
use maxhq_protocol::ids::{AttentionId, ClientId, IdempotencyKey};
use maxhq_protocol::message::SendMode;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ApprovalKind, Decided, FileOp, ItemBody, ItemId, ItemStatus, Question, QuestionAnswer,
    QuestionOption, TimelineItem, TurnId,
};
use serde_json::{json, Map, Value};

use super::thread::Outgoing;
use super::{done, items, open_attention, refused, rpc, upsert, CodexServer, Seen, Timer};
use crate::drive::claude::clip;
use crate::drive::{AttentionChange, Effects, Message, Write};

/// What a prompt asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// To run a command.
    Command,
    /// To change files.
    File,
    /// For more sandbox access.
    Permissions,
    /// Answers to questions.
    Input,
    /// What an MCP server asks for.
    Elicitation,
}

impl Kind {
    /// The kind for a request method, or `None` for one the driver doesn't
    /// handle.
    fn of(method: &str) -> Option<Self> {
        match method {
            "item/commandExecution/requestApproval" => Some(Kind::Command),
            "item/fileChange/requestApproval" => Some(Kind::File),
            "item/permissions/requestApproval" => Some(Kind::Permissions),
            "item/tool/requestUserInput" => Some(Kind::Input),
            "mcpServer/elicitation/request" => Some(Kind::Elicitation),
            _ => None,
        }
    }
}

/// A request from Codex waiting for your answer.
#[derive(Clone, Debug)]
pub struct Prompt {
    /// The app-server's ID for the request.
    pub id: Value,
    /// The holder's key for it.
    pub key: String,
    /// The thread that asked.
    pub thread: String,
    /// The turn it blocks.
    pub turn: Option<String>,
    /// What it asks for.
    pub kind: Kind,
    /// The approval or question on the timeline.
    pub item: TimelineItem,
    /// The attention event.
    pub event: AttentionEvent,
    /// The reply each option sends.
    pub replies: Vec<(String, Value)>,
}

/// What a prompt shows and the reply behind each option.
struct Asked {
    approval_kind: ApprovalKind,
    title: String,
    detail: Option<String>,
    /// What the attention event says, when it's more than the title.
    summary: Option<String>,
    options: Vec<(String, String, Value)>,
}

/// The most a permission request's summary says.
const SUMMARY_CHARS: usize = 200;

/// The option that says no, which a typed answer also picks.
const DECLINE: &str = "decline";

// How much of a command the title shows, as Claude's does.
const TITLE_CHARS: usize = 80;

impl CodexServer {
    /// Acts on a request from the app-server.
    pub(super) fn request(
        &mut self,
        id: Value,
        method: &str,
        params: Value,
        seen: Seen,
        fx: &mut Effects,
    ) {
        let Some(key) = rpc::key(&id) else {
            return;
        };
        // A replayed request the holder no longer holds was answered by an
        // earlier `maxd`.
        if seen.replayed && !self.pending.contains(&key) {
            return;
        }
        let thread = params
            .get("threadId")
            .and_then(Value::as_str)
            .filter(|thread| self.threads.contains_key(*thread))
            .map(str::to_owned);
        if method == "item/tool/call"
            && let Some(thread) = &thread
        {
            self.tool_call(id, key, thread.clone(), &params, seen.at, fx);
            return;
        }
        let (Some(kind), Some(thread)) = (Kind::of(method), thread) else {
            let message = format!("MaxHQ doesn't support {method}");
            fx.writes.push(Write {
                line: rpc::error(&id, rpc::METHOD_NOT_FOUND, &message),
                answers: Some(key.clone()),
            });
            self.pending.remove(&key);
            return;
        };
        self.pending.insert(key.clone());
        let attention = self.attention_id(&thread, kind, method, &params, &key);
        if let Some(prompt) = self.prompts.get_mut(&attention) {
            prompt.id = id;
            prompt.key = key;
            return;
        }
        let Some(agent) = self.threads.get(&thread).map(|known| known.agent) else {
            return;
        };
        let created_at = params
            .get("startedAtMs")
            .and_then(Value::as_u64)
            .map(Timestamp)
            .unwrap_or(seen.at);
        let turn = params
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let parent = params
            .get("itemId")
            .and_then(Value::as_str)
            .map(items::item_id);
        let mut item = TimelineItem {
            id: ItemId(String::new()),
            agent,
            turn: turn.clone().map(TurnId),
            parent,
            ordinal: 0,
            status: ItemStatus::Waiting,
            started_at: Some(created_at),
            completed_at: None,
            body: ItemBody::Interrupted,
        };
        let (event, replies) = if kind == Kind::Input {
            let questions = questions(&params);
            let mut options = Vec::new();
            let mut replies = Vec::new();
            if let [question] = questions.as_slice() {
                for option in &question.options {
                    options.push(AttentionOption {
                        id: option.label.clone(),
                        label: option.label.clone(),
                        kind: None,
                    });
                    let answer = [QuestionAnswer {
                        id: question.id.clone(),
                        values: vec![option.label.clone()],
                    }];
                    replies.push((option.label.clone(), input_reply(&answer)));
                }
            }
            let expires_at = params
                .get("autoResolutionMs")
                .and_then(Value::as_u64)
                .map(|ms| {
                    let timer = Timer::Expire { attention };
                    self.timers
                        .push((Instant::now() + Duration::from_millis(ms), timer));
                    created_at.after(Duration::from_millis(ms))
                });
            let summary = questions
                .first()
                .map(|question| question.text.clone())
                .unwrap_or_else(|| "Codex has a question".into());
            item.id = ItemId(format!("codex:question:{attention}"));
            item.body = ItemBody::Question {
                attention,
                questions,
                answers: None,
            };
            let event = AttentionEvent {
                id: attention,
                agent,
                kind: AttentionKind::Question,
                summary,
                options,
                accepts_text: true,
                created_at,
                expires_at,
            };
            (event, replies)
        } else {
            let asked = match kind {
                Kind::Command => command(&params),
                Kind::File => self.file_change(&thread, &params),
                Kind::Permissions => permissions(&params),
                _ => elicitation(&params),
            };
            let options: Vec<AttentionOption> = asked
                .options
                .iter()
                .map(|(id, label, _)| AttentionOption {
                    id: id.clone(),
                    label: label.clone(),
                    kind: None,
                })
                .collect();
            let replies = asked
                .options
                .into_iter()
                .map(|(id, _, reply)| (id, reply))
                .collect();
            // A command or permission request names its folder. Anything
            // else is for the thread's own.
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|cwd| !cwd.is_empty())
                .map(|cwd| cwd.strip_prefix("file://").unwrap_or(cwd).to_owned())
                .or_else(|| {
                    self.place_of(&thread)
                        .map(|place| place.cwd.to_string_lossy().into_owned())
                });
            item.id = ItemId(format!("codex:approval:{attention}"));
            item.body = ItemBody::Approval {
                attention,
                approval_kind: asked.approval_kind,
                title: asked.title.clone(),
                detail: asked.detail,
                cwd,
                options: options.clone(),
                decided: None,
                saves: Vec::new(),
            };
            let event = AttentionEvent {
                id: attention,
                agent,
                kind: AttentionKind::Permission,
                summary: asked.summary.unwrap_or(asked.title),
                options,
                accepts_text: true,
                created_at,
                expires_at: None,
            };
            (event, replies)
        };
        upsert(fx, item.clone());
        open_attention(fx, event.clone());
        let prompt = Prompt {
            id,
            key,
            thread,
            turn,
            kind,
            item,
            event,
            replies,
        };
        self.prompts.insert(attention, prompt);
    }

    /// A stable ID for a request, so the same request read again after a
    /// restart opens the same event.
    fn attention_id(
        &self,
        thread: &str,
        kind: Kind,
        method: &str,
        params: &Value,
        key: &str,
    ) -> AttentionId {
        let part = |name: &str| match params.get(name) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let name = if kind == Kind::Elicitation {
            let which = params
                .get("elicitationId")
                .and_then(Value::as_str)
                .map_or_else(|| key.to_owned(), str::to_owned);
            format!(
                "codex:{}:{thread}:{}:{}:{which}:{method}",
                self.device,
                part("turnId"),
                part("serverName"),
            )
        } else {
            format!(
                "codex:{}:{thread}:{}:{}:{}:{method}",
                self.device,
                part("itemId"),
                part("approvalId"),
                part("startedAtMs"),
            )
        };
        AttentionId::derive(&name)
    }

    /// What a file change approval shows, from the change Codex already
    /// started.
    fn file_change(&self, thread: &str, params: &Value) -> Asked {
        let changes = params
            .get("itemId")
            .and_then(Value::as_str)
            .and_then(|item| self.threads.get(thread)?.open.get(item))
            .and_then(|open| match &open.item.body {
                ItemBody::FileChange { changes } => Some(changes.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let grant_root = params.get("grantRoot").and_then(Value::as_str);
        let title = match (grant_root, changes.as_slice()) {
            (Some(root), _) => format!("Allow writes in {root}?"),
            (None, [change]) => {
                let name = change.path.rsplit('/').next().unwrap_or(&change.path);
                match change.op {
                    FileOp::Add => format!("Create {name}?"),
                    FileOp::Delete => format!("Delete {name}?"),
                    FileOp::Rename | FileOp::Modify => format!("Edit {name}?"),
                }
            }
            (None, []) => "Change files?".to_owned(),
            (None, many) => format!("Edit {} files?", many.len()),
        };
        let paths = changes
            .iter()
            .map(|change| change.path.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let detail = join_detail(
            (!paths.is_empty()).then_some(paths),
            params.get("reason").and_then(Value::as_str),
        );
        let options = ["accept", "acceptForSession", DECLINE, "cancel"]
            .into_iter()
            .filter_map(|decision| decision_option(&json!(decision), 0))
            .collect();
        Asked {
            approval_kind: ApprovalKind::FileChange,
            title,
            detail,
            summary: None,
            options,
        }
    }

    /// Answers a prompt with your answer. A typed answer to an approval
    /// declines it and sends what you typed to Codex as your next message.
    pub(super) fn answer(
        &mut self,
        key: IdempotencyKey,
        event: AttentionId,
        answer: AttentionAnswer,
        by: ClientId,
        fx: &mut Effects,
    ) {
        let Some(prompt) = self.prompts.get(&event) else {
            refused(fx, key, "That question was already answered.");
            return;
        };
        let questions = match &prompt.item.body {
            ItemBody::Question { questions, .. } => Some(questions.clone()),
            _ => None,
        };
        let mut given = None;
        let mut follow_up = None;
        let (reply, option) = match (&answer, questions) {
            (AttentionAnswer::Dismiss, _) => {
                let thread = prompt.thread.clone();
                self.dismiss_prompt(event, Timestamp::now(), fx);
                done(fx, key);
                self.interrupt(&thread, fx);
                return;
            }
            (AttentionAnswer::Option { id }, _) => {
                match prompt.replies.iter().find(|(option, _)| option == id) {
                    Some((option, reply)) => (reply.clone(), Some(option.clone())),
                    None => {
                        refused(fx, key, "Codex didn't offer that answer.");
                        return;
                    }
                }
            }
            (AttentionAnswer::Text { text }, Some(questions)) => {
                let answers: Vec<QuestionAnswer> = questions
                    .first()
                    .map(|question| QuestionAnswer {
                        id: question.id.clone(),
                        values: vec![text.clone()],
                    })
                    .into_iter()
                    .collect();
                let reply = input_reply(&answers);
                given = Some(answers);
                (reply, None)
            }
            (AttentionAnswer::Questions { answers }, Some(_)) => {
                given = Some(answers.clone());
                (input_reply(answers), None)
            }
            (AttentionAnswer::Text { text }, None) => {
                match prompt.replies.iter().find(|(option, _)| option == DECLINE) {
                    Some((option, reply)) => {
                        follow_up = Some(text.clone()).filter(|text| !text.trim().is_empty());
                        (reply.clone(), Some(option.clone()))
                    }
                    None => {
                        refused(fx, key, "Codex can't take a typed answer here.");
                        return;
                    }
                }
            }
            (AttentionAnswer::Questions { .. }, None) => {
                refused(
                    fx,
                    key,
                    "That answers questions, and Codex asked for approval.",
                );
                return;
            }
        };
        if let (Some(option), ItemBody::Question { questions, .. }) = (&option, &prompt.item.body)
            && let Some(question) = questions.first()
        {
            given = Some(vec![QuestionAnswer {
                id: question.id.clone(),
                values: vec![option.clone()],
            }]);
        }
        let Some(mut prompt) = self.prompts.remove(&event) else {
            return;
        };
        let now = Timestamp::now();
        let declined =
            matches!(option.as_deref(), Some(DECLINE | "cancel")) && prompt.kind != Kind::Input;
        prompt.item.status = if declined {
            ItemStatus::Declined
        } else {
            ItemStatus::Completed
        };
        match &mut prompt.item.body {
            ItemBody::Approval { decided, .. } => {
                *decided = Some(Decided {
                    answer,
                    by: Some(by),
                    at: now,
                });
            }
            ItemBody::Question { answers, .. } => *answers = given,
            _ => {}
        }
        let thread = prompt.thread.clone();
        self.close_prompt(prompt, reply, now, fx);
        done(fx, key);
        if let Some(text) = follow_up {
            let message = Outgoing::new(
                Message {
                    text,
                    refs: Vec::new(),
                    attachments: Vec::new(),
                },
                None,
                now,
            );
            self.send_message(&thread, message, SendMode::Steer, fx);
        }
    }

    /// Sends a prompt's reply and closes it, with its item as it ended.
    fn close_prompt(&mut self, prompt: Prompt, reply: Value, at: Timestamp, fx: &mut Effects) {
        self.pending.remove(&prompt.key);
        let id = prompt.event.id;
        self.timers
            .retain(|(_, timer)| !matches!(timer, Timer::Expire { attention } if *attention == id));
        fx.writes.push(Write {
            line: rpc::response(&prompt.id, reply),
            answers: Some(prompt.key),
        });
        let mut item = prompt.item;
        item.completed_at = Some(at);
        upsert(fx, item);
        fx.attention.push(AttentionChange::Close(id));
    }

    /// Turns a prompt down without an answer, with the reply [`dismissal`]
    /// gives. Its item ends declined, with nothing decided or answered.
    fn dismiss_prompt(&mut self, attention: AttentionId, at: Timestamp, fx: &mut Effects) {
        let Some(mut prompt) = self.prompts.remove(&attention) else {
            return;
        };
        let reply = dismissal(&prompt);
        prompt.item.status = ItemStatus::Declined;
        self.close_prompt(prompt, reply, at, fx);
    }

    /// Dismisses every prompt on a thread, oldest first, for a message
    /// that stops its turn.
    pub(super) fn dismiss_waiting(&mut self, thread: &str, fx: &mut Effects) {
        let mut open: Vec<_> = self
            .prompts
            .iter()
            .filter(|(_, prompt)| prompt.thread == thread)
            .map(|(attention, prompt)| (prompt.event.created_at, *attention))
            .collect();
        open.sort();
        let at = Timestamp::now();
        for (_, attention) in open {
            self.dismiss_prompt(attention, at, fx);
        }
    }

    /// A question whose time ran out gets an empty answer, the way Codex's
    /// own clients do it.
    pub(super) fn expire(&mut self, attention: AttentionId, fx: &mut Effects) {
        let Some(mut prompt) = self.prompts.remove(&attention) else {
            return;
        };
        prompt.item.status = ItemStatus::Cancelled;
        self.close_prompt(prompt, input_reply(&[]), Timestamp::now(), fx);
    }
}

/// The questions in a `requestUserInput` request.
fn questions(params: &Value) -> Vec<Question> {
    params
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let text = |name: &str| {
                question
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let flag = |name: &str| question.get(name).and_then(Value::as_bool) == Some(true);
            let options = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(QuestionOption {
                        label: option.get("label")?.as_str()?.to_owned(),
                        description: option
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect();
            Some(Question {
                id: text("id")?,
                header: text("header").filter(|header| !header.is_empty()),
                text: text("question").unwrap_or_default(),
                options,
                multi: false,
                other: flag("isOther"),
                secret: flag("isSecret"),
                consent: None,
            })
        })
        .collect()
}

/// Codex's reply to a prompt you dismissed: "decline and stop" where it
/// offers one, else its decline. Questions offer neither, so they get the
/// empty answer Codex's clients send when time runs out, and the turn
/// stops by interrupt.
fn dismissal(prompt: &Prompt) -> Value {
    let declines = ["cancel", DECLINE]
        .into_iter()
        .filter(|_| prompt.kind != Kind::Input);
    declines
        .filter_map(|wanted| prompt.replies.iter().find(|(option, _)| option == wanted))
        .map(|(_, reply)| reply.clone())
        .next()
        .unwrap_or_else(|| input_reply(&[]))
}

/// The reply to `requestUserInput` for `answers`.
fn input_reply(answers: &[QuestionAnswer]) -> Value {
    let answers: Map<String, Value> = answers
        .iter()
        .map(|answer| (answer.id.clone(), json!({ "answers": answer.values })))
        .collect();
    json!({ "answers": answers })
}

/// What a command approval shows, with an option for each decision Codex
/// offers and Decline always among them.
fn command(params: &Value) -> Asked {
    let line = params
        .get("command")
        .and_then(Value::as_str)
        .map(items::unwrap_shell);
    // The title carries the line itself, since its first word can't say what the
    // rest of the command does.
    let shown = line
        .as_deref()
        .map(|line| clip(line, TITLE_CHARS))
        .filter(|shown| !shown.is_empty());
    let host = params
        .pointer("/networkApprovalContext/host")
        .and_then(Value::as_str);
    let (approval_kind, title) = match (host, &shown) {
        (Some(_), Some(shown)) => (
            ApprovalKind::Network,
            format!("Run with network access: `{shown}`?"),
        ),
        (Some(host), None) => (ApprovalKind::Network, format!("Reach {host}?")),
        (None, Some(shown)) => (ApprovalKind::Command, format!("Run `{shown}`?")),
        (None, None) => (ApprovalKind::Command, "Run a command?".to_owned()),
    };
    let detail = join_detail(
        line.map(|line| format!("$ {line}")),
        params.get("reason").and_then(Value::as_str),
    );
    let offered = match params.get("availableDecisions").and_then(Value::as_array) {
        Some(decisions) => decisions.clone(),
        None => ["accept", "acceptForSession", DECLINE, "cancel"]
            .into_iter()
            .map(Value::from)
            .collect(),
    };
    let mut options: Vec<(String, String, Value)> = offered
        .iter()
        .enumerate()
        .filter_map(|(index, decision)| decision_option(decision, index))
        .collect();
    // Codex can leave `decline` out of the list and still take it.
    if !options.iter().any(|(id, _, _)| id == DECLINE) {
        let at = options
            .iter()
            .position(|(id, _, _)| id == "cancel")
            .unwrap_or(options.len());
        let decline = decision_option(&json!(DECLINE), 0);
        options.splice(at..at, decline);
    }
    Asked {
        approval_kind,
        title,
        detail,
        summary: None,
        options,
    }
}

/// The option for one of Codex's approval decisions, with its reply.
fn decision_option(decision: &Value, index: usize) -> Option<(String, String, Value)> {
    let reply = json!({ "decision": decision });
    if let Some(name) = decision.as_str() {
        let (id, label) = match name {
            "accept" => ("accept", "Approve"),
            "acceptForSession" => ("accept_for_session", "Approve for this session"),
            "decline" => (DECLINE, "Decline"),
            "cancel" => ("cancel", "Decline and stop"),
            _ => return None,
        };
        return Some((id.to_owned(), label.to_owned(), reply));
    }
    if let Some(argv) = decision
        .pointer("/acceptWithExecpolicyAmendment/execpolicy_amendment")
        .and_then(Value::as_array)
    {
        let rule: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
        let label = format!("Always allow {}", rule.join(" "));
        return Some(("always_allow".to_owned(), label, reply));
    }
    let amendment = decision.pointer("/applyNetworkPolicyAmendment/network_policy_amendment")?;
    let host = amendment.get("host").and_then(Value::as_str)?;
    let label = match amendment.get("action").and_then(Value::as_str) {
        Some("deny") => format!("Always block {host}"),
        _ => format!("Always allow {host}"),
    };
    Some((format!("network_{index}"), label, reply))
}

/// What a request for more sandbox access shows.
fn permissions(params: &Value) -> Asked {
    let asked = params.get("permissions").unwrap_or(&Value::Null);
    let granted: Map<String, Value> = asked
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    // A title names one kind of access only when that's all the reply grants.
    let only = |name: &str| granted.len() == 1 && granted.contains_key(name);
    let (approval_kind, title) = if only("network") {
        (ApprovalKind::Network, "Allow network access?")
    } else if only("fileSystem") {
        (ApprovalKind::Other, "Allow file access?")
    } else {
        (ApprovalKind::Other, "Allow more access?")
    };
    let mut lines = Vec::new();
    for (name, value) in &granted {
        let label = match name.as_str() {
            "network" => "Network".to_owned(),
            "fileSystem" => "Files".to_owned(),
            other => visible(other),
        };
        grant_lines(&label, value, &mut lines);
    }
    let summary = (!lines.is_empty()).then(|| {
        // The cut takes what comes last, so reads go there. A semicolon in
        // a path can't pass for the next grant.
        let mut shown: Vec<String> = lines.iter().map(|line| line.replace(';', "\\;")).collect();
        shown.sort_by_key(|line| line.starts_with("Files read"));
        let mut summary = format!("{title} {}", shown.join("; "));
        if let Some((end, _)) = summary.char_indices().nth(SUMMARY_CHARS) {
            summary.truncate(end);
            summary.push('…');
        }
        summary
    });
    // The reason is the agent's words, so it comes last on a line of its
    // own and can't pass for one of the grants above it.
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .map(|reason| reason.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|reason| !reason.is_empty())
        .map(|reason| format!("The agent says: {}", visible(&reason)));
    let detail = (!lines.is_empty())
        .then(|| lines.join("\n"))
        .into_iter()
        .chain(reason)
        .collect::<Vec<_>>()
        .join("\n\n");
    let grant = |scope: &str| json!({ "permissions": granted, "scope": scope });
    let options = vec![
        (
            "grant_turn".into(),
            "Allow for this turn".into(),
            grant("turn"),
        ),
        (
            "grant_session".into(),
            "Allow for this session".into(),
            grant("session"),
        ),
        (
            DECLINE.into(),
            "Decline".into(),
            json!({ "permissions": {}, "scope": "turn" }),
        ),
    ];
    Asked {
        approval_kind,
        title: title.to_owned(),
        detail: (!detail.is_empty()).then_some(detail),
        summary,
        options,
    }
}

/// One line per thing a permission request grants: a part of an object or an
/// entry of a list under `label`, and any other value as it came.
fn grant_lines(label: &str, value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Object(parts) if parts.values().any(|part| !part.is_null()) => {
            for (part, value) in parts.iter().filter(|(_, value)| !value.is_null()) {
                grant_lines(&format!("{label} {}", visible(part)), value, lines);
            }
        }
        Value::Array(entries) if !entries.is_empty() => {
            for entry in entries {
                lines.push(format!("{label}: {}", raw(entry)));
            }
        }
        other => lines.push(format!("{label}: {}", raw(other))),
    }
}

/// A string as it is, and any other value as JSON, on one line.
fn raw(value: &Value) -> String {
    match value {
        Value::String(text) => visible(text),
        other => visible(&other.to_string()),
    }
}

/// `text` with anything that could break a line or reorder it written out,
/// so a path can't pass for a second grant.
pub(crate) fn visible(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '"' | '\'' | '\\' => c.to_string(),
            c => c.escape_debug().to_string(),
        })
        .collect()
}

/// What an MCP server's request shows. MaxHQ can't fill in a form, so it
/// offers to accept only when there's nothing to fill in.
fn elicitation(params: &Value) -> Asked {
    let server = params
        .get("serverName")
        .and_then(Value::as_str)
        .unwrap_or("An MCP server");
    let url_mode = params.get("mode").and_then(Value::as_str) == Some("url");
    let tool_call = params
        .pointer("/_meta/codex_approval_kind")
        .and_then(Value::as_str)
        == Some("mcp_tool_call");
    let empty_form = params
        .pointer("/requestedSchema/properties")
        .and_then(Value::as_object)
        .is_none_or(Map::is_empty);
    let title = if tool_call {
        format!("Let {server} run a tool?")
    } else if url_mode {
        format!("Open the page {server} asks for?")
    } else {
        format!("{server} asks for your input")
    };
    let url = params.get("url").and_then(Value::as_str).map(str::to_owned);
    let detail = join_detail(url, params.get("message").and_then(Value::as_str));
    let reply = |action: &str, content: Value| json!({ "action": action, "content": content, "_meta": null });
    let mut options = Vec::new();
    if url_mode || empty_form {
        let content = if url_mode { Value::Null } else { json!({}) };
        options.push(("accept".into(), "Allow".into(), reply("accept", content)));
    }
    options.push((
        DECLINE.into(),
        "Decline".into(),
        reply("decline", Value::Null),
    ));
    options.push((
        "cancel".into(),
        "Decline and stop".into(),
        reply("cancel", Value::Null),
    ));
    Asked {
        approval_kind: ApprovalKind::Tool,
        title,
        detail,
        summary: None,
        options,
    }
}

/// Puts the main detail and Codex's reason on separate lines.
fn join_detail(main: Option<String>, reason: Option<&str>) -> Option<String> {
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());
    match (main, reason) {
        (Some(main), Some(reason)) => Some(format!("{main}\n{reason}")),
        (Some(main), None) => Some(main),
        (None, Some(reason)) => Some(reason.to_owned()),
        (None, None) => None,
    }
}
