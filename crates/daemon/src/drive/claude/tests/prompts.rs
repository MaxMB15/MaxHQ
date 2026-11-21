// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The CLI's requests: permission prompts and their answers, questions,
//! plans, and the requests `maxd` turns down.

use maxhq_protocol::attention::{AttentionAnswer, AttentionKind};
use maxhq_protocol::message::{ActionOutcome, SendMode};
use maxhq_protocol::model::{Activity, PermissionMode};
use maxhq_protocol::timeline::{
    ApprovalKind, Delivery, ItemBody, ItemStatus, NoticeLevel, QuestionAnswer,
};
use serde_json::{json, Value};

use super::*;
use crate::drive::claude::tools::DISMISSED;

/// The input of the recorded request to write a.txt.
fn write_a_input() -> Value {
    json!({"file_path": format!("{CWD}/a.txt"), "content": "ALPHA\n"})
}

/// A run waiting on the recorded request to write a.txt.
fn asked() -> Run {
    let mut run = Run::new();
    run.play(0..=84);
    run
}

/// A run in the middle of its first turn, before any tool call.
fn mid_turn() -> Run {
    let mut run = Run::new();
    run.play(0..=14);
    run
}

/// The option IDs an event offers.
fn option_ids(event: &AttentionEvent) -> Vec<&str> {
    event
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect()
}

/// The decision an approval item shows.
fn decision(item: &TimelineItem) -> Option<&maxhq_protocol::timeline::Decided> {
    match &item.body {
        ItemBody::Approval { decided, .. } => decided.as_ref(),
        body => panic!("not an approval: {body:?}"),
    }
}

#[test]
fn allow_for_the_session_keeps_the_rule_for_the_session_only() {
    let mut run = asked();
    let fx = run.pick("session", approval(WRITE_A_ASK), "allow_session");
    assert_eq!(only_write(&fx)["response"]["request_id"], WRITE_A_ASK);
    assert_eq!(fx.writes[0].answers.as_deref(), Some(WRITE_A_ASK));
    assert_eq!(
        answer_body(&fx),
        json!({
            "behavior": "allow",
            "updatedInput": write_a_input(),
            "updatedPermissions": [
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            ],
            "decisionClassification": "user_permanent",
            "toolUseID": WRITE_A_CALL,
        })
    );
    assert_eq!(fx.results, [(key("session"), ActionOutcome::Done)]);
    assert_eq!(closed(&fx), [approval(WRITE_A_ASK)]);
}

/// Picks an option from a phone whose switch is off.
fn pick_held_back(run: &mut Run, name: &str, event: AttentionId, option: &str) -> Effects {
    run.act(Action::Answer {
        key: key(name),
        event,
        answer: AttentionAnswer::Option {
            id: option.to_owned(),
        },
        by: phone(),
        held_back: true,
    })
}

/// A run asking to run `rm -rf build`, with `suggestions` to keep.
fn asked_with(suggestions: Value) -> Run {
    let mut run = mid_turn();
    let input = json!({"command": "rm -rf build", "description": "Remove the build"});
    run.feed(tool_use("toolu_rm", "Bash", input.clone()));
    run.feed(can_use_tool(
        "req-rm",
        "toolu_rm",
        "Bash",
        input,
        suggestions,
    ));
    run
}

/// The rule Claude suggests for `rm -rf build`.
fn rm_rule(destination: &str) -> Value {
    json!({
        "type": "addRules",
        "rules": [{"toolName": "Bash", "ruleContent": "rm -rf build"}],
        "behavior": "allow",
        "destination": destination,
    })
}

#[test]
fn a_held_back_phone_keeps_the_rules_but_no_mode_past_the_switch() {
    let suggestions = json!([
        rm_rule("localSettings"),
        {"type": "setMode", "mode": "bypassPermissions", "destination": "session"},
        {"type": "setMode", "mode": "auto", "destination": "session"},
        {"type": "setMode", "mode": "aModeFromLater", "destination": "session"},
        {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
        {"type": "setMode", "mode": "plan", "destination": "session"},
        {"type": "setMode", "mode": "default", "destination": "session"},
    ]);
    let mut run = asked_with(suggestions.clone());
    let fx = pick_held_back(&mut run, "session", approval("req-rm"), "allow_session");
    assert_eq!(
        answer_body(&fx)["updatedPermissions"],
        json!([
            rm_rule("session"),
            {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            {"type": "setMode", "mode": "plan", "destination": "session"},
            {"type": "setMode", "mode": "default", "destination": "session"},
        ])
    );
    assert_eq!(fx.results, [(key("session"), ActionOutcome::Done)]);
    // The Mac's answer keeps every rule.
    let mut run = asked_with(suggestions);
    let fx = run.pick("session", approval("req-rm"), "allow_session");
    let kept = answer_body(&fx)["updatedPermissions"].clone();
    assert_eq!(kept.as_array().map(Vec::len), Some(7));
    assert_eq!(
        kept[1],
        json!({"type": "setMode", "mode": "bypassPermissions", "destination": "session"})
    );
    assert_eq!(fx.results, [(key("session"), ActionOutcome::Done)]);
}

#[test]
fn a_held_back_phone_cant_keep_only_a_mode_past_the_switch() {
    let suggestions =
        json!([{"type": "setMode", "mode": "bypassPermissions", "destination": "session"}]);
    let mut run = asked_with(suggestions);
    let event = approval("req-rm");
    let fx = pick_held_back(&mut run, "session", event, "allow_session");
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(key("session"), refused(crate::phones::PHONE_CANT))]
    );
    // The request stays open, and the Mac can still keep the mode.
    let fx = run.pick("mac", event, "allow_session");
    assert_eq!(
        answer_body(&fx)["updatedPermissions"],
        json!([{"type": "setMode", "mode": "bypassPermissions", "destination": "session"}])
    );
    assert_eq!(fx.results, [(key("mac"), ActionOutcome::Done)]);
}

