// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! `ListFolder` and `MakeFolder`: what a folder on this device holds, for
//! the app's path fields and folder browser, and a new folder from the
//! browser. The server lets only the app and proven phones ask.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use maxhq_core::{git, names};
use maxhq_protocol::folders::{FolderCheckout, FolderEntry, FolderListing};
use maxhq_protocol::ids::{DeviceId, IdempotencyKey};
use maxhq_protocol::message::ActionOutcome;
use tokio::sync::{OnceCell, Semaphore};

/// The most entries a listing holds. The device keeps the names that match
/// a field's filter before it cuts the listing short, so any name you type
/// shows, however far down the folder it sorts.
const MOST: usize = 500;

/// The most entries read from one folder, kept or not.
const READ_MOST: usize = 10_000;

/// How long reading one folder, its checkouts' tags included, may take
/// before the listing ends with what it has.
const READ_FOR: Duration = Duration::from_secs(2);

/// How long the answer waits for a folder whose opening alone hangs, as on
/// a network disk that went away.
const ANSWER_IN: Duration = Duration::from_secs(5);

/// How many folder reads and makes run at once. One that hangs keeps its
/// thread until the disk answers, so this many hung ones at most.
const AT_ONCE: usize = 4;

/// How long a request waits for one of the others to end before it's
/// turned away.
const WAIT_TURN: Duration = Duration::from_millis(500);

/// How many `MakeFolder` answers are kept for resends.
const MADE_KEPT: usize = 64;

const OTHER_DEVICE: &str = "That folder is for another device.";
const NOT_ABSOLUTE: &str = "Give the folder as a full path, starting with / or ~.";
const NO_HOME: &str = "maxd can't tell your home folder.";
const TOO_LONG: &str = "That folder took too long to read.";
const MAKE_TOO_LONG: &str = "Making that folder took too long.";
const BUSY: &str = "maxd is still reading other folders. Try again in a moment.";

/// The turns the folder requests take on blocking threads.
static TURNS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(AT_ONCE)));

/// The `MakeFolder` answers kept, by key.
type Made = VecDeque<(IdempotencyKey, Arc<OnceCell<ActionOutcome>>)>;

/// The last `MakeFolder` answers by key, so a resend after a dropped
/// connection gets the first one instead of "there already".
static MADE: LazyLock<Mutex<Made>> = LazyLock::new(Mutex::default);

/// Answers `ListFolder` for this device, `me`.
pub async fn list(
    me: DeviceId,
    device: DeviceId,
    path: String,
    files: bool,
    hidden: bool,
    filter: Option<String>,
) -> ActionOutcome {
    if device != me {
        return refused(OTHER_DEVICE);
    }
    let home = std::env::home_dir();
    let listing = move || read(me, home.as_deref(), &path, files, hidden, filter.as_deref());
    match in_turn(&TURNS, ANSWER_IN, TOO_LONG, listing).await {
        Ok(listing) => ActionOutcome::FolderListed { listing },
        Err(reason) => refused(reason),
    }
}

/// Answers `MakeFolder` for this device, `me`. A resend under the same
/// `key` gets the first answer.
pub async fn make(
    me: DeviceId,
    device: DeviceId,
    key: IdempotencyKey,
    path: String,
) -> ActionOutcome {
    if device != me {
        return refused(OTHER_DEVICE);
    }
    let answer = answer_for(key);
    let made = answer.get_or_try_init(|| async move {
        let home = std::env::home_dir();
        let made = move || Ok(make_at(home.as_deref(), &path));
        match in_turn(&TURNS, ANSWER_IN, MAKE_TOO_LONG, made).await {
            Ok(Ok(())) => Ok(ActionOutcome::Done),
            Ok(Err(reason)) => Ok(refused(reason)),
            // Not kept, so a resend tries again.
            Err(reason) => Err(refused(reason)),
        }
    });
    match made.await {
        Ok(answer) => answer.clone(),
        Err(answer) => answer,
    }
}

