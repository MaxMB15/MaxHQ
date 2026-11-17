// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! One agent's timeline: its items in order, and its recent events so a
//! client that drops can catch up instead of loading everything again.
//!
//! Each new item ID gets the next ordinal. Each event that changes the
//! timeline gets the next seq, and the last [`KEEP_EVENTS`] are kept, as
//! long as they fit in [`KEEP_EVENT_BYTES`]. A client that asks from a seq
//! inside that window gets the events after it, and any other client gets
//! a reset to the newest items.
//!
//! The newest items stay in memory, as many as the device's settings
//! say, and so does every item that may still change. The others go to
//! disk, where a page, a find or a read reaches them, up to the settings'
//! byte limit. See [`Limits`].

mod spill;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io;
use std::path::PathBuf;

use maxhq_core::context;
use maxhq_protocol::ids::{AgentId, AttachmentId, DeviceId};
use maxhq_protocol::model::{DeviceSettings, DEFAULT_HISTORY_MIB, DEFAULT_MEMORY_ITEMS};
use maxhq_protocol::state::Seq;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{
    AttachmentRef, ItemBody, ItemId, ItemStatus, TextField, TimelineEvent, TimelineItem,
    TimelineUpdate,
};
use tracing::warn;

use crate::workflow::Notes;

pub use spill::Spilled;
use spill::{Disk, Place};

/// How many events each timeline keeps for catch-up.
pub const KEEP_EVENTS: usize = 4096;

/// The most bytes of events, as JSON, each timeline keeps for catch-up.
/// The newest event stays whatever its size.
pub const KEEP_EVENT_BYTES: usize = 4 << 20;

/// The most items one reset or page holds.
pub const MAX_PAGE: usize = 4096;

/// What each timeline keeps, from the device's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// How many of the newest items stay in memory.
    pub items: usize,
    /// The most bytes of items on disk, or none for no limit. Past it the
    /// oldest leave the timeline, though the agent's history still has
    /// them.
    pub disk: Option<u64>,
}

impl Limits {
    /// The limits `settings` set.
    pub fn of(settings: &DeviceSettings) -> Self {
        Self {
            items: settings.items_in_memory(),
            disk: settings.history_bytes(),
        }
    }
}

impl Default for Limits {
    /// A little more than a client's page in memory, and 256 MiB on disk.
    fn default() -> Self {
        Self {
            items: usize::from(DEFAULT_MEMORY_ITEMS),
            disk: Some(u64::from(DEFAULT_HISTORY_MIB) << 20),
        }
    }
}

/// One agent's items and recent events.
pub struct Timeline {
    /// Items by ordinal.
    items: BTreeMap<u64, Slot>,
    /// The ordinals of the items in memory.
    kept: BTreeSet<u64>,
    /// How many items were in memory after the last look for ones to put
    /// on disk.
    looked: usize,
    /// Where items not in memory go, or none to keep them all.
    disk: Option<Disk>,
    /// How many items stay in memory, and how many bytes go on disk.
    limits: Limits,
    /// Each item's ordinal.
    ordinals: HashMap<ItemId, u64>,
    /// The ordinals of items that came from history rather than this
    /// process.
    from_history: HashSet<u64>,
    /// Items from before the agent moved to another vendor. They stay
    /// first, since the new vendor's history doesn't have them.
    earlier: HashSet<ItemId>,
    /// Where each `!` run came: the item before it, or none when it came
    /// first, and when it started. The vendor's history doesn't have runs,
    /// so a read of it puts them back here.
    anchors: HashMap<ItemId, (Option<ItemId>, Timestamp)>,
    /// The ordinal the next new item gets.
    next: u64,
    /// The seq of the newest event.
    seq: u64,
    /// The oldest seq a client can catch up from.
    floor: u64,
    /// Recent events with their seqs and sizes, oldest first.
    events: VecDeque<(u64, usize, TimelineEvent)>,
    /// The size of every event kept.
    event_bytes: usize,
    /// The device a live agent pill read back from history runs on, if
    /// it's known.
    here: Option<DeviceId>,
    /// Where the agent's prompt head and commands go, for binding it to
    /// a workflow's nodes.
    notes: Option<(AgentId, Notes)>,
}

/// An item in memory, or where it is on disk.
enum Slot {
    Kept(Box<TimelineItem>),
    Spilled(Place),
}

/// An item as a timeline holds it, for a reader that reads what's on
/// disk off the loop.
// A reader that takes items off the loop owns those in memory, and boxing
// each would cost an allocation to shrink the ones on disk.
#[allow(clippy::large_enum_variant)]
pub enum Stored<'a> {
    Kept(Cow<'a, TimelineItem>),
    /// An item on disk, with its ordinal.
    Spilled(u64, Spilled),
}

impl Stored<'_> {
    /// The item held apart from its timeline, to take off the loop.
    pub fn into_owned(self) -> Stored<'static> {
        match self {
            Stored::Kept(item) => Stored::Kept(Cow::Owned(item.into_owned())),
            Stored::Spilled(ordinal, spilled) => Stored::Spilled(ordinal, spilled),
        }
    }

    /// The item, read back from disk if it's there.
    pub fn load(self) -> io::Result<TimelineItem> {
        match self {
            Stored::Kept(item) => Ok(item.into_owned()),
            Stored::Spilled(ordinal, spilled) => {
                let mut item = spilled.load()?;
                item.ordinal = ordinal;
                Ok(item)
            }
        }
    }
}

impl Timeline {
    /// An empty timeline whose seqs start after `base`. Giving each run of
    /// `maxd` a higher base means a seq from an earlier run never matches.
    /// It keeps every item in memory unless it's [`Timeline::spilling`].
    pub fn new(base: u64) -> Self {
        Self {
            items: BTreeMap::new(),
            kept: BTreeSet::new(),
            looked: 0,
            disk: None,
            limits: Limits::default(),
            ordinals: HashMap::new(),
            from_history: HashSet::new(),
            earlier: HashSet::new(),
            anchors: HashMap::new(),
            next: 0,
            seq: base,
            floor: base,
            events: VecDeque::new(),
            event_bytes: 0,
            here: None,
            notes: None,
        }
    }

