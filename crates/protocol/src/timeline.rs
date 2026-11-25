// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An agent's conversation, as a list of items.
//!
//! The state holds what every client needs for the sidebar. A timeline holds
//! the whole conversation for one agent, so `maxd` sends it only to clients
//! that open it. A client gets the newest items, then each change in order,
//! and asks for older items as you scroll up.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::attention::{AttentionAnswer, AttentionOption};
use crate::context::InlineRef;
use crate::ids::{AccountId, AgentId, AttachmentId, AttentionId, ClientId, DeviceId, TerminalId};
use crate::model::{EditKind, Vendor};
use crate::state::Seq;
use crate::terminal::{FreshReason, SessionBlock, TerminalStatus};
use crate::time::Timestamp;

/// One entry in an agent's conversation. `<vendor>:<native id>` when the
/// CLI gave it an ID, otherwise minted by the driver.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type, JsonSchema,
)]
#[serde(transparent)]
pub struct ItemId(pub String);

/// One turn, as the CLI numbers it.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Type, JsonSchema,
)]
#[serde(transparent)]
pub struct TurnId(pub String);

/// One entry in a timeline. On the wire the body's fields sit beside the
/// item's own, with `kind` naming the body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TimelineItem {
    /// Stable for the life of the item, so a later event can replace it.
    pub id: ItemId,
    /// The agent whose conversation this is.
    pub agent: AgentId,
    /// The turn the item came in, when the CLI numbers turns.
    pub turn: Option<TurnId>,
    /// The item this one belongs under, such as a command an approval is for.
    pub parent: Option<ItemId>,
    /// Position in the timeline, from 0, assigned by maxd. Never reused.
    #[specta(type = specta_typescript::Number)]
    pub ordinal: u64,
    /// Where the item is in its life.
    pub status: ItemStatus,
    /// When the item began, if the CLI said.
    pub started_at: Option<Timestamp>,
    /// When the item ended, if it has and the CLI said.
    pub completed_at: Option<Timestamp>,
    /// What the item is. A kind this build can't read is
    /// [`ItemBody::Unknown`].
    #[serde(flatten, deserialize_with = "crate::tolerant::or_unknown")]
    #[specta(type = ItemBody)]
    pub body: ItemBody,
}

/// Where an item is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Known but not started, such as a queued message.
    Pending,
    /// Under way.
    Running,
    /// Blocked on you, such as an approval nobody has answered.
    Waiting,
    /// Finished as it should.
    Completed,
    /// Finished with an error.
    Failed,
    /// You or a policy said no.
    Declined,
    /// Called off before it started.
    Cancelled,
    /// Stopped partway because the turn was interrupted.
    Interrupted,
}

