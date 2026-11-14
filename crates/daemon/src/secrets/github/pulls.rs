// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The PR viewer's reads, through the cache, and its merge and approve,
//! with your own token. Reads go with the repo's installation token, or
//! through `gh` where the device has no App or its App doesn't reach the
//! repo. Each kind of reply is used for its own time without asking,
//! and asked for with its ETag after that.

use std::time::Duration;

use futures::future::BoxFuture;
use maxhq_core::github_events::is_sha;
use maxhq_protocol::pulls::MergeMethod;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use super::cache::Reader;
use super::{lock, GithubError, RepoName, UserToken};
use crate::pulls::source::{
    CheckRuns, Comment, File, Pacing, Pull, PullSource, RepoSettings, Review, Runs, Statuses,
};
use crate::secrets::Secrets;

/// How long the open list is used without asking.
const LIST_FRESH: Duration = Duration::from_secs(30);
/// How long an open PR is used without asking.
const PULL_FRESH: Duration = Duration::from_secs(60);
/// How long a merged PR is, since it doesn't change.
const MERGED_FRESH: Duration = Duration::from_secs(10 * 60);
/// How long an open PR is while GitHub works out whether it merges.
const MERGEABLE_FRESH: Duration = Duration::from_secs(5);
/// How long the repository's merge settings are.
const SETTINGS_FRESH: Duration = Duration::from_secs(10 * 60);
/// How long the repository's branches are.
const BRANCHES_FRESH: Duration = Duration::from_secs(60);
/// How long checks and statuses are.
const CHECKS_FRESH: Duration = Duration::from_secs(15);
/// How long reviews, comments and files are.
const DETAIL_FRESH: Duration = Duration::from_secs(60);
/// Items per page.
const PAGE: usize = 100;
/// The most files GitHub lists for a PR.
const MAX_FILES: usize = 3_000;

/// How long `pull` is used without asking.
fn pull_fresh(pull: &Value) -> Duration {
    if !pull["merged_at"].is_null() {
        MERGED_FRESH
    } else if pull["state"] == "open" && pull["mergeable"].is_null() {
        MERGEABLE_FRESH
    } else {
        PULL_FRESH
    }
}

/// A branch in GitHub's list.
#[derive(Deserialize)]
struct Listed {
    name: String,
}

/// Whether `path` is `under` or something below it.
fn below(path: &str, under: &str) -> bool {
    path.strip_prefix(under)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?']))
}

impl Secrets {
    /// `GET path` on `repo` through the cache, read as `T`.
    async fn pull_read<T: DeserializeOwned>(
        &self,
        repo: &RepoName,
        path: &str,
        ttl: impl Fn(&Value) -> Duration,
    ) -> Result<T, GithubError> {
        let reader = self.reader(repo).await?;
        let read = self.read(repo, &reader, path, ttl).await;
        let value = match (read, reader) {
            // No App and no `gh` is said as the App's problem, which
            // Settings fixes.
            (Err(GithubError::NoGh | GithubError::GhSignedOut), Reader::Gh(app)) => {
                return Err(app);
            }
            (read, _) => read?,
        };
        T::deserialize(&*value).map_err(|_| GithubError::BadReply)
    }

    /// `method path` with `body`, as you. When GitHub refuses your token
    /// with 401, it's refreshed and the request goes once more.
    async fn as_you(
        &self,
        user: &UserToken,
        method: Method,
        path: &str,
        body: Value,
    ) -> Result<(), GithubError> {
        let api = &self.inner.github.api;
        let body = &body;
        self.again_if_refused(user.clone(), |user| {
            let method = method.clone();
            async move { api.send(method, path, &user.value, body).await.map(drop) }
        })
        .await
    }
}

