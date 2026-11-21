// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Turns past what the recording shows: each kind of tool call, the send
//! modes, sub-agents, background commands, limits, errors and the CLI
//! ending.

use std::sync::atomic::Ordering;
use std::time::Instant;

use maxhq_core::names;
use maxhq_protocol::message::{ActionOutcome, SendMode};
use maxhq_protocol::model::{Activity, AgentLink, PermissionMode};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    Delivery, ItemBody, ItemId, ItemStatus, NoticeLevel, TimelineEvent, Todo, TodoState, WebAction,
};
use serde_json::{json, Value};

use super::*;

/// A run in the middle of its first turn, before any tool call.
fn mid_turn() -> Run {
    let mut run = Run::new();
    run.play(0..=14);
    run
}

/// A run in the middle of the turn the interrupt cuts short.
fn counting() -> Run {
    let mut run = Run::new();
    run.play(0..=217);
    run
}

/// Runs the driver's timer.
fn tick(run: &mut Run) -> Effects {
    let mut fx = Effects::default();
    run.driver.tick(Instant::now(), &mut fx);
    run.screen.apply(&fx);
    fx
}

#[test]
fn each_kind_of_tool_call_shows_as_its_own_item() {
    let mut run = mid_turn();
    let todos = json!([
        {"content": "Write the tests", "status": "completed", "activeForm": "Writing the tests"},
        {"content": "Run them", "status": "in_progress", "activeForm": "Running them"},
        {"content": "Ship it", "status": "pending", "activeForm": "Shipping it"},
    ]);
    let file = json!({"filePath": format!("{CWD}/big.rs"), "content": "", "numLines": 20, "startLine": 10, "totalLines": 400});
    let calls = [
        (
            "toolu_grep",
            "Grep",
            json!({"pattern": "TODO", "path": "src"}),
            "Found 3 files",
            json!({"mode": "files_with_matches", "numFiles": 3, "filenames": ["a", "b", "c"]}),
        ),
        (
            "toolu_glob",
            "Glob",
            json!({"pattern": "**/*.rs", "path": "crates"}),
            "a.rs\nb.rs",
            json!({"numFiles": 2, "filenames": ["a.rs", "b.rs"], "truncated": false}),
        ),
        (
            "toolu_read",
            "Read",
            json!({"file_path": format!("{CWD}/big.rs"), "offset": 10, "limit": 20}),
            "fn main() {}",
            json!({"type": "text", "file": file}),
        ),
        (
            "toolu_fetch",
            "WebFetch",
            json!({"url": "https://example.com/docs", "prompt": "Sum it up"}),
            "The docs say hello.",
            json!({"code": 200, "result": "The docs say hello."}),
        ),
        (
            "toolu_web",
            "WebSearch",
            json!({"query": "serde flatten"}),
            "Results",
            json!({"query": "serde flatten", "results": []}),
        ),
        (
            "toolu_mcp",
            "mcp__github__search_issues",
            json!({"query": "is:open label:bug"}),
            "2 issues",
            json!([{"type": "text", "text": "2 issues"}]),
        ),
        (
            "toolu_todo",
            "TodoWrite",
            json!({"todos": todos}),
            "Todos have been modified successfully.",
            json!({"oldTodos": [], "newTodos": todos}),
        ),
        (
            "toolu_skill",
            "Skill",
            json!({"skill": "pdf"}),
            "Launching skill: pdf",
            json!({"success": true, "commandName": "pdf"}),
        ),
    ];
    for (call, name, input, content, outcome) in calls.clone() {
        run.feed(tool_use(call, name, input));
        assert_eq!(
            run.screen.item(&format!("claude:{call}")).status,
            ItemStatus::Running
        );
        run.feed(tool_result(call, content, outcome));
    }
    let body = |id: &str| run.screen.item(&format!("claude:{id}")).body.clone();
    assert_eq!(
        body("toolu_grep"),
        ItemBody::Search {
            query: "TODO".to_owned(),
            scope: Some("src".to_owned()),
            matches: Some(3),
        }
    );
    assert_eq!(
        body("toolu_glob"),
        ItemBody::Search {
            query: "**/*.rs".to_owned(),
            scope: Some("crates".to_owned()),
            matches: Some(2),
        }
    );
    assert_eq!(
        body("toolu_read"),
        ItemBody::FileRead {
            path: format!("{CWD}/big.rs"),
            lines: Some("10-29".to_owned()),
            images: Vec::new(),
        }
    );
    assert_eq!(
        body("toolu_fetch"),
        ItemBody::Web {
            action: WebAction::Fetch,
            target: "https://example.com/docs".to_owned(),
        }
    );
    assert_eq!(
        body("toolu_web"),
        ItemBody::Web {
            action: WebAction::Search,
            target: "serde flatten".to_owned(),
        }
    );
    assert_eq!(
        body("toolu_mcp"),
        ItemBody::ToolCall {
            server: Some("github".to_owned()),
            name: "search_issues".to_owned(),
            title: Some("is:open label:bug".to_owned()),
            input: Some(r#"{"query":"is:open label:bug"}"#.to_owned()),
            output: Some("2 issues".to_owned()),
            images: Vec::new(),
            artifact: None,
        }
    );
    let todo = |text: &str, state| Todo {
        text: text.to_owned(),
        state,
    };
    assert_eq!(
        body("toolu_todo"),
        ItemBody::Todos {
            todos: vec![
                todo("Write the tests", TodoState::Done),
                todo("Run them", TodoState::InProgress),
                todo("Ship it", TodoState::Pending),
            ],
        }
    );
    assert_eq!(
        body("toolu_skill"),
        ItemBody::ToolCall {
            server: None,
            name: "Skill".to_owned(),
            title: Some("pdf".to_owned()),
            input: Some(r#"{"skill":"pdf"}"#.to_owned()),
            output: Some("Launching skill: pdf".to_owned()),
            images: Vec::new(),
            artifact: None,
        }
    );
    for (call, ..) in calls {
        let item = run.screen.item(&format!("claude:{call}"));
        assert_eq!(item.status, ItemStatus::Completed, "{call}");
        assert_eq!(item.turn, run.turn(FIRST), "{call}");
        assert_eq!((item.agent, item.parent.clone()), (lead(), None), "{call}");
    }
}

/// A tiny PNG.
fn png() -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(3, 2, image::Rgb([20, 120, 220]));
    let mut out = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("a PNG");
    out.into_inner()
}

/// A `user` line with a tool's result that holds `blocks`.
fn tool_result_blocks(call: &str, blocks: Value, result: Value) -> Value {
    line(json!({
        "type": "user",
        "parent_tool_use_id": null,
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": call,
                "content": blocks,
                "is_error": false,
            }],
        },
        "tool_use_result": result,
    }))
}

#[test]
fn a_picture_in_a_tool_result_is_kept_and_shown_on_its_item() {
    use base64::Engine as _;
    let mut run = mid_turn();
    let bytes = png();
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let picture = json!({
        "type": "image",
        "source": {"type": "base64", "media_type": "image/png", "data": data},
    });
    run.feed(tool_use(
        "toolu_png",
        "Read",
        json!({"file_path": format!("{CWD}/shot.png")}),
    ));
    run.feed(tool_result_blocks(
        "toolu_png",
        json!([picture.clone()]),
        json!({"type": "image", "file": {"base64": data, "type": "image/png"}}),
    ));
    run.feed(tool_use(
        "toolu_shot",
        "mcp__browser__screenshot",
        json!({"url": "http://localhost:3000"}),
    ));
    let fx = run.feed(tool_result_blocks(
        "toolu_shot",
        json!([{"type": "text", "text": "Took a screenshot"}, picture]),
        json!([]),
    ));
    let folder = run
        .config
        .path()
        .join("attachments")
        .join(lead().to_string())
        .join("tool-output");
    let sha = maxhq_transfer::receive::sha256_hex(&bytes);
    let kept = folder.join(format!("{sha}.png"));
    assert_eq!(fs::read(&kept).expect("the kept picture"), bytes);
    let images = |id: &str| match &run.screen.item(&format!("claude:{id}")).body {
        ItemBody::FileRead { images, .. } | ItemBody::ToolCall { images, .. } => images.clone(),
        body => panic!("no pictures on {body:?}"),
    };
    for call in ["toolu_png", "toolu_shot"] {
        let shown = images(call);
        assert_eq!(shown.len(), 1, "{call}");
        assert_eq!(shown[0].path, kept.to_string_lossy(), "{call}");
        assert_eq!(shown[0].bytes, bytes.len() as u64, "{call}");
        // The session makes the preview off its loop.
        assert_eq!(shown[0].thumbnail, None, "{call}");
    }
    assert_eq!(fx.pictures.previews.len(), 1);
    assert_eq!(fx.pictures.previews[0].id, images("toolu_shot")[0].id);
    assert_eq!(fx.pictures.previews[0].bytes, bytes);
    let output = match &run.screen.item("claude:toolu_shot").body {
        ItemBody::ToolCall { output, .. } => output.clone(),
        body => panic!("not a tool call: {body:?}"),
    };
    assert_eq!(output.as_deref(), Some("Took a screenshot"));
}

