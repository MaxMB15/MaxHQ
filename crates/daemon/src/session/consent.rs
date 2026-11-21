// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Asking you before an agent in a container reaches outside it. Its
//! permission's Containers setting decides: on Ask, the default, the call
//! waits while a card in the agent's feed says what it wants and what could
//! go wrong. Allow runs the call and Deny refuses it, and with the card's
//! box ticked either is kept on the permission. A card nobody answers in
//! time refuses its call, the way a wait times out. Cards live in memory, so
//! restarting `maxd` drops them with the calls they hold.

use std::time::{Duration, Instant};

use maxhq_core::agents::Refusal;
use maxhq_core::leases::LeaseCall;
use maxhq_core::tools::agents::WAIT;
use maxhq_core::tools::Call;
use maxhq_protocol::agents::{Actor, LeaveContainer, Permission, ToolCall};
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer, AttentionEvent, AttentionKind};
use maxhq_protocol::ids::{
    AgentId, AttentionId, ClientId, DeviceId, IdempotencyKey, PermissionId, WorkspaceId,
};
use maxhq_protocol::message::{ActionOutcome, AgentPlace};
use maxhq_protocol::model::WorkspaceLocation;
use maxhq_protocol::state::{Entity, EntityKey};
use maxhq_protocol::tasks::Maker;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    Consent, ConsentFact, ConsentRemember, ItemBody, ItemId, ItemStatus, Question, QuestionOption,
    TimelineEvent, TimelineItem,
};
use tracing::warn;

use super::agent_tools::{Calling, Reply};
use super::folders::{answered, picked, ticked};
use super::note_tools::device_name;
use super::Manager;
use crate::drive::codex::requests::visible;
use crate::folder_trust::Card;
use crate::leases::kind_label;

/// The card's question, which clients know it by.
pub const QUESTION: &str = "container-reach";
/// Lets the one call go.
pub const ALLOW: &str = "Allow";
/// Refuses the one call.
pub const DENY: &str = "Deny";
/// The box that keeps the answer on the permission, sent back as its own
/// answer.
pub const REMEMBER: &str = "container-reach-remember";
const REMEMBER_LABEL: &str = "Remember for this permission";
/// What the card asks for.
const TITLE: &str = "Reach outside the container";

/// What a call reads when you say no.
const DENIED: &str = "The user didn't allow this call to reach outside your container.";
/// What a call reads when nobody answered in time.
const UNANSWERED: &str =
    "The user didn't answer in time, so this call that reaches outside your container didn't go.";
/// What a task's run reads when nobody answered in time.
const RUN_UNANSWERED: &str = "This run waited for the owner to let it reach outside the \
                              container its task was made in. Nobody answered in time, so it \
                              didn't go.";
/// What a call reads when there's no feed to ask in.
const NO_FEED: &str = "This call reaches outside the container it was made in, so it waits for \
                       the owner to allow it, but the agent whose feed would ask is gone.";

/// How each risk starts, after what runs outside the container.
const MISLED: &str =
    "If the agent in the container was misled by a repository, a web page or a prompt injection,";

/// A call held for your answer.
pub(super) struct Pending {
    pub(super) reply: Reply,
    pub(super) caller: Calling,
    pub(super) call: Call,
    pub(super) raw: ToolCall,
}

/// A card waiting for your answer, and the call it holds.
pub(super) struct Asking {
    event: AttentionId,
    /// The card in the feed that shows it.
    item: TimelineItem,
    pending: Pending,
    /// The permission an answer can be kept on, when the card offers that.
    keep: Option<PermissionId>,
    pub(super) until: Instant,
}

