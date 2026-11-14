// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Cursor's own requests: `cursor/create_plan` and `cursor/ask_question`.
//! Neither is in the 2026.10.22 recording, and their shapes are still read
//! from Cursor's docs. Cursor filters its questions tool out for ACP
//! clients, so nothing counts on `cursor/ask_question` ever coming.

use serde_json::Value;

use super::todos;
use crate::drive::acp::events::list;
use crate::drive::acp::rpc::text;
use crate::drive::acp::{Asked, ExtPrompt, Plan};

/// The plan in `cursor/create_plan`.
pub fn plan(params: &Value) -> Plan {
    Plan {
        markdown: text(params, "plan").unwrap_or_default().to_owned(),
        todos: todos::todos(params),
        detail: text(params, "overview")
            .or_else(|| text(params, "name"))
            .map(str::to_owned),
    }
}

/// The questions in `cursor/ask_question`.
pub fn question(params: &Value) -> ExtPrompt {
    let questions = list(params, "questions")
        .filter_map(|entry| {
            let id = text(entry, "id")?;
            let options = list(entry, "options")
                .filter_map(|option| {
                    let id = text(option, "id")?;
                    Some((
                        id.to_owned(),
                        text(option, "label").unwrap_or(id).to_owned(),
                    ))
                })
                .collect();
            Some(Asked {
                id: id.to_owned(),
                text: text(entry, "prompt").unwrap_or_default().to_owned(),
                options,
                multi: entry.get("allowMultiple").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect();
    ExtPrompt::Question {
        title: text(params, "title").map(str::to_owned),
        questions,
    }
}
