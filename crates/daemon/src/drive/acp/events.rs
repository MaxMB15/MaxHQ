// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What `session/update` says, turned into timeline items. Message and
//! thought chunks grow one item each until something else comes between
//! them. Each tool call is an item rebuilt from everything its updates
//! have said so far, and its `kind` picks how it shows. A `plan` update is
//! the agent's todo list.
//!
//! Item IDs come from the turn's number and the item's place in it, so the
//! replay `session/load` sends gives each item the ID it had live. Tool
//! calls get new IDs in a replay (`replay-<turn>-<n>`), so a call's
//! `toolCallId` never names its item. It can hold a newline, too.
//!
//! The 2026.10.22 recording shows a call start as a `tool_call` with a
//! generic title ("Read File", "Terminal") and empty `rawInput`, then an
//! update with the real title, input and locations, then `in_progress`,
//! then `completed` with `rawOutput`. A call whose `rawOutput` holds an
//! `error` still says `completed`, so the error decides.

use std::collections::HashMap;

use maxhq_core::tools::{self, SERVER};
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    FileEdit, FileOp, ItemBody, ItemId, ItemStatus, TextField, TimelineEvent, TimelineItem, Todo,
    TodoState, TurnId, WebAction,
};
use serde_json::{Map, Value};

use super::rpc::text;

/// Timeline events on their way out, to the live timeline or to history.
pub type Out = Vec<(AgentId, TimelineEvent)>;

/// The items one agent is building: the lead, or a sub-agent's child
/// session.
#[derive(Debug)]
pub struct Stream {
    /// The agent the items are for.
    pub agent: AgentId,
    /// What every item ID starts with.
    base: String,
    /// The turn's number, counted from the session's first prompt.
    pub turn: u64,
    /// The next item's place in the turn.
    next: u64,
    /// The message or thought still growing.
    text: Option<TimelineItem>,
    /// Tool calls that haven't ended, by `toolCallId`.
    pub tools: HashMap<String, Tool>,
    /// The turn's last assistant message, and its item.
    pub reply: Option<TimelineItem>,
    /// The turn's todo list, by each todo's ID.
    todos: Vec<(String, Todo)>,
    /// Set while replayed lines are read, so text goes out as whole items
    /// and reading it twice changes nothing.
    pub quiet: bool,
}

/// A tool call that hasn't ended.
#[derive(Clone, Debug)]
pub struct Tool {
    /// Its item as last sent.
    pub item: TimelineItem,
    /// Every field its updates have set.
    raw: Map<String, Value>,
}

impl Stream {
    /// A stream for `agent` whose item IDs start with `base`.
    pub fn new(agent: AgentId, base: &str) -> Self {
        Self {
            agent,
            base: base.to_owned(),
            turn: 0,
            next: 0,
            text: None,
            tools: HashMap::new(),
            reply: None,
            todos: Vec::new(),
            quiet: false,
        }
    }

    /// Starts turn number `turn`.
    pub fn begin(&mut self, turn: u64) {
        self.turn = turn;
        self.next = 0;
        self.text = None;
        self.reply = None;
        self.todos.clear();
    }

    /// The ID for something the turn has one of, such as its user message.
    pub fn id(&self, what: &str) -> ItemId {
        ItemId(format!("{}:t{}:{what}", self.base, self.turn))
    }

    /// The ID for the turn's next item, `m` for a message or thought and
    /// `c` for a tool call.
    fn next_id(&mut self, kind: char) -> ItemId {
        let id = ItemId(format!("{}:t{}:{kind}{}", self.base, self.turn, self.next));
        self.next += 1;
        id
    }

    /// The turn's ID on its items.
    pub fn turn_id(&self) -> Option<TurnId> {
        (self.turn > 0).then(|| TurnId(format!("t{}", self.turn)))
    }

    /// A new item in this turn.
    pub fn item(
        &self,
        id: ItemId,
        status: ItemStatus,
        at: Timestamp,
        body: ItemBody,
    ) -> TimelineItem {
        let done = !matches!(
            status,
            ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
        );
        TimelineItem {
            id,
            agent: self.agent,
            turn: self.turn_id(),
            parent: None,
            ordinal: 0,
            status,
            started_at: Some(at),
            completed_at: done.then_some(at),
            body,
        }
    }

