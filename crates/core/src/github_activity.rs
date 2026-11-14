// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What happened on a repository, read from GitHub's REST listings. The
//! events API gives an app's token nothing for a private repository, so
//! pull requests, issues, comments and reviews come from the listings that
//! work for it: each new item is made into the events API event a webhook
//! stands for, which [`hooked`] makes into a webhook request like any
//! other. The events API is still read for the types no listing covers,
//! such as pushes, which on a private repository only a webhook brings.
//!
//! A listing's items are new when they happened after the poller began
//! reading it, so each carries when it happened. A pull request's
//! `synchronize` is its head moving between two reads of the list.
//!
//! [`hooked`]: crate::github_events::hooked

use maxhq_protocol::time::Timestamp;
use serde_json::{json, Value};

use crate::github_events::{is_sha, webhook_name};

/// The webhook events the listings cover, which the events API isn't read
/// for.
pub const COVERED: [&str; 5] = [
    "issue_comment",
    "issues",
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
];

/// Where a poller reads what happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listing {
    /// Pull requests, the latest updated first: each one opened, each new
    /// head as a `synchronize`, and which ones may have new reviews.
    Pulls,
    /// Issues, the latest opened first: each one opened.
    Issues,
    /// What happened to issues and pull requests, the latest first:
    /// closed, reopened, labeled and the like.
    IssueEvents,
    /// Comments on issues and pull requests, the latest first.
    Comments,
    /// Comments on pull requests' changes, the latest first.
    ReviewComments,
    /// The events API, for the types no listing covers.
    Events,
}

/// The name a poller notes when it began reading pull requests' reviews
/// under, beside each [`Listing::key`].
pub const REVIEWS: &str = "reviews";

impl Listing {
    const ALL: [Self; 6] = [
        Self::Pulls,
        Self::Issues,
        Self::IssueEvents,
        Self::Comments,
        Self::ReviewComments,
        Self::Events,
    ];

    /// Its name, which a poller notes when it began reading it under.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pulls => "pulls",
            Self::Issues => "issues",
            Self::IssueEvents => "issue_events",
            Self::Comments => "comments",
            Self::ReviewComments => "review_comments",
            Self::Events => "events",
        }
    }

    /// Its path on GitHub's API for `repo` ("owner/name").
    pub fn path(self, repo: &str) -> String {
        let rest = match self {
            Self::Pulls => "pulls?state=all&sort=updated&direction=desc&per_page=100",
            Self::Issues => "issues?state=all&sort=created&direction=desc&per_page=100",
            Self::IssueEvents => "issues/events?per_page=100",
            Self::Comments => "issues/comments?sort=created&direction=desc&per_page=100",
            Self::ReviewComments => "pulls/comments?sort=created&direction=desc&per_page=100",
            Self::Events => "events?per_page=100",
        };
        format!("/repos/{repo}/{rest}")
    }

    /// Whether tasks that watch webhook events `watched` need it.
    fn needed(self, watched: &[&str]) -> bool {
        let any = |names: &[&str]| names.iter().any(|name| watched.contains(name));
        match self {
            Self::Pulls => any(&["pull_request", "pull_request_review"]),
            Self::Issues => any(&["issues"]),
            Self::IssueEvents => any(&["pull_request", "issues"]),
            Self::Comments => any(&["issue_comment"]),
            Self::ReviewComments => any(&["pull_request_review_comment"]),
            Self::Events => !uncovered(watched).is_empty(),
        }
    }
}

/// The listings tasks that watch webhook events `watched` need.
pub fn listings(watched: &[&str]) -> Vec<Listing> {
    Listing::ALL
        .into_iter()
        .filter(|listing| listing.needed(watched))
        .collect()
}

/// The events in `watched` that only the events API or a webhook brings.
pub fn uncovered<'a>(watched: &[&'a str]) -> Vec<&'a str> {
    watched
        .iter()
        .copied()
        .filter(|name| !COVERED.contains(name))
        .collect()
}

/// Where pull request `number`'s reviews in `repo` are listed.
pub fn reviews_path(repo: &str, number: u64) -> String {
    format!("/repos/{repo}/pulls/{number}/reviews?per_page=100")
}

/// When an event made here, or read from the events API, happened.
pub fn happened(event: &Value) -> Option<Timestamp> {
    time(&event["created_at"])
}

