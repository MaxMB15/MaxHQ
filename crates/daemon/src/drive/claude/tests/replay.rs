// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The recorded session played whole, played across a reconnect and a
//! restart, and read back from its transcript.

use std::fs;
use std::sync::atomic::Ordering;

use maxhq_core::names;
use maxhq_protocol::attention::{AttentionAnswer, AttentionKind, AttentionOption};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::model::{Activity, AgentLink, AgentPermission, ModelChoice, PermissionMode};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    ApprovalKind, Delivery, FileOp, ItemBody, ItemStatus, TextField, TimelineEvent,
};
use serde_json::{json, Value};

use super::*;
use crate::drive::claude::{can_continue, History};

/// The statuses of an item that hasn't ended.
fn unfinished(status: ItemStatus) -> bool {
    matches!(
        status,
        ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
    )
}

/// The one change a file item shows.
fn change(item: &TimelineItem) -> &maxhq_protocol::timeline::FileEdit {
    match &item.body {
        ItemBody::FileChange { changes } if changes.len() == 1 => &changes[0],
        body => panic!("not one file change: {body:?}"),
    }
}

#[test]
fn plays_the_session_and_writes_each_line_maxd_wrote() {
    let mut run = Run::new();
    run.play(0..=LAST - 1);
    let running: Vec<&str> = run
        .screen
        .items
        .iter()
        .filter(|item| unfinished(item.status))
        .map(|item| item.id.0.as_str())
        .collect();
    assert!(running.is_empty(), "still running: {running:?}");
    assert!(run.screen.open.is_empty());
    run.step(LAST);
    let lead = run.lead();
    assert_eq!(
        lead.link,
        AgentLink::Down {
            reason: "Claude exited".to_owned(),
            since: Some(Timestamp(stamp(&run.records[LAST]))),
        }
    );
    assert_eq!(lead.activity, Activity::Idle);
    assert!(
        lead.title
            .starts_with("maxhq test: do these steps in order"),
        "{}",
        lead.title
    );
    assert_eq!(lead.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(lead.pid, Some(33451));
    assert_eq!(
        lead.permission,
        Some(AgentPermission {
            requested: PermissionMode::Ask,
            effective: Some(PermissionMode::Ask),
        })
    );
    assert_eq!(lead.vendor_session_id.as_deref(), Some(SESSION));
    assert_eq!(
        lead.context.map(|context| context.window_tokens),
        Some(1_000_000)
    );
    let done: Vec<_> = [3, 5, 8, 89, 114, 140, 183, 207, 218, 239]
        .into_iter()
        .map(|at| (record_key(at), ActionOutcome::Done))
        .collect();
    assert_eq!(run.screen.results, done);
    // The CLI hides thinking, so it shows nothing.
    assert!(!run
        .screen
        .items
        .iter()
        .any(|item| matches!(item.body, ItemBody::Reasoning { .. })));
}

#[test]
fn streamed_text_appends_until_the_final_message_replaces_it() {
    let mut run = Run::new();
    run.play(0..=155);
    let fx = run.step(156);
    let started = upserts(&fx, SUMMARY_TEXT);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].status, ItemStatus::Running);
    assert_eq!(started[0].turn, run.turn(FIRST));
    assert_eq!(
        started[0].body,
        ItemBody::AssistantMessage {
            text: "Steps 1\u{2013}4".to_owned(),
            streaming: true,
        }
    );
    for at in 157..=177 {
        let fx = run.step(at);
        assert!(!fx.timeline.is_empty(), "record {at} showed nothing");
        for (agent, event) in &fx.timeline {
            assert_eq!(*agent, lead());
            match event {
                TimelineEvent::Append { id, field, .. } => {
                    assert_eq!(id.0, SUMMARY_TEXT);
                    assert_eq!(*field, TextField::Text);
                }
                other => panic!("record {at} sent {other:?}"),
            }
        }
    }
    let streamed = match &run.item(SUMMARY_TEXT).body {
        ItemBody::AssistantMessage { text, .. } => text.clone(),
        body => panic!("{body:?}"),
    };
    let fx = run.step(178);
    let done = upserts(&fx, SUMMARY_TEXT);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].status, ItemStatus::Completed);
    let ItemBody::AssistantMessage { text, streaming } = &done[0].body else {
        panic!("{:?}", done[0].body);
    };
    assert!(!streaming);
    assert_eq!(*text, streamed);
    assert!(text.starts_with("Steps 1\u{2013}4 are done, but step 5 was skipped"));
}

