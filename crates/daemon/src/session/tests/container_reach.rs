// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! An agent in a container reaching outside it: what its permission's
//! Containers setting does with each call that leaves, and the card that
//! asks you on Ask.

use maxhq_core::tools::agents::WAIT as CARD_WAIT;
use maxhq_protocol::agents::{Actor, AgentOrigin, LeaveContainer, Limits, Permission, DEPTH};
use maxhq_protocol::ids::{KeyId, PermissionId, TaskId};
use maxhq_protocol::leases::TestKind;
use maxhq_protocol::tasks::{Maker, RunOutcome, TaskTarget};
use maxhq_protocol::timeline::{Question, QuestionAnswer};
use serde_json::{json, Value};

use super::restarts::{server, workspace};
use super::*;
use crate::session::consent::{ALLOW, DENY, QUESTION, REMEMBER};

/// What a call Never refuses reads, in part.
const KEPT_IN: &str = "doesn't let you reach outside it";
/// What a call you said no to reads, in part.
const SAID_NO: &str = "didn't allow this call to reach outside";

/// Another device, which the fake line to other devices answers here.
fn devbox() -> DeviceId {
    DeviceId::derive("devbox")
}

/// The permission the lead holds.
fn held() -> PermissionId {
    PermissionId::derive("p")
}

/// A container with a lead agent in it, and an idle agent on the host.
struct Boxed {
    h: Harness,
    /// The agent in the container, which holds the permission.
    lead: AgentId,
    /// Its holder in the container.
    session: SessionId,
    on_host: AgentId,
    host_session: SessionId,
}

/// A lead in a container whose permission is set to `leave`.
async fn boxed_lead(leave: LeaveContainer) -> Boxed {
    let mut h = Harness::on(server());
    h.container(None);
    let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
    let (on_host, host_session) = h.start_claude(Some("On the host")).await;
    h.moves(host_session, on_host, Activity::Idle).await;
    h.reach_as(lead, leave).await;
    Boxed {
        h,
        lead,
        session,
        on_host,
        host_session,
    }
}

/// Each call that reaches outside the container.
fn outside(h: &Harness, on_host: AgentId) -> Vec<(&'static str, Value)> {
    let folder = h.folder.display().to_string();
    vec![
        (
            "start_agent",
            json!({"task": "t", "vendor": "claude", "folder": folder}),
        ),
        (
            "start_agent",
            json!({"task": "t", "vendor": "claude", "device": devbox(), "workspace": workspace()}),
        ),
        ("send_agent", json!({"agent": on_host, "text": "hello"})),
        (
            "send_agent",
            json!({"agent": on_host, "text": "hello", "device": devbox()}),
        ),
        ("read_agent", json!({"agent": on_host})),
        ("read_agent", json!({"agent": on_host, "device": devbox()})),
        ("wait_agent", json!({"agents": [on_host]})),
        ("interrupt_agent", json!({"agent": on_host})),
        ("lease_open", json!({"kind": "ios_simulator"})),
    ]
}

impl Harness {
    /// Gives `lead` its permission, here and on devbox, set to `leave`.
    async fn reach_as(&mut self, lead: AgentId, leave: LeaveContainer) -> Permission {
        let permission = Permission {
            id: held(),
            holder: Actor::Agent { agent: lead },
            home: self.host.id,
            devices: vec![self.host.id, devbox()],
            vendors: vec![Vendor::Claude, Vendor::Codex],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: vec![TestKind::IosSimulator],
            leave_container: leave,
        };
        self.give(permission).await
    }

    /// `agent`, in the container, calls `tool` and doesn't wait for its
    /// answer.
    async fn ask_in(
        &self,
        session: SessionId,
        agent: AgentId,
        key: &str,
        tool: &str,
        mut arguments: Value,
    ) {
        arguments["request_id"] = key.into();
        let say = Say {
            tools: vec![(key.into(), agent, tool.into(), arguments)],
            ..Say::default()
        };
        self.say_in(session, &say).await;
    }

