// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The device's skill store: core's [`FolderStore`] under maxd's data
//! folder. A landed tree is made read-only, so an agent can't change a
//! skill another project shares. The sweep removes trees nothing has
//! named for [`KEEP_FOR`].

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use maxhq_core::skills::store::{well_formed, FolderStore, TreeStore};
use maxhq_os::fsmode;
use maxhq_protocol::skills::TreeHash;
use tracing::warn;

use super::held::Held;

/// How long a tree stays once no lock or set names it.
pub const KEEP_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The trees this device holds, in `<data>/skills/store/<tree>/`.
#[derive(Clone, Debug)]
pub struct SkillStore {
    folders: FolderStore,
    /// Where copies wait to land, on the store's disk.
    scratch: PathBuf,
}

impl SkillStore {
    /// The store under maxd's data folder.
    pub fn under(data: &Path) -> Self {
        let skills = data.join("skills");
        Self {
            folders: FolderStore::new(skills.join("store")),
            scratch: skills.join("incoming"),
        }
    }

    /// Copies the skill folder at `folder` into the store and gives its
    /// tree and what was read. The folder itself is left as it was. The
    /// folder is read once, so what's stored is what was hashed.
    pub fn copy_in(&self, folder: &Path) -> Result<(TreeHash, Held), String> {
        let held = Held::at(folder).map_err(|err| err.to_string())?;
        let found = held.tree().map_err(|err| err.to_string())?;
        if self.has(&found) {
            touch(&self.folders.root().join(&found.0));
            return Ok((found, held));
        }
        let to = self.scratch.join(uuid::Uuid::now_v7().simple().to_string());
        let copied = held
            .write(&to)
            .map_err(|err| format!("couldn't copy the skill: {err}"))
            .and_then(|()| self.land(&found, &to));
        if copied.is_err() {
            let _ = fs::remove_dir_all(&to);
        }
        copied.map(|_| (found, held))
    }

    /// Removes each tree not in `keep` that nothing kept for
    /// [`KEEP_FOR`] before `now`, and gives the ones it removed. Each kept
    /// tree's folder time moves to `now`, so it marks when a lock or set
    /// last named it.
    pub fn sweep(&self, keep: &HashSet<TreeHash>, now: SystemTime) -> Vec<TreeHash> {
        let mut removed = Vec::new();
        let Ok(found) = fs::read_dir(self.folders.root()) else {
            return removed;
        };
        for entry in found.flatten() {
            let tree = TreeHash(entry.file_name().to_string_lossy().into_owned());
            if !well_formed(&tree) {
                continue;
            }
            let path = entry.path();
            if keep.contains(&tree) {
                set_time(&path, now);
                continue;
            }
            let seen = entry.metadata().and_then(|meta| meta.modified());
            let stale =
                seen.is_ok_and(|seen| now.duration_since(seen).unwrap_or_default() >= KEEP_FOR);
            if stale {
                match remove(&path) {
                    Ok(()) => removed.push(tree),
                    Err(error) => warn!(tree = %tree.0, %error, "couldn't sweep a skill tree"),
                }
            }
        }
        // A copy left halfway by a stopped landing.
        if let Ok(found) = fs::read_dir(&self.scratch) {
            for entry in found.flatten() {
                let _ = remove(&entry.path());
            }
        }
        removed
    }
}

impl TreeStore for SkillStore {
    fn path(&self, tree: &TreeHash) -> Option<PathBuf> {
        self.folders.path(tree)
    }

    fn land(&self, tree: &TreeHash, from: &Path) -> Result<PathBuf, String> {
        let existed = self.has(tree);
        let to = self.folders.land(tree, from)?;
        if existed {
            touch(&to);
        } else {
            seal(&to).map_err(|err| format!("couldn't make the skill read-only: {err}"))?;
        }
        Ok(to)
    }
}

/// Makes every file and folder under `dir` read-only, keeping each
/// file's executable bit. Windows has no modes, so it leaves them be.
fn seal(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            seal(&path)?;
        } else if meta.is_file() {
            let mode = if fsmode::mode_of(&meta) & 0o111 != 0 {
                0o555
            } else {
                0o444
            };
            fsmode::set_mode(&path, mode)?;
        }
    }
    fsmode::set_mode(dir, 0o555)
}

