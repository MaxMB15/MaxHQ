// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The timeline channel. Each window gets one, which carries the updates
//! of every timeline the window has open. Windows that watch the same agent
//! share one subscription to its device, so the app keeps the newest state
//! of each open timeline and gives it to a window that opens it later. A
//! page of older items goes only to the window that asked for it.
//!
//! Updates go out in batches, at most one every [`FRAME`] for each window.
//! The first update after a quiet spell goes at once. While updates wait,
//! appends to the same field of the same item merge into one, and a reset
//! drops whatever its agent had waiting before it. One thread sends every
//! batch, so each window gets its batches in order.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

use maxhq_protocol::ids::{AgentId, LinkId};
use maxhq_protocol::state::Seq;
use maxhq_protocol::timeline::{
    ItemBody, ItemId, TextField, TimelineEvent, TimelineItem, TimelineUpdate,
};
use serde::Serialize;
use specta::Type;

pub use crate::pace::FRAME;
use crate::pace::{lock, Outbox};

/// How many items a reset or a page of older items holds.
pub const PAGE: u32 = 200;

/// One timeline update, as a window's channel carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct TimelineMessage {
    /// The agent whose timeline changed.
    pub agent: AgentId,
    /// What changed.
    pub update: TimelineUpdate,
    /// Set for events that just happened on the device, as opposed to a
    /// reset, a page, or what an open or a reconnect caught up on.
    pub live: bool,
}

/// Where a window's batches go: its channel in the desktop app, and a
/// recorder in tests. Timelines and workflow runs each have their own.
pub trait Sink<M = TimelineMessage>: Send + Sync + 'static {
    /// Tells channels apart. A window that reloads opens a new channel.
    fn id(&self) -> u32;

    /// Sends a batch, and says whether it went out.
    fn send(&self, batch: Vec<M>) -> bool;
}

/// A timeline that no window watches any more, which its link should close.
pub type Close = (LinkId, AgentId);

/// What a window opening a timeline asks of the links.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// The link to open the timeline on, when no window watched it before.
    pub open: Option<LinkId>,
    /// Timelines the window watched through the channel it had before it
    /// reloaded, which no window watches now. Close these before opening.
    pub close: Vec<Close>,
}

/// Every window's timeline channel, with the thread that sends their
/// batches. Clones share it, and the thread stops once the last one drops.
#[derive(Clone)]
pub struct Timelines(Arc<Owner>);

/// Stops the thread when the last [`Timelines`] drops.
struct Owner(Arc<Shared>);

/// What the thread shares with the callers.
#[derive(Default)]
struct Shared {
    hub: Mutex<Hub>,
    /// Wakes the thread when something new waits to go out.
    wake: Condvar,
}

impl Drop for Owner {
    fn drop(&mut self) {
        lock(&self.0.hub).stop = true;
        self.0.wake.notify_all();
    }
}

impl Timelines {
    /// Starts the thread that sends batches.
    pub fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let sender = Arc::clone(&shared);
        if let Err(error) = thread::Builder::new()
            .name("timelines".to_owned())
            .spawn(move || send_batches(&sender))
        {
            tracing::error!(%error, "couldn't start the timeline thread, so no timeline shows");
        }
        Self(Arc::new(Owner(shared)))
    }

    /// Has a window watch an agent's timeline, which comes through `link`
    /// if no window watches it yet.
    pub fn subscribe(
        &self,
        window: &str,
        sink: Arc<dyn Sink>,
        agent: AgentId,
        link: LinkId,
    ) -> Plan {
        let plan = self
            .hub()
            .subscribe(window, sink, agent, link, Instant::now());
        self.0 .0.wake.notify_one();
        plan
    }

    /// Takes one of a window's subscriptions to an agent's timeline back.
    /// Returns the timeline to close once no window watches it.
    pub fn unsubscribe(&self, window: &str, agent: AgentId) -> Option<Close> {
        self.hub().unsubscribe(window, agent)
    }

    /// Forgets a window that closed. Returns the timelines no window
    /// watches now.
    pub fn release(&self, window: &str) -> Vec<Close> {
        self.hub().release(window)
    }

    /// Passes an update a link reported to every window watching the
    /// agent.
    pub fn deliver(&self, link: LinkId, agent: AgentId, update: TimelineUpdate, live: bool) {
        if self
            .hub()
            .deliver(link, agent, update, live, Instant::now())
        {
            self.0 .0.wake.notify_one();
        }
    }

    /// The link an agent's timeline comes through, if a window watches it.
    #[cfg(test)]
    pub fn link_of(&self, agent: AgentId) -> Option<LinkId> {
        self.hub().agents.get(&agent).map(|watched| watched.link)
    }

    /// Every timeline a window watches, with the link it comes through.
    pub fn watched(&self) -> Vec<(AgentId, LinkId)> {
        let hub = self.hub();
        let watched = hub.agents.iter();
        watched
            .map(|(agent, watched)| (*agent, watched.link))
            .collect()
    }

    /// Notes that a window asks for a page of older items before `before`
    /// in a timeline it watches. Returns the link to ask and the ask's ID,
    /// or None when the window doesn't watch the timeline. The link's next
    /// page goes only to this window.
    pub fn ask_older(&self, window: &str, agent: AgentId, before: u64) -> Option<(LinkId, u64)> {
        self.hub().ask_older(window, agent, before)
    }

    /// Takes back an ask whose link refused it, so no page comes for it.
    pub fn forget_ask(&self, agent: AgentId, ask: u64) {
        self.hub().forget_ask(agent, ask);
    }

    /// Has an agent's timeline come through `to` instead of `from`, if it
    /// comes through `from` now. Returns the version the app holds, so `to`
    /// can pick up after it, or None when the timeline isn't on `from`.
    /// Pages asked for on `from` never come, so each window that asked
    /// gets an empty one and can ask again.
    pub fn relink(&self, agent: AgentId, from: LinkId, to: LinkId) -> Option<Option<Seq>> {
        let held = self.hub().relink(agent, from, to, Instant::now());
        self.0 .0.wake.notify_one();
        held
    }

    /// Takes the timelines on `link` that no window watches since a
    /// window's channel stopped taking batches.
    pub fn orphans(&self, link: LinkId) -> Vec<AgentId> {
        let mut hub = self.hub();
        let (theirs, rest) = std::mem::take(&mut hub.orphans)
            .into_iter()
            .partition(|(on, _)| *on == link);
        hub.orphans = rest;
        theirs.into_iter().map(|(_, agent)| agent).collect()
    }

    fn hub(&self) -> MutexGuard<'_, Hub> {
        lock(&self.0 .0.hub)
    }
}