#[test]
fn each_recorded_tool_call_is_one_item() {
    let run = Run::full();
    let read = run.item(&format!("claude:{READ_CALL}"));
    assert_eq!(
        read.body,
        ItemBody::FileRead {
            path: format!("{CWD}/seed.txt"),
            lines: None,
            images: Vec::new(),
        }
    );
    let ls = run.item(&format!("claude:{LS_CALL}"));
    let ItemBody::Command {
        command,
        exit_code,
        output,
        background,
        ..
    } = &ls.body
    else {
        panic!("{:?}", ls.body);
    };
    assert_eq!(command, "ls");
    assert_eq!(*exit_code, Some(0));
    assert_eq!(output, "seed.txt");
    assert!(!background);
    let write = change(run.item(&format!("claude:{WRITE_A_CALL}")));
    assert_eq!(write.path, format!("{CWD}/a.txt"));
    assert_eq!(write.op, FileOp::Add);
    assert_eq!((write.additions, write.deletions), (1, 0));
    assert!(write
        .patch
        .as_deref()
        .is_some_and(|patch| patch.contains("+ALPHA")));
    let edit = change(run.item(&format!("claude:{EDIT_CALL}")));
    assert_eq!(edit.path, format!("{CWD}/seed.txt"));
    assert_eq!(edit.op, FileOp::Modify);
    assert_eq!((edit.additions, edit.deletions), (1, 1));
    let patch = edit.patch.as_deref().expect("a patch");
    assert!(
        patch.contains("\n-one") && patch.contains("\n+two"),
        "{patch}"
    );
    let denied = run.item(&format!("claude:{WRITE_B_CALL}"));
    assert_eq!(denied.status, ItemStatus::Declined);
    let calls = [READ_CALL, LS_CALL, WRITE_A_CALL, EDIT_CALL, WRITE_B_CALL];
    for call in calls {
        let item = run.item(&format!("claude:{call}"));
        assert_eq!(item.turn, run.turn(FIRST), "{call}");
        assert_eq!(item.agent, lead());
    }
    for call in &calls[..4] {
        assert_eq!(
            run.item(&format!("claude:{call}")).status,
            ItemStatus::Completed
        );
    }
    let tools = run
        .screen
        .items
        .iter()
        .filter(|item| item.id.0.starts_with("claude:toolu_") && !item.id.0.ends_with(":approval"))
        .count();
    assert_eq!(tools, calls.len());
    for call in [WRITE_A_CALL, EDIT_CALL, WRITE_B_CALL] {
        let item = run.item(&format!("claude:{call}:approval"));
        assert_eq!(
            item.parent.as_ref().map(|id| id.0.as_str()),
            Some(format!("claude:{call}").as_str())
        );
        assert_eq!(item.turn, run.turn(FIRST));
    }
}

