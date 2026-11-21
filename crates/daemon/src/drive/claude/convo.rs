// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Turns Claude Code's messages into timeline items. The live stream and a
//! transcript carry the same messages in slightly different shapes, so both
//! come through here and get the same item IDs.
//!
//! The IDs are `claude:` and then the message ID and block index for text
//! and thinking, the `tool_use` ID for a tool call, that ID and `:approval`
//! for the prompt that asked to run it, that ID and `:sent` for the files a
//! call to MaxHQ's send tool sent, and the line's UUID for a prompt and
//! most else. The transcript keeps all of them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use maxhq_core::tools as ours;
use maxhq_protocol::attention::AttentionAnswer;
use maxhq_protocol::ids::{AccountId, AgentId, AttentionId};
use maxhq_protocol::model::DEFAULT_MEMORY_ITEMS;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ApprovalKind, AttachmentRef, Decided, Delivery, ItemBody, ItemId, ItemStatus, TextField,
    TimelineItem, TurnId,
};
use serde_json::Value;

use super::artifacts::Asked;
use super::attach;
use super::items::Book;
use super::tools::{self, Finish};
use super::wire::{content_text, flag, number, text, text_or_empty};
use crate::drive::pictures::{self, Kept, Pictures};
use crate::observe::claude::time::parse_iso8601;

/// Whether a failed API call's `assistant` line says the login ran out. The
/// stream spells the status in snake case, a transcript in camel case.
fn signed_out(line: &Value) -> bool {
    text(line, "error") == Some("authentication_failed")
        || number(line, "api_error_status") == Some(401)
        || number(line, "apiErrorStatus") == Some(401)
}

/// The item ID for the line, prompt or request with this ID.
pub fn item_id(id: &str) -> ItemId {
    ItemId(format!("claude:{id}"))
}

/// The item ID for one block of one API message.
pub fn block_id(message: &str, index: u64) -> ItemId {
    ItemId(format!("claude:{message}:{index}"))
}

/// The item ID for the permission prompt about a tool call.
pub fn approval_id(call: &str) -> ItemId {
    ItemId(format!("claude:{call}:approval"))
}

/// The item for the files a call to MaxHQ's send tool sent.
pub fn sent_id(call: &str) -> ItemId {
    ItemId(format!("claude:{call}:sent"))
}

/// The item for the file a call to MaxHQ's open tool opened.
pub fn opened_id(call: &str) -> ItemId {
    ItemId(format!("claude:{call}:opened"))
}

/// The item for the page a call to MaxHQ's show tool showed.
pub fn visual_id(call: &str) -> ItemId {
    ItemId(format!("claude:{call}:visual"))
}

/// The turn a prompt starts.
pub fn turn_id(prompt: &str) -> TurnId {
    TurnId(format!("claude:{prompt}"))
}

/// Text that marks a turn cut short, which the CLI sends as a user line.
const INTERRUPTED: &str = "[Request interrupted by user";

/// Tags the CLI wraps around text it adds to the conversation itself.
const ADDED_TAGS: [&str; 7] = [
    "<task-notification>",
    "<local-command-caveat>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<system-reminder>",
    "<bash-stdout>",
    "<bash-stderr>",
];

/// Whether a transcript line holds text a client sent. The CLI marks such
/// a line, and never the lines it adds itself, so the tags and the
/// interrupt text in it are only what the sender wrote. A CLI too old to
/// mark its lines gives no way to tell.
fn client_sent(line: &Value) -> bool {
    text(line, "promptSource") == Some("sdk")
}

/// Whose timeline a line belongs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    /// The agent whose timeline it is.
    pub agent: AgentId,
    /// The item to nest under, for a sub-agent's line that arrives before
    /// the sub-agent has an agent of its own.
    pub parent: Option<ItemId>,
}

/// Where a tool call stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// The call is running or waiting for permission.
    Running,
    /// The call carries on in the background.
    Background,
    /// The call is over.
    Ended,
}

/// The wakeups and cron jobs the model set, which only the running CLI
/// fires. They're noted when their calls end, since old calls are
/// forgotten.
#[derive(Debug, Default)]
pub struct Schedule {
    /// When each wakeup is due, in milliseconds.
    pub wakeups: Vec<u64>,
    /// The cron jobs made, less the ones deleted.
    pub crons: i64,
    /// When the last cron job was made, in milliseconds.
    pub last_cron: u64,
}

impl Schedule {
    /// Notes a call to `name` that worked, which ended `at` milliseconds.
    fn made(&mut self, name: &str, input: &Value, at: u64) {
        match name {
            "ScheduleWakeup" => {
                let delay = input.get("delaySeconds").and_then(Value::as_u64);
                // A wakeup that fired before this call is done with.
                self.wakeups.retain(|&due| due > at);
                self.wakeups.push(at + delay.unwrap_or(0) * 1000);
            }
            "CronCreate" => {
                self.crons += 1;
                self.last_cron = self.last_cron.max(at);
            }
            "CronDelete" => self.crons -= 1,
            _ => {}
        }
    }
}

/// A tool call the model made.
#[derive(Clone, Debug)]
pub struct Tool {
    /// The tool's name, such as `Bash`.
    pub name: String,
    /// What the model passed it.
    pub input: Value,
    /// The item that shows it.
    pub item: ItemId,
    /// The agent that made the call.
    pub agent: AgentId,
    /// Where it stands.
    pub state: ToolState,
}

/// A tool call whose result just arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    /// The call's `tool_use` ID.
    pub tool_use_id: String,
    /// The tool's name.
    pub name: String,
    /// What the permission check said, when the result reports it.
    pub allowed: Option<bool>,
    /// Whether the call carries on in the background.
    pub background: bool,
}

/// Which content blocks of a message have started streaming but have no
/// finished `assistant` line yet.
#[derive(Debug, Default)]
struct Blocks {
    /// Each started block's index and kind, oldest first.
    started: Vec<(u64, BlockKind)>,
    /// The index after the highest one seen.
    next: u64,
}

/// What a content block holds.
#[derive(Clone, Debug, PartialEq, Eq)]
enum BlockKind {
    /// Reply text.
    Text,
    /// Thinking, shown or redacted.
    Thinking,
    /// A tool call, by its ID.
    Tool(String),
}

