// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Held agents' timelines read back after a restart while the manager
//! takes requests: what goes first, and what waits for a timeline.

use std::collections::HashSet;

use maxhq_protocol::model::DeviceSettingChange;

use super::*;

/// Starts a Claude agent with a one-item history, doing `activity` and
/// last active `minutes` after the test started. Returns it, its holder
/// and its session.
async fn agent_with_history(
    h: &mut Harness,
    minutes: u64,
    activity: Activity,
) -> (AgentId, SessionId, String) {
    let (agent, session) = h.start_claude(None).await;
    let (uuid, _) = *h.vendors.launches().last().expect("the agent launched");
    let uuid = uuid.to_string();
    h.vendors.set_history(
        &uuid,
        vec![reply(agent, "h1", "Before", ItemStatus::Completed)],
    );
    let say = Say {
        agents: vec![Agent {
            activity,
            last_activity_at: Timestamp(h.epoch.0 + minutes * 60_000),
            ..h.agent(agent)
        }],
        ..Say::default()
    };
    h.say(session, &say).await;
    (agent, session, uuid)
}

#[tokio::test]
async fn the_manager_answers_while_a_timeline_is_still_read() {
    let mut h = Harness::new();
    let (agent, _, uuid) = agent_with_history(&mut h, 1, Activity::Idle).await;
    h.vendors.hold_reads(&uuid);
    h.restart().await;
    eventually("the history read", || h.vendors.waiting_reads() == 1).await;

    // The agent shows, and a request that doesn't need its timeline is
    // answered while the read waits.
    assert_eq!(h.agent(agent).id, agent);
    let outcome = h
        .act(|key| ClientMessage::UpdateDeviceSettings {
            key,
            device: device().id,
            changes: vec![DeviceSettingChange::SnoozeAtLimit { value: true }],
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    h.phone.send(ClientMessage::OpenTimeline {
        agent,
        since: None,
        limit: 50,
        lead: None,
    });
    h.phone
        .quiet(
            "the timeline",
            |message| matches!(message, ServerMessage::Timeline { agent: got, .. } if *got == agent),
        )
        .await;

    h.vendors.release_reads(&uuid);
    let (_, items, _) = reset(h.phone.timeline(agent).await);
    assert_eq!(ids(&items), ["h1"]);
}

#[tokio::test]
async fn an_agent_with_work_waiting_is_read_before_idle_ones() {
    let mut h = Harness::new();
    // The busy agent is the one least recently active, so only its work
    // puts it first.
    let (_, _, busy) = agent_with_history(&mut h, 1, Activity::Busy).await;
    let mut idle = Vec::new();
    for minutes in 2..5 {
        idle.push(agent_with_history(&mut h, minutes, Activity::Idle).await.2);
    }
    for uuid in idle.iter().chain([&busy]) {
        h.vendors.hold_reads(uuid);
    }
    let from = h.vendors.reads().len();
    h.restart().await;
    eventually("two reads", || h.vendors.waiting_reads() == 2).await;

    // The two run together, so either may start first.
    let reads: HashSet<String> = h.vendors.reads()[from..].iter().cloned().collect();
    assert_eq!(reads, HashSet::from([busy.clone(), idle[2].clone()]));
    for uuid in idle.iter().chain([&busy]) {
        h.vendors.release_reads(uuid);
    }
}

#[tokio::test]
async fn an_agent_a_client_opens_is_read_next() {
    let mut h = Harness::new();
    let mut agents = Vec::new();
    for minutes in 1..5 {
        agents.push(agent_with_history(&mut h, minutes, Activity::Idle).await);
    }
    for (_, _, uuid) in &agents {
        h.vendors.hold_reads(uuid);
    }
    let from = h.vendors.reads().len();
    h.restart().await;
    eventually("two reads", || h.vendors.waiting_reads() == 2).await;

    // The oldest would be read last, but a client opened it.
    let (oldest, _, oldest_uuid) = agents[0].clone();
    h.phone.send(ClientMessage::OpenTimeline {
        agent: oldest,
        since: None,
        limit: 50,
        lead: None,
    });
    h.phone
        .quiet(
            "the timeline",
            |message| matches!(message, ServerMessage::Timeline { agent: got, .. } if *got == oldest),
        )
        .await;
    h.vendors.release_reads(&agents[3].2);
    eventually("the next read", || h.vendors.reads().len() == from + 3).await;
    assert_eq!(h.vendors.reads()[from + 2], oldest_uuid);

    h.vendors.release_reads(&oldest_uuid);
    let (_, items, _) = reset(h.phone.timeline(oldest).await);
    assert_eq!(ids(&items), ["h1"]);
    for (_, _, uuid) in &agents {
        h.vendors.release_reads(uuid);
    }
}

#[tokio::test]
async fn a_message_to_an_agent_still_read_waits_for_its_timeline() {
    let mut h = Harness::new();
    let (agent, session, uuid) = agent_with_history(&mut h, 1, Activity::Idle).await;
    h.vendors.hold_reads(&uuid);
    h.restart().await;
    eventually("the history read", || h.vendors.waiting_reads() == 1).await;

    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::SendInput {
        key,
        agent,
        input: UserInput {
            text: "next".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
        },
        mode: SendMode::Queue,
    });
    h.phone
        .quiet("the result", |message| {
            matches!(message, ServerMessage::ActionResult { key: got, .. } if *got == key)
        })
        .await;
    assert!(!h.sent(session).contains(&"next".to_owned()));

    // Its holder is reached once the timeline is in, and the message goes.
    h.vendors.release_reads(&uuid);
    assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    eventually("the message", || {
        h.sent(session).contains(&"next".to_owned())
    })
    .await;
}
