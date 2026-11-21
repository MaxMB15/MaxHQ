// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The shell rules Cursor saved in its own settings, since
//! [`crate::message::CURSOR_RULES_SINCE`]. Always allow in an earlier
//! MaxHQ added `Shell(...)` entries to the allow list in the Cursor CLI's
//! settings file, and Cursor runs a command that starts with one without
//! asking. A client lists a device's entries and removes one with
//! [`crate::ClientMessage::CursorRules`]. The rest of the allow list and
//! the deny list stay as they are.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

/// What [`crate::ClientMessage::CursorRules`] asks of a device. Since
/// protocol 57.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CursorRuleCall {
    /// The device's `Shell(...)` entries.
    List,
    /// Takes one `Shell(...)` entry out of the allow list, each copy of it,
    /// and lists what's left. One that isn't there lists what is.
    Remove {
        /// The entry as the list gave it, such as `Shell(git status)`.
        rule: String,
    },
}

/// The `Shell(...)` entries in Cursor's allow list on a device, for
/// [`crate::message::ActionOutcome::CursorRules`]. Since protocol 57.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct CursorRules {
    /// The settings file the device read, with `~` for its home folder.
    pub path: String,
    /// The entries as written, such as `Shell(git status)`, in the file's
    /// order. Empty when the file isn't there.
    pub rules: Vec<String>,
}

/// A device's Cursor rules, or why the app has none. Since protocol 57.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CursorRuleList {
    /// What the device holds.
    Listed {
        /// The entries.
        rules: CursorRules,
    },
    /// The device's `maxd` is too old to read Cursor's rules.
    TooOld,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ids::{DeviceId, IdempotencyKey};
    use crate::message::{ActionOutcome, CURSOR_RULES_SINCE};
    use crate::ClientMessage;

    const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";
    const DEVICE: &str = "0199a7c2-0000-7000-8000-000000000007";

    #[test]
    fn rule_requests_read_back_and_wait_for_their_version() {
        let key: IdempotencyKey = KEY.parse().unwrap();
        let device: DeviceId = DEVICE.parse().unwrap();
        let messages = [
            (
                ClientMessage::CursorRules {
                    key,
                    device,
                    call: CursorRuleCall::List,
                },
                json!({
                    "type": "cursor_rules",
                    "key": KEY,
                    "device": DEVICE,
                    "call": { "kind": "list" },
                }),
            ),
            (
                ClientMessage::CursorRules {
                    key,
                    device,
                    call: CursorRuleCall::Remove {
                        rule: "Shell(git status)".into(),
                    },
                },
                json!({
                    "type": "cursor_rules",
                    "key": KEY,
                    "device": DEVICE,
                    "call": { "kind": "remove", "rule": "Shell(git status)" },
                }),
            ),
        ];
        for (message, wire) in messages {
            assert_eq!(serde_json::to_value(&message).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<ClientMessage>(wire).unwrap(),
                message
            );
            assert_eq!(message.since(), CURSOR_RULES_SINCE);
            assert!(!message.reaches(Some(CURSOR_RULES_SINCE - 1)));
            assert!(message.reaches(Some(CURSOR_RULES_SINCE)));
            assert_eq!(message.action_key(), Some(key));
        }
    }

    #[test]
    fn a_list_reads_back() {
        let rules = CursorRules {
            path: "~/.cursor/cli-config.json".into(),
            rules: vec!["Shell(git status)".into(), "Shell(ls)".into()],
        };
        let outcome = ActionOutcome::CursorRules {
            rules: rules.clone(),
        };
        let wire = json!({
            "status": "cursor_rules",
            "rules": {
                "path": "~/.cursor/cli-config.json",
                "rules": ["Shell(git status)", "Shell(ls)"],
            },
        });
        assert_eq!(serde_json::to_value(&outcome).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(wire).unwrap(),
            outcome
        );
        assert_eq!(
            serde_json::to_value(CursorRuleList::Listed { rules }).unwrap(),
            json!({
                "status": "listed",
                "rules": {
                    "path": "~/.cursor/cli-config.json",
                    "rules": ["Shell(git status)", "Shell(ls)"],
                },
            })
        );
        assert_eq!(
            serde_json::to_value(CursorRuleList::TooOld).unwrap(),
            json!({ "status": "too_old" })
        );
    }
}
