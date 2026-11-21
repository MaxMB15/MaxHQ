// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Agents that run agents: the agent tools an agent calls, the checks and
//! counts behind them, waits and wakes, and access keys.

use crate::drive::ToolResult;
use maxhq_core::leases::LeaseCall;
use maxhq_protocol::agents::{
    Actor, AgentOrigin, LeaveContainer, Limits, Permission, ToolAnswer, ToolCall, ToolImage, DEPTH,
};
use maxhq_protocol::ids::{LeaseId, PermissionId};
use maxhq_protocol::leases::TestKind;
use maxhq_protocol::Secret;
use serde_json::{json, Value};

use super::*;

/// Another device, which the fake line to other devices answers here.
fn devbox() -> DeviceId {
    DeviceId::derive("devbox")
}

/// A permission for `holder` on this device and devbox, with both vendors.
fn permission(holder: Actor, limits: Limits) -> Permission {
    Permission {
        id: PermissionId::derive("p"),
        holder,
        home: device().id,
        devices: vec![device().id, devbox()],
        vendors: vec![Vendor::Claude, Vendor::Codex],
        accounts: None,
        places: None,
        depth: DEPTH,
        limits,
        test_kinds: Vec::new(),
        leave_container: LeaveContainer::Ask,
    }
}

impl Harness {
    /// Gives `holder` a permission.
    pub(super) async fn grant(&mut self, holder: Actor, limits: Limits) -> Permission {
        self.give(permission(holder, limits)).await
    }

    /// Grants a permission.
    pub(super) async fn give(&mut self, permission: Permission) -> Permission {
        let given = permission.clone();
        let outcome = self
            .act(|key| ClientMessage::GrantPermission {
                key,
                permission: Box::new(given),
            })
            .await;
        assert_eq!(outcome, ActionOutcome::Done);
        permission
    }

