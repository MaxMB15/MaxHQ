// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Where a repository's pull requests are read from, and where a merge or
//! an approval goes. The replies keep GitHub's REST shapes, so what reads
//! GitHub only fetches and caches, and what links and watches PRs reads
//! these.

use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_protocol::pulls::MergeMethod;
use serde::Deserialize;

use crate::secrets::{GithubError, RepoName, UserToken};

/// An account on GitHub.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct User {
    /// Its login.
    pub login: String,
    /// "User", "Bot" or "Organization". An older answer may leave it out.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

/// A repository a branch is in.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Repo {
    /// "owner/name".
    pub full_name: String,
}

/// A repository's merge settings and default branch. GitHub leaves the
/// merge settings out for a reader without push access, so one that's
/// missing counts as allowed.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RepoSettings {
    /// Whether a merge commit is allowed.
    #[serde(default = "allowed")]
    pub allow_merge_commit: bool,
    /// Whether a squash merge is allowed.
    #[serde(default = "allowed")]
    pub allow_squash_merge: bool,
    /// Whether a rebase merge is allowed.
    #[serde(default = "allowed")]
    pub allow_rebase_merge: bool,
    /// The branch a clone checks out.
    #[serde(default)]
    pub default_branch: Option<String>,
}

fn allowed() -> bool {
    true
}

impl Default for RepoSettings {
    fn default() -> Self {
        Self {
            allow_merge_commit: true,
            allow_squash_merge: true,
            allow_rebase_merge: true,
            default_branch: None,
        }
    }
}

impl RepoSettings {
    /// The merge methods the repository allows, or all of them where it
    /// says none, so GitHub's refusal tells why.
    pub fn methods(&self) -> Vec<MergeMethod> {
        let methods: Vec<MergeMethod> = [
            (self.allow_merge_commit, MergeMethod::Merge),
            (self.allow_squash_merge, MergeMethod::Squash),
            (self.allow_rebase_merge, MergeMethod::Rebase),
        ]
        .into_iter()
        .filter_map(|(allowed, method)| allowed.then_some(method))
        .collect();
        if methods.is_empty() {
            Self::default().methods()
        } else {
            methods
        }
    }
}

/// One end of a PR.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Branch {
    /// The branch's name.
    #[serde(rename = "ref")]
    pub branch: String,
    /// The commit it's at.
    pub sha: String,
    /// The repository it's in, none when that's been deleted.
    pub repo: Option<Repo>,
}

/// A PR, as `pulls` and `pulls/{n}` give it. The list leaves out
/// `mergeable`, `mergeable_state` and the line counts.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Pull {
    /// Its number.
    pub number: u64,
    /// Its title.
    pub title: String,
    /// "open" or "closed".
    pub state: String,
    /// Whether it's a draft.
    #[serde(default)]
    pub draft: bool,
    /// Who opened it, none for a deleted account.
    pub user: Option<User>,
    /// What it brings in.
    pub head: Branch,
    /// What it goes into.
    pub base: Branch,
    /// Its description.
    pub body: Option<String>,
    /// Its page on GitHub.
    pub html_url: String,
    /// When it last changed, ISO 8601.
    pub updated_at: String,
    /// When it merged, ISO 8601.
    pub merged_at: Option<String>,
    /// Whether it merges cleanly, none while GitHub works it out.
    #[serde(default)]
    pub mergeable: Option<bool>,
    /// Such as "clean", "blocked", "dirty" or "unknown".
    #[serde(default)]
    pub mergeable_state: Option<String>,
    /// Lines added.
    #[serde(default)]
    pub additions: u32,
    /// Lines removed.
    #[serde(default)]
    pub deletions: u32,
}

/// A check run on a commit.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct CheckRun {
    /// Its name.
    pub name: String,
    /// "queued", "in_progress" or "completed".
    pub status: String,
    /// How it ended, once completed.
    pub conclusion: Option<String>,
    /// Its page on GitHub.
    pub html_url: Option<String>,
}

/// A commit's check runs, as `commits/{sha}/check-runs` gives them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct CheckRuns {
    /// The runs.
    pub check_runs: Vec<CheckRun>,
}

/// A commit status.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Status {
    /// What it's for, such as "ci/build".
    pub context: String,
    /// "success", "failure", "error" or "pending".
    pub state: String,
    /// Its page.
    pub target_url: Option<String>,
}

/// A commit's statuses, as `commits/{sha}/status` gives them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Statuses {
    /// All of them together: "success", "failure" or "pending".
    pub state: String,
    /// Each one.
    pub statuses: Vec<Status>,
}

