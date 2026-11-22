// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's own tools as dynamic tools: a send that runs off the loop, its
//! answer and its item, a read, a call that lasts through a reconnect,
//! and the files read back from history.

use maxhq_core::excerpt::CONTEXT;
use maxhq_core::tools::{self, Call, Read, Send, Target};

use super::*;
use crate::drive::{ToolCall, ToolDone, ToolResult};

/// Codex's ID for the send call.
const CALL: &str = "exec-6f1c2e0a-5b7d-4c1e-9a3f-2d8b7c6e5a41";

/// The timeline ID of the files it sends.
fn sent_item() -> String {
    format!("codex:{CALL}:sent")
}

/// The send call's arguments.
fn send_input() -> Value {
    json!({"paths": ["out/chart.png"], "message": "The chart you asked for."})
}

/// An `item/tool/call` request with the app-server's ID `id`, for a tool
/// in `namespace`.
fn namespaced_call(id: u64, namespace: Value, tool: &str, arguments: Value) -> String {
    json!({
        "method": "item/tool/call",
        "id": id,
        "params": {"threadId": THREAD, "turnId": TURN_COMMAND, "callId": CALL,
                   "namespace": namespace, "tool": tool, "arguments": arguments},
    })
    .to_string()
}

/// An `item/tool/call` request for one of MaxHQ's tools.
fn tool_call(id: u64, tool: &str, arguments: Value) -> String {
    namespaced_call(id, json!("maxhq"), tool, arguments)
}

/// The send call.
fn send_call() -> String {
    tool_call(7, "send_attachments", send_input())
}

/// The file the send copied.
fn chart() -> AttachmentRef {
    AttachmentRef {
        id: AttachmentId::derive("chart"),
        name: "chart.png".to_owned(),
        kind: AttachmentKind::Image,
        bytes: 2048,
        mime: Some("image/png".to_owned()),
        path: "/tmp/maxhq-test/attachments/chart/chart.png".to_owned(),
        thumbnail: Some("data:image/png;base64,AAAA".to_owned()),
    }
}

/// The send call's item in Codex's camel case, as it finished.
fn finished_call(said: &str, success: bool) -> Value {
    json!({
        "type": "dynamicToolCall",
        "id": CALL,
        "namespace": "maxhq",
        "tool": "send_attachments",
        "arguments": send_input(),
        "status": "completed",
        "contentItems": [{"type": "inputText", "text": said}],
        "success": success,
        "durationMs": 40,
    })
}

/// The answer the one write carries, checked to name the request.
fn tool_answer(fx: &Effects, id: u64) -> Value {
    assert_eq!(fx.writes.len(), 1, "{:#?}", written(fx));
    assert_eq!(
        fx.writes[0].answers.as_deref(),
        Some(id.to_string().as_str())
    );
    let line = &written(fx)[0];
    assert_eq!(line["id"], id);
    line["result"].clone()
}

/// A run in the command turn whose model called the send tool, which now
/// runs.
fn sending() -> Run {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    run.feed(&send_call());
    let fx = run.take();
    assert!(fx.writes.is_empty(), "a call is answered when it ends");
    assert_eq!(
        fx.calls,
        [ToolCall {
            key: CALL.to_owned(),
            call_id: CALL.to_owned(),
            agent: agent(),
            arguments: send_input(),
            call: Call::SendAttachments(Send {
                paths: vec!["out/chart.png".to_owned()],
                message: Some("The chart you asked for.".to_owned()),
                to: Target::Own,
            }),
        }]
    );
    run
}

impl Run {
    fn tool_done(&mut self, result: Result<ToolResult, String>) -> Effects {
        let done = ToolDone {
            key: CALL.to_owned(),
            result,
        };
        self.server.tool_done(done, &mut self.fx);
        self.take()
    }

    /// A reconnect from the holder, which still holds the send call.
    fn reconnect(&mut self) -> Effects {
        let held = [HeldLine {
            seq: 40,
            at: Timestamp(1_792_921_752_000),
            text: send_call(),
            replayed: true,
        }];
        self.server.connected(false, &held, &mut self.fx);
        self.take()
    }
}