/// `at`, a time as GitHub writes it, such as `2026-01-02T03:04:05Z`.
pub fn time(at: &Value) -> Option<Timestamp> {
    let at = at.as_str()?.parse::<jiff::Timestamp>().ok()?;
    u64::try_from(at.as_millisecond()).ok().map(Timestamp)
}

/// The event `item` from `listing` stands for, in `repository` as GitHub
/// reads it, or none when it stands for nothing a task can watch. Pull
/// requests come from [`pull_opened`] and [`pull_synchronized`] instead.
pub fn listed(listing: Listing, item: &Value, repository: &Value) -> Option<Value> {
    match listing {
        Listing::Pulls => None,
        Listing::Issues => issue_opened(item, repository),
        Listing::IssueEvents => issue_event(item, repository),
        Listing::Comments => comment(item, repository),
        Listing::ReviewComments => review_comment(item, repository),
        Listing::Events => {
            let name = webhook_name(item["type"].as_str()?)?;
            (!COVERED.contains(&name)).then(|| item.clone())
        }
    }
}

/// Pull request `pull`, as the pull requests list has it, opened.
pub fn pull_opened(pull: &Value, repository: &Value) -> Option<Value> {
    let number = pull["number"].as_u64()?;
    let payload = json!({ "action": "opened", "number": number, "pull_request": pull });
    let id = format!("pull-{number}-opened");
    made(
        id,
        "PullRequestEvent",
        &pull["user"],
        &pull["created_at"],
        repository,
        payload,
    )
}

/// Pull request `pull`'s head moved from `before` to where the list has it
/// now, which its author pushed.
pub fn pull_synchronized(pull: &Value, before: &str, repository: &Value) -> Option<Value> {
    let number = pull["number"].as_u64()?;
    let after = pull["head"]["sha"].as_str().filter(|sha| is_sha(sha))?;
    let payload = json!({
        "action": "synchronize",
        "number": number,
        "pull_request": pull,
        "before": before,
        "after": after,
    });
    let id = format!("pull-{number}-synchronize-{after}");
    made(
        id,
        "PullRequestEvent",
        &pull["user"],
        &pull["updated_at"],
        repository,
        payload,
    )
}

/// `review` of `pull`, from the pull request's reviews, submitted. One
/// still pending isn't.
pub fn review_submitted(review: &Value, pull: &Value, repository: &Value) -> Option<Value> {
    let id = review["id"].as_u64()?;
    let state = review["state"].as_str()?;
    if state == "PENDING" {
        return None;
    }
    let mut review = review.clone();
    // A webhook names the state in lower case.
    review["state"] = json!(state.to_ascii_lowercase());
    let payload = json!({ "action": "submitted", "review": &review, "pull_request": pull });
    let id = format!("review-{id}");
    made(
        id,
        "PullRequestReviewEvent",
        &review["user"],
        &review["submitted_at"],
        repository,
        payload,
    )
}

/// Issue `issue`, from the issues list, opened. A pull request in the
/// list isn't an issue.
fn issue_opened(issue: &Value, repository: &Value) -> Option<Value> {
    if issue.get("pull_request").is_some() {
        return None;
    }
    let number = issue["number"].as_u64()?;
    let payload = json!({ "action": "opened", "issue": issue });
    let id = format!("issue-{number}-opened");
    made(
        id,
        "IssuesEvent",
        &issue["user"],
        &issue["created_at"],
        repository,
        payload,
    )
}

/// Each issue event a listing has that a webhook sends, and the action it
/// sends it as. A pull request's opening and new commits come from the
/// pull requests list, and a merge as its `closed`.
const ISSUE_ACTIONS: [(&str, &str); 16] = [
    ("closed", "closed"),
    ("reopened", "reopened"),
    ("labeled", "labeled"),
    ("unlabeled", "unlabeled"),
    ("assigned", "assigned"),
    ("unassigned", "unassigned"),
    ("milestoned", "milestoned"),
    ("demilestoned", "demilestoned"),
    ("locked", "locked"),
    ("unlocked", "unlocked"),
    ("pinned", "pinned"),
    ("unpinned", "unpinned"),
    ("review_requested", "review_requested"),
    ("review_request_removed", "review_request_removed"),
    ("ready_for_review", "ready_for_review"),
    ("convert_to_draft", "converted_to_draft"),
];

/// What a webhook carries beside its issue or pull request, when the
/// issue event has it.
const ISSUE_EVENT_PARTS: [&str; 5] = [
    "label",
    "assignee",
    "requested_reviewer",
    "requested_team",
    "milestone",
];

