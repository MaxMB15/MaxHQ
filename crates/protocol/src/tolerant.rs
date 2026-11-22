// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading what a newer `maxd` sends. A message or list item this build
//! can't read is dropped and the rest still applies. A timeline item body,
//! outcome, plan item's outcome, peer request, error code or app key's
//! origin it can't read reads as its type's unknown variant. Each kind it
//! can't read is logged once, up to a limit.

use std::collections::BTreeSet;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Mutex, Once, PoisonError};

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess,
    Visitor,
};
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
    each(deserializer, |item: Value| match T::deserialize(&item) {
        Ok(item) => Ok(Some(item)),
        Err(_) => {
            unreadable(&kind_of(&item, &[]));
            Ok(None)
        }
    })
}

/// Reads a list one item at a time, so only one item is ever held before
/// it's read. `read` gives what to keep of each, nothing to leave it out,
/// or an error that fails the list.
pub(crate) fn each<'de, D, E, T, F>(deserializer: D, read: F) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    E: Deserialize<'de>,
    F: FnMut(E) -> serde_json::Result<Option<T>>,
{
    struct Each<E, T, F>(F, PhantomData<fn(E) -> T>);

    impl<'de, E, T, F> Visitor<'de> for Each<E, T, F>
    where
        E: Deserialize<'de>,
        F: FnMut(E) -> serde_json::Result<Option<T>>,
    {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list")
        }

        fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<E>()? {
                items.extend((self.0)(item).map_err(de::Error::custom)?);
            }
            Ok(items)
        }
    }

    deserializer.deserialize_seq(Each(read, PhantomData))
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

/// Reads an optional value, or none when it doesn't decode, such as a kind
/// from a newer `maxd`. For `#[serde(default, deserialize_with =
/// "crate::tolerant::or_none")]`, which also takes
/// `#[specta(type = Option<...>)]` naming the field's type.
pub(crate) fn or_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(Option::<T>::deserialize(&value).unwrap_or_else(|_| {
        unreadable(&kind_of(&value, &[]));
        None
    }))
}

/// Reads a list item by item, with the stand-in for each item that doesn't
/// decode. For `#[serde(deserialize_with = "crate::tolerant::unknowns")]`,
/// which also takes `#[specta(type = Vec<...>)]` naming the list's type.
pub(crate) fn unknowns<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Unknown,
{
    each(deserializer, |item: Value| Ok(Some(stand_in(item))))
}

/// The value, or its stand-in when it doesn't decode.
fn stand_in<T: Unknown>(value: Value) -> T {
    T::deserialize(&value).unwrap_or_else(|_| unknown(&kind_of(&value, &[])))
}

/// The stand-in for a kind this build can't read, logged.
pub(crate) fn unknown<T: Unknown>(kind: &str) -> T {
    unreadable(kind);
    T::unknown(kind)
}

/// The fields an item may be named by, in the order [`kind_of`] picks
/// them. `value` and `key` name it by their own `entity`.
const NAMED_BY: [&str; 7] = ["entity", "value", "key", "op", "type", "kind", "status"];

/// The kind an item names: itself when it's a string, its entity, its
/// change's entity, or its tag, cut to [`KIND_LEN`]. Fields named in
/// `skip` don't count. Only the names are kept while it reads, so an item
/// of any size costs no more than them.
pub(crate) fn kind_of<'de, D: Deserializer<'de>>(item: D, skip: &[&str]) -> String {
    item.deserialize_any(Names(skip))
        .ok()
        .flatten()
        .unwrap_or_else(|| "unnamed".to_owned())
}

/// The visit methods of a visitor that skips all but strings and maps,
/// naming nothing for them.
macro_rules! names_nothing_else {
    () => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }
    };
}

/// Reads an item for [`kind_of`], skipping the fields in it.
struct Names<'s>(&'s [&'s str]);

impl<'de> Visitor<'de> for Names<'_> {
    type Value = Option<String>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an item")
    }

    fn visit_str<E: de::Error>(self, kind: &str) -> Result<Self::Value, E> {
        Ok(Some(cut(kind).to_owned()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found: [Option<String>; NAMED_BY.len()] = Default::default();
        while let Some(field) = map.next_key_seed(Field(self.0))? {
            let Some(at) = field else {
                map.next_value::<IgnoredAny>()?;
                continue;
            };
            found[at] = if matches!(NAMED_BY[at], "value" | "key") {
                map.next_value_seed(Entity)?
            } else {
                map.next_value_seed(Name)?
            };
        }
        Ok(found.into_iter().flatten().next())
    }

    names_nothing_else!();
}

/// Reads a key as its place in [`NAMED_BY`], or none when it names
/// nothing or is skipped.
struct Field<'s>(&'s [&'s str]);

impl<'de> DeserializeSeed<'de> for Field<'_> {
    type Value = Option<usize>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_str(self)
    }
}

impl Visitor<'_> for Field<'_> {
    type Value = Option<usize>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a field's name")
    }

    fn visit_str<E: de::Error>(self, field: &str) -> Result<Self::Value, E> {
        if self.0.contains(&field) {
            return Ok(None);
        }
        Ok(NAMED_BY.iter().position(|named| *named == field))
    }
}

/// Reads a field's value as a name when it's a string, and skips it
/// otherwise.
struct Name;

impl<'de> DeserializeSeed<'de> for Name {
    type Value = Option<String>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Name {
    type Value = Option<String>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a name")
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
        Ok(Some(cut(name).to_owned()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(None)
    }

    names_nothing_else!();
}

/// Reads a `value` or `key` field for the `entity` it names, and skips
/// anything else.
struct Entity;

impl<'de> DeserializeSeed<'de> for Entity {
    type Value = Option<String>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Entity {
    type Value = Option<String>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an entity or its key")
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entity = None;
        while let Some(field) = map.next_key_seed(Field(&[]))? {
            if field.is_some_and(|at| NAMED_BY[at] == "entity") {
                entity = map.next_value_seed(Name)?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(entity)
    }

    names_nothing_else!();
}

/// How many kinds this build remembers having logged. The peer picks the
/// names, so the set can't grow with what it sends.
const MOST_KINDS: usize = 64;

/// How many bytes of a kind's name this build logs and remembers.
pub(crate) const KIND_LEN: usize = 64;

/// The kinds logged so far, each cut to `KIND_LEN`.
static SEEN: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The start of a kind's name, at most [`KIND_LEN`] bytes, cut between
/// characters.
pub(crate) fn cut(kind: &str) -> &str {
    let at = (0..=KIND_LEN.min(kind.len()))
        .rev()
        .find(|at| kind.is_char_boundary(*at))
        .unwrap_or(0);
    &kind[..at]
}

/// Logs a kind this build can't read, the first time it comes.
pub(crate) fn unreadable(kind: &str) {
    once(kind, "this build can't read this kind; a newer app can");
}

/// Logs a kind that came larger than this build reads it, the first time
/// it comes.
pub(crate) fn too_large(kind: &str) {
    once(
        kind,
        "a message of this kind came larger than this build reads",
    );
}

/// Logs a kind the first time it comes, for why it was left out.
fn once(kind: &str, why: &str) {
    let kind = cut(kind);
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if seen.contains(kind) {
        return;
    }
    if seen.len() < MOST_KINDS {
        seen.insert(kind.to_owned());
        tracing::warn!(kind, "{why}");
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
        replica.apply_snapshot(snapshot).unwrap();
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
        replica.apply_snapshot(snapshot).unwrap();
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
            reads_keys: false,
            full_screen: false,
            app_cursor: false,
            waiting: false,
            answers: false,
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