    /// Adds a message chunk, or a thought chunk when `thought` is set.
    pub fn chunk(&mut self, thought: bool, chunk: &str, at: Timestamp, out: &mut Out) {
        if chunk.is_empty() {
            return;
        }
        let same = self.text.as_ref().is_some_and(|open| {
            matches!(
                (&open.body, thought),
                (ItemBody::Reasoning { .. }, true) | (ItemBody::AssistantMessage { .. }, false)
            )
        });
        if same {
            if let Some(open) = &mut self.text {
                match &mut open.body {
                    ItemBody::Reasoning { text, .. } | ItemBody::AssistantMessage { text, .. } => {
                        text.push_str(chunk)
                    }
                    _ => {}
                }
                if self.quiet {
                    out.push((self.agent, TimelineEvent::Upsert { item: open.clone() }));
                } else {
                    out.push((
                        self.agent,
                        TimelineEvent::Append {
                            id: open.id.clone(),
                            field: TextField::Text,
                            text: chunk.to_owned(),
                        },
                    ));
                }
                if !thought {
                    self.reply = Some(open.clone());
                }
            }
            return;
        }
        self.close_text(at, out);
        let id = self.next_id('m');
        let body = if thought {
            ItemBody::Reasoning {
                text: chunk.to_owned(),
                streaming: true,
            }
        } else {
            ItemBody::AssistantMessage {
                text: chunk.to_owned(),
                streaming: true,
            }
        };
        let item = self.item(id, ItemStatus::Running, at, body);
        if !thought {
            self.reply = Some(item.clone());
        }
        upsert(out, item.clone());
        self.text = Some(item);
    }

    /// Ends the message or thought still growing.
    pub fn close_text(&mut self, at: Timestamp, out: &mut Out) {
        let Some(mut item) = self.text.take() else {
            return;
        };
        if let ItemBody::Reasoning { streaming, .. }
        | ItemBody::AssistantMessage { streaming, .. } = &mut item.body
        {
            *streaming = false;
        }
        item.status = ItemStatus::Completed;
        item.completed_at = Some(at);
        if matches!(item.body, ItemBody::AssistantMessage { .. }) {
            self.reply = Some(item.clone());
        }
        upsert(out, item);
    }

    /// A `tool_call` or `tool_call_update`. Gives the call's ID and
    /// whether it has ended.
    pub fn tool(&mut self, update: &Value, at: Timestamp, out: &mut Out) -> Option<(String, bool)> {
        let call = text(update, "toolCallId")?.to_owned();
        self.close_text(at, out);
        let mut tool = match self.tools.remove(&call) {
            Some(tool) => tool,
            None => {
                let id = self.next_id('c');
                Tool {
                    item: self.item(id, ItemStatus::Pending, at, ItemBody::Interrupted),
                    raw: Map::new(),
                }
            }
        };
        if let Some(fields) = update.as_object() {
            for (name, value) in fields {
                if name != "sessionUpdate" && !value.is_null() {
                    tool.raw.insert(name.clone(), value.clone());
                }
            }
        }
        let failed = tool
            .raw
            .get("rawOutput")
            .and_then(|output| text(output, "error"))
            .is_some_and(|error| !error.is_empty());
        let status = match tool.raw.get("status").and_then(Value::as_str) {
            Some("in_progress") => ItemStatus::Running,
            Some("completed") if failed => ItemStatus::Failed,
            Some("completed") => ItemStatus::Completed,
            Some("failed") => ItemStatus::Failed,
            _ => tool.item.status,
        };
        // A sub-agent's call keeps the body its task gave it, and takes
        // what the call said back as the sub-agent's result.
        match &mut tool.item.body {
            ItemBody::Subagent { result, .. } => {
                if let Some(said) = content_text(tool.raw.get("content")) {
                    *result = Some(said);
                }
            }
            body => *body = body_of(&tool.raw),
        }
        tool.item.status = status;
        let ended = matches!(status, ItemStatus::Completed | ItemStatus::Failed);
        if ended {
            tool.item.completed_at = Some(at);
        }
        upsert(out, tool.item.clone());
        if !ended {
            self.tools.insert(call.clone(), tool);
        }
        Some((call, ended))
    }

