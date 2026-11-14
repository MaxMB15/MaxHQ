// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! GitHub's REST shapes, read into what MaxHQ keeps and shows of a PR: its
//! snapshot, its checks, its reviews and what was said on it. The watcher
//! and the app's requests both read PRs through here.

use maxhq_protocol::pulls::{
    CheckState, ChecksState, Mergeable, PullCheck, PullComment, PullSnapshot, PullState,
    ReviewState,
};
use maxhq_protocol::time::Timestamp;

use super::source::{CheckRuns, Comment, Pull, PullSource, Review, Runs, Statuses, User};
use crate::observe::claude::time::parse_iso8601;
use crate::secrets::{GithubError, RepoName};

/// Whether a PR is open, merged or closed without merging.
pub fn state(pull: &Pull) -> PullState {
    if pull.merged_at.is_some() {
        PullState::Merged
    } else if pull.state == "open" {
        PullState::Open
    } else {
        PullState::Closed
    }
}

/// A PR's snapshot, with its checks and review already worked out. GitHub
/// says nothing about merging while it works that out, so until it does
/// the last answer for the same commit stands.
pub fn snapshot(
    pull: &Pull,
    checks: ChecksState,
    review: ReviewState,
    before: Option<&PullSnapshot>,
) -> PullSnapshot {
    let mergeable = match (pull.mergeable, pull.mergeable_state.as_deref()) {
        (_, Some("dirty")) | (Some(false), _) => Mergeable::Conflicting,
        (Some(true), _) => Mergeable::Yes,
        (None, _) => before
            .filter(|before| before.head_sha == pull.head.sha)
            .map_or(Mergeable::Unknown, |before| before.mergeable),
    };
    PullSnapshot {
        title: pull.title.clone(),
        state: state(pull),
        draft: pull.draft,
        author: author(pull.user.as_ref()),
        head: pull.head.branch.clone(),
        base: pull.base.branch.clone(),
        head_sha: pull.head.sha.clone(),
        checks,
        mergeable,
        review,
        additions: pull.additions,
        deletions: pull.deletions,
        updated_at: parse_iso8601(&pull.updated_at).unwrap_or(Timestamp(0)),
        merged_at: pull.merged_at.as_deref().and_then(parse_iso8601),
    }
}

/// Whether a PR comes from another repository than the one it goes into,
/// whose checks show up as workflow runs.
pub fn from_fork(pull: &Pull) -> bool {
    let head = pull.head.repo.as_ref().map(|repo| &repo.full_name);
    let base = pull.base.repo.as_ref().map(|repo| &repo.full_name);
    head.is_none_or(|head| base.is_none_or(|base| !head.eq_ignore_ascii_case(base)))
}

/// The checks on a PR's head, read at once: its check runs and statuses
/// and, for a fork's PR, its workflow runs. None when the App may not read
/// them, and none of a kind the server doesn't have.
pub async fn head_checks(
    source: &dyn PullSource,
    repo: &RepoName,
    pull: &Pull,
) -> Result<Option<Vec<PullCheck>>, GithubError> {
    let sha = pull.head.sha.as_str();
    let (runs, statuses, workflows) = tokio::join!(
        source.checks(repo, sha),
        source.statuses(repo, sha),
        async {
            match from_fork(pull) {
                true => Some(source.runs(repo, sha).await),
                false => None,
            }
        },
    );
    let (Some(runs), Some(statuses)) = (readable(runs)?, readable(statuses)?) else {
        return Ok(None);
    };
    let workflows = workflows.map(readable).transpose()?.flatten();
    Ok(Some(checks(&runs, &statuses, workflows.as_ref())))
}

/// What was read: nothing where the server doesn't have it, which
/// a 404, 405 or 501 says, and none where the App may not read it.
fn readable<T: Default>(read: Result<T, GithubError>) -> Result<Option<T>, GithubError> {
    match read {
        Ok(read) => Ok(Some(read)),
        Err(GithubError::Refused { status: 403, .. }) => Ok(None),
        Err(GithubError::Refused {
            status: 404 | 405 | 501,
            ..
        }) => Ok(Some(T::default())),
        Err(err) => Err(err),
    }
}