/// What a call reaches, for the card.
struct Reach {
    /// What it asks to do, after "asks to".
    wants: String,
    /// What could go wrong.
    risk: String,
    /// The details to judge it by.
    details: Vec<(&'static str, String)>,
}

impl Manager {
    /// Refuses a call that reaches outside its caller's container, or holds
    /// it for your answer when its permission says to ask.
    pub(super) fn refuse_or_ask(
        &mut self,
        refusal: Refusal,
        pending: Pending,
        permission: &Permission,
        device: DeviceId,
    ) {
        if refusal != Refusal::NeedsConsent {
            return self.reply_now(pending.reply, Err(refusal.to_string()));
        }
        let Some(feed) = self.feed_of(pending.caller) else {
            return self.reply_now(pending.reply, Err(NO_FEED.into()));
        };
        let (summary, question) = self.consent_question(feed, &pending, permission, device);
        let keep = self.keeps(permission).then_some(permission.id);
        let (event, item) = self.open_timed_card(feed, "container-reach", summary, question);
        self.agent_tools.asking.push(Asking {
            event,
            item,
            pending,
            keep,
            until: self.clock.now() + Duration::from_secs(WAIT),
        });
    }

    /// Opens a card in `feed`'s timeline that waits [`WAIT`] seconds for
    /// your answer, with `name` in its item's id. Answers with its event and
    /// item.
    pub(super) fn open_timed_card(
        &mut self,
        feed: AgentId,
        name: &str,
        summary: String,
        question: Question,
    ) -> (AttentionId, TimelineItem) {
        let now = self.clock.wall();
        let event = AttentionId::generate();
        let item = TimelineItem {
            id: ItemId(format!("maxhq-{name}-{event}")),
            agent: feed,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Waiting,
            started_at: Some(now),
            completed_at: None,
            body: ItemBody::Question {
                attention: event,
                questions: vec![question],
                answers: None,
            },
        };
        let attention = AttentionEvent {
            id: event,
            agent: feed,
            kind: AttentionKind::Question,
            summary,
            // No buttons: the phone answers in the app, never from the
            // lock screen, and the inbox sends you to the feed.
            options: Vec::new(),
            accepts_text: false,
            created_at: now,
            expires_at: Some(Timestamp(now.0.saturating_add(WAIT * 1000))),
        };
        self.open_folder_card(Card {
            event: attention,
            item: item.clone(),
        });
        (event, item)
    }

    /// Closes a timed card, showing what you picked on its `question`, or
    /// that it closed unanswered.
    pub(super) fn close_timed_card(
        &mut self,
        event: AttentionId,
        mut item: TimelineItem,
        question: &str,
        picked: Option<Vec<String>>,
    ) {
        self.close_attention(event);
        let now = self.clock.wall();
        match picked {
            Some(values) => answered(&mut item, question, values, now),
            None => {
                item.status = ItemStatus::Cancelled;
                item.completed_at = Some(now);
            }
        }
        let feed = item.agent;
        self.timeline_events(vec![(feed, TimelineEvent::Upsert { item })]);
    }

    /// Whether an event is a card holding a call.
    pub(super) fn is_consent_card(&self, event: AttentionId) -> bool {
        self.agent_tools.asking.iter().any(|a| a.event == event)
    }

    /// Answers a card. Only a person can, so an agent's tool route hears
    /// there's no such event, and an answer it doesn't offer leaves it open.
    pub(super) fn answer_consent(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        event: AttentionId,
        answer: &AttentionAnswer,
        by: ClientId,
    ) -> AnswerOutcome {
        let person = self.clients.get(&conn).is_some_and(|viewer| viewer.person);
        let Some(index) = self
            .agent_tools
            .asking
            .iter()
            .position(|a| a.event == event)
            .filter(|_| person)
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(pick) = picked(answer, QUESTION).filter(|pick| [ALLOW, DENY].contains(pick))
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let allow = pick == ALLOW;
        // A ticked box keeps the answer only where the card offered it.
        let keep = self.agent_tools.asking[index]
            .keep
            .filter(|_| ticked(answer, REMEMBER));
        let mut values = vec![pick.to_owned()];
        if keep.is_some() {
            values.push(REMEMBER_LABEL.to_owned());
        }
        let asking = self.agent_tools.asking.swap_remove(index);
        self.record_answer(event, key, by);
        self.close_consent(&asking, Some(values));
        if let Some(permission) = keep {
            let leave = if allow {
                LeaveContainer::Allow
            } else {
                LeaveContainer::Never
            };
            self.keep_reach(permission, leave);
        }
        let Pending {
            reply,
            caller,
            call,
            raw,
        } = asking.pending;
        if allow {
            // You allowed this call, so it runs as if from outside.
            let caller = Calling {
                container: None,
                ..caller
            };
            self.run_agent_call(reply, caller, call, raw, false);
        } else {
            self.reply_now(reply, Err(DENIED.into()));
        }
        AnswerOutcome::Accepted
    }

