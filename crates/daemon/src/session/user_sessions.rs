// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The commands you run in an agent's session, as `UserSession` cards in
//! its timeline.
//!
//! A `!` goes to the agent's session, the one shell its commands share,
//! through `crate::terminals`. Each command that goes in becomes a block
//! in the agent's open card for that shell. A card starts where the
//! timeline stood at its first command, and is kept with the `!` runs, in
//! their table, so a timeline read again gets it back in place.
//!
//! Your next message to the agent seals its open cards: their blocks stay
//! shared or private as they are, the next command starts a new card, and
//! each card notes whether the message took its pill along. Before any
//! message goes to an agent, each session pill in it keeps only the
//! blocks its card still shares, so a private block never reaches an
//! agent, whatever a client sends.

use std::collections::HashSet;

use maxhq_protocol::context::InlineRef;
use maxhq_protocol::ids::{AgentId, IdempotencyKey, TerminalId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::terminal::{FreshReason, SessionBlock, TerminalStatus};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineEvent, TimelineItem};
use tracing::warn;

use super::idem::Outcome;
use super::{refused, Manager};
use crate::db::UserRunRow;
use crate::terminals::access::Opener;
use crate::terminals::{BlockChange, SessionAsk, SessionRequest, SessionResult};

/// The refusal for a `!` with nothing after it.
const NO_COMMAND: &str = "Type a command after the !.";

/// The refusal for a share toggle on a block no card here has.
const NO_BLOCK: &str = "No session card here has that block.";

/// The refusal for a share toggle on a sealed card.
const SEALED: &str = "Your message sealed that card, so its blocks stay as they are.";

/// The refusal for a command that went into the session after its agent
/// ended.
const AGENT_ENDED: &str = "The agent ended before the session took that.";

/// The size of a session the folder chip opens, until a client sizes it.
const CHIP_SIZE: (u16, u16) = (80, 24);

/// What a session card's item ID starts with.
const CARD: &str = "session-";

/// The ID of the card whose first block is `block` in the session
/// `terminal`.
fn card_item(terminal: TerminalId, block: u32) -> ItemId {
    ItemId(format!("{CARD}{terminal}-{block}"))
}

/// Why `command` goes to a fresh shell whatever the session does, if it
/// must: you asked, it holds a line break or another control character
/// the shell would take as a key, or `exec` would take the shell's place.
pub(super) fn fresh_reason(command: &str, asked: bool) -> Option<FreshReason> {
    if asked {
        Some(FreshReason::Asked)
    } else if command.chars().any(char::is_control) {
        Some(FreshReason::Lines)
    } else if command.split_whitespace().next() == Some("exec") {
        Some(FreshReason::Exec)
    } else {
        None
    }
}

/// Whether a kept item is a card that a message hasn't sealed yet.
fn is_open(item: &TimelineItem) -> bool {
    matches!(item.body, ItemBody::UserSession { sealed: false, .. })
}

/// The session a card's blocks ran in.
fn card_terminal(item: &TimelineItem) -> Option<TerminalId> {
    match &item.body {
        ItemBody::UserSession { terminal, .. } => Some(*terminal),
        _ => None,
    }
}

impl Manager {
    /// Asks an agent's session to run a command you typed after `!`.
    pub(super) fn session_run(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        (command, private, asked): (String, bool, bool),
        (cols, rows): (u16, u16),
    ) -> Option<ActionOutcome> {
        if command.trim().is_empty() {
            return Some(refused(NO_COMMAND));
        }
        let fresh = fresh_reason(&command, asked);
        let ask = SessionAsk::Run {
            command,
            private,
            fresh,
        };
        self.ask_session(key, agent, ask, (cols, rows))
    }

    /// Asks an agent's session to move to a folder, or to open there.
    pub(super) fn session_cd(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        path: String,
    ) -> Option<ActionOutcome> {
        self.ask_session(key, agent, SessionAsk::Cd { path }, CHIP_SIZE)
    }

    /// Hands the terminals a request for an agent's session, for an agent
    /// whose terminals open here. Its answer comes as
    /// [`crate::terminals::RunNote::Session`].
    fn ask_session(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        ask: SessionAsk,
        (cols, rows): (u16, u16),
    ) -> Option<ActionOutcome> {
        let (opener, conn) = self.openers.remove(&key).unwrap_or((Opener::App, u64::MAX));
        let cwd = match self.terminal_folder(agent) {
            Ok(cwd) => cwd,
            Err(reason) => return Some(refused(reason)),
        };
        self.terminals.session(SessionRequest {
            key,
            agent,
            ask,
            cwd,
            cols,
            rows,
            opener,
            conn,
        });
        None
    }

