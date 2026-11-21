// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Running the drivers: handing each action to the driver it's for, and
//! carrying out the effects a driver asks for.

use std::time::Duration;

use maxhq_protocol::attention::AnswerOutcome;
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, IdempotencyKey, SessionId};
use maxhq_protocol::message::{ActionOutcome, AgentPlace, AgentSpec, ServerMessage};
use maxhq_protocol::model::{Activity, Agent, PauseReason, PermissionMode, Vendor};
use maxhq_protocol::state::{Change, Entity, EntityKey, Seq};
use maxhq_protocol::timeline::{ItemBody, TimelineEvent, TimelineItem, TimelineUpdate};
use tracing::{info, warn};

use super::held::{
    internal_key, is_busy, After, Held, Internal, Open, Pending, ServerKey, Target, Written,
};
use super::holders::WriteOutcome;
use super::idem::Outcome;
use super::loading::Later;
use super::recovery::Restarts;
use super::vendors::one_process_per_agent;
use super::{refused, wake_when_read, Input, Manager, MAX_STRAYS};
use crate::drive::{Action, ActionKey, AttentionChange, Effects, Write};

/// How long one write to a CLI may take before it counts as failed.
const WRITE_LIMIT: Duration = Duration::from_secs(5);

/// How long a CLI asked to stop gets before the holder kills it.
pub(super) const STOP_GRACE: Duration = Duration::from_secs(5);

impl Manager {
    /// Hands one action to the driver it's for.
    pub(super) async fn dispatch(&mut self, target: Target, action: Action) {
        let key = action.key();
        let lead = match target {
            Target::Codex(server) => return self.codex(server, target, action).await,
            Target::Agent(agent) => self.lead_of(agent),
        };
        // An agent whose timeline is still read after a restart gets it
        // once that's in, and is read next.
        if self.loading.waits(lead) {
            self.hurry(lead);
            self.wait_for(lead, Later::Dispatch(target, Box::new(action)));
            return;
        }
        let Some(held) = self.held.get(&lead) else {
            self.result(key, ActionOutcome::UnknownAgent, Written::default());
            return;
        };
        match held.vendor() {
            vendor if one_process_per_agent(vendor) => match self.session_of(held) {
                Some(session) => {
                    if let Some(action) = self.handed(lead, action) {
                        self.act(session, target, action).await;
                    }
                }
                None => {
                    if self.wake_released(lead, target, action).is_some() {
                        self.result(key, refused("the agent isn't running"), Written::default());
                    }
                }
            },
            _ => {
                let server = held.server_key();
                if held.adopt && !matches!(action, Action::Adopt { .. }) {
                    self.adopt_first(lead, target, action);
                } else if let Some(action) = self.handed(lead, action) {
                    self.codex(server, target, action).await;
                }
            }
        }
    }

    /// An adopt for a held Codex agent's thread, or a new thread for one
    /// that moved to Codex and has none yet.
    fn adopt_action(&self, lead: AgentId) -> Option<(ServerKey, Action)> {
        let held = self.held.get(&lead)?;
        let key = internal_key();
        let place = self.place(held);
        let action = if held.vendor_session.is_empty() {
            let spec = AgentSpec {
                effort: None,
                vendor: Vendor::Codex,
                account: held.account,
                place: AgentPlace::Folder {
                    path: held.cwd.to_string_lossy().into_owned(),
                },
                model: held.model.clone(),
                permission: Some(held.mode),
                title: held.title.clone(),
                first: None,
                branch: None,
                origin: None,
            };
            Action::Start {
                key,
                place,
                spec: Box::new(spec),
                first: None,
                agent: Some(lead),
            }
        } else {
            Action::Adopt {
                key,
                place,
                agent: lead,
                thread: held.vendor_session.clone(),
            }
        };
        Some((held.server_key(), action))
    }