/// What a timeline item is, tagged by `kind`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemBody {
    /// Something you sent.
    UserMessage {
        /// What you typed, with U+FFFC where each placed ref sits.
        text: String,
        /// Files and folders sent with it.
        attachments: Vec<AttachmentRef>,
        /// The context in the text, in text order. A `maxd` from before
        /// leaves this out.
        #[serde(default)]
        refs: Vec<InlineRef>,
        /// How it reached the agent.
        delivery: Delivery,
    },
    /// The agent's reply.
    AssistantMessage {
        /// The reply so far.
        text: String,
        /// Set while more text is still coming.
        streaming: bool,
    },
    /// The agent's thinking, where the CLI shows it.
    Reasoning {
        /// The thinking so far.
        text: String,
        /// Set while more text is still coming.
        streaming: bool,
    },
    /// A shell command.
    Command {
        /// The command line as the agent ran it.
        command: String,
        /// The folder it ran in, if the CLI said.
        cwd: Option<String>,
        /// The exit status, once it has one.
        exit_code: Option<i32>,
        /// What it printed, stdout and stderr together.
        output: String,
        /// Set when `output` holds only part of what it printed.
        output_truncated: bool,
        /// Set when the command keeps running after the agent moves on.
        background: bool,
    },
    /// Edits to one or more files.
    FileChange {
        /// One entry for each file.
        changes: Vec<FileEdit>,
    },
    /// The agent read a file.
    FileRead {
        /// The file, as the agent named it.
        path: String,
        /// The lines it read, such as "10-40", or none for the whole file.
        lines: Option<String>,
        /// The pictures the read showed the agent, kept where the device
        /// can serve them. A `maxd` from before leaves this out.
        #[serde(default)]
        images: Vec<AttachmentRef>,
    },
    /// A search through files.
    Search {
        /// What the agent searched for.
        query: String,
        /// Where it searched, such as a folder or a glob.
        scope: Option<String>,
        /// How many matches it found, once it knows.
        matches: Option<u32>,
    },
    /// A web search or fetch.
    Web {
        /// Which of the two.
        action: WebAction,
        /// The query or the URL.
        target: String,
    },
    /// A tool that has no kind of its own here, such as one from an MCP server.
    ToolCall {
        /// The MCP server that provides the tool, if any.
        server: Option<String>,
        /// The tool's name.
        name: String,
        /// One line to show, if the CLI gave one.
        title: Option<String>,
        /// The input, as the CLI showed it.
        input: Option<String>,
        /// The result, once there is one.
        output: Option<String>,
        /// The pictures in the result, kept where the device can serve
        /// them. A `maxd` from before leaves this out.
        #[serde(default)]
        images: Vec<AttachmentRef>,
        /// What the call did with a claude.ai page, for Claude Code's
        /// artifact tools. A `maxd` from before
        /// [`crate::message::ARTIFACTS_SINCE`] leaves it out, which reads
        /// as a plain tool call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[specta(optional)]
        artifact: Option<Box<ArtifactCall>>,
    },
    /// A sub-agent the agent started.
    Subagent {
        /// The sub-agent, once `maxd` knows which agent it is. Named
        /// `subagent` so it doesn't clash with the item's own `agent`.
        subagent: Option<AgentId>,
        /// The kind of sub-agent, as the CLI names it.
        agent_type: Option<String>,
        /// What the agent asked it to do.
        description: String,
        /// What it reported back, once it has.
        result: Option<String>,
    },
    /// A plan the agent wrote.
    Plan {
        /// The plan so far, as Markdown.
        markdown: String,
        /// Set while more text is still coming.
        streaming: bool,
    },
    /// The agent's to-do list, replaced whole on each change.
    Todos {
        /// In the agent's order.
        todos: Vec<Todo>,
    },
    /// The agent asked to do something.
    Approval {
        /// The attention event you answer.
        attention: AttentionId,
        /// What it asked to do. Named `approval_kind` because `kind` is the
        /// tag that names the body.
        approval_kind: ApprovalKind,
        /// One line to show.
        title: String,
        /// More to show, such as the full command or the diff.
        detail: Option<String>,
        /// The folder the request is for, as the CLI said or else where
        /// the agent runs.
        cwd: Option<String>,
        /// The answers you can pick, in order.
        options: Vec<AttentionOption>,
        /// How it ended, once it has.
        decided: Option<Decided>,
        /// The rules "Always allow" saves for a command, as their words, when
        /// MaxHQ keeps them. An older `maxd` leaves it out, which reads as
        /// none. Since protocol 56.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        saves: Vec<String>,
        /// Whether turning it down is the default, so a stray Enter can't
        /// allow it, as for a call only a person may approve. A `maxd` from
        /// before [`crate::message::ARTIFACTS_SINCE`] leaves it out, which
        /// reads as no.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[specta(optional)]
        deny_first: bool,
        /// Why the agent's CLI asks instead of going ahead, such as a
        /// safety check the call tripped, when it says. A `maxd` from
        /// before [`crate::message::ARTIFACTS_SINCE`] leaves it out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[specta(optional)]
        reason: Option<String>,
    },
    /// The agent asked you one or more questions.
    Question {
        /// The attention event you answer.
        attention: AttentionId,
        /// In the agent's order.
        questions: Vec<Question>,
        /// Your answers, once you've given them.
        answers: Option<Vec<QuestionAnswer>>,
    },
    /// Something went wrong.
    Error {
        /// What went wrong, for a person to read.
        message: String,
        /// Set when the CLI will try again on its own.
        retrying: bool,
        /// The account whose login ran out, when signing in again fixes
        /// this. An older `maxd` leaves it out, which reads as none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sign_in: Option<AccountId>,
    },
    /// The agent hit a usage limit.
    LimitWait {
        /// The account that hit it, if known.
        account: Option<AccountId>,
        /// When the limit resets, if the vendor said.
        resets_at: Option<Timestamp>,
        /// Set when the agent carries on by itself after the reset.
        auto_resume: bool,
    },
    /// The CLI shrank the conversation to fit the context window.
    Compaction {
        /// Tokens before, if the CLI said.
        #[specta(type = Option<specta_typescript::Number>)]
        before_tokens: Option<u64>,
        /// Tokens after, if the CLI said.
        #[specta(type = Option<specta_typescript::Number>)]
        after_tokens: Option<u64>,
    },
    /// You stopped the turn.
    Interrupted,
    /// The end of a turn.
    TurnSummary {
        /// How long the turn took, if the CLI said.
        #[specta(type = Option<specta_typescript::Number>)]
        duration_ms: Option<u64>,
        /// How many tokens the context held at the end, if the CLI said.
        #[specta(type = Option<specta_typescript::Number>)]
        context_tokens: Option<u64>,
    },
    /// A message from the CLI or `maxd` that isn't part of the conversation.
    Notice {
        /// How much it matters.
        level: NoticeLevel,
        /// What to show.
        message: String,
    },
    /// The agent moved to another vendor here, carrying the conversation
    /// over. Pending until the next message takes the conversation to the
    /// new vendor, and cancelled if the agent moves again first. A move
    /// between Claude and a local model is one too, since a local agent
    /// runs Claude Code with its own config folder.
    Handoff {
        /// The vendor the agent left.
        from_vendor: Vendor,
        /// The model it ran there, if it had one set.
        from_model: Option<String>,
        /// The vendor the agent moved to.
        to_vendor: Vendor,
        /// The model it runs there, if you picked one.
        to_model: Option<String>,
        /// Whether the agent left a local model, which runs as
        /// [`Vendor::Claude`]: show that end as Local. Since
        /// [`crate::message::LOCAL_MODELS_SINCE`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        from_local: bool,
        /// Whether the agent moved to a local model. Since
        /// [`crate::message::LOCAL_MODELS_SINCE`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        to_local: bool,
    },
    /// The agent started a fresh vendor session here, with the same vendor,
    /// model and permission, for a task's run. What comes before it stays
    /// in the timeline, but the agent no longer has it in its context. An
    /// older app shows nothing for it. Since protocol 64.
    NewSession,
    /// Files the agent sent you with MaxHQ's send tool, from copies MaxHQ
    /// keeps in its attachments folder.
    AgentAttachments {
        /// The message it sent with them, or empty.
        text: String,
        /// The files.
        attachments: Vec<AttachmentRef>,
    },
    /// A file the agent asked MaxHQ to show you with its open tool. The app
    /// opens it in its file viewer from the device, and never gets its
    /// contents through the timeline.
    AgentOpenedFile {
        /// Where the file is on the agent's device: absolute once `maxd`
        /// resolved it, and as the agent wrote it otherwise.
        path: String,
        /// The line to show, counting from 1, if the agent gave one.
        #[serde(default)]
        line: Option<u32>,
        /// Set when the file is outside the agent's folders, so the viewer
        /// asks before it shows it.
        #[serde(default)]
        outside: bool,
    },
    /// A web page the agent made with MaxHQ's show_visual tool, such as a
    /// chart or a diagram, from a copy MaxHQ keeps in its attachments
    /// folder. Since protocol 39.
    AgentVisual {
        /// What the page shows.
        title: String,
        /// The page.
        attachment: AttachmentRef,
        /// The earlier visual this one is a new version of, which the app
        /// folds under it. An older `maxd` leaves it out, which reads as
        /// none.
        #[serde(default)]
        replaces: Option<ItemId>,
    },
    /// A change the dispatcher made without asking, as a card on its own
    /// timeline. Since protocol 52.
    DispatcherAction {
        /// The device it changed.
        device: DeviceId,
        /// What it did.
        action: Box<crate::dispatch::AdminAction>,
        /// What it changed, by name as it was before, such as the agent's
        /// old title.
        target: String,
        /// What [`crate::message::ClientMessage::UndoDispatcherAction`]
        /// names to put it back, when it has an inverse.
        /// [`crate::dispatch::Dispatcher::undo`] says whether it still may.
        undo: Option<crate::ids::IdempotencyKey>,
    },
    /// An agent or a task the dispatcher started or saved without asking,
    /// as a card on its own timeline. An older app shows it as an item it
    /// doesn't know. Since protocol 56.
    DispatcherStep {
        /// What it did: a start or a task.
        step: Box<crate::dispatch::PlanStep>,
        /// How it went: the agent it started or the task it saved.
        outcome: crate::dispatch::PlanOutcome,
        /// What [`crate::message::ClientMessage::UndoDispatcherAction`]
        /// names to stop the agent or delete the task, while it may.
        undo: Option<crate::ids::IdempotencyKey>,
    },
    /// A command you ran with `!` in the composer, as a card. The command
    /// runs in a terminal of its own, and the card shows it live until it
    /// ends. Since protocol 45.
    UserRun {
        /// The terminal it runs in.
        terminal: TerminalId,
        /// The command line, as typed after the `!`.
        command: String,
        /// The folder it ran in, as the device writes it.
        cwd: String,
        /// The device it ran on.
        device: DeviceId,
        /// Whether you ran it with `!!`, so its output only goes to the
        /// agent when you attach it to a message.
        private: bool,
        /// Whether its process still runs, and how it ended. It isn't
        /// called `status`, which is the item's own: running while the
        /// command runs, then completed when it exits with 0 and failed
        /// otherwise.
        process: TerminalStatus,
        /// How long it ran, once it ended.
        #[specta(type = Option<specta_typescript::Number>)]
        duration_ms: Option<u64>,
        /// How many lines it wrote.
        lines: u32,
        /// What it wrote, with colors and styles as SGR escapes and no
        /// other escapes. A password you sent at a prompt shows as the
        /// prompt followed by ` 🔒 sent`, never as what you typed.
        output: String,
        /// Whether `output` holds only the last part of what it wrote.
        output_truncated: bool,
        /// Whether you moved it to the terminal drawer, where it goes on as
        /// a tab.
        moved: bool,
        /// Why it ran in a fresh shell rather than the agent's session.
        /// None from a `maxd` before protocol 48, which had no sessions.
        #[serde(default)]
        fresh: Option<FreshReason>,
    },
    /// The commands you ran with `!` in the agent's session between two of
    /// your messages, as one card with a block for each. The item runs
    /// while a block does, and is completed otherwise. Since protocol 48.
    UserSession {
        /// The session's terminal.
        terminal: TerminalId,
        /// The device it runs on.
        device: DeviceId,
        /// The commands, in the order they ran.
        blocks: Vec<SessionBlock>,
        /// Whether a message you sent closed the card, so its blocks can't
        /// change between shared and private and the next command starts
        /// a new card.
        sealed: bool,
        /// Whether the message that sealed the card took its session pill
        /// along. False while the card is open.
        sent: bool,
    },
    /// Something the driver doesn't know how to show yet.
    Unknown {
        /// The CLI's own name for it, for logs and bug reports.
        native_kind: String,
    },
}