#[test]
fn a_request_opens_an_approval_and_its_answer_closes_it() {
    let mut run = Run::new();
    run.play(0..=83);
    let fx = run.step(84);
    let event = approval(WRITE_A_ASK);
    let shown = opened(&fx);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id, event);
    assert_eq!(shown[0].agent, lead());
    assert_eq!(shown[0].kind, AttentionKind::Permission);
    assert_eq!(shown[0].summary, "Write a.txt?");
    assert!(shown[0].accepts_text);
    let options: Vec<&str> = shown[0]
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect();
    assert_eq!(options, ["allow", "allow_session", "deny"]);
    assert_eq!(
        shown[0].options[1],
        AttentionOption {
            id: "allow_session".to_owned(),
            label: "Allow for this session".to_owned(),
            kind: None,
        }
    );
    assert_eq!(
        run.lead().activity,
        Activity::NeedsInput { attention: event }
    );
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Waiting);
    assert_eq!(ask.turn, run.turn(FIRST));
    let ItemBody::Approval {
        attention,
        approval_kind,
        title,
        detail,
        decided,
        ..
    } = &ask.body
    else {
        panic!("{:?}", ask.body);
    };
    assert_eq!(*attention, event);
    assert_eq!(*approval_kind, ApprovalKind::FileChange);
    assert_eq!(title, "Write a.txt?");
    assert_eq!(*detail, None);
    assert_eq!(*decided, None);
    let tool = format!("claude:{WRITE_A_CALL}");
    assert_eq!(run.screen.item(&tool).status, ItemStatus::Waiting);

    run.play(85..=88);
    let fx = run.step(89);
    assert_eq!(closed(&fx), [event]);
    assert_eq!(fx.results, [(record_key(89), ActionOutcome::Done)]);
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Completed);
    let ItemBody::Approval {
        decided: Some(decided),
        ..
    } = &ask.body
    else {
        panic!("{:?}", ask.body);
    };
    assert_eq!(decided.by, Some(phone()));
    assert_eq!(
        decided.answer,
        AttentionAnswer::Option {
            id: "allow".to_owned()
        }
    );
    assert_eq!(run.screen.item(&tool).status, ItemStatus::Running);
    assert_ne!(
        run.lead().activity,
        Activity::NeedsInput { attention: event }
    );

    run.play(90..=139);
    run.step(140);
    let ask = run.item(&format!("claude:{WRITE_B_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Declined);
    let ItemBody::Approval {
        decided: Some(decided),
        ..
    } = &ask.body
    else {
        panic!("{:?}", ask.body);
    };
    assert_eq!(
        decided.answer,
        AttentionAnswer::Text {
            text: "maxhq test: skip b.txt".to_owned()
        }
    );
    let tool = format!("claude:{WRITE_B_CALL}");
    assert_eq!(run.screen.item(&tool).status, ItemStatus::Declined);
    run.play(141..=142);
    assert_eq!(run.screen.item(&tool).status, ItemStatus::Declined);
}

#[test]
fn a_queued_message_waits_and_an_interrupt_cuts_the_turn_short() {
    let mut run = Run::new();
    run.play(0..=218);
    let queued = run.message(QUEUED);
    assert_eq!(queued.status, ItemStatus::Pending);
    assert_eq!(queued.turn, None);
    assert!(matches!(
        queued.body,
        ItemBody::UserMessage {
            delivery: Delivery::Queued,
            ..
        }
    ));
    let count = run.item(COUNT_TEXT);
    assert!(matches!(
        count.body,
        ItemBody::AssistantMessage {
            streaming: true,
            ..
        }
    ));
    run.play(219..=238);
    let fx = run.step(239);
    assert_eq!(only_write(&fx)["request"], json!({"subtype": "interrupt"}));
    assert!(fx.results.is_empty(), "an interrupt waits for its reply");
    let fx = run.step(240);
    assert_eq!(fx.results, [(record_key(239), ActionOutcome::Done)]);

    run.play(241..=246);
    let count = run.item(COUNT_TEXT);
    assert_eq!(count.status, ItemStatus::Interrupted);
    assert!(matches!(
        count.body,
        ItemBody::AssistantMessage {
            streaming: false,
            ..
        }
    ));
    let marker = run.item(MARKER);
    assert_eq!(marker.body, ItemBody::Interrupted);
    assert_eq!(marker.turn, run.turn(COUNT));
    assert!(
        run.screen
            .find(&format!("claude:{CUT_RESULT}:error"))
            .is_none(),
        "an interrupt isn't an error"
    );

    run.step(247);
    let cut = run.message(COUNT);
    assert_eq!(cut.status, ItemStatus::Completed);
    assert!(matches!(
        cut.body,
        ItemBody::UserMessage {
            delivery: Delivery::Sent,
            ..
        }
    ));
    run.step(248);
    let queued = run.message(QUEUED);
    assert_eq!(queued.status, ItemStatus::Completed);
    assert_eq!(queued.turn, run.turn(QUEUED));
    assert!(matches!(
        queued.body,
        ItemBody::UserMessage {
            delivery: Delivery::Sent,
            ..
        }
    ));
}

#[test]
fn a_mode_change_follows_the_cli_and_the_init_reply_lists_models() {
    let mut run = Run::new();
    let modes = |run: &Run| {
        let permission = run.lead().permission.expect("a mode");
        (permission.requested, permission.effective)
    };
    let fx = run.play(0..=2);
    assert_eq!(
        modes(&run),
        (PermissionMode::Ask, Some(PermissionMode::Ask))
    );
    // Each model lists the effort levels the CLI takes for it, and the one
    // it runs at when told none: what the CLI ships, with no settings.
    let efforts = || {
        ["low", "medium", "high", "xhigh", "max"]
            .map(str::to_owned)
            .to_vec()
    };
    assert_eq!(
        fx.models,
        [(
            account(),
            vec![
                ModelChoice {
                    id: "opus".to_owned(),
                    label: "Opus 5.5".to_owned(),
                    default: true,
                    efforts: efforts(),
                    default_effort: Some("medium".to_owned()),
                },
                ModelChoice {
                    id: "fable".to_owned(),
                    label: "Fable 5.1".to_owned(),
                    default: false,
                    efforts: efforts(),
                    default_effort: Some("high".to_owned()),
                },
                ModelChoice {
                    id: "sonnet".to_owned(),
                    label: "Sonnet 5.5".to_owned(),
                    default: false,
                    efforts: efforts(),
                    default_effort: Some("medium".to_owned()),
                },
            ],
        )]
    );
    run.step(3);
    assert_eq!(
        modes(&run),
        (PermissionMode::AcceptEdits, Some(PermissionMode::Ask))
    );
    run.step(4);
    assert_eq!(
        modes(&run),
        (
            PermissionMode::AcceptEdits,
            Some(PermissionMode::AcceptEdits)
        )
    );
    run.play(5..=6);
    assert_eq!(
        modes(&run),
        (PermissionMode::Ask, Some(PermissionMode::AcceptEdits))
    );
    run.play(7..=9);
    assert_eq!(
        modes(&run),
        (PermissionMode::Ask, Some(PermissionMode::Ask))
    );
    // Each change is done once the CLI confirms it, and the message at 8
    // is done once written.
    assert_eq!(
        run.screen.results,
        [
            (record_key(3), ActionOutcome::Done),
            (record_key(5), ActionOutcome::Done),
            (record_key(8), ActionOutcome::Done),
        ]
    );
}

#[test]
fn a_rate_limit_event_reports_the_accounts_windows() {
    let mut run = Run::new();
    run.play(0..=40);
    let fx = run.step(41);
    assert_eq!(fx.usage.len(), 1);
    let (owner, usage) = &fx.usage[0];
    assert_eq!(*owner, account());
    let windows: Vec<_> = usage
        .windows
        .iter()
        .map(|window| {
            (
                window.scope.clone(),
                window.window_minutes,
                window.resets_at,
            )
        })
        .collect();
    assert_eq!(
        windows,
        [
            (None, 300, Some(Timestamp(1_792_936_200_000))),
            (None, 10_080, Some(Timestamp(1_793_239_200_000))),
        ]
    );
    assert!((usage.windows[0].used_percent - 10.0).abs() < 1e-9);
    assert!((usage.windows[1].used_percent - 57.0).abs() < 1e-9);
}

#[test]
fn a_result_ends_the_turn_with_a_summary() {
    let run = Run::full();
    let summary = run.item(&format!("claude:{FIRST_RESULT}"));
    assert_eq!(summary.turn, run.turn(FIRST));
    let ItemBody::TurnSummary {
        duration_ms,
        context_tokens,
    } = summary.body
    else {
        panic!("{:?}", summary.body);
    };
    assert_eq!(duration_ms, Some(10_551));
    assert!(context_tokens.is_some_and(|tokens| tokens > 0));
}

#[test]
fn a_reconnect_replays_nothing_twice() {
    let full = Run::full();
    let mut run = Run::new();
    run.play(0..=85);
    let fx = run.replay(1..=84);
    assert_eq!(fx, Effects::default());
    run.play(86..=LAST);
    let event = approval(WRITE_A_ASK);
    assert_eq!(
        run.screen.opened.iter().filter(|id| **id == event).count(),
        1
    );
    let answers = run
        .screen
        .writes
        .iter()
        .filter(|write| write.answers.as_deref() == Some(WRITE_A_ASK))
        .count();
    assert_eq!(answers, 1);
    assert_eq!(run.normal_items(), full.normal_items());
}

#[test]
fn a_new_driver_picks_up_an_open_request_from_the_holder() {
    let full = Run::full();
    let mut before = Run::new();
    before.play(0..=84);
    // `maxd` restarts while the CLI waits on the request, and the CLI has
    // written its transcript up to the Write call.
    let mut run = Run::after_restart(&before, 12);
    let fx = run.step(85);
    assert_eq!(fx.writes.len(), 1);
    assert_eq!(
        opened(&fx).iter().map(|event| event.id).collect::<Vec<_>>(),
        [approval(WRITE_A_ASK)]
    );
    let fx = run.replay(1..=84);
    assert!(fx.writes.is_empty(), "{:?}", fx.writes);
    assert_eq!(appends(&fx), 0);
    assert!(opened(&fx).is_empty());
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(
        ask.parent.as_ref().map(|id| id.0.clone()),
        Some(format!("claude:{WRITE_A_CALL}"))
    );
    assert_eq!(ask.turn, run.turn(FIRST));
    assert_eq!(run.message(FIRST).turn, run.turn(FIRST));
    run.play(86..=LAST);
    let answers = run
        .screen
        .writes
        .iter()
        .filter(|write| write.answers.as_deref() == Some(WRITE_A_ASK))
        .count();
    assert_eq!(answers, 1);
    assert_eq!(run.normal_items(), full.normal_items());
}

#[test]
fn a_new_driver_doesnt_answer_a_request_answered_before() {
    let mut before = Run::new();
    before.play(0..=92);
    let mut run = Run::after_restart(&before, 13);
    let fx = run.connect(false, &[]);
    assert_eq!(requests(&fx), [bare_initialize(), tools_changed()]);
    let fx = run.replay(1..=92);
    assert!(fx.writes.is_empty(), "{:?}", fx.writes);
    assert!(opened(&fx).is_empty());
    run.play(93..=LAST);
    assert_eq!(
        run.screen.opened,
        [approval(EDIT_ASK), approval(WRITE_B_ASK)]
    );
    for ask in [EDIT_ASK, WRITE_B_ASK] {
        let answers = run
            .screen
            .writes
            .iter()
            .filter(|write| write.answers.as_deref() == Some(ask))
            .count();
        assert_eq!(answers, 1, "{ask}");
    }
}

#[test]
fn a_new_driver_reads_the_open_turn_again_and_shows_the_whole_message() {
    let full = Run::full();
    let mut before = Run::new();
    before.play(0..=166);
    // The turn started with the CLI's running state.
    assert_eq!(before.driver.checkpoint(), Some(seq_of(10)));
    let streamed = before.item(SUMMARY_TEXT).body.clone();
    // `maxd` restarts while the summary streams. The transcript has each
    // finished message so far, up to the summary's thinking.
    let mut run = Run::after_restart(&before, 18);
    let fx = run.connect(false, &[]);
    assert_eq!(requests(&fx), [bare_initialize(), tools_changed()]);
    let fx = run.replay(10..=166);
    assert!(fx.writes.is_empty(), "{:?}", fx.writes);
    assert!(opened(&fx).is_empty());
    let summary = run.item(SUMMARY_TEXT);
    assert_eq!(summary.body, streamed);
    assert_eq!(summary.status, ItemStatus::Running);
    assert_eq!(summary.turn, run.turn(FIRST));
    // The summary went whole once, and each later piece as an append.
    assert_eq!(upserts(&fx, SUMMARY_TEXT).len(), 1);
    assert_eq!(appends(&fx), 166 - 156);
    assert_eq!(run.driver.checkpoint(), Some(seq_of(10)));
    run.play(167..=LAST);
    assert_eq!(as_read(&run), as_read(&full));
    assert_eq!(run.screen.items.len(), full.screen.items.len());
    assert_eq!(run.driver.checkpoint(), None);
}

#[test]
fn what_comes_while_the_transcript_is_read_waits_for_it_in_order() {
    let full = Run::full();
    let mut before = Run::new();
    before.play(0..=166);
    let streamed = before.item(SUMMARY_TEXT).body.clone();
    let mut run = Run::after_restart(&before, 18);
    run.connect(false, &[]);
    run.hold_jobs();
    let fx = run.replay(10..=166);
    // Nothing goes out before the read is back, and a restart now would
    // read every line again.
    assert!(fx.timeline.is_empty(), "{:?}", fx.timeline);
    assert!(fx.history.is_empty());
    assert_eq!(run.driver.checkpoint(), Some(seq_of(10)));
    let later = Action::Send {
        key: key("later"),
        agent: lead(),
        text: "And then?".to_owned(),
        refs: Vec::new(),
        attachments: Vec::new(),
        mode: SendMode::Queue,
    };
    let fx = run.act(later);
    assert!(fx.results.is_empty(), "{:?}", fx.results);
    let fx = run.release_jobs();
    // The message goes to the CLI's queue once the history is in.
    assert_eq!(only_write(&fx)["priority"], "later");
    assert_eq!(fx.history.len(), 1);
    let summary = run.item(SUMMARY_TEXT);
    assert_eq!(summary.body, streamed);
    assert_eq!(summary.status, ItemStatus::Running);
    assert_eq!(upserts(&fx, SUMMARY_TEXT).len(), 1);
    assert_eq!(appends(&fx), 166 - 156);
    // The message came last, so it waits behind the running turn.
    assert_eq!(fx.results.len(), 1);
    assert_eq!(fx.results[0].0, key("later"));
    let queued = run.screen.items.last().expect("the message");
    assert!(
        matches!(
            &queued.body,
            ItemBody::UserMessage { text, delivery: Delivery::Queued, .. } if text == "And then?"
        ),
        "{queued:?}"
    );
    assert_eq!(run.driver.checkpoint(), Some(seq_of(10)));
    // Without the message, the rest plays as it did live.
    let mut run = Run::after_restart(&before, 18);
    run.connect(false, &[]);
    run.hold_jobs();
    run.replay(10..=166);
    run.release_jobs();
    run.play(167..=LAST);
    assert_eq!(as_read(&run), as_read(&full));
    assert_eq!(run.driver.checkpoint(), None);
}

#[test]
fn a_new_driver_ends_a_turn_that_ended_while_maxd_was_down() {
    let full = Run::full();
    let mut before = Run::new();
    before.play(0..=166);
    // The turn ended before `maxd` came back, so the transcript has the
    // summary too.
    let mut run = Run::after_restart(&before, 19);
    run.connect(false, &[]);
    let fx = run.replay(10..=182);
    assert!(fx.writes.is_empty(), "{:?}", fx.writes);
    assert!(opened(&fx).is_empty());
    let summary = run.item(SUMMARY_TEXT);
    assert_eq!(summary.status, ItemStatus::Completed);
    assert_eq!(summary.body, full.item(SUMMARY_TEXT).body);
    assert_eq!(upserts(&fx, &format!("claude:{FIRST_RESULT}")).len(), 1);
    assert_eq!(run.driver.checkpoint(), None);
    run.play(183..=LAST);
    assert_eq!(as_read(&run), as_read(&full));
    assert_eq!(run.screen.items.len(), full.screen.items.len());
}

/// An hour, in milliseconds.
const HOUR: u64 = 60 * 60 * 1000;

/// A new driver for `before`'s session an hour later, handed the lead as
/// `maxd` stored it, with the CLI's transcript up to `records` lines.
fn met_again(before: &Run, records: usize) -> (Run, Agent) {
    let known = before.lead().clone();
    before.clock.fetch_add(HOUR, Ordering::SeqCst);
    let mut run = Run::after_restart(before, records);
    run.driver.met_again(&known);
    (run, known)
}

#[test]
fn an_agent_read_before_a_restart_keeps_its_times() {
    let mut before = Run::new();
    before.play(0..=185);
    // `maxd` restarts an hour after the turn ended, and the CLI waits.
    let (mut run, known) = met_again(&before, 19);
    let fx = run.connect(false, &[]);
    let init: Value = serde_json::from_str(&fx.writes[0].line).expect("JSON");
    run.reply(&init, json!({"session_state": "idle"}));
    let lead = run.lead();
    assert_eq!(lead.activity, Activity::Idle);
    assert_eq!(lead.started_at, known.started_at);
    assert_eq!(lead.last_activity_at, known.last_activity_at);
    // Hooks and status lines from the CLI aren't activity either.
    run.feed(system(
        "hook_started",
        json!({"hook_name": "SessionStart:resume"}),
    ));
    run.feed(system(
        "hook_response",
        json!({"hook_name": "SessionStart:resume"}),
    ));
    run.feed(system(
        "status",
        json!({"status": null, "permissionMode": "default"}),
    ));
    assert_eq!(run.lead().last_activity_at, known.last_activity_at);
}

#[test]
fn replayed_lines_keep_the_times_they_had() {
    let mut before = Run::new();
    before.play(0..=166);
    let (mut run, known) = met_again(&before, 18);
    run.connect(false, &[]);
    run.replay(10..=166);
    let lead = run.lead();
    assert_eq!(lead.activity, Activity::Busy);
    assert_eq!(lead.started_at, known.started_at);
    assert_eq!(lead.last_activity_at, known.last_activity_at);
}

#[test]
fn a_turn_that_ended_while_maxd_was_down_is_new_activity() {
    let mut before = Run::new();
    before.play(0..=166);
    let (mut run, known) = met_again(&before, 19);
    run.connect(false, &[]);
    run.replay(10..=185);
    let lead = run.lead();
    assert!(
        matches!(
            lead.activity,
            Activity::Idle | Activity::IdleCacheWarm { .. }
        ),
        "{:?}",
        lead.activity
    );
    assert_eq!(lead.started_at, known.started_at);
    // The turn ended after the agent was last seen, so it reads as new.
    assert_eq!(lead.last_activity_at, Timestamp(stamp(&run.records[185])));
    assert!(lead.last_activity_at > known.last_activity_at);
}

#[test]
fn a_sub_agent_read_again_keeps_its_times() {
    let mut before = Run::new();
    before.play(0..=14);
    let input = json!({"description": "Look around", "prompt": "Find it", "subagent_type": "general-purpose"});
    let started = json!({"task_id": "a1b2c3", "tool_use_id": "toolu_agent",
        "description": "Look around", "task_type": "local_agent"});
    let found = line(json!({"type": "assistant", "message": {"id": "msg_sub",
        "role": "assistant", "content": [{"type": "text", "text": "Looking"}]}}));
    // A second apart, so each time is its own.
    let mut lines = Vec::new();
    for value in [
        tool_use("toolu_agent", "Agent", input),
        system("task_started", started),
        under(found, "toolu_agent"),
    ] {
        before.seq += 1;
        let at = before.clock.fetch_add(1000, Ordering::SeqCst) + 1000;
        let line = HeldLine {
            seq: before.seq,
            at: Timestamp(at),
            text: value.to_string(),
            replayed: false,
        };
        before.held(line.clone());
        lines.push(HeldLine {
            replayed: true,
            ..line
        });
    }
    let sub = names::subagent(lead(), "a1b2c3");
    let was = before.screen.agent(sub).clone();
    assert!(was.last_activity_at > was.started_at);
    let (mut run, _) = met_again(&before, 12);
    run.connect(false, &[]);
    run.replay(10..=14);
    for line in lines {
        run.held(line);
    }
    let now = run.screen.agent(sub);
    assert_eq!(now.started_at, was.started_at);
    assert_eq!(now.last_activity_at, was.last_activity_at);
}

#[test]
fn an_idle_cli_that_stops_isnt_activity() {
    // The first turn's result, before the next message goes out.
    let mut run = Run::new();
    run.play(0..=182);
    let known = run.lead().clone();
    assert!(
        matches!(known.activity, Activity::IdleCacheWarm { .. }),
        "{:?}",
        known.activity
    );
    run.clock.fetch_add(HOUR, Ordering::SeqCst);
    run.act(Action::Stop {
        key: key("stop"),
        agent: lead(),
    });
    run.exit(Some(0), None, "");
    assert_eq!(run.lead().last_activity_at, known.last_activity_at);
    // One that dies with an error is news.
    let mut run = Run::new();
    run.play(0..=182);
    run.clock.fetch_add(HOUR, Ordering::SeqCst);
    run.exit(Some(1), None, "boom");
    assert!(run.lead().last_activity_at > known.last_activity_at);
}

#[test]
fn the_checkpoint_follows_each_turn() {
    let mut run = Run::new();
    run.play(0..=9);
    assert_eq!(run.driver.checkpoint(), None);
    run.step(10);
    assert_eq!(run.driver.checkpoint(), Some(seq_of(10)));
    run.play(11..=181);
    assert_eq!(run.driver.checkpoint(), Some(seq_of(10)));
    run.step(182);
    assert_eq!(run.driver.checkpoint(), None);
    run.play(183..=186);
    assert_eq!(run.driver.checkpoint(), Some(seq_of(186)));
    // A queued message starts the next turn as the interrupted one ends.
    run.play(187..=246);
    assert_eq!(run.driver.checkpoint(), None);
    run.play(247..=248);
    assert_eq!(run.driver.checkpoint(), Some(seq_of(248)));
    run.play(249..=LAST);
    assert_eq!(run.driver.checkpoint(), None);
}

/// An item without what a transcript can't hold: a prompt's attachments,
/// and an approval's request, the options it offered and who answered it.
fn without_attachments(mut item: Value) -> Value {
    if item["kind"] == "user_message" {
        item["attachments"] = json!([]);
    }
    if item["kind"] == "approval" {
        item["attention"] = Value::Null;
        item["options"] = json!([]);
        if item["decided"].is_object() {
            item["decided"]["by"] = Value::Null;
        }
    }
    item
}

/// A run's items, each without what a transcript can't hold. Approvals
/// answered before a restart come back from the transcript.
fn as_read(run: &Run) -> BTreeMap<String, Value> {
    run.normal_items()
        .into_iter()
        .map(|(id, item)| (id, without_attachments(item)))
        .collect()
}

#[test]
fn history_reads_the_transcript_into_the_live_streams_items() {
    let full = Run::full();
    let config = TempDir::new();
    let folder = config.path().join("projects").join(PROJECT);
    fs::create_dir_all(&folder).expect("a projects folder");
    fs::write(
        folder.join(format!("{SESSION}.jsonl")),
        full.ours(TRANSCRIPT),
    )
    .expect("a transcript");
    let mut history = History::find(config.path(), SESSION, lead()).expect("the transcript");
    let items = history.read().expect("a read");
    let read: BTreeMap<String, Value> = items
        .iter()
        .map(|item| {
            (
                full.recorded(&item.id.0),
                without_attachments(full.normal(item)),
            )
        })
        .collect();
    // A transcript written with `-p` records no turn's end.
    let live: BTreeMap<String, Value> = full
        .normal_items()
        .into_iter()
        .filter(|(_, item)| item["kind"] != "turn_summary")
        .map(|(id, item)| (id, without_attachments(item)))
        .collect();
    assert_eq!(
        read.keys().collect::<Vec<_>>(),
        live.keys().collect::<Vec<_>>()
    );
    assert_eq!(read, live);
    assert!(history.read().expect("a read").is_empty());
}

#[test]
fn history_keeps_the_pictures_a_tool_result_holds() {
    use base64::Engine as _;
    let config = TempDir::new();
    let folder = config.path().join("projects").join(PROJECT);
    fs::create_dir_all(&folder).expect("a projects folder");
    let bytes = fs::read(IMAGE).expect("the fixture image");
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let at = "2026-10-25T09:48:42.000Z";
    let lines = [
        json!({"type": "user", "uuid": "u1", "timestamp": at,
            "message": {"role": "user", "content": "Look at the picture"}}),
        json!({"type": "assistant", "uuid": "a1", "timestamp": at,
            "message": {"id": "msg_read", "role": "assistant", "content": [{
                "type": "tool_use", "id": "toolu_png", "name": "Read",
                "input": {"file_path": format!("{CWD}/red.png")}}]}}),
        json!({"type": "user", "uuid": "u2", "timestamp": at,
        "message": {"role": "user", "content": [{
            "type": "tool_result", "tool_use_id": "toolu_png", "content": [{
                "type": "image",
                "source": {"type": "base64", "media_type": "image/png", "data": data},
            }]}]}}),
    ];
    let jsonl: String = lines.iter().map(|line| format!("{line}\n")).collect();
    fs::write(folder.join(format!("{SESSION}.jsonl")), jsonl).expect("a transcript");
    let images = |history: &mut History| {
        let items = history.read().expect("a read");
        let item = items
            .iter()
            .find(|item| item.id.0 == "claude:toolu_png")
            .expect("the read");
        match &item.body {
            ItemBody::FileRead { images, .. } => images.clone(),
            body => panic!("not a read: {body:?}"),
        }
    };

    // Read as a container's guest reads it, the picture lands in the
    // attachments folder.
    let root = config.path().join("attachments");
    let mut history = History::find(config.path(), SESSION, lead())
        .expect("the transcript")
        .keeping_pictures_in(root.clone());
    let kept = images(&mut history);
    assert_eq!(kept.len(), 1);
    let path = root
        .join(lead().to_string())
        .join("tool-output")
        .join(format!(
            "{}.png",
            maxhq_transfer::receive::sha256_hex(&bytes)
        ));
    assert_eq!(kept[0].path, path.to_string_lossy());
    assert_eq!(fs::read(&path).expect("the kept picture"), bytes);

    // Without a folder to keep them in, the read leaves them out.
    let mut history = History::find(config.path(), SESSION, lead()).expect("the transcript");
    assert!(images(&mut history).is_empty());
}

#[test]
fn a_driver_that_meets_a_session_under_way_reads_its_sub_agents_too() {
    let config = TempDir::new();
    let folder = config.path().join("projects").join(PROJECT);
    let subagents = folder.join(SESSION).join("subagents");
    fs::create_dir_all(&subagents).expect("a sub-agents folder");
    let at = "2026-10-25T09:48:42.000Z";
    let lead_lines = [
        json!({"type": "user", "uuid": "u1", "timestamp": at,
            "message": {"role": "user", "content": "Ask a helper"}}),
        json!({"type": "assistant", "uuid": "a1", "timestamp": at,
            "message": {"id": "msg_lead", "role": "assistant", "content": [{
                "type": "tool_use", "id": "toolu_agent", "name": "Agent",
                "input": {"description": "Look around", "prompt": "Find it"}}]}}),
    ];
    let sub_lines = [
        json!({"type": "user", "uuid": "s0", "timestamp": at, "isSidechain": true,
            "agentId": "a7", "message": {"role": "user", "content": "Find it"}}),
        json!({"type": "assistant", "uuid": "s1", "timestamp": at, "isSidechain": true,
            "agentId": "a7", "message": {"id": "msg_sub", "role": "assistant",
                "content": [{"type": "text", "text": "Looking"}]}}),
    ];
    let jsonl = |lines: &[Value]| {
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>()
    };
    fs::write(folder.join(format!("{SESSION}.jsonl")), jsonl(&lead_lines)).expect("a transcript");
    fs::write(subagents.join("agent-a7.jsonl"), jsonl(&sub_lines)).expect("a transcript");
    let meta = json!({"agentType": "general-purpose", "toolUseId": "toolu_agent"});
    fs::write(subagents.join("agent-a7.meta.json"), meta.to_string()).expect("a meta file");
    let records = records();
    let now = stamp(&records[0]);
    let mut run = Run::with(config, Vec::new(), now, records);
    let fx = run.feed(system("status", json!({"status": null})));
    let seeded: Vec<&str> = fx
        .history
        .iter()
        .flat_map(|(agent, items)| {
            assert_eq!(*agent, lead());
            items.iter().map(|item| item.id.0.as_str())
        })
        .collect();
    assert_eq!(
        seeded,
        ["claude:u1", "claude:toolu_agent", "claude:msg_sub:0"]
    );
    let sub = run.screen.item("claude:msg_sub:0");
    assert_eq!(
        sub.parent.as_ref().map(|id| id.0.as_str()),
        Some("claude:toolu_agent")
    );

    // Its start, read again, keeps it nested where its items are.
    let started = json!({"task_id": "a7", "tool_use_id": "toolu_agent",
        "description": "Look around", "task_type": "local_agent"});
    let fx = run.feed_with(system("task_started", started), true);
    assert!(fx.agents.iter().all(|agent| agent.id == lead()));
    let found = line(json!({"type": "assistant", "message": {"id": "msg_sub2",
        "role": "assistant", "content": [{"type": "text", "text": "Found it"}]}}));
    run.feed(under(found, "toolu_agent"));
    let found = run.screen.item("claude:msg_sub2:0");
    assert_eq!(found.agent, lead());
    assert_eq!(
        found.parent.as_ref().map(|id| id.0.as_str()),
        Some("claude:toolu_agent")
    );
}

#[test]
fn a_session_continues_only_while_no_cli_runs_it() {
    let config = TempDir::new();
    assert!(can_continue(config.path(), SESSION));
    let sessions = config.path().join("sessions");
    fs::create_dir_all(&sessions).expect("a sessions folder");
    let running = json!({"pid": std::process::id(), "sessionId": SESSION, "cwd": CWD});
    fs::write(sessions.join("1.json"), running.to_string()).expect("a session file");
    assert!(!can_continue(config.path(), SESSION));
    assert!(can_continue(
        config.path(),
        "0b6f6c55-8f5e-4c1a-9d43-3f7e6a1c2d10"
    ));
}
