// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Requests the CLI sends the driver: `session/request_permission`, and
//! the plans and questions its flavor reads from the CLI's own requests.
//! Each becomes an item and an attention event until you answer. Anything
//! else gets "method not found".
//!
//! A prompt still open when its tool call ends or its turn ends was
//! settled elsewhere, so the driver answers it as cancelled and closes it.
//!
//! Cursor's 2026.10.22 recording settles the permission request: it comes
//! after its call's `in_progress` update, names the call with its title
//! and kind but no `rawInput`, and says why in `content` ("Not in
//! allowlist: cat, pwd"). Its options are allow-once, allow-always and
//! reject-once. The CLI's own request IDs start at 0 in every process, so
//! a prompt's ID comes from its tool call as well.
//!
//! The 2026.10.22 recording shows the same request for an MCP call, with
//! "<server>-<tool>: <tool>" as its title and the arguments in `content`.
//!
//! A shell command's request has no `rawInput` either, but the call's
//! update before it gives the command line. MaxHQ answers it itself when
//! [`commands::allows`] does: in Auto for a command that only reads, and
//! in any mode but Plan for one the project's rules name. Picking an
//! allow-always option saves rules for the command too.
//!
//! In Accept edits and Auto a change passes alone when every path it names
//! lands inside the agent's folders and off the files that decide what runs
//! later, as [`commands::Folders::may_change`] reads them. One that names no
//! path asks, including through what its call's updates said.

use maxhq_protocol::attention::{AttentionAnswer, AttentionEvent, AttentionKind, AttentionOption};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, IdempotencyKey};
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ApprovalKind, Decided, ItemBody, ItemId, ItemStatus, Question, QuestionAnswer, QuestionOption,
    TimelineItem,
};
use serde_json::{json, Value};

use super::events::{self, Out};
use super::rpc::{self, text};
use super::{close_attention, done, refused, AcpSession, Asked, ExtPrompt, Plan};
use crate::drive::{commands, AttentionChange, Effects, Write};

/// The option that starts on a plan.
const ACCEPT: &str = "accept";

/// The option that keeps planning.
const REJECT: &str = "reject";

/// A request waiting on your answer.
#[derive(Clone, Debug)]
pub struct Prompt {
    /// The request's ID, to echo in the answer.
    pub id: Value,
    /// The holder's key for it.
    pub key: String,
    /// What it asks.
    pub kind: Kind,
    /// Its item as last sent.
    pub item: TimelineItem,
    /// The tool call it's for.
    pub call: Option<String>,
}

/// What a prompt asks.
#[derive(Clone, Debug)]
pub enum Kind {
    /// To run a tool, with the CLI's options as ID and kind, and the
    /// command line when the tool runs one.
    Permission(Vec<(String, String)>, Option<String>),
    /// To start on a plan.
    Plan,
    /// Questions, each with its ID and its options as ID and label.
    Question(Vec<(String, Vec<(String, String)>)>),
}

/// Fields of a call's input that name a path it changes.
const CHANGED_INPUT: &[&str] = &[
    "path",
    "file_path",
    "filePath",
    "target_file",
    "source",
    "from",
    "oldPath",
    "to",
    "destination",
    "target",
    "newPath",
];

/// Every path a call says it changes: in its locations, its diffs and its input,
/// and in the `asked` content the request itself carried.
fn changed_paths<'a>(tool: &'a Value, asked: Option<&'a Value>) -> Vec<&'a str> {
    let list = |field: Option<&'a Value>| field.and_then(Value::as_array).into_iter().flatten();
    let mut paths: Vec<&str> = list(tool.get("locations"))
        .chain(list(tool.get("content")))
        .chain(list(asked))
        .filter_map(|entry| text(entry, "path"))
        .collect();
    if let Some(input) = tool.get("rawInput") {
        paths.extend(CHANGED_INPUT.iter().filter_map(|name| text(input, name)));
    }
    paths
}

