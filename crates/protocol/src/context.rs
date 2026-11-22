// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Context a message carries inline, such as a quote or a pull request.
//!
//! A message's text holds U+FFFC, the object replacement character, where
//! each [`InlineRef::Attachment`], [`InlineRef::Quote`], [`InlineRef::Agent`],
//! [`InlineRef::Note`], [`InlineRef::Visual`], [`InlineRef::Output`] and
//! [`InlineRef::Session`] sits, one for one in the order of its refs. A [`InlineRef::GitItem`] has
//! no U+FFFC: the text keeps `#123` as typed, and the ref says which number
//! resolved.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, AttachmentId, DeviceId, NoteId, ProjectId, TerminalId};
use crate::model::Vendor;
use crate::terminal::LineSpan;
use crate::time::Timestamp;
use crate::timeline::ItemId;

/// What marks a placed ref's spot in a message's text.
pub const REF_MARK: char = '\u{FFFC}';

/// One piece of context in a message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InlineRef {
    /// A file from the message's attachments.
    Attachment {
        /// Names one of the message's attachments.
        id: AttachmentId,
    },
    /// Text quoted from a timeline.
    Quote {
        /// The agent whose timeline it's from.
        agent: AgentId,
        /// The agent's title when quoted, for the context entry.
        agent_title: String,
        /// The item it's from.
        item: ItemId,
        /// The quoted text.
        text: String,
        /// Where the text starts in the item's text as MaxHQ shows it, in
        /// UTF-16 units with each run of whitespace as one, which tells
        /// repeats of the text apart.
        position: u32,
        /// What kind of item it's from.
        source: QuoteSource,
        /// What produced it, such as "cargo test", for `Output`.
        from: Option<String>,
    },
    /// Another agent, read live through MaxHQ's read tool, or from a
    /// snapshot when the agent's device is out of the tool's reach.
    Agent {
        /// The agent.
        agent: AgentId,
        /// Its title when picked.
        title: String,
        /// Its CLI.
        vendor: Vendor,
        /// The device it runs on.
        device: DeviceId,
        /// The snapshot, staged like any attachment, when the tool can't
        /// reach the agent.
        excerpt: Option<AttachmentId>,
    },
    /// A project note, read live through MaxHQ's note tools when it's in
    /// the agent's own project, or from a copy when it's in another one,
    /// which those tools can't reach. Since protocol 28.
    Note {
        /// The project the note is in.
        project: ProjectId,
        /// The note.
        note: NoteId,
        /// Its title when picked.
        title: String,
        /// The note's text as it was when sent, staged like any attachment,
        /// when the note is in another project.
        copy: Option<AttachmentId>,
    },
    /// A web page an agent showed with MaxHQ's show_visual tool, pasted
    /// into a message and staged like any attachment. A client sends a
    /// `maxd` older than 39 a plain [`InlineRef::Attachment`] instead.
    /// Since protocol 39.
    Visual {
        /// Names the page among the message's attachments.
        attachment: AttachmentId,
        /// What the page shows.
        title: String,
        /// Where it came from, in free text, such as "Token audit on Mac
        /// mini".
        #[serde(default)]
        from: Option<String>,
    },
    /// What a command you ran with `!` printed, as plain text with no
    /// escapes, from the last lines only. A client sends it only to a
    /// `maxd` at [`crate::message::TERMINALS_SINCE`] or later. Since
    /// protocol 45.
    Output {
        /// The command line, as typed after the `!`.
        command: String,
        /// The name of the device it ran on, such as "Mac".
        device_name: String,
        /// The folder it ran in, as the device writes it.
        cwd: String,
        /// Its exit code, or none while it runs or when a signal ended it.
        exit_code: Option<i32>,
        /// The signal that ended it, if one did. None from a client before
        /// [`crate::message::INPUT_MODES_SINCE`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        /// Whether your Stop or Force stop ended it. False from a client
        /// before [`crate::message::INPUT_MODES_SINCE`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stopped: bool,
        /// Whether it still ran when the message went.
        running: bool,
        /// When it started.
        ran_at: Timestamp,
        /// How long it ran, or has run so far, if known.
        #[specta(type = Option<specta_typescript::Number>)]
        duration_ms: Option<u64>,
        /// How many lines it wrote in all, which may be more than `text`
        /// holds.
        lines_total: u32,
        /// Its last lines, as plain text, or the lines in `lines`.
        text: String,
        /// The terminal it came from, so the app can follow a command that
        /// still runs. Agents never see it, so it reads back as none.
        #[serde(default)]
        terminal: Option<TerminalId>,
        /// The lines `text` holds when you picked them in the drawer,
        /// counted in all the terminal printed. None means its last lines.
        #[serde(default)]
        lines: Option<LineSpan>,
    },
    /// The commands you ran in an agent's session, from one card: the
    /// blocks you shared, in order, each with its last lines as plain text.
    /// A client sends it only to a `maxd` at
    /// [`crate::message::SESSIONS_SINCE`] or later. Since protocol 48.
    Session {
        /// The name of the device the session runs on, such as "Mac".
        device_name: String,
        /// The shared blocks. A private one isn't here.
        blocks: Vec<SessionRefBlock>,
        /// The card it came from, so `maxd` marks the card as sent with
        /// the message. Agents never see it, so it reads back as none.
        #[serde(default)]
        item: Option<ItemId>,
    },
    /// A pull request or issue, checked when picked.
    GitItem {
        /// "owner/name".
        repo: String,
        /// The number after `#`.
        number: u32,
        /// A pull request or an issue.
        kind: GitItemKind,
        /// Its title.
        title: String,
        /// Its state when checked.
        state: GitItemState,
        /// Its page on GitHub.
        url: String,
        /// A PR's head branch.
        head: Option<String>,
        /// A PR's base branch.
        base: Option<String>,
        /// The first 2,000 characters of the body.
        body: String,
        /// When MaxHQ checked it.
        checked_at: Timestamp,
    },
}

