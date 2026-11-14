// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The items a driver knows, and the events that tell the session manager
//! how they changed. Streamed text goes out as appends while the item has
//! already been sent whole. Everything else goes out as whole items, once
//! per line, in the order the items were made.
//!
//! A quiet book appends only to items it sent whole itself, since it can't
//! tell what the session manager holds of the rest.

use std::collections::{HashMap, HashSet};

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::timeline::{ItemBody, ItemId, TextField, TimelineEvent, TimelineItem};

/// Every item a driver has made, and what changed since the last flush.
#[derive(Debug, Default)]
pub struct Book {
    /// Items by ID.
    items: HashMap<ItemId, TimelineItem>,
    /// Item IDs in the order they were made.
    order: Vec<ItemId>,
    /// Items to send whole at the next flush, in the order they changed.
    changed: Vec<ItemId>,
    /// The same IDs as `changed`, for lookups.
    marked: HashSet<ItemId>,
    /// Items made since the last flush, which the session manager hasn't
    /// heard of yet.
    fresh: HashSet<ItemId>,
    /// Items a flush has sent whole, so the session manager holds them as
    /// this book does.
    sent: HashSet<ItemId>,
    /// Appends and removals waiting for the next flush.
    events: Vec<(AgentId, TimelineEvent)>,
    /// Whether to hold back appends to items no flush has sent. Set while
    /// replaying lines the session manager may have seen in part, since an
    /// append isn't safe to repeat.
    quiet: bool,
    /// How many items the book had when it last forgot settled ones.
    looked: usize,
}

impl Book {
    /// Whether appends to items no flush has sent are held back.
    pub fn set_quiet(&mut self, quiet: bool) {
        self.quiet = quiet;
    }

    /// The item with this ID.
    pub fn get(&self, id: &ItemId) -> Option<&TimelineItem> {
        self.items.get(id)
    }

    /// Whether an item with this ID exists.
    pub fn contains(&self, id: &ItemId) -> bool {
        self.items.contains_key(id)
    }

    /// Every item, oldest first.
    pub fn items(&self) -> impl Iterator<Item = &TimelineItem> {
        self.order.iter().filter_map(|id| self.items.get(id))
    }

    /// Adds an item or replaces the one with its ID. A replacement that
    /// changes nothing sends nothing.
    pub fn put(&mut self, item: TimelineItem) {
        match self.items.get_mut(&item.id) {
            Some(current) if *current == item => {}
            Some(current) => {
                *current = item;
                let id = current.id.clone();
                self.mark(id);
            }
            None => {
                let id = item.id.clone();
                self.order.push(id.clone());
                self.fresh.insert(id.clone());
                self.items.insert(id.clone(), item);
                self.mark(id);
            }
        }
    }

    /// Changes an item in place, and sends it if anything changed. Returns
    /// whether the item exists.
    pub fn update(&mut self, id: &ItemId, change: impl FnOnce(&mut TimelineItem)) -> bool {
        let Some(item) = self.items.get_mut(id) else {
            return false;
        };
        let before = item.clone();
        change(item);
        if *item != before {
            self.mark(id.clone());
        }
        true
    }

    /// Adds streamed text to an item's text or output.
    pub fn append(&mut self, id: &ItemId, field: TextField, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(item) = self.items.get_mut(id) else {
            return;
        };
        let target = match (&mut item.body, field) {
            (ItemBody::AssistantMessage { text, .. }, TextField::Text)
            | (ItemBody::Reasoning { text, .. }, TextField::Text)
            | (ItemBody::Plan { markdown: text, .. }, TextField::Text)
            | (ItemBody::Command { output: text, .. }, TextField::Output) => text,
            _ => return,
        };
        target.push_str(text);
        if (self.quiet && !self.sent.contains(id)) || self.marked.contains(id) {
            self.mark(id.clone());
        } else {
            self.events.push((
                item.agent,
                TimelineEvent::Append {
                    id: id.clone(),
                    field,
                    text: text.to_owned(),
                },
            ));
        }
    }

    /// Drops an item and tells the session manager to drop it too.
    pub fn remove(&mut self, id: &ItemId) {
        let Some(item) = self.items.remove(id) else {
            return;
        };
        self.order.retain(|kept| kept != id);
        self.changed.retain(|kept| kept != id);
        self.marked.remove(id);
        self.sent.remove(id);
        if !self.fresh.remove(id) {
            self.events
                .push((item.agent, TimelineEvent::Remove { id: id.clone() }));
        }
    }

    /// The items that changed since the last call, whole, oldest first.
    /// Removals are dropped, since only a live stream reports them.
    pub fn take_changed(&mut self) -> Vec<TimelineItem> {
        self.events.clear();
        self.drain_changed()
    }

    /// The events since the last flush: appends and removals first, then
    /// each changed item whole, oldest first.
    pub fn flush(&mut self) -> Vec<(AgentId, TimelineEvent)> {
        let mut events = std::mem::take(&mut self.events);
        for item in self.drain_changed() {
            self.sent.insert(item.id.clone());
            events.push((item.agent, TimelineEvent::Upsert { item }));
        }
        events
    }

