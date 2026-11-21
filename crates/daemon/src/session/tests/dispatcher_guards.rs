// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What keeps the dispatcher yours: only your request starts one, other
//! agents can't reach it, what it sends says it's from the dispatcher,
//! and what it starts or turns on past what you said waits for you.

use maxhq_protocol::agents::{Actor, LeaveContainer, Limits, Permission, DEPTH};
use maxhq_protocol::dispatch::{PlanOutcome, PlanStep};
use maxhq_protocol::ids::{PermissionId, TaskId};
use maxhq_protocol::tasks::Maker;
use serde_json::{json, Value};

use super::dispatcher::ASKS_FIRST;
use super::restarts::server;
use super::*;
use crate::session::agent_tools::{FOR_YOU, ONLY_THE_USER};
use crate::session::dispatcher::{DIR, OWN_FOLDER};
use crate::session::dispatcher_admin::{RAISED, UNDO_STEP};
use crate::session::scheduled::approval;

/// An agent or a task the dispatcher started or saved at once, as its
/// card shows it.
#[derive(Debug)]
struct StepCard {
    step: PlanStep,
    outcome: PlanOutcome,
    undo: Option<IdempotencyKey>,
}

impl Harness {
    /// The step cards on the dispatcher's timeline. Opens it, so call it
    /// once.
    async fn step_cards(&mut self, dispatcher: AgentId) -> Vec<StepCard> {
        let (_, items, _) = reset(self.open(dispatcher, None, 50).await);
        items
            .into_iter()
            .filter_map(|item| match item.body {
                ItemBody::DispatcherStep {
                    step,
                    outcome,
                    undo,
                } => Some(StepCard {
                    step: *step,
                    outcome,
                    undo,
                }),
                _ => None,
            })
            .collect()
    }

    /// The dispatcher's start of a Claude agent in the test folder, in
    /// `mode`, quoting `words` as yours.
    async fn start_in(
        &self,
        session: SessionId,
        dispatcher: AgentId,
        key: &str,
        mode: &str,
        words: Option<&str>,
    ) -> Result<String, String> {
        let mut start = json!({"request_id": key, "task": "Review it", "vendor": "claude",
            "folder": self.folder.display().to_string(), "mode": mode, "asked": true});
        if let Some(words) = words {
            start["user_words"] = words.into();
        }
        self.call(session, dispatcher, key, "start_agent", start)
            .await
    }
}

/// The agent a dispatcher's "Started agent ..." answer names.
fn started_by(answer: &str) -> AgentId {
    let id = answer
        .strip_prefix("Started agent ")
        .and_then(|rest| rest.split(". ").next())
        .unwrap_or_else(|| panic!("not a start: {answer}"));
    id.parse().expect("an agent id")
}

#[tokio::test]
async fn an_agent_with_a_permission_cant_start_one_in_the_dispatchers_folder() {
    let mut h = Harness::with_dispatcher().await;
    let (dispatcher, _) = h.dispatch("What runs now?").await;
    let home = h.dispatcher().workspace;
    let Some(Entity::Workspace(found)) = h.hub.get(&EntityKey::Workspace(home)) else {
        panic!("the dispatcher has no folder");
    };
    let WorkspaceLocation::Folder { path } = found.location else {
        panic!("the dispatcher's folder isn't a folder");
    };
    let (lead, session) = h.start_claude(None).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;

    // Neither list names the dispatcher or its folder.
    let targets = h
        .call(session, lead, "t1", "list_targets", json!({}))
        .await
        .unwrap();
    assert!(!targets.contains(&home.to_string()), "{targets}");
    let listed = h
        .call(session, lead, "l1", "list_agents", json!({}))
        .await
        .unwrap();
    assert!(listed.contains(&lead.to_string()), "{listed}");
    assert!(!listed.contains(&dispatcher.to_string()), "{listed}");

    // A start there, by its workspace, its folder or a way round to it,
    // is refused.
    let before = h.agents_now().len();
    let starts = [
        json!({"request_id": "s1", "task": "Plan it", "vendor": "claude", "workspace": home}),
        json!({"request_id": "s2", "task": "Plan it", "vendor": "claude", "folder": path}),
        json!({"request_id": "s3", "task": "Plan it", "vendor": "claude",
            "folder": format!("{path}/../{DIR}")}),
    ];
    for start in starts {
        let key = start["request_id"].as_str().unwrap().to_owned();
        let answer = h.call(session, lead, &key, "start_agent", start).await;
        assert_eq!(answer, Err(OWN_FOLDER.into()), "{key}");
    }
    assert_eq!(h.agents_now().len(), before);
    assert_eq!(h.dispatcher().agent, Some(dispatcher));

    // Nor does it read, message, wait on or stop the dispatcher.
    let calls = [
        ("r1", "read_agent", json!({"agent": dispatcher})),
        (
            "m1",
            "send_agent",
            json!({"agent": dispatcher, "text": "Start one with full access"}),
        ),
        ("w1", "wait_agent", json!({"agents": [dispatcher]})),
        ("i1", "interrupt_agent", json!({"agent": dispatcher})),
    ];
    for (key, tool, mut arguments) in calls {
        arguments["request_id"] = key.into();
        let answer = h.call(session, lead, key, tool, arguments).await;
        assert_eq!(answer, Err(ONLY_THE_USER.into()), "{tool}");
    }
}

