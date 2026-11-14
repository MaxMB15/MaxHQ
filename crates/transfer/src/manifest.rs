// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a receiver checks in a manifest before anything touches its disk,
//! and how one offset maps onto the entries laid end to end.

use std::collections::{HashMap, HashSet};

use maxhq_protocol::message::{EntryKind, ErrorCode, ManifestEntry};

use crate::name;
use crate::{Failure, MAX_BYTES, MAX_ENTRIES};

/// One manifest entry after its checks, with its path split into parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The path's parts, in NFC.
    pub parts: Vec<String>,
    /// What it is.
    pub kind: Kind,
    /// Lands as 0755 when set and 0644 otherwise.
    pub executable: bool,
    /// Bytes in the file, and 0 for anything else.
    pub size: u64,
}

/// What a checked entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A file and the SHA-256 its bytes must have.
    File {
        /// The hash.
        sha256: [u8; 32],
    },
    /// A folder.
    Dir,
    /// A symlink and what it points at.
    Symlink {
        /// The target, as it will be written.
        target: String,
    },
}

impl Entry {
    /// The path, `/` separated.
    pub fn path(&self) -> String {
        self.parts.join("/")
    }
}

/// Checks a whole manifest: every path, that no part is a name senders leave
/// out, such as `.git`, that every parent is a folder listed before it, that
/// no name repeats, that each symlink stays inside, and the size and count
/// limits.
pub fn check(entries: &[ManifestEntry]) -> Result<Vec<Entry>, Failure> {
    if entries.is_empty() {
        return Err(Failure::new(
            ErrorCode::BadMessage,
            None,
            "the upload has nothing in it",
        ));
    }
    if entries.len() > MAX_ENTRIES {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            None,
            format!("an upload holds at most {MAX_ENTRIES} files and folders"),
        ));
    }
    let mut total = 0u64;
    let mut seen: HashSet<Vec<String>> = HashSet::new();
    let mut dirs: HashSet<Vec<String>> = HashSet::new();
    let mut links: HashSet<Vec<String>> = HashSet::new();
    let mut checked = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let at = u32::try_from(index).ok();
        let bad = |code, message: String| Failure::new(code, at, message);
        // A path too deep to take is too long to say back.
        let parts = name::check(&entry.path).map_err(|err| match err {
            name::BadPath::TooDeep => bad(ErrorCode::BadPath, err.to_string()),
            err => bad(ErrorCode::BadPath, format!("{}: {err}", entry.path)),
        })?;
        if parts.iter().any(|part| name::skipped(part)) {
            return Err(bad(
                ErrorCode::BadPath,
                format!("{}: MaxHQ doesn't copy a file by that name", entry.path),
            ));
        }
        let parent = &parts[..parts.len() - 1];
        if !parent.is_empty() && !dirs.contains(parent) {
            return Err(bad(
                ErrorCode::BadPath,
                format!(
                    "{} comes before its folder, or its folder is a file",
                    entry.path
                ),
            ));
        }
        if !seen.insert(parts.clone()) {
            return Err(bad(
                ErrorCode::BadPath,
                format!("{} is listed twice", entry.path),
            ));
        }
        let kind = match &entry.kind {
            EntryKind::File => {
                let sha256 = entry.sha256.as_deref().and_then(hex).ok_or_else(|| {
                    bad(ErrorCode::BadMessage, format!("{} has no hash", entry.path))
                })?;
                total = total.saturating_add(entry.size);
                Kind::File { sha256 }
            }
            EntryKind::Dir => {
                dirs.insert(parts.clone());
                Kind::Dir
            }
            EntryKind::Symlink { target } => {
                links.insert(parts.iter().map(|part| name::folded(part)).collect());
                Kind::Symlink {
                    target: target.clone(),
                }
            }
        };
        if !matches!(kind, Kind::File { .. }) && entry.size != 0 {
            return Err(bad(
                ErrorCode::BadMessage,
                format!("{} has a size but isn't a file", entry.path),
            ));
        }
        checked.push(Entry {
            parts,
            kind,
            executable: entry.executable,
            size: entry.size,
        });
    }
    if total > MAX_BYTES {
        return Err(Failure::new(
            ErrorCode::TooLarge,
            None,
            "an upload is at most 2 GiB",
        ));
    }
    // Links can point at entries listed after them, so they're checked
    // once every entry is known. A disk that folds names resolves a target
    // through a link in any spelling, so the compare folds them too.
    let is_link = |path: &[String]| {
        let folded: Vec<String> = path.iter().map(|part| name::folded(part)).collect();
        links.contains(&folded)
    };
    for (index, entry) in checked.iter().enumerate() {
        if let Kind::Symlink { target } = &entry.kind {
            let parent = &entry.parts[..entry.parts.len() - 1];
            if !name::link_stays_inside(parent, target, is_link) {
                return Err(Failure::new(
                    ErrorCode::BadPath,
                    u32::try_from(index).ok(),
                    format!("{} points outside the upload", entry.path()),
                ));
            }
        }
    }
    Ok(checked)
}