    /// Runs `action` once the Codex agent's thread is adopted, adopting it
    /// first if nothing does yet.
    fn adopt_first(&mut self, lead: AgentId, target: Target, action: Action) {
        if let Some(key) = self.adopting.get(&lead) {
            self.followups
                .entry(*key)
                .or_default()
                .push((target, action));
            return;
        }
        let Some((server, adopt)) = self.adopt_action(lead) else {
            self.result(
                action.key(),
                ActionOutcome::UnknownAgent,
                Written::default(),
            );
            return;
        };
        let key = adopt.key();
        self.internal.insert(key, Internal::Adopt(lead));
        self.adopting.insert(lead, key);
        let mut followups = self.settings_after_adopt(lead);
        followups.push((target, action));
        self.followups.insert(key, followups);
        self.work.push_front((Target::Codex(server), adopt));
    }

    /// Adopts a held Codex agent's thread, unless an adopt is already on
    /// its way.
    pub(super) fn adopt_now(&mut self, lead: AgentId) {
        if self.adopting.contains_key(&lead) {
            return;
        }
        let Some((server, adopt)) = self.adopt_action(lead) else {
            return;
        };
        let key = adopt.key();
        self.internal.insert(key, Internal::Adopt(lead));
        self.adopting.insert(lead, key);
        let followups = self.settings_after_adopt(lead);
        if !followups.is_empty() {
            self.followups.insert(key, followups);
        }
        self.work.push_back((Target::Codex(server), adopt));
    }

    /// The held Codex agent's mode and model, to set again once its thread
    /// is adopted. An app-server that never ran the thread knows neither,
    /// and would run it in the account's own defaults.
    fn settings_after_adopt(&mut self, lead: AgentId) -> Vec<(Target, Action)> {
        let Some(held) = self.held.get(&lead) else {
            return Vec::new();
        };
        let (mode, model, effort) = (held.mode, held.model.clone(), held.effort.clone());
        self.set_mode_and_model(lead, mode, model, effort)
    }

    /// Internal actions that set a Codex agent's mode, model and effort
    /// level.
    pub(super) fn set_mode_and_model(
        &mut self,
        agent: AgentId,
        mode: PermissionMode,
        model: Option<String>,
        effort: Option<String>,
    ) -> Vec<(Target, Action)> {
        let mut actions = Vec::new();
        // Codex has no plan mode, so no Codex agent is in it.
        if mode != PermissionMode::Plan {
            actions.push(Action::SetPermission {
                key: internal_key(),
                agent,
                mode,
            });
        }
        if let Some(model) = model {
            actions.push(Action::SetModel {
                key: internal_key(),
                agent,
                model,
            });
        }
        if effort.is_some() {
            actions.push(Action::SetEffort {
                key: internal_key(),
                agent,
                effort,
            });
        }
        actions
            .into_iter()
            .map(|action| {
                self.internal.insert(action.key(), Internal::Plain);
                (Target::Agent(agent), action)
            })
            .collect()
    }

    /// Hands an action to a Codex app-server, starting it if it isn't
    /// running.
    async fn codex(&mut self, server_key: ServerKey, target: Target, action: Action) {
        // A thread that loads waits for its folder's check first.
        let Some(action) = self.check_codex(target, action) else {
            return;
        };
        let key = action.key();
        self.codex_wanted(target, &action);
        // An adopt that runs ahead of your message counts as yours.
        let ours = self.internal.contains_key(&key)
            && !self.followups.get(&key).is_some_and(|followups| {
                followups
                    .iter()
                    .any(|(_, followup)| !self.internal.contains_key(&followup.key()))
            });
        let Some(server) = self.servers.get_mut(&server_key) else {
            self.result(
                key,
                refused("Codex isn't set up for this account"),
                Written::default(),
            );
            return;
        };
        if let Some(reason) = &server.failed {
            // Only something you asked for tries again after Codex crashed
            // too often or couldn't be confirmed as ended.
            if ours {
                let reason = reason.clone();
                self.result(key, refused(reason), Written::default());
                return;
            }
            server.failed = None;
            server.restarts = Restarts::default();
        }
        if server.restart_at.is_some() {
            server.waiting.push((target, action));
            return;
        }
        match self.ensure_server(server_key, false) {
            Ok(session) => self.act(session, target, action).await,
            Err(reason) => self.result(key, refused(reason), Written::default()),
        }
    }

