// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How fast the app sends a page what changed. A page does its work once
//! for each message it gets, so changes wait in an [`Outbox`] and go out
//! together, at most one batch every [`FRAME`]. The first change after a
//! quiet spell goes at once. [`Paced`] gives an outbox a thread of its own
//! that sends it.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// The shortest time between two batches to one page, about a frame.
pub const FRAME: Duration = Duration::from_millis(16);

/// What waits to go to one page, oldest first, and when it should go.
#[derive(Debug)]
pub struct Outbox<T> {
    queued: Vec<T>,
    /// When the oldest change waiting came.
    since: Option<Instant>,
    /// When the last batch went out.
    sent: Option<Instant>,
    /// The shortest time between two batches.
    gap: Duration,
}

impl<T> Default for Outbox<T> {
    fn default() -> Self {
        Self {
            queued: Vec::new(),
            since: None,
            sent: None,
            gap: FRAME,
        }
    }
}

impl<T> Outbox<T> {
    /// An outbox whose batches go at most one every `gap`, instead of a
    /// frame.
    pub fn every(gap: Duration) -> Self {
        Self {
            gap,
            ..Self::default()
        }
    }

    /// What waits, for a change that came at `now` to join.
    pub fn waiting(&mut self, now: Instant) -> &mut Vec<T> {
        if self.queued.is_empty() {
            self.since = Some(now);
        }
        &mut self.queued
    }

    /// Queues a change that came at `now`.
    pub fn push(&mut self, change: T, now: Instant) {
        self.waiting(now).push(change);
    }

    /// Keeps only the waiting changes `keep` returns true for.
    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        self.queued.retain(keep);
    }

    /// When what waits should go out: at once after a quiet spell, and
    /// otherwise a gap after the last batch. None when nothing waits.
    pub fn due(&self) -> Option<Instant> {
        let since = self.since.filter(|_| !self.queued.is_empty())?;
        Some(match self.sent {
            Some(sent) => since.max(sent + self.gap),
            None => since,
        })
    }

    /// Takes what waits, as the batch going out now.
    pub fn take(&mut self, now: Instant) -> Vec<T> {
        self.sent = Some(now);
        self.since = None;
        std::mem::take(&mut self.queued)
    }
}

/// An outbox with a thread that sends each batch once it's due. Only that
/// thread sends, so the batches go out in order. Clones share it, and the
/// thread stops once the last one drops.
pub struct Paced<T>(Arc<Owner<T>>);

impl<T> Clone for Paced<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// Stops the thread when the last [`Paced`] drops.
struct Owner<T>(Arc<Shared<T>>);

/// What the thread shares with the callers.
struct Shared<T> {
    state: Mutex<State<T>>,
    /// Wakes the thread when a change waits after a quiet spell.
    wake: Condvar,
}

struct State<T> {
    outbox: Outbox<T>,
    stop: bool,
}

impl<T> Drop for Owner<T> {
    fn drop(&mut self) {
        lock(&self.0.state).stop = true;
        self.0.wake.notify_all();
    }
}

impl<T: Send + 'static> Paced<T> {
    /// Starts the thread, named `name`, that hands each batch to `send`.
    pub fn start(name: &str, send: impl Fn(Vec<T>) + Send + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                outbox: Outbox::default(),
                stop: false,
            }),
            wake: Condvar::new(),
        });
        let sender = Arc::clone(&shared);
        if let Err(error) = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || send_batches(&sender, &send))
        {
            tracing::error!(%error, name, "couldn't start a thread that sends changes, so none go out");
        }
        Self(Arc::new(Owner(shared)))
    }

    /// Queues a change, to go out with the next batch.
    pub fn push(&self, change: T) {
        let shared = &self.0 .0;
        let mut state = lock(&shared.state);
        let quiet = state.outbox.due().is_none();
        state.outbox.push(change, Instant::now());
        drop(state);
        // A change that joins others leaves the time the thread waits for as it was.
        if quiet {
            shared.wake.notify_one();
        }
    }
}

/// Sends each batch when it's due, until the last [`Paced`] drops.
fn send_batches<T>(shared: &Shared<T>, send: &dyn Fn(Vec<T>)) {
    let mut state = lock(&shared.state);
    loop {
        if state.stop {
            return;
        }
        let now = Instant::now();
        state = match state.outbox.due() {
            Some(due) if due <= now => {
                let batch = state.outbox.take(now);
                // A slow send doesn't hold up the changes still coming in.
                drop(state);
                send(batch);
                lock(&shared.state)
            }
            Some(due) => {
                shared
                    .wake
                    .wait_timeout(state, due - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => shared
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
        };
    }
}

/// Locks `mutex`. Each change under these locks is made whole, so a panic
/// elsewhere can't leave one half-written.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_batch_goes_at_once_after_a_quiet_spell_and_a_frame_later_otherwise() {
        let start = Instant::now();
        let mut outbox = Outbox::default();
        assert_eq!(outbox.due(), None, "nothing waits");

        outbox.push(1, start);
        assert_eq!(outbox.due(), Some(start), "the first change goes at once");
        assert_eq!(outbox.take(start), [1]);

        outbox.push(2, start + ms(5));
        outbox.push(3, start + ms(10));
        assert_eq!(
            outbox.due(),
            Some(start + FRAME),
            "a frame after the last batch"
        );
        assert_eq!(outbox.take(start + FRAME), [2, 3]);

        let quiet = start + ms(100);
        outbox.push(4, quiet);
        assert_eq!(outbox.due(), Some(quiet), "a quiet spell ended");

        outbox.retain(|_| false);
        assert_eq!(outbox.due(), None, "what waited was dropped");
    }

    #[test]
    fn changes_that_come_while_a_batch_goes_out_go_next_together_in_order() {
        use std::sync::mpsc::{channel, RecvTimeoutError};
        let (sent, heard) = channel();
        // Each send waits for the test to let it finish.
        let (finish, finishing) = channel::<()>();
        let paced = Paced::start("paced-test", move |batch: Vec<u32>| {
            sent.send(batch).unwrap();
            let _ = finishing.recv();
        });
        let wait = ms(5_000);
        paced.push(1);
        assert_eq!(
            heard.recv_timeout(wait).unwrap(),
            [1],
            "the first goes at once"
        );

        for change in 2..=50 {
            paced.push(change);
        }
        finish.send(()).unwrap();
        assert_eq!(
            heard.recv_timeout(wait).unwrap(),
            (2..=50).collect::<Vec<_>>()
        );

        drop(paced);
        drop(finish);
        assert_eq!(
            heard.recv_timeout(wait),
            Err(RecvTimeoutError::Disconnected),
            "the thread stops"
        );
    }
}
