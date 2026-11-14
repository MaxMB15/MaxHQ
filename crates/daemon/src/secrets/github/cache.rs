// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What this device read from GitHub lately, by the way it read and the
//! path. A reply younger than its time to live answers without a request.
//! An older one is asked for with its ETag, and a 304, which GitHub doesn't
//! count against the rate limit, makes it young again. The `#` lookup, the
//! PR viewer and the watcher all read through it. At most [`MAX_ENTRIES`]
//! replies and [`MAX_BYTES`] are kept, the least recently used going
//! first. Nothing here holds a token.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde_json::Value;

use super::api::{Fetched, Paced};
use super::{lock, GithubError, RepoName};
use crate::secrets::Secrets;

/// The most replies kept.
pub(super) const MAX_ENTRIES: usize = 512;
/// The most bytes of replies kept, about.
pub(super) const MAX_BYTES: usize = 32 * 1024 * 1024;

/// The replies kept.
#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
    /// The sizes of every entry together.
    bytes: usize,
    /// Counts reads, so the least recently used entry is the one with the
    /// lowest count.
    uses: u64,
}

/// A reply kept for reuse.
struct Entry {
    value: Arc<Value>,
    etag: Option<String>,
    /// When GitHub last said it's current.
    at: Timestamp,
    /// How long it's used without asking.
    ttl: Duration,
    size: usize,
    used: u64,
}

impl Cache {
    /// The reply under `key`, while it's younger than its time to live.
    fn fresh(&mut self, key: &str, now: Timestamp) -> Option<Arc<Value>> {
        self.uses += 1;
        let entry = self.entries.get_mut(key)?;
        if now >= entry.at.after(entry.ttl) {
            return None;
        }
        entry.used = self.uses;
        Some(entry.value.clone())
    }

    /// The ETag to ask for `key` with.
    fn etag(&self, key: &str) -> Option<String> {
        self.entries.get(key)?.etag.clone()
    }

    /// The reply under `key`, young again since GitHub said it's unchanged.
    fn still(&mut self, key: &str, now: Timestamp) -> Option<Arc<Value>> {
        self.uses += 1;
        let entry = self.entries.get_mut(key)?;
        entry.at = now;
        entry.used = self.uses;
        Some(entry.value.clone())
    }

