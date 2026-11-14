// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Drives `AcpSession` through a local command's flavor, as a fake agent
//! that speaks ACP v2: `info` and `capabilities` in its `initialize`
//! reply, config options by `configId`, a prompt's turn that ends with a
//! `state_update`, and permission requests that name a subject. The
//! fixtures under `fixtures/v2` are written by hand from the schema
//! `rpc::V2_SCHEMA` names.

use maxhq_protocol::acp::AcpFeatures;

use super::local::id;
use super::*;
use crate::drive::acp::rpc;

fn v2(name: &str) -> Vec<String> {
    fixture_in("v2", name)
}

impl Run {
    /// A local command's driver past the v2 `handshake`, in `mode`.
    fn v2_ready(handshake: &str, mode: PermissionMode) -> Self {
        let mut run = Self::local(Kept::default(), mode);
        run.connect();
        run.feed_all(&v2(handshake));
        run.take();
        run
    }

    /// A v2 driver with a first turn under way.
    fn v2_turning(text: &str, mode: PermissionMode) -> Self {
        let mut run = Self::v2_ready("handshake.jsonl", mode);
        run.act(send(key(1), text, SendMode::Queue));
        run.take();
        run
    }
}

// The handshake.

#[test]
fn the_handshake_reads_v2s_capabilities_and_config_options() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed_all(&v2("handshake.jsonl"));
    let fx = run.take();
    assert_eq!(run.session.wire.generation(), Some(rpc::Generation::V2));
    let (_, features) = fx.features.last().expect("features");
    assert!(features.images && features.load && features.plan);
    let methods: Vec<&str> = features
        .auth
        .iter()
        .map(|method| method.id.as_str())
        .collect();
    assert_eq!(methods, ["api-key", "browser"]);
    let (_, models) = fx.models.last().expect("models");
    let listed: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(listed, ["pro", "flash"]);
    let agent = last_agent(&fx, lead());
    assert_eq!(agent.model.as_deref(), Some("pro"));
    assert_eq!(agent.activity, Activity::Idle);
}

#[test]
fn a_v2_agent_with_no_session_capability_neither_loads_nor_takes_images() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed_all(&v2("bare.jsonl"));
    let fx = run.take();
    assert_eq!(fx.features.last().unwrap().1, AcpFeatures::default());
}

#[test]
fn a_new_v2_session_names_its_mcp_servers_type() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed(&v2("handshake.jsonl")[0]);
    let fx = run.take();
    let new = sent_once(&fx, "session/new");
    for server in new["params"]["mcpServers"].as_array().unwrap() {
        assert_eq!(server["type"], "stdio");
    }
}

// Turns.

#[test]
fn a_turn_ends_with_the_idle_state() {
    let mut run = Run::v2_turning("Say hello", PermissionMode::Ask);
    let turn = v2("turn.jsonl");
    run.feed_all(&turn[..3]);
    let fx = run.take();
    // The prompt's acceptance doesn't end the turn.
    assert!(fx
        .agents
        .iter()
        .all(|agent| agent.activity != Activity::Idle));
    run.feed(&turn[3]);
    let fx = run.take();
    assert_eq!(
        text_of(item(&items(&fx, lead()), &id("t1:m0"))),
        "Hello there."
    );
    assert_eq!(last_agent(&fx, lead()).activity, Activity::Idle);
    run.act(send(key(2), "Again", SendMode::Queue));
    run.feed_all(&v2("turn.jsonl"));
    let fx = run.take();
    assert_eq!(
        text_of(item(&items(&fx, lead()), &id("t2:m0"))),
        "Hello there."
    );
}

#[test]
fn a_cancelled_turn_ends_interrupted() {
    let mut run = Run::v2_turning("Run the tests", PermissionMode::Ask);
    run.feed_all(&v2("command-start.jsonl"));
    run.act(Action::Interrupt {
        key: key(2),
        agent: lead(),
    });
    let fx = run.take();
    sent_once(&fx, "session/cancel");
    run.feed_all(&v2("cancelled.jsonl"));
    let fx = run.take();
    assert_eq!(
        item(&items(&fx, lead()), &id("t1:c0")).status,
        ItemStatus::Interrupted
    );
    assert_eq!(last_agent(&fx, lead()).activity, Activity::Idle);
}

#[test]
fn a_refused_prompt_ends_the_turn() {
    let mut run = Run::v2_turning("Hi", PermissionMode::Ask);
    run.feed_all(&v2("refused.jsonl"));
    let fx = run.take();
    assert!(items(&fx, lead()).iter().any(|item| matches!(
        &item.body,
        ItemBody::Error { message, sign_in: None, .. } if message.contains("overloaded")
    )));
    // A late idle state has no turn left to end.
    run.feed_all(&v2("cancelled.jsonl"));
    run.take();
    run.act(send(key(2), "Again", SendMode::Queue));
    sent_once(&run.take(), "session/prompt");
}

// Loading.

