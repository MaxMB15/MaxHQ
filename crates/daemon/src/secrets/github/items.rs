// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The composer's `#` lookup: a repo's pull requests and issues, read
//! with the repo's installation token, or through `gh` where the device
//! has no App or its App doesn't reach the repo. It reads the repo's most
//! recently updated pull requests and issues, looks a number up on its own
//! when they don't hold it, and for words asks GitHub's search too. A list
//! is kept for [`FRESH`] and then asked for with its ETag, so a list that
//! didn't change costs no rate limit. A search is kept for
//! [`SEARCH_FRESH`], and no more than [`SEARCHES_PER_MINUTE`] go out, below
//! GitHub's 30. Nothing here logs a token.

use std::time::Duration;

use maxhq_core::git_items::{self, FoundItem, MAX_GIT_ITEMS};
use maxhq_protocol::context::{GitItemKind, GitItemState};
use maxhq_protocol::find::GitItemSearch;
use serde::Deserialize;
use serde_json::Value;

use super::cache::Reader;
use super::{lock, GithubError, RepoName};
use crate::secrets::Secrets;

/// How long a list is used before GitHub is asked again.
const FRESH: Duration = Duration::from_secs(30);
/// How long a search is used before GitHub is asked again.
const SEARCH_FRESH: Duration = Duration::from_secs(60);
/// The most searches sent in a minute.
pub(super) const SEARCHES_PER_MINUTE: usize = 20;
/// How much of a body an item carries.
const BODY_CHARS: usize = 2_000;
/// How many recent pull requests, and issues, a lookup reads.
const RECENT: usize = 100;
/// How many items a search asks for.
const SEARCHED: usize = 20;
/// The longest query a search sends, in characters. GitHub takes 256 in
/// all, and the repo goes with it.
const MAX_SEARCH: usize = 128;

/// Why a lookup names a repo that isn't on GitHub.
pub const NOT_GITHUB: &str = "The agent's repo isn't on GitHub.";

/// A pull request as `/pulls` gives it.
#[derive(Deserialize)]
struct Pull {
    number: u32,
    title: String,
    html_url: String,
    state: String,
    #[serde(default)]
    draft: bool,
    merged_at: Option<String>,
    body: Option<String>,
    updated_at: String,
    head: Option<Branch>,
    base: Option<Branch>,
}

/// A pull request's branch.
#[derive(Deserialize)]
struct Branch {
    #[serde(rename = "ref")]
    name: String,
}

/// An issue as `/issues` and the search give it. A pull request is one
/// too, with `pull_request` set.
#[derive(Deserialize)]
struct Issue {
    number: u32,
    title: String,
    html_url: String,
    state: String,
    state_reason: Option<String>,
    #[serde(default)]
    draft: bool,
    body: Option<String>,
    updated_at: String,
    pull_request: Option<IssuePull>,
}

/// What an issue says of the pull request it is.
#[derive(Deserialize)]
struct IssuePull {
    merged_at: Option<String>,
}

impl Pull {
    fn found(self, repo: &str) -> FoundItem {
        FoundItem {
            repo: repo.to_owned(),
            number: self.number,
            kind: GitItemKind::PullRequest,
            title: self.title,
            state: pull_state(&self.state, self.draft, self.merged_at.is_some()),
            url: self.html_url,
            head: self.head.map(|branch| branch.name),
            base: self.base.map(|branch| branch.name),
            body: excerpt(self.body),
            updated_at: self.updated_at,
        }
    }
}

impl Issue {
    fn found(self, repo: &str) -> FoundItem {
        let (kind, state) = match &self.pull_request {
            Some(pull) => (
                GitItemKind::PullRequest,
                pull_state(&self.state, self.draft, pull.merged_at.is_some()),
            ),
            None => (
                GitItemKind::Issue,
                issue_state(&self.state, self.state_reason.as_deref()),
            ),
        };
        FoundItem {
            repo: repo.to_owned(),
            number: self.number,
            kind,
            title: self.title,
            state,
            url: self.html_url,
            head: None,
            base: None,
            body: excerpt(self.body),
            updated_at: self.updated_at,
        }
    }
}

