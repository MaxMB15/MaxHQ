// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminals a device's `maxd` holds for an agent, since
//! [`crate::message::TERMINALS_SINCE`]: a shell in the agent's terminal
//! drawer, or a command you ran with `!` in the composer.
//!
//! The view carries each one as a [`Terminal`] entity. Its bytes go in
//! binary frames, kind 1 out and kind 2 in (see [`crate::frame`]), to the
//! clients that sent [`crate::ClientMessage::TerminalAttach`]. A command
//! you ran also leaves a [`crate::timeline::ItemBody::UserRun`] in the
//! agent's timeline.
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
    /// Whether it waits for a password at a prompt that hides what you
    /// type, so clients offer a secure field.
    pub password_prompt: bool,
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
    use crate::message::{ActionOutcome, ServerMessage, TERMINALS_SINCE};
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
                ClientMessage::TerminalInterrupt { key, terminal },
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
    }

    #[test]
    fn a_terminal_reads_back_with_its_key() {
        let entity = Entity::Terminal(Terminal {
            id: terminal(),
            agent: AGENT.parse().unwrap(),
            device: DEVICE.parse().unwrap(),
            kind: TerminalKind::Shell,
            moved: false,
            title: "zsh".into(),
            cwd: "~/code/example-app".into(),
            status: TerminalStatus::Running,
            password_prompt: false,
            started_at: Timestamp(1_793_368_920_000),
            cols: 120,
            rows: 32,
        });
        assert_eq!(
            round_trip(&entity),
            json!({
                "entity": "terminal",
                "id": TERMINAL,
                "agent": AGENT,
                "device": DEVICE,
                "kind": { "type": "shell" },
                "moved": false,
                "title": "zsh",
                "cwd": "~/code/example-app",
                "status": { "state": "running" },
                "password_prompt": false,
                "started_at": 1_793_368_920_000u64,
                "cols": 120,
                "rows": 32,
            })
        );
        assert_eq!(entity.key(), EntityKey::Terminal(terminal()));
        assert_eq!(
            round_trip(&entity.key()),
            json!({ "entity": "terminal", "id": TERMINAL })
        );
        assert_eq!(
            round_trip(&TerminalStatus::Exited {
                code: None,
                signal: Some(9),
            }),
            json!({ "state": "exited", "code": null, "signal": 9 })
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
                },
                duration_ms: Some(4_200),
                lines: 214,
                output: "\u{1b}[31mFAIL\u{1b}[0m src/cache.test.ts".into(),
                output_truncated: true,
                moved: false,
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
            })
        );
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
    }
}
