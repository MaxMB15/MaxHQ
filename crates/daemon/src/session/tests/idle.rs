// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Stopping the CLI of an agent left idle, and starting it again for the
//! next message.

use maxhq_protocol::model::DeviceSettingChange;
use maxhq_protocol::state::Delta;
use tokio::sync::broadcast;

use super::*;
use crate::session::holders::Credentials as _;

/// Sets how long an agent stays idle before its CLI stops, with 0 for
/// never.
async fn release_after(h: &mut Harness, minutes: u16) {
    let outcome = h
        .act(|key| ClientMessage::UpdateDeviceSettings {
            key,
            device: device().id,
            changes: vec![DeviceSettingChange::IdleReleaseMinutes { value: minutes }],
        })
        .await;
    assert_eq!(outcome, ActionOutcome::Done);
}

/// A message the CLI has in its transcript.
fn said(agent: AgentId, id: &str, text: &str) -> TimelineItem {
    let body = ItemBody::UserMessage {
        text: text.into(),
        attachments: Vec::new(),
        refs: Vec::new(),
        delivery: Delivery::Sent,
    };
    item(agent, id, ItemStatus::Completed, body)
}

/// An idle Claude agent with a conversation `--resume` can load.
async fn idle_claude(h: &mut Harness) -> (AgentId, SessionId) {
    let (agent, session) = h.start_claude(Some("hi")).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![upsert(agent, said(agent, "u1", "hi"))],
        ..Say::default()
    };
    h.say(session, &say).await;
    (agent, session)
}

/// Moves the clock on by `minutes`.
fn wait(h: &Harness, minutes: u64) {
    h.clock.advance(Duration::from_secs(minutes * 60));
}

/// Moves the clock on by `minutes` and checks the CLI still runs.
async fn still_runs(h: &Harness, session: SessionId, minutes: u64, why: &str) {
    wait(h, minutes);
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(session).is_empty(), "{why}");
}

/// Moves the clock on by `minutes` and waits for the CLI to stop.
async fn stops(h: &Harness, session: SessionId, minutes: u64) {
    wait(h, minutes);
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    eventually("the holder's files going", || h.holders.forgotten(session)).await;
}

/// The upserts of `agent` the hub sent since `deltas` began.
fn upserts(deltas: &mut broadcast::Receiver<Delta>, agent: AgentId) -> Vec<Agent> {
    let mut out = Vec::new();
    while let Ok(delta) = deltas.try_recv() {
        for change in delta.changes {
            if let Change::Upsert {
                value: Entity::Agent(sent),
            } = change
                && sent.id == agent
            {
                out.push(sent);
            }
        }
    }
    out
}

#[tokio::test]
async fn an_idle_cli_stops_after_the_set_time_and_its_agent_stays_as_it_was() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    let before = h.agent(agent);

    still_runs(&h, session, 14, "not idle long enough yet").await;
    stops(&h, session, 2).await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [session], "nothing starts it again");
    let shown = h.agent(agent);
    assert_eq!(shown.activity, Activity::Idle);
    assert_eq!(shown.link, AgentLink::Live);
    assert_eq!(shown.last_activity_at, before.last_activity_at);
    assert_eq!(shown.inbox, before.inbox);

    // The next message starts it again on the same session.
    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    let resumed = h.holders.last();
    assert_ne!(resumed, session);
    assert_eq!(
        h.vendors.launches().last().map(|launch| launch.1),
        Some(true)
    );
    assert_eq!(h.sent(resumed), ["go on"]);
}

#[tokio::test]
async fn a_stopped_cli_marks_its_agent_asleep_once_and_the_next_message_wakes_it() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    assert!(!h.agent(agent).asleep);
    let mut deltas = h.hub.subscribe(None).deltas;

    stops(&h, session, 16).await;
    // Later checks find it asleep already and send nothing.
    for _ in 0..3 {
        wait(&h, 2);
        tokio::time::sleep(QUIET).await;
    }
    assert_eq!(h.holders.sessions(), [session], "nothing starts it again");
    let sent = upserts(&mut deltas, agent);
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert!(sent[0].asleep);
    assert_eq!(sent[0].link, AgentLink::Live);
    assert!(h.agent(agent).asleep);

    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    assert!(!h.agent(agent).asleep);
    assert!(upserts(&mut deltas, agent).iter().any(|sent| !sent.asleep));
}

#[tokio::test]
async fn the_default_stops_an_idle_cli_after_an_hour() {
    let mut h = Harness::new();
    let (_, session) = idle_claude(&mut h).await;
    still_runs(&h, session, 59, "not idle an hour yet").await;
    stops(&h, session, 2).await;
}

#[tokio::test]
async fn never_keeps_every_cli_running() {
    let mut h = Harness::new();
    release_after(&mut h, 0).await;
    let (_, session) = idle_claude(&mut h).await;
    still_runs(&h, session, 3 * 24 * 60, "the setting says never").await;
}

