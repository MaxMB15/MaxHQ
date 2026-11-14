// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: filtering the
// composer's pull request matches and putting the linked ones first,
// translated to Rust and widened from pull requests to issues. The repo's recent items stand in for the
// thread's linked ones, and there's no project filter, since the lookup
// already names one repo.

//! Ranks the pull requests and issues a `#` lookup finds. A query of
//! digits matches numbers that hold it, the exact number first, then the
//! most recently updated. A query of words matches recent items whose
//! number, title, repo and head branch hold every word, ahead of what
//! GitHub's search found.

use std::collections::HashSet;

use maxhq_protocol::context::{GitItemKind, GitItemState, InlineRef};
use maxhq_protocol::time::Timestamp;

/// The most items one lookup gives.
pub const MAX_GIT_ITEMS: usize = 20;

/// A pull request or issue as GitHub described it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundItem {
    /// "owner/name".
    pub repo: String,
    /// The number after `#`.
    pub number: u32,
    /// A pull request or an issue.
    pub kind: GitItemKind,
    /// Its title.
    pub title: String,
    /// Where it stands.
    pub state: GitItemState,
    /// Its page on GitHub.
    pub url: String,
    /// A PR's head branch, when the reply named it.
    pub head: Option<String>,
    /// A PR's base branch, when the reply named it.
    pub base: Option<String>,
    /// The start of its body.
    pub body: String,
    /// When it last changed, in ISO 8601, which sorts as text.
    pub updated_at: String,
}

impl FoundItem {
    /// The item as a ref, checked at `checked_at`.
    pub fn into_ref(self, checked_at: Timestamp) -> InlineRef {
        InlineRef::GitItem {
            repo: self.repo,
            number: self.number,
            kind: self.kind,
            title: self.title,
            state: self.state,
            url: self.url,
            head: self.head,
            base: self.base,
            body: self.body,
            checked_at,
        }
    }

    /// What tells two items apart: the repo, whatever its case, and the
    /// number.
    fn key(&self) -> (String, u32) {
        (self.repo.trim().to_lowercase(), self.number)
    }

    /// The text a word query looks in.
    fn text(&self) -> String {
        format!(
            "#{} {} {} {}",
            self.number,
            self.title,
            self.repo,
            self.head.as_deref().unwrap_or_default()
        )
        .to_lowercase()
    }
}