impl InlineRef {
    /// Whether the ref has a U+FFFC in the text. A git item doesn't.
    pub fn is_placed(&self) -> bool {
        !matches!(self, Self::GitItem { .. })
    }
}

/// One command in an [`InlineRef::Session`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SessionRefBlock {
    /// The command line, as typed after the `!`.
    pub command: String,
    /// The folder it started in, as the device writes it.
    pub cwd: String,
    /// The folder the shell was in once it ended, when the command moved
    /// it.
    #[serde(default)]
    pub cwd_after: Option<String>,
    /// Its exit code, or none while it runs or when the session lost track
    /// of it.
    pub exit_code: Option<i32>,
    /// Whether your Stop or Force stop ended it. False from a client before
    /// [`crate::message::INPUT_MODES_SINCE`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// Whether it still ran when the message went.
    pub running: bool,
    /// How long it ran, or has run so far, if known.
    #[specta(type = Option<specta_typescript::Number>)]
    pub duration_ms: Option<u64>,
    /// How many lines it wrote in all, which may be more than `text` holds.
    pub lines_total: u32,
    /// Its last lines, as plain text.
    pub text: String,
}

/// What kind of item a quote is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuoteSource {
    /// An agent's reply.
    Reply,
    /// A message you sent.
    Request,
    /// A question the agent asked you.
    Question,
    /// What a tool or command printed.
    Output,
    /// Anything else.
    Other,
}

/// A pull request or an issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitItemKind {
    /// A pull request.
    PullRequest,
    /// An issue.
    Issue,
}