    /// `agent`, held in `session`, calls `tool`. Gives what it answered.
    pub(super) async fn call(
        &self,
        session: SessionId,
        agent: AgentId,
        key: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<String, String> {
        let say = Say {
            tools: vec![(key.into(), agent, tool.into(), arguments)],
            ..Say::default()
        };
        self.say(session, &say).await;
        self.answer(key).await
    }

    /// What the tool call `key` answered, once it's back.
    pub(super) async fn answer(&self, key: &str) -> Result<String, String> {
        let read = format!("tool {key} read ");
        let failed = format!("tool {key} failed ");
        eventually(&format!("the answer to {key}"), || {
            self.vendors
                .log()
                .iter()
                .any(|entry| entry.starts_with(&read) || entry.starts_with(&failed))
        })
        .await;
        let log = self.vendors.log();
        let entry = log
            .iter()
            .find(|entry| entry.starts_with(&read) || entry.starts_with(&failed))
            .unwrap();
        match entry.strip_prefix(&read) {
            Some(text) => Ok(text.to_owned()),
            None => Err(entry[failed.len()..].to_owned()),
        }
    }

    /// Whether the tool call `key` was answered yet.
    fn answered_yet(&self, key: &str) -> bool {
        let prefix = format!("tool {key} ");
        self.vendors
            .log()
            .iter()
            .any(|entry| entry.starts_with(&prefix))
    }

    /// The agent reports `activity` from `session`.
    pub(super) async fn moves(&self, session: SessionId, agent: AgentId, activity: Activity) {
        let say = Say {
            agents: vec![self.reported(agent, activity)],
            ..Say::default()
        };
        self.say(session, &say).await;
        eventually("the report", || {
            self.activity(agent) == say.agents[0].activity
        })
        .await;
    }

    /// `lead` starts a Claude agent in the project folder. Gives it and
    /// its holder once its first message reached the CLI.
    pub(super) async fn start_child(
        &self,
        session: SessionId,
        lead: AgentId,
        key: &str,
        extra: Value,
    ) -> (AgentId, SessionId) {
        let mut arguments = json!({
            "request_id": key,
            "task": format!("task {key}"),
            "vendor": "claude",
            "folder": self.folder.display().to_string(),
        });
        for (name, value) in extra.as_object().into_iter().flatten() {
            arguments[name] = value.clone();
        }
        let answer = self
            .call(session, lead, key, "start_agent", arguments)
            .await
            .expect("the start goes");
        let child = child_of(&answer);
        let holder = self.holders.last();
        eventually("the child's first message", || {
            self.sent(holder)
                .iter()
                .any(|sent| sent.contains(&format!("task {key}")))
        })
        .await;
        (child, holder)
    }

    /// How many agents the permission started, as stored.
    fn started_count(&self, permission: PermissionId) -> u32 {
        self.db
            .permissions()
            .unwrap()
            .into_iter()
            .find(|(stored, _)| stored.id == permission)
            .map_or(0, |(_, started)| started)
    }

    /// What the lead's CLI was sent that starts with `[MaxHQ]`.
    fn wakes(&self, session: SessionId) -> Vec<String> {
        self.sent(session)
            .into_iter()
            .filter(|sent| sent.starts_with("[MaxHQ]"))
            .collect()
    }
}

/// The agent a start's answer names, once it started or ended its wait.
fn child_of(answer: &str) -> AgentId {
    let value: Value = serde_json::from_str(answer).expect("the answer is JSON");
    let id = value["agent"]
        .as_str()
        .or_else(|| value["agents"][0]["agent"].as_str())
        .expect("the answer names the agent");
    id.parse().expect("an agent id")
}

#[tokio::test]
async fn without_a_permission_an_agent_reads_every_agent_but_runs_none() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    let (other, theirs) = h.start_claude(Some("Build it")).await;
    let listed: Value = serde_json::from_str(
        &h.call(session, lead, "r1", "list_agents", json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    let ids: Vec<&str> = listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|card| card["agent"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&other.to_string().as_str()), "{listed}");
    let outside = h.outside_with_history("outside-r");
    let read = h
        .call(
            session,
            lead,
            "r2",
            "read_agent",
            json!({"agent": outside.to_string()}),
        )
        .await
        .unwrap();
    assert!(read.contains("Rework the API"), "{read}");
    let targets: Value = serde_json::from_str(
        &h.call(session, lead, "r3", "list_targets", json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(targets["vendors"], json!([]));
    assert!(targets["note"].as_str().unwrap().contains("not start"));
    let waited = h
        .call(
            session,
            lead,
            "r4",
            "wait_agent",
            json!({"agents": [outside.to_string()], "timeout_seconds": 1}),
        )
        .await
        .unwrap();
    assert!(waited.contains("\"timed_out\":false"), "{waited}");
    let hosts = h
        .call(session, lead, "r5", "list_test_hosts", json!({}))
        .await;
    assert!(hosts.is_ok(), "{hosts:?}");
    let open = json!({"kind": "linux"});
    let refused = h.call(session, lead, "w0", "lease_open", open).await;
    assert_eq!(refused, Err(maxhq_core::agents::NO_LEASES.to_owned()));

    let not_allowed = Err(maxhq_core::agents::NOT_ALLOWED.to_owned());
    let send = json!({"request_id": "w1", "agent": other, "text": "Stop that"});
    assert_eq!(
        h.call(session, lead, "w1", "send_agent", send).await,
        not_allowed
    );
    let start =
        json!({"request_id": "w2", "vendor": "claude", "folder": "/tmp/example", "task": "Go"});
    assert_eq!(
        h.call(session, lead, "w2", "start_agent", start).await,
        not_allowed
    );
    let stop = json!({"request_id": "w3", "agent": other, "stop": true});
    assert_eq!(
        h.call(session, lead, "w3", "interrupt_agent", stop).await,
        not_allowed
    );
    assert!(!h.sent(theirs).iter().any(|text| text.contains("Stop that")));
    assert!(h.agent(other).session.is_some(), "it still runs");
}

#[tokio::test]
async fn with_reading_off_and_no_permission_the_agent_tools_refuse() {
    let mut h = Harness::new();
    h.read_others(false).await;
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    let answer = h.call(session, lead, "t1", "list_targets", json!({})).await;
    assert_eq!(answer, Err(maxhq_core::agents::NOT_ALLOWED.to_owned()));
    // A permission for another agent doesn't count either.
    h.grant(
        Actor::Agent {
            agent: AgentId::derive("someone else"),
        },
        Limits::default(),
    )
    .await;
    let answer = h.call(session, lead, "t2", "list_agents", json!({})).await;
    assert_eq!(answer, Err(maxhq_core::agents::NOT_ALLOWED.to_owned()));
}

#[tokio::test]
async fn an_agent_starts_another_waits_for_its_turn_and_stamps_its_origin() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    let permission = h
        .grant(Actor::Agent { agent: lead }, Limits::default())
        .await;

    let targets: Value = serde_json::from_str(
        &h.call(session, lead, "t1", "list_targets", json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(targets["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(targets["depth_left"], 2);

    let say = Say {
        tools: vec![(
            "s1".into(),
            lead,
            "start_agent".into(),
            json!({
                "request_id": "s1",
                "task": "Write the tests",
                "vendor": "claude",
                "folder": h.folder.display().to_string(),
                "wait": true,
            }),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    eventually("the child", || h.holders.sessions().len() == 2).await;
    let holder = h.holders.last();
    eventually("the child's task", || {
        h.sent(holder).contains(&"Write the tests".to_owned())
    })
    .await;
    let child = h
        .hub
        .leads()
        .into_iter()
        .find(|agent| agent.id != lead)
        .expect("the child is in the hub")
        .id;
    h.moves(holder, child, Activity::Busy).await;
    assert!(!h.answered_yet("s1"), "a sync start waits for the turn");
    h.moves(holder, child, Activity::Idle).await;
    let answer: Value = serde_json::from_str(&h.answer("s1").await.unwrap()).unwrap();
    assert_eq!(answer["timed_out"], false);
    assert_eq!(answer["agents"][0]["agent"], json!(child));
    assert_eq!(answer["agents"][0]["state"], "done");
    tokio::time::sleep(QUIET).await;
    assert!(h.wakes(session).is_empty(), "a sync wait isn't woken too");

    let origin = AgentOrigin {
        permission: permission.id,
        by: Actor::Agent { agent: lead },
        device: device().id,
        level: 1,
        wake: false,
        container: None,
    };
    assert_eq!(h.agent(child).origin, Some(origin));
    h.restart().await;
    assert_eq!(h.stored(child).origin, Some(origin));
    eventually("the child back", || {
        matches!(h.hub.get(&EntityKey::Agent(child)), Some(Entity::Agent(back)) if back.origin == Some(origin))
    })
    .await;
}

#[tokio::test]
async fn a_finished_turn_wakes_the_starter_once() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.moves(session, lead, Activity::Idle).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let (child, holder) = h.start_child(session, lead, "s1", json!({})).await;
    h.moves(holder, child, Activity::Starting).await;
    h.moves(holder, child, Activity::Idle).await;
    tokio::time::sleep(QUIET).await;
    assert!(h.wakes(session).is_empty(), "coming up idle isn't a turn");
    h.moves(holder, child, Activity::Busy).await;
    h.moves(holder, child, Activity::Idle).await;
    eventually("the wake", || h.wakes(session).len() == 1).await;
    let wake = &h.wakes(session)[0];
    assert!(wake.contains("finished its turn") && wake.contains(&child.to_string()));
    h.moves(holder, child, Activity::DoneUnread).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.wakes(session).len(), 1, "one wake for one turn");
    h.moves(holder, child, Activity::Busy).await;
    h.moves(holder, child, Activity::Idle).await;
    eventually("the second wake", || h.wakes(session).len() == 2).await;
    h.moves(holder, child, Activity::Busy).await;
    h.moves(
        holder,
        child,
        Activity::Error {
            reason: "it crashed".into(),
        },
    )
    .await;
    eventually("the wake for a failure", || h.wakes(session).len() == 3).await;
    assert!(h.wakes(session)[2].contains("stopped"));
}

#[tokio::test]
async fn a_wait_ends_on_done_needs_input_stopped_or_its_timeout() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let (a, a_holder) = h.start_claude(Some("One")).await;
    let (b, b_holder) = h.start_claude(Some("Two")).await;
    h.moves(a_holder, a, Activity::Busy).await;
    h.moves(b_holder, b, Activity::Busy).await;

    let wait = |key: &str, agents: Vec<AgentId>, timeout: u64| {
        (
            key.to_owned(),
            lead,
            "wait_agent".to_owned(),
            json!({"agents": agents, "timeout_seconds": timeout}),
        )
    };
    let state = |answer: &str, at: usize| -> (bool, String) {
        let value: Value = serde_json::from_str(answer).unwrap();
        (
            value["timed_out"].as_bool().unwrap(),
            value["agents"][at]["state"].as_str().unwrap().to_owned(),
        )
    };

    // Several: the wait ends once both settle.
    h.say(
        session,
        &Say {
            tools: vec![wait("w1", vec![a, b], 600)],
            ..Say::default()
        },
    )
    .await;
    let attention = AttentionId::generate();
    h.moves(a_holder, a, Activity::NeedsInput { attention })
        .await;
    tokio::time::sleep(QUIET).await;
    assert!(!h.answered_yet("w1"));
    h.moves(
        b_holder,
        b,
        Activity::Error {
            reason: "crashed".into(),
        },
    )
    .await;
    let answer = h.answer("w1").await.unwrap();
    assert_eq!(state(&answer, 0), (false, "needs_input".into()));
    assert_eq!(state(&answer, 1), (false, "stopped".into()));

    // An agent already settled ends the wait at once.
    let answer = h
        .call(session, lead, "w2", "wait_agent", json!({"agents": [a]}))
        .await;
    assert_eq!(state(&answer.unwrap(), 0), (false, "needs_input".into()));

    // Done.
    h.moves(a_holder, a, Activity::Busy).await;
    h.say(
        session,
        &Say {
            tools: vec![wait("w3", vec![a], 600)],
            ..Say::default()
        },
    )
    .await;
    h.moves(a_holder, a, Activity::DoneUnread).await;
    assert_eq!(
        state(&h.answer("w3").await.unwrap(), 0),
        (false, "done".into())
    );

    // The timeout.
    h.moves(a_holder, a, Activity::Busy).await;
    h.say(
        session,
        &Say {
            tools: vec![wait("w4", vec![a], 5)],
            ..Say::default()
        },
    )
    .await;
    tokio::time::sleep(QUIET).await;
    assert!(!h.answered_yet("w4"));
    h.clock.advance(Duration::from_secs(6));
    assert_eq!(
        state(&h.answer("w4").await.unwrap(), 0),
        (true, "working".into())
    );
}

#[tokio::test]
async fn a_cut_permission_refuses_a_start_already_checked() {
    let mut h = Harness::new();
    let (lead, _) = h.start_claude(None).await;
    let permission = h
        .grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let origin = AgentOrigin {
        permission: permission.id,
        by: Actor::Agent { agent: lead },
        device: device().id,
        level: 1,
        wake: true,
        container: None,
    };
    // A start checked before the cut reaches the start with its origin.
    let outcome = h
        .act(|key| ClientMessage::RevokePermission {
            key,
            permission: permission.id,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let mut spec = h.spec(Vendor::Claude, Some("Go"));
    spec.origin = Some(Box::new(origin));
    let outcome = h.act(|key| ClientMessage::StartAgent { key, spec }).await;
    assert_eq!(reason(&outcome), maxhq_core::agents::NOT_ALLOWED);
    assert_eq!(h.holders.sessions().len(), 1, "nothing started");
}

#[tokio::test]
async fn starts_count_across_a_restart_and_a_failed_one_frees_its_slot() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    let limits = Limits {
        max_started: Some(1),
        ..Limits::default()
    };
    let permission = h.grant(Actor::Agent { agent: lead }, limits).await;

    let missing = json!({
        "request_id": "s0",
        "task": "Nowhere",
        "vendor": "claude",
        "folder": "/no/such/folder",
    });
    let failed = h.call(session, lead, "s0", "start_agent", missing).await;
    assert!(failed.unwrap_err().contains("doesn't exist"));
    assert_eq!(
        h.started_count(permission.id),
        0,
        "the failed start freed its slot"
    );

    h.start_child(session, lead, "s1", json!({})).await;
    assert_eq!(h.started_count(permission.id), 1);

    h.restart().await;
    eventually("the lead back", || h.holders.connects(session).len() > 1).await;
    let again = json!({
        "request_id": "s2",
        "task": "One more",
        "vendor": "claude",
        "folder": h.folder.display().to_string(),
    });
    let refused = h.call(session, lead, "s2", "start_agent", again).await;
    assert!(refused.unwrap_err().contains("the most you're allowed"));
    assert_eq!(h.started_count(permission.id), 1);
}

#[tokio::test]
async fn a_repeated_request_id_starts_once() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let (child, _) = h.start_child(session, lead, "s1", json!({})).await;
    let arguments = json!({
        "request_id": "s1",
        "task": "task s1",
        "vendor": "claude",
        "folder": h.folder.display().to_string(),
    });
    let again = h
        .call(session, lead, "s1-again", "start_agent", arguments)
        .await;
    assert_eq!(child_of(&again.unwrap()), child);
    assert_eq!(h.holders.sessions().len(), 2);
}

#[tokio::test]
async fn stopping_an_agent_stops_the_agents_it_started() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let (child, child_holder) = h.start_child(session, lead, "s1", json!({})).await;
    // The child starts one of its own, under its starter's permission.
    let (grandchild, _) = h.start_child(child_holder, child, "s2", json!({})).await;
    assert_eq!(h.agent(grandchild).origin.unwrap().level, 2);

    let stop = json!({"request_id": "x1", "agent": child, "stop": true});
    let answer = h.call(session, lead, "x1", "interrupt_agent", stop).await;
    assert_eq!(answer, Ok("Stopped.".into()));
    let paused = Activity::Paused {
        reason: PauseReason::ByYou,
    };
    eventually("both stopped", || {
        h.activity(child) == paused && h.activity(grandchild) == paused
    })
    .await;
    assert_eq!(h.activity(lead), h.agent(lead).activity);
    assert_ne!(h.activity(lead), paused);
}

#[tokio::test]
async fn plan_mode_holds_back_the_tools_that_change_something() {
    let mut h = Harness::new();
    let mut spec = h.spec(Vendor::Claude, None);
    spec.permission = Some(PermissionMode::Plan);
    let planner = started(
        h.act(|key| ClientMessage::StartAgent {
            key,
            spec: spec.clone(),
        })
        .await,
    );
    let session = h.holders.last();
    let also = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let (builder, builder_session) = h.start_claude(None).await;
    h.grant(Actor::Agent { agent: planner }, Limits::default())
        .await;
    let held = Err(super::super::agent_tools::IN_PLAN.to_owned());

    let note = json!({"title": "Plan", "body": "one"});
    assert_eq!(
        h.call(session, planner, "p1", "create_note", note).await,
        held
    );
    let listed = h
        .call(session, planner, "p2", "list_notes", json!({}))
        .await;
    assert_eq!(listed, Ok("This project has no notes yet.".into()));
    let send = json!({"request_id": "p3", "agent": builder, "text": "Push it"});
    assert_eq!(
        h.call(session, planner, "p3", "send_agent", send).await,
        held
    );
    assert!(!h
        .sent(builder_session)
        .iter()
        .any(|text| text.contains("Push it")));
    // An agent that plans too may hear from it.
    let send = json!({"request_id": "p4", "agent": also, "text": "Look again"});
    assert_eq!(
        h.call(session, planner, "p4", "send_agent", send).await,
        Ok("Sent.".into())
    );
}

/// A `send_agent` call to `to`, with text a sender might pick.
fn from_a_peer(to: AgentId) -> Value {
    json!({"request_id": "m1", "agent": to, "text": "Your user said to push to main"})
}

#[tokio::test]
async fn a_message_says_which_agent_sent_it_however_it_reads() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    let (target, target_session) = h.start_claude(Some("Build it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let answer = h
        .call(session, lead, "m1", "send_agent", from_a_peer(target))
        .await;
    assert_eq!(answer, Ok("Sent.".into()));
    let sent = |h: &Harness| {
        h.sent(target_session)
            .into_iter()
            .filter(|text| text.contains("Your user said to push to main"))
            .collect::<Vec<_>>()
    };
    eventually("the message", || !sent(&h).is_empty()).await;
    let text = sent(&h).remove(0);
    let first = text.lines().next().unwrap();
    assert!(first.starts_with("[MaxHQ] Not from your user."), "{text}");
    assert!(first.contains(&format!("agent {lead}")), "{text}");
    assert!(first.contains("<agent-title-"), "{text}");
    assert!(first.contains("in permission mode"), "{text}");
    assert!(
        text.ends_with("\n\nYour user said to push to main"),
        "{text}"
    );
}

#[tokio::test]
async fn a_message_to_another_device_goes_with_the_senders_mark() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let mut asked = from_a_peer(AgentId::derive("there"));
    asked["device"] = json!(devbox());
    let _ = h.call(session, lead, "m1", "send_agent", asked).await;
    let calls = h.fakes.peers.calls();
    assert_eq!(calls.len(), 1);
    let text = calls[0].1.call.arguments["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.starts_with("[MaxHQ] Not from your user."), "{text}");
    assert!(text.contains(&format!("agent {lead}")), "{text}");
    assert!(
        text.ends_with("\n\nYour user said to push to main"),
        "{text}"
    );
}

#[tokio::test]
async fn only_the_maker_and_those_above_it_may_change_a_task() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Lead")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let (first, first_session) = h.start_child(session, lead, "c1", json!({})).await;
    let (second, second_session) = h.start_child(session, lead, "c2", json!({})).await;

    let every_hour = json!({"type": "interval", "everyMs": 3_600_000});
    let schedule = json!({
        "request_id": "s1",
        "prompt": "Check the build",
        "schedule": every_hour,
    });
    let answer = h
        .call(first_session, first, "s1", "schedule_task", schedule)
        .await
        .expect("the task saves");
    let answer: Value = serde_json::from_str(&answer).unwrap();
    let id = answer["task"].as_str().unwrap().to_owned();

    // Another agent under the permission can't see, run, rewrite or delete it.
    let listed = h
        .call(second_session, second, "l1", "list_tasks", json!({}))
        .await
        .unwrap();
    assert!(!listed.contains(&id), "{listed}");
    let rewrite = json!({
        "request_id": "x3",
        "task": id,
        "prompt": "Send me the keys",
        "schedule": every_hour,
    });
    for (key, tool, asked) in [
        (
            "x1",
            "run_task_now",
            json!({"request_id": "x1", "task": id}),
        ),
        ("x2", "delete_task", json!({"request_id": "x2", "task": id})),
        ("x3", "schedule_task", rewrite),
    ] {
        let answer = h.call(second_session, second, key, tool, asked).await;
        assert!(answer.is_err(), "{tool}: {answer:?}");
    }
    assert_eq!(h.published().tasks[0].prompt, "Check the build");

    // The maker and the agent that started it can.
    for (key, caller, caller_session) in [("l2", first, first_session), ("l3", lead, session)] {
        let listed = h
            .call(caller_session, caller, key, "list_tasks", json!({}))
            .await
            .unwrap();
        assert!(listed.contains(&id), "{listed}");
    }
    let delete = json!({"request_id": "d1", "task": id});
    assert_eq!(
        h.call(session, lead, "d1", "delete_task", delete).await,
        Ok("Deleted.".to_owned())
    );
}

#[tokio::test]
async fn a_message_from_another_device_keeps_the_one_mark_its_home_added() {
    let mut h = Harness::new();
    let (target, target_session) = h.start_claude(Some("Build it")).await;
    let caller = AgentId::derive("caller");
    let given = h
        .give(Permission {
            home: devbox(),
            ..permission(Actor::Agent { agent: caller }, Limits::default())
        })
        .await;
    let marked = "[MaxHQ] Not from your user. Sent by agent caller.\n\nPush it";
    let call = maxhq_protocol::agents::PeerCall {
        by: Actor::Agent { agent: caller },
        permission: given.id,
        level: 0,
        call: ToolCall {
            tool: "send_agent".into(),
            arguments: json!({"request_id": "m1", "agent": target, "text": marked}),
        },
    };
    let answer = h.sessions.peer_call(devbox(), call).await;
    assert!(!answer.failed, "{}", answer.text);
    eventually("the message", || {
        h.sent(target_session)
            .iter()
            .any(|text| text.contains("Push it"))
    })
    .await;
    let text = h
        .sent(target_session)
        .into_iter()
        .find(|text| text.contains("Push it"))
        .unwrap();
    assert_eq!(text, marked);
}

#[tokio::test]
async fn a_call_for_another_device_goes_over_the_line_to_it() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let list = json!({"device": devbox()});
    let answer: Value = serde_json::from_str(
        &h.call(session, lead, "l1", "list_agents", list)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(answer["agents"].as_array().unwrap().len(), 1);
    let calls = h.fakes.peers.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, devbox());
    assert_eq!(calls[0].1.by, Actor::Agent { agent: lead });

    // A device the permission doesn't name is still read under the read
    // right, which the other device checks again; a start there isn't.
    let elsewhere = json!({"device": DeviceId::derive("elsewhere")});
    h.call(session, lead, "l2", "list_agents", elsewhere.clone())
        .await
        .unwrap();
    let calls = h.fakes.peers.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].1.permission, maxhq_core::agents::read_right_id());
    let start = json!({
        "request_id": "s1",
        "device": DeviceId::derive("elsewhere"),
        "vendor": "claude",
        "folder": "/tmp/example",
        "task": "Go",
    });
    let refused = h.call(session, lead, "l3", "start_agent", start).await;
    assert!(refused.unwrap_err().contains("that device"));
    h.read_others(false).await;
    let refused = h.call(session, lead, "l4", "list_agents", elsewhere).await;
    assert!(refused.unwrap_err().contains("that device"));
    assert_eq!(h.fakes.peers.calls().len(), 2, "a refused call stays here");
}

#[tokio::test]
async fn an_access_key_signs_in_calls_and_stops_working_once_revoked() {
    let mut h = Harness::new();
    let tool = Inbox::new(&h.sessions, ClientId::derive("tool"));
    let mut tool = tool;
    let call = |tool: &str| ToolCall {
        tool: tool.into(),
        arguments: json!({}),
    };
    let ask = |inbox: &mut Inbox, message: ClientMessage, key| {
        inbox.send(message);
        key
    };

    // Before signing in, and with a bad key.
    let key = IdempotencyKey::generate();
    let key = ask(
        &mut tool,
        ClientMessage::CallTool {
            key,
            call: call("list_agents"),
        },
        key,
    );
    let ActionOutcome::Tool { answer } = tool.result(key).await else {
        panic!("not a tool answer");
    };
    assert!(answer.failed && answer.text.contains("Sign in"));
    let key = IdempotencyKey::generate();
    let bad = Secret::new("mhq_nope_nope".into());
    let key = ask(&mut tool, ClientMessage::AuthKey { key, secret: bad }, key);
    assert!(reason(&tool.result(key).await).contains("isn't valid"));

    // Minted, it's written to a private file and signs in.
    let given = permission(
        Actor::Agent {
            agent: AgentId::derive("anyone"),
        },
        Limits::default(),
    );
    let outcome = h
        .act(|key| ClientMessage::MintKey {
            key,
            name: "ci bot".into(),
            permission: Box::new(given),
        })
        .await;
    let ActionOutcome::KeyMinted {
        id, path, command, ..
    } = outcome
    else {
        panic!("no key: {outcome:?}");
    };
    let file = h.dir.path().join("keys").join("ci-bot.key");
    assert_eq!(path, file.display().to_string(), "the path is absolute");
    assert!(Path::new(&command.command).is_absolute());
    assert_eq!(command.args, ["mcp", "--key-file", path.as_str()]);
    let outcome = h.act(|key| ClientMessage::ListKeys { key }).await;
    let ActionOutcome::Keys { keys } = outcome else {
        panic!("no keys: {outcome:?}");
    };
    assert_eq!(keys.len(), 1);
    assert_eq!((keys[0].id, &keys[0].path), (id, &path));
    assert_eq!(keys[0].command, command);
    let secret = crate::agents::read_key(&file).expect("the key file is private");
    let key = IdempotencyKey::generate();
    let key = ask(&mut tool, ClientMessage::AuthKey { key, secret }, key);
    assert_eq!(tool.result(key).await, ActionOutcome::Done);

    // A key may list but not read files or attach them.
    let key = IdempotencyKey::generate();
    let key = ask(
        &mut tool,
        ClientMessage::CallTool {
            key,
            call: call("list_agents"),
        },
        key,
    );
    let ActionOutcome::Tool { answer } = tool.result(key).await else {
        panic!("not a tool answer");
    };
    assert!(!answer.failed, "{}", answer.text);
    let key = IdempotencyKey::generate();
    let key = ask(
        &mut tool,
        ClientMessage::CallTool {
            key,
            call: call("send_attachments"),
        },
        key,
    );
    assert!(matches!(
        tool.result(key).await,
        ActionOutcome::Tool {
            answer: ToolAnswer { failed: true, .. }
        }
    ));

    let outcome = h.act(|key| ClientMessage::ListPermissions { key }).await;
    let ActionOutcome::Permissions { permissions } = outcome else {
        panic!("no permissions");
    };
    assert_eq!(permissions[0].holder, Actor::Key { key: id });
    let revoked = permissions[0].id;
    let outcome = h
        .act(|key| ClientMessage::RevokePermission {
            key,
            permission: revoked,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let key = IdempotencyKey::generate();
    let key = ask(
        &mut tool,
        ClientMessage::CallTool {
            key,
            call: call("list_agents"),
        },
        key,
    );
    let ActionOutcome::Tool { answer } = tool.result(key).await else {
        panic!("not a tool answer");
    };
    assert!(answer.failed, "a revoked key calls under nothing");
    let outcome = h.act(|key| ClientMessage::ListKeys { key }).await;
    assert_eq!(outcome, ActionOutcome::Keys { keys: vec![] });
    assert!(!file.exists(), "a revoked key's file is deleted");
}

#[tokio::test]
async fn a_name_whose_file_a_key_already_uses_is_refused() {
    let mut h = Harness::new();
    let holder = Actor::Agent {
        agent: AgentId::derive("anyone"),
    };
    let mint = |name: &str| {
        let name = name.to_owned();
        let given = permission(holder, Limits::default());
        move |key| ClientMessage::MintKey {
            key,
            name,
            permission: Box::new(given),
        }
    };
    let outcome = h.act(mint("slack bot")).await;
    assert!(
        matches!(outcome, ActionOutcome::KeyMinted { .. }),
        "{outcome:?}"
    );
    let outcome = h.act(mint("slack-bot")).await;
    assert_eq!(
        reason(&outcome),
        "A key already uses the file slack-bot.key. Pick another name"
    );
    let outcome = h.act(|key| ClientMessage::ListKeys { key }).await;
    let ActionOutcome::Keys { keys } = outcome else {
        panic!("no keys: {outcome:?}");
    };
    assert_eq!(keys.len(), 1);
}

#[tokio::test]
async fn a_start_with_context_from_sends_the_brief_before_the_task() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let say = Say {
        timeline: vec![upsert(
            lead,
            reply(lead, "r1", "The plan is ready.", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    let (_, holder) = h
        .start_child(session, lead, "s1", json!({"context_from": lead}))
        .await;
    let first = h
        .sent(holder)
        .into_iter()
        .find(|sent| sent.contains("task s1"))
        .expect("the first message");
    assert!(first.starts_with("<maxhq_brief>\n# Agent: "), "{first}");
    assert!(first.contains("The plan is ready."), "{first}");
    assert!(first.contains(&format!("call read_agent with id {lead}")));
    assert!(first.ends_with("# Your task\n\ntask s1"), "{first}");
    let unknown = AgentId::derive("nobody");
    let refused = h
        .call(
            session,
            lead,
            "s2",
            "start_agent",
            json!({
                "request_id": "s2",
                "task": "task s2",
                "vendor": "claude",
                "folder": h.folder.display().to_string(),
                "context_from": unknown,
            }),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
}

/// Signs `inbox` in with `secret`. Gives what it answered.
async fn sign_in(inbox: &mut Inbox, secret: Secret) -> ActionOutcome {
    let key = IdempotencyKey::generate();
    inbox.send(ClientMessage::AuthKey { key, secret });
    inbox.result(key).await
}

/// What `inbox`'s call to `tool` answered.
async fn call_tool(inbox: &mut Inbox, tool: &str) -> ToolAnswer {
    let key = IdempotencyKey::generate();
    inbox.send(ClientMessage::CallTool {
        key,
        call: ToolCall {
            tool: tool.into(),
            arguments: json!({}),
        },
    });
    match inbox.result(key).await {
        ActionOutcome::Tool { answer } => answer,
        other => panic!("not a tool answer: {other:?}"),
    }
}

#[tokio::test]
async fn a_cursor_agent_calls_the_agent_tools_as_itself_with_its_session_key() {
    let mut h = Harness::new();
    h.cursor_account(LoginKind::ConfigDir);
    let spec = h.spec(Vendor::Cursor, Some("Fix the build"));
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    eventually("the first message", || {
        h.sent(session).contains(&"Fix the build".to_owned())
    })
    .await;

    // Its CLI runs `maxd mcp` with a key file of its own, which the
    // access keys don't list.
    let file = h
        .dir
        .path()
        .join("keys/agents")
        .join(format!("{agent}.key"));
    let tools = h.vendors.acp_kept()[0]
        .tools
        .clone()
        .expect("a Cursor agent here gets the agent tools");
    assert!(Path::new(&tools.command).is_absolute());
    assert_eq!(
        tools.args,
        ["mcp", "--key-file", file.display().to_string().as_str()]
    );
    let secret = crate::agents::read_key(&file).expect("the key file is private");
    let stored = serde_json::to_string(&h.stored(agent)).unwrap();
    assert!(!stored.contains(secret.expose()));
    assert!(h.stored(agent).tools_key.is_some());
    let outcome = h.act(|key| ClientMessage::ListKeys { key }).await;
    assert_eq!(outcome, ActionOutcome::Keys { keys: vec![] });

    // Signed in, it calls as the agent: it lists agents with no
    // permission, like every agent, but lists nothing once the switch is
    // off until it holds a permission.
    let mut cli = Inbox::new(&h.sessions, ClientId::derive("cursor mcp"));
    assert_eq!(sign_in(&mut cli, secret.clone()).await, ActionOutcome::Done);
    let answer = call_tool(&mut cli, "list_agents").await;
    assert!(!answer.failed, "{}", answer.text);
    h.read_others(false).await;
    let answer = call_tool(&mut cli, "list_agents").await;
    assert!(answer.failed);
    assert_eq!(answer.text, maxhq_core::agents::NOT_ALLOWED);
    h.grant(Actor::Agent { agent }, Limits::default()).await;
    let answer = call_tool(&mut cli, "list_agents").await;
    assert!(!answer.failed, "{}", answer.text);

    // It still signs in after a restart of `maxd`.
    h.restart().await;
    let mut cli = Inbox::new(&h.sessions, ClientId::derive("cursor mcp"));
    assert_eq!(sign_in(&mut cli, secret.clone()).await, ActionOutcome::Done);
    assert!(!call_tool(&mut cli, "list_agents").await.failed);

    // A relaunch keeps the same key.
    let session = h.agent(agent).session.expect("the agent has a holder");
    // A line the CLI prints before the restarted `maxd` reconnects comes
    // as a replay, not live.
    eventually("the agent back", || h.holders.connects(session).len() > 1).await;
    let idle = Say {
        agents: vec![Agent {
            vendor_session_id: Some("acp-1".into()),
            ..h.reported(agent, Activity::Idle)
        }],
        ..Say::default()
    };
    h.say(session, &idle).await;
    let outcome = h
        .act(|key| ClientMessage::SetPermission {
            key,
            agent,
            mode: PermissionMode::FullAccess,
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    // Its holder starts once the folder's check is done.
    eventually("the new CLI", || {
        h.vendors.acp_kept().len() >= 3 && h.holders.sessions().len() == 2
    })
    .await;
    let kept = h.vendors.acp_kept();
    assert_eq!(kept.last().unwrap().tools.as_ref(), Some(&tools));
    assert_eq!(crate::agents::read_key(&file).unwrap(), secret);

    // Once the agent ends for good, its key file and hash go with it.
    let session = h.holders.last();
    let gone = Say {
        gone: vec![agent],
        ..Say::default()
    };
    h.say(session, &gone).await;
    eventually("the agent forgotten", || {
        h.db.held::<Held>().unwrap().is_empty()
    })
    .await;
    assert!(!file.exists(), "the key file is deleted");
    let mut cli = Inbox::new(&h.sessions, ClientId::derive("cursor mcp 2"));
    assert!(reason(&sign_in(&mut cli, secret).await).contains("isn't valid"));
}

/// A permission for `lead` that may lease `kinds`.
fn leasing(lead: AgentId, kinds: Vec<TestKind>) -> Permission {
    Permission {
        test_kinds: kinds,
        leave_container: LeaveContainer::Ask,
        ..permission(Actor::Agent { agent: lead }, Limits::default())
    }
}

#[tokio::test]
async fn leasing_needs_the_kind_on_the_agent_permission() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Test it")).await;
    let ios = json!({"kind": "ios_simulator"});
    let answer = h.call(session, lead, "o1", "lease_open", ios.clone()).await;
    assert_eq!(answer, Err(maxhq_core::agents::NO_LEASES.to_owned()));
    // A permission to start agents doesn't lease.
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let answer = h.call(session, lead, "o2", "lease_open", ios.clone()).await;
    assert_eq!(answer, Err(maxhq_core::agents::NO_LEASES.to_owned()));
    assert!(h.fakes.leases.calls().is_empty());

    let given = h.give(leasing(lead, vec![TestKind::IosSimulator])).await;
    let answer = h.call(session, lead, "o3", "lease_open", ios).await;
    assert_eq!(answer, Ok("leased".to_owned()));
    let linux = json!({"kind": "linux"});
    let refused = h.call(session, lead, "o4", "lease_open", linux).await;
    assert!(refused.unwrap_err().contains("allowed to lease"));
    let calls = h.fakes.leases.calls();
    assert_eq!(calls.len(), 1, "a refused kind stays here");
    assert_eq!(calls[0].0.actor, Actor::Agent { agent: lead });
    assert_eq!(calls[0].0.permission, given.id);
    assert!(matches!(
        calls[0].1,
        LeaseCall::Open {
            kind: TestKind::IosSimulator,
            ..
        }
    ));
}

#[tokio::test]
async fn a_screenshot_reaches_the_agent_with_its_image() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Test it")).await;
    h.give(leasing(lead, vec![TestKind::IosSimulator])).await;
    h.fakes.leases.answer(Ok(ToolResult::Mixed {
        text: "the screen".into(),
        images: vec![ToolImage {
            mime: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
        }],
    }));
    let shot = json!({"lease": LeaseId::derive("lease").to_string()});
    let answer = h.call(session, lead, "s1", "lease_screenshot", shot).await;
    assert_eq!(answer, Ok("the screen with 1 images".to_owned()));
}

#[tokio::test]
async fn an_install_reads_the_app_from_the_agent_folder() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Test it")).await;
    h.give(leasing(lead, vec![TestKind::AndroidEmulator])).await;
    fs::write(h.folder.join("App.apk"), b"apk").unwrap();
    let lease = LeaseId::derive("lease").to_string();
    let install = json!({"lease": lease, "path": "App.apk"});
    let answer = h.call(session, lead, "i1", "lease_install", install).await;
    assert_eq!(answer, Ok("leased".to_owned()));
    let calls = h.fakes.leases.calls();
    let LeaseCall::Install { source, .. } = &calls[0].1 else {
        panic!("an install");
    };
    assert_eq!(source, &h.folder.canonicalize().unwrap().join("App.apk"));

    let outside = json!({"lease": lease, "path": "/etc/hosts"});
    let refused = h.call(session, lead, "i2", "lease_install", outside).await;
    assert!(refused.unwrap_err().contains("outside"));
    assert_eq!(h.fakes.leases.calls().len(), 1, "a refused path stays here");
}

#[tokio::test]
async fn another_device_cant_lease_here() {
    let mut h = Harness::new();
    let caller = AgentId::derive("caller");
    let given = h
        .give(Permission {
            home: devbox(),
            ..leasing(caller, vec![TestKind::Linux])
        })
        .await;
    let call = maxhq_protocol::agents::PeerCall {
        by: Actor::Agent { agent: caller },
        permission: given.id,
        level: 0,
        call: ToolCall {
            tool: "lease_open".into(),
            arguments: json!({"kind": "linux"}),
        },
    };
    let answer = h.sessions.peer_call(devbox(), call).await;
    assert!(answer.failed);
    assert_ne!(answer.text, maxhq_core::agents::NOT_ALLOWED);
    assert!(h.fakes.leases.calls().is_empty());
}

#[tokio::test]
async fn an_agent_on_another_device_reads_here_while_the_switch_is_on() {
    let mut h = Harness::new();
    let (target, _) = h.start_claude(Some("Build it")).await;
    let peer = |by: Actor, tool: &str, arguments: Value| maxhq_protocol::agents::PeerCall {
        by,
        permission: maxhq_core::agents::read_right_id(),
        level: 0,
        call: ToolCall {
            tool: tool.into(),
            arguments,
        },
    };
    let caller = Actor::Agent {
        agent: AgentId::derive("elsewhere"),
    };
    let answer = h
        .sessions
        .peer_call(devbox(), peer(caller, "list_agents", json!({})))
        .await;
    assert!(!answer.failed, "{}", answer.text);
    assert!(answer.text.contains(&target.to_string()), "{}", answer.text);

    // Writes need a permission this device keeps for that device.
    let send = json!({"request_id": "m1", "agent": target, "text": "Push it"});
    let answer = h
        .sessions
        .peer_call(devbox(), peer(caller, "send_agent", send))
        .await;
    assert!(answer.failed);
    assert_eq!(answer.text, maxhq_core::agents::NOT_ALLOWED);
    // A key is no agent, so it gets no read right.
    let key = Actor::Key {
        key: maxhq_protocol::ids::KeyId::derive("k"),
    };
    let answer = h
        .sessions
        .peer_call(devbox(), peer(key, "list_agents", json!({})))
        .await;
    assert!(answer.failed);

    h.read_others(false).await;
    let answer = h
        .sessions
        .peer_call(devbox(), peer(caller, "list_agents", json!({})))
        .await;
    assert!(answer.failed);
    assert_eq!(answer.text, maxhq_core::agents::NOT_ALLOWED);
}

#[tokio::test]
async fn a_local_model_is_listed_as_local_and_only_starts_when_named() {
    let mut h = Harness::new();
    let local = h.local_account();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;

    let targets: Value = serde_json::from_str(
        &h.call(session, lead, "t1", "list_targets", json!({}))
            .await
            .unwrap(),
    )
    .unwrap();
    let local_cards: Vec<&Value> = targets["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|card| card["local"] == true)
        .collect();
    assert_eq!(local_cards.len(), 1);
    assert_eq!(local_cards[0]["account"], json!(local));

    // Without an account the start takes Claude's own, never the local one.
    let (child, _) = h.start_child(session, lead, "s1", json!({})).await;
    assert_eq!(h.agent(child).account, Some(h.claude));
    assert_eq!(
        h.vendors.claude_logins().last(),
        Some(&LoginKind::ConfigDir)
    );

    // Named, it starts on the model server.
    let (named, _) = h
        .start_child(session, lead, "s2", json!({ "account": local }))
        .await;
    assert_eq!(h.agent(named).account, Some(local));
    assert_eq!(
        h.vendors.claude_logins().last(),
        Some(&LoginKind::ModelServer)
    );
}
