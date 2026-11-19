// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `maxd`'s own database, `maxd.db` in the data dir. It keeps when you last
//! looked at each agent, so done-unread survives a restart, and what the
//! session manager needs to pick its agents up again: the device's
//! settings, each held agent, each holder with the last seq read from it
//! and where its open turn started, and the first answer to each attention
//! event. It also keeps each container workspace's record, each project's
//! settings on this device and each lead agent's inbox, with when its
//! agent was first found gone, the models each account's CLI last
//! listed, so the app can offer them before an agent runs, and the PRs
//! linked to each agent. It keeps each local ACP command's definition
//! too, whose variables' values go in the secret store instead, and the
//! test hosts and their leases. Every version of each project's notes is
//! here, by hash, with each note's one head, which `crate::notes` reads
//! and writes.
//!
//! The session manager's records are JSON, so their shape can grow without
//! a migration.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use maxhq_core::db::{self, OpenError};
use maxhq_protocol::guest::ScannedFile;
use maxhq_protocol::ids::{
    AccountId, AcpAgentId, AgentId, AttentionId, ClientId, DeviceId, IdempotencyKey, LeaseId,
    MoveId, ProjectId, SessionId, SkillSourceId, TestHostId, WorkspaceId,
};
use maxhq_protocol::leases::{TestHost, TestLease};
use maxhq_protocol::model::{CommandRule, DeviceSettings, Inbox, ModelChoice, ProjectSettings};
use maxhq_protocol::pulls::{PullKey, PullLink};
use maxhq_protocol::skills::{ProjectSkills, SkillSource};
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// The database's file name in the data dir.
pub const FILE: &str = "maxd.db";

/// The trusted sets of files kept for each project, newest first, so files
/// that change back, as on another branch, run without a new card. An older
/// set gets the card again.
const TRUSTS_KEPT: i64 = 50;

/// Every schema change, oldest first. Add new ones at the end and never
/// edit one that has shipped. The comment by each table says what removes
/// its rows, and a new table needs one too.
pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        // seen: goes when you delete the agent.
        M::up("CREATE TABLE seen (agent TEXT PRIMARY KEY, at INTEGER NOT NULL);"),
        // device_settings: one row per device, kept. held: goes when the
        // agent ends or you delete it. holders: goes when its holder ends.
        // answers: dropped at restore once a week old.
        M::up(
            "CREATE TABLE device_settings (device TEXT PRIMARY KEY, record TEXT NOT NULL);
             CREATE TABLE held (agent TEXT PRIMARY KEY, record TEXT NOT NULL);
             CREATE TABLE holders (
                 session TEXT PRIMARY KEY,
                 record TEXT NOT NULL,
                 last_seq INTEGER NOT NULL
             );
             CREATE TABLE answers (
                 event TEXT PRIMARY KEY,
                 key TEXT NOT NULL,
                 at INTEGER NOT NULL,
                 client TEXT NOT NULL
             );",
        ),
        M::up("ALTER TABLE holders ADD COLUMN checkpoint INTEGER;"),
        // workspaces: goes when you delete the workspace. project_settings:
        // one row per project, kept. boots and transfers: unused and empty.
        M::up(
            "CREATE TABLE workspaces (id TEXT PRIMARY KEY, instance TEXT NOT NULL UNIQUE, record TEXT NOT NULL);
             CREATE TABLE project_settings (project TEXT PRIMARY KEY, record TEXT NOT NULL);
             CREATE TABLE boots (place TEXT PRIMARY KEY, boot_id TEXT NOT NULL, at INTEGER NOT NULL);
             CREATE TABLE transfers (id TEXT PRIMARY KEY, record TEXT NOT NULL, committed INTEGER NOT NULL, at INTEGER NOT NULL);",
        ),
        // inbox: forgotten once its agent has been gone 30 days.
        M::up("CREATE TABLE inbox (agent TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        M::up("ALTER TABLE inbox ADD COLUMN missing_since INTEGER;"),
        // folders: one row per clone MaxHQ made, kept.
        M::up("CREATE TABLE folders (path TEXT PRIMARY KEY, added_at INTEGER NOT NULL);"),
        // permissions and access_keys: go when you revoke them or delete
        // the task that holds them.
        M::up(
            "CREATE TABLE permissions (id TEXT PRIMARY KEY, record TEXT NOT NULL, started INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE access_keys (id TEXT PRIMARY KEY, hash BLOB NOT NULL, record TEXT NOT NULL);",
        ),
        // tasks: until you delete one, and the rest go with it. task_runs:
        // the newest 50 per task. hook_deliveries: the newest few in each
        // state per task. hook_seen: dropped once past the replay window.
        // github_cursors: goes when the repository is no longer watched.
        M::up(
            "CREATE TABLE tasks (id TEXT PRIMARY KEY, record TEXT NOT NULL);
             CREATE TABLE task_runs (
                 task TEXT NOT NULL,
                 fire_key TEXT NOT NULL,
                 started INTEGER NOT NULL,
                 record TEXT NOT NULL,
                 PRIMARY KEY (task, fire_key)
             );
             CREATE TABLE hook_tokens (task TEXT PRIMARY KEY, hash BLOB NOT NULL);
             CREATE TABLE hook_deliveries (
                 task TEXT NOT NULL,
                 id TEXT NOT NULL,
                 received INTEGER NOT NULL,
                 state TEXT NOT NULL,
                 record TEXT NOT NULL,
                 PRIMARY KEY (task, id)
             );
             CREATE TABLE hook_seen (
                 task TEXT NOT NULL,
                 key TEXT NOT NULL,
                 seen INTEGER NOT NULL,
                 PRIMARY KEY (task, key)
             );
             CREATE TABLE github_cursors (repo TEXT PRIMARY KEY, record TEXT NOT NULL);",
        ),
        // peer_book and hook_address: one row each.
        M::up("CREATE TABLE peer_book (id INTEGER PRIMARY KEY CHECK (id = 1), record TEXT NOT NULL);"),
        M::up("CREATE TABLE hook_address (id INTEGER PRIMARY KEY CHECK (id = 0), record TEXT NOT NULL);"),
        // models: goes with its account.
        M::up("CREATE TABLE models (account TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        // acp_agents: until you remove one.
        M::up("CREATE TABLE acp_agents (id TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        // pull_links: go when you unlink them or the agent goes.
        M::up(
            "CREATE TABLE pull_links (agent TEXT NOT NULL, repo TEXT NOT NULL, \
             number INTEGER NOT NULL, record TEXT NOT NULL, PRIMARY KEY (agent, repo, number)); \
             CREATE INDEX pull_links_by_pull ON pull_links (repo, number);",
        ),
        // hook_held: goes once delivered, or with its task.
        M::up(
            "CREATE TABLE hook_held (task TEXT NOT NULL, id TEXT NOT NULL, device TEXT NOT NULL, \
             due INTEGER NOT NULL, record TEXT NOT NULL, PRIMARY KEY (task, id));",
        ),
        // leases: go when they end. test_hosts: until you remove one.
        M::up(
            "CREATE TABLE leases (id TEXT PRIMARY KEY, record TEXT NOT NULL); \
             CREATE TABLE test_hosts (id TEXT PRIMARY KEY, record TEXT NOT NULL);",
        ),
        // skill_sources: until you remove one. project_skills: one row per
        // project, kept.
        M::up("CREATE TABLE skill_sources (id TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        M::up("CREATE TABLE project_skills (id TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        // moves: one per container moved, kept as the record. acp_installs:
        // one per registry agent, with the versions you confirmed.
        M::up(
            "CREATE TABLE moves (id TEXT PRIMARY KEY, record TEXT NOT NULL); \
             CREATE TABLE acp_installs (id TEXT PRIMARY KEY, record TEXT NOT NULL);",
        ),
        // Network access keeps only the SHA-256 of each token and code.
        // A code goes once used, and both go when you revoke the phone or at
        // start once expired.
        M::up(
            "CREATE TABLE phone_sessions (id TEXT PRIMARY KEY, phone TEXT NOT NULL, \
             hash BLOB NOT NULL UNIQUE, created INTEGER NOT NULL, expires INTEGER NOT NULL, \
             last_used INTEGER); \
             CREATE TABLE phone_codes (hash BLOB PRIMARY KEY, phone TEXT NOT NULL, \
             expires INTEGER NOT NULL);",
        ),
        // Agents you deleted, which observers' reports never bring back.
        // Kept, so a deleted agent stays deleted.
        M::up("CREATE TABLE deleted (agent TEXT PRIMARY KEY, at INTEGER NOT NULL);"),
        // hidden_projects: until you show the project again.
        M::up("CREATE TABLE hidden_projects (project TEXT PRIMARY KEY);"),
        // The timeline items from before an agent moved to another vendor,
        // whose own history doesn't have them. Goes with the held agent.
        M::up("CREATE TABLE earlier (agent TEXT PRIMARY KEY, items TEXT NOT NULL);"),
        // The names you gave agents, which stand over their CLIs' titles.
        // Until you clear the name or delete the agent.
        M::up("CREATE TABLE agent_names (agent TEXT PRIMARY KEY, name TEXT NOT NULL);"),
        // Each copy of a GitHub event one source heard, until the other
        // source's copy pairs with it, so one heard both ways runs once;
        // the SHA-256 of each repository webhook delivery let in; and each
        // repository's webhook this device made. Copies and deliveries are
        // dropped once old, and a webhook goes when it's removed.
        M::up(
            "CREATE TABLE IF NOT EXISTS github_copies (repo TEXT NOT NULL, key TEXT NOT NULL, \
             hooked INTEGER NOT NULL, at INTEGER NOT NULL, seen INTEGER NOT NULL); \
             CREATE INDEX IF NOT EXISTS github_copies_by_key ON github_copies (repo, key); \
             CREATE TABLE IF NOT EXISTS github_deliveries (body BLOB PRIMARY KEY, \
             seen INTEGER NOT NULL); \
             CREATE TABLE IF NOT EXISTS github_hooks (repo TEXT PRIMARY KEY, hook INTEGER NOT NULL);",
        ),
        // The commands agents may run without asking, by project, in the
        // order you saved them. Until you remove one.
        M::up(
            "CREATE TABLE command_rules (project TEXT NOT NULL, command TEXT NOT NULL, \
             exact INTEGER NOT NULL, PRIMARY KEY (project, command, exact));",
        ),
        // The folders you trust to run their own files, and the agents kept
        // from theirs, with the card still waiting for an answer. A project
        // keeps its newest trusts, and a kept agent goes once answered or
        // gone.
        M::up(
            "CREATE TABLE trusted_folders (key TEXT NOT NULL, fingerprint TEXT NOT NULL, \
             at INTEGER NOT NULL, client TEXT NOT NULL, files TEXT NOT NULL, \
             PRIMARY KEY (key, fingerprint)); \
             CREATE TABLE restricted_agents (agent TEXT PRIMARY KEY, record TEXT NOT NULL);",
        ),
        // Every version of each project's notes, by hash, and the one head
        // each note's versions merge to. Kept, as the notes' history.
        M::up(
            "CREATE TABLE note_versions (hash TEXT PRIMARY KEY, project TEXT NOT NULL, \
             note TEXT NOT NULL, record TEXT NOT NULL); \
             CREATE INDEX note_versions_by_note ON note_versions (project, note); \
             CREATE TABLE note_heads (project TEXT NOT NULL, note TEXT NOT NULL, \
             hash TEXT NOT NULL, PRIMARY KEY (project, note));",
        ),
        // The phones the Mac lets do everything here but let other keys
        // in. Until it turns the switch off or the phone is removed.
        M::up("CREATE TABLE phone_everything (phone TEXT PRIMARY KEY);"),
        // Sessions you started in a terminal that ended there, kept as the
        // agent last shown. Until it runs again, MaxHQ continues it, or you
        // settle or delete it.
        M::up("CREATE TABLE ended_outside (agent TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        // The agents here each other device's enabled tasks send runs to,
        // as that device last said, so auto-settle leaves them be. Until
        // it says otherwise, or says nothing for a week.
        M::up("CREATE TABLE task_targets (home TEXT PRIMARY KEY, record TEXT NOT NULL);"),
        // The commands you ran with `!`, as their timeline items, with the
        // item each came after and when it started, so a timeline read
        // again from its vendor gets them back in place. Until their agent
        // is forgotten.
        M::up(
            "CREATE TABLE user_runs (agent TEXT NOT NULL, item TEXT NOT NULL, after TEXT, \
             at INTEGER NOT NULL, record TEXT NOT NULL, PRIMARY KEY (agent, item));",
        ),
    ])
}

/// Why a record couldn't be read or written.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// SQLite failed.
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    /// The record didn't turn into JSON or back.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// The first answer to an attention event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnswerRow {
    /// The event answered.
    pub event: AttentionId,
    /// The answer's idempotency key.
    pub key: IdempotencyKey,
    /// When it arrived.
    pub at: Timestamp,
    /// The client that sent it.
    pub by: ClientId,
}

/// A stored holder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderRow<T> {
    /// The holder's session.
    pub session: SessionId,
    /// What's stored about it.
    pub record: T,
    /// The last seq read from it.
    pub last_seq: u64,
    /// The seq of the line that started the turn still open then, if one
    /// was.
    pub checkpoint: Option<u64>,
}

/// A stored `!` run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRunRow<T> {
    /// Its timeline item's ID.
    pub item: String,
    /// The item it came after, or none when it came first.
    pub after: Option<String>,
    /// When it started.
    pub at: Timestamp,
    /// What's stored about it.
    pub record: T,
}

