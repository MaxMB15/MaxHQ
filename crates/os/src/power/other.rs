// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Other systems don't sleep under an agent, or have no hold to take.

use std::io;

use super::Keep;

pub enum Hold {}

pub const fn supports(_keep: Keep) -> bool {
    false
}

pub fn take(_keep: Keep, _reason: &str) -> io::Result<Hold> {
    Err(crate::unsupported("keeping the system awake"))
}

pub fn on_charger() -> io::Result<bool> {
    Err(crate::unsupported("the power status"))
}
