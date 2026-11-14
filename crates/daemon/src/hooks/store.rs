// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What webhooks keep in the database: how the device's hooks are reached,
//! each hook's token hash, its delivery log with the coalescing windows
//! still open, the replay keys it saw, and the requests it holds.

use std::collections::BTreeMap;

use maxhq_core::hooks::dedupe;
use maxhq_protocol::ids::{DeviceId, TaskId};
use maxhq_protocol::tasks::{HookAddress, TaskDelivery};
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::db::{Db, RecordError};

/// The state of a row whose window is still open.
pub(super) const OPEN: &str = "open";

/// One row of a delivery log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Row {
    /// What the log shows.
    pub delivery: TaskDelivery,
    /// The coalescing window the delivery opened, while it's open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
}

/// A coalescing window: its key, when it runs, and the requests in it,
/// which the run takes its prompt from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Window {
    pub key: String,
    pub due: Timestamp,
    #[serde(flatten)]
    pub requests: Requests,
}

/// A request a hook holds while its target's device is offline: its row
/// in the log, the requests it runs with, and how many times it was tried
/// again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Held {
    pub delivery: TaskDelivery,
    #[serde(flatten)]
    pub requests: Requests,
    pub tries: u32,
}

/// One request as a window or a hold keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Kept {
    pub headers: BTreeMap<String, String>,
    pub query: String,
    pub body: String,
}

/// The most requests a window keeps for its run.
const MAX_KEPT: usize = 20;

/// The most bytes of bodies a window keeps for its run, past its latest.
const MAX_KEPT_BYTES: usize = 2 * 1024 * 1024;

/// The requests one run reads: the latest, the distinct ones before it,
/// and how many earlier ones went past the caps. A record from before
/// windows kept more than one has only the latest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Requests {
    #[serde(flatten)]
    pub latest: Kept,
    /// Oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier: Vec<Kept>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dropped: u32,
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

impl Requests {
    pub fn one(latest: Kept) -> Self {
        Self {
            latest,
            earlier: Vec::new(),
            dropped: 0,
        }
    }

    /// Makes `request` the latest. One the same as a kept request takes
    /// its place, and past [`MAX_KEPT`] or [`MAX_KEPT_BYTES`] the oldest
    /// go.
    pub fn push(&mut self, request: Kept) {
        let before = std::mem::replace(&mut self.latest, request);
        self.earlier.push(before);
        self.earlier.retain(|kept| *kept != self.latest);
        let mut bytes: usize = self.earlier.iter().map(|kept| kept.body.len()).sum();
        let over = |earlier: &[Kept], bytes: usize| {
            !earlier.is_empty() && (earlier.len() >= MAX_KEPT || bytes > MAX_KEPT_BYTES)
        };
        let mut gone = 0;
        while over(&self.earlier[gone..], bytes) {
            bytes -= self.earlier[gone].body.len();
            gone += 1;
        }
        self.earlier.drain(..gone);
        self.dropped = self
            .dropped
            .saturating_add(u32::try_from(gone).unwrap_or(u32::MAX));
    }

    /// Each request, oldest first.
    pub fn all(&self) -> impl Iterator<Item = &Kept> {
        self.earlier.iter().chain(std::iter::once(&self.latest))
    }
}

/// A stamp as SQLite keeps it.
fn at(time: Timestamp) -> i64 {
    i64::try_from(time.0).unwrap_or(i64::MAX)
}

