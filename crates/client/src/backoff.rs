// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Retry delays, per decision N-4: decorrelated jitter from a base to a cap,
//! with a longer cap once a device has been gone a long time.

use std::time::Duration;

/// The limits a [`Backoff`] works within.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BackoffLimits {
    /// The shortest delay, and the first one after a reset.
    pub base: Duration,
    /// The longest delay while the device has been gone less than
    /// [`BackoffLimits::long_after`].
    pub cap: Duration,
    /// The longest delay once the device has been gone a long time.
    pub long_cap: Duration,
    /// How long a device has to be gone before the long cap applies.
    pub long_after: Duration,
}

impl BackoffLimits {
    /// N-4's numbers: 1 second to a 60-second cap, and 5 minutes once a
    /// device has been gone an hour.
    pub const DEVICE: Self = Self {
        base: Duration::from_secs(1),
        cap: Duration::from_secs(60),
        long_cap: Duration::from_secs(5 * 60),
        long_after: Duration::from_secs(60 * 60),
    };
}

/// Decorrelated jitter: each delay is random between the base and three
/// times the last one, then held to the cap. Delays spread out so many
/// clients don't retry in step, and they grow without a fixed schedule.
#[derive(Clone, Debug)]
pub(crate) struct Backoff {
    limits: BackoffLimits,
    last: Duration,
}

impl Backoff {
    /// A backoff whose next delay is near the base.
    pub fn new(limits: BackoffLimits) -> Self {
        Self {
            limits,
            last: limits.base,
        }
    }

    /// The delay before the next attempt, given how long the device has been
    /// gone.
    pub fn next(&mut self, gone_for: Duration) -> Duration {
        let cap = if gone_for >= self.limits.long_after {
            self.limits.long_cap
        } else {
            self.limits.cap
        };
        let low = millis(self.limits.base);
        let high = millis(self.last).saturating_mul(3).max(low);
        let delay = Duration::from_millis(fastrand::u64(low..=high)).min(cap);
        self.last = delay;
        delay
    }

    /// Starts over from the base, as after a refresh or a peer coming
    /// online.
    pub fn reset(&mut self) {
        self.last = self.limits.base;
    }
}

/// A duration in whole milliseconds, saturating.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(60 * 60);

    #[test]
    fn delays_stay_between_the_base_and_the_cap() {
        let mut backoff = Backoff::new(BackoffLimits::DEVICE);
        let mut longest = Duration::ZERO;
        for _ in 0..10_000 {
            let delay = backoff.next(Duration::from_secs(5));
            assert!(delay >= Duration::from_secs(1), "{delay:?}");
            assert!(delay <= Duration::from_secs(60), "{delay:?}");
            longest = longest.max(delay);
        }
        // Jitter that never reached the cap in 10,000 tries would be broken.
        assert!(longest > Duration::from_secs(50), "{longest:?}");
    }

    #[test]
    fn the_first_delay_is_near_the_base() {
        for _ in 0..1_000 {
            let mut backoff = Backoff::new(BackoffLimits::DEVICE);
            assert!(backoff.next(Duration::ZERO) <= Duration::from_secs(3));
        }
    }

    #[test]
    fn devices_gone_an_hour_wait_up_to_five_minutes() {
        let mut backoff = Backoff::new(BackoffLimits::DEVICE);
        let mut longest = Duration::ZERO;
        for _ in 0..10_000 {
            let delay = backoff.next(HOUR);
            assert!(delay >= Duration::from_secs(1), "{delay:?}");
            assert!(delay <= Duration::from_secs(300), "{delay:?}");
            longest = longest.max(delay);
        }
        assert!(longest > Duration::from_secs(60), "{longest:?}");
    }

    #[test]
    fn coming_back_from_the_long_cap_drops_to_the_short_one() {
        let mut backoff = Backoff::new(BackoffLimits::DEVICE);
        for _ in 0..100 {
            backoff.next(HOUR);
        }
        assert!(backoff.next(Duration::from_secs(10)) <= Duration::from_secs(60));
    }

    #[test]
    fn reset_starts_over() {
        let mut backoff = Backoff::new(BackoffLimits::DEVICE);
        for _ in 0..100 {
            backoff.next(Duration::ZERO);
        }
        backoff.reset();
        assert!(backoff.next(Duration::ZERO) <= Duration::from_secs(3));
    }
}
