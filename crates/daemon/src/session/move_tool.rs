// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `move_agent` tool. A move stops an agent you may have started, so
//! one a tool asks for waits for your answer on a card in the caller's
//! feed, as a task an agent makes does. Before the card opens, the device
//! the agent leaves says it can go and the device it goes to says what it
//! will run as there, so the card names both. Approve runs the move as the
//! app's Move does. Decline refuses it, and so does a card nobody answers
//! in time. Cards live in memory, so restarting `maxd` drops them with the
//! calls they hold.

use std::time::{Duration, Instant};

use maxhq_core::agents::{self as rules, Ask, NOT_ALLOWED};
use maxhq_core::tools::agents::{Move, WAIT};
use maxhq_protocol::agents::{AgentMoveStep, Departure, MoveCheck, MoveTo, Permission};
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, DeviceId, IdempotencyKey, PermissionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::timeline::{Consent, ConsentFact, Question, QuestionOption, TimelineItem};

use super::agent_tools::{failure, request_key, Calling, Reply, Then, NO_CONN};
use super::folders::picked;
use super::idem::Outcome;
use super::move_agent::{Later, ALREADY_THERE, NOT_HOME};
use super::{refused, Input, Manager};
use crate::drive::codex::requests::visible;
use crate::moves::vendor_name;

/// The card's question, which clients know it by.
pub const QUESTION: &str = "agent-move";
/// Runs the move.
pub const APPROVE: &str = "Approve";
/// Refuses it.
pub const DECLINE: &str = "Decline";
/// What the card asks for.
const TITLE: &str = "Move an agent to another device";

/// What a call reads when you say no.
pub(super) const DECLINED: &str = "The user didn't approve this move.";
/// What a call reads when nobody answered in time.
pub(super) const UNANSWERED: &str = "The user didn't answer in time, so the agent didn't move.";
/// What a call reads when there's no feed to ask in.
const NO_FEED: &str = "A move waits for the user's approval in MaxHQ, and this call has no \
                       agent's feed to ask in.";
/// What an agent reads when it asks to move itself.
const ITSELF: &str = "You can't move yourself while you work. Ask the user to move you, from \
                      your menu in MaxHQ.";

/// A move waiting for your answer.
pub(super) struct Asking {
    event: AttentionId,
    /// The card in the feed that shows it.
    item: TimelineItem,
    /// The call's key, which hears how the move went.
    key: IdempotencyKey,
    agent: AgentId,
    from: DeviceId,
    to: MoveTo,
    pub(super) until: Instant,
}

/// What both devices said about a move a tool asked for, before the card.
pub(super) struct Looked {
    key: IdempotencyKey,
    caller: Calling,
    permission: PermissionId,
    feed: AgentId,
    agent: AgentId,
    from: DeviceId,
    to: MoveTo,
    /// Whether the agent can leave, as its device says.
    look: ActionOutcome,
    /// What it will run as there, as the other device says.
    check: ActionOutcome,
}

impl Manager {
    /// Runs a tool's move: asks both devices, then asks you on a card.
    /// The call answers with the new agent once the move ran, or why not.
    pub(super) fn move_tool(
        &mut self,
        reply: Reply,
        caller: Calling,
        permission: &Permission,
        asked: Move,
    ) {
        let Move {
            request_id,
            device,
            agent,
            to,
        } = asked;
        let from = device.unwrap_or(self.device);
        let key = request_key(caller.actor, &request_id);
        let then = Then::Moved { device: to.device };
        if self.repeat(key) {
            return self.join(key, reply, then);
        }
        let feed = match self.may_ask_move(caller, permission, agent, from, &to) {
            Ok(feed) => feed,
            Err(reason) => return self.reply_now(reply, Err(reason)),
        };
        self.keys.begin(key, NO_CONN);
        self.join(key, reply, then);
        let mover = self.mover();
        let tx = self.tx.clone();
        let permission = permission.id;
        let check = AgentMoveStep::Check { to: to.clone() };
        tokio::spawn(async move {
            let (look, check) = tokio::join!(
                mover.step(from, AgentMoveStep::Look { agent }),
                mover.step(to.device, check),
            );
            let looked = Looked {
                key,
                caller,
                permission,
                feed,
                agent,
                from,
                to,
                look,
                check,
            };
            let _ = tx.send(Input::Move(Box::new(Later::Looked(Box::new(looked)))));
        });
    }

    /// Why a caller can't ask for this move, or the feed that asks you.
    fn may_ask_move(
        &self,
        caller: Calling,
        permission: &Permission,
        agent: AgentId,
        from: DeviceId,
        to: &MoveTo,
    ) -> Result<AgentId, String> {
        if self.agent_tools.peers.home().unwrap_or(self.device) != self.device {
            return Err(NOT_HOME.into());
        }
        if from == to.device {
            return Err(ALREADY_THERE.into());
        }
        let now = self.clock.wall();
        for device in [from, to.device] {
            rules::check(Some(permission), caller.rules(), &Ask::Act { device }, now)
                .map_err(|refusal| refusal.to_string())?;
        }
        if from == self.device && caller.agent.is_some_and(|me| self.lead_of(me) == agent) {
            return Err(ITSELF.into());
        }
        self.feed_of_actor(caller.actor)
            .ok_or_else(|| NO_FEED.into())
    }

