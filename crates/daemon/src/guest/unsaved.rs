// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! W-10's unsaved work: each host link hears what the repo and its
//! worktrees hold that no remote has, every minute and when an agent's
//! turn ends. Each checkout costs one `git status` and one count of the
//! commits not on a remote. A report the same as the last one isn't sent
//! again, unless a turn just ended.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use maxhq_hold::RunAs;
use maxhq_protocol::guest::{
    GuestAgent, GuestAgentState, GuestToHost, GuestUnsaved, MAX_UNSAVED_WORKTREES,
};
use maxhq_protocol::ids::AgentId;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

use super::link::{Guest, Users, AGENT};
use super::worktree::{self, WORKTREES};
use crate::guestlink::wire::Outbox;

/// How often the guest reads its checkouts.
pub(crate) const EVERY: Duration = Duration::from_secs(60);

/// One link's reports, which end with it.
pub(super) struct Watch {
    turn: Arc<Notify>,
    busy: HashSet<AgentId>,
    task: AbortHandle,
}

impl Watch {
    /// Starts reporting to `outbox`, first now and then `every`.
    pub(super) fn start(guest: &Guest, every: Duration, outbox: Outbox) -> Self {
        Self::with(guest.work.clone(), guest.users, every, outbox)
    }

    fn with(work: PathBuf, users: Users, every: Duration, outbox: Outbox) -> Self {
        let turn = Arc::new(Notify::new());
        let ended = turn.clone();
        let task = tokio::spawn(async move {
            let mut last = None;
            let mut force = true;
            loop {
                let user = users(AGENT).ok().flatten();
                let work = work.clone();
                let Ok(report) =
                    tokio::task::spawn_blocking(move || read(&work, user.as_ref())).await
                else {
                    return;
                };
                if force || last.as_ref() != Some(&report) {
                    let message = GuestToHost::UnsavedReport {
                        worktrees: report.clone(),
                    };
                    if outbox.send(&message).await.is_err() {
                        return;
                    }
                    last = Some(report);
                }
                force = tokio::select! {
                    () = tokio::time::sleep(every) => false,
                    () = ended.notified() => true,
                };
            }
        })
        .abort_handle();
        Self {
            turn,
            busy: HashSet::new(),
            task,
        }
    }

