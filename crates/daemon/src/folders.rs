// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `ListFolder` and `MakeFolder`: what a folder on this device holds, for
//! the app's path fields and folder browser, and a new folder from the
//! browser. The server lets only the app and proven phones ask.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use maxhq_core::{git, names};
use maxhq_protocol::folders::{FolderCheckout, FolderEntry, FolderListing};
use maxhq_protocol::ids::DeviceId;
use maxhq_protocol::message::ActionOutcome;

/// The most entries a listing holds. A field filters by what you type, so
/// the rest of a bigger folder shows once you type more of a name.
const MOST: usize = 500;

/// The most entries read from one folder, kept or not.
const READ_MOST: usize = 10_000;

/// How long reading one folder may take before the listing ends with what
/// it has.
const READ_FOR: Duration = Duration::from_secs(2);

/// How long the answer waits for a folder whose opening alone hangs, as on
/// a network disk that went away.
const ANSWER_IN: Duration = Duration::from_secs(5);

const OTHER_DEVICE: &str = "That folder is for another device.";
const NOT_ABSOLUTE: &str = "Give the folder as a full path, starting with / or ~.";
const NO_HOME: &str = "maxd can't tell your home folder.";
const TOO_LONG: &str = "That folder took too long to read.";

/// Answers `ListFolder` for this device, `me`.
pub async fn list(
    me: DeviceId,
    device: DeviceId,
    path: String,
    files: bool,
    hidden: bool,
) -> ActionOutcome {
    if device != me {
        return refused(OTHER_DEVICE);
    }
    let home = std::env::home_dir();
    let read = tokio::task::spawn_blocking(move || read(me, home.as_deref(), &path, files, hidden));
    match tokio::time::timeout(ANSWER_IN, read).await {
        Ok(Ok(Ok(listing))) => ActionOutcome::FolderListed { listing },
        Ok(Ok(Err(reason))) => refused(reason),
        Ok(Err(_)) | Err(_) => refused(TOO_LONG),
    }
}

/// Answers `MakeFolder` for this device, `me`.
pub async fn make(me: DeviceId, device: DeviceId, path: String) -> ActionOutcome {
    if device != me {
        return refused(OTHER_DEVICE);
    }
    let home = std::env::home_dir();
    let made = tokio::task::spawn_blocking(move || make_at(home.as_deref(), &path));
    match tokio::time::timeout(ANSWER_IN, made).await {
        Ok(Ok(Ok(()))) => ActionOutcome::Done,
        Ok(Ok(Err(reason))) => refused(reason),
        Ok(Err(_)) | Err(_) => refused("Making that folder took too long."),
    }
}

/// Lists the folder at `path`. A folder that isn't there, or a file, lists
/// as not there. One that can't be read lists as empty.
fn read(
    me: DeviceId,
    home: Option<&Path>,
    path: &str,
    files: bool,
    hidden: bool,
) -> Result<FolderListing, String> {
    let path = expand(path, home)?;
    let (path, exists) = match fs::canonicalize(&path) {
        Ok(real) if real.is_dir() => (real, true),
        _ => (path, false),
    };
    let mut listing = FolderListing {
        path: path.to_string_lossy().into_owned(),
        home: home
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_default(),
        exists,
        entries: Vec::new(),
        truncated: false,
    };
    let Some(dir) = exists.then(|| fs::read_dir(&path).ok()).flatten() else {
        return Ok(listing);
    };
    let deadline = Instant::now() + READ_FOR;
    for (seen, entry) in dir.enumerate() {
        if seen == READ_MOST || Instant::now() > deadline {
            listing.truncated = true;
            break;
        }
        // Skip what can't be read, a name that isn't UTF-8 and a link to
        // nothing.
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !hidden && name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let folder = if kind.is_symlink() {
            match fs::metadata(entry.path()) {
                Ok(target) => target.is_dir(),
                Err(_) => continue,
            }
        } else {
            kind.is_dir()
        };
        if folder || files {
            listing.entries.push(FolderEntry {
                name,
                folder,
                checkout: None,
            });
        }
    }
    listing
        .entries
        .sort_by_cached_key(|entry| (!entry.folder, entry.name.to_lowercase()));
    if listing.entries.len() > MOST {
        listing.entries.truncate(MOST);
        listing.truncated = true;
    }
    for entry in listing.entries.iter_mut().filter(|entry| entry.folder) {
        entry.checkout = checkout(me, &path.join(&entry.name));
    }
    Ok(listing)
}