/// Where a pull request stands.
fn pull_state(state: &str, draft: bool, merged: bool) -> GitItemState {
    match state {
        _ if merged => GitItemState::Merged,
        "open" if draft => GitItemState::Draft,
        "open" => GitItemState::Open,
        _ => GitItemState::Closed,
    }
}

/// Where an issue stands.
fn issue_state(state: &str, reason: Option<&str>) -> GitItemState {
    match (state, reason) {
        ("open", _) => GitItemState::Open,
        (_, Some("completed")) => GitItemState::Completed,
        (_, Some("not_planned")) => GitItemState::NotPlanned,
        _ => GitItemState::Closed,
    }
}

/// The start of a body, with Windows line ends made plain.
fn excerpt(body: Option<String>) -> String {
    body.unwrap_or_default()
        .replace("\r\n", "\n")
        .chars()
        .take(BODY_CHARS)
        .collect()
}

/// Each item of a list, skipping any GitHub described oddly.
fn each<T: for<'de> Deserialize<'de>>(list: &Value) -> impl Iterator<Item = T> + '_ {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|one| T::deserialize(one).ok())
}

/// What a reply holds.
#[derive(Clone, Copy)]
enum Shape {
    /// A list of pull requests.
    Pulls,
    /// A list of issues, whose pull requests are skipped, since the pull
    /// requests' own list names their branches.
    Issues,
    /// A search's items, pull requests among them.
    Search,
    /// One pull request.
    OnePull,
    /// One issue, which may be a pull request.
    OneIssue,
}

impl Shape {
    fn read(self, value: &Value, repo: &str) -> Vec<FoundItem> {
        match self {
            Self::Pulls => each::<Pull>(value).map(|pull| pull.found(repo)).collect(),
            Self::Issues => each::<Issue>(value)
                .filter(|issue| issue.pull_request.is_none())
                .map(|issue| issue.found(repo))
                .collect(),
            Self::Search => each::<Issue>(&value["items"])
                .map(|issue| issue.found(repo))
                .collect(),
            Self::OnePull => Pull::deserialize(value)
                .map(|pull| vec![pull.found(repo)])
                .unwrap_or_default(),
            Self::OneIssue => Issue::deserialize(value)
                .map(|issue| vec![issue.found(repo)])
                .unwrap_or_default(),
        }
    }
}

