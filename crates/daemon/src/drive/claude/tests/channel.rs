// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's own tools over the control channel: the MCP handshake, a send
//! that runs off the loop, its answer and its item, a read, and a call
//! that lasts through a reconnect.

use maxhq_core::excerpt::{CONTEXT, PAGE};
use maxhq_core::tools::{self, Call, Read, Send, Target, Tool};

use crate::drive::ToolCall;
use maxhq_protocol::timeline::ItemId;

use super::*;

/// The send tool as the CLI names it.
const SEND: &str = "mcp__maxhq__send_attachments";

/// The send call's `tool_use` ID.
const CALL: &str = "toolu_send";

/// The CLI's request that carries the call.
const REQUEST: &str = "req-send";

/// A run in the middle of its first turn, before any tool call.
fn mid_turn() -> Run {
    let mut run = Run::new();
    run.play(0..=14);
    run
}

/// An `mcp_message` request for MaxHQ's server.
fn mcp_message(request: &str, message: Value) -> Value {
    json!({
        "type": "control_request",
        "request_id": request,
        "request": {"subtype": "mcp_message", "server_name": "maxhq", "message": message},
    })
}

/// The send call's arguments.
fn send_input() -> Value {
    json!({"paths": ["out/chart.png"], "message": "The chart you asked for."})
}

/// The `tools/call` for the send call.
fn send_call() -> Value {
    mcp_message(
        REQUEST,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "send_attachments",
                "arguments": send_input(),
                "_meta": {"claudecode/toolUseId": CALL, "progressToken": 4},
            },
        }),
    )
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

/// The MCP answer a write carries.
fn mcp_answer(fx: &Effects) -> Value {
    assert_eq!(fx.writes.len(), 1, "{:?}", fx.writes);
    // The holder reads which request it answers from the line.
    assert_eq!(fx.writes[0].answers, None);
    answer_body(fx)["mcp_response"].clone()
}

