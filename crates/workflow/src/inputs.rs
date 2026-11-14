// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the engine reads: files through [`Files`] and the facts `maxd`
//! already keeps about agents through [`Agents`]. The engine never touches
//! the disk or a process itself, so the same code runs in `maxd`, in a guest
//! and under test fakes.

use std::io;
use std::path::Path;

use maxhq_protocol::ids::AgentId;
use maxhq_protocol::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A file's size and when it last changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    /// Its length in bytes.
    pub size: u64,
    /// When it last changed.
    pub modified: Timestamp,
}

/// One entry of a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name, with no folder.
    pub name: String,
    /// Whether it's a folder.
    pub dir: bool,
}

/// Read-only access to the files a workflow writes. Paths are absolute.
pub trait Files {
    /// The whole file if it's a regular one. A real disk stops one byte past
    /// [`MAX_FILE`](crate::MAX_FILE), which is enough to tell it's over.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// Up to `max` bytes from `offset` on. Fewer than `max` means the end.
    fn read_from(&self, path: &Path, offset: u64, max: usize) -> io::Result<Vec<u8>>;
    /// Its size and modification time.
    fn stat(&self, path: &Path) -> io::Result<Stat>;
    /// The entries of a folder, in any order.
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>>;
    /// Whether a process with this pid is running, next to the files.
    fn alive(&self, pid: u32) -> bool;
}

/// How an agent stands, as the hub sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// It's working on a turn.
    Busy,
    /// It waits for you, on a question or a permission.
    Waiting,
    /// It's running and has nothing to do.
    Idle,
    /// It ended, or its process is gone.
    Gone,
}

impl AgentState {
    /// Whether the agent still runs.
    pub fn alive(self) -> bool {
        self != Self::Gone
    }
}

/// What the hub knows about one agent. Cheap facts only; the prompt head
/// and commands come through [`Agents`] when a pattern needs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentFacts {
    /// The agent.
    pub id: AgentId,
    /// The agent that started it, for a sub-agent.
    pub lead: Option<AgentId>,
    /// Its title. For a Claude sub-agent, the task's description.
    pub title: Option<String>,
    /// Its process, when it has one of its own.
    pub pid: Option<u32>,
    /// How it stands.
    pub state: AgentState,
    /// When it started.
    pub started: Option<Timestamp>,
    /// When it last wrote anything.
    pub last_write: Option<Timestamp>,
    /// Whether a tool call of its own is open.
    pub tool_open: bool,
}

/// The agents in one project, as the hub keeps them.
pub trait Agents {
    /// The time now.
    fn now(&self) -> Timestamp;
    /// Every agent in the project, leads and sub-agents.
    fn list(&self) -> Vec<AgentFacts>;
    /// The first few kilobytes of the agent's first prompt.
    fn prompt_head(&self, agent: AgentId) -> Option<String>;
    /// Calls `visit` with the text of each finished command the agent ran.
    fn each_command(&self, agent: AgentId, visit: &mut dyn FnMut(&str));
    /// Whether these are the project's agents at all. A guest that leaves
    /// binding to the host has none, so no node goes stale for want of one.
    fn binds(&self) -> bool {
        true
    }
}

/// No agents at all, for a guest that follows files and leaves binding to
/// the host.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAgents;

impl Agents for NoAgents {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    fn list(&self) -> Vec<AgentFacts> {
        Vec::new()
    }

    fn prompt_head(&self, _: AgentId) -> Option<String> {
        None
    }

    fn each_command(&self, _: AgentId, _: &mut dyn FnMut(&str)) {}

    fn binds(&self) -> bool {
        false
    }
}
