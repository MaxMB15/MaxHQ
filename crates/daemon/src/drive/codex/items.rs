// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Turns Codex thread items into timeline items. The app-server sends them
//! in camel case, as in `{"type":"commandExecution","aggregatedOutput":..}`,
//! and rollouts write the same items in Codex's own shape, as in
//! `{"type":"CommandExecution","aggregated_output":..}`. Both carry the same
//! IDs, so one reader takes either, matching type names and fields with
//! case and underscores ignored.

use std::path::Path;

use maxhq_core::context;
use maxhq_core::names;
use maxhq_core::tools::{self, Tool};
use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{AccountId, AgentId, AttachmentId};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    AttachmentKind, AttachmentRef, Delivery, FileEdit, FileOp, ItemBody, ItemId, ItemStatus,
    NoticeLevel, TimelineItem, TurnId, WebAction,
};
use serde_json::Value;

use crate::sidecar;

/// The longest title taken from a prompt.
const TITLE_CHARS: usize = 60;

/// The timeline ID for a Codex item ID.
pub fn item_id(native: &str) -> ItemId {
    ItemId(format!("codex:{native}"))
}

/// The ID of a message sent from here, for the `clientUserMessageId` it
/// goes out with. It shows under this ID from the moment you send it.
pub fn local_id(client_id: &str) -> ItemId {
    ItemId(format!("codex:local:{client_id}"))
}

/// The ID of the files sent with MaxHQ's send tool in the call `call`.
pub fn sent_id(call: &str) -> ItemId {
    ItemId(format!("codex:{call}:sent"))
}

/// The ID of the file a call to MaxHQ's open tool opened.
pub fn opened_id(call: &str) -> ItemId {
    ItemId(format!("codex:{call}:opened"))
}

/// The ID of the page a call to MaxHQ's show tool showed.
pub fn visual_id(call: &str) -> ItemId {
    ItemId(format!("codex:{call}:visual"))
}

/// The ID of a turn's closing summary.
pub fn summary_id(turn: &str) -> ItemId {
    ItemId(format!("codex:turn:{turn}"))
}

/// The ID of the marker for an interrupted turn.
pub fn interrupted_id(turn: &str) -> ItemId {
    ItemId(format!("codex:interrupted:{turn}"))
}

/// The ID of a turn's error, which a retry replaces.
pub fn error_id(turn: &str) -> ItemId {
    ItemId(format!("codex:error:{turn}"))
}

/// One Codex item, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Native {
    /// Codex's ID for it.
    pub id: String,
    /// The type, lower case without underscores, such as `usermessage`.
    pub kind: String,
    /// Where it stands.
    pub status: ItemStatus,
    /// What it shows.
    pub body: ItemBody,
    /// For a user message, the `clientId` sent with it.
    pub client_id: Option<String>,
    /// For a sub-agent, its thread.
    pub spawned: Option<String>,
}

/// Reads one item. `done` says it came from `item/completed` or a rollout,
/// so anything without a status of its own is finished.
pub fn native(item: &Value, done: bool) -> Option<Native> {
    let kind_name = item.get("type")?.as_str()?;
    let id = item.get("id")?.as_str()?.to_owned();
    let kind = squash(kind_name);
    let mut spawned = None;
    let mut status = status(item, done);
    let body = match kind.as_str() {
        "usermessage" => {
            status = ItemStatus::Completed;
            user_message(item)
        }
        "agentmessage" => ItemBody::AssistantMessage {
            text: message_text(item),
            streaming: !done,
        },
        "reasoning" => ItemBody::Reasoning {
            text: reasoning_text(item),
            streaming: !done,
        },
        "plan" => ItemBody::Plan {
            markdown: text(item, &["text"]).unwrap_or_default(),
            streaming: !done,
        },
        "commandexecution" => command(item),
        "filechange" => ItemBody::FileChange {
            changes: file_changes(item),
        },
        "mcptoolcall" => ItemBody::ToolCall {
            server: text(item, &["server"]),
            name: text(item, &["tool"]).unwrap_or_default(),
            title: None,
            input: field(item, &["arguments"]).map(compact),
            output: mcp_output(item),
            images: Vec::new(),
            artifact: None,
        },
        "dynamictoolcall" => {
            let (server, name) = dynamic_tool(item, "tool");
            ItemBody::ToolCall {
                server,
                name,
                title: None,
                input: field(item, &["arguments"]).map(compact),
                output: content_text(field(item, &["contentItems", "content_items"])),
                images: Vec::new(),
                artifact: None,
            }
        }
        "collabagenttoolcall" => {
            let (body, child) = collab(item);
            spawned = child;
            body
        }
        "subagentactivity" => {
            spawned = text(item, &["agentThreadId", "agent_thread_id"]);
            status = match text(item, &["kind"]).as_deref() {
                Some("started") if !done => ItemStatus::Running,
                Some("interrupted") => ItemStatus::Interrupted,
                _ => ItemStatus::Completed,
            };
            ItemBody::Subagent {
                subagent: None,
                agent_type: None,
                description: text(item, &["agentPath", "agent_path"]).unwrap_or_default(),
                result: None,
            }
        }
        "websearch" => web_search(item),
        "imageview" => {
            let path = text(item, &["path"]).unwrap_or_default();
            ItemBody::FileRead {
                images: shown(&path),
                path,
                lines: None,
            }
        }
        "sleep" => ItemBody::ToolCall {
            server: None,
            name: "sleep".into(),
            title: Some("Wait".into()),
            input: field(item, &["durationMs", "duration_ms"])
                .and_then(Value::as_u64)
                .map(|ms| format!("{ms} ms")),
            output: None,
            images: Vec::new(),
            artifact: None,
        },
        "imagegeneration" => ItemBody::ToolCall {
            server: None,
            name: "image_generation".into(),
            title: None,
            input: text(item, &["revisedPrompt", "revised_prompt"]),
            output: text(item, &["savedPath", "saved_path", "failure"]),
            images: shown(&text(item, &["savedPath", "saved_path"]).unwrap_or_default()),
            artifact: None,
        },
        "enteredreviewmode" => notice("Review started", item),
        "exitedreviewmode" => notice("Review finished", item),
        "contextcompaction" => ItemBody::Compaction {
            before_tokens: None,
            after_tokens: None,
        },
        "hookprompt" => ItemBody::Notice {
            level: NoticeLevel::Info,
            message: fragments(item),
        },
        "functioncalloutput" => {
            let (server, name) = dynamic_tool(item, "name");
            ItemBody::ToolCall {
                server,
                name,
                title: None,
                input: None,
                output: match field(item, &["output"]) {
                    Some(Value::String(output)) => Some(output.clone()),
                    other => content_text(other),
                },
                images: Vec::new(),
                artifact: None,
            }
        }
        _ => ItemBody::Unknown {
            native_kind: kind_name.to_owned(),
        },
    };
    Some(Native {
        id,
        kind,
        status,
        body,
        client_id: text(item, &["clientId", "client_id"]),
        spawned,
    })
}