#[test]
fn deny_tells_the_model_and_declines_the_call() {
    let mut run = asked();
    let fx = run.pick("deny", approval(WRITE_A_ASK), "deny");
    assert_eq!(
        answer_body(&fx),
        json!({
            "behavior": "deny",
            "message": "The user denied this request.",
            "interrupt": false,
            "decisionClassification": "user_reject",
            "toolUseID": WRITE_A_CALL,
        })
    );
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Declined);
    assert_eq!(
        decision(ask).map(|decided| decided.answer.clone()),
        Some(AttentionAnswer::Option {
            id: "deny".to_owned()
        })
    );
    let tool = run.item(&format!("claude:{WRITE_A_CALL}"));
    assert_eq!(tool.status, ItemStatus::Declined);
}

#[test]
fn a_wrong_answer_is_refused_and_leaves_the_request_open() {
    let mut run = asked();
    let event = approval(WRITE_A_ASK);
    let answers = AttentionAnswer::Questions {
        answers: Vec::new(),
    };
    let fx = run.answer("answers", event, answers);
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(
            key("answers"),
            refused("This request takes an option, not answers.")
        )]
    );
    let fx = run.pick("maybe", event, "maybe");
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(key("maybe"), refused("This request has no option maybe."))]
    );
    assert!(run.screen.open.contains_key(&event));
    let fx = run.pick("allow", event, "allow");
    assert_eq!(answer_body(&fx)["behavior"], "allow");
    let fx = run.pick("again", event, "allow");
    assert!(fx.writes.is_empty(), "a request is answered once");
    assert_eq!(
        fx.results,
        [(key("again"), refused("That request is no longer open."))]
    );
}

#[test]
fn a_request_with_no_rule_to_keep_offers_no_session_option() {
    let mut run = mid_turn();
    let input = json!({"command": "rm -rf build", "description": "Remove the build"});
    run.feed(tool_use("toolu_rm", "Bash", input.clone()));
    let fx = run.feed(can_use_tool(
        "req-rm",
        "toolu_rm",
        "Bash",
        input.clone(),
        json!([]),
    ));
    let shown = opened(&fx);
    assert_eq!(shown.len(), 1);
    assert_eq!(option_ids(&shown[0]), ["allow", "deny"]);
    assert_eq!(shown[0].summary, "Run `rm -rf build`?");
    let ask = run.screen.item("claude:toolu_rm:approval");
    let ItemBody::Approval {
        approval_kind,
        detail,
        cwd,
        ..
    } = &ask.body
    else {
        panic!("{:?}", ask.body);
    };
    assert_eq!(*approval_kind, ApprovalKind::Command);
    assert_eq!(detail.as_deref(), Some("rm -rf build"));
    // Bash names no folder, so it's the session's.
    assert_eq!(cwd.as_deref(), Some(CWD));
    assert_eq!(
        ask.parent.as_ref().map(|id| id.0.as_str()),
        Some("claude:toolu_rm")
    );
    assert_eq!(
        run.screen.item("claude:toolu_rm").status,
        ItemStatus::Waiting
    );
    let event = approval("req-rm");
    let fx = run.pick("session", event, "allow_session");
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(
            key("session"),
            refused("This request has no rule to keep for the session.")
        )]
    );
    let text = AttentionAnswer::Text {
        text: "Keep the build".to_owned(),
    };
    let fx = run.answer("say", event, text);
    assert_eq!(
        answer_body(&fx),
        json!({
            "behavior": "deny",
            "message": "Keep the build",
            "interrupt": false,
            "decisionClassification": "user_reject",
            "toolUseID": "toolu_rm",
        })
    );
    assert_eq!(
        run.screen.item("claude:toolu_rm:approval").status,
        ItemStatus::Declined
    );
}

