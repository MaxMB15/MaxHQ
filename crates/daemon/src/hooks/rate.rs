// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Sliding-window counters per task: how many requests it took a minute,
//! how many runs a day, and how many log rows of a kind a minute. GitHub's
//! doorbell counts each repository's the same way. They live in memory,
//! so a restart starts them over.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use maxhq_protocol::ids::TaskId;
use maxhq_protocol::time::Timestamp;

/// What [`Counter::take`] says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// It counted.
    Allowed,
    /// It's over the limit, the first time since one counted.
    FirstRejected,
    /// It's over the limit again.
    Rejected,
}

/// Counts what each task, or other key, took within a span of time.
pub(crate) struct Counter<K = TaskId> {
    tasks: HashMap<K, Window>,
}

impl<K> Default for Counter<K> {
    fn default() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }
}

#[derive(Default)]
struct Window {
    taken: VecDeque<Timestamp>,
    rejecting: bool,
}

impl<K: Hash + Eq> Counter<K> {
    /// Counts one for `task` at `now` when it has taken fewer than `limit`
    /// in the `span_ms` before.
    pub fn take(&mut self, task: K, now: Timestamp, limit: u32, span_ms: u64) -> Slot {
        let window = self.tasks.entry(task).or_default();
        while window
            .taken
            .front()
            .is_some_and(|first| now.0.saturating_sub(first.0) >= span_ms)
        {
            window.taken.pop_front();
        }
        if window.taken.len() < limit as usize {
            window.taken.push_back(now);
            window.rejecting = false;
            Slot::Allowed
        } else if window.rejecting {
            Slot::Rejected
        } else {
            window.rejecting = true;
            Slot::FirstRejected
        }
    }

    /// Whether `task` has nothing left to take at `now`, without counting one.
    pub fn spent(&self, task: K, now: Timestamp, limit: u32, span_ms: u64) -> bool {
        self.tasks.get(&task).is_some_and(|window| {
            let live = window
                .taken
                .iter()
                .filter(|at| now.0.saturating_sub(at.0) < span_ms);
            live.count() >= limit as usize
        })
    }

    /// Forgets what `task` took.
    pub fn forget(&mut self, task: K) {
        self.tasks.remove(&task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_up_to_the_limit_within_the_span_and_says_the_first_rejection() {
        let task = TaskId::generate();
        let mut counter = Counter::default();
        assert_eq!(counter.take(task, Timestamp(0), 2, 1000), Slot::Allowed);
        assert_eq!(counter.take(task, Timestamp(10), 2, 1000), Slot::Allowed);
        assert_eq!(
            counter.take(task, Timestamp(20), 2, 1000),
            Slot::FirstRejected
        );
        assert_eq!(counter.take(task, Timestamp(30), 2, 1000), Slot::Rejected);
        assert_eq!(
            counter.take(TaskId::generate(), Timestamp(30), 2, 1000),
            Slot::Allowed
        );
        assert_eq!(counter.take(task, Timestamp(1000), 2, 1000), Slot::Allowed);
        assert_eq!(
            counter.take(task, Timestamp(1001), 2, 1000),
            Slot::FirstRejected
        );
        counter.forget(task);
        assert_eq!(counter.take(task, Timestamp(1002), 2, 1000), Slot::Allowed);
    }
}
