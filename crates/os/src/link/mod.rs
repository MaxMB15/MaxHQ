// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A name that points at a version folder or program and moves to another
//! in one step, as updates need: a symlink renamed into place on Unix, and
//! a directory junction on Windows.

use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

/// The temporary name [`point`] makes beside `link` before it moves into
/// place: the link's name with `.maxhq-new` on the end. A crash can leave
/// it behind, and the next [`point`] removes it.
pub fn temp_for(link: &Path) -> PathBuf {
    let mut name = link.file_name().unwrap_or_default().to_owned();
    name.push(".maxhq-new");
    link.with_file_name(name)
}

/// Points `link` at `target`, replacing what it pointed at. A reader sees
/// the old target or the new one, never neither.
pub fn point(link: &Path, target: &Path) -> io::Result<()> {
    imp::point(link, target)
}

/// What `link` points at.
pub fn resolve(link: &Path) -> io::Result<PathBuf> {
    imp::resolve(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_temp_name_sits_beside_the_link() {
        assert_eq!(
            temp_for(Path::new("/home/me/.local/bin/maxd")),
            Path::new("/home/me/.local/bin/maxd.maxhq-new")
        );
    }

    #[cfg(unix)]
    #[test]
    fn point_moves_the_link_to_a_new_target() {
        let dir = std::env::temp_dir().join(format!("maxhq-os-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("one")).unwrap();
        std::fs::create_dir_all(dir.join("two")).unwrap();
        let link = dir.join("current");
        point(&link, &dir.join("one")).unwrap();
        assert_eq!(resolve(&link).unwrap(), dir.join("one"));
        // A temp file a crash left is cleared away.
        std::fs::write(temp_for(&link), b"").unwrap();
        point(&link, &dir.join("two")).unwrap();
        assert_eq!(resolve(&link).unwrap(), dir.join("two"));
        assert!(!temp_for(&link).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn point_moves_a_junction_to_a_new_folder() {
        let dir = std::env::temp_dir().join(format!("maxhq-os-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["one", "two"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
            std::fs::write(dir.join(name).join("maxd.exe"), name).unwrap();
        }
        let link = dir.join("current");
        point(&link, &dir.join("one")).unwrap();
        assert_eq!(resolve(&link).unwrap(), dir.join("one"));
        // A program open through the link stays the one it opened.
        let mut open = std::fs::File::open(link.join("maxd.exe")).unwrap();

        // A temp folder a crash left is cleared away.
        std::fs::create_dir(temp_for(&link)).unwrap();
        point(&link, &dir.join("two")).unwrap();
        assert_eq!(resolve(&link).unwrap(), dir.join("two"));
        assert_eq!(
            std::fs::read_to_string(link.join("maxd.exe")).unwrap(),
            "two"
        );
        assert!(!temp_for(&link).exists());
        let mut was = String::new();
        std::io::Read::read_to_string(&mut open, &mut was).unwrap();
        assert_eq!(was, "one");
        drop(open);

        // A file can't be linked, and a folder that isn't a junction
        // isn't replaced.
        let err = point(&link, &dir.join("one").join("maxd.exe")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
        let err = point(&dir.join("one"), &dir.join("two")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "{err}");

        // Removing the junction leaves its folder.
        std::fs::remove_dir(&link).unwrap();
        assert!(dir.join("two").join("maxd.exe").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
