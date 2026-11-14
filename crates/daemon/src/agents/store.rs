// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The permissions you gave, with how many agents each started, and the
//! access keys `maxd` minted, by the hash of their secret.

use maxhq_protocol::agents::Permission;
#[cfg(test)]
use maxhq_protocol::ids::KeyId;
use maxhq_protocol::ids::PermissionId;
use rusqlite::params;

use super::KeyRecord;
use crate::db::{Db, RecordError};

impl Db {
    /// Every stored permission, with how many agents it started. A row
    /// that doesn't parse is skipped.
    pub fn permissions(&self) -> rusqlite::Result<Vec<(Permission, u32)>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT id, record, started FROM permissions")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (id, record, started) = row?;
            match serde_json::from_str(&record) {
                Ok(permission) => found.push((permission, u32::try_from(started).unwrap_or(0))),
                Err(_) => tracing::warn!(id, "skipping a permission that doesn't parse"),
            }
        }
        Ok(found)
    }

    /// Stores a permission. One stored before keeps its count of starts.
    pub fn put_permission(&self, permission: &Permission) -> Result<(), RecordError> {
        let record = serde_json::to_string(permission)?;
        self.lock().execute(
            "INSERT INTO permissions (id, record) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET record = excluded.record",
            params![permission.id.to_string(), record],
        )?;
        Ok(())
    }

    /// Stores how many agents a permission started.
    pub fn set_started(&self, permission: PermissionId, started: u32) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE permissions SET started = ?2 WHERE id = ?1",
            params![permission.to_string(), started],
        )?;
        Ok(())
    }

    /// Forgets a permission and the keys that call under it.
    pub fn remove_permission(&self, permission: PermissionId) -> Result<(), RecordError> {
        let keys = self.access_keys()?;
        let conn = self.lock();
        conn.execute(
            "DELETE FROM permissions WHERE id = ?1",
            params![permission.to_string()],
        )?;
        for key in keys.iter().filter(|key| key.permission == permission) {
            conn.execute(
                "DELETE FROM access_keys WHERE id = ?1",
                params![key.id.to_string()],
            )?;
        }
        Ok(())
    }

    /// Every stored access key. A row that doesn't parse is skipped.
    pub fn access_keys(&self) -> rusqlite::Result<Vec<KeyRecord>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT id, hash, record FROM access_keys")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut found = Vec::new();
        for row in rows {
            let (id, hash, record) = row?;
            match serde_json::from_str::<KeyRecord>(&record) {
                Ok(key) => found.push(KeyRecord { hash, ..key }),
                Err(_) => tracing::warn!(id, "skipping an access key that doesn't parse"),
            }
        }
        Ok(found)
    }

    /// Stores an access key.
    pub fn put_access_key(&self, key: &KeyRecord) -> Result<(), RecordError> {
        let record = serde_json::to_string(key)?;
        self.lock().execute(
            "INSERT INTO access_keys (id, hash, record) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET hash = excluded.hash, record = excluded.record",
            params![key.id.to_string(), key.hash, record],
        )?;
        Ok(())
    }

    /// Forgets an access key.
    #[cfg(test)]
    pub fn remove_access_key(&self, key: KeyId) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM access_keys WHERE id = ?1",
            params![key.to_string()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::agents::{Actor, LeaveContainer, Limits, DEPTH};
    use maxhq_protocol::ids::DeviceId;
    use maxhq_protocol::model::Vendor;

    use super::*;

    #[test]
    fn permissions_and_keys_round_trip_and_go_together() {
        let db = Db::open_in_memory().expect("the database opens");
        let permission = Permission {
            id: PermissionId::derive("p"),
            holder: Actor::Key {
                key: KeyId::derive("k"),
            },
            home: DeviceId::derive("d"),
            devices: vec![DeviceId::derive("d")],
            vendors: vec![Vendor::Claude],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: LeaveContainer::Ask,
        };
        db.put_permission(&permission).unwrap();
        db.set_started(permission.id, 3).unwrap();
        db.put_permission(&permission).unwrap();
        assert_eq!(db.permissions().unwrap(), vec![(permission.clone(), 3)]);
        let key = super::super::mint("bot", permission.id).unwrap().record;
        db.put_access_key(&key).unwrap();
        assert_eq!(db.access_keys().unwrap(), vec![key.clone()]);
        db.remove_permission(permission.id).unwrap();
        assert!(db.permissions().unwrap().is_empty());
        assert!(db.access_keys().unwrap().is_empty());
        db.put_access_key(&key).unwrap();
        db.remove_access_key(key.id).unwrap();
        assert!(db.access_keys().unwrap().is_empty());
    }

    #[test]
    fn a_permission_saved_before_containers_asked_reads_as_ask() {
        let db = Db::open_in_memory().expect("the database opens");
        let permission = Permission {
            id: PermissionId::derive("old"),
            holder: Actor::Key {
                key: KeyId::derive("k"),
            },
            home: DeviceId::derive("d"),
            devices: vec![DeviceId::derive("d")],
            vendors: vec![Vendor::Claude],
            accounts: None,
            places: None,
            depth: DEPTH,
            limits: Limits::default(),
            test_kinds: Vec::new(),
            leave_container: LeaveContainer::Never,
        };
        let mut record = serde_json::to_value(&permission).unwrap();
        record.as_object_mut().unwrap().remove("leave_container");
        db.lock()
            .execute(
                "INSERT INTO permissions (id, record) VALUES (?1, ?2)",
                params![permission.id.to_string(), record.to_string()],
            )
            .unwrap();
        let [(read, _)] = db.permissions().unwrap().try_into().unwrap();
        assert_eq!(read.leave_container, LeaveContainer::Ask);
    }
}
