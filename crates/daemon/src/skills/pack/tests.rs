// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_core::fs_ext;
use maxhq_os::fsmode;

use super::*;
use crate::testing::TempDir;

/// A fake pack in `<dir>/<name>` whose installer runs `body` from the
/// checkout's root, and its tree.
fn fake(dir: &Path, name: &str, body: &str) -> (PathBuf, TreeHash) {
    let from = dir.join(name);
    fs::create_dir_all(from.join("scripts")).unwrap();
    let script = from.join(INSTALLER);
    fs::write(&script, format!("#!/usr/bin/env bash\nset -eu\n{body}\n")).unwrap();
    fsmode::set_mode(&script, 0o755).unwrap();
    fs::write(from.join("scripts/df.sh"), "#!/bin/sh\n").unwrap();
    let tree = tree::of(&DiskFs, &from).unwrap();
    (from, tree)
}

/// Notes each run in the checkout, as an installer makes its own files.
const NOTE: &str = "echo \"ran pull=$DEVFLOW_SKIP_PULL\" >> installed.txt";

struct Setup {
    dir: TempDir,
    root: PathBuf,
}

impl Setup {
    fn new() -> Self {
        let dir = TempDir::new();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join(".git/info")).unwrap();
        Self { dir, root }
    }

    fn logs(&self) -> PathBuf {
        self.dir.path().join("logs")
    }

    fn land(&self, from: &Path, tree: &TreeHash, retry: bool, timeout: Duration) -> PackState {
        let landing = Landing {
            root: &self.root,
            from,
            tree,
            logs: &self.logs(),
            user: None,
            retry,
            timeout,
        };
        land(&landing, || {})
    }

    fn runs(&self) -> usize {
        fs::read_to_string(self.root.join("installed.txt"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}

#[test]
fn a_pack_lands_and_installs_once_per_tree() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    let installed = PackState::Installed { tree: tree.clone() };
    assert_eq!(s.land(&from, &tree, true, TIMEOUT), installed);
    assert_eq!(s.land(&from, &tree, true, TIMEOUT), installed);
    assert_eq!(s.runs(), 1);
    assert_eq!(
        fs::read_to_string(s.root.join("installed.txt")).unwrap(),
        "ran pull=1\n"
    );
    let vendor = s.root.join(PACK_DIR);
    let mode = fsmode::mode_of(&fs::metadata(vendor.join(INSTALLER)).unwrap());
    if cfg!(unix) {
        assert_eq!(mode & 0o777, 0o755);
    }
    assert!(marker(&vendor).is_some());
    assert_eq!(
        noted(&s.logs(), &s.root).map(|note| note.marker.ran),
        Some(Ran::Installed)
    );
    assert!(log_path(&s.dir.path().join("logs"), &s.root).is_file());
}

/// Makes a `.venv` the way an installer does, with a python, a link and a
/// file only the first run writes.
const MAKES_VENV: &str = "echo ran >> installed.txt
mkdir -p .devflow-vendor/.venv/bin
printf '#!/bin/sh\\necho made\\n' > .devflow-vendor/.venv/bin/python3
chmod 755 .devflow-vendor/.venv/bin/python3
ln -s python3 .devflow-vendor/.venv/bin/python";

#[test]
fn a_new_tree_keeps_the_old_venv() {
    let s = Setup::new();
    let (one, first) = fake(s.dir.path(), "one", MAKES_VENV);
    s.land(&one, &first, true, TIMEOUT);
    let venv = s.root.join(PACK_DIR).join(".venv/bin");
    assert!(noted(&s.logs(), &s.root).unwrap().venv.is_some());
    let (two, second) = fake(s.dir.path(), "two", &format!("{NOTE}\necho two"));
    assert_eq!(
        s.land(&two, &second, false, TIMEOUT),
        PackState::Installed { tree: second }
    );
    assert_eq!(s.runs(), 2);
    assert!(venv.join("python3").is_file());
}

/// A named change made to a `.venv/bin` folder.
type Change = (&'static str, fn(&Path));

#[test]
fn a_venv_changed_since_its_install_isnt_carried_over() {
    let changes: [Change; 5] = [
        ("python", |bin| {
            fs::write(
                bin.join("python3"),
                "#!/bin/sh\necho attacker > pwned.txt\n",
            )
            .unwrap();
        }),
        ("mode", |bin| {
            fsmode::set_mode(&bin.join("python3"), 0o777).unwrap();
        }),
        ("link", |bin| {
            fs_ext::remove_link(&bin.join("python")).unwrap();
            fs_ext::symlink("/bin/sh", bin.join("python")).unwrap();
        }),
        ("added", |bin| {
            fs::write(bin.join("activate"), "echo attacker > pwned.txt\n").unwrap();
        }),
        ("link in place of a folder", |bin| {
            let venv = bin.parent().unwrap();
            let elsewhere = venv.with_file_name("elsewhere");
            fs::rename(venv, &elsewhere).unwrap();
            fs_ext::symlink(&elsewhere, venv).unwrap();
        }),
    ];
    // Windows has no modes to change.
    for (what, change) in changes
        .into_iter()
        .filter(|(what, _)| cfg!(unix) || *what != "mode")
    {
        let s = Setup::new();
        let (one, first) = fake(s.dir.path(), "one", MAKES_VENV);
        s.land(&one, &first, true, TIMEOUT);
        let vendor = s.root.join(PACK_DIR);
        change(&vendor.join(".venv/bin"));
        let (two, second) = fake(s.dir.path(), "two", &format!("{USES_VENV}\necho two"));
        assert_eq!(
            s.land(&two, &second, false, TIMEOUT),
            PackState::Installed { tree: second },
            "{what}"
        );
        assert!(!s.root.join("pwned.txt").exists(), "{what}");
        assert!(vendor.symlink_metadata().unwrap().is_dir(), "{what}");
        assert!(vendor.join(".venv").symlink_metadata().is_err(), "{what}");
    }
}

#[test]
fn the_venv_hash_sees_every_kind_of_change_and_refuses_odd_files() {
    let s = Setup::new();
    let vendor = s.root.join(PACK_DIR);
    let bin = vendor.join(".venv/bin");
    assert_eq!(venv_hash(&vendor), None);
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("python3"), "one").unwrap();
    fs_ext::symlink("python3", bin.join("python")).unwrap();
    let first = venv_hash(&vendor).unwrap();
    assert_eq!(venv_hash(&vendor).as_ref(), Some(&first));

    let mut seen = vec![first];
    let mut check = |what: &str| {
        let now = venv_hash(&vendor).unwrap();
        assert!(!seen.contains(&now), "{what}");
        seen.push(now);
    };
    fs::write(bin.join("python3"), "two").unwrap();
    check("bytes");
    if cfg!(unix) {
        fsmode::set_mode(&bin.join("python3"), 0o755).unwrap();
        check("mode");
    }
    fs_ext::remove_link(&bin.join("python")).unwrap();
    fs_ext::symlink("python3.12", bin.join("python")).unwrap();
    check("link");
    fs::create_dir(bin.join("lib")).unwrap();
    check("folder");
    fs::rename(bin.join("lib"), bin.join("lib2")).unwrap();
    check("name");

    // Windows has no pipes on a disk.
    if cfg!(unix) {
        let made = Command::new("mkfifo").arg(bin.join("pipe")).status();
        assert!(made.unwrap().success());
        assert_eq!(venv_hash(&vendor), None);
    }
}

/// Runs the python in the copy's `.venv` when it can, as devflow's
/// installer does.
const USES_VENV: &str = "PY=.devflow-vendor/.venv/bin/python3
if [ -x \"$PY\" ]; then \"$PY\"; fi
echo ran >> installed.txt";

/// Puts what a repository could commit in `vendor`: a marker for `tree`
/// that says `ran`, and a `.venv` whose python leaves `pwned.txt`.
fn plant(vendor: &Path, tree: &TreeHash, ran: &str) {
    let bin = vendor.join(".venv/bin");
    fs::create_dir_all(&bin).unwrap();
    let python = bin.join("python3");
    fs::write(&python, "#!/bin/sh\necho attacker > pwned.txt\n").unwrap();
    fsmode::set_mode(&python, 0o755).unwrap();
    let tree = serde_json::to_string(tree).unwrap();
    fs::write(
        vendor.join(MARKER),
        format!("{{\"tree\":{tree},\"ran\":\"{ran}\",\"lines\":[]}}"),
    )
    .unwrap();
}

#[test]
fn a_planted_marker_cant_run_a_planted_installer() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    let vendor = s.root.join(PACK_DIR);
    fs::create_dir_all(&vendor).unwrap();
    fs::write(
        vendor.join(INSTALLER),
        "#!/bin/sh\necho attacker > pwned.txt\n",
    )
    .unwrap();
    plant(&vendor, &tree, "pending");
    assert_eq!(
        s.land(&from, &tree, false, TIMEOUT),
        PackState::Installed { tree: tree.clone() }
    );
    assert!(!s.root.join("pwned.txt").exists());
    assert_eq!(s.runs(), 1);
    assert_eq!(
        fs::read(vendor.join(INSTALLER)).unwrap(),
        fs::read(from.join(INSTALLER)).unwrap()
    );
}

#[test]
fn a_planted_marker_cant_keep_a_planted_venv() {
    let other = TreeHash("other".into());
    for (marked, ran) in [
        (None, "pending"),
        (None, "failed"),
        (Some(&other), "installed"),
    ] {
        let s = Setup::new();
        let (from, tree) = fake(s.dir.path(), "pack", USES_VENV);
        let vendor = s.root.join(PACK_DIR);
        // The pack's own files, so that only the `.venv` is out of place.
        apply::copy_dir(&from, &vendor).unwrap();
        plant(&vendor, marked.unwrap_or(&tree), ran);
        assert_eq!(
            s.land(&from, &tree, true, TIMEOUT),
            PackState::Installed { tree: tree.clone() },
            "{ran}"
        );
        assert!(!s.root.join("pwned.txt").exists(), "{ran}");
        assert!(!vendor.join(".venv").exists(), "{ran}");
    }
}

#[test]
fn a_marker_with_no_note_installs_once_more_without_its_venv() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    let vendor = s.root.join(PACK_DIR);
    apply::copy_dir(&from, &vendor).unwrap();
    // What an earlier maxd left, and what a repository can fake.
    plant(&vendor, &tree, "installed");
    let installed = PackState::Installed { tree: tree.clone() };
    assert_eq!(s.land(&from, &tree, false, TIMEOUT), installed);
    assert_eq!(s.runs(), 1);
    assert!(!vendor.join(".venv").exists());
    assert_eq!(s.land(&from, &tree, false, TIMEOUT), installed);
    assert_eq!(s.runs(), 1);
}

