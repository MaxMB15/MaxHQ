// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Auto-settle's check for work an agent hasn't saved: changes in its
//! worktree that aren't committed, or commits its remote doesn't have. A
//! folder on this device is checked with git here. A container's worktree
//! is checked in the container, as the delete's check for unpushed work
//! is, and only while it runs. Anything that can't be told for sure counts
//! as unknown, and an agent whose work is unknown never settles on its own.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use maxhq_protocol::guest::{GuestUnsaved, MAX_UNSAVED_WORKTREES};
use maxhq_protocol::model::UnsavedSummary;
use maxhq_protocol::time::Timestamp;
use tokio::process::Command;

use super::exec::{IncusExec, Unpushed};
use super::manager::Manager;
use crate::incus::InstanceName;
use crate::recipe::{Exec, ExecUser};
use crate::session::clock::BoxFuture;
use crate::session::holders::{Checkout, Unsaved};

/// How long one check may take before it counts as unknown.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Says what's unsaved in the worktree at `$1`, in the lines
/// [`Unpushed::parse`] reads, and `noupstream` when its branch tracks no
/// remote branch. Exits 3 when it isn't a worktree. Optional locks stay
/// off, so the check never holds up the agent's own git.
///
/// A repository's own config can name programs that git runs during this
/// check. The `-c` options turn off the ones an option can. For the rest
/// the script exits 4 before `status` runs: filters, includes, a worktree
/// set elsewhere and promisor remotes in the repository's config, and any
/// submodule that's checked out, since its config does the same. Filters
/// in the owner's global config stay, as Git LFS needs them.
pub(super) const WORKTREE_SCRIPT: &str = r#"cd "$1" 2>/dev/null || exit 3
set -f
g() { GIT_NO_LAZY_FETCH=1 git -c core.fsmonitor=false -c core.hooksPath=/dev/null -c safe.bareRepository=explicit "$@"; }
[ "$(g rev-parse --is-inside-work-tree 2>/dev/null)" = true ] || exit 3
names=$(g config --local --name-only --list) || exit 4
case "$names" in *extensions.worktreeconfig*)
  names="$names $(g config --worktree --name-only --list)" || exit 4
esac
for name in $names; do
  case "$name" in filter.*|include.*|includeif.*|core.worktree|extensions.partialclone|*.promisor|*.partialclonefilter) exit 4; esac
done
g -c core.quotePath=false ls-files --stage -- :/ | grep '^160000 ' | while read -r _ _ _ path; do
  case "$path" in '"'*) exit 4; esac
  [ ! -e "$path/.git" ] || exit 4
done || exit 4
out=$(GIT_OPTIONAL_LOCKS=0 g status --porcelain --ignore-submodules=dirty --no-renames) || exit 4
[ -z "$out" ] || printf 'changes\t%s\n' "$1"
n=$(g rev-list --count HEAD --not --remotes) || exit 4
branch=$(g rev-parse --abbrev-ref HEAD) || exit 4
[ "$n" = 0 ] || printf 'commits\t%s\t%s\n' "$n" "$branch"
g rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1 || printf 'noupstream\n'"#;

impl Unsaved for Manager {
    fn unsaved(&self, checkout: Checkout) -> BoxFuture<Option<bool>> {
        let manager = self.clone();
        Box::pin(async move {
            let check = async {
                match checkout {
                    Checkout::Folder(dir) => in_folder(&dir).await,
                    Checkout::Container { workspace, dir } => {
                        let record = manager.record(workspace)?;
                        let name = InstanceName::new(&record.instance).ok()?;
                        // A stopped container can't be checked, and starting
                        // it to check isn't auto-settle's to do.
                        let found = manager.incus().instance(&name).await.ok()??;
                        if !found.is_running() {
                            return None;
                        }
                        let exec = IncusExec::new(manager.incus().clone(), name);
                        let cmd = ["sh", "-c", WORKTREE_SCRIPT, "sh", &dir];
                        let output = exec.run(&cmd, ExecUser::Agent, None).await.ok()?;
                        output.success().then(|| verdict(&output.stdout)).flatten()
                    }
                }
            };
            tokio::time::timeout(CHECK_TIMEOUT, check)
                .await
                .ok()
                .flatten()
        })
    }
}

/// Runs the check in a folder on this device. A folder that isn't in a
/// checkout is unknown without running git at all.
async fn in_folder(dir: &Path) -> Option<bool> {
    maxhq_core::git::find_checkout(dir)?;
    let mut command = Command::new("sh");
    command
        .args(["-c", WORKTREE_SCRIPT, "sh"])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0");
    // Tests run git as whoever runs them, so their own config stays out of
    // it, and a git hook's variables would point it at the hook's repo.
    #[cfg(test)]
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE");
    let output = command.output().await.ok()?;
    output
        .status
        .success()
        .then(|| verdict(&output.stdout))
        .flatten()
}

