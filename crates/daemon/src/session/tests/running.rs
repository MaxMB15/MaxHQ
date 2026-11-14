// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a lead started that still runs, from the port poll, and what its
//! driver says it waits on.

use maxhq_protocol::processes::{AgentRunning, AgentWait};

use super::*;

fn jobs(count: u32, names: &[&str]) -> AgentRunning {
    AgentRunning {
        count,
        names: names.iter().map(|name| (*name).to_owned()).collect(),
        waiting: None,
    }
}

#[tokio::test]
async fn an_agent_shows_its_holders_jobs_and_what_its_driver_waits_on() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..Say::default()
    };
    h.say(session, &say).await;
    assert_eq!(h.agent(agent).running, None);

    let two = jobs(2, &["pnpm dev", "cargo watch"]);
    h.fakes
        .ports
        .set_jobs([(session, two.clone())].into_iter().collect());
    eventually("the jobs to show", || {
        h.agent(agent).running.as_ref() == Some(&two)
    })
    .await;

    // The driver's word on what it waits on joins the poll's count.
    let wait = AgentWait::Task {
        name: "pnpm test".into(),
    };
    let mut waiting = h.reported(agent, Activity::Idle);
    waiting.running = Some(AgentRunning {
        waiting: Some(wait.clone()),
        ..AgentRunning::default()
    });
    h.say(
        session,
        &Say {
            agents: vec![waiting.clone()],
            ..Say::default()
        },
    )
    .await;
    let both = AgentRunning {
        waiting: Some(wait.clone()),
        ..two
    };
    eventually("the wait to show", || {
        h.agent(agent).running.as_ref() == Some(&both)
    })
    .await;

    // Jobs that end leave the wait, and then nothing.
    h.fakes.ports.set_jobs(crate::ports::Jobs::new());
    eventually("the jobs to go", || {
        h.agent(agent)
            .running
            .is_some_and(|running| running.count == 0 && running.waiting == Some(wait.clone()))
    })
    .await;
    waiting.running = None;
    h.say(
        session,
        &Say {
            agents: vec![waiting],
            ..Say::default()
        },
    )
    .await;
    eventually("nothing to show", || h.agent(agent).running.is_none()).await;
}
