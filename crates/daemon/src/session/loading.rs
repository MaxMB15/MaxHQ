// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading the held agents' timelines back after a restart. Each one is
//! read off the loop, a few at a time, while the loop takes requests:
//! agents with work waiting first, then any a client opens or pages, then
//! the rest. Until an agent's timeline is in, what needs it waits for it,
//! and its holder isn't reached, so a CLI that died while `maxd` was down
//! is cut off from its whole timeline.

use std::collections::{HashMap, VecDeque};

use maxhq_protocol::ids::{AgentId, IdempotencyKey, SessionId};
use maxhq_protocol::message::ClientMessage;
use maxhq_protocol::model::{Activity, Vendor};
use maxhq_protocol::timeline::{ItemId, TimelineEvent, TimelineItem};
use tracing::warn;

use super::held::{is_busy, Reaching, Target};
use super::idem::Outcome;
use super::jobs::AT_ONCE;
use super::places::source;
use super::reads::read_history;
use super::timeline::Timeline;
use super::{Input, Manager};
use crate::drive::Action;

/// The timelines still to read after a restart, and what waits for them.
#[derive(Default)]
pub(super) struct Loading {
    /// The agents not read yet, in the order they're read.
    queue: VecDeque<AgentId>,
    /// Every agent whose timeline isn't in yet, with what waits for it.
    waiting: HashMap<AgentId, Vec<Later>>,
    /// How many reads run.
    reading: usize,
    /// The holders a previous run left, reached once their agents'
    /// timelines are in. None for one whose CLI died while `maxd` was down.
    holders: Vec<(SessionId, Option<u64>)>,
}

/// What waits for an agent's timeline.
pub(super) enum Later {
    /// A client asked for the items before an ordinal.
    Page { conn: u64, before: u64, limit: u32 },
    /// A keyed request for the agent.
    Request(IdempotencyKey, Box<ClientMessage>),
    /// An action for the agent's driver.
    Dispatch(Target, Box<Action>),
    /// Starting the agent's CLI again.
    Resume,
    /// Sending what the agent is owed.
    FollowUp,
    /// Reading the agent's history again.
    Reseed,
    /// Showing a `!` run as it was last kept.
    Run(ItemId),
    /// Starting a task's fresh session.
    Fresh,
}

impl Loading {
    /// Whether the agent's timeline is still to be read.
    pub fn waits(&self, agent: AgentId) -> bool {
        self.waiting.contains_key(&agent)
    }

    /// Keeps a holder back until its agents' timelines are in. `after` is
    /// the line it's read from, or none when its CLI is gone.
    pub fn wait_holder(&mut self, session: SessionId, after: Option<u64>) {
        self.holders.push((session, after));
    }
}

impl Manager {
    /// Starts reading every held agent's timeline, those with work waiting
    /// first and the most recently active first within each. A Codex agent
    /// with work brings the others on its app-server along, since their
    /// holder waits for all of them.
    pub(super) fn load_timelines(&mut self) {
        let mut agents: Vec<(bool, AgentId)> = self
            .held
            .values()
            .map(|held| {
                let id = held.agent.id;
                let pending = is_busy(&held.agent.activity)
                    || held.agent.activity == Activity::Starting
                    || held.owed.is_some()
                    || held.resuming
                    || self.attention.values().any(|open| open.agent == id)
                    || self.has_queued(id);
                (pending, id)
            })
            .collect();
        let pending: Vec<AgentId> = agents
            .iter()
            .filter(|(pending, _)| *pending)
            .map(|(_, id)| *id)
            .collect();
        for (flag, id) in &mut agents {
            *flag = *flag || pending.iter().any(|other| self.same_server(*other, *id));
        }
        let at = |id: &AgentId| self.held.get(id).map(|held| held.agent.last_activity_at);
        agents.sort_by(|(a_pending, a), (b_pending, b)| {
            b_pending.cmp(a_pending).then_with(|| at(b).cmp(&at(a)))
        });
        for (_, id) in agents {
            self.loading.waiting.insert(id, Vec::new());
            self.loading.queue.push_back(id);
            if self
                .held
                .get(&id)
                .is_some_and(|held| !held.fresh.is_empty())
            {
                self.wait_for(id, Later::Fresh);
            }
        }
        self.next_loads();
    }