/// How a message you sent reached the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// The agent has it.
    Sent,
    /// Waiting for the current turn to end.
    Queued,
    /// Went into the running turn.
    Steer,
    /// You took it back before it was sent.
    Cancelled,
}

/// The edit to one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FileEdit {
    /// The file, as the agent named it.
    pub path: String,
    /// Where a renamed file was before.
    pub old_path: Option<String>,
    /// What happened to the file.
    pub op: FileOp,
    /// Lines added.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// A unified diff, when the CLI gave one.
    pub patch: Option<String>,
}

/// What happened to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileOp {
    /// A new file.
    Add,
    /// Changed in place.
    Modify,
    /// Removed.
    Delete,
    /// Moved, and maybe changed too.
    Rename,
}

/// What a web item did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WebAction {
    /// Searched the web.
    Search,
    /// Fetched one page.
    Fetch,
}

/// What one of Claude Code's artifact tools did with a claude.ai page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ArtifactCall {
    /// What it did, as the tool's `action` names it, such as `publish`,
    /// `read`, `list`, `delete`, `reply` or `set`.
    pub action: String,
    /// The page's title, once the input or the result names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub title: Option<String>,
    /// The page's link. Set only for an https link on claude.ai.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub url: Option<String>,
    /// One more thing the call named, such as the file it read, the
    /// collection it changed or the kind of page it started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub detail: Option<String>,
    /// The page's version after the call, when the result says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub version: Option<String>,
    /// Whether the page is yours, when the result says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub owned: Option<bool>,
}

