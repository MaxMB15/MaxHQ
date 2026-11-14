// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the composer's lookups find: files and folders for `@`, pull
//! requests and issues for `#`, and an agent's excerpt for an agent pill
//! whose agent MaxHQ's read tool can't reach.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::context::InlineRef;

/// The most matches one path search gives.
pub const MAX_PATH_MATCHES: usize = 50;

/// A file or folder in an agent's folder, for the `@` menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct PathMatch {
    /// The path relative to the agent's folder, with `/` between parts.
    pub path: String,
    /// Whether it's a file or a folder.
    pub kind: PathKind,
}

/// What a [`PathMatch`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    /// A file, or a link to one.
    File,
    /// A folder.
    Folder,
}

/// An agent's timeline as Markdown, cut to a budget, for an agent pill
/// that travels as a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AgentExcerpt {
    /// A file name for it, such as `agent-api-refactor.md`.
    pub name: String,
    /// The Markdown.
    pub text: String,
    /// How many messages it holds.
    pub messages: u32,
    /// How many messages it left out to fit the budget.
    pub omitted: u32,
}

/// What a `#` lookup found in the GitHub repo of an agent's folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct GitItemSearch {
    /// The repo it looked in, as "owner/name", or none when the folder has
    /// no GitHub remote.
    pub repo: Option<String>,
    /// What matched, best first. Each is an [`InlineRef::GitItem`].
    pub items: Vec<InlineRef>,
    /// Why nothing could be looked up, for a person to read, such as the
    /// GitHub App not being installed on the repo.
    pub unavailable: Option<String>,
}