    /// Takes in what came of a request to an agent's session.
    pub(super) fn session_note(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        result: SessionResult,
    ) {
        let outcome = match result {
            // The fresh shell's card says why, once it opens.
            SessionResult::Fresh(reason) => {
                self.fresh.insert(key, reason);
                return;
            }
            SessionResult::Refused(reason) => refused(reason),
            SessionResult::Opened(_) | SessionResult::Wrote { .. }
                if !self.held.contains_key(&agent) =>
            {
                self.terminals.end_agent(agent);
                refused(AGENT_ENDED)
            }
            SessionResult::Opened(terminal) => ActionOutcome::TerminalOpened {
                terminal: terminal.id,
            },
            SessionResult::Wrote {
                terminal,
                block,
                command,
                cwd,
                private,
                at,
            } => {
                let added = SessionBlock {
                    id: block,
                    command,
                    cwd,
                    cwd_after: None,
                    private,
                    process: TerminalStatus::Running,
                    started_at: at,
                    duration_ms: None,
                    lines: 0,
                    output: String::new(),
                    output_truncated: false,
                    secret_prompts: 0,
                    start: None,
                };
                let item = self.add_block(agent, terminal, added);
                ActionOutcome::SessionRan {
                    terminal,
                    item,
                    block,
                }
            }
        };
        self.finish(key, Outcome::Action(outcome));
    }

    /// Adds a block that went into a session to the agent's open card for
    /// it, or to a new card where the timeline stands. Returns the card.
    fn add_block(&mut self, agent: AgentId, terminal: TerminalId, block: SessionBlock) -> ItemId {
        let (id, at) = (block.id, block.started_at);
        let open = (self.open_cards(agent).into_iter())
            .find(|row| card_terminal(&row.record) == Some(terminal));
        let item = match open {
            Some(row) => {
                let item = ItemId(row.item.clone());
                self.change_kept(agent, &item, |card| {
                    if let ItemBody::UserSession { blocks, .. } = &mut card.body {
                        blocks.push(block);
                    }
                    settle(card);
                });
                item
            }
            None => {
                let card = TimelineItem {
                    id: card_item(terminal, id),
                    agent,
                    turn: None,
                    parent: None,
                    ordinal: 0,
                    status: ItemStatus::Running,
                    started_at: Some(at),
                    completed_at: None,
                    body: ItemBody::UserSession {
                        terminal,
                        device: self.device,
                        blocks: vec![block],
                        sealed: false,
                        sent: false,
                    },
                };
                let item = card.id.clone();
                self.add_kept(card, at);
                (self.open_cards.entry(agent).or_default()).push(item.clone());
                item
            }
        };
        self.blocks.insert((terminal, id), (agent, item.clone()));
        item
    }

    /// Takes in what happened to one of a session's blocks, in the card
    /// that holds it, sealed or not.
    pub(super) fn block_note(
        &mut self,
        agent: AgentId,
        terminal: TerminalId,
        block: u32,
        change: BlockChange,
    ) {
        let Some(item) = self.block_card(agent, terminal, block) else {
            return;
        };
        if matches!(change, BlockChange::Ended { .. }) {
            self.blocks.remove(&(terminal, block));
            if !self.session_runs(terminal) {
                self.blocks.retain(|(kept, _), _| *kept != terminal);
            }
        }
        self.change_kept(agent, &item, |card| {
            change_block(card, block, change);
            settle(card);
        });
    }

    /// The card that holds a session's block: the one it went into in
    /// this run of `maxd`, or the one kept with it.
    fn block_card(&self, agent: AgentId, terminal: TerminalId, block: u32) -> Option<ItemId> {
        if let Some((_, item)) = self.blocks.get(&(terminal, block)) {
            return Some(item.clone());
        }
        let prefix = format!("{CARD}{terminal}-");
        let cards = (self.db.user_runs_starting::<TimelineItem>(agent, &prefix))
            .inspect_err(|err| warn!(%err, %agent, "couldn't read session cards"))
            .ok()?;
        cards.into_iter().find_map(|row| match &row.record.body {
            ItemBody::UserSession { blocks, .. } if blocks.iter().any(|kept| kept.id == block) => {
                Some(ItemId(row.item))
            }
            _ => None,
        })
    }

