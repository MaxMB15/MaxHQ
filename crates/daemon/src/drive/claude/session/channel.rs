// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! MaxHQ's own tools, which the CLI reaches as the SDK MCP server `maxhq`.
//! `initialize` names the server, and the CLI then sends each MCP message
//! as an `mcp_message` request on the pipe the holder already keeps. The
//! handshake and the tool list are answered at once; a tool call runs off
//! the loop and is answered when it ends.
//!
//! The holder keeps an unanswered `mcp_message` and gives it back on a
//! reconnect. A call still running is left to finish, and one that ended
//! while no one held the pipe runs again and shows on the same item.

use maxhq_core::tools::{self, agents, Mcp, Tool};
use serde_json::{json, Value};

use super::{ClaudeSession, Ours};
use crate::drive::claude::convo;
use crate::drive::claude::wire::{self, text};
use crate::drive::{Effects, ToolCall, ToolDone, ToolResult, Write};

/// How long the CLI waits on one call to MaxHQ's tools: long enough to
/// copy large files out of a container, and for the longest `wait_agent`
/// to answer that it timed out. The CLI ends a call at this, whatever it
/// is doing.
const CALL_TIMEOUT_MS: u64 = (agents::MAX_WAIT + 5 * 60) * 1000;

/// A call to MaxHQ's tools that runs now.
#[derive(Clone, Debug)]
pub struct Running {
    /// The JSON-RPC ID of the `tools/call`, which the answer repeats.
    rpc: Value,
    /// The CLI's ID for the tool call, which names its items.
    tool_use_id: Option<String>,
    /// The message sent with the files, for a send.
    message: Option<String>,
}

/// Adds MaxHQ's tool server to an `initialize` request.
pub(super) fn declare(body: &mut Value) {
    if let Value::Object(fields) = body {
        fields.insert("sdkMcpServers".to_owned(), json!([tools::SERVER]));
        fields.insert(
            "sdkMcpServerConfigs".to_owned(),
            json!({tools::SERVER: {"timeout": CALL_TIMEOUT_MS}}),
        );
    }
}

impl ClaudeSession {
    /// Tells a CLI held across a restart that MaxHQ's tools changed. It
    /// read the list from the `maxd` that ran before, and a repeat
    /// `initialize` doesn't make it read the list again, so a tool added
    /// since would stay out of its reach. The notice makes it ask for
    /// `tools/list` again, which is answered like the first time.
    pub(super) fn tools_changed(&mut self, fx: &mut Effects) {
        let body = json!({
            "subtype": "mcp_message",
            "server_name": tools::SERVER,
            "message": tools::mcp_tools_changed(),
        });
        self.send_request(body, Ours::ToolsChanged, fx);
    }

    /// Reads an `mcp_message` request: one MCP message for a server the
    /// CLI was told about in `initialize`.
    pub(super) fn mcp_message(&mut self, id: &str, body: &Value, fx: &mut Effects) {
        if self.calls.contains_key(id) {
            return;
        }
        let server = text(body, "server_name").unwrap_or_default();
        if server != tools::SERVER {
            let line = wire::failure(id, &format!("MaxHQ has no MCP server {server:?}."));
            self.mcp_reply(id, line, fx);
            return;
        }
        let message = body.get("message").unwrap_or(&Value::Null);
        let offered = self.place.tools_for(&self.login);
        match tools::mcp(message, env!("CARGO_PKG_VERSION"), offered) {
            Mcp::Answer(answer) => {
                let line = wire::success(id, json!({"mcp_response": answer}));
                self.mcp_reply(id, line, fx);
            }
            Mcp::Run {
                id: rpc,
                call,
                tool_use_id,
            } => {
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                let message = call.message();
                fx.calls.push(ToolCall {
                    arguments,
                    key: id.to_owned(),
                    call_id: tool_use_id.clone().unwrap_or_else(|| id.to_owned()),
                    agent: self.lead(),
                    call: *call,
                });
                self.calls.insert(
                    id.to_owned(),
                    Running {
                        rpc,
                        tool_use_id,
                        message,
                    },
                );
            }
        }
    }

    /// Whether a `can_use_tool` request is for one of MaxHQ's own tools,
    /// which go ahead in every mode. Some copy files or read a timeline.
    /// Others act on other agents, such as `send_agent`, `start_agent` and
    /// `schedule_task`, and `maxd` checks those against the caller's
    /// permission when they run. A request with no `mcp_server` counts as
    /// ours.
    pub(super) fn is_ours(body: &Value) -> bool {
        let name = text(body, "tool_name").unwrap_or_default();
        let from_sdk = match body.get("mcp_server") {
            None | Some(Value::Null) => true,
            Some(server) => {
                text(server, "name") == Some(tools::SERVER) && text(server, "source") == Some("sdk")
            }
        };
        Tool::from_claude(name).is_some() && from_sdk
    }

    /// Answers a call that ended, and shows what a send sent. A call the CLI
    /// withdrew meanwhile isn't answered, and what it copied isn't shown.
    pub(super) fn call_done(&mut self, done: ToolDone, fx: &mut Effects) {
        let Some(running) = self.calls.remove(&done.key) else {
            return;
        };
        if self.answered.contains(&done.key) {
            return;
        }
        let at = self.now();
        let (said, images, failed) = match done.result {
            Ok(ToolResult::Sent(attachments)) => {
                let said = tools::sent_text(&attachments);
                let call = running.tool_use_id.as_deref().unwrap_or(&done.key);
                self.convo.sent(call, running.message, attachments, at);
                self.touch(at);
                (said, Vec::new(), false)
            }
            Ok(ToolResult::Opened(opened)) => {
                let said = tools::opened_text(&opened);
                let call = running.tool_use_id.as_deref().unwrap_or(&done.key);
                self.convo.opened(call, opened, at);
                self.touch(at);
                (said, Vec::new(), false)
            }
            Ok(ToolResult::Visual(visual)) => {
                let call = running.tool_use_id.as_deref().unwrap_or(&done.key);
                let said = tools::visual_text(&convo::visual_id(call), &visual);
                self.convo.visual(call, visual, at);
                self.touch(at);
                (said, Vec::new(), false)
            }
            Ok(ToolResult::Text(text)) => (text, Vec::new(), false),
            Ok(ToolResult::Mixed { text, images }) => (text, images, false),
            Err(reason) => (reason, Vec::new(), true),
        };
        let answer = tools::mcp_answer(&running.rpc, &said, &images, failed);
        let line = wire::success(&done.key, json!({"mcp_response": answer}));
        self.mcp_reply(&done.key, line, fx);
    }

    /// Answers an `mcp_message`, once. The holder reads which request the
    /// line answers from the line itself: one from before `maxd` handled
    /// these doesn't wait on them, and would drop an answer named for one.
    fn mcp_reply(&mut self, id: &str, line: String, fx: &mut Effects) {
        self.answered.insert(id.to_owned());
        fx.writes.push(Write {
            line,
            answers: None,
        });
    }
}
