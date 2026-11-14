// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Polling GitHub with conditional requests: ask with the last ETag, act
//! only on a change, and never ask sooner than GitHub's `X-Poll-Interval`.
//! The event pollers use it, and so can anything else that watches GitHub.

use std::future::Future;
use std::time::Duration;

use crate::secrets::Fetched;
use crate::session::clock::Clock;

/// How long to wait between polls when GitHub doesn't say.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
/// The longest wait after polls that failed in a row.
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// What one poll got.
pub struct Polled<T> {
    /// What GitHub sent, or that nothing changed.
    pub fetched: Fetched<T>,
    /// How long GitHub asks to wait before the next poll.
    pub poll_after: Option<Duration>,
}

/// Polls forever, starting now with `etag`. `fetch` asks with the ETag
/// of the last change, and gives none when it failed, after saying why.
/// `on_change` gets each change and its ETag, and the next poll waits for
/// it. After a failure the wait doubles from [`DEFAULT_INTERVAL`] up to 15
/// minutes.
pub async fn poll_loop<T, Fetch, Fetching, Change, Changing>(
    clock: &dyn Clock,
    etag: Option<String>,
    fetch: Fetch,
    on_change: Change,
) where
    Fetch: FnMut(Option<String>) -> Fetching,
    Fetching: Future<Output = Option<Polled<T>>>,
    Change: FnMut(T, Option<String>) -> Changing,
    Changing: Future<Output = ()>,
{
    paced_loop(etag, fetch, on_change, |wait| {
        clock.sleep_until(clock.now() + wait)
    })
    .await;
}

/// [`poll_loop`], with `pause` waiting between polls. It gets the wait
/// GitHub asked for, or the backoff, so a caller can stretch it.
pub async fn paced_loop<T, Fetch, Fetching, Change, Changing, Pause, Pausing>(
    mut etag: Option<String>,
    mut fetch: Fetch,
    mut on_change: Change,
    mut pause: Pause,
) where
    Fetch: FnMut(Option<String>) -> Fetching,
    Fetching: Future<Output = Option<Polled<T>>>,
    Change: FnMut(T, Option<String>) -> Changing,
    Changing: Future<Output = ()>,
    Pause: FnMut(Duration) -> Pausing,
    Pausing: Future<Output = ()>,
{
    let mut failures = 0;
    loop {
        let wait = match fetch(etag.clone()).await {
            Some(Polled {
                fetched,
                poll_after,
            }) => {
                failures = 0;
                if let Fetched::Fresh { value, etag: next } = fetched {
                    on_change(value, next.clone()).await;
                    etag = next;
                }
                poll_after.unwrap_or(DEFAULT_INTERVAL)
            }
            None => {
                failures += 1;
                backoff(failures)
            }
        };
        pause(wait).await;
    }
}

/// The wait after `failures` polls in a row failed.
pub(super) fn backoff(failures: u32) -> Duration {
    DEFAULT_INTERVAL
        .saturating_mul(1 << failures.saturating_sub(1).min(8))
        .min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_back_off_up_to_fifteen_minutes() {
        let minutes = |failures| backoff(failures).as_secs() / 60;
        assert_eq!([1, 2, 3, 4, 5, 40].map(minutes), [1, 2, 4, 8, 15, 15]);
    }
}
