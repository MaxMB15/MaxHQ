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

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many hex characters of a binary's sha256 name its build.
const BUILD_LEN: usize = 16;

/// Nothing here depends on where the app runs, since the service runs the
/// copy, so no place is the wrong one.
pub(super) fn misplaced(_bundled: &Path) -> Option<&'static str> {
    None
}

/// Copies `bundled` out to the versions under `home` and points
/// `~/.local/bin/maxd` at the copy. Returns the program the service runs.
pub(super) fn stage(bundled: &Path, home: Option<&Path>) -> io::Result<PathBuf> {
    let home = home.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME isn't set"))?;
    copy_out(bundled, &versions(home), &link(home))
}

/// The program the service runs once the app installed `maxd`:
/// `~/.local/bin/maxd`. `None` without a home folder.
pub(super) fn installed(_bundled: &Path, home: Option<&Path>) -> Option<PathBuf> {
    home.map(link)
}

/// `maxd`'s file name here.
fn program() -> String {
    format!("maxd{}", std::env::consts::EXE_SUFFIX)
}

/// `~/.local/bin/maxd`, the link the service runs.
fn link(home: &Path) -> PathBuf {
    home.join(".local/bin").join(program())
}

/// `~/.local/lib/maxhq/versions`, with one folder per build.
fn versions(home: &Path) -> PathBuf {
    home.join(".local/lib/maxhq/versions")
}

/// Copies `bundled` to `versions/<build>/maxd`, where the build is the
/// first 16 hex characters of its sha256. The copy is made under a
/// temporary name and renamed into place, so nothing ever runs half of
/// one. On Unix it then points `link` at the copy and removes every build
/// but this one and the one the link named before, which may still run
/// until the service restarts. Returns what the service should run: the
/// link on Unix, and the copy itself elsewhere, where the link step is
/// left to that system.
pub(crate) fn copy_out(bundled: &Path, versions: &Path, link: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(versions)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = versions.join(format!(".incoming-{}-{nonce}", std::process::id()));
    let placed = place(bundled, &temp, versions);
    if placed.is_err() {
        let _ = fs::remove_file(&temp);
    }
    let (build, copy) = placed?;
    link_to(link, &copy, versions, &build)
}

/// Copies `bundled` to `temp`, makes it executable, and renames it into
/// the folder of its build under `versions`. Returns the build and the
/// copy.
fn place(bundled: &Path, temp: &Path, versions: &Path) -> io::Result<(String, PathBuf)> {
    fs::copy(bundled, temp)?;
    make_executable(temp)?;
    // The service starts this file at the next login, so it has to be on
    // the disk before its name is.
    fs::File::open(temp)?.sync_all()?;
    let mut build = crate::fleet::sha256_file(temp)?;
    build.truncate(BUILD_LEN);
    let dir = versions.join(&build);
    let copy = dir.join(program());
    // A build's folder is named after what's in it, so one that's there
    // already holds this very file, and it may be running.
    if copy.is_file() {
        fs::remove_file(temp)?;
        return Ok((build, copy));
    }
    fs::create_dir_all(&dir)?;
    fs::rename(temp, &copy)?;
    Ok((build, copy))
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

/// Points `link` at `copy` and removes the builds nothing runs any more.
#[cfg(unix)]
fn link_to(link: &Path, copy: &Path, versions: &Path, build: &str) -> io::Result<PathBuf> {
    let previous = linked_build(link, versions);
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir)?;
    }
    point(link, copy)?;
    let keep: Vec<&str> = [Some(build), previous.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    prune(versions, &keep);
    Ok(link.to_owned())
}

/// Symbolic links need rights most accounts lack elsewhere, so the
/// service runs the copy itself.
#[cfg(not(unix))]
fn link_to(_link: &Path, copy: &Path, _versions: &Path, _build: &str) -> io::Result<PathBuf> {
    Ok(copy.to_owned())
}

/// Points `link` at `target` through a new link beside it and a rename,
/// so the service never finds it missing. Setup and maxd's own updates
/// flip it through the same temporary name.
#[cfg(unix)]
fn point(link: &Path, target: &Path) -> io::Result<()> {
    let temp = link.with_file_name(maxhq_core::paths::MAXD_LINK_TEMP);
    let _ = fs::remove_file(&temp);
    std::os::unix::fs::symlink(target, &temp)?;
    fs::rename(&temp, link)
}

/// The build `link` leads to, when it leads into `versions`.
#[cfg(unix)]
fn linked_build(link: &Path, versions: &Path) -> Option<String> {
    let target = fs::canonicalize(link).ok()?;
    let versions = fs::canonicalize(versions).ok()?;
    let dir = target.parent()?;
    if dir.parent()? != versions {
        return None;
    }
    dir.file_name()?.to_str().map(str::to_owned)
}

/// Removes every build in `versions` but `keep`. Only folders named like
/// a build go, so nothing else kept there is touched.
#[cfg(unix)]
fn prune(versions: &Path, keep: &[&str]) {
    let Ok(entries) = fs::read_dir(versions) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let is_build = name.len() == BUILD_LEN && name.bytes().all(|b| b.is_ascii_hexdigit());
        if !is_build || keep.contains(&name) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(entry.path()) {
            tracing::warn!(%error, build = name, "couldn't remove an old copy of maxd");
        }
    }
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
            versions(&self.home())
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
        let link = link(&home);
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
        let link = link(&home);
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