    /// Keeps `value` under `key` for `ttl`, dropping the least recently
    /// used replies past the caps. A reply larger than the whole cache
    /// isn't kept.
    fn keep(
        &mut self,
        key: String,
        value: Value,
        etag: Option<String>,
        ttl: Duration,
        now: Timestamp,
    ) -> Arc<Value> {
        let value = Arc::new(value);
        self.remove(&key);
        let size = key.len() + size_of(&value);
        if size > MAX_BYTES {
            return value;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + size > MAX_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => self.remove(&oldest),
                None => break,
            }
        }
        self.uses += 1;
        self.bytes += size;
        let entry = Entry {
            value: value.clone(),
            etag,
            at: now,
            ttl,
            size,
            used: self.uses,
        };
        self.entries.insert(key, entry);
        value
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes -= entry.size;
        }
    }

    /// Drops every reply whose path, in lowercase, `drop` picks, however it
    /// was read.
    pub(super) fn forget(&mut self, drop: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self
            .entries
            .keys()
            .filter(|key| drop(path_of(key)))
            .cloned()
            .collect();
        for key in gone {
            self.remove(&key);
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The path part of a key.
fn path_of(key: &str) -> &str {
    key.split_once(' ').map_or(key, |(_, path)| path)
}

/// About how many bytes `value` takes, without writing it out.
fn size_of(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 8,
        Value::String(text) => text.len() + 8,
        Value::Array(items) => items.iter().map(size_of).sum::<usize>() + 8,
        Value::Object(fields) => {
            fields
                .iter()
                .map(|(name, value)| name.len() + size_of(value))
                .sum::<usize>()
                + 8
        }
    }
}

/// What GitHub is read with.
pub(super) enum Reader {
    /// The App's token for the repo.
    App(Secret),
    /// `gh`, since the App can't read the repo for the reason it holds.
    Gh(GithubError),
}

impl Reader {
    /// What a reply to `path` is kept under, apart from the other way's, so
    /// nothing the App read is shown through `gh` once the App no longer
    /// reaches the repo.
    fn key(&self, path: &str) -> String {
        let way = match self {
            Self::App(_) => "app",
            Self::Gh(_) => "gh",
        };
        format!("{way} {}", path.to_ascii_lowercase())
    }
}

impl Secrets {
    /// How to read `repo`: with the App where it reaches the repo, and with
    /// `gh` where the device has no App or the App isn't installed there.
    pub(super) async fn reader(&self, repo: &RepoName) -> Result<Reader, GithubError> {
        match self.github(&repo.to_string()).await {
            Ok(token) => Ok(Reader::App(token.value)),
            Err(app @ (GithubError::NoApp | GithubError::NotInstalled(_))) => Ok(Reader::Gh(app)),
            Err(err) => Err(err),
        }
    }

    /// The reply kept for `path`, while it's young, with no request.
    pub(super) fn peek(&self, reader: &Reader, path: &str) -> Option<Arc<Value>> {
        let now = self.now();
        lock(&self.inner.github.cache_reads).fresh(&reader.key(path), now)
    }

    /// `GET path` on `repo`, from what's kept while it's young, and after
    /// that asked again with its ETag. A new reply is kept for what `ttl`
    /// says of it.
    pub(super) async fn read(
        &self,
        repo: &RepoName,
        reader: &Reader,
        path: &str,
        ttl: impl Fn(&Value) -> Duration,
    ) -> Result<Arc<Value>, GithubError> {
        let key = reader.key(path);
        let reads = &self.inner.github.cache_reads;
        let etag = {
            let mut cache = lock(reads);
            if let Some(value) = cache.fresh(&key, self.now()) {
                return Ok(value);
            }
            cache.etag(&key)
        };
        let (value, etag) = match self.get(repo, reader, path, etag.as_deref()).await? {
            Fetched::Fresh { value, etag } => (value, etag),
            Fetched::Unchanged => {
                if let Some(value) = lock(reads).still(&key, self.now()) {
                    return Ok(value);
                }
                // Dropped from what's kept meanwhile, so ask in full.
                match self.get(repo, reader, path, None).await? {
                    Fetched::Fresh { value, etag } => (value, etag),
                    Fetched::Unchanged => return Err(GithubError::BadReply),
                }
            }
        };
        let ttl = ttl(&value);
        Ok(lock(reads).keep(key, value, etag, ttl, self.now()))
    }

    /// `GET path` the way `reader` reads, naming `etag` when there is one,
    /// and noting how fast the App may read `repo` now.
    async fn get(
        &self,
        repo: &RepoName,
        reader: &Reader,
        path: &str,
        etag: Option<&str>,
    ) -> Result<Fetched<Value>, GithubError> {
        let github = &self.inner.github;
        match reader {
            Reader::App(token) => {
                let (fetched, paced) = github.api.get_paced(token, path, etag).await?;
                self.paced(repo, paced);
                Ok(fetched)
            }
            Reader::Gh(_) => github.gh.get_if_changed(path, etag).await,
        }
    }

    /// Notes what a reply on `repo` said of how fast to read, keeping what
    /// it left out from before.
    fn paced(&self, repo: &RepoName, paced: Paced) {
        let mut pacing = lock(&self.inner.github.pacing);
        let kept = pacing.entry(repo.key()).or_default();
        *kept = Paced {
            poll_after: paced.poll_after.or(kept.poll_after),
            remaining: paced.remaining.or(kept.remaining),
            limit: paced.limit.or(kept.limit),
            reset: paced.reset.or(kept.reset),
        };
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: Timestamp = Timestamp(1_000_000);
    const TTL: Duration = Duration::from_secs(30);

    #[test]
    fn a_reply_is_used_until_its_time_to_live_ends() {
        let mut cache = Cache::default();
        cache.keep("app /a".into(), json!(1), Some("\"e\"".into()), TTL, NOW);
        assert_eq!(
            cache.fresh("app /a", NOW.after(TTL / 2)),
            Some(Arc::new(json!(1)))
        );
        assert_eq!(cache.fresh("app /a", NOW.after(TTL)), None);
        assert_eq!(cache.etag("app /a").as_deref(), Some("\"e\""));
        assert!(cache.still("app /a", NOW.after(TTL)).is_some());
        assert!(cache.fresh("app /a", NOW.after(TTL + TTL / 2)).is_some());
    }

    #[test]
    fn the_least_recently_used_goes_first_past_the_count() {
        let mut cache = Cache::default();
        for n in 0..MAX_ENTRIES {
            cache.keep(format!("app /{n}"), json!(n), None, TTL, NOW);
        }
        // Reading the first makes the second the least recently used.
        assert!(cache.fresh("app /0", NOW).is_some());
        cache.keep("app /new".into(), json!(0), None, TTL, NOW);
        assert_eq!(cache.len(), MAX_ENTRIES);
        assert!(cache.fresh("app /0", NOW).is_some());
        assert!(cache.fresh("app /1", NOW).is_none());
    }

    #[test]
    fn the_bytes_are_capped_too() {
        let mut cache = Cache::default();
        let big = || json!("x".repeat(MAX_BYTES / 3));
        cache.keep("app /a".into(), big(), None, TTL, NOW);
        cache.keep("app /b".into(), big(), None, TTL, NOW);
        cache.keep("app /c".into(), big(), None, TTL, NOW);
        assert!(cache.fresh("app /a", NOW).is_none());
        assert!(cache.fresh("app /c", NOW).is_some());
        assert!(cache.bytes <= MAX_BYTES);

        // One reply larger than all of it isn't kept, and drops nothing.
        cache.keep(
            "app /huge".into(),
            json!("x".repeat(MAX_BYTES)),
            None,
            TTL,
            NOW,
        );
        assert!(cache.fresh("app /huge", NOW).is_none());
        assert!(cache.fresh("app /c", NOW).is_some());
    }

    #[test]
    fn forgetting_picks_by_path_whichever_way_it_was_read() {
        let mut cache = Cache::default();
        cache.keep("app /x/1".into(), json!(1), None, TTL, NOW);
        cache.keep("gh /x/1".into(), json!(1), None, TTL, NOW);
        cache.keep("app /x/2".into(), json!(2), None, TTL, NOW);
        cache.forget(|path| path == "/x/1");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes, "app /x/2".len() + 8);
    }
}