#[test]
fn a_containers_picture_goes_to_its_guest_as_the_result_arrives() {
    use base64::Engine as _;
    let mut run = Run::new();
    let workspace = WorkspaceId::derive("box");
    let root = run.config.path().join("in-the-container");
    let place = Place {
        attachments: root.join(lead().to_string()),
        host: PlaceHost::Guest { workspace },
        ..place()
    };
    let login = Login {
        account: account(),
        config_dir: run.config.path().to_path_buf(),
        auth: LoginAuth::ConfigDir,
    };
    let tick = Arc::clone(&run.clock);
    run.driver = ClaudeSession::new(login, place, lead(), PermissionMode::Ask)
        .with_clock(move || Timestamp(tick.load(Ordering::SeqCst)));
    run.play(0..=14);
    let bytes = png();
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    run.feed(tool_use(
        "toolu_shot",
        "mcp__browser__screenshot",
        json!({"url": "http://localhost:3000"}),
    ));
    let fx = run.feed(tool_result_blocks(
        "toolu_shot",
        json!([{
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": data},
        }]),
        json!([]),
    ));
    let ItemBody::ToolCall { images, .. } = &run.screen.item("claude:toolu_shot").body else {
        panic!("not a tool call");
    };
    let sha = maxhq_transfer::receive::sha256_hex(&bytes);
    let named = root
        .join(lead().to_string())
        .join("tool-output")
        .join(format!("{sha}.png"));
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].path, named.to_string_lossy());
    // The host writes nothing there: the guest does.
    assert!(!root.exists());
    assert_eq!(fx.pictures.guest.len(), 1);
    assert_eq!(fx.pictures.guest[0].workspace, workspace);
    assert_eq!(fx.pictures.guest[0].agent, lead());
    assert_eq!(fx.pictures.guest[0].data, data);
}

#[test]
fn a_failed_command_shows_its_exit_code() {
    let mut run = mid_turn();
    run.feed(tool_use(
        "toolu_false",
        "Bash",
        json!({"command": "exit 3"}),
    ));
    run.feed(failed(tool_result(
        "toolu_false",
        "Error: Exit code 3",
        json!("Error: Exit code 3"),
    )));
    let item = run.screen.item("claude:toolu_false");
    assert_eq!(item.status, ItemStatus::Failed);
    let ItemBody::Command {
        exit_code, output, ..
    } = &item.body
    else {
        panic!("{:?}", item.body);
    };
    assert_eq!(*exit_code, Some(3));
    assert_eq!(output, "Exit code 3");
}

#[test]
fn a_background_command_runs_until_its_task_ends() {
    let mut run = mid_turn();
    let input = json!({"command": "sleep 30", "run_in_background": true});
    run.feed(tool_use("toolu_bg", "Bash", input));
    let outcome =
        json!({"stdout": "", "stderr": "", "interrupted": false, "backgroundTaskId": "b77"});
    run.feed(tool_result(
        "toolu_bg",
        "Command running in background with ID: b77",
        outcome,
    ));
    let item = run.screen.item("claude:toolu_bg");
    assert_eq!(item.status, ItemStatus::Running);
    assert!(matches!(
        item.body,
        ItemBody::Command {
            background: true,
            ..
        }
    ));
    let started = json!({"task_id": "b77", "tool_use_id": "toolu_bg", "description": "sleep 30", "task_type": "local_bash"});
    let fx = run.feed(system("task_started", started));
    assert_eq!(
        fx.agents.iter().filter(|agent| agent.id != lead()).count(),
        0
    );
    run.feed(system(
        "task_notification",
        json!({"task_id": "b77", "status": "completed", "summary": "Done"}),
    ));
    assert_eq!(
        run.screen.item("claude:toolu_bg").status,
        ItemStatus::Completed
    );
}

#[test]
fn a_long_command_in_the_foreground_ends_with_its_output_not_its_task() {
    // The CLI makes a task of a foreground command that runs past two
    // seconds, and ends that task just before the command's result.
    let mut run = mid_turn();
    let input = json!({"command": "for i in $(seq 3); do echo $i; sleep 1; done"});
    run.feed(tool_use("toolu_long", "Bash", input));
    let started = json!({"task_id": "f4", "tool_use_id": "toolu_long", "description": "Count to 3",
        "task_type": "local_bash", "is_backgrounded": false});
    run.feed(system("task_started", started));
    run.feed(system(
        "task_notification",
        json!({"task_id": "f4", "tool_use_id": "toolu_long", "status": "completed", "output_file": "", "summary": "Count to 3"}),
    ));
    assert_eq!(
        run.screen.item("claude:toolu_long").status,
        ItemStatus::Running
    );
    let outcome = json!({"stdout": "1\n2\n3", "stderr": "", "interrupted": false, "isImage": false, "noOutputExpected": false});
    run.feed(tool_result("toolu_long", "1\n2\n3", outcome));
    let item = run.screen.item("claude:toolu_long");
    assert_eq!(item.status, ItemStatus::Completed);
    let ItemBody::Command {
        exit_code, output, ..
    } = &item.body
    else {
        panic!("{:?}", item.body);
    };
    assert_eq!((*exit_code, output.as_str()), (Some(0), "1\n2\n3"));
}

#[test]
fn a_long_command_that_fails_in_the_foreground_fails_with_its_output() {
    let mut run = mid_turn();
    let input = json!({"command": "for i in $(seq 3); do echo $i; sleep 1; done; exit 1"});
    run.feed(tool_use("toolu_long", "Bash", input));
    let started = json!({"task_id": "f4", "tool_use_id": "toolu_long", "description": "Count to 3",
        "task_type": "local_bash", "is_backgrounded": false});
    run.feed(system("task_started", started));
    run.feed(system(
        "task_notification",
        json!({"task_id": "f4", "tool_use_id": "toolu_long", "status": "failed", "output_file": "", "summary": "Count to 3"}),
    ));
    assert_eq!(
        run.screen.item("claude:toolu_long").status,
        ItemStatus::Running
    );
    run.feed(failed(tool_result(
        "toolu_long",
        "Error: Exit code 1\n1\n2\n3",
        json!("Error: Exit code 1\n1\n2\n3"),
    )));
    let item = run.screen.item("claude:toolu_long");
    assert_eq!(item.status, ItemStatus::Failed);
    let ItemBody::Command {
        exit_code, output, ..
    } = &item.body
    else {
        panic!("{:?}", item.body);
    };
    assert_eq!(
        (*exit_code, output.as_str()),
        (Some(1), "Exit code 1\n1\n2\n3")
    );
}

#[test]
fn a_foreground_command_killed_with_its_cli_reads_as_cut_off() {
    // Stopping the machine kills the command, whose task fails, and the
    // CLI exits before it writes the result.
    let mut run = mid_turn();
    let input = json!({"command": "for i in $(seq 40); do echo $i; sleep 1; done"});
    run.feed(tool_use("toolu_long", "Bash", input));
    let started = json!({"task_id": "f4", "tool_use_id": "toolu_long", "description": "Count to 40",
        "task_type": "local_bash", "is_backgrounded": false});
    run.feed(system("task_started", started));
    run.feed(system(
        "task_notification",
        json!({"task_id": "f4", "tool_use_id": "toolu_long", "status": "failed", "output_file": "", "summary": "Count to 40"}),
    ));
    run.exit(Some(143), None, "");
    assert_eq!(
        run.screen.item("claude:toolu_long").status,
        ItemStatus::Interrupted
    );
}

/// A `task_progress` line for task `task`.
fn progress(task: &str, fields: Value) -> Value {
    let usage = json!({"total_tokens": 900, "tool_uses": 0, "duration_ms": 4000});
    system(
        "task_progress",
        with(json!({"task_id": task, "usage": usage}), fields),
    )
}

/// The progress shown under a call, if any.
fn progress_shown(run: &Run, call: &str) -> Option<TimelineItem> {
    run.screen.find(&format!("claude:{call}:progress")).cloned()
}

