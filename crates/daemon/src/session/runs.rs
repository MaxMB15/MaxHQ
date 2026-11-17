// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Terminals for held agents, and the commands you run with `!` as items
//! in their timelines.
//!
//! The manager knows where an agent runs, so it opens its terminals, in its
//! folder, through `crate::terminals`. A `!` run adds a `UserRun` item where
//! the timeline stood when you ran it. The item shows nothing while the
//! command runs, since clients show the terminal's output live. When it
//! ends, the item gets how it ended, how long it took and what it wrote.
//! Runs are kept in the database with the item they came after, since the
//! vendor's history doesn't have them, so a timeline read again from it
//! gets them back in place.

use maxhq_hold::TermExit;
use maxhq_protocol::ids::{AgentId, IdempotencyKey, TerminalId};
use maxhq_protocol::message::{ActionOutcome, ClientMessage};
use maxhq_protocol::terminal::{Terminal, TerminalKind, TerminalStatus};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineEvent, TimelineItem};
use tracing::warn;

use super::idem::{Outcome, Seen};
use super::loading::Later;
use super::{refused, Manager};
use crate::terminals::access::Opener;
use crate::terminals::{OpenRequest, RunNote};

/// The refusal for an agent this device doesn't hold.
const NOT_HELD: &str = "Terminals open only for agents MaxHQ runs on this device.";

/// The refusal for a sub-agent, whose terminals are its lead's.
const SUB_AGENT: &str = "Open a terminal from the agent that started this one.";

/// The refusal for an agent in a container.
const IN_GUEST: &str = "Terminals don't open in containers yet. Open one for an agent that \
                        runs on this device.";

/// The refusal for an agent that ended before its terminal opened.
const AGENT_GONE: &str = "The agent ended before its terminal opened.";

/// The ID of a `!` run's item.
pub(super) fn run_item(terminal: TerminalId) -> ItemId {
    ItemId(format!("run-{terminal}"))
}

