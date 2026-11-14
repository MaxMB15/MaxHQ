// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What network access keeps in the database: each phone session with the
//! SHA-256 of its token, and each one-time code's SHA-256 with the phone
//! it's bound to. No token or code is kept as it is.

use maxhq_protocol::ids::{DeviceId, PhoneId, PhoneSessionId};
use maxhq_protocol::netaccess::PhoneSession;
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, OptionalExtension, Row};

use crate::db::Db;

/// What a code's SHA-256 finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Code {
    pub phone: PhoneId,
    pub expires: Timestamp,
}

const SESSION: &str = "SELECT id, phone, created, expires, last_used FROM phone_sessions";

impl Db {
    /// Keeps a code's SHA-256, bound to `phone`.
    pub(super) fn put_phone_code(
        &self,
        hash: &[u8],
        phone: PhoneId,
        expires: Timestamp,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO phone_codes (hash, phone, expires) VALUES (?1, ?2, ?3)",
            params![hash, phone.to_string(), millis(expires)],
        )?;
        Ok(())
    }

    /// The code with SHA-256 `hash`, if one is kept.
    pub(super) fn phone_code(&self, hash: &[u8]) -> rusqlite::Result<Option<Code>> {
        let row = self
            .lock()
            .query_row(
                "SELECT phone, expires FROM phone_codes WHERE hash = ?1",
                params![hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(phone, expires)| {
            Some(Code {
                phone: phone.parse().ok()?,
                expires: stamp(expires),
            })
        }))
    }

    /// Uses the code with SHA-256 `code` for `session`, in one step: the
    /// code goes and the session starts, or neither. False when the code
    /// isn't `session.phone`'s or is over by `now`.
    pub(super) fn trade_phone_code(
        &self,
        code: &[u8],
        now: Timestamp,
        session: &PhoneSession,
        hash: &[u8],
    ) -> rusqlite::Result<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let used = tx.execute(
            "DELETE FROM phone_codes WHERE hash = ?1 AND phone = ?2 AND expires > ?3",
            params![code, session.phone.to_string(), millis(now)],
        )?;
        if used != 1 {
            return Ok(false);
        }
        insert(&tx, session, hash)?;
        tx.commit()?;
        Ok(true)
    }

    /// Starts `session`, whose token has SHA-256 `hash`.
    pub(super) fn put_phone_session(
        &self,
        session: &PhoneSession,
        hash: &[u8],
    ) -> rusqlite::Result<()> {
        insert(&self.lock(), session, hash)
    }

    /// The session whose token has SHA-256 `hash`, on `device`.
    pub(super) fn phone_session(
        &self,
        hash: &[u8],
        device: DeviceId,
    ) -> rusqlite::Result<Option<PhoneSession>> {
        let conn = self.lock();
        let found = conn
            .query_row(
                &format!("{SESSION} WHERE hash = ?1"),
                params![hash],
                |row| session(row, device),
            )
            .optional()?;
        Ok(found.flatten())
    }

    /// Whether `session` is still kept.
    pub(super) fn has_phone_session(&self, session: PhoneSessionId) -> rusqlite::Result<bool> {
        self.lock()
            .query_row(
                "SELECT 1 FROM phone_sessions WHERE id = ?1",
                params![session.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map(|found| found.is_some())
    }

    /// Swaps `session`'s token from SHA-256 `old` to `new`, in one step.
    /// False, with the old token kept, when `old` isn't its token now.
    pub(super) fn refresh_phone_session(
        &self,
        session: PhoneSessionId,
        old: &[u8],
        new: &[u8],
        expires: Timestamp,
    ) -> rusqlite::Result<bool> {
        let swapped = self.lock().execute(
            "UPDATE phone_sessions SET hash = ?3, expires = ?4 WHERE id = ?1 AND hash = ?2",
            params![session.to_string(), old, new, millis(expires)],
        )?;
        Ok(swapped == 1)
    }

    /// Notes that the phone used `session` at `at`.
    pub(super) fn touch_phone_session(
        &self,
        session: PhoneSessionId,
        at: Timestamp,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE phone_sessions SET last_used = ?2 WHERE id = ?1",
            params![session.to_string(), millis(at)],
        )?;
        Ok(())
    }

    /// Ends `session`. False when it wasn't kept.
    pub(super) fn remove_phone_session(&self, session: PhoneSessionId) -> rusqlite::Result<bool> {
        let removed = self.lock().execute(
            "DELETE FROM phone_sessions WHERE id = ?1",
            params![session.to_string()],
        )?;
        Ok(removed == 1)
    }

    /// Ends every session of `phone` and drops its codes, in one step, and
    /// says which sessions ended.
    pub(super) fn forget_phone_access(
        &self,
        phone: PhoneId,
    ) -> rusqlite::Result<Vec<PhoneSessionId>> {
        let phone = phone.to_string();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let ended = {
            let mut query = tx.prepare("SELECT id FROM phone_sessions WHERE phone = ?1")?;
            let ids = query.query_map(params![phone], |row| row.get::<_, String>(0))?;
            ids.filter_map(|id| id.ok()?.parse().ok()).collect()
        };
        tx.execute(
            "DELETE FROM phone_sessions WHERE phone = ?1",
            params![phone],
        )?;
        tx.execute("DELETE FROM phone_codes WHERE phone = ?1", params![phone])?;
        tx.commit()?;
        Ok(ended)
    }

    /// Drops the sessions and codes that are over by `now`.
    pub(super) fn drop_expired_phone_access(&self, now: Timestamp) -> rusqlite::Result<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM phone_sessions WHERE expires <= ?1",
            params![millis(now)],
        )?;
        conn.execute(
            "DELETE FROM phone_codes WHERE expires <= ?1",
            params![millis(now)],
        )?;
        Ok(())
    }

    /// Every session, on `device`. A row that doesn't parse is skipped.
    pub(super) fn phone_sessions(&self, device: DeviceId) -> rusqlite::Result<Vec<PhoneSession>> {
        let conn = self.lock();
        let mut query = conn.prepare(SESSION)?;
        let rows = query.query_map([], |row| session(row, device))?;
        let mut found = Vec::new();
        for row in rows {
            found.extend(row?);
        }
        Ok(found)
    }
}