#[test]
fn a_send_answers_when_it_ends_and_shows_its_files() {
    let mut run = sending();
    let fx = run.tool_done(Ok(ToolResult::Sent(vec![chart()])));
    let said = tools::sent_text(&[chart()]);
    assert_eq!(
        tool_answer(&fx, 7),
        json!({"contentItems": [{"type": "inputText", "text": said}], "success": true})
    );
    let sent = last(&fx, &sent_item());
    assert_eq!(sent.agent, agent());
    assert_eq!(
        sent.turn.as_ref().map(|turn| turn.0.as_str()),
        Some(TURN_COMMAND)
    );
    assert_eq!(sent.status, ItemStatus::Completed);
    assert_eq!(
        sent.body,
        ItemBody::AgentAttachments {
            text: "The chart you asked for.".to_owned(),
            attachments: vec![chart()],
        }
    );

    // The call's item follows, and the files show once.
    let completed = json!({"threadId": THREAD, "turnId": TURN_COMMAND,
                           "item": finished_call(&said, true), "completedAtMs": 1_792_921_752_100u64});
    run.feed(&notification("item/completed", completed));
    let fx = run.take();
    assert!(matches!(
        last(&fx, &format!("codex:{CALL}")).body,
        ItemBody::ToolCall { .. }
    ));
    assert!(upserts(&fx).iter().all(|item| item.id.0 != sent_item()));
    let fx = run.tool_done(Ok(ToolResult::Sent(vec![chart()])));
    assert!(fx.writes.is_empty(), "a call is answered once");
}

#[test]
fn a_read_answers_with_its_text_and_shows_no_files() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    run.feed(&tool_call(
        8,
        "read_agent",
        json!({"agent": "API work", "limit": 5}),
    ));
    let fx = run.take();
    assert!(fx.writes.is_empty(), "a call is answered when it ends");
    assert_eq!(
        fx.calls,
        [ToolCall {
            key: CALL.to_owned(),
            call_id: CALL.to_owned(),
            agent: agent(),
            arguments: json!({"agent": "API work", "limit": 5}),
            call: Call::ReadAgent(Read {
                agent: "API work".to_owned(),
                before: None,
                limit: 5,
                item: None,
                around: None,
                position: None,
                context: CONTEXT,
                device: None,
            }),
        }]
    );
    let page = "# Agent: API work\n\nThis is the start of its timeline.";
    let fx = run.tool_done(Ok(ToolResult::Text(page.to_owned())));
    assert_eq!(
        tool_answer(&fx, 8),
        json!({"contentItems": [{"type": "inputText", "text": page}], "success": true})
    );
    assert!(upserts(&fx).iter().all(|item| item.id.0 != sent_item()));
}

#[test]
fn a_screenshot_answers_with_its_text_and_an_image_url() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    let lease = maxhq_protocol::ids::LeaseId::derive("lease");
    run.feed(&tool_call(
        9,
        "lease_screenshot",
        json!({"lease": lease.to_string()}),
    ));
    let fx = run.take();
    assert_eq!(fx.calls[0].call.tool(), tools::Tool::LeaseScreenshot);
    let image = maxhq_protocol::agents::ToolImage {
        mime: "image/png".into(),
        data: "iVBORw0K".into(),
    };
    let fx = run.tool_done(Ok(ToolResult::Mixed {
        text: "The screen.".into(),
        images: vec![image],
    }));
    assert_eq!(
        tool_answer(&fx, 9),
        json!({"contentItems": [
            {"type": "inputText", "text": "The screen."},
            {"type": "inputImage", "imageUrl": "data:image/png;base64,iVBORw0K"},
        ], "success": true})
    );
}

#[test]
fn a_send_that_fails_answers_as_a_tool_error() {
    let mut run = sending();
    let reason = "out/chart.png: the path is a folder. Send the files in it instead";
    let fx = run.tool_done(Err(reason.to_owned()));
    assert_eq!(
        tool_answer(&fx, 7),
        json!({"contentItems": [{"type": "inputText", "text": reason}], "success": false})
    );
    assert!(upserts(&fx).is_empty());
}

#[test]
fn bad_calls_fail_at_once_as_tool_errors() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    run.feed(&tool_call(8, "send_attachments", json!({"paths": []})));
    let fx = run.take();
    assert!(fx.calls.is_empty());
    assert_eq!(tool_answer(&fx, 8)["success"], false);

    run.feed(&tool_call(9, "nothing", json!({})));
    let fx = run.take();
    assert!(fx.calls.is_empty());
    let answer = tool_answer(&fx, 9);
    assert_eq!(answer["success"], false);
    assert_eq!(
        answer["contentItems"][0]["text"],
        "MaxHQ has no tool \"nothing\"."
    );

    // Another namespace's tool of the same name isn't ours.
    let call = namespaced_call(10, json!("other"), "send_attachments", send_input());
    run.feed(&call);
    let fx = run.take();
    assert!(fx.calls.is_empty());
    assert_eq!(tool_answer(&fx, 10)["success"], false);

    // Nor is a flat name inside the namespace.
    run.feed(&tool_call(11, "maxhq_send_attachments", send_input()));
    let fx = run.take();
    assert!(fx.calls.is_empty());
    assert_eq!(tool_answer(&fx, 11)["success"], false);
}