/// One entry in an agent's to-do list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Todo {
    /// What to do.
    pub text: String,
    /// How far along it is.
    pub state: TodoState,
}

/// How far along a to-do is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoState {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Finished.
    Done,
}

/// What an agent asks approval for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// To run a shell command.
    Command,
    /// To edit files.
    FileChange,
    /// To read a file.
    FileRead,
    /// To reach the network.
    Network,
    /// To use a tool.
    Tool,
    /// To start on a plan.
    Plan,
    /// Anything else.
    Other,
}

/// How an approval ended: the option picked, or typed text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Decided {
    /// The answer that won.
    pub answer: AttentionAnswer,
    /// The client that sent it, or none when you answered in the CLI itself.
    pub by: Option<ClientId>,
    /// When the answer arrived.
    pub at: Timestamp,
}

/// One question an agent asks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Question {
    /// Sent back in [`QuestionAnswer::id`].
    pub id: String,
    /// A short label above the question, if the agent gave one.
    pub header: Option<String>,
    /// The question.
    pub text: String,
    /// The answers you can pick, in order.
    pub options: Vec<QuestionOption>,
    /// Whether you can pick more than one.
    pub multi: bool,
    /// Whether you can type an answer of your own.
    pub other: bool,
    /// Whether to hide what you type, such as a password.
    pub secret: bool,
    /// What a card that asks for your consent shows beside the question,
    /// for a client that lays it out: what's asked for, the risk and each
    /// thing it covers. The text says the same in plain words. An older
    /// peer leaving it out reads as a plain question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub consent: Option<Consent>,
}

/// What a card that asks for your consent lays out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Consent {
    /// What's asked for, as the card's title.
    pub title: String,
    /// What could happen, and to what, in a sentence or two.
    pub risk: String,
    /// What changed since you last allowed it, one line each, or none the
    /// first time.
    pub changes: Vec<String>,
    /// The details to judge it by, such as the container an agent runs in,
    /// in order. A peer that leaves this out has none.
    #[serde(default)]
    pub facts: Vec<ConsentFact>,
    /// Each thing it covers, in order.
    pub rows: Vec<ConsentRow>,
    /// The box that keeps the answer, when it can be kept.
    pub remember: Option<ConsentRemember>,
    /// The kind of file an agent asks to change, when that's what the card
    /// asks, so a client can word the card in your language. An older
    /// peer leaving it out, or a kind this build can't read, reads as none,
    /// and the card shows the text above. Since protocol 57.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::tolerant::or_none"
    )]
    #[specta(type = Option<EditKind>)]
    pub edit: Option<EditKind>,
}

/// One labelled detail on a consent card, such as `Container: web`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ConsentFact {
    /// What it is, such as "Container".
    pub label: String,
    /// Its value.
    pub value: String,
}

/// One thing a consent card covers, such as a variable and its value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ConsentRow {
    /// The heading it sits under, such as the workflow that names it.
    pub group: String,
    /// Its name.
    pub name: String,
    /// Its value.
    pub value: String,
    /// Whether the value looks like a secret, so a client hides it until you
    /// ask to see it.
    pub secret: bool,
    /// What it can do beyond the plain, when it can do more.
    pub warning: Option<String>,
    /// Whether it replaces something MaxHQ set itself, so the warning is
    /// the strongest kind.
    pub strong: bool,
}

/// The box on a consent card that keeps your answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct ConsentRemember {
    /// Sent back as a [`QuestionAnswer::id`] with the box's label when it's
    /// ticked.
    pub id: String,
    /// Shown beside the box.
    pub label: String,
    /// The line shown once the box is ticked: what it keeps, and where in
    /// Settings to change it later.
    pub settings: String,
}

/// One answer you can pick for a question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct QuestionOption {
    /// Shown on the choice, and sent back in [`QuestionAnswer::values`].
    pub label: String,
    /// More about the choice, if the agent gave it.
    pub description: Option<String>,
}

/// Your answer to one question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct QuestionAnswer {
    /// The [`Question::id`] this answers.
    pub id: String,
    /// The labels you picked, or what you typed.
    pub values: Vec<String>,
}

/// How much a notice matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    /// For your information.
    Info,
    /// Worth a look.
    Warning,
}

