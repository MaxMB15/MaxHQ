// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The conversation `session/load` replays, made into history. The replay
//! is the transcript: MaxHQ keeps no copy of its own, and the driver folds
//! the replay's events into items the way a timeline would.

use maxhq_core::context;
use maxhq_protocol::ids::{AgentId, AttachmentId};
use maxhq_protocol::timeline::{
    AttachmentRef, Delivery, ItemBody, TextField, TimelineEvent, TimelineItem,
};

use super::events::Out;

/// The items `events` leave behind for `agent`, in the order each first
/// appeared.
pub fn fold(agent: AgentId, events: Out) -> Vec<TimelineItem> {
    let mut items: Vec<TimelineItem> = Vec::new();
    for (to, event) in events {
        if to != agent {
            continue;
        }
        match event {
            TimelineEvent::Upsert { item } => {
                match items.iter_mut().find(|old| old.id == item.id) {
                    Some(old) => *old = item,
                    None => items.push(item),
                }
            }
            TimelineEvent::Append { id, field, text } => {
                if let Some(item) = items.iter_mut().find(|old| old.id == id) {
                    append(&mut item.body, field, &text);
                }
            }
            TimelineEvent::Remove { id } => items.retain(|old| old.id != id),
        }
    }
    items
}

/// Adds `text` to the field of `body` an append names.
fn append(body: &mut ItemBody, field: TextField, text: &str) {
    match (body, field) {
        (
            ItemBody::AssistantMessage { text: into, .. } | ItemBody::Reasoning { text: into, .. },
            TextField::Text,
        )
        | (ItemBody::Command { output: into, .. }, TextField::Output) => into.push_str(text),
        _ => {}
    }
}

/// A user message as the CLI holds it: the text the agent got, read back
/// into what you typed, its pills and the files it names. Each file's ID
/// starts with `base`.
pub fn user_message(base: &str, sent: &str) -> ItemBody {
    let parsed = context::parse(sent, |path| AttachmentId::derive(&format!("{base}:{path}")));
    let attachments = parsed
        .files
        .iter()
        .map(|file| AttachmentRef {
            id: AttachmentId::derive(&format!("{base}:{}", file.path)),
            name: file.name.clone(),
            kind: file.kind,
            bytes: 0,
            mime: None,
            path: file.path.clone(),
            thumbnail: None,
        })
        .collect();
    ItemBody::UserMessage {
        text: parsed.text,
        attachments,
        refs: parsed.refs,
        delivery: Delivery::Sent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::timeline::{AttachmentKind, ItemId, ItemStatus};

    fn agent() -> AgentId {
        "0199a7c2-0000-7000-8000-000000000001".parse().unwrap()
    }

    fn message(id: &str, text: &str) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.into()),
            agent: agent(),
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Running,
            started_at: None,
            completed_at: None,
            body: ItemBody::AssistantMessage {
                text: text.into(),
                streaming: true,
            },
        }
    }

    #[test]
    fn appends_and_upserts_fold_into_one_item_each() {
        let events = vec![
            (
                agent(),
                TimelineEvent::Upsert {
                    item: message("a", "Hel"),
                },
            ),
            (
                agent(),
                TimelineEvent::Append {
                    id: ItemId("a".into()),
                    field: TextField::Text,
                    text: "lo".into(),
                },
            ),
            (
                agent(),
                TimelineEvent::Upsert {
                    item: message("b", "Bye"),
                },
            ),
            (
                agent(),
                TimelineEvent::Remove {
                    id: ItemId("b".into()),
                },
            ),
        ];
        let items = fold(agent(), events);
        assert_eq!(items.len(), 1);
        assert!(
            matches!(&items[0].body, ItemBody::AssistantMessage { text, .. } if text == "Hello")
        );
    }

    #[test]
    fn a_user_message_reads_back_its_attachment_lines() {
        let body = user_message(
            "cursor",
            "Look\n\n[Attached image \"red.png\" is saved at: /a/red.png]",
        );
        let ItemBody::UserMessage {
            text, attachments, ..
        } = body
        else {
            panic!("not a user message");
        };
        assert_eq!(text, "Look");
        assert_eq!(attachments[0].path, "/a/red.png");
        assert_eq!(attachments[0].kind, AttachmentKind::Image);
    }
}