    /// Hands an action to a holder's driver. While the holder is being
    /// started or connected to, the action waits for it, in order.
    async fn act(&mut self, session: SessionId, target: Target, action: Action) {
        let key = action.key();
        let Some(hold) = self.holds.get_mut(&session) else {
            self.result(key, refused("the agent isn't running"), Written::default());
            return;
        };
        // One stopping to free its memory starts again for it.
        if hold.reaching.is_some() || hold.release {
            hold.parked.push((target, action));
            return;
        }
        let mut fx = Effects::default();
        hold.driver.act(action, &mut fx);
        if !fx.results.iter().any(|(done, _)| *done == key) {
            self.routed.insert(key, session);
        }
        self.apply_fx(session, fx).await;
    }

    /// Carries out what a holder's driver asked for, in order.
    pub(super) async fn apply_fx(&mut self, session: SessionId, mut fx: Effects) {
        self.settle(&mut fx);
        let written = self.write(session, std::mem::take(&mut fx.writes)).await;
        for picture in fx.pictures.guest {
            self.places.keep_picture(picture);
        }
        for (agent, items) in fx.history {
            // A thread's history never empties, so an empty one is a read
            // that failed, as Codex's can after a restart. The timeline
            // keeps what it has.
            if items.is_empty()
                && self
                    .timelines
                    .get(&agent)
                    .is_some_and(|timeline| !timeline.is_empty())
            {
                continue;
            }
            // What the driver read is newer than a read still on its way.
            self.reads.remove(&agent);
            self.timeline_mut(agent).replace_history(items);
            self.reset_viewers(agent);
        }
        // A new sub-agent comes with its first items, which need it known
        // first. A lead's own update waits, so its items count as progress
        // against the activity it had before them.
        let (subs, leads): (Vec<Agent>, Vec<Agent>) = fx
            .agents
            .into_iter()
            .partition(|agent| agent.parent.is_some());
        for agent in subs {
            self.agent_update(agent);
        }
        self.timeline_events(fx.timeline);
        self.make_previews(fx.pictures.previews);
        for agent in leads {
            self.agent_update(agent);
        }
        for agent in fx.gone {
            self.gone(agent);
        }
        for change in fx.attention {
            self.attention_change(change);
        }
        for (account, usage) in fx.usage {
            self.hub.set_usage(account, usage);
        }
        for (account, models) in fx.models {
            self.hub.set_models(account, models);
        }
        for (account, features) in fx.features {
            crate::acp_agents::found(account, features);
        }
        for (key, outcome) in fx.results {
            self.result(key, outcome, written);
        }
        for call in fx.calls {
            self.run_tool(session, call);
        }
        self.run_jobs(session, fx.jobs);
        if !fx.command_rules.is_empty() {
            self.save_command_rules(session, fx.command_rules);
        }
        if !fx.allowed_edits.is_empty() {
            self.save_allowed_edits(session, fx.allowed_edits);
        }
        if let Some(at) = fx.wake_at
            && let Some(hold) = self.holds.get_mut(&session)
        {
            hold.wake_at = Some(hold.wake_at.map_or(at, |old| old.min(at)));
        }
        if fx.stop {
            self.stop_hold(session);
        }
    }

    /// Makes the agents that started or adopted actions created, before
    /// the rest of the effects refer to them.
    fn settle(&mut self, fx: &mut Effects) {
        for (key, outcome) in &fx.results {
            let started = match outcome {
                ActionOutcome::Started { agent } => Some(*agent),
                ActionOutcome::Done => None,
                _ => continue,
            };
            let Some(pending) = self.pending.remove(key) else {
                continue;
            };
            match (pending, started) {
                (Pending::Start(info), Some(id)) => {
                    let reported = fx
                        .agents
                        .iter()
                        .find(|agent| agent.id == id)
                        .cloned()
                        .or_else(|| self.strays.remove(&id));
                    let agent = reported.unwrap_or_else(|| {
                        self.placeholder(id, Vendor::Codex, &info, None, Activity::Starting)
                    });
                    let thread = agent.vendor_session_id.clone().unwrap_or_default();
                    self.held.insert(id, Held::new(agent, *info, thread));
                    self.timeline_mut(id);
                    self.save(id);
                    self.publish(id);
                }
                (Pending::Adopt(mut held), _) => {
                    let id = held.agent.id;
                    held.adopt = false;
                    held.resuming = false;
                    if let Some(agent) = fx.agents.iter().find(|agent| agent.id == id) {
                        held.agent = agent.clone();
                    } else if let Some(agent) = self.strays.remove(&id) {
                        held.agent = agent;
                    }
                    self.outside.remove(&id);
                    self.held.insert(id, *held);
                    self.timeline_mut(id);
                    self.after_adopt(id);
                }
                (pending, None) => {
                    // A start answered with done names no agent, so it
                    // fails in `result` like any other start.
                    self.pending.insert(*key, pending);
                }
            }
        }
    }

