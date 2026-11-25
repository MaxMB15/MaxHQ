// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The skills an Incus workspace sees. Its folder under `maxd`'s data
//! holds `sets/<hash>/<name>/`, hard links into the store, and `current`,
//! a relative link to the set in use that a rename swaps. The folder is
//! the workspace's read-only `maxhq-skills` disk at `/opt/maxhq/skills`,
//! so a new set appears beside the old one and nothing in the mount is
//! replaced in place.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use maxhq_core::fs_ext;
use maxhq_core::skills::lock;
use maxhq_core::skills::pack::PACK_DIR;
use maxhq_core::skills::store::TreeStore;
use maxhq_protocol::skills::SkillLock;

/// The workspace's skills disk.
pub const DEVICE: &str = "maxhq-skills";

/// Where the disk is in the container. [`maxhq_core::skills::MOUNT`] is
/// its `current`.
pub const GUEST_PATH: &str = "/opt/maxhq/skills";

/// The link to the set in use.
const CURRENT: &str = "current";

/// The disk's keys, for a workspace whose folder is `root`.
pub fn device(root: &Path) -> BTreeMap<String, String> {
    [
        ("type", "disk"),
        ("source", &root.display().to_string()),
        ("path", GUEST_PATH),
        ("readonly", "true"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect()
}

/// Makes `current` in `root` the set of `skills`, writing the set first
/// when it's new, and drops the other sets. Says whether `current` moved.
pub fn publish(store: &dyn TreeStore, root: &Path, skills: &SkillLock) -> Result<bool, String> {
    let failed = |what: &str, err: io::Error| format!("couldn't {what}: {err}");
    let hash = lock::set_hash(skills);
    let sets = root.join("sets");
    let set = sets.join(&hash);
    if !set.is_dir() {
        let temp = sets.join(format!(".{hash}.{}.tmp", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let made = write_set(store, skills, &temp).and_then(|()| {
            fs::rename(&temp, &set).map_err(|err| failed("keep the skill set", err))
        });
        if let Err(err) = made {
            let _ = fs::remove_dir_all(&temp);
            return Err(err);
        }
    }
    let target = format!("sets/{hash}");
    let current = root.join(CURRENT);
    let moved = fs::read_link(&current).ok().as_deref() != Some(Path::new(&target));
    if moved {
        let temp = root.join(format!(".{CURRENT}.{}.tmp", std::process::id()));
        let _ = fs_ext::remove_link(&temp);
        fs_ext::symlink(&target, &temp).map_err(|err| failed("link the new skill set", err))?;
        fs_ext::rename_link(&temp, &current)
            .map_err(|err| failed("swap in the new skill set", err))?;
    }
    for entry in fs::read_dir(&sets).map_err(|err| failed("list the skill sets", err))? {
        let Ok(entry) = entry else { continue };
        if entry.file_name() != hash.as_str() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(moved)
}

/// Writes the set of `skills` to the folder `to`, with the devflow pack
/// at `.devflow-vendor` for the guest to copy.
fn write_set(store: &dyn TreeStore, skills: &SkillLock, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|err| format!("couldn't make the skill set: {err}"))?;
    let pack = skills.pack.iter().map(|pack| (PACK_DIR, &pack.tree));
    let trees = skills
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), &skill.tree));
    for (name, tree) in trees.chain(pack) {
        let from = store
            .path(tree)
            .filter(|from| from.is_dir())
            .ok_or_else(|| format!("This device doesn't have the skill tree {} yet.", tree.0))?;
        link_tree(&from, &to.join(name))
            .map_err(|err| format!("couldn't add {name} to the skill set: {err}"))?;
    }
    Ok(())
}

/// Puts the tree at `from` at `to` as hard links, or copies where a link
/// can't be made, such as across disks.
fn link_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            link_tree(&entry.path(), &target)?;
        } else if kind.is_file() && fs::hard_link(entry.path(), &target).is_err() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use maxhq_core::skills::disk::DiskFs;
    use maxhq_core::skills::tree;

    use super::*;
    use crate::skills::apply::tests::{land, stocked};
    use crate::testing::TempDir;

    #[test]
    fn a_new_set_swaps_current_and_the_old_one_goes() {
        let dir = TempDir::new();
        let (store, mut skills) = stocked(dir.path(), &[("git-flow", "one"), ("unslop", "two")]);
        let root = dir.path().join("skills/workspace");
        assert!(publish(&store, &root, &skills).unwrap());
        let first = fs::read_link(root.join(CURRENT)).unwrap();
        assert_eq!(
            first,
            Path::new(&format!("sets/{}", lock::set_hash(&skills)))
        );
        let copy = root.join(CURRENT).join("git-flow");
        assert_eq!(tree::of(&DiskFs, &copy).unwrap(), skills.skills[0].tree);
        // A hard link into the store, not a copy. Windows' std can't tell.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let stored = store.path(&skills.skills[0].tree).unwrap().join("SKILL.md");
            assert_eq!(
                fs::metadata(copy.join("SKILL.md")).unwrap().ino(),
                fs::metadata(stored).unwrap().ino()
            );
        }

        assert!(!publish(&store, &root, &skills).unwrap());

        skills.skills[0].tree = land(&store, dir.path(), "git-flow", "three");
        assert!(publish(&store, &root, &skills).unwrap());
        let second = fs::read_link(root.join(CURRENT)).unwrap();
        assert_ne!(first, second);
        assert!(!root.join(&first).exists() && root.join(&second).is_dir());
        assert_eq!(fs::read_dir(root.join("sets")).unwrap().count(), 1);
    }

    #[test]
    fn a_tree_the_store_lacks_leaves_current_alone() {
        let dir = TempDir::new();
        let (store, mut skills) = stocked(dir.path(), &[("git-flow", "one")]);
        let root = dir.path().join("skills/workspace");
        publish(&store, &root, &skills).unwrap();
        let before = fs::read_link(root.join(CURRENT)).unwrap();
        skills.skills[0].tree = maxhq_protocol::skills::TreeHash("b".repeat(64));
        assert!(publish(&store, &root, &skills)
            .unwrap_err()
            .contains("doesn't have"));
        assert_eq!(fs::read_link(root.join(CURRENT)).unwrap(), before);
    }

    #[test]
    fn the_pack_goes_in_the_set_beside_the_skills() {
        let dir = TempDir::new();
        let (store, mut skills) = stocked(dir.path(), &[("git-flow", "one")]);
        let tree = land(&store, dir.path(), "pack", "whole");
        skills.pack = Some(maxhq_protocol::skills::SkillPack {
            source: skills.skills[0].source,
            commit: "abc".into(),
            tree: tree.clone(),
        });
        let root = dir.path().join("skills/workspace");
        publish(&store, &root, &skills).unwrap();
        let copy = root.join(CURRENT).join(PACK_DIR);
        assert_eq!(tree::of(&DiskFs, &copy).unwrap(), tree);
        assert!(root.join(CURRENT).join("git-flow").is_dir());
    }

    #[test]
    fn the_disk_is_read_only_at_the_fixed_path() {
        let keys = device(Path::new("/var/lib/maxhq/skills/ws"));
        assert_eq!(keys["type"], "disk");
        assert_eq!(keys["readonly"], "true");
        assert_eq!(keys["path"], "/opt/maxhq/skills");
        assert_eq!(keys["source"], "/var/lib/maxhq/skills/ws");
    }
}
