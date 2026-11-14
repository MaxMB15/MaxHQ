// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The PR watcher: one per device, on step 6's [`poll_loop`]. Every 15
//! seconds it looks at the repositories of the lead agents it watches and
//! reads those whose turn has come, at the cadence in
//! [`maxhq_core::pulls::cadence`]. A due mark, from a push or a merge,
//! runs a repository at the next pass. The open list links PRs by branch
//! and shows which linked ones ended; each linked open PR is read with its
//! checks and reviews, and a watched one with its comments too.
//!
//! The watcher only reads. What it finds goes to the session manager,
//! which owns the links, and only what differs from the links as they
//! stand goes: a read that changed nothing is a poll that found nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use maxhq_core::pulls::{self, cadence, watch};
use maxhq_protocol::ids::{AgentId, ProjectId, WorkspaceId};
use maxhq_protocol::pulls::{
    ChecksState, PullCheck, PullComment, PullKey, PullLink, PullSnapshot, PullState, PullWatch,
};
use maxhq_protocol::state::Entity;
use maxhq_protocol::time::Timestamp;
use tracing::{debug, warn};

use super::read;
use super::source::{Pacing, Pull, PullSource};
use crate::github_events::poll::{poll_loop, Polled};
use crate::hub::Hub;
use crate::secrets::{Fetched, GithubError, RepoName};
use crate::session::clock::Clock;

/// The repositories to read at the next pass, by [`RepoName::key`].
#[derive(Clone, Default)]
pub struct Due(Arc<Mutex<HashSet<String>>>);

impl Due {
    /// Marks a repository, which may have changed on GitHub.
    pub fn mark(&self, repo: &RepoName) {
        self.lock().insert(repo.key());
    }

    fn take(&self) -> HashSet<String> {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, HashSet<String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Something the watcher found.
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    /// An open PR's head is a lead agent's branch.
    Matched { agent: AgentId, key: PullKey },
    /// A linked PR changed.
    Read(Box<ReadPull>),
    /// A linked PR that was never read isn't there.
    Missing(PullKey),
    /// A watched PR couldn't be read.
    Failed(PullKey),
}

/// A linked PR, as just read.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadPull {
    /// Which PR.
    pub key: PullKey,
    /// Its page on GitHub.
    pub url: String,
    /// How it stands.
    pub snapshot: PullSnapshot,
    /// Each of its checks.
    pub checks: Vec<PullCheck>,
    /// Its remarks, read only while it's watched.
    pub remarks: Vec<PullComment>,
    /// The logins of its remarks with a role on the repository, or that are
    /// bot accounts, whose words its agent reads.
    pub known: Vec<String>,
    /// The logins on the PR's own side, whose remarks don't wake: its
    /// author and the App's bot, which agents comment and review as.
    pub own: Vec<String>,
}

impl ReadPull {
    /// What came of the read for `watch`, whose PR last read as `before`.
    pub fn evaluate(
        &self,
        watch: &PullWatch,
        before: Option<&PullSnapshot>,
        now: Timestamp,
    ) -> watch::Report {
        let own: Vec<&str> = self.own.iter().map(String::as_str).collect();
        watch::evaluate(
            watch,
            before,
            &self.snapshot,
            &self.checks,
            &self.remarks,
            &own,
            now,
        )
    }
}

/// Starts the watcher. `found` gets what each pass found.
pub fn start(
    hub: Hub,
    source: Arc<dyn PullSource>,
    clock: Arc<dyn Clock>,
    due: Due,
    found: impl Fn(Vec<Found>) + Send + Sync + 'static,
) {
    let watcher = Arc::new(Watcher {
        hub,
        source,
        clock: clock.clone(),
        due,
        next: Mutex::default(),
    });
    let found = Arc::new(found);
    crate::spawn_task("the PR watcher", async move {
        poll_loop(
            &*clock,
            None,
            |_| {
                let watcher = watcher.clone();
                async move { watcher.pass().await }
            },
            |changes, _| {
                found(changes);
                async {}
            },
        )
        .await;
    });
}