#[test]
fn a_load_resumes_from_the_start_into_history() {
    let mut run = Run::local(
        Kept {
            session: Some(SESSION.to_owned()),
            ..Kept::default()
        },
        PermissionMode::Ask,
    );
    run.connect();
    run.feed(&v2("handshake.jsonl")[0]);
    let fx = run.take();
    assert!(sent(&fx, "session/load").is_empty());
    let resume = sent_once(&fx, "session/resume");
    assert_eq!(resume["params"]["sessionId"], SESSION);
    assert_eq!(resume["params"]["replayFrom"]["type"], "start");
    run.feed_all(&v2("load.jsonl"));
    let fx = run.take();
    let (_, history) = fx.history.last().expect("history");
    assert_eq!(text_of(&history[0]), "Say hi.");
    assert_eq!(text_of(&history[1]), "Hi.");
    assert_eq!(last_agent(&fx, lead()).model.as_deref(), Some("flash"));
}

// Config options.

#[test]
fn a_model_change_sets_the_config_option_by_id() {
    let mut run = Run::v2_ready("handshake.jsonl", PermissionMode::Ask);
    run.act(Action::SetModel {
        key: key(1),
        agent: lead(),
        model: "flash".into(),
    });
    let fx = run.take();
    let set = sent_once(&fx, "session/set_config_option");
    assert_eq!(set["params"]["configId"], "model");
    assert_eq!(set["params"]["value"], "flash");
    assert_eq!(set["params"]["type"], "id");
    let lines = v2("plan.jsonl");
    let options =
        &serde_json::from_str::<Value>(&lines[2]).unwrap()["params"]["update"]["configOptions"];
    run.feed(&format!(
        r#"{{"jsonrpc":"2.0","id":"$session/set_config_option","result":{{"configOptions":{options}}}}}"#
    ));
    let fx = run.take();
    assert_eq!(outcome(&fx, key(1)), ActionOutcome::Done);
    assert_eq!(last_agent(&fx, lead()).model.as_deref(), Some("flash"));
}

#[test]
fn plan_is_the_mode_option_named_plan() {
    let mut run = Run::v2_ready("handshake.jsonl", PermissionMode::Ask);
    run.act(Action::SetPermission {
        key: key(1),
        agent: lead(),
        mode: PermissionMode::Plan,
    });
    let fx = run.take();
    let set = sent_once(&fx, "session/set_config_option");
    assert_eq!(set["params"]["configId"], "mode");
    assert_eq!(set["params"]["value"], "plan");
}

#[test]
fn a_plan_update_is_the_todo_list_and_a_config_update_moves_the_model() {
    let mut run = Run::v2_turning("Fix it", PermissionMode::Ask);
    run.feed_all(&v2("plan.jsonl"));
    let fx = run.take();
    let items = items(&fx, lead());
    assert!(
        matches!(&item(&items, &id("t1:todos")).body, ItemBody::Todos { todos } if todos.len() == 2)
    );
    let agent = last_agent(&fx, lead());
    assert_eq!(agent.model.as_deref(), Some("flash"));
    // An idle state with no stop reason ends the turn as done.
    assert_eq!(agent.activity, Activity::Idle);
}

// Signing in.

#[test]
fn a_sign_in_error_logs_in_and_tries_once_more() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed(&v2("handshake.jsonl")[0]);
    run.feed_all(&v2("signed-out.jsonl"));
    let fx = run.take();
    assert!(sent(&fx, "authenticate").is_empty());
    assert_eq!(
        sent_once(&fx, "auth/login")["params"]["methodId"],
        "browser"
    );
    run.feed(r#"{"jsonrpc":"2.0","id":"$auth/login","result":{}}"#);
    let fx = run.take();
    sent_once(&fx, "session/new");
    run.feed(&v2("handshake.jsonl")[1]);
    assert_eq!(last_agent(&run.take(), lead()).activity, Activity::Idle);
}

// Permissions.

/// What maxd answered for itself to `fixture`'s request in `mode`, if it
/// didn't ask you.
fn answered(mode: PermissionMode, fixture: &str) -> Option<Value> {
    let mut run = Run::v2_turning("Clean up", mode);
    run.feed_all(&v2(fixture));
    let fx = run.take();
    let reply = answers(&fx).pop();
    assert_eq!(
        opened(&fx).is_empty(),
        reply.is_some(),
        "{mode:?} {fixture}"
    );
    reply.map(|reply| reply["result"]["outcome"]["optionId"].clone())
}

#[test]
fn a_permissions_subject_is_its_tool_call() {
    let yes = Some(Value::from("yes"));
    assert_eq!(answered(PermissionMode::Ask, "permission.jsonl"), None);
    assert_eq!(
        answered(PermissionMode::Ask, "permission-command.jsonl"),
        None
    );
    assert_eq!(
        answered(PermissionMode::AcceptEdits, "permission.jsonl"),
        yes
    );
    // A command subject is a command to run.
    assert_eq!(
        answered(PermissionMode::AcceptEdits, "permission-command.jsonl"),
        None
    );
    assert_eq!(
        answered(PermissionMode::FullAccess, "permission-command.jsonl"),
        yes
    );
}

#[test]
fn a_command_permission_shows_its_command() {
    let mut run = Run::v2_turning("Clean up", PermissionMode::Ask);
    run.feed_all(&v2("permission-command.jsonl"));
    let fx = run.take();
    let event = opened(&fx).pop().expect("an event");
    let shown = format!("{event:?}");
    assert!(shown.contains("make clean"), "{shown}");
}