/// Removes a sealed folder, opening each of its folders first.
fn remove(dir: &Path) -> io::Result<()> {
    fn open(dir: &Path) -> io::Result<()> {
        fsmode::set_mode(dir, 0o755)?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                open(&entry.path())?;
            }
        }
        Ok(())
    }
    match open(dir) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        _ => {}
    }
    fs::remove_dir_all(dir)
}

fn touch(dir: &Path) {
    set_time(dir, SystemTime::now());
}

fn set_time(dir: &Path, at: SystemTime) {
    if let Err(error) = fsmode::set_modified(dir, at) {
        warn!(path = %dir.display(), %error, "couldn't mark a skill tree as kept");
    }
}

#[cfg(test)]
impl SkillStore {
    /// Opens every tree so the test's temp folder can go.
    pub fn open_up(&self) {
        let _ = remove(self.folders.root());
    }
}

#[cfg(test)]
mod tests {
    use maxhq_core::skills::disk::DiskFs;
    use maxhq_core::skills::tree;

    use super::*;
    use crate::testing::TempDir;

    fn skill(dir: &Path, body: &str) -> PathBuf {
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
        fs::write(dir.join("scripts/run.sh"), "echo hi\n").unwrap();
        fsmode::set_mode(&dir.join("scripts/run.sh"), 0o755).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn a_copied_skill_lands_read_only_and_keeps_its_source() {
        let dir = TempDir::new();
        let store = SkillStore::under(dir.path());
        let from = skill(&dir.path().join("mine"), "---\nname: mine\n---\n");

        let (tree, held) = store.copy_in(&from).unwrap();
        assert_eq!(tree, tree::of(&DiskFs, &from).unwrap());
        assert_eq!(held.entries.len(), 3);
        assert!(from.join("SKILL.md").is_file());

        let landed = store.path(&tree).unwrap();
        assert_eq!(tree::of(&DiskFs, &landed).unwrap(), tree);
        // Windows has no modes to seal a tree with.
        if cfg!(unix) {
            let mode = |path: &Path| fsmode::mode_of(&fs::metadata(path).unwrap()) & 0o777;
            assert_eq!(mode(&landed), 0o555);
            assert_eq!(mode(&landed.join("SKILL.md")), 0o444);
            assert_eq!(mode(&landed.join("scripts/run.sh")), 0o555);
            assert!(fs::write(landed.join("SKILL.md"), "changed").is_err());
        }

        // The same files again land nowhere new.
        assert_eq!(store.copy_in(&from).unwrap().0, tree);
        store.open_up();
    }

    #[test]
    fn a_folder_that_doesnt_match_its_tree_is_refused() {
        let dir = TempDir::new();
        let store = SkillStore::under(dir.path());
        let from = skill(&dir.path().join("in"), "---\nname: other\n---\n");
        let wrong = TreeHash("a".repeat(64));
        assert!(store
            .land(&wrong, &from)
            .unwrap_err()
            .contains("don't match"));
        assert!(!store.has(&wrong) && from.exists());
    }

    #[test]
    fn the_sweep_keeps_named_trees_and_drops_old_ones() {
        let dir = TempDir::new();
        let store = SkillStore::under(dir.path());
        let kept = store
            .copy_in(&skill(&dir.path().join("a"), "---\nname: a\n---\n"))
            .unwrap()
            .0;
        let dropped = store
            .copy_in(&skill(&dir.path().join("b"), "---\nname: b\n---\n"))
            .unwrap()
            .0;
        let keep = HashSet::from([kept.clone()]);
        let now = SystemTime::now();

        assert!(store.sweep(&keep, now).is_empty());
        let soon = now + KEEP_FOR - Duration::from_secs(60);
        assert!(store.sweep(&keep, soon).is_empty());
        assert!(store.has(&dropped));

        let later = now + KEEP_FOR + Duration::from_secs(60);
        assert_eq!(store.sweep(&keep, later), vec![dropped.clone()]);
        assert!(store.has(&kept) && !store.has(&dropped));

        // The kept tree's time moved, so it outlives a later unnaming.
        let after = later + KEEP_FOR - Duration::from_secs(60);
        assert!(store.sweep(&HashSet::new(), after).is_empty());
        assert!(store.has(&kept));
        store.open_up();
    }
}
