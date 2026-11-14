// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The card that asks before an agent gets its project's workflow
//! variables. A workflow's descriptor names variables for the agents that
//! start in its project, and some kinds can send an agent's Claude sign-in
//! elsewhere or run code when a tool starts. So none reach a launch until
//! you allow them: a start that would carry them goes without and opens a
//! card in the agent's feed, and Allow starts the agent again with them once
//! it's between turns. Allowing for the project keeps the answer in its
//! settings on this device for as long as the names and values stay the
//! same. Any change asks again, and the card says what changed.

use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer, AttentionEvent, AttentionKind};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, IdempotencyKey, ProjectId, WorkspaceId};
use maxhq_protocol::model::{AllowedWorkflowEnv, AllowedWorkflowVar};
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::timeline::{
    Consent, ConsentRemember, ConsentRow, ItemBody, ItemId, ItemStatus, Question, QuestionOption,
    TimelineEvent, TimelineItem,
};
use maxhq_transfer::receive::sha256_hex;
use maxhq_workflow::env_risk;
use serde::{Deserialize, Serialize};
use tracing::warn;

use super::folders::{answered, picked, ticked};
use super::Manager;
use crate::diagnostics::mask::looks_secret;
use crate::drive::codex::requests::visible;
use crate::drive::{Launch, Place, PlaceHost};
use crate::folder_trust::{self, Card};
use crate::workflow::env::{self, Var};

/// The card's one question.
pub(super) const QUESTION: &str = "workflow-env";
/// The answer that gives the agent the variables.
pub(super) const ALLOW_LABEL: &str = "Allow";
/// The answer that keeps the agent going without them.
pub(super) const NOT_NOW_LABEL: &str = "Not now";
/// The box that keeps an Allow for the project, sent back as its own
/// answer.
pub(super) const REMEMBER: &str = "workflow-env-remember";
const REMEMBER_LABEL: &str = "Remember for this project";

const TITLE: &str = "Give this agent the workflow's variables?";
const RISK: &str = "These go into this agent's environment. Some kinds can send your Claude \
                    sign-in elsewhere or run code when a tool starts.";
const STRONG_RISK: &str = " Some replace what MaxHQ set for this agent, such as its Claude \
                           sign-in, so allow them only if you meant the workflow to.";
/// What a variable that replaces what the launch set can do.
const STRONG_WARNING: &str = "replaces what MaxHQ set for this agent, such as its Claude sign-in";

/// What an agent's last start found of its project's workflow variables,
/// and what you said about them. Stored with the agent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowEnv {
    /// The project they're for, when the agent's folder is in one.
    pub project: Option<ProjectId>,
    /// Each variable's name and its value's digest, by name.
    pub vars: Vec<AllowedWorkflowVar>,
    /// What you answered for this agent: true for Allow, false for Not now.
    pub allowed: Option<bool>,
    /// Whether the start carried them.
    pub given: bool,
    /// The card, while it waits.
    pub card: Option<Card>,
}

/// Each variable's name and its value's digest, by name.
fn digests(vars: &[Var]) -> Vec<AllowedWorkflowVar> {
    let mut digests: Vec<AllowedWorkflowVar> = vars
        .iter()
        .map(|var| AllowedWorkflowVar {
            name: var.name.clone(),
            digest: sha256_hex(var.value.as_bytes()),
        })
        .collect();
    digests.sort_by(|a, b| a.name.cmp(&b.name));
    digests
}

