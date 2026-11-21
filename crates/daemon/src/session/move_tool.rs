// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `move_agent` tool. A move stops an agent you may have started, so
//! one a tool asks for waits for your answer on a card in the caller's
//! feed, as a task an agent makes does. Before the card opens, the device
//! the agent leaves says it can go and the device it goes to says what it
//! will run as there, so the card names both. The agent starts there in
//! the lower of the mode it ran in and that device's default, and a second
//! Approve starts it in the higher one. Approve runs the move as the app's
//! Move does. Decline refuses it, and so does a card nobody answers in
//! time. A move that crosses a container's wall says so, with the risk the
//! container's own card names. Cards live in memory, so restarting `maxd`
//! drops them with the calls they hold.

use std::time::{Duration, Instant};

use maxhq_core::agents::{self as rules, mode_name, mode_title, runnable, Ask, NOT_ALLOWED};
use maxhq_core::tools::agents::{Move, WAIT};
use maxhq_protocol::agents::{AgentMoveStep, Departure, MoveCheck, MoveTo, Permission};
use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::ids::{AgentId, AttentionId, ClientId, DeviceId, IdempotencyKey, PermissionId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::timeline::{Consent, ConsentFact, Question, QuestionOption, TimelineItem};

use super::agent_tools::{failure, request_key, Calling, Reply, Then, NO_CONN};
use super::consent::{goes_inside, runs_outside};
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
/// What a call reads when the device it goes to would start the agent in
/// a broader mode than the caller's.
const ABOVE_CALLER: &str = "That device's maxd is older and would start the agent in the mode it \
                            ran in, which is broader than yours. Update maxd there, or ask the \
                            user to move it.";
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
    modes: Modes,
    pub(super) until: Instant,
}

/// The modes a move's card offers: Approve's, and the higher one a second
/// Approve offers when it differs.
#[derive(Clone, Copy)]
struct Modes {
    approve: PermissionMode,
    higher: Option<PermissionMode>,
}

