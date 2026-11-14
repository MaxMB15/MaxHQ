// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `cursor/task`, which names a sub-agent by the tool call that started
//! it. With `_meta.subagents` set the CLI sends each sub-agent's work as
//! updates for a child session. A task that reports how long it ran has
//! ended.
//!
//! None of this is in the 2026.10.22 recording, which ran no sub-agent:
//! `cursor/task` and the child sessions are still read from Cursor's docs,
//! though `initialize` does list `sessionCapabilities.subagents`.

use serde_json::Value;

use crate::drive::acp::rpc::text;
use crate::drive::acp::Task;

/// The task in a `cursor/task` notification, or none without a tool call.
pub fn task(params: &Value) -> Option<Task> {
    let call = text(params, "toolCallId")?.to_owned();
    let kind = match params.get("subagentType") {
        Some(Value::String(kind)) if kind != "unspecified" => Some(kind.clone()),
        Some(Value::Object(custom)) => custom
            .get("custom")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    };
    Some(Task {
        call,
        kind,
        description: text(params, "description")
            .or_else(|| text(params, "prompt"))
            .map(str::to_owned),
        model: text(params, "model").map(str::to_owned),
        ended: params.get("durationMs").is_some(),
    })
}
