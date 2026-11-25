// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A task's runs in a fresh session: the agent ends its vendor session
//! and starts a new one in the same place, whose first message is the
//! prompt, once its turn is over.

use maxhq_protocol::agents::{Actor, Limits};
use maxhq_protocol::ids::TaskId;
use maxhq_protocol::tasks::{RunOutcome, TaskTarget};
use serde_json::json;

use super::tasks::{hourly, task};
use super::*;

impl Harness {
    /// Saves a task that runs `agent` in a fresh session, and runs it now.
    /// Gives the task.
    async fn run_fresh(&mut self, agent: AgentId) -> TaskId {
        let target = TaskTarget::Agent {
            device: self.host.id,
            agent,
            fresh: true,
        };
        let saved = self.save(task("Triage", hourly(), target)).await;
        assert_eq!(self.run_now(saved.id).await, ActionOutcome::Done);
        saved.id
    }

    /// Waits for a holder past the first `before`, and gives it.
    async fn new_holder(&self, before: usize) -> SessionId {
        eventually("a new holder", || self.holders.sessions().len() > before).await;
        self.holders.last()
    }
}

/// How many fresh-session markers the timeline holds.
fn markers(items: &[TimelineItem]) -> usize {
    items
        .iter()
        .filter(|item| matches!(item.body, ItemBody::NewSession))
        .count()
}

#[tokio::test]
async fn a_fresh_run_starts_a_new_claude_session_with_the_prompt_first() {
    let mut h = Harness::new();
    let (agent, session) = h.claude_after_a_turn().await;
    let (old, _) = h.vendors.launches()[0];
    let holders = h.holders.sessions().len();
    let id = h.run_fresh(agent).await;

    // The old CLI stops, and a session named before it starts takes its
    // place, with nothing to resume.
    eventually("the old CLI stopping", || {
        h.holders.stops(session) == [STOP_GRACE]
    })
    .await;
    eventually("a new Claude CLI", || h.vendors.launches().len() == 2).await;
    let (uuid, resume) = h.vendors.launches()[1];
    assert!(!resume);
    assert_ne!(uuid, old);
    let stored = h.stored(agent);
    assert_eq!(stored.vendor_session, uuid.to_string());
    assert_eq!(stored.left, Some(old.to_string()));
    assert_eq!(stored.cwd, h.folder);

    // The prompt is the new session's first message, and nothing from
    // before goes with it.
    let fresh = h.new_holder(holders).await;
    eventually("the prompt", || !h.sent(fresh).is_empty()).await;
    assert_eq!(h.sent(fresh), ["Run Triage"]);
    assert_eq!(h.agent(agent).id, agent);
    assert_eq!(h.agent(agent).context, None);

    // The timeline keeps what came before, under the marker.
    let items = h.items(agent).await;
    assert_eq!(ids(&items)[..2], ["u1", "a1"]);
    assert_eq!(markers(&items), 1);

    // The run is the new session's turn.
    h.moves(fresh, agent, Activity::Busy).await;
    h.moves(fresh, agent, Activity::DoneUnread).await;
    h.run_where(id, "the end", |run| run.outcome == RunOutcome::Done)
        .await;
}

#[tokio::test]
async fn a_fresh_run_waits_for_a_busy_agents_turn_to_end() {
    let mut h = Harness::new();
    let (agent, session) = h.claude_after_a_turn().await;
    h.moves(session, agent, Activity::Busy).await;
    let holders = h.holders.sessions().len();
    let id = h.run_fresh(agent).await;

    // The turn goes on: nothing stops or starts, and the run waits.
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(session).is_empty());
    assert_eq!(h.vendors.launches().len(), 1);
    assert!(!h.sent(session).contains(&"Run Triage".to_owned()));
    assert_eq!(h.stored(agent).fresh.len(), 1);
    assert_eq!(h.activity(agent), Activity::Busy);
    assert_eq!(markers(&h.items(agent).await), 0);

    // Once it ends, the run starts the fresh session.
    let done = Say {
        agents: vec![h.reported(agent, Activity::DoneUnread)],
        ..Say::default()
    };
    h.say(session, &done).await;
    eventually("a new Claude CLI", || h.vendors.launches().len() == 2).await;
    assert!(!h.vendors.launches()[1].1);
    let fresh = h.new_holder(holders).await;
    eventually("the prompt", || !h.sent(fresh).is_empty()).await;
    assert_eq!(h.sent(fresh), ["Run Triage"]);
    assert!(h.stored(agent).fresh.is_empty());
    assert_eq!(markers(&h.items(agent).await), 1);

    // The turn it waited for wasn't the run's.
    let run = h
        .run_where(id, "the run", |run| run.outcome == RunOutcome::Running)
        .await;
    assert!(run.ended.is_none());
    h.moves(fresh, agent, Activity::Busy).await;
    h.moves(fresh, agent, Activity::DoneUnread).await;
    h.run_where(id, "the end", |run| run.outcome == RunOutcome::Done)
        .await;
}

