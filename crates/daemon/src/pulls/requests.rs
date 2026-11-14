// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The app's PR requests: the list, the viewer's reads, and merging and
//! approving as you. Reads go through a [`PullSource`], so asking again
//! soon costs nothing. A merge or an approval drops what's cached for the
//! PR before and after it, and a merge is told as merged only once a
//! reread says so.

use std::cmp::Reverse;
use std::collections::HashMap;

use futures::stream::{self, StreamExt};
use maxhq_protocol::ids::{AgentId, ProjectId};
use maxhq_protocol::message::{ActionOutcome, ClientMessage};
use maxhq_protocol::pulls::{
    ChecksState, LinkSource, MergeMethod, PullAction, PullCheck, PullComment, PullDetail, PullFile,
    PullFileStatus, PullKey, PullLink, PullListState, PullReview, PullSnapshot, PullState,
    PullSummary, ReviewVerdict,
};
use maxhq_protocol::state::{Entity, EntityKey};

use super::read;
use super::source::{Pull, PullSource, Review};
use crate::hub::Hub;
use crate::observe::claude::time::parse_iso8601;
use crate::secrets::{self, GithubError, RepoName, UserToken};

/// How many PRs of a list have their checks and reviews read at once.
const AT_ONCE: usize = 8;

/// Why a merge or an approval of a head you didn't see is refused.
pub const MOVED_MERGE: &str = "The PR changed since you looked. Reload it and merge again.";
pub const MOVED_APPROVE: &str = "The PR changed since you looked. Reload it and approve again.";
/// Why GitHub refused you a merge or an approval.
const NO_WRITE_MERGE: &str = "You need write access on this repository to merge.";
const NO_WRITE_APPROVE: &str = "You need write access on this repository to approve.";
const NOT_OPEN: &str = "This PR isn't open anymore.";
const YOUR_OWN: &str = "GitHub doesn't let you approve your own PR.";

/// Answers one PR request from the app.
pub async fn handle(message: ClientMessage, hub: Hub) -> ActionOutcome {
    let Some(source) = secrets::pull_source() else {
        return refused(&GithubError::NoApp);
    };
    let links = links(&hub);
    let answered = match message {
        ClientMessage::ListPulls { project, state, .. } => {
            let Some(repo) = project_repo(&hub, project) else {
                return refused(&GithubError::BadRepo);
            };
            list(source, &repo, state, &links)
                .await
                .map(|pulls| ActionOutcome::Pulls { pulls })
        }
        ClientMessage::ListBranches { project, .. } => match project_repo(&hub, project) {
            Some(repo) => branches(source, &repo).await,
            None => Err(GithubError::BadRepo),
        },
        ClientMessage::GetPull { repo, number, .. } => match RepoName::parse(&repo) {
            Some(repo) => {
                detail(source, &repo, number, &links)
                    .await
                    .map(|pull| ActionOutcome::Pull {
                        pull: Box::new(pull),
                    })
            }
            None => Err(GithubError::BadRepo),
        },
        ClientMessage::PullFiles {
            repo, number, page, ..
        } => match RepoName::parse(&repo) {
            Some(repo) => files(source, &repo, number, page)
                .await
                .map(|(files, more)| ActionOutcome::PullFiles { files, more }),
            None => Err(GithubError::BadRepo),
        },
        ClientMessage::PullComments { repo, number, .. } => match RepoName::parse(&repo) {
            Some(repo) => comments(source, &repo, number)
                .await
                .map(|comments| ActionOutcome::PullComments { comments }),
            None => Err(GithubError::BadRepo),
        },
        ClientMessage::PullChecks { repo, number, .. } => match RepoName::parse(&repo) {
            Some(repo) => match source.pull(&repo, number).await {
                Ok(pull) => {
                    let (state, checks) = shown_checks(source, &repo, &pull).await;
                    Ok(ActionOutcome::PullChecks { state, checks })
                }
                Err(err) => Err(err),
            },
            None => Err(GithubError::BadRepo),
        },
        ClientMessage::PullAction {
            repo,
            number,
            action,
            expect_sha,
            ..
        } => {
            let Some(repo) = RepoName::parse(&repo) else {
                return refused(&GithubError::BadRepo);
            };
            let user = secrets::github_user().await;
            return act(source, user, &repo, number, action, &expect_sha).await;
        }
        _ => return refused_in(crate::pulls::NOT_YET),
    };
    answered.unwrap_or_else(|err| refused(&err))
}

