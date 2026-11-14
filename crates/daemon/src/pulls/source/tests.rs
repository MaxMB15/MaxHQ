// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use maxhq_protocol::pulls::MergeMethod;
use maxhq_protocol::Secret;
use serde_json::json;

use super::fake::FakePulls;
use super::*;

fn repo() -> RepoName {
    RepoName::parse("owner/repo").unwrap()
}

fn octocat() -> UserToken {
    UserToken {
        value: Secret::new("token".to_owned()),
        login: "octocat".to_owned(),
    }
}

/// A PR as GitHub's list gives it, without `mergeable` or line counts.
fn listed() -> Pull {
    serde_json::from_value(json!({
        "number": 12,
        "title": "Add a thing",
        "state": "open",
        "draft": false,
        "user": {"login": "octocat"},
        "head": {"ref": "feature", "sha": "abc", "repo": {"full_name": "owner/repo"}},
        "base": {"ref": "main", "sha": "def", "repo": {"full_name": "owner/repo"}},
        "body": null,
        "html_url": "https://github.com/owner/repo/pull/12",
        "updated_at": "2026-10-31T12:00:00Z",
        "merged_at": null,
    }))
    .unwrap()
}

#[test]
fn a_listed_pull_reads_without_what_only_its_own_page_has() {
    let pull = listed();
    assert_eq!(pull.head.branch, "feature");
    assert_eq!(pull.mergeable, None);
    assert_eq!(pull.additions, 0);
}

#[tokio::test]
async fn the_fake_merges_only_at_the_head_it_was_shown() {
    let fake = FakePulls::default();
    fake.state().pulls.insert(repo().key(), vec![listed()]);
    let user = octocat();

    let stale = fake
        .merge(&user, &repo(), 12, MergeMethod::Squash, "old")
        .await;
    assert!(matches!(
        stale,
        Err(GithubError::Refused { status: 409, .. })
    ));
    fake.merge(&user, &repo(), 12, MergeMethod::Squash, "abc")
        .await
        .unwrap();

    assert!(fake.open_list(&repo()).await.unwrap().is_empty());
    let merged = fake.pull(&repo(), 12).await.unwrap();
    assert!(merged.merged_at.is_some());
    let state = fake.state();
    assert_eq!(
        state.merged,
        vec![(
            (repo().key(), 12),
            MergeMethod::Squash,
            "octocat".to_owned()
        )]
    );
    assert_eq!(state.calls[0], "merge owner/repo#12");
}

#[tokio::test]
async fn the_fake_pages_files_and_fails_when_told() {
    let fake = FakePulls::default();
    let file = |name: &str| File {
        filename: name.to_owned(),
        status: "modified".to_owned(),
        ..File::default()
    };
    fake.state()
        .files
        .insert((repo().key(), 12), vec![file("a"), file("b"), file("c")]);
    let (first, more) = fake.files(&repo(), 12, 1).await.unwrap();
    assert_eq!((first.len(), more), (2, true));
    let (last, more) = fake.files(&repo(), 12, 2).await.unwrap();
    assert_eq!((last.len(), more), (1, false));

    fake.state().failing = Some(GithubError::NoApp);
    assert_eq!(fake.pull(&repo(), 12).await, Err(GithubError::NoApp));
}
