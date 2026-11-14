// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Project notes without storage: how a version is hashed and checked, how
//! two heads merge, and how two ends swap the versions each lacks. They're
//! pure, so every device that holds the same versions makes the same
//! merges and ends at the same head.
//!
//! The mock data source in `packages/shared/src/notes/merge.ts` keeps the
//! same rules in TypeScript for the dev preview. The real app never merges
//! there: a change to the rules here goes there too.

#[cfg(any(test, feature = "testing"))]
pub mod memory;
mod merge;
pub mod sync;

use maxhq_protocol::notes::{
    NoteId, NoteVersion, NoteVersionInfo, VersionHash, MAX_NOTE_BODY, MAX_NOTE_TITLE,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use merge::{base, heads, merge, settle};

/// The fields a version's hash covers, in the order JSON sorts them, so
/// the encoding is canonical.
#[derive(Serialize)]
struct Hashed<'a> {
    body: &'a str,
    deleted: bool,
    note: NoteId,
    parents: Vec<&'a VersionHash>,
    title: &'a str,
}

/// The hash of a version with these fields: lowercase hex SHA-256 over
/// compact JSON of the note, its sorted parents, title, body and whether
/// it's deleted, with the keys in that sorted order.
#[must_use]
pub fn hash_of(
    note: NoteId,
    parents: &[VersionHash],
    title: &str,
    body: &str,
    deleted: bool,
) -> VersionHash {
    let mut parents: Vec<&VersionHash> = parents.iter().collect();
    parents.sort();
    let hashed = Hashed {
        body,
        deleted,
        note,
        parents,
        title,
    };
    // Strings and IDs always encode.
    let json = serde_json::to_vec(&hashed).unwrap_or_default();
    let digest = Sha256::digest(&json);
    VersionHash(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The version with its parents sorted and its hash set from its fields.
#[must_use]
pub fn sealed(mut version: NoteVersion) -> NoteVersion {
    version.parents.sort();
    version.parents.dedup();
    version.hash = hash_of(
        version.note,
        &version.parents,
        &version.title,
        &version.body,
        version.deleted,
    );
    version
}

/// Refuses a version whose hash doesn't match its fields, or whose
/// parents aren't sorted.
///
/// # Errors
///
/// Says which version and why.
pub fn check(version: &NoteVersion) -> Result<(), String> {
    if !version.parents.is_sorted() {
        return Err(format!(
            "Note version {} lists its parents out of order.",
            version.hash.0
        ));
    }
    let hash = hash_of(
        version.note,
        &version.parents,
        &version.title,
        &version.body,
        version.deleted,
    );
    if hash != version.hash {
        return Err(format!(
            "Note version {} doesn't match its hash.",
            version.hash.0
        ));
    }
    Ok(())
}

/// Refuses a title or body over the limits.
///
/// # Errors
///
/// Says which is too long.
pub fn check_note(title: &str, body: &str) -> Result<(), String> {
    if title.chars().count() > MAX_NOTE_TITLE {
        return Err(format!(
            "A note's title can be at most {MAX_NOTE_TITLE} characters."
        ));
    }
    if body.len() > MAX_NOTE_BODY {
        return Err(format!(
            "A note can be at most {} MiB.",
            MAX_NOTE_BODY / (1024 * 1024)
        ));
    }
    Ok(())
}

/// A body's size for a summary, which a body under the limit always fits.
#[must_use]
pub fn size_of(body: &str) -> u32 {
    u32::try_from(body.len()).unwrap_or(u32::MAX)
}

/// A version for a note's history, without its body.
#[must_use]
pub fn info(version: &NoteVersion) -> NoteVersionInfo {
    NoteVersionInfo {
        hash: version.hash.clone(),
        parents: version.parents.clone(),
        title: version.title.clone(),
        deleted: version.deleted,
        at: version.at,
        by: version.by,
        size: size_of(&version.body),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use maxhq_protocol::ids::ProjectId;
    use maxhq_protocol::time::Timestamp;

    use super::*;

    /// A note for tests, the same one each time.
    pub(crate) fn note() -> NoteId {
        NoteId::derive("release")
    }

    /// The project tests' notes are in.
    pub(crate) fn project() -> ProjectId {
        ProjectId::derive("atlas")
    }

    /// A sealed version of [`note`] after `parents`.
    pub(crate) fn version(
        parents: &[&NoteVersion],
        title: &str,
        body: &str,
        at: u64,
    ) -> NoteVersion {
        sealed(NoteVersion {
            hash: VersionHash(String::new()),
            project: project(),
            note: note(),
            parents: parents.iter().map(|parent| parent.hash.clone()).collect(),
            title: title.into(),
            body: body.into(),
            deleted: false,
            at: Timestamp(at),
            by: None,
        })
    }

    #[test]
    fn the_hash_matches_the_typescript_one() {
        // The same vector is in packages/shared/src/notes/merge.test.ts.
        let note: NoteId =
            serde_json::from_str("\"0190f5a2-7c3e-7b1a-9d4e-2f6a8b0c1d2e\"").unwrap();
        let parents = [VersionHash("b".repeat(64)), VersionHash("a".repeat(64))];
        let hash = hash_of(
            note,
            &parents,
            "Plan \"v2\"",
            "- [ ] ship\n\tcafé \u{1}\n",
            false,
        );
        assert_eq!(
            hash.0,
            "7692edc0276a95e2dd67cf9d836c4e34d3ccf261887f3cb0d6d83172aeffc73a"
        );
    }

    #[test]
    fn a_bad_hash_is_refused() {
        let first = version(&[], "Release checklist", "- [ ] Tag\n", 1);
        assert_eq!(check(&first), Ok(()));
        let mut forged = first.clone();
        forged.body = "- [x] Tag\n".into();
        assert!(check(&forged)
            .unwrap_err()
            .contains("doesn't match its hash"));
        let mut unsorted = version(&[&first], "Release checklist", "", 2);
        unsorted.parents = vec![VersionHash("f".repeat(64)), first.hash.clone()];
        assert!(check(&unsorted).unwrap_err().contains("out of order"));
    }

    #[test]
    fn when_and_by_whom_stay_out_of_the_hash() {
        let a = version(&[], "Notes", "x\n", 1);
        let mut b = version(&[], "Notes", "x\n", 99);
        b.by = Some(maxhq_protocol::notes::NoteAuthor {
            device: maxhq_protocol::ids::DeviceId::derive("devbox"),
            agent: None,
        });
        assert_eq!(sealed(b).hash, a.hash);
    }

    #[test]
    fn titles_and_bodies_over_the_limits_are_refused() {
        assert_eq!(check_note("Notes", "body"), Ok(()));
        assert_eq!(check_note(&"é".repeat(MAX_NOTE_TITLE), ""), Ok(()));
        assert!(check_note(&"é".repeat(MAX_NOTE_TITLE + 1), "").is_err());
        assert!(check_note("Notes", &"x".repeat(MAX_NOTE_BODY + 1)).is_err());
    }
}
