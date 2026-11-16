// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Actions, answers, timelines and workflow runs over a link, against the
//! fake `maxd`.

use maxhq_protocol::attention::{AnswerOutcome, AttentionAnswer};
use maxhq_protocol::ids::{AttentionId, IdempotencyKey, ProjectId};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::timeline::TimelineUpdate;
use maxhq_protocol::workflow::{RunKey, RunPatch, WorkflowUpdate};

use super::*;

fn agent() -> AgentId {
    AgentId::derive("agent")
}

pub(super) fn interrupt(key: IdempotencyKey) -> ClientMessage {
    ClientMessage::Interrupt {
        key,
        agent: agent(),
    }
}

pub(super) fn result(key: IdempotencyKey, outcome: ActionOutcome) -> ServerMessage {
    ServerMessage::ActionResult { key, outcome }
}

fn open(since: Option<u64>) -> ClientMessage {
    ClientMessage::OpenTimeline {
        agent: agent(),
        since: since.map(Seq),
        limit: 50,
        lead: None,
    }
}

fn reset(seq: u64) -> TimelineUpdate {
    TimelineUpdate::Reset {
        seq: Seq(seq),
        items: Vec::new(),
        has_older: false,
    }
}

fn later(seq: u64) -> TimelineUpdate {
    TimelineUpdate::Events {
        seq: Seq(seq),
        events: Vec::new(),
    }
}

fn timeline(update: TimelineUpdate) -> ServerMessage {
    ServerMessage::Timeline {
        agent: agent(),
        update,
    }
}

fn update(update: TimelineUpdate) -> LinkEvent {
    LinkEvent::Timeline {
        agent: agent(),
        update,
        live: false,
    }
}

fn live(update: TimelineUpdate) -> LinkEvent {
    LinkEvent::Timeline {
        agent: agent(),
        update,
        live: true,
    }
}

/// Drops the connection and takes the link through a reconnect to a fake
/// `maxd` of the given instance.
async fn reconnect(
    rig: &mut Rig,
    events: &mut mpsc::Receiver<LinkEvent>,
    maxd: FakeMaxd,
    instance: InstanceId,
) -> FakeMaxd {
    drop(maxd);
    assert_eq!(
        next_state(events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    let mut maxd = rig.next_maxd().await;
    assert_eq!(
        next_state(events).await,
        connecting(ConnectStep::Handshake, 1)
    );
    maxd.accept(instance, "boot-1").await;
    assert!(is_connected(&next_state(events).await));
    maxd
}

#[tokio::test]
async fn an_action_goes_again_under_its_key_after_a_reconnect() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;
    let key = IdempotencyKey::generate();

    let (outcome, ()) = tokio::join!(handle.act(interrupt(key)), async {
        assert_eq!(maxd.recv().await, Some(interrupt(key)));
        let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
        assert_eq!(maxd.recv().await, Some(interrupt(key)));
        maxd.send(&result(key, ActionOutcome::Done)).await;
    });
    assert_eq!(outcome, Ok(ActionOutcome::Done));
}

#[tokio::test]
async fn a_repeated_result_settles_its_action_once() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, _events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    let first = IdempotencyKey::generate();
    let second = IdempotencyKey::generate();

    let (outcome, ()) = tokio::join!(handle.act(interrupt(first)), async {
        assert_eq!(maxd.recv().await, Some(interrupt(first)));
        maxd.send(&result(first, ActionOutcome::Done)).await;
        maxd.send(&result(first, ActionOutcome::UnknownAgent)).await;
    });
    assert_eq!(outcome, Ok(ActionOutcome::Done));

    // The repeat went nowhere, and the next action gets its own result.
    let refused = ActionOutcome::Refused {
        reason: "busy".into(),
    };
    let (outcome, ()) = tokio::join!(handle.act(interrupt(second)), async {
        assert_eq!(maxd.recv().await, Some(interrupt(second)));
        maxd.send(&result(first, ActionOutcome::UnknownAgent)).await;
        maxd.send(&result(second, refused.clone())).await;
    });
    assert_eq!(outcome, Ok(refused));
}