fn refused(err: &GithubError) -> ActionOutcome {
    refused_in(&err.to_string())
}

fn refused_in(reason: &str) -> ActionOutcome {
    ActionOutcome::Refused {
        reason: reason.to_owned(),
    }
}

/// The repository of `project`, where it's on GitHub.
fn project_repo(hub: &Hub, project: ProjectId) -> Option<RepoName> {
    hub.get(&EntityKey::Project(project))
        .and_then(|entity| match entity {
            Entity::Project(project) => project.repo,
            _ => None,
        })
        .as_deref()
        .and_then(RepoName::parse)
}

/// Every PR linked to an agent on this device, and the agent.
fn links(hub: &Hub) -> Vec<(AgentId, PullLink)> {
    hub.pick(|entity| match entity {
        Entity::Agent(agent) => Some((agent.id, agent.pulls.clone())),
        _ => None,
    })
    .into_iter()
    .flat_map(|(agent, pulls)| pulls.into_iter().map(move |link| (agent, link)))
    .filter(|(_, link)| link.source != LinkSource::Dismissed)
    .collect()
}

/// Whether `key` is PR `number` in `repo`.
fn is(key: &PullKey, repo: &RepoName, number: u64) -> bool {
    key.number == number && RepoName::parse(&key.repo).as_ref() == Some(repo)
}

/// The agents PR `number` in `repo` is linked to, and the snapshot the
/// watcher read of it last.
fn linked<'a>(
    links: &'a [(AgentId, PullLink)],
    repo: &RepoName,
    number: u64,
) -> (Vec<AgentId>, Option<&'a PullSnapshot>) {
    let mut agents = Vec::new();
    let mut snapshot = None;
    for (agent, link) in links.iter().filter(|(_, link)| is(&link.key, repo, number)) {
        if !agents.contains(agent) {
            agents.push(*agent);
        }
        snapshot = snapshot.or(link.snapshot.as_ref());
    }
    (agents, snapshot)
}

/// The repository's open PRs, newest change first, each with its checks
/// and reviews, or the closed and merged ones linked to agents here.
pub async fn list(
    source: &dyn PullSource,
    repo: &RepoName,
    state: PullListState,
    links: &[(AgentId, PullLink)],
) -> Result<Vec<PullSummary>, GithubError> {
    if state == PullListState::Closed {
        // The open list is all GitHub is read for. Closed PRs are the
        // ones the watcher saw close.
        let mut seen: HashMap<u64, PullSummary> = HashMap::new();
        for (_, link) in links {
            let Some(snapshot) = &link.snapshot else {
                continue;
            };
            if snapshot.state == PullState::Open
                || !is(&link.key, repo, link.key.number)
                || seen.contains_key(&link.key.number)
            {
                continue;
            }
            let (agents, _) = linked(links, repo, link.key.number);
            let summary = PullSummary {
                key: link.key.clone(),
                pull: snapshot.clone(),
                agents,
            };
            seen.insert(link.key.number, summary);
        }
        let mut closed: Vec<PullSummary> = seen.into_values().collect();
        closed.sort_by_key(|summary| Reverse(summary.pull.updated_at));
        return Ok(closed);
    }
    let open = source.open_list(repo).await?;
    let rows = stream::iter(open)
        .map(|pull| async move {
            let ((checks, _), reviews) = tokio::join!(
                shown_checks(source, repo, &pull),
                source.reviews(repo, pull.number),
            );
            // A list leaves out what merges and the line counts, so they
            // come from what the watcher read, where it read the PR.
            let (agents, seen) = linked(links, repo, pull.number);
            let review = read::review_state(&reviews.unwrap_or_default());
            let mut snapshot = read::snapshot(&pull, checks, review, None);
            if let Some(seen) = seen.filter(|seen| seen.head_sha == snapshot.head_sha) {
                snapshot.mergeable = seen.mergeable;
                snapshot.additions = seen.additions;
                snapshot.deletions = seen.deletions;
            }
            PullSummary {
                key: key(repo, pull.number),
                pull: snapshot,
                agents,
            }
        })
        .buffered(AT_ONCE)
        .collect()
        .await;
    Ok(rows)
}

