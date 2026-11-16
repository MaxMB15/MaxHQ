// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The devices you added, kept in the app's own database (decision U-7).
//!
//! Each device is one link, keyed by the [`LinkId`] it got when you added
//! it. The `maxd` on this machine is always there as [`Route::Local`], and
//! every other device is an SSH host. The phone has no `maxd` of its own,
//! and its devices are the ones in the books it paired with, one per Mac
//! (see [`Registry::set_phone_books`]).
//!
//! The same database holds the outbox: inbox changes made while their
//! device was away, kept until it's back to take them, and the app's own
//! settings.

use std::collections::BTreeMap;
use std::path::Path;

use maxhq_client::LinkSpec;
use maxhq_core::db::{self, OpenError};
use maxhq_protocol::ids::{AgentId, DeviceId, IdempotencyKey, LinkId, PermissionId};
use maxhq_protocol::link::Route;
use maxhq_protocol::model::InboxChange;
use maxhq_protocol::phones::PhoneBook;
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;

use crate::phone;

mod claims;
mod seed;

/// The app's schema. Append to it, never edit a step that has shipped.
fn migrations() -> Migrations<'static> {
    Migrations::new(steps())
}

fn steps() -> Vec<M<'static>> {
    vec![
        M::up(
            "CREATE TABLE device (
        link TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('local', 'ssh')),
        host TEXT UNIQUE,
        added_at INTEGER NOT NULL,
        CHECK ((kind = 'local') = (host IS NULL))
    ) STRICT;
    CREATE UNIQUE INDEX device_one_local ON device (kind) WHERE kind = 'local';",
        ),
        // A change goes with the link it was queued through, so removing
        // the device drops it.
        M::up(
            "CREATE TABLE outbox (
        id INTEGER PRIMARY KEY,
        key TEXT NOT NULL UNIQUE,
        link TEXT NOT NULL REFERENCES device (link) ON DELETE CASCADE,
        device TEXT NOT NULL,
        agent TEXT NOT NULL,
        title TEXT NOT NULL,
        change TEXT NOT NULL,
        queued_at INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX outbox_device ON outbox (device, id);",
        ),
        // A device's routes after its first, which stays in `device.host`,
        // and the route that last reached it on each network (N-7).
        M::up(
            "CREATE TABLE route (
        link TEXT NOT NULL REFERENCES device (link) ON DELETE CASCADE,
        pos INTEGER NOT NULL,
        host TEXT NOT NULL,
        PRIMARY KEY (link, pos)
    ) STRICT;
    CREATE TABLE last_good (
        link TEXT NOT NULL REFERENCES device (link) ON DELETE CASCADE,
        net TEXT NOT NULL,
        host TEXT NOT NULL,
        at INTEGER NOT NULL,
        PRIMARY KEY (link, net)
    ) STRICT;",
        ),
        // The app's settings, each one JSON under its name.
        M::up(
            "CREATE TABLE setting (
        name TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    ) STRICT;",
        ),
        // The device each link first reached, which is the only one it
        // reaches from then on. Removing the device forgets it.
        M::up(
            "CREATE TABLE link_device (
        link TEXT PRIMARY KEY NOT NULL REFERENCES device (link) ON DELETE CASCADE,
        device TEXT NOT NULL
    ) STRICT;",
        ),
        // When each link first reached its device, and the links whose
        // hello named a device another link knows, held until you answer.
        // Links with no device yet take the one the app's records already
        // tie them to, so most never ask.
        M::up_with_hook(
            "ALTER TABLE link_device ADD COLUMN at INTEGER;
    CREATE TABLE link_claim (
        link TEXT PRIMARY KEY NOT NULL REFERENCES device (link) ON DELETE CASCADE,
        device TEXT NOT NULL,
        known_by TEXT,
        different INTEGER NOT NULL DEFAULT 0 CHECK (different IN (0, 1)),
        at INTEGER NOT NULL
    ) STRICT;",
            seed::link_devices,
        ),
        // Two Macs' networks can each have a device at 192.168.1.20, so a
        // first route no longer has to be one device's alone. SQLite drops
        // a UNIQUE only by building the table again, and dropping the old
        // one empties the tables that point at it, so their rows wait in
        // copies. The phone's one book becomes the first of its books.
        M::up(
            "CREATE TABLE device_next (
        link TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('local', 'ssh')),
        host TEXT,
        added_at INTEGER NOT NULL,
        CHECK ((kind = 'local') = (host IS NULL))
    ) STRICT;
    INSERT INTO device_next (rowid, link, name, kind, host, added_at)
        SELECT rowid, link, name, kind, host, added_at FROM device;
    CREATE TEMP TABLE outbox_kept AS SELECT * FROM outbox;
    CREATE TEMP TABLE route_kept AS SELECT * FROM route;
    CREATE TEMP TABLE last_good_kept AS SELECT * FROM last_good;
    CREATE TEMP TABLE link_device_kept AS SELECT * FROM link_device;
    CREATE TEMP TABLE link_claim_kept AS SELECT * FROM link_claim;
    DROP TABLE device;
    ALTER TABLE device_next RENAME TO device;
    CREATE UNIQUE INDEX device_one_local ON device (kind) WHERE kind = 'local';
    INSERT INTO outbox SELECT * FROM temp.outbox_kept;
    INSERT INTO route SELECT * FROM temp.route_kept;
    INSERT INTO last_good SELECT * FROM temp.last_good_kept;
    INSERT INTO link_device SELECT * FROM temp.link_device_kept;
    INSERT INTO link_claim SELECT * FROM temp.link_claim_kept;
    DROP TABLE temp.outbox_kept;
    DROP TABLE temp.route_kept;
    DROP TABLE temp.last_good_kept;
    DROP TABLE temp.link_device_kept;
    DROP TABLE temp.link_claim_kept;
    INSERT INTO setting (name, value)
        SELECT 'phone_books', '[' || value || ']' FROM setting WHERE name = 'phone_book';
    DELETE FROM setting WHERE name = 'phone_book';",
        )
        .foreign_key_check(),
    ]
}