    /// The timeline, telling `notes` the agent's first prompt and each
    /// command it finishes.
    pub fn noting(mut self, agent: AgentId, notes: Notes) -> Self {
        self.notes = Some((agent, notes));
        self
    }

    /// The timeline, with each live agent pill read back from history
    /// placed on `device`. The read tool only reaches agents there, so a
    /// pill sent for that tool names one of them.
    pub fn on(mut self, device: DeviceId) -> Self {
        self.here = Some(device);
        self
    }

    /// The timeline, putting the items it doesn't keep in memory in files
    /// in `dir`.
    pub fn spilling(mut self, dir: PathBuf) -> Self {
        self.disk = Some(Disk::new(dir));
        self
    }

    /// The timeline, keeping to `limits`.
    pub fn limited(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Keeps to `limits` from now on. Fewer items or bytes than before
    /// take effect at the next look for items to put on disk, so a change
    /// doesn't move every timeline's items at once.
    pub fn limit(&mut self, limits: Limits) {
        self.limits = limits;
    }

    /// The timeline's version.
    pub fn seq(&self) -> Seq {
        Seq(self.seq)
    }

    /// The item with this ID.
    pub fn get(&self, id: &ItemId) -> Option<Cow<'_, TimelineItem>> {
        self.at(*self.ordinals.get(id)?)
    }

    /// Whether an item has this ID.
    pub fn contains(&self, id: &ItemId) -> bool {
        self.ordinals.contains_key(id)
    }

