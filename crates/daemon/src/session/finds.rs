// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Find across an agent's timeline. The app asks on every keystroke and
//! again for each next or previous, so two caches keep it quick. What
//! each item shows is kept per agent with the timeline seq it's for, and
//! only the items that changed since are worked out again. The counts of
//! a query are kept by agent, query and seq, so stepping through matches
//! only selects again. Both are bounded, and the work runs off the loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use maxhq_core::timeline_find::{self, Searchable, MAX_QUERY};
use maxhq_protocol::ids::{AgentId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use maxhq_protocol::state::Seq;
use maxhq_protocol::timeline::{FindAt, ItemBody, ItemId, TimelineEvent};
use tracing::warn;

use super::lookups::{history, Located};
use super::timeline::{Spilled, Stored, Timeline};
use super::{refused, Input, Manager};

/// Why find has nothing to look for.
const EMPTY: &str = "Type something to find.";

/// Why find won't look for a query.
const TOO_LONG: &str = "Find looks for at most 200 characters.";

/// How many agents' shown items are kept.
const SHOWN_AGENTS: usize = 4;

/// The most an agent's shown items keep of text. A bigger timeline is
/// worked out again on each find.
const SHOWN_BYTES: usize = 64 << 20;

/// How many queries' counts are kept.
const COUNTS: usize = 8;

/// What each item of a timeline shows, by item.
type ShownItems = HashMap<ItemId, Arc<Searchable>>;

/// Each item's matches, oldest first.
type Counts = Arc<Vec<(ItemId, u32)>>;

/// What an agent's items show, as of a seq of its timeline.
struct Shown {
    agent: AgentId,
    seq: Seq,
    items: Arc<ShownItems>,
}

/// How often a query shows in each item of an agent's timeline, as of a
/// seq of it.
struct Counted {
    agent: AgentId,
    /// The query, folded.
    query: String,
    seq: Seq,
    counts: Counts,
}

/// The caches, least recently used first.
#[derive(Default)]
struct Caches {
    shown: VecDeque<Shown>,
    counted: VecDeque<Counted>,
}

/// The caches find keeps, shared with the work it runs off the loop.
#[derive(Clone, Default)]
pub(super) struct Finds(Arc<Mutex<Caches>>);

impl Finds {
    fn lock(&self) -> std::sync::MutexGuard<'_, Caches> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The counts kept for a query in an agent's timeline at `seq`.
    fn counts(&self, agent: AgentId, query: &str, seq: Seq) -> Option<Counts> {
        let mut caches = self.lock();
        let at = caches
            .counted
            .iter()
            .position(|kept| kept.agent == agent && kept.seq == seq && kept.query == query)?;
        let kept = caches.counted.remove(at)?;
        let counts = kept.counts.clone();
        caches.counted.push_back(kept);
        Some(counts)
    }

    /// What an agent's items show, as of the seq it was kept at.
    fn shown(&self, agent: AgentId) -> Option<(Seq, Arc<ShownItems>)> {
        let caches = self.lock();
        let kept = caches.shown.iter().find(|kept| kept.agent == agent)?;
        Some((kept.seq, kept.items.clone()))
    }

    /// Keeps what an agent's items show at `seq`, unless what's kept is
    /// newer. Counts kept for an older seq go.
    fn keep(
        &self,
        agent: AgentId,
        seq: Seq,
        items: Arc<ShownItems>,
        query: String,
        counts: Counts,
    ) {
        let mut caches = self.lock();
        let caches = &mut *caches;
        caches
            .counted
            .retain(|kept| kept.agent != agent || kept.seq >= seq);
        if caches.counted.len() >= COUNTS {
            caches.counted.pop_front();
        }
        caches.counted.push_back(Counted {
            agent,
            query,
            seq,
            counts,
        });
        if caches
            .shown
            .iter()
            .any(|kept| kept.agent == agent && kept.seq > seq)
        {
            return;
        }
        caches.shown.retain(|kept| kept.agent != agent);
        let bytes: usize = items.values().map(|shown| shown.bytes()).sum();
        if bytes > SHOWN_BYTES {
            return;
        }
        if caches.shown.len() >= SHOWN_AGENTS {
            caches.shown.pop_front();
        }
        caches.shown.push_back(Shown { agent, seq, items });
    }
}

/// An item to count in, with what it shows if that's kept, or its body
/// to work it out from.
enum ToCount {
    Kept(ItemId, Arc<Searchable>),
    Body(ItemId, ItemBody),
    /// An item on disk with its ordinal, read back off the loop.
    Spilled(u64, Spilled),
}

impl Manager {
    /// Answers `FindInTimeline`: counts the query in each item of the
    /// agent's timeline as its row shows it, and picks the match `at`
    /// names.
    pub(super) fn find_in_timeline(
        &mut self,
        conn: u64,
        key: IdempotencyKey,
        agent: AgentId,
        query: &str,
        at: FindAt,
    ) {
        let query = query.trim();
        if query.is_empty() {
            return self.found(conn, key, refused(EMPTY));
        }
        if timeline_find::utf16_len(query) > MAX_QUERY {
            return self.found(conn, key, refused(TOO_LONG));
        }
        let query = timeline_find::fold(query);
        let tx = self.tx.clone();
        let answer = move |outcome: ActionOutcome| {
            let _ = tx.send(Input::Found {
                conn,
                key,
                outcome: Box::new(outcome),
            });
        };
        let timeline = match self.timeline_at(agent) {
            Ok((_, Located::Kept(timeline))) => timeline,
            Ok((_, Located::History(source))) => {
                let (vendors, places) = (self.vendors.clone(), self.places.clone());
                tokio::spawn(async move {
                    let outcome = match history(vendors, places, source, agent).await {
                        Ok(items) => {
                            let items = items
                                .into_iter()
                                .map(|item| ToCount::Body(item.id, item.body))
                                .collect();
                            counted(items, query, at).await.0
                        }
                        Err(reason) => refused(reason),
                    };
                    answer(outcome);
                });
                return;
            }
            Err(reason) => return self.found(conn, key, refused(reason)),
        };
        let seq = timeline.seq();
        if let Some(counts) = self.finds.counts(agent, &query, seq) {
            let find = timeline_find::select(&counts, &at);
            return self.found(conn, key, ActionOutcome::TimelineFind { find });
        }
        let items = to_count(timeline, self.finds.shown(agent));
        let finds = self.finds.clone();
        tokio::spawn(async move {
            let (outcome, kept) = counted(items, query.clone(), at).await;
            if let Some((shown, counts)) = kept {
                finds.keep(agent, seq, shown, query, counts);
            }
            answer(outcome);
        });
    }

    /// How often each of `words`, already folded, shows in an agent's
    /// timeline as its rows show it, added up and worked out off the loop.
    /// An agent whose timeline can't be read has none.
    pub(super) fn matches_in(
        &self,
        agent: AgentId,
        words: Vec<String>,
    ) -> impl Future<Output = u32> + Send + 'static {
        let found = match self.timeline_at(agent) {
            Ok((_, Located::Kept(timeline))) => {
                Some(Ok(to_count(timeline, self.finds.shown(agent))))
            }
            Ok((_, Located::History(source))) => Some(Err(source)),
            Err(_) => None,
        };
        let (vendors, places) = (self.vendors.clone(), self.places.clone());
        async move {
            let items = match found {
                Some(Ok(items)) => items,
                Some(Err(source)) => match history(vendors, places, source, agent).await {
                    Ok(items) => items
                        .into_iter()
                        .map(|item| ToCount::Body(item.id, item.body))
                        .collect(),
                    Err(_) => return 0,
                },
                None => return 0,
            };
            let work = tokio::task::spawn_blocking(move || {
                items
                    .into_iter()
                    .filter_map(searchable)
                    .map(|(_, shown)| words.iter().map(|word| shown.count(word)).sum::<u32>())
                    .sum()
            });
            work.await.unwrap_or(0)
        }
    }
}

