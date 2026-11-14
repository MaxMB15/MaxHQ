// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Time for the session manager, so tests move it by hand instead of
//! sleeping.

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use maxhq_protocol::time::Timestamp;

/// A future the manager can hold across its loop.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Where the manager reads the time and waits for it.
pub trait Clock: Send + Sync + 'static {
    /// The time now, for timers.
    fn now(&self) -> Instant;
    /// The time now, for stamps clients see.
    fn wall(&self) -> Timestamp;
    /// Resolves once [`Clock::now`] reaches `at`.
    fn sleep_until(&self, at: Instant) -> BoxFuture<()>;
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> Timestamp {
        Timestamp::now()
    }

    fn sleep_until(&self, at: Instant) -> BoxFuture<()> {
        Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(at)))
    }
}

#[cfg(test)]
pub use fake::FakeClock;

#[cfg(test)]
mod fake {
    use std::time::Duration;

    use tokio::sync::watch;

    use super::*;

    /// A clock that moves only when a test calls [`FakeClock::advance`].
    pub struct FakeClock {
        start: Instant,
        wall: u64,
        elapsed: watch::Sender<Duration>,
    }

    impl FakeClock {
        /// A clock stopped at a fixed wall time.
        pub fn new() -> Self {
            Self {
                start: Instant::now(),
                wall: 1_800_000_000_000,
                elapsed: watch::Sender::new(Duration::ZERO),
            }
        }

        /// Moves the clock forward and wakes every sleeper it passes.
        pub fn advance(&self, by: Duration) {
            self.elapsed.send_modify(|elapsed| *elapsed += by);
        }

        /// How many sleeps are waiting, so a test can move the clock once
        /// what it drives has gone to sleep.
        pub fn sleepers(&self) -> usize {
            self.elapsed.receiver_count()
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.start + *self.elapsed.borrow()
        }

        fn wall(&self) -> Timestamp {
            let elapsed = u64::try_from(self.elapsed.borrow().as_millis()).unwrap_or(u64::MAX);
            Timestamp(self.wall + elapsed)
        }

        fn sleep_until(&self, at: Instant) -> BoxFuture<()> {
            let start = self.start;
            let mut elapsed = self.elapsed.subscribe();
            Box::pin(async move {
                while start + *elapsed.borrow_and_update() < at {
                    if elapsed.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            })
        }
    }

    #[tokio::test]
    async fn sleepers_wake_when_the_clock_passes_them() {
        let clock = FakeClock::new();
        let sleep = clock.sleep_until(clock.now() + Duration::from_secs(2));
        let waiting = tokio::spawn(sleep);
        clock.advance(Duration::from_secs(1));
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        clock.advance(Duration::from_secs(1));
        waiting.await.unwrap();
        assert_eq!(clock.wall(), Timestamp(1_800_000_002_000));
    }
}