    /// Whether two held agents are Codex threads on the same app-server.
    fn same_server(&self, a: AgentId, b: AgentId) -> bool {
        match (self.held.get(&a), self.held.get(&b)) {
            (Some(a), Some(b)) => {
                a.vendor() == Vendor::Codex
                    && b.vendor() == a.vendor()
                    && a.server_key() == b.server_key()
            }
            _ => false,
        }
    }

    /// Starts the next reads, while there's room. Each one reads what the
    /// agent said before it moved vendors first, then its history.
    fn next_loads(&mut self) {
        while self.loading.reading < AT_ONCE
            && let Some(agent) = self.loading.queue.pop_front()
        {
            self.loading.reading += 1;
            let Some(held) = self.held.get(&agent) else {
                self.post(Input::Loaded {
                    agent,
                    timeline: None,
                });
                continue;
            };
            let source = source(held);
            let mut timeline = self.new_timeline(agent);
            let (vendors, places, db) =
                (self.vendors.clone(), self.places.clone(), self.db.clone());
            let bytes = self.limits.disk;
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let read = tokio::task::spawn_blocking(move || {
                    if let Some(items) = db.earlier::<TimelineItem>(agent) {
                        timeline.thaw(items);
                    }
                    // The vendor's history doesn't have the `!` runs, so
                    // they go back where they came.
                    match db.user_runs::<TimelineItem>(agent) {
                        Ok(runs) => timeline.add_runs(
                            (runs.into_iter())
                                .map(|run| (run.record, run.after.map(ItemId), run.at))
                                .collect(),
                        ),
                        Err(err) => warn!(%err, %agent, "couldn't read the agent's runs"),
                    }
                    match read_history(&*vendors, &*places, &source, agent, bytes) {
                        Some(Ok(items)) => timeline.replace_history(items),
                        Some(Err(err)) => warn!(%err, %agent, "couldn't read the agent's history"),
                        None => {}
                    }
                    timeline
                })
                .await;
                let timeline = match read {
                    Ok(timeline) => Some(Box::new(timeline)),
                    Err(err) => {
                        warn!(%err, %agent, "the history read didn't finish");
                        None
                    }
                };
                let _ = tx.send(Input::Loaded { agent, timeline });
            });
        }
    }

    /// Takes in a timeline read after a restart. What the agent got while
    /// it was read, such as the messages you queued, goes on its end, and
    /// clients that have it open get it whole. Then what waited for it goes
    /// on.
    pub(super) fn loaded(&mut self, agent: AgentId, timeline: Option<Box<Timeline>>) {
        self.loading.reading = self.loading.reading.saturating_sub(1);
        let laters = self.loading.waiting.remove(&agent).unwrap_or_default();
        // The `!` runs that changed during the read change before clients
        // get the timeline, so none shows a run as it was.
        let (runs, laters): (Vec<_>, Vec<_>) =
            (laters.into_iter()).partition(|later| matches!(later, Later::Run(_)));
        if self.held.contains_key(&agent) {
            let mut timeline =
                timeline.map_or_else(|| self.new_timeline(agent), |timeline| *timeline);
            timeline.limit(self.limits);
            if let Some(got) = self.timelines.remove(&agent) {
                for item in got.items() {
                    if !timeline.contains(&item.id) {
                        timeline.apply(TimelineEvent::Upsert {
                            item: item.into_owned(),
                        });
                    }
                }
            }
            for later in runs {
                let Later::Run(id) = later else { continue };
                if let Some(row) = self.db.user_run::<TimelineItem>(agent, &id.0) {
                    timeline.apply(TimelineEvent::Upsert { item: row.record });
                }
            }
            self.timelines.insert(agent, timeline);
            self.reset_viewers(agent);
        }
        self.release_holders();
        self.next_loads();
        for later in laters {
            self.go_on(agent, later);
        }
    }

    /// Reaches the holders whose agents' timelines are all in. One whose CLI
    /// died while `maxd` was down counts as lost, as a failed reach would.
    pub(super) fn release_holders(&mut self) {
        let holders = std::mem::take(&mut self.loading.holders);
        for (session, after) in holders {
            if self
                .leads_of(session)
                .iter()
                .any(|lead| self.loading.waits(*lead))
            {
                self.loading.holders.push((session, after));
                continue;
            }
            match after {
                Some(after) => self.reach(session, Reaching::Restored, None, after, None),
                None => {
                    let Some(hold) = self.holds.get_mut(&session) else {
                        continue;
                    };
                    hold.reaching = None;
                    let parked = std::mem::take(&mut hold.parked);
                    self.refuse_parked(parked, "couldn't reach the agent's process");
                    self.lost(session);
                }
            }
        }
    }

    /// Carries on with something that waited for the agent's timeline.
    fn go_on(&mut self, agent: AgentId, later: Later) {
        match later {
            Later::Page {
                conn,
                before,
                limit,
            } => self.load_older(conn, agent, before, limit),
            Later::Request(key, message) => {
                if let Some(outcome) = self.action(key, *message) {
                    self.finish(key, Outcome::Action(outcome));
                }
            }
            Later::Dispatch(target, action) => self.work.push_back((target, *action)),
            Later::Resume => self.resume_agent(agent),
            Later::FollowUp => self.follow_up(agent),
            Later::Reseed => self.reseed(agent),
            Later::Run(id) => self.show_run(agent, &id),
            Later::Fresh => self.fresh_due(agent),
        }
    }

    /// Has `later` wait for the agent's timeline.
    pub(super) fn wait_for(&mut self, agent: AgentId, later: Later) {
        if let Some(laters) = self.loading.waiting.get_mut(&agent) {
            laters.push(later);
        }
    }

    /// Reads the agent's timeline next, with the other threads on its
    /// app-server for a Codex agent. One already being read goes on.
    pub(super) fn hurry(&mut self, agent: AgentId) {
        if !self.loading.waits(agent) {
            return;
        }
        let mut agents: Vec<AgentId> = self
            .held
            .keys()
            .filter(|other| **other != agent && self.same_server(agent, **other))
            .copied()
            .collect();
        agents.push(agent);
        for agent in agents {
            if let Some(at) = self
                .loading
                .queue
                .iter()
                .position(|queued| *queued == agent)
            {
                self.loading.queue.remove(at);
                self.loading.queue.push_front(agent);
            }
        }
    }

    /// The request, unless it acts on an agent whose timeline is still to
    /// be read. Then it waits for it, and that agent's read goes next.
    pub(super) fn after_loading(
        &mut self,
        key: IdempotencyKey,
        message: ClientMessage,
    ) -> Option<ClientMessage> {
        let Some(agent) = acts_on(&message).map(|agent| self.lead_of(agent)) else {
            return Some(message);
        };
        if !self.loading.waits(agent) {
            return Some(message);
        }
        self.hurry(agent);
        self.wait_for(agent, Later::Request(key, Box::new(message)));
        None
    }
}

/// The agent a keyed request acts on through its timeline or holder.
fn acts_on(message: &ClientMessage) -> Option<AgentId> {
    match message {
        ClientMessage::SendInput { agent, .. }
        | ClientMessage::CancelQueued { agent, .. }
        | ClientMessage::Interrupt { agent, .. }
        | ClientMessage::SetPermission { agent, .. }
        | ClientMessage::SetModel { agent, .. }
        | ClientMessage::SetEffort { agent, .. }
        | ClientMessage::StopAgent { agent, .. }
        | ClientMessage::DeleteAgent { agent, .. }
        | ClientMessage::TerminalOpen { agent, .. }
        | ClientMessage::SessionRun { agent, .. }
        | ClientMessage::SessionShare { agent, .. }
        | ClientMessage::SessionCd { agent, .. } => Some(*agent),
        _ => None,
    }
}
