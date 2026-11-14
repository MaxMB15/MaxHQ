// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Telling running agents their skills changed. Claude watches its skills
//! folders, so it needs nothing. A Codex app-server rescans the changed
//! checkouts through `skills/list`. A Cursor or ACP agent reads skills when
//! its process starts, so it's marked and started again on its session
//! once it's idle. When an agent's turn ends, its
//! checkout's skill proposals are read again.

use std::path::PathBuf;

use maxhq_protocol::ids::{AgentId, SessionId, WorkspaceId};

use super::held::ServerKey;
use super::vendors::speaks_acp;
use super::{Input, Manager, Sessions};
use crate::drive::Effects;
use crate::skills::deliver::{self, Changed, Running};

impl Running for Sessions {
    fn skills_changed(&self, changed: Vec<Changed>) {
        let _ = self.tx.send(Input::SkillsChanged(changed));
    }
}

impl Manager {
    /// Tells the agents in each changed checkout.
    pub(super) async fn skills_changed(&mut self, changed: Vec<Changed>) {
        let servers: Vec<(ServerKey, SessionId)> = self
            .servers
            .iter()
            .filter_map(|(key, server)| Some((*key, server.session?)))
            .collect();
        for (key, session) in servers {
            let cwds: Vec<String> = changed
                .iter()
                .filter(|change| change.guest == key.workspace)
                .map(|change| change.path.display().to_string())
                .collect();
            let Some(hold) = self.holds.get_mut(&session) else {
                continue;
            };
            let mut fx = Effects::default();
            hold.driver.skills_changed(&cwds, &mut fx);
            self.apply_fx(session, fx).await;
        }
        for (agent, held) in &self.held {
            let restarts = speaks_acp(held.vendor())
                && held.session.is_some()
                && changed
                    .iter()
                    .any(|change| change.guest == held.guest && change.path == held.cwd);
            if restarts {
                self.skill_restarts
                    .insert(*agent, (held.guest, held.cwd.clone()));
                deliver::restart_marked(&held.cwd, true);
            }
        }
        self.restart_for_skills();
    }

    /// Starts each marked agent again once it's idle, and clears a
    /// checkout's mark once none of its agents waits.
    pub(super) fn restart_for_skills(&mut self) {
        if self.skill_restarts.is_empty() {
            return;
        }
        let marked: Vec<AgentId> = self.skill_restarts.keys().copied().collect();
        for agent in marked {
            let session = self.held.get(&agent).and_then(|held| held.session);
            if session.is_some() && self.mid_turn(agent) {
                continue;
            }
            let Some((guest, path)) = self.skill_restarts.remove(&agent) else {
                continue;
            };
            // One that stopped on its own reads the skills when it starts.
            if let Some(session) = session {
                tracing::info!(%agent, "starting an idle agent again so it sees its new skills");
                self.stop_to_relaunch(session);
            }
            if !self.waits_for_skills(guest, &path) {
                deliver::restart_marked(&path, false);
            }
        }
    }

    /// Stops a holder's CLI so its agent comes straight back.
    pub(super) fn stop_to_relaunch(&mut self, session: SessionId) {
        if let Some(hold) = self.holds.get_mut(&session) {
            hold.relaunch = true;
        }
        self.stop_hold(session);
    }

    /// Looks for skill proposals in the checkout an agent's turn just
    /// ended in.
    pub(super) fn turn_ended_for_skills(&self, agent: AgentId) {
        if let Some(held) = self.held.get(&agent) {
            deliver::turn_ended(agent, held.agent.workspace, held.guest, held.cwd.clone());
        }
    }

    fn waits_for_skills(&self, guest: Option<WorkspaceId>, path: &PathBuf) -> bool {
        self.skill_restarts
            .values()
            .any(|(other, at)| *other == guest && at == path)
    }
}