/// A commit's checks: its check runs, its statuses and, for a fork's PR,
/// its workflow runs. A workflow run named like a check already there is
/// that check, so it isn't counted twice. Whoever sets a check picks its
/// link, and the page opens it, so only an https one stays.
pub fn checks(runs: &CheckRuns, statuses: &Statuses, workflows: Option<&Runs>) -> Vec<PullCheck> {
    let https = |url: &String| maxhq_core::git::is_https(url);
    let mut checks: Vec<PullCheck> = runs
        .check_runs
        .iter()
        .map(|run| PullCheck {
            name: run.name.clone(),
            state: run_state(Some(&run.status), run.conclusion.as_deref()),
            url: run.html_url.clone().filter(https),
        })
        .collect();
    checks.extend(statuses.statuses.iter().map(|status| PullCheck {
        name: status.context.clone(),
        state: match status.state.as_str() {
            "success" => CheckState::Passing,
            "failure" | "error" => CheckState::Failing,
            _ => CheckState::Pending,
        },
        url: status.target_url.clone().filter(https),
    }));
    for run in workflows.into_iter().flat_map(|runs| &runs.workflow_runs) {
        let name = run.name.as_deref().unwrap_or("workflow");
        if checks.iter().any(|check| check.name == name) {
            continue;
        }
        checks.push(PullCheck {
            name: name.to_owned(),
            state: run_state(run.status.as_deref(), run.conclusion.as_deref()),
            url: Some(run.html_url.clone()).filter(https),
        });
    }
    checks
}

/// How a check run or workflow run stands from its status and conclusion.
fn run_state(status: Option<&str>, conclusion: Option<&str>) -> CheckState {
    if status != Some("completed") {
        return CheckState::Pending;
    }
    match conclusion {
        Some("success") => CheckState::Passing,
        Some("neutral" | "skipped" | "stale") => CheckState::Skipped,
        None => CheckState::Pending,
        Some(_) => CheckState::Failing,
    }
}

/// All of a PR's checks together: failing when any fails, pending while
/// any runs, passing once every one has passed or was skipped.
pub fn checks_state(checks: &[PullCheck]) -> ChecksState {
    let any = |state: CheckState| checks.iter().any(|check| check.state == state);
    if checks.is_empty() {
        ChecksState::None
    } else if any(CheckState::Failing) {
        ChecksState::Failing
    } else if any(CheckState::Pending) {
        ChecksState::Pending
    } else {
        ChecksState::Passing
    }
}

/// Where a PR's reviews stand: each reviewer's last approval or request
/// for changes counts, and one request for changes outweighs approvals.
pub fn review_state(reviews: &[Review]) -> ReviewState {
    let mut last: Vec<(&str, &str)> = Vec::new();
    for review in reviews {
        let state = review.state.as_str();
        if !matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
            continue;
        }
        let login = review.user.as_ref().map_or("", |user| user.login.as_str());
        match last.iter_mut().find(|(seen, _)| *seen == login) {
            Some(entry) => entry.1 = state,
            None => last.push((login, state)),
        }
    }
    let any = |state: &str| last.iter().any(|(_, seen)| *seen == state);
    if any("CHANGES_REQUESTED") {
        ReviewState::ChangesRequested
    } else if any("APPROVED") {
        ReviewState::Approved
    } else {
        ReviewState::None
    }
}

/// A PR's conversation as the viewer shows it: its comments and the
/// reviews that say something, oldest first.
pub fn conversation(reviews: &[Review], comments: &[Comment]) -> Vec<PullComment> {
    said(reviews, comments, false)
}

/// The remarks a watch tells its agent of: the conversation, with an
/// approval or a request for changes that says nothing put in words.
pub fn remarks(reviews: &[Review], comments: &[Comment]) -> Vec<PullComment> {
    said(reviews, comments, true)
}

fn said(reviews: &[Review], comments: &[Comment], verdicts: bool) -> Vec<PullComment> {
    let reviews = reviews.iter().filter_map(|review| {
        let body = review.body.clone().filter(|body| !body.trim().is_empty());
        let body = match review.state.as_str() {
            "APPROVED" if verdicts => body.unwrap_or_else(|| "Approved.".to_owned()),
            "CHANGES_REQUESTED" if verdicts => {
                body.unwrap_or_else(|| "Changes requested.".to_owned())
            }
            _ => body?,
        };
        Some(PullComment {
            id: review.id,
            author: author(review.user.as_ref()),
            body,
            at: review.submitted_at.as_deref().and_then(parse_iso8601)?,
            path: None,
            line: None,
            url: review.html_url.clone(),
        })
    });
    let comments = comments.iter().filter_map(|comment| {
        Some(PullComment {
            id: comment.id,
            author: author(comment.user.as_ref()),
            body: comment.body.clone().unwrap_or_default(),
            at: parse_iso8601(&comment.created_at)?,
            path: None,
            line: None,
            url: comment.html_url.clone(),
        })
    });
    let mut said: Vec<PullComment> = reviews.chain(comments).collect();
    said.sort_by_key(|said| said.at);
    said
}

