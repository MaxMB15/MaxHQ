// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading a session's transcript when the driver meets it already under
//! way, so it knows the older items. A long transcript takes a while, so
//! the read runs off the loop with the conversation it fills. Whatever
//! comes for the driver meanwhile waits, in order, and goes through once
//! the read is back.

use std::collections::HashSet;
use std::path::Path;

use maxhq_protocol::timeline::{ItemBody, ItemId, TimelineItem};
use serde_json::Value;

use super::{ClaudeSession, TITLE_CHARS};
use crate::drive::claude::convo::Convo;
use crate::drive::claude::history;
use crate::drive::claude::wire::{clip, text};
use crate::drive::{Action, ChildExit, Effects, HeldLine, Job, JobDone, ToolDone};
use crate::observe::claude::transcript::LastTurn;

/// Something that came for the driver while its seed was read.
#[derive(Debug)]
pub enum Later {
    /// A line from the CLI.
    Line(HeldLine),
    /// An action of yours.
    Act(Action),
    /// A call to MaxHQ's tools that finished.
    ToolDone(ToolDone),
    /// A connection to the holder, fresh or not, with the requests it
    /// still waits on.
    Connected(bool, Vec<HeldLine>),
    /// The CLI's end.
    Exited(ChildExit),
}

/// What a seed read, for the driver to take back.
struct Seeded {
    convo: Convo,
    last_turn: LastTurn,
    /// The items from the transcript that didn't go out live, or none
    /// when there's no transcript.
    older: Option<Vec<TimelineItem>>,
}

impl ClaudeSession {
    /// Starts reading the transcript of `session` off the loop. What comes
    /// meanwhile waits for it, starting with `line`, the first that named
    /// the session.
    pub(super) fn start_seed(&mut self, session: &str, line: &HeldLine, fx: &mut Effects) {
        let fresh = self.fresh_convo();
        let mut convo = std::mem::replace(&mut self.convo, fresh);
        let mut last_turn = std::mem::take(&mut self.last_turn);
        let config_dir = self.login.config_dir.clone();
        let session = session.to_owned();
        fx.jobs.push(Job::new(move || {
            let older = seed(&config_dir, &session, &mut convo, &mut last_turn);
            Box::new(Seeded {
                convo,
                last_turn,
                older,
            }) as JobDone
        }));
        self.seeding = Some(vec![Later::Line(line.clone())]);
    }

    /// Keeps what came while the seed is read, and says whether it did.
    pub(super) fn wait_for_seed(&mut self, later: impl FnOnce() -> Later) -> bool {
        match &mut self.seeding {
            Some(waiting) => {
                waiting.push(later());
                true
            }
            None => false,
        }
    }

    /// Takes back what the seed read, then hands on what came meanwhile.
    /// A read that failed took the conversation with it, so the driver goes
    /// on from what comes next.
    pub(super) fn seed_done(&mut self, done: Option<JobDone>, fx: &mut Effects) {
        let Some(waiting) = self.seeding.take() else {
            return;
        };
        match done.and_then(|done| done.downcast::<Seeded>().ok()) {
            Some(seeded) => {
                let Seeded {
                    convo,
                    last_turn,
                    older,
                } = *seeded;
                self.convo = convo;
                self.last_turn = last_turn;
                if let Some(older) = older {
                    self.seeded(older, fx);
                }
            }
            None => {
                tracing::warn!("reading a Claude transcript failed, so older items are missing")
            }
        }
        for later in waiting {
            use crate::drive::Driver as _;
            match later {
                Later::Line(line) => {
                    self.last_seq = Some(line.seq);
                    self.read(&line, fx);
                }
                Later::Act(action) => self.act(action, fx),
                Later::ToolDone(done) => self.tool_done(done, fx),
                Later::Connected(fresh, pending) => self.connected(fresh, &pending, fx),
                Later::Exited(exit) => self.exited(&exit, fx),
            }
        }
    }

    /// The seq of the first line waiting for the seed, which a new driver
    /// after a restart has to read again.
    pub(super) fn waiting_from(&self) -> Option<u64> {
        self.seeding.iter().flatten().find_map(|later| match later {
            Later::Line(line) => Some(line.seq),
            _ => None,
        })
    }

    /// Takes in the `older` items the seed found.
    fn seeded(&mut self, older: Vec<TimelineItem>, fx: &mut Effects) {
        self.join_tool_turns();
        if self.title.is_none() {
            let mut items = older.iter().chain(self.convo.book.items());
            self.title = items.find_map(|item| match &item.body {
                ItemBody::UserMessage { text, .. }
                    if item.agent == self.lead() && !text.trim().is_empty() =>
                {
                    Some(clip(text.trim(), TITLE_CHARS))
                }
                _ => None,
            });
        }
        if !older.is_empty() {
            fx.history.push((self.lead(), older));
        }
    }
}

/// Reads the transcript of `session` into `convo`, and the lead's last
/// reply into `last_turn`. Gives the items that didn't already go out
/// live, or none when there's no transcript.
fn seed(
    config_dir: &Path,
    session: &str,
    convo: &mut Convo,
    last_turn: &mut LastTurn,
) -> Option<Vec<TimelineItem>> {
    let known: HashSet<ItemId> = convo.book.items().map(|item| item.id.clone()).collect();
    convo.book.set_quiet(true);
    let lead = |record: &Value, line: &[u8]| {
        if text(record, "type") == Some("assistant") && text(record, "parent_tool_use_id").is_none()
        {
            last_turn.read_line(line, true);
        }
    };
    let older = history::seed(config_dir, session, convo, lead).map(|()| {
        convo
            .book
            .take_changed()
            .into_iter()
            .filter(|item| !known.contains(&item.id))
            .collect()
    });
    convo.book.set_quiet(false);
    convo.forget_settled();
    older
}
