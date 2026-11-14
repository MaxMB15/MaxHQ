// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Pull requests: where they're read from on GitHub, what MaxHQ keeps of
//! them, the app's requests to list, read, merge and approve them, and the
//! watcher that keeps each agent's links current. The session manager owns
//! the links themselves.

pub mod read;
pub mod requests;
pub mod source;
pub mod watcher;

/// The refusal for every PR request this device doesn't run yet.
pub const NOT_YET: &str = "PRs aren't set up on this device yet.";
