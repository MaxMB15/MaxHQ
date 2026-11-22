// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Tests that drive a session with recorded lines.
//!
//! `turns.jsonl` is one live session with `claude -p` in stream-json mode:
//! each line `maxd` wrote, each line the CLI printed, and how it exited.
//! `transcript.jsonl` is the transcript the CLI wrote meanwhile. A run
//! plays the printed lines as held lines and turns each written line back
//! into the action that wrote it, then checks the driver writes the same
//! line. The driver makes its own request IDs and message UUIDs, so a run
//! keeps a map between its IDs and the recorded ones.

mod channel;
mod prompts;
mod release;
mod replay;
mod turns;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use maxhq_protocol::attention::{AttentionAnswer, AttentionEvent};
use maxhq_protocol::ids::{
    AccountId, AgentId, AttachmentId, AttentionId, ClientId, DeviceId, IdempotencyKey, WorkspaceId,
};
use maxhq_protocol::message::{ActionOutcome, SendMode};
use maxhq_protocol::model::{Agent, PermissionMode, Usage};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    AttachmentKind, AttachmentRef, ItemBody, TextField, TimelineEvent, TimelineItem, TurnId,
};
use serde_json::{json, Value};

use super::{wire, ClaudeSession};
use crate::drive::{
    Action, AttentionChange, ChildExit, Driver, Effects, HeldLine, Job, Login, LoginAuth, Place,
    PlaceHost, Staged, ToolDone, ToolResult, Write,
};
use crate::testing::TempDir;

/// The recorded session, one record a line.
const TURNS: &str = include_str!("../../../fixtures/claude-m2/turns.jsonl");

/// The transcript the CLI wrote during the recorded session.
const TRANSCRIPT: &str = include_str!("../../../fixtures/claude-m2/transcript.jsonl");

/// The image the recorded session sent.
const IMAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/claude-m2/red.png");

/// The recorded session's ID.
const SESSION: &str = "fa807b40-8646-4b65-902e-8b0f6f54f532";

/// Where the recorded session ran.
const CWD: &str = "/tmp/maxhq-test/work";

/// The folder under `projects` that holds its transcript.
const PROJECT: &str = "-tmp-maxhq-test-work";

/// The first message: five tool calls and a reply.
const FIRST: &str = "77e75cbe-540d-4766-b42a-9269caea5d52";

/// The message with the image.
const PICTURE: &str = "ac8546b1-10d4-42f7-a02b-208dc381e3c5";

/// The message the interrupt cut short.
const COUNT: &str = "58f89355-e1d4-46e6-b343-513d7f4d895a";

/// The message queued while `COUNT` ran.
const QUEUED: &str = "675f6ca2-0438-4082-9a72-6bcb7d5c3f7f";

/// The request to write a.txt, answered allow.
const WRITE_A_ASK: &str = "65313252-8cb9-4d7c-98bf-880b8a318071";

/// The request to edit seed.txt, answered allow.
const EDIT_ASK: &str = "ebbd8683-7bb2-4562-ba77-181ceb65b78c";

/// The request to write b.txt, denied with a message.
const WRITE_B_ASK: &str = "968ab53c-5051-401f-8c8e-2e1ffc6eaee0";

/// The Read call.
const READ_CALL: &str = "toolu_01MYLJDTrSRuCvqAeYMdZJKa";

/// The Bash call that ran `ls`.
const LS_CALL: &str = "toolu_01Ag9atEJMQxDiv4BzTTAcFQ";

/// The Write call for a.txt.
const WRITE_A_CALL: &str = "toolu_016YufgEcmLXHYkcsfA1LtDK";

/// The Edit call for seed.txt.
const EDIT_CALL: &str = "toolu_01TTtmrxw2XWTCyqgG3eXo6y";

/// The Write call for b.txt.
const WRITE_B_CALL: &str = "toolu_01RKsp9yMU5zozpCbs6tGyFE";

/// The reply that ends the first turn.
const SUMMARY_TEXT: &str = "claude:msg_011CgNkCVSGJNigU17A367YF:1";

/// The count the interrupt cut short.
const COUNT_TEXT: &str = "claude:msg_011CgNkCmwhtLw9Hdzh9AxiR:0";

/// The marker the CLI adds after the interrupt.
const MARKER: &str = "claude:3e7b2236-b2a5-437e-8ff8-554ea3ae62fd";

