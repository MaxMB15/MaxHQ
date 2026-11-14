// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_protocol::pulls::{CheckState, Mergeable, ReviewState};
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::Secret;
use serde_json::json;

use super::*;
use crate::pulls::source::fake::FakePulls;
use crate::pulls::source::{
    CheckRun, CheckRuns, Comment, File, RepoSettings, Run, Runs, Status, Statuses,
};

fn repo() -> RepoName {
    RepoName::parse("owner/repo").unwrap()
}

fn you() -> Result<UserToken, GithubError> {
    Ok(UserToken {
        value: Secret::new("user-token".to_owned()),
        login: "you".to_owned(),
    })
}

fn pull(number: u64, author: &str, sha: &str) -> Pull {
    serde_json::from_value(json!({
        "number": number,
        "title": format!("PR {number}"),
        "state": "open",
        "user": {"login": author},
        "head": {"ref": "feature", "sha": sha, "repo": {"full_name": "owner/repo"}},
        "base": {"ref": "main", "sha": "base", "repo": {"full_name": "owner/repo"}},
        "body": "What it does",
        "html_url": format!("https://github.com/owner/repo/pull/{number}"),
        "updated_at": format!("2026-10-31T12:00:{number:02}Z"),
        "merged_at": null,
        "mergeable": true,
        "additions": 3,
        "deletions": 1,
    }))
    .unwrap()
}

fn review(user: &str, state: &str, at: &str) -> Review {
    serde_json::from_value(json!({
        "id": 7,
        "user": {"login": user},
        "state": state,
        "body": "",
        "commit_id": "abc",
        "submitted_at": at,
        "html_url": "https://github.com/owner/repo/pull/12#review",
    }))
    .unwrap()
}

fn fake_with(pulls: Vec<Pull>) -> FakePulls {
    let fake = FakePulls::default();
    fake.state().pulls.insert(repo().key(), pulls);
    fake
}

fn run(name: &str, status: &str, conclusion: Option<&str>) -> CheckRun {
    CheckRun {
        name: name.to_owned(),
        status: status.to_owned(),
        conclusion: conclusion.map(str::to_owned),
        html_url: None,
    }
}

#[tokio::test]
async fn with_no_sign_in_nothing_is_asked() {
    let fake = fake_with(vec![pull(12, "octocat", "abc")]);
    let outcome = act(
        &fake,
        Err(GithubError::NotSignedIn),
        &repo(),
        12,
        PullAction::Approve,
        "abc",
    )
    .await;
    let ActionOutcome::Refused { reason } = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert_eq!(reason, GithubError::NotSignedIn.to_string());
    assert!(fake.state().calls.is_empty());
}

#[tokio::test]
async fn a_merge_drops_the_cache_around_it_and_rereads_before_it_says_merged() {
    let fake = fake_with(vec![pull(12, "octocat", "abc")]);
    let merge = PullAction::Merge {
        method: MergeMethod::Squash,
    };
    let outcome = act(&fake, you(), &repo(), 12, merge, "abc").await;
    assert_eq!(outcome, ActionOutcome::PullMerged { queued: false });
    let state = fake.state();
    assert_eq!(
        state.calls,
        [
            "forget owner/repo#12",
            "merge owner/repo#12",
            "forget owner/repo#12",
            "pull owner/repo#12",
        ]
    );
    assert_eq!(state.merged[0].1, MergeMethod::Squash);
    assert_eq!(state.merged[0].2, "you");
}

#[tokio::test]
async fn a_merge_of_a_head_you_didnt_see_is_refused_in_words() {
    let fake = fake_with(vec![pull(12, "octocat", "new")]);
    let merge = PullAction::Merge {
        method: MergeMethod::Merge,
    };
    let outcome = act(&fake, you(), &repo(), 12, merge, "old").await;
    assert_eq!(outcome, refused_in(MOVED_MERGE));
    let state = fake.state();
    assert!(state.merged.is_empty());
    assert_eq!(state.calls.last().unwrap(), "forget owner/repo#12");
}

#[tokio::test]
async fn an_approval_rereads_and_stops_when_the_head_moved() {
    let fake = fake_with(vec![pull(12, "octocat", "new")]);
    let outcome = act(&fake, you(), &repo(), 12, PullAction::Approve, "old").await;
    assert_eq!(outcome, refused_in(MOVED_APPROVE));
    let state = fake.state();
    assert!(!state.calls.iter().any(|call| call.starts_with("approve")));
}

#[tokio::test]
async fn an_approval_goes_at_the_head_you_saw() {
    let fake = fake_with(vec![pull(12, "octocat", "abc")]);
    let outcome = act(&fake, you(), &repo(), 12, PullAction::Approve, "abc").await;
    assert_eq!(outcome, ActionOutcome::Done);
    let approved = fake.state().approved.clone();
    assert_eq!(approved, [((repo().key(), 12), "abc".into(), "you".into())]);
}

