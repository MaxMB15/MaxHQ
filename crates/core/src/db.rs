// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Opens a SQLite database the way every MaxHQ process does (decision U-7).
//! Each process brings its own migrations: `maxd` keeps its registry and
//! history, and the app keeps a cache it can rebuild.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use rusqlite_migration::Migrations;
use thiserror::Error;

/// Why a database didn't open.
#[derive(Debug, Error)]
pub enum OpenError {
    /// SQLite refused the file or a setting.
    #[error("couldn't open the database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A migration failed, or the file is from a newer build.
    #[error("couldn't migrate the database: {0}")]
    Migrate(#[from] rusqlite_migration::Error),
}

/// Opens or creates the database at `path` and migrates it to the latest
/// schema. Writes go through the write-ahead log, so readers never block the
/// writer, and a busy database is retried for up to 5 seconds.
pub fn open(path: &Path, migrations: &Migrations<'_>) -> Result<Connection, OpenError> {
    let mut conn = Connection::open(path)?;
    configure(&mut conn, migrations)?;
    Ok(conn)
}

/// An in-memory database with the same settings, for tests.
pub fn open_in_memory(migrations: &Migrations<'_>) -> Result<Connection, OpenError> {
    let mut conn = Connection::open_in_memory()?;
    configure(&mut conn, migrations)?;
    Ok(conn)
}

fn configure(conn: &mut Connection, migrations: &Migrations<'_>) -> Result<(), OpenError> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations.to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite_migration::M;

    #[test]
    fn migrates_to_the_latest_schema() {
        let migrations = Migrations::new(vec![
            M::up("CREATE TABLE note (id INTEGER PRIMARY KEY, body TEXT NOT NULL);"),
            M::up("ALTER TABLE note ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;"),
        ]);
        assert!(migrations.validate().is_ok());
        let conn = open_in_memory(&migrations).unwrap();
        conn.execute("INSERT INTO note (body, pinned) VALUES ('hi', 1)", [])
            .unwrap();
        let pinned: i64 = conn
            .query_row("SELECT pinned FROM note", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pinned, 1);
    }
}