/// One change to a timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
// Upserts are most events, so boxing the item would add an allocation to
// the common case to shrink the rare one.
#[allow(clippy::large_enum_variant)]
pub enum TimelineEvent {
    /// Adds the item, or replaces the one with its ID.
    Upsert {
        /// The whole item.
        item: TimelineItem,
    },
    /// Adds text to the end of one field, for streaming.
    Append {
        /// The item to add to.
        id: ItemId,
        /// The field to add to.
        field: TextField,
        /// The text to add.
        text: String,
    },
    /// Takes the item out, such as a queued message you cancelled.
    Remove {
        /// The item to take out.
        id: ItemId,
    },
}

/// `Text` is the message, reasoning or plan text. `Output` is a command's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextField {
    /// The `text` of a message or reasoning, or the `markdown` of a plan.
    Text,
    /// The `output` of a command.
    Output,
}

/// Which match a find across a timeline makes active.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FindAt {
    /// The match at this index among all of them, oldest first, held to
    /// the last one.
    Index {
        /// From 0.
        index: u32,
    },
    /// The match `offset` steps from the first one at or after `from`,
    /// wrapping round. A `from` item the timeline doesn't have counts
    /// from the first match.
    From {
        /// Where to count from.
        from: FindMatch,
        /// How many matches to step, back when negative.
        offset: i32,
    },
}

/// One match of a find, in the item that holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FindMatch {
    /// The item.
    pub item: ItemId,
    /// Which of the item's matches, counted from 0 in the order the row
    /// shows its text.
    pub occurrence: u32,
}

/// An item with matches, and where they sit among all of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct FindItem {
    /// The item.
    pub item: ItemId,
    /// The index of the item's first match among all of them.
    pub first: u32,
    /// How many matches the item has.
    pub count: u32,
}

/// What a find across a timeline found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct TimelineFind {
    /// How many matches the whole timeline has.
    pub total: u32,
    /// The active match's index among all of them. 0 when there are none.
    pub active: u32,
    /// The active match. None when there are no matches.
    pub found: Option<FindMatch>,
    /// Up to 17 items with matches around the active one's, in the order
    /// next steps through them: the active item 9th, wrapping round. All of
    /// them, oldest first, when 17 or fewer items match.
    pub near: Vec<FindItem>,
}

/// What a client gets for one open timeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineUpdate {
    /// The newest items, replacing whatever the client had. `seq` is the
    /// timeline's version after them.
    Reset {
        /// The timeline's version after these items.
        seq: Seq,
        /// Oldest first.
        items: Vec<TimelineItem>,
        /// Whether there are items before the first of these.
        has_older: bool,
    },
    /// Changes in order. `seq` is the version after the last one.
    Events {
        /// The timeline's version after the last event.
        seq: Seq,
        /// Applied in order. One this build can't read is left out.
        #[serde(deserialize_with = "crate::tolerant::list")]
        #[specta(type = Vec<TimelineEvent>)]
        events: Vec<TimelineEvent>,
    },
    /// Items before the oldest the client has, oldest first.
    Older {
        /// Oldest first.
        items: Vec<TimelineItem>,
        /// Whether there are items before the first of these.
        has_older: bool,
    },
}

/// A file or folder on the device, ready to send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AttachmentRef {
    /// Names the attachment in [`crate::message::UserInput::attachments`].
    pub id: AttachmentId,
    /// The file name to show.
    pub name: String,
    /// How the CLI takes it.
    pub kind: AttachmentKind,
    /// The size on disk. A folder counts every file inside it.
    #[specta(type = specta_typescript::Number)]
    pub bytes: u64,
    /// The media type, when `maxd` can tell.
    pub mime: Option<String>,
    /// Where `maxd` keeps it, as an absolute path on the device.
    pub path: String,
    /// A small preview of a picture as a `data:` URL, when `maxd` made one
    /// while it staged the file.
    pub thumbnail: Option<String>,
}