/// The link for this machine's `maxd`. It's derived, so it's the same in
/// every run, even one that couldn't open the database.
pub fn local_link() -> LinkId {
    LinkId::derive("local")
}

/// The setting the phone's device books are kept under, one per Mac.
const PHONE_BOOKS: &str = "phone_books";

/// The setting the revokes waiting for a device are kept under.
const PENDING_REVOKES: &str = "pending_revokes";

/// What the local device is called until you rename it.
const LOCAL_NAME: &str = if cfg!(target_os = "macos") {
    "This Mac"
} else {
    "This computer"
};

/// Why a registry change didn't happen.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The database refused it.
    #[error("couldn't save the device list: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// No device has this link.
    #[error("no such device")]
    NotFound,
    /// The host can't be an SSH destination.
    #[error("{0}")]
    BadHost(&'static str),
    /// A name has to have something in it.
    #[error("a device needs a name")]
    EmptyName,
    /// This machine's `maxd` can't be removed.
    #[error("this computer can't be removed")]
    Local,
}

/// An inbox change waiting for its device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    /// The key it goes out with, every time, so a resend can't apply it
    /// twice.
    pub key: IdempotencyKey,
    /// The link it was queued through.
    pub link: LinkId,
    /// The device that keeps the agent's inbox.
    pub device: DeviceId,
    pub agent: AgentId,
    /// What the agent was called when the change was made, for a message
    /// about it once the agent is gone.
    pub title: String,
    pub change: InboxChange,
    /// When it was made, which is the time it applies at.
    pub at: Timestamp,
}

/// The devices you added.
pub struct Registry {
    conn: Connection,
}

