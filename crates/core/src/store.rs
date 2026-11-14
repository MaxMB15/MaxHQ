// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The state `maxd` serves: every entity, the current sequence number, and
//! the recent deltas a reconnecting client catches up from.
//!
//! Observers poll every few seconds and report what they see whether or not
//! it changed. The store drops changes that change nothing, so an idle device
//! sends no deltas at all.

use std::collections::{BTreeMap, VecDeque};

use maxhq_protocol::state::{Change, Delta, Entity, EntityKey, Seq, Snapshot};

/// Every entity at one sequence number, plus the deltas that led there.
#[derive(Clone, Debug)]
pub struct Store {
    seq: Seq,
    entities: BTreeMap<EntityKey, Entity>,
    recent: VecDeque<Delta>,
    keep: usize,
}

impl Store {
    /// An empty store at sequence 0 that keeps the last `keep` deltas.
    pub fn new(keep: usize) -> Self {
        Self {
            seq: Seq::default(),
            entities: BTreeMap::new(),
            recent: VecDeque::with_capacity(keep),
            keep,
        }
    }

    /// The current version.
    pub fn seq(&self) -> Seq {
        self.seq
    }

    /// The entity with this key.
    pub fn get(&self, key: &EntityKey) -> Option<&Entity> {
        self.entities.get(key)
    }

    /// Every entity, ordered by key.
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    /// The whole state now.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            seq: self.seq,
            entities: self.entities.values().cloned().collect(),
        }
    }

    /// The deltas after `since`, oldest first. None means the client needs
    /// a snapshot, because those deltas are gone or `since` is from another
    /// run of `maxd`.
    pub fn since(&self, since: Seq) -> Option<Vec<Delta>> {
        if since > self.seq {
            return None;
        }
        if since == self.seq {
            return Some(Vec::new());
        }
        let oldest = self.recent.front()?.seq;
        if since.next() < oldest {
            return None;
        }
        Some(
            self.recent
                .iter()
                .filter(|delta| delta.seq > since)
                .cloned()
                .collect(),
        )
    }

    /// Applies the changes in order and returns the delta they make, or
    /// none if they change nothing.
    pub fn apply(&mut self, changes: impl IntoIterator<Item = Change>) -> Option<Delta> {
        let mut applied = Vec::new();
        for change in changes {
            match change {
                Change::Upsert { value } => {
                    let key = value.key();
                    if self.entities.get(&key) != Some(&value) {
                        self.entities.insert(key, value.clone());
                        applied.push(Change::Upsert { value });
                    }
                }
                Change::Remove { key } => {
                    if self.entities.remove(&key).is_some() {
                        applied.push(Change::Remove { key });
                    }
                }
            }
        }
        if applied.is_empty() {
            return None;
        }
        self.seq = self.seq.next();
        let delta = Delta {
            seq: self.seq,
            changes: applied,
        };
        if self.recent.len() == self.keep {
            self.recent.pop_front();
        }
        if self.keep > 0 {
            self.recent.push_back(delta.clone());
        }
        Some(delta)
    }

    /// Makes the entities that `scope` selects exactly `entities`. Each one
    /// is upserted, and any entity in scope that isn't among them is
    /// removed. An observer passes everything it sees on each poll, with a
    /// scope that selects what it owns.
    pub fn replace(
        &mut self,
        scope: impl Fn(&Entity) -> bool,
        entities: impl IntoIterator<Item = Entity>,
    ) -> Option<Delta> {
        let entities: Vec<Entity> = entities.into_iter().collect();
        let keep: Vec<EntityKey> = entities.iter().map(Entity::key).collect();
        let gone: Vec<EntityKey> = self
            .entities
            .iter()
            .filter(|(key, entity)| scope(entity) && !keep.contains(key))
            .map(|(key, _)| *key)
            .collect();
        let changes = entities
            .into_iter()
            .map(|value| Change::Upsert { value })
            .chain(gone.into_iter().map(|key| Change::Remove { key }));
        self.apply(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maxhq_protocol::ids::{BootId, DeviceId};
    use maxhq_protocol::model::Device;
    use maxhq_protocol::state::Replica;

    fn device(n: u128, name: &str) -> Entity {
        Entity::Device(Device {
            id: DeviceId(uuid::Uuid::from_u128(n)),
            name: name.into(),
            os: "macos".into(),
            boot_id: BootId("b".into()),
            maxd_version: "0.1.0".into(),
            winding_down_until: None,
            workspace_host: None,
            build: None,
            target: None,
        })
    }

    fn upsert(value: Entity) -> Change {
        Change::Upsert { value }
    }

    #[test]
    fn changes_that_change_nothing_make_no_delta() {
        let mut store = Store::new(8);
        assert!(store.apply([upsert(device(1, "mac"))]).is_some());
        assert_eq!(store.apply([upsert(device(1, "mac"))]), None);
        let missing = device(2, "devbox").key();
        assert_eq!(store.apply([Change::Remove { key: missing }]), None);
        assert_eq!(store.seq(), Seq(1));
    }

    #[test]
    fn a_replica_fed_snapshot_and_deltas_matches_the_store() {
        let mut store = Store::new(8);
        store.apply([upsert(device(1, "mac"))]);
        let mut replica = Replica::default();
        replica.apply_snapshot(store.snapshot());

        let renamed = store.apply([upsert(device(1, "studio"))]).unwrap();
        let added = store.apply([upsert(device(2, "devbox"))]).unwrap();
        replica.apply_delta(renamed).unwrap();
        replica.apply_delta(added).unwrap();

        assert_eq!(replica.seq(), Some(store.seq()));
        assert!(replica.entities().eq(store.entities()));
    }

    #[test]
    fn since_returns_the_missing_deltas_or_asks_for_a_snapshot() {
        let mut store = Store::new(2);
        for n in 1..=3 {
            store.apply([upsert(device(n, "d"))]);
        }
        assert_eq!(store.since(Seq(3)), Some(vec![]));
        let seqs: Vec<Seq> = store.since(Seq(1)).unwrap().iter().map(|d| d.seq).collect();
        assert_eq!(seqs, [Seq(2), Seq(3)]);
        assert_eq!(store.since(Seq(0)), None, "delta 1 is no longer kept");
        assert_eq!(store.since(Seq(9)), None, "a sequence from another run");
    }

    #[test]
    fn replace_removes_what_the_scope_owns_and_no_longer_sees() {
        let mut store = Store::new(8);
        store.apply([upsert(device(1, "a")), upsert(device(2, "b"))]);
        let owned = |entity: &Entity| matches!(entity, Entity::Device(d) if d.name != "keep");
        store.apply([upsert(device(3, "keep"))]);

        let delta = store.replace(owned, [device(1, "a")]).unwrap();
        assert_eq!(
            delta.changes,
            vec![Change::Remove {
                key: device(2, "b").key()
            }]
        );
        assert_eq!(store.entities().count(), 2);
        assert_eq!(store.replace(owned, [device(1, "a")]), None);
    }
}
