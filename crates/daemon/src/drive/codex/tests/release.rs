// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Letting an idle thread go, and taking it up again for the next message.

use super::*;

#[test]
fn an_idle_thread_lets_go_and_the_next_message_resumes_it() {
    let mut run = Run::started(None);
    assert!(!run.server.can_release(agent()), "a turn runs");
    run.feed_all(&fixture("turn-image.jsonl")[2..]);
    run.take();
    assert!(run.server.can_release(agent()));

    run.server.release(agent(), &mut run.fx);
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "thread/unsubscribe")["params"]["threadId"],
        THREAD
    );
    assert!(fx.results.is_empty());
    run.feed(r#"{"id":"$thread/unsubscribe","result":{}}"#);
    assert!(run.take().results.is_empty());

    run.act(send(key(2), "again", SendMode::Queue));
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "thread/resume")["params"]["threadId"],
        THREAD
    );
    assert!(sent(&fx, "turn/start").is_empty());
    run.feed_all(&fixture("resume.jsonl"));
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "turn/start")["params"]["input"][0]["text"],
        "again"
    );
}

#[test]
fn a_message_on_its_way_or_a_prompt_keeps_the_thread() {
    let mut run = Run::started(None);
    run.feed_all(&fixture("turn-image.jsonl")[2..]);
    run.take();
    run.act(send(key(2), "again", SendMode::Queue));
    assert!(!run.server.can_release(agent()), "a turn starts");

    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 13));
    run.take();
    assert!(!run.server.can_release(agent()), "an approval waits on you");
}

#[test]
fn an_unknown_agent_has_nothing_to_release() {
    let run = Run::ready();
    assert!(!run.server.can_release(agent()));
}

#[test]
fn a_fresh_session_leaves_the_old_thread_and_drops_its_late_lines() {
    let mut run = Run::started(None);
    run.feed_all(&fixture("turn-image.jsonl")[2..]);
    run.take();

    // A start for an agent that has a thread is a fresh session for it.
    let fresh = "01a17276-ffff-7bb1-8561-9ac24632a961";
    let Action::Start { place, spec, .. } = start(key(2), None) else {
        unreachable!()
    };
    run.act(Action::Start {
        key: key(2),
        place,
        spec,
        first: None,
        agent: Some(agent()),
    });
    assert_eq!(sent(&run.take(), "thread/start").len(), 1);
    let started: Vec<String> = fixture_lines("turn-image.jsonl", 1, 2)
        .iter()
        .map(|line| line.replace(THREAD, fresh))
        .collect();
    run.feed_all(&started);
    let fx = run.take();
    assert_eq!(
        fx.results,
        [(key(2), ActionOutcome::Started { agent: agent() })]
    );
    assert_eq!(
        sent_once(&fx, "thread/unsubscribe")["params"]["threadId"],
        THREAD
    );
    assert_eq!(
        agent_update(&fx, agent()).vendor_session_id.as_deref(),
        Some(fresh)
    );
    assert!(!run.server.threads.contains_key(THREAD));

    // A turn the old thread still reports reaches no one.
    run.feed(&fixture_lines("turn-image.jsonl", 10, 10)[0]);
    let fx = run.take();
    assert!(fx.agents.is_empty(), "{:?}", fx.agents);
    assert!(upserts(&fx).is_empty());
    assert!(!run.server.strays.contains_key(THREAD));
}
