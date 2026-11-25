// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Domain logic and storage shared by `maxd` and the desktop app, so both
//! apply the same rules.

#![warn(missing_docs)]

pub mod agents;
pub mod attached;
pub mod authorized_keys;
pub mod clone_progress;
pub mod context;
pub mod db;
pub mod dpop;
pub mod excerpt;
pub mod find;
pub mod fs;
pub mod fs_ext;
pub mod git;
pub mod git_items;
pub mod github_activity;
pub mod github_events;
pub mod handoff;
pub mod hooks;
pub mod host_spec;
pub mod ignored_files;
pub mod inbox;
pub mod leases;
pub mod mask;
pub mod names;
pub mod notes;
pub mod notify;
pub mod paths;
pub mod pill_text;
#[cfg(target_os = "macos")]
pub mod power;
pub mod prove;
pub mod pulls;
pub mod quote_span;
pub mod schedule;
pub mod service;
pub mod shell;
pub mod skills;
pub mod store;
#[cfg(test)]
mod test_dir;
pub mod timeline_find;
pub mod tools;