#[test]
fn a_sub_agent_shows_its_progress_under_its_call_until_it_ends() {
    let mut run = mid_turn();
    let input = json!({"description": "Find the tests", "prompt": "Find where the tests live.", "subagent_type": "Explore"});
    run.feed(tool_use("toolu_agent", "Agent", input));
    let started = json!({"task_id": "a1b2c3", "tool_use_id": "toolu_agent", "description": "Find the tests", "task_type": "local_agent"});
    run.feed(system("task_started", started));
    let sub = names::subagent(lead(), "a1b2c3");

    // The task's own description says nothing new, so the last tool does.
    run.feed(progress(
        "a1b2c3",
        json!({"tool_use_id": "toolu_agent", "description": "Find the tests", "last_tool_name": "Grep",
            "usage": {"total_tokens": 1200, "tool_uses": 1, "duration_ms": 3000}}),
    ));
    let shown = progress_shown(&run, "toolu_agent").expect("progress");
    assert_eq!(
        shown.body,
        ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "Using Grep · 1 tool call".to_owned(),
        }
    );
    assert_eq!(
        (shown.agent, shown.parent.clone(), shown.status),
        (
            lead(),
            Some(ItemId("claude:toolu_agent".to_owned())),
            ItemStatus::Running
        )
    );
    assert_eq!(shown.turn, run.screen.item("claude:toolu_agent").turn);

    // Each line replaces the last, and a summary wins over the rest.
    let before = run.screen.items.len();
    run.feed(progress(
        "a1b2c3",
        json!({"tool_use_id": "toolu_agent", "description": "Searching for #[test]", "last_tool_name": "Grep",
            "usage": {"total_tokens": 2400, "tool_uses": 3, "duration_ms": 6000}}),
    ));
    assert_eq!(
        progress_shown(&run, "toolu_agent").map(|item| item.body),
        Some(ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "Searching for #[test] · 3 tool calls".to_owned(),
        })
    );
    run.feed(progress(
        "a1b2c3",
        json!({"tool_use_id": "toolu_agent", "description": "Found the tests in tests/", "summary": "Found the tests in tests/",
            "usage": {"total_tokens": 3000, "tool_uses": 4, "duration_ms": 8000}}),
    ));
    assert_eq!(
        progress_shown(&run, "toolu_agent").map(|item| item.body),
        Some(ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "Found the tests in tests/ · 4 tool calls".to_owned(),
        })
    );
    assert_eq!(run.screen.items.len(), before);
    assert!(run.screen.agents.contains_key(&sub));

    // The transcript keeps no progress, so it goes when the call ends.
    run.feed(tool_result(
        "toolu_agent",
        "The tests live in tests/.",
        json!({"status": "completed", "agentId": "a1b2c3"}),
    ));
    assert_eq!(progress_shown(&run, "toolu_agent"), None);
    run.feed(progress(
        "a1b2c3",
        json!({"tool_use_id": "toolu_agent", "description": "Late", "summary": "Late"}),
    ));
    assert_eq!(progress_shown(&run, "toolu_agent"), None);
}

#[test]
fn a_background_task_shows_its_status_until_its_task_ends() {
    let mut run = mid_turn();
    let input = json!({"description": "Render the frames", "prompt": "Render every frame.", "run_in_background": true});
    run.feed(tool_use("toolu_bg_agent", "Agent", input));
    run.feed(tool_result(
        "toolu_bg_agent",
        "Async agent launched.",
        json!({"status": "async_launched", "agentId": "f9e8"}),
    ));
    let started = json!({"task_id": "f9e8", "tool_use_id": "toolu_bg_agent", "description": "Render the frames", "task_type": "local_agent"});
    run.feed(system("task_started", started));
    run.feed(progress(
        "f9e8",
        json!({"description": "Render the frames", "summary": "50% · frame 12 of 24"}),
    ));
    assert_eq!(
        progress_shown(&run, "toolu_bg_agent").map(|item| item.body),
        Some(ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "50% · frame 12 of 24".to_owned(),
        })
    );
    // A line with nothing to say leaves the last one up.
    run.feed(progress(
        "f9e8",
        json!({"description": "Render the frames"}),
    ));
    assert_eq!(
        progress_shown(&run, "toolu_bg_agent").map(|item| item.body),
        Some(ItemBody::Notice {
            level: NoticeLevel::Info,
            message: "50% · frame 12 of 24".to_owned(),
        })
    );
    run.feed(system(
        "task_notification",
        json!({"task_id": "f9e8", "status": "completed", "summary": "Rendered 24 frames"}),
    ));
    assert_eq!(progress_shown(&run, "toolu_bg_agent"), None);
    assert_eq!(
        run.screen.item("claude:toolu_bg_agent").status,
        ItemStatus::Completed
    );
}

#[test]
fn progress_for_a_task_never_seen_starting_still_shows_and_goes() {
    let mut run = mid_turn();
    let input = json!({"command": "make render", "run_in_background": true});
    run.feed(tool_use("toolu_make", "Bash", input));
    run.feed(tool_result(
        "toolu_make",
        "Command running in background with ID: m5",
        json!({"stdout": "", "stderr": "", "interrupted": false, "backgroundTaskId": "m5"}),
    ));
    run.feed(progress(
        "m5",
        json!({"tool_use_id": "toolu_make", "description": "Rendering"}),
    ));
    assert!(progress_shown(&run, "toolu_make").is_some());
    run.feed(progress(
        "unknown",
        json!({"description": "Nothing to hang it on"}),
    ));
    assert!(run.screen.find("claude:unknown:progress").is_none());
    let fx = run.exit(Some(0), None, "");
    assert!(fx.results.is_empty());
    assert_eq!(progress_shown(&run, "toolu_make"), None);
}

#[test]
fn a_sub_agent_is_an_agent_of_its_own_until_its_call_ends() {
    let mut run = mid_turn();
    let input = json!({"description": "Find the tests", "prompt": "Find where the tests live.", "subagent_type": "Explore"});
    run.feed(tool_use("toolu_agent", "Agent", input));
    let started = json!({"task_id": "a1b2c3", "tool_use_id": "toolu_agent", "description": "Find the tests", "task_type": "local_agent"});
    let fx = run.feed(system("task_started", started));
    let sub = names::subagent(lead(), "a1b2c3");
    assert_eq!(
        run.screen.item("claude:toolu_agent").body,
        ItemBody::Subagent {
            subagent: Some(sub),
            agent_type: Some("Explore".to_owned()),
            description: "Find the tests".to_owned(),
            result: None,
        }
    );
    let agent = fx
        .agents
        .iter()
        .find(|agent| agent.id == sub)
        .expect("the sub-agent");
    assert_eq!(agent.parent, Some(lead()));
    assert_eq!(agent.title, "Find the tests");
    assert_eq!(agent.vendor_session_id.as_deref(), Some("a1b2c3"));
    assert_eq!(agent.activity, Activity::Busy);
    assert_eq!(agent.link, AgentLink::Live);
    assert_eq!((agent.model.clone(), agent.permission), (None, None));
    assert_eq!(agent.pid, Some(33451));
    assert_eq!(run.lead().activity, Activity::BusyInSubagents);

    // Its own steps go on its own timeline.
    let grep = json!({"pattern": "#[test]"});
    let fx = run.feed(under(
        tool_use("toolu_inner", "Grep", grep.clone()),
        "toolu_agent",
    ));
    assert!(!fx.timeline.is_empty());
    assert!(fx.timeline.iter().all(|(agent, _)| *agent == sub));
    let request = can_use_tool("req-inner", "toolu_inner", "Grep", grep, json!([]));
    let fx = run.feed(request);
    let shown = opened(&fx);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].agent, sub);
    assert_eq!(shown[0].summary, "Search for `#[test]`?");
    let event = approval("req-inner");
    assert_eq!(
        run.screen.agent(sub).activity,
        Activity::NeedsInput { attention: event }
    );
    assert_eq!(run.lead().activity, Activity::BusyInSubagents);
    let ask = run.screen.item("claude:toolu_inner:approval");
    assert_eq!(ask.agent, sub);
    assert_eq!(ask.parent, Some(ItemId("claude:toolu_inner".to_owned())));
    run.pick("allow", event, "allow");
    assert_eq!(run.screen.agent(sub).activity, Activity::Busy);
    let found = json!({"numFiles": 2, "filenames": ["a.rs", "b.rs"]});
    run.feed(under(
        tool_result("toolu_inner", "Found 2 files", found),
        "toolu_agent",
    ));
    assert_eq!(
        run.screen.item("claude:toolu_inner").status,
        ItemStatus::Completed
    );

    // Only the lead takes these.
    let fx = run.act(send("tell", sub, "Look in src too.", SendMode::Queue));
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(key("tell"), refused("You can't message a sub-agent."))]
    );
    let fx = run.act(Action::SetPermission {
        key: key("mode"),
        agent: sub,
        mode: PermissionMode::Plan,
    });
    assert_eq!(
        fx.results,
        [(key("mode"), refused("A sub-agent runs in its lead's mode."))]
    );
    let fx = run.act(Action::SetModel {
        key: key("model"),
        agent: sub,
        model: "opus".to_owned(),
    });
    assert_eq!(
        fx.results,
        [(key("model"), refused("A sub-agent runs its lead's model."))]
    );
    let fx = run.act(Action::SetEffort {
        key: key("effort"),
        agent: sub,
        effort: Some("low".to_owned()),
    });
    assert_eq!(
        fx.results,
        [(
            key("effort"),
            refused("A sub-agent runs at its lead's effort.")
        )]
    );
    let fx = run.act(Action::CancelQueued {
        key: key("cancel"),
        agent: sub,
        item: ItemId("claude:nothing".to_owned()),
    });
    assert_eq!(
        fx.results,
        [(key("cancel"), refused("A sub-agent has no queue."))]
    );

    // Interrupting a sub-agent stops its task alone.
    let fx = run.act(Action::Interrupt {
        key: key("stop"),
        agent: sub,
    });
    let written = only_write(&fx);
    assert_eq!(
        written["request"],
        json!({"subtype": "stop_task", "task_id": "a1b2c3"})
    );
    assert!(fx.results.is_empty());
    let fx = run.reply(&written, json!({}));
    assert_eq!(fx.results, [(key("stop"), ActionOutcome::Done)]);

    // Its call's result ends it.
    let said = "The tests live in tests/.";
    let fx = run.feed(tool_result(
        "toolu_agent",
        said,
        json!({"status": "completed", "agentId": "a1b2c3"}),
    ));
    assert_eq!(fx.gone, [sub]);
    assert!(!run.screen.agents.contains_key(&sub));
    let item = run.screen.item("claude:toolu_agent");
    assert_eq!(item.status, ItemStatus::Completed);
    assert!(
        matches!(&item.body, ItemBody::Subagent { result: Some(result), .. } if result == said)
    );
    assert_eq!(run.lead().activity, Activity::Busy);
    let fx = run.act(Action::Interrupt {
        key: key("late"),
        agent: sub,
    });
    assert_eq!(fx.results, [(key("late"), ActionOutcome::UnknownAgent)]);
}