    /// Forgets the oldest items `settled` says won't change again, beyond
    /// the newest `keep`, once the session manager holds them. It looks
    /// again only after the book grew by half of `keep`, so a book of
    /// items that never settle isn't read through on every line. Returns
    /// whether it looked.
    pub fn forget_settled(&mut self, keep: usize, settled: impl Fn(&TimelineItem) -> bool) -> bool {
        if self.order.len() < self.looked + keep / 2 || self.order.len() <= keep {
            return false;
        }
        let cut = self.order.len() - keep;
        let mut at = 0;
        self.order.retain(|id| {
            at += 1;
            let forget = at <= cut
                && !self.marked.contains(id)
                && !self.fresh.contains(id)
                && self.items.get(id).is_some_and(&settled);
            if forget {
                self.items.remove(id);
                self.sent.remove(id);
            }
            !forget
        });
        self.looked = self.order.len();
        true
    }

    /// Notes that an item needs sending whole.
    fn mark(&mut self, id: ItemId) {
        if self.marked.insert(id.clone()) {
            self.changed.push(id);
        }
    }

    /// The changed items in the order they were made, which is the order
    /// the session manager numbers new ones in.
    fn drain_changed(&mut self) -> Vec<TimelineItem> {
        self.fresh.clear();
        if self.changed.is_empty() {
            return Vec::new();
        }
        self.changed.clear();
        let marked = std::mem::take(&mut self.marked);
        self.order
            .iter()
            .filter(|id| marked.contains(*id))
            .filter_map(|id| self.items.get(id).cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::timeline::ItemStatus;

    fn message(id: &str, text: &str) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.into()),
            agent: AgentId::derive("lead"),
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

    fn upserted(events: &[(AgentId, TimelineEvent)]) -> Vec<String> {
        events
            .iter()
            .map(|(_, event)| match event {
                TimelineEvent::Upsert { item } => format!("upsert {}", item.id.0),
                TimelineEvent::Append { id, text, .. } => format!("append {} {text}", id.0),
                TimelineEvent::Remove { id } => format!("remove {}", id.0),
            })
            .collect()
    }

    #[test]
    fn new_items_go_whole_and_later_text_goes_as_appends() {
        let mut book = Book::default();
        let id = ItemId("a".into());
        book.put(message("a", "Hel"));
        book.append(&id, TextField::Text, "lo");
        assert_eq!(upserted(&book.flush()), ["upsert a"]);
        book.append(&id, TextField::Text, " there");
        assert_eq!(upserted(&book.flush()), ["append a  there"]);
        let ItemBody::AssistantMessage { text, .. } = &book.get(&id).unwrap().body else {
            panic!();
        };
        assert_eq!(text, "Hello there");
    }

    #[test]
    fn a_quiet_book_sends_whole_items_and_skips_unchanged_ones() {
        let mut book = Book::default();
        let id = ItemId("a".into());
        book.put(message("a", "Hi"));
        book.take_changed();
        book.set_quiet(true);
        book.put(message("a", "Hi"));
        assert!(book.flush().is_empty());
        book.append(&id, TextField::Text, "!");
        assert_eq!(upserted(&book.flush()), ["upsert a"]);
    }

    #[test]
    fn a_quiet_book_appends_to_items_it_sent_whole() {
        let mut book = Book::default();
        let id = ItemId("a".into());
        book.set_quiet(true);
        book.put(message("a", "Hel"));
        book.append(&id, TextField::Text, "l");
        assert_eq!(upserted(&book.flush()), ["upsert a"]);
        book.append(&id, TextField::Text, "o");
        assert_eq!(upserted(&book.flush()), ["append a o"]);
        book.remove(&id);
        book.flush();
        book.put(message("a", "Hi"));
        book.take_changed();
        book.append(&id, TextField::Text, "!");
        assert_eq!(upserted(&book.flush()), ["upsert a"]);
    }

    #[test]
    fn only_old_settled_items_the_manager_holds_are_forgotten() {
        let mut book = Book::default();
        for at in 0..8 {
            book.put(message(&at.to_string(), "text"));
        }
        book.flush();
        let settled = |item: &TimelineItem| item.status == ItemStatus::Completed;
        for at in [0, 1, 2, 6, 7] {
            book.update(&ItemId(at.to_string()), |item| {
                item.status = ItemStatus::Completed
            });
        }
        // Item 2 changed since the flush, so it waits to be sent.
        book.flush();
        book.update(&ItemId("2".into()), |item| item.ordinal = 1);
        book.put(message("8", "new"));
        book.forget_settled(4, settled);
        let ids: Vec<&str> = book.items().map(|item| item.id.0.as_str()).collect();
        assert_eq!(ids, ["2", "3", "4", "5", "6", "7", "8"]);
        assert_eq!(upserted(&book.flush()), ["upsert 2", "upsert 8"]);
        // A forgotten item that comes again is new.
        book.put(message("0", "text"));
        assert_eq!(upserted(&book.flush()), ["upsert 0"]);
    }

    #[test]
    fn changes_flush_in_the_order_items_were_made() {
        let mut book = Book::default();
        book.put(message("a", "1"));
        book.put(message("b", "2"));
        book.flush();
        book.update(&ItemId("b".into()), |item| {
            item.status = ItemStatus::Completed
        });
        book.update(&ItemId("a".into()), |item| {
            item.status = ItemStatus::Completed
        });
        book.put(message("c", "3"));
        book.remove(&ItemId("c".into()));
        book.remove(&ItemId("b".into()));
        assert_eq!(upserted(&book.flush()), ["remove b", "upsert a"]);
        assert!(!book.update(&ItemId("c".into()), |_| {}));
    }
}
