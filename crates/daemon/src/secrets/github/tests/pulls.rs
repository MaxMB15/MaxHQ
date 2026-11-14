// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The PR reads, merge and approve against the fake GitHub.

use maxhq_protocol::pulls::{ChecksState, MergeMethod, PullAction};

use super::*;
use crate::pulls::requests::{act, branches, shown_checks, MOVED_MERGE};
use crate::pulls::source::PullSource;
use crate::secrets::github::fake::GhLogin;

const USER_TOKEN: &str = "ghu_test-not-a-real-user-token";

fn repo() -> RepoName {
    RepoName::parse("octo/maxhq").unwrap()
}

fn pull(number: u64, author: &str, sha: &str) -> Value {
    json!({
        "number": number,
        "title": "Sync faster",
        "state": "open",
        "draft": false,
        "user": { "login": author },
        "head": { "ref": "feat/sync", "sha": sha, "repo": { "full_name": "octo/maxhq" } },
        "base": { "ref": "main", "sha": "0000", "repo": { "full_name": "octo/maxhq" } },
        "body": "What it changes.",
        "html_url": format!("https://github.com/octo/maxhq/pull/{number}"),
        "updated_at": "2026-10-25T11:00:00Z",
        "merged_at": null,
        "mergeable": true,
    })
}

fn you() -> UserToken {
    UserToken {
        value: Secret::new(USER_TOKEN.to_owned()),
        login: "you".to_owned(),
    }
}

/// A rig whose App is set, with PR 12 open on `octo/maxhq` and a user
/// token for "you".
async fn rig() -> Rig {
    let rig = Rig::new().await;
    assert_eq!(rig.set(), ActionOutcome::Done);
    let mut state = rig.fake.state();
    state.pulls = vec![pull(12, "octocat", "abc123")];
    state.users.insert(USER_TOKEN.to_owned(), "you".to_owned());
    drop(state);
    rig
}

/// How many GETs went to `path`.
fn gets(rig: &Rig, path: &str) -> usize {
    let state = rig.fake.state();
    let sent = state.requests.iter();
    sent.filter(|request| request.method == "GET" && request.path == path)
        .count()
}

#[tokio::test]
async fn a_read_is_used_until_it_ages_then_asked_for_with_its_etag() {
    let rig = rig().await;
    let path = "/repos/octo/maxhq/pulls/12";
    let first = rig.secrets.pull(&repo(), 12).await.unwrap();
    assert_eq!(first.head.sha, "abc123");
    rig.secrets.pull(&repo(), 12).await.unwrap();
    assert_eq!(gets(&rig, path), 1);

    rig.clock.advance(Duration::from_secs(61));
    let again = rig.secrets.pull(&repo(), 12).await.unwrap();
    assert_eq!(again, first);
    assert_eq!(gets(&rig, path), 2);
    assert_eq!(rig.fake.state().unchanged, 1);

    // Forgetting the PR sends the next read to GitHub.
    PullSource::forget(&rig.secrets, &repo(), Some(12));
    rig.secrets.pull(&repo(), 12).await.unwrap();
    assert_eq!(gets(&rig, path), 3);
}

#[tokio::test]
async fn forgetting_one_pr_keeps_the_others() {
    let rig = rig().await;
    rig.fake.state().pulls.push(pull(13, "octocat", "def456"));
    rig.secrets.pull(&repo(), 12).await.unwrap();
    rig.secrets.pull(&repo(), 13).await.unwrap();
    rig.secrets.open_list(&repo()).await.unwrap();
    PullSource::forget(&rig.secrets, &repo(), Some(12));
    rig.secrets.pull(&repo(), 13).await.unwrap();
    rig.secrets.open_list(&repo()).await.unwrap();
    assert_eq!(gets(&rig, "/repos/octo/maxhq/pulls/13"), 1);
    assert_eq!(gets(&rig, "/repos/octo/maxhq/pulls"), 2);
}

#[tokio::test]
async fn the_rate_limit_comes_from_the_replies() {
    let rig = rig().await;
    let reset = at("2026-10-25T12:30:00Z").0 / 1000;
    rig.fake.state().rate = Some((4_321, 5_000, reset));
    rig.fake.state().poll_interval = Some(60);
    rig.secrets.open_list(&repo()).await.unwrap();
    let pacing = rig.secrets.pacing(&repo());
    assert_eq!(pacing.remaining, Some(4_321));
    assert_eq!(pacing.limit, Some(5_000));
    assert_eq!(pacing.poll_after, Some(Duration::from_secs(60)));
    assert_eq!(pacing.resets_in, Some(Duration::from_secs(30 * 60)));
}