/// A workflow run.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Run {
    /// The workflow's name.
    pub name: Option<String>,
    /// "queued", "in_progress" or "completed".
    pub status: Option<String>,
    /// How it ended, once completed.
    pub conclusion: Option<String>,
    /// Its page on GitHub.
    pub html_url: String,
}

/// A commit's workflow runs, as `actions/runs?head_sha=` gives them. A
/// fork's checks show up here.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Runs {
    /// The runs.
    pub workflow_runs: Vec<Run>,
}

/// A review on a PR.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Review {
    /// Its id.
    pub id: u64,
    /// Who left it.
    pub user: Option<User>,
    /// "APPROVED", "CHANGES_REQUESTED", "COMMENTED" or "DISMISSED".
    pub state: String,
    /// What the account is to the repository, such as "MEMBER" or "NONE".
    #[serde(default)]
    pub author_association: Option<String>,
    /// What it says.
    pub body: Option<String>,
    /// The commit it was left on.
    pub commit_id: Option<String>,
    /// When it was left, ISO 8601.
    pub submitted_at: Option<String>,
    /// Its page on GitHub.
    pub html_url: String,
}

/// A comment on a PR's conversation, as `issues/{n}/comments` gives it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Comment {
    /// Its id.
    pub id: u64,
    /// Who wrote it.
    pub user: Option<User>,
    /// What the account is to the repository, such as "MEMBER" or "NONE".
    #[serde(default)]
    pub author_association: Option<String>,
    /// What it says.
    pub body: Option<String>,
    /// When it was written, ISO 8601.
    pub created_at: String,
    /// Its page on GitHub.
    pub html_url: String,
}

/// A file a PR changes.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct File {
    /// Its path.
    pub filename: String,
    /// Such as "added", "modified" or "renamed".
    pub status: String,
    /// Lines added.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// The diff, none for a binary file or one too large.
    pub patch: Option<String>,
}

/// How fast a repository may be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pacing {
    /// How long GitHub last asked a poller to wait.
    pub poll_after: Option<Duration>,
    /// Requests left this hour, from the last reply.
    pub remaining: Option<u32>,
    /// Requests allowed this hour, from the last reply.
    pub limit: Option<u32>,
    /// How long until the hour starts over.
    pub resets_in: Option<Duration>,
}

/// Where pull requests are read from, and where merges and approvals go.
/// Reads go through a cache, so asking twice in a row costs at most a
/// request GitHub doesn't count.
pub trait PullSource: Send + Sync {
    /// The repository's open PRs, one page.
    fn open_list<'a>(&'a self, repo: &'a RepoName)
        -> BoxFuture<'a, Result<Vec<Pull>, GithubError>>;

    /// One PR, with whether it merges.
    fn pull<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Pull, GithubError>>;

    /// The repository's merge settings.
    fn settings<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<RepoSettings, GithubError>>;

    /// The repository's branches by name, one page.
    fn branches<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<Vec<String>, GithubError>>;

    /// The latest check runs on a commit.
    fn checks<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<CheckRuns, GithubError>>;

    /// The statuses on a commit.
    fn statuses<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<Statuses, GithubError>>;

    /// The workflow runs on a commit, for a fork's PR.
    fn runs<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<Runs, GithubError>>;

    /// A PR's reviews.
    fn reviews<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Vec<Review>, GithubError>>;

    /// A PR's conversation comments.
    fn comments<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Vec<Comment>, GithubError>>;

    /// One page of the files a PR changes, from 1, and whether there are
    /// more.
    fn files<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
        page: u32,
    ) -> BoxFuture<'a, Result<(Vec<File>, bool), GithubError>>;

    /// Merges a PR as you, only while its head is still at `sha`.
    fn merge<'a>(
        &'a self,
        user: &'a UserToken,
        repo: &'a RepoName,
        number: u64,
        method: MergeMethod,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<(), GithubError>>;

    /// Approves a PR as you, at `sha`.
    fn approve<'a>(
        &'a self,
        user: &'a UserToken,
        repo: &'a RepoName,
        number: u64,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<(), GithubError>>;

    /// How fast the repository may be read now.
    fn pacing(&self, repo: &RepoName) -> Pacing;

    /// The login of the GitHub App's bot, such as "maxhq[bot]", which
    /// agents comment and review as. None while the device has no App or
    /// GitHub hasn't said its slug.
    fn bot(&self) -> Option<String>;

    /// Drops what's cached for the repository's list and, given a number,
    /// for that PR, so the next read asks GitHub.
    fn forget(&self, repo: &RepoName, number: Option<u64>);
}

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;