    /// Hears the host's list of this container's agents, and reads the
    /// checkouts again when one of them stops being busy.
    pub(super) fn agents(&mut self, agents: &[GuestAgent]) {
        if turn_ended(&mut self.busy, agents) {
            self.turn.notify_one();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Whether an agent busy in the last list isn't now, keeping this list's
/// busy ones for the next.
fn turn_ended(busy: &mut HashSet<AgentId>, agents: &[GuestAgent]) -> bool {
    let now: HashSet<AgentId> = agents
        .iter()
        .filter(|agent| agent.state == GuestAgentState::Busy)
        .map(|agent| agent.id)
        .collect();
    let ended = busy.iter().any(|id| !now.contains(id));
    *busy = now;
    ended
}

/// What each checkout under `work` holds that no remote has. A checkout
/// with nothing unsaved, or that git can't read, is left out.
fn read(work: &Path, user: Option<&RunAs>) -> Vec<GuestUnsaved> {
    checkouts(work)
        .into_iter()
        .filter_map(|dir| checkout(&dir, user))
        .filter(|found| found.changed > 0 || found.commits > 0)
        .take(MAX_UNSAVED_WORKTREES)
        .collect()
}

/// The repos in `work` and the worktrees in its worktree folder.
fn checkouts(work: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in [work.to_owned(), work.join(WORKTREES)] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join(".git").exists())
            .collect();
        here.sort();
        found.extend(here);
    }
    found
}

/// One checkout's unsaved work.
fn checkout(dir: &Path, user: Option<&RunAs>) -> Option<GuestUnsaved> {
    let status = run(dir, user, &["status", "--porcelain=v2", "--branch"])?;
    let mut changed = 0u32;
    let mut upstream = false;
    let mut detached = false;
    let mut born = true;
    for line in status.lines() {
        match line.strip_prefix("# ") {
            Some(header) => {
                upstream |= header.starts_with("branch.upstream ");
                detached |= header == "branch.head (detached)";
                born &= header != "branch.oid (initial)";
            }
            None if !line.is_empty() => changed = changed.saturating_add(1),
            None => {}
        }
    }
    let commits = if born {
        run(
            dir,
            user,
            &["rev-list", "--count", "HEAD", "--not", "--remotes"],
        )?
        .trim()
        .parse()
        .ok()?
    } else {
        0
    };
    Some(GuestUnsaved {
        path: dir.to_string_lossy().into_owned(),
        changed,
        commits,
        no_upstream: !upstream && !detached,
    })
}

/// Git's output in `dir`, or None when it fails. Optional locks stay off,
/// so the read never holds up the agent's own git.
fn run(dir: &Path, user: Option<&RunAs>, args: &[&str]) -> Option<String> {
    let output = worktree::git(user)
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guestlink::wire::{Read, Reader};
    use crate::testing::TempDir;

    fn has_git() -> bool {
        worktree::git(None)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = worktree::git(None)
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A work folder with a repo cloned from a local bare remote, one
    /// commit pushed.
    fn work(root: &Path) -> (PathBuf, PathBuf) {
        let remote = root.join("remote.git");
        let work = root.join("work");
        let repo = work.join("ws-alpha");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "-b", "main"]);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "first"]);
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        (work, repo)
    }

    fn agent(id: u64, state: GuestAgentState) -> GuestAgent {
        GuestAgent {
            id: format!("0199a7c2-5f3e-7d10-8a4b-{id:012x}")
                .parse()
                .unwrap(),
            lead: None,
            branch: None,
            title: None,
            pid: None,
            state,
            started: None,
            last_write: None,
            prompt_head: None,
            commands: Vec::new(),
        }
    }

    #[test]
    fn a_clean_pushed_checkout_reports_nothing() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let (work, _) = work(dir.path());
        assert!(read(&work, None).is_empty());
    }

    #[test]
    fn changes_and_commits_show_per_checkout() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let (work, repo) = work(dir.path());
        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        std::fs::write(repo.join("b.txt"), "new\n").unwrap();
        let tree = worktree::path(&repo, "fix/x");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fix/x",
                tree.to_str().unwrap(),
            ],
        );
        std::fs::write(tree.join("c.txt"), "c\n").unwrap();
        git(&tree, &["add", "c.txt"]);
        git(&tree, &["commit", "-q", "-m", "second"]);
        // The worktree lives in the repo's worktree folder, which the
        // guest's own layout puts beside the repos.
        let beside = work.join(WORKTREES);
        std::fs::create_dir_all(&beside).unwrap();
        std::fs::rename(&tree, beside.join("fix-x")).unwrap();
        git(
            &repo,
            &["worktree", "repair", beside.join("fix-x").to_str().unwrap()],
        );

        let found = read(&work, None);
        assert_eq!(
            found,
            vec![
                GuestUnsaved {
                    path: repo.to_string_lossy().into_owned(),
                    changed: 2,
                    commits: 0,
                    no_upstream: false,
                },
                GuestUnsaved {
                    path: beside.join("fix-x").to_string_lossy().into_owned(),
                    changed: 0,
                    commits: 1,
                    no_upstream: true,
                },
            ]
        );
    }

    async fn report(queue: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> Vec<GuestUnsaved> {
        let frame = tokio::time::timeout(Duration::from_secs(10), queue.recv())
            .await
            .expect("a report came")
            .unwrap();
        let mut reader = Reader::new(&frame[..]);
        let Ok(Read::Message(body)) = reader.next().await else {
            panic!("the frame isn't a message");
        };
        match serde_json::from_slice(&body).unwrap() {
            GuestToHost::UnsavedReport { worktrees } => worktrees,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn reports_come_on_change_and_at_turn_end() {
        if !has_git() {
            return;
        }
        let dir = TempDir::new();
        let (work, repo) = work(dir.path());
        let (outbox, mut queue) = Outbox::new();
        let mut watch = Watch::with(work, |_| Ok(None), Duration::from_millis(50), outbox);

        // The first read goes out, though it's empty.
        assert!(report(&mut queue).await.is_empty());

        std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
        let changed = report(&mut queue).await;
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].changed, 1);

        // Nothing changes, so nothing more comes on the clock.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(queue.try_recv().is_err());

        // A turn's end sends it again.
        watch.agents(&[agent(1, GuestAgentState::Busy)]);
        watch.agents(&[agent(1, GuestAgentState::Idle)]);
        assert_eq!(report(&mut queue).await, changed);
    }

    #[test]
    fn a_turn_ends_when_a_busy_agent_isn_t() {
        let mut busy = HashSet::new();
        assert!(!turn_ended(&mut busy, &[agent(1, GuestAgentState::Busy)]));
        assert!(!turn_ended(
            &mut busy,
            &[
                agent(1, GuestAgentState::Busy),
                agent(2, GuestAgentState::Idle)
            ]
        ));
        assert!(turn_ended(&mut busy, &[agent(1, GuestAgentState::Waiting)]));
        assert!(!turn_ended(&mut busy, &[agent(1, GuestAgentState::Idle)]));
        assert!(!turn_ended(&mut busy, &[agent(2, GuestAgentState::Busy)]));
        // An agent that's gone from the list ended its turn too.
        assert!(turn_ended(&mut busy, &[]));
    }
}