#[test]
fn a_retry_gets_a_clean_copy_and_keeps_its_venv() {
    let s = Setup::new();
    // What a real install leaves in the copy before it fails.
    let body = format!(
        "{NOTE}
mkdir -p .devflow-vendor/scripts/__pycache__ .devflow-vendor/.venv
touch .devflow-vendor/scripts/__pycache__/x.pyc .devflow-vendor/.venv/made
exit 2"
    );
    let (from, tree) = fake(s.dir.path(), "pack", &body);
    assert!(matches!(
        s.land(&from, &tree, true, TIMEOUT),
        PackState::Failed { .. }
    ));
    let vendor = s.root.join(PACK_DIR);
    let stale = vendor.join("stale");
    fs::write(&stale, "").unwrap();
    assert!(matches!(
        s.land(&from, &tree, true, TIMEOUT),
        PackState::Failed { .. }
    ));
    assert_eq!(s.runs(), 2);
    assert!(!stale.exists());
    assert!(vendor.join(".venv/made").is_file());
}

#[test]
fn a_link_in_the_packs_place_is_replaced_and_what_it_leads_to_stays() {
    for noted in [false, true] {
        let s = Setup::new();
        let (from, tree) = fake(s.dir.path(), "pack", NOTE);
        let vendor = s.root.join(PACK_DIR);
        if noted {
            s.land(&from, &tree, true, TIMEOUT);
            apply::clear(&vendor).unwrap();
        }
        let elsewhere = s.dir.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        plant(&elsewhere, &TreeHash("other".into()), "failed");
        fs_ext::symlink(&elsewhere, &vendor).unwrap();
        let (two, second) = fake(s.dir.path(), "two", &format!("{NOTE}\necho two"));
        assert_eq!(
            s.land(&two, &second, true, TIMEOUT),
            PackState::Installed { tree: second }
        );
        assert!(vendor.symlink_metadata().unwrap().is_dir());
        assert!(vendor.join(INSTALLER).is_file());
        assert!(!vendor.join(".venv").exists());
        assert!(elsewhere.join(".venv/bin/python3").is_file());
        assert_eq!(marker(&elsewhere).map(|m| m.ran), Some(Ran::Failed));
    }
}