/// The result of the first turn.
const FIRST_RESULT: &str = "fc582d29-a909-4e39-b044-2729a5bb60c8";

/// The result of the turn the interrupt cut short.
const CUT_RESULT: &str = "1f77a26c-ded7-4f53-a816-76014dc1ee79";

/// The index of the last record, where the CLI exits.
const LAST: usize = 271;

/// The lead agent.
fn lead() -> AgentId {
    AgentId::derive("claude lead")
}

/// The account the session runs under.
fn account() -> AccountId {
    AccountId::derive("claude max")
}

/// The client that answers.
fn phone() -> ClientId {
    ClientId::derive("phone")
}

/// A key for an action.
fn key(name: &str) -> IdempotencyKey {
    IdempotencyKey::derive(name)
}

/// The key of the action that wrote record `at`.
fn record_key(at: usize) -> IdempotencyKey {
    key(&format!("record {at}"))
}

/// The attention event for a permission request.
fn approval(request: &str) -> AttentionId {
    AttentionId::derive(&format!("claude:{request}"))
}

/// Where the session runs.
fn place() -> Place {
    Place {
        device: DeviceId::derive("laptop"),
        workspace: WorkspaceId::derive("work"),
        cwd: PathBuf::from(CWD),
        attachments: PathBuf::from("/tmp/maxhq-test/attachments"),
        host: PlaceHost::Local,
        dispatcher: false,
        trust: None,
        claude_artifacts: true,
    }
}

/// The image, staged.
fn red_png() -> Staged {
    let bytes = fs::metadata(IMAGE).expect("the fixture image").len();
    Staged {
        attachment: AttachmentRef {
            id: AttachmentId::derive("red.png"),
            name: "red.png".to_owned(),
            kind: AttachmentKind::Image,
            bytes,
            mime: Some("image/png".to_owned()),
            path: "/client/red.png".to_owned(),
            thumbnail: None,
        },
        path: PathBuf::from(IMAGE),
        text: None,
    }
}

/// The holder's sequence number for record `at`, with room between
/// records for made-up lines.
fn seq_of(at: usize) -> u64 {
    (at as u64 + 1) * 100
}

/// When a record happened.
fn stamp(record: &Value) -> u64 {
    record["at"].as_u64().expect("a record's time")
}

/// The recorded session's records.
fn records() -> Vec<Value> {
    TURNS
        .lines()
        .map(|line| serde_json::from_str(line).expect("a record"))
        .collect()
}

/// Adds what `fx` holds to `into`.
fn merge(into: &mut Effects, fx: Effects) {
    into.writes.extend(fx.writes);
    into.history.extend(fx.history);
    into.timeline.extend(fx.timeline);
    into.agents.extend(fx.agents);
    into.gone.extend(fx.gone);
    into.attention.extend(fx.attention);
    into.usage.extend(fx.usage);
    into.models.extend(fx.models);
    into.results.extend(fx.results);
    into.calls.extend(fx.calls);
    into.jobs.extend(fx.jobs);
    into.wake_at = fx.wake_at;
    into.stop |= fx.stop;
}

/// A line from the CLI, given a UUID and the session's ID if it has none.
fn line(mut value: Value) -> Value {
    if let Value::Object(fields) = &mut value {
        fields
            .entry("uuid")
            .or_insert_with(|| json!(uuid::Uuid::now_v7().to_string()));
        fields.entry("session_id").or_insert_with(|| json!(SESSION));
    }
    value
}

/// `value` with `fields` added.
fn with(mut value: Value, fields: Value) -> Value {
    if let (Value::Object(into), Value::Object(fields)) = (&mut value, fields) {
        into.extend(fields);
    }
    value
}

/// `value` as a line from inside the sub-agent that `call` started.
fn under(value: Value, call: &str) -> Value {
    with(value, json!({"parent_tool_use_id": call}))
}

/// An `assistant` line that calls a tool.
fn tool_use(call: &str, name: &str, input: Value) -> Value {
    line(json!({
        "type": "assistant",
        "parent_tool_use_id": null,
        "message": {
            "id": format!("msg_{call}"),
            "type": "message",
            "role": "assistant",
            "model": "claude-haiku-5-5",
            "content": [{"type": "tool_use", "id": call, "name": name, "input": input}],
        },
    }))
}