impl Manager {
    /// Takes a request to open a terminal from a connection the server let
    /// do so, as `opener`.
    pub(super) fn terminal_open(&mut self, conn: u64, opener: Opener, message: ClientMessage) {
        let ClientMessage::TerminalOpen { key, .. } = &message else {
            return;
        };
        let key = *key;
        match self.keys.begin(key, conn) {
            Seen::New => {}
            Seen::Running => return,
            Seen::Done(outcome) => {
                self.tell(conn, outcome.message(key));
                return;
            }
        }
        self.openers.insert(key, (opener, conn));
        if let Some(outcome) = self.action(key, message) {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// Opens a terminal in a held agent's folder. The outcome comes once it
    /// has, as [`RunNote::Opened`].
    pub(super) fn open_terminal(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        kind: TerminalKind,
        cols: u16,
        rows: u16,
    ) -> Option<ActionOutcome> {
        let (opener, conn) = self.openers.remove(&key).unwrap_or((Opener::App, u64::MAX));
        let Some(held) = self.held.get(&agent) else {
            let reason = if self.parents.contains_key(&agent) {
                SUB_AGENT
            } else {
                NOT_HELD
            };
            return Some(refused(reason));
        };
        if held.guest.is_some() {
            return Some(refused(IN_GUEST));
        }
        self.terminals.open(OpenRequest {
            key,
            agent,
            kind,
            cwd: held.cwd.clone(),
            cols,
            rows,
            opener,
            conn,
        });
        None
    }

    /// Takes in what the terminals said.
    pub(super) fn run_note(&mut self, note: RunNote) {
        match note {
            RunNote::Opened { key, agent, result } => {
                let outcome = match result {
                    Err(reason) => refused(reason),
                    Ok(terminal) if !self.held.contains_key(&agent) => {
                        self.terminals.end_agent(terminal.agent);
                        refused(AGENT_GONE)
                    }
                    Ok(terminal) => {
                        self.add_run(&terminal);
                        ActionOutcome::TerminalOpened {
                            terminal: terminal.id,
                        }
                    }
                };
                self.finish(key, Outcome::Action(outcome));
            }
            RunNote::Ended {
                agent,
                terminal,
                exit,
            } => {
                self.change_run(agent, terminal, |item| end_run(item, exit.as_ref()));
                self.terminals.release(terminal);
            }
            RunNote::Moved { agent, terminal } => {
                self.change_run(agent, terminal, |item| {
                    if let ItemBody::UserRun { moved, .. } = &mut item.body {
                        *moved = true;
                    }
                });
            }
        }
    }

    /// Adds a `!` run that started to its agent's timeline, after the item
    /// that was newest, and keeps it.
    fn add_run(&mut self, terminal: &Terminal) {
        let TerminalKind::Run { command, private } = &terminal.kind else {
            return;
        };
        let agent = terminal.agent;
        let item = TimelineItem {
            id: run_item(terminal.id),
            agent,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Running,
            started_at: Some(terminal.started_at),
            completed_at: None,
            body: ItemBody::UserRun {
                terminal: terminal.id,
                command: command.clone(),
                cwd: terminal.cwd.clone(),
                device: terminal.device,
                private: *private,
                process: TerminalStatus::Running,
                duration_ms: None,
                lines: 0,
                output: String::new(),
                output_truncated: false,
                moved: false,
            },
        };
        let timeline = self.timeline_mut(agent);
        let after = timeline.last_id();
        timeline.anchor(item.id.clone(), after.clone(), terminal.started_at);
        let after = after.map(|ItemId(id)| id);
        if let Err(err) = (self.db).put_user_run(
            agent,
            &item.id.0,
            after.as_deref(),
            terminal.started_at,
            &item,
        ) {
            warn!(%err, %agent, "couldn't keep a run");
        }
        self.timeline_events(vec![(agent, TimelineEvent::Upsert { item })]);
    }

    /// Changes a kept `!` run with `change`, keeps it, and shows it once
    /// its agent's timeline is in.
    fn change_run(
        &mut self,
        agent: AgentId,
        terminal: TerminalId,
        change: impl FnOnce(&mut TimelineItem),
    ) {
        let id = run_item(terminal);
        let Some(row) = self.db.user_run::<TimelineItem>(agent, &id.0) else {
            return;
        };
        let mut item = row.record;
        change(&mut item);
        if let Err(err) = (self.db).put_user_run(agent, &id.0, row.after.as_deref(), row.at, &item)
        {
            warn!(%err, %agent, "couldn't keep a run");
        }
        self.show_run(agent, &id);
    }

    /// Shows a `!` run as it was last kept, once its agent's timeline is
    /// in.
    pub(super) fn show_run(&mut self, agent: AgentId, id: &ItemId) {
        if !self.held.contains_key(&agent) {
            return;
        }
        if self.loading.waits(agent) {
            self.wait_for(agent, Later::Run(id.clone()));
            return;
        }
        let Some(row) = self.db.user_run::<TimelineItem>(agent, &id.0) else {
            return;
        };
        self.timeline_events(vec![(agent, TimelineEvent::Upsert { item: row.record })]);
    }
}

/// Marks a `!` run's item ended as `exit` says, or as ended without a word
/// when its holder went before it could say.
fn end_run(item: &mut TimelineItem, exit: Option<&TermExit>) {
    let ItemBody::UserRun {
        process,
        duration_ms,
        lines,
        output,
        output_truncated,
        ..
    } = &mut item.body
    else {
        return;
    };
    let code = exit.and_then(|exit| exit.code);
    *process = TerminalStatus::Exited {
        code,
        signal: exit.and_then(|exit| exit.signal),
    };
    let at = exit.map_or_else(Timestamp::now, |exit| exit.at);
    *duration_ms = item
        .started_at
        .map(|started| at.0.saturating_sub(started.0));
    if let Some(shown) = exit.and_then(|exit| exit.output.as_ref()) {
        *lines = shown.lines;
        output.clone_from(&shown.text);
        *output_truncated = shown.truncated;
    }
    item.status = if code == Some(0) {
        ItemStatus::Completed
    } else {
        ItemStatus::Failed
    };
    item.completed_at = Some(at);
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_hold::RunOutput;
    use maxhq_protocol::ids::DeviceId;

    fn running(started: u64) -> TimelineItem {
        TimelineItem {
            id: run_item(TerminalId::derive("t")),
            agent: AgentId::derive("a"),
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Running,
            started_at: Some(Timestamp(started)),
            completed_at: None,
            body: ItemBody::UserRun {
                terminal: TerminalId::derive("t"),
                command: "make".into(),
                cwd: "~/code/example-app".into(),
                device: DeviceId::derive("d"),
                private: false,
                process: TerminalStatus::Running,
                duration_ms: None,
                lines: 0,
                output: String::new(),
                output_truncated: false,
                moved: false,
            },
        }
    }

    #[test]
    fn a_run_that_exits_shows_how_long_it_took_and_what_it_wrote() {
        let mut item = running(1_000);
        let exit = TermExit {
            code: Some(2),
            signal: None,
            at: Timestamp(3_500),
            output: Some(RunOutput {
                text: "error\n".into(),
                lines: 120,
                truncated: true,
            }),
        };
        end_run(&mut item, Some(&exit));
        assert_eq!(item.status, ItemStatus::Failed);
        assert_eq!(item.completed_at, Some(Timestamp(3_500)));
        let ItemBody::UserRun {
            process,
            duration_ms,
            lines,
            output,
            output_truncated,
            ..
        } = item.body
        else {
            panic!("not a run");
        };
        assert_eq!(
            process,
            TerminalStatus::Exited {
                code: Some(2),
                signal: None
            }
        );
        assert_eq!(duration_ms, Some(2_500));
        assert_eq!(
            (lines, output.as_str(), output_truncated),
            (120, "error\n", true)
        );

        let mut item = running(1_000);
        end_run(
            &mut item,
            Some(&TermExit {
                code: Some(0),
                signal: None,
                at: Timestamp(1_000),
                output: None,
            }),
        );
        assert_eq!(item.status, ItemStatus::Completed);
    }

    #[test]
    fn a_run_whose_holder_went_ends_without_a_code() {
        let mut item = running(0);
        end_run(&mut item, None);
        assert_eq!(item.status, ItemStatus::Failed);
        let ItemBody::UserRun { process, .. } = item.body else {
            panic!("not a run");
        };
        assert_eq!(
            process,
            TerminalStatus::Exited {
                code: None,
                signal: None
            }
        );
    }
}