/// Runs git in the checkout, with its own idea of who commits.
fn git(root: &Path, args: &[&str]) {
    let output = crate::guest::worktree::git(None)
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

#[test]
fn a_pack_git_tracks_is_left_alone() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", USES_VENV);
    git(&s.root, &["init", "-q"]);
    // maxd's own copy, then a branch that lands its files over the top.
    let installed = PackState::Installed { tree: tree.clone() };
    assert_eq!(s.land(&from, &tree, true, TIMEOUT), installed);
    let vendor = s.root.join(PACK_DIR);
    plant(&vendor, &TreeHash("other".into()), "pending");
    let monitor = s.dir.path().join("monitor");
    fs::write(&monitor, "#!/bin/sh\ntouch \"$0.ran\"\nexit 1\n").unwrap();
    fsmode::set_mode(&monitor, 0o755).unwrap();
    git(&s.root, &["add", "-f", PACK_DIR]);
    git(&s.root, &["commit", "-q", "-m", "vendored"]);
    git(
        &s.root,
        &["config", "core.fsmonitor", monitor.to_str().unwrap()],
    );

    let (two, second) = fake(s.dir.path(), "two", &format!("{USES_VENV}\necho two"));
    assert_eq!(s.land(&two, &second, true, TIMEOUT), PackState::Vendored);
    assert_eq!(
        remove(&s.root, &s.logs(), None).unwrap(),
        Some(PackState::Vendored)
    );
    assert!(!s.root.join("pwned.txt").exists());
    assert_eq!(s.runs(), 1);
    assert!(vendor.join(".venv/bin/python3").is_file());
    assert!(!s.dir.path().join("monitor.ran").exists());
}