/// A run whose model called the send tool, which now runs.
fn sending() -> Run {
    let mut run = mid_turn();
    run.feed(tool_use(CALL, SEND, send_input()));
    let fx = run.feed(send_call());
    assert!(fx.writes.is_empty(), "a call is answered when it ends");
    assert_eq!(
        fx.calls,
        [ToolCall {
            key: REQUEST.to_owned(),
            call_id: CALL.to_owned(),
            agent: lead(),
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

#[test]
fn the_handshake_and_the_tool_list_are_answered_at_once() {
    let mut run = mid_turn();
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                   "clientInfo": {"name": "claude-code", "version": "2.1.314"}},
    });
    let fx = run.feed(mcp_message("req-init", initialize));
    let answer = mcp_answer(&fx);
    assert_eq!(answer["id"], 0);
    assert_eq!(answer["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(answer["result"]["serverInfo"]["name"], "maxhq");

    let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let fx = run.feed(mcp_message("req-note", note));
    assert_eq!(
        mcp_answer(&fx),
        json!({"jsonrpc": "2.0", "result": {}, "id": 0})
    );

    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let fx = run.feed(mcp_message("req-list", list));
    let answer = mcp_answer(&fx);
    assert_eq!(answer["id"], 1);
    assert_eq!(
        answer["result"]["tools"][0]["name"],
        tools::Tool::SendAttachments.name()
    );
    assert_eq!(
        answer["result"]["tools"][1]["name"],
        tools::Tool::OpenFile.name()
    );
    assert_eq!(
        answer["result"]["tools"][2]["name"],
        tools::Tool::ShowVisual.name()
    );
    assert_eq!(
        answer["result"]["tools"][3]["name"],
        tools::Tool::ReadAgent.name()
    );
    assert!(fx.calls.is_empty());
    assert!(fx.attention.is_empty());
}

#[test]
fn a_cli_held_across_a_restart_reads_the_tool_list_again() {
    // The CLI read its tools from the `maxd` before, which may have had
    // fewer, and a repeat `initialize` doesn't make it read them again.
    let mut run = mid_turn();
    let fx = run.connect(false, &[]);
    assert_eq!(requests(&fx), [bare_initialize(), tools_changed()]);
    assert_eq!(fx.writes[1].answers, None);
    let notice: Value = serde_json::from_str(&fx.writes[1].line).expect("a JSON line");

    let list = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"});
    let fx = run.feed(mcp_message("req-relist", list));
    let answer = mcp_answer(&fx);
    assert_eq!(answer["id"], 7);
    let names: Vec<&str> = answer["result"]["tools"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    let all: Vec<&str> = Tool::ALL.iter().map(|tool| tool.name()).collect();
    assert_eq!(names, all);

    // The CLI's reply needs nothing more, and an older CLI's refusal is
    // no trouble either.
    let fx = run.reply(&notice, json!({}));
    assert!(fx.writes.is_empty() && fx.results.is_empty());
    let fx = run.connect(false, &[]);
    let notice: Value = serde_json::from_str(&fx.writes[1].line).expect("a JSON line");
    let fx = run.reply_error(&notice, "Unsupported control request subtype: mcp_message");
    assert!(fx.writes.is_empty() && fx.results.is_empty());

    // A CLI `maxd` just started reads them in its handshake.
    let mut run = Run::new();
    let fx = run.connect(true, &[]);
    assert_eq!(requests(&fx).len(), 1);
    assert_eq!(requests(&fx)[0]["subtype"], "initialize");
}

#[test]
fn maxds_own_tools_go_ahead_without_asking() {
    let mut run = mid_turn();
    run.feed(tool_use(CALL, SEND, send_input()));
    let mut request = can_use_tool("req-allow", CALL, SEND, send_input(), json!([]));
    request["request"]["mcp_server"] = json!({"name": "maxhq", "source": "sdk"});
    let fx = run.feed(request);
    assert_eq!(
        answer_body(&fx),
        json!({"behavior": "allow", "updatedInput": send_input(), "toolUseID": CALL})
    );
    assert_eq!(fx.writes[0].answers.as_deref(), Some("req-allow"));
    assert!(fx.attention.is_empty());

    // A server of the user's own with a tool of the same name still asks.
    let other = "mcp__maxhq__send_attachments";
    let mut request = can_use_tool("req-other", "toolu_other", other, json!({}), json!([]));
    request["request"]["mcp_server"] = json!({"name": "maxhq", "source": "user"});
    let fx = run.feed(request);
    assert_eq!(opened(&fx).len(), 1);
}

#[test]
fn a_send_answers_when_it_ends_and_shows_its_files() {
    let mut run = sending();
    let fx = run.tool_done(REQUEST, Ok(ToolResult::Sent(vec![chart()])));
    let answer = mcp_answer(&fx);
    assert_eq!(answer["id"], 4);
    assert!(answer["result"].get("isError").is_none());
    let said = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert_eq!(said, tools::sent_text(&[chart()]));
    let sent = run.screen.item("claude:toolu_send:sent");
    assert_eq!(sent.agent, lead());
    assert_eq!(sent.parent, None);
    assert_eq!(
        sent.body,
        ItemBody::AgentAttachments {
            text: "The chart you asked for.".to_owned(),
            attachments: vec![chart()],
        }
    );

    // The call's result follows, and shows the files once.
    let fx = run.feed(tool_result(CALL, said, Value::Null));
    assert!(upserts(&fx, "claude:toolu_send:sent").is_empty());
    let fx = run.tool_done(REQUEST, Ok(ToolResult::Sent(vec![chart()])));
    assert!(fx.writes.is_empty(), "a call is answered once");
}

#[test]
fn an_open_answers_when_it_ends_and_shows_the_file() {
    let mut run = mid_turn();
    let input = json!({"path": "docs/plan.md", "line": 12});
    run.feed(tool_use(
        "toolu_open",
        "mcp__maxhq__open_file",
        input.clone(),
    ));
    let call = mcp_message(
        "req-open",
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "open_file",
                "arguments": input,
                "_meta": {"claudecode/toolUseId": "toolu_open"},
            },
        }),
    );
    let fx = run.feed(call);
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
        outside: false,
    };
    let fx = run.tool_done("req-open", Ok(ToolResult::Opened(opened.clone())));
    let answer = mcp_answer(&fx);
    assert!(answer["result"].get("isError").is_none());
    let said = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned();
    assert_eq!(said, tools::opened_text(&opened));
    let shown = run.screen.item("claude:toolu_open:opened");
    assert_eq!(shown.agent, lead());
    let body = ItemBody::AgentOpenedFile {
        path: "/tmp/example/docs/plan.md".to_owned(),
        line: Some(12),
        outside: false,
    };
    assert_eq!(shown.body, body);

    // The call's result follows, and shows the file once.
    let fx = run.feed(tool_result("toolu_open", &said, Value::Null));
    assert!(upserts(&fx, "claude:toolu_open:opened").is_empty());

    // A run read back from history shows it from the result alone.
    let mut again = mid_turn();
    again.feed(tool_use(
        "toolu_open",
        "mcp__maxhq__open_file",
        json!({"path": "docs/plan.md", "line": 12}),
    ));
    again.feed(tool_result("toolu_open", &said, Value::Null));
    assert_eq!(again.screen.item("claude:toolu_open:opened").body, body);
}