    /// Shares a block of an open card with the agent again, or keeps it
    /// from it.
    pub(super) fn share_block(
        &mut self,
        agent: AgentId,
        item: &ItemId,
        block: u32,
        private: bool,
    ) -> ActionOutcome {
        let Some(row) = self.db.user_run::<TimelineItem>(agent, &item.0) else {
            return refused(NO_BLOCK);
        };
        let ItemBody::UserSession { blocks, sealed, .. } = &row.record.body else {
            return refused(NO_BLOCK);
        };
        if !blocks.iter().any(|kept| kept.id == block) {
            return refused(NO_BLOCK);
        }
        if *sealed {
            return refused(SEALED);
        }
        self.change_kept(agent, item, |card| {
            if let ItemBody::UserSession { blocks, .. } = &mut card.body
                && let Some(kept) = blocks.iter_mut().find(|kept| kept.id == block)
            {
                kept.private = private;
            }
        });
        ActionOutcome::Done
    }

    /// Seals the agent's open cards as a message you sent it goes, each
    /// noting whether the message took its pill along. With `every` false,
    /// only the cards whose pill it took.
    pub(super) fn seal_cards(&mut self, agent: AgentId, refs: &[InlineRef], every: bool) {
        let carried: HashSet<&ItemId> = (refs.iter())
            .filter_map(|made| match made {
                InlineRef::Session {
                    item: Some(item), ..
                } => Some(item),
                _ => None,
            })
            .collect();
        if !every && carried.is_empty() {
            return;
        }
        let mut still = Vec::new();
        let mut gone = Vec::new();
        for row in self.open_cards(agent) {
            let item = ItemId(row.item);
            let took = carried.contains(&item);
            if !every && !took {
                still.push(item);
                continue;
            }
            if card_terminal(&row.record).is_some_and(|terminal| !self.session_runs(terminal)) {
                gone.push(item.clone());
            }
            self.change_kept(agent, &item, |card| {
                if let ItemBody::UserSession { sealed, sent, .. } = &mut card.body {
                    *sealed = true;
                    *sent = took;
                }
            });
        }
        self.open_cards.insert(agent, still);
        // A sealed card whose session ended gets no more blocks. One that
        // ends late finds it kept.
        self.blocks.retain(|_, (_, item)| !gone.contains(item));
    }

    /// Lets go of what this `maxd` remembers of an agent's cards, as the
    /// agent goes.
    pub(super) fn forget_cards(&mut self, agent: AgentId) {
        self.open_cards.remove(&agent);
        self.blocks.retain(|_, (owner, _)| *owner != agent);
    }

    /// How many of an agent's open cards this `maxd` knows, once read, and
    /// how many of its blocks that run.
    #[cfg(test)]
    pub(super) fn remembered(&self, agent: AgentId) -> (Option<usize>, usize) {
        let open = self.open_cards.get(&agent).map(Vec::len);
        let blocks = (self.blocks.values())
            .filter(|(owner, _)| *owner == agent)
            .count();
        (open, blocks)
    }

    /// Whether a session's shell still runs, as the hub has it.
    fn session_runs(&self, terminal: TerminalId) -> bool {
        matches!(
            self.hub.get(&EntityKey::Terminal(terminal)),
            Some(Entity::Terminal(kept)) if kept.status == TerminalStatus::Running
        )
    }

    /// Leaves each session pill in `refs` only the blocks its card still
    /// shares, in its order. A pill whose card isn't here keeps none. The
    /// pill itself always stays, so its text keeps its place.
    pub(super) fn keep_shared(&self, refs: &mut [InlineRef]) {
        for made in refs {
            let InlineRef::Session { blocks, item, .. } = made else {
                continue;
            };
            let card = (item.as_ref()).and_then(|item| {
                (self.db.user_run_anywhere::<TimelineItem>(&item.0)).map(|(_, row)| row.record)
            });
            let shared: Vec<&SessionBlock> = match card.as_ref().map(|card| &card.body) {
                Some(ItemBody::UserSession { blocks, .. }) => {
                    blocks.iter().filter(|kept| !kept.private).collect()
                }
                _ => Vec::new(),
            };
            let mut from = 0;
            blocks.retain(|sent| {
                let next = (shared[from..].iter()).position(|kept| kept.command == sent.command);
                if let Some(at) = next {
                    from += at + 1;
                }
                next.is_some()
            });
        }
    }

