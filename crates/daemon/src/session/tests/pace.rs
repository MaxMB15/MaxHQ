// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How often clients get a streaming agent: a few times a second, with
//! what can't wait at once.

use maxhq_protocol::model::ContextUsage;
use maxhq_protocol::state::Delta;
use tokio::sync::broadcast;

use super::*;

/// The agent's upserts among the deltas so far.
fn upserts(deltas: &mut broadcast::Receiver<Delta>, id: AgentId) -> Vec<Agent> {
    let mut agents = Vec::new();
    while let Ok(delta) = deltas.try_recv() {
        for change in delta.changes {
            if let Change::Upsert {
                value: Entity::Agent(agent),
            } = change
                && agent.id == id
            {
                agents.push(agent);
            }
        }
    }
    agents
}

/// The agent as its driver reports it at chunk `n` of a stream.
fn chunk(h: &Harness, base: &Agent, n: u32) -> Say {
    Say {
        agents: vec![Agent {
            last_activity_at: h.clock.wall(),
            context: Some(ContextUsage {
                used_tokens: 1000 + n,
                window_tokens: 200_000,
            }),
            ..base.clone()
        }],
        ..Say::default()
    }
}

/// A busy agent, after a quiet spell, and its holder.
async fn busy(h: &mut Harness) -> (AgentId, SessionId, Agent) {
    let (agent, session) = h.start_claude(None).await;
    let base = h.reported(agent, Activity::Busy);
    h.say(
        session,
        &Say {
            agents: vec![base.clone()],
            ..Say::default()
        },
    )
    .await;
    h.clock.advance(Duration::from_secs(5));
    (agent, session, base)
}

#[tokio::test]
async fn a_hundred_chunks_in_a_second_send_the_agent_at_most_five_times() {
    let mut h = Harness::new();
    let (agent, session, base) = busy(&mut h).await;
    let mut deltas = h.hub.subscribe(None).deltas;
    for n in 0..100 {
        h.say(session, &chunk(&h, &base, n)).await;
        h.clock.advance(Duration::from_millis(10));
    }
    let last = Timestamp(h.clock.wall().0 - 10);
    eventually("the last chunk", || h.agent(agent).last_activity_at == last).await;

    let sent = upserts(&mut deltas, agent);
    println!("upserts for 100 chunks in a second: {}", sent.len());
    assert!(sent.len() <= 5, "{} upserts", sent.len());
    let shown = sent.last().expect("the stream went out");
    assert_eq!(shown.last_activity_at, last);
    assert_eq!(shown.context.map(|context| context.used_tokens), Some(1099));

    // Nothing more comes of a quiet agent.
    h.clock.advance(Duration::from_secs(5));
    tokio::time::sleep(QUIET).await;
    assert_eq!(upserts(&mut deltas, agent), []);
}

#[tokio::test]
async fn a_limit_mid_stream_goes_at_once_with_the_newest_chunk() {
    let mut h = Harness::new();
    let (agent, session, base) = busy(&mut h).await;
    for n in 0..50 {
        h.say(session, &chunk(&h, &base, n)).await;
        h.clock.advance(Duration::from_millis(10));
    }
    // The clock stops, so only a change that can't wait goes out now.
    let limited = Activity::WaitingForLimitReset { resets_at: None };
    let mut say = chunk(&h, &base, 50);
    say.agents[0].activity = limited.clone();
    h.say(session, &say).await;
    eventually("the limit", || h.activity(agent) == limited).await;
    let shown = h.agent(agent);
    assert_eq!(shown.last_activity_at, h.clock.wall());
    assert_eq!(shown.context.map(|context| context.used_tokens), Some(1050));
}

#[tokio::test]
async fn a_rename_mid_stream_takes_the_waiting_chunk_along() {
    let mut h = Harness::new();
    let (agent, session, base) = busy(&mut h).await;
    h.say(session, &chunk(&h, &base, 0)).await;
    h.clock.advance(Duration::from_millis(10));
    h.say(session, &chunk(&h, &base, 1)).await;
    assert_eq!(
        h.agent(agent).context.map(|context| context.used_tokens),
        Some(1000)
    );

    let outcome = h
        .act(|key| ClientMessage::RenameAgent {
            key,
            agent,
            title: "Ship the fix".into(),
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let shown = h.agent(agent);
    assert_eq!(shown.title, "Ship the fix");
    assert_eq!(shown.context.map(|context| context.used_tokens), Some(1001));
}