    /// Everything a tool call's updates have said, if it's still open.
    pub fn tool_raw(&self, call: &str) -> Option<&Map<String, Value>> {
        self.tools.get(call).map(|tool| &tool.raw)
    }

    /// Ends a tool call with `status`, if it's still open, and gives its
    /// item's ID.
    pub fn end_tool(
        &mut self,
        call: &str,
        status: ItemStatus,
        at: Timestamp,
        out: &mut Out,
    ) -> Option<ItemId> {
        let mut tool = self.tools.remove(call)?;
        tool.item.status = status;
        tool.item.completed_at = Some(at);
        let id = tool.item.id.clone();
        upsert(out, tool.item);
        Some(id)
    }

    /// Replaces the turn's todo list with `todos`, or merges them in by ID
    /// when `merge` is set.
    pub fn todos(&mut self, todos: Vec<(String, Todo)>, merge: bool, at: Timestamp, out: &mut Out) {
        if merge {
            for (id, todo) in todos {
                match self.todos.iter_mut().find(|(known, _)| *known == id) {
                    Some((_, known)) => *known = todo,
                    None => self.todos.push((id, todo)),
                }
            }
        } else {
            self.todos = todos;
        }
        self.close_text(at, out);
        let body = ItemBody::Todos {
            todos: self.todos.iter().map(|(_, todo)| todo.clone()).collect(),
        };
        let item = self.item(self.id("todos"), ItemStatus::Completed, at, body);
        upsert(out, item);
    }

    /// Ends everything still open as the turn ends. Tool calls end with
    /// `status`.
    pub fn finish(&mut self, status: ItemStatus, at: Timestamp, out: &mut Out) {
        self.close_text(at, out);
        let calls: Vec<String> = self.tools.keys().cloned().collect();
        for call in calls {
            self.end_tool(&call, status, at, out);
        }
    }
}

/// The text blocks in a tool call's `content`, joined.
pub fn content_text(content: Option<&Value>) -> Option<String> {
    let said: Vec<&str> = content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let block = entry.get("content")?;
            (text(block, "type") == Some("text")).then(|| text(block, "text"))?
        })
        .collect();
    (!said.is_empty()).then(|| said.join("\n"))
}

/// Sends an item whole.
pub fn upsert(out: &mut Out, item: TimelineItem) {
    out.push((item.agent, TimelineEvent::Upsert { item }));
}

/// The todos in an ACP `plan` update's `entries`.
pub fn plan_entries(update: &Value) -> Vec<(String, Todo)> {
    list(update, "entries")
        .enumerate()
        .filter_map(|(index, entry)| {
            let todo = todo(entry, "content")?;
            Some((index.to_string(), todo))
        })
        .collect()
}

/// One todo, with its text in `field`.
pub fn todo(entry: &Value, field: &str) -> Option<Todo> {
    let state = match text(entry, "status") {
        Some("in_progress") => TodoState::InProgress,
        Some("completed" | "cancelled") => TodoState::Done,
        _ => TodoState::Pending,
    };
    Some(Todo {
        text: text(entry, field)?.to_owned(),
        state,
    })
}

/// The items of an array field, or none.
pub fn list<'a>(value: &'a Value, name: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(name)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The first string among `names` in `value`.
fn first<'a>(value: Option<&'a Value>, names: &[&str]) -> Option<&'a str> {
    let value = value?;
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .filter(|found| !found.is_empty())
}

