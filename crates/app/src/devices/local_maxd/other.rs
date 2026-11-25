// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Every system but macOS runs a copy of the app's `maxd`, not the one
//! that ships with the app. An AppImage mounts at a new path each time it
//! runs, and a package manager replaces `/usr/bin/maxd` whenever it likes,
//! so a service pointed there would lose its program. The copy goes where
//! updates put `maxd` on Linux servers, `~/.local/lib/maxhq/versions/<build>`,
//! and the service runs `~/.local/bin/maxd`, a link to it. This machine's
//! `maxd` and a server's then look the same, and moving to a new build is
//! one more copy and a flip of the link.
//!
//! Windows lays the copies out under its own folders, in `windows.rs`,
//! and links the build's folder with a junction rather than the program
//! with a symbolic link.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use maxhq_core::paths::{MaxdLayout, BUILD_LEN};

/// Nothing here depends on where the app runs, since the service runs the
/// copy, so no place is the wrong one.
#[cfg_attr(windows, allow(dead_code))]
pub(super) fn misplaced(_bundled: &Path) -> Option<&'static str> {
    None
}

/// Copies `bundled` out to the versions under `home` and points
/// `~/.local/bin/maxd` at the copy. Returns the program the service runs.
#[cfg_attr(windows, allow(dead_code))]
pub(super) fn stage(bundled: &Path, home: Option<&Path>) -> io::Result<PathBuf> {
    let home = home.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME isn't set"))?;
    copy_out(bundled, &MaxdLayout::new(home))
}

/// The program the service runs once the app installed `maxd`:
/// `~/.local/bin/maxd`. `None` without a home folder.
#[cfg_attr(windows, allow(dead_code))]
pub(super) fn installed(_bundled: &Path, home: Option<&Path>) -> Option<PathBuf> {
    home.map(|home| MaxdLayout::new(home).program())
}

/// Copies `bundled` into `layout`'s versions, as `versions/<build>/maxd`
/// where the build is the first 16 hex characters of its sha256. The copy
/// is made under a temporary name and renamed into place, so nothing ever
/// runs half of one. It then points the layout's link at the copy, on
/// Windows a junction at the copy's folder, and removes every build but
/// this one and the one the link named before, which may still run until
/// the service restarts. Windows won't delete a program that runs, nor
/// its folder. Returns what the service should run: the link on Unix, and
/// `maxd.exe` through it on Windows.
pub(super) fn copy_out(bundled: &Path, layout: &MaxdLayout) -> io::Result<PathBuf> {
    let versions = layout.versions();
    fs::create_dir_all(&versions)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = versions.join(format!(".incoming-{}-{nonce}", std::process::id()));
    let placed = place(bundled, &temp, layout);
    if placed.is_err() {
        let _ = fs::remove_file(&temp);
    }
    let build = placed?;

    let previous = layout.linked_build();
    let link = layout.link();
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir)?;
    }
    maxhq_os::link::point(&link, &layout.link_target(&build))?;
    let keep: Vec<&str> = [Some(build.as_str()), previous.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    for (build, error) in layout.prune(&keep) {
        tracing::warn!(%error, build, "couldn't remove an old copy of maxd");
    }
    Ok(layout.program())
}

/// Copies `bundled` to `temp`, makes it executable, and renames it into
/// the folder of its build in `layout`. Returns the build.
fn place(bundled: &Path, temp: &Path, layout: &MaxdLayout) -> io::Result<String> {
    fs::copy(bundled, temp)?;
    make_executable(temp)?;
    // The service starts this file at the next login, so it has to be on
    // the disk before its name is. Windows only flushes a file opened for
    // writing.
    fs::OpenOptions::new().write(true).open(temp)?.sync_all()?;
    let mut build = crate::fleet::sha256_file(temp)?;
    build.truncate(BUILD_LEN);
    let copy = layout.version(&build);
    // A build's folder is named after what's in it, so one that's there
    // already holds this very file, and it may be running.
    if copy.is_file() {
        fs::remove_file(temp)?;
        return Ok(build);
    }
    if let Some(dir) = copy.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::rename(temp, &copy)?;
    Ok(build)
}

/// Lets everyone run `path` and only its owner change it.
#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