    /// `agent`, in the container, calls `tool`. Gives what it answered.
    async fn call_in(
        &self,
        session: SessionId,
        agent: AgentId,
        key: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<String, String> {
        self.ask_in(session, agent, key, tool, arguments).await;
        self.answer(key).await
    }

    /// Whether the tool call `key` was answered yet.
    fn replied(&self, key: &str) -> bool {
        let prefix = format!("tool {key} ");
        self.vendors
            .log()
            .iter()
            .any(|entry| entry.starts_with(&prefix))
    }

    /// The container reach cards open now.
    fn reach_cards(&self) -> Vec<AttentionEvent> {
        self.hub
            .snapshot()
            .entities
            .into_iter()
            .filter_map(|entity| match entity {
                Entity::Attention(event)
                    if event.summary.contains("reach outside its container") =>
                {
                    Some(event)
                }
                _ => None,
            })
            .collect()
    }

    /// Waits for the one card there should be.
    async fn reach_card(&self) -> AttentionEvent {
        eventually("a card", || !self.reach_cards().is_empty()).await;
        let cards = self.reach_cards();
        assert_eq!(cards.len(), 1, "{cards:#?}");
        cards.into_iter().next().expect("there's one")
    }

    /// Answers a card with one of its labels, with its box ticked when
    /// `remember` is set.
    async fn pick_reach(
        &mut self,
        event: AttentionId,
        label: &str,
        remember: bool,
    ) -> AnswerOutcome {
        let key = IdempotencyKey::generate();
        let mut answers = vec![QuestionAnswer {
            id: QUESTION.into(),
            values: vec![label.into()],
        }];
        if remember {
            answers.push(QuestionAnswer {
                id: REMEMBER.into(),
                values: vec!["Remember for this permission".into()],
            });
        }
        self.phone.send(ClientMessage::Answer {
            idempotency_key: key,
            event,
            answer: AttentionAnswer::Questions { answers },
            refs: Vec::new(),
        });
        self.phone.answered(key).await
    }

    /// The last card the agent's timeline shows, as an item. A timeline
    /// opened before still sends the changes since, ahead of the reset.
    async fn reach_item(&mut self, agent: AgentId) -> TimelineItem {
        let mut update = self.open(agent, None, 200).await;
        let items = loop {
            match update {
                TimelineUpdate::Reset { items, .. } => break items,
                TimelineUpdate::Events { .. } => update = self.phone.timeline(agent).await,
                other => panic!("expected a reset, got {other:?}"),
            }
        };
        items
            .into_iter()
            .rev()
            .find(|item| {
                matches!(&item.body, ItemBody::Question { questions, .. }
                    if questions.iter().any(|q| q.id == QUESTION))
            })
            .expect("the timeline has the card")
    }

    /// The question on the agent's last card.
    async fn reach_question(&mut self, agent: AgentId) -> Question {
        match self.reach_item(agent).await.body {
            ItemBody::Question { mut questions, .. } => questions.remove(0),
            _ => unreachable!("it's a question"),
        }
    }

    /// What the lead's permission says now, as this device keeps it.
    fn leave_now(&self) -> LeaveContainer {
        self.leave_of(held())
    }

    /// What a permission says now, as this device keeps it.
    fn leave_of(&self, id: PermissionId) -> LeaveContainer {
        let stored = self.db.permissions().unwrap();
        let (found, _) = stored.iter().find(|(p, _)| p.id == id).unwrap();
        found.leave_container
    }
}

#[tokio::test]
async fn never_refuses_every_call_that_leaves_and_keeps_the_ones_inside() {
    let Boxed {
        mut h,
        lead,
        session,
        on_host,
        ..
    } = boxed_lead(LeaveContainer::Never).await;
    let (beside, _) = h.start_boxed_claude(Some("Beside it")).await;
    for (n, (tool, arguments)) in outside(&h, on_host).into_iter().enumerate() {
        let key = format!("never-{n}");
        let answer = h
            .call_in(session, lead, &key, tool, arguments.clone())
            .await;
        let reason = answer.expect_err(&format!("{tool} {arguments} went"));
        assert!(reason.contains(KEPT_IN), "{tool}: {reason}");
    }
    assert!(h.reach_cards().is_empty(), "Never asks nobody");
    assert!(h.fakes.peers.calls().is_empty(), "nothing went to devbox");
    assert!(h.fakes.leases.calls().is_empty(), "nothing was leased");

    // Inside its container it works as before.
    let sent = json!({"agent": beside, "text": "hello"});
    h.call_in(session, lead, "in-send", "send_agent", sent)
        .await
        .expect("a message inside goes");
    let start = json!({"task": "t", "vendor": "claude"});
    h.call_in(session, lead, "in-start", "start_agent", start)
        .await
        .expect("a start inside goes");
    let read = json!({"agent": beside});
    let read = h
        .call_in(session, lead, "in-read", "read_agent", read)
        .await;
    assert!(!read.is_err_and(|reason| reason.contains(KEPT_IN)));
}

#[tokio::test]
async fn always_allow_lets_every_call_that_leaves_go_without_a_card() {
    let Boxed {
        h,
        lead,
        session,
        on_host,
        ..
    } = boxed_lead(LeaveContainer::Allow).await;
    for (n, (tool, arguments)) in outside(&h, on_host).into_iter().enumerate() {
        let key = format!("allow-{n}");
        let answer = h
            .call_in(session, lead, &key, tool, arguments.clone())
            .await;
        assert!(
            !answer
                .as_ref()
                .is_err_and(|reason| reason.contains(KEPT_IN)),
            "{tool} {arguments}: {answer:?}"
        );
    }
    assert!(h.reach_cards().is_empty(), "Always allow asks nobody");
    let peers = h.fakes.peers.calls();
    assert_eq!(
        peers.len(),
        3,
        "a start, a message and a read went to devbox"
    );
    assert!(peers.iter().all(|(device, _)| *device == devbox()));
    assert_eq!(h.fakes.leases.calls().len(), 1, "the lease went");
}

#[tokio::test]
async fn ask_holds_every_call_that_leaves_until_you_answer() {
    let Boxed {
        mut h,
        lead,
        session,
        on_host,
        ..
    } = boxed_lead(LeaveContainer::Ask).await;
    for (n, (tool, arguments)) in outside(&h, on_host).into_iter().enumerate() {
        let key = format!("ask-{n}");
        h.ask_in(session, lead, &key, tool, arguments.clone()).await;
        let card = h.reach_card().await;
        assert_eq!(card.agent, lead);
        assert_eq!(card.kind, AttentionKind::Question);
        tokio::time::sleep(QUIET).await;
        assert!(!h.replied(&key), "{tool} waits for the card");
        let question = h.reach_question(lead).await;
        assert!(question.text.contains("mhq-box"), "{}", question.text);
        assert!(question.text.contains(tool), "{}", question.text);
        assert!(
            question.text.contains("prompt injection"),
            "{}",
            question.text
        );
        assert_eq!(
            h.pick_reach(card.id, DENY, false).await,
            AnswerOutcome::Accepted
        );
        let reason = h.answer(&key).await.expect_err("Deny refuses");
        assert!(reason.contains(SAID_NO), "{tool}: {reason}");
        eventually("the card closes", || h.reach_cards().is_empty()).await;
    }
    assert!(h.fakes.peers.calls().is_empty(), "nothing went to devbox");
    assert!(h.fakes.leases.calls().is_empty(), "nothing was leased");
    assert_eq!(
        h.leave_now(),
        LeaveContainer::Ask,
        "Deny alone keeps nothing"
    );

    // Allow lets the one call go, and only that one.
    for (n, (tool, arguments)) in outside(&h, on_host).into_iter().enumerate() {
        let key = format!("let-{n}");
        h.ask_in(session, lead, &key, tool, arguments.clone()).await;
        let card = h.reach_card().await;
        assert_eq!(
            h.pick_reach(card.id, ALLOW, false).await,
            AnswerOutcome::Accepted
        );
        let answer = h.answer(&key).await;
        assert!(
            !answer
                .as_ref()
                .is_err_and(|reason| reason.contains(KEPT_IN) || reason.contains(SAID_NO)),
            "{tool} {arguments}: {answer:?}"
        );
        eventually("the card closes", || h.reach_cards().is_empty()).await;
    }
    assert_eq!(h.fakes.peers.calls().len(), 3);
    assert_eq!(h.fakes.leases.calls().len(), 1);
    assert_eq!(
        h.leave_now(),
        LeaveContainer::Ask,
        "Allow alone keeps nothing"
    );
    let item = h.reach_item(lead).await;
    assert!(
        matches!(&item.body, ItemBody::Question { answers: Some(answers), .. }
            if answers[0].values == [ALLOW]),
        "{item:#?}"
    );
}

#[tokio::test]
async fn remember_keeps_the_answer_on_the_permission() {
    let Boxed {
        mut h,
        lead,
        session,
        on_host,
        host_session,
    } = boxed_lead(LeaveContainer::Ask).await;
    let send = json!({"agent": on_host, "text": "hello"});

    h.ask_in(session, lead, "r1", "send_agent", send.clone())
        .await;
    let card = h.reach_card().await;
    let question = h.reach_question(lead).await;
    let labels: Vec<&str> = question.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, [ALLOW, DENY]);
    let consent = question.consent.expect("the card says what it's for");
    assert_eq!(consent.title, "Reach outside the container");
    assert!(
        consent.risk.contains("prompt injection"),
        "{}",
        consent.risk
    );
    let fact = |label: &str| {
        consent
            .facts
            .iter()
            .find(|fact| fact.label == label)
            .map(|fact| fact.value.as_str())
    };
    assert_eq!(fact("Container"), Some("mhq-box"), "{:#?}", consent.facts);
    assert_eq!(fact("Call"), Some("send_agent"), "{:#?}", consent.facts);
    let remember = consent.remember.as_ref().expect("the card offers its box");
    assert_eq!(remember.id, REMEMBER);
    assert!(
        remember.settings.contains("Change what it can run"),
        "{}",
        remember.settings
    );
    assert!(
        remember.settings.contains("Containers"),
        "{}",
        remember.settings
    );
    assert_eq!(
        h.pick_reach(card.id, ALLOW, true).await,
        AnswerOutcome::Accepted
    );
    h.answer("r1").await.expect("the message goes");
    eventually("the message", || {
        h.sent(host_session)
            .iter()
            .any(|text| text.ends_with("hello"))
    })
    .await;
    assert_eq!(h.leave_now(), LeaveContainer::Allow);
    let item = h.reach_item(lead).await;
    assert!(
        matches!(&item.body, ItemBody::Question { answers: Some(answers), .. }
            if answers[0].values == [ALLOW, "Remember for this permission"]),
        "{item:#?}"
    );
    h.call_in(session, lead, "r2", "send_agent", send.clone())
        .await
        .expect("the next one goes without asking");
    assert!(h.reach_cards().is_empty());

