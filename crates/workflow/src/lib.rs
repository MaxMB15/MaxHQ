// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `maxhq.workflow` standard and the engine that follows it.
//!
//! A workflow ships a descriptor that says where its runs live and how to
//! read them. [`Catalog::scan`] finds a project's descriptors,
//! [`Scan::runs`] finds their runs, and [`Run::derive`] reads one run into
//! the [`RunSnapshot`](maxhq_protocol::workflow::RunSnapshot) the canvas
//! draws. [`Diff::diff`] turns two snapshots into a patch. The engine reads
//! only through [`Files`] and [`Agents`], never runs a workflow's scripts
//! and never writes into a run folder.

#![warn(missing_docs)]

mod bind;
mod catalog;
pub mod descriptor;
mod diff;
mod eval;
mod events;
pub mod fake;
mod inputs;
mod load;
pub mod reference;
mod run;
pub mod schema;

pub use bind::{lead_state, DEFAULT_LINE};
pub use catalog::{fallback_leads, Catalog, Place, Problem, RunRef, Scan, Workflow, WORKFLOWS_DIR};
pub use descriptor::{env_risk, env_shaped, Descriptor, AGENTS_ID};
pub use diff::Diff;
pub use events::Event;
pub use inputs::{AgentFacts, AgentState, Agents, Entry, Files, NoAgents, Stat};
pub use load::{load, LoadError, MAX_FILE};
pub use run::{summarize, Run};