/// Nothing to do where a file runs by its name.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use maxhq_protocol::ids::LinkId;

    use super::*;

    /// A folder with an AppImage's mount in it, holding a fake `maxd`,
    /// and a home folder. Removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("maxhq-copy-{}", LinkId::generate()));
            fs::create_dir_all(dir.join(".mount_MaxHQ1/usr/bin")).unwrap();
            fs::create_dir_all(dir.join("home")).unwrap();
            let scratch = Self(dir);
            scratch.bundle("one");
            scratch
        }

        /// Makes the bundled `maxd` print `word`, which makes it a new build.
        fn bundle(&self, word: &str) {
            let maxd = self.bundled();
            fs::write(&maxd, format!("#!/bin/sh\necho {word}\n")).unwrap();
            fs::set_permissions(&maxd, fs::Permissions::from_mode(0o700)).unwrap();
        }

        fn bundled(&self) -> PathBuf {
            self.0.join(".mount_MaxHQ1/usr/bin/maxd")
        }

        fn home(&self) -> PathBuf {
            self.0.join("home")
        }

        fn versions(&self) -> PathBuf {
            MaxdLayout::new(self.home()).versions()
        }

        /// The builds under the versions folder, sorted.
        fn builds(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.versions())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }

        /// The build of the bundled `maxd` now.
        fn build(&self) -> String {
            crate::fleet::sha256_file(&self.bundled()).unwrap()[..BUILD_LEN].to_owned()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// What running `program` prints.
    fn run(program: &Path) -> String {
        let output = Command::new(program).output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn an_appimages_maxd_is_copied_out_and_the_link_points_at_the_copy() {
        let scratch = Scratch::new();
        let home = scratch.home();
        let first = scratch.build();
        let program = stage(&scratch.bundled(), Some(&home)).unwrap();
        assert_eq!(program, home.join(".local/bin/maxd"));
        assert_eq!(
            installed(&scratch.bundled(), Some(&home)),
            Some(program.clone())
        );
        assert_eq!(misplaced(&scratch.bundled()), None);

        let copy = scratch.versions().join(&first).join("maxd");
        assert_eq!(fs::read_link(&program).unwrap(), copy);
        let mode = fs::metadata(&copy).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(
            scratch.builds(),
            std::slice::from_ref(&first),
            "no temporary file is left"
        );
        assert!(!home.join(".local/bin/maxd.maxhq-new").exists());

        // The service keeps its program once the AppImage unmounts.
        fs::remove_dir_all(scratch.0.join(".mount_MaxHQ1")).unwrap();
        assert_eq!(run(&program), "one");

        // Without a home folder nothing is copied.
        fs::create_dir_all(scratch.0.join(".mount_MaxHQ1/usr/bin")).unwrap();
        scratch.bundle("two");
        assert_eq!(installed(&scratch.bundled(), None), None);
        assert_eq!(
            stage(&scratch.bundled(), None).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(scratch.builds(), [first]);
    }

    #[test]
    fn a_new_build_flips_the_link_and_keeps_only_the_one_before() {
        let scratch = Scratch::new();
        let home = scratch.home();
        let link = MaxdLayout::new(&home).link();
        let one = scratch.build();
        stage(&scratch.bundled(), Some(&home)).unwrap();
        // Folders that aren't builds stay, whatever happens to the builds.
        fs::create_dir_all(scratch.versions().join("notes")).unwrap();
        fs::create_dir_all(scratch.versions().join("not-a-build-name")).unwrap();

        scratch.bundle("two");
        let two = scratch.build();
        assert_ne!(one, two);
        stage(&scratch.bundled(), Some(&home)).unwrap();
        assert_eq!(run(&link), "two");
        let mut both = vec![one, two.clone()];
        both.sort();
        let with_others = |builds: &[String]| {
            let mut all = builds.to_vec();
            all.extend(["not-a-build-name".to_owned(), "notes".to_owned()]);
            all.sort();
            all
        };
        assert_eq!(scratch.builds(), with_others(&both));

        // The same build again keeps its copy and lets the one before go,
        // since the link moved on from it before.
        let copy = scratch.versions().join(&two).join("maxd");
        let first_copy = fs::metadata(&copy).unwrap().modified().unwrap();
        stage(&scratch.bundled(), Some(&home)).unwrap();
        assert_eq!(run(&link), "two");
        assert_eq!(fs::metadata(&copy).unwrap().modified().unwrap(), first_copy);
        assert_eq!(scratch.builds(), with_others(std::slice::from_ref(&two)));

        scratch.bundle("three");
        let three = scratch.build();
        stage(&scratch.bundled(), Some(&home)).unwrap();
        assert_eq!(run(&link), "three");
        let mut last = vec![two, three];
        last.sort();
        assert_eq!(scratch.builds(), with_others(&last), "the first build goes");
    }

    #[test]
    fn a_plain_file_at_the_link_is_replaced_and_a_missing_maxd_copies_nothing() {
        let scratch = Scratch::new();
        let home = scratch.home();
        let link = MaxdLayout::new(&home).link();
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        fs::write(&link, "#!/bin/sh\necho old\n").unwrap();
        stage(&scratch.bundled(), Some(&home)).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(run(&link), "one");

        fs::remove_file(scratch.bundled()).unwrap();
        assert_eq!(
            stage(&scratch.bundled(), Some(&home)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(scratch.builds().len(), 1, "no temporary file is left");
        assert_eq!(run(&link), "one");
    }
}
