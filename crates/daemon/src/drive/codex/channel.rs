// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's own tools, which `thread/start` gives each thread as dynamic
//! tools. The thread keeps them through every resume. A call comes as an
//! `item/tool/call` request on the pipe the holder already keeps, runs off
//! the loop, and is answered when it ends.
//!
//! The holder keeps an unanswered request and gives it back on a
//! reconnect. A call still running is left to finish, and one that ended
//! while no one held the pipe runs again and shows on the same item.
//! Calls end with the app-server that asked for them.

use maxhq_core::tools::{self, Tool};
use maxhq_protocol::agents::ToolImage;
use maxhq_protocol::ids::AgentId;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{ItemBody, ItemId, ItemStatus, TimelineItem, TurnId};
use serde_json::{json, Value};

use super::{items, rpc, upsert, CodexServer};
use crate::drive::{Effects, ToolCall, ToolDone, ToolResult, Write};

/// A call to MaxHQ's tools that runs now.
#[derive(Clone, Debug)]
pub struct Running {
    /// The app-server's ID for the request, which the answer repeats.
    id: Value,
    /// The holder's key for the request.
    key: String,
    /// The thread that called.
    thread: String,
    /// The turn the call is in.
    turn: Option<String>,
    /// The message sent with the files, for a send.
    message: Option<String>,
    /// When the call came.
    at: Timestamp,
}

impl CodexServer {
    /// Takes an `item/tool/call` request from `thread`. A call whose tool
    /// or arguments don't check out fails at once, as a tool error.
    pub(super) fn tool_call(
        &mut self,
        id: Value,
        key: String,
        thread: String,
        params: &Value,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        let name = params.get("tool").and_then(Value::as_str).unwrap_or("");
        let namespace = params.get("namespace").and_then(Value::as_str);
        // A thread calls only the tools its place gives it.
        let served = self
            .threads
            .get(&thread)
            .and_then(|known| known.place.as_ref())
            .map_or(&Tool::ALL[..], |place| place.tools());
        let parsed = match Tool::from_codex(namespace, name).filter(|tool| served.contains(tool)) {
            Some(tool) => tool.parse(params.get("arguments").unwrap_or(&Value::Null)),
            None => Err(format!("MaxHQ has no tool {name:?}.")),
        };
        let call_id = params
            .get("callId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let (call, call_id) = match (parsed, call_id) {
            (Ok(call), Some(call_id)) => (call, call_id),
            (Ok(_), None) => {
                let items = content_items("The call has no ID.", &[]);
                self.tool_reply(&id, &key, items, false, fx);
                return;
            }
            (Err(reason), _) => {
                self.tool_reply(&id, &key, content_items(&reason, &[]), false, fx);
                return;
            }
        };
        self.pending.insert(key.clone());
        if let Some(running) = self.tool_calls.get_mut(&call_id) {
            running.id = id;
            running.key = key;
            return;
        }
        let Some(agent) = self.threads.get(&thread).map(|known| known.agent) else {
            return;
        };
        let message = call.message();
        fx.calls.push(ToolCall {
            arguments: params.get("arguments").cloned().unwrap_or(Value::Null),
            key: call_id.clone(),
            call_id: call_id.clone(),
            agent,
            call,
        });
        let turn = params
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.tool_calls.insert(
            call_id,
            Running {
                id,
                key,
                thread,
                turn,
                message,
                at,
            },
        );
    }

    /// Answers a call that ended, and shows what a send sent or an open
    /// opened.
    pub(super) fn call_done(&mut self, done: ToolDone, fx: &mut Effects) {
        let Some(running) = self.tool_calls.remove(&done.key) else {
            return;
        };
        let (said, images, success) = match done.result {
            Ok(ToolResult::Sent(attachments)) => {
                let said = tools::sent_text(&attachments);
                let body = ItemBody::AgentAttachments {
                    text: running.message.clone().unwrap_or_default(),
                    attachments,
                };
                self.show(items::sent_id(&done.key), body, &running, fx);
                (said, Vec::new(), true)
            }
            Ok(ToolResult::Opened(opened)) => {
                let said = tools::opened_text(&opened);
                let body = ItemBody::AgentOpenedFile {
                    path: opened.path,
                    line: opened.line,
                    outside: opened.outside,
                };
                self.show(items::opened_id(&done.key), body, &running, fx);
                (said, Vec::new(), true)
            }
            Ok(ToolResult::Visual(visual)) => {
                let id = items::visual_id(&done.key);
                let said = tools::visual_text(&id, &visual);
                let body = ItemBody::AgentVisual {
                    title: visual.title,
                    attachment: visual.attachment,
                    replaces: visual.replaces,
                };
                self.show(id, body, &running, fx);
                (said, Vec::new(), true)
            }
            Ok(ToolResult::Text(text)) => (text, Vec::new(), true),
            Ok(ToolResult::Mixed { text, images }) => (text, images, true),
            Err(reason) => (reason, Vec::new(), false),
        };
        let items = content_items(&said, &images);
        self.tool_reply(&running.id, &running.key, items, success, fx);
    }

    /// Puts what a call to one of MaxHQ's tools showed on the timeline of
    /// the agent whose thread made it.
    fn show(&mut self, id: ItemId, body: ItemBody, running: &Running, fx: &mut Effects) {
        let Some(agent) = self.threads.get(&running.thread).map(|known| known.agent) else {
            return;
        };
        self.sends_shown.insert(id.0.clone());
        upsert(
            fx,
            TimelineItem {
                id,
                agent,
                turn: running.turn.clone().map(TurnId),
                parent: None,
                ordinal: 0,
                status: ItemStatus::Completed,
                started_at: Some(running.at),
                completed_at: Some(Timestamp::now()),
                body,
            },
        );
    }

    /// Shows the files a finished send put on the timeline, or the file an
    /// open opened, read back from the call's item, unless they show
    /// already.
    pub(super) fn show_sent(
        &mut self,
        agent: AgentId,
        turn: Option<&str>,
        item: &Value,
        at: Timestamp,
        fx: &mut Effects,
    ) {
        let Some(files) = items::sent(agent, turn, item, Some(at)) else {
            return;
        };
        if self.sends_shown.insert(files.id.0.clone()) {
            upsert(fx, files);
        }
    }

    /// Answers an `item/tool/call` request.
    fn tool_reply(
        &mut self,
        id: &Value,
        key: &str,
        items: Vec<Value>,
        success: bool,
        fx: &mut Effects,
    ) {
        let result = json!({"contentItems": items, "success": success});
        fx.writes.push(Write {
            line: rpc::response(id, result),
            answers: Some(key.to_owned()),
        });
        self.pending.remove(key);
    }
}

/// A dynamic tool's answer as Codex reads it: the text, then each image
/// as a data URL, as codex-cli 0.166's `DynamicToolCallResponse` schema
/// spells them.
fn content_items(text: &str, images: &[ToolImage]) -> Vec<Value> {
    let mut items = vec![json!({"type": "inputText", "text": text})];
    items.extend(images.iter().map(|image| {
        json!({"type": "inputImage", "imageUrl": format!("data:{};base64,{}", image.mime, image.data)})
    }));
    items
}