/// What a tool call shows, from everything its updates said.
pub fn body_of(raw: &Map<String, Value>) -> ItemBody {
    let kind = raw.get("kind").and_then(Value::as_str).unwrap_or("other");
    let title = raw
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let input = raw.get("rawInput");
    let output = raw.get("rawOutput");
    let content: Vec<&Value> = raw
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let said = content_text(raw.get("content"));
    let location = raw
        .get("locations")
        .and_then(Value::as_array)
        .and_then(|locations| locations.first())
        .and_then(|location| text(location, "path"));
    match kind {
        "execute" => {
            let command = input
                .and_then(|input| match input.get("command") {
                    Some(Value::String(line)) => Some(line.clone()),
                    Some(Value::Array(words)) => Some(
                        words
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    _ => None,
                })
                .unwrap_or_else(|| title.clone());
            let out = printed(output).or(said).unwrap_or_default();
            let exit_code = output
                .and_then(|output| output.get("exitCode").or_else(|| output.get("exit_code")))
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            let background = input.is_some_and(|input| {
                ["isBackground", "is_background", "runInBackground"]
                    .iter()
                    .any(|name| input.get(*name).and_then(Value::as_bool) == Some(true))
            });
            ItemBody::Command {
                command,
                cwd: first(input, &["cwd", "workingDirectory", "working_directory"])
                    .map(str::to_owned),
                exit_code,
                output: out,
                output_truncated: false,
                background,
            }
        }
        "edit" | "delete" | "move" => {
            let mut changes: Vec<FileEdit> = content
                .iter()
                .filter(|part| text(part, "type") == Some("diff"))
                .filter_map(|diff| {
                    let path = text(diff, "path")?.to_owned();
                    let old = text(diff, "oldText");
                    let new = text(diff, "newText").unwrap_or_default();
                    let op = match (kind, old) {
                        ("delete", _) => FileOp::Delete,
                        (_, None) => FileOp::Add,
                        _ => FileOp::Modify,
                    };
                    let (additions, deletions) = counts(old.unwrap_or_default(), new);
                    Some(FileEdit {
                        path,
                        old_path: None,
                        op,
                        additions,
                        deletions,
                        patch: None,
                    })
                })
                .collect();
            if changes.is_empty()
                && let Some(path) = location.or_else(|| first(input, &["path", "file_path"]))
            {
                changes.push(FileEdit {
                    path: path.to_owned(),
                    old_path: None,
                    op: match kind {
                        "delete" => FileOp::Delete,
                        "move" => FileOp::Rename,
                        _ => FileOp::Modify,
                    },
                    additions: 0,
                    deletions: 0,
                    patch: None,
                });
            }
            if kind == "move"
                && let Some(change) = changes.first_mut()
            {
                change.op = FileOp::Rename;
                change.old_path = first(input, &["source", "from", "oldPath"]).map(str::to_owned);
            }
            ItemBody::FileChange { changes }
        }
        "read" => ItemBody::FileRead {
            path: location
                .or_else(|| first(input, &["path", "file_path", "target_file"]))
                .map_or_else(|| title.clone(), str::to_owned),
            lines: None,
            images: Vec::new(),
        },
        "search" => ItemBody::Search {
            query: first(input, &["query", "pattern", "regex", "glob"])
                .map_or_else(|| title.clone(), str::to_owned),
            scope: first(input, &["path", "directory"])
                .or(location)
                .map(str::to_owned),
            matches: None,
        },
        "fetch" => match first(input, &["url"]) {
            Some(url) => ItemBody::Web {
                action: WebAction::Fetch,
                target: url.to_owned(),
            },
            None => ItemBody::Web {
                action: WebAction::Search,
                target: first(input, &["query", "search_term"])
                    .map_or_else(|| title.clone(), str::to_owned),
            },
        },
        _ => {
            let mcp = mcp_tool(raw);
            let name = match &mcp {
                Some(call) => call.tool.to_owned(),
                None if title.is_empty() => kind.to_owned(),
                None => title.clone(),
            };
            let input = match &mcp {
                Some(call) => call.args,
                None => input,
            };
            ItemBody::ToolCall {
                server: mcp.as_ref().map(|call| call.server.to_owned()),
                name,
                title: (!title.is_empty()).then(|| title.clone()),
                input: input.filter(|input| !input.is_null()).map(Value::to_string),
                output: said.or_else(|| {
                    output
                        .filter(|output| !output.is_null())
                        .map(Value::to_string)
                }),
                images: Vec::new(),
            }
        }
    }
}

/// An MCP call a tool call names.
#[derive(Debug, PartialEq)]
pub struct McpCall<'a> {
    /// The MCP server.
    pub server: &'a str,
    /// The tool on it.
    pub tool: &'a str,
    /// What the tool was given.
    pub args: Option<&'a Value>,
}