#[tokio::test]
async fn your_own_pr_and_a_closed_one_arent_approved() {
    let mut closed = pull(13, "octocat", "abc");
    closed.state = "closed".into();
    let fake = fake_with(vec![pull(12, "You", "abc"), closed]);
    let own = act(&fake, you(), &repo(), 12, PullAction::Approve, "abc").await;
    assert_eq!(own, refused_in(YOUR_OWN));
    let shut = act(&fake, you(), &repo(), 13, PullAction::Approve, "abc").await;
    assert_eq!(shut, refused_in(NOT_OPEN));
    assert!(fake.state().approved.is_empty());
}

#[tokio::test]
async fn checks_and_statuses_add_up() {
    let fake = FakePulls::default();
    let pull = pull(12, "octocat", "abc");
    fake.state().checks.insert(
        "abc".into(),
        CheckRuns {
            check_runs: vec![
                run("build", "completed", Some("success")),
                run("lint", "completed", Some("skipped")),
                run("test", "in_progress", None),
            ],
        },
    );
    let (state, checks) = shown_checks(&fake, &repo(), &pull).await;
    assert_eq!(state, ChecksState::Pending);
    let states: Vec<CheckState> = checks.iter().map(|check| check.state).collect();
    assert_eq!(
        states,
        [
            CheckState::Passing,
            CheckState::Skipped,
            CheckState::Pending
        ]
    );

    fake.state().statuses.insert(
        "abc".into(),
        Statuses {
            state: "failure".into(),
            statuses: vec![Status {
                context: "ci/deploy".into(),
                state: "error".into(),
                target_url: None,
            }],
        },
    );
    let (state, checks) = shown_checks(&fake, &repo(), &pull).await;
    assert_eq!(state, ChecksState::Failing);
    assert_eq!(checks.last().unwrap().name, "ci/deploy");
    // The PR comes from the repo itself, so workflow runs aren't read.
    assert!(!fake
        .state()
        .calls
        .iter()
        .any(|call| call.starts_with("runs")));
}

#[tokio::test]
async fn a_forks_workflow_runs_count_too() {
    let fake = FakePulls::default();
    let mut pull = pull(12, "octocat", "abc");
    pull.head.repo = Some(crate::pulls::source::Repo {
        full_name: "someone/repo".into(),
    });
    fake.state().runs.insert(
        "abc".into(),
        Runs {
            workflow_runs: vec![Run {
                name: Some("CI".into()),
                status: Some("completed".into()),
                conclusion: Some("failure".into()),
                html_url: "https://github.com/owner/repo/actions/runs/1".into(),
            }],
        },
    );
    let (state, checks) = shown_checks(&fake, &repo(), &pull).await;
    assert_eq!(state, ChecksState::Failing);
    assert_eq!(checks[0].name, "CI");
}

#[tokio::test]
async fn checks_the_app_cant_read_are_unknown_and_none_are_none() {
    let fake = FakePulls::default();
    let pull = pull(12, "octocat", "abc");
    let (state, checks) = shown_checks(&fake, &repo(), &pull).await;
    assert_eq!((state, checks.len()), (ChecksState::None, 0));
    fake.state().failing = Some(GithubError::Refused {
        status: 403,
        message: "Resource not accessible by integration".into(),
    });
    let (state, _) = shown_checks(&fake, &repo(), &pull).await;
    assert_eq!(state, ChecksState::Unknown);
}

#[tokio::test]
async fn the_open_list_has_checks_reviews_and_whats_linked() {
    let fake = fake_with(vec![pull(12, "octocat", "abc"), pull(13, "octocat", "def")]);
    fake.state().checks.insert(
        "abc".into(),
        CheckRuns {
            check_runs: vec![run("build", "completed", Some("success"))],
        },
    );
    fake.state().reviews.insert(
        (repo().key(), 12),
        vec![review("a", "APPROVED", "2026-10-31T12:00:00Z")],
    );
    let agent = AgentId::derive("agent");
    let mut seen = read::snapshot(
        &pull(12, "octocat", "abc"),
        ChecksState::None,
        ReviewState::None,
        None,
    );
    seen.mergeable = Mergeable::Conflicting;
    let link = PullLink {
        key: PullKey {
            repo: "Owner/Repo".into(),
            number: 12,
        },
        source: LinkSource::Branch,
        at: Timestamp(1),
        snapshot: Some(seen),
        watch: None,
    };
    let rows = list(&fake, &repo(), PullListState::Open, &[(agent, link)])
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key.number, 12);
    assert_eq!(rows[0].agents, [agent]);
    assert_eq!(rows[0].pull.checks, ChecksState::Passing);
    assert_eq!(rows[0].pull.review, ReviewState::Approved);
    assert_eq!(rows[0].pull.mergeable, Mergeable::Conflicting);
    assert_eq!(rows[1].pull.checks, ChecksState::None);
    assert!(rows[1].agents.is_empty());
}