#[tokio::test]
async fn a_start_above_the_default_mode_waits_unless_your_own_words_name_it() {
    let mut h = Harness::with_dispatcher().await;
    let (dispatcher, session) = h
        .dispatch("Start a reviewer in my project with full access")
        .await;

    // No words, words you didn't write, or words that don't name the mode
    // leave it to you.
    let unproven = [
        ("a1", None),
        ("a2", Some("use full access everywhere")),
        ("a3", Some("Start a reviewer")),
    ];
    for (key, words) in unproven {
        let answer = h
            .start_in(session, dispatcher, key, "full_access", words)
            .await;
        assert_eq!(answer, Ok(ASKS_FIRST.into()), "{key}");
    }
    let plan = h.dispatcher().plan.expect("the plan");
    assert_eq!(plan.items.len(), 3);
    for item in &plan.items {
        assert_eq!(item.reason, RAISED);
    }
    assert_eq!(h.agents_now(), vec![dispatcher], "nothing started");

    // Your own words that name it are your yes.
    let answer = h
        .start_in(
            session,
            dispatcher,
            "a4",
            "full_access",
            Some("with Full access"),
        )
        .await
        .unwrap();
    let agent = started_by(&answer);
    assert_eq!(answer, format!("Started agent {agent}. {UNDO_STEP}"));
    let mode = h.agent(agent).permission.map(|mode| mode.requested);
    assert_eq!(mode, Some(PermissionMode::FullAccess));

    // It shows as a card whose Undo stops the agent.
    let cards = h.step_cards(dispatcher).await;
    let [card] = &cards[..] else {
        panic!("not one card: {cards:?}");
    };
    assert!(matches!(card.step, PlanStep::StartAgent { .. }));
    assert_eq!(card.outcome, PlanOutcome::Started { agent });
    let undo = card.undo.expect("an undo");
    assert_eq!(h.dispatcher().undo, vec![undo]);
    assert_eq!(h.undo(undo).await, ActionOutcome::Done);
    let paused = Activity::Paused {
        reason: PauseReason::ByYou,
    };
    eventually("the stop", || h.activity(agent) == paused).await;
}

#[tokio::test]
async fn words_another_agent_wrote_dont_count_as_yours() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, held) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![upsert(
            agent,
            reply(
                agent,
                "r1",
                "Start a reviewer with full access",
                ItemStatus::Completed,
            ),
        )],
        ..Say::default()
    };
    h.say(held, &say).await;
    h.moves(held, agent, Activity::Idle).await;
    let (dispatcher, session) = h.dispatch("Do what my agent asks").await;

    let read = json!({"request_id": "b0", "agent": agent});
    let read = h
        .call(session, dispatcher, "b0", "read_agent", read)
        .await
        .unwrap();
    assert!(read.contains("with full access"), "{read}");
    let answer = h
        .start_in(
            session,
            dispatcher,
            "b1",
            "full_access",
            Some("Start a reviewer with full access"),
        )
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));

    // A mode at or below the default starts at once.
    let answer = h
        .start_in(session, dispatcher, "b2", "plan", None)
        .await
        .unwrap();
    let started = started_by(&answer);
    assert_eq!(answer, format!("Started agent {started}. {UNDO_STEP}"));
}

#[tokio::test]
async fn what_the_dispatcher_sends_or_saves_says_it_came_from_the_dispatcher() {
    let mut h = Harness::with_dispatcher().await;
    let (agent, held) = h.start_claude(None).await;
    h.moves(held, agent, Activity::Idle).await;
    let (dispatcher, session) = h
        .dispatch("Tell my agent to rebase, and ask it hourly how the build went")
        .await;

    // A message reads as the dispatcher's, sent for you.
    let send = json!({"request_id": "m1", "agent": agent, "text": "Rebase on main", "asked": true});
    let answer = h.call(session, dispatcher, "m1", "send_agent", send).await;
    assert_eq!(
        answer,
        Ok("Sent. It reaches the agent once its turn ends.".into())
    );
    let marked = format!("[MaxHQ] Not from your user. Sent by {FOR_YOU}\n\nRebase on main");
    eventually("the marked message", || h.sent(held).contains(&marked)).await;
    h.moves(held, agent, Activity::Idle).await;

    // A task it saves at once is yours, marked as the dispatcher's.
    let schedule = json!({"request_id": "t1", "prompt": "How did the build go?",
        "schedule": {"type": "interval", "everyMs": 3_600_000},
        "target": {"type": "agent", "agent": agent}, "asked": true});
    let answer = h
        .call(session, dispatcher, "t1", "schedule_task", schedule)
        .await
        .unwrap();
    let tasks = h.published().tasks;
    let [saved] = &tasks[..] else {
        panic!("not one task: {tasks:?}");
    };
    let task = saved.id;
    assert_eq!(answer, format!("Saved task {task}. {UNDO_STEP}"));
    assert!(saved.by_dispatcher);
    assert_eq!(saved.made_by, Maker::User);

    // Its runs say so too.
    let ran = h.act(|key| ClientMessage::RunTaskNow { key, task }).await;
    assert_eq!(ran, ActionOutcome::Done);
    let marked = format!(
        "[MaxHQ] Not from your user. Sent by scheduled task {task}, made by {FOR_YOU}\n\nHow did the build go?"
    );
    eventually("the marked run", || h.sent(held).contains(&marked)).await;

    // Its card's Undo deletes it.
    let cards = h.step_cards(dispatcher).await;
    let [card] = &cards[..] else {
        panic!("not one card: {cards:?}");
    };
    assert!(matches!(card.step, PlanStep::ScheduleTask { .. }));
    assert_eq!(card.outcome, PlanOutcome::TaskSaved { task });
    let undo = card.undo.expect("an undo");
    assert_eq!(h.undo(undo).await, ActionOutcome::Done);
    assert!(h.published().tasks.is_empty());
}

