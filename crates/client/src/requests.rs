// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a link has asked its device and still waits on: actions and answers
//! with no result yet, the timelines and workflow runs it has open, and the pages of older
//! items asked for and not yet come. The handle adds to it, the task settles
//! it as results come in, and after each reconnect the task sends it all
//! again.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

use maxhq_protocol::attention::AnswerOutcome;
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::Seq;
use maxhq_protocol::timeline::TimelineUpdate;
use maxhq_protocol::workflow::RunKey;
use maxhq_protocol::ClientMessage;
use tokio::sync::{mpsc, oneshot};

use crate::LinkError;

/// One link's book of requests, shared by its handle and its task.
#[derive(Default)]
pub(crate) struct Book {
    /// What the book holds.
    requests: Mutex<Requests>,
    /// Held while pages of older items go out, so they go out on each
    /// connection in the order the book holds them.
    pages: tokio::sync::Mutex<()>,
}

impl Book {
    /// Locks the book. Every change under the lock is made whole, so a panic
    /// elsewhere can't leave one half-written.
    pub fn lock(&self) -> MutexGuard<'_, Requests> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for the turn to send pages of older items. The task holds it
    /// from opening a connection until its replay is queued, and the handle
    /// while it asks for a page, so the device answers the pages of each
    /// connection in the order the book expects them.
    pub async fn pages(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.pages.lock().await
    }
}

/// What the book holds.
#[derive(Default)]
pub(crate) struct Requests {
    /// Actions and answers sent with no result yet, by key.
    pending: BTreeMap<IdempotencyKey, Pending>,
    /// The timelines open on this link.
    timelines: BTreeMap<AgentId, Timeline>,
    /// The workflow runs open on this link, with their opens not yet
    /// matched by a close.
    runs: HashMap<RunKey, usize>,
    /// Counts the link's connections, so a page asked for on one that
    /// dropped goes again on the next.
    session: u64,
    /// Pages of older items still to come on this connection for a
    /// timeline that closed meanwhile, by agent. The device answers every
    /// page, so these come before any asked for after a reopen.
    strays: BTreeMap<AgentId, usize>,
    /// Set once the task ends, after which nothing new is taken.
    closed: bool,
}

/// One request waiting for its result.
struct Pending {
    /// The request, sent again as it is after a reconnect.
    message: ClientMessage,
    /// Where the result goes.
    reply: Reply,
}

/// Where a result goes, by the kind of result the request gets.
pub(crate) enum Reply {
    /// An action, which gets an `ActionResult`.
    Action(oneshot::Sender<ActionOutcome>),
    /// An answer, which gets an `AnswerResult`.
    Answer(oneshot::Sender<AnswerOutcome>),
    /// A proof, enrollment or removal sent while a connection proves
    /// itself. It gets an `ActionResult`, or two when the first says the
    /// device asked for a yes given somewhere else. It names its connection,
    /// so it never goes again on another.
    Proof(mpsc::UnboundedSender<ActionOutcome>),
}

/// One open timeline.
struct Timeline {
    /// Opens not yet matched by a close.
    opens: usize,
    /// How many of the newest items a reset holds.
    limit: u32,
    /// The timeline's version after the last update, so a reconnect picks
    /// up after it. None until the first reset.
    seq: Option<Seq>,
    /// Pages of older items asked for and not yet come, oldest first.
    older: Vec<Older>,
    /// Opens on this connection that `maxd` hasn't answered yet. It answers
    /// each with a reset or the events since, which hold what came before
    /// rather than something that just happened.
    owed: usize,
}

/// One page of older items asked for.
struct Older {
    /// The connection it went out on.
    session: u64,
    /// The ordinal the page ends before.
    before: u64,
    /// How many items it holds at most.
    limit: u32,
}