#[test]
fn a_tool_that_names_its_folder_asks_for_that_folder() {
    let mut run = mid_turn();
    let input = json!({"command": "make", "cwd": "/srv/build"});
    run.feed(tool_use("toolu_make", "mcp__shell__run", input.clone()));
    run.feed(can_use_tool(
        "req-make",
        "toolu_make",
        "mcp__shell__run",
        input,
        json!([]),
    ));
    let ask = run.screen.item("claude:toolu_make:approval");
    let ItemBody::Approval { cwd, .. } = &ask.body else {
        panic!("{:?}", ask.body);
    };
    assert_eq!(cwd.as_deref(), Some("/srv/build"));
}

#[test]
fn a_withdrawn_request_closes_as_decided_elsewhere() {
    let mut run = asked();
    let event = approval(WRITE_A_ASK);
    let fx = run.feed(json!({"type": "control_cancel_request", "request_id": WRITE_A_ASK}));
    assert!(fx.writes.is_empty());
    assert_eq!(closed(&fx), [event]);
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Cancelled);
    assert_eq!(decision(ask), None);
    let tool = run.item(&format!("claude:{WRITE_A_CALL}"));
    assert_eq!(tool.status, ItemStatus::Running);
    assert_ne!(
        run.lead().activity,
        Activity::NeedsInput { attention: event }
    );
    let fx = run.pick("late", event, "allow");
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(key("late"), refused("That request is no longer open."))]
    );
}

#[test]
fn a_call_that_goes_ahead_without_maxd_closes_its_prompt() {
    let mut run = Run::new();
    run.play(0..=88);
    // Someone answered in the terminal, so the result arrives with no
    // answer from `maxd`.
    let fx = run.play(90..=92);
    assert!(fx.writes.is_empty());
    assert_eq!(closed(&fx), [approval(WRITE_A_ASK)]);
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Completed);
    let decided = decision(ask).expect("a decision");
    assert_eq!(decided.by, None);
    assert_eq!(
        decided.answer,
        AttentionAnswer::Option {
            id: "allow".to_owned()
        }
    );
    let tool = run.item(&format!("claude:{WRITE_A_CALL}"));
    assert_eq!(tool.status, ItemStatus::Completed);
}

/// One question with two choices.
fn one_question() -> Value {
    json!({"questions": [{
        "question": "Which file?",
        "header": "File",
        "options": [{"label": "a.txt", "description": "The first one"}, {"label": "b.txt"}],
        "multiSelect": false,
    }]})
}

/// Asks [`one_question`] as the call `toolu_ask`, and gives what that
/// opened.
fn ask_one(run: &mut Run) -> Effects {
    let input = one_question();
    run.feed(tool_use("toolu_ask", "AskUserQuestion", input.clone()));
    run.feed(can_use_tool(
        "req-ask",
        "toolu_ask",
        "AskUserQuestion",
        input,
        json!([]),
    ))
}

#[test]
fn a_question_opens_with_its_choices_and_takes_one() {
    let mut run = mid_turn();
    let input = one_question();
    let fx = ask_one(&mut run);
    let event = AttentionId::derive("claude:toolu_ask");
    let shown = opened(&fx);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id, event);
    assert_eq!(shown[0].kind, AttentionKind::Question);
    assert_eq!(shown[0].summary, "Which file?");
    assert_eq!(option_ids(&shown[0]), ["a.txt", "b.txt"]);
    assert!(shown[0].accepts_text);
    assert!(
        run.screen.find("claude:req-ask").is_none(),
        "a question needs no approval"
    );
    let item = run.screen.item("claude:toolu_ask");
    assert_eq!(item.status, ItemStatus::Waiting);
    let ItemBody::Question {
        attention,
        questions,
        answers,
    } = &item.body
    else {
        panic!("{:?}", item.body);
    };
    assert_eq!(*attention, event);
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].header.as_deref(), Some("File"));
    assert_eq!(questions[0].options.len(), 2);
    assert_eq!(*answers, None);
    assert_eq!(
        run.lead().activity,
        Activity::NeedsInput { attention: event }
    );

    let fx = run.pick("b", event, "b.txt");
    let mut updated = input.clone();
    updated["answers"] = json!({"Which file?": "b.txt"});
    assert_eq!(
        answer_body(&fx),
        json!({"behavior": "allow", "updatedInput": updated, "toolUseID": "toolu_ask"})
    );
    assert_eq!(fx.writes[0].answers.as_deref(), Some("req-ask"));
    assert_eq!(closed(&fx), [event]);
    let item = run.screen.item("claude:toolu_ask");
    assert_eq!(item.status, ItemStatus::Running);
    let given = vec![QuestionAnswer {
        id: "q1".to_owned(),
        values: vec!["b.txt".to_owned()],
    }];
    assert!(
        matches!(&item.body, ItemBody::Question { answers: Some(shown), .. } if *shown == given)
    );

    let result = json!({"questions": input["questions"], "answers": {"Which file?": "b.txt"}});
    run.feed(tool_result(
        "toolu_ask",
        "User has answered your questions.",
        result,
    ));
    let item = run.screen.item("claude:toolu_ask");
    assert_eq!(item.status, ItemStatus::Completed);
    assert!(
        matches!(&item.body, ItemBody::Question { answers: Some(shown), .. } if *shown == given)
    );
}

