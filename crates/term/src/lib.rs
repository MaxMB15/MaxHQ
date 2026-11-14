// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Headless terminal emulation.
//!
//! `maxd-hold` feeds each session's output through an emulator, so a client
//! that attaches late gets the screen as it stands instead of a replay of
//! everything. The emulator sits behind one trait so libghostty-vt can be
//! swapped out (decision A-4). Empty until M1.