impl BlockKind {
    /// The kind of `block`, or none for one the timeline doesn't show.
    fn of(block: &Value) -> Option<Self> {
        match text(block, "type")? {
            "text" => Some(Self::Text),
            "thinking" | "redacted_thinking" => Some(Self::Thinking),
            "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                Some(Self::Tool(text(block, "id")?.to_owned()))
            }
            _ => None,
        }
    }
}

/// One conversation's items and what's needed to keep building them.
#[derive(Debug)]
pub struct Convo {
    /// The items so far.
    pub book: Book,
    /// The lead agent.
    pub lead: AgentId,
    /// The lead's running turn.
    pub turn: Option<TurnId>,
    /// Every tool call by its `tool_use` ID.
    pub tools: HashMap<String, Tool>,
    /// What the model set to fire later.
    pub schedule: Schedule,
    /// Sub-agents by the `tool_use` ID of the Agent call that started them.
    pub subs: HashMap<String, AgentId>,
    /// Agent calls whose sub-agent transcript was read, so the sub-agent's
    /// items nest under the call.
    pub sidechains: HashSet<String>,
    /// The account a usage limit or a login that ran out applies to.
    pub account: Option<AccountId>,
    /// When the account's usage limit resets, while one applies.
    pub resets_at: Option<Timestamp>,
    /// The folder the session runs in, for paths a transcript record
    /// doesn't place.
    pub cwd: PathBuf,
    /// Where pictures in tool results are kept, or none to leave them out.
    pub pictures: Option<Pictures>,
    /// Pictures for a container's guest to write, until the driver hands
    /// them over.
    pub kept: Kept,
    /// How many settled items it keeps besides the running turn's, as many
    /// as the timeline keeps in memory. The session manager holds every
    /// item, so the rest are only ever sent again.
    pub keep: usize,
    /// How many tokens the lead's context held at its last API call, from
    /// a transcript.
    context: Option<u64>,
    /// Streaming blocks by API message ID.
    blocks: HashMap<String, Blocks>,
    /// The API message streaming now, by the Agent call it belongs to.
    streams: HashMap<Option<String>, String>,
}

impl Convo {
    /// An empty conversation for `lead`.
    pub fn new(lead: AgentId) -> Self {
        Self {
            book: Book::default(),
            lead,
            turn: None,
            tools: HashMap::new(),
            schedule: Schedule::default(),
            subs: HashMap::new(),
            sidechains: HashSet::new(),
            account: None,
            resets_at: None,
            cwd: PathBuf::new(),
            pictures: None,
            kept: Kept::default(),
            keep: usize::from(DEFAULT_MEMORY_ITEMS),
            context: None,
            blocks: HashMap::new(),
            streams: HashMap::new(),
        }
    }

    /// Whose timeline a line with this `parent_tool_use_id` goes on.
    pub fn owner(&self, parent: Option<&str>) -> Owner {
        match parent {
            None => Owner {
                agent: self.lead,
                parent: None,
            },
            Some(call) => match self.subs.get(call) {
                Some(sub) => Owner {
                    agent: *sub,
                    parent: None,
                },
                None => Owner {
                    agent: self.lead,
                    parent: Some(item_id(call)),
                },
            },
        }
    }

    /// The owner of a live or transcript line.
    pub fn owner_of(&self, line: &Value) -> Owner {
        self.owner(text(line, "parent_tool_use_id"))
    }

    /// A new item for `owner`, in the lead's running turn when it's the
    /// lead's. One nested under another item joins that item's turn.
    pub fn item(&self, id: ItemId, owner: &Owner, at: Timestamp, body: ItemBody) -> TimelineItem {
        let turn = match owner
            .parent
            .as_ref()
            .and_then(|parent| self.book.get(parent))
        {
            Some(parent) => parent.turn.clone(),
            None => self.turn_of(owner.agent),
        };
        TimelineItem {
            id,
            agent: owner.agent,
            turn,
            parent: owner.parent.clone(),
            ordinal: 0,
            status: ItemStatus::Completed,
            started_at: Some(at),
            completed_at: Some(at),
            body,
        }
    }

    /// The turn new items of `agent` join.
    pub fn turn_of(&self, agent: AgentId) -> Option<TurnId> {
        if agent == self.lead {
            self.turn.clone()
        } else {
            None
        }
    }

    /// Forgets the blocks of finished messages, at the end of a turn.
    pub fn end_turn(&mut self) {
        self.turn = None;
        self.blocks.clear();
        self.streams.clear();
    }

    /// Forgets old items that won't change again, and the ended calls
    /// they showed. The running turn's items stay, since a reconnect reads
    /// its lines again.
    pub fn forget_settled(&mut self) {
        let turn = self.turn.as_ref();
        let looked = self.book.forget_settled(self.keep, |item| {
            !matches!(
                item.status,
                ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
            ) && (turn.is_none() || item.turn.as_ref() != turn)
        });
        if !looked {
            return;
        }
        let book = &self.book;
        self.tools
            .retain(|_, tool| tool.state != ToolState::Ended || book.contains(&tool.item));
    }

    /// Reads one transcript record.
    pub fn record(&mut self, record: &Value) {
        let at = record_time(record);
        match text(record, "type") {
            Some("user") => {
                self.user(record, at, false);
            }
            Some("assistant") => {
                if let Some(tokens) = context_tokens(record) {
                    self.context = Some(tokens);
                }
                self.assistant(record, at);
            }
            Some("system") => match text(record, "subtype") {
                Some("compact_boundary") => self.compaction(record, at),
                Some("turn_duration") => self.turn_duration(record, at),
                _ => {}
            },
            _ => {}
        }
    }

    /// Reads one record of the sub-agent transcript that the Agent call
    /// `call` started. Its items nest under the call, as a sub-agent's live
    /// lines do before it has an agent of its own.
    pub fn sidechain(&mut self, call: &str, record: &Value) {
        if !self.subs.contains_key(call) {
            self.sidechains.insert(call.to_owned());
        }
        let mut record = record.clone();
        if let Value::Object(fields) = &mut record {
            fields.insert("parent_tool_use_id".to_owned(), Value::from(call));
        }
        let at = record_time(&record);
        match text(&record, "type") {
            Some("user") => {
                self.user(&record, at, false);
            }
            Some("assistant") => self.assistant(&record, at),
            Some("system") if text(&record, "subtype") == Some("compact_boundary") => {
                self.compaction(&record, at);
            }
            _ => {}
        }
    }

