// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Removes a folder however deep it goes.
//!
//! `std::fs::remove_dir_all` goes down one call and one descriptor per
//! level, so a tree a few thousand folders deep can run a thread out of
//! stack and take the process with it. This stays one level below the top:
//! it empties each folder there, moves that folder's subfolders up to the
//! top and goes round again. Everything goes by descriptor and name without
//! following a link, as the standard library does.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use crate::beneath::{self, Dir};

/// Removes a folder and what's in it. A link at `path` is removed itself and
/// what it points to stays.
pub fn remove_tree(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if beneath::remove_if_link(path)? {
        return Ok(());
    }
    {
        // Windows can't remove a folder this process still holds open.
        let top = Dir::open(path)?;
        empty(&top)?;
    }
    std::fs::remove_dir(path)
}

/// Empties the folder. A round that removes nothing ends with its last
/// error, so what can't be removed can't keep this going.
fn empty(top: &Dir) -> io::Result<()> {
    let mut moved = 0u64;
    loop {
        let items = top.items()?;
        if items.is_empty() {
            return Ok(());
        }
        let mut last = None;
        let mut progress = false;
        for (name, is_dir) in items {
            match clear(top, &name, is_dir, &mut moved) {
                Ok(()) => progress = true,
                Err(err) => last = Some(err),
            }
        }
        if let (false, Some(err)) = (progress, last) {
            return Err(err);
        }
    }
}

/// Removes one thing at the top, moving any folder holding folders up first.
fn clear(top: &Dir, name: &OsStr, is_dir: bool, moved: &mut u64) -> io::Result<()> {
    if !is_dir {
        return unlinked(top.unlink(name, false));
    }
    if remove_empty(top, name)? {
        return Ok(());
    }
    {
        let dir = top.subdir(name)?;
        for (child, child_is_dir) in dir.items()? {
            if !child_is_dir {
                unlinked(dir.unlink(&child, false))?;
            } else if !remove_empty(&dir, &child)? {
                lift(top, &dir, &child, moved)?;
            }
        }
    }
    unlinked(top.unlink(name, true))
}

/// Removes a folder if it's empty, whatever its mode, and says if it's gone.
fn remove_empty(parent: &Dir, name: &OsStr) -> io::Result<bool> {
    match parent.unlink(name, true) {
        Ok(()) => Ok(true),
        Err(err) if beneath::missing(&err) => Ok(true),
        Err(err) if beneath::not_empty(&err) => Ok(false),
        Err(err) => Err(err),
    }
}

/// Moves a folder from `dir` to the top under a name not in use there.
fn lift(top: &Dir, dir: &Dir, name: &OsStr, moved: &mut u64) -> io::Result<()> {
    loop {
        let to = format!(".removing-{moved}");
        *moved += 1;
        match dir.move_to(name, top, OsStr::new(&to)) {
            Ok(()) => return Ok(()),
            Err(err) if beneath::missing(&err) => return Ok(()),
            // Something at the top already has that name.
            Err(err) if beneath::name_taken(&err) => {}
            Err(err) => return Err(err),
        }
    }
}

/// Something already gone counts as removed.
fn unlinked(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if beneath::missing(&err) => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beneath::Dir as Beneath;
    use crate::testing::{self, TempDir};
    use std::fs;

    fn nest(root: &Path, depth: usize) {
        let mut at = Beneath::open(root).unwrap();
        for _ in 0..depth {
            at.make_dir("d", 0o755).unwrap();
            at = at.dir(&["d".to_owned()]).unwrap();
        }
    }

    /// Past what `std::fs::remove_dir_all` can do on a small stack.
    #[test]
    fn a_deep_tree_goes_on_a_small_stack() {
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        fs::create_dir(&tree).unwrap();
        nest(&tree, 1200);
        let removing = tree.clone();
        let removed = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(move || remove_tree(&removing))
            .unwrap()
            .join()
            .unwrap();
        removed.unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn a_wide_tree_goes_with_its_files_and_links() {
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        for top in ["a", "b", ".removing-0"] {
            for inner in ["x", "y/z"] {
                let folder = tree.join(top).join(inner);
                fs::create_dir_all(&folder).unwrap();
                fs::write(folder.join("file"), "text").unwrap();
                testing::symlink("/nowhere", &folder.join("broken"));
            }
        }
        fs::write(tree.join(".removing-1"), "taken by a file").unwrap();
        remove_tree(&tree).unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn a_link_goes_and_what_it_points_to_stays() {
        let dir = TempDir::new();
        let kept = dir.path().join("kept");
        fs::create_dir(&kept).unwrap();
        fs::write(kept.join("file"), "text").unwrap();

        let top_link = dir.path().join("top-link");
        testing::dir_link(&kept, &top_link);
        remove_tree(&top_link).unwrap();
        assert!(fs::symlink_metadata(&top_link).is_err());
        assert!(kept.join("file").is_file());

        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("a/b")).unwrap();
        testing::dir_link(&kept, &tree.join("inner-link"));
        testing::dir_link(&kept, &tree.join("a").join("b").join("inner-link"));
        remove_tree(&tree).unwrap();
        assert!(!tree.exists());
        assert!(kept.join("file").is_file());
    }

    #[test]
    fn nothing_there_is_not_found_and_a_file_is_refused() {
        let dir = TempDir::new();
        let missing = remove_tree(dir.path().join("missing")).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        let file = dir.path().join("file");
        fs::write(&file, "text").unwrap();
        assert!(remove_tree(&file).is_err());
        assert!(file.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_folder_goes_whatever_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        let shut = tree.join("shut");
        fs::create_dir_all(&shut).unwrap();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
        remove_tree(&tree).unwrap();
        assert!(!tree.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_it_cant_open_is_an_error_and_it_doesnt_go_round_for_ever() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        let shut = tree.join("shut");
        fs::create_dir_all(&shut).unwrap();
        fs::write(shut.join("file"), "text").unwrap();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
        let tried = remove_tree(&tree);
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(tried.is_err());
        assert!(shut.join("file").is_file());
    }
}