/// A timeline item for `native` on `agent`'s timeline. A sub-agent item
/// names the child agent the way the observer does, and a user message
/// that carries the client ID it went out with keeps its local item's ID.
pub fn timeline(
    agent: AgentId,
    turn: Option<&str>,
    native: Native,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
) -> TimelineItem {
    let mut body = native.body;
    if let (Some(child), ItemBody::Subagent { subagent, .. }) = (&native.spawned, &mut body) {
        *subagent = Some(names::subagent(agent, child));
    }
    let id = match (&native.client_id, &body) {
        (Some(client_id), ItemBody::UserMessage { .. }) => local_id(client_id),
        _ => item_id(&native.id),
    };
    TimelineItem {
        id,
        agent,
        turn: turn.map(|turn| TurnId(turn.to_owned())),
        parent: None,
        ordinal: 0,
        status: native.status,
        started_at,
        completed_at,
        body,
    }
}

/// The timeline items for a `Turn` object from `thread/turns/list`, with
/// its closing marker. Items in it carry no times of their own, so they
/// take the turn's. Items still running stay so only when `active` says
/// the turn is, and an empty reasoning never shows. Pictures MCP tools gave
/// back are kept as [`keep_pictures`] keeps them. A failure from a login
/// that ran out names `account`.
pub fn turn(
    agent: AgentId,
    account: Option<AccountId>,
    turn: &Value,
    active: bool,
    pictures: Option<&crate::drive::pictures::Pictures>,
    kept: &mut crate::drive::pictures::Kept,
) -> Vec<TimelineItem> {
    let Some(id) = turn.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let secs = |name: &str| {
        turn.get(name)
            .and_then(Value::as_u64)
            .map(|secs| Timestamp(secs.saturating_mul(1000)))
    };
    let started_at = secs("startedAt");
    let completed_at = secs("completedAt");
    let mut out = Vec::new();
    for item in turn
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(mut found) = native(item, true) else {
            continue;
        };
        keep_pictures(item, &mut found.body, agent, pictures, kept);
        if let ItemBody::Reasoning { text, .. } = &found.body
            && text.is_empty()
        {
            continue;
        }
        if found.status == ItemStatus::Running && !active {
            found.status = ItemStatus::Interrupted;
        }
        let files = sent(agent, Some(id), item, completed_at);
        let running = found.status == ItemStatus::Running;
        let mut item = timeline(agent, Some(id), found, started_at, completed_at);
        if running {
            item.completed_at = None;
        }
        out.push(item);
        out.extend(files);
    }
    match turn.get("status").and_then(Value::as_str) {
        Some("completed") => out.push(summary(
            agent,
            id,
            turn.get("durationMs").and_then(Value::as_u64),
            None,
            started_at,
            completed_at,
        )),
        Some("interrupted") => out.push(interrupted(agent, id, completed_at)),
        Some("failed") => {
            let error = turn.get("error").cloned().unwrap_or(Value::Null);
            out.push(failure(
                agent,
                account,
                id,
                &error,
                None,
                false,
                completed_at,
            ));
        }
        _ => {}
    }
    out
}

/// What a finished call to MaxHQ's send, open or show tool showed, read
/// back from the call's own item. A send's result names where MaxHQ keeps
/// each copy, an open's names the file, and a show's names its page. None for any other item, and for a
/// call that failed.
pub fn sent(
    agent: AgentId,
    turn: Option<&str>,
    item: &Value,
    at: Option<Timestamp>,
) -> Option<TimelineItem> {
    let tool = Tool::from_codex(
        text(item, &["namespace"]).as_deref(),
        &text(item, &["tool"])?,
    )?;
    if squash(&text(item, &["type"])?) != "dynamictoolcall"
        || !tool.shows()
        || field(item, &["success"]).and_then(Value::as_bool) != Some(true)
    {
        return None;
    }
    let said = content_text(field(item, &["contentItems", "content_items"]))?;
    let call = text(item, &["id"])?;
    let (id, body) = if tool == Tool::OpenFile {
        let opened = tools::parse_opened(&said)?;
        let body = ItemBody::AgentOpenedFile {
            path: opened.path,
            line: opened.line,
            outside: opened.outside,
        };
        (opened_id(&call), body)
    } else if tool == Tool::ShowVisual {
        let shown = tools::parse_visual(&said)?;
        let body = ItemBody::AgentVisual {
            title: shown.title,
            attachment: shown.attachment,
            replaces: shown.replaces,
        };
        (visual_id(&call), body)
    } else {
        let mut attachments = tools::parse_sent(&said)?;
        sidecar::add_previews(&mut attachments);
        let arguments = match field(item, &["arguments"]) {
            Some(Value::String(json)) => serde_json::from_str(json).unwrap_or(Value::Null),
            Some(arguments) => arguments.clone(),
            None => Value::Null,
        };
        let message = text(&arguments, &["message"]).filter(|message| !message.trim().is_empty());
        let body = ItemBody::AgentAttachments {
            text: message.unwrap_or_default(),
            attachments,
        };
        (sent_id(&call), body)
    };
    Some(TimelineItem {
        id,
        agent,
        turn: turn.map(|turn| TurnId(turn.to_owned())),
        parent: None,
        ordinal: 0,
        status: ItemStatus::Completed,
        started_at: at,
        completed_at: at,
        body,
    })
}

/// The Codex IDs of the finished items in a `Turn` object from
/// `thread/turns/list`. In a running turn only items whose own status says
/// so count, since a message still streaming carries no status.
pub fn settled(turn: &Value, active: bool) -> Vec<String> {
    turn.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| !active || item.get("status").and_then(Value::as_str).is_some())
        .filter_map(|item| native(item, true))
        .filter(|found| found.status != ItemStatus::Running)
        .map(|found| found.id)
        .collect()
}