#[test]
fn a_failure_keeps_its_last_lines_and_runs_again_only_on_retry() {
    let s = Setup::new();
    let body = format!("{NOTE}\necho 'error: uv is required' >&2\nexit 2");
    let (from, tree) = fake(s.dir.path(), "pack", &body);
    let PackState::Failed { lines, .. } = s.land(&from, &tree, true, TIMEOUT) else {
        panic!("the install should fail");
    };
    assert!(lines.contains(&"error: uv is required".to_owned()));
    assert!(lines.last().unwrap().starts_with("The installer stopped"));
    assert!(matches!(
        s.land(&from, &tree, false, TIMEOUT),
        PackState::Failed { .. }
    ));
    assert_eq!(s.runs(), 1);
    s.land(&from, &tree, true, TIMEOUT);
    assert_eq!(s.runs(), 2);
}

#[test]
fn an_installer_past_the_timeout_is_stopped() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", "sleep 30");
    let started = Instant::now();
    let PackState::Failed { lines, .. } = s.land(&from, &tree, true, Duration::from_millis(200))
    else {
        panic!("the install should be stopped");
    };
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(lines.last().unwrap().contains("ran past"));
}

#[test]
fn someone_elses_pack_is_left_alone() {
    let s = Setup::new();
    let vendor = s.root.join(PACK_DIR);
    fs::create_dir_all(&vendor).unwrap();
    fs::write(vendor.join(INSTALLER), "mine").unwrap();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    assert_eq!(s.land(&from, &tree, true, TIMEOUT), PackState::Vendored);
    assert!(exclude_lines(&s.root).is_empty());
    assert_eq!(
        remove(&s.root, &s.logs(), None).unwrap(),
        Some(PackState::Vendored)
    );
    assert_eq!(unlocked(&s.root), Some(PackState::Vendored));
    assert_eq!(fs::read_to_string(vendor.join(INSTALLER)).unwrap(), "mine");
    assert_eq!(s.runs(), 0);
}

