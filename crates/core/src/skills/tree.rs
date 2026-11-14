// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A skill folder's [`TreeHash`], over the same records a transfer
//! manifest carries: each file's path, executable bit and SHA-256, sorted
//! by path. Folders don't count, Finder files and `.git` are dropped, and
//! a link anywhere in a skill is refused, so a tree means the same files
//! on every device.

use std::path::Path;

use maxhq_protocol::message::{EntryKind, ManifestEntry};
use maxhq_protocol::skills::TreeHash;
use thiserror::Error;

use super::{ignored, sha256_hex, Node, SkillFs};

/// The most files and folders [`read`] takes from one skill.
pub const MAX_ENTRIES: usize = 10_000;

/// The most bytes [`read`] takes from one skill.
pub const MAX_BYTES: u64 = 8_000_000;

/// Why a folder has no tree hash.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TreeError {
    /// A link inside the skill.
    #[error("{0} is a link, and skills can't hold links.")]
    Link(String),
    /// A device, a pipe or a socket inside the skill.
    #[error("{0} isn't a file or a folder.")]
    Special(String),
    /// A path that isn't relative and plain.
    #[error("{0} isn't a plain relative path.")]
    BadPath(String),
    /// A file whose hash the manifest leaves out.
    #[error("{0} has no hash.")]
    NoHash(String),
    /// The same path twice.
    #[error("{0} is listed twice.")]
    Twice(String),
    /// More than [`MAX_ENTRIES`] entries or [`MAX_BYTES`] bytes.
    #[error("The skill is too large.")]
    TooLarge,
    /// A read failed.
    #[error("{path} couldn't be read: {reason}")]
    Read {
        /// The path, inside the skill.
        path: String,
        /// What went wrong.
        reason: String,
    },
}

/// Whether a manifest path is one a skill never carries, because a part
/// of it is [`ignored`].
pub fn dropped(path: &str) -> bool {
    path.split('/').any(ignored)
}

/// The tree hash of a skill's manifest, whose paths are relative to the
/// skill folder. Dropped paths don't count; a link is refused.
pub fn hash(entries: &[ManifestEntry]) -> Result<TreeHash, TreeError> {
    let mut files: Vec<(&str, bool, &str)> = Vec::new();
    for entry in entries {
        let path = entry.path.as_str();
        if path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".." || part.contains('\0'))
        {
            return Err(TreeError::BadPath(entry.path.clone()));
        }
        if dropped(path) {
            continue;
        }
        match &entry.kind {
            EntryKind::Dir => {}
            EntryKind::Symlink { .. } => return Err(TreeError::Link(entry.path.clone())),
            EntryKind::File => {
                let sha = entry
                    .sha256
                    .as_deref()
                    .ok_or_else(|| TreeError::NoHash(entry.path.clone()))?;
                files.push((path, entry.executable, sha));
            }
        }
    }
    files.sort_unstable_by(|a, b| a.0.cmp(b.0));
    if let Some(pair) = files.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(TreeError::Twice(pair[0].0.to_owned()));
    }
    let mut records = String::new();
    for (path, executable, sha) in files {
        records.push_str(path);
        records.push('\0');
        records.push(if executable { 'x' } else { '-' });
        records.push('\0');
        records.push_str(&sha.to_ascii_lowercase());
        records.push('\n');
    }
    Ok(TreeHash(sha256_hex(records.as_bytes())))
}