/// The logins with a role on the repository, or that are bot accounts, among
/// those who wrote `reviews` and `comments`. No role or no word of one leaves a
/// login out.
pub fn known(reviews: &[Review], comments: &[Comment]) -> Vec<String> {
    let reviews = reviews
        .iter()
        .map(|review| (review.user.as_ref(), review.author_association.as_deref()));
    let comments = comments
        .iter()
        .map(|comment| (comment.user.as_ref(), comment.author_association.as_deref()));
    reviews
        .chain(comments)
        .filter_map(|(user, role)| {
            let user = user?;
            (matches!(role, Some("OWNER" | "MEMBER" | "COLLABORATOR"))
                || user.kind.as_deref() == Some("Bot"))
            .then(|| user.login.clone())
        })
        .collect()
}

/// Who an account is, as GitHub names a deleted one.
pub fn author(user: Option<&User>) -> String {
    user.map_or_else(|| "ghost".to_owned(), |user| user.login.clone())
}
#[cfg(test)]
mod tests {
    use super::super::source::{Branch, CheckRun, Repo, Run, Status};
    use super::*;

    fn pull() -> Pull {
        let branch = |name: &str, sha: &str| Branch {
            branch: name.to_owned(),
            sha: sha.to_owned(),
            repo: Some(Repo {
                full_name: "owner/repo".to_owned(),
            }),
        };
        Pull {
            number: 12,
            title: "Fix the build".to_owned(),
            state: "open".to_owned(),
            user: Some(User {
                login: "octocat".to_owned(),
                ..User::default()
            }),
            head: branch("fix-build", "abc"),
            base: branch("main", "def"),
            updated_at: "2026-10-01T12:00:00Z".to_owned(),
            mergeable: Some(true),
            ..Pull::default()
        }
    }

    #[test]
    fn a_pr_reads_into_its_snapshot() {
        let open = snapshot(&pull(), ChecksState::Pending, ReviewState::None, None);
        assert_eq!(open.state, PullState::Open);
        assert_eq!(open.mergeable, Mergeable::Yes);
        assert_eq!(
            open.updated_at,
            parse_iso8601("2026-10-01T12:00:00Z").unwrap()
        );
        assert!(!from_fork(&pull()));

        let working = Pull {
            mergeable: None,
            ..pull()
        };
        let kept = snapshot(
            &working,
            ChecksState::Pending,
            ReviewState::None,
            Some(&open),
        );
        assert_eq!(kept.mergeable, Mergeable::Yes, "same commit keeps it");
        let moved = Pull {
            head: Branch {
                sha: "new".to_owned(),
                ..working.head.clone()
            },
            ..working
        };
        let unknown = snapshot(&moved, ChecksState::Pending, ReviewState::None, Some(&open));
        assert_eq!(unknown.mergeable, Mergeable::Unknown);

        let merged = Pull {
            state: "closed".to_owned(),
            merged_at: Some("2026-10-01T13:00:00Z".to_owned()),
            ..pull()
        };
        assert_eq!(state(&merged), PullState::Merged);
        let closed = Pull {
            state: "closed".to_owned(),
            ..pull()
        };
        assert_eq!(state(&closed), PullState::Closed);
    }

    #[test]
    fn checks_add_up() {
        let run = |name: &str, status: &str, conclusion: Option<&str>| CheckRun {
            name: name.to_owned(),
            status: status.to_owned(),
            conclusion: conclusion.map(str::to_owned),
            html_url: None,
        };
        let runs = CheckRuns {
            check_runs: vec![
                run("lint", "completed", Some("success")),
                run("docs", "completed", Some("skipped")),
            ],
        };
        let none = Statuses::default();
        assert_eq!(
            checks_state(&checks(&runs, &none, None)),
            ChecksState::Passing
        );
        let statuses = Statuses {
            state: "failure".to_owned(),
            statuses: vec![Status {
                context: "ci/test".to_owned(),
                state: "failure".to_owned(),
                target_url: None,
            }],
        };
        let all = checks(&runs, &statuses, None);
        assert_eq!(checks_state(&all), ChecksState::Failing);
        assert_eq!(all[2].name, "ci/test");
        let running = CheckRuns {
            check_runs: vec![run("test", "in_progress", None)],
        };
        assert_eq!(
            checks_state(&checks(&running, &none, None)),
            ChecksState::Pending
        );
        assert_eq!(checks_state(&[]), ChecksState::None);
    }

