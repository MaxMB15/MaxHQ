// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Drives `AcpSession` through a local command's flavor, as a fake agent
//! that speaks plain ACP v1: v1 `modes` and `models` in place of config
//! options, a sign-in it runs itself, and no extensions. The fixtures under
//! `fixtures/local` are written by hand from ACP's schema.

use std::sync::Arc;

use maxhq_protocol::acp::{AcpAgent, AcpFeatures};
use maxhq_protocol::ids::AcpAgentId;

use super::*;
use crate::drive::acp::{needs_sign_in, rpc};
use crate::drive::local::{AcpCommand, Local};

fn local(name: &str) -> Vec<String> {
    fixture_in("local", name)
}

fn agent() -> AcpAgent {
    AcpAgent {
        id: AcpAgentId::derive("example"),
        device: DeviceId::derive("devbox"),
        name: "Example".to_owned(),
        program: "example-agent".to_owned(),
        args: vec!["--acp".to_owned()],
        env: Vec::new(),
        containers: false,
        problem: None,
        skills: Default::default(),
    }
}

fn local_login() -> Login {
    Login {
        account: account(),
        config_dir: PathBuf::new(),
        auth: LoginAuth::AcpCommand(Arc::new(AcpCommand {
            agent: agent(),
            env: Vec::new(),
        })),
    }
}

impl Run {
    /// A local command's driver, kept with `kept`.
    pub(super) fn local(kept: Kept, mode: PermissionMode) -> Self {
        let login = local_login();
        Self::of(Box::new(Local::of(&login)), &login, kept, mode)
    }

    /// A local command's driver past `handshake`, in `mode`.
    fn local_ready(handshake: &str, mode: PermissionMode) -> Self {
        let mut run = Self::local(Kept::default(), mode);
        run.connect();
        run.feed_all(&local(handshake));
        run.take();
        run
    }

    /// A local command's driver with a first turn under way.
    fn local_turning(text: &str, mode: PermissionMode) -> Self {
        let mut run = Self::local_ready("handshake.jsonl", mode);
        run.act(send(key(1), text, SendMode::Queue));
        run.take();
        run
    }
}

/// The ID of item `rest` in a local agent's stream.
pub(super) fn id(rest: &str) -> String {
    format!("acp:{rest}")
}

// The handshake.

#[test]
fn the_handshake_reports_features_and_v1_models_and_modes() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    let fx = run.take();
    let init = sent_once(&fx, "initialize");
    assert_eq!(
        init["params"]["clientCapabilities"]["_meta"],
        serde_json::json!({})
    );
    run.feed_all(&local("handshake.jsonl"));
    let fx = run.take();
    let (to, features) = fx.features.last().expect("features");
    assert_eq!(*to, account());
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
    assert_eq!(agent.vendor, Vendor::Acp);
    assert_eq!(agent.model.as_deref(), Some("pro"));
    assert_eq!(agent.activity, Activity::Idle);
    // Features go out once, until they change.
    run.act(send(key(1), "Hi", SendMode::Queue));
    assert!(run.take().features.is_empty());
}

#[test]
fn an_agent_with_no_models_or_modes_runs_without_them() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed_all(&local("bare.jsonl"));
    let fx = run.take();
    assert!(fx.models.is_empty());
    assert_eq!(fx.features.last().unwrap().1, AcpFeatures::default());
    assert_eq!(last_agent(&fx, lead()).model, None);
    run.act(Action::SetModel {
        key: key(1),
        agent: lead(),
        model: "pro".into(),
    });
    run.act(Action::SetPermission {
        key: key(2),
        agent: lead(),
        mode: PermissionMode::Plan,
    });
    let fx = run.take();
    assert!(fx.writes.is_empty());
    assert!(matches!(
        outcome(&fx, key(1)),
        ActionOutcome::Refused { .. }
    ));
    assert!(matches!(
        outcome(&fx, key(2)),
        ActionOutcome::Refused { .. }
    ));
}