impl Manager {
    /// Gives a Claude launch `MAXHQ_AGENT`, and the workflow variables of
    /// its project once you allowed them, and says what to ask. Only a
    /// folder on this device gets them, while its project has them on.
    pub(super) fn give_workflow_env(
        &self,
        lead: AgentId,
        place: &Place,
        launch: &mut Launch,
    ) -> Option<WorkflowEnv> {
        let settings = self.workspace_settings(place.workspace);
        let on = matches!(place.host, PlaceHost::Local)
            && settings
                .as_ref()
                .is_none_or(|settings| settings.workflow_env);
        let vars = if on {
            self.workflows.envs.for_folder(&place.cwd)
        } else {
            Vec::new()
        };
        if vars.is_empty() {
            env::give(launch, lead, &[]);
            return None;
        }
        let digests = digests(&vars);
        let remembered = settings
            .and_then(|settings| settings.workflow_env_allowed)
            .map(|allowed| allowed.vars);
        let last = self
            .held
            .get(&lead)
            .and_then(|held| held.workflow_env.as_ref());
        let same = last.filter(|last| last.vars == digests);
        let allowed = same.and_then(|same| same.allowed);
        let given = remembered.as_ref() == Some(&digests) || allowed == Some(true);
        let project = self.project_in(place.workspace);
        let card = if given || allowed == Some(false) {
            None
        } else if let Some(card) = same.and_then(|same| same.card.clone()) {
            Some(card)
        } else {
            let before = remembered.or_else(|| {
                last.filter(|last| last.allowed == Some(true))
                    .map(|last| last.vars.clone())
            });
            Some(self.env_card(lead, project, &vars, &digests, before.as_deref(), launch))
        };
        env::give(launch, lead, if given { &vars } else { &[] });
        Some(WorkflowEnv {
            project,
            vars: digests,
            allowed,
            given,
            card,
        })
    }