/// A `user` line with a tool's result.
fn tool_result(call: &str, content: &str, result: Value) -> Value {
    line(json!({
        "type": "user",
        "parent_tool_use_id": null,
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": call,
                "content": content,
                "is_error": false,
            }],
        },
        "tool_use_result": result,
    }))
}

/// A tool result the CLI marks as an error.
fn failed(mut line: Value) -> Value {
    line["message"]["content"][0]["is_error"] = json!(true);
    line
}

/// A `result` line.
fn result(uuid: &str, fields: Value) -> Value {
    line(with(
        json!({"type": "result", "subtype": "success", "is_error": false, "uuid": uuid}),
        fields,
    ))
}

/// A message sent from the phone.
fn send(name: &str, agent: AgentId, text: &str, mode: SendMode) -> Action {
    Action::Send {
        key: key(name),
        agent,
        text: text.to_owned(),
        refs: Vec::new(),
        attachments: Vec::new(),
        mode,
    }
}

/// A `can_use_tool` request.
fn can_use_tool(request: &str, call: &str, name: &str, input: Value, suggestions: Value) -> Value {
    json!({
        "type": "control_request",
        "request_id": request,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": name,
            "input": input,
            "tool_use_id": call,
            "permission_suggestions": suggestions,
        },
    })
}

/// A `system` line.
fn system(subtype: &str, fields: Value) -> Value {
    line(with(json!({"type": "system", "subtype": subtype}), fields))
}

/// The attention events `fx` opened.
fn opened(fx: &Effects) -> Vec<AttentionEvent> {
    fx.attention
        .iter()
        .filter_map(|change| match change {
            AttentionChange::Open(event) => Some(event.clone()),
            AttentionChange::Close(_) => None,
        })
        .collect()
}

/// The attention events `fx` closed.
fn closed(fx: &Effects) -> Vec<AttentionId> {
    fx.attention
        .iter()
        .filter_map(|change| match change {
            AttentionChange::Close(id) => Some(*id),
            AttentionChange::Open(_) => None,
        })
        .collect()
}

/// The items `fx` sent whole with this ID.
fn upserts<'a>(fx: &'a Effects, id: &str) -> Vec<&'a TimelineItem> {
    fx.timeline
        .iter()
        .filter_map(|(_, event)| match event {
            TimelineEvent::Upsert { item } if item.id.0 == id => Some(item),
            _ => None,
        })
        .collect()
}

/// Whether `fx` sent any appends.
fn appends(fx: &Effects) -> usize {
    fx.timeline
        .iter()
        .filter(|(_, event)| matches!(event, TimelineEvent::Append { .. }))
        .count()
}

/// The one line `fx` wrote, parsed.
fn only_write(fx: &Effects) -> Value {
    assert_eq!(
        fx.writes.len(),
        1,
        "expected one write, got {:?}",
        fx.writes
    );
    serde_json::from_str(&fx.writes[0].line).expect("a JSON line")
}

/// The answer a write carries, as the CLI reads it.
fn answer_body(fx: &Effects) -> Value {
    let line = only_write(fx);
    assert_eq!(line["type"], "control_response");
    assert_eq!(line["response"]["subtype"], "success");
    line["response"]["response"].clone()
}

/// The `initialize` a reconnect sends, which reads the CLI's state back
/// and names MaxHQ's tool server.
fn bare_initialize() -> Value {
    json!({
        "subtype": "initialize",
        "sdkMcpServers": ["maxhq"],
        "sdkMcpServerConfigs": {"maxhq": {"timeout": 3_600_000}},
    })
}