    /// Whether the timeline has no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Every item, oldest first, each read back from disk if it's there.
    pub fn items(&self) -> impl DoubleEndedIterator<Item = Cow<'_, TimelineItem>> {
        self.items
            .iter()
            .filter_map(|(ordinal, slot)| self.read(*ordinal, slot))
    }

    /// The items in memory, oldest first. They include every item that
    /// may still change, so every queued message, and every message with
    /// pills.
    pub fn kept(&self) -> impl DoubleEndedIterator<Item = &TimelineItem> {
        self.kept
            .iter()
            .filter_map(|ordinal| match self.items.get(ordinal) {
                Some(Slot::Kept(item)) => Some(&**item),
                _ => None,
            })
    }

    /// Every item, oldest first, as it's held, for a reader that reads
    /// what's on disk itself.
    pub fn stored(&self) -> impl Iterator<Item = Stored<'_>> {
        self.items.iter().filter_map(|(ordinal, slot)| match slot {
            Slot::Kept(item) => Some(Stored::Kept(Cow::Borrowed(item))),
            Slot::Spilled(place) => {
                let spilled = self.disk.as_ref()?.spilled(*place).ok()?;
                Some(Stored::Spilled(*ordinal, spilled))
            }
        })
    }

    /// Each item's ID by its ordinal.
    pub fn ids(&self) -> HashMap<u64, &ItemId> {
        self.ordinals
            .iter()
            .map(|(id, ordinal)| (*ordinal, id))
            .collect()
    }

    /// Applies one event. Returns whether it finished a tool item or an
    /// assistant message, or none when the event changed nothing it knows,
    /// such as text for an item it doesn't have.
    pub fn apply(&mut self, event: TimelineEvent) -> Option<bool> {
        let (event, finished) = match event {
            TimelineEvent::Upsert { mut item } => {
                self.place(&mut item);
                let ordinal = match self.ordinals.get(&item.id) {
                    Some(ordinal) => *ordinal,
                    None => {
                        let ordinal = self.next;
                        self.next += 1;
                        self.ordinals.insert(item.id.clone(), ordinal);
                        ordinal
                    }
                };
                item.ordinal = ordinal;
                let before = self.at(ordinal).map(|stored| {
                    keep_previews(&stored, &mut item);
                    stored.status
                });
                self.from_history.remove(&ordinal);
                let finished = finishes(&item) && before.is_none_or(|status| !is_end(status));
                if let Some((agent, notes)) = &self.notes {
                    notes.item(*agent, &item, finished);
                }
                self.keep(item.clone());
                (TimelineEvent::Upsert { item }, finished)
            }
            TimelineEvent::Append { id, field, text } => {
                let ordinal = *self.ordinals.get(&id)?;
                let item = self.kept_mut(ordinal)?;
                append(&mut item.body, field, &text)?;
                (TimelineEvent::Append { id, field, text }, false)
            }
            TimelineEvent::Remove { id } => {
                let ordinal = self.ordinals.remove(&id)?;
                self.anchors.remove(&id);
                self.items.remove(&ordinal);
                self.kept.remove(&ordinal);
                self.from_history.remove(&ordinal);
                (TimelineEvent::Remove { id }, false)
            }
        };
        self.record(event);
        self.trim();
        Some(finished)
    }

    /// Fills in the preview of attachment `id` on each item that shows it
    /// without one, which clients get as an update of the item. The item
    /// itself hasn't changed, so it stays as it came from history. Such
    /// items stay in memory until they have their previews.
    pub fn show_preview(&mut self, id: &AttachmentId, preview: &str) {
        let mut changed = Vec::new();
        for ordinal in &self.kept {
            let Some(Slot::Kept(item)) = self.items.get_mut(ordinal) else {
                continue;
            };
            let mut filled = false;
            for image in images_mut(&mut item.body) {
                if image.id == *id && image.thumbnail.is_none() {
                    image.thumbnail = Some(preview.to_owned());
                    filled = true;
                }
            }
            if filled {
                changed.push((**item).clone());
            }
        }
        for item in changed {
            self.record(TimelineEvent::Upsert { item });
        }
    }

    /// Replaces the items from before this process with `items`, oldest
    /// first. Items this process reported stay after them, and an item in
    /// both keeps what this process said. The ordinals all change, so every
    /// client needs a reset afterwards.
    pub fn replace_history(&mut self, items: Vec<TimelineItem>) {
        // Items from before the agent changed vendors go first, and the
        // history has no say over them. Each item comes with whether it's
        // from the history.
        let mut order: Vec<(TimelineItem, bool)> = Vec::with_capacity(items.len());
        let mut live: Vec<Option<TimelineItem>> = Vec::new();
        for (ordinal, slot) in std::mem::take(&mut self.items) {
            if self.from_history.contains(&ordinal) {
                continue;
            }
            let item = match slot {
                Slot::Kept(item) => *item,
                Slot::Spilled(place) => match self.load(ordinal, place) {
                    Some(item) => item,
                    None => continue,
                },
            };
            if self.earlier.contains(&item.id) {
                order.push((item, false));
            } else {
                live.push(Some(item));
            }
        }
        if let Some(disk) = &mut self.disk {
            disk.clear();
        }
        let mut placed: HashSet<ItemId> = order.iter().map(|(item, _)| item.id.clone()).collect();
        let live_at: HashMap<ItemId, usize> = live
            .iter()
            .enumerate()
            .filter_map(|(at, item)| Some((item.as_ref()?.id.clone(), at)))
            .collect();
        for item in items {
            if !placed.insert(item.id.clone()) {
                continue;
            }
            match live_at.get(&item.id).and_then(|at| live[*at].take()) {
                Some(current) => order.push((current, false)),
                None => order.push((item, true)),
            }
        }
        order.extend(live.into_iter().flatten().map(|item| (item, false)));
        let order = self.anchored(order);
        self.ordinals.clear();
        self.kept.clear();
        self.from_history.clear();
        for (mut item, from_history) in order {
            self.place(&mut item);
            item.ordinal = self.next;
            self.next += 1;
            self.ordinals.insert(item.id.clone(), item.ordinal);
            if from_history {
                self.from_history.insert(item.ordinal);
            }
            self.keep(item);
        }
        self.seq += 1;
        self.floor = self.seq;
        self.events.clear();
        self.event_bytes = 0;
        if let Some((agent, notes)) = &self.notes {
            notes.history(*agent, self.kept());
        }
        self.looked = 0;
        self.trim();
    }

    /// The ID of the newest item, if there is one.
    pub fn last_id(&self) -> Option<ItemId> {
        let (id, _) = self.ordinals.iter().max_by_key(|(_, ordinal)| **ordinal)?;
        Some(id.clone())
    }

    /// Notes that the `!` run `id` came after the item `after`, or first
    /// with none, at `at`, so a history read puts it back there.
    pub fn anchor(&mut self, id: ItemId, after: Option<ItemId>, at: Timestamp) {
        self.anchors.insert(id, (after, at));
    }

    /// Puts back the `!` runs an agent kept, each with the item it came
    /// after and when it started. A history read then places them.
    pub fn add_runs(&mut self, runs: Vec<(TimelineItem, Option<ItemId>, Timestamp)>) {
        for (item, after, at) in runs {
            self.anchor(item.id.clone(), after, at);
            self.apply(TimelineEvent::Upsert { item });
        }
    }

    /// `order` with each `!` run moved to just after the item it came
    /// after, and after any run placed there that started before it. A run
    /// whose item has gone goes before the first item that started after
    /// it.
    fn anchored(&self, order: Vec<(TimelineItem, bool)>) -> Vec<(TimelineItem, bool)> {
        if self.anchors.is_empty() {
            return order;
        }
        let (mut runs, mut order): (Vec<_>, Vec<_>) = order.into_iter().partition(|(item, _)| {
            self.anchors.contains_key(&item.id) && !self.earlier.contains(&item.id)
        });
        runs.sort_by_key(|(item, _)| self.anchors[&item.id].1);
        let mut placed: HashMap<ItemId, Timestamp> = HashMap::new();
        for run in runs {
            let (after, at) = &self.anchors[&run.0.id];
            let mut index = match after {
                None => (order.iter())
                    .take_while(|(item, _)| self.earlier.contains(&item.id))
                    .count(),
                Some(after) => match order.iter().position(|(item, _)| item.id == *after) {
                    Some(index) => index + 1,
                    None => (order.iter())
                        .position(|(item, _)| item.started_at.is_some_and(|started| started > *at))
                        .unwrap_or(order.len()),
                },
            };
            while (order.get(index))
                .is_some_and(|(item, _)| placed.get(&item.id).is_some_and(|other| other <= at))
            {
                index += 1;
            }
            placed.insert(run.0.id.clone(), *at);
            order.insert(index, run);
        }
        order
    }

    /// Keeps every item ahead of what any history read brings from now
    /// on, as when the agent moves to a vendor whose history doesn't have
    /// them.
    pub fn freeze(&mut self) {
        self.from_history.clear();
        self.earlier = self.ordinals.keys().cloned().collect();
    }

    /// The items kept by [`Timeline::freeze`], oldest first.
    pub fn earlier(&self) -> Vec<TimelineItem> {
        let mut ordinals: Vec<u64> = self
            .earlier
            .iter()
            .filter_map(|id| self.ordinals.get(id).copied())
            .collect();
        ordinals.sort_unstable();
        ordinals
            .into_iter()
            .filter_map(|ordinal| self.at(ordinal))
            .map(Cow::into_owned)
            .collect()
    }

    /// Puts back the items [`Timeline::earlier`] gave before a restart,
    /// ahead of everything else.
    pub fn thaw(&mut self, items: Vec<TimelineItem>) {
        self.earlier = items.iter().map(|item| item.id.clone()).collect();
        self.replace_history(items);
        self.from_history.clear();
    }

    /// Places the item's live agent pills on this timeline's device.
    fn place(&self, item: &mut TimelineItem) {
        if let Some(device) = self.here {
            context::place(&mut item.body, device);
        }
    }

    /// The events after `seq`, or none when they're no longer kept or the
    /// seq was never this timeline's.
    pub fn since(&self, seq: Seq) -> Option<Vec<TimelineEvent>> {
        if seq.0 < self.floor || seq.0 > self.seq {
            return None;
        }
        Some(
            self.events
                .iter()
                .filter(|(at, ..)| *at > seq.0)
                .map(|(.., event)| event.clone())
                .collect(),
        )
    }

    /// The newest `limit` items, oldest first, and whether older ones exist.
    pub fn newest(&self, limit: usize) -> (Vec<TimelineItem>, bool) {
        page(self.items().rev(), limit)
    }

    /// The `limit` items before `ordinal`, oldest first, and whether older
    /// ones exist.
    pub fn older(&self, ordinal: u64, limit: usize) -> (Vec<TimelineItem>, bool) {
        page(
            self.items
                .range(..ordinal)
                .rev()
                .filter_map(|(ordinal, slot)| self.read(*ordinal, slot)),
            limit,
        )
    }

    /// What a client opening the timeline gets: the events after its seq
    /// when they're kept, or a reset to the newest `limit` items.
    pub fn open(&self, since: Option<Seq>, limit: usize) -> TimelineUpdate {
        if let Some(events) = since.and_then(|seq| self.since(seq)) {
            return TimelineUpdate::Events {
                seq: self.seq(),
                events,
            };
        }
        self.reset(limit)
    }

    /// A reset to the newest `limit` items.
    pub fn reset(&self, limit: usize) -> TimelineUpdate {
        let (items, has_older) = self.newest(limit);
        TimelineUpdate::Reset {
            seq: self.seq(),
            items,
            has_older,
        }
    }

    fn record(&mut self, event: TimelineEvent) {
        self.seq += 1;
        let bytes = json_len(&event);
        self.event_bytes += bytes;
        self.events.push_back((self.seq, bytes, event));
        while self.events.len() > KEEP_EVENTS
            || (self.event_bytes > KEEP_EVENT_BYTES && self.events.len() > 1)
        {
            if let Some((seq, bytes, _)) = self.events.pop_front() {
                self.floor = seq;
                self.event_bytes -= bytes;
            }
        }
    }

    /// The item at `ordinal`, read back from disk if it's there.
    fn at(&self, ordinal: u64) -> Option<Cow<'_, TimelineItem>> {
        self.read(ordinal, self.items.get(&ordinal)?)
    }

    /// The item in `slot`, read back from disk if it's there.
    fn read<'a>(&'a self, ordinal: u64, slot: &'a Slot) -> Option<Cow<'a, TimelineItem>> {
        match slot {
            Slot::Kept(item) => Some(Cow::Borrowed(item)),
            Slot::Spilled(place) => self.load(ordinal, *place).map(Cow::Owned),
        }
    }

    /// Reads back the item at `ordinal` from `place` on disk.
    fn load(&self, ordinal: u64, place: Place) -> Option<TimelineItem> {
        match self.disk.as_ref()?.read(place) {
            Ok(mut item) => {
                item.ordinal = ordinal;
                Some(item)
            }
            Err(err) => {
                warn!(%err, ordinal, "couldn't read a timeline item back from disk");
                None
            }
        }
    }

    /// Puts `item` in memory at its ordinal.
    fn keep(&mut self, item: TimelineItem) {
        self.kept.insert(item.ordinal);
        self.items.insert(item.ordinal, Slot::Kept(Box::new(item)));
    }

    /// The item at `ordinal`, back in memory to change.
    fn kept_mut(&mut self, ordinal: u64) -> Option<&mut TimelineItem> {
        if let Some(Slot::Spilled(place)) = self.items.get(&ordinal) {
            let item = self.load(ordinal, *place)?;
            self.keep(item);
        }
        match self.items.get_mut(&ordinal)? {
            Slot::Kept(item) => Some(item),
            Slot::Spilled(_) => None,
        }
    }

    /// Puts on disk the items in memory that won't change, all but the
    /// newest the limits keep, once enough have built up to be worth it.
    fn trim(&mut self) {
        let Some(disk) = &mut self.disk else {
            return;
        };
        let keep = self.limits.items.max(1);
        if self.kept.len() < self.looked.max(keep) + keep / 2 {
            return;
        }
        let Some(&newest) = self.items.keys().nth_back(keep - 1) else {
            return;
        };
        let out: Vec<(u64, &TimelineItem)> = self
            .kept
            .range(..newest)
            .filter_map(|ordinal| match self.items.get(ordinal) {
                Some(Slot::Kept(item)) if !stays(item) => Some((*ordinal, &**item)),
                _ => None,
            })
            .collect();
        let written = disk.write(out.iter().map(|(_, item)| *item));
        let out: Vec<u64> = out.into_iter().map(|(ordinal, _)| ordinal).collect();
        match written {
            Ok(places) => {
                for (ordinal, place) in out.into_iter().zip(places) {
                    self.kept.remove(&ordinal);
                    self.items.insert(ordinal, Slot::Spilled(place));
                }
            }
            Err(err) => {
                warn!(%err, "couldn't put a timeline's older items on disk, so they stay in memory")
            }
        }
        self.looked = self.kept.len();
        self.cap();
    }

    /// Drops the oldest items on disk while they take more than the
    /// limits keep. Clients may have paged back to them, so each one
    /// dropped goes out as a removal.
    fn cap(&mut self) {
        let (Some(disk), Some(most)) = (&mut self.disk, self.limits.disk) else {
            return;
        };
        let mut files = HashSet::new();
        while disk.size() > most
            && let Some(file) = disk.drop_oldest()
        {
            files.insert(file);
        }
        if files.is_empty() {
            return;
        }
        let gone: HashSet<u64> = self
            .items
            .iter()
            .filter_map(|(ordinal, slot)| {
                matches!(slot, Slot::Spilled(place) if files.contains(&place.file()))
                    .then_some(*ordinal)
            })
            .collect();
        self.items.retain(|ordinal, _| !gone.contains(ordinal));
        self.from_history.retain(|ordinal| !gone.contains(ordinal));
        let mut removed: Vec<(u64, ItemId)> = Vec::with_capacity(gone.len());
        self.ordinals.retain(|id, ordinal| {
            let keep = !gone.contains(ordinal);
            if !keep {
                self.earlier.remove(id);
                self.anchors.remove(id);
                removed.push((*ordinal, id.clone()));
            }
            keep
        });
        removed.sort_unstable_by_key(|(ordinal, _)| *ordinal);
        for (_, id) in removed {
            self.record(TimelineEvent::Remove { id });
        }
        warn!(
            items = gone.len(),
            "dropped a timeline's oldest items, past what it keeps on disk"
        );
    }
}