/// What a move's card says besides what both devices answered.
struct Shown {
    /// The names you gave the devices the agent leaves and goes to.
    from: String,
    to: String,
    /// How many automations follow it.
    follow: usize,
    modes: Modes,
    /// The containers it crosses: the caller's, the one the agent leaves
    /// and the one it goes into.
    asked_from: Option<String>,
    leaves: Option<String>,
    enters: Option<String>,
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
    /// The user's words the caller quotes for the mode it runs in there.
    words: Option<String>,
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
            user_words: words,
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
                words,
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
            words,
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
        let cap =
            match rules::check_move(&permission, caller.rules(), from, start, self.clock.wall()) {
                Ok(cap) => cap,
                Err(refusal) => {
                    return self.finish(key, Outcome::Action(refused(refusal.to_string())))
                }
            };
        let Some(modes) = self.card_modes(caller, words.as_deref(), &departure, &check, cap) else {
            return self.finish(key, Outcome::Action(refused(ABOVE_CALLER)));
        };
        let shown = Shown {
            from: self.booked_name(from),
            to: self.booked_name(to.device),
            follow: self.following(from, agent).len(),
            modes,
            asked_from: caller
                .container
                .map(|(_, workspace)| self.workspace_name(workspace)),
            leaves: self
                .container_of(from, agent)
                .map(|workspace| self.workspace_name(workspace))
                .or_else(|| departure.container.as_deref().map(visible)),
            enters: check.container.as_deref().map(visible),
        };
        let (summary, question) = move_question(&departure, &check, &shown);
        let (event, item) = self.open_timed_card(feed, QUESTION, summary, question);
        self.moves.asking.push(Asking {
            event,
            item,
            key,
            agent,
            from,
            to,
            modes,
            until: self.clock.now() + Duration::from_secs(WAIT),
        });
    }

    /// The modes a move's card offers. The device it goes to starts it in
    /// the lower of the mode it ran in and its own default unless you pick
    /// the higher, and neither goes past `cap`, the caller's own mode. A
    /// device from before that choice starts it in the mode it ran in, so
    /// that's the one shown, and none when it's broader than `cap`. When
    /// `words` the caller quotes are yours and name the higher mode,
    /// Approve takes it with no second choice.
    fn card_modes(
        &self,
        caller: Calling,
        words: Option<&str>,
        departure: &Departure,
        check: &MoveCheck,
        cap: Option<PermissionMode>,
    ) -> Option<Modes> {
        let capped = |mode| {
            runnable(
                check.vendor,
                cap.map_or(mode, |cap| rules::narrower(mode, cap)),
            )
        };
        let Some(default) = check.mode else {
            let ran = runnable(check.vendor, departure.mode);
            return (capped(ran) == ran).then_some(Modes {
                approve: ran,
                higher: None,
            });
        };
        let (lower, higher) = rules::move_modes(departure.mode, default);
        let (lower, higher) = (capped(lower), capped(higher));
        let asked = caller
            .agent
            .zip(words)
            .is_some_and(|(agent, words)| self.owner_asked_for(agent, words, higher));
        if asked {
            return Some(Modes {
                approve: higher,
                higher: None,
            });
        }
        Some(Modes {
            approve: lower,
            higher: (higher != lower).then_some(higher),
        })
    }

    /// The name you gave a device in the link book, or this device's own,
    /// rather than the one a device reports for itself.
    fn booked_name(&self, device: DeviceId) -> String {
        let name = if device == self.device {
            self.hub.device().name.clone()
        } else {
            self.agent_tools
                .peers
                .name_of(device)
                .unwrap_or_else(|| device.to_string())
        };
        visible(&name)
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
        let modes = self.moves.asking[index].modes;
        let (pick, mode) = match picked(answer, QUESTION) {
            Some(pick @ APPROVE) => (pick, Some(modes.approve)),
            Some(pick @ DECLINE) => (pick, None),
            Some(pick) if modes.higher.is_some_and(|mode| pick == approve_in(mode)) => {
                (pick, modes.higher)
            }
            _ => return AnswerOutcome::UnknownEvent,
        };
        let values = vec![pick.to_owned()];
        let asking = self.moves.asking.swap_remove(index);
        self.record_answer(event, key, by);
        self.close_timed_card(asking.event, asking.item, QUESTION, Some(values));
        let outcome = match mode {
            Some(mode) => {
                let to = MoveTo {
                    mode: Some(mode),
                    ..asking.to
                };
                self.move_agent(asking.key, asking.agent, asking.from, to, false)
            }
            None => Some(refused(DECLINED)),
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

/// The label of the Approve that starts the agent in `mode`, the higher
/// of the two.
fn approve_in(mode: PermissionMode) -> String {
    format!("{APPROVE} in {}", mode_title(mode))
}

/// The card's inbox line and its question, for a move of the agent that
/// `departure` describes to where `check` says, as `shown` adds.
fn move_question(departure: &Departure, check: &MoveCheck, shown: &Shown) -> (String, Question) {
    let title = visible(&departure.title);
    let Shown { from, to, .. } = shown;
    let Modes { approve, higher } = shown.modes;
    let mut risk = crossing(shown);
    risk.push_str(&format!(
        "{title} stops on {from} and settles. A new agent starts on {to} under the same name, \
         from a summary of the conversation. Files and uncommitted changes in its folder stay \
         on {from}."
    ));
    if let Some(higher) = higher {
        risk.push_str(&format!(
            " It runs in {} there, the lower of the mode it ran in and {to}'s default, unless \
             you pick {}.",
            mode_name(approve),
            approve_in(higher)
        ));
    }
    match shown.follow {
        0 => {}
        1 => risk.push_str(" 1 automation will follow it."),
        n => risk.push_str(&format!(" {n} automations will follow it.")),
    }
    let mut facts = vec![
        ("Agent", title.clone()),
        ("From", from.clone()),
        ("To", to.clone()),
        (
            "Runs as",
            format!("{}, {}", vendor_name(check.vendor), visible(&check.account)),
        ),
        ("Runs in", mode_title(approve)),
        ("Model", visible(&check.model)),
        ("Folder", visible(&check.place)),
        (
            "Carries over",
            "Only a summary of the conversation".to_owned(),
        ),
        ("Automations", shown.follow.to_string()),
    ];
    let containers = [
        ("Asked from container", &shown.asked_from),
        ("Leaves container", &shown.leaves),
        ("Goes into container", &shown.enters),
    ];
    for (label, name) in containers {
        if let Some(name) = name {
            facts.push((label, name.clone()));
        }
    }
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
    let choice = |label: String, description: String| QuestionOption {
        label,
        description: Some(description),
    };
    let mut options = vec![choice(
        APPROVE.to_owned(),
        format!(
            "Stops the agent and starts it again there, in {}.",
            mode_name(approve)
        ),
    )];
    if let Some(higher) = higher {
        options.push(choice(
            approve_in(higher),
            format!("Starts it there in {} instead.", mode_name(higher)),
        ));
    }
    options.push(choice(
        DECLINE.to_owned(),
        "Leaves the agent where it is.".to_owned(),
    ));
    let summary: String = format!("Move {title} to {to}?").chars().take(80).collect();
    let question = Question {
        id: QUESTION.to_owned(),
        header: Some(TITLE.to_owned()),
        text,
        options,
        multi: false,
        other: false,
        secret: false,
        consent: Some(consent),
    };
    (summary, question)
}

/// What the card says first when the move crosses a container's wall, or
/// nothing when it doesn't.
fn crossing(shown: &Shown) -> String {
    let risk = if shown.enters.is_some() {
        goes_inside()
    } else if shown.leaves.is_some() || shown.asked_from.is_some() {
        runs_outside("The new agent")
    } else {
        return String::new();
    };
    format!("This move crosses a container's wall. {risk} ")
}