/// The summary that closes a finished turn.
pub fn summary(
    agent: AgentId,
    turn: &str,
    duration_ms: Option<u64>,
    context_tokens: Option<u64>,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
) -> TimelineItem {
    marker(
        agent,
        turn,
        summary_id(turn),
        ItemBody::TurnSummary {
            duration_ms,
            context_tokens,
        },
        started_at,
        completed_at,
    )
}

/// The marker for an interrupted turn.
pub fn interrupted(agent: AgentId, turn: &str, at: Option<Timestamp>) -> TimelineItem {
    marker(
        agent,
        turn,
        interrupted_id(turn),
        ItemBody::Interrupted,
        at,
        at,
    )
}

/// What a turn's error shows: a wait for the limit to reset when the
/// account ran out, or the error, which names `account`, the one the turn
/// ran under, when its login ran out.
pub fn failure(
    agent: AgentId,
    account: Option<AccountId>,
    turn: &str,
    error: &Value,
    limit: Option<(Option<AccountId>, Option<Timestamp>)>,
    retrying: bool,
    at: Option<Timestamp>,
) -> TimelineItem {
    let body = match limit {
        Some((account, resets_at)) => ItemBody::LimitWait {
            account,
            resets_at,
            auto_resume: false,
        },
        None => ItemBody::Error {
            message: text(error, &["message"]).unwrap_or_else(|| "Codex hit an error".into()),
            retrying,
            sign_in: account.filter(|_| is_unauthorized(error)),
        },
    };
    let mut item = marker(agent, turn, error_id(turn), body, at, at);
    if retrying {
        item.status = ItemStatus::Running;
        item.completed_at = None;
    } else {
        item.status = ItemStatus::Failed;
    }
    item
}

/// Whether a turn's error says the account ran out of its limit.
pub fn limit_hit(error: &Value) -> bool {
    error_name(error).as_deref() == Some("usagelimitexceeded")
}

/// Whether a turn's error says the account's login ran out or was turned
/// down, so signing in again fixes it.
pub fn is_unauthorized(error: &Value) -> bool {
    error_name(error).as_deref() == Some("unauthorized")
}

/// The kind a turn's error names in `codexErrorInfo`, squashed, in any
/// casing. It's a string, or an object keyed by the kind.
fn error_name(error: &Value) -> Option<String> {
    let name = match field(error, &["codexErrorInfo", "codex_error_info"])? {
        Value::String(name) => name.clone(),
        Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
        _ => return None,
    };
    Some(squash(&name))
}

/// A finished item that belongs to a turn rather than to Codex.
fn marker(
    agent: AgentId,
    turn: &str,
    id: ItemId,
    body: ItemBody,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
) -> TimelineItem {
    TimelineItem {
        id,
        agent,
        turn: Some(TurnId(turn.to_owned())),
        parent: None,
        ordinal: 0,
        status: ItemStatus::Completed,
        started_at,
        completed_at,
        body,
    }
}

/// A title from the first line of a prompt.
pub fn title(prompt: &str) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().nth(TITLE_CHARS).is_some() {
        title.truncate(title.trim_end().len());
        title.push('…');
    }
    Some(title)
}

/// What you typed in a user message, its attachments and its pills. The
/// attachments are those its text names on lines of their own or in its
/// context block, then the `images` it sent by path that the text doesn't
/// name, such as those in a message sent before MaxHQ held the thread.
pub fn typed_and_attached(
    text: &str,
    images: &[String],
) -> (String, Vec<AttachmentRef>, Vec<InlineRef>) {
    let parsed = context::parse(text, |path| AttachmentId::derive(&format!("codex:{path}")));
    let mut attachments: Vec<AttachmentRef> = parsed
        .files
        .iter()
        .map(|line| attachment_ref(line.kind, line.name.clone(), &line.path))
        .collect();
    for path in images {
        if !parsed.files.iter().any(|line| line.path == *path) {
            let name = path.rsplit('/').next().unwrap_or(path).to_owned();
            attachments.push(attachment_ref(AttachmentKind::Image, name, path));
        }
    }
    (parsed.text, attachments, parsed.refs)
}

/// An attachment Codex names by path, with the preview kept beside a
/// picture when this device has it.
fn attachment_ref(kind: AttachmentKind, name: String, path: &str) -> AttachmentRef {
    let image = kind == AttachmentKind::Image;
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let mime = match extension.as_deref() {
        Some("png") if image => Some("image/png"),
        Some("jpg" | "jpeg") if image => Some("image/jpeg"),
        Some("gif") if image => Some("image/gif"),
        Some("webp") if image => Some("image/webp"),
        _ => None,
    };
    let thumbnail = (image && path.starts_with('/'))
        .then(|| sidecar::thumbnail_of(Path::new(path)))
        .flatten();
    AttachmentRef {
        id: AttachmentId::derive(&format!("codex:{path}")),
        name,
        kind,
        bytes: 0,
        mime: mime.map(str::to_owned),
        path: path.to_owned(),
        thumbnail,
    }
}

/// A name in lower case without underscores, so `commandExecution` and
/// `CommandExecution` match.
pub fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The first of `names` that `value` has and isn't null.
fn field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names
        .iter()
        .find_map(|name| value.get(*name).filter(|found| !found.is_null()))
}

