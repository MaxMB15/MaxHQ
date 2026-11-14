// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Running list: the processes in an agent's holder tree, and the
//! signal a client sends one of them. Each request runs off the loop,
//! through the port watch's scanner for a holder on this device or the
//! guest for one in a container, and answers the connection that asked.
//! A sub-agent shares its lead's holder, so it lists the lead's tree.
//! Each lead also shows what it started that still runs, as the port poll
//! last found it, with what its driver says it waits on.

use std::sync::PoisonError;

use maxhq_protocol::ids::{AgentId, IdempotencyKey, SessionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::Agent;
use maxhq_protocol::processes::{AgentRunning, ProcessList, ProcessSignal};

use super::lookups::NO_AGENT;
use super::{refused, Input, Manager};
use crate::ports::{processes, Holding, Jobs};

/// Why a signal names an agent whose holder isn't running.
const NOT_RUNNING: &str = "That agent isn't running, so it has no processes to stop.";

impl Manager {
    /// What the agent started that still runs, from the port poll, with
    /// what its driver says it waits on.
    pub(super) fn running(
        &self,
        agent: &Agent,
        session: Option<SessionId>,
    ) -> Option<AgentRunning> {
        let mut running = session
            .and_then(|session| self.jobs_running.get(&session))
            .cloned()
            .unwrap_or_default();
        running.waiting = agent.running.as_ref().and_then(|was| was.waiting.clone());
        (running != AgentRunning::default()).then_some(running)
    }

    /// Publishes the leads whose holders' jobs changed.
    pub(super) fn jobs_changed(&mut self, jobs: Jobs) {
        let old = std::mem::replace(&mut self.jobs_running, jobs);
        let leads: Vec<AgentId> = self
            .held
            .iter()
            .filter(|(_, held)| {
                self.session_of(held)
                    .is_some_and(|session| old.get(&session) != self.jobs_running.get(&session))
            })
            .map(|(lead, _)| *lead)
            .collect();
        for lead in leads {
            self.publish(lead);
        }
    }

    /// The holder an agent's processes run under, or none until it runs.
    fn holding_of(&self, agent: AgentId) -> Result<Option<Holding>, String> {
        let held = self
            .held
            .get(&self.lead_of(agent))
            .ok_or_else(|| NO_AGENT.to_owned())?;
        Ok(self
            .session_of(held)
            .and_then(|session| self.holding(session)))
    }

    /// Answers `ListProcesses` with the processes in the agent's holder
    /// tree. An agent that isn't running has none.
    pub(super) fn list_processes(&mut self, conn: u64, key: IdempotencyKey, agent: AgentId) {
        let holding = match self.holding_of(agent) {
            Ok(Some(holding)) => holding,
            Ok(None) => {
                let list = ProcessList::Listed {
                    processes: Vec::new(),
                };
                return self.found(conn, key, ActionOutcome::Processes { list });
            }
            Err(reason) => return self.found(conn, key, refused(reason)),
        };
        let scanner = self.ports.scanner();
        let places = self.places.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = if holding.guest {
                places
                    .list_processes(holding.workspace, holding.holder)
                    .await
            } else {
                tokio::task::spawn_blocking(move || {
                    let mut scanner = scanner.lock().unwrap_or_else(PoisonError::into_inner);
                    let processes = scanner.processes(&holding.holder);
                    ProcessList::Listed { processes }
                })
                .await
                .map_err(|err| format!("the list stopped: {err}"))
            };
            let outcome = match result {
                Ok(list) => ActionOutcome::Processes { list },
                Err(reason) => refused(format!("Couldn't list the agent's processes: {reason}")),
            };
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(outcome),
            });
        });
    }

    /// Answers `SignalProcess`. The signal goes only to a process in the
    /// agent's holder tree now, with the start time the client saw.
    pub(super) fn signal_process(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        agent: AgentId,
        target: (u32, u64),
        signal: ProcessSignal,
    ) {
        let (pid, start) = target;
        let holding = match self.holding_of(agent) {
            Ok(Some(holding)) => holding,
            Ok(None) => return self.found(conn, key, refused(NOT_RUNNING)),
            Err(reason) => return self.found(conn, key, refused(reason)),
        };
        let scanner = self.ports.scanner();
        let places = self.places.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = if holding.guest {
                places
                    .signal_process(holding.workspace, holding.holder, pid, start, signal)
                    .await
            } else {
                tokio::task::spawn_blocking(move || {
                    let mut scanner = scanner.lock().unwrap_or_else(PoisonError::into_inner);
                    processes::signal(&mut **scanner, &holding.holder, pid, start, signal)
                })
                .await
                .unwrap_or_else(|err| Err(format!("the signal stopped: {err}")))
            };
            let outcome = match result {
                Ok(()) => ActionOutcome::Done,
                Err(reason) => refused(format!("Couldn't stop that process: {reason}")),
            };
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(outcome),
            });
        });
    }
}
