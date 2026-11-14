// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Swapping note versions between two ends, so both hold every version of
//! a project's notes. `maxd` runs it with its peers, and the app between
//! the devices it links to. Each end merges the heads it's handed, and two
//! ends that hold the same versions make the same merges.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;

use maxhq_protocol::ids::ProjectId;
use maxhq_protocol::notes::{NoteVersion, VersionHash};

/// What an end answers with, later.
pub type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// The most hashes one read asks for.
const FETCH: usize = 256;

/// The most JSON one put carries, well under a frame's 16 MiB so the
/// request around it fits too. A version over it goes alone.
const PUT_BUDGET: usize = 4 * 1024 * 1024;

/// How many times an exchange compares the two ends before it stops. One
/// is enough unless an end merged between batches, or saved meanwhile.
const ROUNDS: usize = 3;

/// One end of an exchange: a device's own store, or a link to another
/// device's.
pub trait NotesEnd: Sync {
    /// The hash of every version it holds for the project.
    fn hashes(&self, project: ProjectId) -> Answer<'_, Vec<VersionHash>>;

    /// The versions it holds of `hashes`, in the order asked. It may give
    /// only the first few when they're large.
    fn versions(
        &self,
        project: ProjectId,
        hashes: Vec<VersionHash>,
    ) -> Answer<'_, Vec<NoteVersion>>;

    /// Stores versions it lacks, parents first, refusing any whose hash
    /// doesn't match, then merges the heads they leave. Gives how many it
    /// didn't hold before.
    fn put(&self, project: ProjectId, versions: Vec<NoteVersion>) -> Answer<'_, u32>;
}

/// How many versions an exchange handed each end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    /// Those `a` didn't hold before.
    pub to_a: u32,
    /// Those `b` didn't hold before.
    pub to_b: u32,
}

/// Hands each end the versions of the project's notes it lacks from the
/// other.
///
/// # Errors
///
/// Says why an end couldn't answer, or refused what it was handed.
pub async fn exchange(
    a: &dyn NotesEnd,
    b: &dyn NotesEnd,
    project: ProjectId,
) -> Result<Moved, String> {
    let mut moved = Moved::default();
    for _ in 0..ROUNDS {
        let in_a: BTreeSet<VersionHash> = a.hashes(project).await?.into_iter().collect();
        let in_b: BTreeSet<VersionHash> = b.hashes(project).await?.into_iter().collect();
        if in_a == in_b {
            break;
        }
        let for_b: Vec<VersionHash> = in_a.difference(&in_b).cloned().collect();
        let for_a: Vec<VersionHash> = in_b.difference(&in_a).cloned().collect();
        moved.to_b += copy(a, b, project, for_b).await?;
        moved.to_a += copy(b, a, project, for_a).await?;
    }
    Ok(moved)
}

/// Reads `wanted` from `from` and puts it in `to`.
async fn copy(
    from: &dyn NotesEnd,
    to: &dyn NotesEnd,
    project: ProjectId,
    wanted: Vec<VersionHash>,
) -> Result<u32, String> {
    if wanted.is_empty() {
        return Ok(0);
    }
    let mut got: BTreeMap<VersionHash, NoteVersion> = BTreeMap::new();
    for chunk in wanted.chunks(FETCH) {
        let mut left: Vec<VersionHash> = chunk.to_vec();
        while !left.is_empty() {
            let asked: BTreeSet<VersionHash> = left.iter().cloned().collect();
            let answer = from.versions(project, left.clone()).await?;
            let before = got.len();
            for version in answer {
                if version.project == project && asked.contains(&version.hash) {
                    got.insert(version.hash.clone(), version);
                }
            }
            if got.len() == before {
                return Err("A device didn't send the note versions it listed.".to_owned());
            }
            left.retain(|hash| !got.contains_key(hash));
        }
    }
    let mut stored = 0;
    for batch in batches(parents_first(got)) {
        stored += to.put(project, batch).await?;
    }
    Ok(stored)
}

/// The versions with each one's parents before it, where they're among
/// them.
fn parents_first(mut versions: BTreeMap<VersionHash, NoteVersion>) -> Vec<NoteVersion> {
    let mut order = Vec::with_capacity(versions.len());
    let hashes: Vec<VersionHash> = versions.keys().cloned().collect();
    for hash in hashes {
        // Walk up to the first parent not placed yet, place it, and go on
        // from the bottom again, so no recursion runs deep on a long
        // history.
        let mut stack = vec![hash];
        while let Some(top) = stack.last().cloned() {
            let Some(version) = versions.get(&top) else {
                stack.pop();
                continue;
            };
            if let Some(parent) = version
                .parents
                .iter()
                .find(|parent| versions.contains_key(*parent))
            {
                stack.push(parent.clone());
                continue;
            }
            if let Some(version) = versions.remove(&top) {
                order.push(version);
            }
            stack.pop();
        }
    }
    order
}