/// The pull request or issue event issue event `item` stands for. A pull
/// request comes slim, so it's read in full.
fn issue_event(item: &Value, repository: &Value) -> Option<Value> {
    let id = item["id"].as_u64()?;
    let name = item["event"].as_str()?;
    let action = ISSUE_ACTIONS.iter().find(|(event, _)| *event == name)?.1;
    let issue = &item["issue"];
    let number = issue["number"].as_u64()?;
    let mut payload = json!({ "action": action });
    for part in ISSUE_EVENT_PARTS {
        if item[part].is_object() {
            payload[part] = item[part].clone();
        }
    }
    let kind = if issue["pull_request"].is_object() {
        payload["number"] = json!(number);
        payload["pull_request"] = json!({ "number": number, "url": issue["pull_request"]["url"] });
        "PullRequestEvent"
    } else {
        payload["issue"] = issue.clone();
        "IssuesEvent"
    };
    let id = format!("issue-event-{id}");
    made(
        id,
        kind,
        &item["actor"],
        &item["created_at"],
        repository,
        payload,
    )
}

/// Comment `comment`, on an issue or a pull request, made. Its issue
/// comes slim, so it's read in full.
fn comment(comment: &Value, repository: &Value) -> Option<Value> {
    let id = comment["id"].as_u64()?;
    let url = comment["issue_url"].as_str()?;
    let number = last_number(url)?;
    let payload = json!({
        "action": "created",
        "comment": comment,
        "issue": { "number": number, "url": url },
    });
    let id = format!("comment-{id}");
    made(
        id,
        "IssueCommentEvent",
        &comment["user"],
        &comment["created_at"],
        repository,
        payload,
    )
}

/// Comment `comment`, on a pull request's changes, made. Its pull request
/// comes slim, so it's read in full.
fn review_comment(comment: &Value, repository: &Value) -> Option<Value> {
    let id = comment["id"].as_u64()?;
    let url = comment["pull_request_url"].as_str()?;
    let number = last_number(url)?;
    let payload = json!({
        "action": "created",
        "comment": comment,
        "pull_request": { "number": number, "url": url },
    });
    let id = format!("review-comment-{id}");
    let kind = "PullRequestReviewCommentEvent";
    made(
        id,
        kind,
        &comment["user"],
        &comment["created_at"],
        repository,
        payload,
    )
}

/// The number `url` ends with, such as an issue's.
fn last_number(url: &str) -> Option<u64> {
    url.rsplit('/').next()?.parse().ok()
}