#[tokio::test]
async fn the_closed_list_is_what_the_watcher_saw_close() {
    let fake = FakePulls::default();
    let mut merged = read::snapshot(
        &pull(9, "octocat", "abc"),
        ChecksState::None,
        ReviewState::None,
        None,
    );
    merged.state = PullState::Merged;
    let open = read::snapshot(
        &pull(10, "octocat", "def"),
        ChecksState::None,
        ReviewState::None,
        None,
    );
    let link = |number, snapshot| PullLink {
        key: PullKey {
            repo: "owner/repo".into(),
            number,
        },
        source: LinkSource::Created,
        at: Timestamp(1),
        snapshot: Some(snapshot),
        watch: None,
    };
    let agent = AgentId::derive("agent");
    let links = [(agent, link(9, merged)), (agent, link(10, open))];
    let rows = list(&fake, &repo(), PullListState::Closed, &links)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key.number, 9);
    assert!(fake.state().calls.is_empty());
}

#[tokio::test]
async fn the_viewer_reads_the_pull_its_checks_and_reviews() {
    let fake = fake_with(vec![pull(12, "octocat", "abc")]);
    fake.state().reviews.insert(
        (repo().key(), 12),
        vec![
            review("a", "PENDING", "2026-10-31T12:00:00Z"),
            review("b", "APPROVED", "2026-10-31T12:00:00Z"),
        ],
    );
    let detail = detail(&fake, &repo(), 12, &[]).await.unwrap();
    assert_eq!(detail.body.as_deref(), Some("What it does"));
    assert_eq!(detail.summary.pull.mergeable, Mergeable::Yes);
    assert_eq!(detail.summary.pull.review, ReviewState::Approved);
    assert_eq!(detail.reviews.len(), 1);
    assert_eq!(detail.reviews[0].commit_sha, "abc");
    assert_eq!(detail.merge_methods.len(), 3);
}

#[tokio::test]
async fn the_viewer_offers_only_the_merges_the_repo_allows() {
    let fake = fake_with(vec![pull(12, "octocat", "abc")]);
    let settings = RepoSettings {
        allow_merge_commit: false,
        allow_rebase_merge: false,
        ..RepoSettings::default()
    };
    fake.state().settings.insert(repo().key(), settings.clone());
    let squash = detail(&fake, &repo(), 12, &[]).await.unwrap();
    assert_eq!(squash.merge_methods, [MergeMethod::Squash]);

    // A repo that allows none offers all, so GitHub's refusal says why.
    let none = RepoSettings {
        allow_squash_merge: false,
        ..settings
    };
    fake.state().settings.insert(repo().key(), none);
    let all = detail(&fake, &repo(), 12, &[]).await.unwrap();
    assert_eq!(all.merge_methods.len(), 3);
}

#[tokio::test]
async fn the_branch_list_says_the_default_branch_where_github_does() {
    let fake = FakePulls::default();
    let names = vec!["dev".to_owned(), "main".to_owned()];
    fake.state().branches.insert(repo().key(), names.clone());
    let listed = |default_branch: Option<&str>| ActionOutcome::Branches {
        branches: names.clone(),
        default_branch: default_branch.map(str::to_owned),
    };
    assert_eq!(branches(&fake, &repo()).await.unwrap(), listed(None));

    let settings = RepoSettings {
        default_branch: Some("main".to_owned()),
        ..RepoSettings::default()
    };
    fake.state().settings.insert(repo().key(), settings);
    assert_eq!(
        branches(&fake, &repo()).await.unwrap(),
        listed(Some("main"))
    );

    fake.state().failing = Some(GithubError::BadReply);
    assert_eq!(branches(&fake, &repo()).await, Err(GithubError::BadReply));
}

#[tokio::test]
async fn files_and_comments_read_as_the_viewer_shows_them() {
    let fake = FakePulls::default();
    let file = |name: &str, status: &str| File {
        filename: name.into(),
        status: status.into(),
        additions: 1,
        deletions: 0,
        patch: None,
    };
    fake.state().files.insert(
        (repo().key(), 12),
        vec![
            file("a.rs", "added"),
            file("b.rs", "renamed"),
            file("c.rs", "modified"),
        ],
    );
    let (files, more) = files(&fake, &repo(), 12, 1).await.unwrap();
    assert!(more);
    assert_eq!(files[1].status, PullFileStatus::Renamed);
    assert_eq!(files[0].path, "a.rs");

    fake.state().comments.insert(
        (repo().key(), 12),
        vec![Comment {
            id: 1,
            user: None,
            author_association: None,
            body: Some("Later".into()),
            created_at: "2026-10-31T12:05:00Z".into(),
            html_url: "https://github.com/owner/repo/pull/12#c1".into(),
        }],
    );
    let mut said = review("a", "CHANGES_REQUESTED", "2026-10-31T12:01:00Z");
    said.body = Some("Fix the test".into());
    let silent = review("b", "APPROVED", "2026-10-31T12:02:00Z");
    fake.state()
        .reviews
        .insert((repo().key(), 12), vec![said, silent]);
    let comments = comments(&fake, &repo(), 12).await.unwrap();
    let bodies: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, ["Fix the test", "Later"]);
}