/// A PR for the viewer.
pub async fn detail(
    source: &dyn PullSource,
    repo: &RepoName,
    number: u64,
    links: &[(AgentId, PullLink)],
) -> Result<PullDetail, GithubError> {
    let pull = source.pull(repo, number).await?;
    let ((state, checks), reviews, settings) = tokio::join!(
        shown_checks(source, repo, &pull),
        source.reviews(repo, number),
        source.settings(repo),
    );
    let reviews = reviews?;
    let (agents, seen) = linked(links, repo, number);
    Ok(PullDetail {
        summary: PullSummary {
            key: key(repo, number),
            pull: read::snapshot(&pull, state, read::review_state(&reviews), seen),
            agents,
        },
        body: pull.body,
        checks,
        reviews: reviews.iter().filter_map(review).collect(),
        // Settings that can't be read offer every method, and GitHub
        // refuses one the repository doesn't allow.
        merge_methods: settings.unwrap_or_default().methods(),
    })
}

/// A page of the files a PR changes, and whether there are more.
pub async fn files(
    source: &dyn PullSource,
    repo: &RepoName,
    number: u64,
    page: u32,
) -> Result<(Vec<PullFile>, bool), GithubError> {
    let (files, more) = source.files(repo, number, page).await?;
    let files = files
        .into_iter()
        .map(|file| PullFile {
            status: match file.status.as_str() {
                "added" => PullFileStatus::Added,
                "removed" => PullFileStatus::Removed,
                "renamed" => PullFileStatus::Renamed,
                "copied" => PullFileStatus::Copied,
                "changed" => PullFileStatus::Changed,
                "unchanged" => PullFileStatus::Unchanged,
                _ => PullFileStatus::Modified,
            },
            path: file.filename,
            additions: file.additions,
            deletions: file.deletions,
            patch: file.patch,
        })
        .collect();
    Ok((files, more))
}

/// A PR's conversation: its comments and what its reviews said, oldest
/// first.
pub async fn comments(
    source: &dyn PullSource,
    repo: &RepoName,
    number: u64,
) -> Result<Vec<PullComment>, GithubError> {
    let (comments, reviews) =
        tokio::join!(source.comments(repo, number), source.reviews(repo, number));
    Ok(read::conversation(&reviews?, &comments?))
}

/// The repository's branches, for the new-agent form, and its default
/// branch where its settings can be read.
pub async fn branches(
    source: &dyn PullSource,
    repo: &RepoName,
) -> Result<ActionOutcome, GithubError> {
    let (branches, settings) = tokio::join!(source.branches(repo), source.settings(repo));
    Ok(ActionOutcome::Branches {
        branches: branches?,
        default_branch: settings.ok().and_then(|settings| settings.default_branch),
    })
}