#[test]
fn each_send_mode_takes_its_place_in_the_cli_queue() {
    let mut run = counting();
    let modes = [
        ("steer", SendMode::Steer, Some("next"), Delivery::Steer),
        ("queue", SendMode::Queue, Some("later"), Delivery::Queued),
        ("now", SendMode::Interrupt, Some("now"), Delivery::Sent),
    ];
    let mut uuids = Vec::new();
    for (name, mode, priority, delivery) in modes {
        let fx = run.act(send(name, lead(), &format!("maxhq test: {name}"), mode));
        let written = only_write(&fx);
        assert_eq!(written["priority"].as_str(), priority, "{name}");
        assert_eq!(written["type"], "user");
        assert_eq!(fx.results, [(key(name), ActionOutcome::Done)]);
        let uuid = written["uuid"].as_str().expect("a UUID").to_owned();
        let item = run.screen.item(&format!("claude:{uuid}"));
        assert_eq!(item.status, ItemStatus::Pending, "{name}");
        assert_eq!(item.turn, None);
        assert!(
            matches!(&item.body, ItemBody::UserMessage { delivery: shown, .. } if *shown == delivery),
            "{name}: {:?}",
            item.body
        );
        uuids.push(uuid);
    }
    // The steer joins the turn at its next step.
    let steer = &uuids[0];
    run.feed(json!({"type": "command_lifecycle", "command_uuid": steer, "state": "started"}));
    let item = run.screen.item(&format!("claude:{steer}"));
    assert_eq!(item.status, ItemStatus::Completed);
    assert_eq!(item.turn, Some(TurnId(format!("claude:{steer}"))));
    // The queued message waits, and once it starts it shows as sent.
    let queued = &uuids[1];
    run.feed(json!({"type": "command_lifecycle", "command_uuid": queued, "state": "queued"}));
    assert_eq!(
        run.screen.item(&format!("claude:{queued}")).status,
        ItemStatus::Pending
    );
    run.feed(json!({"type": "command_lifecycle", "command_uuid": queued, "state": "started"}));
    let item = run.screen.item(&format!("claude:{queued}"));
    assert_eq!(item.status, ItemStatus::Completed);
    assert!(matches!(
        item.body,
        ItemBody::UserMessage {
            delivery: Delivery::Sent,
            ..
        }
    ));
    // A message the CLI drops before it starts shows as cancelled.
    let dropped = &uuids[2];
    run.feed(json!({"type": "command_lifecycle", "command_uuid": dropped, "state": "cancelled"}));
    let item = run.screen.item(&format!("claude:{dropped}"));
    assert_eq!(item.status, ItemStatus::Cancelled);
    assert!(matches!(
        item.body,
        ItemBody::UserMessage {
            delivery: Delivery::Cancelled,
            ..
        }
    ));
    let fx = run.act(send(
        "nobody",
        AgentId::derive("nobody"),
        "Hello?",
        SendMode::Queue,
    ));
    assert!(fx.writes.is_empty());
    assert_eq!(fx.results, [(key("nobody"), ActionOutcome::UnknownAgent)]);
}

#[test]
fn a_large_picture_goes_to_the_cli_shrunk_and_a_small_one_as_it_is() {
    use base64::Engine;
    let mut run = Run::new();
    run.play(0..=9);
    let dir = TempDir::new();
    let path = dir.path().join("wide.png");
    image::RgbImage::from_pixel(3000, 600, image::Rgb([10, 20, 30]))
        .save(&path)
        .expect("a picture");
    let wide = Staged {
        attachment: AttachmentRef {
            id: AttachmentId::derive("wide.png"),
            name: "wide.png".to_owned(),
            kind: AttachmentKind::Image,
            bytes: fs::metadata(&path).expect("the picture").len(),
            mime: Some("image/png".to_owned()),
            path: "/client/wide.png".to_owned(),
            thumbnail: None,
        },
        path,
        text: None,
    };
    let fx = run.act(Action::Send {
        key: key("pictures"),
        agent: lead(),
        text: "Compare these".to_owned(),
        refs: Vec::new(),
        attachments: vec![wide, red_png()],
        mode: SendMode::Queue,
    });
    let written = only_write(&fx);
    let blocks = written["message"]["content"].as_array().expect("blocks");
    let data = |at: usize| {
        let data = blocks[at]["source"]["data"].as_str().expect("base64");
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("valid base64")
    };
    let shrunk =
        image::load_from_memory_with_format(&data(1), image::ImageFormat::Png).expect("a PNG");
    assert_eq!((shrunk.width(), shrunk.height()), (2000, 400));
    assert_eq!(data(2), fs::read(IMAGE).expect("the fixture image"));
    let item = run.message(written["uuid"].as_str().expect("a UUID"));
    let ItemBody::UserMessage { attachments, .. } = &item.body else {
        panic!("{item:?}");
    };
    assert_eq!(attachments.len(), 2);
}

#[test]
fn a_queued_message_can_be_taken_back_before_it_starts() {
    let mut run = Run::new();
    run.play(0..=219);
    let queued = ItemId(format!("claude:{}", run.ours(QUEUED)));
    let fx = run.act(Action::CancelQueued {
        key: key("take back"),
        agent: lead(),
        item: queued.clone(),
    });
    let written = only_write(&fx);
    assert_eq!(
        written["request"],
        json!({"subtype": "cancel_async_message", "message_uuid": run.ours(QUEUED)})
    );
    assert!(fx.results.is_empty(), "it waits for the CLI");
    let fx = run.reply(&written, json!({"cancelled": true}));
    assert_eq!(fx.results, [(key("take back"), ActionOutcome::Done)]);
    let item = run.message(QUEUED);
    assert_eq!(item.status, ItemStatus::Cancelled);
    assert!(matches!(
        item.body,
        ItemBody::UserMessage {
            delivery: Delivery::Cancelled,
            ..
        }
    ));
    let fx = run.act(Action::CancelQueued {
        key: key("again"),
        agent: lead(),
        item: queued,
    });
    assert_eq!(
        fx.results,
        [(key("again"), refused("That isn't a queued message."))]
    );
    let started = ItemId(format!("claude:{}", run.ours(COUNT)));
    let fx = run.act(Action::CancelQueued {
        key: key("started"),
        agent: lead(),
        item: started,
    });
    assert_eq!(
        fx.results,
        [(key("started"), refused("That message already started."))]
    );
    let fx = run.act(Action::CancelQueued {
        key: key("tool"),
        agent: lead(),
        item: ItemId(READ_CALL.to_owned()),
    });
    assert_eq!(
        fx.results,
        [(key("tool"), refused("That isn't a queued message."))]
    );

    // The CLI can refuse, when the message started meanwhile.
    let fx = run.act(send("late", lead(), "maxhq test: late", SendMode::Queue));
    let uuid = only_write(&fx)["uuid"].as_str().expect("a UUID").to_owned();
    let fx = run.act(Action::CancelQueued {
        key: key("too late"),
        agent: lead(),
        item: ItemId(format!("claude:{uuid}")),
    });
    let written = only_write(&fx);
    let fx = run.reply_error(&written, "The message already started.");
    assert_eq!(
        fx.results,
        [(key("too late"), refused("The message already started."))]
    );
    assert_eq!(
        run.screen.item(&format!("claude:{uuid}")).status,
        ItemStatus::Pending
    );
}