#[tokio::test]
async fn the_merge_settings_read_once_for_ten_minutes() {
    let rig = rig().await;
    let path = "/repos/octo/maxhq";
    // Without push access GitHub leaves the settings out.
    let all = rig.secrets.settings(&repo()).await.unwrap();
    assert_eq!(all.methods().len(), 3);

    rig.fake.state().settings = json!({
        "allow_merge_commit": false,
        "allow_squash_merge": true,
        "allow_rebase_merge": false,
    });
    rig.clock.advance(Duration::from_secs(9 * 60));
    rig.secrets.settings(&repo()).await.unwrap();
    assert_eq!(gets(&rig, path), 1);
    rig.clock.advance(Duration::from_secs(2 * 60));
    let squash = rig.secrets.settings(&repo()).await.unwrap();
    assert_eq!(squash.methods(), [MergeMethod::Squash]);
    assert_eq!(gets(&rig, path), 2);
}

#[tokio::test]
async fn the_branches_read_one_page_for_a_minute_with_the_default_one() {
    let rig = rig().await;
    let path = "/repos/octo/maxhq/branches";
    rig.fake.state().branches = vec![
        json!({ "name": "feat/sync", "commit": { "sha": "abc123" }, "protected": false }),
        json!({ "name": "main", "commit": { "sha": "0000" }, "protected": true }),
    ];
    rig.fake.state().settings = json!({ "default_branch": "main" });
    let listed = branches(&rig.secrets, &repo()).await.unwrap();
    assert_eq!(
        listed,
        ActionOutcome::Branches {
            branches: vec!["feat/sync".to_owned(), "main".to_owned()],
            default_branch: Some("main".to_owned()),
        }
    );
    let query = {
        let state = rig.fake.state();
        let asked = state.requests.iter().find(|request| request.path == path);
        asked.unwrap().query.clone()
    };
    assert_eq!(query, "per_page=100");

    rig.clock.advance(Duration::from_secs(59));
    rig.secrets.branches(&repo()).await.unwrap();
    assert_eq!(gets(&rig, path), 1);
    rig.clock.advance(Duration::from_secs(2));
    rig.secrets.branches(&repo()).await.unwrap();
    assert_eq!(gets(&rig, path), 2);
}

#[tokio::test]
async fn checks_read_where_the_installation_grants_them() {
    let rig = Rig::new().await;
    rig.fake.state().installations[0].reads = true;
    assert_eq!(rig.set(), ActionOutcome::Done);
    rig.fake.state().checks.insert(
        "abc123".into(),
        json!({ "total_count": 1, "check_runs": [
            { "name": "build", "status": "completed", "conclusion": "success", "html_url": null },
        ]}),
    );
    let pull: crate::pulls::source::Pull =
        serde_json::from_value(pull(12, "octocat", "abc123")).unwrap();
    let (state, checks) = shown_checks(&rig.secrets, &repo(), &pull).await;
    assert_eq!(state, ChecksState::Passing);
    assert_eq!(checks.len(), 1);
    let (_, minted) = &rig.fake.repo_mints()[0];
    for read in ["checks", "statuses", "actions"] {
        assert_eq!(minted["permissions"][read], "read");
    }
}

#[tokio::test]
async fn checks_the_installation_didnt_grant_are_unknown() {
    let rig = rig().await;
    let pull: crate::pulls::source::Pull =
        serde_json::from_value(pull(12, "octocat", "abc123")).unwrap();
    let (state, _) = shown_checks(&rig.secrets, &repo(), &pull).await;
    assert_eq!(state, ChecksState::Unknown);
    let (_, minted) = &rig.fake.repo_mints()[0];
    assert!(minted["permissions"].get("checks").is_none());
}

#[tokio::test]
async fn a_merge_goes_as_you_at_the_head_you_saw() {
    let rig = rig().await;
    let merge = PullAction::Merge {
        method: MergeMethod::Squash,
    };
    // The fake takes a merge only with a user token, so an installation
    // token would be refused.
    let outcome = act(&rig.secrets, Ok(you()), &repo(), 12, merge, "abc123").await;
    assert_eq!(outcome, ActionOutcome::PullMerged { queued: false });
    let state = rig.fake.state();
    let sent = state
        .requests
        .iter()
        .find(|request| request.method == "PUT")
        .unwrap();
    assert_eq!(sent.path, "/repos/octo/maxhq/pulls/12/merge");
    assert_eq!(
        sent.body,
        json!({ "merge_method": "squash", "sha": "abc123" })
    );
}