    /// Ends the lead's turn with the summary an interactive session records.
    /// A session run with `-p` records none.
    fn turn_duration(&mut self, record: &Value, at: Timestamp) {
        let Some(uuid) = text(record, "uuid") else {
            return;
        };
        let body = ItemBody::TurnSummary {
            duration_ms: number(record, "durationMs"),
            context_tokens: self.context,
        };
        let item = self.item(item_id(uuid), &self.owner(None), at, body);
        self.book.put(item);
        self.end_turn();
    }

    /// Reads a live `stream_event` line: the start of a message or block,
    /// or a piece of text or thinking.
    pub fn stream_event(&mut self, line: &Value, at: Timestamp) {
        let Some(event) = line.get("event") else {
            return;
        };
        let parent = text(line, "parent_tool_use_id").map(str::to_owned);
        if text(event, "type") == Some("message_start") {
            let id = text(line, "api_message_id")
                .or_else(|| event.get("message").and_then(|message| text(message, "id")));
            if let Some(id) = id {
                self.streams.insert(parent, id.to_owned());
            }
            return;
        }
        let message = match text(line, "api_message_id") {
            Some(id) => id.to_owned(),
            None => match self.streams.get(&parent) {
                Some(id) => id.clone(),
                None => return,
            },
        };
        let Some(index) = number(event, "index") else {
            return;
        };
        match text(event, "type") {
            Some("content_block_start") => {
                if let Some(kind) = event.get("content_block").and_then(BlockKind::of) {
                    let blocks = self.blocks.entry(message).or_default();
                    blocks.started.push((index, kind));
                    blocks.next = blocks.next.max(index + 1);
                }
            }
            Some("content_block_delta") => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                let (piece, thinking) = match text(delta, "type") {
                    Some("text_delta") => (text_or_empty(delta, "text"), false),
                    Some("thinking_delta") => (text_or_empty(delta, "thinking"), true),
                    _ => return,
                };
                self.stream_text(line, &message, index, piece, thinking, at);
            }
            _ => {}
        }
    }

    /// Adds a piece of streamed text to its item, making the item first if
    /// this is its first piece.
    fn stream_text(
        &mut self,
        line: &Value,
        message: &str,
        index: u64,
        piece: &str,
        thinking: bool,
        at: Timestamp,
    ) {
        if piece.is_empty() {
            return;
        }
        let id = block_id(message, index);
        match self.book.get(&id).map(|item| is_streaming(&item.body)) {
            Some(true) => self.book.append(&id, TextField::Text, piece),
            // A finished block already has all its text.
            Some(false) => {}
            None => {
                let owner = self.owner_of(line);
                let text = piece.to_owned();
                let body = if thinking {
                    ItemBody::Reasoning {
                        text,
                        streaming: true,
                    }
                } else {
                    ItemBody::AssistantMessage {
                        text,
                        streaming: true,
                    }
                };
                let mut item = self.item(id, &owner, at, body);
                item.status = ItemStatus::Running;
                item.completed_at = None;
                self.book.put(item);
            }
        }
    }

    /// Reads an `assistant` line, live or from a transcript. Each holds one
    /// finished block of an API message.
    pub fn assistant(&mut self, line: &Value, at: Timestamp) {
        let owner = self.owner_of(line);
        if line.get("error").is_some() || flag(line, "isApiErrorMessage") {
            self.api_error(line, &owner, at);
            return;
        }
        let Some(message) = line.get("message") else {
            return;
        };
        let Some(id) = text(message, "id").map(str::to_owned) else {
            return;
        };
        let aborted = flag(line, "aborted") || flag(line, "isAbortedMidStream");
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            return;
        };
        for block in content {
            let Some(kind) = BlockKind::of(block) else {
                continue;
            };
            let index = self.block_index(&id, line, &kind);
            match kind {
                BlockKind::Text => {
                    let body = ItemBody::AssistantMessage {
                        text: text_or_empty(block, "text").to_owned(),
                        streaming: false,
                    };
                    self.finish_block(block_id(&id, index), &owner, at, body, aborted);
                }
                BlockKind::Thinking => {
                    let body = ItemBody::Reasoning {
                        text: text_or_empty(block, "thinking").to_owned(),
                        streaming: false,
                    };
                    self.finish_block(block_id(&id, index), &owner, at, body, aborted);
                }
                BlockKind::Tool(_) => self.tool_use(block, &owner, at),
            }
        }
    }

    /// The index of a finished block in its message: the transcript's
    /// `apiBlockIndex`, else the oldest started block of the same kind,
    /// else the next index.
    fn block_index(&mut self, message: &str, line: &Value, kind: &BlockKind) -> u64 {
        let blocks = self.blocks.entry(message.to_owned()).or_default();
        let index = if let Some(index) = number(line, "apiBlockIndex") {
            blocks.started.retain(|(at, _)| *at != index);
            index
        } else if let Some(at) = blocks.started.iter().position(|(_, seen)| seen == kind) {
            blocks.started.remove(at).0
        } else {
            blocks.next
        };
        blocks.next = blocks.next.max(index + 1);
        index
    }

    /// Puts a finished text or thinking block, replacing what streamed.
    /// Empty thinking, which is what the CLI sends when it hides thinking,
    /// shows nothing.
    fn finish_block(
        &mut self,
        id: ItemId,
        owner: &Owner,
        at: Timestamp,
        body: ItemBody,
        aborted: bool,
    ) {
        let empty = matches!(&body, ItemBody::AssistantMessage { text, .. } | ItemBody::Reasoning { text, .. } if text.is_empty());
        let streamed = self.book.get(&id).cloned();
        if empty && streamed.is_none() {
            return;
        }
        let mut item = match streamed {
            Some(mut item) => {
                item.body = body;
                item.completed_at = Some(at);
                item
            }
            None => self.item(id, owner, at, body),
        };
        item.status = if aborted {
            ItemStatus::Interrupted
        } else {
            ItemStatus::Completed
        };
        self.book.put(item);
    }

    /// Starts the item for a tool call.
    fn tool_use(&mut self, block: &Value, owner: &Owner, at: Timestamp) {
        let Some(id) = text(block, "id") else {
            return;
        };
        if self.tools.contains_key(id) {
            return;
        }
        let name = text_or_empty(block, "name").to_owned();
        let input = block
            .get("input")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let item_id = item_id(id);
        if !self.book.contains(&item_id) {
            let body = tools::start(&name, id, &input, &self.cwd);
            let mut item = self.item(item_id.clone(), owner, at, body);
            item.status = ItemStatus::Running;
            item.completed_at = None;
            self.book.put(item);
        }
        self.tools.insert(
            id.to_owned(),
            Tool {
                name,
                input,
                item: item_id,
                agent: owner.agent,
                state: ToolState::Running,
            },
        );
    }

    /// An `assistant` line the CLI wrote for a failed API call. A usage
    /// limit becomes a wait for the reset, anything else an error, which
    /// names the account when its login ran out.
    fn api_error(&mut self, line: &Value, owner: &Owner, at: Timestamp) {
        let Some(uuid) = text(line, "uuid") else {
            return;
        };
        let message = line
            .get("message")
            .and_then(|message| message.get("content"))
            .map(content_text)
            .unwrap_or_default();
        let body = if text(line, "error") == Some("rate_limit") {
            ItemBody::LimitWait {
                account: self.account,
                resets_at: self.resets_at.or_else(|| limit_reset(&message)),
                auto_resume: false,
            }
        } else {
            ItemBody::Error {
                message: if message.is_empty() {
                    "The API request failed.".to_owned()
                } else {
                    message
                },
                retrying: false,
                sign_in: self.account.filter(|_| signed_out(line)),
            }
        };
        let item = self.item(item_id(uuid), owner, at, body);
        self.book.put(item);
    }

    /// Reads a `user` line, live or from a transcript: tool results, a
    /// prompt, or the marker the CLI adds when a turn is cut short. Live
    /// prompts come from the driver itself, since the CLI doesn't echo
    /// them, so only a transcript's make items here.
    pub fn user(&mut self, line: &Value, at: Timestamp, live: bool) -> Vec<Ended> {
        if [
            "isMeta",
            "isSynthetic",
            "isCompactSummary",
            "isVisibleInTranscriptOnly",
        ]
        .into_iter()
        .any(|key| flag(line, key))
        {
            return Vec::new();
        }
        let Some(content) = line
            .get("message")
            .and_then(|message| message.get("content"))
        else {
            return Vec::new();
        };
        let results: Vec<&Value> = content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| text(block, "type") == Some("tool_result"))
            .collect();
        if !results.is_empty() {
            return results
                .into_iter()
                .filter_map(|block| self.tool_result(block, line, at))
                .collect();
        }
        let Some(uuid) = text(line, "uuid") else {
            return Vec::new();
        };
        let said = attach::said(content);
        let owner = self.owner_of(line);
        let sent = client_sent(line);
        if !sent && said.starts_with(INTERRUPTED) {
            self.interrupted(uuid, &owner, at);
        } else if owner.parent.is_none() && (!live || owner.agent != self.lead) {
            self.prompt(uuid, &said, sent, &owner, at);
        }
        Vec::new()
    }

    /// Puts the files the send tool call `call` sent, beside the call, once.
    pub fn sent(
        &mut self,
        call: &str,
        message: Option<String>,
        attachments: Vec<AttachmentRef>,
        at: Timestamp,
    ) {
        let body = ItemBody::AgentAttachments {
            text: message.unwrap_or_default(),
            attachments,
        };
        self.shown(call, sent_id(call), body, at);
    }

    /// Shows the file a call to MaxHQ's open tool opened, once, beside the
    /// call on its owner's timeline.
    pub fn opened(&mut self, call: &str, opened: ours::Opened, at: Timestamp) {
        let body = ItemBody::AgentOpenedFile {
            path: opened.path,
            line: opened.line,
            outside: opened.outside,
        };
        self.shown(call, opened_id(call), body, at);
    }

    /// Shows the page a call to MaxHQ's show tool showed, once, beside the
    /// call on its owner's timeline.
    pub fn visual(&mut self, call: &str, visual: ours::Visual, at: Timestamp) {
        let body = ItemBody::AgentVisual {
            title: visual.title,
            attachment: visual.attachment,
            replaces: visual.replaces,
        };
        self.shown(call, visual_id(call), body, at);
    }

    /// Puts what one of MaxHQ's tools showed, once, on the timeline of the
    /// call's owner.
    fn shown(&mut self, call: &str, id: ItemId, body: ItemBody, at: Timestamp) {
        if self.book.contains(&id) {
            return;
        }
        let owner = self
            .book
            .get(&item_id(call))
            .map(|tool| Owner {
                agent: tool.agent,
                parent: tool.parent.clone(),
            })
            .unwrap_or_else(|| self.owner(None));
        let item = self.item(id, &owner, at, body);
        self.book.put(item);
    }

    /// Puts a prompt read back from a transcript, or a sub-agent's prompt.
    /// A lead's prompt starts a turn. Text a client sent reads as it is.
    fn prompt(&mut self, uuid: &str, said: &str, sent: bool, owner: &Owner, at: Timestamp) {
        let Some(said) = (if sent {
            Some(said.to_owned())
        } else {
            typed(said)
        }) else {
            return;
        };
        if owner.agent == self.lead {
            self.turn = Some(turn_id(uuid));
        }
        let id = item_id(uuid);
        if self.book.contains(&id) {
            return;
        }
        let (said, attachments, refs) = attach::parse(&said);
        let body = ItemBody::UserMessage {
            text: said,
            attachments,
            refs,
            delivery: Delivery::Sent,
        };
        let item = self.item(id, owner, at, body);
        self.book.put(item);
    }

    /// Marks a turn cut short: streaming text and running calls stop, and
    /// an interrupted marker goes on the timeline. A sub-agent nested under
    /// its call cuts only what's nested there.
    fn interrupted(&mut self, uuid: &str, owner: &Owner, at: Timestamp) {
        let agent = owner.agent;
        let book = &self.book;
        let inside = |item: &TimelineItem| {
            item.agent == agent && (owner.parent.is_none() || item.parent == owner.parent)
        };
        let cut: Vec<ItemId> = book
            .items()
            .filter(|item| inside(item) && is_streaming(&item.body))
            .map(|item| item.id.clone())
            .chain(
                self.tools
                    .values()
                    .filter(|tool| {
                        tool.state == ToolState::Running && book.get(&tool.item).is_some_and(inside)
                    })
                    .map(|tool| tool.item.clone()),
            )
            .collect();
        for id in &cut {
            self.book.update(id, |item| {
                stop_streaming(&mut item.body);
                if matches!(
                    item.status,
                    ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
                ) {
                    item.status = ItemStatus::Interrupted;
                    item.completed_at = Some(at);
                }
            });
        }
        for tool in self.tools.values_mut() {
            if tool.agent == agent && tool.state == ToolState::Running {
                tool.state = ToolState::Ended;
            }
        }
        let item = self.item(item_id(uuid), owner, at, ItemBody::Interrupted);
        self.book.put(item);
    }

    /// Fills in a tool call's item from its result.
    fn tool_result(&mut self, block: &Value, line: &Value, at: Timestamp) -> Option<Ended> {
        let call = text(block, "tool_use_id")?;
        let tool = self.tools.get_mut(call)?;
        if tool.state == ToolState::Ended {
            return None;
        }
        let said = tools::result_text(block);
        let failed = flag(block, "is_error");
        let result = line
            .get("tool_use_result")
            .or_else(|| line.get("toolUseResult"))
            .cloned()
            .unwrap_or(Value::Null);
        let check = permission(line, call);
        let allowed = match check.decision.as_deref() {
            Some("accept") => Some(true),
            Some("reject") => Some(false),
            _ => None,
        };
        let cut = said.starts_with(INTERRUPTED);
        let declined = allowed == Some(false) || check.refused || tools::dismissed(&said);
        let mut background = false;
        let mut worked = false;
        let input = tool.input.clone();
        let name = tool.name.clone();
        let images = self
            .pictures
            .as_ref()
            .map(|kept| pictures::keep(block.get("content"), tool.agent, kept, &mut self.kept))
            .unwrap_or_default();
        self.book.update(&tool.item, |item| {
            pictures::show(&mut item.body, images);
            match tools::finish(&mut item.body, &input, &result, &said, failed) {
                Finish::Background => {
                    background = true;
                    item.status = ItemStatus::Running;
                }
                Finish::Ended(status) => {
                    // A call whose prompt you turned down stays declined
                    // as the turn it stopped ends.
                    item.status = if item.status == ItemStatus::Declined {
                        ItemStatus::Declined
                    } else if cut {
                        ItemStatus::Interrupted
                    } else if declined {
                        ItemStatus::Declined
                    } else {
                        status
                    };
                    item.completed_at = Some(at);
                    worked = item.status == ItemStatus::Completed;
                }
            }
        });
        tool.state = if background {
            ToolState::Background
        } else {
            ToolState::Ended
        };
        if worked {
            self.schedule.made(&name, &input, at.0);
        }
        // A send read back from a transcript shows its files again, from
        // what the call said back.
        if !failed
            && !cut
            && ours::Tool::from_claude(&name) == Some(ours::Tool::SendAttachments)
            && let Some(mut attachments) = ours::parse_sent(&said)
        {
            crate::sidecar::add_previews(&mut attachments);
            let message = text(&input, "message")
                .filter(|message| !message.trim().is_empty())
                .map(str::to_owned);
            self.sent(call, message, attachments, at);
        }
        // So does an open, from the file its answer names.
        if !failed
            && !cut
            && ours::Tool::from_claude(&name) == Some(ours::Tool::OpenFile)
            && let Some(opened) = ours::parse_opened(&said)
        {
            self.opened(call, opened, at);
        }
        // And a show, from the page its answer names.
        if !failed
            && !cut
            && ours::Tool::from_claude(&name) == Some(ours::Tool::ShowVisual)
            && let Some(visual) = ours::parse_visual(&said)
        {
            self.visual(call, visual, at);
        }
        self.answered(call, &check, &said, line, at);
        // Nothing reads an ended call's input again, and it can be long.
        if let Some(tool) = self.tools.get_mut(call)
            && tool.state == ToolState::Ended
        {
            tool.input = Value::Null;
        }
        Some(Ended {
            tool_use_id: call.to_owned(),
            name,
            allowed,
            background,
        })
    }

    /// Puts the approval a tool result says you answered, when no prompt
    /// showed it: one answered before `maxd` attached, or one read back
    /// from a transcript. The result holds neither the request's ID nor
    /// the options it offered, so the approval gets a stand-in attention
    /// ID and shows the options that fit the answer.
    fn answered(
        &mut self,
        call: &str,
        check: &Permission,
        said: &str,
        line: &Value,
        at: Timestamp,
    ) {
        let id = approval_id(call);
        let Some(source) = check
            .source
            .as_deref()
            .and_then(|source| source.strip_prefix("user_"))
        else {
            return;
        };
        let Some(Tool { name, input, .. }) = self.tools.get(call) else {
            return;
        };
        let Some(tool) = self.book.get(&item_id(call)) else {
            return;
        };
        if name == "AskUserQuestion" || self.book.contains(&id) {
            return;
        }
        let (name, input) = (name.as_str(), input);
        let plan = name == "ExitPlanMode";
        let (answer, status) = match (check.decision.as_deref(), source) {
            (_, "abort") => (None, ItemStatus::Cancelled),
            (Some("accept"), "permanent") if !plan => {
                (Some(option("allow_session")), ItemStatus::Completed)
            }
            (Some("accept"), _) => (Some(option("allow")), ItemStatus::Completed),
            (Some("reject"), _) => (tools::refusal_answer(said), ItemStatus::Declined),
            _ => return,
        };
        let options = if plan {
            tools::plan_options()
        } else {
            tools::tool_options(source == "permanent")
        };
        let cwd = text(line, "cwd").map_or_else(|| self.cwd.clone(), PathBuf::from);
        let card = tools::approval_card(name, input, &cwd, &Asked::default());
        let body = ItemBody::Approval {
            attention: AttentionId::derive(&id.0),
            approval_kind: if plan {
                ApprovalKind::Plan
            } else {
                tools::approval_kind(name)
            },
            title: card.title,
            detail: card.detail,
            cwd: tools::approval_cwd(input, &cwd),
            options,
            decided: answer.map(|answer| Decided {
                answer,
                by: None,
                at,
            }),
            deny_first: false,
            reason: card.reason,
        };
        let owner = Owner {
            agent: tool.agent,
            parent: Some(tool.id.clone()),
        };
        let started_at = tool.started_at;
        let mut item = self.item(id, &owner, at, body);
        item.status = status;
        item.started_at = started_at;
        self.book.put(item);
    }

    /// Puts the divider a compaction leaves.
    pub fn compaction(&mut self, line: &Value, at: Timestamp) {
        let Some(uuid) = text(line, "uuid") else {
            return;
        };
        let meta = line
            .get("compact_metadata")
            .or_else(|| line.get("compactMetadata"));
        let count = |snake: &str, camel: &str| {
            meta.and_then(|meta| number(meta, snake).or_else(|| number(meta, camel)))
        };
        let body = ItemBody::Compaction {
            before_tokens: count("pre_tokens", "preTokens"),
            after_tokens: count("post_tokens", "postTokens"),
        };
        let item = self.item(item_id(uuid), &self.owner_of(line), at, body);
        self.book.put(item);
    }
}

