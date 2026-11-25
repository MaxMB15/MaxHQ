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
    // Reads a symlink on either system, and a junction on Windows.
    std::fs::read_link(link)
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
    fn point_says_it_isnt_built_yet() {
        let err = point(Path::new(r"C:\nowhere\maxd"), Path::new(r"C:\nowhere\v1")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