    h.reach_as(lead, LeaveContainer::Ask).await;
    h.ask_in(session, lead, "r3", "send_agent", send.clone())
        .await;
    let card = h.reach_card().await;
    assert_eq!(
        h.pick_reach(card.id, DENY, true).await,
        AnswerOutcome::Accepted
    );
    let reason = h.answer("r3").await.expect_err("Deny refuses");
    assert!(reason.contains(SAID_NO), "{reason}");
    assert_eq!(h.leave_now(), LeaveContainer::Never);
    let reason = h
        .call_in(session, lead, "r4", "send_agent", send)
        .await
        .expect_err("the next one is refused without asking");
    assert!(reason.contains(KEPT_IN), "{reason}");
    assert!(h.reach_cards().is_empty());
}

#[tokio::test]
async fn a_ticked_box_keeps_nothing_on_a_permission_an_agent_doesnt_hold() {
    let mut h = Harness::on(server());
    h.container(None);
    // An access key holds the permission, and started the agent under it.
    let id = PermissionId::derive("k");
    let holder = Actor::Key {
        key: KeyId::derive("tool"),
    };
    h.give(Permission {
        id,
        holder,
        home: h.host.id,
        devices: vec![h.host.id],
        vendors: vec![Vendor::Claude],
        accounts: None,
        places: None,
        depth: DEPTH,
        limits: Limits::default(),
        test_kinds: Vec::new(),
        leave_container: LeaveContainer::Ask,
    })
    .await;
    let origin = AgentOrigin {
        permission: id,
        by: holder,
        device: h.host.id,
        level: 1,
        wake: false,
        container: None,
    };
    let (agent, session) = h
        .start_boxed_claude_under(Some("Plan it"), Some(origin))
        .await;
    let (on_host, host_session) = h.start_claude(Some("On the host")).await;
    h.moves(host_session, on_host, Activity::Idle).await;
    let send = json!({"agent": on_host, "text": "hello"});

    h.ask_in(session, agent, "b1", "send_agent", send.clone())
        .await;
    let card = h.reach_card().await;
    let consent = h
        .reach_question(agent)
        .await
        .consent
        .expect("a consent card");
    assert_eq!(
        consent.remember, None,
        "only an agent's permission keeps it"
    );
    // A client that sends the box anyway changes nothing.
    assert_eq!(
        h.pick_reach(card.id, ALLOW, true).await,
        AnswerOutcome::Accepted
    );
    h.answer("b1").await.expect("the message goes");
    eventually("the message", || {
        h.sent(host_session)
            .iter()
            .any(|text| text.ends_with("hello"))
    })
    .await;
    assert_eq!(h.leave_of(id), LeaveContainer::Ask);
    let item = h.reach_item(agent).await;
    assert!(
        matches!(&item.body, ItemBody::Question { answers: Some(answers), .. }
            if answers[0].values == [ALLOW]),
        "{item:#?}"
    );
    eventually("the card closes", || h.reach_cards().is_empty()).await;
    h.ask_in(session, agent, "b2", "send_agent", send).await;
    h.reach_card().await;
    assert!(!h.replied("b2"), "the next call asks again");
}