#[tokio::test]
async fn a_released_agent_stays_released_across_a_restart() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    stops(&h, session, 16).await;

    h.restart().await;
    eventually("the agent coming back", || {
        h.hub.get(&EntityKey::Agent(agent)).is_some()
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [session], "it isn't resumed");
    let shown = h.agent(agent);
    assert_eq!(shown.activity, Activity::Idle);
    assert_eq!(shown.link, AgentLink::Live);
    assert!(shown.asleep, "it follows the stored release");

    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    assert_eq!(h.sent(h.holders.last()), ["go on"]);
    assert!(!h.agent(agent).asleep);
}

#[tokio::test]
async fn a_turn_keeps_the_cli_and_idle_time_counts_from_its_end() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        turn: Some(true),
        ..Say::default()
    };
    h.say(session, &say).await;
    still_runs(&h, session, 30, "a turn runs").await;

    // A message waits behind the turn.
    assert_eq!(h.send(agent, "next").await, ActionOutcome::Done);
    still_runs(&h, session, 30, "a turn runs").await;

    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        turn: Some(false),
        ..Say::default()
    };
    h.say(session, &say).await;
    still_runs(&h, session, 14, "the turn just ended").await;
    stops(&h, session, 2).await;
}

#[tokio::test]
async fn a_question_for_you_keeps_the_cli() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    h.ask(session, &h.prompt(agent, "Run cargo test?")).await;
    still_runs(&h, session, 60, "a question waits on you").await;
}

#[tokio::test]
async fn work_only_the_cli_holds_keeps_it() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (_, session) = idle_claude(&mut h).await;
    let keeps = |keeps| Say {
        keeps: Some(keeps),
        ..Say::default()
    };
    h.say(session, &keeps(true)).await;
    still_runs(&h, session, 60, "a background command runs").await;
    h.say(session, &keeps(false)).await;
    still_runs(&h, session, 14, "it just ended").await;
    stops(&h, session, 2).await;
}

#[tokio::test]
async fn an_open_pane_keeps_the_cli() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    h.open(agent, None, 50).await;
    still_runs(&h, session, 60, "you're watching it").await;

    h.phone.send(ClientMessage::CloseTimeline { agent });
    tokio::time::sleep(QUIET).await;
    still_runs(&h, session, 14, "you just looked away").await;
    stops(&h, session, 2).await;
}

#[tokio::test]
async fn a_session_with_nothing_to_resume_keeps_its_cli() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (_, session) = h.start_claude(None).await;
    still_runs(&h, session, 60, "--resume would find nothing").await;
}

#[tokio::test]
async fn a_message_sent_while_the_cli_stops_waits_for_it_to_start_again() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (agent, session) = idle_claude(&mut h).await;
    h.holders.linger_on_stop();
    wait(&h, 16);
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;

    let key = IdempotencyKey::generate();
    h.phone.send(ClientMessage::SendInput {
        key,
        agent,
        input: UserInput {
            text: "go on".into(),
            attachments: Vec::new(),
            refs: Vec::new(),
        },
        mode: SendMode::Queue,
    });
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.sent(session), ["hi"], "the stopping CLI gets nothing");

    h.holders.exit(session, None, Some(15));
    assert_eq!(h.phone.result(key).await, ActionOutcome::Done);
    let resumed = h.holders.last();
    assert_ne!(resumed, session);
    assert_eq!(h.sent(resumed), ["go on"]);
}

#[tokio::test]
async fn an_idle_codex_thread_lets_go_and_its_app_server_stops_once_none_is_in_use() {
    let mut h = Harness::new();
    release_after(&mut h, 15).await;
    let (first, server) = h.start_codex("thread-1").await;
    let (second, _) = h.start_codex("thread-2").await;
    let idle = |h: &Harness, agent| Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        ..Say::default()
    };
    h.say(server, &idle(&h, first)).await;

    wait(&h, 16);
    h.logged(&format!("release {first}")).await;
    tokio::time::sleep(QUIET).await;
    assert!(
        h.holders.stops(server).is_empty(),
        "the other thread is busy"
    );
    assert_eq!(h.agent(first).link, AgentLink::Live);
    assert!(h.agent(first).asleep);
    assert!(!h.agent(second).asleep);

    h.say(server, &idle(&h, second)).await;
    wait(&h, 16);
    h.logged(&format!("release {second}")).await;
    eventually("the app-server's stop", || {
        h.holders.stops(server) == [STOP_GRACE]
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(h.holders.sessions(), [server]);
    assert_eq!(h.agent(first).link, AgentLink::Live);
    assert_eq!(h.agent(first).activity, Activity::Idle);

    // The next message starts a new app-server, which takes the thread up.
    let from = h.vendors.log().len();
    assert_eq!(h.send(first, "go on").await, ActionOutcome::Done);
    let log = h.vendors.log()[from..].to_vec();
    assert!(
        log.iter().any(|logged| logged == "adopt thread-1"),
        "{log:#?}"
    );
    let again = h.holders.last();
    assert_ne!(again, server);
    let sent = h.sent(again);
    assert_eq!(sent.last().map(String::as_str), Some("go on"), "{sent:#?}");
    assert!(!h.agent(first).asleep);
    assert!(h.agent(second).asleep);
}

#[tokio::test]
async fn signing_in_again_stops_the_accounts_idle_clis_even_with_a_pane_open() {
    let mut h = Harness::new();
    // Never releasing on a timer doesn't keep them from the new login.
    release_after(&mut h, 0).await;
    let (agent, session) = idle_claude(&mut h).await;
    h.open(agent, None, 50).await;
    let (busy, running) = h.start_claude(Some("work")).await;
    let say = Say {
        agents: vec![h.reported(busy, Activity::Busy)],
        turn: Some(true),
        ..Say::default()
    };
    h.say(running, &say).await;
    let (thread, server) = h.start_codex("thread-1").await;
    let say = Say {
        agents: vec![h.reported(thread, Activity::Idle)],
        ..Say::default()
    };
    h.say(server, &say).await;

    h.sessions.signed_in(h.claude);
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(running).is_empty(), "a turn runs");
    assert!(h.holders.stops(server).is_empty(), "another account's");
    assert!(h.agent(agent).asleep);
    assert!(!h.agent(busy).asleep);

    // The next message starts it again, on the new login.
    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    assert_eq!(h.sent(h.holders.last()), ["go on"]);
}