/// Where the answer to the `MakeFolder` under `key` is kept, new or the
/// one an earlier send made.
fn answer_for(key: IdempotencyKey) -> Arc<OnceCell<ActionOutcome>> {
    let mut made = MADE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, answer)) = made.iter().find(|(kept, _)| *kept == key) {
        return Arc::clone(answer);
    }
    if made.len() == MADE_KEPT {
        made.pop_front();
    }
    let answer = Arc::new(OnceCell::new());
    made.push_back((key, Arc::clone(&answer)));
    answer
}

/// Runs `work` on a blocking thread once one of `turns` is free, and waits
/// `answer_in` for it. A request that finds every turn taken for a moment
/// is turned away, and a thread that outlives its answer keeps its turn
/// until it ends, so a hung disk ties up a few threads, not all of them.
async fn in_turn<T: Send + 'static>(
    turns: &Arc<Semaphore>,
    answer_in: Duration,
    too_long: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let turn = match tokio::time::timeout(WAIT_TURN, Arc::clone(turns).acquire_owned()).await {
        Ok(Ok(turn)) => turn,
        Ok(Err(_)) | Err(_) => return Err(BUSY.to_owned()),
    };
    let run = tokio::task::spawn_blocking(move || {
        let _turn = turn;
        work()
    });
    match tokio::time::timeout(answer_in, run).await {
        Ok(Ok(done)) => done,
        Ok(Err(_)) | Err(_) => Err(too_long.to_owned()),
    }
}

/// Lists the folder at `path`. A folder that isn't there, or a file, lists
/// as not there. One that can't be read lists as empty. With `filter`,
/// only the names that hold it list, ignoring case, those that start with
/// it first.
fn read(
    me: DeviceId,
    home: Option<&Path>,
    path: &str,
    files: bool,
    hidden: bool,
    filter: Option<&str>,
) -> Result<FolderListing, String> {
    let deadline = Instant::now() + READ_FOR;
    let path = normalize(&expand(path, home)?);
    let exists = fs::metadata(&path).is_ok_and(|meta| meta.is_dir());
    let mut listing = FolderListing {
        path: path.to_string_lossy().into_owned(),
        home: home.map(|home| home.to_string_lossy().into_owned()),
        exists,
        entries: Vec::new(),
        truncated: false,
    };
    let Some(dir) = exists.then(|| fs::read_dir(&path).ok()).flatten() else {
        return Ok(listing);
    };
    let filter = filter
        .map(str::to_lowercase)
        .filter(|filter| !filter.is_empty());
    // Each kept entry with whether it starts with the filter, and its name
    // in lower case to sort by.
    let mut kept = Vec::new();
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
        let lower = name.to_lowercase();
        let starts = match &filter {
            None => true,
            Some(filter) if lower.starts_with(filter.as_str()) => true,
            Some(filter) if lower.contains(filter.as_str()) => false,
            Some(_) => continue,
        };
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
            let entry = FolderEntry {
                name,
                folder,
                checkout: None,
            };
            kept.push((!starts, !folder, lower, entry));
        }
    }
    kept.sort_unstable_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    if kept.len() > MOST {
        kept.truncate(MOST);
        listing.truncated = true;
    }
    listing.entries = kept.into_iter().map(|(.., entry)| entry).collect();
    if !tag_checkouts(me, &path, &mut listing.entries, deadline) {
        listing.truncated = true;
    }
    Ok(listing)
}

/// Tags each folder in `entries` that's the top of a checkout with its
/// branch and project, until `deadline`. Whether it got to the end.
fn tag_checkouts(
    me: DeviceId,
    folder: &Path,
    entries: &mut [FolderEntry],
    deadline: Instant,
) -> bool {
    for entry in entries.iter_mut().filter(|entry| entry.folder) {
        if Instant::now() > deadline {
            return false;
        }
        entry.checkout = checkout(me, &folder.join(&entry.name));
    }
    true
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

/// `path` with each `.` dropped and each `..` taking off the name before
/// it, read as written. Links in it stay links, so a folder reached through
/// one keeps the path you took to it.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            part => normal.push(part),
        }
    }
    normal
}