/// The MCP call a tool call names, if it's one. Cursor names the server
/// and tool in `rawInput`. Another agent may only name them in the title,
/// so a title that names one of MaxHQ's own tools counts too, with
/// `rawInput` as its arguments.
pub fn mcp_tool(raw: &Map<String, Value>) -> Option<McpCall<'_>> {
    let input = raw.get("rawInput");
    let named = input.and_then(|input| {
        Some(McpCall {
            server: text(input, "providerIdentifier")?,
            tool: text(input, "toolName")?,
            args: input.get("args"),
        })
    });
    named.or_else(|| {
        let tool = titled(raw.get("title")?.as_str()?)?;
        tools::Tool::named(tool)?;
        Some(McpCall {
            server: SERVER,
            tool,
            args: input,
        })
    })
}

/// The tool a title names on [`SERVER`]: `mcp__maxhq__tool`, `maxhq` and
/// the tool with a separator between, maybe followed by more, or
/// `tool (maxhq MCP Server)`.
fn titled(title: &str) -> Option<&str> {
    let title = title.trim();
    if let Some(tool) = title
        .strip_suffix(" MCP Server)")
        .and_then(|rest| rest.strip_suffix(SERVER))
        .and_then(|rest| rest.strip_suffix(" ("))
    {
        return Some(tool.trim());
    }
    let rest = title.strip_prefix("mcp_").unwrap_or(title);
    let rest = rest
        .strip_prefix('_')
        .unwrap_or(rest)
        .strip_prefix(SERVER)?;
    let is_separator = |c: char| "-_.:/ ".contains(c);
    if !rest.starts_with(is_separator) {
        return None;
    }
    let rest = rest.trim_start_matches(is_separator);
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|tool| !tool.is_empty())
}

/// Whether a call is to MaxHQ's own agent tools: the server must be
/// [`SERVER`] exactly, and the tool one of them. Only a call of kind `other`,
/// or with no kind, counts. Any other kind runs, changes or fetches something
/// whatever it's called, because its title can hold words the agent picked.
pub fn ours(raw: &Map<String, Value>) -> bool {
    raw.get("kind").is_none_or(|kind| kind == "other")
        && mcp_tool(raw)
            .is_some_and(|call| call.server == SERVER && tools::Tool::named(call.tool).is_some())
}

/// What a command printed, from its `rawOutput`: Cursor's has `stdout`
/// and `stderr`, which go out in that order.
fn printed(output: Option<&Value>) -> Option<String> {
    let mut printed = String::new();
    for name in ["output", "stdout", "stderr"] {
        let Some(part) = first(output, &[name]) else {
            continue;
        };
        if !printed.is_empty() && !printed.ends_with('\n') {
            printed.push('\n');
        }
        printed.push_str(part);
    }
    (!printed.is_empty()).then_some(printed)
}