/// The page a show call showed.
fn shown_page(replaces: &str) -> tools::Visual {
    tools::Visual {
        title: "Weekly usage".to_owned(),
        attachment: AttachmentRef {
            id: maxhq_protocol::ids::AttachmentId::derive("visual"),
            name: "visual-weekly-usage.html".to_owned(),
            kind: maxhq_protocol::timeline::AttachmentKind::File,
            bytes: 900,
            mime: Some("text/html".to_owned()),
            path: "/tmp/example/attachments/visual-weekly-usage.html".to_owned(),
            thumbnail: None,
        },
        replaces: Some(ItemId(replaces.to_owned())),
        from_file: true,
        stretches: false,
    }
}

#[test]
fn a_show_answers_with_its_id_and_shows_the_visual() {
    let mut run = mid_turn();
    let input = json!({
        "title": "Weekly usage",
        "path": "out/usage.html",
        "replaces": "claude:toolu_old:visual",
    });
    run.feed(tool_use(
        "toolu_show",
        "mcp__maxhq__show_visual",
        input.clone(),
    ));
    let call = mcp_message(
        "req-show",
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "show_visual",
                "arguments": input,
                "_meta": {"claudecode/toolUseId": "toolu_show"},
            },
        }),
    );
    let fx = run.feed(call);
    assert_eq!(fx.calls.len(), 1);
    assert!(matches!(&fx.calls[0].call, Call::ShowVisual(show) if show.title == "Weekly usage"));
    let visual = shown_page("claude:toolu_old:visual");
    let fx = run.tool_done("req-show", Ok(ToolResult::Visual(visual.clone())));
    let answer = mcp_answer(&fx);
    assert!(answer["result"].get("isError").is_none());
    let said = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned();
    let id = ItemId("claude:toolu_show:visual".to_owned());
    assert_eq!(said, tools::visual_text(&id, &visual));
    let body = ItemBody::AgentVisual {
        title: visual.title.clone(),
        attachment: visual.attachment.clone(),
        replaces: visual.replaces.clone(),
    };
    let shown = run.screen.item("claude:toolu_show:visual");
    assert_eq!(shown.agent, lead());
    assert_eq!(shown.body, body);

    // The call's result follows, and shows the page once.
    let fx = run.feed(tool_result("toolu_show", &said, Value::Null));
    assert!(upserts(&fx, "claude:toolu_show:visual").is_empty());

    // A run read back from history shows it from the result alone.
    let mut again = mid_turn();
    again.feed(tool_use(
        "toolu_show",
        "mcp__maxhq__show_visual",
        json!({"title": "Weekly usage", "html": "<p>hi</p>"}),
    ));
    again.feed(tool_result("toolu_show", &said, Value::Null));
    assert_eq!(again.screen.item("claude:toolu_show:visual").body, body);
}

#[test]
fn a_read_answers_with_its_text_and_shows_no_files() {
    let mut run = mid_turn();
    let input = json!({"agent": "API work", "before": 12});
    run.feed(tool_use(
        "toolu_read",
        "mcp__maxhq__read_agent",
        input.clone(),
    ));
    let call = mcp_message(
        "req-read",
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "read_agent",
                "arguments": input,
                "_meta": {"claudecode/toolUseId": "toolu_read"},
            },
        }),
    );
    let fx = run.feed(call);
    assert_eq!(
        fx.calls,
        [ToolCall {
            key: "req-read".to_owned(),
            call_id: "toolu_read".to_owned(),
            agent: lead(),
            arguments: json!({"agent": "API work", "before": 12}),
            call: Call::ReadAgent(Read {
                agent: "API work".to_owned(),
                before: Some(12),
                limit: PAGE,
                item: None,
                around: None,
                position: None,
                context: CONTEXT,
                device: None,
            }),
        }]
    );
    let page = "# Agent: API work\n\nThis is the start of its timeline.";
    let fx = run.tool_done("req-read", Ok(ToolResult::Text(page.to_owned())));
    let answer = mcp_answer(&fx);
    assert_eq!(answer["id"], 5);
    assert!(answer["result"].get("isError").is_none());
    assert_eq!(answer["result"]["content"][0]["text"], page);
    assert!(run.screen.find("claude:toolu_read:sent").is_none());
}