/// Splits versions into puts that each fit [`PUT_BUDGET`], keeping their
/// order.
fn batches(versions: Vec<NoteVersion>) -> Vec<Vec<NoteVersion>> {
    let mut batches: Vec<Vec<NoteVersion>> = Vec::new();
    let mut size = 0;
    for version in versions {
        let len = serde_json::to_vec(&version).map_or(0, |json| json.len());
        match batches.last_mut() {
            Some(batch) if size + len <= PUT_BUDGET => {
                batch.push(version);
                size += len;
            }
            _ => {
                batches.push(vec![version]);
                size = len;
            }
        }
    }
    batches
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use maxhq_protocol::notes::MAX_NOTE_BODY;
    use maxhq_protocol::time::Timestamp;

    use super::*;
    use crate::notes::memory::{MemoryNotes, Save};
    use crate::notes::tests::{note, project, version};

    fn save(end: &MemoryNotes, base: Option<&VersionHash>, body: &str, at: u64) -> NoteVersion {
        end.save(Save {
            project: project(),
            note: note(),
            base: base.cloned(),
            title: "Plan".into(),
            body: body.into(),
            deleted: false,
            at: Timestamp(at),
            by: None,
        })
        .unwrap()
        .0
    }

    #[test]
    fn two_devices_editing_different_lines_converge() {
        let (mac, devbox) = (MemoryNotes::default(), MemoryNotes::default());
        let first = save(&mac, None, "one\ntwo\nthree\n", 1);
        block_on(exchange(&mac, &devbox, project())).unwrap();
        assert_eq!(devbox.head(project(), note()).unwrap(), first);

        save(&mac, Some(&first.hash), "ONE\ntwo\nthree\n", 2);
        save(&devbox, Some(&first.hash), "one\ntwo\nTHREE\n", 3);
        let moved = block_on(exchange(&mac, &devbox, project())).unwrap();
        assert_eq!(moved, Moved { to_a: 1, to_b: 1 });
        let head = mac.head(project(), note()).unwrap();
        assert_eq!(head.body, "ONE\ntwo\nTHREE\n");
        assert_eq!(devbox.head(project(), note()).unwrap(), head);
        assert_eq!(mac.hashes_now(project()), devbox.hashes_now(project()));
        assert_eq!(
            block_on(exchange(&mac, &devbox, project())).unwrap(),
            Moved::default()
        );
    }

    #[test]
    fn three_devices_converge_whatever_order_they_meet_in() {
        let ends = [
            MemoryNotes::default(),
            MemoryNotes::default(),
            MemoryNotes::default(),
        ];
        let first = save(&ends[0], None, "a\nb\nc\nd\n", 1);
        block_on(exchange(&ends[0], &ends[1], project())).unwrap();
        block_on(exchange(&ends[0], &ends[2], project())).unwrap();
        save(&ends[0], Some(&first.hash), "A\nb\nc\nd\n", 2);
        save(&ends[1], Some(&first.hash), "a\nB\nc\nd\n", 3);
        save(&ends[2], Some(&first.hash), "a\nb\nc\nD\n", 4);
        block_on(exchange(&ends[1], &ends[2], project())).unwrap();
        block_on(exchange(&ends[0], &ends[2], project())).unwrap();
        block_on(exchange(&ends[0], &ends[1], project())).unwrap();
        let head = ends[0].head(project(), note()).unwrap();
        assert_eq!(head.body, "A\nB\nc\nD\n");
        for end in &ends[1..] {
            assert_eq!(end.head(project(), note()).unwrap().hash, head.hash);
        }
    }

    #[test]
    fn an_end_that_answers_a_few_at_a_time_still_gets_everything() {
        let mac = MemoryNotes::answering_at_most(2);
        let devbox = MemoryNotes::default();
        let mut last = save(&mac, None, "0\n", 0);
        for at in 1..10 {
            last = save(&mac, Some(&last.hash), &format!("{at}\n"), at);
        }
        let moved = block_on(exchange(&mac, &devbox, project())).unwrap();
        assert_eq!(moved.to_b, 10);
        assert_eq!(devbox.head(project(), note()).unwrap(), last);
    }

    #[test]
    fn a_bad_hash_is_refused_and_nothing_is_stored() {
        let devbox = MemoryNotes::default();
        let good = version(&[], "Plan", "one\n", 1);
        let mut bad = version(&[&good], "Plan", "two\n", 2);
        bad.body = "forged\n".into();
        let refused = block_on(devbox.put(project(), vec![good, bad]));
        assert!(refused.unwrap_err().contains("doesn't match its hash"));
        assert!(devbox.hashes_now(project()).is_empty());
    }

    #[test]
    fn a_version_before_its_parent_is_refused() {
        let devbox = MemoryNotes::default();
        let first = version(&[], "Plan", "one\n", 1);
        let second = version(&[&first], "Plan", "two\n", 2);
        let refused = block_on(devbox.put(project(), vec![second]));
        assert!(refused.unwrap_err().contains("before its parents"));
    }

    #[test]
    fn large_versions_go_in_several_puts_parents_first() {
        let big = "x".repeat(MAX_NOTE_BODY - 16);
        let mut versions = vec![version(&[], "Plan", &format!("{big}0"), 0)];
        for at in 1..6 {
            let next = version(
                &[versions.last().unwrap()],
                "Plan",
                &format!("{big}{at}"),
                at,
            );
            versions.push(next);
        }
        let by_hash = versions
            .iter()
            .map(|v| (v.hash.clone(), v.clone()))
            .collect();
        let ordered = parents_first(by_hash);
        assert_eq!(ordered, versions);
        let split = batches(ordered);
        assert!(split.len() >= 2);
        assert!(split.iter().all(|batch| {
            batch.len() == 1 || serde_json::to_vec(batch).unwrap().len() <= PUT_BUDGET + 64
        }));
        assert_eq!(split.concat(), versions);
    }
}