#[tokio::test]
async fn an_answer_waits_for_its_result() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, _events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    let key = IdempotencyKey::generate();
    let event = AttentionId::derive("event");
    let answer = AttentionAnswer::Text { text: "yes".into() };

    let (outcome, ()) = tokio::join!(
        handle.answer(key, event, answer.clone(), Vec::new()),
        async {
            assert_eq!(
                maxd.recv().await,
                Some(ClientMessage::Answer {
                    idempotency_key: key,
                    event,
                    answer: answer.clone(),
                    refs: Vec::new(),
                })
            );
            maxd.send(&ServerMessage::AnswerResult {
                idempotency_key: key,
                outcome: AnswerOutcome::Expired,
            })
            .await;
        }
    );
    assert_eq!(outcome, Ok(AnswerOutcome::Expired));
}

#[tokio::test]
async fn an_action_with_no_result_times_out() {
    let timing = Timing {
        reply: Duration::from_millis(100),
        ..FAST
    };
    let mut rig = Rig::new(timing, Tailnet::default(), &[]);
    let (handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;
    let key = IdempotencyKey::generate();

    assert_eq!(handle.act(interrupt(key)).await, Err(LinkError::TimedOut));
    assert_eq!(maxd.recv().await, Some(interrupt(key)));
    assert_eq!(handle.act(mark_seen()).await, Err(LinkError::NotAnAction));

    // A late result is ignored, and the link carries on.
    maxd.send(&result(key, ActionOutcome::Done)).await;
    maxd.send(&snapshot(1, &[1])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Synced(vec![entity(1)]));
}

#[tokio::test]
async fn an_action_on_a_link_that_isnt_connected_is_refused_at_once() {
    let rig = Rig::new(NO_RETRY, Tailnet::default(), &[]);
    rig.fake
        .set_down(Some(DialError::refused("maxd isn't running")));
    let (handle, mut events) = rig.client.link(spec(Route::Local));
    assert_eq!(next_state(&mut events).await, LinkState::Unknown);
    assert_eq!(
        next_state(&mut events).await,
        connecting(ConnectStep::Dialing, 1)
    );
    assert_eq!(
        next_state(&mut events).await,
        unreachable("maxd isn't running", None)
    );
    assert_eq!(
        handle.act(interrupt(IdempotencyKey::generate())).await,
        Err(LinkError::NotConnected)
    );
    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(
        handle.load_older(agent(), 10, 20).await,
        Err(LinkError::NotConnected)
    );
}

#[tokio::test]
async fn a_timeline_reopens_after_its_last_version_on_a_reconnect() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;

    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
    maxd.send(&timeline(reset(3))).await;
    assert_eq!(next(&mut events).await, update(reset(3)));
    maxd.send(&timeline(later(4))).await;
    assert_eq!(next(&mut events).await, live(later(4)));

    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open(Some(4))));

    // A restarted maxd has none of it, so the timeline starts over.
    let mut maxd = reconnect(&mut rig, &mut events, maxd, InstanceId::generate()).await;
    assert_eq!(maxd.recv().await, Some(open(None)));
}

#[tokio::test]
async fn only_events_after_the_answer_to_an_open_are_live() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;

    handle
        .open_timeline(agent(), Some(Seq(2)), 50, None)
        .await
        .unwrap();
    assert_eq!(maxd.recv().await, Some(open(Some(2))));
    // The events since the version held answer the open.
    maxd.send(&timeline(later(3))).await;
    assert_eq!(next(&mut events).await, update(later(3)));
    maxd.send(&timeline(later(4))).await;
    assert_eq!(next(&mut events).await, live(later(4)));

    // What a reconnect catches up on came before, so it isn't live.
    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open(Some(4))));
    maxd.send(&timeline(later(6))).await;
    assert_eq!(next(&mut events).await, update(later(6)));
    maxd.send(&timeline(later(7))).await;
    assert_eq!(next(&mut events).await, live(later(7)));

    // A second open is answered too, before events are live again.
    handle
        .open_timeline(agent(), Some(Seq(7)), 50, None)
        .await
        .unwrap();
    assert_eq!(maxd.recv().await, Some(open(Some(7))));
    maxd.send(&timeline(reset(8))).await;
    assert_eq!(next(&mut events).await, update(reset(8)));
    maxd.send(&timeline(later(9))).await;
    assert_eq!(next(&mut events).await, live(later(9)));
}