    /// The agent's cards no message has sealed yet, oldest first.
    fn open_cards(&mut self, agent: AgentId) -> Vec<UserRunRow<TimelineItem>> {
        let rows = match self.open_cards.get(&agent) {
            Some(known) => (known.iter())
                .filter_map(|item| self.db.user_run::<TimelineItem>(agent, &item.0))
                .collect(),
            None => match self.db.user_runs_starting::<TimelineItem>(agent, CARD) {
                Ok(rows) => rows,
                Err(err) => {
                    warn!(%err, %agent, "couldn't read session cards");
                    Vec::new()
                }
            },
        };
        let open: Vec<_> = rows
            .into_iter()
            .filter(|row| is_open(&row.record))
            .collect();
        let known = open.iter().map(|row| ItemId(row.item.clone())).collect();
        self.open_cards.insert(agent, known);
        open
    }

    /// Adds a kept item, a `!` run or a session card that started at `at`,
    /// after the item that was newest in its agent's timeline, and keeps
    /// it.
    pub(super) fn add_kept(&mut self, item: TimelineItem, at: Timestamp) {
        let agent = item.agent;
        let timeline = self.timeline_mut(agent);
        let after = timeline.last_id();
        timeline.anchor(item.id.clone(), after.clone(), at);
        let after = after.map(|ItemId(id)| id);
        if let Err(err) = (self.db).put_user_run(agent, &item.id.0, after.as_deref(), at, &item) {
            warn!(%err, %agent, "couldn't keep a run");
        }
        self.timeline_events(vec![(agent, TimelineEvent::Upsert { item })]);
    }
}

/// Changes a card's block as `change` says.
fn change_block(card: &mut TimelineItem, block: u32, change: BlockChange) {
    let ItemBody::UserSession { blocks, .. } = &mut card.body else {
        return;
    };
    let Some(kept) = blocks.iter_mut().find(|kept| kept.id == block) else {
        return;
    };
    match change {
        BlockChange::Started { start, at, cwd } => {
            kept.start = Some(start);
            kept.started_at = at;
            if let Some(cwd) = cwd {
                kept.cwd = cwd;
            }
        }
        BlockChange::Ended {
            exit,
            at,
            cwd_after,
            output,
            secret_prompts,
        } => {
            kept.process = TerminalStatus::Exited {
                code: exit,
                signal: None,
            };
            kept.duration_ms = Some(at.0.saturating_sub(kept.started_at.0));
            kept.cwd_after = cwd_after;
            kept.lines = output.lines;
            kept.output = output.text;
            kept.output_truncated = output.truncated;
            kept.secret_prompts = secret_prompts;
        }
    }
}

/// Sets a card's status from its blocks: running while one runs, and
/// completed when the last one ended.
fn settle(card: &mut TimelineItem) {
    let ItemBody::UserSession { blocks, .. } = &card.body else {
        return;
    };
    let running = (blocks.iter()).any(|block| block.process == TerminalStatus::Running);
    let ended = (blocks.iter())
        .filter_map(|block| {
            block
                .duration_ms
                .map(|took| block.started_at.0.saturating_add(took))
        })
        .max();
    if running {
        card.status = ItemStatus::Running;
        card.completed_at = None;
    } else {
        card.status = ItemStatus::Completed;
        card.completed_at = ended.map(Timestamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_goes_fresh_when_asked_or_when_the_session_cant_take_it() {
        assert_eq!(fresh_reason("ls", true), Some(FreshReason::Asked));
        assert_eq!(
            fresh_reason("echo a\necho b", false),
            Some(FreshReason::Lines)
        );
        assert_eq!(
            fresh_reason("printf 'a\tb'", false),
            Some(FreshReason::Lines)
        );
        assert_eq!(fresh_reason("exec zsh", false), Some(FreshReason::Exec));
        assert_eq!(fresh_reason("  exec", false), Some(FreshReason::Exec));
        assert_eq!(fresh_reason("execute-this", false), None);
        assert_eq!(fresh_reason("echo exec", false), None);
        assert_eq!(fresh_reason("ls -la", false), None);
    }
}