/// A handle to the database. Clones share one connection.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens or creates the database at `path` and brings its schema up to
    /// date.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        db::open(path, &migrations()).map(Self::wrap)
    }

    /// An empty database in memory, for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, OpenError> {
        db::open_in_memory(&migrations()).map(Self::wrap)
    }

    /// Runs `sql`, for tests that need a row the methods here won't write
    /// or a table that's gone.
    #[cfg(test)]
    pub fn execute(&self, sql: &str) -> rusqlite::Result<usize> {
        self.lock().execute(sql, [])
    }

    /// Shares `conn` between clones.
    fn wrap(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// When you last looked at each agent. A row that doesn't hold an agent
    /// ID is skipped.
    pub fn seen(&self) -> rusqlite::Result<HashMap<AgentId, Timestamp>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, at FROM seen")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut seen = HashMap::new();
        for row in rows {
            let (agent, at) = row?;
            match agent.parse() {
                Ok(agent) => {
                    seen.insert(agent, Timestamp(u64::try_from(at).unwrap_or(0)));
                }
                Err(_) => tracing::warn!(agent, "skipping a seen row that isn't an agent ID"),
            }
        }
        Ok(seen)
    }

    /// Records that you looked at `agent` at `at`. An earlier time than the
    /// one stored changes nothing, so writes can land in any order.
    pub fn mark_seen(&self, agent: AgentId, at: Timestamp) -> rusqlite::Result<()> {
        let at = i64::try_from(at.0).unwrap_or(i64::MAX);
        self.lock().execute(
            "INSERT INTO seen (agent, at) VALUES (?1, ?2)
             ON CONFLICT (agent) DO UPDATE SET at = max(at, excluded.at)",
            params![agent.to_string(), at],
        )?;
        Ok(())
    }

    /// Every agent you deleted. A row that isn't an agent ID is skipped.
    pub fn deleted(&self) -> rusqlite::Result<HashSet<AgentId>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent FROM deleted")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let mut deleted = HashSet::new();
        for row in rows {
            let agent = row?;
            match agent.parse() {
                Ok(agent) => {
                    deleted.insert(agent);
                }
                Err(_) => tracing::warn!(agent, "skipping a deleted row that isn't an agent ID"),
            }
        }
        Ok(deleted)
    }

    /// Records that you deleted these agents at `at`, and forgets when you
    /// last looked at each and the name you gave it.
    pub fn mark_deleted(&self, agents: &[AgentId], at: Timestamp) -> rusqlite::Result<()> {
        let at = i64::try_from(at.0).unwrap_or(i64::MAX);
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for agent in agents {
            let agent = agent.to_string();
            tx.execute(
                "INSERT INTO deleted (agent, at) VALUES (?1, ?2) ON CONFLICT (agent) DO NOTHING",
                params![agent, at],
            )?;
            tx.execute("DELETE FROM seen WHERE agent = ?1", params![agent])?;
            tx.execute("DELETE FROM agent_names WHERE agent = ?1", params![agent])?;
        }
        tx.commit()
    }

    /// The names you gave agents. A row that isn't an agent ID is skipped.
    pub fn agent_names(&self) -> rusqlite::Result<HashMap<AgentId, String>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, name FROM agent_names")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut names = HashMap::new();
        for row in rows {
            let (agent, name) = row?;
            match agent.parse() {
                Ok(agent) => {
                    names.insert(agent, name);
                }
                Err(_) => tracing::warn!(agent, "skipping a name row that isn't an agent ID"),
            }
        }
        Ok(names)
    }

    /// Stores the name you gave an agent, or forgets it.
    pub fn set_agent_name(&self, agent: AgentId, name: Option<&str>) -> rusqlite::Result<()> {
        let agent = agent.to_string();
        let conn = self.lock();
        match name {
            Some(name) => conn.execute(
                "INSERT INTO agent_names (agent, name) VALUES (?1, ?2)
                 ON CONFLICT (agent) DO UPDATE SET name = excluded.name",
                params![agent, name],
            )?,
            None => conn.execute("DELETE FROM agent_names WHERE agent = ?1", params![agent])?,
        };
        Ok(())
    }

    /// The device's stored settings, if any were stored.
    pub fn device_settings(&self, device: DeviceId) -> Result<Option<DeviceSettings>, RecordError> {
        let record: Option<String> = self
            .lock()
            .query_row(
                "SELECT record FROM device_settings WHERE device = ?1",
                params![device.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(record
            .map(|record| serde_json::from_str(&record))
            .transpose()?)
    }

    /// Whether the device's stored settings name `field`. A row stored by a
    /// `maxd` from before a setting leaves it out, and so does no row.
    pub fn settings_name(&self, device: DeviceId, field: &str) -> Result<bool, RecordError> {
        let record: Option<String> = self
            .lock()
            .query_row(
                "SELECT record FROM device_settings WHERE device = ?1",
                params![device.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(record) = record else {
            return Ok(false);
        };
        let record: serde_json::Value = serde_json::from_str(&record)?;
        Ok(record.get(field).is_some())
    }

    /// The other devices this one links to, as the app last sent them.
    pub fn peer_book(&self) -> Result<Option<maxhq_protocol::agents::PeerBook>, RecordError> {
        let record: Option<String> = self
            .lock()
            .query_row("SELECT record FROM peer_book WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(record
            .map(|record| serde_json::from_str(&record))
            .transpose()?)
    }

    /// Replaces the other devices this one links to.
    pub fn set_peer_book(
        &self,
        book: &maxhq_protocol::agents::PeerBook,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(book)?;
        self.lock().execute(
            "INSERT INTO peer_book (id, record) VALUES (1, ?1)
             ON CONFLICT (id) DO UPDATE SET record = excluded.record",
            params![record],
        )?;
        Ok(())
    }

    /// Stores the device's settings.
    pub fn set_device_settings(&self, settings: &DeviceSettings) -> Result<(), RecordError> {
        let record = serde_json::to_string(settings)?;
        self.lock().execute(
            "INSERT INTO device_settings (device, record) VALUES (?1, ?2)
             ON CONFLICT (device) DO UPDATE SET record = excluded.record",
            params![settings.device.to_string(), record],
        )?;
        Ok(())
    }

    /// Every held agent's record. A row that doesn't parse is skipped.
    pub fn held<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<T>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, record FROM held")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut held = Vec::new();
        for row in rows {
            let (agent, record) = row?;
            match serde_json::from_str(&record) {
                Ok(record) => held.push(record),
                Err(err) => tracing::warn!(agent, %err, "skipping a held agent that doesn't parse"),
            }
        }
        Ok(held)
    }

    /// Every terminal session kept since it ended. A row that doesn't parse
    /// is skipped.
    pub fn ended_outside<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<T>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, record FROM ended_outside")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut ended = Vec::new();
        for row in rows {
            let (agent, record) = row?;
            match serde_json::from_str(&record) {
                Ok(record) => ended.push(record),
                Err(err) => {
                    tracing::warn!(agent, %err, "skipping an ended session that doesn't parse")
                }
            }
        }
        Ok(ended)
    }

    /// Keeps a terminal session that ended.
    pub fn put_ended_outside(
        &self,
        agent: AgentId,
        record: &impl Serialize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        self.lock().execute(
            "INSERT INTO ended_outside (agent, record) VALUES (?1, ?2)
             ON CONFLICT (agent) DO UPDATE SET record = excluded.record",
            params![agent.to_string(), record],
        )?;
        Ok(())
    }

    /// Forgets a terminal session kept since it ended.
    pub fn remove_ended_outside(&self, agent: AgentId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM ended_outside WHERE agent = ?1",
            params![agent.to_string()],
        )?;
        Ok(())
    }

    /// What each other device last said its enabled tasks reach here. A
    /// row that doesn't parse is skipped.
    pub fn task_targets<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<T>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT home, record FROM task_targets")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut heard = Vec::new();
        for row in rows {
            let (home, record) = row?;
            match serde_json::from_str(&record) {
                Ok(record) => heard.push(record),
                Err(err) => {
                    tracing::warn!(home, %err, "skipping task targets that don't parse")
                }
            }
        }
        Ok(heard)
    }

    /// Keeps what `home` said its enabled tasks reach here, in place of
    /// what it said before.
    pub fn put_task_targets(
        &self,
        home: DeviceId,
        record: &impl Serialize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        self.lock().execute(
            "INSERT INTO task_targets (home, record) VALUES (?1, ?2)
             ON CONFLICT (home) DO UPDATE SET record = excluded.record",
            params![home.to_string(), record],
        )?;
        Ok(())
    }

    /// Forgets what `home` said its enabled tasks reach here.
    pub fn remove_task_targets(&self, home: DeviceId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM task_targets WHERE home = ?1",
            params![home.to_string()],
        )?;
        Ok(())
    }

    /// The agent of every held row, whether its record parses or not.
    pub fn held_ids(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent FROM held")?;
        let ids = query.query_map([], |row| row.get::<_, String>(0))?;
        ids.collect()
    }

    /// Stores a held agent's record.
    pub fn put_held(&self, agent: AgentId, record: &impl Serialize) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        self.lock().execute(
            "INSERT INTO held (agent, record) VALUES (?1, ?2)
             ON CONFLICT (agent) DO UPDATE SET record = excluded.record",
            params![agent.to_string(), record],
        )?;
        Ok(())
    }

    /// Forgets a held agent, with the timeline items it kept from other
    /// vendors.
    pub fn remove_held(&self, agent: AgentId) -> rusqlite::Result<()> {
        let id = agent.to_string();
        let conn = self.lock();
        conn.execute("DELETE FROM held WHERE agent = ?1", params![id])?;
        conn.execute("DELETE FROM earlier WHERE agent = ?1", params![id])?;
        conn.execute("DELETE FROM user_runs WHERE agent = ?1", params![id])?;
        Ok(())
    }

    /// Every `!` run an agent kept, oldest first. A row that doesn't parse
    /// is skipped.
    pub fn user_runs<T: DeserializeOwned>(
        &self,
        agent: AgentId,
    ) -> rusqlite::Result<Vec<UserRunRow<T>>> {
        let conn = self.lock();
        let mut query = conn.prepare(
            "SELECT item, after, at, record FROM user_runs WHERE agent = ?1 ORDER BY at, rowid",
        )?;
        let rows = query.query_map(params![agent.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut runs = Vec::new();
        for row in rows {
            let (item, after, at, record) = row?;
            match serde_json::from_str(&record) {
                Ok(record) => runs.push(UserRunRow {
                    item,
                    after,
                    at: Timestamp(u64::try_from(at).unwrap_or(0)),
                    record,
                }),
                Err(err) => tracing::warn!(%agent, item, %err, "skipping a run that doesn't parse"),
            }
        }
        Ok(runs)
    }

    /// One `!` run an agent kept, or none when it has no such run or it
    /// doesn't parse.
    pub fn user_run<T: DeserializeOwned>(
        &self,
        agent: AgentId,
        item: &str,
    ) -> Option<UserRunRow<T>> {
        let (after, at, record): (Option<String>, i64, String) = self
            .lock()
            .query_row(
                "SELECT after, at, record FROM user_runs WHERE agent = ?1 AND item = ?2",
                params![agent.to_string(), item],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .ok()??;
        let record = serde_json::from_str(&record)
            .inspect_err(
                |err| tracing::warn!(%agent, item, %err, "skipping a run that doesn't parse"),
            )
            .ok()?;
        Some(UserRunRow {
            item: item.to_owned(),
            after,
            at: Timestamp(u64::try_from(at).unwrap_or(0)),
            record,
        })
    }

    /// Keeps a `!` run. A run kept before keeps its place: only its record
    /// changes.
    pub fn put_user_run(
        &self,
        agent: AgentId,
        item: &str,
        after: Option<&str>,
        at: Timestamp,
        record: &impl Serialize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        let at = i64::try_from(at.0).unwrap_or(i64::MAX);
        self.lock().execute(
            "INSERT INTO user_runs (agent, item, after, at, record) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (agent, item) DO UPDATE SET record = excluded.record",
            params![agent.to_string(), item, after, at, record],
        )?;
        Ok(())
    }

    /// The timeline items an agent kept from the vendors it ran on before,
    /// or none when it has none or they don't parse.
    pub fn earlier<T: DeserializeOwned>(&self, agent: AgentId) -> Option<Vec<T>> {
        let items: String = self
            .lock()
            .query_row(
                "SELECT items FROM earlier WHERE agent = ?1",
                params![agent.to_string()],
                |row| row.get(0),
            )
            .optional()
            .ok()??;
        serde_json::from_str(&items)
            .inspect_err(
                |err| tracing::warn!(%agent, %err, "skipping earlier items that don't parse"),
            )
            .ok()
    }

    /// Stores the timeline items an agent kept from the vendors it ran on
    /// before.
    pub fn put_earlier<T: Serialize>(
        &self,
        agent: AgentId,
        items: &[T],
    ) -> Result<(), RecordError> {
        let items = serde_json::to_string(items)?;
        self.lock().execute(
            "INSERT INTO earlier (agent, items) VALUES (?1, ?2)
             ON CONFLICT (agent) DO UPDATE SET items = excluded.items",
            params![agent.to_string(), items],
        )?;
        Ok(())
    }

    /// Every holder's record, the last seq read from it and its
    /// checkpoint. A row that doesn't parse is skipped.
    pub fn holders<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<HolderRow<T>>> {
        let conn = self.lock();
        let mut query =
            conn.prepare("SELECT session, record, last_seq, checkpoint FROM holders")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        let mut holders = Vec::new();
        for row in rows {
            let (session, record, last_seq, checkpoint) = row?;
            let parsed = session.parse().ok().zip(serde_json::from_str(&record).ok());
            match parsed {
                Some((session, record)) => holders.push(HolderRow {
                    session,
                    record,
                    last_seq: u64::try_from(last_seq).unwrap_or(0),
                    checkpoint: checkpoint.and_then(|seq| u64::try_from(seq).ok()),
                }),
                None => tracing::warn!(session, "skipping a holder that doesn't parse"),
            }
        }
        Ok(holders)
    }

    /// Stores a holder's record. A holder stored before keeps its last seq.
    pub fn put_holder(
        &self,
        session: SessionId,
        record: &impl Serialize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        self.lock().execute(
            "INSERT INTO holders (session, record, last_seq) VALUES (?1, ?2, 0)
             ON CONFLICT (session) DO UPDATE SET record = excluded.record",
            params![session.to_string(), record],
        )?;
        Ok(())
    }

    /// Records the last seq read from a holder, and the seq of the line
    /// that started the turn still open, if one is.
    pub fn set_holder_seq(
        &self,
        session: SessionId,
        last_seq: u64,
        checkpoint: Option<u64>,
    ) -> rusqlite::Result<()> {
        let last_seq = i64::try_from(last_seq).unwrap_or(i64::MAX);
        let checkpoint = checkpoint.and_then(|seq| i64::try_from(seq).ok());
        self.lock().execute(
            "UPDATE holders SET last_seq = ?2, checkpoint = ?3 WHERE session = ?1",
            params![session.to_string(), last_seq, checkpoint],
        )?;
        Ok(())
    }

    /// Whether a holder has a row, whether its record parses or not.
    pub fn has_holder(&self, session: SessionId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row(
                "SELECT 1 FROM holders WHERE session = ?1",
                params![session.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
    }

    /// Forgets a holder.
    pub fn remove_holder(&self, session: SessionId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM holders WHERE session = ?1",
            params![session.to_string()],
        )?;
        Ok(())
    }

    /// The first answer to every event still remembered. A row that doesn't
    /// parse is skipped.
    pub fn answers(&self) -> rusqlite::Result<Vec<AnswerRow>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT event, key, at, client FROM answers")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut answers = Vec::new();
        for row in rows {
            let (event, key, at, client) = row?;
            match (event.parse(), key.parse(), client.parse()) {
                (Ok(event), Ok(key), Ok(by)) => answers.push(AnswerRow {
                    event,
                    key,
                    at: Timestamp(u64::try_from(at).unwrap_or(0)),
                    by,
                }),
                _ => tracing::warn!(event, "skipping an answer that doesn't parse"),
            }
        }
        Ok(answers)
    }

    /// Records the first answer to an event. A later one changes nothing.
    pub fn put_answer(&self, answer: &AnswerRow) -> rusqlite::Result<()> {
        let at = i64::try_from(answer.at.0).unwrap_or(i64::MAX);
        self.lock().execute(
            "INSERT INTO answers (event, key, at, client) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (event) DO NOTHING",
            params![
                answer.event.to_string(),
                answer.key.to_string(),
                at,
                answer.by.to_string()
            ],
        )?;
        Ok(())
    }

    /// Forgets the answers that arrived before `before`.
    pub fn prune_answers(&self, before: Timestamp) -> rusqlite::Result<usize> {
        let before = i64::try_from(before.0).unwrap_or(i64::MAX);
        self.lock()
            .execute("DELETE FROM answers WHERE at < ?1", params![before])
    }

    /// Whether you trusted these files for this project.
    pub fn folder_trusted(&self, key: &str, fingerprint: &str) -> rusqlite::Result<bool> {
        self.lock()
            .prepare("SELECT 1 FROM trusted_folders WHERE key = ?1 AND fingerprint = ?2")?
            .exists(params![key, fingerprint])
    }

    /// Stores that `by` trusted these files for this project, and forgets
    /// the project's trusts past the newest [`TRUSTS_KEPT`].
    pub fn trust_folder(
        &self,
        key: &str,
        fingerprint: &str,
        at: Timestamp,
        by: ClientId,
        files: &[ScannedFile],
    ) -> Result<(), RecordError> {
        let at = i64::try_from(at.0).unwrap_or(i64::MAX);
        let conn = self.lock();
        conn.execute(
            "INSERT OR REPLACE INTO trusted_folders (key, fingerprint, at, client, files)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                key,
                fingerprint,
                at,
                by.to_string(),
                serde_json::to_string(files)?
            ],
        )?;
        conn.execute(
            "DELETE FROM trusted_folders WHERE key = ?1 AND rowid NOT IN (
                 SELECT rowid FROM trusted_folders WHERE key = ?1
                 ORDER BY at DESC, rowid DESC LIMIT ?2)",
            params![key, TRUSTS_KEPT],
        )?;
        Ok(())
    }

    /// The files of what you last trusted for this project.
    pub fn last_trusted_files(&self, key: &str) -> Result<Option<Vec<ScannedFile>>, RecordError> {
        let files: Option<String> = self
            .lock()
            .query_row(
                "SELECT files FROM trusted_folders WHERE key = ?1
                 ORDER BY at DESC, rowid DESC LIMIT 1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(files
            .map(|files| serde_json::from_str(&files))
            .transpose()?)
    }

    /// Stores what keeps an agent from its folder's files.
    pub fn put_restricted<T: Serialize>(
        &self,
        agent: AgentId,
        record: &T,
    ) -> Result<(), RecordError> {
        self.lock().execute(
            "INSERT OR REPLACE INTO restricted_agents (agent, record) VALUES (?1, ?2)",
            params![agent.to_string(), serde_json::to_string(record)?],
        )?;
        Ok(())
    }

    /// Forgets that an agent is kept from its folder's files.
    pub fn remove_restricted(&self, agent: AgentId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM restricted_agents WHERE agent = ?1",
            params![agent.to_string()],
        )?;
        Ok(())
    }

    /// Every restricted agent. A row that doesn't parse is skipped, and its
    /// agent's next start checks again.
    pub fn restricted<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<(AgentId, T)>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, record FROM restricted_agents")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut restricted = Vec::new();
        for row in rows {
            let (agent, record) = row?;
            match (agent.parse(), serde_json::from_str(&record)) {
                (Ok(agent), Ok(record)) => restricted.push((agent, record)),
                _ => tracing::warn!(agent, "skipping a restricted agent that doesn't parse"),
            }
        }
        Ok(restricted)
    }

    /// Every container workspace's record. A row that doesn't parse is
    /// skipped.
    pub fn workspaces<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<T>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT id, record FROM workspaces")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut workspaces = Vec::new();
        for row in rows {
            let (id, record) = row?;
            match serde_json::from_str(&record) {
                Ok(record) => workspaces.push(record),
                Err(err) => tracing::warn!(id, %err, "skipping a workspace that doesn't parse"),
            }
        }
        Ok(workspaces)
    }

    /// The id of every container workspace's row, whether its record
    /// parses or not.
    pub fn workspace_ids(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT id FROM workspaces")?;
        let ids = query.query_map([], |row| row.get::<_, String>(0))?;
        ids.collect()
    }

    /// Stores a container workspace's record. A row for another workspace
    /// with the same instance goes, since the instance is the truth.
    pub fn put_workspace(
        &self,
        id: WorkspaceId,
        instance: &str,
        record: &impl Serialize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM workspaces WHERE instance = ?1 AND id != ?2",
            params![instance, id.to_string()],
        )?;
        tx.execute(
            "INSERT INTO workspaces (id, instance, record) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET instance = excluded.instance, record = excluded.record",
            params![id.to_string(), instance, record],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Forgets a container workspace.
    pub fn remove_workspace(&self, id: WorkspaceId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM workspaces WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// Every project's settings on this device. A row that doesn't parse
    /// is skipped.
    pub fn project_settings(&self) -> rusqlite::Result<Vec<ProjectSettings>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT project, record FROM project_settings")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut settings = Vec::new();
        for row in rows {
            let (project, record) = row?;
            match serde_json::from_str(&record) {
                Ok(record) => settings.push(record),
                Err(err) => {
                    tracing::warn!(project, %err, "skipping project settings that don't parse");
                }
            }
        }
        Ok(settings)
    }

    /// Stores one project's settings.
    pub fn put_project_settings(&self, settings: &ProjectSettings) -> Result<(), RecordError> {
        let record = serde_json::to_string(settings)?;
        self.lock().execute(
            "INSERT INTO project_settings (project, record) VALUES (?1, ?2)
             ON CONFLICT (project) DO UPDATE SET record = excluded.record",
            params![settings.project.to_string(), record],
        )?;
        Ok(())
    }

    /// The projects hidden from the sidebar. A row that doesn't hold a
    /// project ID is skipped.
    pub fn hidden_projects(&self) -> rusqlite::Result<Vec<ProjectId>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT project FROM hidden_projects ORDER BY project")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let mut hidden = Vec::new();
        for row in rows {
            let project = row?;
            match project.parse() {
                Ok(id) => hidden.push(id),
                Err(_) => tracing::warn!(project, "skipping a hidden row that isn't a project ID"),
            }
        }
        Ok(hidden)
    }

    /// Hides a project from the sidebar, or shows it again.
    pub fn set_project_hidden(&self, project: ProjectId, hidden: bool) -> rusqlite::Result<()> {
        let statement = if hidden {
            "INSERT OR IGNORE INTO hidden_projects (project) VALUES (?1)"
        } else {
            "DELETE FROM hidden_projects WHERE project = ?1"
        };
        self.lock()
            .execute(statement, params![project.to_string()])?;
        Ok(())
    }

    /// Every project's command rules, in the order they were saved. A row
    /// that doesn't hold a project ID is skipped.
    pub fn command_rules(&self) -> rusqlite::Result<Vec<(ProjectId, CommandRule)>> {
        let conn = self.lock();
        let mut query =
            conn.prepare("SELECT project, command, exact FROM command_rules ORDER BY rowid")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        let mut rules = Vec::new();
        for row in rows {
            let (project, command, exact) = row?;
            match project.parse() {
                Ok(id) => rules.push((id, CommandRule { command, exact })),
                Err(_) => tracing::warn!(project, "skipping a command rule without a project ID"),
            }
        }
        Ok(rules)
    }

    /// Keeps a command rule for a project. One it already has stays where
    /// it is.
    pub fn add_command_rule(&self, project: ProjectId, rule: &CommandRule) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT OR IGNORE INTO command_rules (project, command, exact) VALUES (?1, ?2, ?3)",
            params![project.to_string(), rule.command, rule.exact],
        )?;
        Ok(())
    }

    /// Removes a project's command rule.
    pub fn remove_command_rule(
        &self,
        project: ProjectId,
        rule: &CommandRule,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM command_rules WHERE project = ?1 AND command = ?2 AND exact = ?3",
            params![project.to_string(), rule.command, rule.exact],
        )?;
        Ok(())
    }

    /// Every stored inbox. A row that doesn't hold an agent ID or doesn't
    /// parse is skipped.
    pub fn inboxes(&self) -> rusqlite::Result<HashMap<AgentId, Inbox>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, record FROM inbox")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut inboxes = HashMap::new();
        for row in rows {
            let (agent, record) = row?;
            let Ok(id) = agent.parse() else {
                tracing::warn!(agent, "skipping an inbox row that isn't an agent ID");
                continue;
            };
            match serde_json::from_str(&record) {
                Ok(inbox) => {
                    inboxes.insert(id, inbox);
                }
                Err(err) => tracing::warn!(agent, %err, "skipping an inbox that doesn't parse"),
            }
        }
        Ok(inboxes)
    }

    /// Stores an agent's inbox. The default inbox isn't kept, so an agent
    /// you never filed has no row.
    pub fn put_inbox(&self, agent: AgentId, inbox: &Inbox) -> Result<(), RecordError> {
        if *inbox == Inbox::default() {
            return Ok(self.remove_inbox(agent)?);
        }
        let record = serde_json::to_string(inbox)?;
        self.lock().execute(
            "INSERT INTO inbox (agent, record) VALUES (?1, ?2)
             ON CONFLICT (agent) DO UPDATE SET record = excluded.record",
            params![agent.to_string(), record],
        )?;
        Ok(())
    }

    /// When each agent with a stored inbox was first found gone, for those
    /// not back since. A row that doesn't hold an agent ID is skipped.
    pub fn missing_since(&self) -> rusqlite::Result<HashMap<AgentId, Timestamp>> {
        let conn = self.lock();
        let mut query =
            conn.prepare("SELECT agent, missing_since FROM inbox WHERE missing_since IS NOT NULL")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut missing = HashMap::new();
        for row in rows {
            let (agent, since) = row?;
            if let Ok(agent) = agent.parse() {
                missing.insert(agent, Timestamp(u64::try_from(since).unwrap_or(0)));
            }
        }
        Ok(missing)
    }

    /// Stores when an agent was first found gone, or None once it's back.
    /// It goes with the agent's inbox, so an agent without one keeps
    /// nothing.
    pub fn set_missing_since(
        &self,
        agent: AgentId,
        since: Option<Timestamp>,
    ) -> rusqlite::Result<()> {
        let since = since.map(|since| i64::try_from(since.0).unwrap_or(i64::MAX));
        self.lock().execute(
            "UPDATE inbox SET missing_since = ?2 WHERE agent = ?1",
            params![agent.to_string(), since],
        )?;
        Ok(())
    }

    /// Forgets an agent's inbox.
    pub fn remove_inbox(&self, agent: AgentId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM inbox WHERE agent = ?1",
            params![agent.to_string()],
        )?;
        Ok(())
    }

    /// The PRs linked to each agent, oldest link first. A row that doesn't
    /// hold an agent ID or doesn't parse is skipped.
    pub fn pull_links(&self) -> rusqlite::Result<HashMap<AgentId, Vec<PullLink>>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT agent, record FROM pull_links ORDER BY rowid")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut links: HashMap<AgentId, Vec<PullLink>> = HashMap::new();
        for row in rows {
            let (agent, record) = row?;
            let Ok(id) = agent.parse() else {
                tracing::warn!(agent, "skipping a PR link row that isn't an agent ID");
                continue;
            };
            match serde_json::from_str(&record) {
                Ok(link) => links.entry(id).or_default().push(link),
                Err(err) => tracing::warn!(agent, %err, "skipping a PR link that doesn't parse"),
            }
        }
        Ok(links)
    }

    /// Stores one of an agent's PR links, over the one for the same PR.
    pub fn put_pull_link(&self, agent: AgentId, link: &PullLink) -> Result<(), RecordError> {
        let record = serde_json::to_string(link)?;
        self.lock().execute(
            "INSERT INTO pull_links (agent, repo, number, record) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (agent, repo, number) DO UPDATE SET record = excluded.record",
            params![
                agent.to_string(),
                link.key.repo,
                i64::try_from(link.key.number).unwrap_or(i64::MAX),
                record
            ],
        )?;
        Ok(())
    }

    /// Forgets an agent's link to PR `key`, or every link of an agent
    /// that's gone.
    pub fn remove_pull_links(&self, agent: AgentId, key: Option<&PullKey>) -> rusqlite::Result<()> {
        let conn = self.lock();
        match key {
            Some(key) => conn.execute(
                "DELETE FROM pull_links WHERE agent = ?1 AND repo = ?2 AND number = ?3",
                params![
                    agent.to_string(),
                    key.repo,
                    i64::try_from(key.number).unwrap_or(i64::MAX)
                ],
            )?,
            None => conn.execute(
                "DELETE FROM pull_links WHERE agent = ?1",
                params![agent.to_string()],
            )?,
        };
        Ok(())
    }

    /// The models each account's CLI last listed. A row that doesn't hold
    /// an account ID or doesn't parse is skipped.
    pub fn models(&self) -> rusqlite::Result<HashMap<AccountId, Vec<ModelChoice>>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT account, record FROM models")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut models = HashMap::new();
        for row in rows {
            let (account, record) = row?;
            let Ok(id) = account.parse() else {
                tracing::warn!(account, "skipping a models row that isn't an account ID");
                continue;
            };
            match serde_json::from_str(&record) {
                Ok(list) => {
                    models.insert(id, list);
                }
                Err(err) => tracing::warn!(account, %err, "skipping models that don't parse"),
            }
        }
        Ok(models)
    }

    /// Stores the models an account's CLI listed. An empty list isn't
    /// kept, so an account without models has no row.
    pub fn put_models(
        &self,
        account: AccountId,
        models: &[ModelChoice],
    ) -> Result<(), RecordError> {
        if models.is_empty() {
            return Ok(self.remove_models(account)?);
        }
        let record = serde_json::to_string(models)?;
        self.lock().execute(
            "INSERT INTO models (account, record) VALUES (?1, ?2)
             ON CONFLICT (account) DO UPDATE SET record = excluded.record",
            params![account.to_string(), record],
        )?;
        Ok(())
    }

    /// Forgets an account's models.
    pub fn remove_models(&self, account: AccountId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM models WHERE account = ?1",
            params![account.to_string()],
        )?;
        Ok(())
    }

    /// The local ACP agents defined on this device, as their records were
    /// stored.
    pub fn acp_agents(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT record FROM acp_agents ORDER BY id")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// Stores a local ACP agent's record, in place of the one it had.
    pub fn put_acp_agent(&self, id: AcpAgentId, record: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO acp_agents (id, record) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET record = excluded.record",
            params![id.to_string(), record],
        )?;
        Ok(())
    }

    /// Forgets a local ACP agent.
    pub fn remove_acp_agent(&self, id: AcpAgentId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM acp_agents WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// The ACP registry agents installed on this device, by registry ID,
    /// as their records were stored.
    pub fn acp_installs(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT id, record FROM acp_installs ORDER BY id")?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Stores a registry agent's install record, in place of the one it
    /// had.
    pub fn put_acp_install(&self, id: &str, record: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO acp_installs (id, record) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET record = excluded.record",
            params![id, record],
        )?;
        Ok(())
    }

    /// The folders MaxHQ cloned, oldest first, which the hub shows again
    /// at startup.
    pub fn folders(&self) -> rusqlite::Result<Vec<PathBuf>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT path FROM folders ORDER BY added_at, path")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| row.map(PathBuf::from)).collect()
    }

    /// Stores a folder MaxHQ cloned. Storing it again keeps when it was
    /// first added.
    pub fn add_folder(&self, path: &str, at: Timestamp) -> rusqlite::Result<()> {
        let at = i64::try_from(at.0).unwrap_or(i64::MAX);
        self.lock().execute(
            "INSERT INTO folders (path, added_at) VALUES (?1, ?2) ON CONFLICT (path) DO NOTHING",
            params![path, at],
        )?;
        Ok(())
    }

    /// The test leases as stored. A row that won't read is skipped.
    pub fn leases(&self) -> rusqlite::Result<Vec<TestLease>> {
        self.records("leases")
    }

    /// Stores a lease, in place of the record it had.
    pub fn put_lease(&self, lease: &TestLease) -> Result<(), RecordError> {
        self.put_record("leases", &lease.id.to_string(), lease)
    }

    /// Forgets a lease.
    pub fn remove_lease(&self, id: LeaseId) -> rusqlite::Result<()> {
        self.remove_record("leases", &id.to_string())
    }

    /// The test hosts as stored. A row that won't read is skipped.
    pub fn test_hosts(&self) -> rusqlite::Result<Vec<TestHost>> {
        self.records("test_hosts")
    }

    /// Stores a test host, in place of the record it had.
    pub fn put_test_host(&self, host: &TestHost) -> Result<(), RecordError> {
        self.put_record("test_hosts", &host.id.to_string(), host)
    }

    /// Forgets a test host.
    pub fn remove_test_host(&self, id: TestHostId) -> rusqlite::Result<()> {
        self.remove_record("test_hosts", &id.to_string())
    }

    /// The skill sources this device's library keeps.
    pub fn skill_sources(&self) -> rusqlite::Result<Vec<SkillSource>> {
        self.records("skill_sources")
    }

    /// Stores a skill source, in place of the record it had.
    pub fn put_skill_source(&self, source: &SkillSource) -> Result<(), RecordError> {
        self.put_record("skill_sources", &source.id.to_string(), source)
    }

    /// Forgets a skill source.
    pub fn remove_skill_source(&self, id: SkillSourceId) -> rusqlite::Result<()> {
        self.remove_record("skill_sources", &id.to_string())
    }

    /// Every project skills record as plain JSON, for the store's sweep,
    /// which only looks for the trees they name.
    pub fn project_skill_records(&self) -> rusqlite::Result<Vec<serde_json::Value>> {
        self.records("project_skills")
    }

    /// Each project's skills as this device keeps them: its lock, with no
    /// checkouts, since those are found again.
    pub fn project_skills(&self) -> rusqlite::Result<Vec<ProjectSkills>> {
        self.records("project_skills")
    }

    /// Stores a project's skills, in place of the record it had.
    pub fn put_project_skills(&self, skills: &ProjectSkills) -> Result<(), RecordError> {
        self.put_record("project_skills", &skills.project.to_string(), skills)
    }

    /// The workspace moves as stored. A row that won't read is skipped.
    pub fn moves<T: DeserializeOwned>(&self) -> rusqlite::Result<Vec<T>> {
        self.records("moves")
    }

    /// Stores a workspace move, in place of the record it had.
    pub fn put_move(&self, id: MoveId, record: &impl Serialize) -> Result<(), RecordError> {
        self.put_record("moves", &id.to_string(), record)
    }

    /// Every JSON record in `table`, a table of `id` and `record`.
    fn records<T: DeserializeOwned>(&self, table: &str) -> rusqlite::Result<Vec<T>> {
        let conn = self.lock();
        let mut query = conn.prepare(&format!("SELECT record FROM {table} ORDER BY id"))?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let mut records = Vec::new();
        for row in rows {
            if let Ok(record) = serde_json::from_str(&row?) {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Stores `record` as JSON under `id` in `table`.
    fn put_record(
        &self,
        table: &str,
        id: &str,
        record: &impl Serialize,
    ) -> Result<(), RecordError> {
        let record = serde_json::to_string(record)?;
        self.lock().execute(
            &format!(
                "INSERT INTO {table} (id, record) VALUES (?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET record = excluded.record"
            ),
            params![id, record],
        )?;
        Ok(())
    }

    /// Forgets the record under `id` in `table`.
    fn remove_record(&self, table: &str, id: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute(&format!("DELETE FROM {table} WHERE id = ?1"), params![id])?;
        Ok(())
    }

    /// The connection, for one statement or a few.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // SQLite keeps the file consistent whatever the panicking thread
        // was doing, so carry on with the connection.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use maxhq_protocol::model::PermissionMode;

    #[test]
    fn the_migrations_are_valid() {
        assert!(migrations().validate().is_ok());
    }

    #[test]
    fn a_database_from_before_acp_agents_gains_their_table() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        {
            let mut conn = Connection::open(&path).unwrap();
            migrations().to_version(&mut conn, 12).unwrap();
            conn.execute(
                "INSERT INTO held (agent, record) VALUES ('a', '{\"vendor\":\"claude\"}')",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.held_ids().unwrap(), ["a"], "the old rows stay");
        db.execute("INSERT INTO acp_agents (id, record) VALUES ('g', '{}')")
            .unwrap();
        let count: i64 = db
            .lock()
            .query_row("SELECT count(*) FROM acp_agents", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_held_acp_record_an_older_build_cant_read_is_skipped() {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        enum OldVendor {
            #[serde(rename = "claude")]
            Claude,
        }
        #[derive(serde::Deserialize)]
        struct Old {
            #[allow(dead_code)]
            vendor: OldVendor,
        }
        let db = Db::open_in_memory().unwrap();
        db.put_held(
            AgentId::derive("a"),
            &serde_json::json!({ "vendor": "claude" }),
        )
        .unwrap();
        db.put_held(
            AgentId::derive("b"),
            &serde_json::json!({ "vendor": "acp" }),
        )
        .unwrap();
        assert_eq!(db.held::<Old>().unwrap().len(), 1);
    }

    #[test]
    fn an_older_database_gains_pull_links_and_keeps_its_rows() {
        an_older_database_gains(13, "pull_links");
    }

    #[test]
    fn an_older_database_gains_lease_tables_and_keeps_its_rows() {
        for table in ["leases", "test_hosts"] {
            an_older_database_gains(15, table);
        }
    }

    /// Opens a database at schema version `before`, the one just before
    /// `table` came, with one `seen` row, and checks the row survives and
    /// `table` is there and empty. The version is fixed, so later
    /// migrations don't move what's tested.
    fn an_older_database_gains(before: usize, table: &str) {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let agent = AgentId::derive("a");
        {
            let mut conn = Connection::open(&path).unwrap();
            migrations().to_version(&mut conn, before).unwrap();
            let already: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(already, 0, "{table} came before version {before}");
            conn.execute(
                "INSERT INTO seen (agent, at) VALUES (?1, 10)",
                params![agent.to_string()],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.seen().unwrap(), HashMap::from([(agent, Timestamp(10))]));
        let rows: i64 = db
            .lock()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }

    #[test]
    fn an_older_database_gains_phone_switches_and_keeps_its_rows() {
        an_older_database_gains(28, "phone_everything");
    }

    #[test]
    fn an_older_database_gains_skill_sources_and_keeps_its_rows() {
        an_older_database_gains(16, "skill_sources");
    }

    #[test]
    fn an_older_database_gains_project_skills_and_keeps_its_rows() {
        an_older_database_gains(17, "project_skills");
    }

    #[test]
    fn an_older_database_gains_move_and_install_tables_and_keeps_its_rows() {
        an_older_database_gains(18, "moves");
        an_older_database_gains(18, "acp_installs");
    }

    #[test]
    fn an_older_database_gains_the_github_tables_and_keeps_its_rows() {
        for table in ["github_copies", "github_deliveries", "github_hooks"] {
            an_older_database_gains(24, table);
        }
    }

    #[test]
    fn an_older_database_gains_the_note_tables_and_keeps_its_rows() {
        for table in ["note_versions", "note_heads"] {
            an_older_database_gains(27, table);
        }
    }

    #[test]
    fn ended_sessions_survive_a_reopen_until_forgotten() {
        an_older_database_gains(29, "ended_outside");
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let (gone, kept) = (AgentId::derive("gone"), AgentId::derive("kept"));
        let db = Db::open(&path).unwrap();
        db.put_ended_outside(gone, &"first").unwrap();
        db.put_ended_outside(gone, &"second").unwrap();
        db.put_ended_outside(kept, &"kept").unwrap();
        db.remove_ended_outside(gone).unwrap();
        let reopened = Db::open(&path).unwrap();
        assert_eq!(reopened.ended_outside::<String>().unwrap(), vec!["kept"]);
    }

    #[test]
    fn runs_keep_their_place_and_go_with_their_agent() {
        an_older_database_gains(31, "user_runs");
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let (agent, other) = (AgentId::derive("a"), AgentId::derive("b"));
        let db = Db::open(&path).unwrap();
        db.put_user_run(agent, "run-2", Some("m2"), Timestamp(20), &"second")
            .unwrap();
        db.put_user_run(agent, "run-1", None, Timestamp(10), &"first")
            .unwrap();
        db.put_user_run(other, "run-3", Some("m1"), Timestamp(5), &"other")
            .unwrap();
        // An update keeps where the run came.
        db.put_user_run(agent, "run-1", Some("m9"), Timestamp(99), &"ended")
            .unwrap();
        let reopened = Db::open(&path).unwrap();
        let row = |item: &str, after: Option<&str>, at, record: &str| UserRunRow {
            item: item.to_owned(),
            after: after.map(str::to_owned),
            at: Timestamp(at),
            record: record.to_owned(),
        };
        assert_eq!(
            reopened.user_runs::<String>(agent).unwrap(),
            [
                row("run-1", None, 10, "ended"),
                row("run-2", Some("m2"), 20, "second")
            ]
        );
        assert_eq!(
            reopened.user_run::<String>(agent, "run-2"),
            Some(row("run-2", Some("m2"), 20, "second"))
        );
        assert_eq!(reopened.user_run::<String>(agent, "run-3"), None);
        reopened.remove_held(agent).unwrap();
        assert!(reopened.user_runs::<String>(agent).unwrap().is_empty());
        assert_eq!(reopened.user_runs::<String>(other).unwrap().len(), 1);
    }

    #[test]
    fn task_targets_survive_a_reopen_until_forgotten() {
        an_older_database_gains(30, "task_targets");
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let (gone, kept) = (DeviceId::derive("gone"), DeviceId::derive("kept"));
        let db = Db::open(&path).unwrap();
        db.put_task_targets(gone, &"first").unwrap();
        db.put_task_targets(kept, &"first").unwrap();
        db.put_task_targets(kept, &"second").unwrap();
        db.remove_task_targets(gone).unwrap();
        let reopened = Db::open(&path).unwrap();
        assert_eq!(reopened.task_targets::<String>().unwrap(), vec!["second"]);
    }

    #[test]
    fn an_older_database_gains_hidden_projects_and_keeps_its_rows() {
        an_older_database_gains(21, "hidden_projects");
    }

    #[test]
    fn command_rules_survive_a_reopen_in_the_order_they_were_saved() {
        an_older_database_gains(25, "command_rules");
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (ProjectId::derive("a"), ProjectId::derive("b"));
        let rule = |command: &str, exact| CommandRule {
            command: command.into(),
            exact,
        };
        db.add_command_rule(a, &rule("git show", false)).unwrap();
        db.add_command_rule(b, &rule("ls", false)).unwrap();
        db.add_command_rule(a, &rule("rm -rf build", true)).unwrap();
        db.add_command_rule(a, &rule("git show", false)).unwrap();
        db.add_command_rule(a, &rule("cargo test", false)).unwrap();
        db.remove_command_rule(a, &rule("cargo test", false))
            .unwrap();
        db.remove_command_rule(a, &rule("rm -rf build", false))
            .unwrap();
        db.lock()
            .execute(
                "INSERT INTO command_rules (project, command, exact) VALUES ('x', 'ls', 0)",
                [],
            )
            .unwrap();
        assert_eq!(
            Db::open(&path).unwrap().command_rules().unwrap(),
            [
                (a, rule("git show", false)),
                (b, rule("ls", false)),
                (a, rule("rm -rf build", true)),
            ]
        );
    }

    #[test]
    fn hidden_projects_survive_a_reopen() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (ProjectId::derive("a"), ProjectId::derive("b"));
        db.set_project_hidden(a, true).unwrap();
        db.set_project_hidden(a, true).unwrap();
        db.set_project_hidden(b, true).unwrap();
        db.set_project_hidden(b, false).unwrap();
        db.lock()
            .execute("INSERT INTO hidden_projects (project) VALUES ('x')", [])
            .unwrap();
        assert_eq!(Db::open(&path).unwrap().hidden_projects().unwrap(), [a]);
    }

    #[test]
    fn agent_names_survive_a_reopen_until_cleared_or_deleted() {
        an_older_database_gains(23, "agent_names");
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let [kept, cleared, gone] = ["kept", "cleared", "gone"].map(AgentId::derive);
        db.set_agent_name(kept, Some("First")).unwrap();
        db.set_agent_name(kept, Some("Second")).unwrap();
        db.set_agent_name(cleared, Some("Cleared")).unwrap();
        db.set_agent_name(cleared, None).unwrap();
        db.set_agent_name(gone, Some("Gone")).unwrap();
        db.mark_deleted(&[gone], Timestamp(10)).unwrap();
        db.lock()
            .execute(
                "INSERT INTO agent_names (agent, name) VALUES ('x', 'y')",
                [],
            )
            .unwrap();
        assert_eq!(
            Db::open(&path).unwrap().agent_names().unwrap(),
            HashMap::from([(kept, "Second".to_owned())])
        );
    }

    #[test]
    fn seen_times_survive_a_reopen() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let agent = AgentId::derive("a");
        Db::open(&path)
            .unwrap()
            .mark_seen(agent, Timestamp(10))
            .unwrap();
        let reopened = Db::open(&path).unwrap();
        assert_eq!(
            reopened.seen().unwrap(),
            HashMap::from([(agent, Timestamp(10))])
        );
    }

    #[test]
    fn deleted_agents_survive_a_reopen_without_their_seen_times() {
        an_older_database_gains(20, "deleted");
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let (gone, kept) = (AgentId::derive("gone"), AgentId::derive("kept"));
        let db = Db::open(&path).unwrap();
        db.mark_seen(gone, Timestamp(10)).unwrap();
        db.mark_seen(kept, Timestamp(10)).unwrap();
        db.mark_deleted(&[gone], Timestamp(20)).unwrap();
        db.mark_deleted(&[gone], Timestamp(30)).unwrap();
        let reopened = Db::open(&path).unwrap();
        assert_eq!(reopened.deleted().unwrap(), HashSet::from([gone]));
        assert_eq!(
            reopened.seen().unwrap(),
            HashMap::from([(kept, Timestamp(10))])
        );
    }

    #[test]
    fn the_latest_time_wins() {
        let db = Db::open_in_memory().unwrap();
        let agent = AgentId::derive("a");
        db.mark_seen(agent, Timestamp(20)).unwrap();
        db.mark_seen(agent, Timestamp(10)).unwrap();
        assert_eq!(db.seen().unwrap()[&agent], Timestamp(20));
        db.mark_seen(agent, Timestamp(30)).unwrap();
        assert_eq!(db.seen().unwrap()[&agent], Timestamp(30));
    }

    #[test]
    fn rows_that_arent_agents_are_skipped() {
        let db = Db::open_in_memory().unwrap();
        db.lock()
            .execute("INSERT INTO seen (agent, at) VALUES ('nope', 1)", [])
            .unwrap();
        let agent = AgentId::derive("a");
        db.mark_seen(agent, Timestamp(5)).unwrap();
        assert_eq!(db.seen().unwrap().len(), 1);
    }

    #[test]
    fn workspaces_round_trip_and_an_instance_has_one_row() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (WorkspaceId::derive("a"), WorkspaceId::derive("b"));
        db.put_workspace(a, "mhq-a", &"first").unwrap();
        db.put_workspace(a, "mhq-a", &"second").unwrap();
        db.put_workspace(b, "mhq-b", &"other").unwrap();
        let mut rows: Vec<String> = Db::open(&path).unwrap().workspaces().unwrap();
        rows.sort();
        assert_eq!(rows, ["other", "second"]);

        // A new ID for the same instance replaces the old row.
        let c = WorkspaceId::derive("c");
        db.put_workspace(c, "mhq-a", &"third").unwrap();
        let mut rows: Vec<String> = db.workspaces().unwrap();
        rows.sort();
        assert_eq!(rows, ["other", "third"]);

        db.remove_workspace(b).unwrap();
        db.lock()
            .execute(
                "INSERT INTO workspaces (id, instance, record) VALUES ('x', 'mhq-x', '{')",
                [],
            )
            .unwrap();
        assert_eq!(db.workspaces::<String>().unwrap(), ["third"]);
        // Its id is still read, so the workspace still counts.
        let mut ids = db.workspace_ids().unwrap();
        ids.sort();
        assert_eq!(ids, [c.to_string(), "x".to_owned()]);
    }

    #[test]
    fn project_settings_round_trip() {
        let db = Db::open_in_memory().unwrap();
        let settings = ProjectSettings {
            project: maxhq_protocol::ids::ProjectId::derive("p"),
            device: DeviceId::derive("d"),
            resume_on_restart: false,
            ignored_ports: vec![5173],
            git_author: None,
            auto_settle_days: None,
            settle_on_merge: true,
            workflow_env: true,
            memory_soft: None,
            workflow_env_allowed: None,
        };
        db.put_project_settings(&settings).unwrap();
        let mut changed = settings.clone();
        changed.ignored_ports.clear();
        db.put_project_settings(&changed).unwrap();
        assert_eq!(db.project_settings().unwrap(), [changed]);
    }

    #[test]
    fn cloned_folders_survive_a_reopen_in_the_order_added() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        db.add_folder("/home/dev/b", Timestamp(2)).unwrap();
        db.add_folder("/home/dev/a", Timestamp(3)).unwrap();
        db.add_folder("/home/dev/b", Timestamp(9)).unwrap();
        drop(db);
        assert_eq!(
            Db::open(&path).unwrap().folders().unwrap(),
            [PathBuf::from("/home/dev/b"), PathBuf::from("/home/dev/a")]
        );
    }

    #[test]
    fn device_settings_survive_a_reopen() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let device = DeviceId::derive("d");
        assert_eq!(
            Db::open(&path).unwrap().device_settings(device).unwrap(),
            None
        );
        let settings = DeviceSettings {
            device,
            default_permission: PermissionMode::Ask,
            workspace_network: None,
            workspace_pool: None,
            auto_settle_days: 5,
            snooze_at_limit: false,
            continue_claude_at_limit: false,
            continue_codex_at_limit: false,
            claude_effort: None,
            codex_effort: None,
            placement: Default::default(),
            tool_updates: Vec::new(),
            cursor_usage: false,
            keep_awake: Default::default(),
            test_hosts: Default::default(),
            skill_library: None,
            network: Default::default(),
            idle_release_minutes: 60,
            memory_items: 256,
            history_mib: 256,
            claude_setup_token: false,
            shell_integration: true,
        };
        Db::open(&path)
            .unwrap()
            .set_device_settings(&settings)
            .unwrap();
        let reopened = Db::open(&path).unwrap();
        assert_eq!(reopened.device_settings(device).unwrap(), Some(settings));
    }

    #[test]
    fn held_agents_round_trip_and_bad_rows_are_skipped() {
        let db = Db::open_in_memory().unwrap();
        let agent = AgentId::derive("a");
        db.put_held(agent, &"first").unwrap();
        db.put_held(agent, &"second").unwrap();
        db.lock()
            .execute(
                "INSERT INTO held (agent, record) VALUES ('x', 'not json')",
                [],
            )
            .unwrap();
        assert_eq!(db.held::<String>().unwrap(), ["second"]);
        // Its agent is still read, so the agent still counts as held.
        let mut ids = db.held_ids().unwrap();
        ids.sort();
        assert_eq!(ids, [agent.to_string(), "x".to_owned()]);
        db.remove_held(agent).unwrap();
        assert!(db.held::<String>().unwrap().is_empty());
    }

    #[test]
    fn earlier_items_round_trip_and_go_with_their_agent() {
        an_older_database_gains(22, "earlier");
        let db = Db::open_in_memory().unwrap();
        let agent = AgentId::derive("a");
        assert_eq!(db.earlier::<String>(agent), None);
        db.put_held(agent, &"held").unwrap();
        db.put_earlier(agent, &["one"]).unwrap();
        db.put_earlier(agent, &["one", "two"]).unwrap();
        assert_eq!(db.earlier::<String>(agent).unwrap(), ["one", "two"]);
        db.remove_held(agent).unwrap();
        assert_eq!(db.earlier::<String>(agent), None);
    }

    #[test]
    fn holders_keep_their_last_seq_across_record_writes() {
        let db = Db::open_in_memory().unwrap();
        let session = SessionId::derive("s");
        db.put_holder(session, &7_u32).unwrap();
        let row = |record, last_seq, checkpoint| HolderRow {
            session,
            record,
            last_seq,
            checkpoint,
        };
        assert_eq!(db.holders::<u32>().unwrap(), [row(7, 0, None)]);
        db.set_holder_seq(session, 42, Some(30)).unwrap();
        db.put_holder(session, &8_u32).unwrap();
        assert_eq!(db.holders::<u32>().unwrap(), [row(8, 42, Some(30))]);
        db.set_holder_seq(session, 50, None).unwrap();
        assert_eq!(db.holders::<u32>().unwrap(), [row(8, 50, None)]);
        assert!(db.has_holder(session).unwrap());
        db.remove_holder(session).unwrap();
        assert!(db.holders::<u32>().unwrap().is_empty());
        assert!(!db.has_holder(session).unwrap());
    }

    #[test]
    fn inboxes_survive_a_reopen_and_the_default_has_no_row() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (AgentId::derive("a"), AgentId::derive("b"));
        let snoozed = Inbox {
            snoozed_until: Some(Timestamp(20)),
            snoozed_at: Some(Timestamp(10)),
            ..Inbox::default()
        };
        db.put_inbox(a, &snoozed).unwrap();
        db.put_inbox(b, &snoozed).unwrap();
        db.put_inbox(b, &Inbox::default()).unwrap();
        db.lock()
            .execute("INSERT INTO inbox (agent, record) VALUES ('x', '{}')", [])
            .unwrap();
        assert_eq!(
            Db::open(&path).unwrap().inboxes().unwrap(),
            HashMap::from([(a, snoozed)])
        );
        db.remove_inbox(a).unwrap();
        assert!(db.inboxes().unwrap().is_empty());
    }

    #[test]
    fn models_survive_a_reopen_and_an_empty_list_has_no_row() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (AccountId::derive("a"), AccountId::derive("b"));
        let models = vec![ModelChoice {
            id: "gpt-6.1-sol".into(),
            label: "GPT-6.1 Sol".into(),
            default: true,
            efforts: Vec::new(),
            default_effort: None,
            no_images: false,
        }];
        db.put_models(a, &models).unwrap();
        db.put_models(b, &models).unwrap();
        db.put_models(b, &[]).unwrap();
        db.lock()
            .execute(
                "INSERT INTO models (account, record) VALUES ('x', '[]')",
                [],
            )
            .unwrap();
        assert_eq!(
            Db::open(&path).unwrap().models().unwrap(),
            HashMap::from([(a, models)])
        );
        db.remove_models(a).unwrap();
        assert!(db.models().unwrap().is_empty());
    }

    #[test]
    fn pr_links_survive_a_reopen_and_a_new_record_replaces_the_old() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (AgentId::derive("a"), AgentId::derive("b"));
        use maxhq_protocol::pulls::LinkSource;
        let link = |number, source| PullLink {
            key: maxhq_core::pulls::key("owner/repo", number),
            source,
            at: Timestamp(1),
            snapshot: None,
            watch: None,
        };
        db.put_pull_link(a, &link(12, LinkSource::Created)).unwrap();
        db.put_pull_link(a, &link(3, LinkSource::Branch)).unwrap();
        db.put_pull_link(a, &link(12, LinkSource::Dismissed))
            .unwrap();
        db.put_pull_link(b, &link(12, LinkSource::You)).unwrap();
        let links = Db::open(&path).unwrap().pull_links().unwrap();
        assert_eq!(
            links[&a],
            [link(12, LinkSource::Dismissed), link(3, LinkSource::Branch)]
        );
        db.remove_pull_links(a, Some(&link(3, LinkSource::Branch).key))
            .unwrap();
        assert_eq!(
            db.pull_links().unwrap()[&a],
            [link(12, LinkSource::Dismissed)]
        );
        db.remove_pull_links(a, None).unwrap();
        assert_eq!(db.pull_links().unwrap().keys().collect::<Vec<_>>(), [&b]);
    }

    #[test]
    fn when_an_agent_went_missing_lasts_until_its_back_or_its_inbox_goes() {
        let dir = TempDir::new();
        let path = dir.path().join(FILE);
        let db = Db::open(&path).unwrap();
        let (a, b) = (AgentId::derive("a"), AgentId::derive("b"));
        let snoozed = |until| Inbox {
            snoozed_until: Some(Timestamp(until)),
            ..Inbox::default()
        };
        db.put_inbox(a, &snoozed(20)).unwrap();
        db.put_inbox(b, &snoozed(20)).unwrap();
        db.set_missing_since(a, Some(Timestamp(5))).unwrap();
        db.set_missing_since(b, Some(Timestamp(5))).unwrap();
        db.set_missing_since(AgentId::derive("c"), Some(Timestamp(5)))
            .unwrap();
        db.put_inbox(a, &snoozed(30)).unwrap();
        db.set_missing_since(b, None).unwrap();
        assert_eq!(
            Db::open(&path).unwrap().missing_since().unwrap(),
            HashMap::from([(a, Timestamp(5))]),
            "a change to the inbox keeps it, and an agent without one keeps nothing"
        );

        db.put_inbox(a, &Inbox::default()).unwrap();
        db.put_inbox(a, &snoozed(20)).unwrap();
        assert!(
            db.missing_since().unwrap().is_empty(),
            "a new inbox starts over"
        );
    }

    #[test]
    fn the_first_answer_is_kept_until_pruned() {
        let db = Db::open_in_memory().unwrap();
        let first = AnswerRow {
            event: AttentionId::derive("e"),
            key: IdempotencyKey::derive("k1"),
            at: Timestamp(10),
            by: ClientId::derive("mac"),
        };
        db.put_answer(&first).unwrap();
        db.put_answer(&AnswerRow {
            key: IdempotencyKey::derive("k2"),
            at: Timestamp(20),
            by: ClientId::derive("phone"),
            ..first
        })
        .unwrap();
        assert_eq!(db.answers().unwrap(), [first]);
        assert_eq!(db.prune_answers(Timestamp(11)).unwrap(), 1);
        assert!(db.answers().unwrap().is_empty());
    }

    #[test]
    fn a_project_keeps_only_its_newest_trusts() {
        let db = Db::open_in_memory().unwrap();
        let by = ClientId::derive("mac");
        let files = |n: i64| {
            vec![ScannedFile {
                name: ".mcp.json".into(),
                digest: n.to_string(),
            }]
        };
        let extra = 3;
        for n in 0..TRUSTS_KEPT + extra {
            let at = Timestamp(u64::try_from(n).unwrap());
            db.trust_folder("p", &n.to_string(), at, by, &files(n))
                .unwrap();
        }
        db.trust_folder("other", "0", Timestamp(0), by, &files(0))
            .unwrap();
        for n in 0..extra {
            assert!(!db.folder_trusted("p", &n.to_string()).unwrap(), "{n}");
        }
        for n in extra..TRUSTS_KEPT + extra {
            assert!(db.folder_trusted("p", &n.to_string()).unwrap(), "{n}");
        }
        assert!(db.folder_trusted("other", "0").unwrap());
        let last = TRUSTS_KEPT + extra - 1;
        assert_eq!(db.last_trusted_files("p").unwrap(), Some(files(last)));

        // Trusting an older set again makes it the newest.
        db.trust_folder("p", "3", Timestamp(1_000), by, &files(3))
            .unwrap();
        db.trust_folder("p", "x", Timestamp(1_001), by, &files(-1))
            .unwrap();
        assert!(db.folder_trusted("p", "3").unwrap());
        assert!(!db.folder_trusted("p", "4").unwrap());
    }
}
