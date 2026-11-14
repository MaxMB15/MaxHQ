// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The todos in `cursor/update_todos` and `cursor/create_plan`. Each has
//! its own ID, or else its place in the list.

use maxhq_protocol::timeline::Todo;
use serde_json::Value;

use crate::drive::acp::events::{list, todo};
use crate::drive::acp::rpc::text;

/// The todos in `params`, by ID.
pub fn todos(params: &Value) -> Vec<(String, Todo)> {
    list(params, "todos")
        .enumerate()
        .filter_map(|(index, entry)| {
            let todo = todo(entry, "content")?;
            let id = text(entry, "id").map_or_else(|| index.to_string(), str::to_owned);
            Some((id, todo))
        })
        .collect()
}