fn insert(
    conn: &rusqlite::Connection,
    session: &PhoneSession,
    hash: &[u8],
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO phone_sessions (id, phone, hash, created, expires, last_used) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            session.id.to_string(),
            session.phone.to_string(),
            hash,
            millis(session.created),
            millis(session.expires),
            session.last_used.map(millis),
        ],
    )?;
    Ok(())
}

fn session(row: &Row<'_>, device: DeviceId) -> rusqlite::Result<Option<PhoneSession>> {
    let id: String = row.get(0)?;
    let phone: String = row.get(1)?;
    let (Ok(id), Ok(phone)) = (id.parse(), phone.parse()) else {
        tracing::warn!(id, "skipping a phone session that doesn't parse");
        return Ok(None);
    };
    Ok(Some(PhoneSession {
        id,
        device,
        phone,
        created: stamp(row.get(2)?),
        expires: stamp(row.get(3)?),
        last_used: row.get::<_, Option<i64>>(4)?.map(stamp),
    }))
}

fn millis(at: Timestamp) -> i64 {
    i64::try_from(at.0).unwrap_or(i64::MAX)
}

fn stamp(millis: i64) -> Timestamp {
    Timestamp(u64::try_from(millis).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::hash;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn started(phone: PhoneId, expires: u64) -> PhoneSession {
        PhoneSession {
            id: PhoneSessionId::generate(),
            device: DeviceId::derive("devbox"),
            phone,
            created: Timestamp(1),
            expires: Timestamp(expires),
            last_used: None,
        }
    }

    #[test]
    fn a_code_works_once_for_its_own_phone_until_it_expires() {
        let db = db();
        let (phone, other) = (PhoneId::derive("phone"), PhoneId::derive("other"));
        let code = hash("test-code");
        db.put_phone_code(&code, phone, Timestamp(100)).unwrap();
        assert_eq!(
            db.phone_code(&code).unwrap(),
            Some(Code {
                phone,
                expires: Timestamp(100)
            })
        );

        // Bound to its phone: another can't use it, and it stays.
        let theirs = started(other, 1000);
        assert!(!db
            .trade_phone_code(&code, Timestamp(10), &theirs, &hash("a"))
            .unwrap());
        assert!(db.phone_sessions(theirs.device).unwrap().is_empty());
        // Past its time it does nothing.
        let mine = started(phone, 1000);
        assert!(!db
            .trade_phone_code(&code, Timestamp(100), &mine, &hash("b"))
            .unwrap());
        // In time it works, once.
        assert!(db
            .trade_phone_code(&code, Timestamp(99), &mine, &hash("b"))
            .unwrap());
        assert_eq!(
            db.phone_session(&hash("b"), mine.device).unwrap(),
            Some(mine.clone())
        );
        assert_eq!(db.phone_code(&code).unwrap(), None);
        let again = started(phone, 1000);
        assert!(!db
            .trade_phone_code(&code, Timestamp(10), &again, &hash("c"))
            .unwrap());
    }

    #[test]
    fn a_refresh_swaps_the_token_in_one_step_or_not_at_all() {
        let db = db();
        let session = started(PhoneId::derive("phone"), 100);
        db.put_phone_session(&session, &hash("old")).unwrap();
        // A wrong old token changes nothing.
        assert!(!db
            .refresh_phone_session(session.id, &hash("wrong"), &hash("new"), Timestamp(200))
            .unwrap());
        assert!(db
            .phone_session(&hash("old"), session.device)
            .unwrap()
            .is_some());
        assert!(db
            .refresh_phone_session(session.id, &hash("old"), &hash("new"), Timestamp(200))
            .unwrap());
        assert_eq!(
            db.phone_session(&hash("old"), session.device).unwrap(),
            None
        );
        let now = db
            .phone_session(&hash("new"), session.device)
            .unwrap()
            .unwrap();
        assert_eq!((now.id, now.expires), (session.id, Timestamp(200)));
        // The old token can't refresh twice.
        assert!(!db
            .refresh_phone_session(session.id, &hash("old"), &hash("newer"), Timestamp(300))
            .unwrap());

        db.touch_phone_session(session.id, Timestamp(50)).unwrap();
        let touched = db
            .phone_session(&hash("new"), session.device)
            .unwrap()
            .unwrap();
        assert_eq!(touched.last_used, Some(Timestamp(50)));
    }

    #[test]
    fn revoking_or_removing_the_phone_ends_sessions() {
        let db = db();
        let (phone, other) = (PhoneId::derive("phone"), PhoneId::derive("other"));
        let (one, two, theirs) = (
            started(phone, 100),
            started(phone, 100),
            started(other, 100),
        );
        db.put_phone_session(&one, &hash("one")).unwrap();
        db.put_phone_session(&two, &hash("two")).unwrap();
        db.put_phone_session(&theirs, &hash("theirs")).unwrap();
        db.put_phone_code(&hash("code"), phone, Timestamp(100))
            .unwrap();

        assert!(db.remove_phone_session(one.id).unwrap());
        assert!(!db.has_phone_session(one.id).unwrap());
        assert!(!db.remove_phone_session(one.id).unwrap());

        assert_eq!(db.forget_phone_access(phone).unwrap(), vec![two.id]);
        assert_eq!(db.phone_session(&hash("two"), two.device).unwrap(), None);
        assert_eq!(db.phone_code(&hash("code")).unwrap(), None);
        assert_eq!(
            db.phone_sessions(theirs.device).unwrap(),
            vec![theirs.clone()]
        );

        db.drop_expired_phone_access(Timestamp(100)).unwrap();
        assert!(db.phone_sessions(theirs.device).unwrap().is_empty());
    }

    #[test]
    fn no_token_or_code_is_kept_as_it_is() {
        let db = db();
        let session = started(PhoneId::derive("phone"), 100);
        let (token, code) = ("test-token-not-real", "test-code-not-real");
        db.put_phone_session(&session, &hash(token)).unwrap();
        db.put_phone_code(&hash(code), session.phone, Timestamp(100))
            .unwrap();
        let conn = db.lock();
        for table in ["phone_sessions", "phone_codes"] {
            let mut query = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let columns = query.column_count();
            let mut rows = query.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for at in 0..columns {
                    let value = match row.get_ref(at).unwrap() {
                        rusqlite::types::ValueRef::Text(bytes)
                        | rusqlite::types::ValueRef::Blob(bytes) => bytes.to_vec(),
                        _ => continue,
                    };
                    for secret in [token, code] {
                        assert!(
                            !value
                                .windows(secret.len())
                                .any(|window| window == secret.as_bytes()),
                            "{table} keeps a secret as it is"
                        );
                    }
                }
            }
        }
    }
}