    /// Refuses the calls whose cards nobody answered in time.
    pub(super) fn expire_consents(&mut self) {
        let now = self.clock.now();
        let ended: Vec<Asking> = self
            .agent_tools
            .asking
            .extract_if(.., |asking| asking.until <= now)
            .collect();
        for asking in ended {
            self.close_consent(&asking, None);
            let reason = match asking.pending.caller.actor {
                Actor::Task { .. } => RUN_UNANSWERED,
                _ => UNANSWERED,
            };
            self.reply_now(asking.pending.reply, Err(reason.into()));
        }
    }

    /// Closes a card, showing what you picked, or that it closed unanswered.
    fn close_consent(&mut self, asking: &Asking, picked: Option<Vec<String>>) {
        self.close_timed_card(asking.event, asking.item.clone(), QUESTION, picked);
    }

    /// Keeps an answer on the permission, for every later call under it.
    fn keep_reach(&mut self, permission: PermissionId, leave: LeaveContainer) {
        let Some(mut kept) = self.agent_tools.permissions.get(&permission).cloned() else {
            return;
        };
        kept.leave_container = leave;
        if let ActionOutcome::Refused { reason } = self.grant(kept) {
            warn!(%reason, %permission, "couldn't keep the answer on the permission");
        }
    }

    /// Whether a card offers to keep its answer on the permission: only one
    /// an agent holds, whose header menu changes it. The read right every
    /// agent gets isn't kept anywhere, so its cards don't offer it.
    fn keeps(&self, permission: &Permission) -> bool {
        matches!(permission.holder, Actor::Agent { .. })
            && self.agent_tools.permissions.contains_key(&permission.id)
    }

    /// The agent whose feed asks about a call: the caller, or for a task's
    /// run, the agent that made the task.
    fn feed_of(&self, caller: Calling) -> Option<AgentId> {
        self.feed_of_actor(caller.actor)
    }

    /// The lead agent whose feed asks about what `actor` does: the agent
    /// itself, or for a task's run, the agent that made the task. None for
    /// an access key, or an agent that's gone.
    pub(super) fn feed_of_actor(&self, actor: Actor) -> Option<AgentId> {
        let agent = match actor {
            Actor::Agent { agent } => agent,
            Actor::Task { task } => match self.task_maker(task)? {
                Maker::Tools { origin } => match origin.by {
                    Actor::Agent { agent } => agent,
                    _ => return None,
                },
                Maker::User => return None,
            },
            Actor::Key { .. } => return None,
        };
        let lead = self.lead_of(agent);
        self.held.contains_key(&lead).then_some(lead)
    }