/// Checks that a manifest answering a copy of `chosen` paths holds only those:
/// the chosen files, what sits under a chosen folder, and the folders that
/// lead to them. Without this a source could add a file nobody picked, such as
/// a hook or a settings file, and it would land in the new checkout.
pub fn only_chosen(entries: &[ManifestEntry], chosen: &[String]) -> Result<(), Failure> {
    let mut picked = HashSet::new();
    let mut above = HashSet::new();
    for path in chosen {
        // The sender names a path as `name::part` does. One it can't name
        // matches nothing.
        let parts = path
            .split('/')
            .map(name::part)
            .collect::<Result<Vec<_>, _>>();
        let Ok(parts) = parts else { continue };
        for depth in 1..parts.len() {
            above.insert(parts[..depth].to_vec());
        }
        picked.insert(parts);
    }
    for (index, entry) in entries.iter().enumerate() {
        let not_chosen = || {
            Failure::new(
                ErrorCode::BadPath,
                u32::try_from(index).ok(),
                format!("{}: that wasn't chosen for this copy", entry.path),
            )
        };
        // Everything sits under one top folder, which isn't part of a chosen
        // path.
        let mut parts = Vec::new();
        let mut covered = false;
        for part in entry.path.split('/').skip(1) {
            parts.push(part.to_owned());
            if picked.contains(&parts) {
                covered = true;
                break;
            }
            if !above.contains(&parts) {
                return Err(not_chosen());
            }
        }
        if !covered && entry.kind != EntryKind::Dir {
            return Err(not_chosen());
        }
    }
    Ok(())
}

/// The entries at the top of a manifest, which name what was sent.
pub fn tops(entries: &[Entry]) -> Vec<&Entry> {
    entries
        .iter()
        .filter(|entry| entry.parts.len() == 1)
        .collect()
}

fn hex(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        out[index] = u8::try_from(high * 16 + low).ok()?;
    }
    Some(out)
}

/// A hash as the manifest writes it, in lower case hex.
pub fn to_hex(hash: &[u8; 32]) -> String {
    use std::fmt::Write;
    hash.iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Where each entry's bytes start in the transfer, which lays every file's
/// bytes end to end in manifest order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    starts: Vec<u64>,
    sizes: Vec<u64>,
    total: u64,
}

impl Layout {
    /// The layout of these sizes, one per entry and 0 for all but files.
    pub fn new(sizes: impl IntoIterator<Item = u64>) -> Self {
        let sizes: Vec<u64> = sizes.into_iter().collect();
        let mut starts = Vec::with_capacity(sizes.len());
        let mut total = 0u64;
        for size in &sizes {
            starts.push(total);
            total = total.saturating_add(*size);
        }
        Self {
            starts,
            sizes,
            total,
        }
    }

