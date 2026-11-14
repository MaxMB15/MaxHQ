// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Small pieces of the stream-json wire format: the control envelope, the
//! permission mode names and lookups into loosely typed JSON.

use maxhq_protocol::model::PermissionMode;
use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};

/// The CLI's name for a permission mode.
pub fn mode_name(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "default",
        PermissionMode::AcceptEdits => "acceptEdits",
        PermissionMode::Auto => "auto",
        PermissionMode::Plan => "plan",
        PermissionMode::FullAccess => "bypassPermissions",
    }
}

/// The mode the CLI names, or none for one MaxHQ has no name for, such as
/// `dontAsk`.
pub fn mode_from(name: &str) -> Option<PermissionMode> {
    match name {
        "default" => Some(PermissionMode::Ask),
        "acceptEdits" => Some(PermissionMode::AcceptEdits),
        "auto" => Some(PermissionMode::Auto),
        "plan" => Some(PermissionMode::Plan),
        "bypassPermissions" => Some(PermissionMode::FullAccess),
        _ => None,
    }
}

/// A request from MaxHQ to the CLI.
pub fn request(id: &str, body: Value) -> String {
    json!({"type": "control_request", "request_id": id, "request": body}).to_string()
}

/// A successful answer to one of the CLI's requests.
pub fn success(id: &str, body: Value) -> String {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": body},
    })
    .to_string()
}

/// A failed answer to one of the CLI's requests.
pub fn failure(id: &str, error: &str) -> String {
    json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": id, "error": error},
    })
    .to_string()
}

/// The string at `key`, if there is one.
pub fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The string at `key`, or an empty one.
pub fn text_or_empty<'a>(value: &'a Value, key: &str) -> &'a str {
    text(value, key).unwrap_or_default()
}

/// The whole number at `key`, if there is one.
pub fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

/// Whether `key` holds `true`.
pub fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A time the CLI gave in seconds since the epoch.
pub fn unix_seconds(value: &Value) -> Option<Timestamp> {
    let seconds = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    // Rounds to the millisecond, well inside u64 for any real date.
    Some(Timestamp((seconds * 1000.0).round() as u64))
}

/// `text` cut to at most `max` characters, with an ellipsis when cut.
pub fn clip(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", line[..end].trim_end()),
        None if line.len() < text.trim().len() => format!("{line}…"),
        None => line.to_owned(),
    }
}

/// `text` cut to at most `max` bytes on a character boundary, and whether
/// anything was cut.
pub fn cap(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

/// The text in a tool result's or message's content, which is either a
/// string or a list of blocks.
pub fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| text(block, "type") == Some("text"))
            .filter_map(|block| text(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_through_their_names() {
        for mode in [
            PermissionMode::Ask,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
            PermissionMode::Plan,
            PermissionMode::FullAccess,
        ] {
            assert_eq!(mode_from(mode_name(mode)), Some(mode));
        }
        assert_eq!(mode_from("dontAsk"), None);
    }

    #[test]
    fn clips_to_the_first_line() {
        assert_eq!(clip("ls -la", 20), "ls -la");
        assert_eq!(clip("cargo test\n--workspace", 20), "cargo test…");
        assert_eq!(clip("abcdefghij", 4), "abcd…");
    }

    #[test]
    fn caps_on_a_character_boundary() {
        assert_eq!(cap("héllo", 2), ("h".to_owned(), true));
        assert_eq!(cap("hi", 8), ("hi".to_owned(), false));
    }

    #[test]
    fn reads_seconds_as_milliseconds() {
        assert_eq!(
            unix_seconds(&json!(1792936200)),
            Some(Timestamp(1_792_936_200_000))
        );
        assert_eq!(unix_seconds(&json!(null)), None);
    }
}