impl Registry {
    /// Opens the registry at `path`, creating it with this machine in it.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        Self::start(db::open(path, &migrations())?)
    }

    /// A registry that lasts as long as the app, for tests and for when the
    /// file won't open.
    pub fn in_memory() -> Result<Self, OpenError> {
        Self::start(db::open_in_memory(&migrations())?)
    }

    /// Opens the phone's registry at `path`. It has no device until the
    /// phone pairs and gets its book.
    pub fn open_phone(path: &Path) -> Result<Self, OpenError> {
        Ok(Self {
            conn: db::open(path, &migrations())?,
        })
    }

    /// A phone's registry that lasts as long as the app.
    pub fn in_memory_phone() -> Result<Self, OpenError> {
        Ok(Self {
            conn: db::open_in_memory(&migrations())?,
        })
    }

    fn start(conn: Connection) -> Result<Self, OpenError> {
        conn.execute(
            "INSERT INTO device (link, name, kind, host, added_at)
             VALUES (?1, ?2, 'local', NULL, 0)
             ON CONFLICT DO NOTHING",
            params![local_link().to_string(), LOCAL_NAME],
        )?;
        Ok(Self { conn })
    }

    /// Every device, this machine first and then in the order you added
    /// them.
    pub fn list(&self) -> Result<Vec<LinkSpec>, RegistryError> {
        let mut statement = self.conn.prepare(
            "SELECT link, name, host FROM device
             ORDER BY kind = 'ssh', added_at, rowid",
        )?;
        let rows = statement.query_map([], |row| {
            let link: String = row.get(0)?;
            let name: String = row.get(1)?;
            let host: Option<String> = row.get(2)?;
            Ok((link, name, host))
        })?;
        let mut devices = Vec::new();
        for row in rows {
            let (link, name, host) = row?;
            // A row this build can't read is skipped rather than failing
            // the whole list. Only this module writes the table.
            let Ok(id) = link.parse() else {
                tracing::warn!(link, "skipping a device with a bad link ID");
                continue;
            };
            let route = match host {
                Some(host) => Route::Ssh { host },
                None => Route::Local,
            };
            devices.push(LinkSpec {
                id,
                name,
                route,
                fallbacks: Vec::new(),
                last_good: BTreeMap::new(),
            });
        }
        for device in &mut devices {
            device.fallbacks = self.fallbacks(&link_key(device.id))?;
            device.last_good = self.last_good(&link_key(device.id))?;
        }
        Ok(devices)
    }

    fn fallbacks(&self, link: &str) -> Result<Vec<Route>, RegistryError> {
        let mut statement = self
            .conn
            .prepare("SELECT host FROM route WHERE link = ?1 ORDER BY pos")?;
        let hosts = statement.query_map(params![link], |row| row.get::<_, String>(0))?;
        hosts.map(|host| Ok(Route::Ssh { host: host? })).collect()
    }

    fn last_good(&self, link: &str) -> Result<BTreeMap<String, Route>, RegistryError> {
        let mut statement = self
            .conn
            .prepare("SELECT net, host FROM last_good WHERE link = ?1")?;
        let rows = statement.query_map(params![link], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (net, host) = row?;
            Ok((net, Route::Ssh { host }))
        })
        .collect()
    }

    /// Sets a device's routes, in the order they're tried. The first is the
    /// device's host, and each is checked as [`Registry::add`] checks one.
    /// A route dropped from the list is forgotten where it last worked.
    pub fn set_routes(&self, link: LinkId, hosts: &[String]) -> Result<LinkSpec, RegistryError> {
        if link == local_link() {
            return Err(RegistryError::Local);
        }
        let mut checked: Vec<&str> = Vec::with_capacity(hosts.len());
        for host in hosts {
            let host = check_host(host)?;
            if !checked.contains(&host) {
                checked.push(host);
            }
        }
        let Some((first, rest)) = checked.split_first() else {
            return Err(RegistryError::BadHost("a device needs at least one route"));
        };
        let key = link_key(link);
        let taken: Option<String> = self
            .conn
            .query_row(
                "SELECT link FROM device WHERE host = ?1 AND link != ?2",
                params![first, key],
                |row| row.get(0),
            )
            .optional()?;
        if taken.is_some() {
            return Err(RegistryError::BadHost(
                "another device already starts with that route",
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE device SET host = ?2 WHERE link = ?1 AND kind = 'ssh'",
            params![key, first],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound);
        }
        tx.execute("DELETE FROM route WHERE link = ?1", params![key])?;
        for (pos, host) in rest.iter().enumerate() {
            let pos = i64::try_from(pos).unwrap_or(i64::MAX);
            tx.execute(
                "INSERT INTO route (link, pos, host) VALUES (?1, ?2, ?3)",
                params![key, pos, host],
            )?;
        }
        let mut keep = String::new();
        for (index, _) in checked.iter().enumerate() {
            if index > 0 {
                keep.push(',');
            }
            keep.push_str(&format!("?{}", index + 2));
        }
        let forget = format!("DELETE FROM last_good WHERE link = ?1 AND host NOT IN ({keep})");
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&key];
        values.extend(checked.iter().map(|host| host as &dyn rusqlite::ToSql));
        tx.execute(&forget, values.as_slice())?;
        tx.commit()?;
        self.get(link)
    }

    /// Notes that `host` reached the device on network `net`, so it's
    /// tried first there next time. A host that isn't one of the device's
    /// routes is ignored.
    pub fn record_good(&self, link: LinkId, net: &str, host: &str) -> Result<(), RegistryError> {
        let device = self.get(link)?;
        let route = Route::Ssh {
            host: host.to_owned(),
        };
        if device.route != route && !device.fallbacks.contains(&route) {
            return Ok(());
        }
        let at = i64::try_from(Timestamp::now().0).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO last_good (link, net, host, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (link, net) DO UPDATE SET host = excluded.host, at = excluded.at",
            params![link_key(link), net, host, at],
        )?;
        Ok(())
    }

    /// The device with this link.
    pub fn get(&self, link: LinkId) -> Result<LinkSpec, RegistryError> {
        self.list()?
            .into_iter()
            .find(|device| device.id == link)
            .ok_or(RegistryError::NotFound)
    }

    /// Adds an SSH host, named after the host until you rename it. Adding
    /// a host that's already there returns the device you have.
    pub fn add(&self, host: &str) -> Result<LinkSpec, RegistryError> {
        let host = check_host(host)?;
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT link FROM device WHERE host = ?1",
                params![host],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(link) = existing.and_then(|link| link.parse().ok()) {
            return self.get(link);
        }
        let device = LinkSpec {
            id: LinkId::generate(),
            name: host.to_owned(),
            route: Route::Ssh {
                host: host.to_owned(),
            },
            fallbacks: Vec::new(),
            last_good: BTreeMap::new(),
        };
        let added_at = i64::try_from(Timestamp::now().0).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO device (link, name, kind, host, added_at)
             VALUES (?1, ?2, 'ssh', ?3, ?4)",
            params![device.id.to_string(), device.name, host, added_at],
        )?;
        Ok(device)
    }

    /// Renames a device. Spaces around the name are dropped.
    pub fn rename(&self, link: LinkId, name: &str) -> Result<LinkSpec, RegistryError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RegistryError::EmptyName);
        }
        let changed = self.conn.execute(
            "UPDATE device SET name = ?2 WHERE link = ?1",
            params![link.to_string(), name],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound);
        }
        self.get(link)
    }

    /// Forgets a device. This machine stays.
    pub fn remove(&self, link: LinkId) -> Result<(), RegistryError> {
        if link == local_link() {
            return Err(RegistryError::Local);
        }
        let changed = self.conn.execute(
            "DELETE FROM device WHERE link = ?1 AND kind = 'ssh'",
            params![link.to_string()],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound);
        }
        Ok(())
    }

    /// The only device `link` may reach, or None while any may: one the
    /// phone's book names, or else the first to say hello on it. A hello
    /// is free to say any ID, so this keeps a device from taking another's
    /// place. This machine has none, since it can't be removed to trust a
    /// new ID.
    pub fn known_device(&self, link: LinkId) -> Result<Option<DeviceId>, RegistryError> {
        let books = self.phone_books()?;
        let booked = phone::listed(&books)
            .into_iter()
            .find(|(_, entry)| phone::book_link(entry.device) == link)
            .map(|(_, entry)| entry.device);
        if booked.is_some() {
            return Ok(booked);
        }
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT device FROM link_device WHERE link = ?1",
                params![link_key(link)],
                |row| row.get(0),
            )
            .optional()?;
        Ok(stored.and_then(|device| device.parse().ok()))
    }

    /// Keeps the device that said hello on `link`, unless the link has one,
    /// and lets go of a question about it.
    pub fn keep_device(&self, link: LinkId, device: DeviceId) -> Result<(), RegistryError> {
        self.conn.execute(
            "INSERT INTO link_device (link, device, at)
             SELECT link, ?2, ?3 FROM device WHERE link = ?1 AND kind = 'ssh'
             ON CONFLICT (link) DO NOTHING",
            params![link_key(link), device.to_string(), claims::now()],
        )?;
        self.conn.execute(
            "DELETE FROM link_claim WHERE link = ?1",
            params![link_key(link)],
        )?;
        Ok(())
    }

    /// Keeps a change until its device takes it. A change already queued
    /// under the same key stays as it is.
    pub fn queue(&self, queued: &Queued) -> Result<(), RegistryError> {
        let change = serde_json::to_string(&queued.change)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        let at = i64::try_from(queued.at.0).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO outbox (key, link, device, agent, title, change, queued_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (key) DO NOTHING",
            params![
                queued.key.to_string(),
                queued.link.to_string(),
                queued.device.to_string(),
                queued.agent.to_string(),
                queued.title,
                change,
                at,
            ],
        )?;
        Ok(())
    }

    /// Every queued change, in the order they were made.
    pub fn queued(&self) -> Result<Vec<Queued>, RegistryError> {
        let mut statement = self.conn.prepare(
            "SELECT key, link, device, agent, title, change, queued_at FROM outbox
             ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut queued = Vec::new();
        for row in rows {
            let (key, link, device, agent, title, change, at) = row?;
            // As with devices, a row this build can't read is skipped. It
            // stays in the table until its device is removed.
            let read = || {
                Some(Queued {
                    key: key.parse().ok()?,
                    link: link.parse().ok()?,
                    device: device.parse().ok()?,
                    agent: agent.parse().ok()?,
                    title: title.clone(),
                    change: serde_json::from_str(&change).ok()?,
                    at: Timestamp(u64::try_from(at).ok()?),
                })
            };
            match read() {
                Some(change) => queued.push(change),
                None => tracing::warn!(key, "skipping a queued change this build can't read"),
            }
        }
        Ok(queued)
    }

    /// Drops a queued change once its device has answered it.
    pub fn unqueue(&self, key: IdempotencyKey) -> Result<(), RegistryError> {
        self.conn.execute(
            "DELETE FROM outbox WHERE key = ?1",
            params![key.to_string()],
        )?;
        Ok(())
    }

    /// The device books the phone paired with, one per Mac, in the order
    /// it paired.
    pub fn phone_books(&self) -> Result<Vec<PhoneBook>, RegistryError> {
        Ok(self.setting(PHONE_BOOKS)?.unwrap_or_default())
    }

    /// Makes the books' devices the ones the phone has, in their order, and
    /// keeps the books for their host keys. A device more than one book
    /// lists comes once, from the first. A device that left every book is
    /// forgotten, with what waited for it. One with no routes is left out.
    /// Each keeps its link from one book to the next, and the routes that
    /// last worked for it while they're still in its book. A device's
    /// network access comes before its SSH routes.
    pub fn set_phone_books(&self, books: &[PhoneBook]) -> Result<(), RegistryError> {
        let wanted: Vec<(LinkId, &str, Vec<String>)> = phone::listed(books)
            .into_iter()
            .map(|(_, entry)| {
                let hosts = phone::route_hosts(entry);
                (phone::book_link(entry.device), entry.name.as_str(), hosts)
            })
            .filter(|(_, _, hosts)| !hosts.is_empty())
            .collect();
        let tx = self.conn.unchecked_transaction()?;
        let keep: Vec<String> = wanted.iter().map(|(link, ..)| link_key(*link)).collect();
        let stored: Vec<String> = {
            let mut statement = tx.prepare("SELECT link FROM device WHERE kind = 'ssh'")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for link in stored.iter().filter(|link| !keep.contains(link)) {
            tx.execute("DELETE FROM device WHERE link = ?1", params![link])?;
        }
        for (pos, (link, name, hosts)) in wanted.iter().enumerate() {
            let key = link_key(*link);
            let pos = i64::try_from(pos).unwrap_or(i64::MAX);
            tx.execute(
                "INSERT INTO device (link, name, kind, host, added_at)
                 VALUES (?1, ?2, 'ssh', ?3, ?4)
                 ON CONFLICT (link) DO UPDATE SET
                     name = excluded.name, host = excluded.host, added_at = excluded.added_at",
                params![key, name, hosts[0], pos],
            )?;
            tx.execute("DELETE FROM route WHERE link = ?1", params![key])?;
            for (pos, host) in hosts.iter().enumerate().skip(1) {
                let pos = i64::try_from(pos - 1).unwrap_or(i64::MAX);
                tx.execute(
                    "INSERT INTO route (link, pos, host) VALUES (?1, ?2, ?3)",
                    params![key, pos, host],
                )?;
            }
            let mut statement = tx.prepare("SELECT host FROM last_good WHERE link = ?1")?;
            let good: Vec<String> = statement
                .query_map(params![key], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            for host in good.iter().filter(|host| !hosts.contains(host)) {
                tx.execute(
                    "DELETE FROM last_good WHERE link = ?1 AND host = ?2",
                    params![key, host],
                )?;
            }
        }
        // The same connection, so inside the transaction.
        self.set_setting(PHONE_BOOKS, &books)?;
        tx.commit()?;
        Ok(())
    }

    /// Moves the changes queued through one link to another, before the
    /// first is removed while the second still reaches their device.
    pub fn hand_over(&self, from: LinkId, to: LinkId) -> Result<(), RegistryError> {
        self.conn.execute(
            "UPDATE outbox SET link = ?2 WHERE link = ?1",
            params![from.to_string(), to.to_string()],
        )?;
        Ok(())
    }

    /// The setting saved under `name`, or None when there's none or it no
    /// longer reads as a `T`.
    pub fn setting<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, RegistryError> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM setting WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.and_then(|value| serde_json::from_str(&value).ok()))
    }

    /// Saves a setting under `name`, in place of the one there.
    pub fn set_setting<T: Serialize>(&self, name: &str, value: &T) -> Result<(), RegistryError> {
        let value = serde_json::to_string(value)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        self.conn.execute(
            "INSERT INTO setting (name, value) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
            params![name, value],
        )?;
        Ok(())
    }

    /// The permissions waiting to come off a device that wasn't reached
    /// when they were revoked, oldest first.
    pub fn pending_revokes(&self) -> Result<Vec<(DeviceId, PermissionId)>, RegistryError> {
        Ok(self.setting(PENDING_REVOKES)?.unwrap_or_default())
    }

    /// Keeps a revoke for `device` until it answers.
    pub fn revoke_later(
        &self,
        device: DeviceId,
        permission: PermissionId,
    ) -> Result<(), RegistryError> {
        let mut pending = self.pending_revokes()?;
        if pending.contains(&(device, permission)) {
            return Ok(());
        }
        pending.push((device, permission));
        self.set_setting(PENDING_REVOKES, &pending)
    }

    /// Lets go of a revoke waiting for `device`, once it has it or holds
    /// the permission again.
    pub fn forget_revoke(
        &self,
        device: DeviceId,
        permission: PermissionId,
    ) -> Result<(), RegistryError> {
        let mut pending = self.pending_revokes()?;
        let before = pending.len();
        pending.retain(|waiting| *waiting != (device, permission));
        if pending.len() == before {
            return Ok(());
        }
        self.set_setting(PENDING_REVOKES, &pending)
    }
}

