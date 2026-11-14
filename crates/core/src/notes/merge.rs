// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! How two heads of a note merge. Every device that has the same versions
//! picks the same base, orders the heads the same way and makes the same
//! merge, so they all end at the same hash.

use std::collections::{BTreeMap, BTreeSet};

use maxhq_protocol::notes::{NoteVersion, VersionHash};
use similar::{capture_diff_slices, Algorithm, DiffTag};

use super::sealed;

/// The versions no other version names as a parent, sorted by hash. Give
/// it one note's versions.
#[must_use]
pub fn heads(versions: &[NoteVersion]) -> Vec<&NoteVersion> {
    let named: BTreeSet<&VersionHash> = versions.iter().flat_map(|v| &v.parents).collect();
    let mut heads: Vec<&NoteVersion> = versions
        .iter()
        .filter(|version| !named.contains(&version.hash))
        .collect();
    heads.sort_by(|a, b| a.hash.cmp(&b.hash));
    heads.dedup_by(|a, b| a.hash == b.hash);
    heads
}

/// Every version `from` follows, itself included, among `versions`.
fn ancestors<'a>(
    from: &'a VersionHash,
    versions: &BTreeMap<&'a VersionHash, &'a NoteVersion>,
) -> BTreeSet<&'a VersionHash> {
    let mut seen = BTreeSet::new();
    let mut next = vec![from];
    while let Some(hash) = next.pop() {
        if !seen.insert(hash) {
            continue;
        }
        if let Some(version) = versions.get(hash) {
            next.extend(&version.parents);
        }
    }
    seen
}

/// The version `a` and `b` both follow that's nearest to them. Of several,
/// any that another follows is dropped and the smallest hash of the rest
/// wins. None when they share no history, and the merge starts from an
/// empty note.
#[must_use]
pub fn base<'a>(
    a: &NoteVersion,
    b: &NoteVersion,
    versions: &'a [NoteVersion],
) -> Option<&'a NoteVersion> {
    let by_hash: BTreeMap<&VersionHash, &NoteVersion> = versions
        .iter()
        .map(|version| (&version.hash, version))
        .collect();
    let (Some(a), Some(b)) = (by_hash.get(&a.hash), by_hash.get(&b.hash)) else {
        return None;
    };
    let shared: BTreeSet<&VersionHash> = ancestors(&a.hash, &by_hash)
        .intersection(&ancestors(&b.hash, &by_hash))
        .copied()
        .filter(|hash| by_hash.contains_key(hash))
        .collect();
    // A candidate another candidate follows is further back.
    let mut further = BTreeSet::new();
    for hash in &shared {
        let mut above = ancestors(hash, &by_hash);
        above.remove(hash);
        further.extend(above);
    }
    shared
        .into_iter()
        .find(|hash| !further.contains(hash))
        .and_then(|hash| by_hash.get(hash).copied())
}

/// Merges two heads of a note from `base`, or from an empty note when
/// they share none. The head with the smaller hash is ours.
///
/// - The body merges line by line. A region only one side changed takes
///   that side, one both changed the same way comes once, and one they
///   changed differently keeps our lines and then theirs.
/// - A title only one side changed takes that side, and ours when both
///   did.
/// - An edit beats a delete, so the merge is deleted only when both are,
///   or one is and the other didn't change the base.
/// - The parents are the two heads, it's as late as the later, and it's
///   by no one.
#[must_use]
pub fn merge(a: &NoteVersion, b: &NoteVersion, base: Option<&NoteVersion>) -> NoteVersion {
    let (ours, theirs) = if a.hash <= b.hash { (a, b) } else { (b, a) };
    let (base_title, base_body, base_deleted) = base.map_or(("", "", false), |base| {
        (base.title.as_str(), base.body.as_str(), base.deleted)
    });
    let title = if ours.title == base_title {
        theirs.title.clone()
    } else {
        ours.title.clone()
    };
    let body = merge_text(base_body, &ours.body, &theirs.body);
    let unchanged = |side: &NoteVersion| {
        side.title == base_title && side.body == base_body && side.deleted == base_deleted
    };
    let deleted = if ours.deleted {
        theirs.deleted || unchanged(theirs)
    } else {
        theirs.deleted && unchanged(ours)
    };
    sealed(NoteVersion {
        hash: VersionHash(String::new()),
        project: ours.project,
        note: ours.note,
        parents: vec![ours.hash.clone(), theirs.hash.clone()],
        title,
        body,
        deleted,
        at: ours.at.max(theirs.at),
        by: None,
    })
}

