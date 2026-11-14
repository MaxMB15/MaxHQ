// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The hold: a hook that holds requests keeps one whose target's device
//! is offline for up to 24 hours, or the hook's max age if that's
//! shorter, and tries it again after 1, 5, 15 and 60 minutes, then every
//! hour.

use maxhq_protocol::time::Timestamp;

const MINUTE_MS: u64 = 60 * 1000;

/// The longest a request is held.
pub const HOLD_MS: u64 = 24 * 60 * MINUTE_MS;

/// The minutes after it came in that a held request is tried again,
/// before it's tried every hour.
const RETRIES: [u64; 4] = [1, 5, 15, 60];

/// When a request that came in at `received` stops being held.
pub fn expires(received: Timestamp, max_age_minutes: Option<u32>) -> Timestamp {
    let age = max_age_minutes.map_or(HOLD_MS, |minutes| {
        (u64::from(minutes) * MINUTE_MS).min(HOLD_MS)
    });
    Timestamp(received.0.saturating_add(age))
}

/// When a held request is next looked at after `tries` tries again: its
/// next try, or its end if that comes first.
pub fn due(received: Timestamp, tries: u32, max_age_minutes: Option<u32>) -> Timestamp {
    let minutes = match RETRIES.get(tries as usize) {
        Some(minutes) => *minutes,
        None => 60 * (u64::from(tries) - 2),
    };
    let next = Timestamp(received.0.saturating_add(minutes * MINUTE_MS));
    next.min(expires(received, max_age_minutes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IN: Timestamp = Timestamp(1_000_000);

    fn minutes_after(due: Timestamp) -> u64 {
        (due.0 - IN.0) / MINUTE_MS
    }

    #[test]
    fn a_held_request_is_tried_after_1_5_15_and_60_minutes_then_hourly() {
        let tries: Vec<u64> = (0..7).map(|n| minutes_after(due(IN, n, None))).collect();
        assert_eq!(tries, [1, 5, 15, 60, 120, 180, 240]);
    }

    #[test]
    fn a_hold_ends_after_a_day_or_the_hook_s_max_age() {
        assert_eq!(minutes_after(expires(IN, None)), 24 * 60);
        assert_eq!(minutes_after(expires(IN, Some(30))), 30);
        assert_eq!(minutes_after(expires(IN, Some(48 * 60))), 24 * 60);
        assert_eq!(minutes_after(due(IN, 3, Some(30))), 30);
        assert_eq!(minutes_after(due(IN, 40, None)), 24 * 60);
    }
}
