// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a holder needs from the OS besides its program's tree: leaving the
//! spawner behind, swapping its own stdio, and telling whether a path
//! still names a file it has open.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub(crate) use imp::{
    hide_window, in_spawner_job, leave_session, os_bytes, os_from_bytes, same_file, set_output,
    set_stdin, spawn_second, stdin_file, Quit, NULL,
};