/// Merges or approves a PR as `user`, with what's cached for it dropped
/// before and after, whatever came of it.
pub async fn act(
    source: &dyn PullSource,
    user: Result<UserToken, GithubError>,
    repo: &RepoName,
    number: u64,
    action: PullAction,
    expect_sha: &str,
) -> ActionOutcome {
    let user = match user {
        Ok(user) => user,
        Err(err) => return refused(&err),
    };
    source.forget(repo, Some(number));
    let acted = match action {
        PullAction::Merge { method } => {
            merge(source, &user, repo, number, method, expect_sha).await
        }
        PullAction::Approve => approve(source, &user, repo, number, expect_sha).await,
    };
    source.forget(repo, Some(number));
    match acted {
        Ok(Acted::Merged) => {
            // A merge GitHub took can still wait in a merge queue, so only a
            // reread that says merged tells it as merged.
            let merged = source
                .pull(repo, number)
                .await
                .is_ok_and(|pull| pull.merged_at.is_some());
            ActionOutcome::PullMerged { queued: !merged }
        }
        Ok(Acted::Approved) => ActionOutcome::Done,
        Err(reason) => refused_in(&reason),
    }
}

/// What a merge or an approval came to.
enum Acted {
    Merged,
    Approved,
}

async fn merge(
    source: &dyn PullSource,
    user: &UserToken,
    repo: &RepoName,
    number: u64,
    method: MergeMethod,
    expect_sha: &str,
) -> Result<Acted, String> {
    match source.merge(user, repo, number, method, expect_sha).await {
        Ok(()) => Ok(Acted::Merged),
        Err(GithubError::Refused { status: 409, .. }) => Err(MOVED_MERGE.to_owned()),
        Err(GithubError::Refused { status: 403, .. }) => Err(NO_WRITE_MERGE.to_owned()),
        Err(err) => Err(err.to_string()),
    }
}

async fn approve(
    source: &dyn PullSource,
    user: &UserToken,
    repo: &RepoName,
    number: u64,
    expect_sha: &str,
) -> Result<Acted, String> {
    let pull = source
        .pull(repo, number)
        .await
        .map_err(|err| err.to_string())?;
    if pull.state != "open" || pull.merged_at.is_some() {
        return Err(NOT_OPEN.to_owned());
    }
    if pull.head.sha != expect_sha {
        return Err(MOVED_APPROVE.to_owned());
    }
    let author = pull.user.as_ref().map(|author| author.login.as_str());
    if author.is_some_and(|author| author.eq_ignore_ascii_case(&user.login)) {
        return Err(YOUR_OWN.to_owned());
    }
    match source.approve(user, repo, number, expect_sha).await {
        Ok(()) => Ok(Acted::Approved),
        Err(GithubError::Refused { status: 403, .. }) => Err(NO_WRITE_APPROVE.to_owned()),
        Err(err) => Err(err.to_string()),
    }
}

fn key(repo: &RepoName, number: u64) -> PullKey {
    maxhq_core::pulls::key(&repo.key(), number)
}

/// A review as the viewer shows it. A review you haven't sent yet isn't.
fn review(review: &Review) -> Option<PullReview> {
    let verdict = match review.state.as_str() {
        "APPROVED" => ReviewVerdict::Approved,
        "CHANGES_REQUESTED" => ReviewVerdict::ChangesRequested,
        "COMMENTED" => ReviewVerdict::Commented,
        "DISMISSED" => ReviewVerdict::Dismissed,
        _ => return None,
    };
    Some(PullReview {
        author: read::author(review.user.as_ref()),
        verdict,
        commit_sha: review.commit_id.clone().unwrap_or_default(),
        at: review.submitted_at.as_deref().and_then(parse_iso8601),
    })
}

/// The checks on a PR's head and how they stand together, unknown where
/// they couldn't be read.
pub async fn shown_checks(
    source: &dyn PullSource,
    repo: &RepoName,
    pull: &Pull,
) -> (ChecksState, Vec<PullCheck>) {
    match read::head_checks(source, repo, pull).await {
        Ok(Some(checks)) => (read::checks_state(&checks), checks),
        _ => (ChecksState::Unknown, Vec::new()),
    }
}

#[cfg(test)]
mod tests;
