// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Notes in `maxd.db`: every version by hash, and each note's one head.
//! A save or a put stores its versions and merges the heads they leave in
//! one transaction, so a note always has one head on disk.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use maxhq_core::notes::{check, check_note, heads, sealed, settle};
use maxhq_protocol::ids::{DeviceId, NoteId, ProjectId};
use maxhq_protocol::notes::{
    NoteAuthor, NoteSummary, NoteVersion, NoteVersionInfo, VersionHash, MAX_NOTE_HISTORY,
};
use maxhq_protocol::time::Timestamp;
use rusqlite::{params, Connection, OptionalExtension};

use crate::db::Db;

/// The refusal for a version the device doesn't hold.
pub const NO_VERSION: &str = "That note version isn't on this device.";

/// The refusal for a note the device doesn't hold.
pub const NO_NOTE: &str = "That note isn't on this device.";

/// The most JSON one read of versions answers with, so the answer fits a
/// frame. A version over it goes alone.
const READ_BUDGET: usize = 4 * 1024 * 1024;

/// A save, as `SaveNote` asks for it.
#[derive(Clone, Debug)]
pub struct Save {
    /// The project the note is in.
    pub project: ProjectId,
    /// Which note.
    pub note: NoteId,
    /// The version the text was edited from, or none for a new note.
    pub base: Option<VersionHash>,
    /// The title.
    pub title: String,
    /// The body.
    pub body: String,
    /// Whether it's deleted.
    pub deleted: bool,
    /// When it was saved.
    pub at: Timestamp,
    /// Who saved it.
    pub by: Option<NoteAuthor>,
}

/// Says what SQLite or JSON failed at.
fn failed(err: impl std::fmt::Display) -> String {
    format!("Couldn't reach the notes on this device: {err}")
}

/// Saves a new version after `save.base` and merges it with any head that
/// came since. Gives the note's head and whether it merged.
pub fn save(db: &Db, save: Save) -> Result<(NoteVersion, bool), String> {
    check_note(&save.title, &save.body)?;
    let mut conn = db.lock();
    let tx = conn.transaction().map_err(failed)?;
    let parents = match &save.base {
        Some(base) => match version(&tx, save.project, base)? {
            Some(held) if held.note == save.note => vec![base.clone()],
            _ => return Err(NO_VERSION.into()),
        },
        None => Vec::new(),
    };
    let version = sealed(NoteVersion {
        hash: VersionHash(String::new()),
        project: save.project,
        note: save.note,
        parents,
        title: save.title,
        body: save.body,
        deleted: save.deleted,
        at: save.at,
        by: save.by,
    });
    // A save sent again makes the same version, which changes nothing.
    let merged = if project_holding(&tx, &version.hash)?.is_some() {
        false
    } else {
        insert(&tx, &version)?;
        settle_note(&tx, save.project, save.note, &[&version])?
    };
    let head = head(&tx, save.project, save.note)?.ok_or(NO_NOTE)?;
    tx.commit().map_err(failed)?;
    Ok((head, merged))
}

/// Stores the versions of `project`'s notes it lacks, parents first, and
/// merges the heads they leave. It refuses the whole put when one
/// version's hash doesn't match, or one comes before its parents. Gives
/// how many it didn't hold before.
pub fn put(db: &Db, project: ProjectId, versions: &[NoteVersion]) -> Result<u32, String> {
    let mut conn = db.lock();
    let tx = conn.transaction().map_err(failed)?;
    let mut seen: HashSet<&VersionHash> = HashSet::new();
    for version in versions {
        if version.project != project {
            return Err(format!(
                "Note version {} is from another project.",
                version.hash.0
            ));
        }
        check(version)?;
        check_note(&version.title, &version.body)?;
        for parent in &version.parents {
            if !seen.contains(parent) && project_holding(&tx, parent)? != Some(project) {
                return Err(format!(
                    "Note version {} came before its parents.",
                    version.hash.0
                ));
            }
        }
        seen.insert(&version.hash);
    }
    let mut stored = 0;
    let mut new: BTreeMap<NoteId, Vec<&NoteVersion>> = BTreeMap::new();
    for version in versions {
        match project_holding(&tx, &version.hash)? {
            Some(held) if held == project => continue,
            Some(_) => {
                return Err(format!(
                    "Note version {} is from another project.",
                    version.hash.0
                ))
            }
            None => {}
        }
        insert(&tx, version)?;
        stored += 1;
        new.entry(version.note).or_default().push(version);
    }
    for (note, new) in new {
        settle_note(&tx, project, note, &new)?;
    }
    tx.commit().map_err(failed)?;
    Ok(stored)
}

