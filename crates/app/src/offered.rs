// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The paths you handed the app. A window can only learn about a path by
//! being handed one: a drop, a paste or the picker. The shell records
//! those here, and every request that reads, stages or uploads a file
//! refuses any other path, so a script in the page can't read your disk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use thiserror::Error;

/// The paths you've handed the app by dropping, pasting or picking them.
/// Nothing else can be described, thumbnailed or attached.
#[derive(Debug, Default)]
pub struct Offered(Mutex<HashSet<PathBuf>>);

/// A path that didn't come from a drop, a paste or the picker.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{path} wasn't dropped, pasted or picked")]
pub struct NotOffered {
    /// The path the window asked about.
    pub path: String,
}

impl Offered {
    /// Remembers paths you handed over.
    pub fn offer(&self, paths: impl IntoIterator<Item = PathBuf>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(paths);
    }

    /// Whether you handed over this exact path.
    pub fn contains(&self, path: &Path) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(path)
    }

    /// The path, if you handed it over.
    pub fn check(&self, path: &str) -> Result<PathBuf, NotOffered> {
        let path = PathBuf::from(path);
        if self.contains(&path) {
            Ok(path)
        } else {
            Err(NotOffered {
                path: path.display().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_offered_paths_pass() {
        let offered = Offered::default();
        offered.offer([PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b")]);
        assert_eq!(offered.check("/tmp/a.png"), Ok(PathBuf::from("/tmp/a.png")));
        // A trailing slash names the same path.
        assert!(offered.check("/tmp/b/").is_ok());
        assert_eq!(
            offered.check("/tmp/b/c.txt"),
            Err(NotOffered {
                path: "/tmp/b/c.txt".to_owned()
            })
        );
        assert!(offered.check("/tmp/../tmp/a.png").is_err());
    }
}