/// What a tool result's permission check said.
#[derive(Debug)]
struct Permission {
    /// `accept` or `reject`.
    decision: Option<String>,
    /// Who decided, such as `config`, `hook` or `user_temporary`.
    source: Option<String>,
    /// Whether the call was refused without running.
    refused: bool,
}

/// What a tool result's permission check said. The live stream puts this
/// in `tool_result_meta`, a transcript on the record itself.
fn permission(line: &Value, call: &str) -> Permission {
    let read = |decision: Option<&Value>, refused: bool| {
        let field = |key| {
            decision
                .and_then(|decision| text(decision, key))
                .map(str::to_owned)
        };
        Permission {
            decision: field("decision"),
            source: field("source"),
            refused,
        }
    };
    if let Some(meta) = line
        .get("tool_result_meta")
        .and_then(Value::as_array)
        .and_then(|metas| metas.iter().find(|meta| text(meta, "id") == Some(call)))
    {
        return read(
            meta.get("permission_decision"),
            meta.get("non_execution_kind").is_some(),
        );
    }
    read(
        line.get("permissionDecision"),
        line.get("toolDenialKind").is_some(),
    )
}

/// An answer that picks the option `id`.
fn option(id: &str) -> AttentionAnswer {
    AttentionAnswer::Option { id: id.to_owned() }
}