/// `text` for a URL's query, with only unreserved characters left as
/// they are.
fn encoded(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Why neither the App nor `gh` could look in `repo`, naming both ways
/// out. Anything else `gh` hit is said as it is.
fn neither(app: &GithubError, gh: GithubError, repo: &RepoName) -> String {
    let (no_app, fix_app, device, unseen) = match app {
        GithubError::NotInstalled(_) => (
            format!("The GitHub App isn't installed on {repo}"),
            "Install the App there from its page on GitHub",
            "this device",
            "it".to_owned(),
        ),
        _ => (
            "This device has no GitHub App".to_owned(),
            "Send the App to it in Settings",
            "it",
            repo.to_string(),
        ),
    };
    match gh {
        GithubError::NoGh => format!(
            "{no_app}, and gh isn't installed here. {fix_app}, or install gh and run gh auth login on {device}."
        ),
        GithubError::GhSignedOut => format!(
            "{no_app}, and gh isn't signed in here. {fix_app}, or run gh auth login on {device}."
        ),
        GithubError::Refused { status: 404, .. } => format!(
            "{no_app}, and the account gh is signed in to can't see {unseen}. {fix_app}, or sign gh in to an account that can."
        ),
        other => other.to_string(),
    }
}

/// Whether GitHub said the token can't read what was asked, or that it
/// isn't there.
fn out_of_reach(err: &GithubError) -> bool {
    matches!(
        err,
        GithubError::Refused {
            status: 403 | 404 | 410,
            ..
        }
    )
}

impl Secrets {
    /// Answers a `#` lookup for `query` in `repo`, "owner/name" or a
    /// GitHub clone URL, through the App where it reaches the repo and
    /// `gh` where it doesn't. A query of digits looks that number up, a
    /// query of words searches titles too, and an empty one lists the most
    /// recently updated. A leading `#` is dropped.
    pub async fn find_git_items(&self, repo: &str, query: &str) -> GitItemSearch {
        let Some(repo) = RepoName::parse(repo) else {
            return GitItemSearch {
                repo: None,
                items: Vec::new(),
                unavailable: Some(NOT_GITHUB.to_owned()),
            };
        };
        let query = query.trim();
        let query = query.strip_prefix('#').unwrap_or(query).trim();
        let reader = match self.reader(&repo).await {
            Ok(reader) => reader,
            Err(err) => return unavailable(&repo, err.to_string()),
        };
        let (items, unavailable) = match self.look_up(&repo, &reader, query).await {
            Ok(found) => {
                let now = self.now();
                let items = found.into_iter().map(|item| item.into_ref(now)).collect();
                (items, None)
            }
            Err(err) => (
                Vec::new(),
                Some(match reader {
                    Reader::App(_) => err.to_string(),
                    Reader::Gh(app) => neither(&app, err, &repo),
                }),
            ),
        };
        GitItemSearch {
            repo: Some(repo.to_string()),
            items,
            unavailable,
        }
    }

    async fn look_up(
        &self,
        repo: &RepoName,
        reader: &Reader,
        query: &str,
    ) -> Result<Vec<FoundItem>, GithubError> {
        let recent = self.recent(repo, reader).await?;
        if query.is_empty() {
            return Ok(git_items::newest(recent, MAX_GIT_ITEMS));
        }
        if query.bytes().all(|byte| byte.is_ascii_digit()) {
            let mut found = recent;
            if let Ok(number) = query.parse::<u32>()
                && !found.iter().any(|item| item.number == number)
            {
                found.extend(self.numbered(repo, reader, number).await?);
            }
            return Ok(git_items::by_number(found, query, MAX_GIT_ITEMS));
        }
        // A search that fails or waits its turn leaves the recent matches.
        let searched = self.searched(repo, reader, query).await.unwrap_or_default();
        let recent = git_items::newest(recent, usize::MAX);
        Ok(git_items::recent_first(
            searched,
            recent,
            query,
            MAX_GIT_ITEMS,
        ))
    }

    /// The repo's most recently updated pull requests and issues. A token
    /// that can't read issues, where the installation didn't grant them,
    /// still finds the pull requests.
    async fn recent(
        &self,
        repo: &RepoName,
        reader: &Reader,
    ) -> Result<Vec<FoundItem>, GithubError> {
        let (owner, name) = (repo.owner(), repo.name());
        let order = format!("state=all&sort=updated&direction=desc&per_page={RECENT}");
        let pulls = format!("/repos/{owner}/{name}/pulls?{order}");
        let issues = format!("/repos/{owner}/{name}/issues?{order}");
        let (pulls, issues) = tokio::join!(
            self.fetch(repo, reader, &pulls, Shape::Pulls, FRESH),
            self.fetch(repo, reader, &issues, Shape::Issues, FRESH),
        );
        let mut found = pulls?;
        match issues {
            Ok(issues) => found.extend(issues),
            Err(err) if out_of_reach(&err) => {}
            Err(err) => return Err(err),
        }
        Ok(found)
    }

    /// Pull request or issue `number`, or none when the repo has no such
    /// item the token can read.
    async fn numbered(
        &self,
        repo: &RepoName,
        reader: &Reader,
        number: u32,
    ) -> Result<Vec<FoundItem>, GithubError> {
        let (owner, name) = (repo.owner(), repo.name());
        let pull = format!("/repos/{owner}/{name}/pulls/{number}");
        let issue = format!("/repos/{owner}/{name}/issues/{number}");
        match self
            .fetch(repo, reader, &issue, Shape::OneIssue, FRESH)
            .await
        {
            Ok(found) => match found.first() {
                // A pull request's own reply names its branches.
                Some(item) if item.kind == GitItemKind::PullRequest => {}
                _ => return Ok(found),
            },
            // A token without issues can still read pull requests.
            Err(GithubError::Refused { status: 403, .. }) => {}
            Err(err) if out_of_reach(&err) => return Ok(Vec::new()),
            Err(err) => return Err(err),
        }
        match self.fetch(repo, reader, &pull, Shape::OnePull, FRESH).await {
            Err(err) if out_of_reach(&err) => Ok(Vec::new()),
            found => found,
        }
    }

    /// What GitHub's search finds for `query` in the repo, or none while
    /// the minute's searches are spent.
    async fn searched(
        &self,
        repo: &RepoName,
        reader: &Reader,
        query: &str,
    ) -> Result<Vec<FoundItem>, GithubError> {
        let query: String = query.chars().take(MAX_SEARCH).collect();
        let q = encoded(&format!("{query} repo:{repo}"));
        let path = format!("/search/issues?q={q}&sort=updated&order=desc&per_page={SEARCHED}");
        if let Some(kept) = self.peek(reader, &path) {
            return Ok(Shape::Search.read(&kept, &repo.to_string()));
        }
        {
            let now = self.now();
            let mut searches = lock(&self.inner.github.searches);
            while searches
                .front()
                .is_some_and(|sent| sent.after(Duration::from_secs(60)) <= now)
            {
                searches.pop_front();
            }
            if searches.len() >= SEARCHES_PER_MINUTE {
                return Ok(Vec::new());
            }
            searches.push_back(now);
        }
        self.fetch(repo, reader, &path, Shape::Search, SEARCH_FRESH)
            .await
    }

    /// The items `GET path` holds, read through the cache and kept for
    /// `fresh`.
    async fn fetch(
        &self,
        repo: &RepoName,
        reader: &Reader,
        path: &str,
        shape: Shape,
        fresh: Duration,
    ) -> Result<Vec<FoundItem>, GithubError> {
        let value = self.read(repo, reader, path, |_| fresh).await?;
        Ok(shape.read(&value, &repo.to_string()))
    }
}

/// A reply for `repo` with nothing in it, and why.
fn unavailable(repo: &RepoName, reason: String) -> GitItemSearch {
    GitItemSearch {
        repo: Some(repo.to_string()),
        items: Vec::new(),
        unavailable: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_follow_githubs_fields() {
        assert_eq!(pull_state("open", true, false), GitItemState::Draft);
        assert_eq!(pull_state("open", false, false), GitItemState::Open);
        assert_eq!(pull_state("closed", false, true), GitItemState::Merged);
        assert_eq!(pull_state("closed", true, false), GitItemState::Closed);
        assert_eq!(issue_state("open", None), GitItemState::Open);
        assert_eq!(
            issue_state("closed", Some("completed")),
            GitItemState::Completed
        );
        assert_eq!(
            issue_state("closed", Some("not_planned")),
            GitItemState::NotPlanned
        );
        assert_eq!(
            issue_state("closed", Some("reopened")),
            GitItemState::Closed
        );
    }

    #[test]
    fn a_query_is_encoded_for_the_url() {
        assert_eq!(
            encoded("login bug repo:owner/repo"),
            "login%20bug%20repo%3Aowner%2Frepo"
        );
        assert_eq!(encoded("ünï"), "%C3%BCn%C3%AF");
    }

    #[test]
    fn a_body_is_cut_and_its_line_ends_made_plain() {
        let body = format!("a\r\nb{}", "x".repeat(3_000));
        let cut = excerpt(Some(body));
        assert!(cut.starts_with("a\nb"));
        assert_eq!(cut.chars().count(), BODY_CHARS);
        assert_eq!(excerpt(None), "");
    }
}