    /// Opens the card for a move both devices allow, under the caller's
    /// permission as it is now, or refuses the call.
    pub(super) fn move_looked(&mut self, looked: Looked) {
        let Looked {
            key,
            caller,
            permission,
            feed,
            agent,
            from,
            to,
            look,
            check,
        } = looked;
        let departure = match look {
            ActionOutcome::Departed { departure } => departure,
            other => return self.finish(key, Outcome::Action(refused(failure(&other)))),
        };
        let check = match check {
            ActionOutcome::Checked { check } => check,
            other => return self.finish(key, Outcome::Action(refused(failure(&other)))),
        };
        let Some(permission) = self.agent_tools.permissions.get(&permission).cloned() else {
            return self.finish(key, Outcome::Action(refused(NOT_ALLOWED)));
        };
        let start = rules::Start {
            device: to.device,
            vendor: check.vendor,
            account: to.account,
            place: &to.place,
            workspace: Some(check.workspace),
            mode: None,
            counts: None,
        };
        let allowed =
            rules::check_move(&permission, caller.rules(), from, start, self.clock.wall());
        if let Err(refusal) = allowed {
            return self.finish(key, Outcome::Action(refused(refusal.to_string())));
        }
        let follow = self.following(from, agent).len();
        let (summary, question) = move_question(&departure, &check, follow);
        let (event, item) = self.open_timed_card(feed, QUESTION, summary, question);
        self.moves.asking.push(Asking {
            event,
            item,
            key,
            agent,
            from,
            to,
            until: self.clock.now() + Duration::from_secs(WAIT),
        });
    }

    /// Whether an event is a move's card that waits.
    pub(super) fn is_move_card(&self, event: AttentionId) -> bool {
        self.moves.asking.iter().any(|asking| asking.event == event)
    }

    /// Answers a move's card. Only a person can, so an agent's tool route
    /// hears there's no such event, and an answer it doesn't offer leaves
    /// it open.
    pub(super) fn answer_move_card(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        event: AttentionId,
        answer: &AttentionAnswer,
        by: ClientId,
    ) -> AnswerOutcome {
        let person = self.clients.get(&conn).is_some_and(|viewer| viewer.person);
        let Some(index) = self
            .moves
            .asking
            .iter()
            .position(|asking| asking.event == event)
            .filter(|_| person)
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let Some(pick) = picked(answer, QUESTION).filter(|pick| [APPROVE, DECLINE].contains(pick))
        else {
            return AnswerOutcome::UnknownEvent;
        };
        let approve = pick == APPROVE;
        let values = vec![pick.to_owned()];
        let asking = self.moves.asking.swap_remove(index);
        self.record_answer(event, key, by);
        self.close_timed_card(asking.event, asking.item, QUESTION, Some(values));
        let outcome = if approve {
            self.move_agent(asking.key, asking.agent, asking.from, asking.to, false)
        } else {
            Some(refused(DECLINED))
        };
        if let Some(outcome) = outcome {
            self.finish(asking.key, Outcome::Action(outcome));
        }
        AnswerOutcome::Accepted
    }

    /// Refuses the moves whose cards nobody answered in time.
    pub(super) fn expire_move_cards(&mut self) {
        let now = self.clock.now();
        let ended: Vec<Asking> = self
            .moves
            .asking
            .extract_if(.., |asking| asking.until <= now)
            .collect();
        for asking in ended {
            self.close_timed_card(asking.event, asking.item, QUESTION, None);
            self.finish(asking.key, Outcome::Action(refused(UNANSWERED)));
        }
    }
}

/// The card's inbox line and its question, for a move of the agent that
/// `departure` describes to where `check` says, with `follow` tasks.
fn move_question(departure: &Departure, check: &MoveCheck, follow: usize) -> (String, Question) {
    let title = visible(&departure.title);
    let from = visible(&departure.device);
    let to = visible(&check.device);
    let mut risk = format!(
        "{title} stops on {from} and settles. A new agent starts on {to} under the same name, \
         from a summary of the conversation. Files and uncommitted changes in its folder stay \
         on {from}."
    );
    match follow {
        0 => {}
        1 => risk.push_str(" 1 automation will follow it."),
        n => risk.push_str(&format!(" {n} automations will follow it.")),
    }
    let facts = vec![
        ("Agent", title.clone()),
        ("From", from),
        ("To", to.clone()),
        (
            "Runs as",
            format!("{}, {}", vendor_name(check.vendor), visible(&check.account)),
        ),
        ("Model", visible(&check.model)),
        ("Folder", visible(&check.place)),
        (
            "Carries over",
            "Only a summary of the conversation".to_owned(),
        ),
        ("Automations", follow.to_string()),
    ];
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
    };
    let choice = |label: &str, description: &str| QuestionOption {
        label: label.to_owned(),
        description: Some(description.to_owned()),
    };
    let summary: String = format!("Move {title} to {to}?").chars().take(80).collect();
    let question = Question {
        id: QUESTION.to_owned(),
        header: Some(TITLE.to_owned()),
        text,
        options: vec![
            choice(APPROVE, "Stops the agent and starts it again there."),
            choice(DECLINE, "Leaves the agent where it is."),
        ],
        multi: false,
        other: false,
        secret: false,
        consent: Some(consent),
    };
    (summary, question)
}