/// Sends each window's batch once it's due, until the hub stops.
fn send_batches(shared: &Shared) {
    let mut hub = lock(&shared.hub);
    loop {
        if hub.stop {
            return;
        }
        let now = Instant::now();
        let due = hub.take_due(now);
        if !due.is_empty() {
            // Sending can take a while for a large reset, so it happens
            // outside the lock. Only this thread sends, which keeps each
            // window's batches in order.
            drop(hub);
            let failed: Vec<(String, u32)> = due
                .into_iter()
                .filter(|out| !out.sink.send(out.batch.clone()))
                .map(|out| (out.window, out.sink.id()))
                .collect();
            hub = lock(&shared.hub);
            for (window, sink) in failed {
                tracing::debug!(%window, "a window's timeline channel stopped taking batches");
                hub.failed(&window, sink);
            }
            continue;
        }
        hub = match hub.next_due() {
            Some(at) => {
                let wait = at.saturating_duration_since(now);
                shared
                    .wake
                    .wait_timeout(hub, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .wake
                .wait(hub)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/// Which windows watch which timelines, and what waits to go to each. It
/// takes the time as an argument, so tests drive it with made-up times.
#[derive(Default)]
struct Hub {
    windows: BTreeMap<String, Window>,
    agents: BTreeMap<AgentId, Watched>,
    /// Timelines no window watches since a window's channel failed, until
    /// their link closes them.
    orphans: Vec<Close>,
    /// Counts the pages of older items asked for, to tell them apart.
    asked: u64,
    /// Tells the thread to end.
    stop: bool,
}

/// One window's channel.
struct Window {
    sink: Arc<dyn Sink>,
    /// How many times the window opened each agent's timeline, less the
    /// times it closed it.
    agents: BTreeMap<AgentId, usize>,
    batch: Batcher,
}

/// One timeline that at least one window watches.
struct Watched {
    /// The link the timeline comes through.
    link: LinkId,
    /// The newest state, once the first reset came.
    cache: Option<Cache>,
    /// The windows watching it, by label.
    windows: BTreeSet<String>,
    /// Pages of older items asked for and not yet come, in the order the
    /// link sends them.
    asks: VecDeque<Ask>,
}

/// One page of older items a window asked for.
struct Ask {
    id: u64,
    /// The window's label.
    window: String,
    /// The window's channel when it asked. A page for a channel that's gone
    /// goes nowhere.
    sink: u32,
    /// The ordinal the page ends before.
    before: u64,
    /// Set once a reset replaced what the window asked from, so the page no
    /// longer fits.
    stale: bool,
}

/// One batch on its way out.
struct Outgoing {
    window: String,
    sink: Arc<dyn Sink>,
    batch: Vec<TimelineMessage>,
}

impl Hub {
    /// Has a window watch an agent's timeline. A window that joins after
    /// the first reset gets a reset from the cache.
    fn subscribe(
        &mut self,
        label: &str,
        sink: Arc<dyn Sink>,
        agent: AgentId,
        link: LinkId,
        now: Instant,
    ) -> Plan {
        let mut plan = Plan::default();
        // A window that reloaded without letting go has a new channel, and
        // what it watched before is gone.
        if self
            .windows
            .get(label)
            .is_some_and(|window| window.sink.id() != sink.id())
        {
            plan.close = self.release(label);
        }
        let window = self
            .windows
            .entry(label.to_owned())
            .or_insert_with(|| Window {
                sink,
                agents: BTreeMap::new(),
                batch: Batcher::default(),
            });
        *window.agents.entry(agent).or_default() += 1;
        let watched = self.agents.entry(agent).or_insert_with(|| {
            plan.open = Some(link);
            Watched {
                link,
                cache: None,
                windows: BTreeSet::new(),
                asks: VecDeque::new(),
            }
        });
        watched.windows.insert(label.to_owned());
        if let Some(cache) = &watched.cache {
            window.batch.push(agent, cache.reset(), false, now);
        }
        plan
    }

    /// Takes one of a window's subscriptions back.
    fn unsubscribe(&mut self, label: &str, agent: AgentId) -> Option<Close> {
        let window = self.windows.get_mut(label)?;
        let count = window.agents.get_mut(&agent)?;
        *count -= 1;
        if *count > 0 {
            return None;
        }
        window.agents.remove(&agent);
        window.batch.forget(agent);
        self.leave(label, agent)
    }

    /// Forgets a window and everything it watched.
    fn release(&mut self, label: &str) -> Vec<Close> {
        let Some(window) = self.windows.remove(label) else {
            return Vec::new();
        };
        window
            .agents
            .into_keys()
            .filter_map(|agent| self.leave(label, agent))
            .collect()
    }

    /// Takes a window off an agent's timeline, and returns the timeline to
    /// close if no window watches it now.
    fn leave(&mut self, label: &str, agent: AgentId) -> Option<Close> {
        let watched = self.agents.get_mut(&agent)?;
        watched.windows.remove(label);
        if !watched.windows.is_empty() {
            return None;
        }
        let link = watched.link;
        self.agents.remove(&agent);
        Some((link, agent))
    }

    /// Queues an update for every window watching the agent, or a page of
    /// older items for the window that asked for it, and says whether it
    /// queued anything. Updates from a link the timeline doesn't come
    /// through, and repeats, go nowhere.
    fn deliver(
        &mut self,
        link: LinkId,
        agent: AgentId,
        update: TimelineUpdate,
        live: bool,
        now: Instant,
    ) -> bool {
        let Some(watched) = self.agents.get_mut(&agent) else {
            return false;
        };
        if watched.link != link {
            return false;
        }
        if let TimelineUpdate::Older { items, has_older } = update {
            // The link answers pages in the order they were asked for.
            let Some(ask) = watched.asks.pop_front() else {
                return false;
            };
            if ask.stale {
                return false;
            }
            watched.fold(ask.before, &items, has_older);
            let Some(window) = asker(&mut self.windows, &ask, agent) else {
                return false;
            };
            window.batch.push(
                agent,
                TimelineUpdate::Older { items, has_older },
                false,
                now,
            );
            return true;
        }
        if !watched.note(&update) {
            return false;
        }
        for label in &watched.windows {
            if let Some(window) = self.windows.get_mut(label) {
                window.batch.push(agent, update.clone(), live, now);
            }
        }
        true
    }

    /// Notes a page of older items a window asks for, if it watches the
    /// timeline.
    fn ask_older(&mut self, label: &str, agent: AgentId, before: u64) -> Option<(LinkId, u64)> {
        let window = self.windows.get(label)?;
        if !window.agents.contains_key(&agent) {
            return None;
        }
        let watched = self.agents.get_mut(&agent)?;
        self.asked += 1;
        watched.asks.push_back(Ask {
            id: self.asked,
            window: label.to_owned(),
            sink: window.sink.id(),
            before,
            stale: false,
        });
        Some((watched.link, self.asked))
    }

    /// Takes back an ask no page will come for.
    fn forget_ask(&mut self, agent: AgentId, id: u64) {
        if let Some(watched) = self.agents.get_mut(&agent) {
            watched.asks.retain(|ask| ask.id != id);
        }
    }

    /// Moves a timeline from one link to another, and answers the pages
    /// asked for on the old one with empty pages.
    fn relink(
        &mut self,
        agent: AgentId,
        from: LinkId,
        to: LinkId,
        now: Instant,
    ) -> Option<Option<Seq>> {
        let watched = self.agents.get_mut(&agent)?;
        if watched.link != from {
            return None;
        }
        watched.link = to;
        for ask in std::mem::take(&mut watched.asks) {
            if ask.stale {
                continue;
            }
            if let Some(window) = asker(&mut self.windows, &ask, agent) {
                let empty = TimelineUpdate::Older {
                    items: Vec::new(),
                    has_older: true,
                };
                window.batch.push(agent, empty, false, now);
            }
        }
        Some(watched.cache.as_ref().map(|cache| cache.seq))
    }

    /// Takes every batch that's due by `now`.
    fn take_due(&mut self, now: Instant) -> Vec<Outgoing> {
        self.windows
            .iter_mut()
            .filter(|(_, window)| window.batch.due().is_some_and(|due| due <= now))
            .map(|(label, window)| Outgoing {
                window: label.clone(),
                sink: Arc::clone(&window.sink),
                batch: window.batch.take(now),
            })
            .collect()
    }

    /// When the next batch is due, if anything waits.
    fn next_due(&self) -> Option<Instant> {
        self.windows
            .values()
            .filter_map(|window| window.batch.due())
            .min()
    }

    /// Forgets a window whose channel stopped taking batches, unless it
    /// has opened a new channel since.
    fn failed(&mut self, label: &str, sink: u32) {
        if self
            .windows
            .get(label)
            .is_some_and(|window| window.sink.id() == sink)
        {
            let closes = self.release(label);
            self.orphans.extend(closes);
        }
    }
}

/// The window that made an ask, if it still watches the timeline through
/// the channel it asked on.
fn asker<'a>(
    windows: &'a mut BTreeMap<String, Window>,
    ask: &Ask,
    agent: AgentId,
) -> Option<&'a mut Window> {
    windows
        .get_mut(&ask.window)
        .filter(|window| window.sink.id() == ask.sink && window.agents.contains_key(&agent))
}

impl Watched {
    /// Brings the cache up to date with an update, and says whether the
    /// windows should get it. Changes from before the cache's version are
    /// repeats, and changes before the first reset have nothing to apply
    /// to. A reset leaves the pages asked for before it nothing to fit.
    fn note(&mut self, update: &TimelineUpdate) -> bool {
        match update {
            TimelineUpdate::Reset {
                seq,
                items,
                has_older,
            } => {
                let mut cache = Cache {
                    seq: *seq,
                    items: items.clone(),
                    has_older: *has_older,
                };
                cache.trim();
                self.cache = Some(cache);
                self.asks.iter_mut().for_each(|ask| ask.stale = true);
                true
            }
            TimelineUpdate::Events { seq, events } => match &mut self.cache {
                Some(cache) if *seq > cache.seq => {
                    cache.apply(events);
                    cache.trim();
                    cache.seq = *seq;
                    true
                }
                _ => false,
            },
            // Pages go to the window that asked, through deliver.
            TimelineUpdate::Older { .. } => false,
        }
    }

    /// Adds a page of older items that ends before `before` to the cache.
    /// Whether anything is older than the cache's oldest item changes only
    /// with a page that ends at that item.
    fn fold(&mut self, before: u64, items: &[TimelineItem], has_older: bool) {
        let Some(cache) = &mut self.cache else {
            return;
        };
        let first = cache.items.first().map(|first| first.ordinal);
        // A window can hold more than a trimmed cache. Its page ends short of
        // the cache's oldest item, and adding it would leave a hole.
        if first.is_some_and(|first| before < first) {
            return;
        }
        let mut seen: HashSet<_> = cache.items.iter().map(|have| &have.id).collect();
        let new: Vec<_> = items
            .iter()
            .filter(|item| seen.insert(&item.id))
            .cloned()
            .collect();
        drop(seen);
        cache.items.extend(new);
        // Stable, so an item keeps its place among those of the same ordinal.
        cache.items.sort_by_key(|item| item.ordinal);
        if first.is_none_or(|first| before == first) {
            cache.has_older = has_older;
        }
        cache.trim();
    }
}

/// The most items kept for one open timeline. A device decides how many it
/// sends, and each window scrolls back through more than this by asking.
const MAX_CACHED: usize = 5_000;

/// The newest state of one open timeline.
#[derive(Debug)]
struct Cache {
    /// The version after the last update.
    seq: Seq,
    /// In ordinal order.
    items: Vec<TimelineItem>,
    has_older: bool,
}

impl Cache {
    /// Drops the oldest items past the most kept. A window that wants them
    /// asks for them again.
    fn trim(&mut self) {
        let over = self.items.len().saturating_sub(MAX_CACHED);
        if over > 0 {
            self.items.drain(..over);
            self.has_older = true;
        }
    }

    /// A reset to this state, for a window that joins late.
    fn reset(&self) -> TimelineUpdate {
        TimelineUpdate::Reset {
            seq: self.seq,
            items: self.items.clone(),
            has_older: self.has_older,
        }
    }

    /// Applies a batch of changes in order. Removes in a row, such as the
    /// oldest items past a device's cap, leave in one pass.
    fn apply(&mut self, events: &[TimelineEvent]) {
        let mut gone = HashSet::new();
        for event in events {
            if !matches!(event, TimelineEvent::Remove { .. }) {
                self.remove(&mut gone);
            }
            match event {
                TimelineEvent::Upsert { item } => self.put(item.clone()),
                TimelineEvent::Append { id, field, text } => {
                    let item = self.items.iter_mut().rev().find(|item| &item.id == id);
                    if let Some(target) = item.and_then(|item| text_of(&mut item.body, *field)) {
                        target.push_str(text);
                    }
                }
                TimelineEvent::Remove { id } => {
                    gone.insert(id);
                }
            }
        }
        self.remove(&mut gone);
    }

    /// Drops the items in `gone`, and empties it.
    fn remove(&mut self, gone: &mut HashSet<&ItemId>) {
        if !gone.is_empty() {
            self.items.retain(|item| !gone.contains(&item.id));
            gone.clear();
        }
    }

    /// Adds an item in ordinal order, or replaces the one with its ID.
    fn put(&mut self, item: TimelineItem) {
        // Most changes are to the newest items, so the search starts there.
        match self.items.iter().rposition(|have| have.id == item.id) {
            Some(at) if self.items[at].ordinal == item.ordinal => self.items[at] = item,
            found => {
                if let Some(at) = found {
                    self.items.remove(at);
                }
                let at = self
                    .items
                    .partition_point(|have| have.ordinal <= item.ordinal);
                self.items.insert(at, item);
            }
        }
    }
}

/// The text an append adds to, if the item has that field.
fn text_of(body: &mut ItemBody, field: TextField) -> Option<&mut String> {
    match (body, field) {
        (
            ItemBody::UserMessage { text, .. }
            | ItemBody::AssistantMessage { text, .. }
            | ItemBody::Reasoning { text, .. },
            TextField::Text,
        ) => Some(text),
        (ItemBody::Plan { markdown, .. }, TextField::Text) => Some(markdown),
        (ItemBody::Command { output, .. }, TextField::Output) => Some(output),
        _ => None,
    }
}

/// What waits to go to one window.
#[derive(Debug, Default)]
struct Batcher(Outbox<TimelineMessage>);

impl Batcher {
    /// Queues an update. A reset drops what its agent had waiting, and
    /// changes join the agent's last waiting changes, with appends to the
    /// same field of the same item merged. Changes that join waiting ones
    /// stay live only if both were.
    fn push(&mut self, agent: AgentId, update: TimelineUpdate, live: bool, now: Instant) {
        let queued = self.0.waiting(now);
        let update = match update {
            TimelineUpdate::Reset { .. } => {
                queued.retain(|message| message.agent != agent);
                update
            }
            TimelineUpdate::Events { seq, events } => {
                let last = queued
                    .iter_mut()
                    .rev()
                    .find(|message| message.agent == agent);
                if let Some(TimelineMessage {
                    update:
                        TimelineUpdate::Events {
                            seq: waiting_seq,
                            events: waiting,
                        },
                    live: waiting_live,
                    ..
                }) = last
                {
                    *waiting_seq = seq;
                    *waiting_live &= live;
                    events.into_iter().for_each(|event| merge(waiting, event));
                    return;
                }
                let mut merged = Vec::with_capacity(events.len());
                events
                    .into_iter()
                    .for_each(|event| merge(&mut merged, event));
                TimelineUpdate::Events {
                    seq,
                    events: merged,
                }
            }
            TimelineUpdate::Older { .. } => update,
        };
        queued.push(TimelineMessage {
            agent,
            update,
            live,
        });
    }

    /// Drops what an agent had waiting.
    fn forget(&mut self, agent: AgentId) {
        self.0.retain(|message| message.agent != agent);
    }

    /// When what waits should go out. None when nothing waits.
    fn due(&self) -> Option<Instant> {
        self.0.due()
    }

    /// Takes what waits, as the batch going out now.
    fn take(&mut self, now: Instant) -> Vec<TimelineMessage> {
        self.0.take(now)
    }
}

/// Adds an event to the end of `events`, merging an append into an append
/// just before it to the same field of the same item.
fn merge(events: &mut Vec<TimelineEvent>, event: TimelineEvent) {
    if let (
        Some(TimelineEvent::Append {
            id: last_id,
            field: last_field,
            text: last_text,
        }),
        TimelineEvent::Append { id, field, text },
    ) = (events.last_mut(), &event)
        && last_id == id
        && last_field == field
    {
        last_text.push_str(text);
        return;
    }
    events.push(event);
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use maxhq_protocol::timeline::{ItemId, ItemStatus};

    /// A sink that records what it gets.
    pub struct Recorder<M = TimelineMessage> {
        id: u32,
        /// Set to make sends fail.
        pub broken: AtomicBool,
        pub batches: Mutex<Vec<Vec<M>>>,
    }

    impl<M: Clone + Send + 'static> Recorder<M> {
        pub fn new(id: u32) -> Arc<Self> {
            Arc::new(Self {
                id,
                broken: AtomicBool::new(false),
                batches: Mutex::new(Vec::new()),
            })
        }

        /// Every message so far, in order.
        pub fn messages(&self) -> Vec<M> {
            self.batches.lock().unwrap().concat()
        }
    }

    impl<M: Clone + Send + 'static> Sink<M> for Recorder<M> {
        fn id(&self) -> u32 {
            self.id
        }

        fn send(&self, batch: Vec<M>) -> bool {
            if self.broken.load(Ordering::SeqCst) {
                return false;
            }
            self.batches.lock().unwrap().push(batch);
            true
        }
    }

    fn agent(name: &str) -> AgentId {
        AgentId::derive(name)
    }

    fn link(name: &str) -> LinkId {
        LinkId::derive(name)
    }

    fn reply(id: &str, ordinal: u64, text: &str) -> TimelineItem {
        TimelineItem {
            id: ItemId(id.into()),
            agent: agent("a"),
            turn: None,
            parent: None,
            ordinal,
            status: ItemStatus::Running,
            started_at: None,
            completed_at: None,
            body: ItemBody::AssistantMessage {
                text: text.into(),
                streaming: true,
            },
        }
    }

    fn append(id: &str, field: TextField, text: &str) -> TimelineEvent {
        TimelineEvent::Append {
            id: ItemId(id.into()),
            field,
            text: text.into(),
        }
    }

    fn events(seq: u64, events: Vec<TimelineEvent>) -> TimelineUpdate {
        TimelineUpdate::Events {
            seq: Seq(seq),
            events,
        }
    }

    fn reset(seq: u64, items: Vec<TimelineItem>) -> TimelineUpdate {
        TimelineUpdate::Reset {
            seq: Seq(seq),
            items,
            has_older: false,
        }
    }

    fn message(name: &str, update: TimelineUpdate) -> TimelineMessage {
        TimelineMessage {
            agent: agent(name),
            update,
            live: false,
        }
    }

    #[test]
    fn live_changes_stay_live_unless_they_join_caught_up_ones() {
        let start = Instant::now();
        let mut batch = Batcher::default();
        batch.push(agent("a"), events(1, vec![]), true, start);
        batch.push(agent("a"), events(2, vec![]), true, start);
        batch.push(agent("b"), events(1, vec![]), false, start);
        batch.push(agent("b"), events(2, vec![]), true, start);
        let lives: Vec<bool> = batch.take(start).iter().map(|m| m.live).collect();
        assert_eq!(lives, [true, false]);

        // A late window's reset from the cache is never live.
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        hub.deliver(nl, agent("a"), reset(1, vec![]), false, start);
        assert!(hub.deliver(nl, agent("a"), events(2, vec![]), true, start));
        hub.subscribe("late", Recorder::new(2), agent("a"), nl, start);
        let due = hub.take_due(start);
        let late = due.iter().find(|out| out.window == "late").unwrap();
        assert!(late.batch.iter().all(|message| !message.live));
        let main = due.iter().find(|out| out.window == "main").unwrap();
        assert!(main.batch.iter().any(|message| message.live));
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_batch_goes_at_once_after_a_quiet_spell_and_a_frame_later_otherwise() {
        let start = Instant::now();
        let mut batch = Batcher::default();
        assert_eq!(batch.due(), None, "nothing waits");

        batch.push(agent("a"), events(1, vec![]), false, start);
        assert_eq!(batch.due(), Some(start), "the first update goes at once");
        assert_eq!(batch.take(start).len(), 1);

        batch.push(agent("a"), reset(2, vec![]), false, start + ms(5));
        batch.push(agent("b"), reset(1, vec![]), false, start + ms(10));
        assert_eq!(
            batch.due(),
            Some(start + FRAME),
            "a frame after the last batch"
        );
        assert_eq!(batch.take(start + FRAME).len(), 2);

        let quiet = start + ms(100);
        batch.push(agent("a"), events(3, vec![]), false, quiet);
        assert_eq!(batch.due(), Some(quiet), "a quiet spell ended");
    }

    #[test]
    fn appends_to_one_field_merge_while_they_wait() {
        let start = Instant::now();
        let mut batch = Batcher::default();
        batch.push(
            agent("a"),
            events(1, vec![append("r", TextField::Text, "he")]),
            false,
            start,
        );
        batch.push(
            agent("b"),
            events(1, vec![append("r", TextField::Text, "other")]),
            false,
            start,
        );
        batch.push(
            agent("a"),
            events(
                2,
                vec![
                    append("r", TextField::Text, "l"),
                    append("r", TextField::Text, "lo"),
                    append("r", TextField::Output, "out"),
                    append("s", TextField::Text, "x"),
                ],
            ),
            false,
            start,
        );
        let upsert = TimelineEvent::Upsert {
            item: reply("s", 2, "x"),
        };
        batch.push(
            agent("a"),
            events(3, vec![upsert.clone(), append("s", TextField::Text, "y")]),
            false,
            start,
        );
        assert_eq!(
            batch.take(start),
            [
                message(
                    "a",
                    events(
                        3,
                        vec![
                            append("r", TextField::Text, "hello"),
                            append("r", TextField::Output, "out"),
                            append("s", TextField::Text, "x"),
                            upsert,
                            append("s", TextField::Text, "y"),
                        ]
                    )
                ),
                message("b", events(1, vec![append("r", TextField::Text, "other")])),
            ]
        );
    }

    #[test]
    fn a_reset_drops_what_its_agent_had_waiting() {
        let start = Instant::now();
        let mut batch = Batcher::default();
        batch.push(agent("a"), events(1, vec![]), false, start);
        batch.push(agent("b"), events(1, vec![]), false, start);
        batch.push(
            agent("a"),
            TimelineUpdate::Older {
                items: vec![],
                has_older: false,
            },
            false,
            start,
        );
        batch.push(agent("a"), reset(5, vec![]), false, start);
        assert_eq!(
            batch.take(start),
            [
                message("b", events(1, vec![])),
                message("a", reset(5, vec![]))
            ]
        );
    }

    #[test]
    fn windows_share_one_subscription_and_a_late_one_starts_from_the_cache() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let (main, other, late) = (Recorder::new(1), Recorder::new(2), Recorder::new(3));
        let nl = link("nl");

        let plan = hub.subscribe("main", main.clone(), agent("a"), nl, start);
        assert_eq!(
            plan,
            Plan {
                open: Some(nl),
                close: vec![]
            }
        );
        let plan = hub.subscribe("other", other.clone(), agent("a"), link("mac"), start);
        assert_eq!(
            plan.open, None,
            "the first window's subscription serves both"
        );

        assert!(
            !hub.deliver(nl, agent("a"), events(1, vec![]), false, start),
            "changes before the first reset have nothing to apply to"
        );
        assert!(hub.deliver(
            nl,
            agent("a"),
            reset(4, vec![reply("r", 1, "hi")]),
            false,
            start
        ));
        assert!(!hub.deliver(link("mac"), agent("a"), reset(9, vec![]), false, start));
        assert!(hub.deliver(
            nl,
            agent("a"),
            events(5, vec![append("r", TextField::Text, " there")]),
            false,
            start
        ));
        assert!(
            !hub.deliver(nl, agent("a"), events(5, vec![]), false, start),
            "a repeat"
        );
        let due = hub.take_due(start);
        assert_eq!(due.len(), 2);
        for out in &due {
            assert_eq!(
                out.batch,
                [
                    message("a", reset(4, vec![reply("r", 1, "hi")])),
                    message("a", events(5, vec![append("r", TextField::Text, " there")])),
                ]
            );
        }

        let older = TimelineUpdate::Older {
            items: vec![reply("q", 0, "before")],
            has_older: true,
        };
        assert!(
            !hub.deliver(nl, agent("a"), older.clone(), false, start),
            "no window asked for it"
        );
        assert_eq!(
            hub.ask_older("main", agent("a"), 1).map(|(on, _)| on),
            Some(nl)
        );
        assert!(hub.deliver(nl, agent("a"), older.clone(), false, start));
        hub.subscribe("late", late.clone(), agent("a"), nl, start + ms(1));
        let due = hub.take_due(start + FRAME);
        let batches: BTreeMap<&str, &[TimelineMessage]> = due
            .iter()
            .map(|out| (out.window.as_str(), out.batch.as_slice()))
            .collect();
        assert_eq!(
            batches,
            BTreeMap::from([
                ("main", [message("a", older)].as_slice()),
                (
                    "late",
                    [message(
                        "a",
                        TimelineUpdate::Reset {
                            seq: Seq(5),
                            items: vec![reply("q", 0, "before"), reply("r", 1, "hi there")],
                            has_older: true,
                        }
                    )]
                    .as_slice()
                ),
            ]),
            "the page goes to the window that asked, and the cache keeps it"
        );
    }

    fn page(items: Vec<TimelineItem>, has_older: bool) -> TimelineUpdate {
        TimelineUpdate::Older { items, has_older }
    }

    #[test]
    fn a_page_that_no_longer_fits_goes_nowhere() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        hub.subscribe("other", Recorder::new(2), agent("a"), nl, start);
        hub.subscribe("idle", Recorder::new(3), agent("b"), nl, start);
        hub.deliver(
            nl,
            agent("a"),
            reset(1, vec![reply("r", 5, "")]),
            false,
            start,
        );
        assert_eq!(
            hub.ask_older("idle", agent("a"), 5),
            None,
            "the window doesn't watch it"
        );

        hub.ask_older("main", agent("a"), 5).unwrap();
        hub.deliver(
            nl,
            agent("a"),
            reset(2, vec![reply("s", 9, "")]),
            false,
            start,
        );
        assert!(
            !hub.deliver(
                nl,
                agent("a"),
                page(vec![reply("q", 4, "")], false),
                false,
                start
            ),
            "a reset came after the ask"
        );

        let (_, ask) = hub.ask_older("main", agent("a"), 9).unwrap();
        hub.forget_ask(agent("a"), ask);
        assert!(
            !hub.deliver(nl, agent("a"), page(vec![], false), false, start),
            "the link refused the ask"
        );

        hub.ask_older("main", agent("a"), 9).unwrap();
        hub.subscribe("main", Recorder::new(7), agent("a"), nl, start);
        assert!(
            !hub.deliver(
                nl,
                agent("a"),
                page(vec![reply("q", 4, "")], false),
                false,
                start
            ),
            "the window reloaded since it asked"
        );
        hub.subscribe("late", Recorder::new(4), agent("a"), nl, start);
        let due = hub.take_due(start);
        let late = due.iter().find(|out| out.window == "late").unwrap();
        assert_eq!(
            late.batch,
            [message(
                "a",
                TimelineUpdate::Reset {
                    seq: Seq(2),
                    items: vec![reply("q", 4, ""), reply("s", 9, "")],
                    has_older: false,
                }
            )],
            "the cache still keeps the page"
        );
    }

    #[test]
    fn the_cache_keeps_the_newest_items_and_a_window_still_gets_older_ones() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        let many = |from: u64, to: u64| -> Vec<TimelineItem> {
            (from..to).map(|n| reply(&format!("r{n}"), n, "")).collect()
        };
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        let (oldest, newest) = (200, 200 + MAX_CACHED as u64);
        hub.deliver(
            nl,
            agent("a"),
            reset(1, many(oldest - 50, newest)),
            false,
            start,
        );
        let held = |hub: &Hub| {
            let cache = hub.agents[&agent("a")].cache.as_ref().unwrap();
            (cache.items.len(), cache.items[0].ordinal, cache.has_older)
        };
        assert_eq!(held(&hub), (MAX_CACHED, oldest, true), "the oldest go");

        // The window pages back from the cache's oldest item and from further
        // back still, and gets every page though the cache is full.
        hub.ask_older("main", agent("a"), oldest).unwrap();
        let next = page(many(oldest - 50, oldest), true);
        assert!(hub.deliver(nl, agent("a"), next, false, start));
        assert_eq!(held(&hub), (MAX_CACHED, oldest, true), "the newest stay");
        hub.ask_older("main", agent("a"), oldest - 50).unwrap();
        let last = page(many(oldest - 60, oldest - 50), false);
        assert!(hub.deliver(nl, agent("a"), last, false, start));
        assert_eq!(
            held(&hub),
            (MAX_CACHED, oldest, true),
            "a page past a gap adds no hole, and older items are still there to ask for"
        );
    }

    #[test]
    fn events_that_outgrow_the_cache_drop_its_oldest() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        let full = (0..MAX_CACHED as u64)
            .map(|n| reply(&format!("r{n}"), n, ""))
            .collect();
        hub.deliver(nl, agent("a"), reset(1, full), false, start);
        let events = TimelineUpdate::Events {
            seq: Seq(2),
            events: vec![TimelineEvent::Upsert {
                item: reply("new", MAX_CACHED as u64, ""),
            }],
        };
        hub.deliver(nl, agent("a"), events, false, start);
        let cache = hub.agents[&agent("a")].cache.as_ref().unwrap();
        assert_eq!(cache.items.len(), MAX_CACHED);
        assert_eq!(cache.items[0].ordinal, 1);
        assert!(cache.has_older);
    }

    #[test]
    fn a_burst_of_removes_keeps_the_rest_in_order() {
        let mut cache = Cache {
            seq: Seq(1),
            items: (0..10).map(|n| reply(&format!("r{n}"), n, "")).collect(),
            has_older: false,
        };
        let remove = |id: &str| TimelineEvent::Remove {
            id: ItemId(id.into()),
        };
        cache.apply(&[
            remove("r0"),
            remove("r1"),
            remove("r2"),
            TimelineEvent::Upsert {
                item: reply("r3", 20, ""),
            },
            remove("r4"),
            remove("unknown"),
            append("r4", TextField::Text, "lost"),
            TimelineEvent::Upsert {
                item: reply("r4", 4, "back"),
            },
        ]);
        let ids: Vec<_> = cache.items.iter().map(|item| item.id.0.as_str()).collect();
        assert_eq!(ids, ["r4", "r5", "r6", "r7", "r8", "r9", "r3"]);
        assert!(matches!(
            &cache.items[0].body,
            ItemBody::AssistantMessage { text, .. } if text == "back"
        ));
    }

    #[test]
    fn a_big_page_folds_in_order_with_its_repeats_gone() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        let at = 2_000;
        hub.deliver(
            nl,
            agent("a"),
            reset(1, vec![reply("last", at, "")]),
            false,
            start,
        );
        hub.ask_older("main", agent("a"), at).unwrap();
        let mut items: Vec<_> = (0..at).map(|n| reply(&format!("r{n}"), n, "")).collect();
        items.extend(items.clone());
        hub.deliver(nl, agent("a"), page(items, false), false, start);
        let cache = hub.agents[&agent("a")].cache.as_ref().unwrap();
        let ordinals: Vec<_> = cache.items.iter().map(|item| item.ordinal).collect();
        assert_eq!(ordinals, (0..=at).collect::<Vec<_>>());
        assert!(!cache.has_older, "the page reached the start");
    }

    #[test]
    fn a_page_inside_the_cache_leaves_what_it_says_is_older() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        hub.subscribe("other", Recorder::new(2), agent("a"), nl, start);
        let held = TimelineUpdate::Reset {
            seq: Seq(1),
            items: vec![reply("r", 5, "")],
            has_older: true,
        };
        hub.deliver(nl, agent("a"), held, false, start);
        hub.ask_older("main", agent("a"), 5).unwrap();
        hub.deliver(
            nl,
            agent("a"),
            page(vec![reply("q", 3, "")], true),
            false,
            start,
        );
        hub.ask_older("main", agent("a"), 3).unwrap();
        hub.deliver(
            nl,
            agent("a"),
            page(vec![reply("p", 1, "")], false),
            false,
            start,
        );
        // The other window asks from where its own items end, which the
        // cache already holds older items than.
        hub.ask_older("other", agent("a"), 5).unwrap();
        hub.deliver(
            nl,
            agent("a"),
            page(vec![reply("q", 3, "")], true),
            false,
            start,
        );
        let cache = hub.agents[&agent("a")].cache.as_ref().unwrap();
        assert!(!cache.has_older);
        assert_eq!(
            cache.items,
            [reply("p", 1, ""), reply("q", 3, ""), reply("r", 5, "")]
        );
    }

    #[test]
    fn a_moved_timeline_comes_through_its_new_link() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let (nl, lan) = (link("nl"), link("nl.lan"));
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        assert_eq!(
            hub.relink(agent("a"), nl, lan, start),
            Some(None),
            "nothing came yet"
        );
        assert_eq!(hub.relink(agent("a"), lan, nl, start), Some(None));
        hub.deliver(
            nl,
            agent("a"),
            reset(3, vec![reply("r", 1, "")]),
            false,
            start,
        );
        hub.take_due(start);
        hub.ask_older("main", agent("a"), 1).unwrap();

        assert_eq!(
            hub.relink(agent("a"), lan, nl, start),
            None,
            "it isn't on lan"
        );
        assert_eq!(hub.relink(agent("b"), nl, lan, start), None);
        assert_eq!(hub.relink(agent("a"), nl, lan, start), Some(Some(Seq(3))));
        assert!(!hub.deliver(nl, agent("a"), events(4, vec![]), false, start));
        assert!(
            !hub.deliver(lan, agent("a"), page(vec![], false), false, start),
            "the page asked for on nl got its answer"
        );
        assert!(
            !hub.deliver(lan, agent("a"), events(3, vec![]), false, start),
            "a repeat"
        );
        let next = events(4, vec![append("r", TextField::Text, "hi")]);
        assert!(hub.deliver(lan, agent("a"), next.clone(), false, start));
        let due = hub.take_due(start + FRAME);
        assert_eq!(
            due[0].batch,
            [message("a", page(vec![], true)), message("a", next)]
        );
    }

    #[test]
    fn a_timeline_closes_once_no_window_watches_it() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let (main, other) = (Recorder::new(1), Recorder::new(2));
        let nl = link("nl");
        hub.subscribe("main", main.clone(), agent("a"), nl, start);
        hub.subscribe("main", main.clone(), agent("a"), nl, start);
        hub.subscribe("other", other.clone(), agent("a"), nl, start);

        assert_eq!(hub.unsubscribe("main", agent("a")), None);
        assert_eq!(hub.unsubscribe("main", agent("a")), None);
        assert_eq!(hub.unsubscribe("other", agent("a")), Some((nl, agent("a"))));
        assert_eq!(hub.unsubscribe("other", agent("a")), None, "already closed");
        assert!(!hub.deliver(nl, agent("a"), reset(1, vec![]), false, start));
    }

    #[test]
    fn a_reloaded_window_lets_go_of_what_it_watched() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        hub.subscribe("main", Recorder::new(1), agent("b"), nl, start);

        let plan = hub.subscribe("main", Recorder::new(7), agent("b"), nl, start);
        assert_eq!(
            plan,
            Plan {
                open: Some(nl),
                close: vec![(nl, agent("a")), (nl, agent("b"))],
            }
        );
    }

    #[test]
    fn a_window_whose_channel_fails_is_forgotten() {
        let start = Instant::now();
        let mut hub = Hub::default();
        let nl = link("nl");
        hub.subscribe("main", Recorder::new(1), agent("a"), nl, start);
        hub.failed("main", 2);
        assert!(hub.orphans.is_empty(), "the window has a newer channel");
        hub.failed("main", 1);
        assert_eq!(hub.orphans, [(nl, agent("a"))]);
        assert!(hub.windows.is_empty());
    }

    #[test]
    fn the_thread_sends_batches_as_they_come_due() {
        let timelines = Timelines::start();
        let main = Recorder::new(1);
        let nl = link("nl");
        let plan = timelines.subscribe("main", main.clone(), agent("a"), nl);
        assert_eq!(plan.open, Some(nl));
        assert_eq!(timelines.link_of(agent("a")), Some(nl));

        timelines.deliver(nl, agent("a"), reset(1, vec![reply("r", 1, "")]), false);
        for n in 0..20 {
            let text = n.to_string();
            timelines.deliver(
                nl,
                agent("a"),
                events(n + 2, vec![append("r", TextField::Text, &text)]),
                false,
            );
        }
        let wanted: String = (0..20).map(|n| n.to_string()).collect();
        let started = Instant::now();
        loop {
            let messages = main.messages();
            let text: String = messages
                .iter()
                .flat_map(|message| match &message.update {
                    TimelineUpdate::Events { events, .. } => events.clone(),
                    _ => Vec::new(),
                })
                .map(|event| match event {
                    TimelineEvent::Append { text, .. } => text,
                    _ => String::new(),
                })
                .collect();
            if text == wanted {
                assert!(matches!(messages[0].update, TimelineUpdate::Reset { .. }));
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "got {text:?}");
            thread::sleep(ms(2));
        }

        main.broken.store(true, Ordering::SeqCst);
        timelines.deliver(nl, agent("a"), events(30, vec![]), false);
        let started = Instant::now();
        while timelines.link_of(agent("a")).is_some() {
            assert!(started.elapsed() < Duration::from_secs(5));
            thread::sleep(ms(2));
        }
        assert_eq!(timelines.orphans(nl), [agent("a")]);
        assert!(timelines.orphans(nl).is_empty());
    }
}