#[test]
fn a_thread_started_before_the_namespace_calls_the_old_names() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    run.feed(&namespaced_call(
        7,
        Value::Null,
        "maxhq_send_attachments",
        send_input(),
    ));
    let fx = run.take();
    assert_eq!(fx.calls.len(), 1, "{:#?}", written(&fx));
    assert_eq!(fx.calls[0].call.tool(), tools::Tool::SendAttachments);

    // Without a namespace, a name has to carry the prefix.
    run.feed(&namespaced_call(
        8,
        Value::Null,
        "send_attachments",
        send_input(),
    ));
    let fx = run.take();
    assert!(fx.calls.is_empty());
    assert_eq!(tool_answer(&fx, 8)["success"], false);
}

#[test]
fn an_old_threads_call_shows_as_maxhqs_own_tool() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    let input = json!({"agent": "API work", "limit": 5});
    for (namespace, tool) in [
        (Value::Null, "maxhq_read_agent"),
        (json!("maxhq"), "read_agent"),
    ] {
        let item = json!({
            "type": "dynamicToolCall",
            "id": "exec-read",
            "namespace": namespace,
            "tool": tool,
            "arguments": input,
            "status": "completed",
            "contentItems": [{"type": "inputText", "text": "API work: idle"}],
            "success": true,
            "durationMs": 12,
        });
        run.feed(&notification(
            "item/completed",
            json!({"threadId": THREAD, "turnId": TURN_COMMAND, "item": item}),
        ));
        let fx = run.take();
        let ItemBody::ToolCall { server, name, .. } = &last(&fx, "codex:exec-read").body else {
            panic!("{:#?}", last(&fx, "codex:exec-read").body);
        };
        assert_eq!(server.as_deref(), Some("maxhq"), "{tool}");
        assert_eq!(name, "read_agent", "{tool}");
    }

    // Another namespace's tool keeps its own names, and so does a flat
    // name MaxHQ never gave.
    for (namespace, tool, shown) in [
        (json!("app"), "maxhq_read_agent", Some("app")),
        (Value::Null, "maxhq_nothing", None),
        (Value::Null, "read_agent", None),
    ] {
        let item = json!({
            "type": "dynamicToolCall",
            "id": "exec-other",
            "namespace": namespace,
            "tool": tool,
            "arguments": {},
            "status": "completed",
            "contentItems": [],
            "success": true,
        });
        run.feed(&notification(
            "item/completed",
            json!({"threadId": THREAD, "turnId": TURN_COMMAND, "item": item}),
        ));
        let fx = run.take();
        let ItemBody::ToolCall { server, name, .. } = &last(&fx, "codex:exec-other").body else {
            panic!("{:#?}", last(&fx, "codex:exec-other").body);
        };
        assert_eq!(server.as_deref(), shown, "{tool}");
        assert_eq!(name, tool);
    }
}

#[test]
fn a_call_still_running_is_left_to_finish_across_a_reconnect() {
    let mut run = sending();
    let fx = run.reconnect();
    assert!(fx.calls.is_empty(), "it isn't run twice");
    assert!(fx.writes.is_empty());
    let fx = run.tool_done(Ok(ToolResult::Sent(vec![chart()])));
    assert_eq!(tool_answer(&fx, 7)["success"], true);
}

#[test]
fn a_call_whose_answer_was_lost_runs_again_with_the_same_id() {
    let mut run = sending();
    run.tool_done(Ok(ToolResult::Sent(vec![chart()])));
    // The answer went while no one held the pipe, so the holder still
    // waits on the request.
    let fx = run.reconnect();
    assert_eq!(fx.calls.len(), 1);
    assert_eq!(fx.calls[0].call_id, CALL);
    let fx = run.tool_done(Ok(ToolResult::Sent(vec![chart()])));
    assert_eq!(tool_answer(&fx, 7)["success"], true);
    assert_eq!(last(&fx, &sent_item()).id.0, sent_item());
}

#[test]
fn calls_end_with_the_app_server() {
    let mut run = sending();
    run.server.exited(
        &ChildExit {
            code: Some(1),
            signal: None,
            at: Timestamp(1_792_921_760_000),
            stderr_tail: String::new(),
            with_machine: false,
        },
        &mut run.fx,
    );
    run.take();
    let fx = run.tool_done(Ok(ToolResult::Sent(vec![chart()])));
    assert!(fx.writes.is_empty(), "a new app-server never asked");
    assert!(upserts(&fx).iter().all(|item| item.id.0 != sent_item()));
}