/// The first of `names` that `value` has as a string.
fn text(value: &Value, names: &[&str]) -> Option<String> {
    field(value, names)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// A dynamic tool call's namespace and the tool's name, from the item's
/// `namespace` and `name_field`. A thread started before MaxHQ's tools
/// went in the [`tools::SERVER`] namespace calls them by their flat
/// [`tools::CODEX_PREFIX`] names, and those show as the namespaced ones
/// do, so the app knows them for MaxHQ's own.
fn dynamic_tool(item: &Value, name_field: &str) -> (Option<String>, String) {
    let namespace = text(item, &["namespace"]);
    let name = text(item, &[name_field]).unwrap_or_default();
    match namespace {
        None => match Tool::from_codex(None, &name) {
            Some(tool) => (Some(tools::SERVER.to_owned()), tool.name().to_owned()),
            None => (None, name),
        },
        namespace => (namespace, name),
    }
}

/// A value as compact JSON, or itself when it's a string.
fn compact(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Where an item stands. Items without a status of their own are running
/// until they finish.
fn status(item: &Value, done: bool) -> ItemStatus {
    match text(item, &["status"])
        .map(|status| squash(&status))
        .as_deref()
    {
        Some("inprogress" | "running" | "pendinginit") => ItemStatus::Running,
        Some("failed" | "errored") => ItemStatus::Failed,
        Some("declined") => ItemStatus::Declined,
        Some("interrupted") => ItemStatus::Interrupted,
        Some(_) => ItemStatus::Completed,
        None if done => ItemStatus::Completed,
        None => ItemStatus::Running,
    }
}

/// A user message, with the attachments it names, its images and its
/// pills. The label before a numbered image, such as "Image 1:", isn't
/// part of what you typed.
fn user_message(item: &Value) -> ItemBody {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    let parts: Vec<&Value> = field(item, &["content"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let kind = |part: &Value| text(part, &["type"]).map(|kind| squash(&kind));
    for (at, part) in parts.iter().enumerate() {
        match kind(part).as_deref() {
            Some("text") => {
                let said = text(part, &["text"]);
                let labels = parts
                    .get(at + 1)
                    .and_then(|next| kind(next))
                    .is_some_and(|next| matches!(next.as_str(), "localimage" | "image"));
                if !(labels && said.as_deref().is_some_and(context::is_label)) {
                    texts.extend(said);
                }
            }
            Some("localimage") => images.extend(text(part, &["path"])),
            Some("image") => images.extend(text(part, &["url", "image_url"])),
            Some("mention" | "skill") => texts.extend(text(part, &["path", "name"])),
            _ => {}
        }
    }
    let (text, attachments, refs) = typed_and_attached(&texts.join("\n"), &images);
    ItemBody::UserMessage {
        text,
        attachments,
        refs,
        delivery: Delivery::Sent,
    }
}

/// An assistant message's text, from `text` or from its content parts.
fn message_text(item: &Value) -> String {
    if let Some(text) = text(item, &["text"]) {
        return text;
    }
    content_text(field(item, &["content"])).unwrap_or_default()
}

/// The text in a list of content parts, joined.
fn content_text(parts: Option<&Value>) -> Option<String> {
    let texts: Vec<&str> = parts?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// Reasoning shows its summary, or its raw text when there's no summary.
fn reasoning_text(item: &Value) -> String {
    let parts = |names: &[&str]| -> Vec<String> {
        field(item, names)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| match part {
                Value::String(text) => Some(text.clone()),
                other => other.get("text").and_then(Value::as_str).map(str::to_owned),
            })
            .filter(|text| !text.is_empty())
            .collect()
    };
    let summary = parts(&["summary", "summary_text"]);
    if !summary.is_empty() {
        return summary.join("\n\n");
    }
    parts(&["content", "raw_content"]).join("\n\n")
}

/// A command, or the file read or search it is when it does only that.
fn command(item: &Value) -> ItemBody {
    let actions = field(item, &["commandActions", "parsed_cmd"]).and_then(Value::as_array);
    if let Some([action]) = actions.map(Vec::as_slice) {
        match text(action, &["type"]).map(|kind| squash(&kind)).as_deref() {
            Some("read") => {
                if let Some(path) = text(action, &["path", "name"]) {
                    return ItemBody::FileRead {
                        path,
                        lines: None,
                        images: Vec::new(),
                    };
                }
            }
            Some("search") => {
                if let Some(query) = text(action, &["query"]) {
                    return ItemBody::Search {
                        query,
                        scope: text(action, &["path"]),
                        matches: None,
                    };
                }
            }
            _ => {}
        }
    }
    let command = match field(item, &["command"]) {
        Some(Value::String(line)) => unwrap_shell(line),
        Some(Value::Array(argv)) => {
            let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            join_argv(&argv)
        }
        _ => String::new(),
    };
    let cwd = text(item, &["cwd"]).map(|cwd| match cwd.strip_prefix("file://") {
        Some(path) => path.to_owned(),
        None => cwd,
    });
    ItemBody::Command {
        command,
        cwd,
        exit_code: field(item, &["exitCode", "exit_code"])
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        output: text(item, &["aggregatedOutput", "aggregated_output"]).unwrap_or_default(),
        output_truncated: false,
        background: false,
    }
}

/// Whether `program` is a shell that takes a script after `-c`.
fn is_shell(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "fish")
}

/// The script in `/bin/zsh -lc 'script'`, which is how Codex runs every
/// command, or the line as it is.
pub fn unwrap_shell(line: &str) -> String {
    let Some((program, rest)) = line.split_once(' ') else {
        return line.to_owned();
    };
    let Some(script) = rest
        .strip_prefix("-lc ")
        .or_else(|| rest.strip_prefix("-c "))
    else {
        return line.to_owned();
    };
    if !is_shell(program) {
        return line.to_owned();
    }
    if let Some(inner) = script
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("'\\''", "'");
    }
    if let Some(inner) = script
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner.replace("\\\"", "\"");
    }
    script.to_owned()
}

/// An argument list as one line, with the shell wrapper taken off.
fn join_argv(argv: &[&str]) -> String {
    if let [program, flag, script] = argv
        && is_shell(program)
        && matches!(*flag, "-c" | "-lc")
    {
        return (*script).to_owned();
    }
    argv.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains([' ', '\'', '"']) {
                format!("'{}'", arg.replace('\'', "'\\''"))
            } else {
                (*arg).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The files a change touches. The app-server lists them, and rollouts
/// key them by path.
fn file_changes(item: &Value) -> Vec<FileEdit> {
    match item.get("changes") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|change| {
                let path = text(change, &["path"])?;
                let kind = change.get("kind").unwrap_or(&Value::Null);
                let kind_name = text(kind, &["type"]).or_else(|| kind.as_str().map(str::to_owned));
                Some(file_edit(
                    path,
                    kind_name.as_deref().unwrap_or("update"),
                    text(kind, &["move_path", "movePath"]),
                    text(change, &["diff"]),
                ))
            })
            .collect(),
        Some(Value::Object(by_path)) => by_path
            .iter()
            .map(|(path, change)| {
                file_edit(
                    path.clone(),
                    text(change, &["type"]).as_deref().unwrap_or("update"),
                    text(change, &["move_path", "movePath"]),
                    text(change, &["unified_diff", "content", "diff"]),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// One file's change. Codex gives a new or deleted file's content rather
/// than a diff, so those become a diff of added or removed lines.
fn file_edit(
    path: String,
    kind: &str,
    move_path: Option<String>,
    diff: Option<String>,
) -> FileEdit {
    let (op, path, old_path) = match squash(kind).as_str() {
        "add" => (FileOp::Add, path, None),
        "delete" => (FileOp::Delete, path, None),
        _ => match move_path {
            Some(to) => (FileOp::Rename, to, Some(path)),
            None => (FileOp::Modify, path, None),
        },
    };
    let (additions, deletions, patch) = match (op, diff) {
        (FileOp::Add, Some(content)) => {
            let patch = prefix_lines(&content, '+');
            (line_count(&content), 0, Some(patch))
        }
        (FileOp::Delete, Some(content)) => {
            let patch = prefix_lines(&content, '-');
            (0, line_count(&content), Some(patch))
        }
        (_, Some(diff)) => {
            let (additions, deletions) = count_diff(&diff);
            (additions, deletions, Some(diff))
        }
        (_, None) => (0, 0, None),
    };
    FileEdit {
        path,
        old_path,
        op,
        additions,
        deletions,
        patch,
    }
}

/// How many lines `text` has.
fn line_count(text: &str) -> u32 {
    u32::try_from(text.lines().count()).unwrap_or(u32::MAX)
}

/// Every line of `text` with `mark` in front.
fn prefix_lines(text: &str, mark: char) -> String {
    text.lines().map(|line| format!("{mark}{line}\n")).collect()
}

/// The lines a unified diff adds and removes, leaving out its headers.
fn count_diff(diff: &str) -> (u32, u32) {
    let mut additions = 0u32;
    let mut deletions = 0u32;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            additions = additions.saturating_add(1);
        } else if line.starts_with('-') {
            deletions = deletions.saturating_add(1);
        }
    }
    (additions, deletions)
}

/// The picture at `path` as an item shows it, or none when there's no
/// path. Codex keeps the file, so MaxHQ fetches it from there.
fn shown(path: &str) -> Vec<AttachmentRef> {
    if path.is_empty() {
        return Vec::new();
    }
    let name = path.rsplit('/').next().unwrap_or(path).to_owned();
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let mime = match extension.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    };
    vec![AttachmentRef {
        id: AttachmentId::derive(&format!("codex-shown:{path}")),
        name,
        kind: AttachmentKind::Image,
        bytes: 0,
        mime: Some(mime.to_owned()),
        path: path.to_owned(),
        thumbnail: None,
    }]
}

/// Keeps the pictures an MCP tool gave back in `item`, a finished
/// `mcpToolCall`, where `pictures` says, and shows them on `body`. One for
/// a container goes on `kept`.
pub fn keep_pictures(
    item: &Value,
    body: &mut ItemBody,
    agent: AgentId,
    pictures: Option<&crate::drive::pictures::Pictures>,
    kept: &mut crate::drive::pictures::Kept,
) {
    let Some(pictures) = pictures else {
        return;
    };
    if text(item, &["type"]).map(|kind| squash(&kind)).as_deref() != Some("mcptoolcall") {
        return;
    }
    let content = field(item, &["result"]).and_then(|result| result.get("content"));
    let images = crate::drive::pictures::keep(content, agent, pictures, kept);
    crate::drive::pictures::show(body, images);
}

/// What an MCP tool gave back, or its error.
fn mcp_output(item: &Value) -> Option<String> {
    if let Some(error) = field(item, &["error"]) {
        return text(error, &["message"]).or_else(|| Some(compact(error)));
    }
    let result = field(item, &["result"])?;
    content_text(result.get("content")).or_else(|| {
        result
            .get("structuredContent")
            .filter(|found| !found.is_null())
            .map(compact)
    })
}

/// A call to the multi-agent tools. Spawning shows as a sub-agent, and
/// the rest as tool calls.
fn collab(item: &Value) -> (ItemBody, Option<String>) {
    let tool = text(item, &["tool"]).unwrap_or_default();
    let receivers: Vec<String> = field(item, &["receiverThreadIds", "receiver_thread_ids"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let states = field(item, &["agentsStates", "agents_states"]).and_then(Value::as_object);
    let prompt = text(item, &["prompt"]);
    if squash(&tool) == "spawnagent" {
        let child = receivers.first().cloned();
        let result = child
            .as_ref()
            .and_then(|child| states?.get(child))
            .and_then(|state| text(state, &["message"]));
        let body = ItemBody::Subagent {
            subagent: None,
            agent_type: text(item, &["model"]),
            description: prompt.unwrap_or_default(),
            result,
        };
        return (body, child);
    }
    let output = states.map(|states| {
        states
            .iter()
            .map(|(thread, state)| {
                let status = text(state, &["status"]).unwrap_or_default();
                match text(state, &["message"]) {
                    Some(message) => format!("{thread}: {status}: {message}"),
                    None => format!("{thread}: {status}"),
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    let body = ItemBody::ToolCall {
        server: None,
        name: tool,
        title: None,
        input: prompt,
        output: output.filter(|output| !output.is_empty()),
        images: Vec::new(),
        artifact: None,
    };
    (body, None)
}

/// A web search, or a page it opened.
fn web_search(item: &Value) -> ItemBody {
    let action = item.get("action").unwrap_or(&Value::Null);
    match text(action, &["type"]).map(|kind| squash(&kind)).as_deref() {
        Some("openpage" | "findinpage") => ItemBody::Web {
            action: WebAction::Fetch,
            target: text(action, &["url"]).unwrap_or_default(),
        },
        _ => ItemBody::Web {
            action: WebAction::Search,
            target: text(action, &["query"])
                .or_else(|| text(item, &["query"]))
                .unwrap_or_default(),
        },
    }
}

/// A notice about review mode.
fn notice(what: &str, item: &Value) -> ItemBody {
    let message = match text(item, &["review"]) {
        Some(review) if !review.is_empty() => format!("{what}: {review}"),
        _ => what.to_owned(),
    };
    ItemBody::Notice {
        level: NoticeLevel::Info,
        message,
    }
}

/// The text a hook added to the prompt.
fn fragments(item: &Value) -> String {
    field(item, &["fragments"])
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|fragment| fragment.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(item: Value) -> ItemBody {
        native(&item, true).unwrap().body
    }

    #[test]
    fn shell_wrappers_come_off() {
        assert_eq!(
            unwrap_shell("/bin/zsh -lc 'touch probe.txt'"),
            "touch probe.txt"
        );
        assert_eq!(unwrap_shell("bash -c 'echo '\\''hi'\\'''"), "echo 'hi'");
        assert_eq!(unwrap_shell("ls -la"), "ls -la");
        assert_eq!(join_argv(&["/bin/zsh", "-lc", "sleep 30"]), "sleep 30");
        assert_eq!(join_argv(&["rg", "two words"]), "rg 'two words'");
    }

    #[test]
    fn commands_read_from_either_shape() {
        let live = json!({
            "type": "commandExecution", "id": "exec-1",
            "command": "/bin/zsh -lc 'touch probe.txt'", "cwd": "/work/ws",
            "status": "completed", "commandActions": [{ "type": "unknown", "command": "touch probe.txt" }],
            "aggregatedOutput": "", "exitCode": 0,
        });
        let rollout = json!({
            "type": "CommandExecution", "id": "exec-1",
            "command": ["/bin/zsh", "-lc", "touch probe.txt"], "cwd": "file:///work/ws",
            "status": "completed", "parsed_cmd": [{ "type": "unknown", "cmd": "touch probe.txt" }],
            "aggregated_output": "", "exit_code": 0,
        });
        let expected = ItemBody::Command {
            command: "touch probe.txt".into(),
            cwd: Some("/work/ws".into()),
            exit_code: Some(0),
            output: String::new(),
            output_truncated: false,
            background: false,
        };
        assert_eq!(body(live), expected);
        assert_eq!(body(rollout), expected);
    }

    #[test]
    fn single_reads_and_searches_show_as_such() {
        let read = json!({
            "type": "commandExecution", "id": "exec-2", "command": "cat a.rs", "status": "completed",
            "commandActions": [{ "type": "read", "command": "cat a.rs", "name": "a.rs", "path": "/w/a.rs" }],
        });
        assert_eq!(
            body(read),
            ItemBody::FileRead {
                path: "/w/a.rs".into(),
                lines: None,
                images: Vec::new(),
            }
        );
        let search = json!({
            "type": "commandExecution", "id": "exec-3", "command": "rg main src", "status": "completed",
            "commandActions": [{ "type": "search", "command": "rg main src", "query": "main", "path": "src" }],
        });
        assert_eq!(
            body(search),
            ItemBody::Search {
                query: "main".into(),
                scope: Some("src".into()),
                matches: None
            }
        );
    }

    #[test]
    fn every_item_kind_has_a_body() {
        let cases = [
            (
                json!({ "type": "agentMessage", "id": "m", "text": "hi", "phase": "final_answer" }),
                ItemBody::AssistantMessage {
                    text: "hi".into(),
                    streaming: false,
                },
            ),
            (
                json!({ "type": "reasoning", "id": "r", "summary": ["a", "b"], "content": [] }),
                ItemBody::Reasoning {
                    text: "a\n\nb".into(),
                    streaming: false,
                },
            ),
            (
                json!({ "type": "plan", "id": "p", "text": "1. Do it" }),
                ItemBody::Plan {
                    markdown: "1. Do it".into(),
                    streaming: false,
                },
            ),
            (
                json!({ "type": "mcpToolCall", "id": "t", "server": "github", "tool": "get_issue",
                        "status": "completed", "arguments": { "n": 1 },
                        "result": { "content": [{ "type": "text", "text": "open" }] }, "error": null }),
                ItemBody::ToolCall {
                    server: Some("github".into()),
                    name: "get_issue".into(),
                    title: None,
                    input: Some("{\"n\":1}".into()),
                    output: Some("open".into()),
                    images: Vec::new(),
                    artifact: None,
                },
            ),
            (
                json!({ "type": "dynamicToolCall", "id": "d", "namespace": "app", "tool": "pick",
                        "arguments": {}, "status": "completed",
                        "contentItems": [{ "type": "inputText", "text": "done" }] }),
                ItemBody::ToolCall {
                    server: Some("app".into()),
                    name: "pick".into(),
                    title: None,
                    input: Some("{}".into()),
                    output: Some("done".into()),
                    images: Vec::new(),
                    artifact: None,
                },
            ),
            (
                json!({ "type": "webSearch", "id": "w", "query": "rust",
                        "action": { "type": "search", "query": "rust 2024" } }),
                ItemBody::Web {
                    action: WebAction::Search,
                    target: "rust 2024".into(),
                },
            ),
            (
                json!({ "type": "webSearch", "id": "w2", "query": "",
                        "action": { "type": "openPage", "url": "https://example.com" } }),
                ItemBody::Web {
                    action: WebAction::Fetch,
                    target: "https://example.com".into(),
                },
            ),
            (
                json!({ "type": "imageView", "id": "i", "path": "/w/shot.png" }),
                ItemBody::FileRead {
                    path: "/w/shot.png".into(),
                    lines: None,
                    images: shown("/w/shot.png"),
                },
            ),
            (
                json!({ "type": "contextCompaction", "id": "c" }),
                ItemBody::Compaction {
                    before_tokens: None,
                    after_tokens: None,
                },
            ),
            (
                json!({ "type": "enteredReviewMode", "id": "e", "review": "current changes" }),
                ItemBody::Notice {
                    level: NoticeLevel::Info,
                    message: "Review started: current changes".into(),
                },
            ),
            (
                json!({ "type": "hookPrompt", "id": "h", "fragments": [{ "text": "Be brief", "hookRunId": "x" }] }),
                ItemBody::Notice {
                    level: NoticeLevel::Info,
                    message: "Be brief".into(),
                },
            ),
            (
                json!({ "type": "sleep", "id": "s", "durationMs": 1500 }),
                ItemBody::ToolCall {
                    server: None,
                    name: "sleep".into(),
                    title: Some("Wait".into()),
                    input: Some("1500 ms".into()),
                    output: None,
                    images: Vec::new(),
                    artifact: None,
                },
            ),
            (
                json!({ "type": "somethingNew", "id": "n" }),
                ItemBody::Unknown {
                    native_kind: "somethingNew".into(),
                },
            ),
        ];
        for (item, expected) in cases {
            assert_eq!(body(item), expected);
        }
    }

    #[test]
    fn file_changes_count_their_lines() {
        let live = json!({
            "type": "fileChange", "id": "exec-9", "status": "declined",
            "changes": [
                { "path": "/w/note.txt", "kind": { "type": "add" }, "diff": "hello\n" },
                { "path": "/w/a.rs", "kind": { "type": "update", "move_path": "/w/b.rs" },
                  "diff": "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n+more\n" },
            ],
        });
        let native = native(&live, true).unwrap();
        assert_eq!(native.status, ItemStatus::Declined);
        let ItemBody::FileChange { changes } = native.body else {
            panic!("not a file change");
        };
        assert_eq!(changes[0].op, FileOp::Add);
        assert_eq!(changes[0].additions, 1);
        assert_eq!(changes[0].patch.as_deref(), Some("+hello\n"));
        assert_eq!(changes[1].op, FileOp::Rename);
        assert_eq!(changes[1].path, "/w/b.rs");
        assert_eq!(changes[1].old_path.as_deref(), Some("/w/a.rs"));
        assert_eq!((changes[1].additions, changes[1].deletions), (2, 1));
        let rollout = json!({
            "type": "FileChange", "id": "exec-9", "status": "declined",
            "changes": { "/w/note.txt": { "type": "add", "content": "hello\n" } },
        });
        let ItemBody::FileChange { changes } = body(rollout) else {
            panic!("not a file change");
        };
        assert_eq!(changes[0].path, "/w/note.txt");
        assert_eq!(changes[0].additions, 1);
    }

    #[test]
    fn spawned_agents_are_named_like_the_observer_names_them() {
        let parent = names::agent(maxhq_protocol::model::Vendor::Codex, "parent-thread");
        let spawn = json!({
            "type": "collabAgentToolCall", "id": "call_1", "tool": "spawnAgent", "status": "completed",
            "senderThreadId": "parent-thread", "receiverThreadIds": ["child-thread"],
            "prompt": "Look around", "model": null,
            "agentsStates": { "child-thread": { "status": "running", "message": null } },
        });
        let item = timeline(parent, Some("t"), native(&spawn, true).unwrap(), None, None);
        assert_eq!(
            item.body,
            ItemBody::Subagent {
                subagent: Some(names::subagent(parent, "child-thread")),
                agent_type: None,
                description: "Look around".into(),
                result: None,
            }
        );
        let wait = json!({
            "type": "collabAgentToolCall", "id": "call_2", "tool": "wait", "status": "completed",
            "receiverThreadIds": ["child-thread"],
            "agentsStates": { "child-thread": { "status": "completed", "message": "found it" } },
        });
        assert_eq!(
            body(wait),
            ItemBody::ToolCall {
                server: None,
                name: "wait".into(),
                title: None,
                input: None,
                output: Some("child-thread: completed: found it".into()),
                images: Vec::new(),
                artifact: None,
            }
        );
    }

    #[test]
    fn user_messages_keep_their_images() {
        let message = json!({
            "type": "UserMessage", "id": "u",
            "content": [
                { "type": "text", "text": "what color?", "text_elements": [] },
                { "type": "local_image", "path": "/w/red.png" },
            ],
        });
        let ItemBody::UserMessage {
            text, attachments, ..
        } = body(message)
        else {
            panic!("not a user message");
        };
        assert_eq!(text, "what color?");
        assert_eq!(attachments[0].name, "red.png");
        assert_eq!(attachments[0].mime.as_deref(), Some("image/png"));
    }

    #[test]
    fn user_messages_read_back_what_their_lines_name() {
        let message = json!({
            "type": "userMessage", "id": "u",
            "content": [
                {
                    "type": "text",
                    "text": "look\n\n[Attached image \"red.png\" is saved at: /w/1/red.png]\n\n\
                             [Attached file \"a.docx\" is saved at: /w/2/a.docx \
                             (text extracted to: /w/2/a.docx.txt)]",
                    "text_elements": [],
                },
                { "type": "localImage", "path": "/w/1/red.png" },
                { "type": "localImage", "path": "/old/blue.png" },
            ],
        });
        let ItemBody::UserMessage {
            text, attachments, ..
        } = body(message)
        else {
            panic!("not a user message");
        };
        assert_eq!(text, "look");
        let found: Vec<_> = attachments
            .iter()
            .map(|a| (a.kind, a.name.as_str(), a.path.as_str(), a.mime.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    AttachmentKind::Image,
                    "red.png",
                    "/w/1/red.png",
                    Some("image/png")
                ),
                (AttachmentKind::File, "a.docx", "/w/2/a.docx", None),
                (
                    AttachmentKind::Image,
                    "blue.png",
                    "/old/blue.png",
                    Some("image/png")
                ),
            ]
        );
    }

    #[test]
    fn pills_come_back_from_what_the_input_sent() {
        use crate::drive::Staged;
        use maxhq_protocol::context::{GitItemKind, GitItemState, QuoteSource};

        let staged = |path: &str, kind: AttachmentKind| Staged {
            attachment: AttachmentRef {
                id: AttachmentId::derive(path),
                name: path.rsplit('/').next().unwrap().to_owned(),
                kind,
                bytes: 1,
                mime: None,
                path: path.to_owned(),
                thumbnail: None,
            },
            path: path.into(),
            text: None,
        };
        let shot = staged("/path/to/1/shot.png", AttachmentKind::Image);
        let spec = staged("/path/to/2/spec.pdf", AttachmentKind::Pdf);
        let extra = staged("/path/to/3/extra.png", AttachmentKind::Image);
        let notes = staged("/path/to/4/notes.txt", AttachmentKind::File);
        let more = staged("/path/to/5/notes.txt", AttachmentKind::File);
        let other: AgentId = "0199a7c2-0000-7000-8000-000000000002".parse().unwrap();
        let pr = InlineRef::GitItem {
            repo: "owner/repo".into(),
            number: 123,
            kind: GitItemKind::PullRequest,
            title: "Fix cache expiry".into(),
            state: GitItemState::Open,
            url: "https://github.com/owner/repo/pull/123".into(),
            head: Some("fix/cache".into()),
            base: Some("main".into()),
            body: "Closes #45".into(),
            checked_at: Timestamp(1_761_834_131_250),
        };
        let docs = InlineRef::Agent {
            agent: other,
            title: "docs".into(),
            vendor: maxhq_protocol::model::Vendor::Claude,
            device: context::HERE,
            excerpt: None,
        };
        let to = names::agent(maxhq_protocol::model::Vendor::Codex, "t");
        let quote = InlineRef::Quote {
            agent: to,
            agent_title: "t".into(),
            item: ItemId("codex:item_01".into()),
            text: "the cache never expires".into(),
            position: 8,
            source: QuoteSource::Reply,
            from: None,
        };
        let refs = vec![
            InlineRef::Attachment {
                id: shot.attachment.id,
            },
            InlineRef::Attachment {
                id: spec.attachment.id,
            },
            docs.clone(),
            quote.clone(),
            InlineRef::Attachment {
                id: notes.attachment.id,
            },
            InlineRef::Attachment {
                id: more.attachment.id,
            },
            pr.clone(),
        ];
        let typed = "Why does \u{FFFC} fail? See #123, \u{FFFC} and \u{FFFC}, \
                     \u{FFFC} in \u{FFFC} or \u{FFFC}";
        let content =
            super::super::input::input(typed, &refs, &[shot, spec, extra, notes, more], to, true);
        let labels = content
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part["text"] == "Image 1:")
            .count();
        assert_eq!(labels, 1, "{content:#}");

        let message = json!({ "type": "userMessage", "id": "u", "content": content });
        let ItemBody::UserMessage {
            text,
            attachments,
            refs: back,
            ..
        } = body(message)
        else {
            panic!("not a user message");
        };
        assert_eq!(text, typed);
        let id = |path: &str| AttachmentId::derive(&format!("codex:{path}"));
        assert_eq!(
            back,
            [
                InlineRef::Attachment {
                    id: id("/path/to/1/shot.png")
                },
                InlineRef::Attachment {
                    id: id("/path/to/2/spec.pdf")
                },
                docs,
                quote,
                InlineRef::Attachment {
                    id: id("/path/to/4/notes.txt")
                },
                InlineRef::Attachment {
                    id: id("/path/to/5/notes.txt")
                },
                pr,
            ]
        );
        let mut found: Vec<_> = attachments
            .iter()
            .map(|one| (one.id, one.name.as_str()))
            .collect();
        found.sort_by_key(|(_, name)| *name);
        assert_eq!(
            found,
            [
                (id("/path/to/3/extra.png"), "extra.png"),
                (id("/path/to/4/notes.txt"), "notes.txt"),
                (id("/path/to/5/notes.txt"), "notes.txt"),
                (id("/path/to/1/shot.png"), "shot.png"),
                (id("/path/to/2/spec.pdf"), "spec.pdf"),
            ]
        );
    }

    #[test]
    fn an_output_pill_comes_back_from_what_the_input_sent() {
        let to = names::agent(maxhq_protocol::model::Vendor::Codex, "t");
        let refs = vec![crate::testing::output_pill()];
        let typed = "Why did \u{FFFC} fail?";
        let content = super::super::input::input(typed, &refs, &[], to, true);
        let message = json!({ "type": "userMessage", "id": "u", "content": content });
        let ItemBody::UserMessage {
            text,
            attachments,
            refs: back,
            ..
        } = body(message)
        else {
            panic!("not a user message");
        };
        assert_eq!(text, typed);
        assert!(attachments.is_empty());
        assert_eq!(back, refs);
    }

    #[test]
    fn a_label_someone_typed_stays() {
        let message = json!({
            "type": "userMessage", "id": "u",
            "content": [
                { "type": "text", "text": "Image 1:", "text_elements": [] },
                { "type": "text", "text": "what's this?", "text_elements": [] },
            ],
        });
        let ItemBody::UserMessage { text, refs, .. } = body(message) else {
            panic!("not a user message");
        };
        assert_eq!(text, "Image 1:\nwhat's this?");
        assert!(refs.is_empty());
    }

    #[test]
    fn a_message_sent_from_here_keeps_its_local_id() {
        let agent = names::agent(maxhq_protocol::model::Vendor::Codex, "t");
        let id = |item: Value| timeline(agent, None, native(&item, true).unwrap(), None, None).id;
        let text = json!([{ "type": "text", "text": "hi" }]);

        let sent = json!({ "type": "userMessage", "id": "u", "clientId": "c", "content": text });
        let typed = json!({ "type": "userMessage", "id": "u", "clientId": null, "content": text });
        let reply = json!({ "type": "agentMessage", "id": "m", "clientId": "c", "text": "ok" });

        assert_eq!(id(sent), local_id("c"));
        assert_eq!(id(typed), item_id("u"));
        assert_eq!(id(reply), item_id("m"));
    }

    #[test]
    fn limit_errors_are_recognised() {
        assert!(limit_hit(
            &json!({ "codexErrorInfo": "usageLimitExceeded" })
        ));
        assert!(limit_hit(
            &json!({ "codex_error_info": "usage_limit_exceeded" })
        ));
        assert!(!limit_hit(
            &json!({ "codexErrorInfo": "contextWindowExceeded" })
        ));
        assert!(!limit_hit(&json!({ "message": "boom" })));
        assert!(!limit_hit(&json!({ "codexErrorInfo": "unauthorized" })));
    }

    #[test]
    fn a_login_that_ran_out_is_recognised_in_any_casing() {
        assert!(is_unauthorized(
            &json!({ "codexErrorInfo": "unauthorized" })
        ));
        assert!(is_unauthorized(
            &json!({ "codex_error_info": "Unauthorized" })
        ));
        assert!(is_unauthorized(
            &json!({ "codexErrorInfo": { "unauthorized": {} } })
        ));
        assert!(!is_unauthorized(
            &json!({ "codexErrorInfo": "usageLimitExceeded" })
        ));
        assert!(!is_unauthorized(&json!({ "message": "unauthorized" })));
    }

    #[test]
    fn only_an_unauthorized_failure_names_the_account_to_sign_in() {
        let account = AccountId::derive("codex");
        let agent = AgentId::derive("lead");
        let error = json!({ "message": "401 Unauthorized", "codexErrorInfo": "unauthorized" });
        let item = failure(agent, Some(account), "t1", &error, None, false, None);
        assert_eq!(
            item.body,
            ItemBody::Error {
                message: "401 Unauthorized".into(),
                retrying: false,
                sign_in: Some(account),
            }
        );
        let other = json!({ "message": "boom", "codexErrorInfo": "internalServerError" });
        let item = failure(agent, Some(account), "t1", &other, None, false, None);
        assert!(matches!(item.body, ItemBody::Error { sign_in: None, .. }));
        // A read that doesn't know the account names none.
        let item = failure(agent, None, "t1", &error, None, false, None);
        assert!(matches!(item.body, ItemBody::Error { sign_in: None, .. }));
    }

    #[test]
    fn titles_take_the_first_line() {
        assert_eq!(
            title("\n  Fix the build \nnow").as_deref(),
            Some("Fix the build")
        );
        let long = "x".repeat(80);
        assert_eq!(title(&long).unwrap().chars().count(), TITLE_CHARS + 1);
        assert_eq!(title("  "), None);
    }
}