#[test]
fn removing_the_pack_leaves_what_the_installer_made() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    s.land(&from, &tree, true, TIMEOUT);
    assert_eq!(remove(&s.root, &s.logs(), None).unwrap(), None);
    assert!(!s.root.join(PACK_DIR).exists());
    assert!(s.root.join("installed.txt").is_file());
    assert_eq!(unlocked(&s.root), None);
    assert!(noted(&s.logs(), &s.root).is_none());
}

#[test]
fn removing_the_pack_takes_its_log() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    s.land(&from, &tree, true, TIMEOUT);
    let log = log_path(&s.logs(), &s.root);
    assert!(log.is_file());
    remove(&s.root, &s.logs(), None).unwrap();
    assert!(!log.exists());
    assert_eq!(fs::read_dir(s.logs()).unwrap().count(), 0);
}

/// Links a skill and an agent of the pack the way devflow's installer
/// does, and writes an instructions file of its own.
const LINKS: &str = "mkdir -p .claude/skills .codex/skills .github/agents
ln -s ../../.devflow-vendor/skills/review .claude/skills/review
ln -s ../../.devflow-vendor/skills/review .codex/skills/review
ln -s ../../.devflow-vendor/agents/x.agent.md .github/agents/x.agent.md
echo devflow > CLAUDE.md";

// Git Bash's `ln -s` copies unless it's told to link, so the installer
// leaves no links on Windows.
#[cfg(unix)]
#[test]
fn removing_the_pack_takes_its_links_and_exclude_lines_and_nothing_else() {
    let s = Setup::new();
    let (from, _) = fake(s.dir.path(), "pack", LINKS);
    fs::create_dir_all(from.join("skills/review")).unwrap();
    fs::write(
        from.join("skills/review/SKILL.md"),
        "---\nname: review\n---\n",
    )
    .unwrap();
    fs::create_dir_all(from.join("agents")).unwrap();
    fs::write(from.join("agents/x.agent.md"), "agent").unwrap();
    let tree = tree::of(&DiskFs, &from).unwrap();
    assert_eq!(
        s.land(&from, &tree, true, TIMEOUT),
        PackState::Installed { tree: tree.clone() }
    );
    // Your own skill, a link of someone else's, and a folder you made.
    fs::create_dir_all(s.root.join(".claude/skills/mine")).unwrap();
    fs::write(s.root.join(".claude/skills/mine/SKILL.md"), "mine").unwrap();
    fs::create_dir_all(s.root.join(".cursor/skills")).unwrap();
    let elsewhere = s.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs_ext::symlink(&elsewhere, s.root.join(".cursor/skills/other")).unwrap();
    fs::create_dir_all(s.root.join(".github/workflows")).unwrap();
    let mut exclude = "*.log\n/CLAUDE.md\n# MaxHQ skills\n/.maxhq/\n".to_owned();
    for line in pack::exclude_lines() {
        exclude.push_str(&line);
        exclude.push('\n');
    }
    fs::write(s.root.join(".git/info/exclude"), exclude).unwrap();

    assert_eq!(remove(&s.root, &s.logs(), None).unwrap(), None);
    assert!(!s.root.join(PACK_DIR).exists());
    let gone = |path: &str| s.root.join(path).symlink_metadata().is_err();
    assert!(gone(".claude/skills/review"));
    assert!(gone(".codex"), "a folder only the installer used goes");
    assert!(gone(".github/agents"));
    assert!(s.root.join(".github/workflows").is_dir());
    assert!(s.root.join(".claude/skills/mine/SKILL.md").is_file());
    assert!(s
        .root
        .join(".cursor/skills/other")
        .symlink_metadata()
        .is_ok());
    assert_eq!(
        fs::read_to_string(s.root.join("CLAUDE.md")).unwrap(),
        "devflow\n"
    );
    assert_eq!(
        fs::read_to_string(s.root.join(".git/info/exclude")).unwrap(),
        "*.log\n/CLAUDE.md\n# MaxHQ skills\n/.maxhq/\n"
    );
}

