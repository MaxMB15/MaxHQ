// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The run-state contract's two files in their reference shapes. A
//! workflow that writes its run file this way needs no mapping in its
//! descriptor. Both are generated into the checked-in schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

/// A run file in the reference shape. Any YAML or JSON file the
/// descriptor's `runs.glob` matches, replaced whole on each change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunFile {
    /// The run itself.
    pub run: RunInfo,
    /// Its nodes, in the order they're drawn.
    #[serde(default)]
    pub nodes: Vec<NodeEntry>,
}

/// What a run says about itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunInfo {
    /// The run's id, unique in the project.
    pub id: String,
    /// The run's status, in the workflow's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// What it's called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The agent leading it: the `MAXHQ_AGENT` it was started with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// One node of a run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeEntry {
    /// Its id, unique in the run.
    pub id: String,
    /// Its status, in the workflow's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The ids of the nodes it needs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// The group it sits in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// How many attempts it took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u64>,
    /// What it's called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The agent working on it, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Anything else, for the descriptor's `fields`.
    #[serde(flatten)]
    pub extra: Map<String, Json>,
}

/// One line of a run's event log, `{jsonl: <file>}`: UTF-8 JSON Lines,
/// appended to and never rewritten. `t` and `node` are required, plus
/// `status` or `note`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(extend("anyOf" = [{ "required": ["status"] }, { "required": ["note"] }]))]
pub struct EventLine {
    /// When it happened, RFC 3339, such as `2026-11-01T14:02:07.120Z`.
    #[schemars(extend("format" = "date-time"))]
    pub t: String,
    /// The node, by id.
    pub node: String,
    /// Its new status, in the workflow's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// A note on it, such as why a dispatch was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The attempt it belongs to. A busy event with the same attempt as the
    /// one before continues it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    /// The agent that made it, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Anything else, kept as is.
    #[serde(flatten)]
    pub extra: Map<String, Json>,
}