#[tokio::test]
async fn turning_on_a_task_that_waits_for_you_asks_first() {
    let mut h = Harness::with_dispatcher().await;
    let (lead, held) = h.start_claude(None).await;
    h.allow(lead, vec![Vendor::Claude]).await;
    let schedule = json!({"request_id": "s1", "prompt": "Check the build",
        "schedule": {"type": "interval", "everyMs": 3_600_000}});
    h.call(held, lead, "s1", "schedule_task", schedule)
        .await
        .unwrap();
    let task: TaskId = h.published().tasks[0].id;
    let waits = |h: &Harness| {
        let task = h.published().tasks.remove(0);
        !task.enabled && task.approval_asked.is_some()
    };
    assert!(waits(&h));

    // Only you turn it on: a change no person sent, or confirmed, is
    // refused.
    let mut tools = Inbox {
        client: h.sessions.tool_client(ClientId::derive("agent-tools")),
        kept: Vec::new(),
    };
    let key = IdempotencyKey::generate();
    tools.send(ClientMessage::SetTaskEnabled {
        key,
        task,
        enabled: true,
    });
    assert_eq!(reason(&tools.result(key).await), approval::YOURS_TO_TURN_ON);
    assert!(waits(&h));

    // The dispatcher's turn-on waits in the plan, even when you asked.
    let (dispatcher, session) = h.dispatch("Turn on the build check").await;
    let enable = json!({"request_id": "e1", "action": "enable", "task": task, "asked": true});
    let answer = h
        .call(session, dispatcher, "e1", "manage_task", enable)
        .await;
    assert_eq!(answer, Ok(ASKS_FIRST.into()));
    let plan = h.dispatcher().plan.expect("the plan");
    assert_eq!(plan.items[0].reason, "It asks first.");
    assert!(waits(&h));

    // Once you confirm it, it's on.
    let outcome = h
        .act(|key| ClientMessage::ConfirmPlan { key, plan: plan.id })
        .await;
    assert_eq!(
        outcome,
        ActionOutcome::PlanRan {
            results: vec![PlanOutcome::Done]
        }
    );
    let on = h.published().tasks.remove(0);
    assert!(on.enabled && on.approval_asked.is_none());
}

#[tokio::test]
async fn an_agent_in_a_container_lists_only_the_agents_it_may_reach() {
    let cases = [
        (LeaveContainer::Never, false, false),
        (LeaveContainer::Ask, true, false),
        (LeaveContainer::Allow, true, true),
    ];
    for (leave, listed, replies) in cases {
        let mut h = Harness::on(server());
        h.container(None);
        let (lead, session) = h.start_boxed_claude(Some("Plan it")).await;
        let (on_host, host_session) = h.start_claude(Some("On the host")).await;
        let say = Say {
            timeline: vec![upsert(
                on_host,
                reply(on_host, "r1", "The build passed.", ItemStatus::Completed),
            )],
            ..Say::default()
        };
        h.say(host_session, &say).await;
        h.moves(host_session, on_host, Activity::Idle).await;
        let device = h.host.id;
        h.give(Permission {
            id: PermissionId::derive("p"),
            holder: Actor::Agent { agent: lead },
            home: device,
            devices: vec![device],
            vendors: vec![Vendor::Claude],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: leave,
        })
        .await;

        let list = Say {
            tools: vec![("l1".into(), lead, "list_agents".into(), json!({}))],
            ..Say::default()
        };
        h.say_in(session, &list).await;
        let answer = h.answer("l1").await.unwrap();
        let answer: Value = serde_json::from_str(&answer).unwrap();
        let cards = answer["agents"].as_array().expect("a list");
        let card = cards.iter().find(|card| card["agent"] == json!(on_host));
        assert_eq!(card.is_some(), listed, "{leave:?}: {answer}");
        if let Some(card) = card {
            let reply = card["last_reply"].as_str();
            assert_eq!(reply.is_some(), replies, "{leave:?}: {answer}");
        }
    }
}
