// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The prompts a CLI waits on, and which writes answer them.
//!
//! A prompt is pending from the line that asks it until a write answers it
//! or the CLI withdraws it. The first write to answer a prompt reaches the
//! CLI. Any later one gets [`crate::WriteOutcome::AlreadyAnswered`] and is
//! dropped, so two clients answering at once can't both get through.

use std::collections::{HashSet, VecDeque};

use serde_json::Value;

use crate::files::PendingEntry;
use crate::log::Entry;
use crate::wire::MAX_LINE;
use crate::Framing;

/// The Claude control requests the CLI waits on: the ones that wait on a
/// person, and `mcp_message`, a call to the host's own tools that can still
/// be running when the host reconnects. The others, such as
/// `hook_callback`, are for the host to answer at once.
const CLAUDE_PROMPTS: [&str; 4] = [
    "can_use_tool",
    "elicitation",
    "request_user_dialog",
    "mcp_message",
];

/// How many answered or withdrawn prompts to remember, so a late answer to
/// one of them is dropped rather than sent.
const RESOLVED_KEEP: usize = 4096;

/// How many prompts stay pending. A CLI asks a few at a time, and each change
/// rewrites `pending.json`, so one that asks without end loses its oldest.
const MAX_WAITING: usize = 1024;

/// What a line from the CLI does to the prompts.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FromCli {
    /// It asks a prompt with this key.
    Asks(String),
    /// It withdraws the prompt with this key.
    Withdraws(String),
    /// Neither.
    Other,
}

/// Reads a line the CLI printed.
pub(crate) fn from_cli(framing: Framing, text: &str) -> FromCli {
    match framing {
        Framing::ClaudeStreamJson => {
            if !text.contains("control_") {
                return FromCli::Other;
            }
            let Some(message) = parse(text) else {
                return FromCli::Other;
            };
            let key = || message.get("request_id").and_then(key_of);
            match message.get("type").and_then(Value::as_str) {
                Some("control_request") => {
                    let subtype = message
                        .get("request")
                        .and_then(|request| request.get("subtype"))
                        .and_then(Value::as_str);
                    match (subtype, key()) {
                        (Some(subtype), Some(key)) if CLAUDE_PROMPTS.contains(&subtype) => {
                            FromCli::Asks(key)
                        }
                        _ => FromCli::Other,
                    }
                }
                Some("control_cancel_request") => key().map_or(FromCli::Other, FromCli::Withdraws),
                _ => FromCli::Other,
            }
        }
        Framing::JsonRpc => {
            if !text.contains("\"id\"") && !text.contains("serverRequest/resolved") {
                return FromCli::Other;
            }
            let Some(message) = parse(text) else {
                return FromCli::Other;
            };
            let method = message.get("method").and_then(Value::as_str);
            if method == Some("serverRequest/resolved") {
                return message
                    .get("params")
                    .and_then(|params| params.get("requestId"))
                    .and_then(key_of)
                    .map_or(FromCli::Other, FromCli::Withdraws);
            }
            match (method, message.get("id").and_then(key_of)) {
                (Some(_), Some(key)) => FromCli::Asks(key),
                _ => FromCli::Other,
            }
        }
    }
}

/// The prompt a line written to the CLI answers, if it answers one.
pub(crate) fn answers(framing: Framing, text: &str) -> Option<String> {
    match framing {
        Framing::ClaudeStreamJson => {
            if !text.contains("control_response") {
                return None;
            }
            let message = parse(text)?;
            if message.get("type").and_then(Value::as_str) != Some("control_response") {
                return None;
            }
            message.get("response")?.get("request_id").and_then(key_of)
        }
        Framing::JsonRpc => {
            if !text.contains("\"id\"") {
                return None;
            }
            let message = parse(text)?;
            let is_response = message.get("method").is_none()
                && (message.get("result").is_some() || message.get("error").is_some());
            if !is_response {
                return None;
            }
            message.get("id").and_then(key_of)
        }
    }
}