#[test]
fn a_mode_and_a_model_change_go_through_the_cli() {
    let mut run = Run::new();
    run.play(0..=9);
    for (mode, name) in [
        (PermissionMode::FullAccess, "bypassPermissions"),
        (PermissionMode::Auto, "auto"),
        (PermissionMode::Plan, "plan"),
    ] {
        let fx = run.act(Action::SetPermission {
            key: key(name),
            agent: lead(),
            mode,
        });
        let written = only_write(&fx);
        assert_eq!(
            written["request"],
            json!({"subtype": "set_permission_mode", "mode": name})
        );
        run.reply(&written, json!({"mode": name}));
        let permission = run.lead().permission.expect("a mode");
        assert_eq!(
            (permission.requested, permission.effective),
            (mode, Some(mode))
        );
    }
    let fx = run.act(Action::SetModel {
        key: key("model"),
        agent: lead(),
        model: "sonnet".to_owned(),
    });
    let written = only_write(&fx);
    assert_eq!(
        written["request"],
        json!({"subtype": "set_model", "model": "sonnet"})
    );
    assert!(fx.results.is_empty());
    let fx = run.reply(&written, json!({}));
    assert_eq!(fx.results, [(key("model"), ActionOutcome::Done)]);
    assert_eq!(run.lead().model.as_deref(), Some("sonnet"));
}

#[test]
fn an_effort_change_goes_through_the_cli() {
    let mut run = Run::new();
    run.play(0..=9);
    let fx = run.act(Action::SetEffort {
        key: key("effort"),
        agent: lead(),
        effort: Some("high".to_owned()),
    });
    let written = only_write(&fx);
    assert_eq!(
        written["request"],
        json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": "high"}})
    );
    assert!(fx.results.is_empty());
    let fx = run.reply(&written, json!({}));
    assert_eq!(fx.results, [(key("effort"), ActionOutcome::Done)]);
    // Back to the default clears the setting.
    let fx = run.act(Action::SetEffort {
        key: key("default"),
        agent: lead(),
        effort: None,
    });
    let written = only_write(&fx);
    assert_eq!(
        written["request"],
        json!({"subtype": "apply_flag_settings", "settings": {"effortLevel": null}})
    );
    let fx = run.reply(&written, json!({}));
    assert_eq!(fx.results, [(key("default"), ActionOutcome::Done)]);
}

/// The lead's requested and effective modes.
fn modes(run: &Run) -> (PermissionMode, Option<PermissionMode>) {
    let permission = run.lead().permission.expect("a mode");
    (permission.requested, permission.effective)
}

/// Asks for a mode and returns the request written.
fn set_mode(run: &mut Run, name: &str, mode: PermissionMode) -> Value {
    let fx = run.act(Action::SetPermission {
        key: key(name),
        agent: lead(),
        mode,
    });
    assert!(fx.results.is_empty());
    only_write(&fx)
}

#[test]
fn a_mode_the_cli_refuses_goes_back_with_its_reason() {
    let mut run = Run::new();
    run.play(0..=9);
    let written = set_mode(&mut run, "plan", PermissionMode::Plan);
    run.reply(&written, json!({"mode": "plan"}));

    let written = set_mode(&mut run, "refused", PermissionMode::FullAccess);
    assert_eq!(
        modes(&run),
        (PermissionMode::FullAccess, Some(PermissionMode::Plan))
    );
    let fx = run.reply_error(&written, "Cannot set permission mode to bypassPermissions");
    assert_eq!(
        fx.results,
        [(
            key("refused"),
            refused("Cannot set permission mode to bypassPermissions")
        )]
    );
    assert_eq!(
        modes(&run),
        (PermissionMode::Plan, Some(PermissionMode::Plan))
    );
}

#[test]
fn a_refused_mode_under_a_later_one_falls_back_past_both() {
    let mut run = Run::new();
    run.play(0..=9);
    let written = set_mode(&mut run, "plan", PermissionMode::Plan);
    run.reply(&written, json!({"mode": "plan"}));

    let first = set_mode(&mut run, "auto", PermissionMode::Auto);
    let second = set_mode(&mut run, "full", PermissionMode::FullAccess);
    run.reply_error(&first, "No auto mode on this plan");
    assert_eq!(modes(&run).0, PermissionMode::FullAccess);
    let fx = run.reply_error(&second, "Cannot set permission mode");
    assert_eq!(
        fx.results,
        [(key("full"), refused("Cannot set permission mode"))]
    );
    assert_eq!(modes(&run).0, PermissionMode::Plan);
}

#[test]
fn a_mode_change_the_cli_never_answers_goes_back_when_it_ends() {
    let mut run = Run::new();
    run.play(0..=9);
    let before = modes(&run).0;
    set_mode(&mut run, "lost", PermissionMode::FullAccess);
    assert_eq!(modes(&run).0, PermissionMode::FullAccess);
    let fx = run.exit(Some(1), None, "");
    assert!(fx
        .results
        .contains(&(key("lost"), refused("Claude ended before it answered."))));
    assert_eq!(modes(&run).0, before);
}

#[test]
fn a_session_starts_and_an_early_action_initializes_first() {
    let mut run = Run::new();
    let fx = run.connect(true, &[]);
    assert_eq!(
        only_write(&fx)["request"],
        json!({
            "subtype": "initialize",
            "forwardSubagentText": true,
            "agentProgressSummaries": true,
            "perTaskStopAffordance": true,
            "sdkMcpServers": ["maxhq"],
            "sdkMcpServerConfigs": {"maxhq": {"timeout": 600_000}},
        })
    );
    assert_eq!(run.lead().activity, Activity::Starting);
    assert_eq!(run.lead().title, "work");

    let mut early = Run::new();
    let fx = early.act(send("first", lead(), "maxhq test: hi", SendMode::Queue));
    assert_eq!(fx.writes.len(), 2);
    let first: Value = serde_json::from_str(&fx.writes[0].line).expect("JSON");
    assert_eq!(first["request"]["subtype"], "initialize");
    assert!(first["request"]["forwardSubagentText"]
        .as_bool()
        .unwrap_or_default());
    let second: Value = serde_json::from_str(&fx.writes[1].line).expect("JSON");
    assert_eq!(second["type"], "user");
    assert!(second.get("priority").is_none(), "nothing runs yet");
}

#[test]
fn a_first_message_keeps_the_agent_busy_while_the_cli_says_idle() {
    let mut run = Run::new();
    let fx = run.act(send("first", lead(), "maxhq test: hi", SendMode::Queue));
    let init: Value = serde_json::from_str(&fx.writes[0].line).expect("JSON");
    let first: Value = serde_json::from_str(&fx.writes[1].line).expect("JSON");
    let uuid = first["uuid"].as_str().expect("a UUID").to_owned();

    run.reply(&init, json!({"session_state": "idle"}));
    assert_eq!(
        run.lead().activity,
        Activity::Busy,
        "the CLI hasn't read it"
    );
    let fx = run.act(send("second", lead(), "and then", SendMode::Queue));
    let second: Value = serde_json::from_str(&fx.writes[0].line).expect("JSON");
    assert_eq!(second["priority"], "later");

    run.feed(json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "started"}));
    assert_eq!(run.lead().activity, Activity::Busy);
    run.feed(json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "completed"}));
    run.feed(result(&uuid::Uuid::now_v7().to_string(), json!({})));
    assert_eq!(
        run.lead().activity,
        Activity::Busy,
        "the second message waits its turn"
    );

    let uuid = second["uuid"].as_str().expect("a UUID");
    run.feed(json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "started"}));
    run.feed(json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "completed"}));
    run.feed(result(&uuid::Uuid::now_v7().to_string(), json!({})));
    assert!(
        matches!(
            run.lead().activity,
            Activity::Idle | Activity::IdleCacheWarm { .. }
        ),
        "{:?}",
        run.lead().activity
    );
}

