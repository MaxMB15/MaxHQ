// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminals a device's `maxd` holds for an agent, since
//! [`crate::message::TERMINALS_SINCE`]: a shell in the agent's terminal
//! drawer, a command you ran with `!` in the composer, or, since
//! [`crate::message::SESSIONS_SINCE`], the agent's session, the shell its
//! `!` commands share.
//!
//! The view carries each one as a [`Terminal`] entity. Its bytes go in
//! binary frames, kind 1 out and kind 2 in (see [`crate::frame`]), to the
//! clients that sent [`crate::ClientMessage::TerminalAttach`]. A command
//! you ran in a terminal of its own also leaves a
//! [`crate::timeline::ItemBody::UserRun`] in the agent's timeline, and the
//! commands you ran in its session between two of your messages leave a
//! [`crate::timeline::ItemBody::UserSession`].
//!
//! Only the app and phones may send the terminal requests. A peer link and
//! an agent's own tools never may: what you type into a terminal is yours.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, DeviceId, TerminalId};
use crate::time::Timestamp;

/// What a terminal runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalKind {
    /// Your login shell, as a tab in the agent's terminal drawer.
    Shell,
    /// One command you ran with `!`, shown as a card in the timeline until
    /// you move it to the drawer.
    Run {
        /// The command line, as typed after the `!`.
        command: String,
        /// Whether you ran it with `!!`, so its output only goes to the
        /// agent when you attach it to a message.
        private: bool,
    },
    /// The agent's session: the one shell its `!` commands share, so `cd`
    /// and `export` carry over. A tab first in the drawer, titled
    /// "Session". An agent has one at most. Since protocol 48: an older
    /// client leaves the whole terminal out.
    Session,
}

/// Why a command you ran with `!` ran in a fresh shell rather than the
/// agent's session. Since protocol 48.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FreshReason {
    /// You asked for one, with ⌥↩.
    Asked,
    /// A command still ran in the session, or you had typed into it since
    /// its prompt.
    Busy,
    /// The command spans lines, which the session would run as separate
    /// commands, or holds another control character, such as a tab, that
    /// the shell would take as a key.
    Lines,
    /// The command starts with `exec`, which would take the session
    /// shell's place.
    Exec,
    /// Shell integration is off on the device, so its shells report no
    /// commands.
    IntegrationOff,
    /// The session's shell reported no commands: it isn't zsh, bash or
    /// fish, or it showed no marked prompt in time.
    NoMarks,
}

/// One command you ran in an agent's session, as a block in its card. It
/// has the fields of a `UserRun` card, so the app shows it the same way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct SessionBlock {
    /// The block's ID. A session counts its commands from 1 across all
    /// its cards, so no two of its blocks share one.
    pub id: u32,
    /// The command line, as typed after the `!`.
    pub command: String,
    /// The folder it started in, as the device writes it.
    pub cwd: String,
    /// The folder the shell was in once it ended, when the command moved
    /// it, as the device writes it.
    pub cwd_after: Option<String>,
    /// Whether it's kept from the agent: you ran it with `!!`, or switched
    /// it to private.
    pub private: bool,
    /// Whether it still runs, and its exit code once it ended. A session
    /// reports no signals, and a command it lost track of, such as one
    /// running when its shell ended, ends with no code.
    pub process: TerminalStatus,
    /// When it started.
    pub started_at: Timestamp,
    /// How long it ran, once it ended.
    #[specta(type = Option<specta_typescript::Number>)]
    pub duration_ms: Option<u64>,
    /// How many lines it wrote, once it ended.
    pub lines: u32,
    /// What it wrote, once it ended, styled as a `UserRun`'s output is. A
    /// password you sent at a prompt shows as the prompt followed by
    /// ` 🔒 sent`, never as what you typed.
    pub output: String,
    /// Whether `output` holds only the last part of what it wrote, or the
    /// terminal no longer held its start.
    pub output_truncated: bool,
    /// How many passwords you sent at prompts that hid what you typed.
    pub secret_prompts: u32,
    /// Where the shell's mark for the command's start begins, with its
    /// output right after, as an offset in all the session's terminal
    /// printed. A client follows the block live from there with
    /// [`crate::ClientMessage::TerminalAttach`], whose `after` is then one
    /// less. None until the shell marks the command's start.
    #[specta(type = Option<specta_typescript::Number>)]
    pub start: Option<u64>,
}

