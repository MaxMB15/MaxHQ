// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The card that asks before a task an agent made runs. A task an agent
//! makes, turns on, or changes in what it runs waits, off, until you say:
//! Approve turns it on and Decline deletes it. The card sits in the feed of
//! the agent that made it, and comes from the stored task, so it's back
//! after a restart for as long as the task waits. With no feed to show it
//! in, the task waits in Settings, where turning it on approves it.

use maxhq_core::agents::mode_name;
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer, AttentionEvent, AttentionKind};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, IdempotencyKey};
use maxhq_protocol::tasks::{Days, Maker, Task, TaskTarget, Trigger, Weekday};
use maxhq_protocol::timeline::{
    Consent, ConsentFact, ItemBody, ItemId, ItemStatus, Question, QuestionOption, TimelineEvent,
    TimelineItem,
};
use tracing::warn;

use super::super::folders::{answered, picked};
use super::super::note_tools::device_name;
use super::super::Manager;
use crate::drive::codex::requests::visible;
use crate::folder_trust::Card;

/// The card's question, which clients know it by.
pub const QUESTION: &str = "task-approval";
/// Turns the task on.
pub const APPROVE: &str = "Approve";
/// Deletes the task.
pub const DECLINE: &str = "Decline";

const TITLE: &str = "Approve this scheduled task?";

/// What the agent's tool reads about a task that waits.
pub const WAITS: &str =
    "It waits, off, for the user's approval in MaxHQ, and runs once they approve it.";
/// What an agent's `run_task_now` reads for a task that waits.
pub const STILL_WAITS: &str =
    "That task waits for the user's approval in MaxHQ, so it can't run yet.";
/// What an agent's `run_task_now` reads for a task that's off.
pub const OFF: &str = "That task is off. Only the user turns it on, in MaxHQ.";
/// Why a task that waits for the user, or one an agent made, stays off
/// when anyone else turns it on.
pub const YOURS_TO_TURN_ON: &str =
    "That task waits for the user's approval, so only the user turns it on, in MaxHQ.";

/// The card's event for a task that waits: one for each time it asks, and
/// each version of what it runs, since an answered event stays answered.
fn card_id(task: &Task) -> Option<AttentionId> {
    let asked = task.approval_asked?;
    let runs = serde_json::to_string(&(&task.prompt, &task.trigger, &task.target)).ok()?;
    Some(AttentionId::derive(&format!(
        "task-approval:{}:{}:{runs}",
        task.id, asked.0
    )))
}

impl Manager {
    /// Swaps the card of a task that was `before` for the one it needs
    /// now, when that's another: none once it no longer waits, and a new
    /// one when it starts to wait or waits for a new version of itself.
    pub(super) fn sync_task_card(&mut self, before: Option<&Task>, task: &Task) {
        if before.and_then(card_id) == card_id(task) {
            return;
        }
        if let Some(old) = before {
            self.withdraw_task_card(old);
        }
        self.ask_task_approval(task);
    }

    /// Opens the card for a task that waits, in the feed of the agent that
    /// made it, unless it's open already.
    fn ask_task_approval(&mut self, task: &Task) {
        let Some(card) = self.task_card(task) else {
            return;
        };
        if !self.attention.contains_key(&card.event.id) {
            self.open_folder_card(card);
        }
    }

    /// Takes away the card of a task that no longer waits, or waits for
    /// another version of itself, while it's open.
    pub(super) fn withdraw_task_card(&mut self, task: &Task) {
        let Some(event) = card_id(task) else {
            return;
        };
        let Some(open) = self.attention.get(&event) else {
            return;
        };
        let agent = open.agent;
        self.close_attention(event);
        let id = ItemId(format!("maxhq-task-approval-{event}"));
        self.timeline_events(vec![(agent, TimelineEvent::Remove { id })]);
    }

    /// Brings the cards back after a restart.
    pub(in crate::session) fn restore_task_cards(&mut self) {
        let waiting: Vec<Task> = self
            .tasks
            .tasks
            .values()
            .filter(|task| task.approval_asked.is_some())
            .cloned()
            .collect();
        for task in &waiting {
            self.ask_task_approval(task);
        }
    }

    /// An agent went, and the cards in its feed with it. Its tasks still
    /// wait, in Settings.
    pub(in crate::session) fn forget_task_cards(&mut self, lead: AgentId) {
        let open: Vec<AttentionId> = self
            .tasks
            .tasks
            .values()
            .filter_map(card_id)
            .filter(|event| {
                self.attention
                    .get(event)
                    .is_some_and(|open| open.agent == lead)
            })
            .collect();
        for event in open {
            self.close_attention(event);
        }
    }