#[tokio::test]
async fn a_card_nobody_answers_refuses_its_call_once_the_wait_runs_out() {
    let Boxed {
        mut h,
        lead,
        session,
        on_host,
        host_session,
    } = boxed_lead(LeaveContainer::Ask).await;
    let send = json!({"agent": on_host, "text": "hello"});
    h.ask_in(session, lead, "t1", "send_agent", send).await;
    let card = h.reach_card().await;
    h.clock.advance(Duration::from_secs(CARD_WAIT - 1));
    tokio::time::sleep(QUIET).await;
    assert!(!h.replied("t1"), "it still waits");
    h.clock.advance(Duration::from_secs(2));
    let reason = h.answer("t1").await.expect_err("nobody answered");
    assert!(reason.contains("didn't answer in time"), "{reason}");
    eventually("the card closes", || h.reach_cards().is_empty()).await;
    assert_eq!(h.reach_item(lead).await.status, ItemStatus::Cancelled);
    // A late answer finds nothing to answer.
    assert_ne!(
        h.pick_reach(card.id, ALLOW, false).await,
        AnswerOutcome::Accepted
    );
    assert_eq!(h.leave_now(), LeaveContainer::Ask);
    assert!(!h
        .sent(host_session)
        .iter()
        .any(|text| text.ends_with("hello")));
}