/// Moves a note's head past the versions just stored, parents first, and
/// merges the heads that leaves. Says whether it merged.
///
/// Every stored version's parents are stored too, so a new version is no
/// one's parent yet: the heads are the old head and the new versions,
/// less any of them a new version follows. Only when that leaves more
/// than one does it read the note's whole history to merge.
fn settle_note(
    conn: &Connection,
    project: ProjectId,
    note: NoteId,
    new: &[&NoteVersion],
) -> Result<bool, String> {
    let mut left: BTreeSet<VersionHash> = head_hash(conn, project, note)?.into_iter().collect();
    for version in new {
        for parent in &version.parents {
            left.remove(parent);
        }
        left.insert(version.hash.clone());
    }
    let (head, merged) = if left.len() <= 1 {
        (left.pop_first(), false)
    } else {
        let all = of_note(conn, project, note)?;
        let made = settle(&all);
        for version in &made {
            insert(conn, version)?;
        }
        let head = match made.last() {
            Some(last) => Some(last.hash.clone()),
            None => heads(&all).first().map(|head| head.hash.clone()),
        };
        (head, !made.is_empty())
    };
    if let Some(head) = head {
        conn.execute(
            "INSERT INTO note_heads (project, note, hash) VALUES (?1, ?2, ?3)
             ON CONFLICT (project, note) DO UPDATE SET hash = excluded.hash",
            params![project.to_string(), note.to_string(), head.0],
        )
        .map_err(failed)?;
    }
    Ok(merged)
}

/// Stores one version. One held already stays as it was.
fn insert(conn: &Connection, version: &NoteVersion) -> Result<(), String> {
    let record = serde_json::to_string(version).map_err(failed)?;
    conn.execute(
        "INSERT OR IGNORE INTO note_versions (hash, project, note, record) VALUES (?1, ?2, ?3, ?4)",
        params![
            version.hash.0,
            version.project.to_string(),
            version.note.to_string(),
            record
        ],
    )
    .map_err(failed)?;
    Ok(())
}

