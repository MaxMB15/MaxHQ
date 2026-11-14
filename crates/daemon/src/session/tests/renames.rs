// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Naming an agent, held or seen outside MaxHQ: the name stands over the
//! titles its CLI gives and through a restart, until a blank name or a
//! delete clears it.

use super::*;

async fn rename(h: &mut Harness, agent: AgentId, title: &str) -> ActionOutcome {
    let title = title.to_owned();
    h.act(|key| ClientMessage::RenameAgent { key, agent, title })
        .await
}

#[tokio::test]
async fn a_held_agents_name_stands_over_its_clis_title_through_a_restart() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(Some("Fix the flaky test")).await;
    assert_eq!(h.agent(agent).title, "Fix the flaky test");
    assert_eq!(
        rename(&mut h, agent, "  Flaky test  ").await,
        ActionOutcome::Done
    );
    assert_eq!(h.agent(agent).title, "Flaky test");

    let named = Agent {
        title: "Named by the CLI".into(),
        ..h.reported(agent, Activity::Idle)
    };
    let say = Say {
        agents: vec![named],
        ..Say::default()
    };
    h.say(session, &say).await;
    assert_eq!(h.agent(agent).title, "Flaky test");

    h.restart().await;
    eventually("the agent back", || {
        h.hub.get(&EntityKey::Agent(agent)).is_some()
    })
    .await;
    assert_eq!(h.agent(agent).title, "Flaky test");

    assert_eq!(rename(&mut h, agent, " \t ").await, ActionOutcome::Done);
    assert_eq!(h.agent(agent).title, "Named by the CLI");
    assert!(h.db.agent_names().unwrap().is_empty());
}

#[tokio::test]
async fn an_outside_agents_name_stands_over_its_observers_title_through_a_restart() {
    let mut h = Harness::new();
    let agent = h.outside("outside", Activity::Idle);
    let long = "x".repeat(100);
    assert_eq!(rename(&mut h, agent, &long).await, ActionOutcome::Done);
    assert_eq!(h.agent(agent).title, "x".repeat(80), "a long name is cut");

    // The observer reports it again.
    h.outside("outside", Activity::Busy);
    assert_eq!(h.agent(agent).title, "x".repeat(80));

    h.restart().await;
    h.outside("outside", Activity::Idle);
    assert_eq!(h.agent(agent).title, "x".repeat(80));

    assert_eq!(rename(&mut h, agent, "").await, ActionOutcome::Done);
    assert_eq!(h.agent(agent).title, "Outside");
}

#[tokio::test]
async fn deleting_an_agent_forgets_its_name() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    assert_eq!(rename(&mut h, agent, "Doomed").await, ActionOutcome::Done);
    assert_eq!(
        h.db.agent_names().unwrap(),
        std::collections::HashMap::from([(agent, "Doomed".to_owned())])
    );
    assert_eq!(
        h.act(|key| ClientMessage::DeleteAgent { key, agent }).await,
        ActionOutcome::Done
    );
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    assert!(h.db.agent_names().unwrap().is_empty());
}

#[tokio::test]
async fn an_unknown_agent_isnt_named() {
    let mut h = Harness::new();
    let nobody = AgentId::derive("nobody");
    assert_eq!(
        rename(&mut h, nobody, "Ghost").await,
        ActionOutcome::UnknownAgent
    );
    assert!(h.db.agent_names().unwrap().is_empty());
}