#[test]
fn a_send_seen_only_as_its_finished_item_shows_its_files() {
    // As when the holder replays a turn to a new `maxd`.
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    let said = tools::sent_text(&[chart()]);
    let completed = json!({"threadId": THREAD, "turnId": TURN_COMMAND,
                           "item": finished_call(&said, true), "completedAtMs": 1_792_921_752_100u64});
    run.feed_as(&notification("item/completed", completed), true);
    let fx = run.take();
    let mut shown = chart();
    shown.thumbnail = None;
    assert_eq!(
        last(&fx, &sent_item()).body,
        ItemBody::AgentAttachments {
            text: "The chart you asked for.".to_owned(),
            attachments: vec![shown],
        }
    );

    // One that failed shows nothing.
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    let completed = json!({"threadId": THREAD, "turnId": TURN_COMMAND,
                           "item": finished_call("No such file.", false), "completedAtMs": 1_792_921_752_100u64});
    run.feed(&notification("item/completed", completed));
    assert!(upserts(&run.take())
        .iter()
        .all(|item| item.id.0 != sent_item()));
}

#[test]
fn a_send_read_back_from_history_shows_its_files() {
    let said = tools::sent_text(&[chart()]);
    let mut shown = chart();
    shown.thumbnail = None;
    let expected = ItemBody::AgentAttachments {
        text: "The chart you asked for.".to_owned(),
        attachments: vec![shown],
    };

    // From a rollout, in Codex's own shape: of a thread started before
    // the tools had a namespace, and of one started since.
    for (namespace, tool) in [
        (Value::Null, "maxhq_send_attachments"),
        (json!("maxhq"), "send_attachments"),
    ] {
        let home = TempDir::new();
        let path = home.path().join("rollout.jsonl");
        let item = json!({
            "type": "DynamicToolCall",
            "id": CALL,
            "namespace": namespace,
            "tool": tool,
            "arguments": send_input(),
            "status": "completed",
            "content_items": [{"type": "inputText", "text": said}],
            "success": true,
        });
        let lines = [
            json!({"timestamp": "2026-10-30T10:00:00.000Z", "type": "event_msg",
                   "payload": {"type": "task_started", "turn_id": "t1", "started_at": 1_792_921_750u64}}),
            json!({"timestamp": "2026-10-30T10:00:01.000Z", "type": "event_msg",
                   "payload": {"type": "item_completed", "thread_id": THREAD, "turn_id": "t1",
                               "item": item, "started_at_ms": 1_792_921_751_000u64,
                               "completed_at_ms": 1_792_921_751_040u64}}),
        ];
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&path, format!("{}\n", text.join("\n"))).unwrap();
        let read = History::new(path, agent()).read().unwrap();
        let sent = read
            .iter()
            .find(|item| item.id.0 == sent_item())
            .expect("the files show");
        assert_eq!(sent.body, expected, "{tool}");
        assert_eq!(sent.completed_at, Some(Timestamp(1_792_921_751_040)));
    }

    // From a turn the app-server lists.
    let turn = json!({"id": "t1", "status": "completed", "startedAt": 1_792_921_750u64,
                      "completedAt": 1_792_921_752u64, "items": [finished_call(&said, true)]});
    let listed = items::turn(agent(), None, &turn, false, None, &mut Default::default());
    let sent = listed
        .iter()
        .find(|item| item.id.0 == sent_item())
        .expect("the files show");
    assert_eq!(sent.body, expected);
}