impl Db {
    /// How this device's hooks are reached. This device only until picked.
    pub(super) fn hook_address(&self) -> Result<HookAddress, RecordError> {
        let record: Option<String> = self
            .lock()
            .query_row("SELECT record FROM hook_address WHERE id = 0", [], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(match record {
            Some(record) => serde_json::from_str(&record)?,
            None => HookAddress::Local,
        })
    }

    /// Replaces how this device's hooks are reached.
    pub(super) fn put_hook_address(&self, address: &HookAddress) -> Result<(), RecordError> {
        self.lock().execute(
            "INSERT INTO hook_address (id, record) VALUES (0, ?1)
             ON CONFLICT (id) DO UPDATE SET record = excluded.record",
            [serde_json::to_string(address)?],
        )?;
        Ok(())
    }

    /// The hash of a hook's token, if it has one.
    pub(super) fn hook_token(&self, task: TaskId) -> rusqlite::Result<Option<Vec<u8>>> {
        self.lock()
            .query_row(
                "SELECT hash FROM hook_tokens WHERE task = ?1",
                [task.to_string()],
                |row| row.get(0),
            )
            .optional()
    }

    /// Replaces a hook's token hash.
    pub(super) fn put_hook_token(&self, task: TaskId, hash: &[u8]) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO hook_tokens (task, hash) VALUES (?1, ?2)
             ON CONFLICT (task) DO UPDATE SET hash = excluded.hash",
            params![task.to_string(), hash],
        )?;
        Ok(())
    }

    /// Forgets a hook's token, deliveries, replay keys and held requests.
    pub(super) fn forget_hook(&self, task: TaskId) -> rusqlite::Result<()> {
        let conn = self.lock();
        let task = task.to_string();
        for table in ["hook_tokens", "hook_deliveries", "hook_seen", "hook_held"] {
            conn.execute(&format!("DELETE FROM {table} WHERE task = ?1"), [&task])?;
        }
        Ok(())
    }

    /// Whether a task saw replay key `key` recently enough to drop a
    /// request at `now`.
    pub(super) fn replayed(
        &self,
        task: TaskId,
        key: &str,
        now: Timestamp,
    ) -> rusqlite::Result<bool> {
        let seen: Option<i64> = self
            .lock()
            .query_row(
                "SELECT seen FROM hook_seen WHERE task = ?1 AND key = ?2",
                params![task.to_string(), key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(seen.is_some_and(|seen| dedupe::replayed(Timestamp(seen.max(0) as u64), now)))
    }

    /// Notes that a task saw replay key `key` at `now`.
    pub(super) fn claim_key(
        &self,
        task: TaskId,
        key: &str,
        now: Timestamp,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO hook_seen (task, key, seen) VALUES (?1, ?2, ?3)
             ON CONFLICT (task, key) DO UPDATE SET seen = excluded.seen",
            params![task.to_string(), key, at(now)],
        )?;
        Ok(())
    }

    /// Gives back a replay key whose request didn't run, so a retry can.
    pub(super) fn release_key(&self, task: TaskId, key: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM hook_seen WHERE task = ?1 AND key = ?2",
            params![task.to_string(), key],
        )?;
        Ok(())
    }

    /// Deletes the replay keys too old to drop anything at `now`.
    pub(super) fn clean_seen(&self, now: Timestamp) -> rusqlite::Result<usize> {
        let before = at(Timestamp(now.0.saturating_sub(dedupe::REPLAY_MS)));
        self.lock()
            .execute("DELETE FROM hook_seen WHERE seen <= ?1", [before])
    }

    /// Writes a row of a task's delivery log in `state`, then drops the
    /// oldest rows in that state past `keep`.
    pub(super) fn put_delivery(
        &self,
        row: &Row,
        state: &str,
        keep: usize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(row)?;
        let task = row.delivery.task.to_string();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO hook_deliveries (task, id, received, state, record)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (task, id) DO UPDATE
             SET received = excluded.received, state = excluded.state, record = excluded.record",
            params![
                task,
                row.delivery.id,
                at(row.delivery.received),
                state,
                record
            ],
        )?;
        conn.execute(
            "DELETE FROM hook_deliveries WHERE task = ?1 AND state = ?2 AND rowid NOT IN (
                 SELECT rowid FROM hook_deliveries WHERE task = ?1 AND state = ?2
                 ORDER BY received DESC, rowid DESC LIMIT ?3
             )",
            params![task, state, i64::try_from(keep).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    /// A task's delivery log, newest first. A row that doesn't parse is
    /// skipped.
    pub(super) fn deliveries(&self, task: TaskId) -> rusqlite::Result<Vec<TaskDelivery>> {
        let rows: Vec<Row> = self.rows(
            "SELECT record FROM hook_deliveries WHERE task = ?1
             ORDER BY received DESC, rowid DESC",
            params![task.to_string()],
        )?;
        Ok(rows.into_iter().map(|row| row.delivery).collect())
    }

    /// The window still open at `now` that a task's coalescing key `key`
    /// joins.
    pub(super) fn open_window(
        &self,
        task: TaskId,
        key: &str,
        now: Timestamp,
    ) -> rusqlite::Result<Option<Row>> {
        let rows: Vec<Row> = self.rows(
            "SELECT record FROM hook_deliveries WHERE task = ?1 AND state = 'open'
             AND json_extract(record, '$.window.key') = ?2
             AND json_extract(record, '$.window.due') > ?3",
            params![task.to_string(), key, at(now)],
        )?;
        Ok(rows.into_iter().next())
    }

    /// How many windows a task has open.
    pub(super) fn open_windows(&self, task: TaskId) -> rusqlite::Result<usize> {
        self.lock().query_row(
            "SELECT count(*) FROM hook_deliveries WHERE task = ?1 AND state = 'open'",
            [task.to_string()],
            |row| {
                row.get::<_, i64>(0)
                    .map(|count| usize::try_from(count).unwrap_or(0))
            },
        )
    }

    /// Every window that's due at `now`, the earliest first.
    pub(super) fn due_windows(&self, now: Timestamp) -> rusqlite::Result<Vec<Row>> {
        self.rows(
            "SELECT record FROM hook_deliveries WHERE state = 'open'
             AND json_extract(record, '$.window.due') <= ?1
             ORDER BY json_extract(record, '$.window.due')",
            params![at(now)],
        )
    }

    /// When the next window is due, if one is open.
    pub(super) fn next_due(&self) -> rusqlite::Result<Option<Timestamp>> {
        let due: Option<i64> = self.lock().query_row(
            "SELECT min(json_extract(record, '$.window.due')) FROM hook_deliveries
             WHERE state = 'open'",
            [],
            |row| row.get(0),
        )?;
        Ok(due.map(|due| Timestamp(due.max(0) as u64)))
    }

    /// How many requests `task` holds, leaving out `id`, which holding it
    /// again would only replace.
    pub(super) fn held_besides(&self, task: TaskId, id: &str) -> rusqlite::Result<u32> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM hook_held WHERE task = ?1 AND id <> ?2",
            params![task.to_string(), id],
            |row| row.get(0),
        )
    }

    /// Holds a request for `device`, to be looked at again at `due`.
    pub(super) fn put_hook_held(
        &self,
        device: DeviceId,
        due: Timestamp,
        held: &Held,
    ) -> Result<(), RecordError> {
        self.lock().execute(
            "INSERT INTO hook_held (task, id, device, due, record) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (task, id) DO UPDATE
             SET device = excluded.device, due = excluded.due, record = excluded.record",
            params![
                held.delivery.task.to_string(),
                held.delivery.id,
                device.to_string(),
                at(due),
                serde_json::to_string(held)?
            ],
        )?;
        Ok(())
    }

    /// The held requests due at `now`, or for a device in `back`, the
    /// earliest first.
    pub(super) fn hook_held(
        &self,
        now: Timestamp,
        back: &[DeviceId],
    ) -> rusqlite::Result<Vec<Held>> {
        let back: Vec<String> = back.iter().map(ToString::to_string).collect();
        self.rows(
            "SELECT record FROM hook_held WHERE due <= ?1 OR device IN (SELECT value FROM json_each(?2))
             ORDER BY due",
            params![at(now), serde_json::to_string(&back).unwrap_or_default()],
        )
    }

    /// When the next held request is due, if one is held.
    pub(super) fn next_hook_held(&self) -> rusqlite::Result<Option<Timestamp>> {
        let due: Option<i64> =
            self.lock()
                .query_row("SELECT min(due) FROM hook_held", [], |row| row.get(0))?;
        Ok(due.map(|due| Timestamp(due.max(0) as u64)))
    }

    /// Lets go of a held request, and its body with it.
    pub(super) fn drop_hook_held(&self, task: TaskId, id: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM hook_held WHERE task = ?1 AND id = ?2",
            params![task.to_string(), id],
        )?;
        Ok(())
    }

    /// The records `sql` selects.
    fn rows<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> rusqlite::Result<Vec<T>> {
        let conn = self.lock();
        let mut query = conn.prepare(sql)?;
        let records = query.query_map(params, |row| row.get::<_, String>(0))?;
        let mut rows = Vec::new();
        for record in records {
            match serde_json::from_str(&record?) {
                Ok(row) => rows.push(row),
                Err(_) => tracing::warn!("skipping a webhook delivery that doesn't parse"),
            }
        }
        Ok(rows)
    }
}
