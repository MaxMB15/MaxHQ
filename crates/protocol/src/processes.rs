// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The processes an agent started, for the Running list in the ports
//! popover. A client asks while the popover is open, and the agent's
//! device walks the process tree under the agent's holder, the same walk
//! that finds its ports. A container agent's guest walks its own tree.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

use crate::time::Timestamp;

/// One process in an agent's tree. The holder itself isn't one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AgentProcess {
    /// Its process ID, as its device or container sees it.
    pub pid: u32,
    /// Its parent's process ID. The CLI's is the holder's.
    pub parent: u32,
    /// When it started, in its device's own units. Only its device reads
    /// it: with `pid` it names this process to
    /// [`crate::ClientMessage::SignalProcess`], so a reused pid is never
    /// signalled.
    #[specta(type = specta_typescript::Number)]
    pub start: u64,
    /// How long it has run, in milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub run_ms: u64,
    /// The CPU time it has used, user and system, in milliseconds.
    #[specta(type = specta_typescript::Number)]
    pub cpu_ms: u64,
    /// Its resident memory, in bytes.
    #[specta(type = specta_typescript::Number)]
    pub memory: u64,
    /// Its process name.
    pub name: String,
    /// Its command line, cut to 1,024 bytes.
    pub command: String,
    /// Whether it's the agent's own CLI, which only stopping the agent
    /// stops.
    pub cli: bool,
    /// The ports it listens on, as port detection counts them, lowest
    /// first.
    pub ports: Vec<u16>,
}

/// What [`crate::ClientMessage::SignalProcess`] sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSignal {
    /// SIGTERM, which asks the process to stop.
    Term,
    /// SIGKILL, which ends it at once.
    Kill,
}

/// What [`crate::ClientMessage::ListProcesses`] found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProcessList {
    /// The processes under the agent's holder, oldest first. An agent
    /// with no holder running has none.
    Listed {
        /// Its processes.
        processes: Vec<AgentProcess>,
    },
    /// The device's `maxd`, or the guest in the agent's container, is too
    /// old to list them.
    TooOld,
}

/// What a lead agent started that still runs, and what it waits on, sent
/// on [`crate::model::Agent::running`] so a list shows it without asking.
/// Its `maxd` sends it again only when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
pub struct AgentRunning {
    /// How many jobs the agent started still run: its background shells
    /// and what its commands left behind, not its CLI or the CLI's own
    /// servers.
    pub count: u32,
    /// A short name for each of the oldest few, such as `pnpm dev`.
    pub names: Vec<String>,
    /// Set while the agent waits on something that wakes it, so it acts
    /// again by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<AgentWait>,
}

/// What an idle agent waits on, which wakes it by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentWait {
    /// A background command or monitor the CLI watches. The agent goes on
    /// when it ends.
    Task {
        /// What it runs, in a few words.
        name: String,
    },
    /// A wakeup the agent set for itself.
    Wakeup {
        /// When it's due.
        at: Timestamp,
    },
}
