// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Stopping the CLI of an agent left idle for the time this device's
//! settings give, to free the memory it holds. The agent stays as it was:
//! same place, timeline and inbox. Whatever needs the CLI next starts it
//! again on the same session, as after any stop: `--resume` for Claude,
//! `session/load` for an ACP agent, and a thread resume for Codex, whose
//! app-server stops once none of its threads is in use.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use maxhq_protocol::ids::{AccountId, AgentId, SessionId};
use maxhq_protocol::model::{LoginKind, Vendor};
use maxhq_protocol::time::Timestamp;
use tracing::info;

use super::held::{ServerKey, Target};
use super::recovery::continue_message;
use super::revive::Cut;
use super::vendors::one_process_per_agent;
use super::Manager;
use crate::drive::claude::same_server;
use crate::drive::{Action, Effects};

/// How often idle agents are looked for.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// What the manager keeps to find idle agents.
#[derive(Default)]
pub(super) struct Idle {
    /// When to look next.
    next_check: Option<Instant>,
    /// When each agent was last kept running by something its last
    /// activity doesn't show, such as an open pane. Its idle time counts
    /// from then too.
    kept: HashMap<AgentId, Timestamp>,
}

impl Idle {
    /// When to look for idle agents next.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.next_check
    }
}

impl Manager {
    /// Stops the CLI of each agent idle for longer than the device's
    /// setting allows, then each Codex app-server none of whose threads is
    /// in use. It looks once a minute, and lets go of local agents' CLIs
    /// whose model server changed at the same time.
    pub(super) async fn release_idle(&mut self) {
        let now = self.clock.now();
        if self.idle.next_check.is_some_and(|at| at > now) {
            return;
        }
        self.idle.next_check = Some(now + CHECK_EVERY);
        if self.winding.is_some() {
            return;
        }
        self.release_changed_servers(None).await;
        let minutes = self.device_settings().idle_release_minutes;
        if minutes == 0 {
            return;
        }
        let timeout = u64::from(minutes) * 60_000;
        let wall = self.clock.wall();
        self.idle
            .kept
            .retain(|agent, _| self.held.contains_key(agent));
        let leads: Vec<AgentId> = self.held.keys().copied().collect();
        for lead in leads {
            let Some(session) = self.running_hold(lead) else {
                continue;
            };
            if self.kept(lead, session) {
                self.idle.kept.insert(lead, wall);
                continue;
            }
            let kept = self.idle.kept.get(&lead).copied().unwrap_or(Timestamp(0));
            let since = self
                .held
                .get(&lead)
                .map_or(kept, |held| held.agent.last_activity_at.max(kept));
            if since.0.saturating_add(timeout) <= wall.0 {
                self.release(lead, session).await;
            }
        }
        self.stop_unused_servers(None);
    }

    /// Lets go of the CLIs this device runs for `account`'s agents, after
    /// you signed in to it again, so each starts again on the new login
    /// with its next message. Whatever keeps a CLI running keeps it, but
    /// an open pane doesn't, since you signed in from there.
    pub(super) async fn signed_in(&mut self, account: AccountId) {
        if self.winding.is_some() {
            return;
        }
        let leads: Vec<AgentId> = self
            .held
            .iter()
            .filter(|(_, held)| held.account == account && held.guest.is_none())
            .map(|(lead, _)| *lead)
            .collect();
        for lead in leads {
            let Some(session) = self.running_hold(lead) else {
                continue;
            };
            if !self.in_use(lead, session) {
                self.release(lead, session).await;
            }
        }
        self.stop_unused_servers(Some(account));
    }

    /// Lets go of the CLIs of local agents whose model server changed since
    /// they started, only `account`'s when one is named, so each starts
    /// again on the server as it is now with its next message. A key a
    /// sign-in replaced, a server that turned cloud, a new context size or
    /// pick of model each count. A CLI an earlier run of `maxd` started
    /// counts as changed, since this run didn't see what it got, which is
    /// how an update's new settings reach it. One with something to do
    /// keeps running until a later look, but an open pane doesn't keep it.
    pub(super) async fn release_changed_servers(&mut self, account: Option<AccountId>) {
        if self.winding.is_some() {
            return;
        }
        let leads: Vec<AgentId> = self
            .held
            .iter()
            .filter(|(_, held)| {
                held.login == LoginKind::ModelServer
                    && held.guest.is_none()
                    && account.is_none_or(|account| held.account == account)
            })
            .map(|(lead, _)| *lead)
            .collect();
        for lead in leads {
            let Some(session) = self.running_hold(lead) else {
                continue;
            };
            let Some(account) = self.held.get(&lead).map(|held| held.account) else {
                continue;
            };
            // A server that isn't ready can't say what changed.
            let Some(now) = self.credentials.model_server(account) else {
                continue;
            };
            let same = self
                .holds
                .get(&session)
                .and_then(|hold| hold.served.as_ref())
                .is_some_and(|served| same_server(&served.server, &now, served.model.as_deref()));
            if same || self.in_use(lead, session) {
                continue;
            }
            info!(%lead, %session, "stopping a local agent's CLI that runs on its server's old settings");
            self.release(lead, session).await;
        }
    }