/// How a link is stored.
fn link_key(link: LinkId) -> String {
    link.to_string()
}

/// Checks that `host` is one SSH destination and returns it trimmed. A
/// leading dash would read as an option to `ssh`, so it's refused.
fn check_host(host: &str) -> Result<&str, RegistryError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(RegistryError::BadHost("enter an SSH host"));
    }
    if host.starts_with('-') {
        return Err(RegistryError::BadHost(
            "an SSH host can't start with a dash",
        ));
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(RegistryError::BadHost(
            "an SSH host is one word, with no spaces",
        ));
    }
    if !maxhq_client::is_safe_destination(host) {
        return Err(RegistryError::BadHost(
            "an SSH host can't start with a dash or hold characters like $, ; or quotes",
        ));
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(registry: &Registry) -> Vec<Route> {
        registry
            .list()
            .unwrap()
            .into_iter()
            .map(|device| device.route)
            .collect()
    }

    #[test]
    fn the_schema_is_valid() {
        assert!(migrations().validate().is_ok());
    }

    #[test]
    fn this_machine_is_always_there_and_stays() {
        let registry = Registry::in_memory().unwrap();
        let devices = registry.list().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, local_link());
        assert_eq!(devices[0].route, Route::Local);
        assert!(matches!(
            registry.remove(local_link()),
            Err(RegistryError::Local)
        ));
        let renamed = registry.rename(local_link(), "  Studio  ").unwrap();
        assert_eq!(renamed.name, "Studio");
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[test]
    fn adding_a_host_twice_keeps_one_device() {
        let registry = Registry::in_memory().unwrap();
        let first = registry.add("dev@devbox").unwrap();
        assert_eq!(first.name, "dev@devbox");
        let again = registry.add(" dev@devbox ").unwrap();
        assert_eq!(again.id, first.id);
        let other = registry.add("cachy").unwrap();
        assert_ne!(other.id, first.id);
        assert_eq!(
            hosts(&registry),
            [
                Route::Local,
                Route::Ssh {
                    host: "dev@devbox".into()
                },
                Route::Ssh {
                    host: "cachy".into()
                },
            ]
        );
    }

    #[test]
    fn hosts_that_arent_one_destination_are_refused() {
        let registry = Registry::in_memory().unwrap();
        for bad in [
            "",
            "   ",
            "-oProxyCommand=sh",
            "ssh://-oProxyCommand=sh",
            "a b",
            "a\nb",
            "a\u{7}b",
            "a$b",
            "a`b",
            "a;b",
            "a'b",
        ] {
            assert!(
                matches!(registry.add(bad), Err(RegistryError::BadHost(_))),
                "{bad:?} was accepted"
            );
        }
        assert_eq!(registry.list().unwrap().len(), 1);
    }

    #[test]
    fn unusual_but_real_hosts_are_kept() {
        let registry = Registry::in_memory().unwrap();
        for host in ["fe80::1%en0", "dev@devbox", "[fd7a::1]:22", "ssh://edge"] {
            assert!(registry.add(host).is_ok(), "{host:?} was refused");
        }
    }

    #[test]
    fn renames_and_removals_need_a_device() {
        let registry = Registry::in_memory().unwrap();
        let device = registry.add("devbox").unwrap();
        assert!(matches!(
            registry.rename(device.id, " "),
            Err(RegistryError::EmptyName)
        ));
        assert_eq!(registry.rename(device.id, "NL").unwrap().name, "NL");
        registry.remove(device.id).unwrap();
        assert!(matches!(
            registry.remove(device.id),
            Err(RegistryError::NotFound)
        ));
        assert!(matches!(
            registry.rename(device.id, "NL"),
            Err(RegistryError::NotFound)
        ));
        assert!(matches!(
            registry.get(device.id),
            Err(RegistryError::NotFound)
        ));
    }

    fn queued(link: LinkId, agent: &str, change: InboxChange, at: u64) -> Queued {
        Queued {
            key: IdempotencyKey::generate(),
            link,
            device: DeviceId::derive("nl"),
            agent: AgentId::derive(agent),
            title: format!("{agent} title"),
            change,
            at: Timestamp(at),
        }
    }

    #[test]
    fn queued_changes_keep_their_order_and_key() {
        let registry = Registry::in_memory().unwrap();
        let device = registry.add("devbox").unwrap();
        let settle = queued(device.id, "a", InboxChange::Settle, 1);
        let pin = queued(device.id, "b", InboxChange::Pin { key: "a0".into() }, 2);
        registry.queue(&settle).unwrap();
        registry.queue(&pin).unwrap();
        registry.queue(&settle).unwrap();
        assert_eq!(registry.queued().unwrap(), [settle.clone(), pin.clone()]);

        registry.unqueue(settle.key).unwrap();
        assert_eq!(registry.queued().unwrap(), [pin]);
    }

    #[test]
    fn removing_a_device_drops_its_queued_changes() {
        let registry = Registry::in_memory().unwrap();
        let first = registry.add("devbox").unwrap();
        let second = registry.add("devbox.lan").unwrap();
        let kept = queued(second.id, "b", InboxChange::Unpin, 2);
        registry
            .queue(&queued(first.id, "a", InboxChange::Settle, 1))
            .unwrap();
        registry.queue(&kept).unwrap();
        registry.remove(first.id).unwrap();
        assert_eq!(registry.queued().unwrap(), std::slice::from_ref(&kept));

        // Handed over first, they stay.
        registry.hand_over(second.id, local_link()).unwrap();
        registry.remove(second.id).unwrap();
        let left = registry.queued().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].link, local_link());
        assert_eq!(left[0].key, kept.key);
    }

    #[test]
    fn a_link_keeps_the_first_device_to_say_hello() {
        let registry = Registry::in_memory().unwrap();
        let spec = registry.add("devbox").unwrap();
        let (first, other) = (DeviceId::derive("first"), DeviceId::derive("other"));
        assert_eq!(registry.known_device(spec.id).unwrap(), None);

        registry.keep_device(spec.id, first).unwrap();
        registry.keep_device(spec.id, other).unwrap();
        assert_eq!(registry.known_device(spec.id).unwrap(), Some(first));

        // A new route is still the same device.
        registry
            .set_routes(spec.id, &["devbox.lan".to_owned()])
            .unwrap();
        assert_eq!(registry.known_device(spec.id).unwrap(), Some(first));
    }

    #[test]
    fn removing_a_device_forgets_its_id_and_adding_it_trusts_a_new_one() {
        let registry = Registry::in_memory().unwrap();
        let spec = registry.add("devbox").unwrap();
        registry
            .keep_device(spec.id, DeviceId::derive("old"))
            .unwrap();
        registry.remove(spec.id).unwrap();
        // A link that's gone keeps nothing, even if a late hello comes in.
        registry
            .keep_device(spec.id, DeviceId::derive("late"))
            .unwrap();
        assert_eq!(registry.known_device(spec.id).unwrap(), None);

        let again = registry.add("devbox").unwrap();
        assert_eq!(registry.known_device(again.id).unwrap(), None);
        let new = DeviceId::derive("new");
        registry.keep_device(again.id, new).unwrap();
        assert_eq!(registry.known_device(again.id).unwrap(), Some(new));
    }

    #[test]
    fn this_machine_keeps_no_device_since_it_cant_be_removed() {
        let registry = Registry::in_memory().unwrap();
        registry
            .keep_device(local_link(), DeviceId::derive("mac"))
            .unwrap();
        assert_eq!(registry.known_device(local_link()).unwrap(), None);
    }

    #[test]
    fn the_phone_knows_its_devices_from_the_book() {
        use crate::phone::book_link;
        use crate::phone::tests::{book, entry};

        let registry = Registry::in_memory_phone().unwrap();
        let devbox = DeviceId::derive("devbox");
        assert_eq!(registry.known_device(book_link(devbox)).unwrap(), None);

        registry
            .set_phone_books(&[book(vec![entry("devbox", &["devbox.local"])])])
            .unwrap();
        // No hello needed, and none can change it.
        assert_eq!(
            registry.known_device(book_link(devbox)).unwrap(),
            Some(devbox)
        );
        registry
            .keep_device(book_link(devbox), DeviceId::derive("other"))
            .unwrap();
        assert_eq!(
            registry.known_device(book_link(devbox)).unwrap(),
            Some(devbox)
        );

        registry.set_phone_books(&[]).unwrap();
        assert_eq!(registry.known_device(book_link(devbox)).unwrap(), None);
    }

    #[test]
    fn devices_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("maxhq-registry-{}", LinkId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("maxhq.db");
        let added = {
            let registry = Registry::open(&path).unwrap();
            let added = registry.add("devbox").unwrap();
            registry.rename(added.id, "NL").unwrap()
        };
        let registry = Registry::open(&path).unwrap();
        let devices = registry.list().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[1], added);
        let change = queued(added.id, "a", InboxChange::Settle, 1);
        registry.queue(&change).unwrap();
        drop(registry);
        let registry = Registry::open(&path).unwrap();
        assert_eq!(registry.queued().unwrap(), [change]);
        drop(registry);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn routes(spec: &LinkSpec) -> Vec<Route> {
        std::iter::once(spec.route.clone())
            .chain(spec.fallbacks.iter().cloned())
            .collect()
    }

    fn strings(hosts: &[&str]) -> Vec<String> {
        hosts.iter().map(|host| (*host).to_owned()).collect()
    }

    #[test]
    fn routes_keep_your_order_and_the_first_is_the_host() {
        let registry = Registry::in_memory().unwrap();
        let device = registry.add("dev@devbox").unwrap();
        assert!(device.fallbacks.is_empty());
        let set = registry
            .set_routes(
                device.id,
                &strings(&["dev@devbox-lan", " dev@devbox ", "dev@devbox-lan"]),
            )
            .unwrap();
        assert_eq!(
            routes(&set),
            [
                Route::Ssh {
                    host: "dev@devbox-lan".into()
                },
                Route::Ssh {
                    host: "dev@devbox".into()
                },
            ]
        );
        assert_eq!(registry.get(device.id).unwrap(), set);
    }

    #[test]
    fn routes_are_checked_like_hosts() {
        let registry = Registry::in_memory().unwrap();
        let device = registry.add("devbox").unwrap();
        let other = registry.add("cachy").unwrap();
        for bad in [vec![], vec!["devbox", "-oProxyCommand=sh"], vec!["a b"]] {
            assert!(
                matches!(
                    registry.set_routes(device.id, &strings(&bad)),
                    Err(RegistryError::BadHost(_))
                ),
                "{bad:?} was accepted"
            );
        }
        assert!(matches!(
            registry.set_routes(device.id, &strings(&["cachy"])),
            Err(RegistryError::BadHost(_))
        ));
        assert!(matches!(
            registry.set_routes(local_link(), &strings(&["devbox"])),
            Err(RegistryError::Local)
        ));
        assert_eq!(routes(&registry.get(device.id).unwrap()).len(), 1);
        assert_eq!(
            registry.get(other.id).unwrap().route,
            Route::Ssh {
                host: "cachy".into()
            }
        );
    }

    #[test]
    fn the_route_that_worked_is_kept_per_network_until_its_route_goes() {
        let registry = Registry::in_memory().unwrap();
        let device = registry.add("devbox").unwrap();
        registry
            .set_routes(device.id, &strings(&["devbox", "devbox-tail"]))
            .unwrap();
        registry.record_good(device.id, "home", "devbox").unwrap();
        registry.record_good(device.id, "cafe", "devbox").unwrap();
        registry
            .record_good(device.id, "cafe", "devbox-tail")
            .unwrap();
        // A host that isn't a route is ignored.
        registry
            .record_good(device.id, "work", "elsewhere")
            .unwrap();
        let kept = registry.get(device.id).unwrap().last_good;
        assert_eq!(
            kept,
            BTreeMap::from([
                (
                    "cafe".to_owned(),
                    Route::Ssh {
                        host: "devbox-tail".into()
                    }
                ),
                (
                    "home".to_owned(),
                    Route::Ssh {
                        host: "devbox".into()
                    }
                ),
            ])
        );
        registry
            .set_routes(device.id, &strings(&["devbox"]))
            .unwrap();
        let kept = registry.get(device.id).unwrap().last_good;
        assert_eq!(
            kept,
            BTreeMap::from([(
                "home".to_owned(),
                Route::Ssh {
                    host: "devbox".into()
                }
            )])
        );
        registry.remove(device.id).unwrap();
        let rows: i64 = registry
            .conn
            .query_row("SELECT count(*) FROM last_good", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0, "removing the device drops what it kept");
    }

    #[test]
    fn a_database_from_before_routes_opens_with_its_devices() {
        let dir = std::env::temp_dir().join(format!("maxhq-registry-{}", LinkId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("maxhq.db");
        {
            let mut conn = Connection::open(&path).unwrap();
            let mut old = steps();
            old.truncate(2);
            Migrations::new(old).to_latest(&mut conn).unwrap();
            conn.execute(
                "INSERT INTO device (link, name, kind, host, added_at)
                 VALUES (?1, 'NL', 'ssh', 'dev@devbox', 1)",
                params![LinkId::generate().to_string()],
            )
            .unwrap();
        }
        let registry = Registry::open(&path).unwrap();
        let devices = registry.list().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[1].name, "NL");
        assert!(devices[1].fallbacks.is_empty());
        assert!(devices[1].last_good.is_empty());
        registry
            .set_routes(devices[1].id, &strings(&["dev@devbox", "dev@devbox-tail"]))
            .unwrap();
        drop(registry);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_setting_reads_back_as_saved_and_unreadable_as_none() {
        let registry = Registry::in_memory().unwrap();
        assert_eq!(registry.setting::<u8>("depth").unwrap(), None);
        registry.set_setting("depth", &2u8).unwrap();
        registry.set_setting("depth", &4u8).unwrap();
        assert_eq!(registry.setting::<u8>("depth").unwrap(), Some(4));
        assert_eq!(registry.setting::<String>("depth").unwrap(), None);
    }

    #[test]
    fn the_phone_takes_its_devices_from_the_book() {
        use crate::phone::book_link;
        use crate::phone::tests::{book, entry};

        let ssh = |host: &str| Route::Ssh {
            host: format!("ssh://dev@{host}:22"),
        };
        let registry = Registry::in_memory_phone().unwrap();
        assert!(registry.list().unwrap().is_empty());
        assert!(registry.phone_books().unwrap().is_empty());

        let first = book(vec![
            entry("devbox", &["devbox.local", "192.0.2.7", "devbox.local"]),
            entry("buildbox", &["buildbox.local"]),
            entry("empty", &[]),
        ]);
        registry
            .set_phone_books(std::slice::from_ref(&first))
            .unwrap();
        let devices = registry.list().unwrap();
        let names: Vec<&str> = devices.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, ["devbox", "buildbox"]);
        let devbox = book_link(DeviceId::derive("devbox"));
        assert_eq!(devices[0].id, devbox);
        assert_eq!(devices[0].route, ssh("devbox.local"));
        assert_eq!(devices[0].fallbacks, [ssh("192.0.2.7")]);
        assert_eq!(registry.phone_books().unwrap(), [first]);

        // The last route that worked stays only while the book has it.
        let Route::Ssh { host: lan } = ssh("192.0.2.7") else {
            unreachable!()
        };
        registry.record_good(devbox, "home", &lan).unwrap();
        registry.record_good(devbox, "away", &lan).unwrap();

        // buildbox takes devbox's old host, devbox moves and goes second.
        let second = book(vec![
            entry("buildbox", &["devbox.local"]),
            entry("devbox", &["192.0.2.7"]),
        ]);
        registry.set_phone_books(&[second]).unwrap();
        assert_eq!(hosts(&registry), [ssh("devbox.local"), ssh("192.0.2.7")]);
        let kept = registry.get(devbox).unwrap();
        assert_eq!(kept.last_good.get("home"), Some(&ssh("192.0.2.7")));

        registry
            .set_phone_books(&[book(vec![entry("devbox", &["devbox.lan"])])])
            .unwrap();
        let only = registry.list().unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].route, ssh("devbox.lan"));
        assert!(only[0].last_good.is_empty());
    }

    #[test]
    fn a_devices_network_access_comes_before_its_ssh_routes() {
        use maxhq_protocol::netaccess::PhoneNetwork;

        use crate::phone::tests::{book, entry};

        let network = |hosts: &[&str]| {
            Some(PhoneNetwork {
                hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                port: 7466,
                pin: "pin".into(),
                code: None,
            })
        };
        let host = |host: &str| Route::Ssh {
            host: host.to_owned(),
        };
        let mut devbox = entry("devbox", &["devbox.local"]);
        devbox.network = network(&["192.168.1.20", "fd00::7"]);
        // A Mac with no SSH is reached over its network access alone.
        let mut mac = entry("mac", &[]);
        mac.network = network(&["192.168.1.30"]);
        let registry = Registry::in_memory_phone().unwrap();
        registry
            .set_phone_books(&[book(vec![devbox, mac])])
            .unwrap();
        let devices = registry.list().unwrap();
        assert_eq!(devices[0].route, host("wss://192.168.1.20:7466"));
        assert_eq!(
            devices[0].fallbacks,
            [
                host("wss://[fd00::7]:7466"),
                host("ssh://dev@devbox.local:22")
            ]
        );
        assert_eq!(devices[1].name, "mac");
        assert_eq!(devices[1].route, host("wss://192.168.1.30:7466"));
        assert!(devices[1].fallbacks.is_empty());
    }

    #[test]
    fn two_macs_can_each_have_a_device_at_one_private_address() {
        use crate::phone::book_link;
        use crate::phone::tests::{entry, mac_book};

        let registry = Registry::in_memory_phone().unwrap();
        let home = mac_book("Home Mac", vec![entry("devbox", &["192.168.1.20"])]);
        let work = mac_book(
            "Work Mac",
            vec![
                entry("build-box", &["192.168.1.20"]),
                // Both Macs list devbox, so it comes once, from Home Mac.
                entry("devbox", &["devbox.work"]),
            ],
        );
        registry
            .set_phone_books(&[home.clone(), work.clone()])
            .unwrap();
        let devices = registry.list().unwrap();
        let names: Vec<&str> = devices.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, ["Home Mac", "devbox", "Work Mac", "build-box"]);
        let lan = Route::Ssh {
            host: "ssh://dev@192.168.1.20:22".into(),
        };
        assert_eq!(devices[1].route, lan);
        assert_eq!(devices[3].route, lan);
        for name in ["devbox", "build-box"] {
            let device = DeviceId::derive(name);
            assert_eq!(
                registry.known_device(book_link(device)).unwrap(),
                Some(device)
            );
        }
        assert_eq!(registry.phone_books().unwrap(), [home.clone(), work]);

        // Work Mac goes, and Home Mac's devices stay as they were.
        registry
            .set_phone_books(std::slice::from_ref(&home))
            .unwrap();
        let names: Vec<String> = registry
            .list()
            .unwrap()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        assert_eq!(names, ["Home Mac", "devbox"]);
        assert_eq!(registry.phone_books().unwrap(), [home]);
    }

    #[test]
    fn a_paired_phone_keeps_its_book_devices_and_waiting_changes_through_the_update() {
        use crate::phone::book_link;
        use crate::phone::tests::{book, entry};

        let dir = std::env::temp_dir().join(format!("maxhq-registry-{}", LinkId::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("maxhq.db");
        let old_book = book(vec![
            entry("devbox", &["192.168.1.20"]),
            entry("build-box", &["build-box.local"]),
        ]);
        let devbox = book_link(DeviceId::derive("devbox"));
        let build_box = book_link(DeviceId::derive("build-box"));
        {
            let mut conn = Connection::open(&path).unwrap();
            let mut old = steps();
            old.truncate(6);
            Migrations::new(old).to_latest(&mut conn).unwrap();
            // What a paired phone held before: its one book, its devices,
            // and what each of their tables kept for them.
            conn.execute_batch(&format!(
                "INSERT INTO device (link, name, kind, host, added_at) VALUES
                     ('{devbox}', 'devbox', 'ssh', 'ssh://dev@192.168.1.20:22', 0),
                     ('{build_box}', 'build-box', 'ssh', 'ssh://dev@build-box.local:22', 1);
                 INSERT INTO route (link, pos, host)
                     VALUES ('{devbox}', 0, 'ssh://dev@devbox.lan:22');
                 INSERT INTO last_good (link, net, host, at)
                     VALUES ('{devbox}', 'home', 'ssh://dev@devbox.lan:22', 5);
                 INSERT INTO link_device (link, device, at) VALUES ('{build_box}', 'x', 6);
                 INSERT INTO link_claim (link, device, known_by, different, at)
                     VALUES ('{build_box}', 'y', NULL, 0, 7);
                 INSERT INTO outbox (key, link, device, agent, title, change, queued_at)
                     VALUES ('k1', '{devbox}', 'd', 'a', 'Fix the build', '{{}}', 8);"
            ))
            .unwrap();
            conn.execute(
                "INSERT INTO setting (name, value) VALUES ('phone_book', ?1)",
                params![serde_json::to_string(&old_book).unwrap()],
            )
            .unwrap();
        }
        let registry = Registry::open_phone(&path).unwrap();
        assert_eq!(registry.phone_books().unwrap(), [old_book]);
        assert_eq!(registry.setting::<PhoneBook>("phone_book").unwrap(), None);
        let devices = registry.list().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, devbox);
        assert_eq!(
            devices[0].fallbacks,
            [Route::Ssh {
                host: "ssh://dev@devbox.lan:22".into()
            }]
        );
        assert_eq!(devices[0].last_good.len(), 1);
        let count = |table: &str| {
            registry
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap()
        };
        for table in ["outbox", "route", "last_good", "link_device", "link_claim"] {
            assert_eq!(count(table), 1, "{table}");
        }

        // A device that goes still takes what pointed at it, and two
        // devices may now start at one address.
        registry
            .set_phone_books(&[book(vec![
                entry("devbox", &["192.168.1.20", "devbox.lan"]),
                entry("other-box", &["192.168.1.20"]),
            ])])
            .unwrap();
        assert_eq!(count("link_device") + count("link_claim"), 0);
        assert_eq!(count("outbox"), 1);
        assert_eq!(registry.list().unwrap().len(), 2);
        drop(registry);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