#[test]
fn two_questions_take_an_answer_each() {
    let mut run = mid_turn();
    let input = json!({"questions": [
        {"question": "Which colors?", "options": [{"label": "Red"}, {"label": "Blue"}], "multiSelect": true},
        {"question": "Which size?", "options": [{"label": "S"}, {"label": "M"}]},
    ]});
    run.feed(tool_use("toolu_two", "AskUserQuestion", input.clone()));
    let request = can_use_tool(
        "req-two",
        "toolu_two",
        "AskUserQuestion",
        input.clone(),
        json!([]),
    );
    let fx = run.feed(request);
    let event = AttentionId::derive("claude:toolu_two");
    let shown = opened(&fx);
    assert_eq!(shown[0].summary, "Which colors?");
    assert!(
        shown[0].options.is_empty(),
        "two questions don't fit on buttons"
    );

    let fx = run.pick("one", event, "Red");
    assert!(fx.writes.is_empty());
    assert_eq!(fx.results, [(key("one"), refused("Answer each question."))]);
    let blank = AttentionAnswer::Questions {
        answers: vec![QuestionAnswer {
            id: "q1".to_owned(),
            values: Vec::new(),
        }],
    };
    let fx = run.answer("blank", event, blank);
    assert_eq!(
        fx.results,
        [(key("blank"), refused("Answer at least one question."))]
    );

    let both = AttentionAnswer::Questions {
        answers: vec![
            QuestionAnswer {
                id: "q1".to_owned(),
                values: vec!["Red".to_owned(), "Blue".to_owned()],
            },
            QuestionAnswer {
                id: "q2".to_owned(),
                values: vec!["M".to_owned()],
            },
        ],
    };
    let fx = run.answer("both", event, both);
    let body = answer_body(&fx);
    assert_eq!(
        body["updatedInput"]["answers"],
        json!({"Which colors?": "Red, Blue", "Which size?": "M"})
    );
    assert_eq!(body["updatedInput"]["questions"], input["questions"]);
    assert_eq!(fx.results, [(key("both"), ActionOutcome::Done)]);
}

/// A run with a finished plan waiting on you.
fn planned() -> (Run, Value) {
    let mut run = mid_turn();
    let input = json!({"plan": "1. Write a.txt\n2. Run the tests"});
    run.feed(tool_use("toolu_plan", "ExitPlanMode", input.clone()));
    run.feed(can_use_tool(
        "req-plan",
        "toolu_plan",
        "ExitPlanMode",
        input.clone(),
        json!([]),
    ));
    (run, input)
}

#[test]
fn a_finished_plan_asks_ready_to_code() {
    let (run, _) = planned();
    let plan = run.screen.item("claude:toolu_plan");
    assert!(matches!(
        &plan.body,
        ItemBody::Plan { markdown, .. } if markdown == "1. Write a.txt\n2. Run the tests"
    ));
    let event = &run.screen.open[&approval("req-plan")];
    assert_eq!(event.kind, AttentionKind::Permission);
    assert_eq!(event.summary, "Ready to code?");
    assert_eq!(option_ids(event), ["accept_edits", "accept", "deny"]);
    let ask = run.screen.item("claude:toolu_plan:approval");
    assert_eq!(
        ask.parent.as_ref().map(|id| id.0.as_str()),
        Some("claude:toolu_plan")
    );
    let ItemBody::Approval {
        approval_kind,
        title,
        detail,
        options,
        ..
    } = &ask.body
    else {
        panic!("{:?}", ask.body);
    };
    assert_eq!(*approval_kind, ApprovalKind::Plan);
    assert_eq!(title, "Ready to code?");
    assert_eq!(*detail, None);
    assert_eq!(*options, event.options);
}

#[test]
fn starting_on_a_plan_switches_the_sessions_mode() {
    let (mut run, input) = planned();
    let fx = run.pick("go", approval("req-plan"), "accept_edits");
    assert_eq!(
        answer_body(&fx),
        json!({
            "behavior": "allow",
            "updatedInput": input,
            "updatedPermissions": [
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"},
            ],
            "toolUseID": "toolu_plan",
        })
    );
    let permission = run.lead().permission.expect("a mode");
    assert_eq!(permission.requested, PermissionMode::AcceptEdits);
    assert_eq!(
        run.screen.item("claude:toolu_plan:approval").status,
        ItemStatus::Completed
    );
    let fx = run.feed(system("status", json!({"permissionMode": "acceptEdits"})));
    assert!(fx.writes.is_empty());
    let permission = run.lead().permission.expect("a mode");
    assert_eq!(permission.effective, Some(PermissionMode::AcceptEdits));
}