/// The checkout whose top folder is `root`, with its branch and project.
fn checkout(me: DeviceId, root: &Path) -> Option<FolderCheckout> {
    let checkout = git::checkout_at(root)?;
    let named = names::project(me, &root.to_string_lossy(), checkout.remote.as_deref());
    Some(FolderCheckout {
        branch: git::head_branch(root),
        project: named.id,
    })
}

/// Makes the one folder at `path`, in a folder that's there.
fn make_at(home: Option<&Path>, path: &str) -> Result<(), String> {
    let path = expand(path, home)?;
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("A new folder's path can't go up with \"..\".".to_owned());
    }
    let parent = match (path.parent(), path.file_name()) {
        (Some(parent), Some(_)) => parent,
        _ => return Err("Give the new folder a name.".to_owned()),
    };
    if !parent.is_dir() {
        return Err(format!("{} isn't there.", parent.display()));
    }
    fs::create_dir(&path).map_err(|err| match err.kind() {
        io::ErrorKind::AlreadyExists => format!("{} is there already.", path.display()),
        _ => format!("maxd couldn't make {}: {err}", path.display()),
    })
}

/// `path` with a leading `~` as the home folder. Only an absolute path, or
/// one from home, names a folder.
fn expand(path: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if path.starts_with('/') {
        return Ok(PathBuf::from(path));
    }
    let rest = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => rest,
        _ => return Err(NOT_ABSOLUTE.to_owned()),
    };
    let home = home.ok_or(NO_HOME)?;
    match rest.trim_start_matches('/') {
        "" => Ok(home.to_path_buf()),
        rest => Ok(home.join(rest)),
    }
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::testing::TempDir;

    fn me() -> DeviceId {
        "0199a7c2-0000-7000-8000-000000000007".parse().unwrap()
    }

    fn names(listing: &FolderListing) -> Vec<(&str, bool)> {
        listing
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.folder))
            .collect()
    }

    /// A folder holding folders, files, a hidden folder and links.
    fn tree() -> TempDir {
        let tmp = TempDir::new();
        let root = tmp.path();
        for folder in ["b-app", "A-app", ".cache", "shared"] {
            fs::create_dir(root.join(folder)).unwrap();
        }
        for file in ["c.txt", "B.md"] {
            fs::write(root.join(file), "").unwrap();
        }
        symlink(root.join("shared"), root.join("linked")).unwrap();
        symlink(root.join("c.txt"), root.join("note")).unwrap();
        symlink(root.join("gone"), root.join("broken")).unwrap();
        tmp
    }

    #[test]
    fn folders_come_first_by_name_and_links_count_as_what_they_reach() {
        let tmp = tree();
        let path = tmp.path().to_string_lossy();
        let listing = read(me(), None, &path, true, false).unwrap();
        assert!(listing.exists);
        assert!(!listing.truncated);
        assert_eq!(
            listing.path,
            fs::canonicalize(tmp.path()).unwrap().to_string_lossy()
        );
        assert_eq!(
            names(&listing),
            [
                ("A-app", true),
                ("b-app", true),
                ("linked", true),
                ("shared", true),
                ("B.md", false),
                ("c.txt", false),
                ("note", false),
            ],
            "the broken link is left out"
        );
    }

    #[test]
    fn hidden_names_and_files_show_when_asked() {
        let tmp = tree();
        let path = tmp.path().to_string_lossy();
        let folders = read(me(), None, &path, false, false).unwrap();
        assert_eq!(
            names(&folders),
            [
                ("A-app", true),
                ("b-app", true),
                ("linked", true),
                ("shared", true)
            ]
        );
        let hidden = read(me(), None, &path, false, true).unwrap();
        assert_eq!(names(&hidden)[0], (".cache", true));
        assert_eq!(hidden.entries.len(), 5);
    }

    #[test]
    fn a_big_folder_is_cut_short() {
        let tmp = TempDir::new();
        for n in 0..MOST + 3 {
            fs::create_dir(tmp.path().join(format!("f{n:04}"))).unwrap();
        }
        let listing = read(me(), None, &tmp.path().to_string_lossy(), false, false).unwrap();
        assert_eq!(listing.entries.len(), MOST);
        assert!(listing.truncated);
        assert_eq!(listing.entries[0].name, "f0000");
    }

    #[test]
    fn a_missing_folder_or_a_file_is_an_answer() {
        let tmp = tree();
        let home = tmp.path();
        let missing = read(me(), Some(home), "~/new/place", true, false).unwrap();
        assert!(!missing.exists);
        assert!(missing.entries.is_empty());
        assert_eq!(missing.path, home.join("new/place").to_string_lossy());
        assert_eq!(missing.home, home.to_string_lossy());
        let file = read(me(), Some(home), "~/c.txt", true, false).unwrap();
        assert!(!file.exists);
        let at_home = read(me(), Some(home), "~", false, false).unwrap();
        assert!(at_home.exists);
        assert_eq!(at_home.entries.len(), 4);
    }

    #[test]
    fn only_a_full_path_or_one_from_home_lists() {
        for path in ["code", "./code", "~other/code", ""] {
            assert_eq!(
                read(me(), Some(Path::new("/home/dev")), path, false, false),
                Err(NOT_ABSOLUTE.to_owned()),
                "{path}"
            );
        }
        assert_eq!(
            read(me(), None, "~/code", false, false),
            Err(NO_HOME.to_owned())
        );
    }

    #[test]
    fn checkouts_say_their_branch_and_project() {
        let tmp = TempDir::new();
        let repo = tmp.path().join("example-app");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:owner/repo.git\n",
        )
        .unwrap();
        let loose = tmp.path().join("scratch");
        fs::create_dir_all(loose.join(".git")).unwrap();
        fs::write(loose.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
        fs::create_dir(tmp.path().join("plain")).unwrap();

        let listing = read(me(), None, &tmp.path().to_string_lossy(), false, false).unwrap();
        let [app, plain, scratch] = &listing.entries[..] else {
            panic!("{listing:?}");
        };
        let shared = names::project(me(), "", Some("git@github.com:owner/repo.git")).id;
        assert_eq!(
            app.checkout,
            Some(FolderCheckout {
                branch: Some("main".to_owned()),
                project: shared,
            }),
            "the same project as the repo's checkouts on every device"
        );
        assert_eq!(plain.checkout, None);
        let root = fs::canonicalize(tmp.path()).unwrap().join("scratch");
        assert_eq!(
            scratch.checkout,
            Some(FolderCheckout {
                branch: None,
                project: names::project(me(), &root.to_string_lossy(), None).id,
            }),
            "a detached checkout with no remote"
        );
    }

    #[test]
    fn makes_one_folder_in_one_that_is_there() {
        let tmp = TempDir::new();
        let home = Some(tmp.path());
        assert_eq!(make_at(home, "~/new"), Ok(()));
        assert!(tmp.path().join("new").is_dir());
        let refusals = [
            ("~/new", "is there already."),
            ("~/a/b", "isn't there."),
            ("~/new/../up", "can't go up"),
            ("relative", "full path"),
            ("/", "a name"),
        ];
        for (path, says) in refusals {
            let reason = make_at(home, path).unwrap_err();
            assert!(reason.contains(says), "{path}: {reason}");
        }
        assert!(!tmp.path().join("a").exists(), "nothing on the way is made");
    }

    #[tokio::test]
    async fn a_folder_for_another_device_is_refused() {
        let other: DeviceId = "0199a7c2-0000-7000-8000-000000000008".parse().unwrap();
        let refused = ActionOutcome::Refused {
            reason: OTHER_DEVICE.to_owned(),
        };
        assert_eq!(list(me(), other, "/".into(), false, false).await, refused);
        assert_eq!(make(me(), other, "/tmp/x".into()).await, refused);
        assert!(matches!(
            list(me(), me(), "/".into(), false, false).await,
            ActionOutcome::FolderListed { .. }
        ));
    }
}