/// Lines added and taken out between `old` and `new`, counted as lines in
/// one that the other doesn't have.
fn counts(old: &str, new: &str) -> (u32, u32) {
    let mut left: HashMap<&str, i64> = HashMap::new();
    for line in old.lines() {
        *left.entry(line).or_default() += 1;
    }
    let mut added = 0u32;
    for line in new.lines() {
        match left.get_mut(line) {
            Some(count) if *count > 0 => *count -= 1,
            _ => added += 1,
        }
    }
    let removed: i64 = left.values().sum();
    (added, u32::try_from(removed).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn execute_calls_are_commands() {
        let body = body_of(&raw(json!({
            "kind": "execute",
            "title": "Run tests",
            "rawInput": { "command": "cargo test", "cwd": "/work/repo" },
            "rawOutput": { "exitCode": 1 },
            "content": [{ "type": "content", "content": { "type": "text", "text": "1 failed" } }],
        })));
        assert_eq!(
            body,
            ItemBody::Command {
                command: "cargo test".into(),
                cwd: Some("/work/repo".into()),
                exit_code: Some(1),
                output: "1 failed".into(),
                output_truncated: false,
                background: false,
            }
        );
    }

    #[test]
    fn diffs_are_file_changes_with_counts() {
        let body = body_of(&raw(json!({
            "kind": "edit",
            "title": "Edit main.rs",
            "content": [
                { "type": "diff", "path": "/r/src/main.rs", "oldText": "a\nb\n", "newText": "a\nc\nd\n" },
                { "type": "diff", "path": "/r/new.rs", "newText": "x\n" },
            ],
        })));
        let ItemBody::FileChange { changes } = body else {
            panic!("not a file change");
        };
        assert_eq!(changes[0].op, FileOp::Modify);
        assert_eq!((changes[0].additions, changes[0].deletions), (2, 1));
        assert_eq!(changes[1].op, FileOp::Add);
    }

    #[test]
    fn reads_searches_and_fetches_have_their_own_shapes() {
        let read = body_of(&raw(json!({
            "kind": "read", "title": "Read lib.rs", "locations": [{ "path": "/r/lib.rs" }],
        })));
        assert!(matches!(read, ItemBody::FileRead { ref path, .. } if path == "/r/lib.rs"));
        let search = body_of(&raw(json!({
            "kind": "search", "title": "Grep", "rawInput": { "pattern": "fn main", "path": "src" },
        })));
        assert!(
            matches!(search, ItemBody::Search { ref query, ref scope, .. }
            if query == "fn main" && scope.as_deref() == Some("src"))
        );
        let fetch = body_of(&raw(json!({
            "kind": "fetch", "title": "Fetch", "rawInput": { "url": "https://example.com" },
        })));
        assert!(matches!(
            fetch,
            ItemBody::Web {
                action: WebAction::Fetch,
                ..
            }
        ));
        let other = body_of(&raw(json!({ "kind": "other", "title": "mcp: lookup" })));
        assert!(matches!(other, ItemBody::ToolCall { ref name, .. } if name == "mcp: lookup"));
    }

    #[test]
    fn a_title_names_maxhqs_own_tools_when_raw_input_doesnt() {
        for title in [
            "mcp__maxhq__list_agents",
            "maxhq_list_agents",
            "mcp_maxhq-list_agents",
            "maxhq: list_agents: every agent",
            "maxhq/list_agents",
            "list_agents (maxhq MCP Server)",
        ] {
            let call = raw(json!({ "title": title, "rawInput": { "scope": "here" } }));
            assert!(ours(&call), "{title}");
            let found = mcp_tool(&call).unwrap();
            assert_eq!((found.server, found.tool), (SERVER, "list_agents"));
            assert_eq!(found.args, Some(&json!({ "scope": "here" })));
        }
        for title in [
            "maxhqlist_agents",
            "other__list_agents",
            "maxhq: no_such_tool",
            "list_agents (other MCP Server)",
            "List agents",
        ] {
            let call = raw(json!({ "title": title }));
            assert!(!ours(&call), "{title}");
            assert_eq!(mcp_tool(&call), None, "{title}");
        }
        let named = raw(json!({
            "title": "maxhq_list_agents",
            "rawInput": { "providerIdentifier": "other", "toolName": "list_agents", "args": {} },
        }));
        assert!(!ours(&named));
    }

    #[test]
    fn a_call_that_runs_or_changes_something_is_never_ours() {
        for kind in [
            "execute",
            "edit",
            "delete",
            "move",
            "read",
            "search",
            "fetch",
            "think",
            "switch_mode",
            "a_kind_from_later",
        ] {
            let titled = raw(json!({ "title": "mcp__maxhq__list_agents", "kind": kind }));
            assert!(!ours(&titled), "{kind}");
            let named = raw(json!({
                "title": "anything",
                "kind": kind,
                "rawInput": { "providerIdentifier": SERVER, "toolName": "list_agents" },
            }));
            assert!(!ours(&named), "{kind}");
        }
        let other = raw(json!({ "title": "mcp__maxhq__list_agents", "kind": "other" }));
        assert!(ours(&other));
        let unsaid = raw(json!({ "title": "mcp__maxhq__list_agents" }));
        assert!(ours(&unsaid));
    }
}