#[test]
fn keeping_on_planning_stops_the_turn() {
    let (mut run, _) = planned();
    let event = approval("req-plan");
    let fx = run.pick("nope", event, "later");
    assert_eq!(
        fx.results,
        [(key("nope"), refused("This plan has no option later."))]
    );
    let fx = run.pick("deny", event, "deny");
    assert_eq!(
        answer_body(&fx),
        json!({
            "behavior": "deny",
            "message": "The user wants to keep planning.",
            "interrupt": true,
            "decisionClassification": "user_reject",
            "toolUseID": "toolu_plan",
        })
    );
    assert_eq!(
        run.screen.item("claude:toolu_plan:approval").status,
        ItemStatus::Declined
    );
    assert_eq!(
        run.screen.item("claude:toolu_plan").status,
        ItemStatus::Declined
    );
    let permission = run.lead().permission.expect("a mode");
    assert_eq!(permission.requested, PermissionMode::Ask);
}

#[test]
fn maxd_never_refreshes_a_token() {
    let mut run = mid_turn();
    let request = json!({
        "type": "control_request",
        "request_id": "req-oauth",
        "request": {"subtype": "oauth_token_refresh"},
    });
    let fx = run.feed(request);
    assert_eq!(
        only_write(&fx),
        json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": "req-oauth",
                "error": "MaxHQ doesn't refresh tokens.",
            },
        })
    );
    assert_eq!(fx.writes[0].answers.as_deref(), Some("req-oauth"));
    assert!(fx.attention.is_empty());
}

#[test]
fn maxd_declines_input_an_mcp_server_asks_for() {
    let mut run = mid_turn();
    let request = json!({
        "type": "control_request",
        "request_id": "req-ask-mcp",
        "request": {"subtype": "elicitation", "mcp_server_name": "github", "message": "Pick a repo"},
    });
    let fx = run.feed(request);
    assert_eq!(
        only_write(&fx)["response"]["response"],
        json!({"action": "decline"})
    );
    assert!(fx.attention.is_empty());
    let notice = run.screen.item("claude:req-ask-mcp");
    assert_eq!(
        notice.body,
        ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "github asked for input, and MaxHQ declined.".to_owned(),
        }
    );
}

#[test]
fn maxd_turns_down_a_request_it_doesnt_know() {
    let mut run = mid_turn();
    let request = json!({
        "type": "control_request",
        "request_id": "req-hook",
        "request": {"subtype": "hook_callback", "callback_id": "1"},
    });
    let fx = run.feed(request.clone());
    assert_eq!(
        only_write(&fx)["response"]["error"],
        "MaxHQ doesn't handle this request."
    );
    let fx = run.feed(request);
    assert!(fx.writes.is_empty(), "a request is answered once");
}

#[test]
fn a_replayed_request_the_holder_no_longer_waits_on_stays_shut() {
    let mut run = mid_turn();
    let input = json!({"command": "ls"});
    run.feed(tool_use("toolu_old", "Bash", input.clone()));
    let request = can_use_tool("req-old", "toolu_old", "Bash", input, json!([]));
    let fx = run.feed_with(request.clone(), true);
    assert!(fx.writes.is_empty());
    assert!(fx.attention.is_empty());
    let fx = run.feed(request);
    assert!(fx.attention.is_empty(), "it was answered before");
}

#[test]
fn a_prompt_answered_while_maxd_was_away_shows_from_the_calls_result() {
    let mut run = mid_turn();
    let input = json!({"command": "ls"});
    run.feed(tool_use("toolu_old", "Bash", input.clone()));
    let request = can_use_tool("req-old", "toolu_old", "Bash", input, json!([]));
    run.feed_with(request, true);
    let meta = json!({"tool_result_meta": [{"id": "toolu_old",
        "permission_decision": {"decision": "accept", "source": "user_temporary"}}]});
    let fx = run.feed(with(tool_result("toolu_old", "a.txt", json!({})), meta));
    assert!(fx.attention.is_empty());
    let ask = run.screen.item("claude:toolu_old:approval");
    assert_eq!(ask.status, ItemStatus::Completed);
    assert_eq!(
        ask.parent.as_ref().map(|id| id.0.as_str()),
        Some("claude:toolu_old")
    );
    let decided = decision(ask).expect("an answer");
    assert_eq!(
        decided.answer,
        AttentionAnswer::Option {
            id: "allow".to_owned()
        }
    );
    assert_eq!(decided.by, None);
}