/// What [`WORKTREE_SCRIPT`] printed means: true for changes or commits
/// the remote lacks, false for neither, and unknown for commits on a
/// branch that tracks nothing, since those may be pushed under another
/// name.
fn verdict(stdout: &[u8]) -> Option<bool> {
    let found = Unpushed::parse(stdout);
    if !found.changed.is_empty() {
        return Some(true);
    }
    if found.commits.is_empty() {
        return Some(false);
    }
    let tracked = !String::from_utf8_lossy(stdout)
        .lines()
        .any(|line| line == "noupstream");
    tracked.then_some(true)
}

/// A guest's report summed over its checkouts, past the most a report
/// carries left out.
pub(super) fn summary(worktrees: &[GuestUnsaved], read_at: Timestamp) -> UnsavedSummary {
    let mut summary = UnsavedSummary {
        changed: 0,
        commits: 0,
        no_upstream: 0,
        read_at,
    };
    for worktree in worktrees.iter().take(MAX_UNSAVED_WORKTREES) {
        summary.changed = summary.changed.saturating_add(worktree.changed);
        summary.commits = summary.commits.saturating_add(worktree.commits);
        summary.no_upstream += u32::from(worktree.no_upstream);
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Stdio;

    use super::*;
    use crate::testing::TempDir;

    fn has_git() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A clone of a local bare remote, with one pushed commit.
    fn pushed(root: &Path) -> std::path::PathBuf {
        let remote = root.join("remote.git");
        let work = root.join("work");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "-b", "main"]);
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(work.join("a.txt"), "a\n").unwrap();
        git(&work, &["add", "a.txt"]);
        git(&work, &["commit", "-q", "-m", "first"]);
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        work
    }

    #[tokio::test]
    async fn a_pushed_clean_worktree_has_nothing_unsaved() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        assert_eq!(in_folder(&work).await, Some(false));
    }

    #[tokio::test]
    async fn changes_and_unpushed_commits_are_unsaved() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        std::fs::write(work.join("b.txt"), "b\n").unwrap();
        assert_eq!(in_folder(&work).await, Some(true), "a new file counts");

        git(&work, &["add", "b.txt"]);
        git(&work, &["commit", "-q", "-m", "second"]);
        assert_eq!(
            in_folder(&work).await,
            Some(true),
            "a commit not pushed counts"
        );

        git(&work, &["push", "-q"]);
        assert_eq!(in_folder(&work).await, Some(false));
    }

    #[tokio::test]
    async fn commits_on_a_branch_that_tracks_nothing_are_unknown() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        git(&work, &["checkout", "-q", "-b", "fix-x"]);
        assert_eq!(
            in_folder(&work).await,
            Some(false),
            "a new branch with nothing of its own has nothing to lose"
        );
        std::fs::write(work.join("c.txt"), "c\n").unwrap();
        git(&work, &["add", "c.txt"]);
        git(&work, &["commit", "-q", "-m", "third"]);
        assert_eq!(in_folder(&work).await, None);
    }

    #[tokio::test]
    async fn a_folder_that_isnt_a_repo_is_unknown() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        assert_eq!(in_folder(dir.path()).await, None);
        assert_eq!(in_folder(&dir.path().join("missing")).await, None);
    }

    #[test]
    fn the_verdict_reads_the_scripts_lines() {
        assert_eq!(verdict(b""), Some(false));
        assert_eq!(verdict(b"noupstream\n"), Some(false));
        assert_eq!(verdict(b"changes\t/w\nnoupstream\n"), Some(true));
        assert_eq!(verdict(b"commits\t2\tmain\n"), Some(true));
        assert_eq!(verdict(b"commits\t2\tfix-x\nnoupstream\n"), None);
    }

    /// A program that leaves a file behind if anything runs it.
    struct Planted {
        program: std::path::PathBuf,
        ran: std::path::PathBuf,
    }

    impl Planted {
        fn new(root: &Path) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let program = root.join("planted.sh");
            let ran = root.join("ran");
            let text = format!("#!/bin/sh\ntouch '{}'\nexit 1\n", ran.display());
            std::fs::write(&program, text).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { program, ran }
        }

        fn path(&self) -> &str {
            self.program.to_str().unwrap()
        }
    }

    /// Gives a tracked file a new time, so `status` has to read it again.
    fn touched(work: &Path) {
        std::fs::File::options()
            .write(true)
            .open(work.join("a.txt"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    }

    /// Sends every tracked file through the filter named `name`.
    fn filtered(work: &Path, name: &str) {
        let info = work.join(".git/info");
        std::fs::create_dir_all(&info).unwrap();
        std::fs::write(info.join("attributes"), format!("* filter={name}\n")).unwrap();
        touched(work);
    }

    /// Commits a file and takes the pushed commit's object away, so git
    /// goes looking for it.
    fn missing_object(work: &Path) {
        std::fs::write(work.join("b.txt"), "b\n").unwrap();
        git(work, &["add", "b.txt"]);
        git(work, &["commit", "-q", "-m", "second"]);
        let parent = std::process::Command::new("git")
            .arg("-C")
            .arg(work)
            .args(["rev-parse", "HEAD~1"])
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        let parent = String::from_utf8(parent.stdout).unwrap();
        let (folder, file) = parent.trim().split_at(2);
        std::fs::remove_file(work.join(".git/objects").join(folder).join(file)).unwrap();
    }

    /// Where a planted promisor remote points: an empty folder.
    fn nowhere(work: &Path) -> String {
        let folder = work.parent().unwrap().join("nowhere");
        std::fs::create_dir_all(&folder).unwrap();
        folder.to_str().unwrap().to_owned()
    }

    /// Adds a submodule to `work` and pushes it.
    fn with_submodule(work: &Path) {
        let root = work.parent().unwrap();
        let inner = root.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        git(&inner, &["init", "-q", "-b", "main"]);
        std::fs::write(inner.join("s.txt"), "s\n").unwrap();
        git(&inner, &["add", "s.txt"]);
        git(&inner, &["commit", "-q", "-m", "first"]);
        git(
            work,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "--quiet",
                "add",
                "--",
                inner.to_str().unwrap(),
                "mod",
            ],
        );
        git(work, &["add", "-A"]);
        git(work, &["commit", "-q", "-m", "submodule"]);
        git(work, &["push", "-q"]);
    }

    /// Every way a repository's config can name a program that the check
    /// can't turn off: the program stays unrun and the answer is unknown.
    #[tokio::test]
    async fn a_program_a_repository_names_doesnt_run_and_the_answer_is_unknown() {
        if !has_git() {
            return;
        }
        type Plant = fn(&Path, &Planted) -> std::path::PathBuf;
        let cases: [(&str, Plant); 12] = [
            ("a clean filter", |work, planted| {
                git(work, &["config", "filter.x.clean", planted.path()]);
                filtered(work, "x");
                work.to_owned()
            }),
            ("a process filter", |work, planted| {
                git(work, &["config", "filter.x.process", planted.path()]);
                filtered(work, "x");
                work.to_owned()
            }),
            ("a filter in capitals", |work, planted| {
                let text = format!("[FILTER \"X\"]\n\tCLEAN = {}\n", planted.path());
                let config = work.join(".git/config");
                let mut all = std::fs::read_to_string(&config).unwrap();
                all.push_str(&text);
                std::fs::write(&config, all).unwrap();
                filtered(work, "X");
                work.to_owned()
            }),
            ("an include", |work, planted| {
                let included = work.join("in-tree.cfg");
                let text = format!("[filter \"x\"]\n\tclean = {}\n", planted.path());
                std::fs::write(&included, text).unwrap();
                git(
                    work,
                    &["config", "include.path", included.to_str().unwrap()],
                );
                filtered(work, "x");
                work.to_owned()
            }),
            ("a conditional include", |work, planted| {
                let included = work.join("in-tree.cfg");
                let text = format!("[filter \"x\"]\n\tclean = {}\n", planted.path());
                std::fs::write(&included, text).unwrap();
                let name = "includeIf.gitdir:**.path";
                git(work, &["config", name, included.to_str().unwrap()]);
                filtered(work, "x");
                work.to_owned()
            }),
            ("a worktree's own config", |work, planted| {
                git(work, &["config", "extensions.worktreeConfig", "true"]);
                let text = format!("[filter \"x\"]\n\tclean = {}\n", planted.path());
                std::fs::write(work.join(".git/config.worktree"), text).unwrap();
                filtered(work, "x");
                work.to_owned()
            }),
            ("a git folder inside the checkout", |work, planted| {
                let sub = work.join("sub");
                std::fs::create_dir_all(sub.join("objects")).unwrap();
                std::fs::create_dir_all(sub.join("refs")).unwrap();
                std::fs::write(sub.join("HEAD"), "ref: refs/heads/main\n").unwrap();
                let text = format!(
                    "[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tworktree = .\n\tfsmonitor = {}\n",
                    planted.path()
                );
                std::fs::write(sub.join("config"), text).unwrap();
                std::fs::write(sub.join("x"), "").unwrap();
                sub
            }),
            ("a submodule's own config", |work, planted| {
                with_submodule(work);
                git(
                    &work.join("mod"),
                    &["config", "core.fsmonitor", planted.path()],
                );
                work.to_owned()
            }),
            ("a partial clone's remote", |work, planted| {
                missing_object(work);
                git(work, &["config", "extensions.partialClone", "evil"]);
                git(work, &["config", "remote.evil.promisor", "true"]);
                git(work, &["config", "remote.evil.url", &nowhere(work)]);
                git(work, &["config", "remote.evil.uploadpack", planted.path()]);
                work.to_owned()
            }),
            (
                "a partial clone filter with no promisor",
                |work, planted| {
                    missing_object(work);
                    git(work, &["config", "--unset", "remote.origin.fetch"]);
                    git(
                        work,
                        &["config", "remote.evil.partialclonefilter", "blob:none"],
                    );
                    git(work, &["config", "remote.evil.url", &nowhere(work)]);
                    git(work, &["config", "remote.evil.uploadpack", planted.path()]);
                    work.to_owned()
                },
            ),
            ("a promisor remote with no extension", |work, planted| {
                missing_object(work);
                git(work, &["config", "--unset", "remote.origin.fetch"]);
                git(work, &["config", "remote.evil.promisor", "true"]);
                git(work, &["config", "remote.evil.url", &nowhere(work)]);
                git(work, &["config", "remote.evil.uploadpack", planted.path()]);
                work.to_owned()
            }),
            (
                "a promisor remote with a space in its name",
                |work, planted| {
                    missing_object(work);
                    let text = format!(
                        "[remote \"a b\"]\n\tpromisor = true\n\turl = {}\n\tuploadpack = {}\n",
                        nowhere(work),
                        planted.path()
                    );
                    let config = work.join(".git/config");
                    let mut all = std::fs::read_to_string(&config).unwrap();
                    all.push_str(&text);
                    std::fs::write(&config, all).unwrap();
                    work.to_owned()
                },
            ),
        ];
        let mut wrong = Vec::new();
        for (name, plant) in cases {
            let dir = TempDir::new();
            let work = pushed(dir.path());
            let planted = Planted::new(dir.path());
            let folder = plant(&work, &planted);
            let answer = in_folder(&folder).await;
            let ran = planted.ran.exists();
            if ran || answer.is_some() {
                wrong.push((name, ran, answer));
            }
        }
        assert!(wrong.is_empty(), "(case, ran, answer): {wrong:?}");
    }

    /// An option can turn this one off, so the answer stays a real one.
    #[tokio::test]
    async fn a_planted_fsmonitor_doesnt_run_and_a_clean_worktree_stays_clean() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        let planted = Planted::new(dir.path());
        git(&work, &["config", "core.fsmonitor", planted.path()]);
        assert_eq!(in_folder(&work).await, Some(false));
        assert!(!planted.ran.exists());
    }

    /// `--no-renames` doesn't change the answer.
    #[tokio::test]
    async fn a_staged_rename_still_counts() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        std::fs::rename(work.join("a.txt"), work.join("c.txt")).unwrap();
        git(&work, &["add", "-A"]);
        assert_eq!(in_folder(&work).await, Some(true));
    }

    /// Git LFS sets its filters in the owner's global config, and a repo
    /// only names them. That repo still settles.
    #[tokio::test]
    async fn a_filter_in_the_owners_own_config_still_settles() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        std::fs::write(work.join(".gitattributes"), "*.txt filter=x\n").unwrap();
        git(&work, &["add", ".gitattributes"]);
        git(&work, &["commit", "-q", "-m", "attributes"]);
        git(&work, &["push", "-q"]);
        let global = dir.path().join("gitconfig");
        let ran = dir.path().join("global-filter-ran");
        let filter = dir.path().join("filter.sh");
        std::fs::write(
            &filter,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", ran.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&filter, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let text = format!("[filter \"x\"]\n\tclean = {}\n", filter.display());
        std::fs::write(&global, text).unwrap();
        touched(&work);
        let output = Command::new("sh")
            .args(["-c", WORKTREE_SCRIPT, "sh"])
            .arg(&work)
            .stdin(Stdio::null())
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_WORK_TREE")
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert_eq!(verdict(&output.stdout), Some(false));
        assert!(ran.exists(), "the owner's filter ran");
    }

    /// With no folder checked out there's no config of its own to read.
    #[tokio::test]
    async fn a_submodule_nobody_checked_out_still_settles() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let work = pushed(dir.path());
        with_submodule(&work);
        let fresh = dir.path().join("fresh");
        let remote = dir.path().join("remote.git");
        git(
            dir.path(),
            &["clone", "-q", "--", remote.to_str().unwrap(), "fresh"],
        );
        assert_eq!(in_folder(&fresh).await, Some(false));
    }
}