/// The items to count in, oldest first, with what each shows where it's
/// kept from `seq` and hasn't changed since, as the timeline's events tell.
fn to_count(timeline: &Timeline, kept: Option<(Seq, Arc<ShownItems>)>) -> Vec<ToCount> {
    let kept = kept.and_then(|(seq, shown)| {
        let changed: HashSet<ItemId> = timeline
            .since(seq)?
            .into_iter()
            .map(|event| match event {
                TimelineEvent::Upsert { item } => item.id,
                TimelineEvent::Append { id, .. } | TimelineEvent::Remove { id } => id,
            })
            .collect();
        // Items on disk have only their ordinals here.
        Some((shown, changed, timeline.ids()))
    });
    timeline
        .stored()
        .map(|stored| {
            let id = match &stored {
                Stored::Kept(item) => Some(&item.id),
                Stored::Spilled(ordinal, _) => kept
                    .as_ref()
                    .and_then(|(.., ids)| ids.get(ordinal).copied()),
            };
            let reused = kept.as_ref().zip(id).and_then(|((shown, changed, _), id)| {
                (!changed.contains(id)).then(|| Some((id.clone(), shown.get(id).cloned()?)))?
            });
            match (reused, stored) {
                (Some((id, shown)), _) => ToCount::Kept(id, shown),
                (None, Stored::Kept(item)) => ToCount::Body(item.id.clone(), item.body.clone()),
                (None, Stored::Spilled(ordinal, spilled)) => ToCount::Spilled(ordinal, spilled),
            }
        })
        .collect()
}

/// Counts `query`, already folded, in each item off the loop and picks
/// the match `at` names. Gives what the items show and the counts too,
/// to keep.
async fn counted(
    items: Vec<ToCount>,
    query: String,
    at: FindAt,
) -> (ActionOutcome, Option<(Arc<ShownItems>, Counts)>) {
    let work = tokio::task::spawn_blocking(move || {
        let mut shown = ShownItems::with_capacity(items.len());
        let mut counts = Vec::with_capacity(items.len());
        for (id, searchable) in items.into_iter().filter_map(searchable) {
            counts.push((id.clone(), searchable.count(&query)));
            shown.insert(id, searchable);
        }
        let find = timeline_find::select(&counts, &at);
        (find, Arc::new(shown), Arc::new(counts))
    });
    match work.await {
        Ok((find, shown, counts)) => (ActionOutcome::TimelineFind { find }, Some((shown, counts))),
        Err(err) => (refused(format!("The find stopped: {err}")), None),
    }
}

/// What an item to count in shows, worked out unless it's kept. An item
/// on disk that can't be read back has none.
fn searchable(item: ToCount) -> Option<(ItemId, Arc<Searchable>)> {
    match item {
        ToCount::Kept(id, searchable) => Some((id, searchable)),
        ToCount::Body(id, body) => Some((id, Arc::new(Searchable::of(&body)))),
        ToCount::Spilled(ordinal, spilled) => match spilled.load() {
            Ok(item) => Some((item.id, Arc::new(Searchable::of(&item.body)))),
            Err(err) => {
                warn!(%err, ordinal, "couldn't read a timeline item back to find in");
                None
            }
        },
    }
}
