// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The commands agents may run without asking, which you saved by
//! answering "Always allow" to an agent's command prompt, and the kinds of
//! file they may change without asking, which you kept with a card's
//! "Remember for this project" box. The `maxd` that runs the agent keeps
//! both for its project and publishes one list per project that has any.
//! A driver that answers its CLI's prompts itself gets the project's rules
//! when it connects and whenever they change; see `crate::drive::commands`
//! for how they match.

use std::collections::{BTreeMap, BTreeSet};

use maxhq_protocol::ids::{AgentId, DeviceId, ProjectId, SessionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{CommandRule, CommandRules, EditKind};
use maxhq_protocol::state::{Change, Entity, EntityKey};
use tracing::warn;

use super::{refused, Manager};

/// Each project's rules, as this run holds them.
pub(super) type Rules = BTreeMap<ProjectId, Vec<CommandRule>>;

/// Each project's remembered kinds of file, as this run holds them.
pub(super) type Edits = BTreeMap<ProjectId, BTreeSet<EditKind>>;

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
        let edits = self.db.allowed_edits().unwrap_or_else(|err| {
            warn!(%err, "couldn't read the remembered kinds of file");
            Vec::new()
        });
        for (project, kind) in edits {
            self.allowed_edits.entry(project).or_default().insert(kind);
        }
        let projects: BTreeSet<ProjectId> = self
            .command_rules
            .keys()
            .chain(self.allowed_edits.keys())
            .copied()
            .collect();
        for project in projects {
            self.show_command_rules(project);
        }
    }

    /// The project an agent works in, if its workspace has one.
    pub(super) fn project_of(&self, agent: AgentId) -> Option<ProjectId> {
        let lead = self.parents.get(&agent).copied().unwrap_or(agent);
        self.project_in(self.held.get(&lead)?.agent.workspace)
    }

    /// Gives a holder's driver its agent's project's rules and remembered
    /// kinds of file.
    pub(super) fn give_command_rules(&mut self, session: SessionId) {
        let Some(agent) = self.holds.get(&session).and_then(|hold| hold.record.agent) else {
            return;
        };
        let project = self.project_of(agent);
        let rules = project
            .and_then(|project| self.command_rules.get(&project).cloned())
            .unwrap_or_default();
        let edits: Option<Vec<EditKind>> = project.map(|project| {
            self.allowed_edits
                .get(&project)
                .map(|kinds| kinds.iter().copied().collect())
                .unwrap_or_default()
        });
        if let Some(hold) = self.holds.get_mut(&session) {
            hold.driver.command_rules(&rules);
            hold.driver.allowed_edits(edits.as_deref());
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

    /// Keeps the kinds of file a driver's card remembered for one of its
    /// session's agents' project.
    pub(super) fn save_allowed_edits(&mut self, session: SessionId, kinds: Vec<EditKind>) {
        let Some(agent) = self.holds.get(&session).and_then(|hold| hold.record.agent) else {
            return;
        };
        let Some(project) = self.project_of(agent) else {
            warn!(%agent, "the agent has no project to remember kinds of file for");
            return;
        };
        let mut added = false;
        for kind in kinds {
            let kept = self.allowed_edits.entry(project).or_default();
            if kept.contains(&kind) {
                continue;
            }
            if let Err(err) = self.db.set_allowed_edit(project, kind, true) {
                warn!(%err, "couldn't store a remembered kind of file");
                continue;
            }
            kept.insert(kind);
            added = true;
        }
        if self
            .allowed_edits
            .get(&project)
            .is_some_and(BTreeSet::is_empty)
        {
            self.allowed_edits.remove(&project);
        }
        if added {
            self.command_rules_changed(project);
        }
    }

    /// Answers `RemoveAllowedEdit`: agents in the project ask again before
    /// their next change to that kind of file.
    pub(super) fn remove_allowed_edit(
        &mut self,
        device: DeviceId,
        project: ProjectId,
        kind: EditKind,
    ) -> ActionOutcome {
        if device != self.device {
            return refused("those rules are on another device");
        }
        if let Err(err) = self.db.set_allowed_edit(project, kind, false) {
            return refused(format!("couldn't turn that off: {err}"));
        }
        if let Some(kept) = self.allowed_edits.get_mut(&project) {
            kept.remove(&kind);
            if kept.is_empty() {
                self.allowed_edits.remove(&project);
            }
        }
        self.command_rules_changed(project);
        ActionOutcome::Done
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

    /// Publishes a project's rules and remembered kinds of file, or takes
    /// its list away once it has neither.
    fn show_command_rules(&self, project: ProjectId) {
        let rules = self
            .command_rules
            .get(&project)
            .cloned()
            .unwrap_or_default();
        let edits: Vec<EditKind> = self
            .allowed_edits
            .get(&project)
            .map(|kinds| kinds.iter().copied().collect())
            .unwrap_or_default();
        let change = if rules.is_empty() && edits.is_empty() {
            Change::Remove {
                key: EntityKey::CommandRules {
                    project,
                    device: self.device,
                },
            }
        } else {
            Change::Upsert {
                value: Entity::CommandRules(CommandRules {
                    project,
                    device: self.device,
                    rules,
                    edits,
                }),
            }
        };
        self.hub.apply([change]);
    }
}