#[test]
fn a_copy_that_isnt_the_tree_is_never_installed() {
    let s = Setup::new();
    let (from, _) = fake(s.dir.path(), "pack", NOTE);
    let PackState::Failed { lines, .. } = s.land(&from, &TreeHash("other".into()), true, TIMEOUT)
    else {
        panic!("the copy should fail");
    };
    assert!(lines[0].contains("changed while it was copied"));
    assert!(!s.root.join(PACK_DIR).exists());
    assert_eq!(s.runs(), 0);
}

/// Who a container's installer runs as, with this test's user as the agent.
/// Containers are Linux, so this and the tests that use it are Unix only.
#[cfg(unix)]
fn agent(s: &Setup) -> RunAs {
    let home = s.dir.path().join("home");
    // The installer runs on the fixed path a guest has. A Nix machine's bash
    // isn't on it, so the test's own goes in the home folder that path starts
    // with.
    let bin = home.join(LOCAL_BIN);
    fs::create_dir_all(&bin).unwrap();
    let path = std::env::var_os("PATH").unwrap();
    let bash = std::env::split_paths(&path)
        .map(|dir| dir.join("bash"))
        .find(|bash| bash.is_file())
        .unwrap();
    fs_ext::symlink(bash, bin.join("bash")).unwrap();
    RunAs {
        user: "agent".to_owned(),
        uid: rustix::process::getuid().as_raw(),
        gid: rustix::process::getgid().as_raw(),
        home,
    }
}

#[cfg(unix)]
#[test]
fn a_containers_copy_is_never_in_the_checkout_while_root_walks_it() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let s = Setup::new();
    let (from, _) = fake(s.dir.path(), "pack", NOTE);
    for sub in ["one", "two", "three"] {
        fs::create_dir_all(from.join(sub)).unwrap();
        for n in 0..100 {
            fs::write(from.join(sub).join(n.to_string()), "x").unwrap();
        }
    }
    let tree = tree::of(&DiskFs, &from).unwrap();
    let user = agent(&s);
    let logs = s.logs();
    let landing = Landing {
        root: &s.root,
        from: &from,
        tree: &tree,
        logs: &logs,
        user: Some(&user),
        retry: true,
        timeout: TIMEOUT,
    };
    // Looks for the copy as the agent does, from the checkout.
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (root, stop) = (s.root.clone(), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                for entry in fs::read_dir(&root).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".tmp") {
                        seen.push(name);
                    }
                }
            }
            seen
        })
    };
    let state = land(&landing, || {});
    stop.store(true, Ordering::Relaxed);
    let seen = watcher.join().unwrap();
    assert_eq!(seen, Vec::<String>::new());
    assert_eq!(state, PackState::Installed { tree: tree.clone() });
    let vendor = s.root.join(PACK_DIR);
    assert!(vendor.join("two/99").is_file());
    assert!(marker(&vendor).is_some());
    let left = fs::read_dir(&logs).unwrap().flatten();
    assert!(!left
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .any(|name| name.ends_with(".copy")));
}

#[cfg(unix)]
#[test]
fn a_containers_copy_is_held_in_a_private_folder_of_maxds() {
    let s = Setup::new();
    let (from, tree) = fake(s.dir.path(), "pack", NOTE);
    let user = agent(&s);
    let logs = s.logs();
    let landing = Landing {
        root: &s.root,
        from: &from,
        tree: &tree,
        logs: &logs,
        user: Some(&user),
        retry: true,
        timeout: TIMEOUT,
    };
    let held = hold(&landing, &s.root.join(PACK_DIR)).unwrap();
    assert_eq!(held.parent(), Some(logs.as_path()));
    let mode = fsmode::mode_of(&fs::metadata(&held).unwrap());
    assert_eq!(mode & 0o777, 0o700);
}