    /// The holder that runs an agent's CLI, while it runs and nothing
    /// else is starting, stopping or moving it.
    fn running_hold(&self, lead: AgentId) -> Option<SessionId> {
        let held = self.held.get(&lead)?;
        let settled = !held.released
            && held.paused.is_none()
            && held.fault.is_none()
            && !held.resuming
            && held.owed.is_none()
            && held.handoff.is_none()
            && (held.own_process() || !held.adopt)
            && !self.adopting.contains_key(&lead)
            && !self.limit_waits.contains_key(&lead)
            && held.guest.is_none_or(|guest| self.link_up(guest));
        let session = self.session_of(held).filter(|_| settled)?;
        let hold = self.holds.get(&session)?;
        let running = hold.writer.is_some()
            && hold.reaching.is_none()
            && hold.waiting.is_none()
            && hold.stranded.is_none()
            && !hold.stopping
            && !hold.relaunch
            && !hold.ending
            && !hold.restarted
            && !hold.release;
        running.then_some(session)
    }

    /// Whether something keeps an agent's CLI running for now: what
    /// [`Self::in_use`] names, or a pane you have open.
    fn kept(&self, lead: AgentId, session: SessionId) -> bool {
        let mut agents = self.subs_of(lead);
        agents.push(lead);
        let watched = self.clients.values().any(|viewer| {
            viewer.person && agents.iter().any(|agent| viewer.open.contains_key(agent))
        });
        watched || self.in_use(lead, session)
    }

    /// Whether an agent's CLI has something to do: a turn, a message on
    /// its way, a question for you, a session with nothing to resume yet,
    /// or work only the CLI holds.
    fn in_use(&self, lead: AgentId, session: SessionId) -> bool {
        let (Some(held), Some(hold)) = (self.held.get(&lead), self.holds.get(&session)) else {
            return true;
        };
        let mut agents = self.subs_of(lead);
        agents.push(lead);
        let asking = self
            .attention
            .values()
            .any(|open| agents.contains(&open.agent));
        // An app-server's other threads have their own actions in flight,
        // which its driver tells apart.
        let in_flight = held.own_process() && self.routed.values().any(|routed| *routed == session);
        let resumable = held.vendor() != Vendor::Claude || self.has_transcript(lead);
        self.mid_turn(lead)
            || !held.queued.is_empty()
            || !hold.parked.is_empty()
            || asking
            || in_flight
            || !resumable
            || !hold.driver.can_release(lead)
    }

    /// Lets an idle agent's CLI go. A Claude or ACP agent's holder stops,
    /// and a Codex agent's thread is let go on its app-server.
    async fn release(&mut self, lead: AgentId, session: SessionId) {
        let Some(own) = self.held.get(&lead).map(super::held::Held::own_process) else {
            return;
        };
        if own {
            info!(%lead, %session, "stopping an idle agent's CLI to free its memory");
            if let Some(hold) = self.holds.get_mut(&session) {
                hold.release = true;
            }
            self.stop_hold(session);
            return;
        }
        info!(%lead, %session, "letting an idle Codex thread go to free its memory");
        let mut fx = Effects::default();
        if let Some(hold) = self.holds.get_mut(&session) {
            hold.driver.release(lead, &mut fx);
        }
        if let Some(held) = self.held.get_mut(&lead) {
            held.released = true;
        }
        self.save(lead);
        self.publish(lead);
        self.apply_fx(session, fx).await;
    }

