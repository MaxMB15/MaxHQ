// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How full a Cursor session's context is, read from the store the CLI
//! keeps for it. The CLI sends no usage over ACP, so the driver reads it
//! here, off the loop, after each turn.
//!
//! Each session has `acp-sessions/<session ID>/store.db` in Cursor's
//! config folder. It is SQLite, with a `meta` table and a `blobs` table.
//! Meta key `0` is JSON written out as hex, and its `latestRootBlobId`
//! names the blob that is the conversation's latest root. The root is
//! protobuf, and its field 5 holds the context: field 1 is the tokens
//! used and field 2 the window. The store is opened read-only, and a
//! store that is missing, busy or doesn't read as that gives nothing, so
//! the context stays as it was. The meta holds the session's keys, so
//! nothing read from it is logged.

use std::path::{Path, PathBuf};
use std::time::Duration;

use maxhq_protocol::model::ContextUsage;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;

use super::rules::config_dir;
use crate::drive::acp::ContextRead;

/// How long a read waits on the CLI's own writes before it gives up.
const BUSY: Duration = Duration::from_millis(250);

/// The largest root a read takes.
const ROOT_BYTES: i64 = 16 << 20;

/// What a read takes from the meta.
#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "latestRootBlobId")]
    latest_root: String,
}

/// A read of the context of the session `session`, from its store in
/// Cursor's config folder on this machine. `maxd` starts Cursor with its
/// own environment, so the folder found here is the CLI's. None when
/// there's no config folder, or the ID isn't a plain name.
pub fn reader(session: &str) -> Option<ContextRead> {
    let dir = config_dir(
        std::env::var_os("CURSOR_CONFIG_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::home_dir().as_deref(),
    )
    .ok()?;
    let store = store(&dir, session)?;
    Some(Box::new(move || read(&store)))
}

/// The store of the session `session` in the config folder `dir`. The ID
/// comes from the CLI, so one that isn't a plain name gives none.
fn store(dir: &Path, session: &str) -> Option<PathBuf> {
    let plain = !session.is_empty()
        && session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| dir.join("acp-sessions").join(session).join("store.db"))
}

/// The context in the store at `store`, or none when it can't be read.
pub(crate) fn read(store: &Path) -> Option<ContextUsage> {
    // Read-only never makes the file, so a session the CLI keeps no store
    // for stays without one.
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(store, flags).ok()?;
    conn.busy_timeout(BUSY).ok()?;
    // One transaction, so the root is the one the meta named.
    let tx = conn.transaction().ok()?;
    let meta = tx
        .query_row("SELECT value FROM meta WHERE key = '0'", [], |row| {
            Ok(match row.get_ref(0)? {
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(bytes.to_vec()),
                _ => None,
            })
        })
        .optional()
        .ok()
        .flatten()
        .flatten()?;
    let root_id = latest_root(&meta)?;
    let root: Vec<u8> = tx
        .query_row(
            "SELECT data FROM blobs WHERE id = ?1 AND length(data) <= ?2",
            rusqlite::params![root_id, ROOT_BYTES],
            |row| row.get(0),
        )
        .optional()
        .ok()??;
    context(&root)
}

/// The latest root's ID in the meta `meta`, which is JSON written out as
/// hex, or JSON as it is.
fn latest_root(meta: &[u8]) -> Option<String> {
    let meta = meta.trim_ascii();
    let json = if meta.starts_with(b"{") {
        meta.to_vec()
    } else {
        unhex(meta)?
    };
    let meta: Meta = serde_json::from_slice(&json).ok()?;
    Some(meta.latest_root).filter(|id| !id.is_empty())
}

/// The bytes the hex `text` writes out.
fn unhex(text: &[u8]) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |byte: u8| char::from(byte).to_digit(16);
    text.chunks_exact(2)
        .map(|pair| u8::try_from((digit(pair[0])? << 4) | digit(pair[1])?).ok())
        .collect()
}

/// The context in a root: field 5's fields 1 and 2. A field that comes
/// more than once counts its last value, as in protobuf. None when either
/// is missing, the window is zero, or the root doesn't parse to its end.
fn context(root: &[u8]) -> Option<ContextUsage> {
    let mut used = None;
    let mut window = None;
    for field in Fields(root) {
        let (5, Wire::Bytes(usage)) = field? else {
            continue;
        };
        for field in Fields(usage) {
            match field? {
                (1, Wire::Varint(n)) => used = Some(n),
                (2, Wire::Varint(n)) => window = Some(n),
                _ => {}
            }
        }
    }
    let window = window.filter(|&n| n > 0)?;
    Some(ContextUsage {
        used_tokens: u32::try_from(used?).unwrap_or(u32::MAX),
        window_tokens: u32::try_from(window).unwrap_or(u32::MAX),
    })
}

/// A protobuf field's value, as far as a read needs it.
enum Wire<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    /// A fixed 32 or 64 bits, which a read skips.
    Fixed,
}