/// The `rate_limit_event` that says the account is out until `resets`,
/// in seconds.
fn out_until(resets: u64) -> Value {
    let rejected = json!({
        "status": "rejected",
        "resetsAt": resets,
        "unifiedWindows": {"five_hour": {"status": "rejected", "utilization": 1.0, "resetsAt": resets}},
    });
    line(json!({"type": "rate_limit_event", "rate_limit_info": rejected}))
}

/// The line the CLI writes for an API call the usage limit turned away.
fn limit_error(uuid: &str) -> Value {
    line(json!({
        "type": "assistant",
        "uuid": uuid,
        "parent_tool_use_id": null,
        "error": "rate_limit",
        "isApiErrorMessage": true,
        "message": {
            "id": "msg_limit",
            "role": "assistant",
            "model": "<synthetic>",
            "content": [{"type": "text", "text": "You've hit your limit \u{b7} resets 3pm"}],
        },
    }))
}

/// The `result` line of a turn the usage limit ended.
fn limit_result(uuid: &str) -> Value {
    let outcome = json!({
        "is_error": true,
        "api_error_status": 429,
        "result": "You've hit your limit \u{b7} resets 3pm",
        "duration_ms": 120,
    });
    result(uuid, outcome)
}

/// A run whose first turn hit the usage limit, and when the limit resets
/// in milliseconds.
fn at_the_limit() -> (Run, u64) {
    let mut run = mid_turn();
    let resets = run.clock.load(Ordering::SeqCst) / 1000 + 3600;
    run.feed(out_until(resets));
    run.feed(limit_error("limit-1"));
    run.feed(limit_result("res-limit"));
    (run, resets * 1000)
}

/// The wait the limit put on the timeline.
fn wait(run: &Run) -> &TimelineItem {
    run.screen.item("claude:limit-1")
}

/// Whether the wait says the agent goes on by itself after the reset.
fn resumes(run: &Run) -> bool {
    match &wait(run).body {
        ItemBody::LimitWait { auto_resume, .. } => *auto_resume,
        body => panic!("{body:?}"),
    }
}

/// The message you sent with these words.
fn sent<'a>(run: &'a Run, said: &str) -> &'a TimelineItem {
    run.screen
        .items
        .iter()
        .find(|item| matches!(&item.body, ItemBody::UserMessage { text, .. } if text == said))
        .unwrap_or_else(|| panic!("no message {said:?}"))
}

/// How a message you sent goes out.
fn delivery(item: &TimelineItem) -> Delivery {
    match &item.body {
        ItemBody::UserMessage { delivery, .. } => *delivery,
        body => panic!("{body:?}"),
    }
}

/// Holds a message queued at the limit, and checks it waits with `maxd`.
fn hold(run: &mut Run, name: &str, said: &str) {
    let fx = run.act(send(name, lead(), said, SendMode::Queue));
    assert!(fx.writes.is_empty(), "{name} waits for the reset");
    assert_eq!(fx.results, [(key(name), ActionOutcome::Done)]);
    let item = sent(run, said);
    assert_eq!(item.status, ItemStatus::Pending);
    assert_eq!(delivery(item), Delivery::Queued);
}

#[test]
fn a_usage_limit_waits_for_the_reset() {
    let mut run = mid_turn();
    let now = run.clock.load(Ordering::SeqCst);
    let resets = now / 1000 + 3600;
    let fx = run.feed(out_until(resets));
    assert_eq!(fx.usage.len(), 1);
    assert!((fx.usage[0].1.windows[0].used_percent - 100.0).abs() < 1e-9);
    run.feed(limit_error("limit-1"));
    let waits = ItemBody::LimitWait {
        account: Some(account()),
        resets_at: Some(Timestamp(resets * 1000)),
        auto_resume: false,
    };
    assert_eq!(wait(&run).body, waits);
    assert_eq!(wait(&run).turn, run.turn(FIRST));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    assert_eq!(wait(&run).completed_at, None);
    let fx = run.feed(limit_result("res-limit"));
    assert!(
        run.screen.find("claude:res-limit:error").is_none(),
        "one wait a turn"
    );
    assert!(run.screen.find("claude:res-limit").is_some());
    assert_eq!(run.lead().model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(
        run.lead().activity,
        Activity::WaitingForLimitReset {
            resets_at: Some(Timestamp(resets * 1000)),
        }
    );
    assert!(fx.wake_at.is_some());
    // The turn is over, and the wait stays open until the reset.
    assert_eq!(wait(&run).status, ItemStatus::Running);
    run.clock.store(resets * 1000 - 1, Ordering::SeqCst);
    tick(&mut run);
    assert_eq!(wait(&run).status, ItemStatus::Running);
    run.clock.store(resets * 1000 + 1, Ordering::SeqCst);
    let fx = tick(&mut run);
    assert!(fx.writes.is_empty());
    assert!(!matches!(
        run.lead().activity,
        Activity::WaitingForLimitReset { .. }
    ));
    assert_eq!(wait(&run).status, ItemStatus::Completed);
    assert_eq!(wait(&run).completed_at, Some(Timestamp(resets * 1000 + 1)));
    assert!(!resumes(&run));
}

#[test]
fn a_wait_without_a_limit_event_holds_until_the_reset_it_names() {
    let mut run = mid_turn();
    let resets = run.clock.load(Ordering::SeqCst) / 1000 + 600;
    let mut error = limit_error("limit-1");
    error["message"]["content"][0]["text"] =
        json!(format!("Claude AI usage limit reached|{resets}"));
    run.feed(error);
    run.feed(limit_result("res-limit"));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    assert_eq!(
        run.lead().activity,
        Activity::WaitingForLimitReset {
            resets_at: Some(Timestamp(resets * 1000)),
        }
    );
    // A limit event that comes after the error sets the time it waits for.
    let later = resets + 600;
    run.feed(out_until(later));
    assert!(matches!(
        wait(&run).body,
        ItemBody::LimitWait { resets_at: Some(at), .. } if at == Timestamp(later * 1000)
    ));
    run.clock.store(resets * 1000 + 1, Ordering::SeqCst);
    tick(&mut run);
    assert_eq!(wait(&run).status, ItemStatus::Running);
    run.clock.store(later * 1000 + 1, Ordering::SeqCst);
    tick(&mut run);
    assert_eq!(wait(&run).status, ItemStatus::Completed);
}

#[test]
fn messages_queued_at_the_limit_go_once_it_resets() {
    let (mut run, resets) = at_the_limit();
    assert!(!resumes(&run));
    hold(&mut run, "first", "maxhq test: after the reset");
    assert!(resumes(&run), "the wait says the agent goes on by itself");
    hold(&mut run, "second", "maxhq test: and then this");
    assert_eq!(
        run.lead().activity,
        Activity::WaitingForLimitReset {
            resets_at: Some(Timestamp(resets)),
        }
    );
    run.clock.store(resets - 1, Ordering::SeqCst);
    let fx = tick(&mut run);
    assert!(fx.writes.is_empty());
    assert!(fx.wake_at.is_some());
    run.clock.store(resets + 1, Ordering::SeqCst);
    let fx = tick(&mut run);
    assert_eq!(fx.writes.len(), 2, "{:?}", fx.writes);
    let lines: Vec<Value> = fx
        .writes
        .iter()
        .map(|write| serde_json::from_str(&write.line).expect("a JSON line"))
        .collect();
    let first = sent(&run, "maxhq test: after the reset").id.clone();
    let second = sent(&run, "maxhq test: and then this").id.clone();
    assert_eq!(
        first.0,
        format!("claude:{}", lines[0]["uuid"].as_str().expect("a UUID"))
    );
    assert_eq!(
        second.0,
        format!("claude:{}", lines[1]["uuid"].as_str().expect("a UUID"))
    );
    assert_eq!(lines[0]["type"], "user");
    assert_eq!(lines[0]["priority"], Value::Null, "the first starts a turn");
    assert_eq!(lines[1]["priority"], "later", "the second waits behind it");
    assert_eq!(
        lines[0]["message"]["content"][0]["text"],
        "maxhq test: after the reset"
    );
    assert_eq!(wait(&run).status, ItemStatus::Completed);
    assert_eq!(wait(&run).completed_at, Some(Timestamp(resets + 1)));
    assert_eq!(run.lead().activity, Activity::Busy);
    let uuid = lines[0]["uuid"].as_str().expect("a UUID");
    run.feed(json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "started"}));
    let item = run.screen.item(&first.0);
    assert_eq!(item.status, ItemStatus::Completed);
    assert_eq!(delivery(item), Delivery::Sent);
    // Nothing goes twice.
    let fx = tick(&mut run);
    assert!(fx.writes.is_empty());
}