    /// The project a workspace is in, if any.
    pub(super) fn project_in(&self, workspace: WorkspaceId) -> Option<ProjectId> {
        match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(workspace)) => workspace.project,
            _ => None,
        }
    }

    /// The card that asks for `vars`. `before` holds what you last allowed,
    /// so it can say what changed, and `launch` is the start without them.
    fn env_card(
        &self,
        agent: AgentId,
        project: Option<ProjectId>,
        vars: &[Var],
        digests: &[AllowedWorkflowVar],
        before: Option<&[AllowedWorkflowVar]>,
        launch: &Launch,
    ) -> Card {
        let id = AttentionId::generate();
        let now = self.clock.wall();
        // The workflow wrote all of it, so it all goes through `visible`.
        let rows: Vec<ConsentRow> = vars
            .iter()
            .map(|var| {
                let strong = env::changes_launch(&var.name, launch);
                let warning = if strong {
                    Some(STRONG_WARNING)
                } else {
                    env_risk(&var.name)
                };
                ConsentRow {
                    group: visible(&var.workflow),
                    name: visible(&var.name),
                    value: visible(&var.value),
                    secret: looks_secret(&var.name, &var.value),
                    warning: warning.map(str::to_owned),
                    strong,
                }
            })
            .collect();
        let changes = before.map_or_else(Vec::new, |before| {
            folder_trust::changes(&named(before), &named(digests))
        });
        let mut risk = RISK.to_owned();
        if rows.iter().any(|row| row.strong) {
            risk.push_str(STRONG_RISK);
        }
        let consent = Consent {
            title: TITLE.to_owned(),
            risk,
            changes,
            facts: Vec::new(),
            rows,
            remember: project.map(|project| ConsentRemember {
                id: REMEMBER.to_owned(),
                label: REMEMBER_LABEL.to_owned(),
                settings: self.env_setting(project),
            }),
        };
        let mut workflows: Vec<&str> = Vec::new();
        for row in &consent.rows {
            if !workflows.contains(&row.group.as_str()) {
                workflows.push(&row.group);
            }
        }
        let named: String = workflows.join(", ").chars().take(80).collect();
        let event = AttentionEvent {
            id,
            agent,
            kind: AttentionKind::Question,
            summary: format!("{TITLE} {named}"),
            // No buttons: an inbox shows only the summary, so it sends you to
            // the agent's feed, where the card lists the variables. That
            // keeps a phone's notification from answering it too.
            options: Vec::new(),
            accepts_text: false,
            created_at: now,
            expires_at: None,
        };
        let choice = |label: &str, description: &str| QuestionOption {
            label: label.to_owned(),
            description: Some(description.to_owned()),
        };
        let item = TimelineItem {
            id: ItemId(format!("maxhq-workflow-env-{id}")),
            agent,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Waiting,
            started_at: Some(now),
            completed_at: None,
            body: ItemBody::Question {
                attention: id,
                questions: vec![Question {
                    id: QUESTION.to_owned(),
                    header: Some("Workflow variables".to_owned()),
                    text: card_text(&consent),
                    options: vec![
                        choice(ALLOW_LABEL, "Restarts the agent between turns with them."),
                        choice(NOT_NOW_LABEL, "This agent carries on without them."),
                    ],
                    multi: false,
                    other: false,
                    secret: false,
                    consent: Some(consent),
                }],
                answers: None,
            },
        };
        Card { event, item }
    }

    /// What the box keeps, and where Settings shows a project's allowed
    /// variables on this device.
    fn env_setting(&self, project: ProjectId) -> String {
        let project = match self.hub.get(&EntityKey::Project(project)) {
            Some(Entity::Project(project)) => project.name,
            _ => "this project".to_owned(),
        };
        let device = match self.hub.get(&EntityKey::Device(self.device)) {
            Some(Entity::Device(device)) => device.name,
            _ => "this device".to_owned(),
        };
        format!(
            "Allow keeps this for the project. Change it later in Settings › Projects › {} › \
             General › On {} › Give agents the workflow's variables.",
            visible(&project),
            visible(&device)
        )
    }

    /// An agent's CLI came up after a start that found `asked`: shows its
    /// card, and closes one it no longer needs. An answer that came in
    /// since the start was decided stands.
    pub(super) fn workflow_env_reached(&mut self, lead: AgentId, asked: Option<WorkflowEnv>) {
        let Some(held) = self.held.get_mut(&lead) else {
            return;
        };
        let last = held.workflow_env.take();
        let mut now = asked;
        if let (Some(now), Some(last)) = (now.as_mut(), last.as_ref())
            && now.vars == last.vars
            && now.allowed.is_none()
            && last.allowed.is_some()
        {
            now.allowed = last.allowed;
            now.card = None;
        }
        held.workflow_env = now.clone();
        self.save(lead);
        let card = now.and_then(|now| now.card);
        if let Some(old) = last.and_then(|last| last.card)
            && card
                .as_ref()
                .is_none_or(|card| card.event.id != old.event.id)
        {
            self.close_env_card(lead, &old, None);
        }
        if let Some(card) = card.filter(|card| !self.attention.contains_key(&card.event.id)) {
            self.open_folder_card(card);
        }
    }

    /// Closes a card, showing what you picked or taking the question away
    /// when it no longer applies.
    fn close_env_card(&mut self, agent: AgentId, card: &Card, picked: Option<Vec<String>>) {
        self.close_attention(card.event.id);
        let event = match picked {
            Some(values) => {
                let mut item = card.item.clone();
                answered(&mut item, QUESTION, values, self.clock.wall());
                TimelineEvent::Upsert { item }
            }
            None => TimelineEvent::Remove {
                id: card.item.id.clone(),
            },
        };
        self.timeline_events(vec![(agent, event)]);
    }

    /// Whether an event is a card still waiting.
    pub(super) fn is_env_card(&self, event: AttentionId) -> bool {
        self.env_card_owner(event).is_some()
    }

    fn env_card_owner(&self, event: AttentionId) -> Option<AgentId> {
        self.held.iter().find_map(|(agent, held)| {
            held.workflow_env
                .as_ref()?
                .card
                .as_ref()
                .is_some_and(|card| card.event.id == event)
                .then_some(*agent)
        })
    }

    /// Answers a card. Only a person can, so an agent's tool route hears
    /// there's no such event, and an answer it doesn't offer leaves it open.
    pub(super) fn answer_env_card(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        event: AttentionId,
        answer: &AttentionAnswer,
        by: ClientId,
    ) -> AnswerOutcome {
        let person = self.clients.get(&conn).is_some_and(|viewer| viewer.person);
        let Some(agent) = self.env_card_owner(event).filter(|_| person) else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(asked) = self
            .held
            .get(&agent)
            .and_then(|held| held.workflow_env.clone())
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(pick) = picked(answer, QUESTION)
            .filter(|pick| asked.card.as_ref().is_some_and(|card| card.offers(pick)))
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let remember = ticked(answer, REMEMBER);
        let allow = pick == ALLOW_LABEL;
        let project = asked.project.filter(|_| allow && remember);
        let kept = project.is_some_and(|project| {
            let allowed = AllowedWorkflowEnv {
                vars: asked.vars.clone(),
                at: self.clock.wall(),
            };
            self.workspaces
                .allow_workflow_env(project, allowed)
                .inspect_err(|err| {
                    // It still counts for this agent.
                    warn!(%err, %agent, "couldn't keep the workflow's variables for the project");
                })
                .is_ok()
        });
        self.settle_env(agent, key, by, allow, kept);
        if kept {
            // Every agent in the project the same variables wait for gets them.
            let others: Vec<AgentId> = self
                .held
                .iter()
                .filter(|(other, held)| {
                    **other != agent
                        && held.workflow_env.as_ref().is_some_and(|other| {
                            other.project == asked.project
                                && other.vars == asked.vars
                                && !other.given
                        })
                })
                .map(|(other, _)| *other)
                .collect();
            for other in others {
                self.settle_env(other, key, by, true, true);
            }
        }
        AnswerOutcome::Accepted
    }

    /// Keeps your answer for an agent and closes its card showing it. Allow
    /// starts the agent again with the variables once it's between turns.
    fn settle_env(
        &mut self,
        agent: AgentId,
        key: IdempotencyKey,
        by: ClientId,
        allow: bool,
        remembered: bool,
    ) {
        let Some(asked) = self
            .held
            .get_mut(&agent)
            .and_then(|held| held.workflow_env.as_mut())
        else {
            return;
        };
        asked.allowed = Some(allow);
        let card = asked.card.take();
        self.save(agent);
        if let Some(card) = card {
            self.record_answer(card.event.id, key, by);
            let pick = if allow { ALLOW_LABEL } else { NOT_NOW_LABEL };
            let mut values = vec![pick.to_owned()];
            if remembered {
                values.push(REMEMBER_LABEL.to_owned());
            }
            self.close_env_card(agent, &card, Some(values));
        }
        if allow {
            self.trust_restarts.insert(agent);
        }
    }

    /// Brings the cards back after a restart, and goes on with a restart an
    /// Allow asked for.
    pub(super) fn restore_env_cards(&mut self) {
        let mut cards = Vec::new();
        for (agent, held) in &self.held {
            let Some(asked) = &held.workflow_env else {
                continue;
            };
            if asked.allowed == Some(true) && !asked.given {
                self.trust_restarts.insert(*agent);
            }
            cards.extend(asked.card.clone());
        }
        for card in cards {
            self.open_folder_card(card);
        }
    }

    /// An agent went, and its card with it.
    pub(super) fn forget_env_card(&mut self, asked: Option<WorkflowEnv>) {
        if let Some(card) = asked.and_then(|asked| asked.card) {
            self.close_attention(card.event.id);
        }
    }
}

/// Each name with its digest, as [`folder_trust::changes`] takes them.
fn named(vars: &[AllowedWorkflowVar]) -> Vec<(&str, &str)> {
    vars.iter()
        .map(|var| (var.name.as_str(), var.digest.as_str()))
        .collect()
}

/// What the card says as plain text, for a client that shows only that.
/// A value that looks secret stays hidden there.
fn card_text(consent: &Consent) -> String {
    let mut text = format!(
        "This project's workflows name variables for the agents that start here. This agent \
         runs without them until you allow them. {}\n",
        consent.risk
    );
    if !consent.changes.is_empty() {
        text.push_str("\nWhat changed since you last allowed them:\n");
        for line in &consent.changes {
            text.push_str(&format!("  {line}\n"));
        }
    }
    let mut group = None;
    for row in &consent.rows {
        if group != Some(&row.group) {
            text.push_str(&format!("\n{}:\n", row.group));
            group = Some(&row.group);
        }
        let value = if row.secret { "[hidden]" } else { &row.value };
        match &row.warning {
            Some(warning) => text.push_str(&format!("  {}={value} ({warning})\n", row.name)),
            None => text.push_str(&format!("  {}={value}\n", row.name)),
        }
    }
    text
}
