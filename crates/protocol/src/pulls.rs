// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pull requests on GitHub: the ones an agent's work is linked to, what
//! the watcher last saw of each, and what the list and the viewer show.
//! The host is always github.com, so a PR is its repository and number.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::AgentId;
use crate::time::Timestamp;

/// One pull request.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullKey {
    /// The repository, as "owner/name". Two spellings that differ only in
    /// case are the same repository.
    pub repo: String,
    /// The PR's number in the repository.
    #[specta(type = specta_typescript::Number)]
    pub number: u64,
}

/// Whether a PR is still open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PullState {
    /// Open, a draft or not.
    Open,
    /// Closed without being merged.
    Closed,
    /// Merged.
    Merged,
}

/// How a PR's checks stand, all of them together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChecksState {
    /// Every check passed.
    Passing,
    /// At least one failed.
    Failing,
    /// None failed and some still run.
    Pending,
    /// The PR has no checks.
    None,
    /// The GitHub App can't read checks, because its installation hasn't
    /// accepted the read permissions.
    Unknown,
}

/// Whether GitHub can merge a PR as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mergeable {
    /// It merges cleanly.
    Yes,
    /// It conflicts with its base.
    Conflicting,
    /// GitHub hasn't worked it out yet.
    Unknown,
}

/// Where a PR's reviews leave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    /// Approved, with no changes asked for since.
    Approved,
    /// A reviewer asked for changes.
    ChangesRequested,
    /// No review decides it yet.
    None,
}

/// What the watcher last read of a PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullSnapshot {
    /// The PR's title.
    pub title: String,
    /// Open, closed or merged.
    pub state: PullState,
    /// Whether it's a draft.
    pub draft: bool,
    /// The login of whoever opened it.
    pub author: String,
    /// The branch it merges from.
    pub head: String,
    /// The branch it merges into.
    pub base: String,
    /// The commit the head branch is at.
    pub head_sha: String,
    /// Its checks, together.
    pub checks: ChecksState,
    /// Whether it merges cleanly.
    pub mergeable: Mergeable,
    /// Where its reviews leave it.
    pub review: ReviewState,
    /// Lines added.
    pub additions: u32,
    /// Lines deleted.
    pub deletions: u32,
    /// When GitHub last changed it.
    pub updated_at: Timestamp,
    /// When it was merged, once it was.
    pub merged_at: Option<Timestamp>,
}

/// How a PR came to be linked to an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkSource {
    /// The agent opened it with `gh pr create`.
    Created,
    /// Its head branch is the agent's branch.
    Branch,
    /// The agent linked it with a tool.
    Agent,
    /// You linked it.
    You,
    /// You unlinked it, so a branch match doesn't bring it back. A client
    /// doesn't show it.
    Dismissed,
}

/// An agent's watch on a PR: it gets a message when checks fail, required
/// checks pass, new comments or reviews arrive, no checks report for a
/// while, or the PR conflicts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullWatch {
    /// Comments and reviews up to this time have been told to the agent.
    pub remarks_through: Option<Timestamp>,
    /// The comments and reviews at `remarks_through` already told, so one
    /// isn't told twice.
    #[specta(type = Vec<specta_typescript::Number>)]
    pub remark_ids: Vec<u64>,
    /// How many times only a comment woke the agent. The watch stops
    /// waking it for comments alone after a limit.
    pub comment_wakes: u32,
    /// Reads of the PR that failed in a row. The watch ends after a limit.
    pub failures: u32,
    /// Since when the PR's head has had no checks, while the agent waits
    /// to hear that none reported. None once a check shows up or it's
    /// been told. A watch from before this field waits from the next push.
    #[serde(default)]
    pub checks_awaited_since: Option<Timestamp>,
}

/// A PR linked to an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullLink {
    /// The PR.
    pub key: PullKey,
    /// How it was linked.
    pub source: LinkSource,
    /// When.
    pub at: Timestamp,
    /// What the watcher last read of it, none until its first read.
    pub snapshot: Option<PullSnapshot>,
    /// The agent's watch on it, if it asked for one.
    #[serde(default)]
    pub watch: Option<PullWatch>,
}

/// Which PRs a list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PullListState {
    /// Open ones.
    Open,
    /// Closed and merged ones.
    Closed,
}

/// A row in the PR list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullSummary {
    /// The PR.
    pub key: PullKey,
    /// What was last read of it.
    pub pull: PullSnapshot,
    /// The agents it's linked to, on the device that answered.
    pub agents: Vec<AgentId>,
}

/// How a merge combines a PR's commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    /// A merge commit.
    Merge,
    /// One squashed commit.
    Squash,
    /// The commits rebased onto the base.
    Rebase,
}

/// What a review said.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    /// Approved.
    Approved,
    /// Asked for changes.
    ChangesRequested,
    /// Only commented.
    Commented,
    /// Was dismissed.
    Dismissed,
}

/// One review of a PR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullReview {
    /// The reviewer's login.
    pub author: String,
    /// What it said.
    pub verdict: ReviewVerdict,
    /// The commit it reviewed, so a client can tell whether you approved
    /// the current head.
    pub commit_sha: String,
    /// When it was sent.
    pub at: Option<Timestamp>,
}

