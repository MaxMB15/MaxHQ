// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Helpers for this crate's tests.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A folder of its own under the system temp folder, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Makes a new, empty folder.
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        loop {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.subsec_nanos())
                .unwrap_or_default();
            let name = format!(
                "maxhq-transfer-{}-{}-{nanos}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("couldn't make {}: {err}", path.display()),
            }
        }
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Makes a link at `link` to `target` as a test would on Unix, and says
/// whether it could. Windows needs developer mode or the right to make
/// links, so a test that needs one goes on without it there. A target
/// Windows reads as a folder gets a folder link.
pub fn symlink(target: impl AsRef<Path>, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).unwrap();
        true
    }
    #[cfg(windows)]
    {
        let target = PathBuf::from(target.as_ref().to_string_lossy().replace('/', "\\"));
        let folder = link
            .parent()
            .map_or_else(|| target.clone(), |parent| parent.join(&target))
            .is_dir();
        let made = if folder {
            std::os::windows::fs::symlink_dir(&target, link)
        } else {
            std::os::windows::fs::symlink_file(&target, link)
        };
        made.is_ok()
    }
}

/// Makes `link` lead to the folder `target`: a symlink on Unix, and on
/// Windows a folder symlink, or a junction where links can't be made.
pub fn dir_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(target, link).is_err() {
        junction(target, link);
    }
}

/// Makes a junction at `link` to the folder `target`, which anyone can.
#[cfg(windows)]
pub fn junction(target: &Path, link: &Path) {
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "mklink /J: {}",
        String::from_utf8_lossy(&made.stderr)
    );
}

/// The permission bits of `path`, or `None` where the system keeps none.
pub fn mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(fs::metadata(path).unwrap().permissions().mode() & 0o7777)
    }
    #[cfg(windows)]
    {
        let _ = path;
        None
    }
}