/// Whether a terminal's process still runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TerminalStatus {
    /// It runs.
    Running,
    /// It ended.
    Exited {
        /// The exit code, or none when a signal ended it.
        code: Option<i32>,
        /// The signal that ended it, if one did.
        signal: Option<i32>,
        /// Whether your Stop or Force stop ended it, rather than the
        /// program on its own. False from a `maxd` before
        /// [`crate::message::INPUT_MODES_SINCE`].
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stopped: bool,
    },
}

/// A terminal one device holds for one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Terminal {
    /// The terminal.
    pub id: TerminalId,
    /// The agent whose folder it runs in.
    pub agent: AgentId,
    /// The device whose `maxd` holds it.
    pub device: DeviceId,
    /// What it runs.
    pub kind: TerminalKind,
    /// Whether you moved this run from the timeline to the drawer, where it
    /// goes on as a tab. A shell is always a tab there, so it's false.
    pub moved: bool,
    /// The tab's name: what the shell set as the window title, or what you
    /// renamed it to.
    pub title: String,
    /// The folder it started in, as the device writes it, such as
    /// `~/code/example-app`.
    pub cwd: String,
    /// Whether it still runs.
    pub status: TerminalStatus,
    /// Whether a command runs in it: a program other than the shell holds
    /// its foreground, its shell marked a command that hasn't ended, or a
    /// `!` run hasn't ended. False while a shell waits at its prompt,
    /// whatever title it sets there, and when the device doesn't say.
    #[serde(default)]
    pub busy: bool,
    /// Whether the OSC 133 in its output are its shell's marks: its holder
    /// lets through only those with the terminal's nonce, in the plain
    /// form, and drops any other. False from a holder or device before
    /// protocol 47, whose output can carry any program's 133, so a client
    /// takes none as a mark.
    #[serde(default)]
    pub marks: bool,
    /// Whether it waits for a password at a prompt that hides what you
    /// type, so clients offer a secure field. Since
    /// [`crate::message::INPUT_MODES_SINCE`] also while a program reads keys
    /// one at a time and the cursor's line reads like it asks for a secret,
    /// such as `? Paste your token:`.
    pub password_prompt: bool,
    /// Whether a program that runs reads keys one at a time, as a picker or
    /// a prompt in raw mode does, so clients send each key as you press it.
    /// False at a secret prompt, which `password_prompt` says, and from a
    /// `maxd` before [`crate::message::INPUT_MODES_SINCE`].
    #[serde(default)]
    pub reads_keys: bool,
    /// Whether a program that runs shows the alternate screen, as a
    /// full-screen program does. False from a `maxd` before
    /// [`crate::message::INPUT_MODES_SINCE`].
    #[serde(default)]
    pub full_screen: bool,
    /// Whether the program asked for application cursor keys, so the arrow
    /// keys go as `ESC O A` rather than `ESC [ A`. False from a `maxd`
    /// before [`crate::message::INPUT_MODES_SINCE`].
    #[serde(default)]
    pub app_cursor: bool,
    /// Whether a program that runs has printed nothing for about a second
    /// and reads what you type, so it waits for you. False from a `maxd`
    /// before [`crate::message::INPUT_MODES_SINCE`].
    #[serde(default)]
    pub waiting: bool,
    /// Whether the device answers the queries a program sends its terminal,
    /// such as where the cursor is, so a client's view must not answer
    /// them too. False from a holder or `maxd` before
    /// [`crate::message::INPUT_MODES_SINCE`], where an open view answers.
    #[serde(default)]
    pub answers: bool,
    /// When it started.
    pub started_at: Timestamp,
    /// Its width in cells.
    pub cols: u16,
    /// Its height in cells.
    pub rows: u16,
}