#[test]
fn taking_back_the_last_held_message_turns_the_resume_off() {
    let (mut run, resets) = at_the_limit();
    hold(&mut run, "first", "maxhq test: one");
    hold(&mut run, "second", "maxhq test: two");
    let one = sent(&run, "maxhq test: one").id.clone();
    let two = sent(&run, "maxhq test: two").id.clone();
    let fx = run.act(Action::CancelQueued {
        key: key("take one"),
        agent: lead(),
        item: one.clone(),
    });
    assert!(fx.writes.is_empty(), "the CLI never had it");
    assert_eq!(fx.results, [(key("take one"), ActionOutcome::Done)]);
    let item = run.screen.item(&one.0);
    assert_eq!(item.status, ItemStatus::Cancelled);
    assert_eq!(delivery(item), Delivery::Cancelled);
    assert!(resumes(&run), "one message still waits");
    let fx = run.act(Action::CancelQueued {
        key: key("take two"),
        agent: lead(),
        item: two.clone(),
    });
    assert_eq!(fx.results, [(key("take two"), ActionOutcome::Done)]);
    assert!(!resumes(&run));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    let fx = run.act(Action::CancelQueued {
        key: key("again"),
        agent: lead(),
        item: two,
    });
    assert_eq!(
        fx.results,
        [(key("again"), refused("That isn't a queued message."))]
    );
    run.clock.store(resets + 1, Ordering::SeqCst);
    let fx = tick(&mut run);
    assert!(fx.writes.is_empty());
    assert_eq!(wait(&run).status, ItemStatus::Completed);
}

/// A reply from the API to the lead.
fn reply(uuid: &str, said: &str) -> Value {
    line(json!({
        "type": "assistant",
        "uuid": uuid,
        "parent_tool_use_id": null,
        "message": {
            "id": format!("msg-{uuid}"),
            "role": "assistant",
            "model": "claude-haiku-5-5",
            "content": [{"type": "text", "text": said}],
        },
    }))
}

#[test]
fn a_turn_the_cli_starts_by_itself_keeps_the_wait() {
    let (mut run, resets) = at_the_limit();
    // A background task ended, and the CLI ran a turn for its notice
    // that made no API call.
    run.feed(system("init", json!({"model": "claude-haiku-5-5"})));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    let empty = json!({"num_turns": 0, "duration_ms": 0, "duration_api_ms": 0});
    run.feed(result("res-empty", empty));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    assert_eq!(
        run.lead().activity,
        Activity::WaitingForLimitReset {
            resets_at: Some(Timestamp(resets)),
        }
    );
    // The reset still ends it.
    run.clock.store(resets + 1, Ordering::SeqCst);
    tick(&mut run);
    assert_eq!(wait(&run).status, ItemStatus::Completed);
    assert_eq!(wait(&run).completed_at, Some(Timestamp(resets + 1)));
    assert!(!matches!(
        run.lead().activity,
        Activity::WaitingForLimitReset { .. }
    ));
}

#[test]
fn a_reply_ends_the_wait_and_held_messages_follow_it() {
    let (mut run, resets) = at_the_limit();
    hold(&mut run, "held", "maxhq test: held");
    // A message that interrupts goes now, and starts a turn of its own.
    let fx = run.act(send("now", lead(), "maxhq test: now", SendMode::Interrupt));
    let written = only_write(&fx);
    assert_eq!(written["priority"], Value::Null);
    let uuid = written["uuid"].as_str().expect("a UUID").to_owned();
    run.feed(json!({"type": "command_lifecycle", "command_uuid": uuid, "state": "started"}));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    let fx = run.feed(system("init", json!({"model": "claude-haiku-5-5"})));
    assert!(fx.writes.is_empty(), "the held message waits for the turn");
    assert_eq!(wait(&run).status, ItemStatus::Running, "no reply yet");
    // A note the CLI writes itself isn't a reply.
    run.feed(line(json!({
        "type": "assistant",
        "parent_tool_use_id": null,
        "message": {"id": "msg-note", "role": "assistant", "model": "<synthetic>",
            "content": [{"type": "text", "text": "No response requested."}]},
    })));
    assert_eq!(wait(&run).status, ItemStatus::Running);
    let fx = run.feed(reply("a-now", "maxhq test: on it"));
    assert!(fx.writes.is_empty(), "the held message waits for the turn");
    let answered = run.clock.load(Ordering::SeqCst);
    assert_eq!(wait(&run).status, ItemStatus::Completed);
    assert_eq!(wait(&run).completed_at, Some(Timestamp(answered)));
    assert!(answered < resets);
    let fx = run.feed(result("res-now", json!({"duration_ms": 80})));
    let written = only_write(&fx);
    assert_eq!(
        format!("claude:{}", written["uuid"].as_str().expect("a UUID")),
        sent(&run, "maxhq test: held").id.0
    );
    assert_eq!(written["priority"], Value::Null);
}

#[test]
fn a_message_queued_while_the_turn_runs_goes_to_the_cli() {
    let mut run = mid_turn();
    let resets = run.clock.load(Ordering::SeqCst) / 1000 + 3600;
    run.feed(out_until(resets));
    run.feed(limit_error("limit-1"));
    let fx = run.act(send("busy", lead(), "maxhq test: busy", SendMode::Queue));
    assert_eq!(only_write(&fx)["priority"], "later");
    assert!(!resumes(&run));
}

#[test]
fn held_messages_and_the_wait_end_with_the_cli() {
    let (mut run, resets) = at_the_limit();
    hold(&mut run, "held", "maxhq test: held");
    run.exit(Some(1), None, "");
    let item = sent(&run, "maxhq test: held");
    assert_eq!(item.status, ItemStatus::Cancelled);
    assert_eq!(delivery(item), Delivery::Cancelled);
    assert!(!matches!(
        wait(&run).status,
        ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
    ));
    run.clock.store(resets + 1, Ordering::SeqCst);
    let fx = tick(&mut run);
    assert!(fx.writes.is_empty());
}

#[test]
fn a_failed_turn_says_why() {
    let mut run = mid_turn();
    let outcome = json!({"subtype": "error_max_turns", "is_error": true, "duration_ms": 900});
    run.feed(result("res-max", outcome));
    let error = run.screen.item("claude:res-max:error");
    assert_eq!(
        error.body,
        ItemBody::Error {
            message: "The turn hit its limit on steps.".to_owned(),
            retrying: false,
            sign_in: None,
        }
    );
    assert_eq!(error.turn, run.turn(FIRST));
    assert!(matches!(
        run.screen.item("claude:res-max").body,
        ItemBody::TurnSummary {
            duration_ms: Some(900),
            ..
        }
    ));

    let mut run = mid_turn();
    let outcome = json!({
        "subtype": "error_during_execution",
        "is_error": true,
        "errors": ["[ede_diagnostic] noise", "Tool crashed"],
    });
    run.feed(result("res-crash", outcome));
    assert!(matches!(
        &run.screen.item("claude:res-crash:error").body,
        ItemBody::Error { message, .. } if message == "Tool crashed"
    ));

    let mut run = Run::new();
    run.play(0..=2);
    let outcome = json!({"is_error": true, "startup_failure_reason": "No login found"});
    run.feed(result("res-start", outcome));
    assert!(matches!(
        &run.screen.item("claude:res-start:error").body,
        ItemBody::Error { message, sign_in: Some(named), .. }
            if message == "Claude Code couldn't start: No login found" && *named == account()
    ));
}

