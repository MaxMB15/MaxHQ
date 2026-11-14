// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The trees a device holds, each in a folder named by its [`TreeHash`].
//! Code that only reads or lands trees takes [`TreeStore`];
//! [`FolderStore`] keeps them in one folder on the disk.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use maxhq_protocol::skills::TreeHash;

use super::disk::DiskFs;
use super::tree;

/// The trees a device holds.
pub trait TreeStore: Send + Sync {
    /// The folder `tree` is in, or would land in, or `None` when `tree`
    /// isn't a hash at all.
    fn path(&self, tree: &TreeHash) -> Option<PathBuf>;

    /// Whether `tree` is in the store.
    fn has(&self, tree: &TreeHash) -> bool {
        self.path(tree).is_some_and(|path| path.is_dir())
    }

    /// Moves the skill folder at `from` into the store as `tree` and says
    /// where it went. The folder must hash to `tree`, and should be on the
    /// store's disk so the move is a rename. When `tree` is already there,
    /// `from` is removed and the stored copy stays.
    fn land(&self, tree: &TreeHash, from: &Path) -> Result<PathBuf, String>;
}

/// Whether `tree` looks like a tree hash: 64 lowercase hex digits, so it
/// can't name a path outside the store.
pub fn well_formed(tree: &TreeHash) -> bool {
    tree.0.len() == 64
        && tree
            .0
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Trees kept as `<root>/<tree>/`.
#[derive(Clone, Debug)]
pub struct FolderStore {
    root: PathBuf,
}

impl FolderStore {
    /// A store in `root`, which is made when the first tree lands.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The folder the trees are in.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl TreeStore for FolderStore {
    fn path(&self, tree: &TreeHash) -> Option<PathBuf> {
        well_formed(tree).then(|| self.root.join(&tree.0))
    }

    fn land(&self, tree: &TreeHash, from: &Path) -> Result<PathBuf, String> {
        let to = self
            .path(tree)
            .ok_or_else(|| format!("{} isn't a skill tree hash.", tree.0))?;
        let found = tree::of(&DiskFs, from).map_err(|err| err.to_string())?;
        if &found != tree {
            return Err(format!(
                "The skill's files don't match {}, so it wasn't stored.",
                tree.0
            ));
        }
        if !to.is_dir() {
            fs::create_dir_all(&self.root)
                .map_err(|err| format!("couldn't make the skill store: {err}"))?;
            match fs::rename(from, &to) {
                Ok(()) => return Ok(to),
                // Another landing of the same tree got there first.
                Err(_) if to.is_dir() => {}
                Err(err) => return Err(format!("couldn't store the skill: {err}")),
            }
        }
        match fs::remove_dir_all(from) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => {
                Err(format!("couldn't remove the extra copy: {err}"))
            }
            _ => Ok(to),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TempDir;

    fn skill(dir: &Path, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn a_tree_lands_once_under_its_hash() {
        let dir = TempDir::new("skill-store");
        let store = FolderStore::new(dir.0.join("store"));
        let from = skill(&dir.0.join("in-1"), "---\nname: unslop\n---\n");
        let tree = tree::of(&DiskFs, &from).unwrap();
        assert!(!store.has(&tree));

        let landed = store.land(&tree, &from).unwrap();
        assert_eq!(landed, dir.0.join("store").join(&tree.0));
        assert!(store.has(&tree) && !from.exists());

        let again = skill(&dir.0.join("in-2"), "---\nname: unslop\n---\n");
        assert_eq!(store.land(&tree, &again).unwrap(), landed);
        assert!(!again.exists());
    }

    #[test]
    fn a_folder_that_doesnt_match_or_a_bad_hash_stays_out() {
        let dir = TempDir::new("skill-store");
        let store = FolderStore::new(dir.0.join("store"));
        let from = skill(&dir.0.join("in"), "changed");
        let other = TreeHash("a".repeat(64));
        assert!(store
            .land(&other, &from)
            .unwrap_err()
            .contains("don't match"));
        assert!(from.exists() && !store.has(&other));

        let escape = TreeHash("../../etc".into());
        assert_eq!(store.path(&escape), None);
        assert!(!store.has(&escape));
        assert!(store.land(&escape, &from).is_err());
        assert!(!well_formed(&TreeHash("A".repeat(64))));
    }
}