    /// Whether an event is a task's card that waits.
    pub(in crate::session) fn is_task_card(&self, event: AttentionId) -> bool {
        self.task_of_card(event).is_some()
    }

    fn task_of_card(&self, event: AttentionId) -> Option<&Task> {
        self.tasks
            .tasks
            .values()
            .find(|task| card_id(task) == Some(event))
    }

    /// Answers a task's card. Only a person can, so an agent's tool route
    /// hears there's no such event, and an answer it doesn't offer leaves
    /// it open.
    pub(in crate::session) fn answer_task_card(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        event: AttentionId,
        answer: &AttentionAnswer,
        by: ClientId,
    ) -> AnswerOutcome {
        let person = self.clients.get(&conn).is_some_and(|viewer| viewer.person);
        let Some(task) = self.task_of_card(event).filter(|_| person).cloned() else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(pick) = picked(answer, QUESTION).filter(|pick| [APPROVE, DECLINE].contains(pick))
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let approve = pick == APPROVE;
        let Some(card) = self.task_card(&task) else {
            return AnswerOutcome::UnknownEvent;
        };
        self.record_answer(event, key, by);
        self.close_attention(event);
        let mut item = card.item;
        answered(
            &mut item,
            QUESTION,
            vec![pick.to_owned()],
            self.clock.wall(),
        );
        self.timeline_events(vec![(card.event.agent, TimelineEvent::Upsert { item })]);
        let settled = if approve {
            self.approve_task(task)
        } else {
            self.delete_task(task.id)
        };
        if let Err(err) = settled {
            warn!(%err, "couldn't settle a task you answered");
        }
        AnswerOutcome::Accepted
    }

    /// Turns on a task that waits.
    fn approve_task(&mut self, mut task: Task) -> Result<(), String> {
        task.enabled = true;
        task.approval_asked = None;
        task.next_run = self.next_due(&task);
        self.keep_task(task)
    }

    /// The card for a task that waits, when the agent that made it has a
    /// feed here.
    fn task_card(&self, task: &Task) -> Option<Card> {
        let event = card_id(task)?;
        let asked = task.approval_asked?;
        let Maker::Tools { origin } = task.made_by else {
            return None;
        };
        let feed = self.feed_of_actor(origin.by)?;
        let maker = self.agent_title(feed);
        let name = visible(&task.name);
        let runs = runs(&task.trigger);
        let reaches = self.reaches(&task.target, feed);
        let risk = format!(
            "{maker} made a task that runs by itself, with what this agent may do. It stays \
             off until you approve it."
        );
        let mut facts = vec![("Task", name.clone()), ("Runs", runs), ("Reaches", reaches)];
        if let TaskTarget::New { device, spec } = &task.target {
            facts.push(("Vendor", spec.vendor.name().to_owned()));
            facts.push(self.place_fact(&spec.place));
            let mode = match spec.permission {
                Some(mode) => {
                    let mut name = mode_name(mode).to_owned();
                    name[..1].make_ascii_uppercase();
                    name
                }
                None => format!(
                    "The default on {}",
                    visible(&device_name(&self.hub, *device))
                ),
            };
            facts.push(("Mode", mode));
        }
        // The whole prompt, each line kept, since it's what runs.
        let prompt = task.prompt.split('\n').map(visible).collect::<Vec<_>>();
        facts.push(("Prompt", prompt.join("\n")));
        let mut text = risk.clone();
        text.push('\n');
        for (label, value) in &facts {
            text.push_str(&format!("\n{label}: {value}"));
        }
        let consent = Consent {
            title: TITLE.to_owned(),
            risk,
            changes: Vec::new(),
            facts: facts
                .into_iter()
                .map(|(label, value)| ConsentFact {
                    label: label.to_owned(),
                    value,
                })
                .collect(),
            rows: Vec::new(),
            remember: None,
            edit: None,
        };
        let summary: String = format!("Approve the task {name}?")
            .chars()
            .take(80)
            .collect();
        let choice = |label: &str, description: &str| QuestionOption {
            label: label.to_owned(),
            description: Some(description.to_owned()),
        };
        let event_ = AttentionEvent {
            id: event,
            agent: feed,
            kind: AttentionKind::Question,
            summary,
            // No buttons: the inbox and a phone's notification send you to
            // the card, which says what the task does.
            options: Vec::new(),
            accepts_text: false,
            created_at: asked,
            expires_at: None,
        };
        let item = TimelineItem {
            id: ItemId(format!("maxhq-task-approval-{event}")),
            agent: feed,
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Waiting,
            started_at: Some(asked),
            completed_at: None,
            body: ItemBody::Question {
                attention: event,
                questions: vec![Question {
                    id: QUESTION.to_owned(),
                    header: Some("Scheduled task".to_owned()),
                    text,
                    options: vec![
                        choice(APPROVE, "Turns the task on."),
                        choice(DECLINE, "Deletes the task."),
                    ],
                    multi: false,
                    other: false,
                    secret: false,
                    consent: Some(consent),
                }],
                answers: None,
            },
        };
        Some(Card {
            event: event_,
            item,
        })
    }

