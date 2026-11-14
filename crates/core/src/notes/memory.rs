// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Notes kept in memory, for tests: an end of an exchange that checks and
//! merges what it's handed the way a real store does, and saves the way
//! `SaveNote` does.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use maxhq_protocol::ids::{NoteId, ProjectId};
use maxhq_protocol::notes::{NoteAuthor, NoteVersion, VersionHash};
use maxhq_protocol::time::Timestamp;

use super::sync::{Answer, NotesEnd};
use super::{check, check_note, heads, sealed, settle};

/// Every version of every project's notes, by hash.
type Versions = BTreeMap<ProjectId, BTreeMap<VersionHash, NoteVersion>>;

/// Notes kept in memory.
#[derive(Debug, Default)]
pub struct MemoryNotes {
    versions: Mutex<Versions>,
    /// The most versions one read answers with, to act like an end that
    /// sends large ones a few at a time.
    most: Option<usize>,
}

/// A save, as `SaveNote` asks for it.
#[derive(Clone, Debug)]
pub struct Save {
    /// The project the note is in.
    pub project: ProjectId,
    /// Which note.
    pub note: NoteId,
    /// The version the text was edited from, None for a new note.
    pub base: Option<VersionHash>,
    /// The note's title.
    pub title: String,
    /// The note's Markdown.
    pub body: String,
    /// Whether it's deleted.
    pub deleted: bool,
    /// When it was saved.
    pub at: Timestamp,
    /// Who saved it.
    pub by: Option<NoteAuthor>,
}

impl MemoryNotes {
    /// Notes that answer a read with at most `most` versions.
    #[must_use]
    pub fn answering_at_most(most: usize) -> Self {
        Self {
            most: Some(most),
            ..Self::default()
        }
    }

    fn lock(&self) -> MutexGuard<'_, Versions> {
        self.versions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Saves a note: a new version after `base`, merged with any head that
    /// came since. Gives the note's head and whether it merged.
    ///
    /// # Errors
    ///
    /// Refuses a title or body over the limits, or a base it doesn't hold.
    pub fn save(&self, save: Save) -> Result<(NoteVersion, bool), String> {
        check_note(&save.title, &save.body)?;
        let mut all = self.lock();
        let held = all.entry(save.project).or_default();
        let parents = match &save.base {
            Some(base) => {
                if held.get(base).is_none_or(|base| base.note != save.note) {
                    return Err("That note version isn't on this device.".to_owned());
                }
                vec![base.clone()]
            }
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
        held.entry(version.hash.clone()).or_insert(version);
        let merged = settle_note(held, save.note);
        let head = head_of(held, save.note).ok_or("The note has no head.")?;
        Ok((head, merged))
    }

    /// A note's head, when it has one.
    #[must_use]
    pub fn head(&self, project: ProjectId, note: NoteId) -> Option<NoteVersion> {
        self.lock()
            .get(&project)
            .and_then(|held| head_of(held, note))
    }

    /// The hashes it holds for a project, sorted.
    #[must_use]
    pub fn hashes_now(&self, project: ProjectId) -> Vec<VersionHash> {
        self.lock()
            .get(&project)
            .map(|held| held.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn put_now(&self, project: ProjectId, versions: Vec<NoteVersion>) -> Result<u32, String> {
        let mut all = self.lock();
        let held = all.entry(project).or_default();
        let mut seen: BTreeSet<&VersionHash> = held.keys().collect();
        for version in &versions {
            if version.project != project {
                return Err(format!(
                    "Note version {} is from another project.",
                    version.hash.0
                ));
            }
            check(version)?;
            check_note(&version.title, &version.body)?;
            if !version.parents.iter().all(|parent| seen.contains(parent)) {
                return Err(format!(
                    "Note version {} came before its parents.",
                    version.hash.0
                ));
            }
            seen.insert(&version.hash);
        }
        let mut stored = 0;
        let mut touched = BTreeSet::new();
        for version in versions {
            touched.insert(version.note);
            if !held.contains_key(&version.hash) {
                stored += 1;
                held.insert(version.hash.clone(), version);
            }
        }
        for note in touched {
            settle_note(held, note);
        }
        Ok(stored)
    }
}

/// One note's versions.
fn of_note(held: &BTreeMap<VersionHash, NoteVersion>, note: NoteId) -> Vec<NoteVersion> {
    held.values()
        .filter(|version| version.note == note)
        .cloned()
        .collect()
}

/// Merges a note's heads into one. Says whether it merged any.
fn settle_note(held: &mut BTreeMap<VersionHash, NoteVersion>, note: NoteId) -> bool {
    let made = settle(&of_note(held, note));
    let merged = !made.is_empty();
    for version in made {
        held.insert(version.hash.clone(), version);
    }
    merged
}

/// A note's one head.
fn head_of(held: &BTreeMap<VersionHash, NoteVersion>, note: NoteId) -> Option<NoteVersion> {
    let versions = of_note(held, note);
    match heads(&versions).as_slice() {
        [head] => Some((*head).clone()),
        _ => None,
    }
}

impl NotesEnd for MemoryNotes {
    fn hashes(&self, project: ProjectId) -> Answer<'_, Vec<VersionHash>> {
        let hashes = self.hashes_now(project);
        Box::pin(async move { Ok(hashes) })
    }

    fn versions(
        &self,
        project: ProjectId,
        hashes: Vec<VersionHash>,
    ) -> Answer<'_, Vec<NoteVersion>> {
        let all = self.lock();
        let found: Vec<NoteVersion> = all
            .get(&project)
            .map(|held| {
                hashes
                    .iter()
                    .filter_map(|hash| held.get(hash).cloned())
                    .take(self.most.unwrap_or(usize::MAX))
                    .collect()
            })
            .unwrap_or_default();
        drop(all);
        Box::pin(async move { Ok(found) })
    }

    fn put(&self, project: ProjectId, versions: Vec<NoteVersion>) -> Answer<'_, u32> {
        let stored = self.put_now(project, versions);
        Box::pin(async move { stored })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::tests::{note, project};

    fn save(base: Option<&VersionHash>, body: &str) -> Save {
        Save {
            project: project(),
            note: note(),
            base: base.cloned(),
            title: "Plan".into(),
            body: body.into(),
            deleted: false,
            at: Timestamp(1),
            by: None,
        }
    }

    #[test]
    fn a_save_from_an_old_version_merges_with_the_head() {
        let notes = MemoryNotes::default();
        let (first, merged) = notes.save(save(None, "one\ntwo\n")).unwrap();
        assert!(!merged);
        let (second, merged) = notes.save(save(Some(&first.hash), "ONE\ntwo\n")).unwrap();
        assert!(!merged);
        assert_eq!(second.parents, vec![first.hash.clone()]);
        let (head, merged) = notes.save(save(Some(&first.hash), "one\nTWO\n")).unwrap();
        assert!(merged);
        assert_eq!(head.body, "ONE\nTWO\n");
    }

    #[test]
    fn a_save_from_a_version_it_lacks_is_refused() {
        let notes = MemoryNotes::default();
        let missing = VersionHash("a".repeat(64));
        assert!(notes.save(save(Some(&missing), "x\n")).is_err());
    }
}