/// When a transcript record was written.
fn record_time(record: &Value) -> Timestamp {
    text(record, "timestamp")
        .and_then(parse_iso8601)
        .unwrap_or(Timestamp(0))
}

/// How many tokens the context held for an API call a transcript record
/// shows. The CLI's own notes, from the `<synthetic>` model, made no call.
fn context_tokens(record: &Value) -> Option<u64> {
    let message = record.get("message")?;
    if text(message, "model") == Some("<synthetic>") {
        return None;
    }
    let usage = message.get("usage")?;
    let input = number(usage, "input_tokens")?;
    let cached = ["cache_read_input_tokens", "cache_creation_input_tokens"]
        .into_iter()
        .filter_map(|key| number(usage, key))
        .sum::<u64>();
    Some(input + cached)
}

/// What you typed, from a prompt's text: slash commands the transcript
/// wraps in tags come back as you typed them, and text the CLI added
/// itself gives nothing.
fn typed(said: &str) -> Option<String> {
    let trimmed = said.trim_start();
    if ADDED_TAGS.iter().any(|tag| trimmed.starts_with(tag)) {
        return None;
    }
    if let Some(name) = between(trimmed, "<command-name>", "</command-name>") {
        let args = between(trimmed, "<command-args>", "</command-args>").unwrap_or_default();
        let name = if name.starts_with('/') {
            name.to_owned()
        } else {
            format!("/{name}")
        };
        return Some(if args.is_empty() {
            name
        } else {
            format!("{name} {args}")
        });
    }
    if let Some(command) = between(trimmed, "<bash-input>", "</bash-input>") {
        return Some(format!("!{command}"));
    }
    Some(said.to_owned())
}