/// When each repository's list and each PR is read next, and the last
/// error each failing repository was warned about.
#[derive(Default)]
struct Next {
    lists: HashMap<String, Instant>,
    pulls: HashMap<PullKey, Instant>,
    warned: HashMap<String, String>,
}

impl Next {
    /// Whether this error is news for the repository: the first since it
    /// last read, or a different one.
    fn fresh(&mut self, key: &str, error: String) -> bool {
        self.warned.get(key) != Some(&error) && {
            self.warned.insert(key.to_owned(), error);
            true
        }
    }
}

struct Watcher {
    hub: Hub,
    source: Arc<dyn PullSource>,
    clock: Arc<dyn Clock>,
    due: Due,
    next: Mutex<Next>,
}

/// A lead agent whose PRs are watched.
struct Watched {
    id: AgentId,
    branch: Option<String>,
    pulls: Vec<PullLink>,
    last_activity_at: Timestamp,
}

/// What the agents in one repository link of one PR.
struct Linked<'a> {
    /// Its last snapshot.
    before: Option<&'a PullSnapshot>,
    /// A watch on it.
    watch: Option<&'a PullWatch>,
    /// When the most recently active agent linking it last was.
    last_activity_at: Timestamp,
}

/// What a pass found in one repository, or none when it read nothing.
type RepoPass = Option<Result<Vec<Found>, GithubError>>;

impl Watcher {
    /// One pass over every watched repository. It fails only when every
    /// repository it read failed, so the loop backs off.
    async fn pass(&self) -> Option<Polled<Vec<Found>>> {
        let repos = watched(&self.hub, self.clock.wall());
        let due = self.due.take();
        {
            let mut next = self.next();
            next.lists.retain(|key, _| repos.contains_key(key));
            next.pulls.retain(|pull, _| repos.contains_key(&pull.repo));
            next.warned.retain(|key, _| repos.contains_key(key));
        }
        let (mut read, mut failed) = (0, 0);
        let mut found = Vec::new();
        for (key, (repo, agents)) in &repos {
            match self.pass_repo(repo, agents, due.contains(key)).await {
                None => {}
                Some(Ok(more)) => {
                    read += 1;
                    found.extend(more);
                    self.next().warned.remove(key);
                }
                Some(Err(err)) => {
                    read += 1;
                    failed += 1;
                    // A device with no App reads nothing, and that's no news;
                    // nor is the same error again on the next pass.
                    if !matches!(err, GithubError::NoApp) && self.next().fresh(key, err.to_string())
                    {
                        warn!(%repo, "couldn't read the repo's PRs: {err}");
                    } else {
                        debug!(%repo, "couldn't read the repo's PRs: {err}");
                    }
                }
            }
        }
        if read > 0 && failed == read {
            return None;
        }
        let fetched = if found.is_empty() {
            Fetched::Unchanged
        } else {
            Fetched::Fresh {
                value: found,
                etag: None,
            }
        };
        Some(Polled {
            fetched,
            poll_after: Some(cadence::DUE_FLOOR),
        })
    }

