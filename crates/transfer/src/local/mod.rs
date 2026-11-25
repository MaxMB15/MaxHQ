// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a client does with transfers on its own disk, where it acts on
//! paths in your folders rather than inside a landing folder: moving a
//! download into place without replacing anything, marking it as from
//! elsewhere, and copying a link as a link for an upload's staged copy.

use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// Renames `from` to `to`, failing with [`io::ErrorKind::AlreadyExists`]
/// if `to` is there. Windows refuses to move a folder while another
/// program, such as a virus scanner looking at what just landed, has a
/// file in it open, so there it tries again for a moment.
pub fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    imp::rename_new(from, to)
}

/// Marks a download and every file in it as from elsewhere, the way
/// browsers do: the quarantine mark on macOS, so Gatekeeper checks it
/// before it opens, and the Mark of the Web on Windows, so SmartScreen and
/// Office treat it as downloaded. Linux has no such mark. Links are left
/// be. It goes on past a file it can't mark and gives the first error.
pub fn mark_from_elsewhere(path: &Path) -> io::Result<()> {
    let Some(mark) = imp::Mark::new() else {
        return Ok(());
    };
    let mut first = None;
    let mut todo = vec![path.to_path_buf()];
    while let Some(at) = todo.pop() {
        let meta = match std::fs::symlink_metadata(&at) {
            Ok(meta) => meta,
            Err(err) => {
                first.get_or_insert(err);
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if let Err(err) = mark.put(&at, meta.is_dir()) {
            first.get_or_insert(err);
        }
        if meta.is_dir() {
            match std::fs::read_dir(&at) {
                Ok(items) => todo.extend(items.flatten().map(|item| item.path())),
                Err(err) => {
                    first.get_or_insert(err);
                }
            }
        }
    }
    first.map_or(Ok(()), Err)
}

/// Copies the link at `from` to a new link at `to`, with the same target,
/// and says whether it made one. Windows without Developer Mode or an
/// administrator can't make links, so there it makes none, and the caller
/// leaves the link out.
pub fn copy_link(from: &Path, to: &Path) -> io::Result<bool> {
    imp::copy_link(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, TempDir};

    #[test]
    fn a_rename_into_a_taken_name_fails_and_leaves_both() {
        let temp = TempDir::new();
        let at = |name: &str| temp.path().join(name);
        let (a, b, c) = (at("a"), at("b"), at("c"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let err = rename_new(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");

        rename_new(&a, &c).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "a");
    }

    #[test]
    fn a_folder_moves_to_a_free_name_and_not_onto_an_empty_one() {
        let temp = TempDir::new();
        let at = |name: &str| temp.path().join(name);
        let (folder, empty, free) = (at("site"), at("empty"), at("free"));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("index.html"), "hi").unwrap();
        std::fs::create_dir(&empty).unwrap();
        let err = rename_new(&folder, &empty).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(folder.join("index.html").exists());

        rename_new(&folder, &free).unwrap();
        assert_eq!(
            std::fs::read_to_string(free.join("index.html")).unwrap(),
            "hi"
        );
    }

    /// A file held open the way a reader that lets others delete it holds
    /// one doesn't stop its folder from moving in.
    #[test]
    fn a_folder_with_a_file_open_for_reading_still_moves() {
        let temp = TempDir::new();
        let (folder, to) = (temp.path().join("site"), temp.path().join("moved"));
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), "a").unwrap();
        // std opens a file letting others read, write and delete it.
        let held = std::fs::File::open(folder.join("a.txt")).unwrap();
        let moved = rename_new(&folder, &to);
        drop(held);
        // Windows refuses to move a folder that holds an open file, which is
        // why the rename tries again; once it's closed the move goes.
        match moved {
            Ok(()) => {}
            Err(_) if cfg!(windows) => rename_new(&folder, &to).unwrap(),
            Err(err) => panic!("{err}"),
        }
        assert_eq!(std::fs::read_to_string(to.join("a.txt")).unwrap(), "a");
    }

    #[test]
    fn a_download_and_the_files_in_it_are_marked_but_its_links_are_not() {
        let temp = TempDir::new();
        let top = temp.path().join("site");
        std::fs::create_dir_all(top.join("assets")).unwrap();
        std::fs::write(top.join("index.html"), "hi").unwrap();
        std::fs::write(top.join("assets/app.js"), "js").unwrap();
        let linked = testing::symlink("index.html", &top.join("home.html"));
        mark_from_elsewhere(&top).unwrap();
        for file in ["index.html", "assets/app.js"] {
            if let Some(marked) = testing::marked(&top.join(file)) {
                assert!(marked, "{file} has no mark");
            }
        }
        if linked && let Some(marked) = testing::marked(&top.join("home.html")) {
            assert!(!marked, "the link was marked");
        }
        // Marking again is fine, as a second download of the same file is.
        mark_from_elsewhere(&top.join("index.html")).unwrap();
    }

    #[test]
    fn a_link_copies_with_its_target_where_this_machine_makes_links() {
        let temp = TempDir::new();
        let (from_dir, to_dir) = (temp.path().join("from"), temp.path().join("to"));
        std::fs::create_dir(&from_dir).unwrap();
        std::fs::create_dir(&to_dir).unwrap();
        std::fs::write(from_dir.join("run.sh"), "#!/bin/sh").unwrap();
        std::fs::create_dir(from_dir.join("lib")).unwrap();
        for (name, target) in [("latest", "run.sh"), ("libs", "lib")] {
            let from = from_dir.join(name);
            if !testing::symlink(target, &from) {
                continue;
            }
            let to = to_dir.join(name);
            let made = copy_link(&from, &to).unwrap();
            // Only Windows leaves links out.
            assert!(made || cfg!(windows), "{name} wasn't made");
            if made {
                assert_eq!(
                    std::fs::read_link(&to).unwrap(),
                    std::fs::read_link(&from).unwrap()
                );
                assert!(std::fs::symlink_metadata(&to).unwrap().is_symlink());
            } else {
                assert!(std::fs::symlink_metadata(&to).is_err());
            }
        }
    }
}
