// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Taking over a session that runs in your terminal: MaxHQ stops its CLI
//! there, now or once its turn ends, then continues it as Continue here
//! does. A wait for the turn to end lasts only while `maxd` runs.

mod stop;

use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::message::{ActionOutcome, TakeOverMode};
use maxhq_protocol::model::{Activity, AgentLink, Control};
use maxhq_protocol::state::{Entity, EntityKey};

use super::held::internal_key;
use super::idem::Outcome;
use super::inbox::in_turn;
use super::requests::ELSEWHERE;
use super::{refused, Input, Manager};
use crate::hub;

const STOPPING: &str = "MaxHQ is already stopping the CLI in the terminal";

/// A take-over in progress.
pub(super) struct TakeOver {
    /// Answered once MaxHQ continues the session, or can't.
    key: IdempotencyKey,
    process: stop::Process,
    /// The CLI is being stopped, so the take-over can't be cancelled.
    stopping: bool,
}

impl Manager {
    /// Stops the CLI that runs an outside agent in your terminal and
    /// continues it here. After a turn, the answer comes once the wait
    /// starts, and the agent shows as taking over until it ends.
    pub(super) fn take_over(
        &mut self,
        key: IdempotencyKey,
        agent: AgentId,
        mode: TakeOverMode,
    ) -> Option<ActionOutcome> {
        if mode == TakeOverMode::Cancel {
            return Some(self.cancel_take_over(agent));
        }
        if let Some(waiting) = self.take_overs.get_mut(&agent) {
            if waiting.stopping {
                return Some(refused(STOPPING));
            }
            if mode == TakeOverMode::AfterTurn {
                return Some(ActionOutcome::Done);
            }
            waiting.key = key;
            waiting.stopping = true;
            let process = waiting.process;
            self.stop_terminal(agent, process);
            return None;
        }
        if self.held.contains_key(&self.lead_of(agent)) {
            return self.continue_here(key, agent);
        }
        let Some(Entity::Agent(found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return Some(ActionOutcome::UnknownAgent);
        };
        if found.control != Control::Observed {
            return Some(refused(
                "MaxHQ can only take over a session from your terminal",
            ));
        }
        // A session whose CLI already ended needs no stop.
        let Some(pid) = found.pid.filter(|_| found.link == AgentLink::Live) else {
            return self.continue_stopped(key, agent);
        };
        let cli = self.vendors.terminal_cli(found.vendor);
        if cli.is_empty() {
            return Some(refused("MaxHQ can't take over this agent's sessions"));
        }
        // Checked before the CLI stops, so a session MaxHQ couldn't
        // continue keeps running.
        if let Err(reason) = self.outside_start(found) {
            return Some(refused(reason));
        }
        let process = match stop::find(pid, cli) {
            Ok(process) => process,
            Err(reason) if reason == stop::GONE => return self.continue_stopped(key, agent),
            Err(reason) => return Some(refused(reason)),
        };
        let busy = self
            .hub
            .reported(agent)
            .is_some_and(|agent| in_turn(&agent.activity));
        if mode == TakeOverMode::AfterTurn && busy {
            let waiting = TakeOver {
                key: internal_key(),
                process,
                stopping: false,
            };
            self.take_overs.insert(agent, waiting);
            self.hub.set_taking_over(agent, true);
            return Some(ActionOutcome::Done);
        }
        let stopping = TakeOver {
            key,
            process,
            stopping: true,
        };
        self.take_overs.insert(agent, stopping);
        self.stop_terminal(agent, process);
        None
    }

    /// Stops waiting for a turn to end, unless the CLI is already stopping.
    fn cancel_take_over(&mut self, agent: AgentId) -> ActionOutcome {
        match self.take_overs.get(&agent) {
            Some(waiting) if waiting.stopping => refused(STOPPING),
            Some(_) => {
                self.take_overs.remove(&agent);
                self.hub.set_taking_over(agent, false);
                ActionOutcome::Done
            }
            None => ActionOutcome::Done,
        }
    }

    /// Stops the CLI once the turn MaxHQ waits on ends.
    pub(super) fn take_over_moved(&mut self, moved: &hub::Moved) {
        let Some(waiting) = self.take_overs.get_mut(&moved.agent) else {
            return;
        };
        if waiting.stopping || in_turn(&moved.to) {
            return;
        }
        waiting.stopping = true;
        let process = waiting.process;
        self.stop_terminal(moved.agent, process);
    }

    /// Stops the CLI off the loop, which can take the grace period and more.
    fn stop_terminal(&self, agent: AgentId, process: stop::Process) {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = stop::stop(process, stop::GRACE);
            let _ = tx.send(Input::TakenOver { agent, result });
        });
    }

    /// Continues the session once its CLI stopped, or says why it didn't.
    pub(super) fn taken_over(&mut self, agent: AgentId, result: Result<(), String>) {
        let Some(waiting) = self.take_overs.remove(&agent) else {
            return;
        };
        self.hub.set_taking_over(agent, false);
        let outcome = match result {
            Ok(()) => self.continue_stopped(waiting.key, agent),
            Err(reason) => Some(refused(reason)),
        };
        if let Some(outcome) = outcome {
            self.finish(waiting.key, Outcome::Action(outcome));
        }
    }

    /// Continues an outside session whose CLI no longer runs, though an
    /// observer may still show it running.
    fn continue_stopped(&mut self, key: IdempotencyKey, agent: AgentId) -> Option<ActionOutcome> {
        if self.held.contains_key(&self.lead_of(agent)) {
            return self.continue_here(key, agent);
        }
        let Some(Entity::Agent(mut found)) = self.hub.get(&EntityKey::Agent(agent)) else {
            return Some(ActionOutcome::UnknownAgent);
        };
        found.pid = None;
        found.taking_over = false;
        if !matches!(found.activity, Activity::Error { .. }) {
            found.activity = Activity::Idle;
        }
        let held = match self.outside_start(found) {
            Ok(held) => held,
            Err(reason) => return Some(refused(reason)),
        };
        if !self
            .vendors
            .can_continue(held.vendor(), &held.config_dir, &held.vendor_session)
        {
            return Some(refused(ELSEWHERE));
        }
        self.continue_outside(key, held)
    }
}