#[test]
fn an_open_answers_when_it_ends_and_shows_the_file() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    let input = json!({"path": "docs/plan.md", "line": 12});
    run.feed(&tool_call(9, "open_file", input.clone()));
    let fx = run.take();
    assert!(fx.writes.is_empty(), "a call is answered when it ends");
    assert_eq!(fx.calls.len(), 1);
    assert_eq!(
        fx.calls[0].call,
        Call::OpenFile(tools::Open {
            path: "docs/plan.md".to_owned(),
            line: Some(12),
        })
    );
    let opened = tools::Opened {
        path: "/tmp/example/docs/plan.md".to_owned(),
        line: Some(12),
        outside: true,
    };
    let fx = run.tool_done(Ok(ToolResult::Opened(opened.clone())));
    let said = tools::opened_text(&opened);
    assert_eq!(
        tool_answer(&fx, 9),
        json!({"contentItems": [{"type": "inputText", "text": said}], "success": true})
    );
    let id = format!("codex:{CALL}:opened");
    let shown = last(&fx, &id);
    assert_eq!(shown.agent, agent());
    assert_eq!(
        shown.turn.as_ref().map(|turn| turn.0.as_str()),
        Some(TURN_COMMAND)
    );
    let body = ItemBody::AgentOpenedFile {
        path: "/tmp/example/docs/plan.md".to_owned(),
        line: Some(12),
        outside: true,
    };
    assert_eq!(shown.body, body);

    // The call's item follows, and the file shows once.
    let finished = json!({
        "type": "dynamicToolCall",
        "id": CALL,
        "namespace": "maxhq",
        "tool": "open_file",
        "arguments": input,
        "status": "completed",
        "contentItems": [{"type": "inputText", "text": said}],
        "success": true,
        "durationMs": 4,
    });
    let completed = json!({"threadId": THREAD, "turnId": TURN_COMMAND,
                           "item": finished.clone(), "completedAtMs": 1_792_921_752_100u64});
    run.feed(&notification("item/completed", completed));
    assert!(upserts(&run.take()).iter().all(|item| item.id.0 != id));

    // A turn the app-server lists shows it from the item alone.
    let turn = json!({"id": "t1", "status": "completed", "startedAt": 1_792_921_750u64,
                      "completedAt": 1_792_921_752u64, "items": [finished]});
    let listed = items::turn(agent(), None, &turn, false, None, &mut Default::default());
    let shown = listed
        .iter()
        .find(|item| item.id.0 == id)
        .expect("the file shows");
    assert_eq!(shown.body, body);
}

#[test]
fn a_show_answers_with_its_id_and_shows_the_visual() {
    let mut run = Run::started(None);
    run.feed_all(&fixture_lines("turn-command.jsonl", 1, 3));
    run.take();
    let input = json!({"title": "Weekly usage", "html": "<p>hi</p>"});
    run.feed(&tool_call(9, "show_visual", input.clone()));
    let fx = run.take();
    assert!(fx.writes.is_empty(), "a call is answered when it ends");
    assert_eq!(fx.calls.len(), 1);
    assert!(matches!(&fx.calls[0].call, Call::ShowVisual(show) if show.title == "Weekly usage"));
    let visual = tools::Visual {
        title: "Weekly usage".to_owned(),
        attachment: AttachmentRef {
            id: AttachmentId::derive("visual"),
            name: "visual-weekly-usage.html".to_owned(),
            kind: AttachmentKind::File,
            bytes: 9,
            mime: Some("text/html".to_owned()),
            path: "/tmp/maxhq-test/attachments/visual/visual-weekly-usage.html".to_owned(),
            thumbnail: None,
        },
        replaces: None,
        from_file: false,
        stretches: true,
    };
    let fx = run.tool_done(Ok(ToolResult::Visual(visual.clone())));
    let id = format!("codex:{CALL}:visual");
    let said = tools::visual_text(&ItemId(id.clone()), &visual);
    assert_eq!(
        tool_answer(&fx, 9),
        json!({"contentItems": [{"type": "inputText", "text": said}], "success": true})
    );
    let shown = last(&fx, &id);
    assert_eq!(shown.agent, agent());
    let body = ItemBody::AgentVisual {
        title: "Weekly usage".to_owned(),
        attachment: visual.attachment.clone(),
        replaces: None,
    };
    assert_eq!(shown.body, body);

    // The call's item follows, and the page shows once.
    let finished = json!({
        "type": "dynamicToolCall",
        "id": CALL,
        "namespace": "maxhq",
        "tool": "show_visual",
        "arguments": input,
        "status": "completed",
        "contentItems": [{"type": "inputText", "text": said}],
        "success": true,
        "durationMs": 4,
    });
    let completed = json!({"threadId": THREAD, "turnId": TURN_COMMAND,
                           "item": finished.clone(), "completedAtMs": 1_792_921_752_100u64});
    run.feed(&notification("item/completed", completed));
    assert!(upserts(&run.take()).iter().all(|item| item.id.0 != id));

    // A turn the app-server lists shows it from the item alone.
    let turn = json!({"id": "t1", "status": "completed", "startedAt": 1_792_921_750u64,
                      "completedAt": 1_792_921_752u64, "items": [finished]});
    let listed = items::turn(agent(), None, &turn, false, None, &mut Default::default());
    let shown = listed
        .iter()
        .find(|item| item.id.0 == id)
        .expect("the page shows");
    assert_eq!(shown.body, body);
}