impl AcpSession {
    /// Acts on a request from the CLI.
    pub(super) fn request(
        &mut self,
        id: Value,
        method: &str,
        params: &Value,
        at: Timestamp,
        replayed: bool,
        fx: &mut Effects,
    ) {
        let Some(key) = rpc::key(&id) else {
            return;
        };
        // A replayed request the holder no longer holds was answered by an
        // earlier `maxd`.
        if replayed && !self.pending.contains(&key) {
            return;
        }
        let call = text(params, "toolCallId")
            .or_else(|| {
                params
                    .pointer("/toolCall/toolCallId")
                    .and_then(Value::as_str)
            })
            .unwrap_or_default();
        let base = self.flavor.base();
        let attention = AttentionId::derive(&format!("{base}:{}:{call}:{key}", self.lead));
        if let Some(prompt) = self.prompts.get_mut(&attention) {
            prompt.id = id;
            prompt.key = key;
            return;
        }
        let agent = self.agent_for(text(params, "sessionId"));
        let call = text(params, "toolCallId").map(str::to_owned);
        let opened = if method == "session/request_permission" {
            self.permission(&id, &key, agent, params, at, fx)
        } else {
            match self.flavor.request(method, params) {
                Some(ExtPrompt::Plan(plan)) => {
                    Some(self.plan(agent, attention, call, plan, at, fx))
                }
                Some(ExtPrompt::Question { title, questions }) => {
                    let name = self.flavor.name();
                    Some(question(name, agent, attention, call, title, questions, at))
                }
                None => {
                    let message = format!("MaxHQ doesn't support {method}");
                    fx.writes.push(Write {
                        line: rpc::error(&id, rpc::METHOD_NOT_FOUND, &message),
                        answers: Some(key.clone()),
                    });
                    self.pending.remove(&key);
                    return;
                }
            }
        };
        let Some((kind, mut item, event, call)) = opened.map(|(kind, item, event, call)| {
            let event = AttentionEvent {
                id: attention,
                ..event
            };
            (kind, item, event, call)
        }) else {
            return;
        };
        item.id = ItemId(format!("{}:approval:{attention}", self.flavor.base()));
        if let ItemBody::Approval {
            attention: slot, ..
        }
        | ItemBody::Question {
            attention: slot, ..
        } = &mut item.body
        {
            *slot = attention;
        }
        if let Some(call) = &call {
            self.deadlines.remove(call);
        }
        self.pending.insert(key.clone());
        let mut out = Out::new();
        events::upsert(&mut out, item.clone());
        self.emit(out, fx);
        fx.attention.push(AttentionChange::Open(event));
        self.prompts.insert(
            attention,
            Prompt {
                id,
                key,
                kind,
                item,
                call,
            },
        );
    }

    /// The agent a request is for, by its session.
    fn agent_for(&self, session: Option<&str>) -> AgentId {
        session
            .filter(|session| self.session_id.as_deref() != Some(*session))
            .and_then(|session| {
                self.subs
                    .iter()
                    .find(|sub| sub.session.as_deref() == Some(session))
            })
            .map_or(self.lead, |sub| sub.agent)
    }