/// A run of lines in what a terminal printed, counted from 1 with both ends
/// in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct LineSpan {
    /// The first line.
    pub first: u32,
    /// The last line.
    pub last: u32,
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::{json, Value};

    use super::*;
    use crate::ids::IdempotencyKey;
    use crate::message::{
        ActionOutcome, ServerMessage, INPUT_MODES_SINCE, SESSIONS_SINCE, TERMINALS_SINCE,
    };
    use crate::state::{Entity, EntityKey};
    use crate::timeline::{ItemBody, ItemId, ItemStatus, TimelineItem};
    use crate::ClientMessage;

    const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";
    const AGENT: &str = "0199a7c2-0000-7000-8000-000000000001";
    const DEVICE: &str = "0199a7c2-0000-7000-8000-000000000007";
    const TERMINAL: &str = "0199a7c2-0000-7000-8000-0000000000e1";

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: &T,
    ) -> Value {
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(&serde_json::from_value::<T>(json.clone()).unwrap(), value);
        json
    }

    fn terminal() -> TerminalId {
        TERMINAL.parse().unwrap()
    }

    #[test]
    fn requests_read_back_and_need_a_newer_maxd() {
        let key: IdempotencyKey = KEY.parse().unwrap();
        let agent: AgentId = AGENT.parse().unwrap();
        let terminal = terminal();
        let messages = [
            (
                ClientMessage::TerminalOpen {
                    key,
                    agent,
                    kind: TerminalKind::Shell,
                    cols: 120,
                    rows: 32,
                },
                json!({
                    "type": "terminal_open",
                    "key": KEY,
                    "agent": AGENT,
                    "kind": { "type": "shell" },
                    "cols": 120,
                    "rows": 32,
                }),
                true,
            ),
            (
                ClientMessage::TerminalOpen {
                    key,
                    agent,
                    kind: TerminalKind::Run {
                        command: "pnpm test".into(),
                        private: true,
                    },
                    cols: 80,
                    rows: 24,
                },
                json!({
                    "type": "terminal_open",
                    "key": KEY,
                    "agent": AGENT,
                    "kind": { "type": "run", "command": "pnpm test", "private": true },
                    "cols": 80,
                    "rows": 24,
                }),
                true,
            ),
            (
                ClientMessage::TerminalAttach {
                    terminal,
                    after: Some(4_096),
                },
                json!({ "type": "terminal_attach", "terminal": TERMINAL, "after": 4_096 }),
                false,
            ),
            (
                ClientMessage::TerminalAttach {
                    terminal,
                    after: None,
                },
                json!({ "type": "terminal_attach", "terminal": TERMINAL, "after": null }),
                false,
            ),
            (
                ClientMessage::TerminalDetach { terminal },
                json!({ "type": "terminal_detach", "terminal": TERMINAL }),
                false,
            ),
            (
                ClientMessage::TerminalResize {
                    terminal,
                    cols: 100,
                    rows: 30,
                },
                json!({ "type": "terminal_resize", "terminal": TERMINAL, "cols": 100, "rows": 30 }),
                false,
            ),
            (
                ClientMessage::TerminalInterrupt {
                    key,
                    terminal,
                    force: false,
                },
                json!({ "type": "terminal_interrupt", "key": KEY, "terminal": TERMINAL }),
                true,
            ),
            (
                ClientMessage::TerminalClose {
                    key,
                    terminal,
                    force: true,
                },
                json!({ "type": "terminal_close", "key": KEY, "terminal": TERMINAL, "force": true }),
                true,
            ),
            (
                ClientMessage::TerminalRename {
                    key,
                    terminal,
                    title: "dev server".into(),
                },
                json!({
                    "type": "terminal_rename",
                    "key": KEY,
                    "terminal": TERMINAL,
                    "title": "dev server",
                }),
                true,
            ),
            (
                ClientMessage::TerminalMove { key, terminal },
                json!({ "type": "terminal_move", "key": KEY, "terminal": TERMINAL }),
                true,
            ),
        ];
        assert_eq!(TERMINALS_SINCE, 45);
        for (message, wire, keyed) in messages {
            assert_eq!(round_trip(&message), wire);
            assert_eq!(message.since(), TERMINALS_SINCE);
            assert!(!message.reaches(Some(44)), "an older maxd never gets it");
            assert!(message.reaches(Some(45)));
            // Attach, detach and resize act on this connection alone, so a
            // resend needs no key.
            assert_eq!(message.action_key(), keyed.then_some(key), "{wire}");
        }

        // Force stop for a session command needs a maxd that knows it, so
        // an older one never takes it for a plain Ctrl-C.
        let force = ClientMessage::TerminalInterrupt {
            key,
            terminal,
            force: true,
        };
        assert_eq!(
            round_trip(&force),
            json!({ "type": "terminal_interrupt", "key": KEY, "terminal": TERMINAL, "force": true })
        );
        assert_eq!(INPUT_MODES_SINCE, 59);
        assert_eq!(force.since(), INPUT_MODES_SINCE);
        assert!(!force.reaches(Some(58)));
        assert!(force.reaches(Some(59)));
    }

    #[test]
    fn a_terminal_reads_back_with_its_key() {
        let entity = Entity::Terminal(Terminal {
            id: terminal(),
            agent: AGENT.parse().unwrap(),
            device: DEVICE.parse().unwrap(),
            kind: TerminalKind::Shell,
            moved: false,
            title: "pnpm dev".into(),
            cwd: "~/code/example-app".into(),
            status: TerminalStatus::Running,
            busy: true,
            marks: true,
            password_prompt: false,
            reads_keys: true,
            full_screen: false,
            app_cursor: true,
            waiting: true,
            answers: true,
            started_at: Timestamp(1_793_368_920_000),
            cols: 120,
            rows: 32,
        });
        let mut wire = json!({
            "entity": "terminal",
            "id": TERMINAL,
            "agent": AGENT,
            "device": DEVICE,
            "kind": { "type": "shell" },
            "moved": false,
            "title": "pnpm dev",
            "cwd": "~/code/example-app",
            "status": { "state": "running" },
            "busy": true,
            "marks": true,
            "password_prompt": false,
            "reads_keys": true,
            "full_screen": false,
            "app_cursor": true,
            "waiting": true,
            "answers": true,
            "started_at": 1_793_368_920_000u64,
            "cols": 120,
            "rows": 32,
        });
        assert_eq!(round_trip(&entity), wire);
        // A device that doesn't say whether it's busy, whether its holder
        // keeps to the shell's marks, or how the program reads input,
        // leaves each false, so a client's view answers queries itself.
        for field in [
            "busy",
            "marks",
            "reads_keys",
            "full_screen",
            "app_cursor",
            "waiting",
            "answers",
        ] {
            wire.as_object_mut().unwrap().remove(field);
        }
        assert!(matches!(
            serde_json::from_value(wire).unwrap(),
            Entity::Terminal(Terminal {
                busy: false,
                marks: false,
                reads_keys: false,
                full_screen: false,
                app_cursor: false,
                waiting: false,
                answers: false,
                ..
            })
        ));
        assert_eq!(entity.key(), EntityKey::Terminal(terminal()));
        assert_eq!(
            round_trip(&entity.key()),
            json!({ "entity": "terminal", "id": TERMINAL })
        );
        assert_eq!(
            round_trip(&TerminalStatus::Exited {
                code: None,
                signal: Some(9),
                stopped: false,
            }),
            json!({ "state": "exited", "code": null, "signal": 9 })
        );
        // Your Stop shows only when it ended the program.
        assert_eq!(
            round_trip(&TerminalStatus::Exited {
                code: None,
                signal: Some(1),
                stopped: true,
            }),
            json!({ "state": "exited", "code": null, "signal": 1, "stopped": true })
        );
    }

    #[test]
    fn a_run_reads_back_as_a_timeline_item() {
        let item = TimelineItem {
            id: ItemId("run:1".into()),
            agent: AGENT.parse().unwrap(),
            turn: None,
            parent: None,
            ordinal: 7,
            status: ItemStatus::Failed,
            started_at: Some(Timestamp(1_793_368_920_000)),
            completed_at: Some(Timestamp(1_793_368_924_200)),
            body: ItemBody::UserRun {
                terminal: terminal(),
                command: "pnpm test".into(),
                cwd: "~/code/example-app".into(),
                device: DEVICE.parse().unwrap(),
                private: false,
                process: TerminalStatus::Exited {
                    code: Some(1),
                    signal: None,
                    stopped: false,
                },
                duration_ms: Some(4_200),
                lines: 214,
                output: "\u{1b}[31mFAIL\u{1b}[0m src/cache.test.ts".into(),
                output_truncated: true,
                moved: false,
                fresh: Some(FreshReason::Busy),
            },
        };
        // The process's status sits beside the item's own, under a name of
        // its own.
        assert_eq!(
            round_trip(&item),
            json!({
                "id": "run:1",
                "agent": AGENT,
                "turn": null,
                "parent": null,
                "ordinal": 7,
                "status": "failed",
                "started_at": 1_793_368_920_000u64,
                "completed_at": 1_793_368_924_200u64,
                "kind": "user_run",
                "terminal": TERMINAL,
                "command": "pnpm test",
                "cwd": "~/code/example-app",
                "device": DEVICE,
                "private": false,
                "process": { "state": "exited", "code": 1, "signal": null },
                "duration_ms": 4_200,
                "lines": 214,
                "output": "\u{1b}[31mFAIL\u{1b}[0m src/cache.test.ts",
                "output_truncated": true,
                "moved": false,
                "fresh": "busy",
            })
        );
        // A run from before sessions says nothing of a fresh shell.
        let mut wire = serde_json::to_value(&item).unwrap();
        wire.as_object_mut().unwrap().remove("fresh");
        assert!(matches!(
            serde_json::from_value(wire).unwrap(),
            TimelineItem {
                body: ItemBody::UserRun { fresh: None, .. },
                ..
            }
        ));
    }

    #[test]
    fn session_requests_read_back_and_need_a_newer_maxd() {
        let key: IdempotencyKey = KEY.parse().unwrap();
        let agent: AgentId = AGENT.parse().unwrap();
        let messages = [
            (
                ClientMessage::SessionRun {
                    key,
                    agent,
                    command: "pnpm test".into(),
                    private: false,
                    fresh: true,
                    cols: 80,
                    rows: 24,
                },
                json!({
                    "type": "session_run",
                    "key": KEY,
                    "agent": AGENT,
                    "command": "pnpm test",
                    "private": false,
                    "fresh": true,
                    "cols": 80,
                    "rows": 24,
                }),
            ),
            (
                ClientMessage::SessionShare {
                    key,
                    agent,
                    item: ItemId("session:1".into()),
                    block: 3,
                    private: true,
                },
                json!({
                    "type": "session_share",
                    "key": KEY,
                    "agent": AGENT,
                    "item": "session:1",
                    "block": 3,
                    "private": true,
                }),
            ),
            (
                ClientMessage::SessionCd {
                    key,
                    agent,
                    path: "~/code/example-app/apps/web".into(),
                },
                json!({
                    "type": "session_cd",
                    "key": KEY,
                    "agent": AGENT,
                    "path": "~/code/example-app/apps/web",
                }),
            ),
            (
                ClientMessage::SessionEnd {
                    key,
                    agent,
                    force: false,
                },
                json!({ "type": "session_end", "key": KEY, "agent": AGENT, "force": false }),
            ),
            (
                ClientMessage::TerminalOpen {
                    key,
                    agent,
                    kind: TerminalKind::Session,
                    cols: 80,
                    rows: 24,
                },
                json!({
                    "type": "terminal_open",
                    "key": KEY,
                    "agent": AGENT,
                    "kind": { "type": "session" },
                    "cols": 80,
                    "rows": 24,
                }),
            ),
        ];
        assert_eq!(SESSIONS_SINCE, 48);
        for (message, wire) in messages {
            assert_eq!(round_trip(&message), wire);
            assert_eq!(message.since(), SESSIONS_SINCE, "{wire}");
            assert!(!message.reaches(Some(47)), "an older maxd never gets it");
            assert!(message.reaches(Some(48)));
            assert_eq!(message.action_key(), Some(key), "{wire}");
        }
    }

    #[test]
    fn a_session_card_reads_back_as_a_timeline_item() {
        let block = |id: u32, command: &str| SessionBlock {
            id,
            command: command.into(),
            cwd: "~/code/example-app".into(),
            cwd_after: None,
            private: false,
            process: TerminalStatus::Running,
            started_at: Timestamp(1_793_368_920_000),
            duration_ms: None,
            lines: 0,
            output: String::new(),
            output_truncated: false,
            secret_prompts: 0,
            start: None,
        };
        let item = TimelineItem {
            id: ItemId("session:1".into()),
            agent: AGENT.parse().unwrap(),
            turn: None,
            parent: None,
            ordinal: 7,
            status: ItemStatus::Running,
            started_at: Some(Timestamp(1_793_368_920_000)),
            completed_at: None,
            body: ItemBody::UserSession {
                terminal: terminal(),
                device: DEVICE.parse().unwrap(),
                blocks: vec![
                    SessionBlock {
                        cwd_after: Some("~/code/example-app/apps/web".into()),
                        process: TerminalStatus::Exited {
                            code: Some(0),
                            signal: None,
                            stopped: false,
                        },
                        duration_ms: Some(3),
                        start: Some(1_024),
                        ..block(1, "cd apps/web")
                    },
                    SessionBlock {
                        private: true,
                        lines: 1,
                        output: "[sudo] password for max: 🔒 sent".into(),
                        secret_prompts: 1,
                        start: Some(2_048),
                        ..block(2, "sudo systemctl restart nginx")
                    },
                ],
                sealed: false,
                sent: false,
            },
        };
        assert_eq!(
            round_trip(&item),
            json!({
                "id": "session:1",
                "agent": AGENT,
                "turn": null,
                "parent": null,
                "ordinal": 7,
                "status": "running",
                "started_at": 1_793_368_920_000u64,
                "completed_at": null,
                "kind": "user_session",
                "terminal": TERMINAL,
                "device": DEVICE,
                "blocks": [
                    {
                        "id": 1,
                        "command": "cd apps/web",
                        "cwd": "~/code/example-app",
                        "cwd_after": "~/code/example-app/apps/web",
                        "private": false,
                        "process": { "state": "exited", "code": 0, "signal": null },
                        "started_at": 1_793_368_920_000u64,
                        "duration_ms": 3,
                        "lines": 0,
                        "output": "",
                        "output_truncated": false,
                        "secret_prompts": 0,
                        "start": 1_024,
                    },
                    {
                        "id": 2,
                        "command": "sudo systemctl restart nginx",
                        "cwd": "~/code/example-app",
                        "cwd_after": null,
                        "private": true,
                        "process": { "state": "running" },
                        "started_at": 1_793_368_920_000u64,
                        "duration_ms": null,
                        "lines": 1,
                        "output": "[sudo] password for max: 🔒 sent",
                        "output_truncated": false,
                        "secret_prompts": 1,
                        "start": 2_048,
                    },
                ],
                "sealed": false,
                "sent": false,
            })
        );
        assert_eq!(
            round_trip(&TerminalKind::Session),
            json!({ "type": "session" })
        );
        let reasons = [
            (FreshReason::Asked, "asked"),
            (FreshReason::Busy, "busy"),
            (FreshReason::Lines, "lines"),
            (FreshReason::Exec, "exec"),
            (FreshReason::IntegrationOff, "integration_off"),
            (FreshReason::NoMarks, "no_marks"),
        ];
        for (reason, wire) in reasons {
            assert_eq!(round_trip(&reason), json!(wire));
        }
    }

    #[test]
    fn attaching_and_opening_answer_with_the_terminal() {
        assert_eq!(
            round_trip(&ServerMessage::TerminalAttached {
                terminal: terminal(),
                from: 8_192,
                reset: true,
            }),
            json!({ "type": "terminal_attached", "terminal": TERMINAL, "from": 8_192, "reset": true })
        );
        assert_eq!(
            round_trip(&ActionOutcome::TerminalOpened {
                terminal: terminal()
            }),
            json!({ "status": "terminal_opened", "terminal": TERMINAL })
        );
        assert_eq!(
            round_trip(&ActionOutcome::SessionRan {
                terminal: terminal(),
                item: ItemId("session:1".into()),
                block: 2,
            }),
            json!({
                "status": "session_ran",
                "terminal": TERMINAL,
                "item": "session:1",
                "block": 2,
            })
        );
    }
}