    /// Reads what's due in one repository.
    async fn pass_repo(&self, repo: &RepoName, agents: &[Watched], due: bool) -> RepoPass {
        let now = self.clock.now();
        let wall = self.clock.wall();
        let key = repo.key();
        let linked = linked(&key, agents);
        let budget = budget(self.source.pacing(repo));
        let open = |link: &Linked| link.before.is_none_or(|s| s.state == PullState::Open);
        let (list_due, mut reads) = {
            let next = self.next();
            let at = |when: Option<&Instant>| due || when.is_none_or(|at| *at <= now);
            let reads: BTreeSet<u64> = linked
                .iter()
                .filter(|(number, link)| {
                    open(link) && at(next.pulls.get(&pulls::key(&key, **number)))
                })
                .map(|(number, _)| *number)
                .collect();
            (at(next.lists.get(&key)), reads)
        };
        if !list_due && reads.is_empty() {
            return None;
        }
        if due {
            self.source.forget(repo, None);
            for number in &reads {
                self.source.forget(repo, Some(*number));
            }
        }

        let mut found = Vec::new();
        if list_due {
            let every = cadence::stretch(cadence::LIST_EVERY, budget);
            self.next().lists.insert(key.clone(), now + every);
            let listed = match self.source.open_list(repo).await {
                Ok(listed) => listed,
                Err(err) => return Some(Err(err)),
            };
            // A linked PR that left the open list merged or closed; one
            // read says which.
            for (number, link) in &linked {
                if open(link) && !listed.iter().any(|pull| pull.number == *number) {
                    reads.insert(*number);
                }
            }
            for (agent, number) in branch_matches(repo, agents, &listed) {
                found.push(Found::Matched {
                    agent,
                    key: pulls::key(&key, number),
                });
                reads.insert(number);
            }
        }

        for number in reads {
            let link = linked.get(&number);
            let last_activity_at = link.map_or(wall, |link| link.last_activity_at);
            let every = cadence::stretch(cadence::pull_every(last_activity_at, wall), budget);
            let pull_key = pulls::key(&key, number);
            self.next().pulls.insert(pull_key.clone(), now + every);
            let before = link.and_then(|link| link.before);
            let watch = link.and_then(|link| link.watch);
            match self.read_pull(repo, number, before, watch).await {
                Ok(Some(read)) => found.push(Found::Read(Box::new(read))),
                Ok(None) => {}
                Err(GithubError::Refused { status: 404, .. }) if before.is_none() => {
                    found.push(Found::Missing(pull_key));
                }
                Err(err) => {
                    debug!(%repo, number, "couldn't read a PR: {err}");
                    if watch.is_some() {
                        found.push(Found::Failed(pull_key));
                    }
                }
            }
        }
        Some(Ok(found))
    }

    /// Reads one PR, or none when nothing about it changed since `before`.
    async fn read_pull(
        &self,
        repo: &RepoName,
        number: u64,
        before: Option<&PullSnapshot>,
        watch: Option<&PullWatch>,
    ) -> Result<Option<ReadPull>, GithubError> {
        let source = &*self.source;
        let pull = source.pull(repo, number).await?;
        let key = pulls::key(&repo.key(), number);
        let (checks, checks_state, review, remarks, known) =
            if read::state(&pull) == PullState::Open {
                let checks = read::head_checks(source, repo, &pull).await?;
                let checks_state = checks
                    .as_deref()
                    .map_or(ChecksState::Unknown, read::checks_state);
                let reviews = source.reviews(repo, number).await?;
                let (remarks, known) = match watch {
                    Some(_) => {
                        let comments = source.comments(repo, number).await?;
                        (
                            read::remarks(&reviews, &comments),
                            read::known(&reviews, &comments),
                        )
                    }
                    None => (Vec::new(), Vec::new()),
                };
                let checks = checks.unwrap_or_default();
                (
                    checks,
                    checks_state,
                    read::review_state(&reviews),
                    remarks,
                    known,
                )
            } else {
                // An ended PR's checks and reviews are as they last were.
                let checks = before.map_or(ChecksState::None, |before| before.checks);
                let review = before.map_or(maxhq_protocol::pulls::ReviewState::None, |before| {
                    before.review
                });
                (Vec::new(), checks, review, Vec::new(), Vec::new())
            };
        let snapshot = read::snapshot(&pull, checks_state, review, before);
        let own = std::iter::once(snapshot.author.clone())
            .chain(source.bot())
            .collect();
        let read = ReadPull {
            key,
            url: pull.html_url,
            snapshot,
            checks,
            remarks,
            known,
            own,
        };
        let wakes = watch.is_some_and(|watch| {
            let report = read.evaluate(watch, before, self.clock.wall());
            !report.changes.is_empty() || report.next != *watch
        });
        if before == Some(&read.snapshot) && !wakes {
            return Ok(None);
        }
        Ok(Some(read))
    }

