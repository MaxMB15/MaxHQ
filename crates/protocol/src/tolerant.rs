// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading what a newer `maxd` sends. A message or list item this build
//! can't read is dropped and the rest still applies. A timeline item body,
//! outcome, plan item's outcome, peer request, error code or app key's
//! origin it can't read reads as its type's unknown variant. Each kind it
//! can't read is logged once, up to a limit.

use std::collections::BTreeSet;
use std::sync::{Mutex, Once, PoisonError};

use serde::de::{DeserializeOwned, Deserializer};
use serde::Deserialize;
use serde_json::Value;

use crate::admission::AppKeyOrigin;
use crate::agents::PeerRequest;
use crate::attention::AnswerOutcome;
use crate::dispatch::PlanOutcome;
use crate::message::{ActionOutcome, ErrorCode};
use crate::timeline::ItemBody;

/// Reads a list item by item, leaving out each item that doesn't decode.
/// For `#[serde(deserialize_with = "crate::tolerant::list")]`, which also
/// takes `#[specta(type = Vec<...>)]` naming the list's type.
pub(crate) fn list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let items = Vec::<Value>::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .filter_map(|item| {
            let kind = kind(&item).map(str::to_owned);
            match serde_json::from_value(item) {
                Ok(item) => Some(item),
                Err(_) => {
                    unreadable(kind.as_deref().unwrap_or("unnamed"));
                    None
                }
            }
        })
        .collect())
}

