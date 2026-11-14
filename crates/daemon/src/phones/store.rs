// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the phones keep in the database: each phone the Mac lets do
//! everything here but let other keys in.

use std::collections::HashSet;

use maxhq_protocol::ids::PhoneId;
use rusqlite::params;

use crate::db::Db;

impl Db {
    /// The phones whose switch is on. A row that doesn't hold a phone ID
    /// is skipped.
    pub(super) fn phones_everything(&self) -> rusqlite::Result<HashSet<PhoneId>> {
        let conn = self.lock();
        let mut query = conn.prepare("SELECT phone FROM phone_everything")?;
        let rows = query.query_map([], |row| row.get::<_, String>(0))?;
        let mut phones = HashSet::new();
        for row in rows {
            if let Ok(phone) = row?.parse() {
                phones.insert(phone);
            }
        }
        Ok(phones)
    }

    /// Turns `phone`'s switch on or off.
    pub(super) fn set_phone_everything(&self, phone: PhoneId, on: bool) -> rusqlite::Result<()> {
        let sql = if on {
            "INSERT OR IGNORE INTO phone_everything (phone) VALUES (?1)"
        } else {
            "DELETE FROM phone_everything WHERE phone = ?1"
        };
        self.lock().execute(sql, params![phone.to_string()])?;
        Ok(())
    }
}
