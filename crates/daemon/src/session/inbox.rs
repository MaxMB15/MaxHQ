// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Each lead agent's inbox: where it sits in the sidebar. The manager is the
//! only writer. It applies your changes with the rules in
//! [`maxhq_core::inbox`], wakes a snooze when it ends or when the agent
//! needs you sooner, snoozes an agent that waits for its usage limit when
//! the device says to, and settles agents that sat idle long enough. Each
//! change is stored and handed to the hub, which shows it on the agent.
//!
//! Settling an agent MaxHQ holds stops its process, the way `StopAgent`
//! does. Settling one you started yourself only moves it in the list:
//! MaxHQ never signals a process it didn't start. A turn you start in its
//! terminal brings it back, as a message sent through MaxHQ does.
//!
//! The agents a lead agent started through the agent tools settle along
//! with it, at every level below, once each one is done and its worktree
//! holds nothing unsaved; the rest stay in view. Each takes its caller's
//! own settle record, so the app shows it under its caller, and when the
//! caller leaves the settled list, those that went along come back.
//!
//! A held agent's inbox goes when MaxHQ stops holding it. One you started
//! yourself goes once it's gone for good: no observer has reported it for
//! 30 days, counted across restarts, and every observer has reported what
//! it watches since `maxd` started, so it isn't one not seen yet.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use maxhq_core::inbox;
use maxhq_protocol::ids::{AgentId, IdempotencyKey, WorkspaceId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{
    Activity, Agent, Control, DeviceSettings, Inbox, InboxChange, ProjectSettings, Settled,
    WorkspaceLocation,
};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::time::Timestamp;
use tracing::warn;

use super::held::{internal_key, is_busy, Internal, Pending, Target};
use super::holders::Checkout;
use super::{is_queued, refused, Input, Manager};
use crate::drive::Action;
use crate::hub::{self, Moved};

pub(super) mod automated;

/// How often auto-settle looks at every lead agent. A snooze whose end the
/// timer missed, such as while the Mac slept, wakes on this beat too.
pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// How long auto-settle waits before it checks the same agent's worktree
/// again, so an agent with work it keeps doesn't run git every minute.
pub(super) const RECHECK_AFTER: Duration = Duration::from_secs(10 * 60);

/// How long an agent you started yourself must be gone before its inbox
/// goes. A session you quit and resume days later comes back as you left
/// it, pin and all. Losing that costs more than a few rows kept too long.
pub(super) const GONE_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What a client hears when it files a sub-agent.
const SUB_AGENT: &str =
    "only lead agents have a place in the inbox. Settle, snooze or pin the agent that started it";

/// The inbox state only the manager needs. The inboxes themselves live in
/// the hub and the database.
#[derive(Default)]
pub(super) struct Inboxes {
    /// When each snoozed agent wakes.
    snoozes: HashMap<AgentId, Timestamp>,
    /// When auto-settle looks next.
    next_sweep: Option<Instant>,
    /// Agents whose worktree is being checked.
    checking: HashSet<AgentId>,
    /// When each agent's worktree was last checked.
    checked: HashMap<AgentId, Instant>,
    /// When a beat first found each agent with an inbox gone, as stored
    /// with its inbox.
    missing: HashMap<AgentId, Timestamp>,
    /// What each other device last said its enabled tasks reach here.
    heard: automated::Heard,
}

/// Why an agent can't settle or snooze now.
pub(super) enum Busy {
    /// It needs your input.
    NeedsInput,
    /// It works.
    Works,
    /// A message waits for it.
    Queued,
}

impl Manager {
    /// Puts back the inboxes the previous run stored, wakes the snoozes
    /// that ended while `maxd` was down and those whose agent needs you
    /// since, and starts auto-settle's beat.
    pub(super) fn load_inboxes(&mut self) {
        let stored = self.db.inboxes().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the stored inboxes");
            HashMap::new()
        });
        self.inbox.snoozes = stored
            .iter()
            .filter_map(|(agent, inbox)| inbox.snoozed_until.map(|until| (*agent, until)))
            .collect();
        self.hub.restore_inboxes(stored);
        // A count that can't be read starts over at the next beat, which
        // only keeps an inbox longer.
        self.inbox.missing = self.db.missing_since().unwrap_or_else(|err| {
            warn!(%err, "couldn't read when the agents with an inbox went missing");
            HashMap::new()
        });
        self.load_task_targets();
        self.wake_due();
        // Snoozed and settled agents an observer reported before the
        // manager watched count as seen for the first time.
        for agent in self.hub.leads() {
            if self.inbox.snoozes.contains_key(&agent.id) || agent.inbox.settled.is_some() {
                self.inbox_moved(Moved {
                    agent: agent.id,
                    from: None,
                    to: agent.activity,
                    at: agent.last_activity_at,
                });
            }
        }
        self.inbox.next_sweep = Some(self.clock.now() + SWEEP_EVERY);
    }

    /// Changes an agent's inbox as you asked. Only lead agents have one, and
    /// an agent that needs you, works or has a message waiting can't be
    /// settled or snoozed out of view.
    pub(super) fn set_inbox(&mut self, agent: AgentId, change: &InboxChange) -> ActionOutcome {
        let lead = self.lead_of(agent);
        let Some(found) = self.hub.reported(agent) else {
            return ActionOutcome::UnknownAgent;
        };
        if lead != agent || found.parent.is_some() {
            return refused(SUB_AGENT);
        }
        let now = self.clock.wall();
        let current = self.hub.inbox(agent);
        let next = inbox::apply(&current, change, now);
        let (settles, snoozes) = match change {
            InboxChange::Settle => (true, false),
            InboxChange::Snooze { until } if *until <= now => {
                return refused("pick a time in the future to snooze the agent until");
            }
            InboxChange::Snooze { .. } => (false, true),
            // An undo that settles or snoozes the agent again is held to
            // the same rules as the change it puts back.
            InboxChange::Restore { .. } => (
                current.settled.is_none() && next.settled.is_some(),
                inbox::is_snoozed(&next, now) && next.snoozed_until != current.snoozed_until,
            ),
            _ => (false, false),
        };
        if settles {
            match self.busy(&found) {
                Some(Busy::NeedsInput) => {
                    return refused("the agent needs your input. Answer it before you settle it");
                }
                Some(Busy::Works) => {
                    return refused(
                        "the agent is still working. Stop it or let it finish before you settle it",
                    );
                }
                Some(Busy::Queued) => {
                    return refused(
                        "a message is waiting for the agent. Take it back or let it go before you \
                         settle it",
                    );
                }
                None => {}
            }
        }
        if snoozes {
            match self.busy(&found) {
                Some(Busy::NeedsInput) => {
                    return refused("the agent needs your input. Answer it before you snooze it");
                }
                Some(Busy::Queued) => {
                    return refused(
                        "a message is waiting for the agent. Take it back or let it go before you \
                         snooze it",
                    );
                }
                Some(Busy::Works) | None => {}
            }
        }
        // The settle is stored before the agent is stopped, so a stop that
        // fails leaves it settled.
        self.write_inbox(agent, next);
        if settles {
            self.stop_settled(agent);
        }
        ActionOutcome::Done
    }

    /// Why the agent can't settle now, or none when it can. A sub-agent
    /// that needs you or works counts for its lead.
    pub(super) fn busy(&self, agent: &Agent) -> Option<Busy> {
        let subs: Vec<Agent> = if self.held.contains_key(&agent.id) {
            self.subs_of(agent.id)
                .iter()
                .filter_map(|sub| self.subs.get(sub).cloned())
                .collect()
        } else {
            self.hub.subagents(agent.id)
        };
        let all = || std::iter::once(&agent.activity).chain(subs.iter().map(|sub| &sub.activity));
        if all().any(|activity| matches!(activity, Activity::NeedsInput { .. })) {
            Some(Busy::NeedsInput)
        } else if all().any(in_turn) {
            Some(Busy::Works)
        } else if self.has_queued(agent.id) {
            Some(Busy::Queued)
        } else {
            None
        }
    }

    /// Whether a message you sent still waits for the agent, or a task's
    /// run waits to start a fresh session for it or just started one.
    pub(super) fn has_queued(&self, agent: AgentId) -> bool {
        self.held
            .get(&agent)
            .is_some_and(|held| !held.fresh.is_empty())
            || self.renewing.contains_key(&agent)
            || self
                .timelines
                .get(&agent)
                .is_some_and(|timeline| timeline.kept().any(is_queued))
    }

    /// Stores an agent's inbox and shows it. An inbox that didn't change
    /// is left alone. The agents it started follow a settle, and come
    /// back with it when it leaves the settled list.
    pub(super) fn write_inbox(&mut self, agent: AgentId, inbox: Inbox) {
        let before = self.hub.inbox(agent);
        if before == inbox {
            return;
        }
        match inbox.snoozed_until {
            Some(until) => self.inbox.snoozes.insert(agent, until),
            None => self.inbox.snoozes.remove(&agent),
        };
        if let Err(err) = self.db.put_inbox(agent, &inbox) {
            warn!(%err, %agent, "couldn't store the agent's inbox");
        }
        self.send_waiting(agent);
        let settled = inbox.settled.clone();
        self.hub.set_inbox(agent, inbox);
        match (before.settled, settled) {
            (None, Some(settled)) => self.settle_along(agent, &settled),
            (Some(settled), None) => self.unsettle_along(agent, &settled),
            _ => {}
        }
    }

    /// The lead agents `caller` started here through the agent tools.
    fn started_leads(&self, caller: AgentId) -> Vec<Agent> {
        self.hub
            .leads()
            .into_iter()
            .filter(|agent| inbox::started_by(agent, caller, self.device))
            .collect()
    }

    /// Starts settling along with a lead agent the agents it started that
    /// are done. Each one's worktree is checked first, off the loop, as
    /// auto-settle checks one. Every level below follows, as each one that
    /// settles takes the agents it started along in turn.
    fn settle_along(&mut self, caller: AgentId, settled: &Settled) {
        let now = self.clock.wall();
        for agent in self.started_leads(caller) {
            if !self.may_go_along(&agent, settled, Some(false), now) {
                continue;
            }
            let Some(checkout) = self.checkout(&agent) else {
                continue;
            };
            let (unsaved, tx, settled) = (self.unsaved.clone(), self.tx.clone(), settled.clone());
            let id = agent.id;
            tokio::spawn(async move {
                let unsaved = unsaved.unsaved(checkout).await;
                let _ = tx.send(Input::Along {
                    caller,
                    settled: Box::new(settled),
                    agent: id,
                    unsaved,
                });
            });
        }
    }

    /// Settles a started agent whose worktree was checked, under its
    /// caller's own record, if the caller is still settled by that settle
    /// and the agent may still go along. Sharing the record is what marks
    /// it as settled with its caller, for the app and for an unsettle.
    pub(super) fn along_checked(
        &mut self,
        caller: AgentId,
        settled: Settled,
        id: AgentId,
        unsaved: Option<bool>,
    ) {
        if self.hub.inbox(caller).settled.as_ref() != Some(&settled) {
            return;
        }
        let Some(agent) = self.hub.reported(id) else {
            return;
        };
        if !inbox::started_by(&agent, caller, self.device)
            || !self.may_go_along(&agent, &settled, unsaved, self.clock.wall())
        {
            return;
        }
        let mut next = self.hub.inbox(id);
        inbox::settle(&mut next, settled);
        self.write_inbox(id, next);
        self.stop_settled(id);
    }

    /// Whether a started agent settles along with its caller now. On top
    /// of the shared rules, one with a message waiting, a sub-agent at work
    /// or an enabled task that reaches it stays, as auto-settle leaves it.
    fn may_go_along(
        &self,
        agent: &Agent,
        settled: &Settled,
        unsaved: Option<bool>,
        now: Timestamp,
    ) -> bool {
        inbox::goes_along(agent, settled, unsaved, now)
            && self.busy(agent).is_none()
            && !self.automated(agent.id)
    }

    /// Brings back the agents that settled along with a lead agent that
    /// left the settled list, and only those, as an unsettle of each would.
    fn unsettle_along(&mut self, caller: AgentId, settled: &Settled) {
        let now = self.clock.wall();
        for agent in self.started_leads(caller) {
            if inbox::settled_with(&agent, settled) {
                let next = inbox::apply(&self.hub.inbox(agent.id), &InboxChange::Unsettle, now);
                self.write_inbox(agent.id, next);
            }
        }
    }

    /// Stops a settled agent's process, if MaxHQ holds it. A failed stop is
    /// only logged, since the agent stays settled either way.
    pub(super) fn stop_settled(&mut self, lead: AgentId) {
        let key = internal_key();
        if self.stop_lead(key, lead) {
            self.internal.insert(key, Internal::Stop(lead));
        }
    }

    /// Stops a held agent and keeps it from coming back, as `StopAgent`
    /// does. One that's paused or whose CLI is down only records that you
    /// stopped it. Returns true when a driver stops it, which answers `key`
    /// once it has; an agent MaxHQ doesn't hold is left alone.
    pub(super) fn stop_lead(&mut self, key: IdempotencyKey, lead: AgentId) -> bool {
        let Some(held) = self.held.get(&lead) else {
            return false;
        };
        let parked = held.paused.is_some() || self.session_of(held).is_none();
        if parked {
            if let Some(held) = self.held.get_mut(&lead) {
                held.paused = Some(maxhq_protocol::model::PauseReason::ByYou);
                held.resuming = false;
                held.owed = None;
            }
            self.save(lead);
            self.publish(lead);
            return false;
        }
        self.work
            .push_back((Target::Agent(lead), Action::Stop { key, agent: lead }));
        true
    }

    /// A message you sent the agent went through, so it comes back: out of
    /// the settled list and its snooze, and watched by auto-settle again.
    pub(super) fn message_sent(&mut self, lead: AgentId) {
        let mut next = self.hub.inbox(lead);
        inbox::on_message(&mut next);
        self.write_inbox(lead, next);
    }

    /// Forgets the inbox of an agent that's gone.
    pub(super) fn forget_inbox(&mut self, agent: AgentId) {
        self.inbox.snoozes.remove(&agent);
        self.inbox.checking.remove(&agent);
        self.inbox.checked.remove(&agent);
        self.inbox.missing.remove(&agent);
        if let Err(err) = self.db.remove_inbox(agent) {
            warn!(%err, %agent, "couldn't forget the agent's inbox");
        }
        self.hub.forget_inbox(agent);
        self.forget_pulls(agent);
    }

    /// Takes in a change of a lead agent's activity. An agent that comes to
    /// need you wakes from its snooze early, and one that leaves the limit
    /// wait its snooze was for wakes too. A turn you start in your own
    /// terminal brings back an agent MaxHQ doesn't hold, as a message does.
    /// One that starts waiting for its usage limit snoozes until the reset,
    /// when the device says to. A run that
    /// ends lets auto-settle look at the agent.
    pub(super) fn inbox_moved(&mut self, moved: Moved) {
        let Moved {
            agent,
            from,
            to,
            at,
        } = moved;
        let now = self.clock.wall();
        let mut next = self.hub.inbox(agent);

        // The first report this run counts only when it's newer than the
        // snooze, since the snooze may have come after it.
        let entered = match &from {
            Some(from) => !inbox::wakes_early(from),
            None => next.snoozed_at.is_none_or(|snoozed| at > snoozed),
        };
        if inbox::is_snoozed(&next, now) && inbox::wakes_early(&to) && entered {
            inbox::wake(&mut next, now);
        }

        if let Some(Activity::WaitingForLimitReset {
            resets_at: Some(reset),
        }) = &from
            && next.snoozed_until == Some(*reset)
            && limit_reset(&to) != Some(*reset)
        {
            inbox::wake(&mut next, now);
        }

        // A turn you start in your own terminal is a message too, so an
        // agent MaxHQ doesn't hold comes back the way it would for one sent
        // through MaxHQ. Only a turn newer than the settle and the snooze
        // counts, so a restart or a rescan that reports the old turn again
        // leaves the agent where you put it. Held agents' turns come through
        // `SendInput`, and a wake above keeps its marker.
        let rested = from.as_ref().is_none_or(|from| !in_turn(from));
        let away = next.settled.is_some() || next.snoozed_until.is_some();
        let newer = next.settled.as_ref().is_none_or(|settled| at > settled.at)
            && next.snoozed_at.is_none_or(|snoozed| at > snoozed);
        if rested && in_turn(&to) && away && newer && self.observed(agent) {
            inbox::on_message(&mut next);
        }

        // A limit wait seen for the first time after a wake doesn't snooze
        // again: it's the wait you already woke it from.
        let woke_since = from.is_none() && next.woke_at.is_some_and(|woke| woke >= at);
        if let Some(reset) = limit_reset(&to)
            && reset > now
            && self.device_settings().snooze_at_limit
            && next.settled.is_none()
            && next.snoozed_until != Some(reset)
            && !woke_since
        {
            next.snoozed_until = Some(reset);
            next.snoozed_at = Some(now);
        }
        self.write_inbox(agent, next);

        if from.as_ref().is_some_and(in_turn) && !in_turn(&to) {
            self.consider(agent, true);
            self.turn_ended_for_skills(agent);
            if let Some(held) = self.held.get(&agent)
                && held.login == maxhq_protocol::model::LoginKind::ModelServer
            {
                crate::local_models::turn_ended(held.account);
            }
        }
    }

    /// Whether the agent runs in a terminal you started it in, which MaxHQ
    /// only watches.
    fn observed(&self, agent: AgentId) -> bool {
        !self.held.contains_key(&agent)
            && self
                .hub
                .reported(agent)
                .is_some_and(|found| found.control == Control::Observed)
    }

    /// The soonest snooze end or auto-settle beat.
    pub(super) fn inbox_deadline(&self) -> Option<Instant> {
        let wall = self.clock.wall();
        let now = self.clock.now();
        let wake = self
            .inbox
            .snoozes
            .values()
            .min()
            .map(|until| now + Duration::from_millis(until.0.saturating_sub(wall.0)));
        wake.into_iter().chain(self.inbox.next_sweep).min()
    }

    /// Wakes the snoozes that ended and runs auto-settle's beat when it's
    /// due, which also forgets the inboxes of agents that are gone.
    pub(super) fn inbox_timers(&mut self) {
        self.wake_due();
        let now = self.clock.now();
        if self.inbox.next_sweep.is_some_and(|at| at <= now) {
            self.inbox.next_sweep = Some(now + SWEEP_EVERY);
            self.forget_gone();
            for agent in self.hub.leads() {
                self.consider(agent.id, false);
            }
        }
    }

    /// Forgets the inbox of each agent gone for [`GONE_FOR`], counted from
    /// the beat that first found it gone and stopped once it's back. Until
    /// every observer has reported what it watches, an agent the hub
    /// doesn't know may only not be seen yet, so no count starts and
    /// nothing is gone.
    fn forget_gone(&mut self) {
        let reported = self.hub.observers_reported();
        let now = self.clock.wall();
        let filed = self.hub.filed();
        self.inbox.missing.retain(|agent, _| filed.contains(agent));
        let mut gone = Vec::new();
        for agent in filed {
            let since = self.inbox.missing.get(&agent).copied();
            if self.still_here(agent) {
                if since.is_some() {
                    self.missing_since(agent, None);
                }
                continue;
            }
            if !reported {
                continue;
            }
            match since {
                Some(since) if now >= since.after(GONE_FOR) => gone.push(agent),
                Some(_) => {}
                None => self.missing_since(agent, Some(now)),
            }
        }
        if gone.is_empty() {
            return;
        }
        let Some(saved) = self.saved_held() else {
            return;
        };
        for agent in gone {
            if !saved.contains(&agent) {
                self.forget_inbox(agent);
            }
        }
    }

    /// Starts or stops an agent's count toward gone for good. It changes
    /// only once stored, so a restart never brings back an older count.
    fn missing_since(&mut self, agent: AgentId, since: Option<Timestamp>) {
        if let Err(err) = self.db.set_missing_since(agent, since) {
            warn!(%err, %agent, "couldn't store when the agent went missing");
            return;
        }
        match since {
            Some(since) => self.inbox.missing.insert(agent, since),
            None => self.inbox.missing.remove(&agent),
        };
    }

    /// Whether an agent is still around: an observer reports it, MaxHQ
    /// holds it, or MaxHQ is taking it over.
    fn still_here(&self, agent: AgentId) -> bool {
        self.hub.get(&EntityKey::Agent(agent)).is_some()
            || self.held.contains_key(&agent)
            || self
                .pending
                .values()
                .any(|pending| matches!(pending, Pending::Adopt(held) if held.agent.id == agent))
    }

    /// The agents with a held row, whether its record parses or not. None
    /// when that isn't known for sure, and then no inbox is forgotten.
    fn saved_held(&self) -> Option<HashSet<AgentId>> {
        let ids = match self.db.held_ids() {
            Ok(ids) => ids,
            Err(err) => {
                warn!(%err, "couldn't read the held agents, so no inbox is forgotten");
                return None;
            }
        };
        let mut saved = HashSet::new();
        for id in ids {
            match id.parse() {
                Ok(agent) => {
                    saved.insert(agent);
                }
                Err(err) => {
                    warn!(id, %err, "a held agent's id doesn't parse, so no inbox is forgotten");
                    return None;
                }
            }
        }
        Some(saved)
    }

    /// Wakes every snooze whose time has come. The wall clock decides, so a
    /// snooze that ended while the Mac slept wakes on the next beat.
    fn wake_due(&mut self) {
        let now = self.clock.wall();
        let due: Vec<(AgentId, Timestamp)> = self
            .inbox
            .snoozes
            .iter()
            .filter(|(_, until)| **until <= now)
            .map(|(agent, until)| (*agent, *until))
            .collect();
        for (agent, until) in due {
            let mut next = self.hub.inbox(agent);
            // The marker says when the snooze ended, even one that ended
            // while `maxd` was down.
            inbox::wake(&mut next, until);
            self.write_inbox(agent, next);
        }
    }

    /// Starts auto-settle's check of a lead agent, once everything cheap
    /// says it may settle. Only then is its worktree checked, off the loop.
    /// `now` skips the wait between checks, for a run that just ended.
    fn consider(&mut self, id: AgentId, now: bool) {
        self.settle_ended(id);
        if self.inbox.checking.contains(&id) {
            return;
        }
        let at = self.clock.now();
        let recent = self
            .inbox
            .checked
            .get(&id)
            .is_some_and(|checked| at < *checked + RECHECK_AFTER);
        if recent && !now {
            return;
        }
        let Some(agent) = self.hub.reported(id) else {
            return;
        };
        if !self.may_auto_settle(&agent, Some(false)) {
            return;
        }
        let Some(checkout) = self.checkout(&agent) else {
            return;
        };
        self.inbox.checking.insert(id);
        self.inbox.checked.insert(id, at);
        let unsaved = self.unsaved.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let unsaved = unsaved.unsaved(checkout).await;
            let _ = tx.send(Input::Checked { agent: id, unsaved });
        });
    }

    /// Settles an agent auto-settle checked, if it still may settle now
    /// that its worktree's state is known.
    pub(super) fn checked(&mut self, id: AgentId, unsaved: Option<bool>) {
        if !self.inbox.checking.remove(&id) {
            return;
        }
        let Some(agent) = self.hub.reported(id) else {
            return;
        };
        if !self.may_auto_settle(&agent, unsaved) {
            return;
        }
        let mut next = self.hub.inbox(id);
        inbox::settle(&mut next, inbox::auto_settled(&agent));
        self.write_inbox(id, next);
        self.stop_settled(id);
    }

    /// Whether auto-settle settles the agent now, given what's known of its
    /// worktree. On top of the shared rules, an agent with a message
    /// waiting, a sub-agent at work or an enabled task that reaches it
    /// stays, and a snooze that ended counts as activity, so an agent that
    /// just woke stays in view.
    fn may_auto_settle(&self, agent: &Agent, unsaved: Option<bool>) -> bool {
        let now = self.clock.wall();
        let mut quiet = agent.clone();
        let snooze_ended = agent.inbox.snoozed_until.filter(|until| *until <= now);
        quiet.last_activity_at = [
            Some(agent.last_activity_at),
            agent.inbox.woke_at,
            snooze_ended,
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(agent.last_activity_at);
        let days = self.settle_days(agent);
        inbox::auto_settles(&quiet, days, unsaved, now)
            && self.busy(agent).is_none()
            && !self.automated(agent.id)
    }

    /// The days auto-settle waits for the agent: its project's setting on
    /// this device when it has one, and the device's otherwise.
    fn settle_days(&self, agent: &Agent) -> u16 {
        let own = self.project_settings(agent);
        own.and_then(|settings| settings.auto_settle_days)
            .unwrap_or_else(|| self.device_settings().auto_settle_days)
    }

    /// The agent's project's settings on this device, if it has any.
    pub(super) fn project_settings(&self, agent: &Agent) -> Option<ProjectSettings> {
        self.workspace_settings(agent.workspace)
    }

    /// The settings on this device of the workspace's project, if it has
    /// any.
    pub(super) fn workspace_settings(&self, workspace: WorkspaceId) -> Option<ProjectSettings> {
        let Some(Entity::Workspace(workspace)) = self.hub.get(&EntityKey::Workspace(workspace))
        else {
            return None;
        };
        let key = EntityKey::ProjectSettings {
            project: workspace.project?,
            device: self.device,
        };
        match self.hub.get(&key) {
            Some(Entity::ProjectSettings(settings)) => Some(settings),
            _ => None,
        }
    }

    /// The device's settings, or the defaults until they're stored.
    pub(super) fn device_settings(&self) -> DeviceSettings {
        match self.hub.get(&EntityKey::DeviceSettings(self.device)) {
            Some(Entity::DeviceSettings(settings)) => settings,
            _ => hub::default_settings(self.hub.device()),
        }
    }

    /// The checkout to check for the agent's unsaved work, or none when
    /// there's none to check, which counts as unsaved.
    fn checkout(&self, agent: &Agent) -> Option<Checkout> {
        if let Some(held) = self.held.get(&agent.id) {
            return Some(match held.guest {
                Some(workspace) => Checkout::Container {
                    workspace,
                    dir: held.cwd.to_string_lossy().into_owned(),
                },
                None => Checkout::Folder(held.cwd.clone()),
            });
        }
        match self.hub.get(&EntityKey::Workspace(agent.workspace)) {
            Some(Entity::Workspace(workspace)) => match workspace.location {
                WorkspaceLocation::Folder { path } => Some(Checkout::Folder(path.into())),
                WorkspaceLocation::Container { path, .. } => self
                    .workspaces
                    .container(workspace.id)
                    .map(|_| Checkout::Container {
                        workspace: workspace.id,
                        dir: path,
                    }),
            },
            _ => None,
        }
    }
}

/// Whether the agent is in a turn: starting, working or waiting on you.
pub(super) fn in_turn(activity: &Activity) -> bool {
    is_busy(activity) || *activity == Activity::Starting
}

/// When the usage limit an activity waits on resets, if it's known.
fn limit_reset(activity: &Activity) -> Option<Timestamp> {
    match activity {
        Activity::WaitingForLimitReset { resets_at } => *resets_at,
        _ => None,
    }
}