/// Merges a note's heads into one: the two with the smallest hashes, then
/// that merge with the next, in hash order. Gives the merges it made, the
/// last of which is the head, and none when there's one head already.
#[must_use]
pub fn settle(versions: &[NoteVersion]) -> Vec<NoteVersion> {
    let mut all = versions.to_vec();
    let order: Vec<NoteVersion> = heads(versions).into_iter().cloned().collect();
    let mut made = Vec::new();
    let mut heads = order.into_iter();
    let Some(mut head) = heads.next() else {
        return made;
    };
    for next in heads {
        let merged = merge(&head, &next, base(&head, &next, &all));
        all.push(merged.clone());
        made.push(merged.clone());
        head = merged;
    }
    made
}

/// Where one side's change sits: the base lines it replaces and the lines
/// it puts there.
#[derive(Clone, Copy, Debug)]
struct Hunk {
    /// 0 for ours, 1 for theirs.
    side: usize,
    base: (usize, usize),
    lines: (usize, usize),
}

/// A text's lines, each with its newline, the last maybe without.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The hunks that turn `base` into `side`.
fn hunks(side: usize, base: &[&str], lines: &[&str]) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, base, lines) {
        let (tag, old, new) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        // Back-to-back changes are one hunk.
        match hunks.last_mut() {
            Some(last) if last.base.1 == old.start && last.lines.1 == new.start => {
                last.base.1 = old.end;
                last.lines.1 = new.end;
            }
            _ => hunks.push(Hunk {
                side,
                base: (old.start, old.end),
                lines: (new.start, new.end),
            }),
        }
    }
    hunks
}

