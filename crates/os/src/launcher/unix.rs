// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! launchd and systemd run `maxd` on Unix, so there's nothing to tie and
//! no window to hide.

use std::io;

pub fn tie_children() -> io::Result<()> {
    Err(crate::unsupported("The maxd launcher"))
}

pub fn without_window(_command: &mut std::process::Command) {}
