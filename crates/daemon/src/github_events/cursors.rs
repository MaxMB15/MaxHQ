// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where each watched repository's poller got to, so a restart picks up
//! what happened while `maxd` was down.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{Db, RecordError};

/// How many items a cursor remembers handing on.
const SEEN_KEPT: usize = 500;
/// How many pull requests' heads a cursor remembers.
const PULLS_KEPT: usize = 300;

/// How far a repository's listings have been read. A record from before
/// the listings reads as a new one, which starts from when it's first
/// read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cursor {
    /// From when each listing's items are new, in milliseconds, by its
    /// key: when it was first read, or later once older items were
    /// forgotten.
    pub since: BTreeMap<String, u64>,
    /// The items handed on, by ID, with when each happened, so a listing
    /// that shows one again doesn't hand it on twice.
    pub seen: BTreeMap<String, u64>,
    /// Each pull request's head when it was last listed, by number.
    pub pulls: BTreeMap<u64, PullMark>,
    /// The ETag each listing last came with, by path.
    pub etags: BTreeMap<String, String>,
}

/// Where a pull request stood when it was last listed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PullMark {
    /// The commit its head was at.
    pub sha: String,
    /// When it had last changed when its reviews were last read, if they
    /// were.
    pub reviewed: Option<String>,
    /// When it last changed, which keeps the latest when there are too
    /// many.
    pub updated: u64,
}

impl Cursor {
    /// Forgets the oldest items past [`SEEN_KEPT`], and moves each
    /// listing's start past them so they stay old, and the pull requests
    /// that changed longest ago past [`PULLS_KEPT`].
    pub fn prune(&mut self) {
        if self.seen.len() > SEEN_KEPT {
            let mut times: Vec<u64> = self.seen.values().copied().collect();
            times.sort_unstable();
            let floor = times[times.len() - SEEN_KEPT - 1];
            self.seen.retain(|_, at| *at > floor);
            for since in self.since.values_mut() {
                *since = (*since).max(floor);
            }
        }
        if self.pulls.len() > PULLS_KEPT {
            let mut times: Vec<u64> = self.pulls.values().map(|mark| mark.updated).collect();
            times.sort_unstable();
            let floor = times[times.len() - PULLS_KEPT - 1];
            self.pulls.retain(|_, mark| mark.updated > floor);
        }
    }
}

impl Db {
    /// The cursor of a repository, by its [`RepoName::key`].
    ///
    /// [`RepoName::key`]: crate::secrets::RepoName::key
    pub fn github_cursor(&self, repo: &str) -> Result<Option<Cursor>, RecordError> {
        let record: Option<String> = self
            .lock()
            .query_row(
                "SELECT record FROM github_cursors WHERE repo = ?1",
                params![repo],
                |row| row.get(0),
            )
            .optional()?;
        Ok(record
            .map(|record| serde_json::from_str(&record))
            .transpose()?)
    }

    /// Stores a repository's cursor.
    pub fn put_github_cursor(&self, repo: &str, cursor: &Cursor) -> Result<(), RecordError> {
        let record = serde_json::to_string(cursor)?;
        self.lock().execute(
            "INSERT INTO github_cursors (repo, record) VALUES (?1, ?2)
             ON CONFLICT (repo) DO UPDATE SET record = excluded.record",
            params![repo, record],
        )?;
        Ok(())
    }

    /// Forgets a repository's cursor.
    pub fn remove_github_cursor(&self, repo: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute("DELETE FROM github_cursors WHERE repo = ?1", params![repo])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_round_trips_and_goes() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.github_cursor("owner/repo").unwrap(), None);
        let mut cursor = Cursor::default();
        cursor.since.insert("pulls".into(), 12);
        cursor.seen.insert("comment-5".into(), 14);
        cursor.pulls.insert(
            4,
            PullMark {
                sha: "abc1".into(),
                reviewed: None,
                updated: 13,
            },
        );
        cursor
            .etags
            .insert("/repos/owner/repo/pulls".into(), "\"abc\"".into());
        db.put_github_cursor("owner/repo", &cursor).unwrap();
        assert_eq!(db.github_cursor("owner/repo").unwrap(), Some(cursor));
        db.remove_github_cursor("owner/repo").unwrap();
        assert_eq!(db.github_cursor("owner/repo").unwrap(), None);
    }

    #[test]
    fn a_cursor_from_before_the_listings_starts_afresh() {
        let db = Db::open_in_memory().unwrap();
        db.lock()
            .execute(
                "INSERT INTO github_cursors (repo, record) VALUES ('owner/repo', ?1)",
                [r#"{"newest":null,"etag":"\"2920\""}"#],
            )
            .unwrap();
        assert_eq!(
            db.github_cursor("owner/repo").unwrap(),
            Some(Cursor::default())
        );
    }

    #[test]
    fn forgetting_old_items_keeps_them_old() {
        let mut cursor = Cursor::default();
        cursor.since.insert("comments".into(), 5);
        for at in 0..SEEN_KEPT as u64 + 10 {
            cursor.seen.insert(format!("comment-{at}"), 100 + at);
        }
        cursor.prune();
        assert_eq!(cursor.seen.len(), SEEN_KEPT);
        assert!(!cursor.seen.contains_key("comment-9"));
        assert!(cursor.seen.contains_key("comment-10"));
        assert_eq!(cursor.since["comments"], 109);

        for number in 0..PULLS_KEPT as u64 + 3 {
            let mark = PullMark {
                updated: number,
                ..PullMark::default()
            };
            cursor.pulls.insert(number, mark);
        }
        cursor.prune();
        assert_eq!(cursor.pulls.len(), PULLS_KEPT);
        assert!(!cursor.pulls.contains_key(&2));
    }
}