#[test]
fn an_agent_that_says_version_2_in_v1s_shape_speaks_v1() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed_all(&local("version-2.jsonl"));
    let fx = run.take();
    // v1's shape: the MCP servers name no type.
    let new = sent_once(&fx, "session/new");
    assert_eq!(new["params"]["mcpServers"], serde_json::json!([]));
    assert_eq!(run.session.wire.generation(), Some(rpc::Generation::V1));
}

#[test]
fn an_agent_in_neither_shape_is_refused() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.act(send(key(1), "Hi", SendMode::Queue));
    run.feed_all(&local("unspoken.jsonl"));
    let fx = run.take();
    assert!(sent(&fx, "session/new").is_empty());
    let reason = "Example speaks ACP version 3, which MaxHQ doesn't.";
    assert!(
        matches!(&last_agent(&fx, lead()).activity, Activity::Error { reason: said } if said == reason)
    );
}

// Turns.

#[test]
fn simple_and_multi_turn() {
    let mut run = Run::local_turning("Say hello", PermissionMode::Ask);
    run.feed_all(&local("turn.jsonl"));
    let fx = run.take();
    let first = items(&fx, lead());
    assert_eq!(text_of(item(&first, &id("t1:m0"))), "Hello there.");
    assert_eq!(last_agent(&fx, lead()).activity, Activity::Idle);
    run.act(send(key(2), "Again", SendMode::Queue));
    run.feed_all(&local("turn.jsonl"));
    let fx = run.take();
    let second = items(&fx, lead());
    assert_eq!(text_of(item(&second, &id("t2:user"))), "Again");
    assert_eq!(text_of(item(&second, &id("t2:m0"))), "Hello there.");
}

#[test]
fn queued_steered_and_interrupted() {
    let mut run = Run::local_turning("Run the tests", PermissionMode::Ask);
    run.feed_all(&local("command-start.jsonl"));
    run.act(send(key(2), "Then lint", SendMode::Queue));
    let fx = run.take();
    assert!(sent(&fx, "session/cancel").is_empty());
    assert!(sent(&fx, "session/prompt").is_empty());

    run.act(send(key(3), "Only the fast ones", SendMode::Steer));
    let fx = run.take();
    sent_once(&fx, "session/cancel");
    run.feed_all(&local("cancelled.jsonl"));
    let fx = run.take();
    let prompt = sent_once(&fx, "session/prompt");
    assert_eq!(prompt["params"]["prompt"][0]["text"], "Only the fast ones");
    assert_eq!(
        item(&items(&fx, lead()), &id("t1:c0")).status,
        ItemStatus::Interrupted
    );

    run.act(Action::Interrupt {
        key: key(4),
        agent: lead(),
    });
    let fx = run.take();
    assert_eq!(outcome(&fx, key(4)), ActionOutcome::Done);
    sent_once(&fx, "session/cancel");
    run.feed_all(&local("cancelled.jsonl"));
    let fx = run.take();
    // The queued message goes once the interrupted turn ends.
    assert_eq!(
        sent_once(&fx, "session/prompt")["params"]["prompt"][0]["text"],
        "Then lint"
    );
}

#[test]
fn a_crash_ends_the_turn_with_what_stderr_said() {
    let mut run = Run::local_turning("Run the tests", PermissionMode::Ask);
    run.feed_all(&local("command-start.jsonl"));
    run.take();
    run.session.exited(
        &ChildExit {
            code: Some(2),
            signal: None,
            at: Timestamp::now(),
            stderr_tail: "panic: no config\n".into(),
            with_machine: false,
        },
        &mut run.fx,
    );
    let fx = run.take();
    let agent = last_agent(&fx, lead());
    assert!(
        matches!(&agent.activity, Activity::Error { reason } if reason.contains("Example") && reason.contains("no config"))
    );
    assert_eq!(
        item(&items(&fx, lead()), &id("t1:c0")).status,
        ItemStatus::Interrupted
    );
}