#[test]
fn a_second_request_for_the_same_call_gets_its_own_approval() {
    let mut run = mid_turn();
    let input = json!({"command": "rm -rf build"});
    run.feed(tool_use("toolu_rm", "Bash", input.clone()));
    let first = can_use_tool("req-rm", "toolu_rm", "Bash", input.clone(), json!([]));
    run.feed(first);
    run.pick("deny", approval("req-rm"), "deny");
    let again = can_use_tool("req-rm-again", "toolu_rm", "Bash", input, json!([]));
    let fx = run.feed(again);
    assert_eq!(opened(&fx)[0].id, approval("req-rm-again"));
    let first = run.screen.item("claude:toolu_rm:approval");
    assert_eq!(first.status, ItemStatus::Declined);
    let second = run.screen.item("claude:req-rm-again");
    assert_eq!(second.status, ItemStatus::Waiting);
    assert_eq!(
        second.parent.as_ref().map(|id| id.0.as_str()),
        Some("claude:toolu_rm")
    );
}

#[test]
fn a_request_that_comes_twice_opens_once() {
    let mut run = asked();
    let fx = run.feed(run.records[84]["msg"].clone());
    assert!(fx.attention.is_empty());
    assert!(fx.writes.is_empty());
}

/// The deny that dismisses the call `call`.
fn dismissal(call: &str) -> Value {
    json!({
        "behavior": "deny",
        "message": DISMISSED,
        "interrupt": true,
        "decisionClassification": "user_reject",
        "toolUseID": call,
    })
}

#[test]
fn dismissing_a_question_stops_the_turn_and_the_queued_message_goes_next() {
    let mut run = mid_turn();
    let fx = run.act(send(
        "later",
        lead(),
        "Look at what was done overnight",
        SendMode::Queue,
    ));
    let queued = only_write(&fx)["uuid"].as_str().expect("a UUID").to_owned();
    ask_one(&mut run);
    let event = AttentionId::derive("claude:toolu_ask");

    let fx = run.answer("dismiss", event, AttentionAnswer::Dismiss);
    assert_eq!(answer_body(&fx), dismissal("toolu_ask"));
    assert_eq!(fx.writes[0].answers.as_deref(), Some("req-ask"));
    assert_eq!(fx.results, [(key("dismiss"), ActionOutcome::Done)]);
    assert_eq!(closed(&fx), [event]);
    let item = run.screen.item("claude:toolu_ask");
    assert_eq!(item.status, ItemStatus::Declined);
    assert!(matches!(
        &item.body,
        ItemBody::Question { answers: None, .. }
    ));
    assert_ne!(
        run.lead().activity,
        Activity::NeedsInput { attention: event }
    );

    // The CLI turns the call down, ends the turn, then starts the message
    // you queued.
    let mut refused = tool_result("toolu_ask", DISMISSED, json!(format!("Error: {DISMISSED}")));
    refused["message"]["content"][0]["is_error"] = json!(true);
    run.feed(refused);
    run.feed(line(json!({
        "type": "user",
        "parent_tool_use_id": null,
        "message": {"role": "user", "content": [
            {"type": "text", "text": "[Request interrupted by user for tool use]"},
        ]},
    })));
    let ended = json!({"subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_tools"});
    run.feed(result("result-dismissed", ended));
    run.feed(json!({"type": "command_lifecycle", "command_uuid": queued, "state": "started"}));
    let item = run.screen.item("claude:toolu_ask");
    assert_eq!(item.status, ItemStatus::Declined);
    assert!(matches!(
        &item.body,
        ItemBody::Question { answers: None, .. }
    ));
    let message = run.screen.item(&format!("claude:{queued}"));
    assert_eq!(message.status, ItemStatus::Completed);
    assert!(matches!(
        message.body,
        ItemBody::UserMessage {
            delivery: Delivery::Sent,
            ..
        }
    ));
}

#[test]
fn a_message_sent_now_dismisses_what_waits_then_goes() {
    let mut run = mid_turn();
    let command = json!({"command": "rm -rf build", "description": "Remove the build"});
    run.feed(tool_use("toolu_rm", "Bash", command.clone()));
    run.feed(can_use_tool(
        "req-rm",
        "toolu_rm",
        "Bash",
        command,
        json!([]),
    ));
    let plan = json!({"plan": "1. Write a.txt"});
    run.feed(tool_use("toolu_plan", "ExitPlanMode", plan.clone()));
    run.feed(can_use_tool(
        "req-plan",
        "toolu_plan",
        "ExitPlanMode",
        plan,
        json!([]),
    ));
    ask_one(&mut run);
    let mode = run.lead().permission;

    let fx = run.act(send(
        "now",
        lead(),
        "Stop and look here",
        SendMode::Interrupt,
    ));
    let written: Vec<Value> = fx
        .writes
        .iter()
        .map(|write| serde_json::from_str(&write.line).expect("a JSON line"))
        .collect();
    assert_eq!(written.len(), 4, "{written:?}");
    let answered: Vec<(Value, Value)> = written[..3]
        .iter()
        .map(|line| {
            (
                line["response"]["request_id"].clone(),
                line["response"]["response"].clone(),
            )
        })
        .collect();
    assert_eq!(
        answered,
        [
            (json!("req-rm"), dismissal("toolu_rm")),
            (json!("req-plan"), dismissal("toolu_plan")),
            (json!("req-ask"), dismissal("toolu_ask")),
        ]
    );
    assert_eq!(written[3]["type"], "user");
    assert_eq!(written[3]["priority"], "now");
    assert_eq!(fx.results, [(key("now"), ActionOutcome::Done)]);
    assert_eq!(closed(&fx).len(), 3);
    assert!(run.screen.open.is_empty());
    for approval in ["claude:toolu_rm:approval", "claude:toolu_plan:approval"] {
        let ask = run.screen.item(approval);
        assert_eq!(ask.status, ItemStatus::Declined, "{approval}");
        assert_eq!(decision(ask), None, "{approval}");
    }
    assert_eq!(
        run.screen.item("claude:toolu_rm").status,
        ItemStatus::Declined
    );
    let question = run.screen.item("claude:toolu_ask");
    assert_eq!(question.status, ItemStatus::Declined);
    assert!(matches!(
        &question.body,
        ItemBody::Question { answers: None, .. }
    ));
    assert_eq!(
        run.lead().permission,
        mode,
        "dismissing a plan keeps the mode"
    );
}