impl Requests {
    /// Starts waiting for a request's result. Refused once the link closed.
    pub fn wait(
        &mut self,
        key: IdempotencyKey,
        message: ClientMessage,
        reply: Reply,
    ) -> Result<(), LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        self.pending.insert(key, Pending { message, reply });
        Ok(())
    }

    /// Stops waiting for a request, such as one that timed out.
    pub fn forget(&mut self, key: IdempotencyKey) {
        self.pending.remove(&key);
    }

    /// Hands an action its result, and says whether one was waiting. A
    /// repeat finds nothing, so each action settles once.
    pub fn settle_action(&mut self, key: IdempotencyKey, outcome: ActionOutcome) -> bool {
        match self.pending.remove(&key) {
            Some(Pending {
                reply: Reply::Action(reply),
                ..
            }) => {
                // The handle may have stopped waiting, which is fine.
                let _ = reply.send(outcome);
                true
            }
            Some(Pending {
                message,
                reply: Reply::Proof(reply),
            }) => {
                // An `Asked` says the real answer comes later on the same
                // key, so the proof keeps waiting for it.
                let asked = outcome == ActionOutcome::Asked;
                let _ = reply.send(outcome);
                if asked {
                    self.pending.insert(
                        key,
                        Pending {
                            message,
                            reply: Reply::Proof(reply),
                        },
                    );
                }
                true
            }
            Some(other) => {
                self.pending.insert(key, other);
                false
            }
            None => false,
        }
    }

    /// Stops waiting for each proof once its connection ends, so none goes
    /// again on the next. A prover still waiting finds its connection gone.
    pub fn end_proofs(&mut self) {
        self.pending
            .retain(|_, pending| !matches!(pending.reply, Reply::Proof(_)));
    }

    /// Hands an answer its result, and says whether one was waiting.
    pub fn settle_answer(&mut self, key: IdempotencyKey, outcome: AnswerOutcome) -> bool {
        match self.pending.remove(&key) {
            Some(Pending {
                reply: Reply::Answer(reply),
                ..
            }) => {
                let _ = reply.send(outcome);
                true
            }
            Some(other) => {
                self.pending.insert(key, other);
                false
            }
            None => false,
        }
    }

    /// Notes one more open of an agent's timeline, which asks for what came
    /// after `since`, or for the newest items with none. A reconnect before
    /// the answer arrives asks the same again, rather than for what came
    /// after a version the open replaced.
    pub fn open(
        &mut self,
        agent: AgentId,
        since: Option<Seq>,
        limit: u32,
    ) -> Result<(), LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        let timeline = self.timelines.entry(agent).or_insert(Timeline {
            opens: 0,
            limit,
            seq: None,
            older: Vec::new(),
            owed: 0,
        });
        timeline.opens += 1;
        timeline.owed += 1;
        timeline.limit = limit;
        timeline.seq = since;
        Ok(())
    }

    /// Notes one close of an agent's timeline, and says whether it was the
    /// last, so the device should stop sending updates.
    pub fn close(&mut self, agent: AgentId) -> bool {
        let Some(timeline) = self.timelines.get_mut(&agent) else {
            return false;
        };
        timeline.opens -= 1;
        if timeline.opens > 0 {
            return false;
        }
        let session = self.session;
        let strays = timeline
            .older
            .iter()
            .filter(|older| older.session == session)
            .count();
        if strays > 0 {
            *self.strays.entry(agent).or_default() += strays;
        }
        self.timelines.remove(&agent);
        true
    }

    /// Notes one more open of a workflow run.
    pub fn open_run(&mut self, run: RunKey) -> Result<(), LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        *self.runs.entry(run).or_default() += 1;
        Ok(())
    }

    /// Notes one close of a workflow run, and says whether it was the last,
    /// so the device should stop sending updates.
    pub fn close_run(&mut self, run: &RunKey) -> bool {
        let Some(opens) = self.runs.get_mut(run) else {
            return false;
        };
        *opens -= 1;
        if *opens > 0 {
            return false;
        }
        self.runs.remove(run);
        true
    }

    /// Whether a workflow run is open, so its updates pass on. Updates for
    /// a closed one are stragglers sent before the close arrived.
    pub fn saw_run(&self, run: &RunKey) -> bool {
        self.runs.contains_key(run)
    }

    /// Notes a page of older items asked for on the current connection,
    /// and says whether to send it. A timeline that isn't open has nowhere
    /// for its page to go, so it isn't sent.
    pub fn ask_older(
        &mut self,
        agent: AgentId,
        before: u64,
        limit: u32,
    ) -> Result<bool, LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        let Some(timeline) = self.timelines.get_mut(&agent) else {
            return Ok(false);
        };
        timeline.older.push(Older {
            session: self.session,
            before,
            limit,
        });
        Ok(true)
    }

    /// Notes the version an update brings an open timeline to, and says
    /// whether to pass it on, and if so whether it's live. Updates for a
    /// closed timeline are stragglers sent before the close arrived. A page
    /// of older items settles one asked for on this connection, and one
    /// nobody waits for is a straggler too, as is one asked for before its
    /// timeline closed. Only events that come after the answer to every
    /// open are live: something that just happened on the device.
    pub fn saw(&mut self, agent: AgentId, update: &TimelineUpdate) -> Option<bool> {
        if matches!(update, TimelineUpdate::Older { .. })
            && let Some(strays) = self.strays.get_mut(&agent)
        {
            *strays -= 1;
            if *strays == 0 {
                self.strays.remove(&agent);
            }
            return None;
        }
        let timeline = self.timelines.get_mut(&agent)?;
        match update {
            TimelineUpdate::Reset { seq, .. } => {
                timeline.seq = Some(*seq);
                timeline.owed = timeline.owed.saturating_sub(1);
                Some(false)
            }
            TimelineUpdate::Events { seq, .. } => {
                timeline.seq = Some(*seq);
                let live = timeline.owed == 0;
                timeline.owed = timeline.owed.saturating_sub(1);
                Some(live)
            }
            TimelineUpdate::Older { .. } => {
                let session = self.session;
                match timeline
                    .older
                    .iter()
                    .position(|older| older.session == session)
                {
                    Some(at) => {
                        timeline.older.remove(at);
                        Some(false)
                    }
                    None => None,
                }
            }
        }
    }

    /// Starts counting a new connection. Call it with the pages lock held,
    /// before the handle can send on the connection, and replay after.
    pub fn begin(&mut self) {
        self.session += 1;
        // What the last connection still owed never comes.
        self.strays.clear();
    }

    /// What a new connection sends again: each open timeline and workflow
    /// run, then each
    /// page of older items a dropped connection never brought, then each
    /// request still waiting. `resume` says the connection reached the same
    /// `maxd` run, so a timeline can pick up after its last version. A page
    /// asked for on this connection already went out, so it isn't sent
    /// twice.
    pub fn replay(&mut self, resume: bool) -> Vec<ClientMessage> {
        let session = self.session;
        // The last connection's answers never come, and each open goes
        // once on this one.
        for timeline in self.timelines.values_mut() {
            timeline.owed = 1;
        }
        let opens = self
            .timelines
            .iter()
            .map(|(agent, timeline)| ClientMessage::OpenTimeline {
                agent: *agent,
                since: timeline.seq.filter(|_| resume),
                limit: timeline.limit,
            });
        let mut messages: Vec<ClientMessage> = opens.collect();
        let runs = self.runs.keys().cloned();
        messages.extend(runs.map(|run| ClientMessage::OpenWorkflow { run }));
        for (agent, timeline) in &mut self.timelines {
            for older in timeline
                .older
                .iter_mut()
                .filter(|older| older.session != session)
            {
                older.session = session;
                messages.push(ClientMessage::LoadOlder {
                    agent: *agent,
                    before: older.before,
                    limit: older.limit,
                });
            }
        }
        // A proof names the connection it went on, so it never goes again.
        let pending = self
            .pending
            .values()
            .filter(|pending| !matches!(pending.reply, Reply::Proof(_)))
            .map(|pending| pending.message.clone());
        messages.extend(pending);
        messages
    }

    /// Closes the book once the task ends. Everything still waiting gets
    /// [`LinkError::Closed`].
    pub fn shut(&mut self) {
        self.closed = true;
        self.pending.clear();
        self.timelines.clear();
        self.runs.clear();
        self.strays.clear();
    }
}
