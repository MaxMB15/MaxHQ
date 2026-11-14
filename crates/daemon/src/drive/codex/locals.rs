// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The local item each message sent from here shows as.
//!
//! A message you send shows at once as a local item, and Codex's copy of it
//! updates that item instead of adding another. Codex's copy has its own
//! ID, which is the one a later read of the thread's turns or its rollout
//! finds. Every driver and history reader in the process shares one map
//! from that ID to the local one, so the message stays one item when a new
//! app-server takes the thread up again or the manager reads the rollout
//! again. The manager keeps the local items until `maxd` exits, and the
//! map lasts as long.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use maxhq_protocol::timeline::{ItemId, TimelineItem};

/// The map the whole process shares.
static SHARED: LazyLock<Locals> = LazyLock::new(Locals::default);

/// Local item IDs, by the ID Codex's copy of the message shows under.
#[derive(Clone, Debug, Default)]
pub struct Locals(Arc<Mutex<HashMap<ItemId, ItemId>>>);

impl Locals {
    /// The map every driver and history reader in this process shares.
    pub fn shared() -> Self {
        SHARED.clone()
    }

    /// Notes that Codex's copy of a message, under `codex`, shows as the
    /// local item `local`.
    pub fn insert(&self, codex: ItemId, local: ItemId) {
        self.lock().insert(codex, local);
    }

    /// Moves Codex's copy of a message sent from here onto the local item
    /// it first showed as. Any other item stays as it is.
    pub fn keep(&self, item: &mut TimelineItem) {
        if let Some(local) = self.lock().get(&item.id) {
            item.id = local.clone();
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ItemId, ItemId>> {
        // Each change is one insert, so a panic elsewhere leaves the map
        // whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::AgentId;
    use maxhq_protocol::timeline::{Delivery, ItemBody, ItemStatus};

    use super::*;

    fn message(id: &str) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.to_owned()),
            agent: AgentId::derive("locals"),
            turn: None,
            parent: None,
            ordinal: 0,
            status: ItemStatus::Completed,
            started_at: None,
            completed_at: None,
            body: ItemBody::UserMessage {
                text: "hi".into(),
                attachments: Vec::new(),
                refs: Vec::new(),
                delivery: Delivery::Sent,
            },
        }
    }

    #[test]
    fn a_copy_moves_onto_its_local_item_and_others_stay() {
        let locals = Locals::default();
        locals.insert(ItemId("codex:u".into()), ItemId("codex:local:c".into()));
        let mut copy = message("codex:u");
        let mut typed = message("codex:v");
        locals.keep(&mut copy);
        locals.keep(&mut typed);
        assert_eq!(copy.id.0, "codex:local:c");
        assert_eq!(typed.id.0, "codex:v");
    }

    #[test]
    fn every_driver_and_reader_shares_one_map() {
        // The ID is this test's own, so tests that read recorded lines
        // through the shared map never meet it.
        Locals::shared().insert(
            ItemId("codex:locals-test".into()),
            ItemId("codex:local:locals-test".into()),
        );
        let mut copy = message("codex:locals-test");
        Locals::shared().keep(&mut copy);
        assert_eq!(copy.id.0, "codex:local:locals-test");
        assert!(Locals::default().lock().is_empty());
    }
}
