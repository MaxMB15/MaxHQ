// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Reading what another device sends without holding much more than it
//! keeps. Serde's tagged enums copy every field of a message while they
//! look for its tag, which can take many times the message's size. So a
//! message's `type` is read first, from its bytes. The kinds that carry
//! lists, timeline items, outcomes or requests are then read a piece at a
//! time from the message's own bytes, and the others are read whole only
//! when they're small.

use std::fmt;

use serde::de::{self, DeserializeOwned, DeserializeSeed, Deserializer, Visitor};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::ids::{AgentId, IdempotencyKey, TransferId};
use crate::message::{ClientMessage, ManifestEntry, ServerMessage};
use crate::state::{Change, Delta, Entity, Seq, Snapshot};
use crate::time::Timestamp;
use crate::timeline::{ItemId, ItemStatus, TimelineEvent, TimelineItem, TimelineUpdate, TurnId};
use crate::tolerant::{self, cut, kind_of, unreadable, Unknown};
use crate::workflow::{RunKey, WorkflowUpdate};

/// The most bytes a message of a kind that's always small may take, 1
/// MiB. The kinds read a piece at a time have no limit of their own.
pub(crate) const MAX_SMALL_LEN: usize = 1024 * 1024;

/// A kind's name as a message gives it, cut to [`tolerant::KIND_LEN`].
pub(crate) struct Tag {
    name: String,
    whole: bool,
}

impl Tag {
    /// The name, unless it was cut. No kind this build knows is that long.
    pub(crate) fn known(&self) -> Option<&str> {
        self.whole.then_some(self.name.as_str())
    }

    /// The name, as far as it was kept.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

impl<'de> Deserialize<'de> for Tag {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Name;

        impl Visitor<'_> for Name {
            type Value = Tag;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a kind's name")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<Tag, E> {
                let kept = cut(name);
                Ok(Tag {
                    name: kept.to_owned(),
                    whole: kept.len() == name.len(),
                })
            }
        }

        deserializer.deserialize_str(Name)
    }
}

/// The `type` a message names, read without keeping anything else.
pub(crate) fn tag(json: &[u8]) -> serde_json::Result<Option<Tag>> {
    #[derive(Deserialize)]
    struct Typed {
        r#type: Option<Tag>,
    }
    Ok(serde_json::from_slice::<Typed>(json)?.r#type)
}

/// How many items a list holds, counted without keeping any.
pub(crate) fn count(list: &str) -> serde_json::Result<usize> {
    struct Count;

    impl<'de> Visitor<'de> for Count {
        type Value = usize;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a list")
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<usize, A::Error> {
            let mut items = 0;
            while seq.next_element::<de::IgnoredAny>()?.is_some() {
                items += 1;
            }
            Ok(items)
        }
    }

    (&mut serde_json::Deserializer::from_str(list)).deserialize_seq(Count)
}

/// A piece larger than this has its values counted before it's read whole.
const COUNT_PAST: usize = 64 * 1024;

/// The most values one piece read whole may hold, counting each object,
/// list, key and plain value. Serde's tagged enums copy each into about 100
/// bytes while they look for the tag, so this keeps a piece under about
/// 100 MB. A real piece holds far fewer.
pub(crate) const MAX_VALUES: usize = 1_000_000;

/// Reads a piece whole, unless it holds more than [`MAX_VALUES`] values.
pub(crate) fn whole<T: DeserializeOwned>(raw: &str) -> serde_json::Result<T> {
    if raw.len() > COUNT_PAST {
        let mut values = 0;
        (&mut serde_json::Deserializer::from_str(raw)).deserialize_any(Values(&mut values))?;
    }
    serde_json::from_str(raw)
}

/// Counts the values in a piece, keeping none, and stops past
/// [`MAX_VALUES`].
struct Values<'a>(&'a mut usize);

impl Values<'_> {
    fn tick<E: de::Error>(self) -> Result<(), E> {
        *self.0 += 1;
        if *self.0 > MAX_VALUES {
            return Err(E::custom(format_args!(
                "a piece with more than {MAX_VALUES} values"
            )));
        }
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for Values<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Values<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        self.tick()
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        self.tick()
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        self.tick()
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        self.tick()
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        self.tick()
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.tick()
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let values = self.0;
        Values(&mut *values).tick()?;
        while seq.next_element_seed(Values(&mut *values))?.is_some() {}
        Ok(())
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let values = self.0;
        Values(&mut *values).tick()?;
        while map.next_key_seed(Values(&mut *values))?.is_some() {
            map.next_value_seed(Values(&mut *values))?;
        }
        Ok(())
    }
}

/// A type read from its JSON as it came, a piece at a time.
pub(crate) trait FromRaw: Sized {
    /// Reads one.
    fn from_raw(raw: &str) -> serde_json::Result<Self>;
}

impl FromRaw for Entity {
    fn from_raw(raw: &str) -> serde_json::Result<Self> {
        whole(raw)
    }
}