/// Reads one message, or none for one this build can't read, such as a
/// type from a newer `maxd`.
pub(crate) fn message<T: DeserializeOwned>(json: &[u8]) -> Option<T> {
    #[derive(Deserialize)]
    struct Tag {
        r#type: Option<String>,
    }
    match serde_json::from_slice(json) {
        Ok(message) => Some(message),
        Err(_) => {
            let tag = serde_json::from_slice::<Tag>(json)
                .ok()
                .and_then(|tag| tag.r#type);
            unreadable(tag.as_deref().unwrap_or("unnamed"));
            None
        }
    }
}

/// A type with a stand-in for a kind this build can't read.
pub(crate) trait Unknown: DeserializeOwned {
    /// The stand-in, given the kind's name.
    fn unknown(kind: &str) -> Self;
}

impl Unknown for ItemBody {
    fn unknown(kind: &str) -> Self {
        Self::Unknown {
            native_kind: kind.to_owned(),
        }
    }
}

impl Unknown for ActionOutcome {
    fn unknown(_: &str) -> Self {
        Self::Unknown
    }
}

impl Unknown for AnswerOutcome {
    fn unknown(_: &str) -> Self {
        Self::Unknown
    }
}

impl Unknown for PeerRequest {
    fn unknown(_: &str) -> Self {
        Self::Unknown
    }
}

impl Unknown for ErrorCode {
    fn unknown(_: &str) -> Self {
        Self::Unknown
    }
}

impl Unknown for PlanOutcome {
    fn unknown(_: &str) -> Self {
        Self::Unknown
    }
}

impl Unknown for AppKeyOrigin {
    fn unknown(_: &str) -> Self {
        Self::Unknown
    }
}

/// Reads a value, or its stand-in when it doesn't decode, such as a kind
/// from a newer `maxd`. For `#[serde(deserialize_with =
/// "crate::tolerant::or_unknown")]`, which also takes
/// `#[specta(type = ...)]` naming the field's type.
pub(crate) fn or_unknown<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Unknown,
{
    Ok(stand_in(Value::deserialize(deserializer)?))
}

/// Reads a list item by item, with the stand-in for each item that doesn't
/// decode. For `#[serde(deserialize_with = "crate::tolerant::unknowns")]`,
/// which also takes `#[specta(type = Vec<...>)]` naming the list's type.
pub(crate) fn unknowns<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Unknown,
{
    let items = Vec::<Value>::deserialize(deserializer)?;
    Ok(items.into_iter().map(stand_in).collect())
}

/// The value, or its stand-in when it doesn't decode.
fn stand_in<T: Unknown>(value: Value) -> T {
    let kind = kind(&value).unwrap_or("unnamed").to_owned();
    serde_json::from_value(value).unwrap_or_else(|_| {
        unreadable(&kind);
        T::unknown(&kind)
    })
}

/// The kind an item names: itself when it's a string, its entity, its
/// change's entity, or its tag.
fn kind(item: &Value) -> Option<&str> {
    fn entity(value: &Value) -> Option<&str> {
        value.get("entity")?.as_str()
    }
    item.as_str()
        .or_else(|| entity(item))
        .or_else(|| item.get("value").and_then(entity))
        .or_else(|| item.get("key").and_then(entity))
        .or_else(|| {
            ["op", "type", "kind", "status"]
                .iter()
                .find_map(|tag| item.get(tag)?.as_str())
        })
}

/// How many kinds this build remembers having logged. The peer picks the
/// names, so the set can't grow with what it sends.
const MOST_KINDS: usize = 64;

/// How many bytes of a kind's name this build logs and remembers.
const KIND_LEN: usize = 64;

/// The kinds logged so far, each cut to `KIND_LEN`.
static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Logs a kind this build can't read, the first time it comes.
fn unreadable(kind: &str) {
    let cut = (0..=KIND_LEN.min(kind.len()))
        .rev()
        .find(|at| kind.is_char_boundary(*at))
        .unwrap_or(0);
    let kind = &kind[..cut];
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if seen.contains(kind) {
        return;
    }
    if seen.len() < MOST_KINDS {
        seen.insert(kind.to_owned());
        tracing::warn!(kind, "this build can't read this kind; a newer app can");
    } else {
        static FULL: Once = Once::new();
        FULL.call_once(|| tracing::warn!("this build won't log any more kinds it can't read"));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{KIND_LEN, MOST_KINDS, SEEN};
    use crate::agents::PeerRequest;
    use crate::attention::AnswerOutcome;
    use crate::dispatch::PlanOutcome;
    use crate::message::{ActionOutcome, ErrorCode, ServerMessage};
    use crate::state::{Entity, EntityKey, Replica, Seq};
    use crate::timeline::{ItemBody, ItemId, TimelineEvent, TimelineUpdate};

    const ONE: &str = "0199a7c2-0000-7000-8000-000000000001";
    const TWO: &str = "0199a7c2-0000-7000-8000-000000000002";
    const KEY: &str = "0199a7c2-0000-7000-8000-00000000000a";

    fn read(json: &Value) -> ServerMessage {
        ServerMessage::read(json.to_string().as_bytes()).expect("reads")
    }

    fn hidden(device: &str) -> Value {
        json!({ "entity": "hidden_projects", "device": device, "projects": [] })
    }

    fn hidden_key(device: &str) -> EntityKey {
        EntityKey::HiddenProjects(device.parse().unwrap())
    }

    #[test]
    fn entities_and_changes_this_build_cant_read_are_left_out() {
        let ServerMessage::Snapshot(snapshot) = read(&json!({
            "type": "snapshot",
            "seq": 4,
            "entities": [
                { "entity": "hologram", "id": "h1", "beam": 3 },
                hidden(ONE),
                { "entity": "device_settings" },
            ],
        })) else {
            panic!("expected a snapshot");
        };
        assert_eq!(snapshot.entities.len(), 1);
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot);
        assert!(replica.get(&hidden_key(ONE)).is_some());

        let ServerMessage::Delta(delta) = read(&json!({
            "type": "delta",
            "seq": 5,
            "changes": [
                { "op": "upsert", "value": { "entity": "hologram", "id": "h1" } },
                { "op": "remove", "key": { "entity": "hologram", "id": "h1" } },
                { "op": "rename", "from": "a", "to": "b" },
                { "op": "upsert", "value": hidden(TWO) },
                { "op": "remove", "key": { "entity": "hidden_projects", "id": ONE } },
            ],
        })) else {
            panic!("expected a delta");
        };
        assert_eq!(delta.changes.len(), 2);
        replica.apply_delta(delta).unwrap();
        assert_eq!(replica.seq(), Some(Seq(5)));
        assert!(replica.get(&hidden_key(ONE)).is_none());
        assert!(replica.get(&hidden_key(TWO)).is_some());
    }

    #[test]
    fn a_dispatcher_reads_beside_a_kind_this_build_cant() {
        let dispatcher = json!({
            "entity": "dispatcher",
            "device": ONE,
            "workspace": TWO,
            "agent": null,
            "plan": null,
        });
        let ServerMessage::Snapshot(snapshot) = read(&json!({
            "type": "snapshot",
            "seq": 1,
            "entities": [{ "entity": "hologram", "id": "h1" }, dispatcher],
        })) else {
            panic!("expected a snapshot");
        };
        let mut replica = Replica::default();
        replica.apply_snapshot(snapshot);
        let key = EntityKey::Dispatcher(ONE.parse().unwrap());
        assert!(matches!(replica.get(&key), Some(Entity::Dispatcher(_))));

        let ServerMessage::Delta(delta) = read(&json!({
            "type": "delta",
            "seq": 2,
            "changes": [
                { "op": "remove", "key": { "entity": "hologram", "id": "h1" } },
                { "op": "remove", "key": { "entity": "dispatcher", "id": ONE } },
            ],
        })) else {
            panic!("expected a delta");
        };
        replica.apply_delta(delta).unwrap();
        assert!(replica.get(&key).is_none());
    }

    #[test]
    fn plan_outcomes_this_build_cant_read_are_unknown() {
        assert_eq!(
            read(&json!({
                "type": "action_result",
                "key": KEY,
                "outcome": {
                    "status": "plan_ran",
                    "results": [
                        { "type": "started", "agent": ONE },
                        { "type": "queued", "behind": ONE },
                        { "type": "failed", "reason": "No room" },
                    ],
                },
            })),
            ServerMessage::ActionResult {
                key: KEY.parse().unwrap(),
                outcome: ActionOutcome::PlanRan {
                    results: vec![
                        PlanOutcome::Started {
                            agent: ONE.parse().unwrap()
                        },
                        PlanOutcome::Unknown,
                        PlanOutcome::Failed {
                            reason: "No room".into()
                        },
                    ],
                },
            }
        );
    }

    #[test]
    fn outcomes_requests_and_codes_this_build_cant_read_are_unknown() {
        assert_eq!(
            read(&json!({
                "type": "action_result",
                "key": KEY,
                "outcome": { "status": "teleported", "to": "mars" },
            })),
            ServerMessage::ActionResult {
                key: KEY.parse().unwrap(),
                outcome: ActionOutcome::Unknown,
            }
        );
        assert_eq!(
            read(&json!({
                "type": "answer_result",
                "idempotency_key": KEY,
                "outcome": { "status": "echoed" },
            })),
            ServerMessage::AnswerResult {
                idempotency_key: KEY.parse().unwrap(),
                outcome: AnswerOutcome::Unknown,
            }
        );
        assert_eq!(
            read(&json!({ "type": "error", "code": "overheated", "message": "Too hot" })),
            ServerMessage::Error {
                code: ErrorCode::Unknown,
                message: "Too hot".into(),
            }
        );
        assert_eq!(
            read(&json!({
                "type": "peer_request",
                "key": KEY,
                "request": { "kind": "dance", "steps": 4 },
            })),
            ServerMessage::PeerRequest {
                key: KEY.parse().unwrap(),
                request: PeerRequest::Unknown,
            }
        );
        // One this build reads is unchanged.
        assert_eq!(
            read(&json!({ "type": "action_result", "key": KEY, "outcome": { "status": "done" } })),
            ServerMessage::ActionResult {
                key: KEY.parse().unwrap(),
                outcome: ActionOutcome::Done,
            }
        );
    }

    #[test]
    fn timeline_kinds_this_build_cant_read_stand_in_or_are_left_out() {
        let ServerMessage::Timeline { update, .. } = read(&json!({
            "type": "timeline",
            "agent": ONE,
            "update": {
                "type": "events",
                "seq": 9,
                "events": [
                    {
                        "type": "upsert",
                        "item": {
                            "id": "claude:msg_01",
                            "agent": ONE,
                            "turn": null,
                            "parent": null,
                            "ordinal": 3,
                            "status": "completed",
                            "started_at": null,
                            "completed_at": null,
                            "kind": "hologram",
                            "beam": 3,
                        },
                    },
                    { "type": "teleport", "id": "claude:msg_01" },
                    { "type": "remove", "id": "claude:msg_02" },
                ],
            },
        })) else {
            panic!("expected a timeline update");
        };
        let TimelineUpdate::Events { events, .. } = update else {
            panic!("expected events");
        };
        let [TimelineEvent::Upsert { item }, TimelineEvent::Remove { id }] = events.as_slice()
        else {
            panic!("expected an upsert and a remove, got {events:?}");
        };
        assert_eq!(
            item.body,
            ItemBody::Unknown {
                native_kind: "hologram".into()
            }
        );
        assert_eq!(id, &ItemId("claude:msg_02".into()));
    }

    #[test]
    fn an_opened_file_stands_in_as_unknown_on_an_older_build() {
        // The item bodies a build before protocol 30 knew end without the
        // opened file. A few of them are enough to stand for the rest.
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            Interrupted,
            AgentAttachments { text: String },
            Unknown { native_kind: String },
        }
        impl super::Unknown for Older {
            fn unknown(kind: &str) -> Self {
                Self::Unknown {
                    native_kind: kind.to_owned(),
                }
            }
        }

        let opened = serde_json::to_value(ItemBody::AgentOpenedFile {
            path: "/tmp/example/src/main.rs".into(),
            line: Some(12),
            outside: false,
        })
        .unwrap();
        assert_eq!(opened["kind"], "agent_opened_file");
        let read: Older = super::stand_in(opened);
        assert_eq!(
            read,
            Older::Unknown {
                native_kind: "agent_opened_file".into()
            }
        );
        let known: Older = super::stand_in(json!({ "kind": "interrupted" }));
        assert_eq!(known, Older::Interrupted);
    }

    #[test]
    fn a_visual_stands_in_as_unknown_on_an_older_build() {
        // A build before protocol 39 knew the opened file but not the
        // visual.
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            AgentOpenedFile { path: String },
            Unknown { native_kind: String },
        }
        impl super::Unknown for Older {
            fn unknown(kind: &str) -> Self {
                Self::Unknown {
                    native_kind: kind.to_owned(),
                }
            }
        }

        let visual = serde_json::to_value(ItemBody::AgentVisual {
            title: "Weekly usage".into(),
            attachment: crate::timeline::AttachmentRef {
                id: crate::ids::AttachmentId::derive("visual"),
                name: "visual-weekly-usage.html".into(),
                kind: crate::timeline::AttachmentKind::File,
                bytes: 2_048,
                mime: Some("text/html".into()),
                path: "/tmp/example/visual-weekly-usage.html".into(),
                thumbnail: None,
            },
            replaces: None,
        })
        .unwrap();
        assert_eq!(visual["kind"], "agent_visual");
        let read: Older = super::stand_in(visual);
        assert_eq!(
            read,
            Older::Unknown {
                native_kind: "agent_visual".into()
            }
        );
    }

    #[test]
    fn a_run_stands_in_as_unknown_on_an_older_build() {
        // A build before protocol 45 knew the visual but not a command you
        // ran.
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            AgentVisual { title: String },
            Unknown { native_kind: String },
        }
        impl super::Unknown for Older {
            fn unknown(kind: &str) -> Self {
                Self::Unknown {
                    native_kind: kind.to_owned(),
                }
            }
        }

        let run = serde_json::to_value(ItemBody::UserRun {
            terminal: ONE.parse().unwrap(),
            command: "pnpm test".into(),
            cwd: "~/code/example-app".into(),
            device: TWO.parse().unwrap(),
            private: false,
            process: crate::terminal::TerminalStatus::Running,
            duration_ms: None,
            lines: 0,
            output: String::new(),
            output_truncated: false,
            moved: false,
            fresh: None,
        })
        .unwrap();
        assert_eq!(run["kind"], "user_run");
        let read: Older = super::stand_in(run);
        assert_eq!(
            read,
            Older::Unknown {
                native_kind: "user_run".into()
            }
        );
    }

    #[test]
    fn a_session_card_stands_in_as_unknown_on_an_older_build() {
        // A build before protocol 48 knew a command you ran but not the
        // card of a session.
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Older {
            UserRun { command: String },
            Unknown { native_kind: String },
        }
        impl super::Unknown for Older {
            fn unknown(kind: &str) -> Self {
                Self::Unknown {
                    native_kind: kind.to_owned(),
                }
            }
        }

        let card = serde_json::to_value(ItemBody::UserSession {
            terminal: ONE.parse().unwrap(),
            device: TWO.parse().unwrap(),
            blocks: Vec::new(),
            sealed: false,
            sent: false,
        })
        .unwrap();
        assert_eq!(card["kind"], "user_session");
        let read: Older = super::stand_in(card);
        assert_eq!(
            read,
            Older::Unknown {
                native_kind: "user_session".into()
            }
        );
    }

    #[test]
    fn a_terminal_is_left_out_on_an_older_build() {
        // A build before protocol 45 knew hidden projects but not
        // terminals.
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "entity", rename_all = "snake_case")]
        enum Older {
            HiddenProjects { device: String },
        }
        #[derive(Debug, PartialEq, serde::Deserialize)]
        #[serde(tag = "entity", content = "id", rename_all = "snake_case")]
        enum OlderKey {
            HiddenProjects(String),
        }
        #[derive(serde::Deserialize)]
        struct Snapshot {
            #[serde(deserialize_with = "super::list")]
            entities: Vec<Older>,
            #[serde(deserialize_with = "super::list")]
            keys: Vec<OlderKey>,
        }

        let terminal = Entity::Terminal(crate::terminal::Terminal {
            id: ONE.parse().unwrap(),
            agent: TWO.parse().unwrap(),
            device: TWO.parse().unwrap(),
            kind: crate::terminal::TerminalKind::Shell,
            moved: false,
            title: "zsh".into(),
            cwd: "~/code/example-app".into(),
            status: crate::terminal::TerminalStatus::Running,
            busy: false,
            marks: false,
            password_prompt: false,
            started_at: crate::time::Timestamp(1_793_368_920_000),
            cols: 120,
            rows: 32,
        });
        let sent = json!({
            "entities": [serde_json::to_value(&terminal).unwrap(), hidden(ONE)],
            "keys": [
                serde_json::to_value(terminal.key()).unwrap(),
                serde_json::to_value(hidden_key(ONE)).unwrap(),
            ],
        });
        let older: Snapshot = serde_json::from_value(sent).unwrap();
        assert_eq!(
            older.entities,
            [Older::HiddenProjects { device: ONE.into() }]
        );
        assert_eq!(older.keys, [OlderKey::HiddenProjects(ONE.into())]);

        // This build keeps it.
        let ServerMessage::Snapshot(snapshot) = read(&json!({
            "type": "snapshot",
            "seq": 1,
            "entities": [serde_json::to_value(&terminal).unwrap()],
        })) else {
            panic!("expected a snapshot");
        };
        assert_eq!(snapshot.entities, [terminal]);
    }

    #[test]
    fn a_message_this_build_cant_read_is_skipped() {
        assert_eq!(
            ServerMessage::read(br#"{"type":"hologram","beam":3}"#),
            None
        );
        assert_eq!(ServerMessage::read(b"not json"), None);
        assert_eq!(
            ServerMessage::read(br#"{"type":"pong","nonce":7}"#),
            Some(ServerMessage::Pong { nonce: 7 })
        );
    }

    #[test]
    fn a_peer_cant_grow_the_kinds_it_names() {
        let long = "k".repeat(1024 * 1024);
        assert_eq!(ServerMessage::read(&message_of_type(&long)), None);
        // A cut in the middle of a character doesn't panic.
        let wide = format!("k{}", "\u{00e9}".repeat(KIND_LEN));
        assert_eq!(ServerMessage::read(&message_of_type(&wide)), None);
        for number in 0..MOST_KINDS * 4 {
            let kind = format!("hologram-{number}");
            assert_eq!(ServerMessage::read(&message_of_type(&kind)), None);
        }

        let seen = SEEN.lock().unwrap();
        assert!(seen.len() <= MOST_KINDS, "{} kinds kept", seen.len());
        let longest = seen.iter().map(String::len).max().unwrap();
        assert!(longest <= KIND_LEN, "a kind of {longest} bytes kept");
    }

    fn message_of_type(kind: &str) -> Vec<u8> {
        json!({ "type": kind }).to_string().into_bytes()
    }
}