#[tokio::test]
async fn a_timeline_closes_on_its_last_close() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;

    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
    assert_eq!(maxd.recv().await, Some(open(None)));

    handle.close_timeline(agent()).await.unwrap();
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));
    handle.close_timeline(agent()).await.unwrap();
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::CloseTimeline { agent: agent() })
    );
    // Opening and closing are the link's own business.
    assert_eq!(handle.send(open(None)).await, Err(LinkError::NotConnected));

    // An update sent before the close arrived goes nowhere.
    maxd.send(&timeline(later(5))).await;
    maxd.send(&snapshot(1, &[1])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Synced(vec![entity(1)]));
}

#[tokio::test]
async fn a_timeline_opened_from_a_version_asks_from_it_until_an_update_comes() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;

    handle
        .open_timeline(agent(), Some(Seq(7)), 50, None)
        .await
        .unwrap();
    assert_eq!(maxd.recv().await, Some(open(Some(7))));

    // No update came before the drop, so the reconnect asks from the same
    // version.
    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open(Some(7))));
    maxd.send(&timeline(later(9))).await;
    assert_eq!(next(&mut events).await, update(later(9)));

    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open(Some(9))));

    // Opening again asks from the version given, not the one held.
    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
}

#[tokio::test]
async fn a_sub_agents_timeline_names_its_lead_again_after_a_reconnect() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, maxd) = connected(&mut rig, instance).await;
    let lead = AgentId::derive("lead");
    let under = |since: Option<u64>| ClientMessage::OpenTimeline {
        agent: agent(),
        since: since.map(Seq),
        limit: 50,
        lead: Some(lead),
    };

    handle
        .open_timeline(agent(), None, 50, Some(lead))
        .await
        .unwrap();
    // An open that doesn't name it keeps the one it had.
    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    let mut maxd = maxd;
    assert_eq!(maxd.recv().await, Some(under(None)));
    assert_eq!(maxd.recv().await, Some(open(None)));

    let mut maxd = reconnect(&mut rig, &mut events, maxd, InstanceId::generate()).await;
    assert_eq!(maxd.recv().await, Some(under(None)));
}

fn older(before: u64) -> ClientMessage {
    ClientMessage::LoadOlder {
        agent: agent(),
        before,
        limit: 20,
    }
}

fn page(has_older: bool) -> TimelineUpdate {
    TimelineUpdate::Older {
        items: Vec::new(),
        has_older,
    }
}

#[tokio::test]
async fn a_page_asked_for_before_a_drop_comes_once_after_the_reconnect() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;

    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
    maxd.send(&timeline(reset(3))).await;
    assert_eq!(next(&mut events).await, update(reset(3)));
    handle.load_older(agent(), 10, 20).await.unwrap();
    assert_eq!(maxd.recv().await, Some(older(10)));

    // The drop lost the page, so the new connection asks again.
    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open(Some(3))));
    assert_eq!(maxd.recv().await, Some(older(10)));
    maxd.send(&timeline(page(true))).await;
    assert_eq!(next(&mut events).await, update(page(true)));

    // A page nobody waits for goes nowhere.
    maxd.send(&timeline(page(false))).await;
    maxd.send(&snapshot(1, &[1])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Synced(vec![entity(1)]));
}

