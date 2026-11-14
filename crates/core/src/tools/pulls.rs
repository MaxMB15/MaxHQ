// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The PR tools' arguments: what each takes, and its checks. Each acts on
//! a PR in the calling agent's own repository, and none merges or
//! approves.

use serde_json::{json, Value};

use super::agents::number;
use super::{fields, Call, Tool};

/// A PR in the caller's repository, by its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Numbered {
    /// The PR's number.
    pub number: u64,
}

/// A short title for each PR tool.
pub(super) fn title(tool: Tool) -> &'static str {
    match tool {
        Tool::LinkPullRequest => "Link a pull request",
        Tool::UnlinkPullRequest => "Unlink a pull request",
        Tool::ListPullRequests => "List your pull requests",
        Tool::WatchPullRequest => "Watch a pull request",
        Tool::UnwatchPullRequest => "Stop watching a pull request",
        _ => unreachable!("not a PR tool"),
    }
}

/// What the agent reads about each PR tool.
pub(super) fn description(tool: Tool) -> &'static str {
    match tool {
        Tool::LinkPullRequest => {
            "Links a pull request in your repository to you by its number, so the user sees \
             it with you."
        }
        Tool::UnlinkPullRequest => "Unlinks a pull request from you by its number.",
        Tool::ListPullRequests => {
            "Lists the pull requests linked to you, with each one's state and checks."
        }
        Tool::WatchPullRequest => {
            "MaxHQ wakes you when checks fail, every check passes, new comments or reviews \
             arrive, or the PR conflicts. End your turn after this call."
        }
        Tool::UnwatchPullRequest => "Stops waking you for a pull request.",
        _ => unreachable!("not a PR tool"),
    }
}

/// The JSON schema for each PR tool's arguments.
pub(super) fn input_schema(tool: Tool) -> Value {
    let (properties, required) = match tool {
        Tool::ListPullRequests => (json!({}), json!([])),
        Tool::LinkPullRequest
        | Tool::UnlinkPullRequest
        | Tool::WatchPullRequest
        | Tool::UnwatchPullRequest => (
            json!({"number": {"type": "integer", "minimum": 1, "description": "The pull request's number in your repository."}}),
            json!(["number"]),
        ),
        _ => unreachable!("not a PR tool"),
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Checks a PR tool's arguments.
pub(super) fn parse(tool: Tool, arguments: &Value) -> Result<Call, String> {
    let args = fields(arguments)?;
    let numbered = || -> Result<Numbered, String> {
        let number = number(&args, "number", 1, u64::MAX)?.ok_or("Give a `number`.")?;
        Ok(Numbered { number })
    };
    Ok(match tool {
        Tool::LinkPullRequest => Call::LinkPullRequest(numbered()?),
        Tool::UnlinkPullRequest => Call::UnlinkPullRequest(numbered()?),
        Tool::ListPullRequests => Call::ListPullRequests,
        Tool::WatchPullRequest => Call::WatchPullRequest(numbered()?),
        Tool::UnwatchPullRequest => Call::UnwatchPullRequest(numbered()?),
        _ => unreachable!("not a PR tool"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pr_tool_takes_a_number() {
        assert_eq!(
            Tool::WatchPullRequest.parse(&json!({"number": 12})),
            Ok(Call::WatchPullRequest(Numbered { number: 12 }))
        );
        assert!(Tool::LinkPullRequest
            .parse(&json!({}))
            .expect_err("refused")
            .contains("`number`"));
        assert!(Tool::UnlinkPullRequest
            .parse(&json!({"number": 0}))
            .is_err());
        assert_eq!(
            Tool::ListPullRequests.parse(&Value::Null),
            Ok(Call::ListPullRequests)
        );
    }

    #[test]
    fn the_pr_tools_are_listed_for_agents_alone() {
        for tool in Tool::PULLS {
            assert_eq!(Tool::named(tool.name()), Some(tool));
            assert!(Tool::ALL.contains(&tool));
            assert!(!tool.for_key() && !tool.runs_agents());
        }
    }
}
