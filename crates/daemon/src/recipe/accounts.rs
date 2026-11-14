// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The per-account config folders. Each agent account gets its own Claude
//! folder for `CLAUDE_CONFIG_DIR` and its own Codex folder for
//! `CODEX_HOME`. The merges change only the keys the recipe owns and keep
//! every other key, and in TOML every comment, as they were.

use serde_json::Value as Json;
use thiserror::Error;
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value as Toml};

use super::{AGENT_HOME, WORK};

/// The key in `.claude.json` that records onboarding as done, so Claude
/// skips its first-run screens.
pub const CLAUDE_ONBOARDED: &str = "hasCompletedOnboarding";
/// What Codex's `trust_level` says for a trusted project.
pub const CODEX_TRUSTED: &str = "trusted";
/// Claude's global config in an account's folder. With `CLAUDE_CONFIG_DIR`
/// set, Claude keeps it in that folder, not in the home.
pub const CLAUDE_CONFIG: &str = ".claude.json";
/// Codex's config in an account's folder.
pub const CODEX_CONFIG: &str = "config.toml";

/// Why a config file couldn't be merged.
#[derive(Debug, Error)]
pub enum MergeError {
    /// The folder isn't one the recipe makes for an account.
    #[error("{folder:?} isn't a folder directly in /home/agent")]
    Folder {
        /// The folder given.
        folder: String,
    },
    /// The file doesn't parse.
    #[error("{path} isn't valid {format}: {reason}")]
    Parse {
        /// The file.
        path: String,
        /// What it should hold, such as JSON.
        format: &'static str,
        /// Where it stops parsing. It never quotes the file.
        reason: String,
    },
    /// A key the merge has to change holds something it can't change.
    #[error("{path} has {key} as something other than a table")]
    Shape {
        /// The file.
        path: String,
        /// The key, or the top level.
        key: String,
    },
}

/// The Claude folder for an account, which `CLAUDE_CONFIG_DIR` names.
pub fn claude_folder(account: &str) -> String {
    format!("{AGENT_HOME}/.claude-{account}")
}

/// The Codex folder for an account, which `CODEX_HOME` names.
pub fn codex_folder(account: &str) -> String {
    format!("{AGENT_HOME}/.codex-{account}")
}

/// Refuses a folder that isn't directly in the agent's home, so a merge
/// never writes anywhere else. Gives the folder's name in the home.
pub fn check_folder(folder: &str) -> Result<&str, MergeError> {
    let name = folder
        .strip_prefix(AGENT_HOME)
        .and_then(|rest| rest.strip_prefix('/'));
    match name {
        Some(name) if !name.is_empty() && !name.contains('/') && name != "." && name != ".." => {
            Ok(name)
        }
        _ => Err(MergeError::Folder {
            folder: folder.to_owned(),
        }),
    }
}

/// The path of [`CLAUDE_CONFIG`] in an account's folder.
pub fn claude_config_path(folder: &str) -> Result<String, MergeError> {
    check_folder(folder)?;
    Ok(format!("{folder}/{CLAUDE_CONFIG}"))
}

/// The path of [`CODEX_CONFIG`] in an account's folder.
pub fn codex_config_path(folder: &str) -> Result<String, MergeError> {
    check_folder(folder)?;
    Ok(format!("{folder}/{CODEX_CONFIG}"))
}

/// The text of `.claude.json` with onboarding done. `existing` is the file
/// as it is, or `None` when there's none. When onboarding is already done,
/// the text comes back unchanged. Otherwise other keys keep their values,
/// though the file is written again with its keys sorted.
pub fn claude_config(folder: &str, existing: Option<&str>) -> Result<String, MergeError> {
    let path = claude_config_path(folder)?;
    let mut config = match existing.filter(|text| !text.trim().is_empty()) {
        None => serde_json::Map::new(),
        Some(text) => match serde_json::from_str::<Json>(text) {
            Ok(Json::Object(config)) => {
                if config.get(CLAUDE_ONBOARDED) == Some(&Json::Bool(true)) {
                    return Ok(text.to_owned());
                }
                config
            }
            Ok(_) => {
                return Err(MergeError::Shape {
                    path,
                    key: "the top level".to_owned(),
                })
            }
            // serde_json's message gives the line and column, never the text.
            Err(err) => {
                return Err(MergeError::Parse {
                    path,
                    format: "JSON",
                    reason: err.to_string(),
                })
            }
        },
    };
    config.insert(CLAUDE_ONBOARDED.to_owned(), Json::Bool(true));
    let mut text = serde_json::to_string_pretty(&config).expect("a JSON object always serializes");
    text.push('\n');
    Ok(text)
}

/// The text of Codex's `config.toml` with `/home/agent/work` trusted.
/// `existing` is the file as it is, or `None` when there's none. When the
/// work folder is already trusted, the text comes back unchanged. Every
/// other key, comment and blank line stays as it was.
pub fn codex_config(folder: &str, existing: Option<&str>) -> Result<String, MergeError> {
    let path = codex_config_path(folder)?;
    let mut doc = match existing {
        None => DocumentMut::new(),
        Some(text) => text
            .parse::<DocumentMut>()
            .map_err(|err| MergeError::Parse {
                path: path.clone(),
                format: "TOML",
                // toml_edit's own message quotes the line, so only the
                // reason and the offset go out.
                reason: match err.span() {
                    Some(span) => format!("{} at byte {}", err.message().trim(), span.start),
                    None => err.message().trim().to_owned(),
                },
            })?,
    };
    let trusted = doc
        .get("projects")
        .and_then(|projects| projects.get(WORK))
        .and_then(|project| project.get("trust_level"))
        .and_then(Item::as_str);
    if let (Some(text), Some(CODEX_TRUSTED)) = (existing, trusted) {
        return Ok(text.to_owned());
    }
    let shape = |key: &str| MergeError::Shape {
        path: path.clone(),
        key: key.to_owned(),
    };
    let projects = doc.entry("projects").or_insert_with(|| {
        // Implicit, so only the `[projects."..."]` headers show.
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    let inline = projects.is_inline_table();
    let projects = projects
        .as_table_like_mut()
        .ok_or_else(|| shape("projects"))?;
    let project = projects.entry(WORK).or_insert_with(|| {
        if inline {
            Item::Value(Toml::InlineTable(InlineTable::new()))
        } else {
            Item::Table(Table::new())
        }
    });
    let project = project
        .as_table_like_mut()
        .ok_or_else(|| shape(&format!("projects.\"{WORK}\"")))?;
    match project.get_mut("trust_level").and_then(Item::as_value_mut) {
        // Keeps any comment that sits after the old value.
        Some(value) => {
            let decor = value.decor().clone();
            *value = Toml::from(CODEX_TRUSTED);
            *value.decor_mut() = decor;
        }
        None => {
            project.insert("trust_level", toml_edit::value(CODEX_TRUSTED));
        }
    }
    Ok(doc.to_string())
}