    /// The card's inbox line and its question.
    fn consent_question(
        &self,
        feed: AgentId,
        pending: &Pending,
        permission: &Permission,
        device: DeviceId,
    ) -> (String, Question) {
        let title = self.agent_title(feed);
        let container = pending.caller.container.map_or_else(
            || "a container".to_owned(),
            |(_, ws)| self.workspace_name(ws),
        );
        let reach = self.call_reach(&pending.call, device);
        let tool = pending.call.tool().name();
        let (who, opening, mut details) = match pending.caller.actor {
            Actor::Task { task } => {
                let name = visible(&self.task_name(task).unwrap_or_default());
                (
                    format!("the task {name}"),
                    format!(
                        "A run of the task {name}, which {title} made in the container \
                         {container}, asks to {}.",
                        reach.wants
                    ),
                    vec![("Task", name), ("Made by", title)],
                )
            }
            _ => (
                title.clone(),
                format!(
                    "{title} runs in the container {container} and asks to {}.",
                    reach.wants
                ),
                vec![("Agent", title)],
            ),
        };
        details.push(("Container", container));
        details.push(("Call", tool.to_owned()));
        details.extend(reach.details);
        let mut text = format!("{opening}\n\n{}\n", reach.risk);
        for (label, value) in &details {
            text.push_str(&format!("\n{label}: {value}"));
        }
        let remember = self.keeps(permission).then(|| ConsentRemember {
            id: REMEMBER.to_owned(),
            label: REMEMBER_LABEL.to_owned(),
            settings: format!(
                "Kept on the permission. To change it later, open the menu in {}, pick Change \
                 what it can run, and set Containers.",
                self.holder_header(feed, permission.holder)
            ),
        });
        let consent = Consent {
            title: TITLE.to_owned(),
            risk: format!("{opening} {}", reach.risk),
            changes: Vec::new(),
            facts: details
                .into_iter()
                .map(|(label, value)| ConsentFact {
                    label: label.to_owned(),
                    value,
                })
                .collect(),
            rows: Vec::new(),
            remember,
        };
        let choice = |label: &str, description: &str| QuestionOption {
            label: label.to_owned(),
            description: Some(description.to_owned()),
        };
        let summary: String = format!("Let {who} reach outside its container?")
            .chars()
            .take(80)
            .collect();
        let question = Question {
            id: QUESTION.to_owned(),
            header: Some(TITLE.to_owned()),
            text,
            options: vec![
                choice(ALLOW, "Lets this call go, this once."),
                choice(DENY, "Refuses this call."),
            ],
            multi: false,
            other: false,
            secret: false,
            consent: Some(consent),
        };
        (summary, question)
    }

    /// What a call reaches, said for the card.
    fn call_reach(&self, call: &Call, device: DeviceId) -> Reach {
        let on = format!("on {}", visible(&device_name(&self.hub, device)));
        let mut details = vec![("Device", visible(&device_name(&self.hub, device)))];
        let runs = |subject: &str| {
            format!(
                "{subject} runs outside the container, without its network limits or file walls. \
                 {MISLED} whatever it asks for runs outside the container with your files and keys."
            )
        };
        let reads = format!(
            "That agent runs outside the container, and its conversation can hold your files \
             and keys. {MISLED} it could pass on what it reads."
        );
        let (wants, risk) = match call {
            Call::StartAgent(start) => {
                let place = match &start.place {
                    Some(AgentPlace::Folder { path }) => {
                        let path = visible(path);
                        details.push(("Folder", path.clone()));
                        format!(" in the folder {path}")
                    }
                    Some(AgentPlace::Workspace { workspace }) => {
                        let name = self.workspace_name(*workspace);
                        details.push(("Workspace", name.clone()));
                        format!(" in the workspace {name}")
                    }
                    None => String::new(),
                };
                let vendor = start.vendor.name();
                details.push(("Vendor", vendor.to_owned()));
                (
                    format!("start a {vendor} agent{place} {on}"),
                    runs("The new agent"),
                )
            }
            Call::SendAgent(message) => (
                format!(
                    "send a message to {}",
                    self.agents_reached(device, &[message.agent], &mut details)
                ),
                runs("That agent"),
            ),
            Call::ReadAgent(read) => {
                let named = match read.agent.trim().parse::<AgentId>() {
                    Ok(agent) => self.agents_reached(device, &[agent], &mut details),
                    Err(_) => visible(&read.agent),
                };
                (format!("read the conversation of {named}"), reads)
            }
            Call::ListAgents(_) => (
                format!("list the agents {on} with their last replies"),
                format!(
                    "Those agents run outside the container, and their replies can hold your \
                     files and keys. {MISLED} it could pass on what it reads."
                ),
            ),
            Call::WaitAgent(wait) => (
                format!(
                    "wait on {} and read where they got to",
                    self.agents_reached(device, &wait.agents, &mut details)
                ),
                reads,
            ),
            Call::InterruptAgent(stop) => {
                let named = self.agents_reached(device, &[stop.agent], &mut details);
                (
                    if stop.stop {
                        format!("stop {named}, with the agents it started")
                    } else {
                        format!("interrupt {named}")
                    },
                    format!(
                        "That agent runs outside the container, where an agent in it can't \
                         otherwise reach. {MISLED} it could stop work you have running there."
                    ),
                )
            }
            Call::Lease(LeaseCall::Open { kind, .. }) => {
                let kind = kind_label(*kind);
                details.push(("Kind", kind.to_owned()));
                (
                    format!("lease a {kind} test machine {on}"),
                    format!(
                        "The test machine runs outside the container, without its network \
                         limits or file walls. {MISLED} whatever it runs there runs outside \
                         the container."
                    ),
                )
            }
            other => (
                format!("use {} {on}", other.tool().name()),
                runs("What it reaches"),
            ),
        };
        Reach {
            wants,
            risk,
            details,
        }
    }

