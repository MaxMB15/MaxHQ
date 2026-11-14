// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `#` lookup against the fake GitHub.

use maxhq_protocol::context::{GitItemKind, GitItemState, InlineRef};
use maxhq_protocol::find::GitItemSearch;

use super::*;
use crate::secrets::github::fake::GhLogin;
use crate::secrets::github::items::SEARCHES_PER_MINUTE;

const REPO: &str = "octo/maxhq";

fn pull(number: u32, title: &str, updated: &str, state: &str, draft: bool, merged: bool) -> Value {
    json!({
        "number": number,
        "title": title,
        "html_url": format!("https://github.com/{REPO}/pull/{number}"),
        "state": state,
        "draft": draft,
        "merged_at": merged.then_some("2026-10-20T00:00:00Z"),
        "body": "What it changes.",
        "updated_at": updated,
        "head": { "ref": format!("feat/{number}") },
        "base": { "ref": "main" },
    })
}

fn issue(number: u32, title: &str, updated: &str, state: &str, reason: Option<&str>) -> Value {
    json!({
        "number": number,
        "title": title,
        "html_url": format!("https://github.com/{REPO}/issues/{number}"),
        "state": state,
        "state_reason": reason,
        "body": null,
        "updated_at": updated,
    })
}

/// A pull request as `/issues` and the search show it.
fn pull_as_issue(number: u32, title: &str, updated: &str) -> Value {
    let mut value = issue(number, title, updated, "open", None);
    value["pull_request"] = json!({ "merged_at": null });
    value["html_url"] = json!(format!("https://github.com/{REPO}/pull/{number}"));
    value
}

/// A rig whose App is set, with `octo/maxhq` holding two pull requests
/// and three issues.
async fn rig() -> Rig {
    let rig = Rig::new().await;
    assert_eq!(rig.set(), ActionOutcome::Done);
    fill(&rig);
    rig
}

/// A rig with no App, where `gh` is signed in to an account that sees
/// `octo/maxhq`, holding the same items.
async fn gh_rig() -> Rig {
    let rig = Rig::new().await;
    rig.fake.state().gh = GhLogin::SignedIn(vec![REPO.to_owned()]);
    fill(&rig);
    rig
}

/// Gives `octo/maxhq` two pull requests and three issues.
fn fill(rig: &Rig) {
    {
        let mut state = rig.fake.state();
        state.pulls = vec![
            pull(
                3,
                "Draft the sync",
                "2026-10-24T00:00:00Z",
                "open",
                true,
                false,
            ),
            pull(
                2,
                "Fix the login page",
                "2026-10-21T00:00:00Z",
                "closed",
                false,
                true,
            ),
        ];
        state.issues = vec![
            pull_as_issue(3, "Draft the sync", "2026-10-24T00:00:00Z"),
            issue(
                5,
                "Login times out",
                "2026-10-23T00:00:00Z",
                "closed",
                Some("not_planned"),
            ),
            issue(
                4,
                "Crash on start",
                "2026-10-22T00:00:00Z",
                "closed",
                Some("completed"),
            ),
            issue(1, "Add a dark theme", "2026-10-01T00:00:00Z", "open", None),
        ];
    }
}

fn numbers(search: &GitItemSearch) -> Vec<u32> {
    search
        .items
        .iter()
        .map(|item| match item {
            InlineRef::GitItem { number, .. } => *number,
            other => panic!("not a git item: {other:?}"),
        })
        .collect()
}

/// The GETs the fake got for `path`.
fn gets(rig: &Rig, path: &str) -> usize {
    rig.fake
        .state()
        .requests
        .iter()
        .filter(|request| request.method == "GET" && request.path == path)
        .count()
}