/// The fields of a protobuf message, each as its number and value. A
/// field that doesn't parse gives none and ends the message.
struct Fields<'a>(&'a [u8]);

impl<'a> Iterator for Fields<'a> {
    type Item = Option<(u64, Wire<'a>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let field = self.field();
        if field.is_none() {
            self.0 = &[];
        }
        Some(field)
    }
}

impl<'a> Fields<'a> {
    /// The next field.
    fn field(&mut self) -> Option<(u64, Wire<'a>)> {
        let key = self.varint()?;
        let value = match key & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => {
                self.take(8)?;
                Wire::Fixed
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                Wire::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Wire::Fixed
            }
            // Groups are gone from protobuf, and Cursor's roots have none.
            _ => return None,
        };
        Some((key >> 3, value))
    }

    /// The next varint, of at most ten bytes.
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for (n, &byte) in self.0.iter().enumerate().take(10) {
            value |= u64::from(byte & 0x7f) << (7 * n);
            if byte & 0x80 == 0 {
                self.0 = &self.0[n + 1..];
                return Some(value);
            }
        }
        None
    }

    /// The next `len` bytes.
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if len > self.0.len() {
            return None;
        }
        let (taken, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(taken)
    }
}

/// Synthetic stores in Cursor's shape, for tests here and the driver's.
#[cfg(test)]
pub(crate) mod testing {
    use std::path::Path;

    use rusqlite::Connection;

    /// A varint.
    pub fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// A varint field.
    pub fn number(field: u64, value: u64) -> Vec<u8> {
        let mut out = varint(field << 3);
        out.extend(varint(value));
        out
    }

    /// A length-delimited field.
    pub fn bytes(field: u64, value: &[u8]) -> Vec<u8> {
        let mut out = varint((field << 3) | 2);
        out.extend(varint(value.len() as u64));
        out.extend_from_slice(value);
        out
    }

    /// A root with `messages` 32-byte message IDs in field 1, as Cursor's
    /// have, and a context of `used` tokens of `window` in field 5.
    pub fn root(messages: u8, used: u64, window: u64) -> Vec<u8> {
        let mut out = Vec::new();
        for n in 0..messages {
            out.extend(bytes(1, &[n; 32]));
        }
        let mut usage = number(1, used);
        usage.extend(number(2, window));
        out.extend(bytes(5, &usage));
        out.extend(number(10, 1));
        out
    }

    /// The meta that names `root` the latest, written out as hex the way
    /// Cursor writes it, next to keys a read leaves alone.
    pub fn meta(root: &str) -> String {
        let json = serde_json::json!({
            "agentId": "00000000-0000-4000-8000-00000000000a",
            "latestRootBlobId": root,
            "blobEncryptionKey": "not-a-real-key",
            "mode": "default",
        })
        .to_string();
        json.bytes().map(|byte| format!("{byte:02x}")).collect()
    }