    /// Writes lines to a holder's CLI, one at a time.
    async fn write(&mut self, session: SessionId, writes: Vec<Write>) -> Written {
        let mut written = Written::default();
        if writes.is_empty() {
            return written;
        }
        let Some(writer) = self
            .holds
            .get(&session)
            .and_then(|hold| hold.writer.clone())
        else {
            written.failed = true;
            return written;
        };
        for write in writes {
            match tokio::time::timeout(WRITE_LIMIT, writer.write(write.line, write.answers)).await {
                Ok(Ok(WriteOutcome::Written)) => {}
                Ok(Ok(WriteOutcome::AlreadyAnswered)) => written.already = true,
                Ok(Err(err)) => {
                    warn!(%err, %session, "couldn't write to the agent's process");
                    written.failed = true;
                }
                Err(_) => {
                    warn!(%session, "writing to the agent's process took too long");
                    written.failed = true;
                }
            }
        }
        written
    }

    /// Applies timeline events and sends them to the clients that have
    /// the timeline open.
    pub(super) fn timeline_events(&mut self, events: Vec<(AgentId, TimelineEvent)>) {
        let mut before: Vec<(AgentId, Seq)> = Vec::new();
        for (agent, event) in events {
            if !before.iter().any(|(seen, _)| *seen == agent) {
                let seq = self.timeline_mut(agent).seq();
                before.push((agent, seq));
            }
            let lead = self.lead_of(agent);
            // Only work done during a turn counts as progress. A command an
            // interrupt didn't stop can finish long after its turn ended.
            let busy = self
                .held
                .get(&lead)
                .is_some_and(|held| is_busy(&held.agent.activity));
            // Your messages are stored with the agent while they're queued,
            // so a restart doesn't lose them.
            let message = |item: &TimelineItem| matches!(item.body, ItemBody::UserMessage { .. });
            let queue = agent == lead
                && match &event {
                    TimelineEvent::Upsert { item } => message(item),
                    TimelineEvent::Remove { id } => self
                        .timelines
                        .get(&agent)
                        .and_then(|timeline| timeline.get(id))
                        .is_some_and(|item| message(&item)),
                    TimelineEvent::Append { .. } => false,
                };
            if queue && self.held.contains_key(&lead) {
                self.mark_dirty(lead);
            }
            let command = match &event {
                TimelineEvent::Upsert { item } if matches!(item.body, ItemBody::Command { .. }) => {
                    Some(item.id.clone())
                }
                _ => None,
            };
            let finished = self.timeline_mut(agent).apply(event);
            if finished == Some(true)
                && let Some(item) = &command
            {
                self.pull_command(agent, item);
            }
            if finished == Some(true)
                && busy
                && let Some(held) = self.held.get_mut(&lead)
                && !held.resumes.progress
            {
                held.resumes.progress = true;
                self.mark_dirty(lead);
            }
        }
        for (agent, since) in before {
            self.broadcast(agent, since);
        }
    }

    /// Sends what changed on a timeline since `since` to each client that
    /// has it open. One still read after a restart reaches them whole once
    /// it's in.
    pub(super) fn broadcast(&mut self, agent: AgentId, since: Seq) {
        if self.loading.waits(agent) {
            return;
        }
        let Some(timeline) = self.timelines.get(&agent) else {
            return;
        };
        if timeline.seq() == since {
            return;
        }
        let events = timeline.since(since).map(|events| TimelineUpdate::Events {
            seq: timeline.seq(),
            events,
        });
        for (conn, viewer) in &mut self.clients {
            let Some(limit) = viewer.open.get(&agent) else {
                continue;
            };
            let update = events.clone().unwrap_or_else(|| timeline.reset(*limit));
            if viewer.send(ServerMessage::Timeline { agent, update }) {
                wake_when_read(*conn, viewer, &self.tx);
            }
        }
    }

