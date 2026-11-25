// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the redaction
// and discard fixtures. The clones with a fake git and a local bare repo
// are MaxHQ's.

//! The tracker, with a fake `git` that acts out a clone, and the real one
//! cloning a bare repo over file://.

use std::process::Command as StdCommand;
use std::time::Instant;

use super::*;
use crate::testing::TempDir;

/// How long a test waits for anything before failing.
const WAIT: Duration = Duration::from_secs(10);

/// What the sink heard.
#[derive(Clone, Debug, PartialEq)]
enum Heard {
    Show(CloneState),
    Done(PathBuf),
    Removed(PathBuf),
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Heard>>);

impl Recorder {
    fn heard(&self) -> Vec<Heard> {
        self.0.lock().unwrap().clone()
    }

    #[cfg(unix)]
    fn last_state(&self) -> Option<CloneState> {
        self.heard()
            .into_iter()
            .rev()
            .find_map(|heard| match heard {
                Heard::Show(state) => Some(state),
                _ => None,
            })
    }

    /// Waits until what was heard passes `check`.
    async fn until(&self, what: &str, check: impl Fn(&[Heard]) -> bool) {
        let start = Instant::now();
        while !check(&self.heard()) {
            assert!(
                start.elapsed() < WAIT,
                "never heard {what}: {:?}",
                self.heard()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Sink for Recorder {
    fn show(&self, _clone: &Spec, progress: CloneProgress) {
        self.0.lock().unwrap().push(Heard::Show(progress.state));
    }

    fn done(&self, clone: &Spec) {
        self.0
            .lock()
            .unwrap()
            .push(Heard::Done(clone.folder.clone()));
    }

    fn removed(&self, clone: &Spec) {
        self.0
            .lock()
            .unwrap()
            .push(Heard::Removed(clone.folder.clone()));
    }
}

/// A tracker whose git runs `body` as a shell script, where `$5` is the
/// folder's name. Windows can't start a script by itself, so this and the
/// tests that use it are Unix only.
#[cfg(unix)]
fn fake(dir: &Path, body: &str) -> (Arc<Tracker>, Arc<Recorder>) {
    let program = dir.join("git");
    fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    maxhq_os::fsmode::set_mode(&program, 0o755).unwrap();
    tracker(program.into_os_string())
}

fn tracker(git: OsString) -> (Arc<Tracker>, Arc<Recorder>) {
    let sink = Arc::new(Recorder::default());
    let tracker = Tracker::stalling_after(
        DeviceId::derive("device"),
        git,
        sink.clone(),
        Duration::from_millis(200),
    );
    (tracker, sink)
}

fn project() -> ProjectId {
    ProjectId::derive("project")
}

/// A parent folder for clones, as the tracker names it.
fn parent(root: &TempDir) -> (String, PathBuf) {
    let parent = root.path().join("src");
    fs::create_dir(&parent).unwrap();
    let parent = maxhq_core::fs_ext::canonicalize(&parent).unwrap();
    (parent.to_str().unwrap().to_owned(), parent)
}

async fn start(tracker: &Arc<Tracker>, parent: &str, url: &str) -> Result<WorkspaceId, String> {
    tracker
        .start(
            project(),
            url.to_owned(),
            parent.to_owned(),
            "name".to_owned(),
        )
        .await
}

const URL: &str = "https://example.com/owner/name.git";

/// A fake clone that makes the checkout, says how far it got and waits.
#[cfg(unix)]
const HANGS: &str = r#"mkdir -p "$5/.git" && touch "$5/README.md"
printf 'Receiving objects:  42%% (42/100)\r' >&2
exec sleep 30"#;

#[test]
fn credentials_go_from_urls() {
    for (text, want) in [
        (
            "https://user:s3cret@github.com/octocat/hello-world.git",
            "https://github.com/octocat/hello-world.git",
        ),
        (
            "https://github.com/octocat/hello-world.git?access_token=s3cret",
            "https://github.com/octocat/hello-world.git",
        ),
        (
            "https://user:pa@rt@github.com/octocat/hello-world.git",
            "https://github.com/octocat/hello-world.git",
        ),
        (
            "fatal: repository 'https://example.com/owner/name.git/?token=a'b' not found",
            "fatal: repository 'https://example.com/owner/name.git/' not found",
        ),
        (
            "fatal: unable to access \"https://tok@example.com/x?y=1\": denied",
            "fatal: unable to access \"https://example.com/x\": denied",
        ),
        (
            "fatal: could not read Username for 'https://example.com': terminal prompts disabled",
            "fatal: could not read Username for 'https://example.com': terminal prompts disabled",
        ),
        (
            "me@example.com wrote a_https://u@h",
            "me@example.com wrote a_https://u@h",
        ),
    ] {
        assert_eq!(redact(text), want, "{text}");
    }
}

#[test]
fn a_query_after_the_host_gets_a_slash() {
    assert_eq!(
        normalize_url("https://example.com?token=a@b"),
        "https://example.com/?token=a@b"
    );
    assert_eq!(normalize_url(URL), URL);
    assert_eq!(
        normalize_url("git@example.com:owner/name"),
        "git@example.com:owner/name"
    );
}

#[test]
fn a_long_reason_is_cut_with_an_ellipsis() {
    assert_eq!(clamp("short", 10), "short");
    assert_eq!(clamp("abcdef", 4), "abc…");
    assert_eq!(clamp(&"é".repeat(5), 5), "ééééé");
}

#[test]
fn the_tail_keeps_the_last_lines_without_credentials() {
    let mut tail = Tail::default();
    assert_eq!(tail.reason(), NOT_CLONED);
    for line in [
        "Cloning into 'name'...",
        "  ",
        "one",
        "two",
        "three",
        "fatal: unable to access 'https://user:tok@example.com/owner/name.git/': denied",
        "five",
    ] {
        tail.push(line);
    }
    assert_eq!(
        tail.reason(),
        "two three fatal: unable to access 'https://example.com/owner/name.git/': denied five"
    );
}

#[test]
fn a_destination_must_be_a_new_or_empty_folder() {
    let root = TempDir::new();
    let (parent, parent_path) = parent(&root);
    let folder = destination(&parent, "name").unwrap();
    assert_eq!(folder, parent_path.join("name"));
    assert_eq!(check_destination(&folder), Ok(false));
    fs::create_dir(&folder).unwrap();
    assert_eq!(check_destination(&folder), Ok(true));
    fs::write(folder.join("notes.txt"), "mine").unwrap();
    assert_eq!(
        check_destination(&folder),
        Err("Destination path already exists and is not empty.".to_owned())
    );
    let file = parent_path.join("file");
    fs::write(&file, "x").unwrap();
    assert_eq!(
        check_destination(&file),
        Err("Destination path already exists and is not a directory.".to_owned())
    );
    let made = destination(&format!("{parent}/new/deeper"), "name").unwrap();
    assert!(made.parent().unwrap().is_dir());
    for (parent, name) in [
        ("relative", "name"),
        (&format!("{parent}/../src") as &str, "name"),
        (&parent, ".."),
        (&parent, "a/b"),
        (&parent, ""),
    ] {
        assert!(destination(parent, name).is_err(), "{parent} {name}");
    }
}

#[test]
fn only_a_partial_clone_is_discarded() {
    let root = TempDir::new();
    let spec = |name: &str, existed| Spec {
        workspace: WorkspaceId::derive(name),
        project: project(),
        folder: root.path().join(name),
        url: URL.to_owned(),
        existed,
    };
    let partial = spec("partial", false);
    fs::create_dir_all(partial.folder.join(".git")).unwrap();
    fs::write(partial.folder.join("README.md"), "# hello-world").unwrap();
    assert_eq!(discard(&partial), Ok(()));
    assert!(!partial.folder.exists());

    let kept = spec("kept", true);
    fs::create_dir_all(kept.folder.join(".git")).unwrap();
    assert_eq!(discard(&kept), Ok(()));
    assert_eq!(fs::read_dir(&kept.folder).unwrap().count(), 0);

    let foreign = spec("foreign", false);
    fs::create_dir(&foreign.folder).unwrap();
    fs::write(foreign.folder.join("notes.txt"), "keep me").unwrap();
    assert_eq!(
        discard(&foreign),
        Err("Destination path contains files that are not from the clone.".to_owned())
    );
    assert!(foreign.folder.join("notes.txt").exists());

    let file = spec("file", false);
    fs::write(&file.folder, "not a folder").unwrap();
    assert_eq!(
        discard(&file),
        Err("The clone destination could not be inspected.".to_owned())
    );

    assert_eq!(discard(&spec("missing", false)), Ok(()));
}

#[cfg(unix)]
#[tokio::test]
async fn a_clone_shows_its_progress_until_it_stalls_and_cancel_takes_it_away() {
    let root = TempDir::new();
    let (parent, parent_path) = parent(&root);
    let (tracker, sink) = fake(root.path(), HANGS);
    let workspace = start(&tracker, &parent, URL).await.unwrap();
    assert_eq!(sink.heard()[0], Heard::Show(connecting()));
    sink.until("the stall", |heard| {
        heard.contains(&Heard::Show(CloneState::Cloning {
            stage: CloneStage::Receiving,
            percent: Some(42),
            stalled: true,
        }))
    })
    .await;
    assert!(sink.heard().contains(&Heard::Show(CloneState::Cloning {
        stage: CloneStage::Receiving,
        percent: Some(42),
        stalled: false,
    })));
    assert_eq!(
        start(&tracker, &parent, URL).await,
        Err("A clone into this destination is already in progress.".to_owned())
    );

    tracker.cancel(workspace).await.unwrap();
    assert_eq!(sink.last_state(), Some(CloneState::Cancelled));
    assert!(!parent_path.join("name").exists());
    assert_eq!(
        tracker.cancel(workspace).await,
        Err("That clone isn't running.".to_owned())
    );

    tracker.remove(workspace).await.unwrap();
    assert_eq!(
        sink.heard().last(),
        Some(&Heard::Removed(parent_path.join("name")))
    );
    assert_eq!(tracker.retry(workspace).await, Err(UNKNOWN.to_owned()));
}

#[cfg(unix)]
#[tokio::test]
async fn a_url_cant_start_a_program_git_finds_by_its_name() {
    let root = TempDir::new();
    let (parent, _) = parent(&root);
    let helpers = root.path().join("helpers");
    fs::create_dir(&helpers).unwrap();
    let ran = root.path().join("ran");
    let helper = helpers.join("git-remote-evil");
    let text = format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display());
    fs::write(&helper, text).unwrap();
    maxhq_os::fsmode::set_mode(&helper, 0o755).unwrap();
    let body = format!(
        "PATH='{}':\"$PATH\"\nexport PATH\nexec git \"$@\"",
        helpers.display()
    );
    let (tracker, sink) = fake(root.path(), &body);
    start(&tracker, &parent, "evil::x").await.unwrap();
    sink.until("the failure", |heard| {
        matches!(heard.last(), Some(Heard::Show(CloneState::Failed { .. })))
    })
    .await;
    assert!(!ran.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_clone_says_why_and_a_retry_starts_it_again() {
    let root = TempDir::new();
    let (parent, parent_path) = parent(&root);
    let marker = root.path().join("tried");
    let body = format!(
        r#"if [ -e '{marker}' ]; then mkdir -p "$5/.git"; exit 0; fi
touch '{marker}'
mkdir -p "$5/.git"
echo "Cloning into '$5'..." >&2
echo "fatal: unable to access 'https://user:tok@example.com/owner/name.git/': denied" >&2
exit 128"#,
        marker = marker.display()
    );
    let (tracker, sink) = fake(root.path(), &body);
    let workspace = start(&tracker, &parent, URL).await.unwrap();
    sink.until("the failure", |heard| {
        matches!(heard.last(), Some(Heard::Show(CloneState::Failed { .. })))
    })
    .await;
    assert_eq!(
        sink.last_state(),
        Some(CloneState::Failed {
            reason: "fatal: unable to access 'https://example.com/owner/name.git/': denied"
                .to_owned()
        })
    );
    assert_eq!(
        tracker.cancel(workspace).await,
        Err("That clone isn't running.".to_owned())
    );

    tracker.retry(workspace).await.unwrap();
    let folder = parent_path.join("name");
    sink.until("the clone", |heard| {
        heard.contains(&Heard::Done(folder.clone()))
    })
    .await;
    assert!(folder.join(".git").is_dir());
    assert_eq!(tracker.remove(workspace).await, Err(UNKNOWN.to_owned()));
}

#[cfg(unix)]
#[tokio::test]
async fn removing_a_running_clone_stops_it() {
    let root = TempDir::new();
    let (parent, parent_path) = parent(&root);
    let (tracker, sink) = fake(root.path(), HANGS);
    let workspace = start(&tracker, &parent, URL).await.unwrap();
    let folder = parent_path.join("name");
    sink.until("the progress", |heard| heard.len() > 1).await;
    tracker.remove(workspace).await.unwrap();
    assert_eq!(sink.heard().last(), Some(&Heard::Removed(folder.clone())));
    assert!(!folder.exists());
    // The folder is free for another clone.
    start(&tracker, &parent, URL).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn an_empty_folder_that_was_there_stays_after_a_cancel() {
    let root = TempDir::new();
    let (parent, parent_path) = parent(&root);
    let folder = parent_path.join("name");
    fs::create_dir(&folder).unwrap();
    let (tracker, sink) = fake(root.path(), HANGS);
    let workspace = start(&tracker, &parent, URL).await.unwrap();
    sink.until("the progress", |heard| heard.len() > 1).await;
    tracker.cancel(workspace).await.unwrap();
    assert!(folder.is_dir());
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
}

/// Runs git in `dir` without the user's config.
fn git(dir: &Path, args: &[&str]) {
    let status = StdCommand::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Dev", "-c", "user.email=dev@example.com"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn git_clones_a_local_bare_repo() {
    let root = TempDir::new();
    let (parent, parent_path) = parent(&root);
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q"]);
    fs::write(source.join("README.md"), "# name\n").unwrap();
    git(&source, &["add", "README.md"]);
    git(&source, &["commit", "-q", "-m", "first"]);
    git(
        root.path(),
        &["clone", "-q", "--bare", "source", "bare.git"],
    );
    // `file:///C:/...` on Windows, as `file:///tmp/...` on Unix.
    let bare = maxhq_core::skills::path_text(&root.path().join("bare.git"));
    let url = if bare.starts_with('/') {
        format!("file://{bare}")
    } else {
        format!("file:///{bare}")
    };

    let (tracker, sink) = tracker(OsString::from("git"));
    start(&tracker, &parent, &url).await.unwrap();
    let folder = parent_path.join("name");
    sink.until("the clone", |heard| {
        heard.contains(&Heard::Done(folder.clone()))
    })
    .await;
    assert_eq!(
        fs::read_to_string(folder.join("README.md")).unwrap(),
        "# name\n"
    );
    assert!(matches!(
        sink.heard().first(),
        Some(Heard::Show(CloneState::Cloning { .. }))
    ));
}
