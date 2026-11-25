// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Fresh sessions for a task's runs. A task that asks for one ends its
//! agent's vendor session and readies a new one in the same place, with
//! the same vendor, model and mode, whose first message is the run's
//! prompt. The timeline keeps what came before, under a marker, and none
//! of it carries over. A run that finds the agent busy waits for its turn
//! to end, as a queued message does, and never cuts the turn off.

use maxhq_core::agents::{self as rules, Turn};
use maxhq_core::context;
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, ClientMessage, SendMode, UserInput};
use maxhq_protocol::model::{Activity, Vendor};
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineEvent, TimelineItem};
use uuid::Uuid;

use super::agent_tools::NO_CONN;
use super::held::{After, FreshRun};
use super::idem::{Outcome, Seen};
use super::inbox::in_turn;
use super::loading::Later;
use super::{is_queued, refused, Manager};
use crate::hub::Moved;

/// Why a task can't start a fresh session for a sub-agent.
const SUB_AGENT: &str = concat!(
    "A sub-agent runs in its lead's session, ",
    "so a task can't start a fresh one for it."
);

/// Why a task can't start a fresh session for the dispatcher.
const DISPATCHER: &str = concat!(
    "The dispatcher keeps its session, ",
    "so a task can't start a fresh one for it."
);

impl Manager {
    /// Sends a task's run to `agent` in a fresh session: now when its turn
    /// is over, or once it is. `key` finishes once the message goes, or
    /// once the run waits.
    pub(super) fn send_fresh(&mut self, key: IdempotencyKey, agent: AgentId, text: String) {
        let lead = self.lead_of(agent);
        let Some(waiting) = self.held.get(&lead).map(|held| !held.fresh.is_empty()) else {
            // The usual refusal says why.
            self.message(NO_CONN, send(key, agent, text));
            return;
        };
        let refusal = if lead != agent {
            Some(SUB_AGENT.to_owned())
        } else if self.is_dispatcher(lead) {
            Some(DISPATCHER.to_owned())
        } else {
            context::check(&text, &[], &[]).err()
        };
        if let Some(reason) = refusal {
            return self.answer_fresh(key, refused(reason));
        }
        let loading = self.loading.waits(lead);
        if loading || waiting || self.renewing.contains_key(&lead) || self.turn_runs(lead) {
            if let Some(held) = self.held.get_mut(&lead) {
                held.fresh.push(FreshRun { text });
            }
            self.save(lead);
            if loading {
                self.wait_for(lead, Later::Fresh);
                self.hurry(lead);
            }
            return self.answer_fresh(key, ActionOutcome::Done);
        }
        self.renew(lead);
        self.renewing.insert(lead, key);
        self.message(NO_CONN, send(key, lead, text));
    }

    /// Starts the fresh session the oldest waiting run asks for, once the
    /// agent's turn is over and the last one it started is under way.
    pub(super) fn fresh_due(&mut self, lead: AgentId) {
        // The run goes once the timeline is in.
        if self.loading.waits(lead) {
            return;
        }
        let waiting = self
            .held
            .get(&lead)
            .is_some_and(|held| !held.fresh.is_empty());
        if !waiting || self.renewing.contains_key(&lead) || self.turn_runs(lead) {
            return;
        }
        let Some(run) = self.held.get_mut(&lead).map(|held| held.fresh.remove(0)) else {
            return;
        };
        self.renew(lead);
        let key = IdempotencyKey::generate();
        self.renewing.insert(lead, key);
        self.message(NO_CONN, send(key, lead, run.text));
    }

    /// Follows an agent's turns for its fresh sessions. The one a run
    /// started is under way once the agent works or stops, and a turn
    /// that ends lets the next run go.
    pub(super) fn fresh_moved(&mut self, moved: &Moved) {
        let lead = self.lead_of(moved.agent);
        if moved.agent == lead && rules::turn(&moved.to) != Turn::Done {
            self.renewing.remove(&lead);
        }
        self.fresh_due(lead);
    }

    /// Lets the next run go when the message a fresh session started with
    /// didn't reach the agent. Any action that ends may also be the one a
    /// waiting run waited for, such as a message on its way.
    pub(super) fn fresh_sent(&mut self, key: IdempotencyKey, outcome: &Outcome) {
        let failed = self
            .renewing
            .iter()
            .find(|(_, sent)| **sent == key)
            .map(|(lead, _)| *lead)
            .filter(|_| !matches!(outcome, Outcome::Action(ActionOutcome::Done)));
        if let Some(lead) = failed {
            self.renewing.remove(&lead);
        }
        let waiting: Vec<AgentId> = self
            .held
            .iter()
            .filter(|(_, held)| !held.fresh.is_empty())
            .map(|(lead, _)| *lead)
            .collect();
        for lead in waiting {
            self.fresh_due(lead);
        }
    }