    /// Sends every client that has the timeline open a fresh copy of it.
    pub(super) fn reset_viewers(&mut self, agent: AgentId) {
        let Some(timeline) = self.timelines.get(&agent) else {
            return;
        };
        for (conn, viewer) in &mut self.clients {
            if let Some(limit) = viewer.open.get(&agent) {
                let update = timeline.reset(*limit);
                if viewer.send(ServerMessage::Timeline { agent, update }) {
                    wake_when_read(*conn, viewer, &self.tx);
                }
            }
        }
    }

    /// Takes in an agent a driver reported.
    fn agent_update(&mut self, agent: Agent) {
        if let Some(parent) = agent.parent {
            let lead = self.lead_of(parent);
            if self.held.contains_key(&lead) {
                self.parents.insert(agent.id, lead);
                self.subs.insert(agent.id, agent);
                self.publish(lead);
            }
            return;
        }
        let id = agent.id;
        let Some(held) = self.held.get_mut(&id) else {
            if self.strays.len() >= MAX_STRAYS {
                self.strays.clear();
            }
            self.strays.insert(id, agent);
            return;
        };
        // A late word from the vendor the agent moved away from.
        if agent.vendor != held.vendor() {
            return;
        }
        // The CLI can change its own mode, as Claude does when it leaves
        // plan mode, and a resume runs in the mode it's in.
        if let Some(permission) = &agent.permission {
            held.mode = permission.requested;
        }
        if let Some(session) = agent.vendor_session_id.as_ref().filter(|s| !s.is_empty()) {
            held.vendor_session.clone_from(session);
        }
        // A limit wait that a restart cuts across resumes after the reset.
        held.resume_after = match &agent.activity {
            Activity::WaitingForLimitReset { resets_at } => *resets_at,
            _ => None,
        };
        held.agent = agent;
        self.mark_dirty(id);
        self.publish(id);
    }

    /// An agent a driver says no longer exists.
    fn gone(&mut self, agent: AgentId) {
        self.strays.remove(&agent);
        if let Some(lead) = self.parents.remove(&agent) {
            self.subs.remove(&agent);
            self.pace.forget(agent);
            self.hub.apply_held([Change::Remove {
                key: EntityKey::Agent(agent),
            }]);
            self.publish(lead);
            return;
        }
        if self.held.contains_key(&agent) {
            self.forget_agent(agent);
        }
    }

    /// Stops holding a lead agent and its sub-agents.
    pub(super) fn forget_agent(&mut self, lead: AgentId) {
        let mut agents = self.subs_of(lead);
        agents.push(lead);
        let events: Vec<AttentionId> = self
            .attention
            .iter()
            .filter(|(_, open)| agents.contains(&open.agent))
            .map(|(id, _)| *id)
            .collect();
        for event in events {
            self.close_attention(event);
        }
        for agent in &agents {
            self.parents.remove(agent);
            self.subs.remove(agent);
            self.pace.forget(*agent);
            self.forget_cards(*agent);
        }
        self.unhold(lead);
        self.adopting.remove(&lead);
        self.forget_inbox(lead);
        self.owner_words.forget(lead);
        self.hub
            .apply_held(agents.into_iter().map(|agent| Change::Remove {
                key: EntityKey::Agent(agent),
            }));
        self.forget_chat(lead);
    }

    /// Forgets lead agents with their sub-agents, as
    /// [`Manager::forget_agent`] does, along with their restart flags and
    /// waits. Returns every agent forgotten, whose timelines go once no
    /// client has them open.
    pub(super) fn forget_leads(&mut self, leads: &[AgentId]) -> Vec<AgentId> {
        let mut agents = Vec::new();
        for lead in leads {
            agents.extend(self.subs_of(*lead));
            agents.push(*lead);
        }
        for lead in leads {
            self.forget_agent(*lead);
            self.drop_flag(*lead);
            self.limit_waits.remove(lead);
            self.idle_ends.remove(lead);
        }
        agents
    }