    /// A permission request, answered at once when the mode allows it.
    fn permission(
        &mut self,
        id: &Value,
        key: &str,
        agent: AgentId,
        params: &Value,
        at: Timestamp,
        fx: &mut Effects,
    ) -> Option<Opened> {
        let mut tool = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let call = text(&tool, "toolCallId").map(str::to_owned);
        // The request's `content` says why the CLI asks, not what the call
        // did, so it never reaches the call's item.
        let asked = tool
            .as_object_mut()
            .and_then(|fields| fields.remove("content"));
        let reason = asked
            .as_ref()
            .and_then(|content| events::content_text(Some(content)));
        if let Some(call) = &call {
            // A call the stream already knows keeps what its updates said,
            // and the request fills in only what it leaves out. A sub-agent's
            // updates come on its own stream.
            let known = if agent == self.lead {
                self.stream.tool_raw(call)
            } else {
                self.subs
                    .iter()
                    .find(|sub| sub.agent == agent)
                    .and_then(|sub| sub.stream.tool_raw(call))
            };
            match known {
                Some(known) => {
                    if let Some(fields) = tool.as_object_mut() {
                        for (name, value) in known {
                            fields.entry(name.clone()).or_insert_with(|| value.clone());
                        }
                    }
                }
                None if agent == self.lead => {
                    let mut out = Out::new();
                    self.begin_if_needed();
                    self.stream.tool(&tool, at, &mut out);
                    self.emit(out, fx);
                }
                None => {}
            }
        }
        let mut options: Vec<(String, String, String)> = params
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                Some((
                    text(option, "optionId")?.to_owned(),
                    text(option, "kind").unwrap_or_default().to_owned(),
                    text(option, "name").unwrap_or_default().to_owned(),
                ))
            })
            .collect();
        let tool_kind = text(&tool, "kind").unwrap_or("other");
        let command = (tool_kind == "execute")
            .then(|| tool.pointer("/rawInput/command").and_then(Value::as_str))
            .flatten()
            .map(str::to_owned);
        // The next run would ask again where rules can't cover the line, so
        // the choice isn't offered there.
        if command
            .as_deref()
            .is_some_and(|line| !commands::always_allow_holds(line, &self.folders()))
            && options.iter().any(|(_, kind, _)| kind == "allow_once")
        {
            options.retain(|(_, kind, _)| kind != "allow_always");
        }
        // The agent tools' own permission decides their calls, so MaxHQ
        // doesn't ask you as well, as Claude's are allowed too. The request
        // names the server only in its title, so the call's own updates
        // say which server it is.
        let auto = tool.as_object().is_some_and(events::ours)
            || match self.effective {
                PermissionMode::FullAccess => true,
                PermissionMode::Plan => false,
                mode => {
                    // A change that names no path could be any file, so it asks.
                    let edits = matches!(mode, PermissionMode::AcceptEdits | PermissionMode::Auto)
                        && matches!(tool_kind, "edit" | "delete" | "move")
                        && {
                            let folders = self.folders();
                            let paths = changed_paths(&tool, asked.as_ref());
                            !paths.is_empty()
                                && paths.into_iter().all(|path| folders.may_change(path))
                        };
                    let auto = mode == PermissionMode::Auto;
                    edits
                        || command.as_deref().is_some_and(|line| {
                            commands::allows(line, auto, &self.command_rules, &self.folders())
                        })
                }
            };
        if auto
            && let Some((option, _, _)) = options
                .iter()
                .find(|(_, kind, _)| kind == "allow_once")
                .or_else(|| {
                    options
                        .iter()
                        .find(|(_, kind, _)| kind.starts_with("allow"))
                })
        {
            fx.writes.push(Write {
                line: rpc::response(id, rpc::selected(option)),
                answers: Some(key.to_owned()),
            });
            self.pending.remove(key);
            return None;
        }
        let title = text(&tool, "title")
            .filter(|title| !title.is_empty())
            .map_or_else(
                || format!("{} asks to use a tool", self.flavor.name()),
                str::to_owned,
            );
        let what = tool
            .pointer("/rawInput/command")
            .and_then(Value::as_str)
            .or_else(|| {
                tool.get("locations")
                    .and_then(Value::as_array)
                    .and_then(|list| list.first())
                    .and_then(|location| text(location, "path"))
            });
        let detail = match (what, reason) {
            (Some(what), Some(reason)) => Some(format!("{what}\n{reason}")),
            (what, reason) => what.map(str::to_owned).or(reason),
        };
        let approval_kind = match tool_kind {
            "execute" => ApprovalKind::Command,
            "edit" | "delete" | "move" => ApprovalKind::FileChange,
            "read" => ApprovalKind::FileRead,
            "fetch" => ApprovalKind::Network,
            "other" => ApprovalKind::Tool,
            _ => ApprovalKind::Other,
        };
        let choices: Vec<AttentionOption> = options
            .iter()
            .map(|(id, kind, name)| AttentionOption {
                id: id.clone(),
                label: if name.is_empty() {
                    id.clone()
                } else {
                    name.clone()
                },
                kind: matches!(
                    kind.as_str(),
                    "allow_once" | "allow_always" | "reject_once" | "reject_always"
                )
                .then(|| kind.clone()),
            })
            .collect();
        let item = waiting(
            agent,
            self.stream.turn_id(),
            at,
            ItemBody::Approval {
                attention: AttentionId::derive("unset"),
                approval_kind,
                title: title.clone(),
                detail,
                cwd: Some(self.cwd()),
                options: choices.clone(),
                decided: None,
            },
        );
        let event = event(agent, AttentionKind::Permission, title, choices, at);
        let kind = Kind::Permission(
            options
                .into_iter()
                .map(|(id, kind, _)| (id, kind))
                .collect(),
            command,
        );
        Some((kind, item, event, call))
    }

    /// A plan to approve, which also shows as the plan and its todos.
    fn plan(
        &mut self,
        agent: AgentId,
        attention: AttentionId,
        call: Option<String>,
        plan: Plan,
        at: Timestamp,
        fx: &mut Effects,
    ) -> Opened {
        let Plan {
            markdown,
            todos,
            detail,
        } = plan;
        if agent == self.lead {
            let mut out = Out::new();
            self.begin_if_needed();
            self.stream.close_text(at, &mut out);
            let plan = self.stream.item(
                ItemId(format!("{}:plan:{attention}", self.flavor.base())),
                ItemStatus::Completed,
                at,
                ItemBody::Plan {
                    markdown,
                    streaming: false,
                },
            );
            events::upsert(&mut out, plan);
            if !todos.is_empty() {
                self.stream.todos(todos, false, at, &mut out);
            }
            self.emit(out, fx);
        }
        let title = "Ready to code?".to_owned();
        let options = vec![
            AttentionOption {
                id: ACCEPT.to_owned(),
                label: "Start coding".to_owned(),
                kind: None,
            },
            AttentionOption {
                id: REJECT.to_owned(),
                label: "Keep planning".to_owned(),
                kind: None,
            },
        ];
        let item = waiting(
            agent,
            self.stream.turn_id(),
            at,
            ItemBody::Approval {
                attention,
                approval_kind: ApprovalKind::Plan,
                title: title.clone(),
                detail,
                cwd: Some(self.cwd()),
                options: options.clone(),
                decided: None,
            },
        );
        let event = event(agent, AttentionKind::Permission, title, options, at);
        (Kind::Plan, item, event, call)
    }

    /// Answers a prompt you settled.
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
        if matches!(answer, AttentionAnswer::Dismiss) {
            self.dismiss(Some(event), Timestamp::now(), fx);
            done(fx, key);
            return;
        }
        let mut follow_up = None;
        let reply = match (&prompt.kind, &answer) {
            (Kind::Permission(options, command), AttentionAnswer::Option { id }) => {
                let picked = options.iter().find(|(option, _)| option == id);
                // The CLI keeps its own rule as well, but MaxHQ's covers the
                // command for every agent in the project, after a restart
                // too.
                if let Some(command) = command
                    && picked.is_some_and(|(_, kind)| kind == "allow_always")
                {
                    fx.command_rules
                        .extend(commands::rules_for(command, &self.folders()));
                }
                picked.map(|_| (rpc::selected(id), declines(options, id)))
            }
            (Kind::Permission(options, _), AttentionAnswer::Text { text }) => {
                let reject = options
                    .iter()
                    .find(|(_, kind)| kind == "reject_once")
                    .or_else(|| options.iter().find(|(_, kind)| kind.starts_with("reject")));
                reject.map(|(option, _)| {
                    follow_up = Some(text.clone());
                    (rpc::selected(option), true)
                })
            }
            (Kind::Plan, AttentionAnswer::Option { id }) if id == ACCEPT => {
                Some((json!({ "outcome": { "outcome": "accepted" } }), false))
            }
            (Kind::Plan, AttentionAnswer::Option { id }) if id == REJECT => {
                Some((json!({ "outcome": { "outcome": "rejected" } }), true))
            }
            (Kind::Plan, AttentionAnswer::Text { text }) => Some((
                json!({ "outcome": { "outcome": "rejected", "reason": text } }),
                true,
            )),
            (Kind::Question(questions), AttentionAnswer::Questions { answers }) => {
                let answered: Vec<Value> = questions
                    .iter()
                    .map(|(id, options)| {
                        let picked: Vec<&str> = answers
                            .iter()
                            .filter(|answer| answer.id == *id)
                            .flat_map(|answer| answer.values.iter())
                            .filter_map(|value| {
                                options
                                    .iter()
                                    .find(|(_, label)| label == value)
                                    .map(|(option, _)| option.as_str())
                            })
                            .collect();
                        json!({ "questionId": id, "selectedOptionIds": picked })
                    })
                    .collect();
                Some((
                    json!({ "outcome": { "outcome": "answered", "answers": answered } }),
                    false,
                ))
            }
            (Kind::Question(_), AttentionAnswer::Text { text }) => Some((
                json!({ "outcome": { "outcome": "skipped", "reason": text } }),
                false,
            )),
            _ => None,
        };
        let Some((reply, declined)) = reply else {
            refused(fx, key, "That answer doesn't fit this question.");
            return;
        };
        let at = Timestamp::now();
        let status = if declined {
            ItemStatus::Declined
        } else {
            ItemStatus::Completed
        };
        let given = match answer.clone() {
            AttentionAnswer::Questions { answers } => Some(answers),
            AttentionAnswer::Text { text: value } | AttentionAnswer::Option { id: value } => {
                Some(vec![QuestionAnswer {
                    id: String::new(),
                    values: vec![value],
                }])
            }
            AttentionAnswer::Dismiss => None,
        };
        let decided = Decided {
            answer,
            by: Some(by),
            at,
        };
        self.close_prompt(event, reply, status, at, fx, |body| match body {
            ItemBody::Approval { decided: slot, .. } => *slot = Some(decided),
            ItemBody::Question { answers, .. } => *answers = given,
            _ => {}
        });
        done(fx, key);
        if let Some(text) = follow_up {
            self.follow_up(key, text, fx);
        }
    }

    /// Answers a prompt and closes it.
    fn close_prompt(
        &mut self,
        attention: AttentionId,
        reply: Value,
        status: ItemStatus,
        at: Timestamp,
        fx: &mut Effects,
        edit: impl FnOnce(&mut ItemBody),
    ) {
        let Some(mut prompt) = self.prompts.remove(&attention) else {
            return;
        };
        fx.writes.push(Write {
            line: rpc::response(&prompt.id, reply),
            answers: Some(prompt.key.clone()),
        });
        self.pending.remove(&prompt.key);
        prompt.item.status = status;
        prompt.item.completed_at = Some(at);
        edit(&mut prompt.item.body);
        let mut out = Out::new();
        events::upsert(&mut out, prompt.item);
        self.emit(out, fx);
        close_attention(fx, attention);
    }

    /// Ends the turn as Esc does: `session/cancel`, then ACP's `cancelled`
    /// outcome for what still waits. The prompts you dismissed, `only` or
    /// all of them, close declined with nothing decided or answered; the
    /// rest close as cancelled with the turn.
    pub(super) fn dismiss(&mut self, only: Option<AttentionId>, at: Timestamp, fx: &mut Effects) {
        let dismissed: Vec<AttentionId> = self
            .prompts
            .keys()
            .copied()
            .filter(|attention| only.is_none_or(|one| one == *attention))
            .collect();
        self.write_cancel(fx);
        for attention in dismissed {
            self.close_prompt(
                attention,
                rpc::cancelled(),
                ItemStatus::Declined,
                at,
                fx,
                |_| {},
            );
        }
        self.drop_prompts(at, fx);
    }

    /// Answers every open prompt as cancelled, as a turn ends.
    pub(super) fn drop_prompts(&mut self, at: Timestamp, fx: &mut Effects) {
        let open: Vec<AttentionId> = self.prompts.keys().copied().collect();
        for attention in open {
            self.close_prompt(
                attention,
                rpc::cancelled(),
                ItemStatus::Cancelled,
                at,
                fx,
                |_| {},
            );
        }
    }

    /// Closes the prompts for a tool call that ended, which were settled
    /// elsewhere.
    pub(super) fn prompts_done_for(&mut self, call: &str, at: Timestamp, fx: &mut Effects) {
        let open: Vec<AttentionId> = self
            .prompts
            .iter()
            .filter(|(_, prompt)| prompt.call.as_deref() == Some(call))
            .map(|(id, _)| *id)
            .collect();
        for attention in open {
            self.close_prompt(
                attention,
                rpc::cancelled(),
                ItemStatus::Cancelled,
                at,
                fx,
                |_| {},
            );
        }
    }
}

