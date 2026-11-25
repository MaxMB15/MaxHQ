// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Starting the CLI as another user, for a holder that runs as root. Only
//! Unix can: a Windows holder would need the user's password or a token
//! from a service, and runs everything as the user it is.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub(crate) use imp::{lookup, prepare, Prepared};
