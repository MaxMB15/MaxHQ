// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Moving an agent to another device: the Mac runs the move, the agent
//! stops where it was and a new one starts on the other device from a
//! summary, under the same name, and the tasks follow it.

use maxhq_core::tools::agents::WAIT;
use maxhq_protocol::agents::{
    Actor, AgentAt, LeaveContainer, Limits, MoveTo, Moved, PeerBook, PeerEntry, Permission, DEPTH,
};
use maxhq_protocol::dispatch::{PlanOutcome, PlanStep};
use maxhq_protocol::ids::PermissionId;
use maxhq_protocol::tasks::TaskTarget;
use maxhq_protocol::timeline::{Question, QuestionAnswer};
use serde_json::{json, Value};

use super::tasks::{hourly, task};
use super::*;
use crate::session::move_agent::NOT_HOME;
use crate::session::move_tool::{APPROVE, DECLINE, DECLINED, QUESTION, UNANSWERED};

/// What a move of a working agent answers.
const WORKING: &str = "The agent is working. Wait for its turn to end, or stop it and move it.";

/// The device agents move to.
pub(super) fn devbox() -> Device {
    Device {
        id: DeviceId::derive("devbox"),
        name: "devbox".into(),
        os: "linux".into(),
        ..device()
    }
}

/// The Mac, which runs each move, and devbox, which answers its line.
fn pair() -> (Harness, Harness) {
    let mac = Harness::new();
    let devbox = Harness::on(devbox());
    mac.fakes.peers.answer_with(&devbox.sessions, mac.host.id);
    (mac, devbox)
}

impl Harness {
    /// A move to this device's Claude account, in its project folder.
    fn move_here(&self) -> MoveTo {
        MoveTo {
            device: self.host.id,
            account: self.claude,
            model: None,
            place: AgentPlace::Folder {
                path: self.folder.display().to_string(),
            },
        }
    }

    /// What a tool asks for to move `agent` here.
    fn move_ask(&self, key: &str, agent: AgentId) -> Value {
        json!({
            "request_id": key,
            "agent": agent,
            "to_device": self.host.id,
            "account": self.claude,
            "folder": self.folder.display().to_string(),
        })
    }

    /// The move cards open now.
    fn move_cards(&self) -> Vec<AttentionEvent> {
        self.hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Attention(event) if event.summary.starts_with("Move ") => Some(event),
                _ => None,
            })
            .collect()
    }

    /// Waits for the one move card there should be.
    async fn move_card(&self) -> AttentionEvent {
        eventually("a move card", || !self.move_cards().is_empty()).await;
        let cards = self.move_cards();
        assert_eq!(cards.len(), 1, "{cards:#?}");
        cards.into_iter().next().expect("there's one")
    }

    /// Answers a move's card as a person.
    async fn answer_move(&mut self, event: AttentionId, label: &str) -> AnswerOutcome {
        let key = IdempotencyKey::generate();
        self.phone.send(ClientMessage::Answer {
            idempotency_key: key,
            event,
            answer: AttentionAnswer::Questions {
                answers: vec![QuestionAnswer {
                    id: QUESTION.into(),
                    values: vec![label.into()],
                }],
            },
            refs: Vec::new(),
        });
        self.phone.answered(key).await
    }

    /// The question the last move card showed in `agent`'s feed.
    async fn move_question(&mut self, agent: AgentId) -> Question {
        let items = match self.open(agent, None, 200).await {
            TimelineUpdate::Reset { items, .. } => items,
            other => panic!("expected a reset, got {other:?}"),
        };
        items
            .iter()
            .rev()
            .find_map(|item| match &item.body {
                ItemBody::Question { questions, .. } => questions
                    .first()
                    .filter(|question| question.id == QUESTION)
                    .cloned(),
                _ => None,
            })
            .expect("the feed has the card's question")
    }

    /// Moves `agent` from `from` as `to` says.
    async fn move_agent(
        &mut self,
        agent: AgentId,
        from: DeviceId,
        to: MoveTo,
        stop: bool,
    ) -> ActionOutcome {
        self.act(|key| ClientMessage::MoveAgent {
            key,
            agent,
            from,
            to,
            stop,
        })
        .await
    }
}