/// Whether an item stays in memory: it may still change, it's a message
/// with pills, which the read tool's checks look through, or a picture in
/// it waits for its preview.
fn stays(item: &TimelineItem) -> bool {
    matches!(
        item.status,
        ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting
    ) || matches!(&item.body, ItemBody::UserMessage { refs, .. } if !refs.is_empty())
        || images(&item.body)
            .iter()
            .any(|image| image.thumbnail.is_none())
}

/// How many bytes `event` takes as JSON.
fn json_len(event: &TimelineEvent) -> usize {
    struct Count(usize);
    impl io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    let _ = serde_json::to_writer(&mut count, event);
    count.0
}

/// The pictures an item shows from a tool's result.
fn images(body: &ItemBody) -> &[AttachmentRef] {
    match body {
        ItemBody::FileRead { images, .. } | ItemBody::ToolCall { images, .. } => images,
        _ => &[],
    }
}

/// The pictures an item shows from a tool's result, to change.
fn images_mut(body: &mut ItemBody) -> &mut [AttachmentRef] {
    match body {
        ItemBody::FileRead { images, .. } | ItemBody::ToolCall { images, .. } => images,
        _ => &mut [],
    }
}

/// Keeps previews on a new copy of an item. A preview fills in here, so the
/// driver's copy doesn't have it.
fn keep_previews(stored: &TimelineItem, item: &mut TimelineItem) {
    let had = images(&stored.body);
    for image in images_mut(&mut item.body) {
        if image.thumbnail.is_none()
            && let Some(old) = had.iter().find(|old| old.id == image.id)
        {
            image.thumbnail.clone_from(&old.thumbnail);
        }
    }
}

