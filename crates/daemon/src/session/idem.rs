// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Idempotency keys. `maxd` acts on a key once. A repeat while the action
//! runs waits for the same outcome, and a repeat afterwards gets it at once.

use std::collections::{HashMap, VecDeque};

use maxhq_protocol::attention::AnswerOutcome;
use maxhq_protocol::ids::IdempotencyKey;
use maxhq_protocol::message::{ActionOutcome, ServerMessage};

/// How many finished keys are remembered.
pub const KEEP_KEYS: usize = 1024;

/// A finished key's outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// What an action came to.
    Action(ActionOutcome),
    /// What an answer came to.
    Answer(AnswerOutcome),
}

impl Outcome {
    /// The message that tells a client.
    pub fn message(&self, key: IdempotencyKey) -> ServerMessage {
        match self {
            Self::Action(outcome) => ServerMessage::ActionResult {
                key,
                outcome: outcome.clone(),
            },
            Self::Answer(outcome) => ServerMessage::AnswerResult {
                idempotency_key: key,
                outcome: outcome.clone(),
            },
        }
    }
}

/// What a key has come to so far.
#[derive(Debug, PartialEq, Eq)]
pub enum Seen {
    /// Never seen, so act on it. The caller now waits for it.
    New,
    /// Still running. The caller now waits for it too.
    Running,
    /// Finished with this outcome.
    Done(Outcome),
}

/// The keys in flight and the last [`KEEP_KEYS`] finished.
pub struct Keys<W> {
    running: HashMap<IdempotencyKey, Vec<W>>,
    done: HashMap<IdempotencyKey, Outcome>,
    order: VecDeque<IdempotencyKey>,
}

impl<W> Keys<W> {
    /// No keys yet.
    pub fn new() -> Self {
        Self {
            running: HashMap::new(),
            done: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Looks a key up, and adds `waiter` to the ones told when it finishes
    /// unless it already has.
    pub fn begin(&mut self, key: IdempotencyKey, waiter: W) -> Seen {
        if let Some(outcome) = self.done.get(&key) {
            return Seen::Done(outcome.clone());
        }
        match self.running.get_mut(&key) {
            Some(waiters) => {
                waiters.push(waiter);
                Seen::Running
            }
            None => {
                self.running.insert(key, vec![waiter]);
                Seen::New
            }
        }
    }

    /// The key's outcome, if it finished.
    pub fn done(&self, key: IdempotencyKey) -> Option<&Outcome> {
        self.done.get(&key)
    }

    /// Whether the key is still running.
    pub fn is_running(&self, key: IdempotencyKey) -> bool {
        self.running.contains_key(&key)
    }

    /// Records the key's outcome and returns everyone waiting for it. A key
    /// already finished keeps its first outcome and returns no one.
    pub fn finish(&mut self, key: IdempotencyKey, outcome: Outcome) -> Vec<W> {
        if self.done.contains_key(&key) {
            return Vec::new();
        }
        let waiters = self.running.remove(&key).unwrap_or_default();
        self.done.insert(key, outcome);
        self.order.push_back(key);
        while self.order.len() > KEEP_KEYS {
            if let Some(old) = self.order.pop_front() {
                self.done.remove(&old);
            }
        }
        waiters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u32) -> IdempotencyKey {
        IdempotencyKey::derive(&n.to_string())
    }

    #[test]
    fn repeats_wait_for_the_first_outcome_then_get_it_at_once() {
        let mut keys = Keys::new();
        assert_eq!(keys.begin(key(1), "first"), Seen::New);
        assert_eq!(keys.begin(key(1), "again"), Seen::Running);
        let done = Outcome::Action(ActionOutcome::Done);
        assert_eq!(keys.finish(key(1), done.clone()), ["first", "again"]);
        assert_eq!(keys.begin(key(1), "later"), Seen::Done(done));
        let refused = Outcome::Action(ActionOutcome::UnknownAgent);
        assert!(
            keys.finish(key(1), refused).is_empty(),
            "the first outcome stays"
        );
    }

    #[test]
    fn only_the_newest_keys_are_kept() {
        let mut keys = Keys::new();
        for n in 0..=u32::try_from(KEEP_KEYS).unwrap() {
            keys.begin(key(n), ());
            keys.finish(key(n), Outcome::Action(ActionOutcome::Done));
        }
        assert_eq!(keys.begin(key(0), ()), Seen::New);
        assert!(matches!(keys.begin(key(1), ()), Seen::Done(_)));
    }
}
