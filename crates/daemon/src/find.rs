// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The walk behind the composer's `@` menu. It runs where the files are:
//! in host `maxd` for a folder on the device, and in `maxd guest` for a
//! container. It skips what `.gitignore` leaves out and `.git` itself, and
//! stops after [`MAX_VISITS`] entries or [`MAX_WALK`], so a keystroke in a
//! huge folder still answers quickly. Nothing is kept between searches.

use std::path::{Component, Path};
use std::time::{Duration, Instant};

use ignore::WalkBuilder;
use maxhq_core::find::PathRanker;
use maxhq_protocol::find::{PathKind, PathMatch, MAX_PATH_MATCHES};

/// The most entries one search looks at.
const MAX_VISITS: usize = 100_000;

/// The longest one search walks.
const MAX_WALK: Duration = Duration::from_millis(1_500);

/// How deep a search goes below the folder.
const MAX_DEPTH: usize = 32;

/// How deep a search with an empty query goes, which lists what's near
/// the top.
const SHALLOW_DEPTH: usize = 2;

/// How many entries go by between looks at the clock.
const CLOCK_EVERY: usize = 512;

/// The longest query taken, in characters.
const MAX_QUERY: usize = 256;

/// Finds the files and folders under `root` that match `query`, best
/// first, as paths relative to `root` with `/` between parts. It blocks, so
/// callers run it off the async threads.
pub fn find_paths(root: &Path, query: &str) -> Result<Vec<PathMatch>, String> {
    if !root.is_absolute() {
        return Err("the agent's folder isn't an absolute path".to_owned());
    }
    if !root.is_dir() {
        return Err(format!("{} isn't a folder here", root.display()));
    }
    let query: String = query.chars().take(MAX_QUERY).collect();
    let depth = if query.trim().is_empty() {
        SHALLOW_DEPTH
    } else {
        MAX_DEPTH
    };
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        // A folder that isn't a checkout still has its .gitignore honored.
        .require_git(false)
        .follow_links(false)
        .max_depth(Some(depth))
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let started = Instant::now();
    let mut ranker = PathRanker::new(&query);
    for (seen, entry) in walk.enumerate() {
        if seen >= MAX_VISITS
            || (seen % CLOCK_EVERY == 0 && seen > 0 && started.elapsed() >= MAX_WALK)
        {
            break;
        }
        // A folder that can't be read is passed over.
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(path) = relative(root, entry.path()) else {
            continue;
        };
        let kind = match entry.file_type() {
            Some(kind) if kind.is_dir() => PathKind::Folder,
            Some(kind) if kind.is_symlink() && entry.path().is_dir() => PathKind::Folder,
            _ => PathKind::File,
        };
        ranker.offer(&path, kind);
    }
    let mut found = ranker.finish();
    found.truncate(MAX_PATH_MATCHES);
    Ok(found)
}

/// `path` relative to `root`, with `/` between parts. None for a name
/// that isn't UTF-8.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for part in rest.components() {
        match part {
            Component::Normal(name) => parts.push(name.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Checks paths a guest found, since the host trusts nothing from it: at
/// most [`MAX_PATH_MATCHES`], each relative, inside the folder and short.
pub fn checked(found: Vec<PathMatch>) -> Result<Vec<PathMatch>, String> {
    if found.len() > MAX_PATH_MATCHES {
        return Err("the guest found more paths than it may send".to_owned());
    }
    for one in &found {
        let path = Path::new(&one.path);
        let inside = !one.path.is_empty()
            && one.path.len() <= 4_096
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_)));
        if !inside {
            return Err("the guest sent a path outside the agent's folder".to_owned());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::TempDir;

    fn tree() -> TempDir {
        let dir = TempDir::new();
        let root = dir.path();
        for folder in ["src/domain", "target/debug", ".git/objects", "docs"] {
            fs::create_dir_all(root.join(folder)).unwrap();
        }
        for file in [
            "src/main.rs",
            "src/domain/mainline.rs",
            "target/debug/main",
            ".git/objects/main",
            "docs/notes.md",
            ".env.example",
            "Cargo.toml",
        ] {
            fs::write(root.join(file), "x").unwrap();
        }
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        dir
    }

    fn paths(found: &[PathMatch]) -> Vec<&str> {
        found.iter().map(|one| one.path.as_str()).collect()
    }

    #[test]
    fn ignored_files_and_git_itself_stay_out() {
        let dir = tree();
        let found = find_paths(dir.path(), "main").unwrap();
        assert_eq!(
            paths(&found),
            ["src/main.rs", "src/domain/mainline.rs", "src/domain"]
        );
        assert_eq!(found[0].kind, PathKind::File);
    }

    #[test]
    fn hidden_files_are_found_and_folders_say_so() {
        let dir = tree();
        let found = find_paths(dir.path(), ".env").unwrap();
        assert_eq!(paths(&found), [".env.example"]);
        let found = find_paths(dir.path(), "domain").unwrap();
        assert_eq!(found[0].path, "src/domain");
        assert_eq!(found[0].kind, PathKind::Folder);
    }

    #[test]
    fn an_empty_query_lists_whats_near_the_top() {
        let dir = tree();
        let found = find_paths(dir.path(), "").unwrap();
        let found = paths(&found);
        assert!(found.contains(&"docs"));
        assert!(found.contains(&"src/main.rs"));
        assert!(!found.contains(&"src/domain/mainline.rs"));
        assert!(!found.iter().any(|path| path.starts_with("target")));
    }

    #[test]
    fn a_missing_or_relative_folder_is_refused() {
        let dir = tree();
        assert!(find_paths(&dir.path().join("nothing"), "x").is_err());
        assert!(find_paths(Path::new("relative/to"), "x").is_err());
    }

    #[test]
    fn a_guests_paths_must_stay_inside() {
        let one = |path: &str| PathMatch {
            path: path.into(),
            kind: PathKind::File,
        };
        assert!(checked(vec![one("src/main.rs")]).is_ok());
        assert!(checked(vec![one("/etc/passwd")]).is_err());
        assert!(checked(vec![one("../up.txt")]).is_err());
        assert!(checked(vec![one("")]).is_err());
        assert!(checked(vec![one("a"); MAX_PATH_MATCHES + 1]).is_err());
    }
}