/// How one check stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// It passed.
    Passing,
    /// It failed.
    Failing,
    /// It still runs, or hasn't started.
    Pending,
    /// It was skipped or ended neutral.
    Skipped,
}

/// One check run or commit status on a PR's head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullCheck {
    /// The check's name, or a status's context.
    pub name: String,
    /// How it stands.
    pub state: CheckState,
    /// Its page on GitHub.
    pub url: Option<String>,
}

/// A PR as the viewer shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullDetail {
    /// Its row.
    pub summary: PullSummary,
    /// Its description, as Markdown.
    pub body: Option<String>,
    /// Its checks.
    pub checks: Vec<PullCheck>,
    /// Its reviews, oldest first.
    pub reviews: Vec<PullReview>,
    /// The merge methods the repository allows.
    pub merge_methods: Vec<MergeMethod>,
}

/// How a file changed in a PR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PullFileStatus {
    /// It's new.
    Added,
    /// It's gone.
    Removed,
    /// Its content changed.
    Modified,
    /// It moved.
    Renamed,
    /// It was copied from another file.
    Copied,
    /// Only its mode changed.
    Changed,
    /// It didn't change.
    Unchanged,
}

/// One file a PR changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullFile {
    /// Its path.
    pub path: String,
    /// How it changed.
    pub status: PullFileStatus,
    /// Lines added.
    pub additions: u32,
    /// Lines deleted.
    pub deletions: u32,
    /// The unified diff, which GitHub leaves out for a binary or very
    /// large file.
    pub patch: Option<String>,
}

/// A comment on a PR, on the conversation or on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PullComment {
    /// GitHub's ID for it.
    #[specta(type = specta_typescript::Number)]
    pub id: u64,
    /// Its author's login.
    pub author: String,
    /// What it says, as Markdown.
    pub body: String,
    /// When it was made.
    pub at: Timestamp,
    /// The file a line comment is on. None for the conversation.
    pub path: Option<String>,
    /// The line a line comment is on.
    pub line: Option<u32>,
    /// Its page on GitHub.
    pub url: String,
}

/// What you do to a PR, as yourself, through the device holding your
/// GitHub sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PullAction {
    /// Merge it.
    Merge {
        /// How.
        method: MergeMethod,
    },
    /// Approve its head.
    Approve,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> PullSnapshot {
        PullSnapshot {
            title: "Fix the build".into(),
            state: PullState::Open,
            draft: false,
            author: "octocat".into(),
            head: "fix-build".into(),
            base: "main".into(),
            head_sha: "abc123".into(),
            checks: ChecksState::Pending,
            mergeable: Mergeable::Unknown,
            review: ReviewState::None,
            additions: 3,
            deletions: 1,
            updated_at: Timestamp(10),
            merged_at: None,
        }
    }

    #[test]
    fn a_link_round_trips() {
        let link = PullLink {
            key: PullKey {
                repo: "owner/repo".into(),
                number: 12,
            },
            source: LinkSource::Created,
            at: Timestamp(5),
            snapshot: Some(snapshot()),
            watch: Some(PullWatch {
                remark_ids: vec![4_000_000_001],
                ..PullWatch::default()
            }),
        };
        let json = serde_json::to_value(&link).expect("it serializes");
        assert_eq!(json["source"], "created");
        assert_eq!(json["snapshot"]["checks"], "pending");
        let back: PullLink = serde_json::from_value(json).expect("it reads back");
        assert_eq!(back, link);
    }

    #[test]
    fn a_link_without_a_watch_reads() {
        let json = serde_json::json!({
            "key": {"repo": "owner/repo", "number": 3},
            "source": "branch",
            "at": 1,
            "snapshot": null,
        });
        let link: PullLink = serde_json::from_value(json).expect("it reads");
        assert_eq!(link.watch, None);
    }

    #[test]
    fn an_action_round_trips() {
        let merge = PullAction::Merge {
            method: MergeMethod::Squash,
        };
        let json = serde_json::to_value(merge).expect("it serializes");
        assert_eq!(
            json,
            serde_json::json!({"action": "merge", "method": "squash"})
        );
        let back: PullAction = serde_json::from_value(json).expect("it reads back");
        assert_eq!(back, merge);
    }

    #[test]
    fn a_detail_round_trips() {
        let detail = PullDetail {
            summary: PullSummary {
                key: PullKey {
                    repo: "owner/repo".into(),
                    number: 7,
                },
                pull: snapshot(),
                agents: vec![AgentId::derive("a")],
            },
            body: Some("Fixes it.".into()),
            checks: vec![PullCheck {
                name: "test".into(),
                state: CheckState::Failing,
                url: None,
            }],
            reviews: vec![PullReview {
                author: "octocat".into(),
                verdict: ReviewVerdict::ChangesRequested,
                commit_sha: "abc123".into(),
                at: Some(Timestamp(9)),
            }],
            merge_methods: vec![MergeMethod::Merge, MergeMethod::Rebase],
        };
        let json = serde_json::to_value(&detail).expect("it serializes");
        assert_eq!(json["reviews"][0]["verdict"], "changes_requested");
        let back: PullDetail = serde_json::from_value(json).expect("it reads back");
        assert_eq!(back, detail);
    }
}
