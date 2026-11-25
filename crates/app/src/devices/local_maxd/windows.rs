// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! On Windows the service runs a copy of the app's `maxd` too. An
//! installer can't replace a program that runs, so an update would fail
//! while the service ran the one beside the app. The copy goes where
//! `maxd`'s own updates put builds, [`MaxdLayout::windows`] in MaxHQ's
//! folder: `%LOCALAPPDATA%\MaxHQ\versions\<build>`, and the logon task
//! runs `%LOCALAPPDATA%\MaxHQ\bin\current\maxd.exe`, where `current` is
//! a junction to the build's folder. Moving to a new build copies it out
//! and points the junction at it, and the old program keeps running from
//! its own folder until the service restarts.

use std::io;
use std::path::{Path, PathBuf};

use maxhq_core::paths::MaxdLayout;

use super::other::copy_out;

/// Why the app won't install `maxd` from where it runs now.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) const MISPLACED: &str =
    "Install MaxHQ with its setup and open it from the Start menu, then start maxd again.";

/// Why `bundled` runs from somewhere it can't be installed from: the temp
/// folder, where Windows puts a program you open from inside a zip, often
/// without the files beside it, and which it clears.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn misplaced(bundled: &Path) -> Option<&'static str> {
    misplaced_in(bundled, &std::env::temp_dir())
}

/// [`misplaced`], with the temp folder given.
fn misplaced_in(bundled: &Path, temp: &Path) -> Option<&'static str> {
    crate::app_update::windows_inside(bundled, temp).then_some(MISPLACED)
}

/// Copies `bundled` out to the versions in MaxHQ's folder and points the
/// junction at it. Returns the program the service runs. The home folder
/// plays no part: the folders are in MaxHQ's local folder.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn stage(bundled: &Path, _home: Option<&Path>) -> io::Result<PathBuf> {
    let layout = layout().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Windows gave no local app data folder",
        )
    })?;
    copy_out(bundled, &layout)
}

/// The program the service runs once the app installed `maxd`:
/// `bin\current\maxd.exe` in MaxHQ's folder.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn installed(_bundled: &Path, _home: Option<&Path>) -> Option<PathBuf> {
    layout().map(|layout| layout.program())
}

/// The layout in MaxHQ's folder, `%LOCALAPPDATA%\MaxHQ`.
#[cfg_attr(not(windows), allow(dead_code))]
fn layout() -> Option<MaxdLayout> {
    maxhq_core::paths::windows_app_dir().map(MaxdLayout::windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_maxd_opened_from_the_temp_folder_is_misplaced() {
        let temp = Path::new(r"C:\Users\<user>\AppData\Local\Temp\");
        let zipped = Path::new(r"C:\Users\<user>\AppData\Local\Temp\Temp1_MaxHQ.zip\maxd.exe");
        assert_eq!(misplaced_in(zipped, temp), Some(MISPLACED));
        let shouting = Path::new(r"c:/users/<USER>/appdata/local/temp/MaxHQ/maxd.exe");
        assert_eq!(misplaced_in(shouting, temp), Some(MISPLACED));
        let installed = Path::new(r"C:\Users\<user>\AppData\Local\MaxHQ\maxd.exe");
        assert_eq!(misplaced_in(installed, temp), None);
        let alike = Path::new(r"C:\Users\<user>\AppData\Local\Temporary\maxd.exe");
        assert_eq!(misplaced_in(alike, temp), None);
        assert_eq!(misplaced_in(installed, Path::new("")), None);
    }

    /// A folder with a fake bundled `maxd.exe` and MaxHQ's folder.
    /// Removed when dropped.
    #[cfg(windows)]
    struct Scratch(PathBuf);

    #[cfg(windows)]
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "maxhq-copy-{}",
                maxhq_protocol::ids::LinkId::generate()
            ));
            std::fs::create_dir_all(dir.join("app")).unwrap();
            std::fs::create_dir_all(dir.join("MaxHQ")).unwrap();
            let scratch = Self(dir);
            scratch.bundle("one");
            scratch
        }

        /// Makes the bundled `maxd.exe` hold `word`, which makes it a new
        /// build.
        fn bundle(&self, word: &str) {
            std::fs::write(self.bundled(), word).unwrap();
        }

        fn bundled(&self) -> PathBuf {
            self.0.join("app").join("maxd.exe")
        }

        fn layout(&self) -> MaxdLayout {
            MaxdLayout::windows(self.0.join("MaxHQ"))
        }

        fn stage(&self) -> PathBuf {
            copy_out(&self.bundled(), &self.layout()).unwrap()
        }

        /// The builds under the versions folder, sorted.
        fn builds(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(self.layout().versions())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
    }

    #[cfg(windows)]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(self.layout().link());
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_bundled_maxd_is_copied_out_and_the_junction_follows_each_build() {
        let scratch = Scratch::new();
        let program = scratch.stage();
        assert_eq!(program, scratch.layout().program());
        assert_eq!(std::fs::read_to_string(&program).unwrap(), "one");
        let [first] = scratch.builds().try_into().unwrap();

        // The program the service runs stays open across a new build, and
        // the junction leads to the new one.
        let mut running = std::fs::File::open(&program).unwrap();
        scratch.bundle("two");
        assert_eq!(scratch.stage(), program);
        assert_eq!(std::fs::read_to_string(&program).unwrap(), "two");
        let mut was = String::new();
        io::Read::read_to_string(&mut running, &mut was).unwrap();
        assert_eq!(was, "one");
        assert_eq!(scratch.builds().len(), 2, "the build before stays");
        drop(running);

        // A third build lets the first go.
        scratch.bundle("three");
        scratch.stage();
        assert_eq!(std::fs::read_to_string(&program).unwrap(), "three");
        let builds = scratch.builds();
        assert_eq!(builds.len(), 2);
        assert!(!builds.contains(&first));
    }
}
