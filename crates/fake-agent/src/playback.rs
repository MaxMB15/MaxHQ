// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! When each event plays, after speed and the idle limit.

use std::time::Duration;

use crate::cast::Recording;

/// How to play a recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    /// 2.0 plays twice as fast. Must be above zero.
    pub speed: f64,
    /// Caps every pause at this length before speed applies. Falls back to
    /// the recording's own limit when unset.
    pub idle_limit: Option<Duration>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            speed: 1.0,
            idle_limit: None,
        }
    }
}

/// The time after playback starts at which each event plays, one per event.
/// A time past what a `Duration` holds comes out as `Duration::MAX`.
pub fn schedule(recording: &Recording, playback: Playback) -> Vec<Duration> {
    let limit = playback.idle_limit.or(recording.idle_time_limit);
    let mut previous = Duration::ZERO;
    let mut clock = Duration::ZERO;
    recording
        .events
        .iter()
        .map(|event| {
            let mut pause = event.at.saturating_sub(previous);
            if let Some(limit) = limit {
                pause = pause.min(limit);
            }
            previous = previous.max(event.at);
            let scaled = Duration::try_from_secs_f64(pause.as_secs_f64() / playback.speed)
                .unwrap_or(Duration::MAX);
            clock = clock.saturating_add(scaled);
            clock
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cast::{Event, EventKind};

    fn recording(times_ms: &[u64], idle_time_limit: Option<Duration>) -> Recording {
        Recording {
            version: 3,
            cols: 80,
            rows: 24,
            idle_time_limit,
            events: times_ms
                .iter()
                .map(|&ms| Event {
                    at: Duration::from_millis(ms),
                    kind: EventKind::Output(String::new()),
                })
                .collect(),
        }
    }

    fn millis(times: &[Duration]) -> Vec<u128> {
        times.iter().map(Duration::as_millis).collect()
    }

    #[test]
    fn keeps_original_timing_by_default() {
        let times = schedule(&recording(&[100, 400, 10_400], None), Playback::default());
        assert_eq!(millis(&times), [100, 400, 10_400]);
    }

    #[test]
    fn caps_pauses_then_applies_speed() {
        let playback = Playback {
            speed: 2.0,
            idle_limit: Some(Duration::from_secs(1)),
        };
        let times = schedule(&recording(&[100, 400, 10_400], None), playback);
        assert_eq!(millis(&times), [50, 200, 700]);
    }

    #[test]
    fn falls_back_to_the_recordings_limit() {
        let recorded = recording(&[0, 5_000], Some(Duration::from_millis(500)));
        assert_eq!(millis(&schedule(&recorded, Playback::default())), [0, 500]);
    }

    #[test]
    fn a_time_that_goes_backwards_is_no_pause() {
        let times = schedule(
            &recording(&[0, 10_000, 5_000, 11_000], None),
            Playback::default(),
        );
        assert_eq!(millis(&times), [0, 10_000, 10_000, 11_000]);
    }

    #[test]
    fn a_tiny_speed_saturates_instead_of_overflowing() {
        let playback = Playback {
            speed: 1e-300,
            idle_limit: None,
        };
        let times = schedule(&recording(&[0, 1_000, 2_000], None), playback);
        assert_eq!(times, [Duration::ZERO, Duration::MAX, Duration::MAX]);
    }
}