    fn next(&self) -> MutexGuard<'_, Next> {
        self.next.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The repositories of the lead agents whose PRs are watched, by
/// [`RepoName::key`], with those agents.
fn watched(hub: &Hub, now: Timestamp) -> BTreeMap<String, (RepoName, Vec<Watched>)> {
    let repos: HashMap<ProjectId, RepoName> = hub
        .pick(|entity| match entity {
            Entity::Project(project) => {
                Some((project.id, RepoName::parse(project.repo.as_deref()?)?))
            }
            _ => None,
        })
        .into_iter()
        .collect();
    let projects: HashMap<WorkspaceId, ProjectId> = hub
        .pick(|entity| match entity {
            Entity::Workspace(workspace) => Some((workspace.id, workspace.project?)),
            _ => None,
        })
        .into_iter()
        .collect();
    let mut watched: BTreeMap<String, (RepoName, Vec<Watched>)> = BTreeMap::new();
    for agent in hub.leads() {
        if !cadence::watched(&agent, now) {
            continue;
        }
        let Some(repo) = projects.get(&agent.workspace).and_then(|p| repos.get(p)) else {
            continue;
        };
        watched
            .entry(repo.key())
            .or_insert_with(|| (repo.clone(), Vec::new()))
            .1
            .push(Watched {
                id: agent.id,
                branch: agent.branch,
                pulls: agent.pulls,
                last_activity_at: agent.last_activity_at,
            });
    }
    watched
}

/// The PRs in repository `key` that the agents link and show, by number.
fn linked<'a>(key: &str, agents: &'a [Watched]) -> BTreeMap<u64, Linked<'a>> {
    let mut linked: BTreeMap<u64, Linked<'a>> = BTreeMap::new();
    for agent in agents {
        let links = agent.pulls.iter();
        for link in links.filter(|link| link.key.repo == key && pulls::shown(link)) {
            let entry = linked.entry(link.key.number).or_insert(Linked {
                before: link.snapshot.as_ref(),
                watch: None,
                last_activity_at: agent.last_activity_at,
            });
            entry.watch = entry.watch.or(link.watch.as_ref());
            entry.last_activity_at = entry.last_activity_at.max(agent.last_activity_at);
        }
    }
    linked
}

/// The open PRs whose head branch, in the repository itself, is an
/// agent's branch, for each agent that doesn't link them yet. One you
/// unlinked stays unlinked.
fn branch_matches(repo: &RepoName, agents: &[Watched], listed: &[Pull]) -> Vec<(AgentId, u64)> {
    let mut matches = Vec::new();
    for pull in listed {
        let head = &pull.head;
        let here = head
            .repo
            .as_ref()
            .and_then(|head| RepoName::parse(&head.full_name))
            .is_some_and(|head| head == *repo);
        if !here || head.branch == pull.base.branch {
            continue;
        }
        let key = pulls::key(&repo.key(), pull.number);
        for agent in agents {
            let on_it = agent.branch.as_deref() == Some(head.branch.as_str());
            if on_it && !agent.pulls.iter().any(|link| link.key == key) {
                matches.push((agent.id, pull.number));
            }
        }
    }
    matches
}

/// What the cadence needs of a repository's pacing.
fn budget(pacing: Pacing) -> cadence::Budget {
    cadence::Budget {
        poll_after: pacing.poll_after,
        remaining: pacing.remaining,
        limit: pacing.limit,
        resets_in: pacing.resets_in,
    }
}

#[cfg(test)]
mod tests {
    use super::Next;

    #[test]
    fn the_same_error_is_news_once_until_the_repo_reads() {
        let mut next = Next::default();
        let missing = || "The GitHub App isn't installed".to_owned();
        assert!(next.fresh("a/b", missing()));
        assert!(!next.fresh("a/b", missing()));
        assert!(next.fresh("a/c", missing()));
        assert!(next.fresh("a/b", "GitHub didn't answer".to_owned()));
        next.warned.remove("a/b");
        assert!(next.fresh("a/b", "GitHub didn't answer".to_owned()));
    }
}
