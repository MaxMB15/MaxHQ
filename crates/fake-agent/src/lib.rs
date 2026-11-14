// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A stand-in for an agent CLI, so tests and benchmarks get the same terminal
//! output every run without a vendor login.
//!
//! It replays asciinema recordings of real sessions with their original
//! timing, and it makes two synthetic loads: a `yes`-style flood and a 10 Hz
//! spinner that rewrites a whole line.

pub mod cast;
pub mod load;
pub mod playback;
