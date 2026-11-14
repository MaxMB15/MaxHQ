// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! JSON Schema for the wire messages, for clients not written in Rust.

use schemars::{schema_for, Schema};

use crate::message::{ClientMessage, ServerMessage};

/// Every schema, with the file name it's checked in under in `schema/`.
pub fn all() -> Vec<(&'static str, Schema)> {
    vec![
        ("client-message.schema.json", schema_for!(ClientMessage)),
        ("server-message.schema.json", schema_for!(ServerMessage)),
    ]
}

/// The schema as it's checked in: pretty-printed, with a final newline.
pub fn render(schema: &Schema) -> String {
    let mut text = serde_json::to_string_pretty(schema).expect("a schema is always valid JSON");
    text.push('\n');
    text
}