#[tokio::test]
async fn an_empty_query_lists_the_newest_with_their_states() {
    let rig = rig().await;
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(search.unavailable, None);
    assert_eq!(search.repo.as_deref(), Some(REPO));
    // The pull request /issues repeats shows once.
    assert_eq!(numbers(&search), [3, 5, 4, 2, 1]);
    let InlineRef::GitItem {
        kind,
        state,
        head,
        base,
        url,
        checked_at,
        ..
    } = &search.items[0]
    else {
        panic!("expected a git item");
    };
    assert_eq!(
        (*kind, *state),
        (GitItemKind::PullRequest, GitItemState::Draft)
    );
    assert_eq!(
        (head.as_deref(), base.as_deref()),
        (Some("feat/3"), Some("main"))
    );
    assert_eq!(url, "https://github.com/octo/maxhq/pull/3");
    assert_eq!(*checked_at, at("2026-10-25T12:00:00Z"));
    // The App reaches the repo, so gh isn't asked.
    assert!(rig.fake.state().gh_runs.is_empty());
    let states: Vec<GitItemState> = search
        .items
        .iter()
        .map(|item| match item {
            InlineRef::GitItem { state, .. } => *state,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        states,
        [
            GitItemState::Draft,
            GitItemState::NotPlanned,
            GitItemState::Completed,
            GitItemState::Merged,
            GitItemState::Open,
        ]
    );
}

#[tokio::test]
async fn a_number_comes_first_and_one_not_listed_is_looked_up() {
    let rig = rig().await;
    rig.fake.state().past_issues = vec![issue(
        41,
        "An old one",
        "2025-01-01T00:00:00Z",
        "open",
        None,
    )];
    let search = rig.secrets.find_git_items(REPO, "#4").await;
    assert_eq!(numbers(&search), [4]);
    assert_eq!(gets(&rig, "/repos/octo/maxhq/issues/4"), 0);

    let search = rig.secrets.find_git_items(REPO, "41").await;
    assert_eq!(numbers(&search), [41]);
    assert_eq!(gets(&rig, "/repos/octo/maxhq/issues/41"), 1);
}

#[tokio::test]
async fn an_old_pull_request_is_read_with_its_branches() {
    let rig = rig().await;
    {
        let mut state = rig.fake.state();
        state.past_issues = vec![pull_as_issue(77, "Old work", "2025-01-01T00:00:00Z")];
        state.past_pulls = vec![pull(
            77,
            "Old work",
            "2025-01-01T00:00:00Z",
            "open",
            false,
            false,
        )];
    }
    let search = rig.secrets.find_git_items(REPO, "77").await;
    let [InlineRef::GitItem { kind, head, .. }] = search.items.as_slice() else {
        panic!("expected one item, got {search:?}");
    };
    assert_eq!(*kind, GitItemKind::PullRequest);
    assert_eq!(head.as_deref(), Some("feat/77"));
}

#[tokio::test]
async fn a_number_nothing_has_finds_nothing() {
    let rig = rig().await;
    let search = rig.secrets.find_git_items(REPO, "999").await;
    assert!(search.items.is_empty());
    assert_eq!(search.unavailable, None);
}

#[tokio::test]
async fn words_find_recent_items_then_what_the_search_found() {
    let rig = rig().await;
    rig.fake.state().searched = vec![
        pull_as_issue(60, "Login rework from last year", "2025-06-01T00:00:00Z"),
        issue(
            5,
            "Login times out",
            "2026-10-23T00:00:00Z",
            "closed",
            Some("not_planned"),
        ),
    ];
    let search = rig.secrets.find_git_items(REPO, "login").await;
    assert_eq!(numbers(&search), [5, 2, 60]);

    let query = rig
        .fake
        .state()
        .requests
        .iter()
        .find(|request| request.path == "/search/issues")
        .map(|request| request.query.clone())
        .unwrap();
    assert!(query.starts_with("q=login%20repo%3Aocto%2Fmaxhq&"));

    // The same words again within the minute don't search again.
    rig.secrets.find_git_items(REPO, "login").await;
    assert_eq!(gets(&rig, "/search/issues"), 1);
}

#[tokio::test]
async fn searches_stop_short_of_githubs_limit() {
    let rig = rig().await;
    for n in 0..SEARCHES_PER_MINUTE + 3 {
        rig.secrets.find_git_items(REPO, &format!("word{n}")).await;
    }
    assert_eq!(gets(&rig, "/search/issues"), SEARCHES_PER_MINUTE);
    rig.clock.advance(Duration::from_secs(61));
    rig.secrets.find_git_items(REPO, "again").await;
    assert_eq!(gets(&rig, "/search/issues"), SEARCHES_PER_MINUTE + 1);
}

#[tokio::test]
async fn a_list_is_kept_then_asked_for_again_with_its_etag() {
    let rig = rig().await;
    let pulls = "/repos/octo/maxhq/pulls";
    rig.secrets.find_git_items(REPO, "").await;
    rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(gets(&rig, pulls), 1);

    rig.clock.advance(Duration::from_secs(31));
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(gets(&rig, pulls), 2);
    assert_eq!(rig.fake.state().unchanged, 2);
    assert_eq!(numbers(&search), [3, 5, 4, 2, 1]);

    // A change comes through in full.
    rig.clock.advance(Duration::from_secs(31));
    rig.fake.state().pulls.remove(0);
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(numbers(&search), [5, 4, 2, 1]);
    assert_eq!(rig.fake.state().unchanged, 3);
}

#[tokio::test]
async fn a_token_without_issues_still_finds_pull_requests() {
    let rig = rig().await;
    rig.fake.state().installations[0].issues = None;
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(search.unavailable, None);
    assert_eq!(numbers(&search), [3, 2]);

    let search = rig.secrets.find_git_items(REPO, "2").await;
    assert_eq!(numbers(&search), [2]);
}

#[tokio::test]
async fn a_lookup_that_cant_happen_says_why() {
    let rig = Rig::new().await;
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(
        search.unavailable.as_deref(),
        Some("This device has no GitHub App, and gh isn't installed here. Send the App to it in Settings, or install gh and run gh auth login on it.")
    );
    assert_eq!(search.repo.as_deref(), Some(REPO));

    rig.fake.state().gh = GhLogin::SignedOut;
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(
        search.unavailable.as_deref(),
        Some("This device has no GitHub App, and gh isn't signed in here. Send the App to it in Settings, or run gh auth login on it.")
    );

    rig.fake.state().gh = GhLogin::SignedIn(vec!["someone/else".into()]);
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(
        search.unavailable.as_deref(),
        Some("This device has no GitHub App, and the account gh is signed in to can't see octo/maxhq. Send the App to it in Settings, or sign gh in to an account that can.")
    );

    rig.fake.state().gh = GhLogin::Missing;
    assert_eq!(rig.set(), ActionOutcome::Done);
    let search = rig.secrets.find_git_items("someone/else", "").await;
    assert_eq!(
        search.unavailable.as_deref(),
        Some("The GitHub App isn't installed on someone/else, and gh isn't installed here. Install the App there from its page on GitHub, or install gh and run gh auth login on this device.")
    );

    rig.fake.state().gh = GhLogin::SignedOut;
    let search = rig.secrets.find_git_items("someone/other", "").await;
    assert_eq!(
        search.unavailable.as_deref(),
        Some("The GitHub App isn't installed on someone/other, and gh isn't signed in here. Install the App there from its page on GitHub, or run gh auth login on this device.")
    );

    rig.fake.state().gh = GhLogin::SignedIn(Vec::new());
    let search = rig.secrets.find_git_items("someone/third", "").await;
    assert_eq!(
        search.unavailable.as_deref(),
        Some("The GitHub App isn't installed on someone/third, and the account gh is signed in to can't see it. Install the App there from its page on GitHub, or sign gh in to an account that can.")
    );

    let search = rig
        .secrets
        .find_git_items("https://gitlab.com/owner/repo.git", "")
        .await;
    assert_eq!(search.repo, None);
    assert_eq!(search.unavailable.as_deref(), Some(NOT_GITHUB));
    assert!(search.items.is_empty());
}

#[tokio::test]
async fn no_lookup_shows_a_token() {
    let rig = rig().await;
    rig.fake.state().searched = vec![issue(8, "Token", "2026-10-01T00:00:00Z", "open", None)];
    for query in ["", "8", "token"] {
        let search = rig.secrets.find_git_items(REPO, query).await;
        let text = serde_json::to_string(&search).unwrap();
        assert!(!text.contains(TOKEN_PREFIX), "{text}");
    }
}

#[tokio::test]
async fn without_an_app_gh_finds_the_same_items() {
    let rig = gh_rig().await;
    rig.fake.state().past_issues = vec![issue(
        41,
        "An old one",
        "2025-01-01T00:00:00Z",
        "open",
        None,
    )];
    rig.fake.state().searched = vec![pull_as_issue(
        60,
        "Login rework from last year",
        "2025-06-01T00:00:00Z",
    )];
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(search.unavailable, None);
    assert_eq!(search.repo.as_deref(), Some(REPO));
    assert_eq!(numbers(&search), [3, 5, 4, 2, 1]);
    let InlineRef::GitItem { head, url, .. } = &search.items[0] else {
        panic!("expected a git item");
    };
    assert_eq!(head.as_deref(), Some("feat/3"));
    assert_eq!(url, "https://github.com/octo/maxhq/pull/3");

    let search = rig.secrets.find_git_items(REPO, "41").await;
    assert_eq!(numbers(&search), [41]);
    let search = rig.secrets.find_git_items(REPO, "login").await;
    assert_eq!(numbers(&search), [5, 2, 60]);

    // Every read went through gh, read-only, to github.com.
    assert!(rig.fake.repo_mints().is_empty());
    let state = rig.fake.state();
    assert!(!state.gh_runs.is_empty());
    for args in &state.gh_runs {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(
            args[..6],
            [
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                "--include"
            ]
        );
    }
}

#[tokio::test]
async fn an_app_that_doesnt_reach_the_repo_leaves_it_to_gh() {
    let rig = rig().await;
    rig.fake.state().gh = GhLogin::SignedIn(vec!["someone/else".into()]);
    let search = rig.secrets.find_git_items("someone/else", "").await;
    assert_eq!(search.unavailable, None);
    assert_eq!(numbers(&search), [3, 5, 4, 2, 1]);
}

#[tokio::test]
async fn gh_keeps_a_list_then_asks_for_it_again_with_its_etag() {
    let rig = gh_rig().await;
    let pulls = "/repos/octo/maxhq/pulls";
    rig.secrets.find_git_items(REPO, "").await;
    rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(gets(&rig, pulls), 1);

    rig.clock.advance(Duration::from_secs(31));
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert_eq!(gets(&rig, pulls), 2);
    assert_eq!(rig.fake.state().unchanged, 2);
    assert_eq!(numbers(&search), [3, 5, 4, 2, 1]);
    let named = rig
        .fake
        .state()
        .gh_runs
        .iter()
        .filter(|args| args.iter().any(|arg| arg.starts_with("If-None-Match: ")))
        .count();
    assert_eq!(named, 2);
}

#[tokio::test]
async fn gh_searches_stop_short_of_githubs_limit() {
    let rig = gh_rig().await;
    for n in 0..SEARCHES_PER_MINUTE + 3 {
        rig.secrets.find_git_items(REPO, &format!("word{n}")).await;
    }
    assert_eq!(gets(&rig, "/search/issues"), SEARCHES_PER_MINUTE);
    rig.clock.advance(Duration::from_secs(61));
    rig.secrets.find_git_items(REPO, "again").await;
    assert_eq!(gets(&rig, "/search/issues"), SEARCHES_PER_MINUTE + 1);
}

#[tokio::test]
async fn what_the_app_read_isnt_shown_once_it_goes() {
    let rig = rig().await;
    assert_eq!(
        numbers(&rig.secrets.find_git_items(REPO, "").await).len(),
        5
    );
    let credential = github_credential(rig.secrets.device());
    assert_eq!(
        rig.secrets.forget_github(credential),
        Some(ActionOutcome::Done)
    );
    rig.fake.state().gh = GhLogin::SignedOut;
    let search = rig.secrets.find_git_items(REPO, "").await;
    assert!(search.items.is_empty());
    assert!(search.unavailable.is_some());
}