#[tokio::test]
async fn a_page_that_came_isnt_asked_for_again() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;

    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
    maxd.send(&timeline(reset(3))).await;
    assert_eq!(next(&mut events).await, update(reset(3)));
    handle.load_older(agent(), 10, 20).await.unwrap();
    assert_eq!(maxd.recv().await, Some(older(10)));
    maxd.send(&timeline(page(true))).await;
    assert_eq!(next(&mut events).await, update(page(true)));

    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open(Some(3))));
    // The next page asked for is the first to go out.
    handle.load_older(agent(), 5, 20).await.unwrap();
    assert_eq!(maxd.recv().await, Some(older(5)));
}

#[tokio::test]
async fn a_page_goes_nowhere_once_its_timeline_closes() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let (handle, mut events, mut maxd) = connected(&mut rig, InstanceId::generate()).await;

    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
    handle.load_older(agent(), 10, 20).await.unwrap();
    assert_eq!(maxd.recv().await, Some(older(10)));
    handle.close_timeline(agent()).await.unwrap();
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::CloseTimeline { agent: agent() })
    );

    // Asking a closed timeline sends nothing, and paging is the link's own
    // business.
    handle.load_older(agent(), 10, 20).await.unwrap();
    assert_eq!(handle.send(older(10)).await, Err(LinkError::NotConnected));
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));

    // A reopened timeline doesn't take the page asked for before as the
    // answer to its own.
    handle.open_timeline(agent(), None, 50, None).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open(None)));
    handle.load_older(agent(), 8, 20).await.unwrap();
    assert_eq!(maxd.recv().await, Some(older(8)));
    maxd.send(&timeline(page(true))).await;
    maxd.send(&timeline(reset(4))).await;
    assert_eq!(next(&mut events).await, update(reset(4)));
    maxd.send(&timeline(page(false))).await;
    assert_eq!(next(&mut events).await, update(page(false)));
}

fn run() -> RunKey {
    RunKey {
        device: DeviceId::derive("devbox"),
        project: ProjectId::derive("git:github.com/owner/repo"),
        workflow: "build".to_owned(),
        run: "r1".to_owned(),
    }
}

fn patch(rev: u64) -> Box<WorkflowUpdate> {
    Box::new(WorkflowUpdate::Patch(RunPatch {
        key: run(),
        rev,
        nodes: Vec::new(),
        loops: None,
        header: None,
    }))
}

#[tokio::test]
async fn a_workflow_run_reopens_on_a_reconnect_and_closes_on_its_last_close() {
    let mut rig = Rig::new(FAST, Tailnet::default(), &[]);
    let instance = InstanceId::generate();
    let (handle, mut events, mut maxd) = connected(&mut rig, instance).await;
    let open = || ClientMessage::OpenWorkflow { run: run() };

    handle.open_workflow(run()).await.unwrap();
    handle.open_workflow(run()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(open()));
    assert_eq!(maxd.recv().await, Some(open()));
    let update = ServerMessage::Workflow {
        run: run(),
        update: patch(2),
    };
    maxd.send(&update).await;
    assert_eq!(
        next(&mut events).await,
        LinkEvent::Workflow {
            run: run(),
            update: patch(2)
        }
    );

    // A reconnect opens it once, and maxd starts it over with a reset.
    let mut maxd = reconnect(&mut rig, &mut events, maxd, instance).await;
    assert_eq!(maxd.recv().await, Some(open()));

    handle.close_workflow(run()).await.unwrap();
    handle.send(mark_seen()).await.unwrap();
    assert_eq!(maxd.recv().await, Some(mark_seen()));
    handle.close_workflow(run()).await.unwrap();
    assert_eq!(
        maxd.recv().await,
        Some(ClientMessage::CloseWorkflow { run: run() })
    );
    // Opening and closing are the link's own business.
    assert_eq!(handle.send(open()).await, Err(LinkError::NotConnected));

    // An update sent before the close arrived goes nowhere.
    maxd.send(&update).await;
    maxd.send(&snapshot(1, &[1])).await;
    assert_eq!(next(&mut events).await, LinkEvent::Synced(vec![entity(1)]));
}