/// The request a reconnect sends after `initialize`, so the CLI reads
/// MaxHQ's tool list again.
fn tools_changed() -> Value {
    json!({
        "subtype": "mcp_message",
        "server_name": "maxhq",
        "message": {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"},
    })
}

/// The request each write carries, in order.
fn requests(fx: &Effects) -> Vec<Value> {
    fx.writes
        .iter()
        .map(|write| {
            let line: Value = serde_json::from_str(&write.line).expect("a JSON line");
            line["request"].clone()
        })
        .collect()
}

/// A refusal with this reason.
fn refused(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

/// What a client shows, built only from the effects a driver sends.
#[derive(Default)]
struct Screen {
    /// Every item, in the order it first showed.
    items: Vec<TimelineItem>,
    /// Every agent shown.
    agents: HashMap<AgentId, Agent>,
    /// Attention events open now.
    open: HashMap<AttentionId, AttentionEvent>,
    /// Each event that opened, in order.
    opened: Vec<AttentionId>,
    /// Each event that closed, in order.
    closed: Vec<AttentionId>,
    /// Each line written.
    writes: Vec<Write>,
    /// Each action's outcome.
    results: Vec<(IdempotencyKey, ActionOutcome)>,
    /// Each usage report.
    usage: Vec<Usage>,
    /// Agents that went.
    gone: Vec<AgentId>,
}

impl Screen {
    /// Applies effects the way the session manager does.
    fn apply(&mut self, fx: &Effects) {
        self.writes.extend(fx.writes.iter().cloned());
        for (agent, items) in &fx.history {
            for item in items {
                assert_eq!(item.agent, *agent, "{} is in the wrong history", item.id.0);
                if self.find(&item.id.0).is_none() {
                    self.items.push(item.clone());
                }
            }
        }
        for (agent, event) in &fx.timeline {
            self.event(*agent, event);
        }
        for agent in &fx.agents {
            self.agents.insert(agent.id, agent.clone());
        }
        for agent in &fx.gone {
            self.agents.remove(agent);
            self.gone.push(*agent);
        }
        for change in &fx.attention {
            match change {
                AttentionChange::Open(event) => {
                    let again = self.open.insert(event.id, event.clone());
                    assert!(again.is_none(), "{:?} opened twice", event.id);
                    self.opened.push(event.id);
                }
                AttentionChange::Close(id) => {
                    assert!(
                        self.open.remove(id).is_some(),
                        "{id:?} closed but wasn't open"
                    );
                    self.closed.push(*id);
                }
            }
        }
        self.usage
            .extend(fx.usage.iter().map(|(_, usage)| usage.clone()));
        self.results.extend(fx.results.iter().cloned());
    }

    /// Applies one timeline event.
    fn event(&mut self, agent: AgentId, event: &TimelineEvent) {
        match event {
            TimelineEvent::Upsert { item } => {
                assert_eq!(
                    item.agent, agent,
                    "{} went to the wrong timeline",
                    item.id.0
                );
                assert_eq!(item.ordinal, 0, "a driver leaves ordering to the manager");
                match self.items.iter_mut().find(|shown| shown.id == item.id) {
                    Some(shown) => *shown = item.clone(),
                    None => self.items.push(item.clone()),
                }
            }
            TimelineEvent::Append { id, field, text } => {
                let Some(item) = self.items.iter_mut().find(|shown| shown.id == *id) else {
                    panic!("an append to {} came before its item", id.0);
                };
                assert_eq!(item.agent, agent, "an append to {} went astray", id.0);
                match (&mut item.body, field) {
                    (
                        ItemBody::AssistantMessage { text: shown, .. }
                        | ItemBody::Reasoning { text: shown, .. }
                        | ItemBody::Plan {
                            markdown: shown, ..
                        },
                        TextField::Text,
                    )
                    | (ItemBody::Command { output: shown, .. }, TextField::Output) => {
                        shown.push_str(text);
                    }
                    (body, field) => panic!("can't append {field:?} to {body:?}"),
                }
            }
            TimelineEvent::Remove { id } => {
                let before = self.items.len();
                self.items.retain(|shown| shown.id != *id);
                assert_eq!(
                    self.items.len() + 1,
                    before,
                    "{} went before it showed",
                    id.0
                );
            }
        }
    }

    /// The item with this ID.
    fn item(&self, id: &str) -> &TimelineItem {
        self.find(id)
            .unwrap_or_else(|| panic!("no item {id}, only {:?}", self.ids()))
    }

    /// The item with this ID, if it showed.
    fn find(&self, id: &str) -> Option<&TimelineItem> {
        self.items.iter().find(|item| item.id.0 == id)
    }

    /// Every item's ID.
    fn ids(&self) -> Vec<&str> {
        self.items.iter().map(|item| item.id.0.as_str()).collect()
    }

    /// The agent with this ID.
    fn agent(&self, id: AgentId) -> &Agent {
        self.agents
            .get(&id)
            .unwrap_or_else(|| panic!("no agent {id:?}"))
    }
}

/// A driver playing the recorded session.
struct Run {
    /// The driver.
    driver: ClaudeSession,
    /// The time the driver reads, in milliseconds.
    clock: Arc<AtomicU64>,
    /// The Claude config folder, which may hold a transcript, and the
    /// agent's attachments folder. It's kept so the folder lasts as long
    /// as the run.
    config: TempDir,
    /// The recorded session.
    records: Vec<Value>,
    /// Each recorded ID and the one this run made in its place.
    ids: Vec<(String, String)>,
    /// What a client shows.
    screen: Screen,
    /// The last sequence number given to a line.
    seq: u64,
    /// The jobs the driver asked for that wait to run, while the test
    /// holds them back.
    held_jobs: Option<Vec<Job>>,
}

impl Run {
    /// A run with a fresh driver and an empty config folder.
    fn new() -> Self {
        let records = records();
        let now = stamp(&records[0]);
        Self::with(TempDir::new(), Vec::new(), now, records)
    }

    /// A fresh driver that has played every record.
    fn full() -> Self {
        let mut run = Self::new();
        run.play(0..=LAST);
        run
    }

    /// A run over `config`, which knows `ids`, at `now`.
    fn with(config: TempDir, ids: Vec<(String, String)>, now: u64, records: Vec<Value>) -> Self {
        let clock = Arc::new(AtomicU64::new(now));
        let tick = Arc::clone(&clock);
        let login = Login {
            account: account(),
            config_dir: config.path().to_path_buf(),
            auth: LoginAuth::ConfigDir,
        };
        // Pictures in tool results land in the run's own folder.
        let place = Place {
            attachments: config.path().join("attachments").join(lead().to_string()),
            ..place()
        };
        let driver = ClaudeSession::new(login, place, lead(), PermissionMode::Ask)
            .with_clock(move || Timestamp(tick.load(Ordering::SeqCst)));
        Self {
            driver,
            clock,
            config,
            records,
            ids,
            screen: Screen::default(),
            seq: 0,
            held_jobs: None,
        }
    }

    /// A new driver for the same session, as after `maxd` restarts while
    /// the holder keeps the CLI running. The CLI had written the first
    /// `records` lines of its transcript by then.
    fn after_restart(before: &Run, records: usize) -> Self {
        let config = TempDir::new();
        let folder = config.path().join("projects").join(PROJECT);
        fs::create_dir_all(&folder).expect("a projects folder");
        let lines: Vec<String> = TRANSCRIPT
            .lines()
            .take(records)
            .map(|line| before.ours(line))
            .collect();
        let transcript = folder.join(format!("{SESSION}.jsonl"));
        fs::write(transcript, lines.join("\n") + "\n").expect("a transcript");
        let now = before.clock.load(Ordering::SeqCst);
        let mut run = Self::with(config, before.ids.clone(), now, before.records.clone());
        run.seq = before.seq;
        run
    }

    /// `text` with the recorded IDs swapped for this run's.
    fn ours(&self, text: &str) -> String {
        self.ids
            .iter()
            .fold(text.to_owned(), |text, (recorded, ours)| {
                text.replace(recorded, ours)
            })
    }

    /// `text` with this run's IDs swapped for the recorded ones.
    fn recorded(&self, text: &str) -> String {
        self.ids
            .iter()
            .fold(text.to_owned(), |text, (recorded, ours)| {
                text.replace(ours, recorded)
            })
    }

    /// Notes that this run made `ours` where the recording has `recorded`.
    fn learn(&mut self, ours: &str, recorded: &str) {
        if ours != recorded && !self.ids.iter().any(|(seen, _)| seen == recorded) {
            self.ids.push((recorded.to_owned(), ours.to_owned()));
        }
    }

    /// The turn a recorded message started.
    fn turn(&self, prompt: &str) -> Option<TurnId> {
        Some(TurnId(format!("claude:{}", self.ours(prompt))))
    }

    /// The item with a recorded ID.
    fn item(&self, recorded: &str) -> &TimelineItem {
        self.screen.item(&self.ours(recorded))
    }

    /// The item for a recorded message.
    fn message(&self, prompt: &str) -> &TimelineItem {
        self.item(&format!("claude:{prompt}"))
    }

    /// The lead as shown.
    fn lead(&self) -> &Agent {
        self.screen.agent(lead())
    }

    /// Plays one record.
    fn step(&mut self, at: usize) -> Effects {
        let record = self.records[at].clone();
        let time = stamp(&record);
        self.clock.store(time, Ordering::SeqCst);
        self.seq = self.seq.max(seq_of(at));
        match record["dir"].as_str() {
            Some("out") => {
                let text = self.ours(&record["msg"].to_string());
                self.held(HeldLine {
                    seq: seq_of(at),
                    at: Timestamp(time),
                    text,
                    replayed: false,
                })
            }
            Some("in") => self.input(at),
            Some("exit") => {
                let code = record["code"]
                    .as_i64()
                    .and_then(|code| i32::try_from(code).ok());
                self.exit(code, None, "")
            }
            other => panic!("record {at} goes {other:?}"),
        }
    }

    /// Plays each record in `range`.
    fn play(&mut self, range: RangeInclusive<usize>) -> Effects {
        let mut all = Effects::default();
        for at in range {
            let fx = self.step(at);
            merge(&mut all, fx);
        }
        all
    }

    /// Plays the CLI's lines in `range` again, as a holder replays them on
    /// a reconnect.
    fn replay(&mut self, range: RangeInclusive<usize>) -> Effects {
        let mut all = Effects::default();
        for at in range {
            if self.records[at]["dir"] != "out" {
                continue;
            }
            let line = HeldLine {
                seq: seq_of(at),
                at: Timestamp(stamp(&self.records[at])),
                text: self.ours(&self.records[at]["msg"].to_string()),
                replayed: true,
            };
            let fx = self.held(line);
            merge(&mut all, fx);
        }
        all
    }

    /// Passes one held line to the driver.
    fn held(&mut self, line: HeldLine) -> Effects {
        let mut fx = Effects::default();
        self.driver.line(&line, &mut fx);
        match &mut self.held_jobs {
            Some(held) => held.append(&mut fx.jobs),
            None => self.finish_jobs(&mut fx),
        }
        self.screen.apply(&fx);
        fx
    }

    /// Holds back the jobs the driver asks for from now on, as when they
    /// take a while.
    fn hold_jobs(&mut self) {
        self.held_jobs = Some(Vec::new());
    }

    /// Runs the jobs held back, and every one after them.
    fn release_jobs(&mut self) -> Effects {
        let mut fx = Effects {
            jobs: self.held_jobs.take().unwrap_or_default(),
            ..Effects::default()
        };
        self.finish_jobs(&mut fx);
        self.screen.apply(&fx);
        fx
    }

    /// Runs the jobs the driver asked for, as the manager does off the
    /// loop, and hands each one's result back.
    fn finish_jobs(&mut self, fx: &mut Effects) {
        while !fx.jobs.is_empty() {
            for job in std::mem::take(&mut fx.jobs) {
                let mut more = Effects::default();
                self.driver.job_done(job.run(), &mut more);
                merge(fx, more);
            }
        }
    }

    /// Passes a made-up line from the CLI, a millisecond after the last.
    fn feed(&mut self, value: Value) -> Effects {
        self.feed_with(value, false)
    }

    /// Passes a made-up line, replayed or not.
    fn feed_with(&mut self, value: Value, replayed: bool) -> Effects {
        self.seq += 1;
        let at = self.clock.fetch_add(1, Ordering::SeqCst) + 1;
        self.held(HeldLine {
            seq: self.seq,
            at: Timestamp(at),
            text: value.to_string(),
            replayed,
        })
    }

    /// Passes an action to the driver.
    fn act(&mut self, action: Action) -> Effects {
        let mut fx = Effects::default();
        self.driver.act(action, &mut fx);
        self.screen.apply(&fx);
        fx
    }

    /// Tells the driver a call to MaxHQ's tools ended.
    fn tool_done(&mut self, key: &str, result: Result<ToolResult, String>) -> Effects {
        let mut fx = Effects::default();
        let done = ToolDone {
            key: key.to_owned(),
            result,
        };
        self.driver.tool_done(done, &mut fx);
        self.screen.apply(&fx);
        fx
    }

    /// Answers an attention event from the phone.
    fn answer(&mut self, name: &str, event: AttentionId, answer: AttentionAnswer) -> Effects {
        self.act(Action::Answer {
            key: key(name),
            event,
            answer,
            by: phone(),
            held_back: false,
        })
    }

    /// Picks an option on an attention event.
    fn pick(&mut self, name: &str, event: AttentionId, option: &str) -> Effects {
        let answer = AttentionAnswer::Option {
            id: option.to_owned(),
        };
        self.answer(name, event, answer)
    }

    /// Connects to the holder, which still waits on the requests in the
    /// records at `pending`.
    fn connect(&mut self, fresh: bool, pending: &[usize]) -> Effects {
        let lines: Vec<HeldLine> = pending
            .iter()
            .map(|&at| HeldLine {
                seq: seq_of(at),
                at: Timestamp(stamp(&self.records[at])),
                text: self.ours(&self.records[at]["msg"].to_string()),
                replayed: true,
            })
            .collect();
        let mut fx = Effects::default();
        self.driver.connected(fresh, &lines, &mut fx);
        self.screen.apply(&fx);
        fx
    }

    /// Tells the driver the CLI ended.
    fn exit(&mut self, code: Option<i32>, signal: Option<i32>, stderr: &str) -> Effects {
        let exit = ChildExit {
            code,
            signal,
            at: Timestamp(self.clock.load(Ordering::SeqCst)),
            stderr_tail: stderr.to_owned(),
            with_machine: false,
        };
        self.end(&exit)
    }

    /// Tells the driver the CLI went down with its container.
    fn went_down(&mut self, code: Option<i32>) -> Effects {
        let exit = ChildExit {
            code,
            signal: None,
            at: Timestamp(self.clock.load(Ordering::SeqCst)),
            stderr_tail: String::new(),
            with_machine: true,
        };
        self.end(&exit)
    }

    /// Tells the driver how the CLI ended.
    fn end(&mut self, exit: &ChildExit) -> Effects {
        let mut fx = Effects::default();
        self.driver.exited(exit, &mut fx);
        self.screen.apply(&fx);
        fx
    }

    /// The records before `before` that hold a request from the CLI no
    /// one had answered or withdrawn.
    fn unanswered(&self, before: usize) -> Vec<usize> {
        let ended: Vec<&str> = self.records[..before]
            .iter()
            .filter_map(|record| {
                let msg = &record["msg"];
                match (record["dir"].as_str(), msg["type"].as_str()) {
                    (Some("in"), Some("control_response")) => {
                        msg["response"]["request_id"].as_str()
                    }
                    (Some("out"), Some("control_cancel_request")) => msg["request_id"].as_str(),
                    _ => None,
                }
            })
            .collect();
        (0..before)
            .filter(|&at| {
                let record = &self.records[at];
                record["dir"] == "out"
                    && record["msg"]["type"] == "control_request"
                    && !ended.contains(&record["msg"]["request_id"].as_str().unwrap_or_default())
            })
            .collect()
    }

    /// Turns a line `maxd` wrote back into the action that wrote it, and
    /// checks the driver writes the same line.
    fn input(&mut self, at: usize) -> Effects {
        let msg = self.records[at]["msg"].clone();
        let fx = match msg["type"].as_str() {
            Some("control_request") => self.request(at, &msg),
            Some("control_response") => {
                let request = msg["response"]["request_id"]
                    .as_str()
                    .expect("a request ID");
                let given = &msg["response"]["response"];
                let answer = match given["behavior"].as_str() {
                    Some("allow") => AttentionAnswer::Option {
                        id: "allow".to_owned(),
                    },
                    Some("deny") => AttentionAnswer::Text {
                        text: given["message"].as_str().unwrap_or_default().to_owned(),
                    },
                    other => panic!("record {at} answers {other:?}"),
                };
                self.act(Action::Answer {
                    key: record_key(at),
                    event: approval(request),
                    answer,
                    by: phone(),
                    held_back: false,
                })
            }
            Some("user") => {
                let mode = match msg["priority"].as_str() {
                    Some("next") => SendMode::Steer,
                    Some("now") => SendMode::Interrupt,
                    _ => SendMode::Queue,
                };
                let blocks = msg["message"]["content"].as_array().expect("blocks");
                let attachments = if blocks.len() > 1 {
                    vec![red_png()]
                } else {
                    Vec::new()
                };
                self.act(Action::Send {
                    key: record_key(at),
                    agent: lead(),
                    text: blocks[0]["text"].as_str().expect("text").to_owned(),
                    refs: Vec::new(),
                    attachments,
                    mode,
                })
            }
            other => panic!("record {at} writes {other:?}"),
        };
        assert_eq!(fx.writes.len(), 1, "record {at} wrote {:?}", fx.writes);
        let written = &fx.writes[0];
        let sent: Value = serde_json::from_str(&written.line).expect("a JSON line");
        match msg["type"].as_str() {
            Some("control_request") => self.learn(
                sent["request_id"].as_str().expect("an ID"),
                msg["request_id"].as_str().expect("an ID"),
            ),
            Some("user") => self.learn(
                sent["uuid"].as_str().expect("a UUID"),
                msg["uuid"].as_str().expect("a UUID"),
            ),
            _ => {}
        }
        let mut sent: Value = serde_json::from_str(&self.recorded(&written.line)).expect("JSON");
        if sent["request"]["subtype"] == "initialize" {
            // The recording came before MaxHQ's tool server.
            let request = sent["request"].as_object_mut().expect("a request");
            assert_eq!(request.remove("sdkMcpServers"), Some(json!(["maxhq"])));
            assert!(request.remove("sdkMcpServerConfigs").is_some());
        }
        if let Some(blocks) = sent
            .pointer_mut("/message/content")
            .and_then(Value::as_array_mut)
            && blocks.len() > 1
        {
            // The recording sent no line naming the image.
            let said = msg["message"]["content"][0]["text"].as_str().expect("text");
            let listed = format!("{said}\n\n[Attached image \"red.png\" is saved at: {IMAGE}]");
            assert_eq!(blocks[0]["text"], json!(listed));
            blocks[0]["text"] = json!(said);
        }
        assert_eq!(sent, msg, "record {at}");
        let answers = (msg["type"] == "control_response").then(|| {
            msg["response"]["request_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        });
        assert_eq!(written.answers, answers, "record {at}");
        fx
    }

    /// Turns a request `maxd` sent back into its action.
    fn request(&mut self, at: usize, msg: &Value) -> Effects {
        let request = &msg["request"];
        match request["subtype"].as_str() {
            Some("initialize") if request.get("forwardSubagentText").is_some() => {
                self.connect(true, &[])
            }
            Some("initialize") => {
                let pending = self.unanswered(at);
                let mut fx = self.connect(false, &pending);
                // The recording came before the notice that MaxHQ's tools
                // changed.
                assert_eq!(requests(&fx).get(1), Some(&tools_changed()));
                fx.writes.remove(1);
                fx
            }
            Some("set_permission_mode") => {
                let mode = request["mode"].as_str().and_then(wire::mode_from);
                self.act(Action::SetPermission {
                    key: record_key(at),
                    agent: lead(),
                    mode: mode.expect("a mode"),
                })
            }
            Some("interrupt") => self.act(Action::Interrupt {
                key: record_key(at),
                agent: lead(),
            }),
            other => panic!("record {at} asks {other:?}"),
        }
    }

    /// Feeds the CLI's reply to a request the driver wrote.
    fn reply(&mut self, written: &Value, body: Value) -> Effects {
        let id = written["request_id"].as_str().expect("a request ID");
        self.feed(json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": id, "response": body},
        }))
    }

    /// Feeds the CLI's refusal of a request the driver wrote.
    fn reply_error(&mut self, written: &Value, error: &str) -> Effects {
        let id = written["request_id"].as_str().expect("a request ID");
        self.feed(json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": id, "error": error},
        }))
    }

    /// An item as JSON with the recorded IDs and no times, which differ
    /// between runs.
    fn normal(&self, item: &TimelineItem) -> Value {
        let text = serde_json::to_string(item).expect("an item serializes");
        let mut value: Value = serde_json::from_str(&self.recorded(&text)).expect("JSON");
        value["started_at"] = Value::Null;
        value["completed_at"] = Value::Null;
        if value["decided"].is_object() {
            value["decided"]["at"] = json!(0);
        }
        value
    }

    /// Every item shown, by recorded ID.
    fn normal_items(&self) -> BTreeMap<String, Value> {
        self.screen
            .items
            .iter()
            .map(|item| (self.recorded(&item.id.0), self.normal(item)))
            .collect()
    }
}