#[tokio::test]
async fn signing_in_to_codex_again_stops_its_idle_app_server() {
    let mut h = Harness::new();
    let (thread, server) = h.start_codex("thread-1").await;

    // A busy thread keeps it running.
    h.sessions.signed_in(h.codex);
    tokio::time::sleep(QUIET).await;
    assert!(h.holders.stops(server).is_empty());

    let say = Say {
        agents: vec![h.reported(thread, Activity::Idle)],
        ..Say::default()
    };
    h.say(server, &say).await;
    h.sessions.signed_in(h.codex);
    eventually("the app-server's stop", || {
        h.holders.stops(server) == [STOP_GRACE]
    })
    .await;
    assert!(h.agent(thread).asleep);
}

/// An idle agent on the local account's model server, with its account.
async fn idle_local(h: &mut Harness) -> (AccountId, AgentId, SessionId) {
    let local = h.local_account();
    let mut spec = h.spec(Vendor::Claude, Some("hi"));
    spec.account = local;
    let agent = started(h.act(|key| ClientMessage::StartAgent { key, spec }).await);
    let session = h.holders.last();
    eventually("the first message", || {
        h.sent(session).contains(&"hi".to_owned())
    })
    .await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        timeline: vec![upsert(agent, said(agent, "u1", "hi"))],
        ..Say::default()
    };
    h.say(session, &say).await;
    (local, agent, session)
}

#[tokio::test]
async fn a_replaced_server_key_stops_a_local_agents_idle_cli_even_with_a_pane_open() {
    let mut h = Harness::new();
    // Never releasing on a timer doesn't keep it on the old key.
    release_after(&mut h, 0).await;
    let (local, agent, session) = idle_local(&mut h).await;
    h.open(agent, None, 50).await;
    assert_eq!(
        h.vendors.claude_logins().last(),
        Some(&LoginKind::ModelServer)
    );

    // A run on the same settings keeps it.
    h.sessions.local_run(local);
    still_runs(&h, session, 1, "the server is as it started on").await;

    // A sign-in replaces the key.
    let mut server = h
        .fakes
        .credentials
        .model_server(local)
        .expect("the local account has a server");
    server.token = maxhq_protocol::secret::Secret::new("signed-in-key".into());
    h.fakes.credentials.set_model_server(local, server);
    h.sessions.local_run(local);
    eventually("the stop", || h.holders.stops(session) == [STOP_GRACE]).await;
    assert!(h.agent(agent).asleep);
    assert_eq!(h.agent(agent).link, AgentLink::Live);

    // The next message starts it again, on the new key.
    assert_eq!(h.send(agent, "go on").await, ActionOutcome::Done);
    let resumed = h.holders.last();
    assert_ne!(resumed, session);
    assert_eq!(h.sent(resumed), ["go on"]);
    still_runs(&h, resumed, 3, "it runs on the new key").await;
}

#[tokio::test]
async fn a_local_agents_cli_waits_for_its_turn_to_end_before_a_new_cap_reaches_it() {
    let mut h = Harness::new();
    release_after(&mut h, 0).await;
    let (local, agent, session) = idle_local(&mut h).await;
    let say = Say {
        agents: vec![h.reported(agent, Activity::Busy)],
        turn: Some(true),
        ..Say::default()
    };
    h.say(session, &say).await;

    // The server counts as cloud now, so its output cap grows.
    let mut server = h
        .fakes
        .credentials
        .model_server(local)
        .expect("the local account has a server");
    server.cloud = true;
    h.fakes.credentials.set_model_server(local, server);
    h.sessions.local_run(local);
    still_runs(&h, session, 3, "a turn runs").await;

    let say = Say {
        agents: vec![h.reported(agent, Activity::Idle)],
        turn: Some(false),
        ..Say::default()
    };
    h.say(session, &say).await;
    stops(&h, session, 2).await;
}