    /// Agents a call reaches and where, such as `Build on the host`, with
    /// each named in the details.
    fn agents_reached(
        &self,
        device: DeviceId,
        agents: &[AgentId],
        details: &mut Vec<(&'static str, String)>,
    ) -> String {
        let names: Vec<String> = agents
            .iter()
            .map(|agent| {
                let name = match self.agent_now(*agent).filter(|_| device == self.device) {
                    Some(found) => visible(&found.title),
                    None => agent.to_string(),
                };
                details.push(("Reaches", name.clone()));
                name
            })
            .collect();
        let place = if device != self.device {
            format!("on {}", visible(&device_name(&self.hub, device)))
        } else {
            match self.common_container(device, agents) {
                Some(workspace) => format!("in the container {}", self.workspace_name(workspace)),
                None => "on the host".to_owned(),
            }
        };
        format!("{} {place}", names.join(", "))
    }

    /// The container every agent runs in, if they all run in the same one
    /// on this device.
    pub(super) fn common_container(
        &self,
        device: DeviceId,
        agents: &[AgentId],
    ) -> Option<WorkspaceId> {
        let mut found = agents.iter().map(|agent| self.container_of(device, *agent));
        let first = found.next().flatten()?;
        found.all(|other| other == Some(first)).then_some(first)
    }

    /// Where the menu that changes a permission is: in its holder's header.
    fn holder_header(&self, feed: AgentId, holder: Actor) -> String {
        match holder {
            Actor::Agent { agent } if agent == feed => "this agent's header".to_owned(),
            Actor::Agent { agent } => match self.agent_now(agent) {
                Some(found) => format!("the header of {}", visible(&found.title)),
                None => "the header of the agent that holds it".to_owned(),
            },
            _ => "the header of the agent that holds it".to_owned(),
        }
    }

    /// An agent's title, for the card.
    pub(super) fn agent_title(&self, agent: AgentId) -> String {
        self.agent_now(agent)
            .map_or_else(|| "An agent".to_owned(), |found| visible(&found.title))
    }

    /// A workspace's name: its container, or its folder's.
    fn workspace_name(&self, workspace: WorkspaceId) -> String {
        let name = match self.hub.get(&EntityKey::Workspace(workspace)) {
            Some(Entity::Workspace(found)) => match found.location {
                WorkspaceLocation::Container { instance, .. } => instance,
                WorkspaceLocation::Folder { path } => path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path)
                    .to_owned(),
            },
            _ => workspace.to_string(),
        };
        visible(&name)
    }
}
