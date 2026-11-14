// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Ranks what a path search finds for the composer's `@` menu. The walk
//! runs where the files are, in `maxd` or in a container's guest, and both
//! rank through here so the order is the same wherever it ran.
//!
//! A name that is the query comes first, then names that start with it,
//! paths that start with it, names and paths that hold it, and last paths
//! that hold its letters in order. Within each, shallower and shorter
//! paths come first. An empty query lists the shallowest entries.

use maxhq_protocol::find::{PathKind, PathMatch, MAX_PATH_MATCHES};

/// Takes candidates as a walk finds them and keeps the best
/// [`MAX_PATH_MATCHES`], so a short query in a big folder doesn't hold
/// every path at once.
pub struct PathRanker {
    /// The query, lowercased, without a leading `./` or `/`.
    query: String,
    found: Vec<(Rank, PathMatch)>,
}

/// Where a candidate sorts. Lower is better.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    tier: u8,
    depth: usize,
    len: usize,
    path: String,
}

impl PathRanker {
    /// A ranker for `query`, matched without regard to case.
    pub fn new(query: &str) -> Self {
        let query = query.trim();
        let query = query.strip_prefix("./").unwrap_or(query);
        Self {
            query: query.trim_start_matches('/').to_lowercase(),
            found: Vec::new(),
        }
    }

    /// Considers `path`, relative to the searched folder with `/` between
    /// parts. It's dropped when it doesn't match.
    pub fn offer(&mut self, path: &str, kind: PathKind) {
        let Some(tier) = tier(&self.query, path) else {
            return;
        };
        let rank = Rank {
            tier,
            depth: path.matches('/').count(),
            len: path.chars().count(),
            path: path.to_owned(),
        };
        self.found.push((
            rank,
            PathMatch {
                path: path.to_owned(),
                kind,
            },
        ));
        if self.found.len() >= MAX_PATH_MATCHES * 8 {
            self.keep_best();
        }
    }

    /// The best matches, best first.
    pub fn finish(mut self) -> Vec<PathMatch> {
        self.keep_best();
        self.found.into_iter().map(|(_, found)| found).collect()
    }

    fn keep_best(&mut self) {
        self.found.sort_by(|a, b| a.0.cmp(&b.0));
        self.found.truncate(MAX_PATH_MATCHES);
    }
}

/// How well `path` matches `query`, or none when it doesn't.
fn tier(query: &str, path: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let path = path.to_lowercase();
    let name = path.rsplit('/').next().unwrap_or(&path);
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if path.starts_with(query) {
        Some(2)
    } else if name.contains(query) {
        Some(3)
    } else if path.contains(query) {
        Some(4)
    } else if in_order(query, &path) {
        Some(5)
    } else {
        None
    }
}

/// Whether every character of `query` appears in `text`, in order.
fn in_order(query: &str, text: &str) -> bool {
    let mut rest = text.chars();
    query.chars().all(|want| rest.any(|got| got == want))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(query: &str, paths: &[(&str, PathKind)]) -> Vec<String> {
        let mut ranker = PathRanker::new(query);
        for (path, kind) in paths {
            ranker.offer(path, *kind);
        }
        ranker
            .finish()
            .into_iter()
            .map(|found| found.path)
            .collect()
    }

    const FILE: PathKind = PathKind::File;
    const FOLDER: PathKind = PathKind::Folder;

    #[test]
    fn names_that_start_with_the_query_come_before_paths_that_hold_it() {
        let paths = [
            ("docs/old-main.md", FILE),
            ("src/main.rs", FILE),
            ("src/domain/mainline.rs", FILE),
            ("main", FOLDER),
            ("src/lib.rs", FILE),
            ("maintenance/notes.txt", FILE),
            ("mxain.txt", FILE),
        ];
        assert_eq!(
            rank("main", &paths),
            [
                "main",
                "src/main.rs",
                "src/domain/mainline.rs",
                "maintenance/notes.txt",
                "docs/old-main.md",
                "mxain.txt",
            ]
        );
    }

    #[test]
    fn case_and_a_leading_dot_slash_dont_matter() {
        let paths = [("src/Main.rs", FILE), ("README.md", FILE)];
        assert_eq!(rank("./MAIN", &paths), ["src/Main.rs"]);
        assert_eq!(rank("readme", &paths), ["README.md"]);
    }

    #[test]
    fn a_query_with_a_folder_finds_whats_inside_it() {
        let paths = [
            ("src", FOLDER),
            ("src/main.rs", FILE),
            ("crates/src/x.rs", FILE),
            ("source.txt", FILE),
        ];
        assert_eq!(rank("src/", &paths), ["src/main.rs", "crates/src/x.rs"]);
    }

    #[test]
    fn an_empty_query_lists_the_shallowest_first() {
        let paths = [
            ("src/deep/file.rs", FILE),
            ("b.txt", FILE),
            ("src", FOLDER),
            ("src/a.rs", FILE),
            ("a.txt", FILE),
        ];
        assert_eq!(
            rank("", &paths),
            ["src", "a.txt", "b.txt", "src/a.rs", "src/deep/file.rs"]
        );
    }

    #[test]
    fn keeps_at_most_fifty_and_the_best_survive() {
        let mut ranker = PathRanker::new("x");
        for n in 0..2_000 {
            ranker.offer(&format!("deep/folder/{n}-x.txt"), FILE);
        }
        ranker.offer("x", FILE);
        let found = ranker.finish();
        assert_eq!(found.len(), MAX_PATH_MATCHES);
        assert_eq!(found[0].path, "x");
    }

    #[test]
    fn letters_in_order_match_last() {
        assert_eq!(rank("mrs", &[("src/main.rs", FILE)]), ["src/main.rs"]);
        assert!(rank("srm", &[("src/lib.rs", FILE)]).is_empty());
    }
}