impl FromRaw for Change {
    fn from_raw(raw: &str) -> serde_json::Result<Self> {
        #[derive(Deserialize)]
        struct Op<'a> {
            op: Option<Tag>,
            #[serde(borrow)]
            value: Option<&'a RawValue>,
        }
        let change: Op = serde_json::from_str(raw)?;
        match (change.op.as_ref().and_then(Tag::known), change.value) {
            (Some("upsert"), Some(value)) => Ok(Change::Upsert {
                value: Entity::from_raw(value.get())?,
            }),
            _ => whole(raw),
        }
    }
}

impl FromRaw for TimelineEvent {
    fn from_raw(raw: &str) -> serde_json::Result<Self> {
        #[derive(Deserialize)]
        struct Event<'a> {
            r#type: Option<Tag>,
            #[serde(borrow)]
            item: Option<&'a RawValue>,
        }
        let event: Event = serde_json::from_str(raw)?;
        match (event.r#type.as_ref().and_then(Tag::known), event.item) {
            (Some("upsert"), Some(item)) => Ok(TimelineEvent::Upsert {
                item: TimelineItem::from_raw(item.get())?,
            }),
            _ => whole(raw),
        }
    }
}

/// A timeline item's own fields. Its body's sit beside them.
const ITEM_FIELDS: [&str; 8] = [
    "id",
    "agent",
    "turn",
    "parent",
    "ordinal",
    "status",
    "started_at",
    "completed_at",
];

impl FromRaw for TimelineItem {
    fn from_raw(raw: &str) -> serde_json::Result<Self> {
        #[derive(Deserialize)]
        struct Own {
            id: ItemId,
            agent: AgentId,
            turn: Option<TurnId>,
            parent: Option<ItemId>,
            ordinal: u64,
            status: ItemStatus,
            started_at: Option<Timestamp>,
            completed_at: Option<Timestamp>,
        }
        let own: Own = serde_json::from_str(raw)?;
        Ok(TimelineItem {
            id: own.id,
            agent: own.agent,
            turn: own.turn,
            parent: own.parent,
            ordinal: own.ordinal,
            status: own.status,
            started_at: own.started_at,
            completed_at: own.completed_at,
            body: or_unknown(raw, &ITEM_FIELDS),
        })
    }
}

/// A list read item by item, leaving out each item that doesn't decode.
struct Each<T>(Vec<T>);

impl<'de, T: FromRaw> Deserialize<'de> for Each<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        tolerant::each(deserializer, |item: &'de RawValue| {
            match T::from_raw(item.get()) {
                Ok(item) => Ok(Some(item)),
                Err(_) => {
                    unreadable(&kind_of(&mut json(item), &[]));
                    Ok(None)
                }
            }
        })
        .map(Each)
    }
}

/// A list read item by item, which fails with its first item that doesn't
/// decode.
pub(crate) struct All<T>(pub(crate) Vec<T>);

impl<'de, T: FromRaw> Deserialize<'de> for All<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        tolerant::each(deserializer, |item: &'de RawValue| {
            T::from_raw(item.get()).map(Some)
        })
        .map(All)
    }
}

/// A reader of what a piece holds, for [`kind_of`].
fn json(raw: &RawValue) -> serde_json::Deserializer<serde_json::de::StrRead<'_>> {
    serde_json::Deserializer::from_str(raw.get())
}

/// Reads a value, or its stand-in when it doesn't decode. Fields named in
/// `skip` don't name its kind.
fn or_unknown<T: Unknown>(raw: &str, skip: &[&str]) -> T {
    whole(raw).unwrap_or_else(|_| {
        let kind = kind_of(&mut serde_json::Deserializer::from_str(raw), skip);
        tolerant::unknown(&kind)
    })
}

/// A message that carries a request by its key.
#[derive(Deserialize)]
struct Request<'a> {
    key: IdempotencyKey,
    #[serde(borrow)]
    request: &'a RawValue,
}

/// A message that carries an outcome by its key.
#[derive(Deserialize)]
struct Outcome<'a> {
    key: IdempotencyKey,
    #[serde(borrow)]
    outcome: &'a RawValue,
}

