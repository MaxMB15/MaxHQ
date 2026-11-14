// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What GitHub events keep in the database beside the cursors: each copy
//! of an event one source heard, until the other source's copy pairs with
//! it, so one heard both ways runs once; the webhook deliveries already
//! let in; and the webhook this device made on each repository.

use maxhq_core::hooks::dedupe;
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, OptionalExtension};

use crate::db::Db;

/// How far apart two copies of one event may say it happened and still
/// pair. A webhook and the events API date an event within seconds of
/// each other, while two events with one key, such as a pull request
/// closed, reopened and closed again, are usually further apart.
pub const PAIR_MS: u64 = 5 * 60 * 1000;
/// How long a copy waits for the other source's, which is longer than
/// the events API ever lags.
const COPY_KEPT_MS: u64 = 24 * 60 * 60 * 1000;
/// How long a delivery's body is remembered: a day past when one counts
/// as stale, so no replay falls between the two.
pub const DELIVERY_KEPT_MS: u64 = dedupe::REPLAY_MS + 24 * 60 * 60 * 1000;

/// Where an event was heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A repository webhook rang.
    Hooked,
    /// The poller read it.
    Polled,
}

impl Db {
    /// Takes a copy of event `key` on repository `repo` that happened at
    /// `at`, heard from `heard` at `now`, and says whether it runs. It
    /// doesn't when the other source heard `key` within [`PAIR_MS`] of
    /// `at`: that copy ran, so the two pair and both are done. Otherwise
    /// it runs, and when it `waits`, it's kept for its pair. Copies only
    /// pair one to one, so a pull request closed, reopened and closed
    /// again runs twice.
    pub fn pair_github(
        &self,
        repo: &str,
        key: &str,
        heard: Heard,
        at: Timestamp,
        now: Timestamp,
        waits: bool,
    ) -> rusqlite::Result<bool> {
        let conn = self.lock();
        let hooked = heard == Heard::Hooked;
        let window = i64::try_from(PAIR_MS).unwrap_or(i64::MAX);
        let pair: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM github_copies
                 WHERE repo = ?1 AND key = ?2 AND hooked = ?3 AND abs(at - ?4) <= ?5
                 ORDER BY abs(at - ?4) LIMIT 1",
                params![repo, key, !hooked, time(at), window],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(pair) = pair {
            conn.execute("DELETE FROM github_copies WHERE rowid = ?1", [pair])?;
            return Ok(false);
        }
        if !waits {
            return Ok(true);
        }
        conn.execute(
            "INSERT INTO github_copies (repo, key, hooked, at, seen) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![repo, key, hooked, time(at), time(now)],
        )?;
        Ok(true)
    }

    /// Notes a webhook delivery by `digest`, its body's SHA-256, at `now`,
    /// and says whether it's the first time, so a replayed body does
    /// nothing whatever delivery ID it comes with.
    pub fn first_github_body(&self, digest: &[u8], now: Timestamp) -> rusqlite::Result<bool> {
        let added = self.lock().execute(
            "INSERT INTO github_deliveries (body, seen) VALUES (?1, ?2) ON CONFLICT (body) DO NOTHING",
            params![digest, time(now)],
        )?;
        Ok(added == 1)
    }

    /// Deletes the copies and deliveries too old to matter at `now`.
    pub fn clean_github_seen(&self, now: Timestamp) -> rusqlite::Result<usize> {
        let before = |kept: u64| time(Timestamp(now.0.saturating_sub(kept)));
        let conn = self.lock();
        let copies = conn.execute(
            "DELETE FROM github_copies WHERE seen <= ?1",
            [before(COPY_KEPT_MS)],
        )?;
        let deliveries = conn.execute(
            "DELETE FROM github_deliveries WHERE seen <= ?1",
            [before(DELIVERY_KEPT_MS)],
        )?;
        Ok(copies + deliveries)
    }

    /// The webhook this device made on a repository, by its
    /// [`RepoName::key`].
    ///
    /// [`RepoName::key`]: crate::secrets::RepoName::key
    pub fn github_hook(&self, repo: &str) -> rusqlite::Result<Option<u64>> {
        let hook: Option<i64> = self
            .lock()
            .query_row(
                "SELECT hook FROM github_hooks WHERE repo = ?1",
                [repo],
                |row| row.get(0),
            )
            .optional()?;
        Ok(hook.map(|hook| hook.max(0) as u64))
    }

    /// The repository whose webhook is `hook`, by its key.
    pub fn github_hook_repo(&self, hook: u64) -> rusqlite::Result<Option<String>> {
        self.lock()
            .query_row(
                "SELECT repo FROM github_hooks WHERE hook = ?1",
                [id(hook)],
                |row| row.get(0),
            )
            .optional()
    }

    /// Every repository this device made a webhook on, by its key.
    pub fn github_hook_repos(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT repo FROM github_hooks ORDER BY repo")?;
        let repos = query.query_map([], |row| row.get(0))?;
        repos.collect()
    }

    /// Notes the webhook this device made on a repository.
    pub fn put_github_hook(&self, repo: &str, hook: u64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO github_hooks (repo, hook) VALUES (?1, ?2)
             ON CONFLICT (repo) DO UPDATE SET hook = excluded.hook",
            params![repo, id(hook)],
        )?;
        Ok(())
    }

    /// Forgets a repository's webhook.
    pub fn remove_github_hook(&self, repo: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute("DELETE FROM github_hooks WHERE repo = ?1", [repo])?;
        Ok(())
    }
}

/// A time as SQLite keeps it.
fn time(at: Timestamp) -> i64 {
    i64::try_from(at.0).unwrap_or(i64::MAX)
}