    #[test]
    fn a_login_counts_as_known_by_its_role_or_as_a_bot() {
        let user = |login: &str, kind: Option<&str>| {
            Some(User {
                login: login.to_owned(),
                kind: kind.map(str::to_owned),
            })
        };
        let comment = |login: &str, role: Option<&str>, kind: Option<&str>| Comment {
            user: user(login, kind),
            author_association: role.map(str::to_owned),
            ..Comment::default()
        };
        let comments = [
            comment("owner", Some("OWNER"), Some("User")),
            comment("member", Some("MEMBER"), None),
            comment("collaborator", Some("COLLABORATOR"), None),
            comment("bot", None, Some("Bot")),
            comment("contributor", Some("CONTRIBUTOR"), Some("User")),
            comment("first-timer", Some("FIRST_TIME_CONTRIBUTOR"), Some("User")),
            comment("stranger", Some("NONE"), Some("User")),
            comment("old-answer", None, None),
            Comment {
                author_association: Some("OWNER".to_owned()),
                ..Comment::default()
            },
        ];
        let reviews = [Review {
            user: user("reviewer", Some("User")),
            author_association: Some("COLLABORATOR".to_owned()),
            ..Review::default()
        }];
        assert_eq!(
            known(&reviews, &comments),
            ["reviewer", "owner", "member", "collaborator", "bot"]
        );
        // GitHub's answer spells them this way.
        let parsed: Comment = serde_json::from_str(
            r#"{"id":1,"user":{"login":"a","type":"Bot"},"author_association":"NONE","created_at":"","html_url":""}"#,
        )
        .unwrap();
        assert_eq!(parsed.user.unwrap().kind.as_deref(), Some("Bot"));
        assert_eq!(parsed.author_association.as_deref(), Some("NONE"));
    }

    #[test]
    fn a_check_keeps_only_an_https_link() {
        let app = "vscode://anysphere.cursor-deeplink/mcp/install?name=x".to_owned();
        let page = "https://github.com/owner/repo/actions/runs/1".to_owned();
        let run = |name: &str, url: &str| CheckRun {
            name: name.to_owned(),
            html_url: Some(url.to_owned()),
            ..CheckRun::default()
        };
        let runs = CheckRuns {
            check_runs: vec![run("planted", &app), run("kept", &page)],
        };
        let statuses = Statuses {
            statuses: vec![Status {
                context: "status".to_owned(),
                target_url: Some(app.clone()),
                ..Status::default()
            }],
            ..Statuses::default()
        };
        let workflows = Runs {
            workflow_runs: vec![Run {
                name: Some("workflow".to_owned()),
                html_url: app.clone(),
                ..Run::default()
            }],
        };
        let all = checks(&runs, &statuses, Some(&workflows));
        let links: Vec<_> = all
            .iter()
            .map(|check| (check.name.as_str(), check.url.as_deref()))
            .collect();
        assert_eq!(
            links,
            [
                ("planted", None),
                ("kept", Some(page.as_str())),
                ("status", None),
                ("workflow", None),
            ]
        );
    }

    #[test]
    fn a_reviewers_last_word_counts() {
        let left = |id, login: &str, state: &str| Review {
            id,
            user: Some(User {
                login: login.to_owned(),
                ..User::default()
            }),
            state: state.to_owned(),
            submitted_at: Some("2026-10-01T12:00:00Z".to_owned()),
            ..Review::default()
        };
        let changed_mind = [
            left(1, "hubot", "CHANGES_REQUESTED"),
            left(2, "hubot", "APPROVED"),
        ];
        assert_eq!(review_state(&changed_mind), ReviewState::Approved);
        let blocked = [
            left(1, "hubot", "APPROVED"),
            left(2, "monalisa", "CHANGES_REQUESTED"),
        ];
        assert_eq!(review_state(&blocked), ReviewState::ChangesRequested);

        let said = remarks(&[left(3, "hubot", "COMMENTED")], &[]);
        assert!(
            said.is_empty(),
            "a review with nothing to say isn't a remark"
        );
        let said = remarks(&changed_mind, &[]);
        assert_eq!(said[1].body, "Approved.");
        assert!(
            conversation(&changed_mind, &[]).is_empty(),
            "the viewer shows a verdict with nothing said among the reviews"
        );
    }
}