/// The items whose number holds `query`, the exact number first, then the
/// most recently updated, without repeats.
pub fn by_number(found: Vec<FoundItem>, query: &str, limit: usize) -> Vec<FoundItem> {
    let query = query.trim();
    let matching = found
        .into_iter()
        .filter(|item| item.number.to_string().contains(query));
    let mut unique = once_each(matching);
    let exact = |item: &FoundItem| item.number.to_string() == query;
    unique.sort_by(|left, right| {
        exact(right)
            .cmp(&exact(left))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    unique.truncate(limit);
    unique
}

/// Every item once, the most recently updated first.
pub fn newest(found: Vec<FoundItem>, limit: usize) -> Vec<FoundItem> {
    let mut unique = once_each(found.into_iter());
    unique.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    unique.truncate(limit);
    unique
}

/// The `recent` items that hold every word of `query`, in their order,
/// then what the search found that isn't among them. Where both hold an
/// item, the recent one is kept, since a list names a PR's branches and a
/// search doesn't.
pub fn recent_first(
    searched: Vec<FoundItem>,
    recent: Vec<FoundItem>,
    query: &str,
    limit: usize,
) -> Vec<FoundItem> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let searched_keys: HashSet<(String, u32)> = searched.iter().map(FoundItem::key).collect();
    let local = recent.into_iter().filter(|item| {
        searched_keys.contains(&item.key()) || {
            let text = item.text();
            words.iter().all(|word| text.contains(word.as_str()))
        }
    });
    let mut picked = once_each(local);
    let picked_keys: HashSet<(String, u32)> = picked.iter().map(FoundItem::key).collect();
    picked.extend(
        searched
            .into_iter()
            .filter(|item| !picked_keys.contains(&item.key())),
    );
    picked.truncate(limit);
    picked
}

/// `items` without repeats, keeping the first of each.
fn once_each(items: impl Iterator<Item = FoundItem>) -> Vec<FoundItem> {
    let mut seen = HashSet::new();
    items.filter(|item| seen.insert(item.key())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(number: u32, title: &str, updated_at: &str) -> FoundItem {
        FoundItem {
            repo: "owner/repo".into(),
            number,
            kind: GitItemKind::PullRequest,
            title: title.into(),
            state: GitItemState::Open,
            url: format!("https://github.com/owner/repo/pull/{number}"),
            head: None,
            base: None,
            body: String::new(),
            updated_at: updated_at.into(),
        }
    }

    fn numbers(items: &[FoundItem]) -> Vec<u32> {
        items.iter().map(|item| item.number).collect()
    }

    #[test]
    fn the_exact_number_comes_first_then_the_newest() {
        let found = vec![
            item(120, "a", "2026-10-01T00:00:00Z"),
            item(12, "b", "2026-09-01T00:00:00Z"),
            item(312, "c", "2026-10-20T00:00:00Z"),
            item(7, "d", "2026-10-29T00:00:00Z"),
        ];
        assert_eq!(numbers(&by_number(found, "12", 10)), [12, 312, 120]);
    }

    #[test]
    fn the_same_item_twice_shows_once() {
        let mut other_case = item(5, "dup", "2026-10-02T00:00:00Z");
        other_case.repo = "Owner/Repo".into();
        let found = vec![item(5, "first", "2026-10-01T00:00:00Z"), other_case];
        let picked = by_number(found, "5", 10);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].title, "first");
    }

    #[test]
    fn newest_sorts_by_update_and_keeps_the_limit() {
        let found = vec![
            item(1, "a", "2026-10-01T00:00:00Z"),
            item(2, "b", "2026-10-03T00:00:00Z"),
            item(3, "c", "2026-10-02T00:00:00Z"),
        ];
        assert_eq!(numbers(&newest(found, 2)), [2, 3]);
    }

    #[test]
    fn recent_items_holding_every_word_come_before_the_search() {
        let mut branch = item(4, "Tidy up", "2026-10-04T00:00:00Z");
        branch.head = Some("fix/login-flow".into());
        let recent = vec![
            item(1, "Fix the login page", "2026-10-01T00:00:00Z"),
            item(2, "Login timeout", "2026-10-02T00:00:00Z"),
            branch,
        ];
        let searched = vec![
            item(9, "Login fix in the old API", "2026-09-01T00:00:00Z"),
            item(1, "Fix the login page", "2026-10-01T00:00:00Z"),
        ];
        let picked = recent_first(searched, recent, "login FIX", 10);
        assert_eq!(numbers(&picked), [1, 4, 9]);
    }

    #[test]
    fn a_recent_item_keeps_its_branches_over_the_searched_copy() {
        let mut listed = item(3, "Speed up sync", "2026-10-01T00:00:00Z");
        listed.head = Some("perf/sync".into());
        let searched = vec![item(3, "Speed up sync", "2026-10-01T00:00:00Z")];
        let picked = recent_first(searched, vec![listed], "unrelated words", 10);
        assert_eq!(picked[0].head.as_deref(), Some("perf/sync"));
    }

    #[test]
    fn a_ref_carries_what_was_found() {
        let found = item(8, "Ship it", "2026-10-01T00:00:00Z");
        let InlineRef::GitItem {
            number, checked_at, ..
        } = found.into_ref(Timestamp(42))
        else {
            panic!("expected a git item");
        };
        assert_eq!((number, checked_at), (8, Timestamp(42)));
    }
}