/// A webhook's ID as SQLite keeps it.
fn id(hook: u64) -> i64 {
    i64::try_from(hook).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = "owner/repo";

    fn pair(db: &Db, key: &str, heard: Heard, at: u64) -> bool {
        db.pair_github(REPO, key, heard, Timestamp(at), Timestamp(at), true)
            .unwrap()
    }

    fn copies(db: &Db) -> i64 {
        db.lock()
            .query_row("SELECT count(*) FROM github_copies", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn an_event_heard_both_ways_runs_once() {
        let db = Db::open_in_memory().unwrap();
        assert!(pair(&db, "push|x", Heard::Hooked, 1_000));
        assert!(!pair(&db, "push|x", Heard::Polled, 31_000));
        assert!(pair(&db, "push|y", Heard::Polled, 1_000));
        assert!(!pair(&db, "push|y", Heard::Hooked, 1_000));
        // The same key on another repository is another event.
        let at = Timestamp(1_000);
        let other = db.pair_github("owner/other", "push|x", Heard::Polled, at, at, true);
        assert!(other.unwrap());
        assert_eq!(copies(&db), 1, "only the unpaired copy waits");
    }

    #[test]
    fn a_copy_that_doesnt_wait_still_pairs() {
        let db = Db::open_in_memory().unwrap();
        let at = Timestamp(1_000);
        assert!(pair(&db, "push|x", Heard::Hooked, 1_000));
        let polled = db.pair_github(REPO, "push|x", Heard::Polled, at, at, false);
        assert!(!polled.unwrap());
        let polled = db.pair_github(REPO, "push|y", Heard::Polled, at, at, false);
        assert!(polled.unwrap());
        assert_eq!(copies(&db), 0);
    }

    #[test]
    fn a_repeat_of_an_event_runs_again_and_pairs_with_its_own_copy() {
        let db = Db::open_in_memory().unwrap();
        let key = "pull_request|closed|4";
        let later = 1_000 + PAIR_MS * 2;
        // Closed, reopened and closed again: the webhook hears both
        // closes before the poll hears either.
        assert!(pair(&db, key, Heard::Hooked, 1_000));
        assert!(pair(&db, key, Heard::Hooked, later));
        assert!(!pair(&db, key, Heard::Polled, later + 2_000));
        assert!(!pair(&db, key, Heard::Polled, 3_000));
        // A third close the webhook missed runs from the poll.
        assert!(pair(&db, key, Heard::Polled, later + PAIR_MS * 2));
    }

    #[test]
    fn a_copy_pairs_with_the_nearest_of_several_in_range() {
        let db = Db::open_in_memory().unwrap();
        let key = "pull_request|closed|4";
        let start = PAIR_MS;
        for (heard, other) in [
            (Heard::Hooked, Heard::Polled),
            (Heard::Polled, Heard::Hooked),
        ] {
            // Two copies from one source, the later one nearer the other
            // source's copy, though both are in range of it.
            assert!(pair(&db, key, heard, start));
            assert!(pair(&db, key, heard, start + 4 * 60_000));
            assert!(!pair(&db, key, other, start + 3 * 60_000));
            // So the earlier one is left, in range of a copy too far from
            // the later one.
            assert!(!pair(&db, key, other, start - 2 * 60_000));
            assert_eq!(copies(&db), 0);
        }
    }

    #[test]
    fn copies_too_far_apart_dont_pair_and_old_ones_go() {
        let db = Db::open_in_memory().unwrap();
        let key = "pull_request|closed|4";
        assert!(pair(&db, key, Heard::Hooked, 1_000));
        assert!(pair(&db, key, Heard::Polled, 1_000 + PAIR_MS + 1));
        // A copy of the same source never pairs with another.
        assert!(pair(&db, key, Heard::Hooked, 1_000));
        let day = Timestamp(1_000 + PAIR_MS + 1 + COPY_KEPT_MS);
        assert_eq!(db.clean_github_seen(day).unwrap(), 3);
        assert!(pair(&db, key, Heard::Polled, 1_000));
    }

    #[test]
    fn a_body_is_let_in_once_until_it_ages_out() {
        let db = Db::open_in_memory().unwrap();
        let now = Timestamp(1_000);
        assert!(db.first_github_body(b"digest", now).unwrap());
        assert!(!db.first_github_body(b"digest", now).unwrap());
        assert!(db.first_github_body(b"other", now).unwrap());
        let week = Timestamp(now.0 + dedupe::REPLAY_MS);
        assert_eq!(db.clean_github_seen(week).unwrap(), 0);
        assert!(!db.first_github_body(b"digest", week).unwrap());
        let later = Timestamp(now.0 + DELIVERY_KEPT_MS);
        assert_eq!(db.clean_github_seen(later).unwrap(), 2);
        assert!(db.first_github_body(b"digest", later).unwrap());
    }

    #[test]
    fn a_hook_round_trips_and_goes() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.github_hook("owner/repo").unwrap(), None);
        db.put_github_hook("owner/repo", 41).unwrap();
        db.put_github_hook("owner/repo", 42).unwrap();
        assert_eq!(db.github_hook("owner/repo").unwrap(), Some(42));
        assert_eq!(
            db.github_hook_repo(42).unwrap().as_deref(),
            Some("owner/repo")
        );
        assert_eq!(db.github_hook_repos().unwrap(), ["owner/repo"]);
        db.remove_github_hook("owner/repo").unwrap();
        assert_eq!(db.github_hook_repo(42).unwrap(), None);
        assert!(db.github_hook_repos().unwrap().is_empty());
    }
}