/// How a CLI takes an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// A picture the model can see.
    Image,
    /// A PDF the model can read.
    Pdf,
    /// Any other file, passed by path.
    File,
    /// A folder, passed by path.
    Folder,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(body: ItemBody) -> TimelineItem {
        TimelineItem {
            id: ItemId("claude:msg_01".into()),
            agent: "0199a7c2-0000-7000-8000-000000000001".parse().unwrap(),
            turn: Some(TurnId("3".into())),
            parent: None,
            ordinal: 12,
            status: ItemStatus::Completed,
            started_at: Some(Timestamp(1_761_350_400_000)),
            completed_at: None,
            body,
        }
    }

    #[test]
    fn the_body_kind_sits_beside_the_item_fields() {
        let command = item(ItemBody::Command {
            command: "cargo test".into(),
            cwd: Some("/work/maxhq".into()),
            exit_code: Some(0),
            output: "ok".into(),
            output_truncated: false,
            background: false,
        });
        let value = serde_json::to_value(&command).unwrap();
        assert_eq!(
            value,
            json!({
                "id": "claude:msg_01",
                "agent": "0199a7c2-0000-7000-8000-000000000001",
                "turn": "3",
                "parent": null,
                "ordinal": 12,
                "status": "completed",
                "started_at": 1_761_350_400_000u64,
                "completed_at": null,
                "kind": "command",
                "command": "cargo test",
                "cwd": "/work/maxhq",
                "exit_code": 0,
                "output": "ok",
                "output_truncated": false,
                "background": false,
            })
        );
        let parsed: TimelineItem = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, command);
    }

    #[test]
    fn every_body_survives_a_round_trip() {
        let attention: AttentionId = "0199a7c2-0000-7000-8000-000000000002".parse().unwrap();
        let attachment = AttachmentRef {
            id: "0199a7c2-0000-7000-8000-000000000003".parse().unwrap(),
            name: "shot.png".into(),
            kind: AttachmentKind::Image,
            bytes: 2048,
            mime: Some("image/png".into()),
            path: "/tmp/maxhq/shot.png".into(),
            thumbnail: Some("data:image/png;base64,iVBORw0KGgo=".into()),
        };
        let bodies = [
            ItemBody::UserMessage {
                text: "look at \u{FFFC}".into(),
                attachments: vec![attachment.clone()],
                refs: vec![InlineRef::Attachment { id: attachment.id }],
                delivery: Delivery::Queued,
            },
            ItemBody::AssistantMessage {
                text: "On it".into(),
                streaming: true,
            },
            ItemBody::Reasoning {
                text: "Hmm".into(),
                streaming: false,
            },
            ItemBody::FileChange {
                changes: vec![FileEdit {
                    path: "b.rs".into(),
                    old_path: Some("a.rs".into()),
                    op: FileOp::Rename,
                    additions: 1,
                    deletions: 2,
                    patch: None,
                }],
            },
            ItemBody::FileRead {
                path: "lib.rs".into(),
                lines: Some("1-20".into()),
                images: Vec::new(),
            },
            ItemBody::Search {
                query: "fn main".into(),
                scope: None,
                matches: Some(3),
            },
            ItemBody::Web {
                action: WebAction::Fetch,
                target: "https://example.com".into(),
            },
            ItemBody::ToolCall {
                server: Some("github".into()),
                name: "get_issue".into(),
                title: None,
                input: Some("{}".into()),
                output: None,
                images: Vec::new(),
                artifact: None,
            },
            ItemBody::Subagent {
                subagent: Some("0199a7c2-0000-7000-8000-000000000004".parse().unwrap()),
                agent_type: Some("Explore".into()),
                description: "Find the parser".into(),
                result: None,
            },
            ItemBody::Plan {
                markdown: "1. Read".into(),
                streaming: false,
            },
            ItemBody::Todos {
                todos: vec![Todo {
                    text: "Write tests".into(),
                    state: TodoState::InProgress,
                }],
            },
            ItemBody::Approval {
                attention,
                approval_kind: ApprovalKind::Command,
                title: "Run cargo test".into(),
                detail: None,
                cwd: Some("/work/maxhq".into()),
                options: vec![AttentionOption {
                    id: "allow".into(),
                    label: "Allow".into(),
                    kind: None,
                }],
                decided: Some(Decided {
                    answer: AttentionAnswer::Option { id: "allow".into() },
                    by: None,
                    at: Timestamp(1_761_350_400_000),
                }),
                saves: Vec::new(),
                deny_first: false,
                reason: None,
            },
            ItemBody::Question {
                attention,
                questions: vec![Question {
                    id: "q1".into(),
                    header: Some("Scope".into()),
                    text: "Which crate?".into(),
                    options: vec![QuestionOption {
                        label: "protocol".into(),
                        description: None,
                    }],
                    multi: false,
                    other: true,
                    secret: false,
                    consent: None,
                }],
                answers: Some(vec![QuestionAnswer {
                    id: "q1".into(),
                    values: vec!["protocol".into()],
                }]),
            },
            ItemBody::Error {
                message: "overloaded".into(),
                retrying: true,
                sign_in: None,
            },
            ItemBody::Error {
                message: "Failed to authenticate.".into(),
                retrying: false,
                sign_in: Some(AccountId::derive("example")),
            },
            ItemBody::LimitWait {
                account: None,
                resets_at: Some(Timestamp(1_761_350_400_000)),
                auto_resume: true,
            },
            ItemBody::Compaction {
                before_tokens: Some(180_000),
                after_tokens: None,
            },
            ItemBody::Interrupted,
            ItemBody::TurnSummary {
                duration_ms: Some(4_200),
                context_tokens: Some(52_000),
            },
            ItemBody::Notice {
                level: NoticeLevel::Warning,
                message: "Model changed".into(),
            },
            ItemBody::Handoff {
                from_vendor: Vendor::Claude,
                from_model: Some("opus".into()),
                to_vendor: Vendor::Codex,
                to_model: None,
                from_local: false,
                to_local: false,
            },
            ItemBody::Handoff {
                from_vendor: Vendor::Claude,
                from_model: Some("opus".into()),
                to_vendor: Vendor::Claude,
                to_model: Some("qwen3.6-35b-a3b@ud-q4_k_xl".into()),
                from_local: false,
                to_local: true,
            },
            ItemBody::NewSession,
            ItemBody::AgentAttachments {
                text: "Here's the plot".into(),
                attachments: vec![attachment.clone()],
            },
            ItemBody::AgentOpenedFile {
                path: "/tmp/example/notes.md".into(),
                line: Some(40),
                outside: true,
            },
            ItemBody::AgentVisual {
                title: "Weekly usage".into(),
                attachment: attachment.clone(),
                replaces: Some(ItemId("claude:toolu_1:visual".into())),
            },
            ItemBody::Unknown {
                native_kind: "hook_result".into(),
            },
        ];
        for body in bodies {
            let original = item(body);
            let text = serde_json::to_string(&original).unwrap();
            let parsed: TimelineItem = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed, original, "{text}");
        }
    }

    #[test]
    fn a_handoff_from_an_older_peer_reads_as_a_move_between_vendors() {
        let mut value = serde_json::to_value(item(ItemBody::Handoff {
            from_vendor: Vendor::Claude,
            from_model: None,
            to_vendor: Vendor::Codex,
            to_model: None,
            from_local: false,
            to_local: false,
        }))
        .unwrap();
        // A move between vendors writes neither flag, as an older peer.
        assert!(value.get("from_local").is_none(), "{value}");
        assert!(value.get("to_local").is_none(), "{value}");
        value["to_vendor"] = "claude".into();
        let parsed: TimelineItem = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.body,
            ItemBody::Handoff {
                from_local: false,
                to_local: false,
                ..
            }
        ));
    }

    #[test]
    fn an_opened_file_from_an_older_peer_reads_without_its_line_or_flag() {
        let mut value = serde_json::to_value(item(ItemBody::AgentOpenedFile {
            path: "/tmp/example/a.rs".into(),
            line: None,
            outside: false,
        }))
        .unwrap();
        value.as_object_mut().unwrap().remove("line");
        value.as_object_mut().unwrap().remove("outside");
        let parsed: TimelineItem = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.body,
            ItemBody::AgentOpenedFile {
                line: None,
                outside: false,
                ..
            }
        ));
    }

    #[test]
    fn a_visual_without_replaces_reads_as_a_first_version() {
        let attachment = AttachmentRef {
            id: AttachmentId::derive("visual"),
            name: "visual-weekly-usage.html".into(),
            kind: AttachmentKind::File,
            bytes: 1_024,
            mime: Some("text/html".into()),
            path: "/tmp/example/visual-weekly-usage.html".into(),
            thumbnail: None,
        };
        let mut value = serde_json::to_value(item(ItemBody::AgentVisual {
            title: "Weekly usage".into(),
            attachment,
            replaces: None,
        }))
        .unwrap();
        assert_eq!(value["kind"], "agent_visual");
        value.as_object_mut().unwrap().remove("replaces");
        let parsed: TimelineItem = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.body,
            ItemBody::AgentVisual { replaces: None, .. }
        ));
    }

    #[test]
    fn an_error_names_the_account_to_sign_in_only_when_there_is_one() {
        let plain = serde_json::to_value(item(ItemBody::Error {
            message: "overloaded".into(),
            retrying: false,
            sign_in: None,
        }))
        .unwrap();
        assert!(plain.get("sign_in").is_none(), "{plain}");
        let account = AccountId::derive("example");
        let marked = serde_json::to_value(item(ItemBody::Error {
            message: "Failed to authenticate.".into(),
            retrying: false,
            sign_in: Some(account),
        }))
        .unwrap();
        assert_eq!(marked["kind"], "error");
        assert_eq!(marked["sign_in"], serde_json::to_value(account).unwrap());
        // An older `maxd` sends none.
        let parsed: TimelineItem = serde_json::from_value(plain).unwrap();
        assert!(matches!(parsed.body, ItemBody::Error { sign_in: None, .. }));
    }

    #[test]
    fn a_unit_body_is_just_its_kind() {
        let value = serde_json::to_value(item(ItemBody::Interrupted)).unwrap();
        assert_eq!(value["kind"], "interrupted");
    }

    #[test]
    fn approvals_keep_their_own_kind_apart_from_the_tag() {
        let value = serde_json::to_value(item(ItemBody::Approval {
            attention: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            approval_kind: ApprovalKind::FileChange,
            title: "Edit lib.rs".into(),
            detail: None,
            cwd: None,
            options: Vec::new(),
            decided: None,
            saves: Vec::new(),
            deny_first: false,
            reason: None,
        }))
        .unwrap();
        assert_eq!(value["kind"], "approval");
        assert_eq!(value["approval_kind"], "file_change");
    }

    #[test]
    fn an_approval_from_an_older_peer_reads_without_its_folder() {
        let mut value = serde_json::to_value(item(ItemBody::Approval {
            attention: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            approval_kind: ApprovalKind::Command,
            title: "Run cargo test".into(),
            detail: None,
            cwd: Some("/work/maxhq".into()),
            options: Vec::new(),
            decided: None,
            saves: Vec::new(),
            deny_first: false,
            reason: None,
        }))
        .unwrap();
        value.as_object_mut().unwrap().remove("cwd");
        let parsed: TimelineItem = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(parsed.body, ItemBody::Approval { cwd: None, .. }));

        // A field from a newer peer is left alone too.
        value["seen_by"] = json!("someone");
        assert!(serde_json::from_value::<TimelineItem>(value).is_ok());
    }

    #[test]
    fn an_approval_from_an_older_peer_asks_as_before() {
        let mut value = serde_json::to_value(item(ItemBody::Approval {
            attention: "0199a7c2-0000-7000-8000-000000000002".parse().unwrap(),
            approval_kind: ApprovalKind::Network,
            title: "Permanently delete \"Old draft\"?".into(),
            detail: None,
            cwd: None,
            options: Vec::new(),
            decided: None,
            saves: Vec::new(),
            deny_first: true,
            reason: Some("Deleting can't be undone.".into()),
        }))
        .unwrap();
        assert_eq!(value["deny_first"], true);
        assert_eq!(value["reason"], "Deleting can't be undone.");
        let fields = value.as_object_mut().unwrap();
        fields.remove("deny_first");
        fields.remove("reason");
        let parsed: TimelineItem = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.body,
            ItemBody::Approval {
                deny_first: false,
                reason: None,
                ..
            }
        ));
    }

    #[test]
    fn a_claude_ai_page_call_round_trips_and_reads_as_none_from_an_older_peer() {
        let call = ArtifactCall {
            action: "publish".into(),
            title: Some("Weekly report".into()),
            url: Some("https://claude.ai/artifact/EXAMPLE".into()),
            detail: Some("scratch/report.html".into()),
            version: Some("2".into()),
            owned: Some(true),
        };
        let published = item(ItemBody::ToolCall {
            server: None,
            name: "Artifact".into(),
            title: Some("Published \"Weekly report\"".into()),
            input: Some(r#"{"file_path":"scratch/report.html"}"#.into()),
            output: None,
            images: Vec::new(),
            artifact: Some(Box::new(call)),
        });
        let mut value = serde_json::to_value(&published).unwrap();
        assert_eq!(
            value["artifact"],
            json!({
                "action": "publish",
                "title": "Weekly report",
                "url": "https://claude.ai/artifact/EXAMPLE",
                "detail": "scratch/report.html",
                "version": "2",
                "owned": true,
            })
        );
        let parsed: TimelineItem = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed, published);

        // A call that names only its action leaves the rest out.
        value["artifact"] = json!({"action": "list"});
        let parsed: TimelineItem = serde_json::from_value(value.clone()).unwrap();
        let ItemBody::ToolCall {
            artifact: Some(listed),
            ..
        } = parsed.body
        else {
            panic!("{parsed:?}");
        };
        assert_eq!(listed.action, "list");
        assert_eq!(
            (
                listed.title,
                listed.url,
                listed.detail,
                listed.version,
                listed.owned
            ),
            (None, None, None, None, None)
        );

        // An older peer's tool call has no page part.
        value.as_object_mut().unwrap().remove("artifact");
        let parsed: TimelineItem = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.body,
            ItemBody::ToolCall { artifact: None, .. }
        ));
    }

    #[test]
    fn a_message_from_an_older_peer_reads_without_refs() {
        let parsed: TimelineItem = serde_json::from_value(json!({
            "id": "claude:msg_01",
            "agent": "0199a7c2-0000-7000-8000-000000000001",
            "turn": null,
            "parent": null,
            "ordinal": 0,
            "status": "completed",
            "started_at": null,
            "completed_at": null,
            "kind": "user_message",
            "text": "hi",
            "attachments": [],
            "delivery": "sent",
        }))
        .unwrap();
        assert!(matches!(parsed.body, ItemBody::UserMessage { refs, .. } if refs.is_empty()));
    }

    #[test]
    fn a_consent_from_a_peer_without_facts_reads_as_none() {
        let parsed: Consent = serde_json::from_value(json!({
            "title": "Give this agent the workflow's variables?",
            "risk": "These go into this agent's environment.",
            "changes": [],
            "rows": [],
            "remember": null,
        }))
        .unwrap();
        assert!(parsed.facts.is_empty());
        assert_eq!(parsed.edit, None);
    }

    #[test]
    fn a_consent_names_the_kind_of_file_and_reads_a_newer_kind_as_none() {
        let consent = |edit: serde_json::Value| {
            serde_json::from_value::<Consent>(json!({
                "title": "Change git hooks?",
                "risk": "Git hooks run on your next commit, with your keys.",
                "changes": [],
                "rows": [],
                "remember": null,
                "edit": edit,
            }))
            .unwrap()
            .edit
        };
        assert_eq!(consent(json!("git_hooks")), Some(EditKind::GitHooks));
        assert_eq!(consent(json!("build_scripts")), None);
        assert_eq!(consent(json!(null)), None);
        // A card about anything else leaves the field out, as before.
        let plain = Consent {
            title: String::new(),
            risk: String::new(),
            changes: Vec::new(),
            facts: Vec::new(),
            rows: Vec::new(),
            remember: None,
            edit: None,
        };
        assert!(serde_json::to_value(&plain).unwrap().get("edit").is_none());
    }

    #[test]
    fn an_attachment_from_an_older_peer_reads_without_its_preview() {
        let parsed: AttachmentRef = serde_json::from_value(json!({
            "id": "0199a7c2-0000-7000-8000-000000000003",
            "name": "shot.png",
            "kind": "image",
            "bytes": 2048,
            "mime": "image/png",
            "path": "/tmp/maxhq/shot.png",
        }))
        .unwrap();
        assert_eq!(parsed.thumbnail, None);
    }

    #[test]
    fn timeline_updates_are_tagged_by_type() {
        let update = TimelineUpdate::Events {
            seq: Seq(4),
            events: vec![
                TimelineEvent::Append {
                    id: ItemId("codex:item_7".into()),
                    field: TextField::Output,
                    text: "compiling".into(),
                },
                TimelineEvent::Remove {
                    id: ItemId("maxhq:queued_1".into()),
                },
            ],
        };
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "events",
                "seq": 4,
                "events": [
                    { "type": "append", "id": "codex:item_7", "field": "output", "text": "compiling" },
                    { "type": "remove", "id": "maxhq:queued_1" },
                ],
            })
        );
        let parsed: TimelineUpdate = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, update);
    }
}