// Loading.

#[test]
fn a_load_reads_the_replay_into_history() {
    let mut run = Run::local(
        Kept {
            session: Some(SESSION.to_owned()),
            ..Kept::default()
        },
        PermissionMode::Ask,
    );
    run.connect();
    run.feed(&local("handshake.jsonl")[0]);
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "session/load")["params"]["sessionId"],
        SESSION
    );
    run.feed_all(&local("load.jsonl"));
    let fx = run.take();
    assert!(fx.timeline.is_empty());
    let (_, history) = fx.history.last().expect("history");
    assert_eq!(text_of(&history[0]), "Say hi.");
    assert_eq!(text_of(&history[1]), "Hi.");
}

#[test]
fn an_agent_that_cant_load_starts_a_new_session_and_says_so() {
    let mut run = Run::local(
        Kept {
            session: Some(SESSION.to_owned()),
            ..Kept::default()
        },
        PermissionMode::Ask,
    );
    run.connect();
    run.feed(&local("bare.jsonl")[0]);
    let fx = run.take();
    sent_once(&fx, "session/new");
    assert!(items(&fx, lead()).iter().any(|item| matches!(
        &item.body,
        ItemBody::Notice { message, .. } if message.contains("Example can't load")
    )));
}

// Models and modes.

#[test]
fn a_model_change_goes_through_set_model() {
    let mut run = Run::local_ready("handshake.jsonl", PermissionMode::Ask);
    run.act(Action::SetModel {
        key: key(1),
        agent: lead(),
        model: "flash".into(),
    });
    let fx = run.take();
    let set = sent_once(&fx, "session/set_model");
    assert_eq!(set["params"]["sessionId"], SESSION);
    assert_eq!(set["params"]["modelId"], "flash");
    run.feed(r#"{"jsonrpc":"2.0","id":"$session/set_model","result":{}}"#);
    let fx = run.take();
    assert_eq!(outcome(&fx, key(1)), ActionOutcome::Done);
    assert_eq!(last_agent(&fx, lead()).model.as_deref(), Some("flash"));
}

#[test]
fn a_new_session_is_asked_for_the_model_picked() {
    let mut run = Run::local(
        Kept {
            model: Some("flash".to_owned()),
            ..Kept::default()
        },
        PermissionMode::Ask,
    );
    run.connect();
    run.feed_all(&local("handshake.jsonl"));
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "session/set_model")["params"]["modelId"],
        "flash"
    );
}

#[test]
fn plan_is_the_mode_named_plan() {
    let mut run = Run::local_ready("handshake.jsonl", PermissionMode::Ask);
    run.act(Action::SetPermission {
        key: key(1),
        agent: lead(),
        mode: PermissionMode::Plan,
    });
    let fx = run.take();
    let set = sent_once(&fx, "session/set_mode");
    assert_eq!(set["params"]["modeId"], "architect");
    run.feed(r#"{"jsonrpc":"2.0","id":"$session/set_mode","result":{}}"#);
    let fx = run.take();
    assert_eq!(outcome(&fx, key(1)), ActionOutcome::Done);
    let permission = last_agent(&fx, lead()).permission.unwrap();
    assert_eq!(permission.effective, Some(PermissionMode::Plan));

    run.act(Action::SetPermission {
        key: key(2),
        agent: lead(),
        mode: PermissionMode::Ask,
    });
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "session/set_mode")["params"]["modeId"],
        "default"
    );
}

#[test]
fn a_prompt_sent_at_start_waits_for_plan_mode() {
    let mut run = Run::local(Kept::default(), PermissionMode::Plan);
    run.connect();
    run.act(send(key(1), "Plan the change", SendMode::Queue));
    run.feed_all(&local("handshake.jsonl"));
    let fx = run.take();
    sent_once(&fx, "session/set_mode");
    assert!(sent(&fx, "session/prompt").is_empty());
    run.feed(r#"{"jsonrpc":"2.0","id":"$session/set_mode","result":{}}"#);
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "session/prompt")["params"]["prompt"][0]["text"],
        "Plan the change"
    );
    let permission = last_agent(&fx, lead()).permission.unwrap();
    assert_eq!(permission.effective, Some(PermissionMode::Plan));
}