#[tokio::test]
async fn a_sub_agent_calls_from_its_leads_container_and_asks_in_its_feed() {
    let Boxed {
        mut h,
        lead,
        session,
        on_host,
        ..
    } = boxed_lead(LeaveContainer::Never).await;
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy), sub.clone()],
        ..Say::default()
    };
    h.say_in(session, &say).await;
    let send = json!({"agent": on_host, "text": "hello"});
    let reason = h
        .call_in(session, sub.id, "s1", "send_agent", send.clone())
        .await
        .expect_err("the sub-agent stays in the container too");
    assert!(reason.contains(KEPT_IN), "{reason}");

    h.reach_as(lead, LeaveContainer::Ask).await;
    h.ask_in(session, sub.id, "s2", "send_agent", send).await;
    let card = h.reach_card().await;
    assert_eq!(card.agent, lead, "the card shows in the lead's feed");
    assert_eq!(
        h.pick_reach(card.id, DENY, false).await,
        AnswerOutcome::Accepted
    );
    assert!(h.answer("s2").await.is_err());
}

#[tokio::test]
async fn a_task_made_in_a_container_asks_when_it_runs_and_fails_unanswered() {
    let Boxed {
        mut h,
        lead,
        session,
        on_host,
        host_session,
    } = boxed_lead(LeaveContainer::Never).await;
    let hourly = json!({"type": "interval", "everyMs": 3_600_000});
    let aimed = json!({"prompt": "Check in", "schedule": hourly,
                       "target": {"type": "agent", "agent": on_host}});
    let reason = h
        .call_in(session, lead, "k1", "schedule_task", aimed.clone())
        .await
        .expect_err("Never refuses the task when it's made");
    assert!(reason.contains(KEPT_IN), "{reason}");

    // On Ask the task is made, and each run asks.
    h.reach_as(lead, LeaveContainer::Ask).await;
    let made = h
        .call_in(session, lead, "k2", "schedule_task", aimed)
        .await
        .expect("Ask makes the task");
    let made: Value = serde_json::from_str(&made).unwrap();
    let task: TaskId = serde_json::from_value(made["task"].clone()).unwrap();
    assert!(h.reach_cards().is_empty(), "making it asks nothing");
    let found = h
        .published()
        .tasks
        .into_iter()
        .find(|t| t.id == task)
        .unwrap();
    assert!(matches!(found.target, TaskTarget::Agent { agent, .. } if agent == on_host));
    assert!(
        matches!(found.made_by, Maker::Tools { origin } if origin.container == Some(workspace()))
    );

    let run = h.act(|key| ClientMessage::RunTaskNow { key, task }).await;
    assert_eq!(run, ActionOutcome::Done);
    let card = h.reach_card().await;
    assert_eq!(card.agent, lead, "the card shows in its maker's feed");
    assert!(card.summary.contains("the task"), "{}", card.summary);
    let question = h.reach_question(lead).await;
    assert!(
        question.text.contains("A run of the task"),
        "{}",
        question.text
    );
    assert!(question.text.contains("mhq-box"), "{}", question.text);

    h.clock.advance(Duration::from_secs(CARD_WAIT + 1));
    eventually("the run fails", || {
        h.db.runs(task).unwrap().iter().any(|run| {
            run.outcome == RunOutcome::Failed
                && run
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("waited for the owner"))
        })
    })
    .await;
    eventually("the card closes", || h.reach_cards().is_empty()).await;
    assert!(
        !h.sent(host_session)
            .iter()
            .any(|text| text.contains("Check in")),
        "the prompt never reached the host"
    );
}
