// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Find across an agent's timeline: the counts, the active match and the
//! items around it, from the timeline maxd keeps or a session's history.

use maxhq_protocol::timeline::{FindAt, FindItem, FindMatch, TextField, TimelineFind};

use super::*;

impl Harness {
    async fn find(&mut self, agent: AgentId, query: &str, at: FindAt) -> ActionOutcome {
        let query = query.to_owned();
        self.act(|key| ClientMessage::FindInTimeline {
            key,
            agent,
            query,
            at,
        })
        .await
    }

    async fn found(&mut self, agent: AgentId, query: &str, at: FindAt) -> TimelineFind {
        match self.find(agent, query, at).await {
            ActionOutcome::TimelineFind { find } => find,
            other => panic!("find answers with what it found, got {other:?}"),
        }
    }
}

/// A message you sent.
fn sent(agent: AgentId, id: &str, text: &str) -> TimelineItem {
    let body = ItemBody::UserMessage {
        text: text.into(),
        attachments: Vec::new(),
        refs: Vec::new(),
        delivery: Delivery::Sent,
    };
    item(agent, id, ItemStatus::Completed, body)
}

fn first() -> FindAt {
    FindAt::Index { index: 0 }
}

fn step(item: &str, occurrence: u32, offset: i32) -> FindAt {
    FindAt::From {
        from: FindMatch {
            item: ItemId(item.into()),
            occurrence,
        },
        offset,
    }
}

fn near(items: &[(&str, u32, u32)]) -> Vec<FindItem> {
    items
        .iter()
        .map(|(item, first, count)| FindItem {
            item: ItemId((*item).into()),
            first: *first,
            count: *count,
        })
        .collect()
}

#[tokio::test]
async fn find_counts_a_query_in_the_timeline_maxd_keeps() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![
            upsert(agent, sent(agent, "u1", "Fix the flaky test")),
            upsert(
                agent,
                command(agent, "c1", "cargo test flaky", ItemStatus::Completed),
            ),
            upsert(agent, queued(agent, "q1", "and the other flaky one")),
            upsert(
                agent,
                reply(
                    agent,
                    "r1",
                    "The **flaky** test\n\nwas flaky, now it's not.",
                    ItemStatus::Completed,
                ),
            ),
        ],
        ..Say::default()
    };
    h.say(session, &say).await;

    let find = h.found(agent, "  FLAKY ", first()).await;
    assert_eq!((find.total, find.active), (4, 0));
    assert_eq!(
        find.found,
        Some(FindMatch {
            item: ItemId("u1".into()),
            occurrence: 0,
        })
    );
    assert_eq!(find.near, near(&[("u1", 0, 1), ("c1", 1, 1), ("r1", 2, 2)]));

    // Stepping from a match reuses the counts.
    let find = h.found(agent, "flaky", step("r1", 1, 1)).await;
    assert_eq!(find.active, 0);
    let find = h.found(agent, "flaky", step("u1", 0, -1)).await;
    assert_eq!(
        find.found,
        Some(FindMatch {
            item: ItemId("r1".into()),
            occurrence: 1,
        })
    );
    // A match can't cross the break between two paragraphs.
    let find = h.found(agent, "test was", first()).await;
    assert_eq!((find.total, find.found), (0, None));

    // New text counts at once.
    let more = Say {
        timeline: vec![
            (
                agent,
                TimelineEvent::Append {
                    id: ItemId("r1".into()),
                    field: TextField::Text,
                    text: " Flaky no more.".into(),
                },
            ),
            upsert(agent, sent(agent, "u2", "Thanks, flaky friend")),
        ],
        ..Say::default()
    };
    h.say(session, &more).await;
    let find = eventually_found(&mut h, agent, 6).await;
    assert_eq!(
        find.near,
        near(&[("u1", 0, 1), ("c1", 1, 1), ("r1", 2, 3), ("u2", 5, 1)])
    );
}

/// What find finds of "flaky" once it counts `total` matches.
async fn eventually_found(h: &mut Harness, agent: AgentId, total: u32) -> TimelineFind {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let find = h.found(agent, "flaky", first()).await;
        if find.total == total || tokio::time::Instant::now() > deadline {
            assert_eq!(find.total, total);
            return find;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn find_reads_an_outside_sessions_history() {
    let mut h = Harness::new();
    let agent = h.outside("outside-1", Activity::Idle);
    h.vendors.set_history(
        "outside-1",
        vec![
            sent(agent, "o1", "Rework the API"),
            reply(agent, "o2", "Reworked the api.", ItemStatus::Completed),
        ],
    );
    let find = h.found(agent, "api", step("o2", 0, 0)).await;
    assert_eq!((find.total, find.active), (2, 1));
    assert_eq!(find.near, near(&[("o1", 0, 1), ("o2", 1, 1)]));
}

#[tokio::test]
async fn find_refuses_a_query_it_cant_look_for() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let say = Say {
        timeline: vec![upsert(agent, sent(agent, "u1", "Find me"))],
        ..Say::default()
    };
    h.say(session, &say).await;
    assert_eq!(
        reason(&h.find(agent, "  \n ", first()).await),
        "Type something to find."
    );
    let long = "x".repeat(201);
    assert_eq!(
        reason(&h.find(agent, &long, first()).await),
        "Find looks for at most 200 characters."
    );
    let find = h.found(agent, &"x".repeat(200), first()).await;
    assert_eq!(find.total, 0);
    let find = h.found(agent, "FIND ME", first()).await;
    assert_eq!(find.total, 1);
    let nobody = AgentId::generate();
    assert_eq!(
        reason(&h.find(nobody, "x", first()).await),
        "This device doesn't have that agent."
    );
}
