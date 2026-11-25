// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The program a holder runs, with everything it starts: an agent's CLI or
//! a terminal's shell.
//!
//! On Unix it's a process group the program leads, and the holder reaps
//! the program itself, so it signals the group only while the ID is still
//! the program's. On Windows it's a Job Object: the program starts
//! suspended, goes into a job of its own and only then runs, so nothing it
//! starts is outside the job. The job has no kill-on-close, so the tree
//! outlives the holder as a group outlives it on Unix.
//!
//! Windows has no SIGTERM or SIGHUP for a process without a console, so
//! there [`Tree::signal`] only kills, and says so for the others.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub(crate) use imp::{prepare, signal_group, Group, Tree};

/// How the program ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// What to send the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sig {
    /// The terminal went away, as SIGHUP says.
    Hangup,
    /// Asks it to end, as SIGTERM does.
    Term,
    /// Ends it at once, as SIGKILL does.
    Kill,
}