/// An events API event of type `kind` with ID `id`, by `actor` at `at`, in
/// `repository`, which goes in `payload` too when GitHub read it whole.
fn made(
    id: String,
    kind: &str,
    actor: &Value,
    at: &Value,
    repository: &Value,
    mut payload: Value,
) -> Option<Value> {
    let name = repository["full_name"].as_str()?;
    let at = at.as_str()?;
    if repository["id"].is_u64() {
        payload["repository"] = repository.clone();
    }
    Some(json!({
        "id": id,
        "type": kind,
        "actor": actor,
        "repo": { "id": repository["id"], "name": name, "url": repository["url"] },
        "payload": payload,
        "created_at": at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github_events::{hooked, shared_key, slim_parts};

    const REPO: &str = "owner/repo";
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const NEXT: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn repository() -> Value {
        json!({ "id": 9, "full_name": REPO, "private": true, "url": "https://api.github.com/repos/owner/repo" })
    }

    fn pull(number: u64) -> Value {
        json!({
            "number": number,
            "state": "open",
            "title": "Fix the stops",
            "user": { "id": 7, "login": "kit" },
            "author_association": "MEMBER",
            "created_at": "2026-11-09T10:00:00Z",
            "updated_at": "2026-11-09T10:05:00Z",
            "head": { "ref": "fix", "sha": NEXT, "repo": { "full_name": REPO } },
            "base": { "ref": "main", "sha": SHA, "repo": { "full_name": REPO } },
        })
    }

    #[test]
    fn tasks_read_only_the_listings_their_events_need() {
        assert_eq!(
            listings(&["issue_comment", "pull_request", "pull_request_review"]),
            [Listing::Pulls, Listing::IssueEvents, Listing::Comments]
        );
        assert_eq!(listings(&["push"]), [Listing::Events]);
        assert_eq!(
            listings(&["issues", "release"]),
            [Listing::Issues, Listing::IssueEvents, Listing::Events]
        );
        assert_eq!(
            listings(&["pull_request_review_comment"]),
            [Listing::ReviewComments]
        );
        assert!(listings(&[]).is_empty());
        assert_eq!(
            uncovered(&["push", "issues", "release"]),
            ["push", "release"]
        );
        assert_eq!(
            Listing::Comments.path(REPO),
            "/repos/owner/repo/issues/comments?sort=created&direction=desc&per_page=100"
        );
    }

    #[test]
    fn a_listed_pull_request_opens_and_moves_as_a_webhook_would_say() {
        let opened = hooked(&pull_opened(&pull(4), &repository()).unwrap()).unwrap();
        assert_eq!(opened.id, "pull-4-opened");
        assert_eq!(opened.event, "pull_request");
        assert_eq!(opened.body["action"], "opened");
        assert_eq!(opened.body["pull_request"]["title"], "Fix the stops");
        assert_eq!(opened.body["sender"]["login"], "kit");
        assert_eq!(opened.body["repository"]["private"], true);
        assert!(slim_parts(&opened, REPO).is_empty());
        assert_eq!(
            shared_key(&opened).as_deref(),
            Some("pull_request|opened|4")
        );

        let event = pull_synchronized(&pull(4), SHA, &repository()).unwrap();
        assert_eq!(happened(&event), time(&json!("2026-11-09T10:05:00Z")));
        let sync = hooked(&event).unwrap();
        assert_eq!(sync.id, format!("pull-4-synchronize-{NEXT}"));
        assert_eq!(sync.body["action"], "synchronize");
        assert_eq!(sync.body["before"], SHA);
        assert_eq!(sync.body["after"], NEXT);
        assert_eq!(
            shared_key(&sync),
            Some(format!("pull_request|synchronize|4|{NEXT}"))
        );

        let mut headless = pull(4);
        headless["head"]["sha"] = Value::Null;
        assert_eq!(pull_synchronized(&headless, SHA, &repository()), None);
    }

    #[test]
    fn a_submitted_review_goes_with_its_pull_request_and_a_pending_one_doesnt() {
        let review = json!({
            "id": 31,
            "user": { "login": "kaya" },
            "state": "CHANGES_REQUESTED",
            "body": "Please rename it",
            "author_association": "COLLABORATOR",
            "submitted_at": "2026-11-09T11:00:00Z",
        });
        let reviewed =
            hooked(&review_submitted(&review, &pull(4), &repository()).unwrap()).unwrap();
        assert_eq!(reviewed.id, "review-31");
        assert_eq!(reviewed.event, "pull_request_review");
        assert_eq!(reviewed.body["action"], "submitted");
        assert_eq!(reviewed.body["review"]["state"], "changes_requested");
        assert_eq!(reviewed.body["pull_request"]["number"], 4);
        assert_eq!(reviewed.body["sender"]["login"], "kaya");
        assert!(slim_parts(&reviewed, REPO).is_empty());

        let mut pending = review.clone();
        pending["state"] = json!("PENDING");
        pending["submitted_at"] = Value::Null;
        assert_eq!(review_submitted(&pending, &pull(4), &repository()), None);
    }

    #[test]
    fn comments_go_with_their_issue_or_pull_request_read_in_full() {
        let made = json!({
            "id": 55,
            "user": { "login": "kaya" },
            "body": "Looks good",
            "issue_url": "https://api.github.com/repos/owner/repo/issues/4",
            "created_at": "2026-11-09T11:00:00Z",
        });
        let commented = hooked(&listed(Listing::Comments, &made, &repository()).unwrap()).unwrap();
        assert_eq!(commented.id, "comment-55");
        assert_eq!(commented.event, "issue_comment");
        assert_eq!(commented.body["action"], "created");
        assert_eq!(commented.body["comment"]["body"], "Looks good");
        assert_eq!(commented.body["sender"]["login"], "kaya");
        let keys: Vec<_> = slim_parts(&commented, REPO)
            .into_iter()
            .map(|part| (part.key, part.path))
            .collect();
        assert_eq!(keys, [("issue", "/repos/owner/repo/issues/4".to_owned())]);

        let on_change = json!({
            "id": 56,
            "user": { "login": "kaya" },
            "path": "src/main.rs",
            "pull_request_url": "https://api.github.com/repos/owner/repo/pulls/4",
            "created_at": "2026-11-09T11:01:00Z",
        });
        let event = listed(Listing::ReviewComments, &on_change, &repository()).unwrap();
        let commented = hooked(&event).unwrap();
        assert_eq!(commented.id, "review-comment-56");
        assert_eq!(commented.event, "pull_request_review_comment");
        assert_eq!(
            slim_parts(&commented, REPO)[0].path,
            "/repos/owner/repo/pulls/4"
        );
    }

    #[test]
    fn issue_events_are_the_pull_request_or_issue_events_a_webhook_sends() {
        let item = |event: &str, issue: Value| {
            json!({
                "id": 801,
                "event": event,
                "actor": { "login": "kaya" },
                "label": { "name": "bug" },
                "created_at": "2026-11-09T12:00:00Z",
                "issue": issue,
            })
        };
        let on_pull = json!({
            "number": 4,
            "user": { "login": "kit" },
            "pull_request": { "url": "https://api.github.com/repos/owner/repo/pulls/4" },
        });
        let closed = listed(
            Listing::IssueEvents,
            &item("closed", on_pull.clone()),
            &repository(),
        );
        let closed = hooked(&closed.unwrap()).unwrap();
        assert_eq!(closed.id, "issue-event-801");
        assert_eq!(closed.event, "pull_request");
        assert_eq!(closed.body["action"], "closed");
        assert_eq!(closed.body["sender"]["login"], "kaya");
        assert_eq!(
            slim_parts(&closed, REPO)[0].path,
            "/repos/owner/repo/pulls/4"
        );
        assert_eq!(
            shared_key(&closed).as_deref(),
            Some("pull_request|closed|4")
        );

        let drafted = listed(
            Listing::IssueEvents,
            &item("convert_to_draft", on_pull.clone()),
            &repository(),
        );
        assert_eq!(drafted.unwrap()["payload"]["action"], "converted_to_draft");

        let an_issue = json!({ "number": 5, "user": { "login": "kit" } });
        let labeled = listed(
            Listing::IssueEvents,
            &item("labeled", an_issue),
            &repository(),
        );
        let labeled = hooked(&labeled.unwrap()).unwrap();
        assert_eq!(labeled.event, "issues");
        assert_eq!(labeled.body["label"]["name"], "bug");
        assert!(slim_parts(&labeled, REPO).is_empty());

        // The pull requests list says these, and some only say who knew.
        for skipped in ["merged", "mentioned", "subscribed", "head_ref_deleted"] {
            assert_eq!(
                listed(
                    Listing::IssueEvents,
                    &item(skipped, on_pull.clone()),
                    &repository()
                ),
                None
            );
        }
    }

    #[test]
    fn the_issues_list_opens_issues_but_not_pull_requests() {
        let issue = json!({
            "number": 5,
            "user": { "login": "kit" },
            "title": "Crash",
            "created_at": "2026-11-09T12:00:00Z",
        });
        let opened = hooked(&listed(Listing::Issues, &issue, &repository()).unwrap()).unwrap();
        assert_eq!(opened.id, "issue-5-opened");
        assert_eq!(opened.event, "issues");
        assert_eq!(opened.body["issue"]["title"], "Crash");
        let mut pull = issue;
        pull["pull_request"] = json!({ "url": "https://api.github.com/repos/owner/repo/pulls/5" });
        assert_eq!(listed(Listing::Issues, &pull, &repository()), None);
    }

    #[test]
    fn the_events_api_gives_only_what_no_listing_covers() {
        let event = |kind: &str| json!({ "id": "26000000001", "type": kind, "created_at": "2026-11-09T12:00:00Z" });
        assert!(listed(Listing::Events, &event("PushEvent"), &repository()).is_some());
        assert!(listed(Listing::Events, &event("ReleaseEvent"), &repository()).is_some());
        assert_eq!(
            listed(Listing::Events, &event("PullRequestEvent"), &repository()),
            None
        );
        assert_eq!(
            listed(Listing::Events, &event("IssueCommentEvent"), &repository()),
            None
        );
        assert_eq!(
            listed(Listing::Events, &event("WorkflowRunEvent"), &repository()),
            None
        );
    }

    #[test]
    fn a_repository_read_only_by_name_still_names_the_events() {
        let by_name = json!({ "full_name": REPO });
        let opened = hooked(&pull_opened(&pull(4), &by_name).unwrap()).unwrap();
        assert_eq!(opened.body["repository"]["full_name"], REPO);
        assert_eq!(opened.body["repository"]["owner"]["login"], "owner");
        assert_eq!(pull_opened(&pull(4), &json!({})), None);
    }
}