/// What a request opens: its kind, its item, its event and its tool call.
type Opened = (Kind, TimelineItem, AttentionEvent, Option<String>);

/// Whether picking `id` says no.
fn declines(options: &[(String, String)], id: &str) -> bool {
    options
        .iter()
        .any(|(option, kind)| option == id && kind.starts_with("reject"))
}

/// Questions the CLI `name` asks.
fn question(
    name: &str,
    agent: AgentId,
    attention: AttentionId,
    call: Option<String>,
    title: Option<String>,
    questions: Vec<Asked>,
    at: Timestamp,
) -> Opened {
    let mut asked = Vec::new();
    let mut kinds = Vec::new();
    for Asked {
        id,
        text,
        options,
        multi,
    } in questions
    {
        asked.push(Question {
            id: id.clone(),
            header: None,
            text,
            options: options
                .iter()
                .map(|(_, label)| QuestionOption {
                    label: label.clone(),
                    description: None,
                })
                .collect(),
            multi,
            other: false,
            secret: false,
            consent: None,
        });
        kinds.push((id, options));
    }
    let summary = title
        .or_else(|| asked.first().map(|question| question.text.clone()))
        .unwrap_or_else(|| format!("{name} has a question"));
    let item = waiting(
        agent,
        None,
        at,
        ItemBody::Question {
            attention,
            questions: asked,
            answers: None,
        },
    );
    let event = event(agent, AttentionKind::Question, summary, Vec::new(), at);
    (Kind::Question(kinds), item, event, call)
}

/// An item waiting on you.
fn waiting(
    agent: AgentId,
    turn: Option<maxhq_protocol::timeline::TurnId>,
    at: Timestamp,
    body: ItemBody,
) -> TimelineItem {
    TimelineItem {
        id: ItemId(String::new()),
        agent,
        turn,
        parent: None,
        ordinal: 0,
        status: ItemStatus::Waiting,
        started_at: Some(at),
        completed_at: None,
        body,
    }
}

/// An attention event, before it has its ID.
fn event(
    agent: AgentId,
    kind: AttentionKind,
    summary: String,
    options: Vec<AttentionOption>,
    at: Timestamp,
) -> AttentionEvent {
    AttentionEvent {
        id: AttentionId::derive("unset"),
        agent,
        kind,
        summary,
        options,
        accepts_text: true,
        created_at: at,
        expires_at: None,
    }
}
