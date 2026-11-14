// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Lines kept for a thread until the driver can take them. Text streamed
//! into one item joins into one line, so a whole turn read again after a
//! restart fits.

use serde_json::Value;

use super::Seen;

/// Notifications that add text to an item.
const STREAMED: [&str; 4] = [
    "item/agentMessage/delta",
    "item/plan/delta",
    "item/commandExecution/outputDelta",
    "item/reasoning/summaryTextDelta",
];

/// What names the text a streamed line adds to.
const SAME: [&str; 5] = [
    "threadId",
    "turnId",
    "itemId",
    "summaryIndex",
    "contentIndex",
];

/// One kept line.
#[derive(Debug)]
enum Line {
    /// A line as it came.
    Text(String),
    /// Streamed text, joined from one or more lines.
    Joined(Value),
}

/// Lines kept in the order they came.
#[derive(Debug, Default)]
pub struct Kept {
    lines: Vec<(Line, Seen)>,
}

impl Kept {
    /// Keeps a line. Streamed text joins the line before when both add to
    /// the same text. Once `max` lines are kept, more are dropped.
    pub fn keep(&mut self, text: &str, method: &str, params: &Value, seen: Seen, max: usize) {
        if STREAMED.contains(&method) {
            if let Some((Line::Joined(last), _)) = self.lines.last_mut()
                && join(last, method, params)
            {
                return;
            }
            if self.lines.len() < max
                && let Ok(value) = serde_json::from_str(text)
            {
                self.lines.push((Line::Joined(value), seen));
                return;
            }
        }
        if self.lines.len() < max {
            self.lines.push((Line::Text(text.to_owned()), seen));
        }
    }

    /// Takes every line, in the order they came.
    pub fn take(&mut self) -> Vec<(String, Seen)> {
        self.lines
            .drain(..)
            .map(|(line, seen)| match line {
                Line::Text(text) => (text, seen),
                Line::Joined(value) => (value.to_string(), seen),
            })
            .collect()
    }

    /// The holder's seq for the first line kept.
    pub fn first_seq(&self) -> Option<u64> {
        self.lines.iter().map(|(_, seen)| seen.seq).min()
    }

    /// Drops every line.
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

/// Adds a streamed line's text to `last` when both add to the same text,
/// and says whether it did.
fn join(last: &mut Value, method: &str, params: &Value) -> bool {
    let Some(delta) = params.get("delta").and_then(Value::as_str) else {
        return false;
    };
    if last.get("method").and_then(Value::as_str) != Some(method) {
        return false;
    }
    let Some(kept) = last.get_mut("params") else {
        return false;
    };
    if SAME.iter().any(|name| kept.get(name) != params.get(name)) {
        return false;
    }
    let Some(Value::String(text)) = kept.get_mut("delta") else {
        return false;
    };
    text.push_str(delta);
    true
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::time::Timestamp;
    use serde_json::json;

    use super::*;

    fn seen(seq: u64) -> Seen {
        Seen {
            at: Timestamp(1_792_921_750_000),
            seq,
            replayed: true,
        }
    }

    fn line(method: &str, params: Value) -> String {
        json!({ "method": method, "params": params, "emittedAtMs": 1_792_921_750_000u64 })
            .to_string()
    }

    fn keep(kept: &mut Kept, seq: u64, method: &str, params: Value, max: usize) {
        kept.keep(
            &line(method, params.clone()),
            method,
            &params,
            seen(seq),
            max,
        );
    }

    #[test]
    fn streamed_text_joins_into_one_line() {
        let mut kept = Kept::default();
        let started = json!({ "threadId": "t", "turnId": "u", "item": { "type": "agentMessage", "id": "m", "text": "" } });
        keep(&mut kept, 1, "item/started", started, 3);
        for (seq, delta) in [(2, "Running"), (3, " the"), (4, " sleep")] {
            let params = json!({ "threadId": "t", "turnId": "u", "itemId": "m", "delta": delta });
            keep(&mut kept, seq, "item/agentMessage/delta", params, 3);
        }
        let other = json!({ "threadId": "t", "turnId": "u", "itemId": "c", "delta": "out" });
        keep(&mut kept, 5, "item/commandExecution/outputDelta", other, 3);
        let late = json!({ "threadId": "t", "turnId": "u", "itemId": "m", "delta": "!" });
        keep(&mut kept, 6, "item/agentMessage/delta", late, 3);
        assert_eq!(kept.first_seq(), Some(1));

        let lines = kept.take();
        assert_eq!(lines.len(), 3);
        let joined: Value = serde_json::from_str(&lines[1].0).unwrap();
        assert_eq!(joined["params"]["delta"], "Running the sleep");
        assert_eq!(joined["emittedAtMs"], 1_792_921_750_000u64);
        assert_eq!(lines[1].1.seq, 2);
        let output: Value = serde_json::from_str(&lines[2].0).unwrap();
        assert_eq!(output["params"]["delta"], "out");
        assert_eq!(kept.first_seq(), None);
    }
}