#[test]
fn a_login_that_ran_out_names_the_account_to_sign_in() {
    let mut run = mid_turn();
    let account = account();
    run.feed(line(json!({
        "type": "assistant",
        "uuid": "auth-1",
        "parent_tool_use_id": null,
        "error": "authentication_failed",
        "is_api_error_message": true,
        "api_error_status": 401,
        "message": {"id": "msg_auth", "role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "Failed to authenticate. API Error: 401 {}"}]},
    })));
    assert_eq!(
        run.screen.item("claude:auth-1").body,
        ItemBody::Error {
            message: "Failed to authenticate. API Error: 401 {}".to_owned(),
            retrying: false,
            sign_in: Some(account),
        }
    );
    let outcome = json!({
        "is_error": true,
        "terminal_reason": "api_error",
        "api_error_status": 401,
        "result": "Failed to authenticate. API Error: 401 {}",
    });
    run.feed(result("res-auth", outcome));
    assert!(matches!(
        run.screen.item("claude:res-auth:error").body,
        ItemBody::Error { sign_in: Some(named), .. } if named == account
    ));

    // A transcript spells the status in camel case.
    let mut run = mid_turn();
    run.feed(line(json!({
        "type": "assistant",
        "uuid": "auth-2",
        "parent_tool_use_id": null,
        "isApiErrorMessage": true,
        "apiErrorStatus": 401,
        "message": {"id": "msg_auth", "role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "API Error: 401"}]},
    })));
    assert!(matches!(
        run.screen.item("claude:auth-2").body,
        ItemBody::Error { sign_in: Some(named), .. } if named == account
    ));

    // Other failures name none.
    let mut run = mid_turn();
    let outcome = json!({"is_error": true, "terminal_reason": "api_error", "api_error_status": 500, "result": "API Error: 500"});
    run.feed(result("res-500", outcome));
    assert!(matches!(
        run.screen.item("claude:res-500:error").body,
        ItemBody::Error { sign_in: None, .. }
    ));
}

#[test]
fn an_api_error_without_a_limit_shows_its_text() {
    let mut run = mid_turn();
    run.feed(line(json!({
        "type": "assistant",
        "uuid": "api-1",
        "parent_tool_use_id": null,
        "error": "server_error",
        "isApiErrorMessage": true,
        "message": {"id": "msg_err", "role": "assistant", "model": "<synthetic>", "content": [{"type": "text", "text": "API Error: 500"}]},
    })));
    assert_eq!(
        run.screen.item("claude:api-1").body,
        ItemBody::Error {
            message: "API Error: 500".to_owned(),
            retrying: false,
            sign_in: None,
        }
    );
}

#[test]
fn a_retry_shows_once_and_goes_when_the_api_answers() {
    let mut run = mid_turn();
    let first = json!({"uuid": "retry-1", "attempt": 1, "max_retries": 10, "retry_delay_ms": 1500, "error_status": 529});
    run.feed(system("api_retry", first));
    let retry = run.screen.item("claude:retry-1");
    assert_eq!(retry.status, ItemStatus::Running);
    assert_eq!(
        retry.body,
        ItemBody::Error {
            message: "The API is overloaded. Claude retries in 2 s, attempt 1 of 10.".to_owned(),
            retrying: true,
            sign_in: None,
        }
    );
    let second = json!({"uuid": "retry-2", "attempt": 2, "max_retries": 10, "retry_delay_ms": 3000, "error_status": 500});
    run.feed(system("api_retry", second));
    assert!(run.screen.find("claude:retry-2").is_none());
    assert!(matches!(
        &run.screen.item("claude:retry-1").body,
        ItemBody::Error { message, .. }
            if message == "The API had a server error. Claude retries in 3 s, attempt 2 of 10."
    ));
    let fx = run.feed(tool_use("toolu_after", "Bash", json!({"command": "ls"})));
    let removed = fx.timeline.iter().any(
        |(_, event)| matches!(event, TimelineEvent::Remove { id } if id.0 == "claude:retry-1"),
    );
    assert!(removed);
    assert!(run.screen.find("claude:retry-1").is_none());
}

#[test]
fn notices_show_blocked_calls_sign_in_trouble_and_compaction() {
    let mut run = mid_turn();
    let blocked = json!({"uuid": "deny-1", "tool_name": "Bash", "decision_reason": "Rule Bash(rm:*) denies it"});
    run.feed(system("permission_denied", blocked));
    assert_eq!(
        run.screen.item("claude:deny-1").body,
        ItemBody::Notice {
            level: NoticeLevel::Warning,
            message: "Blocked Bash: Rule Bash(rm:*) denies it".to_owned(),
        }
    );
    run.feed(line(json!({"type": "auth_status", "uuid": "auth-1", "isAuthenticating": false, "error": "token expired"})));
    assert_eq!(
        run.screen.item("claude:auth-1").body,
        ItemBody::Notice {
            level: NoticeLevel::Warning,
            message: "Claude Code can't sign in: token expired".to_owned(),
        }
    );
    let meta = json!({"uuid": "compact-1", "compact_metadata": {"trigger": "auto", "pre_tokens": 150_000, "post_tokens": 12_000}});
    run.feed(system("compact_boundary", meta));
    assert_eq!(
        run.screen.item("claude:compact-1").body,
        ItemBody::Compaction {
            before_tokens: Some(150_000),
            after_tokens: Some(12_000),
        }
    );
}

#[test]
fn a_crash_ends_what_was_running_and_says_why() {
    let mut run = Run::new();
    run.play(0..=84);
    let fx = run.act(Action::Interrupt {
        key: key("stop it"),
        agent: lead(),
    });
    assert_eq!(only_write(&fx)["request"]["subtype"], "interrupt");
    let fx = run.act(send("later", lead(), "maxhq test: later", SendMode::Queue));
    let uuid = only_write(&fx)["uuid"].as_str().expect("a UUID").to_owned();
    let at = run.clock.load(Ordering::SeqCst);
    let fx = run.exit(Some(1), None, "warming up\nError: bad thing\n\n");
    let reason = "Claude exited with code 1: Error: bad thing".to_owned();
    assert_eq!(
        run.lead().link,
        AgentLink::Down {
            reason: reason.clone(),
            since: Some(Timestamp(at)),
        }
    );
    assert_eq!(run.lead().activity, Activity::Error { reason });
    assert_eq!(closed(&fx), [approval(WRITE_A_ASK)]);
    assert_eq!(
        fx.results,
        [(key("stop it"), refused("Claude ended before it answered."))]
    );
    let ask = run.item(&format!("claude:{WRITE_A_CALL}:approval"));
    assert_eq!(ask.status, ItemStatus::Cancelled);
    assert_eq!(
        run.item(&format!("claude:{WRITE_A_CALL}")).status,
        ItemStatus::Interrupted
    );
    let later = run.screen.item(&format!("claude:{uuid}"));
    assert_eq!(later.status, ItemStatus::Cancelled);
    assert!(matches!(
        later.body,
        ItemBody::UserMessage {
            delivery: Delivery::Cancelled,
            ..
        }
    ));
    let unfinished: Vec<&str> = run
        .screen
        .items
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
            )
        })
        .map(|item| item.id.0.as_str())
        .collect();
    assert!(unfinished.is_empty(), "{unfinished:?}");

    let fx = run.act(send("after", lead(), "maxhq test: hello?", SendMode::Queue));
    assert!(fx.writes.is_empty());
    assert_eq!(
        fx.results,
        [(key("after"), refused("Claude isn't running."))]
    );
    let fx = run.act(Action::Stop {
        key: key("stop"),
        agent: lead(),
    });
    assert!(fx.stop);
    assert_eq!(fx.results, [(key("stop"), ActionOutcome::Done)]);
}

#[test]
fn a_stop_you_asked_for_isnt_an_error() {
    let mut run = Run::new();
    run.play(0..=209);
    let fx = run.act(Action::Stop {
        key: key("stop"),
        agent: lead(),
    });
    assert!(fx.stop);
    assert!(fx.writes.is_empty());
    assert_eq!(fx.results, [(key("stop"), ActionOutcome::Done)]);
    run.exit(None, Some(15), "");
    let lead = run.lead();
    assert!(matches!(
        &lead.link,
        AgentLink::Down { reason, .. } if reason == "Claude was stopped by signal 15"
    ));
    assert_eq!(lead.activity, Activity::Idle);
}

#[test]
fn a_cli_that_went_down_with_its_container_isnt_an_error() {
    let reason = "Claude exited with code 143".to_owned();
    let mut run = mid_turn();
    run.went_down(Some(143));
    let lead = run.lead();
    assert!(matches!(&lead.link, AgentLink::Down { reason: why, .. } if *why == reason));
    assert_eq!(lead.activity, Activity::Idle);

    // The same end on a container that runs on is the CLI's own.
    let mut run = mid_turn();
    run.exit(Some(143), None, "");
    assert_eq!(run.lead().activity, Activity::Error { reason });
}

#[test]
fn a_tool_results_picture_too_large_to_decode_shows_without_a_preview() {
    use base64::Engine as _;
    let name = "drive::claude::tests::turns::a_tool_results_picture_too_large_to_decode_shows_without_a_preview";
    if crate::testing::apart(name) {
        return;
    }
    let mut run = Run::new();
    run.play(0..=9);
    let data = base64::engine::general_purpose::STANDARD.encode(crate::testing::HUGE_PNG);
    run.feed(tool_use(
        "toolu_huge",
        "Read",
        json!({"file_path": "/path/to/huge.png"}),
    ));
    let fx = run.feed(tool_result_blocks(
        "toolu_huge",
        json!([{
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": data},
        }]),
        json!({}),
    ));
    let ItemBody::FileRead { images, .. } = &run.screen.item("claude:toolu_huge").body else {
        panic!("not a read");
    };
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].thumbnail, None);
    let job = &fx.pictures.previews[0];
    assert_eq!(
        crate::thumbnail::thumbnail_of(&job.bytes, Some(job.mime)),
        None
    );
}
