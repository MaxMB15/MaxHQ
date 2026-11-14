// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Usage modules read each vendor login's rate limits and keep the
//! account entities current. Cursor's module keeps the login, and its
//! month's meters once you turn Cursor usage on. Each one checks on a
//! timer and when a client sends `RefreshUsage`.

pub mod claude;
pub mod codex;
pub mod cursor;