/// A made-up claude.ai page's link.
const PAGE_LINK: &str = "https://claude.ai/artifact/EXAMPLE1";

/// A `can_use_tool` request with `fields` added to the request, offering
/// a rule to keep.
fn asking(request: &str, call: &str, name: &str, input: Value, fields: Value) -> Value {
    let rule = json!([{
        "type": "addRules",
        "rules": [{"toolName": name}],
        "behavior": "allow",
        "destination": "session",
    }]);
    let mut line = can_use_tool(request, call, name, input, rule);
    line["request"] = with(line["request"].take(), fields);
    line
}

/// The parts of an approval item these tests look at.
fn approval_parts(item: &TimelineItem) -> (ApprovalKind, &str, Option<&str>, bool, Option<&str>) {
    let ItemBody::Approval {
        approval_kind,
        title,
        detail,
        deny_first,
        reason,
        ..
    } = &item.body
    else {
        panic!("not an approval: {:?}", item.body);
    };
    (
        *approval_kind,
        title,
        detail.as_deref(),
        *deny_first,
        reason.as_deref(),
    )
}

#[test]
fn a_page_ask_shows_its_question_and_the_reply_keeps_the_input_whole() {
    let mut run = mid_turn();
    run.feed(tool_use(
        "toolu_del",
        "Artifact",
        json!({"action": "delete", "url": PAGE_LINK}),
    ));
    let tool = run.screen.item("claude:toolu_del");
    let ItemBody::ToolCall {
        title, artifact, ..
    } = &tool.body
    else {
        panic!("{:?}", tool.body);
    };
    assert_eq!(title.as_deref(), Some("Delete a claude.ai page"));
    assert_eq!(
        artifact.as_ref().and_then(|call| call.url.as_deref()),
        Some(PAGE_LINK)
    );

    // The CLI adds its ask to the input it asks about.
    let input = json!({
        "action": "delete",
        "url": PAGE_LINK,
        "__artifactAsk": {
            "class": "delete", "remember": "none", "slug": "EXAMPLE1", "url": PAGE_LINK,
            "title": "Old draft", "reachable": true, "askedIn": "main",
        },
    });
    let question = "Permanently delete \"Old draft\"?";
    let fx = run.feed(asking(
        "req-del",
        "toolu_del",
        "Artifact",
        input.clone(),
        json!({
            "title": "Claude wants to delete an artifact",
            "display_name": "Artifact",
            "decision_reason": question,
            "decision_reason_type": "other",
            "default_to_no": true,
            "suppress_always_allow_rule": true,
        }),
    ));
    let shown = opened(&fx);
    assert_eq!(shown[0].summary, question);
    // A request that may not keep a rule offers none, even with one.
    assert_eq!(option_ids(&shown[0]), ["allow", "deny"]);
    let ask = run.screen.item("claude:toolu_del:approval");
    assert_eq!(
        approval_parts(ask),
        (
            ApprovalKind::Network,
            question,
            Some(PAGE_LINK),
            true,
            // Its reason only said the question again.
            None
        )
    );
    let event = approval("req-del");
    let fx = run.pick("session", event, "allow_session");
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(
            key("session"),
            refused("This request has no rule to keep for the session.")
        )]
    );
    let fx = run.pick("allow", event, "allow");
    assert_eq!(
        answer_body(&fx),
        json!({
            "behavior": "allow",
            "updatedInput": input,
            "decisionClassification": "user_temporary",
            "toolUseID": "toolu_del",
        })
    );

    let result = json!({"deleted": true, "url": PAGE_LINK, "title": "Old draft"});
    run.feed(tool_result("toolu_del", "Deleted.", result));
    let tool = run.screen.item("claude:toolu_del");
    let ItemBody::ToolCall { title, .. } = &tool.body else {
        panic!("{:?}", tool.body);
    };
    assert_eq!(title.as_deref(), Some("Deleted \"Old draft\""));
}