    /// The agent a task's runs reach, said for the card.
    fn reaches(&self, target: &TaskTarget, feed: AgentId) -> String {
        match target {
            TaskTarget::Agent { device, agent } => {
                let on = visible(&device_name(&self.hub, *device));
                if self.lead_of(*agent) == feed {
                    format!("This agent, on {on}")
                } else {
                    format!("{}, on {on}", self.agent_title(*agent))
                }
            }
            TaskTarget::New { device, .. } => {
                format!(
                    "A new agent for each run, on {}",
                    visible(&device_name(&self.hub, *device))
                )
            }
        }
    }
}

/// When a task runs, said for the card, such as "Every 15 minutes" or
/// "Weekdays at 09:00, Europe/Amsterdam".
fn runs(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Every { minutes: 1 } => "Every minute".to_owned(),
        Trigger::Every { minutes: 60 } => "Every hour".to_owned(),
        Trigger::Every { minutes } if minutes % 60 == 0 => {
            format!("Every {} hours", minutes / 60)
        }
        Trigger::Every { minutes } => format!("Every {minutes} minutes"),
        Trigger::At { time, days, zone } => format!(
            "{} at {:02}:{:02}, {}",
            days_text(days),
            time.hour,
            time.minute,
            visible(&zone.0)
        ),
        Trigger::Hook(_) => "On each request to its webhook, once you turn that on".to_owned(),
        Trigger::GithubEvents { repo, .. } => format!("On events in {}", visible(repo)),
    }
}

/// The days of an `At` task, such as "Daily", "Weekdays" or "Mon, Thu".
fn days_text(days: &Days) -> String {
    const WEEK: [(Weekday, &str); 7] = [
        (Weekday::Monday, "Mon"),
        (Weekday::Tuesday, "Tue"),
        (Weekday::Wednesday, "Wed"),
        (Weekday::Thursday, "Thu"),
        (Weekday::Friday, "Fri"),
        (Weekday::Saturday, "Sat"),
        (Weekday::Sunday, "Sun"),
    ];
    let on: Vec<&str> = WEEK
        .iter()
        .filter(|(day, _)| days.0.contains(day))
        .map(|(_, short)| *short)
        .collect();
    match on.len() {
        0 | 7 => "Daily".to_owned(),
        5 if !days.0.contains(&Weekday::Saturday) && !days.0.contains(&Weekday::Sunday) => {
            "Weekdays".to_owned()
        }
        _ => on.join(", "),
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::tasks::{TimeOfDay, Zone};

    use super::*;

    #[test]
    fn a_schedule_reads_in_words() {
        assert_eq!(runs(&Trigger::Every { minutes: 1 }), "Every minute");
        assert_eq!(runs(&Trigger::Every { minutes: 15 }), "Every 15 minutes");
        assert_eq!(runs(&Trigger::Every { minutes: 120 }), "Every 2 hours");
        let at = |days: Vec<Weekday>| Trigger::At {
            time: TimeOfDay { hour: 9, minute: 5 },
            days: Days(days),
            zone: Zone("Europe/Amsterdam".into()),
        };
        assert_eq!(runs(&at(Vec::new())), "Daily at 09:05, Europe/Amsterdam");
        let weekdays = vec![
            Weekday::Friday,
            Weekday::Monday,
            Weekday::Tuesday,
            Weekday::Wednesday,
            Weekday::Thursday,
        ];
        assert_eq!(runs(&at(weekdays)), "Weekdays at 09:05, Europe/Amsterdam");
        assert_eq!(
            runs(&at(vec![Weekday::Thursday, Weekday::Monday])),
            "Mon, Thu at 09:05, Europe/Amsterdam"
        );
    }
}