/// The manifest of the skill folder at `dir`, with each file hashed,
/// sorted by path and without dropped paths. `dir` itself may be a link,
/// as a linked skill is; anything inside it may not.
pub fn read(fs: &dyn SkillFs, dir: &Path) -> Result<Vec<ManifestEntry>, TreeError> {
    let failed = |path: &str, err: std::io::Error| TreeError::Read {
        path: path.to_owned(),
        reason: err.to_string(),
    };
    let mut entries = Vec::new();
    let mut bytes = 0u64;
    let mut todo = vec![String::new()];
    while let Some(folder) = todo.pop() {
        let mut names = fs
            .list(&dir.join(&folder))
            .map_err(|err| failed(&folder, err))?;
        names.sort_unstable();
        for name in names {
            if ignored(&name) {
                continue;
            }
            let path = if folder.is_empty() {
                name
            } else {
                format!("{folder}/{name}")
            };
            if entries.len() >= MAX_ENTRIES {
                return Err(TreeError::TooLarge);
            }
            let full = dir.join(&path);
            match fs.node(&full).map_err(|err| failed(&path, err))? {
                None => {}
                Some(Node::Link { .. }) => return Err(TreeError::Link(path)),
                Some(Node::Other) => return Err(TreeError::Special(path)),
                Some(Node::Dir) => {
                    entries.push(entry(path.clone(), EntryKind::Dir, false, 0, None));
                    todo.push(path);
                }
                Some(Node::File { size, executable }) => {
                    bytes += size;
                    if bytes > MAX_BYTES {
                        return Err(TreeError::TooLarge);
                    }
                    let content = fs.read(&full, size).map_err(|err| failed(&path, err))?;
                    let sha = sha256_hex(&content);
                    entries.push(entry(path, EntryKind::File, executable, size, Some(sha)));
                }
            }
        }
    }
    entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// The tree hash of the skill folder at `dir`.
pub fn of(fs: &dyn SkillFs, dir: &Path) -> Result<TreeHash, TreeError> {
    hash(&read(fs, dir)?)
}

fn entry(
    path: String,
    kind: EntryKind,
    executable: bool,
    size: u64,
    sha256: Option<String>,
) -> ManifestEntry {
    ManifestEntry {
        path,
        kind,
        executable,
        size,
        sha256,
    }
}

#[cfg(test)]
mod tests {
    use super::super::mem::MemFs;
    use super::*;

    fn file(path: &str, sha: &str) -> ManifestEntry {
        entry(path.into(), EntryKind::File, false, 1, Some(sha.into()))
    }

    #[test]
    fn the_hash_is_stable_across_file_order() {
        let a = [file("SKILL.md", "aa"), file("refs/x.md", "bb")];
        let b = [
            file("refs/x.md", "bb"),
            entry("refs".into(), EntryKind::Dir, false, 0, None),
            file("SKILL.md", "AA"),
        ];
        assert_eq!(hash(&a).unwrap(), hash(&b).unwrap());
        assert_eq!(hash(&a).unwrap().0.len(), 64);
    }

    #[test]
    fn finder_files_and_git_are_ignored() {
        let base = [file("SKILL.md", "aa")];
        let noisy = [
            file("SKILL.md", "aa"),
            file(".DS_Store", "11"),
            file("._SKILL.md", "22"),
            file(".git/HEAD", "33"),
            file(".GIT/hooks/pre-commit", "44"),
        ];
        assert_eq!(hash(&base).unwrap(), hash(&noisy).unwrap());
    }

    #[test]
    fn a_link_is_refused() {
        let entries = [
            file("SKILL.md", "aa"),
            entry(
                "x".into(),
                EntryKind::Symlink {
                    target: "/etc".into(),
                },
                false,
                0,
                None,
            ),
        ];
        assert_eq!(hash(&entries), Err(TreeError::Link("x".into())));
    }

    #[test]
    fn an_executable_bit_changes_the_hash() {
        let plain = [file("run.sh", "aa")];
        let mut exec = plain.clone();
        exec[0].executable = true;
        assert_ne!(hash(&plain).unwrap(), hash(&exec).unwrap());
    }

    #[test]
    fn content_changes_the_hash() {
        assert_ne!(
            hash(&[file("SKILL.md", "aa")]).unwrap(),
            hash(&[file("SKILL.md", "ab")]).unwrap()
        );
    }

    #[test]
    fn bad_paths_and_missing_hashes_are_refused() {
        for path in ["/abs", "a/../b", "a//b", "./a", ""] {
            assert_eq!(
                hash(&[file(path, "aa")]),
                Err(TreeError::BadPath(path.into())),
                "{path}"
            );
        }
        let mut no_hash = file("SKILL.md", "aa");
        no_hash.sha256 = None;
        assert_eq!(hash(&[no_hash]), Err(TreeError::NoHash("SKILL.md".into())));
        assert_eq!(
            hash(&[file("a", "1"), file("a", "2")]),
            Err(TreeError::Twice("a".into()))
        );
    }

    #[test]
    fn a_folder_reads_into_the_manifest_it_hashes() {
        let mut fs = MemFs::new();
        fs.file("/store/x/SKILL.md", "hello")
            .exec("/store/x/scripts/run.sh", "#!/bin/sh\n")
            .file("/store/x/.DS_Store", "junk")
            .link("/work/repo/.claude/skills/x", "/store/x");
        let entries = read(&fs, Path::new("/work/repo/.claude/skills/x")).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["SKILL.md", "scripts", "scripts/run.sh"]);
        assert_eq!(
            entries[0].sha256.as_deref(),
            Some(sha256_hex(b"hello").as_str())
        );
        assert!(entries[2].executable);
        assert_eq!(
            of(&fs, Path::new("/store/x")).unwrap(),
            hash(&entries).unwrap()
        );
    }

    #[test]
    fn a_link_inside_a_folder_is_refused() {
        let mut fs = MemFs::new();
        fs.file("/store/x/SKILL.md", "hello")
            .link("/store/x/secrets", "/home/me/.ssh");
        assert_eq!(
            of(&fs, Path::new("/store/x")),
            Err(TreeError::Link("secrets".into()))
        );
    }
}