#[test]
fn a_screenshot_answers_with_its_text_and_an_image() {
    let mut run = mid_turn();
    let input = json!({"lease": maxhq_protocol::ids::LeaseId::derive("lease").to_string()});
    run.feed(tool_use(
        "toolu_shot",
        "mcp__maxhq__lease_screenshot",
        input.clone(),
    ));
    let call = mcp_message(
        "req-shot",
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
               "params": {"name": "lease_screenshot", "arguments": input}}),
    );
    let fx = run.feed(call);
    assert_eq!(fx.calls[0].call.tool(), Tool::LeaseScreenshot);
    let image = maxhq_protocol::agents::ToolImage {
        mime: "image/png".into(),
        data: "iVBORw0K".into(),
    };
    let result = ToolResult::Mixed {
        text: "The screen.".into(),
        images: vec![image],
    };
    let answer = mcp_answer(&run.tool_done("req-shot", Ok(result)));
    assert_eq!(
        answer["result"]["content"],
        json!([{"type": "text", "text": "The screen."},
               {"type": "image", "data": "iVBORw0K", "mimeType": "image/png"}])
    );
}

#[test]
fn a_send_that_fails_answers_as_a_tool_error() {
    let mut run = sending();
    let reason = "out/chart.png: the path is a folder. Send the files in it instead";
    let fx = run.tool_done(REQUEST, Err(reason.to_owned()));
    let answer = mcp_answer(&fx);
    assert_eq!(answer["result"]["isError"], true);
    assert_eq!(answer["result"]["content"][0]["text"], reason);
    assert!(run.screen.find("claude:toolu_send:sent").is_none());
}

#[test]
fn bad_arguments_fail_at_once_as_a_tool_error() {
    let mut run = mid_turn();
    let call = mcp_message(
        "req-bad",
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
               "params": {"name": "send_attachments", "arguments": {"paths": []}}}),
    );
    let fx = run.feed(call);
    assert!(fx.calls.is_empty());
    let answer = mcp_answer(&fx);
    assert_eq!(answer["result"]["isError"], true);
}

#[test]
fn another_server_is_turned_down() {
    let mut run = mid_turn();
    let mut request = mcp_message("req-elsewhere", json!({"jsonrpc": "2.0", "id": 0}));
    request["request"]["server_name"] = json!("github");
    let fx = run.feed(request);
    assert_eq!(fx.writes.len(), 1);
    assert_eq!(fx.writes[0].answers, None);
    assert_eq!(
        only_write(&fx)["response"]["error"],
        "MaxHQ has no MCP server \"github\"."
    );
}

/// The send call as the holder gives it back on a reconnect.
fn held_call() -> HeldLine {
    HeldLine {
        seq: 1,
        at: Timestamp(1),
        text: send_call().to_string(),
        replayed: true,
    }
}

#[test]
fn a_call_still_running_is_left_to_finish_across_a_reconnect() {
    let mut run = sending();
    let mut fx = Effects::default();
    run.driver.connected(false, &[held_call()], &mut fx);
    run.screen.apply(&fx);
    assert!(fx.calls.is_empty(), "it isn't run twice");
    assert_eq!(requests(&fx), [bare_initialize(), tools_changed()]);
    let fx = run.tool_done(REQUEST, Ok(ToolResult::Sent(vec![chart()])));
    assert_eq!(mcp_answer(&fx)["id"], 4);
}

#[test]
fn a_call_whose_answer_was_lost_runs_again_with_the_same_id() {
    let mut run = sending();
    run.tool_done(REQUEST, Ok(ToolResult::Sent(vec![chart()])));
    // The answer went while no one held the pipe, so the holder still
    // waits on the request.
    let mut fx = Effects::default();
    run.driver.connected(false, &[held_call()], &mut fx);
    run.screen.apply(&fx);
    assert_eq!(fx.calls.len(), 1);
    assert_eq!(fx.calls[0].call_id, CALL);
    let fx = run.tool_done(REQUEST, Ok(ToolResult::Sent(vec![chart()])));
    assert_eq!(mcp_answer(&fx)["id"], 4);
    assert!(upserts(&fx, "claude:toolu_send:sent").is_empty());
}

#[test]
fn a_withdrawn_call_isnt_answered_or_shown() {
    let mut run = sending();
    run.feed(json!({"type": "control_cancel_request", "request_id": REQUEST}));
    let fx = run.tool_done(REQUEST, Ok(ToolResult::Sent(vec![chart()])));
    assert!(fx.writes.is_empty());
    assert!(run.screen.find("claude:toolu_send:sent").is_none());
}

#[test]
fn a_send_read_back_from_its_result_shows_its_files() {
    // As after a restart: the call and its result, with no call running.
    let mut run = mid_turn();
    run.feed(tool_use(CALL, SEND, send_input()));
    let said = tools::sent_text(&[chart()]);
    run.feed(tool_result(CALL, &said, Value::Null));
    let sent = run.screen.item("claude:toolu_send:sent");
    let mut shown = chart();
    shown.thumbnail = None;
    assert_eq!(
        sent.body,
        ItemBody::AgentAttachments {
            text: "The chart you asked for.".to_owned(),
            attachments: vec![shown],
        }
    );
}
