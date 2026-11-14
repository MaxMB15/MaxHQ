// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A [`PullSource`] that answers from what a test puts in it, and keeps
//! what it was asked.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use futures::future::BoxFuture;
use maxhq_protocol::pulls::MergeMethod;

use super::{
    CheckRuns, Comment, File, Pacing, Pull, PullSource, RepoSettings, Review, Runs, Statuses,
};
use crate::secrets::{GithubError, RepoName, UserToken};

/// How many files one page holds, small so tests reach a second page.
pub const FILES_PAGE: usize = 2;

/// A PR by its repository's lowercase "owner/name" and its number.
pub type Number = (String, u64);

/// What the fake answers with, and what it was asked.
#[derive(Default)]
pub struct State {
    /// Each repository's PRs, open or not. The open list is the open ones.
    pub pulls: HashMap<String, Vec<Pull>>,
    /// Each repository's merge settings, all allowed where unset.
    pub settings: HashMap<String, RepoSettings>,
    /// Each repository's branches.
    pub branches: HashMap<String, Vec<String>>,
    /// Check runs by commit.
    pub checks: HashMap<String, CheckRuns>,
    /// Statuses by commit.
    pub statuses: HashMap<String, Statuses>,
    /// Workflow runs by commit.
    pub runs: HashMap<String, Runs>,
    /// Reviews by PR.
    pub reviews: HashMap<Number, Vec<Review>>,
    /// Comments by PR.
    pub comments: HashMap<Number, Vec<Comment>>,
    /// Changed files by PR.
    pub files: HashMap<Number, Vec<File>>,
    /// What [`PullSource::pacing`] says.
    pub pacing: Pacing,
    /// What [`PullSource::bot`] says.
    pub bot: Option<String>,
    /// While set, every call fails with it.
    pub failing: Option<GithubError>,
    /// Each call, such as "pull owner/repo#12", in order.
    pub calls: Vec<String>,
    /// Each merge: the PR, the method, the login it was made as.
    pub merged: Vec<(Number, MergeMethod, String)>,
    /// Each approval: the PR, the commit, the login it was made as.
    pub approved: Vec<(Number, String, String)>,
}

/// The fake.
#[derive(Default)]
pub struct FakePulls {
    state: Mutex<State>,
}

impl FakePulls {
    /// What it answers with and was asked, to change or check.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Notes a call, then answers it, or fails while `failing` is set.
    fn answer<'a, T: Send + 'a>(
        &self,
        call: String,
        answer: impl FnOnce(&mut State) -> Result<T, GithubError>,
    ) -> BoxFuture<'a, Result<T, GithubError>> {
        let mut state = self.state();
        state.calls.push(call);
        let result = match state.failing.clone() {
            Some(err) => Err(err),
            None => answer(&mut state),
        };
        Box::pin(async move { result })
    }
}

/// GitHub's answer for something that isn't there.
fn not_found() -> GithubError {
    GithubError::Refused {
        status: 404,
        message: "Not Found".to_owned(),
    }
}

/// The PR `number` in `repo`, to read or change.
fn find<'s>(
    state: &'s mut State,
    repo: &RepoName,
    number: u64,
) -> Result<&'s mut Pull, GithubError> {
    state
        .pulls
        .get_mut(&repo.key())
        .and_then(|pulls| pulls.iter_mut().find(|pull| pull.number == number))
        .ok_or_else(not_found)
}

impl PullSource for FakePulls {
    fn open_list<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<Vec<Pull>, GithubError>> {
        self.answer(format!("open_list {repo}"), |state| {
            let pulls = state.pulls.get(&repo.key()).into_iter().flatten();
            Ok(pulls.filter(|pull| pull.state == "open").cloned().collect())
        })
    }

    fn pull<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Pull, GithubError>> {
        self.answer(format!("pull {repo}#{number}"), |state| {
            find(state, repo, number).map(|pull| pull.clone())
        })
    }

    fn settings<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<RepoSettings, GithubError>> {
        self.answer(format!("settings {repo}"), |state| {
            Ok(state.settings.get(&repo.key()).cloned().unwrap_or_default())
        })
    }

    fn branches<'a>(
        &'a self,
        repo: &'a RepoName,
    ) -> BoxFuture<'a, Result<Vec<String>, GithubError>> {
        self.answer(format!("branches {repo}"), |state| {
            Ok(state.branches.get(&repo.key()).cloned().unwrap_or_default())
        })
    }

    fn checks<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<CheckRuns, GithubError>> {
        self.answer(format!("checks {repo}@{sha}"), |state| {
            Ok(state.checks.get(sha).cloned().unwrap_or_default())
        })
    }

    fn statuses<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<Statuses, GithubError>> {
        self.answer(format!("statuses {repo}@{sha}"), |state| {
            Ok(state.statuses.get(sha).cloned().unwrap_or_default())
        })
    }

    fn runs<'a>(
        &'a self,
        repo: &'a RepoName,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<Runs, GithubError>> {
        self.answer(format!("runs {repo}@{sha}"), |state| {
            Ok(state.runs.get(sha).cloned().unwrap_or_default())
        })
    }

    fn reviews<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Vec<Review>, GithubError>> {
        self.answer(format!("reviews {repo}#{number}"), |state| {
            Ok(state
                .reviews
                .get(&(repo.key(), number))
                .cloned()
                .unwrap_or_default())
        })
    }

    fn comments<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
    ) -> BoxFuture<'a, Result<Vec<Comment>, GithubError>> {
        self.answer(format!("comments {repo}#{number}"), |state| {
            Ok(state
                .comments
                .get(&(repo.key(), number))
                .cloned()
                .unwrap_or_default())
        })
    }

    fn files<'a>(
        &'a self,
        repo: &'a RepoName,
        number: u64,
        page: u32,
    ) -> BoxFuture<'a, Result<(Vec<File>, bool), GithubError>> {
        self.answer(format!("files {repo}#{number} page {page}"), |state| {
            let files = state
                .files
                .get(&(repo.key(), number))
                .cloned()
                .unwrap_or_default();
            let start = (page.max(1) as usize - 1) * FILES_PAGE;
            let shown = files.iter().skip(start).take(FILES_PAGE).cloned().collect();
            Ok((shown, files.len() > start + FILES_PAGE))
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
        self.answer(format!("merge {repo}#{number}"), |state| {
            let pull = find(state, repo, number)?;
            if pull.head.sha != sha {
                return Err(GithubError::Refused {
                    status: 409,
                    message: "Head branch was modified. Review and try the merge again.".to_owned(),
                });
            }
            pull.state = "closed".to_owned();
            pull.merged_at = Some(pull.updated_at.clone());
            state
                .merged
                .push(((repo.key(), number), method, user.login.clone()));
            Ok(())
        })
    }

    fn approve<'a>(
        &'a self,
        user: &'a UserToken,
        repo: &'a RepoName,
        number: u64,
        sha: &'a str,
    ) -> BoxFuture<'a, Result<(), GithubError>> {
        self.answer(format!("approve {repo}#{number}"), |state| {
            find(state, repo, number)?;
            let approval = ((repo.key(), number), sha.to_owned(), user.login.clone());
            state.approved.push(approval);
            Ok(())
        })
    }

    fn pacing(&self, _: &RepoName) -> Pacing {
        self.state().pacing
    }

    fn bot(&self) -> Option<String> {
        self.state().bot.clone()
    }

    fn forget(&self, repo: &RepoName, number: Option<u64>) {
        let call = match number {
            Some(number) => format!("forget {repo}#{number}"),
            None => format!("forget {repo}"),
        };
        self.state().calls.push(call);
    }
}