/// A three-way merge of `ours` and `theirs` from `base`, line by line, with
/// no conflict markers.
fn merge_text(base: &str, ours: &str, theirs: &str) -> String {
    let base = lines(base);
    let sides = [lines(ours), lines(theirs)];
    let mut all = hunks(0, &base, &sides[0]);
    all.extend(hunks(1, &base, &sides[1]));
    // Inserts sort before a change at the same line, so an insert just
    // above a changed line stays apart from it.
    all.sort_by_key(|hunk| (hunk.base.0, hunk.base.1, hunk.side));

    let mut out = String::new();
    let mut at = 0;
    let mut i = 0;
    while i < all.len() {
        let (start, mut end) = all[i].base;
        let mut region = vec![all[i]];
        i += 1;
        while let Some(next) = all.get(i) {
            let overlaps = next.base.0 < end;
            let same_insert = start == end && next.base == (start, end);
            if !overlaps && !same_insert {
                break;
            }
            end = end.max(next.base.1);
            region.push(*next);
            i += 1;
        }
        out.extend(base[at..start].iter().copied());
        at = end;

        // Each side's lines for the region, widened by the base lines its
        // own hunks don't cover.
        let text = |side: usize| -> String {
            let mine: Vec<&Hunk> = region.iter().filter(|hunk| hunk.side == side).collect();
            let (Some(first), Some(last)) = (mine.first(), mine.last()) else {
                return base[start..end].concat();
            };
            let from = first.lines.0 - (first.base.0 - start);
            let to = last.lines.1 + (end - last.base.1);
            sides[side][from..to].concat()
        };
        let (o, a, b) = (base[start..end].concat(), text(0), text(1));
        if a == b || b == o {
            out.push_str(&a);
        } else if a == o {
            out.push_str(&b);
        } else {
            out.push_str(&a);
            if !a.is_empty() && !a.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&b);
        }
    }
    out.extend(base[at..].iter().copied());
    out
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::notes::NoteVersion;

    use super::*;
    use crate::notes::check;
    use crate::notes::tests::version;

    /// Merges two heads the way a device holding `versions` does.
    fn merged(a: &NoteVersion, b: &NoteVersion, versions: &[NoteVersion]) -> NoteVersion {
        merge(a, b, base(a, b, versions))
    }

    #[test]
    fn edits_to_different_lines_both_land_in_either_order() {
        let first = version(&[], "Plan", "one\ntwo\nthree\nfour\n", 1);
        let a = version(&[&first], "Plan", "ONE\ntwo\nthree\nfour\n", 2);
        let b = version(&[&first], "Plan", "one\ntwo\nthree\nFOUR\n", 3);
        let all = [first, a.clone(), b.clone()];
        let ab = merged(&a, &b, &all);
        assert_eq!(ab.body, "ONE\ntwo\nthree\nFOUR\n");
        assert_eq!(merged(&b, &a, &all), ab);
        assert_eq!(check(&ab), Ok(()));
        assert_eq!(ab.at.0, 3);
        assert_eq!(ab.by, None);
        let mut parents = vec![a.hash.clone(), b.hash.clone()];
        parents.sort();
        assert_eq!(ab.parents, parents);
    }

    #[test]
    fn edits_to_neighboring_lines_both_land() {
        let first = version(&[], "Plan", "one\ntwo\nthree\n", 1);
        let a = version(&[&first], "Plan", "ONE\ntwo\nthree\n", 2);
        let b = version(&[&first], "Plan", "one\nTWO\nthree\n", 3);
        let all = [first, a.clone(), b.clone()];
        assert_eq!(merged(&a, &b, &all).body, "ONE\nTWO\nthree\n");
    }

    #[test]
    fn the_same_edit_on_both_sides_comes_once() {
        let first = version(&[], "Plan", "one\ntwo\n", 1);
        let a = version(&[&first], "Plan", "one\ntwo\nthree\n", 2);
        let b = version(&[&first], "Plan", "one\ntwo\nthree\n", 3);
        // The same text after the same parent is the same version.
        assert_eq!(a.hash, b.hash);
        let c = version(&[&first], "Plan 2", "one\ntwo\nthree\n", 3);
        let all = [first, a.clone(), c.clone()];
        let ac = merged(&a, &c, &all);
        assert_eq!(ac.body, "one\ntwo\nthree\n");
        assert_eq!(ac.title, "Plan 2");
    }

    #[test]
    fn different_edits_to_one_line_keep_both_ours_first() {
        let first = version(&[], "Plan", "one\ntwo\nthree\n", 1);
        let a = version(&[&first], "Mine", "one\nA\nthree\n", 2);
        let b = version(&[&first], "Yours", "one\nB\nthree\n", 3);
        let all = [first, a.clone(), b.clone()];
        let (ours, theirs) = if a.hash < b.hash { (&a, &b) } else { (&b, &a) };
        let ab = merged(&a, &b, &all);
        let line = |v: &NoteVersion| v.body.lines().nth(1).unwrap().to_owned();
        assert_eq!(
            ab.body,
            format!("one\n{}\n{}\nthree\n", line(ours), line(theirs))
        );
        assert_eq!(ab.title, ours.title, "both changed the title, so ours");
    }

    #[test]
    fn a_last_line_without_a_newline_stays_apart() {
        let first = version(&[], "Plan", "one\ntwo", 1);
        let a = version(&[&first], "Plan", "one\nA", 2);
        let b = version(&[&first], "Plan", "one\nB", 3);
        let all = [first, a.clone(), b.clone()];
        let body = merged(&a, &b, &all).body;
        assert!(body == "one\nA\nB" || body == "one\nB\nA", "{body:?}");
    }

    #[test]
    fn an_edit_beats_a_delete() {
        let first = version(&[], "Plan", "one\n", 1);
        let edited = version(&[&first], "Plan", "one\ntwo\n", 2);
        let mut deleted = first.clone();
        deleted.parents = vec![first.hash.clone()];
        deleted.deleted = true;
        deleted.at.0 = 3;
        let deleted = crate::notes::sealed(deleted);
        let all = [first.clone(), edited.clone(), deleted.clone()];
        let kept = merged(&edited, &deleted, &all);
        assert!(!kept.deleted);
        assert_eq!(kept.body, "one\ntwo\n");

        // Against a side that left the base as it was, the delete stands.
        let untouched = version(&[&first], "Plan", "one\n", 5);
        let all = [first, untouched.clone(), deleted.clone()];
        assert!(merged(&untouched, &deleted, &all).deleted);
    }

    #[test]
    fn notes_made_on_two_devices_merge_from_an_empty_one() {
        let a = version(&[], "Ideas", "alpha\n", 1);
        let b = version(&[], "Ideas", "beta\n", 2);
        let all = [a.clone(), b.clone()];
        assert_eq!(base(&a, &b, &all), None);
        let body = merged(&a, &b, &all).body;
        assert!(
            body == "alpha\nbeta\n" || body == "beta\nalpha\n",
            "{body:?}"
        );
    }

    #[test]
    fn merging_the_same_pair_twice_gives_the_same_hash() {
        let first = version(&[], "Plan", "one\n", 1);
        let a = version(&[&first], "Plan", "one\nA\n", 2);
        let b = version(&[&first], "Plan", "B\none\n", 3);
        let all = [first, a.clone(), b.clone()];
        let once = merged(&a, &b, &all);
        let twice = merged(&a, &b, &all);
        assert_eq!(once.hash, twice.hash);
        assert_eq!(once.body, "B\none\nA\n");
    }

    #[test]
    fn a_criss_cross_history_still_converges() {
        // Two devices each merge the other's head before seeing the other
        // merge, so the next merge has two nearest common ancestors.
        let first = version(&[], "Plan", "one\ntwo\nthree\n", 1);
        let a = version(&[&first], "Plan", "ONE\ntwo\nthree\n", 2);
        let b = version(&[&first], "Plan", "one\ntwo\nTHREE\n", 3);
        let mut a2 = merge(&a, &b, Some(&first));
        a2.body = "ONE\ntwo\nTHREE\nfour\n".into();
        let a2 = crate::notes::sealed(a2);
        let b2 = version(&[&a, &b], "Plan", "ONE\nTWO\nTHREE\n", 5);
        let all = [first, a.clone(), b.clone(), a2.clone(), b2.clone()];
        let base_of = base(&a2, &b2, &all).unwrap();
        assert!(base_of.hash == a.hash || base_of.hash == b.hash);
        assert_eq!(base_of.hash, a.hash.clone().min(b.hash.clone()));
        let one = merged(&a2, &b2, &all);
        let two = merged(&b2, &a2, &all);
        assert_eq!(one.hash, two.hash);
        assert_eq!(heads(&all).len(), 2);
        let made = settle(&all);
        assert_eq!(made.last().unwrap().hash, one.hash);
    }

    #[test]
    fn three_heads_settle_into_one_whatever_order_they_arrive_in() {
        let first = version(&[], "Plan", "one\ntwo\nthree\n", 1);
        let a = version(&[&first], "Plan", "ONE\ntwo\nthree\n", 2);
        let b = version(&[&first], "Plan", "one\nTWO\nthree\n", 3);
        let c = version(&[&first], "Plan", "one\ntwo\nTHREE\n", 4);
        let forward = [first.clone(), a.clone(), b.clone(), c.clone()];
        let backward = [c, b, a, first];
        let one = settle(&forward);
        let two = settle(&backward);
        assert_eq!(one.len(), 2);
        assert_eq!(one, two);
        let head = one.last().unwrap();
        assert_eq!(head.body, "ONE\nTWO\nTHREE\n");
        let mut all = forward.to_vec();
        all.extend(one.clone());
        assert_eq!(heads(&all), vec![head]);
        assert!(settle(&all).is_empty());
    }
}