#[tokio::test]
async fn a_fresh_run_starts_a_new_codex_thread_and_the_old_one_is_ignored() {
    let mut h = Harness::new();
    let (agent, server) = h.start_codex("thread-1").await;
    let idle = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![upsert(
            agent,
            reply(agent, "a1", "Shipped it", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(server, &idle).await;
    let from = h.vendors.log().len();
    h.run_fresh(agent).await;

    // A thread starts for the same agent, then gets the prompt alone.
    let server = h.codex_thread(agent, from, "thread-2").await;
    eventually("the prompt", || {
        h.sent(server).contains(&"Run Triage".to_owned())
    })
    .await;
    assert_eq!(
        h.sent(server).last().map(String::as_str),
        Some("Run Triage")
    );
    let stored = h.stored(agent);
    assert_eq!(stored.vendor_session, "thread-2");
    assert_eq!(stored.left.as_deref(), Some("thread-1"));
    assert!(!stored.adopt);
    assert_eq!(markers(&h.items(agent).await), 1);

    // Late news from the old thread doesn't count.
    let late = Say {
        agents: vec![h.codex_agent("thread-1", Activity::Busy)],
        ..Say::default()
    };
    h.say(server, &late).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.activity(agent), Activity::Idle);
    assert_eq!(
        h.agent(agent).vendor_session_id.as_deref(),
        Some("thread-2")
    );
}

#[tokio::test]
async fn a_fresh_run_starts_a_new_acp_session() {
    let mut h = Harness::new();
    let account = h.acp_agent(false);
    let mut spec = h.spec(Vendor::Acp, Some("Build it"));
    spec.account = account;
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    eventually("the first message", || !h.sent(session).is_empty()).await;
    let idle = Say {
        agents: vec![Agent {
            vendor_session_id: Some("acp-1".into()),
            ..h.reported(agent, Activity::Idle)
        }],
        ..Say::default()
    };
    h.say(session, &idle).await;
    eventually("the session named", || {
        h.agent(agent).vendor_session_id.as_deref() == Some("acp-1")
    })
    .await;
    let holders = h.holders.sessions().len();
    h.run_fresh(agent).await;

    // The new CLI makes a new session rather than load the old one, and
    // its items get IDs of their own.
    eventually("the old CLI stopping", || {
        h.holders.stops(session) == [STOP_GRACE]
    })
    .await;
    eventually("a new ACP CLI", || h.vendors.acp_kept().len() == 2).await;
    let kept = h.vendors.acp_kept()[1].clone();
    assert_eq!((kept.session, kept.epoch), (None, 1));
    let fresh = h.new_holder(holders).await;
    eventually("the prompt", || !h.sent(fresh).is_empty()).await;
    assert_eq!(h.sent(fresh), ["Run Triage"]);
    assert_eq!(h.stored(agent).left.as_deref(), Some("acp-1"));
}

#[tokio::test]
async fn only_a_task_asks_for_a_fresh_session() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(Some("Plan it")).await;
    let (other, theirs) = h.start_claude(Some("Build it")).await;
    h.grant(Actor::Agent { agent: lead }, Limits::default())
        .await;
    let send = json!({"request_id": "f1", "agent": other, "text": "Start over", "fresh": true});
    assert_eq!(
        h.call(session, lead, "f1", "send_agent", send).await,
        Err("Only a scheduled task starts a fresh session.".to_owned())
    );
    assert!(!h.sent(theirs).contains(&"Start over".to_owned()));
    assert_eq!(h.vendors.launches().len(), 2);
}