fn refused(reason: impl Into<String>) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;

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
        let listing = read(me(), None, &path, true, false, None).unwrap();
        assert!(listing.exists);
        assert!(!listing.truncated);
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
    fn a_path_stays_as_you_took_it() {
        let tmp = tree();
        let linked = tmp.path().join("linked");
        // The temporary folder is itself under a link on macOS, and
        // "linked" is one everywhere.
        let listing = read(me(), None, &linked.to_string_lossy(), true, false, None).unwrap();
        assert!(listing.exists);
        assert_eq!(listing.path, linked.to_string_lossy());
        let roundabout = format!("{}/./b-app/../linked/", tmp.path().display());
        let listing = read(me(), None, &roundabout, true, false, None).unwrap();
        assert_eq!(listing.path, linked.to_string_lossy());
        assert_eq!(normalize(Path::new("/../a/./b/..")), Path::new("/a"));
    }

    #[test]
    fn hidden_names_and_files_show_when_asked() {
        let tmp = tree();
        let path = tmp.path().to_string_lossy();
        let folders = read(me(), None, &path, false, false, None).unwrap();
        assert_eq!(
            names(&folders),
            [
                ("A-app", true),
                ("b-app", true),
                ("linked", true),
                ("shared", true)
            ]
        );
        let hidden = read(me(), None, &path, false, true, None).unwrap();
        assert_eq!(names(&hidden)[0], (".cache", true));
        assert_eq!(hidden.entries.len(), 5);
    }

    #[test]
    fn a_big_folder_is_cut_short() {
        let tmp = TempDir::new();
        for n in 0..MOST + 3 {
            fs::create_dir(tmp.path().join(format!("f{n:04}"))).unwrap();
        }
        let path = tmp.path().to_string_lossy();
        let listing = read(me(), None, &path, false, false, None).unwrap();
        assert_eq!(listing.entries.len(), MOST);
        assert!(listing.truncated);
        assert_eq!(listing.entries[0].name, "f0000");
    }

    #[test]
    fn a_filter_finds_names_past_the_cut_those_that_start_with_it_first() {
        let tmp = TempDir::new();
        for n in 0..MOST + 3 {
            fs::create_dir(tmp.path().join(format!("f{n:04}"))).unwrap();
        }
        fs::create_dir(tmp.path().join("old-f0502")).unwrap();
        fs::write(tmp.path().join("F0502.txt"), "").unwrap();
        let path = tmp.path().to_string_lossy();
        let listing = read(me(), None, &path, true, false, Some("F0502")).unwrap();
        assert!(!listing.truncated);
        assert_eq!(
            names(&listing),
            [("f0502", true), ("F0502.txt", false), ("old-f0502", true)]
        );
        let every = read(me(), None, &path, true, false, Some("")).unwrap();
        assert_eq!(
            every.entries.len(),
            MOST,
            "an empty filter lists every name"
        );
    }

    #[test]
    fn a_missing_folder_or_a_file_is_an_answer() {
        let tmp = tree();
        let home = tmp.path();
        let missing = read(me(), Some(home), "~/new/place", true, false, None).unwrap();
        assert!(!missing.exists);
        assert!(missing.entries.is_empty());
        assert_eq!(missing.path, home.join("new/place").to_string_lossy());
        assert_eq!(missing.home.as_deref(), Some(&*home.to_string_lossy()));
        let file = read(me(), Some(home), "~/c.txt", true, false, None).unwrap();
        assert!(!file.exists);
        let at_home = read(me(), Some(home), "~", false, false, None).unwrap();
        assert!(at_home.exists);
        assert_eq!(at_home.entries.len(), 4);
    }

    #[test]
    fn an_unknown_home_is_left_out() {
        let tmp = tree();
        let listing = read(
            me(),
            None,
            &tmp.path().to_string_lossy(),
            false,
            false,
            None,
        )
        .unwrap();
        assert_eq!(listing.home, None);
    }

    #[test]
    fn only_a_full_path_or_one_from_home_lists() {
        for path in ["code", "./code", "~other/code", ""] {
            assert_eq!(
                read(me(), Some(Path::new("/home/dev")), path, false, false, None),
                Err(NOT_ABSOLUTE.to_owned()),
                "{path}"
            );
        }
        assert_eq!(
            read(me(), None, "~/code", false, false, None),
            Err(NO_HOME.to_owned())
        );
    }

    /// A folder holding a checkout with a remote, a detached one with none
    /// and a plain folder.
    fn checkouts() -> TempDir {
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
        tmp
    }

    #[test]
    fn checkouts_say_their_branch_and_project() {
        let tmp = checkouts();
        let path = tmp.path().to_string_lossy();
        let listing = read(me(), None, &path, false, false, None).unwrap();
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
        let root = tmp.path().join("scratch");
        assert_eq!(
            scratch.checkout,
            Some(FolderCheckout {
                branch: None,
                project: names::project(me(), &root.to_string_lossy(), None).id,
            }),
            "a detached checkout with no remote, named by the path as given"
        );
    }

    #[test]
    fn checkouts_past_the_deadline_stay_untagged() {
        let tmp = checkouts();
        let path = tmp.path().to_string_lossy();
        let mut entries = read(me(), None, &path, false, false, None).unwrap().entries;
        for entry in &mut entries {
            entry.checkout = None;
        }
        let gone = Instant::now() - Duration::from_millis(1);
        assert!(!tag_checkouts(me(), tmp.path(), &mut entries, gone));
        assert!(entries.iter().all(|entry| entry.checkout.is_none()));
        let later = Instant::now() + READ_FOR;
        assert!(tag_checkouts(me(), tmp.path(), &mut entries, later));
        assert!(entries[0].checkout.is_some());
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
    async fn a_resent_make_gets_the_first_answer() {
        let tmp = TempDir::new();
        let path = tmp.path().join("new").display().to_string();
        let key = IdempotencyKey::generate();
        assert_eq!(
            make(me(), me(), key, path.clone()).await,
            ActionOutcome::Done
        );
        assert_eq!(
            make(me(), me(), key, path.clone()).await,
            ActionOutcome::Done,
            "the resend of a make whose answer was lost"
        );
        assert!(matches!(
            make(me(), me(), IdempotencyKey::generate(), path).await,
            ActionOutcome::Refused { reason } if reason.ends_with("is there already.")
        ));
    }

    #[tokio::test]
    async fn a_hung_read_keeps_its_turn_and_the_next_waits_then_gives_up() {
        let turns = Arc::new(Semaphore::new(1));
        let (release, hung) = mpsc::channel::<()>();
        let stuck = in_turn(&turns, Duration::from_millis(50), TOO_LONG, move || {
            hung.recv().map_err(|_| "released".to_owned())
        });
        assert_eq!(stuck.await, Err(TOO_LONG.to_owned()));
        let next = in_turn(&turns, ANSWER_IN, TOO_LONG, || Ok(()));
        assert_eq!(
            next.await,
            Err(BUSY.to_owned()),
            "the hung read still holds the turn"
        );
        release.send(()).unwrap();
        let after = in_turn(&turns, ANSWER_IN, TOO_LONG, || Ok(7));
        assert_eq!(after.await, Ok(7), "the turn comes back once the read ends");
    }

    #[tokio::test]
    async fn a_folder_for_another_device_is_refused() {
        let other: DeviceId = "0199a7c2-0000-7000-8000-000000000008".parse().unwrap();
        let refused = ActionOutcome::Refused {
            reason: OTHER_DEVICE.to_owned(),
        };
        let key = IdempotencyKey::generate();
        assert_eq!(
            list(me(), other, "/".into(), false, false, None).await,
            refused
        );
        assert_eq!(make(me(), other, key, "/tmp/x".into()).await, refused);
        assert!(matches!(
            list(me(), me(), "/".into(), false, false, None).await,
            ActionOutcome::FolderListed { .. }
        ));
    }
}