/// Lets `lead` on the Mac run agents on both devices, as each keeps it.
async fn allow_both(mac: &mut Harness, devbox: &mut Harness, lead: AgentId) {
    let permission = Permission {
        id: PermissionId::derive("p"),
        holder: Actor::Agent { agent: lead },
        home: mac.host.id,
        devices: vec![mac.host.id, devbox.host.id],
        vendors: vec![Vendor::Claude],
        accounts: None,
        places: None,
        depth: DEPTH,
        limits: Limits::default(),
        test_kinds: Vec::new(),
        leave_container: LeaveContainer::Ask,
    };
    devbox.give(permission.clone()).await;
    mac.give(permission).await;
}

/// The agent a move started, from what the call answered.
fn moved_to(answer: &str) -> AgentId {
    let answer: Value = serde_json::from_str(answer).expect("the answer is JSON");
    serde_json::from_value(answer["agent"].clone()).expect("it names the new agent")
}

#[tokio::test]
async fn an_agent_moves_with_a_summary_its_name_and_its_tasks() {
    let (mut mac, mut devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    let here = mac.host.id;
    let target = TaskTarget::Agent {
        device: here,
        agent: old,
    };
    let nightly = mac.save(task("nightly", hourly(), target)).await;

    let to = devbox.move_here();
    let new = started(mac.move_agent(old, here, to, false).await);

    // The old agent stopped, settled and points at the new one.
    eventually("the old CLI stopping", || {
        !mac.holders.stops(session).is_empty()
    })
    .await;
    let there = AgentAt {
        device: devbox.host.id,
        agent: new,
    };
    let moved = Some(Box::new(Moved {
        to: Some(there),
        from: None,
    }));
    assert_eq!(mac.agent(old).moved, moved);
    assert_eq!(mac.stored(old).moved, moved);
    assert!(mac.hub.inbox(old).settled.is_some());

    // The new one has its name and points back.
    let shown = devbox.agent(new);
    assert_eq!(shown.title, mac.agent(old).title);
    let back = AgentAt {
        device: here,
        agent: old,
    };
    assert_eq!(
        shown.moved,
        Some(Box::new(Moved {
            to: None,
            from: Some(back),
        }))
    );

    // Its first message carries the conversation in front of it.
    assert_eq!(devbox.send(new, "Ship it").await, ActionOutcome::Done);
    let server = devbox.holders.last();
    eventually("the first message", || !devbox.sent(server).is_empty()).await;
    let sent = devbox.sent(server);
    assert!(sent[0].starts_with("<maxhq_handoff>\n"), "{}", sent[0]);
    assert!(sent[0].contains("Built it"), "{}", sent[0]);
    assert!(sent[0].ends_with("User message:\nShip it"), "{}", sent[0]);

    // The task follows it, and devbox keeps a copy of its permission.
    let follows = mac.published_task(nightly.id).expect("the task stays");
    assert_eq!(
        follows.target,
        TaskTarget::Agent {
            device: devbox.host.id,
            agent: new,
        }
    );
    let kept = devbox.db.permissions().expect("permissions can be read");
    assert!(kept
        .iter()
        .any(|(permission, _)| permission.id == nightly.permission));
}

#[tokio::test]
async fn an_agent_on_another_device_moves_here_and_its_device_gives_back_the_tasks_copy() {
    let (mut mac, mut devbox) = pair();
    let (old, _) = devbox.claude_after_a_turn().await;
    let away = TaskTarget::Agent {
        device: devbox.host.id,
        agent: old,
    };
    let nightly = mac.save(task("nightly", hourly(), away)).await;
    let copy = maxhq_core::agents::task_permission(&nightly, mac.host.id);
    devbox.give(copy).await;

    let to = mac.move_here();
    let new = started(mac.move_agent(old, devbox.host.id, to, false).await);

    let here = AgentAt {
        device: mac.host.id,
        agent: new,
    };
    assert_eq!(
        devbox.agent(old).moved.and_then(|moved| moved.to),
        Some(here)
    );
    assert_eq!(
        mac.published_task(nightly.id).map(|task| task.target),
        Some(TaskTarget::Agent {
            device: mac.host.id,
            agent: new,
        })
    );
    let kept = devbox.db.permissions().expect("permissions can be read");
    assert!(!kept
        .iter()
        .any(|(permission, _)| permission.id == nightly.permission));
}

#[tokio::test]
async fn a_busy_agent_moves_only_when_told_to_stop() {
    let (mut mac, devbox) = pair();
    let (old, session) = mac.start_claude(Some("Build it")).await;
    let say = Say {
        agents: vec![mac.reported(old, Activity::Busy)],
        timeline: vec![upsert(
            old,
            reply(old, "a1", "Building it", ItemStatus::Running),
        )],
        ..Say::default()
    };
    mac.say(session, &say).await;
    let here = mac.host.id;

    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    assert_eq!(reason(&outcome), WORKING);
    assert!(mac.holders.stops(session).is_empty());
    assert_eq!(mac.agent(old).moved, None);

    let new = started(mac.move_agent(old, here, devbox.move_here(), true).await);
    assert!(!mac.holders.stops(session).is_empty());
    assert_eq!(
        mac.agent(old).moved.and_then(|moved| moved.to),
        Some(AgentAt {
            device: devbox.host.id,
            agent: new,
        })
    );
}

#[tokio::test]
async fn a_move_checks_the_device_it_goes_to_before_it_stops_the_agent() {
    let (mut mac, mut devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    let here = mac.host.id;
    devbox.claude_model(&[], "");

    let nobody = MoveTo {
        account: AccountId::derive("nobody"),
        ..devbox.move_here()
    };
    let outcome = mac.move_agent(old, here, nobody, false).await;
    assert_eq!(reason(&outcome), "That account isn't on that device.");

    let model = MoveTo {
        model: Some("haiku".into()),
        ..devbox.move_here()
    };
    let outcome = mac.move_agent(old, here, model, false).await;
    assert_eq!(
        reason(&outcome),
        "Claude doesn't offer haiku under that account."
    );

    let gone = MoveTo {
        place: AgentPlace::Folder {
            path: devbox.folder.join("gone").display().to_string(),
        },
        ..devbox.move_here()
    };
    let outcome = mac.move_agent(old, here, gone, false).await;
    assert_eq!(reason(&outcome), "that folder doesn't exist");

    mac.fakes.peers.set_down(true);
    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    assert_eq!(reason(&outcome), "no line to that device");

    // The agent never stopped, and nothing started on devbox.
    assert!(mac.holders.stops(session).is_empty());
    assert_eq!(mac.agent(old).moved, None);
    assert!(devbox.holders.sessions().is_empty());
}

#[tokio::test]
async fn a_start_that_fails_leaves_the_old_agent_stopped_and_unmarked() {
    let (mut mac, devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    let here = mac.host.id;
    let target = TaskTarget::Agent {
        device: here,
        agent: old,
    };
    let nightly = mac.save(task("nightly", hourly(), target.clone())).await;
    devbox.holders.fail_spawns(1);

    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    let said = reason(&outcome);
    assert!(
        said.starts_with("The agent stopped, but the new one didn't start there: "),
        "{said}"
    );
    assert!(
        said.ends_with("It stays where it was, and a message resumes it."),
        "{said}"
    );
    assert!(!mac.holders.stops(session).is_empty());
    assert_eq!(mac.agent(old).moved, None);
    assert!(mac.hub.inbox(old).settled.is_none());
    assert_eq!(
        mac.published_task(nightly.id).map(|task| task.target),
        Some(target)
    );

    // A message resumes it where it was.
    assert_eq!(mac.send(old, "Go on").await, ActionOutcome::Done);
}

#[tokio::test]
async fn only_the_device_that_runs_the_app_moves_agents() {
    let (mut mac, devbox) = pair();
    let (old, session) = mac.claude_after_a_turn().await;
    mac.fakes.peers.set_home(devbox.host.id);
    let here = mac.host.id;

    let outcome = mac.move_agent(old, here, devbox.move_here(), false).await;
    assert_eq!(reason(&outcome), NOT_HOME);
    assert!(mac.holders.stops(session).is_empty());
}

#[tokio::test]
async fn a_tools_move_waits_for_your_approval_and_says_what_it_will_do() {
    let (mut mac, mut devbox) = pair();
    let (old, _) = mac.claude_after_a_turn().await;
    let target = TaskTarget::Agent {
        device: mac.host.id,
        agent: old,
    };
    mac.save(task("nightly", hourly(), target)).await;
    let (lead, session) = mac.start_claude(Some("Lead")).await;
    allow_both(&mut mac, &mut devbox, lead).await;

    let say = Say {
        tools: vec![(
            "m1".into(),
            lead,
            "move_agent".into(),
            devbox.move_ask("m1", old),
        )],
        ..Say::default()
    };
    mac.say(session, &say).await;
    let card = mac.move_card().await;
    let consent = mac
        .move_question(lead)
        .await
        .consent
        .expect("it's a consent card");
    let facts: Vec<(&str, &str)> = consent
        .facts
        .iter()
        .map(|fact| (fact.label.as_str(), fact.value.as_str()))
        .collect();
    let folder = devbox.folder.display().to_string();
    assert!(facts.contains(&("To", "devbox")), "{facts:?}");
    assert!(facts.contains(&("Folder", folder.as_str())), "{facts:?}");
    assert!(
        facts.contains(&("Carries over", "Only a summary of the conversation")),
        "{facts:?}"
    );
    assert!(facts.contains(&("Automations", "1")), "{facts:?}");
    assert!(
        consent.risk.ends_with(" 1 automation will follow it."),
        "{}",
        consent.risk
    );
    assert_eq!(
        mac.agent(old).moved,
        None,
        "nothing moves before you answer"
    );

    assert_eq!(
        mac.answer_move(card.id, APPROVE).await,
        AnswerOutcome::Accepted
    );
    let new = moved_to(&mac.answer("m1").await.expect("it moved"));
    assert_eq!(
        mac.agent(old).moved.and_then(|moved| moved.to),
        Some(AgentAt {
            device: devbox.host.id,
            agent: new,
        })
    );
    assert_eq!(devbox.agent(new).title, mac.agent(old).title);

    // Declining refuses the call, and so does a card nobody answers.
    let (other, _) = mac.claude_after_a_turn().await;
    let say = Say {
        tools: vec![(
            "m2".into(),
            lead,
            "move_agent".into(),
            devbox.move_ask("m2", other),
        )],
        ..Say::default()
    };
    mac.say(session, &say).await;
    let card = mac.move_card().await;
    assert_eq!(
        mac.answer_move(card.id, DECLINE).await,
        AnswerOutcome::Accepted
    );
    assert_eq!(mac.answer("m2").await, Err(DECLINED.into()));
    eventually("the card closes", || mac.move_cards().is_empty()).await;

    let say = Say {
        tools: vec![(
            "m3".into(),
            lead,
            "move_agent".into(),
            devbox.move_ask("m3", other),
        )],
        ..Say::default()
    };
    mac.say(session, &say).await;
    mac.move_card().await;
    mac.clock.advance(Duration::from_secs(WAIT + 1));
    assert_eq!(mac.answer("m3").await, Err(UNANSWERED.into()));
    eventually("the card closes", || mac.move_cards().is_empty()).await;
    assert_eq!(mac.agent(other).moved, None);
}

#[tokio::test]
async fn a_tools_move_of_a_working_agent_is_refused_before_any_card() {
    let (mut mac, mut devbox) = pair();
    let (old, session) = mac.start_claude(Some("Build it")).await;
    let say = Say {
        agents: vec![mac.reported(old, Activity::Busy)],
        timeline: vec![upsert(
            old,
            reply(old, "a1", "Building it", ItemStatus::Running),
        )],
        ..Say::default()
    };
    mac.say(session, &say).await;
    let (lead, lead_session) = mac.start_claude(Some("Lead")).await;
    allow_both(&mut mac, &mut devbox, lead).await;

    let asked = devbox.move_ask("m1", old);
    let answer = mac
        .call(lead_session, lead, "m1", "move_agent", asked)
        .await;
    assert_eq!(answer, Err(WORKING.into()));
    assert!(mac.move_cards().is_empty());
    assert!(mac.holders.stops(session).is_empty());
}

#[tokio::test]
async fn list_targets_with_a_device_asks_that_device() {
    let (mut mac, mut devbox) = pair();
    let (lead, session) = mac.start_claude(Some("Lead")).await;
    allow_both(&mut mac, &mut devbox, lead).await;

    let asked = json!({"device": devbox.host.id});
    let there = mac
        .call(session, lead, "t1", "list_targets", asked)
        .await
        .expect("devbox's targets");
    let there: Value = serde_json::from_str(&there).unwrap();
    let accounts: Vec<&Value> = there["accounts"].as_array().unwrap().iter().collect();
    assert!(
        accounts
            .iter()
            .all(|account| account["device"] == json!(devbox.host.id)),
        "{there}"
    );
    assert!(!accounts.is_empty(), "{there}");
    let calls = mac.fakes.peers.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, devbox.host.id);
}

#[tokio::test]
async fn the_dispatcher_lists_another_device_and_plans_a_move_to_it() {
    let mut mac = Harness::with_dispatcher().await;
    let devbox = Harness::on(devbox());
    mac.fakes.peers.answer_with(&devbox.sessions, mac.host.id);
    let book = PeerBook {
        ssh: true,
        peers: vec![PeerEntry {
            device: devbox.host.id,
            name: "devbox".into(),
            routes: Vec::new(),
            host_key: String::new(),
            test_kinds: Vec::new(),
            home: false,
            model_server: None,
            models_from: Vec::new(),
        }],
    };
    mac.db.set_peer_book(&book).expect("the book is kept");
    let (old, _) = mac.claude_after_a_turn().await;
    let (dispatcher, session) = mac.dispatch("Move my agent to devbox").await;

    let here = mac
        .call(session, dispatcher, "t1", "list_targets", json!({}))
        .await
        .expect("this Mac's targets");
    let here: Value = serde_json::from_str(&here).unwrap();
    assert_eq!(
        here["other_devices"],
        json!([{"device": devbox.host.id, "name": "devbox"}])
    );
    let asked = json!({"device": devbox.host.id});
    let there = mac
        .call(session, dispatcher, "t2", "list_targets", asked)
        .await
        .expect("devbox's targets");
    let there: Value = serde_json::from_str(&there).unwrap();
    assert_eq!(there["device"]["name"], "devbox");
    assert!(there["accounts"]
        .to_string()
        .contains(&devbox.claude.to_string()));
    assert_eq!(there["agents"], json!([]));

    let to_devbox = json!({"type": "move_agent", "reason": "It runs on devbox from now on",
        "agent": old, "to_device": devbox.host.id, "account": devbox.claude,
        "folder": devbox.folder.display().to_string()});
    let mut here_again = to_devbox.clone();
    here_again["to_device"] = json!(mac.host.id);
    let refused = mac
        .call(
            session,
            dispatcher,
            "p0",
            "submit_plan",
            json!({"items": [here_again]}),
        )
        .await;
    assert_eq!(
        refused,
        Err("Item 0: The agent is on that device already.".into())
    );
    mac.call(
        session,
        dispatcher,
        "p1",
        "submit_plan",
        json!({"items": [to_devbox]}),
    )
    .await
    .expect("the plan shows");
    let plan = mac.dispatcher().plan.expect("the plan is shown");
    let PlanStep::MoveAgent { device, agent, to } = &plan.items[0].step else {
        panic!("a move: {plan:?}");
    };
    assert_eq!(
        (*device, *agent, to.device),
        (mac.host.id, old, devbox.host.id)
    );
    assert_eq!(
        mac.agent(old).moved,
        None,
        "nothing runs before you confirm"
    );

    let outcome = mac
        .act(|key| ClientMessage::ConfirmPlan { key, plan: plan.id })
        .await;
    let ActionOutcome::PlanRan { results } = outcome else {
        panic!("the plan didn't run: {outcome:?}");
    };
    let [PlanOutcome::Started { agent: new }] = results[..] else {
        panic!("not a move: {results:?}");
    };
    assert_eq!(
        mac.agent(old).moved.and_then(|moved| moved.to),
        Some(AgentAt {
            device: devbox.host.id,
            agent: new,
        })
    );
}