/// Items newest first, cut to `limit`, returned oldest first.
fn page<'a>(
    newest_first: impl Iterator<Item = Cow<'a, TimelineItem>>,
    limit: usize,
) -> (Vec<TimelineItem>, bool) {
    let limit = limit.min(MAX_PAGE);
    let mut items: Vec<TimelineItem> = Vec::new();
    let mut has_older = false;
    for item in newest_first {
        if items.len() == limit {
            has_older = true;
            break;
        }
        items.push(item.into_owned());
    }
    items.reverse();
    (items, has_older)
}

/// Whether the status is one an item ends in.
fn is_end(status: ItemStatus) -> bool {
    matches!(status, ItemStatus::Completed | ItemStatus::Failed)
}

/// Whether the item is a tool call or an assistant message that has ended,
/// which counts as progress.
fn finishes(item: &TimelineItem) -> bool {
    is_end(item.status)
        && (is_tool(&item.body) || matches!(item.body, ItemBody::AssistantMessage { .. }))
}

/// Whether the body is a tool the agent ran.
pub fn is_tool(body: &ItemBody) -> bool {
    matches!(
        body,
        ItemBody::Command { .. }
            | ItemBody::FileChange { .. }
            | ItemBody::FileRead { .. }
            | ItemBody::Search { .. }
            | ItemBody::Web { .. }
            | ItemBody::ToolCall { .. }
            | ItemBody::Subagent { .. }
    )
}