/// The project holding the version with this hash, if any does.
fn project_holding(conn: &Connection, hash: &VersionHash) -> Result<Option<ProjectId>, String> {
    let project: Option<String> = conn
        .query_row(
            "SELECT project FROM note_versions WHERE hash = ?1",
            params![hash.0],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    Ok(project.and_then(|project| project.parse().ok()))
}

/// A note's head hash.
fn head_hash(
    conn: &Connection,
    project: ProjectId,
    note: NoteId,
) -> Result<Option<VersionHash>, String> {
    conn.query_row(
        "SELECT hash FROM note_heads WHERE project = ?1 AND note = ?2",
        params![project.to_string(), note.to_string()],
        |row| row.get(0).map(VersionHash),
    )
    .optional()
    .map_err(failed)
}

/// One version of `project`'s notes.
pub fn version(
    conn: &Connection,
    project: ProjectId,
    hash: &VersionHash,
) -> Result<Option<NoteVersion>, String> {
    let record: Option<String> = conn
        .query_row(
            "SELECT record FROM note_versions WHERE hash = ?1 AND project = ?2",
            params![hash.0, project.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(failed)?;
    record
        .map(|record| serde_json::from_str(&record).map_err(failed))
        .transpose()
}

/// A note's head.
pub fn head(
    conn: &Connection,
    project: ProjectId,
    note: NoteId,
) -> Result<Option<NoteVersion>, String> {
    match head_hash(conn, project, note)? {
        Some(hash) => version(conn, project, &hash),
        None => Ok(None),
    }
}

/// Every version of one note, with their bodies.
fn of_note(
    conn: &Connection,
    project: ProjectId,
    note: NoteId,
) -> Result<Vec<NoteVersion>, String> {
    let mut query = conn
        .prepare("SELECT record FROM note_versions WHERE project = ?1 AND note = ?2")
        .map_err(failed)?;
    let rows = query
        .query_map(params![project.to_string(), note.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(failed)?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(serde_json::from_str(&row.map_err(failed)?).map_err(failed)?);
    }
    Ok(versions)
}

/// The SQL for a version without its body, and the body's size in bytes,
/// so a list doesn't carry every body out of SQLite.
const BODILESS: &str = "json_set(record, '$.body', ''), \
     length(CAST(json_extract(record, '$.body') AS BLOB))";

/// Reads one row of [`BODILESS`] from column `at`.
fn bodiless(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<(String, u32)> {
    Ok((
        row.get(at)?,
        row.get::<_, Option<u32>>(at + 1)?.unwrap_or(0),
    ))
}

/// A note's versions, newest first, at most [`MAX_NOTE_HISTORY`], without
/// their bodies.
pub fn history(
    conn: &Connection,
    project: ProjectId,
    note: NoteId,
) -> Result<Vec<NoteVersionInfo>, String> {
    let mut query = conn
        .prepare(&format!(
            "SELECT {BODILESS} FROM note_versions WHERE project = ?1 AND note = ?2
             ORDER BY json_extract(record, '$.at') DESC, hash LIMIT ?3"
        ))
        .map_err(failed)?;
    let limit = i64::try_from(MAX_NOTE_HISTORY).unwrap_or(i64::MAX);
    let rows = query
        .query_map(
            params![project.to_string(), note.to_string(), limit],
            |row| bodiless(row, 0),
        )
        .map_err(failed)?;
    let mut versions = Vec::new();
    for row in rows {
        let (record, size) = row.map_err(failed)?;
        let version: NoteVersion = serde_json::from_str(&record).map_err(failed)?;
        versions.push(NoteVersionInfo {
            hash: version.hash,
            parents: version.parents,
            title: version.title,
            deleted: version.deleted,
            at: version.at,
            by: version.by,
            size,
        });
    }
    Ok(versions)
}

/// The hash of every version of `project`'s notes, sorted.
pub fn hashes(conn: &Connection, project: ProjectId) -> Result<Vec<VersionHash>, String> {
    let mut query = conn
        .prepare("SELECT hash FROM note_versions WHERE project = ?1 ORDER BY hash")
        .map_err(failed)?;
    let rows = query
        .query_map(params![project.to_string()], |row| {
            row.get(0).map(VersionHash)
        })
        .map_err(failed)?;
    rows.collect::<Result<_, _>>().map_err(failed)
}

/// The versions it holds of `hashes`, in the order asked, stopping once
/// the answer holds about [`READ_BUDGET`] of them.
pub fn versions(
    conn: &Connection,
    project: ProjectId,
    hashes: &[VersionHash],
) -> Result<Vec<NoteVersion>, String> {
    let mut found = Vec::new();
    let mut size = 0;
    for hash in hashes {
        if size >= READ_BUDGET {
            break;
        }
        let record: Option<String> = conn
            .query_row(
                "SELECT record FROM note_versions WHERE hash = ?1 AND project = ?2",
                params![hash.0, project.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)?;
        if let Some(record) = record {
            size += record.len();
            found.push(serde_json::from_str(&record).map_err(failed)?);
        }
    }
    Ok(found)
}

/// Each of `project`'s notes at its head, tombstones included, sorted by
/// note. `me` stands for the device a note was made on when its first
/// version names none.
pub fn summaries(
    conn: &Connection,
    project: ProjectId,
    me: DeviceId,
) -> Result<Vec<NoteSummary>, String> {
    let made_on = made_on(conn, project)?;
    let mut query = conn
        .prepare(&format!(
            "SELECT {BODILESS} FROM note_heads JOIN note_versions USING (hash, project, note)
             WHERE project = ?1 ORDER BY note"
        ))
        .map_err(failed)?;
    let rows = query
        .query_map(params![project.to_string()], |row| bodiless(row, 0))
        .map_err(failed)?;
    let mut notes = Vec::new();
    for row in rows {
        let (record, size) = row.map_err(failed)?;
        let head: NoteVersion = serde_json::from_str(&record).map_err(failed)?;
        notes.push(NoteSummary {
            created_on: made_on.get(&head.note).copied().unwrap_or(me),
            note: head.note,
            title: head.title,
            head: head.hash,
            at: head.at,
            by: head.by,
            deleted: head.deleted,
            size,
        });
    }
    Ok(notes)
}

/// The device each note's first version was saved on, where it names
/// one. Of two first versions, as when two devices made a note with the
/// same ID, the earlier wins, then the smaller hash.
fn made_on(conn: &Connection, project: ProjectId) -> Result<BTreeMap<NoteId, DeviceId>, String> {
    let mut query = conn
        .prepare(
            "SELECT note, json_extract(record, '$.by.device') FROM note_versions
             WHERE project = ?1 AND json_array_length(record, '$.parents') = 0
             ORDER BY json_extract(record, '$.at') DESC, hash DESC",
        )
        .map_err(failed)?;
    let rows = query
        .query_map(params![project.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(failed)?;
    let mut made = BTreeMap::new();
    // Newest first, so the earliest is written last and stays.
    for row in rows {
        let (note, device) = row.map_err(failed)?;
        if let (Ok(note), Some(Ok(device))) = (note.parse(), device.map(|d| d.parse())) {
            made.insert(note, device);
        }
    }
    Ok(made)
}

/// Every project this device holds notes for.
pub fn projects(conn: &Connection) -> Result<Vec<ProjectId>, String> {
    let mut query = conn
        .prepare("SELECT DISTINCT project FROM note_heads ORDER BY project")
        .map_err(failed)?;
    let rows = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(failed)?;
    let mut projects = Vec::new();
    for row in rows {
        if let Ok(project) = row.map_err(failed)?.parse() {
            projects.push(project);
        }
    }
    Ok(projects)
}

/// Whether this device holds any notes for `project`.
pub fn holds_project(conn: &Connection, project: ProjectId) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM note_heads WHERE project = ?1)",
        params![project.to_string()],
        |row| row.get(0),
    )
    .map_err(failed)
}
