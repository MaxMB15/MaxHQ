// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The notification log, since [`crate::message::NOTICES_SINCE`]. Each
//! `maxd` keeps the alerts it raised about its own agents, the ones the
//! Mac shows and pushes to phones, the newest [`NOTICES_KEPT`]. A client
//! lists, marks read and clears them with [`crate::ClientMessage::Notices`],
//! and merges every device's list by time.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AgentId, DeviceId};
use crate::time::Timestamp;

/// The most notices a device keeps. A new one past it drops the oldest.
pub const NOTICES_KEPT: u32 = 500;

/// What a notice was about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// The agent asked you a question.
    Question,
    /// The agent asked for your permission.
    Permission,
    /// The agent needed some other input.
    Input,
    /// A prompt reported an error.
    Error,
    /// The agent finished a turn.
    Finished,
    /// The agent hit a usage limit.
    Limit,
    /// The usage limit reset and the agent went on.
    Resumed,
}

/// One alert a device raised.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct Notice {
    /// Its number on its device, newer ones higher.
    pub id: u32,
    /// The device that raised it.
    pub device: DeviceId,
    /// The agent it was about, which may be gone since.
    pub agent: AgentId,
    /// What it was about.
    pub kind: NoticeKind,
    /// The agent's title then.
    pub title: String,
    /// The agent's project then, if it had one.
    pub project: Option<String>,
    /// What the prompt said, for a prompt that said something.
    pub summary: Option<String>,
    /// When it was raised.
    pub at: Timestamp,
    /// When you read it, or none while it's unread.
    pub read_at: Option<Timestamp>,
}

/// What [`crate::ClientMessage::Notices`] asks of a device's log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoticeCall {
    /// The newest notices, at most `limit`. The outcome is
    /// [`crate::message::ActionOutcome::Notices`].
    List {
        /// How many to send.
        limit: u32,
    },
    /// Marks the agent's notices read, or every notice when there's no
    /// agent.
    Read {
        /// The agent you opened.
        agent: Option<AgentId>,
    },
    /// Forgets every notice.
    Clear,
}

/// A page of a log, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct NoticePage {
    /// The newest notices.
    pub notices: Vec<Notice>,
    /// How many the log holds, these included.
    pub total: u32,
    /// How many of those are unread.
    pub unread: u32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ids::IdempotencyKey;
    use crate::message::{ActionOutcome, NOTICES_SINCE};
    use crate::ClientMessage;

    const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";
    const DEVICE: &str = "0199a7c2-0000-7000-8000-000000000007";
    const AGENT: &str = "0199a7c2-0000-7000-8000-0000000000a1";

    #[test]
    fn notice_calls_read_back_and_wait_for_their_version() {
        let key: IdempotencyKey = KEY.parse().unwrap();
        let device: DeviceId = DEVICE.parse().unwrap();
        let calls = [
            (
                NoticeCall::List { limit: 20 },
                json!({"kind": "list", "limit": 20}),
            ),
            (
                NoticeCall::Read {
                    agent: Some(AGENT.parse().unwrap()),
                },
                json!({"kind": "read", "agent": AGENT}),
            ),
            (
                NoticeCall::Read { agent: None },
                json!({"kind": "read", "agent": null}),
            ),
            (NoticeCall::Clear, json!({"kind": "clear"})),
        ];
        for (call, wire) in calls {
            let message = ClientMessage::Notices { key, device, call };
            let mut whole = json!({"type": "notices", "key": KEY, "device": DEVICE});
            whole.as_object_mut().unwrap().insert("call".into(), wire);
            assert_eq!(serde_json::to_value(&message).unwrap(), whole);
            assert_eq!(
                serde_json::from_value::<ClientMessage>(whole).unwrap(),
                message
            );
            assert_eq!(message.since(), NOTICES_SINCE);
            assert!(!message.reaches(Some(48)), "an older maxd never gets it");
            assert!(message.reaches(Some(49)));
            assert_eq!(message.action_key(), Some(key));
        }
    }

    #[test]
    fn a_page_reads_back() {
        let outcome = ActionOutcome::Notices {
            page: NoticePage {
                notices: vec![Notice {
                    id: 7,
                    device: DEVICE.parse().unwrap(),
                    agent: AGENT.parse().unwrap(),
                    kind: NoticeKind::Question,
                    title: "Fix the login".into(),
                    project: Some("example-app".into()),
                    summary: Some("Which branch?".into()),
                    at: Timestamp(1_000),
                    read_at: None,
                }],
                total: 3,
                unread: 1,
            },
        };
        let wire = json!({
            "status": "notices",
            "page": {
                "notices": [{
                    "id": 7,
                    "device": DEVICE,
                    "agent": AGENT,
                    "kind": "question",
                    "title": "Fix the login",
                    "project": "example-app",
                    "summary": "Which branch?",
                    "at": 1000,
                    "read_at": null,
                }],
                "total": 3,
                "unread": 1,
            },
        });
        assert_eq!(serde_json::to_value(&outcome).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<ActionOutcome>(wire).unwrap(),
            outcome
        );
    }
}