#[test]
fn a_person_only_ask_offers_no_buttons_but_its_card_answers_it() {
    let mut run = mid_turn();
    let input = json!({
        "action": "read",
        "url": PAGE_LINK,
        "prompt": "the open questions",
        "__artifactAsk": [
            {"class": "foreign_read", "remember": "artifact", "slug": "EXAMPLE1",
             "url": PAGE_LINK, "title": "Launch checklist", "about": "someone_else"},
            {"class": "something_new", "slug": "EXAMPLE1", "url": PAGE_LINK},
        ],
    });
    run.feed(tool_use(
        "toolu_read",
        "Artifact",
        json!({"action": "read", "url": PAGE_LINK}),
    ));
    let fx = run.feed(asking(
        "req-read",
        "toolu_read",
        "Artifact",
        input.clone(),
        json!({
            "description": "Read a page made by someone else",
            "decision_reason": "\u{1b}[33mThe page\u{1b}[0m is\nsomeone else's",
            "requires_user_interaction": true,
        }),
    ));
    let shown = opened(&fx);
    assert!(shown[0].options.is_empty(), "{:?}", shown[0].options);
    assert!(shown[0].accepts_text);
    let first = "\"Launch checklist\" was made by someone else. \
                 Let Claude read it for the rest of this conversation?";
    assert_eq!(shown[0].summary, format!("{first} (and 1 more)"));
    let ask = run.screen.item("claude:toolu_read:approval");
    let (kind, _, detail, deny_first, reason) = approval_parts(ask);
    assert_eq!(kind, ApprovalKind::Network);
    assert_eq!(
        detail,
        Some(&*format!(
            "{first}\nRead a page made by someone else\n{PAGE_LINK}\n\
             Looking for: the open questions"
        ))
    );
    assert!(!deny_first);
    assert_eq!(reason, Some("The page is someone else's"));
    // The card keeps every option.
    let ItemBody::Approval { options, .. } = &ask.body else {
        unreachable!()
    };
    let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
    assert_eq!(ids, ["allow", "allow_session", "deny"]);

    let fx = run.pick("card", approval("req-read"), "allow_session");
    assert_eq!(fx.results, [(key("card"), ActionOutcome::Done)]);
    let body = answer_body(&fx);
    assert_eq!(body["behavior"], "allow");
    assert_eq!(body["updatedInput"], input);
    assert_eq!(
        body["updatedInput"]["__artifactAsk"],
        input["__artifactAsk"]
    );
}

#[test]
fn a_tool_maxhq_has_no_words_for_asks_with_the_requests_own_title() {
    let mut run = mid_turn();
    let input = json!({"script": "steps.js"});
    run.feed(tool_use("toolu_wf", "Workflow", input.clone()));
    let fx = run.feed(asking(
        "req-wf",
        "toolu_wf",
        "Workflow",
        input,
        json!({
            "title": "Claude wants to run\n\u{1b}[1ma workflow\u{1b}[0m",
            "decision_reason": "Workflows always ask",
        }),
    ));
    assert_eq!(opened(&fx)[0].summary, "Claude wants to run a workflow");
    let ask = run.screen.item("claude:toolu_wf:approval");
    assert_eq!(
        approval_parts(ask),
        (
            ApprovalKind::Tool,
            "Claude wants to run a workflow",
            Some(r#"{"script":"steps.js"}"#),
            false,
            Some("Workflows always ask")
        )
    );
}

#[test]
fn a_page_call_answered_while_maxd_was_away_names_its_page() {
    let mut run = mid_turn();
    let input = json!({"action": "set", "url": PAGE_LINK, "collection": "notes", "doc_id": "a"});
    run.feed(tool_use("toolu_set", "ArtifactData", input.clone()));
    let request = can_use_tool("req-set", "toolu_set", "ArtifactData", input, json!([]));
    run.feed_with(request, true);
    let meta = json!({"tool_result_meta": [{"id": "toolu_set",
        "permission_decision": {"decision": "accept", "source": "user_temporary"}}]});
    let result = json!({"written": {"url": PAGE_LINK}, "owned": true});
    run.feed(with(tool_result("toolu_set", "Saved.", result), meta));
    let ask = run.screen.item("claude:toolu_set:approval");
    let title = format!("Write to notes in {PAGE_LINK}?");
    assert_eq!(
        approval_parts(ask),
        (
            ApprovalKind::Network,
            &*title,
            Some("Data: notes/a"),
            false,
            None
        )
    );
    let tool = run.screen.item("claude:toolu_set");
    let ItemBody::ToolCall { title, .. } = &tool.body else {
        panic!("{:?}", tool.body);
    };
    assert_eq!(title.as_deref(), Some("Wrote to notes in a claude.ai page"));
}