    /// Finishes `key` with `outcome` at once, unless it went already.
    fn answer_fresh(&mut self, key: IdempotencyKey, outcome: ActionOutcome) {
        if let Seen::New = self.keys.begin(key, NO_CONN) {
            self.finish(key, Outcome::Action(outcome));
        }
    }

    /// Whether the agent's turn isn't over: it or a sub-agent works, needs
    /// you or waits for a limit, a message to it is on its way or queued,
    /// or its Codex thread is being adopted.
    fn turn_runs(&self, lead: AgentId) -> bool {
        let Some(held) = self.held.get(&lead) else {
            return false;
        };
        let subs = self.subs_of(lead);
        let mut activities = std::iter::once(&held.agent.activity).chain(
            subs.iter()
                .filter_map(|sub| self.subs.get(sub))
                .map(|sub| &sub.activity),
        );
        activities.any(|activity| {
            in_turn(activity)
                || matches!(
                    activity,
                    Activity::NeedsInput { .. } | Activity::WaitingForLimitReset { .. }
                )
        }) || self.adopting.contains_key(&lead)
            || self
                .after
                .values()
                .any(|(agent, after)| *agent == lead && *after == After::Sent)
            || self
                .timelines
                .get(&lead)
                .is_some_and(|timeline| timeline.kept().any(is_queued))
    }

    /// Ends the agent's vendor session and readies a new one in the same
    /// place, which its next message starts. Its sub-agents go with the
    /// old session. The timeline keeps what came before, under a marker
    /// that no handoff reaches past.
    fn renew(&mut self, lead: AgentId) {
        let now = self.clock.wall();
        // A new run asks again before it uses notes from a container.
        self.agent_tools.notes.remove(&lead);
        for sub in self.subs_of(lead) {
            self.gone(sub);
        }
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        let pending = held.handoff.take();
        let session = held.session.take();
        let old = std::mem::take(&mut held.vendor_session);
        held.left = Some(old).filter(|old| !old.is_empty());
        match held.vendor() {
            // A Claude session is named before it starts.
            Vendor::Claude => held.vendor_session = Uuid::now_v7().to_string(),
            // A thread is started for the agent once its message goes.
            Vendor::Codex => held.adopt = true,
            // An ACP CLI names the session it makes.
            _ => held.epoch += 1,
        }
        if held.title.is_none() {
            held.title = Some(held.agent.title.clone());
        }
        held.natives.clear();
        held.resumes = Default::default();
        held.fault = None;
        held.owed = None;
        held.resuming = false;
        held.resume_after = None;
        held.down_since = None;
        held.released = false;
        held.cut_at_limit = false;
        held.limit_resets_at = None;
        held.agent.context = None;
        held.agent.vendor_session_id = None;
        held.agent.activity = Activity::Idle;
        let claude = held.vendor() == Vendor::Claude;
        if held.own_process()
            && let Some(session) = session
            && let Some(hold) = self.holds.get_mut(&session)
        {
            // The holder no longer runs the agent, so nothing it says from
            // here on reaches it.
            hold.record.agent = None;
            let record = hold.record.clone();
            self.store_holder(session, &record);
            self.stop_hold(session);
        }
        if claude {
            // Its session has no transcript to resume yet.
            self.trust_fresh.insert(lead);
        }
        self.idle_ends.remove(&lead);
        self.limit_waits.remove(&lead);
        if let Some(pending) = pending {
            self.settle_marker(lead, &pending.marker, ItemStatus::Cancelled);
        }
        let item = TimelineItem {
            id: ItemId(format!("maxhq-new-session-{}", Uuid::now_v7())),
            agent: lead,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Completed,
            started_at: Some(now),
            completed_at: Some(now),
            body: ItemBody::NewSession,
        };
        self.timeline_events(vec![(lead, TimelineEvent::Upsert { item })]);
        self.timeline_mut(lead).freeze();
        self.keep_earlier(lead);
        self.save(lead);
        self.publish(lead);
    }
}

/// A queued message to `agent`, as a task's run sends it.
fn send(key: IdempotencyKey, agent: AgentId, text: String) -> ClientMessage {
    ClientMessage::SendInput {
        key,
        agent,
        input: UserInput {
            text,
            attachments: Vec::new(),
            refs: Vec::new(),
        },
        mode: SendMode::Queue,
    }
}
