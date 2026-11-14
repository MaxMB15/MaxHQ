// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Cursor's flavor of the ACP driver, for a held `agent acp`. Cursor adds
//! its own todos, sub-agents, plans and questions to ACP, sends its usage
//! limit as an error with its own codes, and reports some failures as
//! ordinary replies (see [`failures`]). Each tool call gets
//! [`TOOL_TIMEOUT`], as Cursor's tools can hang. Full access is the
//! process's sandbox flag, which lasts for its life.

mod failures;
mod launch;
mod limits;
mod requests;
mod tasks;
mod todos;

use std::time::Duration;

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::model::{PermissionMode, Vendor};
use serde_json::{json, Value};

use crate::drive::acp::rpc::RpcError;
use crate::drive::acp::{AcpSession, Ext, ExtPrompt, Failure, Flavor, Kept};
use crate::drive::{Driver, Login, Place};

pub use launch::{launch, login_launch, program, API_KEY, AUTH_TOKEN};

/// How long one tool call may run before the driver cancels the turn.
/// Cursor's tools can hang, and the CLI never gives up on them itself.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the stream's item IDs start with.
const BASE: &str = "cursor";

/// The model ID that leaves the pick to Cursor: Auto.
const AUTO: &str = "default";

/// A held `agent acp` and the one agent it runs.
pub fn session(
    login: &Login,
    place: &Place,
    agent: AgentId,
    kept: Kept,
    mode: PermissionMode,
) -> Box<dyn Driver> {
    Box::new(AcpSession::new(
        Box::new(Cursor),
        login,
        place,
        agent,
        kept,
        mode,
    ))
}

/// Cursor's flavor.
#[derive(Clone, Copy, Debug)]
pub struct Cursor;

impl Flavor for Cursor {
    fn name(&self) -> &str {
        "Cursor"
    }

    fn vendor(&self) -> Vendor {
        Vendor::Cursor
    }

    fn base(&self) -> &str {
        BASE
    }

    /// The two flags are the only ones the CLI reads: without
    /// `parameterizedModelPicker` it offers one fixed preset per model,
    /// and `subagents` shows each sub-agent as a child session.
    fn client_meta(&self) -> Value {
        json!({ "parameterizedModelPicker": true, "subagents": true })
    }

    fn notification(&self, method: &str, params: &Value) -> Option<Ext> {
        match method {
            "cursor/update_todos" => Some(Ext::Todos {
                todos: todos::todos(params),
                merge: params.get("merge").and_then(Value::as_bool) == Some(true),
            }),
            "cursor/task" => tasks::task(params).map(Ext::Subagent),
            _ => None,
        }
    }

    fn request(&self, method: &str, params: &Value) -> Option<ExtPrompt> {
        match method {
            "cursor/create_plan" => Some(ExtPrompt::Plan(requests::plan(params))),
            "cursor/ask_question" => Some(requests::question(params)),
            _ => None,
        }
    }

    fn reply_failure(&self, reply: &str) -> Option<Failure> {
        failures::failure(reply)
    }

    fn stop_reason(&self, stderr: &str) -> Option<String> {
        launch::refused_flag(stderr)
    }

    fn is_limit(&self, error: &RpcError) -> bool {
        limits::is_limit(error)
    }

    fn tool_timeout(&self) -> Option<Duration> {
        Some(TOOL_TIMEOUT)
    }

    fn auto_model(&self) -> Option<&str> {
        Some(AUTO)
    }

    fn modes_are_launch_flags(&self) -> bool {
        true
    }

    /// Cursor's sign-in is `cursor-agent login`, so a signed-out CLI stops.
    fn authenticates(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::acp::rpc;

    #[test]
    fn initialize_asks_for_the_model_picker_and_subagents() {
        let params = rpc::initialize(Cursor.client_meta());
        let meta = &params["clientCapabilities"]["_meta"];
        assert_eq!(meta["parameterizedModelPicker"], json!(true));
        assert_eq!(meta["subagents"], json!(true));
        assert_eq!(params["clientCapabilities"]["terminal"], json!(false));
    }
}
