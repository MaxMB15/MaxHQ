// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The driver for one held Claude Code session and its sub-agents.
//!
//! `launch` gives the command line for `claude -p` in stream-json mode,
//! with permission prompts sent to `maxd` on stdout. [`ClaudeSession`]
//! reads the process's lines into timeline items, agents and attention
//! events, and turns actions into lines for its stdin. [`History`] reads a
//! session's transcript into the same items, for a session MaxHQ didn't
//! start.

mod attach;
mod convo;
mod effort;
pub mod entrypoint;
mod history;
mod items;
mod launch;
mod limits;
pub mod own_mcp;
mod session;
mod tools;
mod wire;

#[cfg(test)]
mod tests;

use std::path::Path;

pub(crate) use wire::clip;

pub use history::{subagent_ids, History};
pub use launch::{add_effort, launch, login_launch, OAUTH_TOKEN, STRIPPED_PREFIXES};
pub use session::ClaudeSession;

use crate::observe::claude::process::Processes;
use crate::observe::claude::session as sessions;

/// Whether MaxHQ can take over `session`: true when no running Claude Code
/// process in `config_dir` has it open. Two processes writing one
/// transcript would corrupt it.
pub fn can_continue(config_dir: &Path, session: &str) -> bool {
    let open: Vec<sessions::Session> = sessions::list(&config_dir.join("sessions"))
        .into_iter()
        .filter(|open| open.session_id == session)
        .collect();
    if open.is_empty() {
        return true;
    }
    let pids: Vec<u32> = open.iter().map(|open| open.pid).collect();
    let processes = Processes::lookup(&pids);
    !open
        .iter()
        .any(|open| processes.runs(open.pid, open.proc_start.as_deref()))
}
