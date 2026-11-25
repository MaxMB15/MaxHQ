// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a holder needs from the OS besides its program's tree: leaving the
//! spawner behind and swapping its own stdio.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub(crate) use imp::{
    hide_window, in_spawner_job, leave_session, os_bytes, os_from_bytes, set_output, set_stdin,
    spawn_second, stdin_file, Quit, NULL,
};
