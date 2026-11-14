// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The commands agents may run without asking, which you saved by
//! answering "Always allow" to an agent's command prompt. The `maxd` that
//! runs the agent keeps them for its project and publishes one list per
//! project that has any. A driver that answers its CLI's prompts itself
//! gets the project's rules when it connects and whenever they change;
//! see `crate::drive::commands` for how they match.

use std::collections::BTreeMap;

use maxhq_protocol::ids::{AgentId, DeviceId, ProjectId, SessionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{CommandRule, CommandRules};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use tracing::warn;

use super::{refused, Manager};

/// Each project's rules, as this run holds them.
pub(super) type Rules = BTreeMap<ProjectId, Vec<CommandRule>>;

impl Manager {
    /// Loads and shows the rules the previous run stored.
    pub(super) fn load_command_rules(&mut self) {
        let stored = self.db.command_rules().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the command rules");
            Vec::new()
        });
        for (project, rule) in stored {
            self.command_rules.entry(project).or_default().push(rule);
        }
        let projects: Vec<ProjectId> = self.command_rules.keys().copied().collect();
        for project in projects {
            self.show_command_rules(project);
        }
    }

    /// The project an agent works in, if its workspace has one.
    pub(super) fn project_of(&self, agent: AgentId) -> Option<ProjectId> {
        let lead = self.parents.get(&agent).copied().unwrap_or(agent);
        self.project_in(self.held.get(&lead)?.agent.workspace)
    }

    /// Gives a holder's driver its agent's project's rules.
    pub(super) fn give_command_rules(&mut self, session: SessionId) {
        let Some(agent) = self.holds.get(&session).and_then(|hold| hold.record.agent) else {
            return;
        };
        let rules = self
            .project_of(agent)
            .and_then(|project| self.command_rules.get(&project).cloned())
            .unwrap_or_default();
        if let Some(hold) = self.holds.get_mut(&session) {
            hold.driver.command_rules(&rules);
        }
    }

    /// Keeps the rules a driver saved for one of its session's agents.
    pub(super) fn save_command_rules(&mut self, session: SessionId, rules: Vec<CommandRule>) {
        let Some(agent) = self.holds.get(&session).and_then(|hold| hold.record.agent) else {
            return;
        };
        let Some(project) = self.project_of(agent) else {
            warn!(%agent, "the agent has no project to keep its command rules");
            return;
        };
        let list = self.command_rules.entry(project).or_default();
        let mut added = false;
        for rule in rules {
            if list.contains(&rule) {
                continue;
            }
            if let Err(err) = self.db.add_command_rule(project, &rule) {
                warn!(%err, "couldn't store a command rule");
                continue;
            }
            list.push(rule);
            added = true;
        }
        if added {
            self.command_rules_changed(project);
        }
    }

    /// Answers `RemoveCommandRule`.
    pub(super) fn remove_command_rule(
        &mut self,
        device: DeviceId,
        project: ProjectId,
        rule: &CommandRule,
    ) -> ActionOutcome {
        if device != self.device {
            return refused("those rules are on another device");
        }
        if let Err(err) = self.db.remove_command_rule(project, rule) {
            return refused(format!("couldn't remove the command rule: {err}"));
        }
        if let Some(list) = self.command_rules.get_mut(&project) {
            list.retain(|kept| kept != rule);
            if list.is_empty() {
                self.command_rules.remove(&project);
            }
        }
        self.command_rules_changed(project);
        ActionOutcome::Done
    }

    /// Shows a project's new rules and gives them to its agents' drivers.
    fn command_rules_changed(&mut self, project: ProjectId) {
        self.show_command_rules(project);
        let sessions: Vec<SessionId> = self.holds.keys().copied().collect();
        for session in sessions {
            let agent = self.holds.get(&session).and_then(|hold| hold.record.agent);
            if agent.and_then(|agent| self.project_of(agent)) == Some(project) {
                self.give_command_rules(session);
            }
        }
    }

    /// Publishes a project's rules, or takes its list away once it has
    /// none.
    fn show_command_rules(&self, project: ProjectId) {
        let change = match self.command_rules.get(&project) {
            Some(rules) if !rules.is_empty() => Change::Upsert {
                value: Entity::CommandRules(CommandRules {
                    project,
                    device: self.device,
                    rules: rules.clone(),
                }),
            },
            _ => Change::Remove {
                key: EntityKey::CommandRules {
                    project,
                    device: self.device,
                },
            },
        };
        self.hub.apply([change]);
    }
}
