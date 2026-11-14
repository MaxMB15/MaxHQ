// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The rules for an agent's pull requests: which ones a command opened,
//! when the watcher reads them, when one wakes its agent and when a merge
//! settles it.

use maxhq_protocol::pulls::{LinkSource, PullKey, PullLink, PullState};

pub mod cadence;
pub mod created;
pub mod settle;
pub mod watch;

pub use created::{changes_pulls, created_by};

/// A PR's key in the one spelling MaxHQ keeps: GitHub matches a
/// repository's name in any case, so it's kept in lower case.
pub fn key(repo: &str, number: u64) -> PullKey {
    PullKey {
        repo: repo.to_ascii_lowercase(),
        number,
    }
}

/// Whether a link counts: one you unlinked only stops a branch match from
/// bringing it back.
pub fn shown(link: &PullLink) -> bool {
    link.source != LinkSource::Dismissed
}

/// Whether a link's PR may still be open: it is, or it hasn't been read.
pub fn open(link: &PullLink) -> bool {
    shown(link)
        && link
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.state == PullState::Open)
}

/// Whether any of an agent's PRs may still be open.
pub fn any_open(links: &[PullLink]) -> bool {
    links.iter().any(open)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use maxhq_protocol::pulls::{
        ChecksState, LinkSource, Mergeable, PullLink, PullSnapshot, PullState, ReviewState,
    };
    use maxhq_protocol::time::Timestamp;

    /// An open PR with pending checks.
    pub fn snapshot() -> PullSnapshot {
        PullSnapshot {
            title: "Fix the build".into(),
            state: PullState::Open,
            draft: false,
            author: "octocat".into(),
            head: "fix-build".into(),
            base: "main".into(),
            head_sha: "abc1234def".into(),
            checks: ChecksState::Pending,
            mergeable: Mergeable::Yes,
            review: ReviewState::None,
            additions: 3,
            deletions: 1,
            updated_at: Timestamp(10),
            merged_at: None,
        }
    }

    /// PR `number` in owner/repo, linked at `at`.
    pub fn link(number: u64, at: u64, snapshot: Option<PullSnapshot>) -> PullLink {
        PullLink {
            key: super::key("owner/repo", number),
            source: LinkSource::Created,
            at: Timestamp(at),
            snapshot,
            watch: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{link, snapshot};
    use super::*;

    #[test]
    fn an_unread_or_open_link_may_be_open() {
        let merged = maxhq_protocol::pulls::PullSnapshot {
            state: PullState::Merged,
            ..snapshot()
        };
        let mut dismissed = link(3, 0, None);
        dismissed.source = LinkSource::Dismissed;
        assert!(open(&link(1, 0, None)));
        assert!(open(&link(2, 0, Some(snapshot()))));
        assert!(!open(&link(4, 0, Some(merged))));
        assert!(!open(&dismissed));
        assert!(!any_open(&[dismissed]));
        assert_eq!(key("Owner/Repo", 4).repo, "owner/repo");
    }
}