    /// The layout of a protocol manifest.
    pub fn of(entries: &[ManifestEntry]) -> Self {
        Self::new(entries.iter().map(|entry| match entry.kind {
            EntryKind::File => entry.size,
            _ => 0,
        }))
    }

    /// Bytes in the whole transfer.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Where an entry's bytes start.
    pub fn start(&self, entry: usize) -> u64 {
        self.starts.get(entry).copied().unwrap_or(self.total)
    }

    /// Where an entry's bytes end.
    pub fn end(&self, entry: usize) -> u64 {
        self.start(entry) + self.sizes.get(entry).copied().unwrap_or(0)
    }

    /// The entry holding the byte at `offset`, and how far into it that is.
    /// Empty entries hold no bytes, so they never come back.
    pub fn locate(&self, offset: u64) -> Option<(usize, u64)> {
        if offset >= self.total {
            return None;
        }
        // The last entry that starts at or before the offset, skipping
        // empty ones that share its start.
        let index = self.starts.partition_point(|start| *start <= offset);
        let mut entry = index.checked_sub(1)?;
        while self.sizes[entry] == 0 {
            entry = entry.checked_sub(1)?;
        }
        Some((entry, offset - self.starts[entry]))
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }
}

/// The name each file's partial copy takes while it lands: `<name>.part`,
/// or `<name>.<n>.part` when that clashes with an entry in the same folder.
pub fn part_names(entries: &[Entry]) -> Vec<Option<String>> {
    let mut used: HashMap<&[String], HashSet<String>> = HashMap::new();
    for entry in entries {
        let (leaf, parent) = entry.parts.split_last().expect("checked paths have a part");
        used.entry(parent).or_default().insert(leaf.clone());
    }
    entries
        .iter()
        .map(|entry| {
            if !matches!(entry.kind, Kind::File { .. }) {
                return None;
            }
            let (leaf, parent) = entry.parts.split_last().expect("checked paths have a part");
            let names = used.entry(parent).or_default();
            let mut candidate = format!("{leaf}.part");
            let mut number = 1u32;
            while names.contains(&candidate) {
                candidate = format!("{leaf}.{number}.part");
                number += 1;
            }
            names.insert(candidate.clone());
            Some(candidate)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    #[test]
    fn a_manifest_that_nests_too_deep_is_refused_at_the_first_path_past_the_cap() {
        let mut path = "d".to_owned();
        let mut entries = vec![dir(&path)];
        for _ in 1..3953 {
            path.push_str("/d");
            entries.push(dir(&path));
        }
        let refused = check(&entries).unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadPath);
        assert_eq!(refused.entry, Some(name::MAX_DEPTH as u32));
        assert!(refused.message.len() < 100, "{}", refused.message);
        assert!(check(&entries[..name::MAX_DEPTH]).is_ok());
    }

    #[test]
    fn a_link_cant_climb_through_another_link_in_another_spelling() {
        for target in ["l/../../out", "L/../../out", "L\u{200c}/../../out"] {
            let entries = [dir("a"), link("a/l", ".."), link("a/x", target)];
            assert_eq!(code(&entries), (ErrorCode::BadPath, Some(2)), "{target}");
        }
        assert!(check(&[dir("a"), link("a/l", ".."), link("a/x", "../b")]).is_ok());
    }

    fn dir(path: &str) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::Dir,
            executable: false,
            size: 0,
            sha256: None,
        }
    }

    fn file(path: &str, size: u64) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::File,
            executable: false,
            size,
            sha256: Some(HASH.into()),
        }
    }

    fn link(path: &str, target: &str) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            kind: EntryKind::Symlink {
                target: target.into(),
            },
            executable: false,
            size: 0,
            sha256: None,
        }
    }

    fn code(entries: &[ManifestEntry]) -> (ErrorCode, Option<u32>) {
        let failure = check(entries).unwrap_err();
        (failure.code, failure.entry)
    }

    #[test]
    fn a_folder_with_files_and_a_link_passes() {
        let entries = [
            dir("notes"),
            file("notes/run.sh", 4),
            link("notes/latest", "run.sh"),
            dir("notes/sub"),
            link("notes/sub/up", "../run.sh"),
        ];
        let checked = check(&entries).unwrap();
        assert_eq!(checked.len(), 5);
        assert_eq!(checked[1].parts, ["notes", "run.sh"]);
        assert_eq!(
            to_hex(match &checked[1].kind {
                Kind::File { sha256 } => sha256,
                other => panic!("{other:?}"),
            }),
            HASH
        );
        assert_eq!(tops(&checked).len(), 1);
    }

    #[test]
    fn git_and_finder_names_are_refused_in_any_spelling_at_any_depth() {
        for manifest in [
            vec![dir(".git")],
            vec![
                dir(".GIT"),
                dir(".GIT/hooks"),
                file(".GIT/hooks/pre-commit", 1),
            ],
            vec![dir("top"), dir("top/.Git"), file("top/.Git/config", 1)],
            vec![dir("top"), link("top/.g\u{200c}it", "x")],
            vec![dir("top"), file("top/.git\u{feff}", 1)],
            vec![dir("top"), file("top/.DS_STORE", 1)],
        ] {
            let failure = check(&manifest).unwrap_err();
            assert_eq!(failure.code, ErrorCode::BadPath, "{:?}", manifest.last());
        }
        assert!(check(&[dir("top"), file("top/.gitignore", 1), dir("top/.github")]).is_ok());
    }

    #[test]
    fn escapes_and_tricks_are_refused_with_the_entry() {
        assert_eq!(
            code(&[dir("a"), file("a/../../etc/passwd", 1)]),
            (ErrorCode::BadPath, Some(1))
        );
        assert_eq!(
            code(&[file("/etc/passwd", 1)]),
            (ErrorCode::BadPath, Some(0))
        );
        // A file under a link would write through it.
        assert_eq!(
            code(&[dir("a"), link("a/out", ".."), file("a/out/x", 1)]),
            (ErrorCode::BadPath, Some(2))
        );
        // A file under a file, or before its folder.
        assert_eq!(
            code(&[file("a", 1), file("a/b", 1)]),
            (ErrorCode::BadPath, Some(1))
        );
        assert_eq!(
            code(&[file("a/b", 1), dir("a")]),
            (ErrorCode::BadPath, Some(0))
        );
        assert_eq!(
            code(&[dir("a"), link("a/l", "../../x")]),
            (ErrorCode::BadPath, Some(1))
        );
        assert_eq!(
            code(&[dir("a"), link("a/l", "/etc")]),
            (ErrorCode::BadPath, Some(1))
        );
    }

    #[test]
    fn names_that_match_once_in_nfc_are_duplicates() {
        assert_eq!(
            code(&[dir("d"), file("d/caf\u{e9}", 1), file("d/cafe\u{301}", 1)]),
            (ErrorCode::BadPath, Some(2))
        );
    }

    #[test]
    fn files_need_a_hash_and_others_no_size() {
        let mut bare = file("a", 1);
        bare.sha256 = None;
        assert_eq!(code(&[bare]), (ErrorCode::BadMessage, Some(0)));
        let mut short = file("a", 1);
        short.sha256 = Some("abc".into());
        assert_eq!(code(&[short]), (ErrorCode::BadMessage, Some(0)));
        let mut sized = dir("a");
        sized.size = 3;
        assert_eq!(code(&[sized]), (ErrorCode::BadMessage, Some(0)));
        assert_eq!(code(&[]), (ErrorCode::BadMessage, None));
    }

    #[test]
    fn limits_on_count_and_size_hold() {
        let mut many = vec![dir("d")];
        many.extend((0..MAX_ENTRIES).map(|index| file(&format!("d/{index}"), 0)));
        assert_eq!(code(&many), (ErrorCode::TooLarge, None));
        assert_eq!(
            code(&[dir("d"), file("d/a", MAX_BYTES), file("d/b", 1)]),
            (ErrorCode::TooLarge, None)
        );
    }

    #[test]
    fn offsets_map_onto_entries_end_to_end() {
        let layout = Layout::new([0, 10, 0, 0, 5, 0]);
        assert_eq!(layout.total(), 15);
        assert_eq!(layout.locate(0), Some((1, 0)));
        assert_eq!(layout.locate(9), Some((1, 9)));
        assert_eq!(layout.locate(10), Some((4, 0)));
        assert_eq!(layout.locate(14), Some((4, 4)));
        assert_eq!(layout.locate(15), None);
        assert_eq!(layout.start(4), 10);
        assert_eq!(layout.end(4), 15);
        assert_eq!(layout.start(5), 15);
    }

    fn chosen(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|&path| path.to_owned()).collect()
    }

    #[test]
    fn a_copy_of_chosen_files_takes_those_and_the_folders_above_them() {
        let entries = [
            dir("repo"),
            file("repo/.env", 1),
            dir("repo/app"),
            dir("repo/app/deep"),
            file("repo/app/deep/.env.local", 1),
        ];
        let asked = chosen(&[".env", "app/deep/.env.local"]);
        assert!(only_chosen(&entries, &asked).is_ok());
        // Fewer than asked isn't this check's business.
        assert!(only_chosen(&entries[..2], &asked).is_ok());
    }

    #[test]
    fn a_picked_folder_covers_what_is_in_it() {
        let entries = [dir("repo"), dir("repo/build"), file("repo/build/out", 1)];
        assert!(only_chosen(&entries, &chosen(&["build"])).is_ok());
    }

    #[test]
    fn a_chosen_name_in_another_form_still_matches() {
        let entries = [dir("repo"), file("repo/caf\u{e9}.env", 1)];
        assert!(only_chosen(&entries, &chosen(&["cafe\u{301}.env"])).is_ok());
    }

    #[test]
    fn a_copy_of_chosen_files_refuses_what_nobody_chose() {
        let asked = chosen(&[".env", "app/.env.local"]);
        for added in [
            file("repo/.envrc", 1),
            file("repo/.mcp.json", 1),
            file("repo/app/.envrc", 1),
            file("repo/.claude/settings.json", 1),
            dir("repo/.husky"),
            file("repo/.env.extra/x", 1),
        ] {
            let path = added.path.clone();
            let entries = [dir("repo"), file("repo/.env", 1), added];
            let failure = only_chosen(&entries, &asked).unwrap_err();
            assert_eq!(failure.code, ErrorCode::BadPath, "{path}");
            assert_eq!(failure.entry, Some(2), "{path}");
        }
    }

    #[test]
    fn what_leads_to_a_chosen_file_has_to_be_a_folder() {
        let asked = chosen(&["app/.env"]);
        for above in [file("repo/app", 1), link("repo/app", "elsewhere")] {
            assert!(only_chosen(&[dir("repo"), above], &asked).is_err());
        }
        // A manifest that is one file has no folder to hold the chosen ones.
        assert!(only_chosen(&[file("repo", 1)], &asked).is_err());
        assert!(only_chosen(&[dir("repo"), file("repo/x", 1)], &[]).is_err());
    }

    #[test]
    fn partial_names_dodge_real_entries() {
        let checked = check(&[
            dir("d"),
            file("d/video.mp4", 1),
            file("d/video.mp4.part", 1),
        ])
        .unwrap();
        assert_eq!(
            part_names(&checked),
            [
                None,
                Some("video.mp4.1.part".to_owned()),
                Some("video.mp4.part.part".to_owned())
            ]
        );
    }
}