/// Adds streamed text to the field it names. Returns none when the body
/// has no such field.
fn append(body: &mut ItemBody, field: TextField, more: &str) -> Option<()> {
    let target = match (field, body) {
        (TextField::Text, ItemBody::AssistantMessage { text, .. })
        | (TextField::Text, ItemBody::Reasoning { text, .. })
        | (TextField::Text, ItemBody::Plan { markdown: text, .. })
        | (TextField::Output, ItemBody::Command { output: text, .. }) => text,
        _ => return None,
    };
    target.push_str(more);
    Some(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use maxhq_protocol::ids::AgentId;

    /// An assistant message with this ID and text.
    pub fn message(id: &str, text: &str, status: ItemStatus) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.into()),
            agent: AgentId::derive("a"),
            turn: None,
            parent: None,
            ordinal: 0,
            status,
            started_at: None,
            completed_at: None,
            body: ItemBody::AssistantMessage {
                text: text.into(),
                streaming: status == ItemStatus::Running,
            },
        }
    }

    fn upsert(item: TimelineItem) -> TimelineEvent {
        TimelineEvent::Upsert { item }
    }

    fn texts(items: &[TimelineItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match &item.body {
                ItemBody::AssistantMessage { text, .. } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn new_ids_get_the_next_ordinal_and_repeats_keep_theirs() {
        let mut timeline = Timeline::new(100);
        timeline.apply(upsert(message("a", "one", ItemStatus::Running)));
        timeline.apply(upsert(message("b", "two", ItemStatus::Running)));
        timeline.apply(upsert(message("a", "one!", ItemStatus::Completed)));
        let ordinals: Vec<u64> = timeline.items().map(|item| item.ordinal).collect();
        assert_eq!(ordinals, [0, 1]);
        assert_eq!(texts(&timeline.newest(10).0), ["one!", "two"]);
        assert_eq!(timeline.seq(), Seq(103));
    }

    #[test]
    fn appends_grow_the_named_field_and_unknown_ids_are_dropped() {
        let mut timeline = Timeline::new(0);
        timeline.apply(upsert(message("a", "Hel", ItemStatus::Running)));
        let more = TimelineEvent::Append {
            id: ItemId("a".into()),
            field: TextField::Text,
            text: "lo".into(),
        };
        assert_eq!(timeline.apply(more), Some(false));
        let lost = TimelineEvent::Append {
            id: ItemId("nope".into()),
            field: TextField::Text,
            text: "x".into(),
        };
        assert_eq!(timeline.apply(lost), None);
        let wrong = TimelineEvent::Append {
            id: ItemId("a".into()),
            field: TextField::Output,
            text: "x".into(),
        };
        assert_eq!(timeline.apply(wrong), None);
        assert_eq!(texts(&timeline.newest(1).0), ["Hello"]);
        assert_eq!(timeline.seq(), Seq(2));
    }

    #[test]
    fn finishing_counts_once() {
        let mut timeline = Timeline::new(0);
        assert_eq!(
            timeline.apply(upsert(message("a", "", ItemStatus::Running))),
            Some(false)
        );
        assert_eq!(
            timeline.apply(upsert(message("a", "done", ItemStatus::Completed))),
            Some(true)
        );
        assert_eq!(
            timeline.apply(upsert(message("a", "done", ItemStatus::Completed))),
            Some(false),
            "a replayed line finishes nothing new"
        );
    }

    #[test]
    fn clients_catch_up_from_a_kept_seq_and_reset_otherwise() {
        let mut timeline = Timeline::new(1000);
        timeline.apply(upsert(message("a", "one", ItemStatus::Completed)));
        let seen = timeline.seq();
        timeline.apply(upsert(message("b", "two", ItemStatus::Completed)));

        let TimelineUpdate::Events { seq, events } = timeline.open(Some(seen), 10) else {
            panic!("expected events");
        };
        assert_eq!(seq, Seq(1002));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            timeline.open(Some(Seq(1002)), 10),
            TimelineUpdate::Events { events, .. } if events.is_empty()
        ));
        for stale in [Seq(999), Seq(1003), Seq(5)] {
            assert!(
                matches!(timeline.open(Some(stale), 10), TimelineUpdate::Reset { .. }),
                "{stale:?}"
            );
        }
        let TimelineUpdate::Reset {
            items, has_older, ..
        } = timeline.open(None, 1)
        else {
            panic!("expected a reset");
        };
        assert_eq!(texts(&items), ["two"]);
        assert!(has_older);
    }

    #[test]
    fn a_seq_too_old_for_the_kept_events_gets_a_reset() {
        let mut timeline = Timeline::new(0);
        for n in 0..KEEP_EVENTS + 10 {
            timeline.apply(upsert(message(&n.to_string(), "", ItemStatus::Completed)));
        }
        assert!(matches!(
            timeline.open(Some(Seq(5)), 10),
            TimelineUpdate::Reset { .. }
        ));
        let recent = Seq(timeline.seq().0 - 3);
        assert!(matches!(
            timeline.open(Some(recent), 10),
            TimelineUpdate::Events { events, .. } if events.len() == 3
        ));
    }

    #[test]
    fn older_pages_back_by_ordinal() {
        let mut timeline = Timeline::new(0);
        for n in 0..5 {
            timeline.apply(upsert(message(
                &n.to_string(),
                &n.to_string(),
                ItemStatus::Completed,
            )));
        }
        let (newest, has_older) = timeline.newest(2);
        assert_eq!(texts(&newest), ["3", "4"]);
        assert!(has_older);
        let (page, has_older) = timeline.older(newest[0].ordinal, 2);
        assert_eq!(texts(&page), ["1", "2"]);
        assert!(has_older);
        let (last, has_older) = timeline.older(page[0].ordinal, 2);
        assert_eq!(texts(&last), ["0"]);
        assert!(!has_older);
    }

    #[test]
    fn history_goes_before_live_items_and_live_wins() {
        let mut timeline = Timeline::new(0);
        timeline.apply(upsert(message("live", "now", ItemStatus::Running)));
        timeline.apply(upsert(message("both", "newer", ItemStatus::Completed)));
        let before = timeline.seq();
        timeline.replace_history(vec![
            message("old", "then", ItemStatus::Completed),
            message("both", "older", ItemStatus::Running),
        ]);
        assert_eq!(texts(&timeline.newest(10).0), ["then", "newer", "now"]);
        assert!(timeline.since(before).is_none(), "clients need a reset");

        timeline.replace_history(vec![message("older", "first", ItemStatus::Completed)]);
        assert_eq!(texts(&timeline.newest(10).0), ["first", "newer", "now"]);
        let ordinals: Vec<u64> = timeline.items().map(|item| item.ordinal).collect();
        assert!(ordinals.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ordinals[0] >= 5, "ordinals are never reused");
    }

    /// A `!` run with this ID that started at `at`.
    fn run(id: &str, at: u64) -> (TimelineItem, Timestamp) {
        let mut item = message(id, id, ItemStatus::Completed);
        item.started_at = Some(Timestamp(at));
        (item, Timestamp(at))
    }

    fn at(mut item: TimelineItem, started: u64) -> TimelineItem {
        item.started_at = Some(Timestamp(started));
        item
    }

    #[test]
    fn runs_stay_where_they_came_through_history_reads() {
        let history = || {
            vec![
                at(message("m1", "m1", ItemStatus::Completed), 10),
                at(message("m2", "m2", ItemStatus::Completed), 20),
                at(message("m3", "m3", ItemStatus::Completed), 40),
            ]
        };
        let mut timeline = Timeline::new(0);
        timeline.replace_history(history()[..2].to_vec());
        assert_eq!(timeline.last_id(), Some(ItemId("m2".into())));
        // Two runs after m2, one before anything, and one after an item
        // the history no longer has.
        let (first, first_at) = run("run-a", 25);
        timeline.anchor(first.id.clone(), Some(ItemId("m2".into())), first_at);
        timeline.apply(upsert(first));
        let (second, second_at) = run("run-b", 30);
        timeline.anchor(second.id.clone(), timeline.last_id(), second_at);
        timeline.apply(upsert(second));
        let (lost, lost_at) = run("run-c", 35);
        timeline.anchor(lost.id.clone(), Some(ItemId("gone".into())), lost_at);
        timeline.apply(upsert(lost));
        let (top, top_at) = run("run-0", 5);
        timeline.anchor(top.id.clone(), None, top_at);
        timeline.apply(upsert(top));
        timeline.replace_history(history());
        let order = ["run-0", "m1", "m2", "run-a", "run-b", "run-c", "m3"];
        assert_eq!(texts(&timeline.newest(10).0), order);

        // After a restart the kept runs come back in the same places.
        let mut again = Timeline::new(100);
        let kept: Vec<_> = timeline
            .items()
            .filter(|item| item.id.0.starts_with("run-"))
            .map(|item| {
                let (after, at) = timeline.anchors[&item.id].clone();
                (item.into_owned(), after, at)
            })
            .collect();
        again.add_runs(kept);
        again.replace_history(history());
        assert_eq!(texts(&again.newest(10).0), order);

        // A run that's removed is forgotten.
        again.apply(TimelineEvent::Remove {
            id: ItemId("run-a".into()),
        });
        assert!(!again.anchors.contains_key(&ItemId("run-a".into())));
    }

    #[test]
    fn frozen_items_stay_ahead_of_a_new_vendors_history() {
        let mut timeline = Timeline::new(0);
        timeline.replace_history(vec![message("claude:1", "hi", ItemStatus::Completed)]);
        timeline.apply(upsert(message("claude:2", "live", ItemStatus::Completed)));
        timeline.freeze();
        timeline.apply(upsert(message("codex:1", "new", ItemStatus::Completed)));
        // The new vendor's history has none of the earlier items, and an
        // earlier item it echoes doesn't move.
        timeline.replace_history(vec![
            message("codex:0", "read", ItemStatus::Completed),
            message("claude:2", "echo", ItemStatus::Completed),
        ]);
        assert_eq!(texts(&timeline.newest(10).0), ["hi", "live", "read", "new"]);
        assert_eq!(texts(&timeline.earlier()), ["hi", "live"]);

        // After a restart the kept items come back first, and a history
        // read goes after them.
        let mut again = Timeline::new(100);
        again.thaw(timeline.earlier());
        again.replace_history(vec![message("codex:0", "read", ItemStatus::Completed)]);
        assert_eq!(texts(&again.newest(10).0), ["hi", "live", "read"]);
        again.replace_history(Vec::new());
        assert_eq!(texts(&again.newest(10).0), ["hi", "live"]);
    }

    #[test]
    fn a_live_agent_pill_from_history_runs_on_this_device() {
        use maxhq_protocol::context::InlineRef;
        use maxhq_protocol::model::Vendor;
        use maxhq_protocol::timeline::Delivery;

        let pill = |device: DeviceId| InlineRef::Agent {
            agent: AgentId::derive("docs"),
            title: "docs".into(),
            vendor: Vendor::Claude,
            device,
            excerpt: None,
        };
        let sent = |id: &str, device: DeviceId| TimelineItem {
            body: ItemBody::UserMessage {
                text: "see \u{FFFC}".into(),
                attachments: Vec::new(),
                refs: vec![pill(device)],
                delivery: Delivery::Sent,
            },
            ..message(id, "", ItemStatus::Completed)
        };
        let refs =
            |timeline: &Timeline, id: &str| match &timeline.get(&ItemId(id.into())).unwrap().body {
                ItemBody::UserMessage { refs, .. } => refs.clone(),
                other => panic!("{other:?}"),
            };
        let here = DeviceId::derive("host-a");
        let there = DeviceId::derive("host-b");
        let mut timeline = Timeline::new(0).on(here);
        timeline.replace_history(vec![sent("old", context::HERE)]);
        timeline.apply(upsert(sent("read", context::HERE)));
        timeline.apply(upsert(sent("live", there)));
        assert_eq!(refs(&timeline, "old"), [pill(here)]);
        assert_eq!(refs(&timeline, "read"), [pill(here)]);
        assert_eq!(
            refs(&timeline, "live"),
            [pill(there)],
            "a known device stays"
        );
    }

    /// `count` finished messages, each with its number as ID and text.
    fn numbered(count: usize) -> Vec<TimelineItem> {
        (0..count)
            .map(|n| message(&n.to_string(), &n.to_string(), ItemStatus::Completed))
            .collect()
    }

    fn numbers(count: usize) -> Vec<String> {
        (0..count).map(|n| n.to_string()).collect()
    }

    /// A timeline whose older items go under `dir`.
    fn spilling(dir: &crate::testing::TempDir) -> Timeline {
        Timeline::new(0).spilling(dir.path().join("timelines"))
    }

    #[test]
    fn older_items_go_to_disk_and_still_read_back() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir);
        for item in numbered(1000) {
            timeline.apply(upsert(item));
        }
        let kept: Vec<u64> = timeline.kept().map(|item| item.ordinal).collect();
        assert!(
            kept.len() < Limits::default().items * 2,
            "{} in memory",
            kept.len()
        );
        assert_eq!(kept, (1000 - kept.len() as u64..1000).collect::<Vec<_>>());

        let all: Vec<TimelineItem> = timeline.items().map(Cow::into_owned).collect();
        assert_eq!(texts(&all), numbers(1000));
        assert!(all
            .iter()
            .enumerate()
            .all(|(n, item)| item.ordinal == n as u64));
        let (newest, has_older) = timeline.newest(200);
        assert_eq!(texts(&newest), numbers(1000)[800..]);
        assert!(has_older);
        let (page, _) = timeline.older(newest[0].ordinal, 200);
        assert_eq!(texts(&page), numbers(1000)[600..800]);
        assert_eq!(
            texts(&[timeline.get(&ItemId("3".into())).unwrap().into_owned()]),
            ["3"]
        );
        let stored: Vec<TimelineItem> = timeline
            .stored()
            .map(|stored| stored.into_owned().load().unwrap())
            .collect();
        assert_eq!(texts(&stored), numbers(1000));
        assert!(stored
            .iter()
            .enumerate()
            .all(|(n, item)| item.ordinal == n as u64));
        assert!(matches!(
            timeline.stored().next(),
            Some(Stored::Spilled(0, _))
        ));
        let files = std::fs::read_dir(dir.path().join("timelines"))
            .unwrap()
            .count();
        assert_eq!(files, 0, "the files have no names");
    }

    #[test]
    fn items_that_may_change_stay_in_memory_and_a_changed_one_comes_back() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir);
        timeline.apply(upsert(message("running", "", ItemStatus::Running)));
        for item in numbered(1000) {
            timeline.apply(upsert(item));
        }
        let first = timeline.kept().next().unwrap();
        assert_eq!(first.id, ItemId("running".into()));
        assert!(!timeline.kept().any(|item| item.id == ItemId("5".into())));

        let more = TimelineEvent::Append {
            id: ItemId("5".into()),
            field: TextField::Text,
            text: "!".into(),
        };
        assert_eq!(timeline.apply(more), Some(false));
        let five = timeline.kept().find(|item| item.id == ItemId("5".into()));
        assert_eq!(texts(&[five.unwrap().clone()]), ["5!"]);
        timeline.apply(upsert(message("7", "seven", ItemStatus::Completed)));
        assert_eq!(
            texts(&[timeline.get(&ItemId("7".into())).unwrap().into_owned()]),
            ["seven"]
        );
        assert_eq!(timeline.items().count(), 1001);
    }

    #[test]
    fn history_replaces_items_on_disk_and_keeps_live_ones() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir);
        timeline.replace_history(numbered(1000));
        assert!(timeline.kept().count() < Limits::default().items * 2);
        for n in 0..500 {
            timeline.apply(upsert(message(
                &format!("live{n}"),
                "live",
                ItemStatus::Completed,
            )));
        }
        timeline.apply(upsert(message("10", "ten", ItemStatus::Completed)));
        timeline.replace_history(numbered(1200));
        let all: Vec<TimelineItem> = timeline.items().map(Cow::into_owned).collect();
        assert_eq!(all.len(), 1700);
        assert_eq!(texts(&all[..10]), numbers(10));
        assert_eq!(texts(&all[10..11]), ["ten"], "live wins");
        assert_eq!(texts(&all[11..1200]), numbers(1200)[11..]);
        assert!(texts(&all[1200..]).iter().all(|text| text == "live"));
        assert!(all.windows(2).all(|pair| pair[0].ordinal < pair[1].ordinal));
        assert!(timeline.kept().count() < Limits::default().items * 2);
    }

    #[test]
    fn the_oldest_items_go_past_the_disk_cap() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir);
        timeline.limit(Limits {
            disk: Some(20 << 20),
            ..Limits::default()
        });
        let text = "x".repeat(100 << 10);
        for n in 0..600 {
            timeline.apply(upsert(message(
                &n.to_string(),
                &text,
                ItemStatus::Completed,
            )));
        }
        let ordinals: Vec<u64> = timeline.items().map(|item| item.ordinal).collect();
        let oldest = ordinals[0];
        assert!(oldest > 0 && oldest < 300, "the oldest left is {oldest}");
        assert_eq!(ordinals, (oldest..600).collect::<Vec<_>>());
        assert!(!timeline.contains(&ItemId("0".into())));
        assert!(timeline.get(&ItemId("0".into())).is_none());
        assert_eq!(timeline.newest(200).0.len(), 200);
    }

    /// Applies `count` finished messages of 100 KiB, numbered from `from`,
    /// and returns the IDs each removal named, in order.
    fn add_big(timeline: &mut Timeline, from: usize, count: usize) -> Vec<ItemId> {
        let text = "x".repeat(100 << 10);
        let mut removed = Vec::new();
        for n in from..from + count {
            let before = timeline.seq();
            timeline.apply(upsert(message(
                &n.to_string(),
                &text,
                ItemStatus::Completed,
            )));
            for event in timeline.since(before).expect("the events are kept") {
                if let TimelineEvent::Remove { id } = event {
                    removed.push(id);
                }
            }
        }
        removed
    }

    #[test]
    fn items_past_the_disk_cap_go_out_as_removals() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir).limited(Limits {
            disk: Some(20 << 20),
            ..Limits::default()
        });
        let removed = add_big(&mut timeline, 0, 600);
        let oldest = timeline.items().next().unwrap().ordinal;
        let dropped: Vec<ItemId> = (0..oldest).map(|n| ItemId(n.to_string())).collect();
        assert!(!dropped.is_empty());
        assert_eq!(removed, dropped, "one removal for each item dropped");
        assert!(timeline.items().all(|item| !removed.contains(&item.id)));
    }

    #[test]
    fn a_new_history_limit_takes_effect_at_the_next_spill() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir).limited(Limits {
            disk: None,
            ..Limits::default()
        });
        assert!(add_big(&mut timeline, 0, 600).is_empty());
        assert_eq!(timeline.items().count(), 600, "no limit keeps every item");

        timeline.limit(Limits {
            disk: Some(20 << 20),
            ..Limits::default()
        });
        let removed = add_big(&mut timeline, 600, 200);
        assert!(!removed.is_empty());
        let oldest = timeline.items().next().unwrap().ordinal;
        assert_eq!(removed.len() as u64, oldest, "the oldest go first");
        assert_eq!(timeline.items().count() + removed.len(), 800);

        timeline.limit(Limits {
            disk: None,
            ..Limits::default()
        });
        assert!(add_big(&mut timeline, 800, 300).is_empty());
    }

    #[test]
    fn a_new_items_limit_takes_effect_without_a_restart() {
        let dir = crate::testing::TempDir::new();
        let mut timeline = spilling(&dir);
        for item in numbered(1000) {
            timeline.apply(upsert(item));
        }
        assert!(timeline.kept().count() >= Limits::default().items);

        let few = Limits {
            items: 64,
            ..Limits::default()
        };
        timeline.limit(few);
        let more = (1000..1100).map(|n| message(&n.to_string(), "", ItemStatus::Completed));
        more.for_each(|item| {
            timeline.apply(upsert(item));
        });
        assert!(
            timeline.kept().count() < 64 * 2,
            "{} in memory",
            timeline.kept().count()
        );
        assert_eq!(timeline.items().count(), 1100);

        timeline.limit(Limits {
            items: 4096,
            ..Limits::default()
        });
        let more = (1100..3100).map(|n| message(&n.to_string(), "", ItemStatus::Completed));
        more.for_each(|item| {
            timeline.apply(upsert(item));
        });
        assert!(
            timeline.kept().count() >= 2000,
            "{} in memory",
            timeline.kept().count()
        );
        assert_eq!(timeline.items().count(), 3100);
    }

    #[test]
    fn events_keep_to_a_byte_cap_but_the_newest_stays() {
        let mut timeline = Timeline::new(0);
        let text = "x".repeat(1 << 20);
        for n in 0..8 {
            timeline.apply(upsert(message(
                &n.to_string(),
                &text,
                ItemStatus::Completed,
            )));
        }
        assert!(timeline.since(Seq(0)).is_none());
        let kept = timeline.since(Seq(timeline.floor)).unwrap();
        assert!(kept.len() < 4, "{} events", kept.len());
        assert!(timeline.event_bytes <= KEEP_EVENT_BYTES);

        timeline.apply(upsert(message(
            "big",
            &"y".repeat(5 << 20),
            ItemStatus::Completed,
        )));
        let last = timeline.since(Seq(timeline.seq().0 - 1)).unwrap();
        assert_eq!(last.len(), 1);
    }
}