/// The text between `open` and `close`.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(text[start..end].trim())
}

/// When a usage limit resets, from the CLI's "usage limit reached|<time>".
fn limit_reset(message: &str) -> Option<Timestamp> {
    let (_, seconds) = message.rsplit_once('|')?;
    super::wire::unix_seconds(&Value::String(seconds.trim().to_owned()))
}

/// Whether text is still streaming into an item.
fn is_streaming(body: &ItemBody) -> bool {
    matches!(
        body,
        ItemBody::AssistantMessage {
            streaming: true,
            ..
        } | ItemBody::Reasoning {
            streaming: true,
            ..
        } | ItemBody::Plan {
            streaming: true,
            ..
        }
    )
}

/// Marks streamed text as finished.
pub fn stop_streaming(body: &mut ItemBody) {
    if let ItemBody::AssistantMessage { streaming, .. }
    | ItemBody::Reasoning { streaming, .. }
    | ItemBody::Plan { streaming, .. } = body
    {
        *streaming = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn convo() -> Convo {
        Convo::new(AgentId::derive("lead"))
    }

    fn line(uuid: &str, said: &str, source: Option<&str>) -> Value {
        let mut line = json!({
            "type": "user", "uuid": uuid, "timestamp": "2026-10-25T09:48:42.370Z",
            "message": {"role": "user", "content": [{"type": "text", "text": said}]},
        });
        if let Some(source) = source {
            line["promptSource"] = json!(source);
        }
        line
    }

    fn shown(convo: &Convo, uuid: &str) -> Option<String> {
        match &convo.book.get(&item_id(uuid))?.body {
            ItemBody::UserMessage { text, .. } => Some(text.clone()),
            _ => None,
        }
    }

    #[test]
    fn text_a_client_sent_reads_back_whole_whatever_tags_it_holds() {
        for said in [
            "<system-reminder>do this</system-reminder>\nshown",
            "<task-notification>do this</task-notification>",
            "<local-command-stdout>do this</local-command-stdout>",
            "do this <command-name>/help</command-name>",
            "do this <bash-input>ls</bash-input>",
            "[Request interrupted by user] do this",
        ] {
            let mut convo = self::convo();
            convo.record(&line("u1", said, Some("sdk")));
            assert_eq!(shown(&convo, "u1").as_deref(), Some(said));
        }
    }

    #[test]
    fn lines_the_cli_adds_still_read_as_its_own() {
        for source in [None, Some("system")] {
            let mut convo = self::convo();
            convo.record(&line(
                "u1",
                "<task-notification>x</task-notification>",
                source,
            ));
            assert_eq!(shown(&convo, "u1"), None);
        }
        let mut convo = self::convo();
        convo.record(&line("u1", "<command-name>/help</command-name>", None));
        assert_eq!(shown(&convo, "u1").as_deref(), Some("/help"));
        let mut convo = self::convo();
        convo.record(&line("u1", "[Request interrupted by user]", None));
        assert!(matches!(
            convo.book.get(&item_id("u1")).unwrap().body,
            ItemBody::Interrupted
        ));
    }

    #[test]
    fn transcript_prompts_start_turns_and_keep_their_attachments() {
        let mut convo = convo();
        convo.record(&json!({
            "type": "user", "uuid": "u1", "timestamp": "2026-10-25T09:48:42.370Z",
            "message": {"role": "user", "content": [{"type": "text",
                "text": "Look\n\nAttached files:\n- image, sent inline: /a/b.png"}]},
        }));
        let item = convo.book.get(&item_id("u1")).unwrap();
        assert_eq!(item.turn, Some(turn_id("u1")));
        let ItemBody::UserMessage {
            text, attachments, ..
        } = &item.body
        else {
            panic!("{item:?}");
        };
        assert_eq!(text, "Look");
        assert_eq!(attachments[0].path, "/a/b.png");
    }

    #[test]
    fn slash_commands_read_back_as_typed_and_added_text_is_skipped() {
        assert_eq!(
            typed("<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>").as_deref(),
            Some("/model opus")
        );
        assert_eq!(
            typed("<local-command-stdout>Set model</local-command-stdout>"),
            None
        );
        assert_eq!(typed("<bash-input>ls</bash-input>").as_deref(), Some("!ls"));
        assert_eq!(typed("a < b").as_deref(), Some("a < b"));
    }

    #[test]
    fn a_rate_limit_error_waits_for_the_reset() {
        let mut convo = convo();
        convo.record(&json!({
            "type": "assistant", "uuid": "e1", "timestamp": "2026-10-25T09:48:42.370Z",
            "error": "rate_limit", "isApiErrorMessage": true,
            "message": {"id": "x", "model": "<synthetic>", "content": [
                {"type": "text", "text": "Claude AI usage limit reached|1792936200"}]},
        }));
        assert_eq!(
            convo.book.get(&item_id("e1")).unwrap().body,
            ItemBody::LimitWait {
                account: None,
                resets_at: Some(Timestamp(1_792_936_200_000)),
                auto_resume: false
            }
        );
    }

    /// A transcript's prompt.
    fn prompt_record(uuid: &str) -> Value {
        json!({
            "type": "user", "uuid": uuid, "timestamp": "2026-10-25T09:48:42.000Z",
            "cwd": "/home/user/work",
            "message": {"role": "user", "content": "Do the thing"},
        })
    }

    /// A transcript's tool call.
    fn call_record(call: &str, name: &str, input: Value) -> Value {
        json!({
            "type": "assistant", "uuid": format!("a-{call}"),
            "timestamp": "2026-10-25T09:48:43.000Z", "cwd": "/home/user/work",
            "message": {"id": format!("msg-{call}"), "role": "assistant", "content": [
                {"type": "tool_use", "id": call, "name": name, "input": input}]},
        })
    }

    /// A transcript's tool result with the permission check's decision.
    fn decided_record(call: &str, said: &str, decision: Value) -> Value {
        json!({
            "type": "user", "uuid": format!("r-{call}"),
            "timestamp": "2026-10-25T09:48:44.000Z", "cwd": "/home/user/work",
            "message": {"role": "user", "content": [{
                "type": "tool_result", "tool_use_id": call, "content": said,
                "is_error": decision["decision"] == "reject",
            }]},
            "permissionDecision": decision,
        })
    }

    /// A conversation that read one call and the decision on it.
    fn decided(name: &str, input: Value, said: &str, decision: Value) -> Convo {
        let mut convo = convo();
        convo.record(&prompt_record("u1"));
        convo.record(&call_record("toolu_1", name, input));
        convo.record(&decided_record("toolu_1", said, decision));
        convo
    }

    /// The approval read back for `toolu_1`, with its answer and options.
    fn read_back(convo: &Convo) -> (&TimelineItem, Option<AttentionAnswer>, Vec<String>) {
        let item = convo
            .book
            .get(&approval_id("toolu_1"))
            .expect("an approval");
        let ItemBody::Approval {
            decided, options, ..
        } = &item.body
        else {
            panic!("{item:?}");
        };
        let options = options.iter().map(|option| option.id.clone()).collect();
        let answer = decided.as_ref().map(|decided| {
            assert_eq!(decided.by, None, "the transcript doesn't say who answered");
            decided.answer.clone()
        });
        (item, answer, options)
    }

    fn picked(id: &str) -> Option<AttentionAnswer> {
        Some(AttentionAnswer::Option { id: id.to_owned() })
    }

    #[test]
    fn a_prompt_you_answered_reads_back_as_an_approval_under_its_call() {
        let input = json!({"file_path": "/home/user/work/a.txt", "content": "ALPHA"});
        let convo = decided(
            "Write",
            input,
            "File created successfully at: /home/user/work/a.txt",
            json!({"decision": "accept", "source": "user_temporary", "grantScope": "once"}),
        );
        let (item, answer, options) = read_back(&convo);
        assert_eq!(item.agent, AgentId::derive("lead"));
        assert_eq!(item.parent, Some(item_id("toolu_1")));
        assert_eq!(item.turn, Some(turn_id("u1")));
        assert_eq!(item.status, ItemStatus::Completed);
        assert_eq!(answer, picked("allow"));
        assert_eq!(options, ["allow", "deny"]);
        let ItemBody::Approval {
            approval_kind,
            title,
            detail,
            cwd,
            attention,
            ..
        } = &item.body
        else {
            panic!("{item:?}");
        };
        assert_eq!(*approval_kind, ApprovalKind::FileChange);
        assert_eq!(title, "Write a.txt?");
        assert_eq!(*detail, None);
        // The folder the transcript line ran in.
        assert_eq!(cwd.as_deref(), Some("/home/user/work"));
        assert_eq!(
            *attention,
            AttentionId::derive("claude:toolu_1:approval"),
            "a stand-in, since the transcript has no request ID"
        );
        assert_eq!(
            convo.book.get(&item_id("toolu_1")).unwrap().status,
            ItemStatus::Completed
        );
    }

    #[test]
    fn each_answer_reads_back_as_the_option_or_the_words_you_gave() {
        let input = json!({"command": "rm -rf build"});
        let refused = "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file).";
        let cases = [
            ("accept", "user_permanent", "ok", picked("allow_session")),
            ("reject", "user_reject", "The user denied this request.", picked("deny")),
            (
                "reject",
                "user_reject",
                &format!("{refused} STOP what you are doing and wait for the user to tell you how to proceed."),
                picked("deny"),
            ),
            (
                "reject",
                "user_reject",
                &format!("{refused} To tell you how to proceed, the user said:\nKeep the build"),
                Some(AttentionAnswer::Text {
                    text: "Keep the build".to_owned(),
                }),
            ),
            (
                "reject",
                "user_reject",
                "Keep the build",
                Some(AttentionAnswer::Text {
                    text: "Keep the build".to_owned(),
                }),
            ),
        ];
        for (decision, source, said, expected) in cases {
            let convo = decided(
                "Bash",
                input.clone(),
                said,
                json!({"decision": decision, "source": source}),
            );
            let (item, answer, options) = read_back(&convo);
            assert_eq!(answer, expected, "{source}: {said}");
            let status = if decision == "accept" {
                ItemStatus::Completed
            } else {
                ItemStatus::Declined
            };
            assert_eq!(item.status, status, "{source}: {said}");
            if source == "user_permanent" {
                assert_eq!(options, ["allow", "allow_session", "deny"]);
            }
            let ItemBody::Approval { title, detail, .. } = &item.body else {
                panic!("{item:?}");
            };
            assert_eq!(title, "Run `rm -rf build`?");
            assert_eq!(detail.as_deref(), Some("rm -rf build"));
        }
    }

    #[test]
    fn a_prompt_the_turns_end_cut_off_reads_back_as_cancelled() {
        let convo = decided(
            "Bash",
            json!({"command": "ls"}),
            "[Request interrupted by user for tool use]",
            json!({"decision": "reject", "source": "user_abort"}),
        );
        let (item, answer, _) = read_back(&convo);
        assert_eq!(answer, None);
        assert_eq!(item.status, ItemStatus::Cancelled);
    }

    #[test]
    fn a_plan_you_answered_reads_back_with_the_plans_options() {
        let input = json!({"plan": "1. Write a.txt"});
        let started = decided(
            "ExitPlanMode",
            input.clone(),
            "User has approved your plan.",
            json!({"decision": "accept", "source": "user_temporary"}),
        );
        let (item, answer, options) = read_back(&started);
        assert_eq!(options, ["accept_edits", "accept", "deny"]);
        assert_eq!(answer, picked("allow"), "the transcript doesn't say which");
        assert!(matches!(
            &item.body,
            ItemBody::Approval { approval_kind: ApprovalKind::Plan, title, .. } if title == "Ready to code?"
        ));
        let kept = decided(
            "ExitPlanMode",
            input,
            "The user wants to keep planning.",
            json!({"decision": "reject", "source": "user_reject"}),
        );
        assert_eq!(read_back(&kept).1, picked("deny"));
    }

    #[test]
    fn only_your_own_answers_read_back_as_approvals() {
        for source in ["config", "hook", "classifier"] {
            let convo = decided(
                "Bash",
                json!({"command": "ls"}),
                "a.txt",
                json!({"decision": "accept", "source": source}),
            );
            assert!(!convo.book.contains(&approval_id("toolu_1")), "{source}");
        }
        let questions = json!({"questions": [{"question": "Which?", "options": []}]});
        let asked = decided(
            "AskUserQuestion",
            questions,
            "User has answered your questions",
            json!({"decision": "accept", "source": "user_temporary"}),
        );
        assert!(!asked.book.contains(&approval_id("toolu_1")));
    }

    #[test]
    fn a_turns_recorded_duration_ends_it_with_a_summary() {
        let mut convo = convo();
        convo.record(&prompt_record("u1"));
        convo.record(&json!({
            "type": "assistant", "uuid": "a1", "timestamp": "2026-10-25T09:48:43.000Z",
            "message": {"id": "msg-1", "model": "claude-opus-5-5", "role": "assistant",
                "content": [{"type": "text", "text": "Done"}],
                "usage": {"input_tokens": 4, "cache_creation_input_tokens": 900,
                    "cache_read_input_tokens": 12_000, "output_tokens": 50}},
        }));
        convo.record(&json!({
            "type": "assistant", "uuid": "a2", "timestamp": "2026-10-25T09:48:44.000Z",
            "message": {"id": "msg-2", "model": "<synthetic>", "role": "assistant",
                "content": [{"type": "text", "text": "No response requested."}],
                "usage": {"input_tokens": 0}},
        }));
        convo.record(&json!({
            "type": "system", "subtype": "turn_duration", "uuid": "t1",
            "timestamp": "2026-10-25T09:48:45.000Z", "durationMs": 5123,
            "messageCount": 3, "isMeta": false,
        }));
        let summary = convo.book.get(&item_id("t1")).expect("a summary");
        assert_eq!(summary.turn, Some(turn_id("u1")));
        assert_eq!(
            summary.body,
            ItemBody::TurnSummary {
                duration_ms: Some(5123),
                context_tokens: Some(12_904),
            }
        );
        assert_eq!(convo.turn, None);
    }

    #[test]
    fn compaction_reads_both_spellings() {
        let mut convo = convo();
        convo.compaction(
            &json!({"uuid": "c1", "compact_metadata": {"trigger": "auto", "pre_tokens": 150_000, "post_tokens": 9000}}),
            Timestamp(1),
        );
        convo.record(
            &json!({"type": "system", "subtype": "compact_boundary", "uuid": "c2",
            "compactMetadata": {"trigger": "manual", "preTokens": 80_000}}),
        );
        assert_eq!(
            convo.book.get(&item_id("c1")).unwrap().body,
            ItemBody::Compaction {
                before_tokens: Some(150_000),
                after_tokens: Some(9000)
            }
        );
        assert_eq!(
            convo.book.get(&item_id("c2")).unwrap().body,
            ItemBody::Compaction {
                before_tokens: Some(80_000),
                after_tokens: None
            }
        );
    }
}