    /// Stops each Codex app-server whose threads are all let go or not
    /// loaded, and none busy, only `account`'s when one is named. The next
    /// action for one of them starts it again and adopts the thread.
    fn stop_unused_servers(&mut self, account: Option<AccountId>) {
        let keys: Vec<ServerKey> = self
            .servers
            .keys()
            .filter(|key| account.is_none_or(|account| key.account == account))
            .copied()
            .collect();
        for key in keys {
            let Some(session) = self.servers.get(&key).and_then(|server| server.session) else {
                continue;
            };
            let running = self.holds.get(&session).is_some_and(|hold| {
                hold.writer.is_some()
                    && hold.reaching.is_none()
                    && hold.waiting.is_none()
                    && hold.stranded.is_none()
                    && !hold.stopping
            });
            let leads = self.codex_leads(key);
            let unused = !leads.is_empty()
                && leads.iter().all(|lead| {
                    self.held
                        .get(lead)
                        .is_some_and(|held| held.released || held.adopt || held.paused.is_some())
                });
            let reachable = key.workspace.is_none_or(|guest| self.link_up(guest));
            if !(running && unused && reachable && self.server_idle(key, session)) {
                continue;
            }
            info!(%session, "stopping a Codex app-server none of whose threads is in use");
            for lead in leads {
                if let Some(held) = self.held.get_mut(&lead)
                    && !held.released
                {
                    held.released = true;
                    self.save(lead);
                    self.publish(lead);
                }
            }
            if let Some(hold) = self.holds.get_mut(&session) {
                hold.release = true;
            }
            self.stop_hold(session);
        }
    }

    /// Takes what came for a holder while it stopped to free its memory,
    /// when it did. A Codex app-server's actions run again at once and
    /// start a new one. One lost on the way, which may still run, refuses
    /// them instead and counts as any lost holder.
    pub(super) fn take_release(
        &mut self,
        session: SessionId,
        stranded: bool,
    ) -> Option<Vec<(Target, Action)>> {
        let hold = self.holds.get_mut(&session).filter(|hold| hold.release)?;
        let parked = std::mem::take(&mut hold.parked);
        let vendor = hold.record.vendor;
        if stranded {
            self.refuse_parked(parked, "the agent's process ended");
            return None;
        }
        if one_process_per_agent(vendor) {
            return Some(parked);
        }
        self.work.extend(parked);
        Some(Vec::new())
    }

    /// An idle agent's CLI stopped. The agent stays as it was, and its CLI
    /// starts again for what came while it stopped, or for a turn that
    /// began meanwhile and was cut off. Otherwise it waits for the next
    /// message.
    pub(super) fn released(&mut self, cut: Cut, parked: Vec<(Target, Action)>) {
        let lead = cut.lead;
        let Some(held) = self.held.get_mut(&lead) else {
            self.refuse_parked(parked, "the agent's process ended");
            return;
        };
        held.session = None;
        held.released = true;
        if cut.busy {
            held.owed = Some(continue_message(&cut.items));
        }
        if cut.busy || !parked.is_empty() {
            self.resume_agent(lead);
            self.work.extend(parked);
            return;
        }
        self.save(lead);
        self.publish(lead);
    }

    /// Starts a released Claude or ACP agent's CLI again for an action that
    /// needs it, which waits for the CLI. Gives back an action it can't
    /// run that way.
    pub(super) fn wake_released(
        &mut self,
        lead: AgentId,
        target: Target,
        action: Action,
    ) -> Option<Action> {
        let wakes = !matches!(action, Action::Stop { .. })
            && self.held.get(&lead).is_some_and(|held| {
                held.released
                    && held.paused.is_none()
                    && held.fault.is_none()
                    && held.own_process()
                    && held.session.is_none()
            });
        if !wakes {
            return Some(action);
        }
        self.resume_agent(lead);
        if self
            .held
            .get(&lead)
            .is_some_and(|held| held.session.is_some())
        {
            self.work.push_front((target, action));
            return None;
        }
        Some(action)
    }

    /// A released Codex agent's thread is wanted again, so it no longer
    /// counts as let go.
    pub(super) fn codex_wanted(&mut self, target: Target, action: &Action) {
        let lead = match (target, action) {
            (Target::Agent(agent), _) => self.lead_of(agent),
            (_, Action::Adopt { agent, .. }) => *agent,
            (
                _,
                Action::Start {
                    agent: Some(agent), ..
                },
            ) => *agent,
            _ => return,
        };
        if let Some(held) = self.held.get_mut(&lead)
            && held.released
        {
            held.released = false;
            self.save(lead);
            self.publish(lead);
        }
    }
}

/// Keeps an agent as it was when its idle CLI stopped: the end only
/// reports the process gone, which the agent doesn't show.
pub(super) fn quiet_exit(fx: &mut Effects) {
    fx.agents.retain(|agent| agent.parent.is_some());
}