fn parse(text: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str(text) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// A request ID as a key: a string as itself and a number in decimal.
pub(crate) fn key_of(id: &Value) -> Option<String> {
    match id {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

/// What to do with a write.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// Send it. It answers no pending prompt.
    Pass,
    /// Send it. It answered this prompt, which is no longer pending.
    Answered(String),
    /// Drop it. The prompt it answers was answered or withdrawn before.
    Already,
}

/// The prompts the CLI waits on.
#[derive(Default)]
pub(crate) struct Pending {
    waiting: Vec<(String, Entry)>,
    resolved: HashSet<String>,
    resolved_order: VecDeque<String>,
}

impl Pending {
    /// Applies a line from the CLI. Returns whether the pending set
    /// changed.
    pub fn apply(&mut self, effect: FromCli, entry: &Entry) -> bool {
        match effect {
            FromCli::Asks(key) => {
                self.waiting.retain(|(waiting, _)| *waiting != key);
                self.forget_resolved(&key);
                self.waiting.push((key, entry.clone()));
                self.drop_oldest();
                true
            }
            FromCli::Withdraws(key) => self.resolve(&key),
            FromCli::Other => false,
        }
    }

    /// Drops the oldest prompts while there are more than [`MAX_WAITING`] or
    /// their lines come to more than [`MAX_LINE`], which keeps `pending.json`
    /// and a reconnect's hello small. The newest always stays.
    fn drop_oldest(&mut self) {
        let mut bytes: usize = self.waiting.iter().map(|(_, entry)| entry.text.len()).sum();
        let mut left = self.waiting.len();
        let mut dropped = 0;
        while left > 1 && (left > MAX_WAITING || bytes > MAX_LINE) {
            bytes -= self.waiting[dropped].1.text.len();
            left -= 1;
            dropped += 1;
        }
        self.waiting.drain(..dropped);
    }

    /// Decides whether a write goes through, taking the prompt it answers
    /// out of the pending set when it does.
    ///
    /// `explicit` is the prompt the writer says it answers. When it's given
    /// and isn't pending, the write is dropped. Without it, the holder
    /// reads the line to find out.
    pub fn claim(&mut self, framing: Framing, line: &str, explicit: Option<&str>) -> Claim {
        let key = match explicit {
            Some(key) => key.to_owned(),
            None => match answers(framing, line) {
                Some(key) => key,
                None => return Claim::Pass,
            },
        };
        if self.resolve(&key) {
            Claim::Answered(key)
        } else if explicit.is_some() || self.resolved.contains(&key) {
            Claim::Already
        } else {
            Claim::Pass
        }
    }

    /// Takes `key` out of the pending set and remembers it as resolved.
    /// Returns whether it was pending. A key that wasn't pending isn't
    /// remembered, so answers to requests the holder doesn't track, such as
    /// Claude's `hook_callback`, keep going through.
    fn resolve(&mut self, key: &str) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|(waiting, _)| waiting != key);
        let was_pending = self.waiting.len() != before;
        if was_pending && self.resolved.insert(key.to_owned()) {
            self.resolved_order.push_back(key.to_owned());
            while self.resolved_order.len() > RESOLVED_KEEP {
                if let Some(old) = self.resolved_order.pop_front() {
                    self.resolved.remove(&old);
                }
            }
        }
        was_pending
    }

    fn forget_resolved(&mut self, key: &str) {
        if self.resolved.remove(key) {
            self.resolved_order.retain(|old| old != key);
        }
    }

    /// Empties the set, as when the CLI has ended. Returns whether it held
    /// anything.
    pub fn clear(&mut self) -> bool {
        let had = !self.waiting.is_empty();
        self.waiting.clear();
        had
    }

    /// The lines that asked the pending prompts, oldest first.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.waiting.iter().map(|(_, entry)| entry)
    }

    /// The set as `pending.json` stores it.
    pub fn to_file(&self) -> Vec<PendingEntry> {
        self.waiting
            .iter()
            .map(|(key, entry)| PendingEntry {
                key: key.clone(),
                seq: entry.seq,
                at: entry.at,
                line: entry.text.to_string(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::time::Timestamp;

    use super::*;

    const CLAUDE: Framing = Framing::ClaudeStreamJson;
    const RPC: Framing = Framing::JsonRpc;

    fn entry(seq: u64, text: &str) -> Entry {
        Entry {
            seq,
            at: Timestamp(0),
            text: text.into(),
        }
    }

    #[test]
    fn the_oldest_prompts_go_past_the_caps_and_the_newest_stays() {
        let mut pending = Pending::default();
        for seq in 0..(MAX_WAITING as u64 + 5) {
            pending.apply(FromCli::Asks(seq.to_string()), &entry(seq, "ask"));
        }
        let seqs: Vec<u64> = pending.entries().map(|entry| entry.seq).collect();
        assert_eq!(seqs.len(), MAX_WAITING);
        assert_eq!(seqs.first(), Some(&5));
        assert_eq!(seqs.last(), Some(&(MAX_WAITING as u64 + 4)));

        let mut pending = Pending::default();
        let long = "x".repeat(MAX_LINE / 2 + 1);
        for seq in 0..3 {
            pending.apply(FromCli::Asks(seq.to_string()), &entry(seq, &long));
        }
        let seqs: Vec<u64> = pending.entries().map(|entry| entry.seq).collect();
        assert_eq!(seqs, [2]);
        // A single line at the cap's size still waits.
        let longest = "x".repeat(MAX_LINE);
        pending.apply(FromCli::Asks("big".into()), &entry(9, &longest));
        let seqs: Vec<u64> = pending.entries().map(|entry| entry.seq).collect();
        assert_eq!(seqs, [9]);
    }

    #[test]
    fn claude_permission_prompts_are_pending_and_host_callbacks_are_not() {
        let asks = |s: &str| from_cli(CLAUDE, s);
        let tool = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash"}}"#;
        assert_eq!(asks(tool), FromCli::Asks("r1".into()));
        for subtype in ["elicitation", "request_user_dialog", "mcp_message"] {
            let line = format!(
                r#"{{"type":"control_request","request_id":"x","request":{{"subtype":"{subtype}"}}}}"#
            );
            assert_eq!(asks(&line), FromCli::Asks("x".into()));
        }
        let hook =
            r#"{"type":"control_request","request_id":"h","request":{"subtype":"hook_callback"}}"#;
        assert_eq!(asks(hook), FromCli::Other);
        let cancel = r#"{"type":"control_cancel_request","request_id":"r1"}"#;
        assert_eq!(asks(cancel), FromCli::Withdraws("r1".into()));
        assert_eq!(asks(r#"{"type":"assistant","message":{}}"#), FromCli::Other);
        assert_eq!(asks("control_ but not json"), FromCli::Other);
    }

    #[test]
    fn json_rpc_server_requests_are_pending_until_resolved() {
        let asks = |s: &str| from_cli(RPC, s);
        let approval = r#"{"id":0,"method":"item/commandExecution/requestApproval","params":{}}"#;
        assert_eq!(asks(approval), FromCli::Asks("0".into()));
        let named = r#"{"id":"abc","method":"x","params":{}}"#;
        assert_eq!(asks(named), FromCli::Asks("abc".into()));
        let resolved =
            r#"{"method":"serverRequest/resolved","params":{"threadId":"t","requestId":0}}"#;
        assert_eq!(asks(resolved), FromCli::Withdraws("0".into()));
        let note = r#"{"method":"item/agentMessage/delta","params":{"threadId":"t"}}"#;
        assert_eq!(asks(note), FromCli::Other);
        let reply = r#"{"id":3,"result":{}}"#;
        assert_eq!(asks(reply), FromCli::Other);
    }

    #[test]
    fn answers_are_read_from_responses_only() {
        let claude = r#"{"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{"behavior":"allow"}}}"#;
        assert_eq!(answers(CLAUDE, claude), Some("r1".into()));
        let user = r#"{"type":"user","message":{"role":"user","content":"control_response"}}"#;
        assert_eq!(answers(CLAUDE, user), None);
        assert_eq!(
            answers(RPC, r#"{"id":0,"result":{"decision":"accept"}}"#),
            Some("0".into())
        );
        assert_eq!(
            answers(RPC, r#"{"id":1,"error":{"code":1}}"#),
            Some("1".into())
        );
        assert_eq!(answers(RPC, r#"{"id":2,"method":"turn/start"}"#), None);
    }

    #[test]
    fn the_first_answer_wins() {
        let mut pending = Pending::default();
        let ask = r#"{"id":0,"method":"m"}"#;
        assert!(pending.apply(from_cli(RPC, ask), &entry(4, ask)));
        let answer = r#"{"id":0,"result":{}}"#;
        assert_eq!(
            pending.claim(RPC, answer, None),
            Claim::Answered("0".into())
        );
        assert_eq!(pending.claim(RPC, answer, None), Claim::Already);
        assert_eq!(pending.claim(RPC, answer, Some("0")), Claim::Already);
        assert_eq!(pending.entries().count(), 0);
    }

    #[test]
    fn an_explicit_key_must_be_pending() {
        let mut pending = Pending::default();
        assert_eq!(pending.claim(CLAUDE, "{}", Some("nope")), Claim::Already);
        assert_eq!(
            pending.claim(CLAUDE, r#"{"type":"user"}"#, None),
            Claim::Pass
        );
        let ask =
            r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool"}}"#;
        pending.apply(from_cli(CLAUDE, ask), &entry(1, ask));
        assert_eq!(
            pending.claim(CLAUDE, "{}", Some("r")),
            Claim::Answered("r".into())
        );
    }

    #[test]
    fn a_withdrawn_prompt_drops_its_late_answer() {
        let mut pending = Pending::default();
        let ask =
            r#"{"type":"control_request","request_id":"r","request":{"subtype":"can_use_tool"}}"#;
        pending.apply(from_cli(CLAUDE, ask), &entry(1, ask));
        let cancel = r#"{"type":"control_cancel_request","request_id":"r"}"#;
        assert!(pending.apply(from_cli(CLAUDE, cancel), &entry(2, cancel)));
        let answer =
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"r"}}"#;
        assert_eq!(pending.claim(CLAUDE, answer, None), Claim::Already);
    }

    #[test]
    fn a_tool_call_answer_is_read_from_its_line() {
        let mut pending = Pending::default();
        let call = r#"{"type":"control_request","request_id":"m1","request":{"subtype":"mcp_message","server_name":"maxhq","message":{"jsonrpc":"2.0","id":2,"method":"tools/call"}}}"#;
        assert!(pending.apply(from_cli(CLAUDE, call), &entry(1, call)));
        let answer = r#"{"type":"control_response","response":{"subtype":"success","request_id":"m1","response":{"mcp_response":{"jsonrpc":"2.0","id":2,"result":{}}}}}"#;
        assert_eq!(
            pending.claim(CLAUDE, answer, None),
            Claim::Answered("m1".into())
        );
        assert_eq!(pending.claim(CLAUDE, answer, None), Claim::Already);
        assert_eq!(pending.entries().count(), 0);
    }

    #[test]
    fn an_unknown_answer_passes_through() {
        let mut pending = Pending::default();
        let answer = r#"{"id":9,"result":{}}"#;
        assert_eq!(pending.claim(RPC, answer, None), Claim::Pass);
        assert_eq!(pending.claim(RPC, answer, None), Claim::Pass);
        // A hook callback isn't a prompt, so its answer always goes through,
        // even after the CLI cancels it.
        let hook =
            r#"{"type":"control_request","request_id":"h","request":{"subtype":"hook_callback"}}"#;
        assert!(!pending.apply(from_cli(CLAUDE, hook), &entry(1, hook)));
        let cancel = r#"{"type":"control_cancel_request","request_id":"h"}"#;
        assert!(!pending.apply(from_cli(CLAUDE, cancel), &entry(2, cancel)));
        let reply =
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"h"}}"#;
        assert_eq!(pending.claim(CLAUDE, reply, None), Claim::Pass);
    }

    #[test]
    fn a_reused_id_is_pending_again() {
        let mut pending = Pending::default();
        let ask = r#"{"id":0,"method":"m"}"#;
        pending.apply(from_cli(RPC, ask), &entry(1, ask));
        pending.claim(RPC, r#"{"id":0,"result":{}}"#, None);
        pending.apply(from_cli(RPC, ask), &entry(2, ask));
        let held: Vec<u64> = pending.entries().map(|e| e.seq).collect();
        assert_eq!(held, [2]);
        assert_eq!(pending.to_file()[0].key, "0");
    }
}