/// Where a pull request or issue stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitItemState {
    /// Open.
    Open,
    /// A pull request still in draft.
    Draft,
    /// A merged pull request.
    Merged,
    /// Closed without a merge, or an issue closed for another reason.
    Closed,
    /// An issue closed as done.
    Completed,
    /// An issue closed as not planned.
    NotPlanned,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent() -> AgentId {
        "0199a7c2-0000-7000-8000-000000000001".parse().unwrap()
    }

    fn all() -> Vec<InlineRef> {
        vec![
            InlineRef::Attachment {
                id: "0199a7c2-0000-7000-8000-000000000003".parse().unwrap(),
            },
            InlineRef::Quote {
                agent: agent(),
                agent_title: "api refactor".into(),
                item: ItemId("claude:msg_01".into()),
                text: "the cache never expires".into(),
                position: 120,
                source: QuoteSource::Output,
                from: Some("cargo test".into()),
            },
            InlineRef::Agent {
                agent: agent(),
                title: "api refactor".into(),
                vendor: Vendor::Codex,
                device: "0199a7c2-0000-7000-8000-000000000007".parse().unwrap(),
                excerpt: None,
            },
            InlineRef::Note {
                project: ProjectId::derive("project"),
                note: NoteId::derive("release"),
                title: "GTFS release steps".into(),
                copy: Some("0199a7c2-0000-7000-8000-000000000009".parse().unwrap()),
            },
            InlineRef::GitItem {
                repo: "owner/repo".into(),
                number: 123,
                kind: GitItemKind::PullRequest,
                title: "Fix cache expiry".into(),
                state: GitItemState::NotPlanned,
                url: "https://github.com/owner/repo/pull/123".into(),
                head: Some("fix/cache".into()),
                base: Some("main".into()),
                body: "Closes #45".into(),
                checked_at: Timestamp(1_761_350_400_000),
            },
            InlineRef::Visual {
                attachment: "0199a7c2-0000-7000-8000-00000000000a".parse().unwrap(),
                title: "Weekly usage".into(),
                from: Some("api refactor".into()),
            },
            InlineRef::Output {
                command: "pnpm test".into(),
                device_name: "Mac".into(),
                cwd: "~/code/example-app".into(),
                exit_code: None,
                signal: Some(1),
                stopped: true,
                running: false,
                ran_at: Timestamp(1_761_350_400_000),
                duration_ms: Some(4_200),
                lines_total: 214,
                text: "FAIL src/cache.test.ts\n1 failed".into(),
                terminal: None,
                lines: Some(LineSpan { first: 6, last: 8 }),
            },
            InlineRef::Session {
                device_name: "devbox".into(),
                blocks: vec![
                    SessionRefBlock {
                        command: "cd apps/web".into(),
                        cwd: "~/code/example-app".into(),
                        cwd_after: Some("~/code/example-app/apps/web".into()),
                        exit_code: Some(0),
                        stopped: false,
                        running: false,
                        duration_ms: Some(3),
                        lines_total: 0,
                        text: String::new(),
                    },
                    SessionRefBlock {
                        command: "pnpm test".into(),
                        cwd: "~/code/example-app/apps/web".into(),
                        cwd_after: None,
                        exit_code: None,
                        stopped: false,
                        running: true,
                        duration_ms: Some(9_100),
                        lines_total: 41,
                        text: "Tests: 1 failed, 37 passed".into(),
                    },
                ],
                item: Some(ItemId("session:1".into())),
            },
        ]
    }

    #[test]
    fn every_ref_survives_a_round_trip() {
        for one in all() {
            let text = serde_json::to_string(&one).unwrap();
            let parsed: InlineRef = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed, one, "{text}");
        }
    }

    #[test]
    fn refs_are_tagged_by_type_in_snake_case() {
        let values: Vec<_> = all()
            .iter()
            .map(|one| serde_json::to_value(one).unwrap())
            .collect();
        assert_eq!(
            values[0],
            json!({ "type": "attachment", "id": "0199a7c2-0000-7000-8000-000000000003" })
        );
        assert_eq!(values[1]["type"], "quote");
        assert_eq!(values[1]["agent_title"], "api refactor");
        assert_eq!(values[1]["source"], "output");
        assert_eq!(values[2]["vendor"], "codex");
        assert_eq!(values[2]["excerpt"], json!(null));
        assert_eq!(values[3]["type"], "note");
        assert_eq!(values[3]["title"], "GTFS release steps");
        assert_eq!(values[3]["copy"], "0199a7c2-0000-7000-8000-000000000009");
        assert_eq!(
            values[4],
            json!({
                "type": "git_item",
                "repo": "owner/repo",
                "number": 123,
                "kind": "pull_request",
                "title": "Fix cache expiry",
                "state": "not_planned",
                "url": "https://github.com/owner/repo/pull/123",
                "head": "fix/cache",
                "base": "main",
                "body": "Closes #45",
                "checked_at": 1_761_350_400_000u64,
            })
        );
        assert_eq!(
            values[6],
            json!({
                "type": "output",
                "command": "pnpm test",
                "device_name": "Mac",
                "cwd": "~/code/example-app",
                "exit_code": null,
                "signal": 1,
                "stopped": true,
                "running": false,
                "ran_at": 1_761_350_400_000u64,
                "duration_ms": 4_200,
                "lines_total": 214,
                "text": "FAIL src/cache.test.ts\n1 failed",
                "terminal": null,
                "lines": { "first": 6, "last": 8 },
            })
        );
        assert_eq!(
            values[7],
            json!({
                "type": "session",
                "device_name": "devbox",
                "blocks": [
                    {
                        "command": "cd apps/web",
                        "cwd": "~/code/example-app",
                        "cwd_after": "~/code/example-app/apps/web",
                        "exit_code": 0,
                        "running": false,
                        "duration_ms": 3,
                        "lines_total": 0,
                        "text": "",
                    },
                    {
                        "command": "pnpm test",
                        "cwd": "~/code/example-app/apps/web",
                        "cwd_after": null,
                        "exit_code": null,
                        "running": true,
                        "duration_ms": 9_100,
                        "lines_total": 41,
                        "text": "Tests: 1 failed, 37 passed",
                    },
                ],
                "item": "session:1",
            })
        );
    }

    #[test]
    fn a_visual_from_the_same_agent_reads_without_from() {
        let parsed: InlineRef = serde_json::from_value(json!({
            "type": "visual",
            "attachment": "0199a7c2-0000-7000-8000-00000000000a",
            "title": "Weekly usage",
        }))
        .unwrap();
        assert!(matches!(parsed, InlineRef::Visual { from: None, .. }));
    }

    #[test]
    fn a_session_read_back_from_an_agent_names_no_card() {
        let parsed: InlineRef = serde_json::from_value(json!({
            "type": "session",
            "device_name": "devbox",
            "blocks": [{
                "command": "ls",
                "cwd": "~",
                "exit_code": 0,
                "running": false,
                "duration_ms": null,
                "lines_total": 1,
                "text": "README.md",
            }],
        }))
        .unwrap();
        let InlineRef::Session { blocks, item, .. } = parsed else {
            panic!("not a session: {parsed:?}");
        };
        assert_eq!(item, None);
        assert_eq!(blocks[0].cwd_after, None);
        assert_eq!(
            crate::message::refs_since(&[InlineRef::Session {
                device_name: "devbox".into(),
                blocks,
                item,
            }]),
            crate::message::SESSIONS_SINCE
        );
    }

    #[test]
    fn only_git_items_sit_outside_the_text() {
        let placed: Vec<_> = all().iter().map(InlineRef::is_placed).collect();
        assert_eq!(placed, [true, true, true, true, false, true, true, true]);
    }
}