impl PullSource for Secrets {
    fn open_list<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<Vec<Pull>, GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!(
                "/repos/{owner}/{name}/pulls?state=open&sort=updated&direction=desc&per_page={PAGE}"
            );
            self.pull_read(repo, &path, |_| LIST_FRESH).await
        })
    }

    fn pull<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Pull, GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/pulls/{number}");
            self.pull_read(repo, &path, pull_fresh).await
        })
    }

    fn settings<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<RepoSettings, GithubError>> {
        Box::pin(async move {
            let path = format!("/repos/{}/{}", repo.owner(), repo.name());
            self.pull_read(repo, &path, |_| SETTINGS_FRESH).await
        })
    }

    fn branches<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<Vec<String>, GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/branches?per_page={PAGE}");
            let listed: Vec<Listed> = self.pull_read(repo, &path, |_| BRANCHES_FRESH).await?;
            Ok(listed.into_iter().map(|branch| branch.name).collect())
        })
    }

    fn checks<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<CheckRuns, GithubError>> {
        Box::pin(async move {
            if !is_sha(sha) {
                return Err(GithubError::BadReply);
            }
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!(
                "/repos/{owner}/{name}/commits/{sha}/check-runs?filter=latest&per_page={PAGE}"
            );
            self.pull_read(repo, &path, |_| CHECKS_FRESH).await
        })
    }

    fn statuses<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<Statuses, GithubError>> {
        Box::pin(async move {
            if !is_sha(sha) {
                return Err(GithubError::BadReply);
            }
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/commits/{sha}/status?per_page={PAGE}");
            self.pull_read(repo, &path, |_| CHECKS_FRESH).await
        })
    }

    fn runs<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<Runs, GithubError>> {
        Box::pin(async move {
            if !is_sha(sha) {
                return Err(GithubError::BadReply);
            }
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!(
                "/repos/{owner}/{name}/actions/runs?head_sha={sha}&event=pull_request&per_page={PAGE}"
            );
            self.pull_read(repo, &path, |_| CHECKS_FRESH).await
        })
    }

    fn reviews<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Vec<Review>, GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/pulls/{number}/reviews?per_page={PAGE}");
            self.pull_read(repo, &path, |_| DETAIL_FRESH).await
        })
    }

    fn comments<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Vec<Comment>, GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/issues/{number}/comments?per_page={PAGE}");
            self.pull_read(repo, &path, |_| DETAIL_FRESH).await
        })
    }

    fn files<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
        page: u32,
    ) -> BoxFuture<'a, Result<(Vec<File>, bool), GithubError>> {
        Box::pin(async move {
            let page = page.max(1);
            let (owner, name) = (repo.owner(), repo.name());
            let path =
                format!("/repos/{owner}/{name}/pulls/{number}/files?per_page={PAGE}&page={page}");
            let files: Vec<File> = self.pull_read(repo, &path, |_| DETAIL_FRESH).await?;
            let read = usize::try_from(page)
                .unwrap_or(usize::MAX)
                .saturating_mul(PAGE);
            let more = files.len() == PAGE && read < MAX_FILES;
            Ok((files, more))
        })
    }

    fn merge<'a>(
        &'a self,
        user: &'a UserToken,
        repo: &'a RepoName,
        number: u64,
        method: MergeMethod,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<(), GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/pulls/{number}/merge");
            let method = match method {
                MergeMethod::Merge => "merge",
                MergeMethod::Squash => "squash",
                MergeMethod::Rebase => "rebase",
            };
            let body = json!({ "merge_method": method, "sha": sha });
            self.as_you(user, Method::PUT, &path, body).await
        })
    }

    fn approve<'a>(
        &'a self,
        user: &'a UserToken,
        repo: &'a RepoName,
        number: u64,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<(), GithubError>> {
        Box::pin(async move {
            let (owner, name) = (repo.owner(), repo.name());
            let path = format!("/repos/{owner}/{name}/pulls/{number}/reviews");
            let body = json!({ "event": "APPROVE", "commit_id": sha });
            self.as_you(user, Method::POST, &path, body).await
        })
    }

    fn pacing(&self, repo: &RepoName) -> Pacing {
        let paced = lock(&self.inner.github.pacing)
            .get(&repo.key())
            .copied()
            .unwrap_or_default();
        let now = self.now().0;
        Pacing {
            poll_after: paced.poll_after,
            remaining: paced.remaining,
            limit: paced.limit,
            resets_in: paced
                .reset
                .map(|reset| Duration::from_millis(reset.saturating_mul(1000).saturating_sub(now))),
        }
    }

    fn bot(&self) -> Option<String> {
        let app = self.inner.github.current()?;
        let slug = lock(&app.found).slug.clone()?;
        Some(format!("{slug}[bot]"))
    }

    fn forget(&self, repo: &RepoName, number: Option<u64>) {
        let base = format!("/repos/{}/{}", repo.owner(), repo.name()).to_ascii_lowercase();
        let list = format!("{base}/pulls?");
        let numbered = number.map(|number| {
            [
                format!("{base}/pulls/{number}"),
                format!("{base}/issues/{number}"),
            ]
        });
        lock(&self.inner.github.cache_reads).forget(|path| {
            path.starts_with(&list)
                || numbered
                    .as_ref()
                    .is_some_and(|paths| paths.iter().any(|under| below(path, under)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_commit_name_goes_in_a_path() {
        assert!(is_sha("0123abcdef"));
        assert!(!is_sha("../../user"));
        assert!(!is_sha("abc"));
        assert!(!is_sha(""));
    }

    #[test]
    fn below_takes_the_path_and_what_it_holds() {
        assert!(below("/repos/o/n/pulls/1", "/repos/o/n/pulls/1"));
        assert!(below(
            "/repos/o/n/pulls/1/files?page=2",
            "/repos/o/n/pulls/1"
        ));
        assert!(!below("/repos/o/n/pulls/12", "/repos/o/n/pulls/1"));
    }

    #[test]
    fn a_merged_pull_is_kept_longer_and_one_github_is_working_out_shorter() {
        let merged = json!({"state": "closed", "merged_at": "2026-10-31T12:00:00Z"});
        let working = json!({"state": "open", "merged_at": null, "mergeable": null});
        let open = json!({"state": "open", "merged_at": null, "mergeable": true});
        assert_eq!(pull_fresh(&merged), MERGED_FRESH);
        assert_eq!(pull_fresh(&working), MERGEABLE_FRESH);
        assert_eq!(pull_fresh(&open), PULL_FRESH);
    }
}
