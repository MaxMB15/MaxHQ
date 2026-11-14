// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Points in time on the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

/// A point in time, as milliseconds since the Unix epoch in UTC.
///
/// TypeScript sees a plain `number`. JavaScript numbers hold every integer up
/// to 2^53 exactly, and 2^53 milliseconds is about 285,000 years.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Type,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Timestamp(#[specta(type = specta_typescript::Number)] pub u64);

impl Timestamp {
    /// The current time. A clock set before 1970 reads as the epoch.
    pub fn now() -> Self {
        Self::from(SystemTime::now())
    }

    /// The time `duration` after this one.
    pub fn after(self, duration: Duration) -> Self {
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(millis))
    }
}

impl From<SystemTime> for Timestamp {
    fn from(time: SystemTime) -> Self {
        let millis = time.duration_since(UNIX_EPOCH).map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
        Self(millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_milliseconds_from_the_epoch() {
        let time = UNIX_EPOCH + Duration::from_millis(1_761_350_400_123);
        assert_eq!(Timestamp::from(time), Timestamp(1_761_350_400_123));
    }

    #[test]
    fn clocks_before_the_epoch_read_as_zero() {
        let time = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(Timestamp::from(time), Timestamp(0));
    }

    #[test]
    fn after_saturates() {
        assert_eq!(
            Timestamp(u64::MAX - 1).after(Duration::from_secs(1)),
            Timestamp(u64::MAX)
        );
    }
}