    /// Deletes a lead agent and its sub-agents for good. It's forgotten,
    /// a CLI that ran for it alone ends, its timelines close for every
    /// client, its attachments go, and the hub drops what observers report
    /// of it from now on. Its folder and its CLI's transcript stay.
    pub(super) fn delete_lead(&mut self, lead: AgentId) {
        let Some(held) = self.held.get(&lead) else {
            return;
        };
        let own = held
            .session
            .filter(|_| one_process_per_agent(held.vendor()));
        let agents = self.forget_leads(&[lead]);
        if let Some(session) = own {
            self.stop_hold(session);
        }
        for viewer in self.clients.values_mut() {
            viewer.open.retain(|agent, _| !agents.contains(agent));
        }
        self.drop_unwatched(agents.iter().copied());
        if let Err(err) = self.db.mark_deleted(&agents, self.clock.wall()) {
            warn!(%err, agent = %lead, "couldn't store that the agent was deleted");
        }
        self.hub.hide_deleted(agents);
        let attachments = self.attachments.agent_dir(&lead.to_string());
        tokio::task::spawn_blocking(move || match maxhq_transfer::remove_tree(&attachments) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => warn!(%err, agent = %lead, "couldn't delete the agent's attachments"),
        });
        info!(agent = %lead, "deleted the agent");
    }

    /// Drops a lead agent's record, here and as stored, with its session
    /// key.
    pub(super) fn unhold(&mut self, lead: AgentId) {
        self.terminals.end_agent(lead);
        if let Some(mut held) = self.held.remove(&lead) {
            self.drop_agent_key(&held);
            self.forget_env_card(held.workflow_env.take());
        }
        self.forget_task_cards(lead);
        self.dirty.remove(&lead);
        self.forget_restricted(lead);
        if let Err(err) = self.db.remove_held(lead) {
            warn!(%err, agent = %lead, "couldn't forget the agent");
        }
    }

    /// Opens or closes an attention event a driver reported.
    fn attention_change(&mut self, change: AttentionChange) {
        match change {
            AttentionChange::Open(event) => {
                self.attention.insert(
                    event.id,
                    Open {
                        agent: event.agent,
                        ours: false,
                        expires_at: event.expires_at,
                    },
                );
                self.send_waiting(event.agent);
                self.hub.apply_held([Change::Upsert {
                    value: Entity::Attention(event),
                }]);
            }
            AttentionChange::Close(event) => self.close_attention(event),
        }
    }

    /// Finishes an action: tells its waiters, runs what follows it and
    /// records what it changed.
    pub(super) fn result(&mut self, key: IdempotencyKey, outcome: ActionOutcome, written: Written) {
        self.routed.remove(&key);
        if let Some((event, by)) = self.answering.remove(&key) {
            let answer = self.answered(key, event, by, &outcome, written);
            self.finish(key, Outcome::Answer(answer));
            return;
        }
        let outcome = match outcome {
            ActionOutcome::Done | ActionOutcome::Started { .. } if written.failed => {
                refused("couldn't reach the agent's process")
            }
            outcome => outcome,
        };
        let ok = matches!(outcome, ActionOutcome::Done | ActionOutcome::Started { .. });
        self.codex_loaded(key, &outcome, ok);

        if let Some(followups) = self.followups.remove(&key) {
            if ok {
                for followup in followups.into_iter().rev() {
                    self.work.push_front(followup);
                }
            } else {
                for (_, action) in followups {
                    self.result(action.key(), outcome.clone(), Written::default());
                }
            }
        }

        if let Some(internal) = self.internal.remove(&key) {
            match internal {
                Internal::Adopt(lead) => {
                    self.adopting.remove(&lead);
                    if ok {
                        self.after_adopt(lead);
                    } else if let Some(held) = self.held.get_mut(&lead) {
                        if let ActionOutcome::Refused { reason } = &outcome {
                            held.fault = Some(reason.clone());
                        }
                        held.resuming = false;
                        self.save(lead);
                        self.publish(lead);
                    }
                }
                Internal::Owed(lead) => {
                    if ok && let Some(held) = self.held.get_mut(&lead) {
                        held.owed = None;
                        self.save(lead);
                    }
                }
                Internal::Plain => {
                    if !ok {
                        warn!(?outcome, "an action maxd sent on its own failed");
                    }
                }
                // The agent stays settled either way.
                Internal::Stop(lead) => {
                    if ok {
                        self.record_after(lead, After::Stopped);
                    } else {
                        warn!(?outcome, agent = %lead, "couldn't stop the settled agent");
                    }
                }
                Internal::Reload(lead) => self.reloaded(lead, ok),
            }
            return;
        }

        let outcome = match self.after.remove(&key) {
            // A delete goes through even when the stop failed, and the
            // agent's own CLI ends with its holder.
            Some((lead, after @ After::Deleted)) => {
                self.record_after(lead, after);
                ActionOutcome::Done
            }
            Some((lead, after)) if ok => {
                self.record_after(lead, after);
                outcome
            }
            _ => outcome,
        };
        self.pending.remove(&key);
        self.finish(key, Outcome::Action(outcome));
    }

    /// Records what a confirmed action changed on the agent.
    fn record_after(&mut self, lead: AgentId, after: After) {
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        let mut stop = None;
        match after {
            After::Mode(mode) => held.mode = mode,
            After::Model(model) => held.model = Some(model),
            After::Effort(effort) => held.effort = effort,
            After::Stopped => {
                held.paused = Some(PauseReason::ByYou);
                held.resuming = false;
                held.owed = None;
                match held.vendor() {
                    vendor if one_process_per_agent(vendor) => stop = held.session,
                    _ => held.adopt = true,
                }
            }
            After::Sent => {
                self.message_sent(lead);
                return;
            }
            After::Deleted => {
                self.delete_lead(lead);
                return;
            }
        }
        if let Some(session) = stop {
            self.stop_hold(session);
        }
        self.save(lead);
        self.publish(lead);
    }

    /// How an answer to an attention event ended.
    fn answered(
        &mut self,
        key: IdempotencyKey,
        event: AttentionId,
        by: ClientId,
        outcome: &ActionOutcome,
        written: Written,
    ) -> AnswerOutcome {
        if let Some(row) = self.answers.get(&event) {
            return if row.key == key {
                AnswerOutcome::Accepted
            } else {
                AnswerOutcome::AlreadyAnswered {
                    answered_at: row.at,
                    answered_by: row.by,
                }
            };
        }
        if written.already {
            // Someone answered in the CLI's own terminal, or a holder
            // client this `maxd` doesn't know about.
            self.close_attention(event);
            return AnswerOutcome::AlreadyAnswered {
                answered_at: self.clock.wall(),
                answered_by: ClientId::derive("elsewhere"),
            };
        }
        if written.failed {
            return AnswerOutcome::Expired;
        }
        match outcome {
            ActionOutcome::Done | ActionOutcome::Started { .. } => {
                self.record_answer(event, key, by);
                self.close_attention(event);
                AnswerOutcome::Accepted
            }
            _ => AnswerOutcome::UnknownEvent,
        }
    }

    /// A Codex thread is loaded on the app-server again.
    pub(super) fn after_adopt(&mut self, lead: AgentId) {
        if let Some(held) = self.held.get_mut(&lead) {
            held.adopt = false;
            held.resuming = false;
            held.fault = None;
            held.down_since = None;
            self.save(lead);
            self.publish(lead);
        }
    }

    /// Asks a holder's CLI to stop. Its exit then isn't a crash.
    pub(super) fn stop_hold(&mut self, session: SessionId) {
        self.end_hold(session, true);
    }

    /// Ends a holder's CLI. An end that wasn't `asked` for, such as an
    /// app-server that never finished its handshake, counts as a crash.
    pub(super) fn end_hold(&mut self, session: SessionId, asked: bool) {
        let Some(hold) = self.holds.get_mut(&session) else {
            return;
        };
        hold.handshake_by = None;
        if asked {
            if hold.stopping {
                return;
            }
            hold.stopping = true;
        }
        if hold.reaching.is_some() {
            // It stops once it's reached.
            return;
        }
        let Some(writer) = hold.writer.clone() else {
            self.post(Input::Lost { session });
            return;
        };
        let grace = if asked { STOP_GRACE } else { Duration::ZERO };
        tokio::spawn(async move {
            if let Err(err) = writer.stop(grace).await {
                warn!(%err, %session, "couldn't stop the agent's process");
            }
        });
    }
}