#[cfg(unix)]
#[test]
fn a_venv_comes_over_after_the_copy_lands_in_a_container() {
    let s = Setup::new();
    let user = agent(&s);
    let logs = s.logs();
    let land_as = |from: &Path, tree: &TreeHash, installing: &dyn Fn()| {
        land(
            &Landing {
                root: &s.root,
                from,
                tree,
                logs: &logs,
                user: Some(&user),
                retry: false,
                timeout: TIMEOUT,
            },
            installing,
        )
    };
    let venv = s.root.join(PACK_DIR).join(".venv/bin");
    // The guest's fixed path may have no mkdir, so the venv is made just
    // before the installer runs, as if it made it.
    let (one, first) = fake(s.dir.path(), "one", NOTE);
    land_as(&one, &first, &|| {
        fs::create_dir_all(&venv).unwrap();
        fs::write(venv.join("python3"), "").unwrap();
    });
    let (two, second) = fake(s.dir.path(), "two", &format!("{NOTE}\necho two"));
    assert_eq!(
        land_as(&two, &second, &|| {}),
        PackState::Installed { tree: second }
    );
    assert!(venv.join("python3").is_file());
    let entries: Vec<_> = fs::read_dir(&s.root).unwrap().flatten().collect();
    assert!(!entries
        .iter()
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".old")));
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_markers_name_doesnt_hold_up_the_read() {
    let s = Setup::new();
    let vendor = s.root.join(PACK_DIR);
    fs::create_dir_all(&vendor).unwrap();
    let made = Command::new("mkfifo")
        .arg(vendor.join(MARKER))
        .status()
        .unwrap();
    assert!(made.success());
    let (told, answer) = std::sync::mpsc::channel();
    let root = s.root.clone();
    std::thread::spawn(move || told.send(foreign(&root)));
    let answer = answer.recv_timeout(Duration::from_secs(20));
    assert_eq!(answer, Ok(true));
}

#[test]
fn only_a_small_regular_file_counts_as_the_marker() {
    let s = Setup::new();
    let vendor = s.root.join(PACK_DIR);
    fs::create_dir_all(&vendor).unwrap();
    let text = serde_json::to_string(&Marker {
        tree: TreeHash("tree".into()),
        ran: Ran::Pending,
        lines: Vec::new(),
    })
    .unwrap();

    // An outsider's good marker, through a link. A link to /dev/zero stops at
    // the same check.
    let elsewhere = s.dir.path().join("elsewhere.json");
    fs::write(&elsewhere, &text).unwrap();
    fs_ext::symlink(&elsewhere, vendor.join(MARKER)).unwrap();
    assert!(marker(&vendor).is_none());
    fs_ext::remove_link(&vendor.join(MARKER)).unwrap();

    // Good JSON that runs past the limit.
    let long = format!("{text}{}", " ".repeat(MARKER_LIMIT as usize));
    fs::write(vendor.join(MARKER), long).unwrap();
    assert!(marker(&vendor).is_none());

    fs::write(vendor.join(MARKER), &text).unwrap();
    assert!(marker(&vendor).is_some());
}

// Git Bash's `ln -s` copies, so there's no link to test on Windows.
#[cfg(unix)]
#[test]
fn an_installer_that_swaps_the_copy_for_a_link_gets_no_marker_through_it() {
    let s = Setup::new();
    let decoy = s.dir.path().join("decoy");
    fs::create_dir_all(&decoy).unwrap();
    let body = format!(
        "mv {PACK_DIR} {PACK_DIR}.moved\nln -s '{}' {PACK_DIR}",
        decoy.display()
    );
    let (from, tree) = fake(s.dir.path(), "pack", &body);
    assert_eq!(
        s.land(&from, &tree, true, TIMEOUT),
        PackState::Installed { tree: tree.clone() }
    );
    assert_eq!(fs::read_dir(&decoy).unwrap().count(), 0);
    assert_eq!(
        noted(&s.logs(), &s.root).map(|note| note.marker.ran),
        Some(Ran::Installed)
    );
}