    /// A store at `path` with `blobs`, as IDs and data, and `meta` under
    /// key `0` unless it is none. The connection stays open, as the CLI's
    /// does, and in WAL mode when `wal` is set.
    pub fn write(
        path: &Path,
        wal: bool,
        meta: Option<&str>,
        blobs: &[(&str, &[u8])],
    ) -> Connection {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(path).unwrap();
        if wal {
            conn.pragma_update(None, "journal_mode", "wal").unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        if let Some(meta) = meta {
            conn.execute("INSERT INTO meta VALUES ('0', ?1)", [meta])
                .unwrap();
        }
        for (id, data) in blobs {
            conn.execute(
                "INSERT INTO blobs VALUES (?1, ?2)",
                rusqlite::params![id, data],
            )
            .unwrap();
        }
        conn
    }

    /// A 64-character blob ID made from `n`.
    pub fn id(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{bytes, id, meta, number, root, varint, write};
    use super::*;
    use crate::testing::TempDir;

    fn usage(used: u32, window: u32) -> Option<ContextUsage> {
        Some(ContextUsage {
            used_tokens: used,
            window_tokens: window,
        })
    }

    /// A store in `dir` whose latest root is `data`.
    fn with_root(dir: &TempDir, wal: bool, data: &[u8]) -> (PathBuf, Connection) {
        let path = dir.path().join("store.db");
        let conn = write(&path, wal, Some(&meta(&id(1))), &[(&id(1), data)]);
        (path, conn)
    }

    #[test]
    fn a_store_gives_the_latest_roots_context() {
        for wal in [true, false] {
            let dir = TempDir::new();
            let (path, conn) = with_root(&dir, wal, &root(3, 345_102, 1_000_000));
            assert_eq!(read(&path), usage(345_102, 1_000_000), "wal {wal}");
            // And again once the CLI has closed it.
            drop(conn);
            assert_eq!(read(&path), usage(345_102, 1_000_000), "wal {wal}");
        }
    }

    #[test]
    fn the_root_the_meta_names_wins_over_a_longer_one() {
        let dir = TempDir::new();
        let path = dir.path().join("store.db");
        let longer = root(40, 900, 200_000);
        let latest = root(2, 1_200, 200_000);
        let _conn = write(
            &path,
            true,
            Some(&meta(&id(2))),
            &[(&id(1), &longer), (&id(2), &latest)],
        );
        assert_eq!(read(&path), usage(1_200, 200_000));
    }

    #[test]
    fn a_missing_store_gives_none_and_stays_missing() {
        let dir = TempDir::new();
        let path = dir.path().join("store.db");
        assert_eq!(read(&path), None);
        assert!(!path.exists());
    }

    #[test]
    fn a_store_without_what_a_read_needs_gives_none() {
        let dir = TempDir::new();
        let cases: [(&str, Option<String>, Vec<u8>); 6] = [
            ("no meta", None, root(1, 10, 100)),
            ("meta not hex", Some("zz".repeat(8)), root(1, 10, 100)),
            ("meta not json", Some("6e6f".to_owned()), root(1, 10, 100)),
            ("no such root", Some(meta(&id(9))), root(1, 10, 100)),
            ("no context", Some(meta(&id(1))), bytes(1, &[0; 32])),
            ("no window", Some(meta(&id(1))), bytes(5, &number(1, 10))),
        ];
        for (n, (case, meta, data)) in cases.into_iter().enumerate() {
            let path = dir.path().join(format!("{n}/store.db"));
            let _conn = write(&path, true, meta.as_deref(), &[(&id(1), &data)]);
            assert_eq!(read(&path), None, "{case}");
        }
    }

    #[test]
    fn a_file_that_isnt_a_store_gives_none() {
        let dir = TempDir::new();
        let path = dir.path().join("store.db");
        std::fs::write(&path, b"not a database at all").unwrap();
        assert_eq!(read(&path), None);
        let empty = dir.path().join("empty.db");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(read(&empty), None);
    }

    #[test]
    fn a_cut_off_root_gives_none() {
        let whole = root(3, 345_102, 1_000_000);
        for len in 1..whole.len() {
            // Every cut either drops the context or leaves a field half
            // read, except one that falls between fields after it.
            let cut = &whole[..len];
            let read = context(cut);
            assert!(
                read.is_none() || read == usage(345_102, 1_000_000),
                "cut at {len}: {read:?}"
            );
        }
        let dir = TempDir::new();
        let (path, _conn) = with_root(&dir, true, &whole[..whole.len() - 4]);
        assert_eq!(read(&path), None);
    }

    #[test]
    fn a_root_that_doesnt_parse_gives_none() {
        // A length past the end, a group, and a varint that never ends.
        let past = [varint((5 << 3) | 2), varint(1_000)].concat();
        let group = number(5, 1)
            .into_iter()
            .chain(varint((7 << 3) | 3))
            .collect::<Vec<_>>();
        let endless = [0x28u8].into_iter().chain([0xff; 11]).collect::<Vec<_>>();
        for (case, data) in [("past", past), ("group", group), ("endless", endless)] {
            assert_eq!(context(&data), None, "{case}");
        }
    }

    #[test]
    fn counts_past_u32_saturate() {
        let mut usage_field = number(1, u64::from(u32::MAX) + 5);
        usage_field.extend(number(2, u64::MAX));
        assert_eq!(context(&bytes(5, &usage_field)), usage(u32::MAX, u32::MAX));
    }

    #[test]
    fn the_last_value_of_a_field_counts() {
        let mut usage_field = number(1, 10);
        usage_field.extend(number(2, 100));
        usage_field.extend(number(1, 20));
        let mut data = bytes(5, &usage_field);
        data.extend(bytes(5, &number(2, 200)));
        assert_eq!(context(&data), usage(20, 200));
    }

    #[test]
    fn a_plain_json_meta_reads_too() {
        let json = format!(r#"{{"latestRootBlobId":"{}"}}"#, id(1));
        assert_eq!(latest_root(json.as_bytes()), Some(id(1)));
        assert_eq!(latest_root(meta(&id(1)).as_bytes()), Some(id(1)));
        assert_eq!(latest_root(meta("").as_bytes()), None);
        assert_eq!(latest_root(b"abc"), None);
    }

    #[test]
    fn a_session_id_that_isnt_a_plain_name_has_no_store() {
        let dir = Path::new("/cfg");
        assert_eq!(
            store(dir, "00000000-0000-4000-8000-000000000001"),
            Some(PathBuf::from(
                "/cfg/acp-sessions/00000000-0000-4000-8000-000000000001/store.db"
            ))
        );
        for bad in ["", "..", "a/b", "../x", "a b", "x.y"] {
            assert_eq!(store(dir, bad), None, "{bad}");
        }
    }
}