#[test]
fn a_refused_plan_mode_starts_in_ask() {
    let mut run = Run::local(Kept::default(), PermissionMode::Plan);
    run.connect();
    run.act(send(key(1), "Plan the change", SendMode::Queue));
    run.feed_all(&local("handshake.jsonl"));
    run.feed(r#"{"jsonrpc":"2.0","id":"$session/set_mode","error":{"code":-32603,"message":"No such mode"}}"#);
    let fx = run.take();
    sent_once(&fx, "session/prompt");
    let permission = last_agent(&fx, lead()).permission.unwrap();
    assert_eq!(permission.effective, Some(PermissionMode::Ask));
    assert!(items(&fx, lead()).iter().any(|item| matches!(
        &item.body,
        ItemBody::Notice { message, .. } if message.contains("No such mode")
    )));
}

#[test]
fn an_agent_with_no_plan_mode_starts_in_ask() {
    let mut run = Run::local(Kept::default(), PermissionMode::Plan);
    run.connect();
    run.act(send(key(1), "Plan the change", SendMode::Queue));
    run.feed_all(&local("bare.jsonl"));
    let fx = run.take();
    assert!(sent(&fx, "session/set_mode").is_empty());
    sent_once(&fx, "session/prompt");
    let permission = last_agent(&fx, lead()).permission.unwrap();
    assert_eq!(permission.effective, Some(PermissionMode::Ask));
}

#[test]
fn an_agent_picked_in_plan_is_asked_for_its_plan_mode() {
    let mut run = Run::local(Kept::default(), PermissionMode::Plan);
    run.connect();
    run.feed_all(&local("handshake.jsonl"));
    let fx = run.take();
    assert_eq!(
        sent_once(&fx, "session/set_mode")["params"]["modeId"],
        "architect"
    );
    // The agent says so itself.
    run.feed(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"00000000-0000-4000-8000-000000000001","update":{"sessionUpdate":"current_mode_update","currentModeId":"architect"}}}"#);
    run.feed(r#"{"jsonrpc":"2.0","id":"$session/set_mode","result":{}}"#);
    let fx = run.take();
    let permission = last_agent(&fx, lead()).permission.unwrap();
    assert_eq!(permission.effective, Some(PermissionMode::Plan));
}

// Signing in.

#[test]
fn a_sign_in_error_gets_authenticate_and_one_more_try() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed(&local("handshake.jsonl")[0]);
    run.feed_all(&local("signed-out.jsonl"));
    let fx = run.take();
    // The browser method, since the agent can't set a variable itself.
    assert_eq!(
        sent_once(&fx, "authenticate")["params"]["methodId"],
        "browser"
    );
    run.feed(r#"{"jsonrpc":"2.0","id":"$authenticate","result":{}}"#);
    let fx = run.take();
    sent_once(&fx, "session/new");
    run.feed(&local("handshake.jsonl")[1]);
    assert_eq!(last_agent(&run.take(), lead()).activity, Activity::Idle);
}

#[test]
fn a_sign_in_error_twice_ends_on_the_sign_in() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.feed(&local("handshake.jsonl")[0]);
    run.feed_all(&local("signed-out.jsonl"));
    run.feed(r#"{"jsonrpc":"2.0","id":"$authenticate","result":{}}"#);
    run.feed_all(&local("signed-out.jsonl"));
    let fx = run.take();
    assert_eq!(sent(&fx, "authenticate").len(), 1);
    let reason = needs_sign_in("Example");
    assert!(
        matches!(&last_agent(&fx, lead()).activity, Activity::Error { reason: said } if *said == reason)
    );
}

#[test]
fn a_sign_in_error_with_no_method_the_agent_runs_ends_on_the_sign_in() {
    let mut run = Run::local(Kept::default(), PermissionMode::Ask);
    run.connect();
    run.act(send(key(1), "Hi", SendMode::Queue));
    run.feed(&local("bare.jsonl")[0]);
    run.feed_all(&local("signed-out.jsonl"));
    let fx = run.take();
    assert!(sent(&fx, "authenticate").is_empty());
    let reason = needs_sign_in("Example");
    assert!(
        matches!(&last_agent(&fx, lead()).activity, Activity::Error { reason: said } if *said == reason)
    );
}

// Images.

#[test]
fn an_image_goes_inline_only_when_the_agent_takes_images() {
    let dir = crate::testing::TempDir::new();
    for (handshake, inline) in [("handshake.jsonl", true), ("bare.jsonl", false)] {
        let mut run = Run::local_ready(handshake, PermissionMode::Ask);
        run.act(send_with(
            key(1),
            "What color?",
            vec![image(dir.path())],
            SendMode::Queue,
        ));
        let fx = run.take();
        let prompt = sent_once(&fx, "session/prompt");
        let blocks = prompt["params"]["prompt"].as_array().unwrap();
        assert_eq!(
            blocks.iter().any(|block| block["type"] == "image"),
            inline,
            "{handshake}"
        );
    }
}

// Permissions.

/// What maxd answered for itself to `fixture`'s request in `mode`, if it
/// didn't ask you.
fn answered(mode: PermissionMode, fixture: &str) -> Option<Value> {
    let mut run = Run::local_turning("Clean up", mode);
    run.feed_all(&local(fixture));
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
fn each_mode_answers_what_it_lets_through() {
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
    assert_eq!(
        answered(PermissionMode::AcceptEdits, "permission-command.jsonl"),
        None
    );
    assert_eq!(
        answered(PermissionMode::FullAccess, "permission.jsonl"),
        yes
    );
    assert_eq!(
        answered(PermissionMode::FullAccess, "permission-command.jsonl"),
        yes
    );
}

#[test]
fn full_access_needs_no_new_process() {
    let mut run = Run::local_ready("handshake.jsonl", PermissionMode::Ask);
    run.act(Action::SetPermission {
        key: key(1),
        agent: lead(),
        mode: PermissionMode::FullAccess,
    });
    let fx = run.take();
    assert_eq!(outcome(&fx, key(1)), ActionOutcome::Done);
    assert!(!fx.stop);
    run.act(send(key(2), "Clean up", SendMode::Queue));
    run.feed_all(&local("permission-command.jsonl"));
    let fx = run.take();
    assert!(opened(&fx).is_empty());
}

#[test]
fn an_unknown_request_is_method_not_found_and_a_notification_is_ignored() {
    let mut run = Run::local_turning("Hi", PermissionMode::Ask);
    run.feed_all(&local("unknown.jsonl"));
    let fx = run.take();
    let reply = answers(&fx).pop().unwrap();
    assert_eq!(reply["id"], 12);
    assert_eq!(reply["error"]["code"], -32601);
    assert_eq!(answers(&fx).len(), 1);
    assert!(opened(&fx).is_empty());
}

// The agent tools.

#[test]
fn an_agent_tools_call_named_only_in_its_title_is_allowed() {
    let mut run = Run::local_turning("List the agents", PermissionMode::Ask);
    run.feed_all(&local("agent-tool.jsonl"));
    let fx = run.take();
    assert!(opened(&fx).is_empty());
    let reply = answers(&fx).pop().expect("an answer");
    assert_eq!(reply["result"]["outcome"]["optionId"], "yes");
    let (server, name, input, status) = last_tool(&fx);
    assert_eq!(server.as_deref(), Some("maxhq"));
    assert_eq!(name, "list_agents");
    assert_eq!(input, serde_json::json!({ "scope": "workspace" }));
    assert_eq!(status, ItemStatus::Completed);
}
