// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Deleting an agent: what goes with it, the stop that comes first, the
//! agents that can't be deleted, and why a deleted agent never comes back.

use super::*;

async fn delete(h: &mut Harness, agent: AgentId) -> ActionOutcome {
    h.act(|key| ClientMessage::DeleteAgent { key, agent }).await
}

/// Gives the lead a sub-agent, and the lead's timeline a reply.
pub(super) async fn add_sub(h: &Harness, lead: AgentId, session: SessionId) -> Agent {
    let sub = Agent {
        id: AgentId::generate(),
        parent: Some(lead),
        title: "Helper".into(),
        ..h.reported(lead, Activity::Busy)
    };
    let say = Say {
        agents: vec![h.reported(lead, Activity::Busy), sub.clone()],
        timeline: vec![upsert(
            lead,
            reply(lead, "r1", "on it", ItemStatus::Completed),
        )],
        ..Say::default()
    };
    h.say(session, &say).await;
    sub
}

#[tokio::test]
async fn deleting_a_running_agent_stops_it_and_forgets_it_with_its_sub_agents() {
    let mut h = Harness::new();
    let (lead, session) = h.start_claude(None).await;
    let sub = add_sub(&h, lead, session).await;
    let prompt = h.prompt(lead, "Run cargo test?");
    h.ask(session, &prompt).await;
    let (_, items, _) = reset(h.open(lead, None, 50).await);
    assert_eq!(ids(&items), ["r1"]);
    let attachments = h.dir.path().join("attachments").join(lead.to_string());
    fs::create_dir_all(&attachments).unwrap();
    fs::write(attachments.join("notes.txt"), "remember").unwrap();
    let flag = h.dir.path().join("resume").join(format!("{lead}.json"));
    fs::create_dir_all(flag.parent().unwrap()).unwrap();
    fs::write(&flag, "{}").unwrap();

    assert_eq!(delete(&mut h, lead).await, ActionOutcome::Done);
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
    eventually("the attachments going", || !attachments.exists()).await;
    for agent in [lead, sub.id] {
        assert!(h.hub.get(&EntityKey::Agent(agent)).is_none());
    }
    assert!(h.hub.get(&EntityKey::Attention(prompt.id)).is_none());
    assert!(h.db.held::<Held>().unwrap().is_empty());
    assert_eq!(
        h.db.deleted().unwrap(),
        std::collections::HashSet::from([lead, sub.id])
    );
    assert!(!flag.exists());
    // The phone's timeline closed with it, so nothing kept the old one.
    let mut laptop = Inbox::new(&h.sessions, ClientId::derive("laptop"));
    laptop.send(ClientMessage::OpenTimeline {
        agent: lead,
        since: None,
        limit: 50,
        lead: None,
    });
    let (_, items, _) = reset(laptop.timeline(lead).await);
    assert!(items.is_empty(), "{items:?}");
}

#[tokio::test]
async fn a_deleted_agent_stays_gone_through_resends_and_a_restart() {
    let mut h = Harness::new();
    let (agent, session) = h.start_claude(None).await;
    let vendor_session = h.agent(agent).vendor_session_id.unwrap();
    assert_eq!(
        h.act(|key| ClientMessage::StopAgent { key, agent }).await,
        ActionOutcome::Done
    );
    eventually("the holder's files going", || h.holders.forgotten(session)).await;

    // A stopped agent goes at once, with no second stop.
    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::DeleteAgent { key, agent });
    assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    assert!(h.hub.get(&EntityKey::Agent(agent)).is_none());
    h.phone.send(ClientMessage::DeleteAgent { key, agent });
    assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    assert_eq!(delete(&mut h, agent).await, ActionOutcome::Done);
    assert_eq!(h.holders.stops(session), [STOP_GRACE]);

    // The scanner finds the CLI's session files again.
    assert_eq!(h.outside(&vendor_session, Activity::Idle), agent);
    assert!(h.hub.get(&EntityKey::Agent(agent)).is_none());

    h.restart().await;
    assert_eq!(h.outside(&vendor_session, Activity::Idle), agent);
    tokio::time::sleep(QUIET).await;
    assert!(h.hub.get(&EntityKey::Agent(agent)).is_none());
    assert_eq!(h.holders.sessions(), [session]);
    assert_eq!(delete(&mut h, agent).await, ActionOutcome::Done);
}

#[tokio::test]
async fn an_agent_from_a_terminal_or_a_sub_agent_isnt_deleted() {
    let mut h = Harness::new();
    let outside = h.outside("outside", Activity::Idle);
    let outcome = delete(&mut h, outside).await;
    assert!(reason(&outcome).contains("outside MaxHQ"), "{outcome:?}");
    assert!(h.hub.get(&EntityKey::Agent(outside)).is_some());

    let (lead, session) = h.start_claude(None).await;
    let sub = add_sub(&h, lead, session).await;
    let outcome = delete(&mut h, sub.id).await;
    assert!(reason(&outcome).contains("sub-agent"), "{outcome:?}");
    assert!(h.hub.get(&EntityKey::Agent(sub.id)).is_some());
    assert!(h.holders.stops(session).is_empty());
    assert!(h.db.deleted().unwrap().is_empty());
}
