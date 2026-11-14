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

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

const OPEN: OFlags = OFlags::DIRECTORY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::RDONLY);

/// Removes a folder and what's in it. A link at `path` is removed itself and
/// what it points to stays.
pub fn remove_tree(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if std::fs::symlink_metadata(path)?.is_symlink() {
        return std::fs::remove_file(path);
    }
    let top = rustix::fs::open(path, OPEN, Mode::empty())?;
    empty(&top)?;
    std::fs::remove_dir(path)
}

/// Empties the folder. A round that removes nothing ends with its last
/// error, so what can't be removed can't keep this going.
fn empty(top: &OwnedFd) -> Result<(), Errno> {
    let mut moved = 0u64;
    loop {
        let items = list(top)?;
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
fn clear(top: &OwnedFd, name: &CStr, is_dir: bool, moved: &mut u64) -> Result<(), Errno> {
    if !is_dir {
        return unlinked(rustix::fs::unlinkat(top, name, AtFlags::empty()));
    }
    if remove_empty(top, name)? {
        return Ok(());
    }
    let dir = rustix::fs::openat(top, name, OPEN, Mode::empty())?;
    for (child, child_is_dir) in list(&dir)? {
        if !child_is_dir {
            unlinked(rustix::fs::unlinkat(&dir, &child, AtFlags::empty()))?;
        } else if !remove_empty(&dir, &child)? {
            lift(top, &dir, &child, moved)?;
        }
    }
    unlinked(rustix::fs::unlinkat(top, name, AtFlags::REMOVEDIR))
}

/// Removes a folder if it's empty, whatever its mode, and says if it's gone.
fn remove_empty(parent: &OwnedFd, name: &CStr) -> Result<bool, Errno> {
    match rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR) {
        Ok(()) | Err(Errno::NOENT) => Ok(true),
        Err(Errno::NOTEMPTY | Errno::EXIST) => Ok(false),
        Err(err) => Err(err),
    }
}

/// Moves a folder from `dir` to the top under a name not in use there.
fn lift(top: &OwnedFd, dir: &OwnedFd, name: &CStr, moved: &mut u64) -> Result<(), Errno> {
    loop {
        let to = CString::new(format!(".removing-{moved}")).expect("it has no zero byte");
        *moved += 1;
        match rustix::fs::renameat(dir, name, top, &to) {
            Ok(()) | Err(Errno::NOENT) => return Ok(()),
            // Something at the top already has that name.
            Err(Errno::NOTEMPTY | Errno::EXIST | Errno::NOTDIR | Errno::ISDIR) => {}
            Err(err) => return Err(err),
        }
    }
}

/// What a folder holds, each with whether it's a folder. A link isn't one.
fn list(dir: &OwnedFd) -> Result<Vec<(CString, bool)>, Errno> {
    let mut items = Vec::new();
    for item in Dir::read_from(dir)? {
        let item = item?;
        let name = item.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let is_dir = match item.file_type() {
            FileType::Directory => true,
            FileType::Unknown => rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Directory),
            _ => false,
        };
        items.push((name.to_owned(), is_dir));
    }
    Ok(items)
}

/// Something already gone counts as removed.
fn unlinked(result: Result<(), Errno>) -> Result<(), Errno> {
    match result {
        Err(Errno::NOENT) => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beneath::Dir as Beneath;
    use crate::testing::TempDir;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

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
                symlink("/nowhere", folder.join("broken")).unwrap();
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
        symlink(&kept, &top_link).unwrap();
        remove_tree(&top_link).unwrap();
        assert!(fs::symlink_metadata(&top_link).is_err());
        assert!(kept.join("file").is_file());

        let tree = dir.path().join("tree");
        fs::create_dir_all(tree.join("a/b")).unwrap();
        symlink(&kept, tree.join("inner-link")).unwrap();
        symlink(&kept, tree.join("a/b/inner-link")).unwrap();
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

    #[test]
    fn an_empty_folder_goes_whatever_its_mode() {
        let dir = TempDir::new();
        let tree = dir.path().join("tree");
        let shut = tree.join("shut");
        fs::create_dir_all(&shut).unwrap();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
        remove_tree(&tree).unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn a_folder_it_cant_open_is_an_error_and_it_doesnt_go_round_for_ever() {
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
