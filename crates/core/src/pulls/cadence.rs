// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How often the watcher reads a repository's PRs: a
//! PR whose agent worked in the last half hour every 2 minutes, a quiet
//! one every 5, the open list every 2. GitHub's `X-Poll-Interval` raises
//! the floor, a low rate limit doubles every wait, and the last 500
//! requests are never spent.

use std::time::Duration;

use maxhq_protocol::model::Agent;
use maxhq_protocol::time::Timestamp;

/// How often a repository's open list is read.
pub const LIST_EVERY: Duration = Duration::from_secs(2 * 60);
/// How often an open PR whose agent is active is read.
pub const ACTIVE_EVERY: Duration = Duration::from_secs(2 * 60);
/// How often a quiet open PR is read.
pub const QUIET_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long after its agent's last activity a PR turns quiet.
pub const QUIET_AFTER: Duration = Duration::from_secs(30 * 60);
/// The soonest a due mark runs a repository again after its last read.
pub const DUE_FLOOR: Duration = Duration::from_secs(15);
/// How long a settled agent's PRs are still watched.
pub const SETTLED_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// The requests never spent, so the rest of the device keeps some.
pub const RESERVE: u32 = 500;
/// The wait while only the reserve is left and GitHub didn't say when it
/// resets.
const RESERVE_WAIT: Duration = Duration::from_secs(15 * 60);

/// What GitHub said about the rate limit and polling on its last reply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    /// `X-Poll-Interval`.
    pub poll_after: Option<Duration>,
    /// Requests left in this window.
    pub remaining: Option<u32>,
    /// Requests in a whole window.
    pub limit: Option<u32>,
    /// How long until the window resets.
    pub resets_in: Option<Duration>,
}

/// Whether a lead agent's PRs are watched: it isn't settled, or settled
/// less than [`SETTLED_FOR`] ago.
pub fn watched(agent: &Agent, now: Timestamp) -> bool {
    agent.parent.is_none()
        && agent
            .inbox
            .settled
            .as_ref()
            .is_none_or(|settled| now.0.saturating_sub(settled.at.0) < millis(SETTLED_FOR))
}

/// How often an open PR is read, by when its agent was last active.
pub fn pull_every(last_activity: Timestamp, now: Timestamp) -> Duration {
    if now.0.saturating_sub(last_activity.0) < millis(QUIET_AFTER) {
        ACTIVE_EVERY
    } else {
        QUIET_EVERY
    }
}

/// `every`, stretched by what GitHub said.
pub fn stretch(every: Duration, budget: Budget) -> Duration {
    let every = every.max(budget.poll_after.unwrap_or_default());
    match (budget.remaining, budget.limit) {
        (Some(remaining), _) if remaining <= RESERVE => {
            every.max(budget.resets_in.unwrap_or(RESERVE_WAIT))
        }
        (Some(remaining), Some(limit)) if u64::from(remaining) * 10 < u64::from(limit) => every * 2,
        _ => every,
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;

    #[test]
    fn a_pr_turns_quiet_half_an_hour_after_its_agent() {
        let now = Timestamp(100 * MIN);
        assert_eq!(pull_every(Timestamp(80 * MIN), now), ACTIVE_EVERY);
        assert_eq!(pull_every(Timestamp(70 * MIN), now), QUIET_EVERY);
    }

    #[test]
    fn github_stretches_the_wait() {
        let every = Duration::from_secs(120);
        let budget = |remaining, poll_after| Budget {
            poll_after,
            remaining,
            limit: Some(10_000),
            resets_in: Some(Duration::from_secs(1800)),
        };
        assert_eq!(stretch(every, Budget::default()), every);
        assert_eq!(
            stretch(every, budget(Some(4000), Some(Duration::from_secs(300)))),
            Duration::from_secs(300)
        );
        assert_eq!(stretch(every, budget(Some(900), None)), every * 2);
        assert_eq!(
            stretch(every, budget(Some(500), None)),
            Duration::from_secs(1800)
        );
    }
}