/// Reads a message from a device, or none for one this build can't read
/// or that's too large for its kind. Each kind left out is logged once.
pub(crate) fn server(json: &[u8]) -> Option<ServerMessage> {
    let Ok(Some(tag)) = tag(json) else {
        unreadable("unnamed");
        return None;
    };
    let read = match tag.known() {
        Some("hello") => serde_json::from_slice(json).map(ServerMessage::Hello),
        Some("snapshot") => {
            #[derive(Deserialize)]
            struct Whole {
                seq: Seq,
                entities: Each<Entity>,
            }
            serde_json::from_slice(json).map(|Whole { seq, entities }| {
                ServerMessage::Snapshot(Snapshot {
                    seq,
                    entities: entities.0,
                })
            })
        }
        Some("delta") => {
            #[derive(Deserialize)]
            struct Next {
                seq: Seq,
                changes: Each<Change>,
            }
            serde_json::from_slice(json).map(|Next { seq, changes }| {
                ServerMessage::Delta(Delta {
                    seq,
                    changes: changes.0,
                })
            })
        }
        Some("timeline") => timeline(json),
        Some("action_result") => serde_json::from_slice(json).map(|Outcome { key, outcome }| {
            ServerMessage::ActionResult {
                key,
                outcome: or_unknown(outcome.get(), &[]),
            }
        }),
        Some("peer_request") => serde_json::from_slice(json).map(|Request { key, request }| {
            ServerMessage::PeerRequest {
                key,
                request: or_unknown(request.get(), &[]),
            }
        }),
        Some("workflow") => workflow(json),
        Some("fetch_manifest") => {
            #[derive(Deserialize)]
            struct Manifest {
                transfer: TransferId,
                entries: Vec<ManifestEntry>,
            }
            serde_json::from_slice(json).map(|Manifest { transfer, entries }| {
                ServerMessage::FetchManifest { transfer, entries }
            })
        }
        _ if json.len() > MAX_SMALL_LEN => {
            tolerant::too_large(tag.name());
            return None;
        }
        _ => serde_json::from_slice(json),
    };
    read.inspect_err(|_| unreadable(tag.name())).ok()
}

/// Reads a timeline message: its update's own `type` first, then the
/// update a piece at a time.
fn timeline(json: &[u8]) -> serde_json::Result<ServerMessage> {
    #[derive(Deserialize)]
    struct Timeline<'a> {
        agent: AgentId,
        #[serde(borrow)]
        update: &'a RawValue,
    }
    #[derive(Deserialize)]
    struct Reset {
        seq: Seq,
        items: All<TimelineItem>,
        has_older: bool,
    }
    #[derive(Deserialize)]
    struct Events {
        seq: Seq,
        events: Each<TimelineEvent>,
    }
    #[derive(Deserialize)]
    struct Older {
        items: All<TimelineItem>,
        has_older: bool,
    }
    let Timeline { agent, update } = serde_json::from_slice(json)?;
    let update = update.get();
    let update = match tag(update.as_bytes())?.as_ref().and_then(Tag::known) {
        Some("reset") => {
            let Reset {
                seq,
                items,
                has_older,
            } = serde_json::from_str(update)?;
            TimelineUpdate::Reset {
                seq,
                items: items.0,
                has_older,
            }
        }
        Some("events") => {
            let Events { seq, events } = serde_json::from_str(update)?;
            TimelineUpdate::Events {
                seq,
                events: events.0,
            }
        }
        Some("older") => {
            let Older { items, has_older } = serde_json::from_str(update)?;
            TimelineUpdate::Older {
                items: items.0,
                has_older,
            }
        }
        _ => return Err(de::Error::custom("a timeline update this build can't read")),
    };
    Ok(ServerMessage::Timeline { agent, update })
}

/// Reads a workflow message: its update's `kind` first, then the run
/// straight into its own type.
fn workflow(json: &[u8]) -> serde_json::Result<ServerMessage> {
    #[derive(Deserialize)]
    struct Workflow<'a> {
        run: RunKey,
        #[serde(borrow)]
        update: &'a RawValue,
    }
    #[derive(Deserialize)]
    struct Update<'a> {
        kind: Option<Tag>,
        #[serde(borrow)]
        run: Option<&'a RawValue>,
    }
    let Workflow { run, update } = serde_json::from_slice(json)?;
    let Update { kind, run: body } = serde_json::from_str(update.get())?;
    let update = match (kind.as_ref().and_then(Tag::known), body) {
        (Some("reset"), Some(body)) => WorkflowUpdate::Reset(whole(body.get())?),
        (Some("patch"), Some(body)) => WorkflowUpdate::Patch(whole(body.get())?),
        _ => return Err(de::Error::custom("a workflow update this build can't read")),
    };
    Ok(ServerMessage::Workflow {
        run,
        update: Box::new(update),
    })
}

/// Reads a message another device's `maxd` sent over a peer link. Its
/// requests and answers are read a piece at a time, and any other message
/// past [`MAX_SMALL_LEN`] is refused unread.
pub(crate) fn peer(json: &[u8]) -> serde_json::Result<ClientMessage> {
    let tag = tag(json)?;
    match tag.as_ref().and_then(Tag::known) {
        Some("peer_request") => {
            let Request { key, request } = serde_json::from_slice(json)?;
            Ok(ClientMessage::PeerRequest {
                key,
                request: or_unknown(request.get(), &[]),
            })
        }
        Some("peer_answer") => {
            let Outcome { key, outcome } = serde_json::from_slice(json)?;
            Ok(ClientMessage::PeerAnswer {
                key,
                outcome: or_unknown(outcome.get(), &[]),
            })
        }
        _ if json.len() > MAX_SMALL_LEN => Err(de::Error::custom(format_args!(
            "a message of this kind may take at most {} KiB",
            MAX_SMALL_LEN / 1024
        ))),
        _ => serde_json::from_slice(json),
    }
}

#[cfg(test)]
mod tests;