#[tokio::test]
async fn a_merge_at_a_stale_head_fails_cleanly() {
    let rig = rig().await;
    let merge = PullAction::Merge {
        method: MergeMethod::Merge,
    };
    let outcome = act(&rig.secrets, Ok(you()), &repo(), 12, merge, "0ld0ld").await;
    assert_eq!(
        outcome,
        ActionOutcome::Refused {
            reason: MOVED_MERGE.to_owned()
        }
    );
    assert_eq!(rig.fake.state().pulls[0]["state"], "open");
}

#[tokio::test]
async fn a_merge_a_queue_took_is_queued() {
    let rig = rig().await;
    rig.fake.state().queues_merges = true;
    // A read from before the merge isn't what the reread finds.
    rig.secrets.pull(&repo(), 12).await.unwrap();
    let merge = PullAction::Merge {
        method: MergeMethod::Merge,
    };
    let outcome = act(&rig.secrets, Ok(you()), &repo(), 12, merge, "abc123").await;
    assert_eq!(outcome, ActionOutcome::PullMerged { queued: true });
    assert_eq!(gets(&rig, "/repos/octo/maxhq/pulls/12"), 2);
}

#[tokio::test]
async fn an_approval_names_the_commit() {
    let rig = rig().await;
    let outcome = act(
        &rig.secrets,
        Ok(you()),
        &repo(),
        12,
        PullAction::Approve,
        "abc123",
    )
    .await;
    assert_eq!(outcome, ActionOutcome::Done);
    let state = rig.fake.state();
    let sent = state
        .requests
        .iter()
        .find(|request| request.method == "POST" && request.path.ends_with("/reviews"))
        .unwrap();
    assert_eq!(
        sent.body,
        json!({ "event": "APPROVE", "commit_id": "abc123" })
    );
}

#[tokio::test]
async fn an_approval_after_the_head_moved_sends_nothing() {
    let rig = rig().await;
    let outcome = act(
        &rig.secrets,
        Ok(you()),
        &repo(),
        12,
        PullAction::Approve,
        "0ld0ld",
    )
    .await;
    assert!(matches!(outcome, ActionOutcome::Refused { .. }));
    let state = rig.fake.state();
    assert!(!state
        .requests
        .iter()
        .any(|request| request.path.ends_with("/reviews")));
}

#[tokio::test]
async fn no_write_access_is_said_plainly_and_no_token_shows() {
    let rig = rig().await;
    rig.fake.state().users_write = false;
    let approve = act(
        &rig.secrets,
        Ok(you()),
        &repo(),
        12,
        PullAction::Approve,
        "abc123",
    )
    .await;
    let merge = PullAction::Merge {
        method: MergeMethod::Rebase,
    };
    let merge = act(&rig.secrets, Ok(you()), &repo(), 12, merge, "abc123").await;
    let reasons: Vec<String> = [approve, merge]
        .into_iter()
        .map(|outcome| match outcome {
            ActionOutcome::Refused { reason } => reason,
            other => panic!("expected a refusal, got {other:?}"),
        })
        .collect();
    assert_eq!(
        reasons,
        [
            "You need write access on this repository to approve.",
            "You need write access on this repository to merge.",
        ]
    );
    let stranger = UserToken {
        value: Secret::new("ghu_test-unknown".to_owned()),
        login: "you".to_owned(),
    };
    let unauthorized = rig
        .secrets
        .merge(&stranger, &repo(), 12, MergeMethod::Merge, "abc123")
        .await;
    let said = unauthorized.unwrap_err().to_string();
    assert!(
        !said.contains("ghu_test") && !said.contains(TOKEN_PREFIX),
        "{said}"
    );
}

#[tokio::test]
async fn with_no_app_prs_read_through_gh_and_with_neither_say_so() {
    let rig = Rig::new().await;
    rig.fake.state().pulls = vec![pull(12, "octocat", "abc123")];
    let err = rig.secrets.pull(&repo(), 12).await.unwrap_err();
    assert!(matches!(err, GithubError::NoApp), "{err}");

    rig.fake.state().gh = GhLogin::SignedIn(vec!["octo/maxhq".to_owned()]);
    let read = rig.secrets.pull(&repo(), 12).await.unwrap();
    assert_eq!(read.number, 12);
}
